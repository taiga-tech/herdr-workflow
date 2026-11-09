//! 実際の子プロセス(`/bin/sh`)を使うスケジューラの試験。Herdrは使わない。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use super::*;
use crate::config::load;
use crate::config::model::InputId;
use crate::config::plugin::PluginConfig;
use crate::runtime::session::{self, RunRequest, SecretsLoader};
use crate::runtime::state_dir::test_support::TempDir;

struct Fixture {
    tmp: TempDir,
    state: StateDir,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new("engine");
        let state = StateDir::open(tmp.0.join("state")).unwrap();
        std::fs::create_dir_all(tmp.0.join("work")).unwrap();
        Self { tmp, state }
    }

    fn work(&self) -> std::path::PathBuf {
        self.tmp.0.join("work")
    }
}

/// tasks部分のYAMLから最小のworkflow設定を作る。
fn workflow(bootstrap: &str, tasks: &str) -> String {
    format!(
        "version: 1\nname: test\nworktree:\n  mode: create\n  branch: test\nbootstrap:\n  targets: [{bootstrap}]\ntasks:\n{tasks}\nworkspace:\n  label: test\n  tabs:\n    - id: main\n      label: main\n      layout: {{ type: grid, columns: 1, rows: 1 }}\n      panes:\n        - id: s\n          label: status\n          view: {{ type: status }}\n          placement: {{ column: 1, row: 1 }}\n"
    )
}

fn job(name: &str, script: &str, extra: &str) -> String {
    format!(
        "  {name}:\n    type: command\n    lifecycle: job\n    runner: supervised\n    script:\n      shell: [/bin/sh, -c]\n      source: '{script}'\n{extra}"
    )
}

fn service(name: &str, script: &str, extra: &str) -> String {
    format!(
        "  {name}:\n    type: command\n    lifecycle: service\n    runner: supervised\n    script:\n      shell: [/bin/sh, -c]\n      source: '{script}'\n{extra}"
    )
}

fn no_secrets() -> SecretsLoader {
    Arc::new(|| Ok(SecretsFile::default()))
}

