//! 検証済みの設定と入力値から、Attemptを起動するための定義を組み立てる。
//!
//! `${inputs.*}`は計画時に置換済みの値を使い、実行時変数`${worktree.path}`は
//! Runの対象checkoutで置き換える。秘密値は名前だけを保持し、起動直前に解決する。

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use regex::Regex;

use crate::config::model::{
    DependsOn, EnvValue, LogStream, OnDependencyLost, PortValue, TaskDef, TaskId, TaskLifecycle,
    WaitFor, WorkflowSpec,
};
use crate::config::substitute::ResolvedValues;
use crate::config::{secrets, validate};
use crate::runtime::readiness::{self, HttpTarget};

/// `stop.graceSeconds`を省略したときの猶予。
pub const DEFAULT_GRACE_SECONDS: u64 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvOp {
    Set(String),
    Unset,
    Secret(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Monitor {
    pub interval: Duration,
    pub failure_threshold: u32,
}

#[derive(Debug, Clone)]
pub enum Probe {
    Http { target: HttpTarget, status: u16 },
    Tcp { host: String, port: u16 },
}

impl Probe {
    /// 起動前のポート使用確認に使う、loopbackの対象。
    #[must_use]
    pub fn loopback_endpoint(&self) -> Option<(&str, u16)> {
        let (host, port) = match self {
            Self::Http { target, .. } => (target.host.as_str(), target.port),
            Self::Tcp { host, port } => (host.as_str(), *port),
        };
        readiness::is_loopback_host(host).then_some((host, port))
    }

    #[must_use]
    pub fn check(&self, timeout: Duration) -> bool {
        match self {
            Self::Http { target, status } => readiness::http_probe(target, *status, timeout),
            Self::Tcp { host, port } => readiness::tcp_probe(host, *port, timeout),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Readiness {
    /// `waitFor`なし。起動を確認した時点でreadyとする。
    Started,
    Poll {
        probe: Probe,
        timeout: Duration,
        interval: Duration,
        successes: u32,
        monitor: Option<Monitor>,
    },
    Log {
        stream: LogStream,
        pattern: Regex,
        timeout: Duration,
    },
}

#[derive(Debug, Clone)]
pub struct PreparedTask {
    pub id: TaskId,
    pub lifecycle: TaskLifecycle,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// `defaults.env`、タスクの`env`の順に適用する。
    pub env_ops: Vec<(String, EnvOp)>,
    pub timeout: Option<Duration>,
    pub readiness: Readiness,
    pub grace: Duration,
    pub locks: Vec<String>,
    pub max_attempts: u32,
    pub retry_delay: Duration,
    pub depends_on: Vec<DependsOn>,
    pub on_dependency_lost: OnDependencyLost,
}

impl PreparedTask {
    /// 表示用のコマンド概要。秘密値は含まない(環境変数は表示しない)。
    #[must_use]
    pub fn command_summary(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[must_use]
    pub fn secret_names(&self) -> BTreeSet<&str> {
        self.env_ops
            .iter()
            .filter_map(|(_, op)| match op {
                EnvOp::Secret(name) => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PrepareError {
    #[error("{path}: runtime variable {raw} is not available in this run")]
    UnsupportedRuntimeVariable { path: String, raw: String },
    #[error("{path}: missing resolved value")]
    MissingValue { path: String },
    #[error("task {0:?} is not a command task")]
    NotCommand(TaskId),
    #[error("tasks.{task}.waitFor.url: {message}")]
    InvalidUrl { task: String, message: String },
    #[error("tasks.{task}.waitFor.port: invalid port {value:?}")]
    InvalidPort { task: String, value: String },
    #[error("tasks.{task}.waitFor.pattern: {message}")]
    InvalidPattern { task: String, message: String },
    #[error("cwd {0} is not an absolute path")]
    RelativeCwd(String),
}

/// 実行時変数を置き換える。対応していない`${worktree.*}`はエラーにする。
fn apply_runtime(path: &str, value: &str, checkout: &Path) -> Result<String, PrepareError> {
    let replaced = value.replace("${worktree.path}", &checkout.to_string_lossy());
    if let Some(start) = replaced.find("${worktree.") {
        let end = replaced[start..]
            .find('}')
            .map_or(replaced.len(), |end| start + end + 1);
        return Err(PrepareError::UnsupportedRuntimeVariable {
            path: path.to_string(),
            raw: replaced[start..end].to_string(),
        });
    }
    Ok(replaced)
}

struct Values<'a> {
    resolved: &'a ResolvedValues,
    checkout: &'a Path,
}

impl Values<'_> {
    fn get(&self, path: &str) -> Result<String, PrepareError> {
        let value = self
            .resolved
            .0
            .get(path)
            .ok_or_else(|| PrepareError::MissingValue {
                path: path.to_string(),
            })?;
        apply_runtime(path, value, self.checkout)
    }
}

fn env_ops(
    prefix: &str,
    env: &BTreeMap<String, EnvValue>,
    values: &Values<'_>,
    out: &mut Vec<(String, EnvOp)>,
) -> Result<(), PrepareError> {
    for (name, value) in env {
        let op = match value {
            EnvValue::Value(_) => EnvOp::Set(values.get(&format!("{prefix}.{name}"))?),
            EnvValue::Unset => EnvOp::Unset,
            EnvValue::Secret(secret) => EnvOp::Secret(secret.secret.clone()),
        };
        out.push((name.clone(), op));
    }
    Ok(())
}

/// 1つのcommandタスクを起動定義へ変換する。
pub fn prepare_task(
    spec: &WorkflowSpec,
    id: &TaskId,
    resolved: &ResolvedValues,
    checkout: &Path,
) -> Result<PreparedTask, PrepareError> {
    let values = Values { resolved, checkout };
    let Some(TaskDef::Command {
        lifecycle,
        argv,
        script,
        env,
        depends_on,
        timeout_seconds,
        wait_for,
        stop,
        on_dependency_lost,
        locks,
        retry,
        ..
    }) = spec.tasks.get(id)
    else {
        return Err(PrepareError::NotCommand(id.clone()));
    };
    let task = id.0.as_str();
    let (program, args) = if let Some(argv) = argv {
        let mut resolved_argv = Vec::with_capacity(argv.len());
        for index in 0..argv.len() {
            resolved_argv.push(values.get(&format!("tasks.{task}.argv.{index}"))?);
        }
        let mut iter = resolved_argv.into_iter();
        let program = iter.next().unwrap_or_default();
        (program, iter.collect())
    } else {
        // scriptは値置換を行わない(execution.md#script)。
        let script = script.as_ref().ok_or_else(|| PrepareError::MissingValue {
            path: format!("tasks.{task}.argv"),
        })?;
        let mut iter = script.shell.iter().cloned();
        let program = iter.next().unwrap_or_default();
        let mut args: Vec<String> = iter.collect();
        args.push(script.source.clone());
        (program, args)
    };

    let defaults = spec.defaults.as_ref();
    let cwd = match defaults.and_then(|defaults| defaults.cwd.as_ref()) {
        Some(_) => PathBuf::from(values.get("defaults.cwd")?),
        None => checkout.to_path_buf(),
    };
    if !cwd.is_absolute() {
        return Err(PrepareError::RelativeCwd(cwd.display().to_string()));
    }

    let mut ops = Vec::new();
    if let Some(defaults) = defaults {
        env_ops("defaults.env", &defaults.env, &values, &mut ops)?;
    }
    env_ops(&format!("tasks.{task}.env"), env, &values, &mut ops)?;

    let timeout = match lifecycle {
        TaskLifecycle::Job => timeout_seconds
            .or_else(|| defaults.and_then(|defaults| defaults.job_timeout_seconds))
            .map(Duration::from_secs),
        TaskLifecycle::Service => timeout_seconds.map(Duration::from_secs),
    };

    let readiness = match wait_for.as_deref() {
        None => Readiness::Started,
        Some(WaitFor::Http {
            status,
            timeout_seconds,
            interval_milliseconds,
            consecutive_successes,
            monitor,
            ..
        }) => {
            let url = values.get(&format!("tasks.{task}.waitFor.url"))?;
            let target =
                readiness::parse_http_url(&url).map_err(|error| PrepareError::InvalidUrl {
                    task: task.to_string(),
                    message: error.to_string(),
                })?;
            Readiness::Poll {
                probe: Probe::Http {
                    target,
                    status: *status,
                },
                timeout: Duration::from_secs(*timeout_seconds),
                interval: Duration::from_millis(*interval_milliseconds),
                successes: *consecutive_successes,
                monitor: monitor.map(|monitor| Monitor {
                    interval: Duration::from_secs(monitor.interval_seconds),
                    failure_threshold: monitor.failure_threshold,
                }),
            }
        }
        Some(WaitFor::Tcp {
            port,
            timeout_seconds,
            interval_milliseconds,
            consecutive_successes,
            monitor,
            ..
        }) => {
            let host = values.get(&format!("tasks.{task}.waitFor.host"))?;
            let port_text = match port {
                PortValue::Number(number) => number.to_string(),
                PortValue::Template(_) => values.get(&format!("tasks.{task}.waitFor.port"))?,
            };
            let port = port_text
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| PrepareError::InvalidPort {
                    task: task.to_string(),
                    value: port_text.clone(),
                })?;
            Readiness::Poll {
                probe: Probe::Tcp { host, port },
                timeout: Duration::from_secs(*timeout_seconds),
                interval: Duration::from_millis(*interval_milliseconds),
                successes: *consecutive_successes,
                monitor: monitor.map(|monitor| Monitor {
                    interval: Duration::from_secs(monitor.interval_seconds),
                    failure_threshold: monitor.failure_threshold,
                }),
            }
        }
        Some(WaitFor::Log {
            stream,
            pattern,
            timeout_seconds,
        }) => Readiness::Log {
            stream: *stream,
            pattern: Regex::new(pattern).map_err(|error| PrepareError::InvalidPattern {
                task: task.to_string(),
                message: error.to_string(),
            })?,
            timeout: Duration::from_secs(*timeout_seconds),
        },
    };

    let max_attempts = match (lifecycle, retry) {
        (TaskLifecycle::Job, Some(retry)) => retry.max_attempts,
        (TaskLifecycle::Job, None) => spec.execution.retries.saturating_add(1),
        (TaskLifecycle::Service, _) => 1,
    };

    Ok(PreparedTask {
        id: id.clone(),
        lifecycle: *lifecycle,
        program,
        args,
        cwd,
        env_ops: ops,
        timeout,
        readiness,
        grace: Duration::from_secs(
            stop.and_then(|stop| stop.grace_seconds)
                .unwrap_or(DEFAULT_GRACE_SECONDS),
        ),
        locks: locks.iter().map(|lock| lock.0.clone()).collect(),
        max_attempts: max_attempts.max(1),
        retry_delay: Duration::from_secs(retry.map_or(0, |retry| retry.delay_seconds)),
        depends_on: depends_on.clone(),
        on_dependency_lost: on_dependency_lost.unwrap_or(OnDependencyLost::Stop),
    })
}

/// 起動元環境へ`env_ops`を適用し、子プロセスへ渡す完全な環境を作る。秘密値は
/// `resolve`で解決し、マスク対象として返す。
pub fn build_env<F>(
    base: &BTreeMap<OsString, OsString>,
    ops: &[(String, EnvOp)],
    mut resolve: F,
) -> Result<(BTreeMap<OsString, OsString>, Vec<String>), secrets::SecretsError>
where
    F: FnMut(&str) -> Result<secrets::SecretValue, secrets::SecretsError>,
{
    let mut env = base.clone();
    let mut masked = Vec::new();
    for (name, op) in ops {
        // HERDR_*は検証で拒否済みだが、念のため設定ファイル由来の変更を適用しない。
        if name.starts_with(validate::RESERVED_ENV_PREFIX) {
            continue;
        }
        match op {
            EnvOp::Set(value) => {
                env.insert(OsString::from(name), OsString::from(value));
            }
            EnvOp::Unset => {
                env.remove(&OsString::from(name));
            }
            EnvOp::Secret(secret) => {
                let value = resolve(secret)?;
                masked.push(value.expose().to_string());
                env.insert(OsString::from(name), OsString::from(value.expose()));
            }
        }
    }
    Ok((env, masked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load, substitute};

    fn example() -> (WorkflowSpec, ResolvedValues) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflow.yaml");
        let spec = load::load_file(path).unwrap();
        let provided = BTreeMap::from([(
            crate::config::model::InputId("branch".to_string()),
            "feature/x".to_string(),
        )]);
        let inputs = substitute::resolve_inputs(&spec, &provided).unwrap();
        let values = substitute::substitute_all(&spec, &inputs).unwrap();
        (spec, values)
    }

    #[test]
    fn example_server_is_prepared_with_runtime_values() {
        let (spec, values) = example();
        let task = prepare_task(&spec, &TaskId::from("server"), &values, Path::new("/w")).unwrap();
        assert_eq!(task.program, "mise");
        assert_eq!(task.args.last().unwrap(), "3000");
        assert_eq!(task.cwd, PathBuf::from("/w"));
        assert_eq!(task.grace, Duration::from_secs(10));
        assert_eq!(task.max_attempts, 1);
        assert!(matches!(
            task.readiness,
            Readiness::Poll {
                probe: Probe::Http { .. },
                successes: 2,
                monitor: Some(_),
                ..
            }
        ));
        assert!(task.env_ops.contains(&(
            "NODE_ENV".to_string(),
            EnvOp::Set("development".to_string())
        )));
        assert!(
            task.env_ops
                .contains(&("PORT".to_string(), EnvOp::Set("3000".to_string())))
        );
    }

    #[test]
    fn job_uses_retry_and_default_timeout() {
        let (spec, values) = example();
        let deps = prepare_task(
            &spec,
            &TaskId::from("dependencies"),
            &values,
            Path::new("/w"),
        )
        .unwrap();
        assert_eq!(deps.max_attempts, 2);
        assert_eq!(deps.retry_delay, Duration::from_secs(5));
        assert_eq!(deps.timeout, Some(Duration::from_secs(900)));
        let toolchain =
            prepare_task(&spec, &TaskId::from("toolchain"), &values, Path::new("/w")).unwrap();
        assert_eq!(toolchain.timeout, Some(Duration::from_secs(600)));
        assert_eq!(toolchain.max_attempts, 1);
    }

    #[test]
    fn agent_is_not_a_command() {
        let (spec, values) = example();
        assert!(matches!(
            prepare_task(&spec, &TaskId::from("developer"), &values, Path::new("/w")),
            Err(PrepareError::NotCommand(_))
        ));
    }

    #[test]
    fn unsupported_runtime_variable_is_rejected() {
        assert!(matches!(
            apply_runtime("p", "${worktree.branchName}/x", Path::new("/w")),
            Err(PrepareError::UnsupportedRuntimeVariable { .. })
        ));
        assert_eq!(
            apply_runtime("p", "${worktree.path}/x", Path::new("/w")).unwrap(),
            "/w/x"
        );
    }

    #[test]
    fn build_env_applies_ops_in_order_and_collects_masks() {
        let base = BTreeMap::from([
            (OsString::from("KEEP"), OsString::from("1")),
            (OsString::from("DROP"), OsString::from("1")),
        ]);
        let ops = vec![
            ("A".to_string(), EnvOp::Set("x".to_string())),
            ("A".to_string(), EnvOp::Set("y".to_string())),
            ("DROP".to_string(), EnvOp::Unset),
            ("TOKEN".to_string(), EnvOp::Secret("t".to_string())),
        ];
        let file = secrets::SecretsFile::default();
        let error = build_env(&base, &ops, |name| file.resolve(name, |_| None)).unwrap_err();
        assert!(matches!(error, secrets::SecretsError::Undefined(_)));

        let (env, masked) = build_env(&base, &ops[..3], |_| unreachable!()).unwrap();
        assert_eq!(env.get(&OsString::from("A")), Some(&OsString::from("y")));
        assert!(!env.contains_key(&OsString::from("DROP")));
        assert!(env.contains_key(&OsString::from("KEEP")));
        assert!(masked.is_empty());
    }
}
