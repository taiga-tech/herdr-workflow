//! TaskRunner(ADR-0002)。Attemptごとに1つ起動し、子プロセスを監視する。
//!
//! Coordinatorとは標準入出力のJSON Linesでやり取りする。
//!
//! - 入力1行目: [`LaunchRequest`](実行内容。秘密値を解決済みの環境変数を含むため、
//!   ファイルへは書かない)。以後: [`Command`](停止信号、ログ行の報告の切り替え)。
//! - 出力: [`Report`](起動、ログ行、stream終了、終了)。
//!
//! 入力が閉じても(Coordinatorの終了)、子プロセスは止めず、ログの取り込みと
//! `result.json`の確定を続ける。出力へ書けなくなっても処理を続ける。

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::runtime::logs::{self, LogRecord, LogWriter, Masker, Stream};
use crate::runtime::process::{self, Exit, LaunchSpec, ProcessIdentity};

/// 実行内容。環境変数はOSの値をそのまま運ぶためバイト列で持つ。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRequest {
    pub run_id: String,
    pub task_id: String,
    pub attempt_id: u32,
    pub attempt_dir: PathBuf,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
    /// ログで置き換える秘密値。
    pub masked: Vec<String>,
    /// readinessのためにログ行を報告するか(後から切り替えられる)。
    pub report_lines: bool,
    pub log_max_bytes: u64,
    pub log_generations: u32,
    /// `launch.json`に記録する表示用のコマンド(秘密値を含まない)。
    pub command_summary: String,
}

impl std::fmt::Debug for LaunchRequest {
    // 環境変数と秘密値は表示しない。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchRequest")
            .field("run_id", &self.run_id)
            .field("task_id", &self.task_id)
            .field("attempt_id", &self.attempt_id)
            .field("program", &self.program)
            .finish_non_exhaustive()
    }
}

impl LaunchRequest {
    #[must_use]
    pub fn encode_env(env: &BTreeMap<OsString, OsString>) -> Vec<(Vec<u8>, Vec<u8>)> {
        env.iter()
            .map(|(key, value)| (key.as_bytes().to_vec(), value.as_bytes().to_vec()))
            .collect()
    }

    fn decode_env(&self) -> BTreeMap<OsString, OsString> {
        self.env
            .iter()
            .map(|(key, value)| {
                (
                    OsString::from_vec(key.clone()),
                    OsString::from_vec(value.clone()),
                )
            })
            .collect()
    }
}

/// Coordinatorからの指示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Command {
    /// `TERM`または`KILL`を子プロセスのグループへ送る。
    Signal { signal: String },
    /// ログ行の報告を切り替える。
    Lines { enabled: bool },
}

/// TaskRunnerからの報告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Report {
    Started {
        child: ProcessIdentity,
        runner: Option<ProcessIdentity>,
        started_at: String,
    },
    LaunchFailed {
        reason: String,
    },
    Line {
        stream: Stream,
        text: String,
    },
    StreamClosed {
        stream: Stream,
        lines: u64,
        degraded: bool,
    },
    /// `result.json`を確定した後に送る。
    Exited {
        exit_code: Option<i32>,
        signal: Option<String>,
        /// 停止時に記録した子孫のうち、終了後も残っていたため強制終了したPID。
        survivors: Vec<u32>,
        error: Option<String>,
    },
}

/// `launch.json`(起動証跡)。秘密値と環境変数は含めない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchFile {
    pub run_id: String,
    pub task_id: String,
    pub attempt_id: u32,
    pub command: String,
    pub cwd: PathBuf,
    pub child: ProcessIdentity,
    pub runner: Option<ProcessIdentity>,
    pub started_at: String,
}

/// `result.json`(終了結果)。一時ファイルからの置換で確定する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultFile {
    pub run_id: String,
    pub task_id: String,
    pub attempt_id: u32,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub finished_at: String,
}

pub(crate) fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// 報告の書き込み口。書けなくなったら以後は捨てる(Coordinatorの終了)。
#[derive(Clone)]
struct Reporter {
    out: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
}

impl Reporter {
    fn send(&self, report: &Report) {
        let mut out = self
            .out
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(writer) = out.as_mut() else {
            return;
        };
        let Ok(mut line) = serde_json::to_vec(report) else {
            return;
        };
        line.push(b'\n');
        if writer
            .write_all(&line)
            .and_then(|()| writer.flush())
            .is_err()
        {
            *out = None;
        }
    }
}

fn signal_number(name: &str) -> Option<libc::c_int> {
    match name {
        "TERM" => Some(libc::SIGTERM),
        "KILL" => Some(libc::SIGKILL),
        _ => None,
    }
}

