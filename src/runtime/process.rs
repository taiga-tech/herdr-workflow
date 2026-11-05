//! 子プロセスの起動、識別、停止(docs/specs/persistence.md#process-identity、
//! docs/specs/execution.md「プロセス停止」)。
//!
//! 子プロセスは新しいプロセスグループの先頭として起動する。停止信号はグループへ
//! 送るが、先頭プロセスを回収(reap)する前に限る。回収前はPIDとグループIDが
//! 解放されないため、別プロセスへの誤送信を避けられる。

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// 起動に必要な値。環境変数は継承分を含む完全な集合を渡す。
#[derive(Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<OsString, OsString>,
}

impl std::fmt::Debug for LaunchSpec {
    // 環境変数全体は秘密値を含みうるため表示しない。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchSpec")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("env", &format_args!("<{} variables>", self.env.len()))
            .finish()
    }
}

/// `launch.json`へ記録するプロセスの識別情報。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_time: Option<u64>,
    pub pgid: Option<u32>,
    pub boot_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IdentityMatch {
    /// 4項目すべて一致。稼働中として扱う。
    Same,
    /// PIDがない、ブート識別子が異なる、またはPIDが再利用された。
    Absent,
    /// OS情報を取得できない、または記録が欠けている。
    Unknown,
}

#[cfg(target_os = "linux")]
fn read_boot_id() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(not(target_os = "linux"))]
fn read_boot_id() -> Option<String> {
    None
}

/// `/proc/<pid>/stat`の`starttime`(22番目)。プロセスが存在しなければ`Err(NotFound)`。
#[cfg(target_os = "linux")]
fn read_start_time(pid: u32) -> io::Result<Option<u64>> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    Ok(parse_start_time(&stat))
}

#[cfg(not(target_os = "linux"))]
fn read_start_time(pid: u32) -> io::Result<Option<u64>> {
    if process_exists(pid) {
        Ok(None)
    } else {
        Err(io::Error::from(io::ErrorKind::NotFound))
    }
}

/// comm欄は空白や括弧を含みうるため、最後の`)`以降を分割する。
fn parse_start_time(stat: &str) -> Option<u64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    // rest[0]がstate(3番目)なので、22番目は19番目の要素。
    rest.split_whitespace().nth(19)?.parse().ok()
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn process_exists(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: 信号0は送信せず存在と権限だけを確認する。
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn current_pgid(pid: u32) -> Option<u32> {
    let pid = libc::pid_t::try_from(pid).ok()?;
    // SAFETY: getpgid はメモリへ触れず、失敗時は-1を返す。
    let pgid = unsafe { libc::getpgid(pid) };
    u32::try_from(pgid).ok()
}

/// 現在のOS情報から識別情報を取得する。プロセスがなければ`None`。
#[must_use]
pub fn observe(pid: u32) -> Option<ProcessIdentity> {
    let start_time = match read_start_time(pid) {
        Ok(start_time) => start_time,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(_) => None,
    };
    Some(ProcessIdentity {
        pid,
        start_time,
        pgid: current_pgid(pid),
        boot_id: read_boot_id(),
    })
}

/// 記録した識別情報と現在のOS情報を比べる(判定表は persistence.md)。
#[must_use]
pub fn compare(recorded: &ProcessIdentity, current: Option<&ProcessIdentity>) -> IdentityMatch {
    let current_boot = read_boot_id();
    compare_with_boot(recorded, current, current_boot.as_deref())
}

fn compare_with_boot(
    recorded: &ProcessIdentity,
    current: Option<&ProcessIdentity>,
    current_boot: Option<&str>,
) -> IdentityMatch {
    if let (Some(recorded_boot), Some(current_boot)) = (&recorded.boot_id, current_boot)
        && recorded_boot != current_boot
    {
        return IdentityMatch::Absent;
    }
    let Some(current) = current else {
        return IdentityMatch::Absent;
    };
    let (Some(recorded_start), Some(recorded_pgid), Some(_)) =
        (recorded.start_time, recorded.pgid, &recorded.boot_id)
    else {
        return IdentityMatch::Unknown;
    };
    let (Some(current_start), Some(current_pgid)) = (current.start_time, current.pgid) else {
        return IdentityMatch::Unknown;
    };
    if current_boot.is_none() {
        return IdentityMatch::Unknown;
    }
    if recorded_start == current_start && recorded_pgid == current_pgid {
        IdentityMatch::Same
    } else {
        IdentityMatch::Absent
    }
}

/// `/proc/<pid>/stat`の親PID(4番目)。
#[cfg(target_os = "linux")]
fn parent_pid(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// 終了済みで回収待ちのプロセスか(Linuxのみ判定し、他は`false`)。
fn is_zombie(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                let rest = stat[stat.rfind(')')? + 1..].to_string();
                rest.split_whitespace().next().map(|state| state == "Z")
            })
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

/// `root`の子孫プロセス(自身を含まない)の識別情報。Linux以外では空を返す。
#[cfg(target_os = "linux")]
#[must_use]
pub fn descendants(root: u32) -> Vec<ProcessIdentity> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut parents = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            && let Some(ppid) = parent_pid(&stat)
        {
            parents.push((pid, ppid));
        }
    }
    let mut found = Vec::new();
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for (pid, ppid) in &parents {
            if *ppid == parent
                && !found
                    .iter()
                    .any(|known: &ProcessIdentity| known.pid == *pid)
            {
                if let Some(identity) = observe(*pid) {
                    found.push(identity);
                }
                frontier.push(*pid);
            }
        }
    }
    found
}

