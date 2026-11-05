//! Coordinatorを使うCLIサブコマンド(docs/reference/cli.md)。
//!
//! `validate`/`plan`/`schema`は`cli`モジュールの純粋関数で扱い、ここでは状態ディレクトリ・
//! 制御ソケット・子プロセスを伴うコマンドを扱う。終了コードは cli.md の案に従う。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::config::model::{TaskId, TaskLifecycle};
use crate::config::{load, plugin};
use crate::coordinator::{self, Client, CoordinatorError};
use crate::plan::compile;
use crate::runtime::engine::{RunOutcome, RunPhase, TaskSnapshot};
use crate::runtime::logs::{self, Stream};
use crate::runtime::state_dir::StateDir;
use crate::runtime::store::{RunRecord, Store};
use crate::state::models::{AttemptOutcome, ReadinessState, TaskState};

pub const EXIT_OK: i32 = 0;
pub const EXIT_CONFIG: i32 = 2;
pub const EXIT_FAILED: i32 = 4;
pub const EXIT_CONNECTION: i32 = 5;
pub const EXIT_UNKNOWN: i32 = 6;
pub const EXIT_CANCELLED: i32 = 130;

const SUBCOMMANDS: &[&str] = &[
    "coordinator",
    "run",
    "up",
    "attach",
    "wait",
    "status",
    "logs",
    "stop",
    "resume",
    "retry",
    "doctor",
];

#[must_use]
pub fn handles(subcommand: &str) -> bool {
    SUBCOMMANDS.contains(&subcommand)
}

#[derive(Debug, Default)]
struct Args {
    positional: Vec<String>,
    config: Option<PathBuf>,
    inputs: Vec<(String, String)>,
    checkout: Option<PathBuf>,
    target: Option<String>,
    task: Option<String>,
    attempt: Option<u32>,
    stream: Option<String>,
    until: Option<String>,
    timeout: Option<u64>,
    json: bool,
    follow: bool,
    idle_exit: Option<u64>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = |name: &str| {
            iter.next()
                .cloned()
                .ok_or_else(|| format!("flag {name} requires a value"))
        };
        match arg.as_str() {
            "--config" => parsed.config = Some(PathBuf::from(value("--config")?)),
            "--input" => {
                let raw = value("--input")?;
                let (key, val) = raw
                    .split_once('=')
                    .ok_or_else(|| format!("--input value must be KEY=VALUE, got {raw:?}"))?;
                parsed.inputs.push((key.to_string(), val.to_string()));
            }
            "--checkout" => parsed.checkout = Some(PathBuf::from(value("--checkout")?)),
            "--target" => parsed.target = Some(value("--target")?),
            "--task" => parsed.task = Some(value("--task")?),
            "--attempt" => {
                let raw = value("--attempt")?;
                parsed.attempt =
                    Some(raw.parse().map_err(|_| {
                        format!("--attempt must be a positive integer, got {raw:?}")
                    })?);
            }
            "--stream" => parsed.stream = Some(value("--stream")?),
            "--until" => parsed.until = Some(value("--until")?),
            "--timeout" => {
                let raw = value("--timeout")?;
                parsed.timeout = Some(
                    raw.parse()
                        .map_err(|_| format!("--timeout must be seconds, got {raw:?}"))?,
                );
            }
            "--idle-exit" => {
                let raw = value("--idle-exit")?;
                parsed.idle_exit = Some(
                    raw.parse()
                        .map_err(|_| format!("--idle-exit must be seconds, got {raw:?}"))?,
                );
            }
            "--json" => parsed.json = true,
            "--follow" | "-f" => parsed.follow = true,
            other if other.starts_with("--") => return Err(format!("unknown flag {other:?}")),
            other => parsed.positional.push(other.to_string()),
        }
    }
    Ok(parsed)
}

fn eprint_error(json: bool, stage: &str, message: &str) {
    if json {
        println!(
            "{}",
            json!({ "ok": false, "error": { "stage": stage, "message": message } })
        );
    } else {
        eprintln!("エラー({stage}): {message}");
    }
}