/// TaskRunnerの本体。終了コード(0: 子プロセスの結果を確定した、1: 起動要求を読めない)。
pub fn serve(input: Box<dyn Read + Send>, output: Box<dyn Write + Send>) -> i32 {
    let reporter = Reporter {
        out: Arc::new(Mutex::new(Some(output))),
    };
    let mut input = BufReader::new(input);
    let mut first = String::new();
    if input.read_line(&mut first).unwrap_or(0) == 0 {
        return 1;
    }
    let request: LaunchRequest = match serde_json::from_str(&first) {
        Ok(request) => request,
        Err(error) => {
            reporter.send(&Report::LaunchFailed {
                reason: format!("invalid launch request: {error}"),
            });
            return 1;
        }
    };
    let writers = (
        LogWriter::create(
            request.attempt_dir.join(Stream::Stdout.file_name()),
            request.log_max_bytes,
            request.log_generations,
        ),
        LogWriter::create(
            request.attempt_dir.join(Stream::Stderr.file_name()),
            request.log_max_bytes,
            request.log_generations,
        ),
    );
    let (stdout_writer, stderr_writer) = match writers {
        (Ok(out), Ok(err)) => (out, err),
        (Err(error), _) | (_, Err(error)) => {
            reporter.send(&Report::LaunchFailed {
                reason: format!("log file: {error}"),
            });
            return 0;
        }
    };
    let spec = LaunchSpec {
        program: request.program.clone(),
        args: request.args.clone(),
        cwd: request.cwd.clone(),
        env: request.decode_env(),
    };
    let spawned = match process::spawn(&spec) {
        Ok(spawned) => spawned,
        Err(error) => {
            reporter.send(&Report::LaunchFailed {
                reason: format!("{}: {error}", request.program),
            });
            return 0;
        }
    };
    let started_at = logs::rfc3339(SystemTime::now());
    let runner = process::observe(std::process::id());
    let launch = LaunchFile {
        run_id: request.run_id.clone(),
        task_id: request.task_id.clone(),
        attempt_id: request.attempt_id,
        command: request.command_summary.clone(),
        cwd: request.cwd.clone(),
        child: spawned.identity.clone(),
        runner: runner.clone(),
        started_at: started_at.clone(),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&launch) {
        let _ = write_atomic(&request.attempt_dir.join("launch.json"), &bytes);
    }
    reporter.send(&Report::Started {
        child: spawned.identity.clone(),
        runner,
        started_at,
    });

    let report_lines = Arc::new(AtomicBool::new(request.report_lines));
    let masker = Masker::new(request.masked.clone());
    let mut pumps = Vec::new();
    for (stream, reader, writer) in [
        (
            Stream::Stdout,
            Box::new(spawned.stdout) as Box<dyn Read + Send>,
            stdout_writer,
        ),
        (Stream::Stderr, Box::new(spawned.stderr), stderr_writer),
    ] {
        let record = LogRecord {
            run_id: request.run_id.clone(),
            task_id: request.task_id.clone(),
            attempt_id: request.attempt_id,
            time: String::new(),
            stream,
            seq: 0,
            data: String::new(),
        };
        let masker = masker.clone();
        let reporter = reporter.clone();
        let wanted = Arc::clone(&report_lines);
        pumps.push(std::thread::spawn(move || {
            let lines_reporter = reporter.clone();
            let (lines, degraded) = logs::pump(reader, writer, &record, &masker, |line| {
                if wanted.load(Ordering::Relaxed) {
                    lines_reporter.send(&Report::Line {
                        stream,
                        text: line.to_string(),
                    });
                }
            });
            reporter.send(&Report::StreamClosed {
                stream,
                lines,
                degraded,
            });
        }));
    }

    // 停止信号を送った時点の子孫(T26)。終了後も残っていれば止める。
    let descendants: Arc<Mutex<Vec<ProcessIdentity>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let handle = spawned.handle.clone();
        let report_lines = Arc::clone(&report_lines);
        let descendants = Arc::clone(&descendants);
        std::thread::spawn(move || {
            for line in input.lines() {
                let Ok(line) = line else { break };
                match serde_json::from_str::<Command>(&line) {
                    Ok(Command::Signal { signal }) => {
                        let Some(number) = signal_number(&signal) else {
                            continue;
                        };
                        {
                            let mut recorded = descendants
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            if recorded.is_empty() {
                                *recorded = process::descendants(handle.pid());
                            }
                        }
                        handle.signal_group(number);
                    }
                    Ok(Command::Lines { enabled }) => {
                        report_lines.store(enabled, Ordering::Relaxed);
                    }
                    Err(_) => {}
                }
            }
            // 入力が閉じても(Coordinatorの終了)、子プロセスは止めない。
        });
    }

    let exit = spawned
        .waiter
        .wait()
        .map(process::classify_exit)
        .map_err(|error| error.to_string());
    for pump in pumps {
        let _ = pump.join();
    }
    let survivors: Vec<u32> = descendants
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|identity| process::signal_if_same(identity, libc::SIGKILL))
        .map(|identity| identity.pid)
        .collect();
    let (exit_code, signal, error) = match &exit {
        Ok(Exit::Code(code)) => (Some(*code), None, None),
        Ok(Exit::Signal(signal)) => (None, Some(signal.clone()), None),
        Err(error) => (None, None, Some(error.clone())),
    };
    if error.is_none() {
        let result = ResultFile {
            run_id: request.run_id.clone(),
            task_id: request.task_id.clone(),
            attempt_id: request.attempt_id,
            exit_code,
            signal: signal.clone(),
            finished_at: logs::rfc3339(SystemTime::now()),
        };
        if let Ok(bytes) = serde_json::to_vec_pretty(&result) {
            let _ = write_atomic(&request.attempt_dir.join("result.json"), &bytes);
        }
    }
    reporter.send(&Report::Exited {
        exit_code,
        signal,
        survivors,
        error,
    });
    0
}