#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn descendants(_root: u32) -> Vec<ProcessIdentity> {
    Vec::new()
}

/// 記録した識別情報と一致する場合だけ、そのPIDへ信号を送る。
pub fn signal_if_same(identity: &ProcessIdentity, signal: libc::c_int) -> bool {
    if compare(identity, observe(identity.pid).as_ref()) != IdentityMatch::Same
        || is_zombie(identity.pid)
    {
        return false;
    }
    let Ok(pid) = libc::pid_t::try_from(identity.pid) else {
        return false;
    };
    // SAFETY: 直前に起動時刻・グループ・ブート識別子を照合したプロセスへ送る。
    unsafe { libc::kill(pid, signal) == 0 }
}

/// 起動済みプロセスへの信号送信口。回収前に限りグループへ送る。
#[derive(Debug, Clone)]
pub struct ProcessHandle {
    pid: u32,
    reaped: Arc<Mutex<bool>>,
}

impl ProcessHandle {
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// プロセスグループへ信号を送る。先頭プロセスを回収済みなら送らず`false`。
    pub fn signal_group(&self, signal: libc::c_int) -> bool {
        let reaped = self
            .reaped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *reaped {
            return false;
        }
        let Ok(pgid) = libc::pid_t::try_from(self.pid) else {
            return false;
        };
        // SAFETY: 先頭プロセスは未回収のため、このグループIDは自分が起動したグループを指す。
        unsafe { libc::killpg(pgid, signal) == 0 }
    }
}

/// 起動したプロセスと、その出力・終了待ちの口。
pub struct Spawned {
    pub identity: ProcessIdentity,
    pub handle: ProcessHandle,
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
    pub waiter: Waiter,
}

/// 先頭プロセスの終了を待つ。終了を検出してから回収までの間、信号送信と排他する。
pub struct Waiter {
    child: Child,
    reaped: Arc<Mutex<bool>>,
}

impl Waiter {
    /// 終了まで待って回収する。
    pub fn wait(mut self) -> io::Result<ExitStatus> {
        let pid = libc::id_t::try_from(self.child.id()).map_err(io::Error::other)?;
        loop {
            // SAFETY: siginfoは出力専用のゼロ初期化領域。WNOWAITで回収はしない。
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid,
                    &raw mut info,
                    libc::WEXITED | libc::WNOWAIT,
                )
            };
            if result == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        let mut reaped = self
            .reaped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let status = self.child.wait();
        *reaped = true;
        status
    }
}

/// 新しいプロセスグループで起動する。標準入力は閉じる。
pub fn spawn(spec: &LaunchSpec) -> io::Result<Spawned> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(&spec.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn()?;
    let pid = child.id();
    let identity = observe(pid).unwrap_or(ProcessIdentity {
        pid,
        start_time: None,
        pgid: None,
        boot_id: read_boot_id(),
    });
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("stdout pipe missing"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("stderr pipe missing"))?;
    let reaped = Arc::new(Mutex::new(false));
    Ok(Spawned {
        identity,
        handle: ProcessHandle {
            pid,
            reaped: Arc::clone(&reaped),
        },
        stdout,
        stderr,
        waiter: Waiter { child, reaped },
    })
}