fn run_with(
    fixture: &Fixture,
    yaml: &str,
    target: Option<&str>,
    loader: SecretsLoader,
    control: Option<mpsc::Receiver<Control>>,
) -> (RunReport, MemoryRecorder) {
    let spec = load::load_str(yaml).unwrap_or_else(|error| panic!("{error}\n{yaml}"));
    let inputs = BTreeMap::<InputId, String>::new();
    let mut env: EnvMap = std::env::vars_os().collect();
    env.insert(OsString::from("WORK"), fixture.work().into_os_string());
    let prepared = session::prepare_run(
        RunRequest {
            spec: &spec,
            plugin_config: &PluginConfig::default(),
            inputs: &inputs,
            checkout: &fixture.work(),
            target: target.map(TaskId::from),
            base_env: env,
            load_secrets: loader,
            config_dir: None,
            bootstrap_only: false,
            herdr: false,
        },
        session::new_run_id(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let (_tx, rx) = mpsc::channel();
    let control = control.unwrap_or(rx);
    let mut recorder = MemoryRecorder::default();
    let report = Engine::new(
        prepared.config,
        fixture.state.clone(),
        &mut recorder,
        control,
    )
    .run();
    (report, recorder)
}

fn run(fixture: &Fixture, yaml: &str, target: Option<&str>) -> (RunReport, MemoryRecorder) {
    run_with(fixture, yaml, target, no_secrets(), None)
}

fn state(report: &RunReport, task: &str) -> TaskState {
    report.snapshot.tasks[&TaskId::from(task)].runtime.state
}

fn attempts(report: &RunReport, task: &str) -> Vec<AttemptOutcome> {
    report.snapshot.tasks[&TaskId::from(task)]
        .runtime
        .attempts
        .iter()
        .map(|attempt| attempt.outcome.clone())
        .collect()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn job_chain_succeeds_in_dependency_order() {
    let fixture = Fixture::new();
    let yaml = workflow(
        "a",
        &[
            job("a", "echo a >> \"$WORK/order\"", ""),
            job(
                "b",
                "echo b >> \"$WORK/order\"",
                "    dependsOn:\n      - task: a\n        condition: succeeded\n",
            ),
        ]
        .concat(),
    );
    let (report, recorder) = run(&fixture, &yaml, Some("b"));
    assert_eq!(
        report.snapshot.outcome,
        RunOutcome::Succeeded,
        "{:?}",
        recorder.notices
    );
    assert_eq!(report.snapshot.phase, RunPhase::Finished);
    assert_eq!(state(&report, "a"), TaskState::Succeeded);
    assert_eq!(state(&report, "b"), TaskState::Succeeded);
    let order = std::fs::read_to_string(fixture.work().join("order")).unwrap();
    assert_eq!(order, "a\nb\n");
}

#[test]
fn bootstrap_failure_skips_everything_else() {
    // T04: 初期化が失敗したら後続タスクを開始しない。
    let fixture = Fixture::new();
    let yaml = workflow(
        "init",
        &[
            job("init", "exit 1", ""),
            job("other", "touch \"$WORK/other\"", ""),
        ]
        .concat(),
    );
    let (report, _) = run(&fixture, &yaml, Some("other"));
    assert_eq!(report.snapshot.outcome, RunOutcome::Failed);
    assert_eq!(state(&report, "init"), TaskState::Failed);
    assert_eq!(state(&report, "other"), TaskState::Skipped);
    assert!(!fixture.work().join("other").exists());
}

#[test]
fn dependent_job_waits_for_service_readiness() {
    // T05: readinessが遅れてもE2Eは準備完了まで開始しない。
    let fixture = Fixture::new();
    let yaml = workflow(
        "",
        &[
            service(
                "server",
                "sleep 0.5; touch \"$WORK/ready\"; echo listening on port; exec sleep 30",
                "    waitFor:\n      type: log\n      pattern: 'listening on'\n      timeoutSeconds: 10\n",
            ),
            job(
                "tests",
                "test -f \"$WORK/ready\"",
                "    dependsOn:\n      - task: server\n        condition: ready\n",
            ),
        ]
        .concat(),
    );
    let (report, recorder) = run(&fixture, &yaml, Some("tests"));
    assert_eq!(
        report.snapshot.outcome,
        RunOutcome::Succeeded,
        "{:?}",
        recorder.notices
    );
    assert_eq!(state(&report, "tests"), TaskState::Succeeded);
    // 対象の結果が出たらserviceを停止して終える。
    assert_eq!(state(&report, "server"), TaskState::Stopped);
}

#[test]
fn service_exit_before_readiness_fails_and_skips_dependents() {
    // T06: readiness前にプロセスが終了したら待機を終えて失敗を記録する。
    let fixture = Fixture::new();
    let port = free_port();
    let yaml = workflow(
        "",
        &[
            service(
                "server",
                "exit 0",
                &format!(
                    "    waitFor:\n      type: tcp\n      port: {port}\n      timeoutSeconds: 30\n      intervalMilliseconds: 100\n      consecutiveSuccesses: 1\n"
                ),
            ),
            job(
                "tests",
                "true",
                "    dependsOn:\n      - task: server\n        condition: ready\n",
            ),
        ]
        .concat(),
    );
    let started = Instant::now();
    let (report, _) = run(&fixture, &yaml, Some("tests"));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(report.snapshot.outcome, RunOutcome::Failed);
    assert_eq!(state(&report, "server"), TaskState::Failed);
    assert_eq!(
        attempts(&report, "server"),
        [AttemptOutcome::UnexpectedExit]
    );
    assert_eq!(state(&report, "tests"), TaskState::Skipped);
}

#[test]
fn port_in_use_is_not_mistaken_for_readiness() {
    // T07: 別プロセスがポートを使用中なら起動しない。
    let fixture = Fixture::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let yaml = workflow(
        "",
        &service(
            "server",
            "touch \"$WORK/started\"; exec sleep 30",
            &format!(
                "    waitFor:\n      type: tcp\n      port: {port}\n      timeoutSeconds: 5\n      intervalMilliseconds: 100\n      consecutiveSuccesses: 1\n"
            ),
        ),
    );
    let (report, _) = run(&fixture, &yaml, Some("server"));
    assert_eq!(report.snapshot.outcome, RunOutcome::Failed);
    assert!(matches!(
        attempts(&report, "server").as_slice(),
        [AttemptOutcome::LaunchFailed(reason)] if reason.contains("already in use")
    ));
    assert!(!fixture.work().join("started").exists());
    drop(listener);
}

#[test]
fn tcp_readiness_requires_consecutive_successes() {
    let fixture = Fixture::new();
    let port = free_port();
    let yaml = workflow(
        "",
        &service(
            "server",
            &format!(
                "exec python3 -c \"import socket,time;s=socket.socket();s.bind((\\\"127.0.0.1\\\",{port}));s.listen();time.sleep(30)\""
            ),
            &format!(
                "    waitFor:\n      type: tcp\n      port: {port}\n      timeoutSeconds: 10\n      intervalMilliseconds: 100\n      consecutiveSuccesses: 2\n"
            ),
        ),
    );
    let (report, recorder) = run(&fixture, &yaml, Some("server"));
    assert_eq!(
        report.snapshot.outcome,
        RunOutcome::Succeeded,
        "{:?}",
        recorder.notices
    );
    assert_eq!(state(&report, "server"), TaskState::Stopped);
}

#[test]
fn failed_job_is_retried_as_a_new_attempt() {
    // T34: 終了失敗は回数内で再試行する。
    let fixture = Fixture::new();
    let yaml = workflow(
        "",
        &job(
            "flaky",
            "if [ -f \"$WORK/once\" ]; then exit 0; fi; touch \"$WORK/once\"; exit 7",
            "    retry:\n      maxAttempts: 3\n",
        ),
    );
    let (report, _) = run(&fixture, &yaml, Some("flaky"));
    assert_eq!(report.snapshot.outcome, RunOutcome::Succeeded);
    assert_eq!(
        attempts(&report, "flaky"),
        [AttemptOutcome::ExitCode(7), AttemptOutcome::ExitCode(0)]
    );
}

#[test]
fn launch_failure_is_not_retried() {
    // T34: 起動失敗は再試行しない。
    let fixture = Fixture::new();
    let yaml = workflow(
        "",
        "  broken:\n    type: command\n    lifecycle: job\n    runner: supervised\n    argv: [/nonexistent/program]\n    retry:\n      maxAttempts: 3\n",
    );
    let (report, _) = run(&fixture, &yaml, Some("broken"));
    assert_eq!(report.snapshot.outcome, RunOutcome::Failed);
    assert!(matches!(
        attempts(&report, "broken").as_slice(),
        [AttemptOutcome::LaunchFailed(_)]
    ));
}

#[test]
fn timed_out_job_is_stopped_and_retried() {
    // T34: 期限切れは再試行の対象。
    let fixture = Fixture::new();
    let yaml = workflow(
        "",
        &job(
            "slow",
            "exec sleep 30",
            "    timeoutSeconds: 1\n    stop:\n      graceSeconds: 1\n    retry:\n      maxAttempts: 2\n",
        ),
    );
    let started = Instant::now();
    let (report, _) = run(&fixture, &yaml, Some("slow"));
    assert!(started.elapsed() < Duration::from_secs(15));
    assert_eq!(report.snapshot.outcome, RunOutcome::Failed);
    assert_eq!(
        attempts(&report, "slow"),
        [AttemptOutcome::TimedOut, AttemptOutcome::TimedOut]
    );
}

#[test]
fn dependency_lost_stops_running_dependents() {
    // T08: serviceがready後に終了したら依存側の停止規則を適用する。
    let fixture = Fixture::new();
    let yaml = workflow(
        "",
        &[
            service(
                "server",
                "echo up; sleep 1; exit 0",
                "    waitFor:\n      type: log\n      pattern: up\n      timeoutSeconds: 10\n",
            ),
            job(
                "tests",
                "exec sleep 30",
                "    dependsOn:\n      - task: server\n        condition: ready\n    onDependencyLost: stop\n    stop:\n      graceSeconds: 1\n",
            ),
        ]
        .concat(),
    );
    let started = Instant::now();
    let (report, _) = run(&fixture, &yaml, Some("tests"));
    assert!(started.elapsed() < Duration::from_secs(15));
    assert_eq!(report.snapshot.outcome, RunOutcome::Failed);
    assert_eq!(state(&report, "server"), TaskState::Failed);
    assert_eq!(state(&report, "tests"), TaskState::Cancelled);
}

#[test]
fn stop_request_stops_services_in_reverse_order() {
    let fixture = Fixture::new();
    let yaml = workflow(
        "",
        &[
            service(
                "db",
                "echo ready; trap \"echo db >> $WORK/stops; exit 0\" TERM; while true; do sleep 0.1; done",
                "    waitFor:\n      type: log\n      pattern: ready\n      timeoutSeconds: 10\n",
            ),
            service(
                "api",
                "echo ready; trap \"echo api >> $WORK/stops; exit 0\" TERM; while true; do sleep 0.1; done",
                "    dependsOn:\n      - task: db\n        condition: ready\n    waitFor:\n      type: log\n      pattern: ready\n      timeoutSeconds: 10\n",
            ),
        ]
        .concat(),
    );
    let (tx, rx) = mpsc::channel();
    let stopper = std::thread::spawn(move || {
        // 両方がreadyになってから停止を要求する。
        std::thread::sleep(Duration::from_secs(2));
        let _ = tx.send(Control::StopAll);
    });
    let (report, recorder) = run_with(&fixture, &yaml, None, no_secrets(), Some(rx));
    stopper.join().unwrap();
    assert_eq!(
        report.snapshot.phase,
        RunPhase::Stopped,
        "{:?}",
        recorder.notices
    );
    assert_eq!(report.snapshot.outcome, RunOutcome::Cancelled);
    assert_eq!(state(&report, "db"), TaskState::Stopped);
    assert_eq!(state(&report, "api"), TaskState::Stopped);
    let stops = std::fs::read_to_string(fixture.work().join("stops")).unwrap();
    assert_eq!(stops, "api\ndb\n");
}

#[test]
fn secret_values_are_passed_to_the_process_but_masked_in_logs() {
    // T36: 秘密値は子プロセスへ渡し、ログ・起動証跡には出さない。
    let fixture = Fixture::new();
    let config_dir = fixture.tmp.0.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let secrets_path = config_dir.join("secrets.yaml");
    std::fs::write(
        &secrets_path,
        "version: 1\nsecrets:\n  token:\n    value: s3cr3t-value\n",
    )
    .unwrap();
    std::fs::set_permissions(
        &secrets_path,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
    let loader: SecretsLoader = Arc::new(move || secrets::load_from_dir(&config_dir));
    let yaml = workflow(
        "",
        &job(
            "show",
            "echo \"token=$TOKEN\"; test \"${#TOKEN}\" -eq 12",
            "    env:\n      TOKEN: { secret: token }\n",
        ),
    );
    let (report, _) = run_with(&fixture, &yaml, Some("show"), loader, None);
    assert_eq!(report.snapshot.outcome, RunOutcome::Succeeded);
    let attempt_dir = fixture
        .state
        .attempt_dir(&report.snapshot.run_id, "show", 1);
    let log = std::fs::read_to_string(attempt_dir.join("stdout.log")).unwrap();
    assert!(log.contains("token=***"), "{log}");
    for file in ["stdout.log", "stderr.log", "launch.json", "result.json"] {
        let text = std::fs::read_to_string(attempt_dir.join(file)).unwrap();
        assert!(!text.contains("s3cr3t-value"), "{file}: {text}");
    }
    assert!(!format!("{:?}", report.snapshot).contains("s3cr3t-value"));
}

#[test]
fn undefined_secret_fails_before_any_process_starts() {
    // T36: 未定義の秘密値は外部操作前に失敗させる。
    let fixture = Fixture::new();
    let yaml = workflow(
        "",
        &job(
            "show",
            "touch \"$WORK/ran\"",
            "    env:\n      TOKEN: { secret: missing }\n",
        ),
    );
    let spec = load::load_str(&yaml).unwrap();
    let inputs = BTreeMap::new();
    let error = session::prepare_run(
        RunRequest {
            spec: &spec,
            plugin_config: &PluginConfig::default(),
            inputs: &inputs,
            checkout: &fixture.work(),
            target: Some(TaskId::from("show")),
            base_env: EnvMap::new(),
            load_secrets: no_secrets(),
            config_dir: None,
            bootstrap_only: false,
            herdr: false,
        },
        session::new_run_id(),
    )
    .err()
    .expect("undefined secret must fail");
    assert!(error.to_string().contains("missing"), "{error}");
    assert!(!fixture.work().join("ran").exists());
}

#[test]
fn resource_lock_serializes_runs() {
    // T35: 同名のlocksを持つタスクは別Runでも同時に実行しない。
    let fixture = Arc::new(Fixture::new());
    let yaml = workflow(
        "",
        &job(
            "migrate",
            "echo start >> \"$WORK/locks\"; sleep 0.5; echo end >> \"$WORK/locks\"",
            "    locks: [db]\n",
        ),
    );
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let fixture = Arc::clone(&fixture);
            let yaml = yaml.clone();
            std::thread::spawn(move || run(&fixture, &yaml, Some("migrate")).0)
        })
        .collect();
    for handle in handles {
        assert_eq!(
            handle.join().unwrap().snapshot.outcome,
            RunOutcome::Succeeded
        );
    }
    let log = std::fs::read_to_string(fixture.work().join("locks")).unwrap();
    assert_eq!(log, "start\nend\nstart\nend\n");
}

#[test]
fn job_limit_is_not_consumed_by_services() {
    // T09: serviceがjob用の実行枠を塞がない。
    let fixture = Fixture::new();
    let yaml = format!(
        "{}execution:\n  maxConcurrentJobs: 1\n",
        workflow(
            "",
            &[
                service(
                    "s1",
                    "echo ready; exec sleep 30",
                    "    waitFor:\n      type: log\n      pattern: ready\n      timeoutSeconds: 10\n",
                ),
                service(
                    "s2",
                    "echo ready; exec sleep 30",
                    "    waitFor:\n      type: log\n      pattern: ready\n      timeoutSeconds: 10\n",
                ),
                job(
                    "tests",
                    "true",
                    "    dependsOn:\n      - task: s1\n        condition: ready\n      - task: s2\n        condition: ready\n",
                ),
            ]
            .concat(),
        )
    );
    let (report, _) = run(&fixture, &yaml, Some("tests"));
    assert_eq!(report.snapshot.outcome, RunOutcome::Succeeded);
}

#[test]
fn log_readiness_ignores_output_of_previous_attempts() {
    // T38: 過去Attemptのログで一致させない(新しいAttemptのログは別ファイル)。
    let fixture = Fixture::new();
    let yaml = workflow(
        "",
        &service(
            "server",
            "echo starting; exec sleep 30",
            "    waitFor:\n      type: log\n      pattern: never-printed\n      timeoutSeconds: 1\n    stop:\n      graceSeconds: 1\n",
        ),
    );
    let (report, _) = run(&fixture, &yaml, Some("server"));
    assert_eq!(report.snapshot.outcome, RunOutcome::Failed);
    assert_eq!(attempts(&report, "server"), [AttemptOutcome::TimedOut]);
}

#[test]
fn monitor_marks_service_unready_after_threshold() {
    // T38: 連続失敗が閾値に達したら`unready`になり、依存jobを止める。
    let fixture = Fixture::new();
    let port = free_port();
    let yaml = workflow(
        "",
        &[
            service(
                "server",
                &format!("exec python3 -c \"import socket,time;s=socket.socket();s.bind((\\\"127.0.0.1\\\",{port}));s.listen();time.sleep(1.5);s.close();time.sleep(30)\""),
                &format!(
                    "    waitFor:\n      type: tcp\n      port: {port}\n      timeoutSeconds: 10\n      intervalMilliseconds: 100\n      consecutiveSuccesses: 1\n      monitor:\n        intervalSeconds: 1\n        failureThreshold: 2\n    stop:\n      graceSeconds: 1\n"
                ),
            ),
            job(
                "tests",
                "exec sleep 30",
                "    dependsOn:\n      - task: server\n        condition: ready\n    stop:\n      graceSeconds: 1\n",
            ),
        ]
        .concat(),
    );
    let (report, recorder) = run(&fixture, &yaml, Some("tests"));
    assert!(
        recorder
            .notices
            .iter()
            .any(|notice| notice.contains("unready")),
        "{:?}",
        recorder.notices
    );
    assert_eq!(state(&report, "tests"), TaskState::Cancelled);
    assert_eq!(report.snapshot.outcome, RunOutcome::Failed);
}

#[cfg(target_os = "linux")]
#[test]
fn stop_reports_and_kills_descendants_that_left_the_group() {
    // T26: グループ外へ移った子孫も、停止時に記録して照合できれば止めて報告する。
    let fixture = Fixture::new();
    let yaml = workflow(
        "",
        &service(
            "server",
            "python3 -c \"import os,time; os.setsid(); open(os.environ[\\\"WORK\\\"]+\\\"/escaped\\\",\\\"w\\\").write(str(os.getpid())); time.sleep(60)\" & echo ready; exec sleep 60",
            "    waitFor:\n      type: log\n      pattern: ready\n      timeoutSeconds: 10\n    stop:\n      graceSeconds: 1\n",
        ),
    );
    let (tx, rx) = mpsc::channel();
    let work = fixture.work();
    let stopper = std::thread::spawn(move || {
        for _ in 0..100 {
            if work.join("escaped").exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(200));
        let _ = tx.send(Control::StopAll);
    });
    let (report, recorder) = run_with(&fixture, &yaml, None, no_secrets(), Some(rx));
    stopper.join().unwrap();
    assert_eq!(report.snapshot.phase, RunPhase::Stopped);
    let pid: u32 = std::fs::read_to_string(fixture.work().join("escaped"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        recorder
            .notices
            .iter()
            .any(|notice| notice.contains("descendant")),
        "{:?}",
        recorder.notices
    );
    std::thread::sleep(Duration::from_millis(200));
    // 終了済み(回収待ちのゾンビを含む)で、稼働していない。
    let state = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    assert!(state.is_empty() || state.contains(") Z"), "{state}");
}

/// 最初の保存だけ成功し、以降は失敗する記録先。
#[derive(Default)]
struct FailingRecorder {
    calls: usize,
    notices: Vec<String>,
}

impl Recorder for FailingRecorder {
    fn record(&mut self, _snapshot: &RunSnapshot) -> Result<(), String> {
        self.calls += 1;
        if self.calls > 1 {
            Err("disk full".to_string())
        } else {
            Ok(())
        }
    }

    fn notice(&mut self, message: &str) {
        self.notices.push(message.to_string());
    }
}

#[test]
fn store_write_failure_stops_new_launches() {
    // T29: 状態ストアへの書き込みが失敗したら、新しいタスクを起動しない。
    let fixture = Fixture::new();
    let yaml = workflow(
        "",
        &[
            job("first", "touch \"$WORK/first\"", ""),
            job(
                "second",
                "touch \"$WORK/second\"",
                "    dependsOn:\n      - task: first\n        condition: succeeded\n",
            ),
        ]
        .concat(),
    );
    let spec = load::load_str(&yaml).unwrap();
    let inputs = BTreeMap::new();
    let mut env: EnvMap = std::env::vars_os().collect();
    env.insert(OsString::from("WORK"), fixture.work().into_os_string());
    let prepared = session::prepare_run(
        RunRequest {
            spec: &spec,
            plugin_config: &PluginConfig::default(),
            inputs: &inputs,
            checkout: &fixture.work(),
            target: None,
            bootstrap_only: false,
            herdr: false,
            base_env: env,
            load_secrets: no_secrets(),
            config_dir: None,
        },
        session::new_run_id(),
    )
    .unwrap();
    let (tx, rx) = mpsc::channel();
    let mut recorder = FailingRecorder::default();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        let _ = tx.send(Control::StopAll);
    });
    let report = Engine::new(prepared.config, fixture.state.clone(), &mut recorder, rx).run();
    stopper.join().unwrap();
    assert!(
        recorder
            .notices
            .iter()
            .any(|notice| notice.contains("状態ストア")),
        "{:?}",
        recorder.notices
    );
    assert!(!fixture.work().join("second").exists());
    assert_ne!(report.snapshot.outcome, RunOutcome::Succeeded);
}

#[test]
fn agent_observations_are_tracked_without_becoming_task_results() {
    // T10: Agentのidleをjob成功へ変換しない。照会できないときは観測値をunknownにする。
    let fixture = Fixture::new();
    let yaml = workflow(
        "init",
        &[
            job("init", "true", ""),
            "  developer:\n    type: agent\n    agent:\n      kind: claude\n    dependsOn:\n      - task: init\n        condition: succeeded\n".to_string(),
        ]
        .concat(),
    )
    .replace(
        "view: { type: status }",
        "view: { type: agent, task: developer }",
    );
    let spec = load::load_str(&yaml).unwrap_or_else(|error| panic!("{error}\n{yaml}"));
    let inputs = BTreeMap::new();
    let mut prepared = session::prepare_run(
        RunRequest {
            spec: &spec,
            plugin_config: &PluginConfig::default(),
            inputs: &inputs,
            checkout: &fixture.work(),
            target: None,
            bootstrap_only: false,
            herdr: true,
            base_env: std::env::vars_os().collect(),
            load_secrets: no_secrets(),
            config_dir: None,
        },
        session::new_run_id(),
    )
    .unwrap();
    assert_eq!(prepared.agents, [TaskId::from("developer")]);
    prepared.config.agents = BTreeMap::from([(TaskId::from("developer"), "developer".to_string())]);
    prepared.config.on_init_complete = Some(Box::new(|| {
        Ok(InitOutcome {
            message: "layout".to_string(),
            agents: BTreeMap::from([(
                TaskId::from("developer"),
                AgentStart::Started("w1:p2".to_string()),
            )]),
        })
    }));
    let probes = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::from([
        AgentProbe::Observed(AgentObservation::Working),
        AgentProbe::Observed(AgentObservation::Idle),
        AgentProbe::Unreachable("connection refused".to_string()),
    ])));
    prepared.config.observe_agent = Some(Arc::new(move |_| {
        probes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(AgentProbe::Observed(AgentObservation::Idle))
    }));
    prepared.config.agent_poll = Duration::from_millis(50);
    let (tx, rx) = mpsc::channel();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(800));
        let _ = tx.send(Control::StopAll);
    });
    let mut recorder = MemoryRecorder::default();
    let report = Engine::new(prepared.config, fixture.state.clone(), &mut recorder, rx).run();
    stopper.join().unwrap();
    let position = |text: &str| {
        recorder
            .notices
            .iter()
            .position(|notice| notice.contains(text))
    };
    assert!(
        position("agent working").is_some(),
        "{:?}",
        recorder.notices
    );
    assert!(position("agent idle") > position("agent working"));
    assert!(position("agent state unknown") > position("agent idle"));
    let developer = &report.snapshot.tasks[&TaskId::from("developer")];
    assert_eq!(developer.kind, TaskKind::Agent);
    // 停止要求でもAgentのプロセスは止めず、追跡だけを終える。
    assert_eq!(developer.runtime.state, TaskState::Stopped);
    assert!(
        developer
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("keeps running")
    );
    assert_eq!(developer.runtime.agent, Some(AgentObservation::Idle));
    // いずれの観測値でもsucceededにはならない。
    assert!(recorder.snapshots.iter().all(|snapshot| {
        snapshot.tasks[&TaskId::from("developer")].runtime.state != TaskState::Succeeded
    }));
}