/// `herdr-workflow runner`の入口。自分のプロセスグループに入り、端末の信号を受けない。
pub fn main() -> i32 {
    // SAFETY: 信号の扱いを無視に変えるだけ。Coordinatorの終了でパイプが切れても続ける。
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }
    serve(Box::new(std::io::stdin()), Box::new(std::io::stdout()))
}

/// TaskRunnerとの接続。
pub struct Connection {
    pub input: Box<dyn Write + Send>,
    pub output: Box<dyn BufRead + Send>,
    /// 別プロセスで動かした場合の子(回収用)。
    pub process: Option<std::process::Child>,
}

/// TaskRunnerの起動方法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launcher {
    /// `<exe> runner`を別プロセス(新しいプロセスグループ)で起動する。
    Process(PathBuf),
    /// 同じプロトコルをスレッドとパイプで動かす(単体試験用)。
    Thread,
}

impl Launcher {
    /// `HERDR_WORKFLOW_BIN`または自分自身の実行ファイルを使う。
    pub fn current() -> std::io::Result<Self> {
        let exe = std::env::var_os("HERDR_WORKFLOW_BIN")
            .map(PathBuf::from)
            .map_or_else(std::env::current_exe, Ok)?;
        Ok(Self::Process(exe))
    }

    pub fn start(&self, request: &LaunchRequest) -> std::io::Result<Connection> {
        let mut line = serde_json::to_vec(request).map_err(std::io::Error::other)?;
        line.push(b'\n');
        match self {
            Self::Process(exe) => {
                use std::os::unix::process::CommandExt;
                let stderr = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(request.attempt_dir.join("runner.log"))?;
                let mut child = std::process::Command::new(exe)
                    .arg("runner")
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(stderr)
                    .process_group(0)
                    .spawn()?;
                let mut input = child
                    .stdin
                    .take()
                    .ok_or_else(|| std::io::Error::other("runner stdin missing"))?;
                let output = child
                    .stdout
                    .take()
                    .ok_or_else(|| std::io::Error::other("runner stdout missing"))?;
                input.write_all(&line)?;
                input.flush()?;
                Ok(Connection {
                    input: Box::new(input),
                    output: Box::new(BufReader::new(output)),
                    process: Some(child),
                })
            }
            Self::Thread => {
                let (to_runner_read, mut to_runner_write) = std::io::pipe()?;
                let (from_runner_read, from_runner_write) = std::io::pipe()?;
                to_runner_write.write_all(&line)?;
                std::thread::spawn(move || {
                    serve(Box::new(to_runner_read), Box::new(from_runner_write));
                });
                Ok(Connection {
                    input: Box::new(to_runner_write),
                    output: Box::new(BufReader::new(from_runner_read)),
                    process: None,
                })
            }
        }
    }
}

/// 報告を1行読む。接続が閉じたら`None`。
pub fn read_report(output: &mut dyn BufRead) -> Option<Report> {
    let mut line = String::new();
    loop {
        line.clear();
        if output.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if let Ok(report) = serde_json::from_str(&line) {
            return Some(report);
        }
    }
}

/// 指示を送る口。送れなければ`false`。
#[derive(Clone)]
pub struct Control {
    input: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("runner::Control")
    }
}

impl Control {
    #[must_use]
    pub fn new(input: Box<dyn Write + Send>) -> Self {
        Self {
            input: Arc::new(Mutex::new(input)),
        }
    }