/// 終了状態の表現(`AttemptOutcome`へ写す前の値)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    Signal(String),
}

#[must_use]
pub fn classify_exit(status: ExitStatus) -> Exit {
    if let Some(code) = status.code() {
        Exit::Code(code)
    } else if let Some(signal) = status.signal() {
        Exit::Signal(signal_name(signal))
    } else {
        Exit::Signal("unknown".to_string())
    }
}

#[must_use]
pub fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGHUP => "SIGHUP".to_string(),
        libc::SIGINT => "SIGINT".to_string(),
        libc::SIGQUIT => "SIGQUIT".to_string(),
        libc::SIGABRT => "SIGABRT".to_string(),
        libc::SIGKILL => "SIGKILL".to_string(),
        libc::SIGSEGV => "SIGSEGV".to_string(),
        libc::SIGPIPE => "SIGPIPE".to_string(),
        libc::SIGTERM => "SIGTERM".to_string(),
        other => format!("signal {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn sh(script: &str) -> LaunchSpec {
        LaunchSpec {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            cwd: std::env::temp_dir(),
            env: std::env::vars_os().collect(),
        }
    }

    #[test]
    fn start_time_parser_handles_parentheses_in_comm() {
        let stat = "123 (a) b (c)) S 1 123 123 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 98765 0 0";
        assert_eq!(parse_start_time(stat), Some(98765));
    }

    #[test]
    fn spawned_process_leads_its_own_group_and_reports_exit_code() {
        let mut spawned = spawn(&sh("echo out; echo err >&2; exit 3")).unwrap();
        assert_eq!(spawned.identity.pgid, Some(spawned.identity.pid));
        let mut out = String::new();
        spawned.stdout.read_to_string(&mut out).unwrap();
        let status = spawned.waiter.wait().unwrap();
        assert_eq!(out, "out\n");
        assert_eq!(classify_exit(status), Exit::Code(3));
        // 回収後はグループへ信号を送らない。
        assert!(!spawned.handle.signal_group(libc::SIGTERM));
    }

    #[test]
    fn signal_group_stops_the_whole_group() {
        let spawned = spawn(&sh("sleep 30 & sleep 30; wait")).unwrap();
        assert!(spawned.handle.signal_group(libc::SIGTERM));
        let status = spawned.waiter.wait().unwrap();
        assert_eq!(classify_exit(status), Exit::Signal("SIGTERM".to_string()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn identity_of_live_process_matches_and_absent_after_exit() {
        let spawned = spawn(&sh("sleep 5")).unwrap();
        let recorded = spawned.identity.clone();
        assert!(recorded.start_time.is_some());
        assert_eq!(
            compare(&recorded, observe(recorded.pid).as_ref()),
            IdentityMatch::Same
        );
        spawned.handle.signal_group(libc::SIGKILL);
        spawned.waiter.wait().unwrap();
        assert_eq!(
            compare(&recorded, observe(recorded.pid).as_ref()),
            IdentityMatch::Absent
        );
    }

    #[test]
    fn identity_comparison_table() {
        let recorded = ProcessIdentity {
            pid: 10,
            start_time: Some(100),
            pgid: Some(10),
            boot_id: Some("boot-a".to_string()),
        };
        let same = recorded.clone();
        assert_eq!(
            compare_with_boot(&recorded, Some(&same), Some("boot-a")),
            IdentityMatch::Same
        );
        assert_eq!(
            compare_with_boot(&recorded, Some(&same), Some("boot-b")),
            IdentityMatch::Absent
        );
        assert_eq!(
            compare_with_boot(&recorded, None, Some("boot-a")),
            IdentityMatch::Absent
        );
        let reused = ProcessIdentity {
            start_time: Some(200),
            ..recorded.clone()
        };
        assert_eq!(
            compare_with_boot(&recorded, Some(&reused), Some("boot-a")),
            IdentityMatch::Absent
        );
        let incomplete = ProcessIdentity {
            start_time: None,
            ..recorded.clone()
        };
        assert_eq!(
            compare_with_boot(&incomplete, Some(&same), Some("boot-a")),
            IdentityMatch::Unknown
        );
        assert_eq!(
            compare_with_boot(&recorded, Some(&same), None),
            IdentityMatch::Unknown
        );
    }

    #[test]
    fn launch_failure_is_an_error() {
        let mut spec = sh("true");
        spec.program = "/nonexistent/program".to_string();
        assert!(spawn(&spec).is_err());
    }
}