/// サブコマンドを実行して終了コードを返す。
pub fn main(args: &[String]) -> i32 {
    let Some((subcommand, rest)) = args.split_first() else {
        return EXIT_CONFIG;
    };
    let parsed = match parse(rest) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprint_error(rest.iter().any(|arg| arg == "--json"), "cli", &message);
            return EXIT_CONFIG;
        }
    };
    let state = match StateDir::open_default() {
        Ok(state) => state,
        Err(error) => {
            eprint_error(parsed.json, "state-dir", &error.to_string());
            return EXIT_CONNECTION;
        }
    };
    match subcommand.as_str() {
        "coordinator" => coordinator_main(&state, &parsed),
        "run" => start_main(&state, &parsed, true),
        "up" | "attach" => start_main(&state, &parsed, false),
        "wait" => wait_main(&state, &parsed),
        "status" => status_main(&state, &parsed),
        "logs" => logs_main(&state, &parsed),
        "stop" => stop_main(&state, &parsed),
        "resume" => resume_main(&state, &parsed, None),
        "retry" => {
            let Some(task) = parsed.task.clone() else {
                eprint_error(parsed.json, "cli", "retry requires --task TASK");
                return EXIT_CONFIG;
            };
            resume_main(&state, &parsed, Some(task))
        }
        "doctor" => doctor_main(&state, &parsed),
        other => {
            eprint_error(parsed.json, "cli", &format!("unknown subcommand {other:?}"));
            EXIT_CONFIG
        }
    }
}

fn coordinator_main(state: &StateDir, args: &Args) -> i32 {
    let idle = Duration::from_secs(args.idle_exit.unwrap_or(30));
    match coordinator::serve(state, idle) {
        Ok(()) => EXIT_OK,
        Err(CoordinatorError::AlreadyRunning) => EXIT_OK,
        Err(error) => {
            eprint_error(false, "coordinator", &error.to_string());
            EXIT_CONNECTION
        }
    }
}