    pub fn send(&self, command: &Command) -> bool {
        let Ok(mut line) = serde_json::to_vec(command) else {
            return false;
        };
        line.push(b'\n');
        let mut input = self
            .input
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        input.write_all(&line).and_then(|()| input.flush()).is_ok()
    }

    pub fn signal(&self, signal: libc::c_int) -> bool {
        let name = if signal == libc::SIGKILL {
            "KILL"
        } else {
            "TERM"
        };
        self.send(&Command::Signal {
            signal: name.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::state_dir::test_support::TempDir;

    fn request(dir: &std::path::Path, script: &str) -> LaunchRequest {
        LaunchRequest {
            run_id: "r".to_string(),
            task_id: "t".to_string(),
            attempt_id: 1,
            attempt_dir: dir.to_path_buf(),
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            cwd: dir.to_path_buf(),
            env: LaunchRequest::encode_env(&std::env::vars_os().collect()),
            masked: vec!["hunter2".to_string()],
            report_lines: true,
            log_max_bytes: logs::DEFAULT_MAX_BYTES,
            log_generations: logs::DEFAULT_GENERATIONS,
            command_summary: "sh".to_string(),
        }
    }

    fn collect(mut connection: Connection) -> Vec<Report> {
        let mut reports = Vec::new();
        while let Some(report) = read_report(connection.output.as_mut()) {
            let done = matches!(report, Report::Exited { .. } | Report::LaunchFailed { .. });
            reports.push(report);
            if done {
                break;
            }
        }
        reports
    }

    #[test]
    fn runner_reports_lines_and_writes_result_files() {
        let tmp = TempDir::new("runner");
        let connection = Launcher::Thread
            .start(&request(&tmp.0, "echo ready hunter2; exit 4"))
            .unwrap();
        let reports = collect(connection);
        assert!(matches!(reports[0], Report::Started { .. }), "{reports:?}");
        assert!(reports.contains(&Report::Line {
            stream: Stream::Stdout,
            text: "ready ***".to_string()
        }));
        assert!(matches!(
            reports.last(),
            Some(Report::Exited {
                exit_code: Some(4),
                ..
            })
        ));
        let result: ResultFile =
            serde_json::from_slice(&std::fs::read(tmp.0.join("result.json")).unwrap()).unwrap();
        assert_eq!(result.exit_code, Some(4));
        let launch = std::fs::read_to_string(tmp.0.join("launch.json")).unwrap();
        assert!(!launch.contains("hunter2"));
        assert!(!launch.contains("PATH"));
    }

    #[test]
    fn launch_failure_is_reported() {
        let tmp = TempDir::new("runner");
        let mut bad = request(&tmp.0, "true");
        bad.program = "/nonexistent/program".to_string();
        let reports = collect(Launcher::Thread.start(&bad).unwrap());
        assert!(matches!(reports.as_slice(), [Report::LaunchFailed { .. }]));
    }

    #[test]
    fn signal_command_stops_the_child_group() {
        let tmp = TempDir::new("runner");
        let connection = Launcher::Thread
            .start(&request(&tmp.0, "echo up; exec sleep 30"))
            .unwrap();
        let control = Control::new(connection.input);
        let mut output = connection.output;
        assert!(matches!(
            read_report(output.as_mut()),
            Some(Report::Started { .. })
        ));
        assert!(control.signal(libc::SIGTERM));
        let mut exited = None;
        while let Some(report) = read_report(output.as_mut()) {
            if let Report::Exited { signal, .. } = report {
                exited = signal;
                break;
            }
        }
        assert_eq!(exited.as_deref(), Some("SIGTERM"));
    }

    #[test]
    fn closing_the_control_input_does_not_stop_the_child() {
        // Coordinatorが終了して入力が閉じても、子プロセスは止めずに結果を確定する。
        let tmp = TempDir::new("runner");
        let connection = Launcher::Thread
            .start(&request(&tmp.0, "sleep 0.3; echo done"))
            .unwrap();
        drop(connection.input);
        let reports = collect(Connection {
            input: Box::new(std::io::sink()),
            output: connection.output,
            process: None,
        });
        assert!(matches!(
            reports.last(),
            Some(Report::Exited {
                exit_code: Some(0),
                ..
            })
        ));
        assert!(tmp.0.join("result.json").exists());
    }

    #[test]
    fn request_debug_hides_environment() {
        let tmp = TempDir::new("runner");
        let text = format!("{:?}", request(&tmp.0, "true"));
        assert!(!text.contains("hunter2"));
        assert!(!text.contains("PATH"));
    }
}
