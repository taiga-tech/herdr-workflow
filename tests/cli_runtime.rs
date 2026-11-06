//! 実行ファイルとCoordinatorを使う非対話実行の契約試験(Herdrは使わない)。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Env {
    root: PathBuf,
    state: PathBuf,
}

impl Env {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        // ソケットパスの長さ上限に収まる短いパスを使う。
        let root = PathBuf::from(format!(
            "/tmp/hwc-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("work")).unwrap();
        // attachの対象はGitの作業ツリーでなければならない。
        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(root.join("work"))
            .status()
            .unwrap();
        assert!(status.success());
        Self {
            state: root.join("s"),
            root,
        }
    }

    fn write_workflow(&self, tasks: &str, bootstrap: &str) -> PathBuf {
        let path = self.root.join("wf.yaml");
        std::fs::write(
            &path,
            format!(
                "version: 1\nname: t\nworktree:\n  mode: attach\nbootstrap:\n  targets: [{bootstrap}]\ntasks:\n{tasks}\nworkspace:\n  label: t\n  tabs:\n    - id: main\n      label: main\n      layout: {{ type: grid, columns: 1, rows: 1 }}\n      panes:\n        - id: s\n          label: status\n          view: {{ type: status }}\n          placement: {{ column: 1, row: 1 }}\n"
            ),
        )
        .unwrap();
        path
    }

    fn cli(&self, args: &[&str]) -> Output {
        self.cli_with(args, &[])
    }

    fn cli_with(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_herdr-workflow"))
            .args(args)
            .envs(envs.iter().copied())
            .env("HERDR_WORKFLOW_STATE_DIR", &self.state)
            .env("HERDR_WORKFLOW_BIN", env!("CARGO_BIN_EXE_herdr-workflow"))
            .env_remove("HERDR_PLUGIN_CONFIG_DIR")
            .current_dir(self.root.join("work"))
            .output()
            .unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // Coordinatorへ終了を求め、状態ディレクトリを片付ける。
        if let Ok(mut stream) =
            std::os::unix::net::UnixStream::connect(self.state.join("coordinator.sock"))
        {
            use std::io::Write;
            let _ = stream.write_all(b"{\"v\":1,\"id\":\"x\",\"method\":\"shutdown\"}\n");
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn job(name: &str, script: &str, extra: &str) -> String {
    format!(
        "  {name}:\n    type: command\n    lifecycle: job\n    runner: supervised\n    argv: [/bin/sh, -c, '{script}']\n{extra}"
    )
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn run_target_reports_success_and_logs() {
    let env = Env::new();
    let config = env.write_workflow(
        &[
            job("setup", "echo preparing", ""),
            job(
                "tests",
                "echo e2e passed",
                "    dependsOn:\n      - task: setup\n        condition: succeeded\n",
            ),
        ]
        .concat(),
        "setup",
    );
    let output = env.cli(&[
        "run",
        "--config",
        path(&config),
        "--target",
        "tests",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["run"]["outcome"], "succeeded");
    let run_id = report["run"]["runId"].as_str().unwrap().to_string();

    let logs = env.cli(&["logs", &run_id, "--task", "tests"]);
    assert_eq!(String::from_utf8_lossy(&logs.stdout), "e2e passed\n");

    let status = env.cli(&["status", &run_id, "--json"]);
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["tasks"]["setup"]["runtime"]["state"], "succeeded");
}

#[test]
fn failing_target_exits_with_run_failure_code() {
    let env = Env::new();
    let config = env.write_workflow(&job("tests", "exit 3", ""), "");
    let output = env.cli(&["run", "--config", path(&config), "--target", "tests"]);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
}

#[test]
fn invalid_config_is_rejected_before_any_process_starts() {
    let env = Env::new();
    let config = env.write_workflow(
        &job("tests", "touch ran", "    env:\n      HERDR_SECRET: x\n"),
        "",
    );
    let output = env.cli(&["run", "--config", path(&config), "--target", "tests"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(!env.root.join("work/ran").exists());
}

#[test]
fn up_then_stop_leaves_run_stopped() {
    let env = Env::new();
    let config = env.write_workflow(
        "  server:\n    type: command\n    lifecycle: service\n    runner: supervised\n    argv: [/bin/sh, -c, 'echo ready; exec sleep 30']\n    waitFor:\n      type: log\n      pattern: ready\n      timeoutSeconds: 10\n",
        "",
    );
    let up = env.cli(&["up", "--config", path(&config)]);
    assert_eq!(up.status.code(), Some(0), "{up:?}");
    let run_id = String::from_utf8_lossy(&up.stdout).trim().to_string();
    let wait = env.cli(&["wait", &run_id, "--until", "ready", "--timeout", "20"]);
    assert_eq!(wait.status.code(), Some(0), "{wait:?}");
    let stop = env.cli(&["stop", &run_id]);
    assert_eq!(stop.status.code(), Some(0), "{stop:?}");
    let finished = env.cli(&[
        "wait",
        &run_id,
        "--until",
        "finished",
        "--timeout",
        "20",
        "--json",
    ]);
    let report: serde_json::Value = serde_json::from_slice(&finished.stdout).unwrap();
    assert_eq!(report["run"]["phase"], "stopped");
    assert_eq!(report["tasks"]["server"]["runtime"]["state"], "stopped");
}

#[test]
fn retry_runs_a_new_attempt_without_repeating_succeeded_tasks() {
    let env = Env::new();
    let config = env.write_workflow(
        &[
            job("setup", "echo x >> setup-count", ""),
            job(
                "flaky",
                "if [ -f ok ]; then exit 0; fi; exit 1",
                "    dependsOn:\n      - task: setup\n        condition: succeeded\n",
            ),
        ]
        .concat(),
        "setup",
    );
    let first = env.cli(&[
        "run",
        "--config",
        path(&config),
        "--target",
        "flaky",
        "--json",
    ]);
    assert_eq!(first.status.code(), Some(4));
    let report: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let run_id = report["run"]["runId"].as_str().unwrap().to_string();
    std::fs::write(env.root.join("work/ok"), "").unwrap();
    let retry = env.cli(&["retry", &run_id, "--task", "flaky", "--json"]);
    assert_eq!(retry.status.code(), Some(0), "{retry:?}");
    let report: serde_json::Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert_eq!(
        report["tasks"]["flaky"]["runtime"]["attempts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let count = std::fs::read_to_string(env.root.join("work/setup-count")).unwrap();
    assert_eq!(count, "x\n");
}

#[test]
fn retry_after_lockfile_change_needs_confirmation_and_reruns_setup() {
    // Q15: 端末のない再開は変更を示して終了コード3で止まり、--yesで成功済みの初期化から実行し直す。
    let env = Env::new();
    std::fs::write(env.root.join("work/pnpm-lock.yaml"), "v1").unwrap();
    let config = env.write_workflow(
        &[
            job("setup", "echo x >> setup-count", ""),
            job(
                "flaky",
                "if [ -f ok ]; then exit 0; fi; exit 1",
                "    dependsOn:\n      - task: setup\n        condition: succeeded\n",
            ),
        ]
        .concat(),
        "setup",
    );
    let first = env.cli(&[
        "run",
        "--config",
        path(&config),
        "--target",
        "flaky",
        "--json",
    ]);
    assert_eq!(first.status.code(), Some(4));
    let report: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let run_id = report["run"]["runId"].as_str().unwrap().to_string();
    std::fs::write(env.root.join("work/ok"), "").unwrap();
    std::fs::write(env.root.join("work/pnpm-lock.yaml"), "v2").unwrap();

    let held = env.cli(&["retry", &run_id, "--task", "flaky"]);
    assert_eq!(held.status.code(), Some(3), "{held:?}");
    let stderr = String::from_utf8_lossy(&held.stderr);
    assert!(stderr.contains("pnpm-lock.yaml changed"), "{stderr}");
    assert!(stderr.contains("--yes"), "{stderr}");
    let count = || std::fs::read_to_string(env.root.join("work/setup-count")).unwrap();
    assert_eq!(count(), "x\n");

    let retry = env.cli(&["retry", &run_id, "--task", "flaky", "--yes", "--json"]);
    assert_eq!(retry.status.code(), Some(0), "{retry:?}");
    assert_eq!(count(), "x\nx\n");
}

#[test]
fn attach_rejects_branch_mismatch_before_starting() {
    // T37: ブランチ不一致ではRunを開始しない。
    let env = Env::new();
    let config = env.write_workflow(&job("tests", "touch ran", ""), "");
    let text = std::fs::read_to_string(&config)
        .unwrap()
        .replace("mode: attach\n", "mode: attach\n  branch: feature/other\n");
    std::fs::write(&config, text).unwrap();
    let work = env.root.join("work");
    let output = env.cli(&[
        "attach",
        "--config",
        path(&config),
        "--checkout",
        path(&work),
    ]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("expected \"feature/other\""));
    assert!(!work.join("ran").exists());
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn worktree_event_requires_approval_and_is_accepted_once() {
    // T24・T25・T11: 未承認では実行せず、設定変更後は古い承認を使わず、
    // 同じイベントの重複では初期化を繰り返さない。
    let env = Env::new();
    let repo = env.root.join("work");
    std::fs::create_dir_all(repo.join(".herdr")).unwrap();
    let workflow = |marker: &str| {
        format!(
            "version: 1\nname: t\ninputs:\n  branch:\n    type: string\n    required: true\nworktree:\n  mode: create\n  branch: '${{inputs.branch}}'\nbootstrap:\n  targets: [init]\ntasks:\n  init:\n    type: command\n    lifecycle: job\n    runner: supervised\n    argv: [/bin/sh, -c, 'echo \"$BRANCH\" >> {marker}']\n    env:\n      BRANCH: '${{inputs.branch}}'\nworkspace:\n  label: t\n  tabs:\n    - id: main\n      label: main\n      layout: {{ type: grid, columns: 1, rows: 1 }}\n      panes:\n        - id: s\n          label: status\n          view: {{ type: status }}\n          placement: {{ column: 1, row: 1 }}\n"
        )
    };
    std::fs::write(repo.join(".herdr/workflow.yaml"), workflow("init-ran")).unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "feature/x", "../wt"],
    );
    let wt = std::fs::canonicalize(env.root.join("wt")).unwrap();
    let payload = format!(
        "{{\"type\":\"worktree_created\",\"workspace\":{{\"workspace_id\":\"w2\"}},\"worktree\":{{\"path\":\"{}\",\"branch\":\"feature/x\",\"is_bare\":false,\"is_detached\":false,\"is_prunable\":false,\"is_linked_worktree\":true,\"label\":\"x\"}}}}",
        wt.display()
    );
    let hook = |env: &Env| {
        env.cli_with(
            &["hook"],
            &[
                ("HERDR_PLUGIN_EVENT", "worktree.created"),
                ("HERDR_PLUGIN_EVENT_JSON", &payload),
            ],
        )
    };

    let held = hook(&env);
    assert_eq!(held.status.code(), Some(3), "{held:?}");
    assert!(!wt.join("init-ran").exists());

    let config = repo.join(".herdr/workflow.yaml");
    let trust = env.cli(&["trust", "--config", path(&config)]);
    assert_eq!(trust.status.code(), Some(0), "{trust:?}");

    let accepted = hook(&env);
    assert_eq!(accepted.status.code(), Some(0), "{accepted:?}");
    let run_id = String::from_utf8_lossy(&accepted.stdout).trim().to_string();
    let wait = env.cli(&["wait", &run_id, "--until", "finished", "--timeout", "20"]);
    assert_eq!(wait.status.code(), Some(0), "{wait:?}");
    assert_eq!(
        std::fs::read_to_string(wt.join("init-ran")).unwrap(),
        "feature/x\n"
    );

    let duplicate = hook(&env);
    assert_eq!(duplicate.status.code(), Some(0), "{duplicate:?}");
    assert!(String::from_utf8_lossy(&duplicate.stdout).contains("already accepted"));
    assert_eq!(
        std::fs::read_to_string(wt.join("init-ran")).unwrap(),
        "feature/x\n"
    );

    // 設定を変更したら、古い承認では実行しない。
    std::fs::write(&config, workflow("init-ran-2")).unwrap();
    let changed = hook(&env);
    assert_eq!(changed.status.code(), Some(3), "{changed:?}");
    assert!(!wt.join("init-ran-2").exists());

    // 承認後にGitのhookが増えたら、同じ設定でも再承認を求める(Q13)。
    let trust = env.cli(&["trust", "--config", path(&config)]);
    assert_eq!(trust.status.code(), Some(0), "{trust:?}");
    let git_hook = repo.join(".git/hooks/post-checkout");
    std::fs::write(&git_hook, "#!/bin/sh\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&git_hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let hooks_changed = hook(&env);
    assert_eq!(hooks_changed.status.code(), Some(3), "{hooks_changed:?}");
    assert!(String::from_utf8_lossy(&hooks_changed.stdout).contains("hooks or filters changed"));
    assert!(!wt.join("init-ran-2").exists());
    let trust = env.cli(&["trust", "--config", path(&config)]);
    assert_eq!(trust.status.code(), Some(0), "{trust:?}");
    assert!(String::from_utf8_lossy(&trust.stdout).contains("git hook: post-checkout"));
    let accepted = hook(&env);
    assert_eq!(accepted.status.code(), Some(0), "{accepted:?}");
    let run_id = String::from_utf8_lossy(&accepted.stdout).trim().to_string();
    let wait = env.cli(&["wait", &run_id, "--until", "finished", "--timeout", "20"]);
    assert_eq!(wait.status.code(), Some(0), "{wait:?}");
    assert!(wt.join("init-ran-2").exists());
}

#[test]
fn remove_never_deletes_a_borrowed_checkout() {
    // T27: 確認できない削除は行わない。
    let env = Env::new();
    let config = env.write_workflow(&job("tests", "echo hi > untracked.txt", ""), "");
    let run = env.cli(&[
        "run",
        "--config",
        path(&config),
        "--target",
        "tests",
        "--json",
    ]);
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    let run_id = report["run"]["runId"].as_str().unwrap().to_string();
    let plan = env.cli(&["cleanup", "plan", &run_id, "--json"]);
    let plan: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert!(
        plan["candidate"]["untracked"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "untracked.txt")
    );
    let remove = env.cli(&["remove", &run_id]);
    assert_eq!(remove.status.code(), Some(4), "{remove:?}");
    assert!(env.root.join("work/untracked.txt").exists());
}

#[test]
fn logs_follow_spans_retried_attempts_and_keep_waits_for_more() {
    let env = Env::new();
    let config = env.write_workflow(
        &job(
            "flaky",
            "if [ -f once ]; then echo second; exit 0; fi; touch once; echo first; exit 1",
            "    retry:\n      maxAttempts: 2\n",
        ),
        "",
    );
    let run = env.cli(&[
        "run",
        "--config",
        path(&config),
        "--target",
        "flaky",
        "--json",
    ]);
    assert_eq!(run.status.code(), Some(0), "{run:?}");
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    let run_id = report["run"]["runId"].as_str().unwrap().to_string();

    // 追従はTaskの結果が確定するまで続き、Attemptの区切りを表示して終える。
    let follow = env.cli(&["logs", &run_id, "--task", "flaky", "--follow"]);
    assert_eq!(follow.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&follow.stdout),
        "first\n--- flaky attempt 2 ---\nsecond\n"
    );

    // `--keep`はRunの終了後もresumeを待って続く(paneで使う)。
    let mut keep = Command::new(env!("CARGO_BIN_EXE_herdr-workflow"))
        .args(["logs", &run_id, "--task", "flaky", "--follow", "--keep"])
        .env("HERDR_WORKFLOW_STATE_DIR", &env.state)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(800));
    assert!(keep.try_wait().unwrap().is_none(), "--keep must not exit");
    keep.kill().unwrap();
    keep.wait().unwrap();
}

#[test]
fn coordinator_restart_keeps_running_task_and_imports_its_result() {
    // T15: Coordinatorが異常終了しても、TaskRunnerは子プロセスを止めずに結果を確定する。
    // 再起動したCoordinatorは稼働中のjobを重複起動せず、確定した結果を取り込む。
    let env = Env::new();
    let config = env.write_workflow(
        &job(
            "slow",
            "echo run >> count; sleep 2; echo done > finished",
            "",
        ),
        "",
    );
    let up = env.cli(&["up", "--config", path(&config)]);
    assert_eq!(up.status.code(), Some(0), "{up:?}");
    let run_id = String::from_utf8_lossy(&up.stdout).trim().to_string();
    let status = |env: &Env| -> serde_json::Value {
        serde_json::from_slice(&env.cli(&["status", &run_id, "--json"]).stdout).unwrap()
    };
    for _ in 0..100 {
        if status(&env)["tasks"]["slow"]["runtime"]["state"] == "running" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let doctor: serde_json::Value =
        serde_json::from_slice(&env.cli(&["doctor", "--json"]).stdout).unwrap();
    let detail = doctor["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "coordinator")
        .unwrap()["detail"]
        .as_str()
        .unwrap()
        .to_string();
    let pid: i32 = detail
        .trim_end_matches(')')
        .rsplit(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    // SAFETY: この試験が起動したCoordinatorを強制終了する。
    assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);

    let mut coordinator = Command::new(env!("CARGO_BIN_EXE_herdr-workflow"))
        .args(["coordinator", "--idle-exit", "10"])
        .env("HERDR_WORKFLOW_STATE_DIR", &env.state)
        .env("HERDR_WORKFLOW_BIN", env!("CARGO_BIN_EXE_herdr-workflow"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut imported = serde_json::Value::Null;
    for _ in 0..200 {
        imported = status(&env);
        if imported["tasks"]["slow"]["runtime"]["state"] == "succeeded" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(
        imported["tasks"]["slow"]["runtime"]["state"], "succeeded",
        "{imported}"
    );
    assert_eq!(imported["run"]["outcome"], "succeeded");
    assert!(env.root.join("work/finished").exists());
    // 再起動で同じjobを二度起動していない。
    assert_eq!(
        std::fs::read_to_string(env.root.join("work/count")).unwrap(),
        "run\n"
    );
    coordinator.kill().unwrap();
    coordinator.wait().unwrap();
}

#[test]
fn removed_worktree_event_stops_runs_using_that_checkout() {
    // T28: 外部からworktreeが消えたら、そのcheckoutで稼働中のRunを止めて状態を更新する。
    let env = Env::new();
    let config = env.write_workflow(
        "  server:\n    type: command\n    lifecycle: service\n    runner: supervised\n    argv: [/bin/sh, -c, 'echo ready; exec sleep 30']\n    waitFor:\n      type: log\n      pattern: ready\n      timeoutSeconds: 10\n",
        "",
    );
    let up = env.cli(&["up", "--config", path(&config)]);
    let run_id = String::from_utf8_lossy(&up.stdout).trim().to_string();
    let wait = env.cli(&["wait", &run_id, "--until", "ready", "--timeout", "20"]);
    assert_eq!(wait.status.code(), Some(0), "{wait:?}");
    let work = std::fs::canonicalize(env.root.join("work")).unwrap();
    let payload = format!(
        "{{\"type\":\"worktree_removed\",\"workspace_id\":\"w2\",\"forced\":false,\"worktree\":{{\"path\":\"{}\",\"branch\":\"main\",\"is_bare\":false,\"is_detached\":false,\"is_prunable\":false,\"is_linked_worktree\":true,\"label\":\"x\"}}}}",
        work.display()
    );
    let hook = env.cli_with(
        &["hook"],
        &[
            ("HERDR_PLUGIN_EVENT", "worktree.removed"),
            ("HERDR_PLUGIN_EVENT_JSON", &payload),
        ],
    );
    assert_eq!(hook.status.code(), Some(0), "{hook:?}");
    let finished = env.cli(&[
        "wait",
        &run_id,
        "--until",
        "finished",
        "--timeout",
        "20",
        "--json",
    ]);
    let report: serde_json::Value = serde_json::from_slice(&finished.stdout).unwrap();
    assert_eq!(report["run"]["phase"], "stopped");
    assert_eq!(report["tasks"]["server"]["runtime"]["state"], "stopped");
}