fn remote_exit(json: bool, error: &CoordinatorError) -> i32 {
    match error {
        CoordinatorError::Remote { code, message } => {
            let stage = if code == "invalid-config" {
                "validate"
            } else {
                "coordinator"
            };
            eprint_error(json, stage, message);
            match code.as_str() {
                "invalid-config" | "invalid-request" => EXIT_CONFIG,
                "not-found" | "not-active" | "conflict" => EXIT_FAILED,
                _ => EXIT_CONNECTION,
            }
        }
        other => {
            eprint_error(json, "connection", &other.to_string());
            EXIT_CONNECTION
        }
    }
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

fn install_interrupt_handler() {
    let handler = on_interrupt as extern "C" fn(libc::c_int);
    // SAFETY: ハンドラはアトミック変数への書き込みだけを行う。
    unsafe {
        libc::signal(libc::SIGINT, handler as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
    }
}

fn start_main(state: &StateDir, args: &Args, follow: bool) -> i32 {
    if follow && args.target.is_none() {
        eprint_error(args.json, "cli", "run requires --target TASK");
        return EXIT_CONFIG;
    }
    let config = args
        .config
        .clone()
        .unwrap_or_else(|| PathBuf::from(".herdr/workflow.yaml"));
    let plugin_config = match plugin::load_from_env() {
        Ok(config) => config,
        Err(error) => {
            eprint_error(args.json, "plugin-config", &error.to_string());
            return EXIT_CONFIG;
        }
    };
    let workflow = match std::fs::read_to_string(&config) {
        Ok(text) => text,
        Err(error) => {
            eprint_error(
                args.json,
                "load",
                &format!("failed to read config file {}: {error}", config.display()),
            );
            return EXIT_CONFIG;
        }
    };
    // 外部操作の前に手元でも検証する(Coordinatorも同じ検証を行う)。
    let spec = match load::load_str(&workflow) {
        Ok(spec) => spec,
        Err(error) => {
            eprint_error(args.json, "load", &error.to_string());
            return EXIT_CONFIG;
        }
    };
    if let Err(error) = compile::compile_with_plugin_config(&spec, &plugin_config) {
        eprint_error(args.json, "compile", &error.to_string());
        return EXIT_CONFIG;
    }
    let checkout = args.checkout.clone().unwrap_or_else(|| PathBuf::from("."));
    let checkout = std::fs::canonicalize(&checkout).unwrap_or(checkout);
    let config_path = std::fs::canonicalize(&config).unwrap_or(config);

    let mut client = match Client::connect_or_spawn(state) {
        Ok(client) => client,
        Err(error) => return remote_exit(args.json, &error),
    };
    let params = json!({
        "configPath": config_path,
        "workflow": workflow,
        "inputs": args.inputs,
        "checkout": checkout,
        "target": args.target,
        "env": coordinator::current_env(),
        "pluginConfig": plugin_config,
    });
    let result = match client.call("run.start", params) {
        Ok(result) => result,
        Err(error) => return remote_exit(args.json, &error),
    };
    let run_id = result["runId"].as_str().unwrap_or_default().to_string();
    if !follow {
        if args.json {
            println!(
                "{}",
                json!({ "ok": true, "runId": run_id, "planHash": result["planHash"] })
            );
        } else {
            println!("{run_id}");
        }
        return EXIT_OK;
    }
    follow_run(state, &mut client, &run_id, args.json)
}

fn resume_main(state: &StateDir, args: &Args, retry_task: Option<String>) -> i32 {
    let Some(run_id) = args.positional.first() else {
        eprint_error(args.json, "cli", "RUN is required");
        return EXIT_CONFIG;
    };
    let mut client = match Client::connect_or_spawn(state) {
        Ok(client) => client,
        Err(error) => return remote_exit(args.json, &error),
    };
    let params = json!({
        "runId": run_id,
        "retryTask": retry_task,
        "env": coordinator::current_env(),
    });
    if let Err(error) = client.call("run.resume", params) {
        return remote_exit(args.json, &error);
    }
    follow_run(state, &mut client, run_id, args.json)
}

fn outcome_exit(outcome: RunOutcome) -> i32 {
    match outcome {
        RunOutcome::Succeeded => EXIT_OK,
        RunOutcome::Failed => EXIT_FAILED,
        RunOutcome::Cancelled => EXIT_CANCELLED,
        RunOutcome::Unknown | RunOutcome::Pending => EXIT_UNKNOWN,
    }
}

fn is_done(phase: RunPhase) -> bool {
    matches!(phase, RunPhase::Finished | RunPhase::Stopped)
}

/// Runの進捗を標準エラーへ表示し、終了まで待つ。Ctrl+Cで停止を要求する。
fn follow_run(state: &StateDir, client: &mut Client, run_id: &str, json: bool) -> i32 {
    install_interrupt_handler();
    let store = match Store::open(&state.db_path()) {
        Ok(store) => store,
        Err(error) => {
            eprint_error(json, "store", &error.to_string());
            return EXIT_CONNECTION;
        }
    };
    let mut last_event = 0;
    let mut stop_sent = false;
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) && !stop_sent {
            stop_sent = true;
            let _ = client.call("run.stop", json!({ "runId": run_id }));
        }
        if let Ok(events) = store.events(run_id, last_event) {
            for (id, time, message) in events {
                last_event = id;
                if !json {
                    eprintln!("{} {message}", &time[11..19.min(time.len())]);
                }
            }
        }
        match store.run(run_id) {
            Ok(run) if is_done(run.phase) => {
                let tasks = store.tasks(run_id).unwrap_or_default();
                if json {
                    println!("{}", status_json(&run, &tasks));
                } else {
                    print!("{}", render_status(&run, &tasks));
                }
                let _ = std::io::stdout().flush();
                return if stop_sent && run.outcome != RunOutcome::Succeeded {
                    EXIT_CANCELLED
                } else {
                    outcome_exit(run.outcome)
                };
            }
            Ok(_) => {}
            Err(error) => {
                eprint_error(json, "store", &error.to_string());
                return EXIT_UNKNOWN;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn stop_main(state: &StateDir, args: &Args) -> i32 {
    let Some(run_id) = args.positional.first() else {
        eprint_error(args.json, "cli", "RUN is required");
        return EXIT_CONFIG;
    };
    let mut client = match Client::connect(state) {
        Ok(client) => client,
        Err(error) => return remote_exit(args.json, &error),
    };
    match client.call("run.stop", json!({ "runId": run_id })) {
        Ok(_) => {
            if args.json {
                println!("{}", json!({ "ok": true, "runId": run_id }));
            } else {
                println!("停止を要求しました: {run_id}");
            }
            EXIT_OK
        }
        Err(error) => remote_exit(args.json, &error),
    }
}

/// `ready`: 初期化が成功し、Runのserviceがすべてready。`finished`: Runが終了。
fn wait_main(state: &StateDir, args: &Args) -> i32 {
    let Some(run_id) = args.positional.first() else {
        eprint_error(args.json, "cli", "RUN is required");
        return EXIT_CONFIG;
    };
    let until = args.until.as_deref().unwrap_or("finished");
    if !matches!(until, "ready" | "finished") {
        eprint_error(args.json, "cli", "--until must be ready or finished");
        return EXIT_CONFIG;
    }
    let deadline = args
        .timeout
        .map(|seconds| Instant::now() + Duration::from_secs(seconds));
    let store = match Store::open(&state.db_path()) {
        Ok(store) => store,
        Err(error) => {
            eprint_error(args.json, "store", &error.to_string());
            return EXIT_CONNECTION;
        }
    };
    loop {
        let run = match store.run(run_id) {
            Ok(run) => run,
            Err(error) => {
                eprint_error(args.json, "store", &error.to_string());
                return EXIT_FAILED;
            }
        };
        let tasks = store.tasks(run_id).unwrap_or_default();
        if until == "ready" && run_is_ready(&run, &tasks) {
            if args.json {
                println!("{}", status_json(&run, &tasks));
            } else {
                println!("ready: {run_id}");
            }
            return EXIT_OK;
        }
        if is_done(run.phase) {
            if args.json {
                println!("{}", status_json(&run, &tasks));
            } else {
                print!("{}", render_status(&run, &tasks));
            }
            return if until == "finished" {
                outcome_exit(run.outcome)
            } else {
                EXIT_FAILED
            };
        }
        if run.outcome == RunOutcome::Failed && until == "ready" {
            print!("{}", render_status(&run, &tasks));
            return EXIT_FAILED;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            eprint_error(args.json, "wait", "timed out");
            return EXIT_UNKNOWN;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn run_is_ready(run: &RunRecord, tasks: &BTreeMap<TaskId, TaskSnapshot>) -> bool {
    run.phase == RunPhase::Active
        && tasks.values().all(|task| match task.kind {
            TaskLifecycle::Service => {
                task.runtime.state == TaskState::Running
                    && task.runtime.readiness == ReadinessState::Ready
            }
            TaskLifecycle::Job => !matches!(
                task.runtime.state,
                TaskState::Failed | TaskState::Unknown | TaskState::Skipped
            ),
        })
}

fn text<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn status_json(run: &RunRecord, tasks: &BTreeMap<TaskId, TaskSnapshot>) -> Value {
    json!({
        "ok": true,
        "run": {
            "runId": run.run_id,
            "phase": run.phase,
            "outcome": run.outcome,
            "target": run.target,
            "checkout": run.checkout,
            "configPath": run.config_path,
            "planHash": run.plan_hash,
            "createdAt": run.created_at,
            "updatedAt": run.updated_at,
        },
        "tasks": tasks,
    })
}

fn last_outcome(task: &TaskSnapshot) -> String {
    match task.runtime.attempts.last().map(|attempt| &attempt.outcome) {
        Some(AttemptOutcome::ExitCode(code)) => format!("exit {code}"),
        Some(AttemptOutcome::TimedOut) => "timed out".to_string(),
        Some(AttemptOutcome::UnexpectedExit) => "unexpected exit".to_string(),
        Some(AttemptOutcome::Signaled(signal)) => signal.clone(),
        Some(AttemptOutcome::LaunchFailed(reason)) => format!("launch failed: {reason}"),
        Some(AttemptOutcome::Unknown) => "unknown".to_string(),
        Some(AttemptOutcome::Pending) | None => String::new(),
    }
}

fn render_status(run: &RunRecord, tasks: &BTreeMap<TaskId, TaskSnapshot>) -> String {
    let mut out = format!(
        "Run: {}  phase: {}  outcome: {}  target: {}\ncheckout: {}  updated: {}\n\n",
        run.run_id,
        text(&run.phase),
        text(&run.outcome),
        run.target.as_deref().unwrap_or("-"),
        run.checkout.display(),
        run.updated_at
    );
    out.push_str(&format!(
        "{:<16} {:<10} {:<10} {:<8} DETAIL\n",
        "TASK", "PHASE", "READINESS", "ATTEMPT"
    ));
    for (id, task) in tasks {
        let readiness = match task.runtime.readiness {
            ReadinessState::NotApplicable => "-".to_string(),
            other => text(&other),
        };
        let attempt = task.runtime.attempts.last().map_or_else(
            || "-".to_string(),
            |attempt| attempt.attempt_id.0.to_string(),
        );
        let detail = task
            .detail
            .clone()
            .filter(|detail| !detail.is_empty())
            .unwrap_or_else(|| last_outcome(task));
        out.push_str(&format!(
            "{:<16} {:<10} {:<10} {:<8} {}\n",
            id.0,
            text(&task.runtime.state),
            readiness,
            attempt,
            detail
        ));
    }
    out
}

fn status_main(state: &StateDir, args: &Args) -> i32 {
    let store = match Store::open(&state.db_path()) {
        Ok(store) => store,
        Err(error) => {
            eprint_error(args.json, "store", &error.to_string());
            return EXIT_CONNECTION;
        }
    };
    if let Some(run_id) = args.positional.first() {
        let run = match store.run(run_id) {
            Ok(run) => run,
            Err(error) => {
                eprint_error(args.json, "store", &error.to_string());
                return EXIT_FAILED;
            }
        };
        let tasks = store.tasks(run_id).unwrap_or_default();
        if args.json {
            println!("{}", status_json(&run, &tasks));
        } else {
            print!("{}", render_status(&run, &tasks));
        }
        return EXIT_OK;
    }
    let runs = store.recent_runs(20).unwrap_or_default();
    if args.json {
        let list: Vec<Value> = runs
            .iter()
            .map(|run| {
                json!({
                    "runId": run.run_id,
                    "phase": run.phase,
                    "outcome": run.outcome,
                    "target": run.target,
                    "checkout": run.checkout,
                    "createdAt": run.created_at,
                })
            })
            .collect();
        println!("{}", json!({ "ok": true, "runs": list }));
    } else {
        println!(
            "{:<30} {:<10} {:<10} {:<12} CHECKOUT",
            "RUN", "PHASE", "OUTCOME", "TARGET"
        );
        for run in runs {
            println!(
                "{:<30} {:<10} {:<10} {:<12} {}",
                run.run_id,
                text(&run.phase),
                text(&run.outcome),
                run.target.as_deref().unwrap_or("-"),
                run.checkout.display()
            );
        }
    }
    EXIT_OK
}

fn logs_main(state: &StateDir, args: &Args) -> i32 {
    let (Some(run_id), Some(task)) = (args.positional.first(), args.task.as_ref()) else {
        eprint_error(
            args.json,
            "cli",
            "usage: logs RUN --task TASK [--attempt N] [--follow]",
        );
        return EXIT_CONFIG;
    };
    let store = match Store::open(&state.db_path()) {
        Ok(store) => store,
        Err(error) => {
            eprint_error(args.json, "store", &error.to_string());
            return EXIT_CONNECTION;
        }
    };
    let streams: Vec<Stream> = match args.stream.as_deref() {
        None | Some("any") => vec![Stream::Stdout, Stream::Stderr],
        Some("stdout") => vec![Stream::Stdout],
        Some("stderr") => vec![Stream::Stderr],
        Some(other) => {
            eprint_error(args.json, "cli", &format!("unknown stream {other:?}"));
            return EXIT_CONFIG;
        }
    };
    let mut printed: BTreeMap<Stream, u64> = BTreeMap::new();
    loop {
        let tasks = store.tasks(run_id).unwrap_or_default();
        let Some(snapshot) = tasks.get(&TaskId(task.clone())) else {
            eprint_error(
                args.json,
                "logs",
                &format!("task {task} not found in run {run_id}"),
            );
            return EXIT_FAILED;
        };
        let Some(attempt) = args.attempt.or_else(|| {
            snapshot
                .runtime
                .attempts
                .last()
                .map(|attempt| attempt.attempt_id.0)
        }) else {
            if !args.follow {
                return EXIT_OK;
            }
            std::thread::sleep(Duration::from_millis(200));
            continue;
        };
        let dir = state.attempt_dir(run_id, task, attempt);
        let mut records = Vec::new();
        for stream in &streams {
            let path = dir.join(stream.file_name());
            if let Ok(found) = logs::read_records(&path, logs::DEFAULT_GENERATIONS) {
                let seen = printed.get(stream).copied().unwrap_or(0);
                let new: Vec<_> = found
                    .into_iter()
                    .filter(|record| record.seq > seen)
                    .collect();
                if let Some(last) = new.last() {
                    printed.insert(*stream, last.seq);
                }
                records.extend(new);
            }
        }
        records.sort_by(|a, b| a.time.cmp(&b.time));
        for record in &records {
            if args.json {
                println!("{}", serde_json::to_string(record).unwrap_or_default());
            } else {
                println!("{}", logs::display_text(&record.data));
            }
        }
        let _ = std::io::stdout().flush();
        if !args.follow {
            return EXIT_OK;
        }
        let finished_attempt = snapshot
            .runtime
            .attempts
            .iter()
            .find(|candidate| candidate.attempt_id.0 == attempt)
            .is_some_and(|candidate| candidate.outcome != AttemptOutcome::Pending);
        let run_done = store.run(run_id).is_ok_and(|run| is_done(run.phase));
        if finished_attempt || run_done {
            return EXIT_OK;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn command_version(program: &str, arg: &str) -> Option<String> {
    let output = std::process::Command::new(program).arg(arg).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Some(text.lines().next().unwrap_or_default().trim().to_string())
}

/// 環境の検査。致命的な項目(状態ディレクトリ、ソケット、git)が失敗したら終了コード5。
fn doctor_main(state: &StateDir, args: &Args) -> i32 {
    let mut checks: Vec<(String, bool, String, bool)> = Vec::new();
    checks.push((
        "state-dir".to_string(),
        true,
        state.root().display().to_string(),
        true,
    ));
    let socket = coordinator::check_socket_path(&state.socket_path());
    checks.push((
        "socket-path".to_string(),
        socket.is_ok(),
        socket.err().map_or_else(
            || state.socket_path().display().to_string(),
            |error| error.to_string(),
        ),
        true,
    ));
    let store = Store::open(&state.db_path());
    checks.push((
        "state-db".to_string(),
        store.is_ok(),
        store.err().map_or_else(
            || format!("schema version {}", crate::runtime::store::SCHEMA_VERSION),
            |error| error.to_string(),
        ),
        true,
    ));
    let git = command_version("git", "--version");
    checks.push((
        "git".to_string(),
        git.is_some(),
        git.unwrap_or_else(|| "git not found".to_string()),
        true,
    ));
    let mise = command_version("mise", "--version");
    checks.push((
        "mise".to_string(),
        mise.is_some(),
        mise.unwrap_or_else(|| {
            "mise not found (needed only by workflows that call it)".to_string()
        }),
        false,
    ));
    let herdr_bin = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string());
    let herdr = command_version(&herdr_bin, "--version");
    checks.push((
        "herdr".to_string(),
        herdr.is_some(),
        herdr.unwrap_or_else(|| {
            format!("{herdr_bin} not found (needed for Herdr integration; headless runs work without it)")
        }),
        false,
    ));
    let coordinator = Client::connect(state)
        .and_then(|mut client| client.call("ping", json!({})))
        .map(|value| format!("running (pid {})", value["pid"]))
        .unwrap_or_else(|_| "not running (started on demand)".to_string());
    checks.push(("coordinator".to_string(), true, coordinator, false));
    checks.push((
        "plugin-config".to_string(),
        plugin::load_from_env().is_ok(),
        plugin::load_from_env()
            .err()
            .map_or_else(|| "ok".to_string(), |error| error.to_string()),
        true,
    ));
    checks.push((
        "secrets".to_string(),
        crate::config::secrets::load_from_env().is_ok(),
        crate::config::secrets::load_from_env()
            .err()
            .map_or_else(|| "ok".to_string(), |error| error.to_string()),
        true,
    ));
    let fatal = checks.iter().any(|(_, ok, _, required)| *required && !ok);
    if args.json {
        let list: Vec<Value> = checks
            .iter()
            .map(|(name, ok, detail, required)| {
                json!({ "name": name, "ok": ok, "detail": detail, "required": required })
            })
            .collect();
        println!("{}", json!({ "ok": !fatal, "checks": list }));
    } else {
        for (name, ok, detail, required) in &checks {
            let mark = match (ok, required) {
                (true, _) => "ok  ",
                (false, true) => "FAIL",
                (false, false) => "warn",
            };
            println!("[{mark}] {name:<14} {detail}");
        }
    }
    if fatal { EXIT_CONNECTION } else { EXIT_OK }
}
