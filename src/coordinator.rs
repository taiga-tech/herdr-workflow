//! Coordinator(状態ディレクトリごとに一つ)と制御通信
//! (docs/specs/persistence.md#control-protocol、ADR-0007)。
//!
//! CLIは`coordinator.sock`へUTF-8のJSON Linesで要求を送る。変更系の要求は受付を
//! 状態ストアへ保存してから`ok:true`を返す。Coordinatorは`coordinator.lock`を
//! OSのファイルロックで保持し、稼働中のRunがない状態が続いたら終了する。

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::model::{InputId, TaskId};
use crate::config::plugin::PluginConfig;
use crate::config::{load, secrets};
use crate::runtime::engine::{Control, Engine, RunOutcome, RunPhase};
use crate::runtime::locks::{self, LockAttempt, LockGuard};
use crate::runtime::process::{self, IdentityMatch};
use crate::runtime::session::{self, RunRequest};
use crate::runtime::state_dir::{self, StateDir};
use crate::runtime::store::{RunRecord, Store, StoreRecorder};
use crate::runtime::{engine, logs};
use crate::state::models::{AttemptOutcome, ReadinessState, TaskState};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
const COORDINATOR_LOCK: &str = "coordinator";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub v: u32,
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub v: u32,
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

impl Response {
    fn ok(id: &str, result: Value) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id: id.to_string(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    fn err(id: &str, code: &str, message: impl Into<String>) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id: id.to_string(),
            ok: false,
            result: None,
            error: Some(ErrorBody {
                code: code.to_string(),
                message: message.into(),
            }),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CoordinatorError {
    #[error(transparent)]
    StateDir(#[from] state_dir::StateDirError),
    #[error("another coordinator is running for this state directory")]
    AlreadyRunning,
    #[error(
        "socket path {path} is {len} bytes, longer than the OS limit of {max}; use a shorter HERDR_WORKFLOW_STATE_DIR"
    )]
    SocketPathTooLong {
        path: PathBuf,
        len: usize,
        max: usize,
    },
    #[error("coordinator I/O error: {0}")]
    Io(String),
    #[error("could not connect to the coordinator: {0}")]
    Connect(String),
    #[error("{code}: {message}")]
    Remote { code: String, message: String },
    #[error("invalid response from the coordinator: {0}")]
    Protocol(String),
}

fn io_err(error: impl std::fmt::Display) -> CoordinatorError {
    CoordinatorError::Io(error.to_string())
}

/// `sockaddr_un.sun_path`の大きさ(終端NULを含む)。
fn sun_path_capacity() -> usize {
    // SAFETY: ゼロ初期化した構造体の大きさを調べるだけ。
    let addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_path.len()
}

pub fn check_socket_path(path: &Path) -> Result<(), CoordinatorError> {
    let len = path.as_os_str().len();
    let max = sun_path_capacity() - 1;
    if len > max {
        return Err(CoordinatorError::SocketPathTooLong {
            path: path.to_path_buf(),
            len,
            max,
        });
    }
    Ok(())
}

/// 接続相手の利用者ID。
#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = libc::socklen_t::try_from(std::mem::size_of::<libc::ucred>()).ok()?;
    // SAFETY: credは出力領域で、lenはその大きさ。
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &raw mut len,
        )
    };
    (result == 0).then_some(cred.uid)
}

#[cfg(not(target_os = "linux"))]
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: uid・gidは出力領域。
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &raw mut uid, &raw mut gid) };
    (result == 0).then_some(uid)
}

/// 1行を上限付きで読む。上限超過は`Err(true)`、接続終了は`Ok(None)`。
fn read_line_limited(reader: &mut impl BufRead) -> Result<Option<String>, bool> {
    let mut buffer = Vec::new();
    let limit = u64::try_from(MAX_LINE_BYTES + 1).unwrap_or(u64::MAX);
    let read = reader
        .take(limit)
        .read_until(b'\n', &mut buffer)
        .map_err(|_| false)?;
    if read == 0 {
        return Ok(None);
    }
    if buffer.last() != Some(&b'\n') {
        if buffer.len() > MAX_LINE_BYTES {
            return Err(true);
        }
    } else {
        buffer.pop();
    }
    String::from_utf8(buffer).map(Some).map_err(|_| false)
}

// ---- 実行中Runの管理 ----

struct ActiveRun {
    control: Sender<Control>,
}

struct Shared {
    state: StateDir,
    runs: Mutex<HashMap<String, ActiveRun>>,
    last_activity: Mutex<Instant>,
    shutdown: AtomicBool,
}

impl Shared {
    fn touch(&self) {
        *self
            .last_activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartParams {
    config_path: PathBuf,
    workflow: String,
    #[serde(default)]
    inputs: Vec<(String, String)>,
    checkout: PathBuf,
    target: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    plugin_config: Option<Value>,
    #[serde(default)]
    bootstrap_only: bool,
    #[serde(default)]
    herdr: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunParams {
    run_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResumeParams {
    run_id: String,
    #[serde(default)]
    retry_task: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

fn load_secrets_from_env(env: &BTreeMap<String, String>) -> session::SecretsLoader {
    let dir = env
        .get(crate::config::plugin::CONFIG_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    Arc::new(move || match &dir {
        Some(dir) => secrets::load_from_dir(dir),
        None => Ok(secrets::SecretsFile::default()),
    })
}

fn env_map(env: &BTreeMap<String, String>) -> engine::EnvMap {
    env.iter()
        .map(|(key, value)| (OsString::from(key), OsString::from(value)))
        .collect()
}

struct StartSpec {
    bootstrap_only: bool,
    herdr: bool,
    record: RunRecord,
    workflow: String,
    plugin_config: PluginConfig,
    env: BTreeMap<String, String>,
    initial: BTreeMap<TaskId, (crate::state::models::TaskRuntimeState, Option<String>)>,
    is_new: bool,
}

/// 設定を検証して実行構成を作り、受付を保存してからエンジンを開始する。
fn invalid(error: impl std::fmt::Display) -> ErrorBody {
    ErrorBody {
        code: "invalid-config".to_string(),
        message: error.to_string(),
    }
}

fn herdr_error(error: impl std::fmt::Display) -> ErrorBody {
    ErrorBody {
        code: "herdr".to_string(),
        message: error.to_string(),
    }
}

/// Herdrによる画面構築とAgent起動(@layout.create)。初期化の成功後に一度だけ呼ばれる。
#[allow(clippy::too_many_arguments)]
fn layout_hook(
    socket: PathBuf,
    workspace_id: String,
    spec: crate::config::model::WorkflowSpec,
    plan_workspace: crate::plan::compile::PlannedWorkspace,
    labels: BTreeMap<String, String>,
    checkout: PathBuf,
    run_id: String,
    state_root: PathBuf,
    agents: Vec<TaskId>,
) -> engine::InitHook {
    Box::new(move || {
        use crate::config::model::PaneView;
        use crate::herdr::{self, HerdrClient, PaneSpec};
        let exe = std::env::var_os("HERDR_WORKFLOW_BIN")
            .map(PathBuf::from)
            .map_or_else(std::env::current_exe, Ok)
            .map_err(|error| error.to_string())?;
        let exe = exe.display().to_string();
        let mut client = HerdrClient::new(socket);
        let mut summary = Vec::new();
        let mut started = BTreeMap::new();
        for (tab, planned) in spec.workspace.tabs.iter().zip(&plan_workspace.tabs) {
            let mut panes = BTreeMap::new();
            let mut agent_panes = Vec::new();
            for pane in &tab.panes {
                let label = labels
                    .get(&format!(
                        "workspace.tabs.{}.panes.{}.label",
                        tab.id.0, pane.id.0
                    ))
                    .cloned()
                    .unwrap_or_else(|| pane.label.clone());
                let follow = |args: &[&str]| {
                    let mut command = vec![exe.clone()];
                    command.extend(args.iter().map(|arg| (*arg).to_string()));
                    Some(command)
                };
                let command = match &pane.view {
                    PaneView::Logs { task } => {
                        follow(&["logs", &run_id, "--task", &task.0, "--follow"])
                    }
                    PaneView::Status => follow(&["status", &run_id, "--watch"]),
                    PaneView::Agent { task } => {
                        agent_panes.push((pane.id.clone(), task.clone()));
                        None
                    }
                    PaneView::Shell => None,
                };
                panes.insert(
                    pane.id.clone(),
                    PaneSpec {
                        label,
                        cwd: checkout.clone(),
                        command,
                        env: BTreeMap::from([(
                            state_dir::STATE_DIR_ENV.to_string(),
                            state_root.display().to_string(),
                        )]),
                    },
                );
            }
            let tab_label = labels
                .get(&format!("workspace.tabs.{}.label", tab.id.0))
                .cloned()
                .unwrap_or_else(|| tab.label.clone());
            let ids = herdr::apply_layout(
                &mut client,
                &workspace_id,
                &tab_label,
                &planned.layout.root,
                &panes,
            )
            .map_err(|error| error.to_string())?;
            summary.push(format!("tab {} ({} panes)", tab.id.0, ids.len()));
            for (pane, task) in agent_panes {
                if !agents.contains(&task) {
                    continue;
                }
                let Some(pane_id) = ids.get(&pane) else {
                    continue;
                };
                let kind = match spec.tasks.get(&task) {
                    Some(crate::config::model::TaskDef::Agent { agent, .. }) => {
                        serde_json::to_value(agent.kind)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_string))
                            .unwrap_or_default()
                    }
                    _ => continue,
                };
                // プロンプトは送らない(設定で明示した場合のみ。初期版は起動だけ)。
                // 1つのAgentの起動失敗はそのTaskの失敗とし、配置済みの画面は残す。
                let result =
                    herdr::start_agent(&mut client, &herdr::agent_name(&task.0), &kind, pane_id)
                        .map(|()| pane_id.clone())
                        .map_err(|error| error.to_string());
                if result.is_ok() {
                    summary.push(format!("agent {} started in {pane_id}", task.0));
                }
                started.insert(task, result);
            }
        }
        let tokens = BTreeMap::from([("flow_state".to_string(), "active".to_string())]);
        if let Err(error) = herdr::report_metadata(&mut client, &workspace_id, &tokens) {
            summary.push(format!("metadata not reported: {error}"));
        }
        Ok(engine::InitOutcome {
            message: format!("@layout.create: {}", summary.join(", ")),
            agents: started,
        })
    })
}

/// `RunRequest`の材料(借用の寿命を呼び出し側に揃えるため所有値で持つ)。
struct OwnedRequest<'a> {
    spec: &'a crate::config::model::WorkflowSpec,
    inputs: &'a BTreeMap<InputId, String>,
    checkout: PathBuf,
    target: Option<TaskId>,
    bootstrap_only: bool,
    herdr: bool,
    plugin_config: &'a PluginConfig,
    env: &'a BTreeMap<String, String>,
    config_dir: Option<PathBuf>,
}

impl OwnedRequest<'_> {
    fn as_request(&self) -> RunRequest<'_> {
        RunRequest {
            spec: self.spec,
            plugin_config: self.plugin_config,
            inputs: self.inputs,
            checkout: &self.checkout,
            target: self.target.clone(),
            bootstrap_only: self.bootstrap_only,
            herdr: self.herdr,
            base_env: env_map(self.env),
            load_secrets: load_secrets_from_env(self.env),
            config_dir: self.config_dir.clone(),
        }
    }
}

fn run_request<'a>(
    spec: &'a crate::config::model::WorkflowSpec,
    inputs: &'a BTreeMap<InputId, String>,
    checkout: PathBuf,
    start: &'a StartSpec,
) -> OwnedRequest<'a> {
    OwnedRequest {
        spec,
        inputs,
        checkout,
        target: start.record.target.clone().map(TaskId),
        bootstrap_only: start.bootstrap_only,
        herdr: start.herdr,
        plugin_config: &start.plugin_config,
        env: &start.env,
        config_dir: start.record.config_path.parent().map(Path::to_path_buf),
    }
}

fn start_engine(shared: &Arc<Shared>, start: StartSpec) -> Result<Value, ErrorBody> {
    let spec = load::load_str(&start.workflow).map_err(invalid)?;
    let inputs: BTreeMap<InputId, String> = start
        .record
        .inputs
        .iter()
        .map(|(key, value)| (InputId(key.clone()), value.clone()))
        .collect();
    let mut start = start;
    let mut workspace_id = start
        .env
        .get("HERDR_WORKSPACE_ID")
        .filter(|id| !id.is_empty())
        .cloned();
    let socket = start
        .env
        .get("HERDR_SOCKET_PATH")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let mut labels = BTreeMap::new();
    if start.herdr {
        let socket = socket
            .clone()
            .ok_or_else(|| herdr_error(crate::herdr::HerdrError::NoSocket))?;
        // 外部操作の前に、作成元のcheckoutで設定・入力・秘密値を検証する。
        let checkout = start.record.checkout.clone();
        session::prepare_run(
            run_request(&spec, &inputs, checkout, &start).as_request(),
            start.record.run_id.clone(),
        )
        .map_err(invalid)?;
        let resolved =
            crate::config::substitute::resolve_inputs(&spec, &inputs).map_err(invalid)?;
        labels = crate::config::substitute::substitute_all(&spec, &resolved)
            .map_err(invalid)?
            .0;
        if start.is_new && spec.worktree.mode == crate::config::model::WorktreeMode::Create {
            let branch = labels
                .get("worktree.branch")
                .cloned()
                .ok_or_else(|| invalid("worktree.branch is required for worktree.mode: create"))?;
            let repo_dir = start
                .record
                .config_path
                .parent()
                .map_or_else(|| start.record.checkout.clone(), Path::to_path_buf);
            let repo = crate::approval::repo_key(&repo_dir);
            // 自分の作成要求から届くworktree.createdを別Runとして扱わない(T12)。
            if let Ok(store) = Store::open(&shared.state.db_path()) {
                let _ = store.accept_once(&format!("pending:{repo}:{branch}"), None);
            }
            let mut client = crate::herdr::HerdrClient::new(socket);
            let label = labels
                .get("workspace.label")
                .cloned()
                .unwrap_or_else(|| branch.clone());
            let created = crate::herdr::create_worktree(
                &mut client,
                &repo_dir,
                &branch,
                labels.get("worktree.base").map(String::as_str),
                &label,
            )
            .map_err(herdr_error)?;
            start.record.checkout = created.path;
            start.record.owned_workspace = Some(created.workspace_id.clone());
            workspace_id = Some(created.workspace_id);
        }
    }
    let checkout = start.record.checkout.clone();
    let mut prepared = session::prepare_run(
        run_request(&spec, &inputs, checkout, &start).as_request(),
        start.record.run_id.clone(),
    )
    .map_err(invalid)?;
    prepared.config.initial = std::mem::take(&mut start.initial);
    if start.herdr {
        let workspace_id = workspace_id.ok_or_else(|| {
            herdr_error("HERDR_WORKSPACE_ID is not set; run inside a Herdr workspace")
        })?;
        prepared.config.on_init_complete = Some(layout_hook(
            socket.clone().unwrap_or_default(),
            workspace_id,
            spec.clone(),
            prepared.config.plan.workspace.clone(),
            labels,
            prepared.checkout.clone(),
            start.record.run_id.clone(),
            shared.state.root().to_path_buf(),
            prepared.agents.clone(),
        ));
        prepared.config.agents = prepared
            .agents
            .iter()
            .map(|task| (task.clone(), crate::herdr::agent_name(&task.0)))
            .collect();
        let observe_socket = socket.unwrap_or_default();
        prepared.config.observe_agent = Some(Arc::new(move |name: &str| {
            let mut client = crate::herdr::HerdrClient::new(observe_socket.clone());
            match crate::herdr::agent_status(&mut client, name) {
                Ok(Some(status)) => serde_json::from_value(Value::String(status)).map_or(
                    engine::AgentProbe::Observed(crate::state::models::AgentObservation::Unknown),
                    engine::AgentProbe::Observed,
                ),
                Ok(None) => engine::AgentProbe::Gone,
                Err(error) => engine::AgentProbe::Unreachable(error.to_string()),
            }
        }));
        // 照会間隔(既定2秒)。診断と試験のために要求元の環境で変更できる。
        if let Some(ms) = start
            .env
            .get("HERDR_WORKFLOW_AGENT_POLL_MS")
            .and_then(|value| value.parse::<u64>().ok())
        {
            prepared.config.agent_poll = Duration::from_millis(ms.max(50));
        }
    }

    let mut store = Store::open(&shared.state.db_path()).map_err(|error| ErrorBody {
        code: "store".to_string(),
        message: error.to_string(),
    })?;
    let mut record = start.record;
    record.plan_hash.clone_from(&prepared.plan_hash);
    record.checkout.clone_from(&prepared.checkout);
    if start.is_new {
        store.insert_run(&record).map_err(|error| ErrorBody {
            code: "store".to_string(),
            message: error.to_string(),
        })?;
    } else {
        store
            .set_run_state(&record.run_id, RunPhase::Queued, RunOutcome::Pending)
            .map_err(|error| ErrorBody {
                code: "store".to_string(),
                message: error.to_string(),
            })?;
    }
    let _ = store.add_event(&record.run_id, "accepted");

    let (control_tx, control_rx) = mpsc::channel();
    let run_id = record.run_id.clone();
    {
        let mut runs = shared
            .runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runs.contains_key(&run_id) {
            return Err(ErrorBody {
                code: "conflict".to_string(),
                message: format!("run {run_id} is already active"),
            });
        }
        runs.insert(
            run_id.clone(),
            ActiveRun {
                control: control_tx,
            },
        );
    }
    let thread_shared = Arc::clone(shared);
    let state = shared.state.clone();
    let config = prepared.config;
    let files = prepared.files;
    let checkout = prepared.checkout;
    let thread_run_id = run_id.clone();
    std::thread::spawn(move || {
        // @files.copy: 初期化タスクより前に、許可されたファイルだけをコピーする。
        if let Some((files, source)) = files {
            match crate::runtime::files::apply(&files, &source, &checkout) {
                Ok(report) => {
                    let _ = store.add_event(
                        &thread_run_id,
                        &format!(
                            "@files.copy: copied [{}], skipped [{}]",
                            report.copied.join(", "),
                            report
                                .skipped
                                .iter()
                                .map(|(path, reason)| format!("{path}: {reason}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    );
                }
                Err(error) => {
                    let _ =
                        store.add_event(&thread_run_id, &format!("@files.copy failed: {error}"));
                    let snapshot = engine::RunSnapshot {
                        run_id: thread_run_id.clone(),
                        phase: RunPhase::Finished,
                        outcome: RunOutcome::Failed,
                        target: config.target.clone(),
                        tasks: config
                            .tasks
                            .iter()
                            .map(|(id, task)| {
                                (
                                    id.clone(),
                                    engine::TaskSnapshot {
                                        kind: task.lifecycle.into(),
                                        runtime: crate::state::models::TaskRuntimeState {
                                            state: TaskState::Skipped,
                                            ..Default::default()
                                        },
                                        detail: Some("files.copy failed".to_string()),
                                    },
                                )
                            })
                            .collect(),
                        updated_at: logs::rfc3339(SystemTime::now()),
                    };
                    let _ = store.save_snapshot(&snapshot);
                    let _ =
                        store.add_event(&thread_run_id, "finished: phase=finished outcome=failed");
                    thread_shared
                        .runs
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&thread_run_id);
                    return;
                }
            }
        }
        let mut recorder = StoreRecorder {
            store: &mut store,
            run_id: thread_run_id.clone(),
            on_notice: Box::new(|_| {}),
        };
        let report = Engine::new(config, state, &mut recorder, control_rx).run();
        drop(recorder);
        let _ = store.add_event(
            &thread_run_id,
            &format!(
                "finished: phase={} outcome={}",
                serde_json::to_value(report.snapshot.phase)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default(),
                serde_json::to_value(report.snapshot.outcome)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default()
            ),
        );
        thread_shared
            .runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&thread_run_id);
        thread_shared.touch();
    });
    Ok(json!({ "runId": run_id, "planHash": record.plan_hash }))
}

fn handle_start(shared: &Arc<Shared>, params: Value) -> Result<Value, ErrorBody> {
    let params: StartParams = serde_json::from_value(params).map_err(|error| ErrorBody {
        code: "invalid-request".to_string(),
        message: error.to_string(),
    })?;
    let plugin_config = match params.plugin_config {
        Some(value) => serde_json::from_value(value).map_err(|error| ErrorBody {
            code: "invalid-config".to_string(),
            message: error.to_string(),
        })?,
        None => PluginConfig::default(),
    };
    let now = logs::rfc3339(SystemTime::now());
    let record = RunRecord {
        run_id: session::new_run_id(),
        created_at: now.clone(),
        updated_at: now,
        plan_hash: String::new(),
        config_path: params.config_path,
        checkout: params.checkout,
        target: params.target,
        phase: RunPhase::Queued,
        outcome: RunOutcome::Pending,
        workflow: params.workflow.clone(),
        inputs: params.inputs,
        coordinator_pid: Some(std::process::id()),
        plugin_config: serde_json::to_string(&plugin_config).unwrap_or_default(),
        owned_workspace: None,
    };
    start_engine(
        shared,
        StartSpec {
            record,
            workflow: params.workflow,
            plugin_config,
            env: params.env,
            initial: BTreeMap::new(),
            is_new: true,
            bootstrap_only: params.bootstrap_only,
            herdr: params.herdr,
        },
    )
}

/// 保存済みの計画で再開する。成功済みのTaskは繰り返さず、状態不明のTaskは
/// 明示した`retry`の対象でない限り起動しない(docs/specs/recovery.md)。
fn handle_resume(shared: &Arc<Shared>, params: Value) -> Result<Value, ErrorBody> {
    let params: ResumeParams = serde_json::from_value(params).map_err(|error| ErrorBody {
        code: "invalid-request".to_string(),
        message: error.to_string(),
    })?;
    let store = Store::open(&shared.state.db_path()).map_err(|error| ErrorBody {
        code: "store".to_string(),
        message: error.to_string(),
    })?;
    let mut record = store.run(&params.run_id).map_err(|error| ErrorBody {
        code: "not-found".to_string(),
        message: error.to_string(),
    })?;
    if !matches!(record.phase, RunPhase::Stopped | RunPhase::Finished) {
        return Err(ErrorBody {
            code: "conflict".to_string(),
            message: format!("run {} is still active", record.run_id),
        });
    }
    let tasks = store.tasks(&record.run_id).map_err(|error| ErrorBody {
        code: "store".to_string(),
        message: error.to_string(),
    })?;
    let plugin_config: PluginConfig = if record.plugin_config.is_empty() {
        PluginConfig::default()
    } else {
        serde_json::from_str(&record.plugin_config).map_err(|error| ErrorBody {
            code: "invalid-config".to_string(),
            message: error.to_string(),
        })?
    };
    let retry_task = params.retry_task.map(TaskId);
    if let Some(task) = &retry_task {
        if !tasks.contains_key(task) {
            return Err(ErrorBody {
                code: "not-found".to_string(),
                message: format!("task {} is not part of run {}", task.0, record.run_id),
            });
        }
        record.target = Some(task.0.clone());
    }
    let mut initial = BTreeMap::new();
    for (id, snapshot) in tasks {
        let mut runtime = snapshot.runtime;
        let keep = runtime.state == TaskState::Succeeded && retry_task.as_ref() != Some(&id);
        let unknown = runtime.state == TaskState::Unknown && retry_task.as_ref() != Some(&id);
        if keep || unknown {
            initial.insert(id, (runtime, snapshot.detail));
            continue;
        }
        runtime.state = TaskState::Waiting;
        runtime.readiness = match snapshot.kind {
            engine::TaskKind::Job => ReadinessState::NotApplicable,
            engine::TaskKind::Service => ReadinessState::Pending,
            engine::TaskKind::Agent => {
                return Err(ErrorBody {
                    code: "unsupported".to_string(),
                    message: format!(
                        "run {} has agent task {}; resume and retry of Herdr runs with agents are not supported yet",
                        record.run_id, id.0
                    ),
                });
            }
        };
        initial.insert(id, (runtime, None));
    }
    let workflow = record.workflow.clone();
    start_engine(
        shared,
        StartSpec {
            record,
            workflow,
            plugin_config,
            env: params.env,
            initial,
            is_new: false,
            bootstrap_only: false,
            herdr: false,
        },
    )
}

fn handle_stop(shared: &Arc<Shared>, params: Value) -> Result<Value, ErrorBody> {
    let params: RunParams = serde_json::from_value(params).map_err(|error| ErrorBody {
        code: "invalid-request".to_string(),
        message: error.to_string(),
    })?;
    let runs = shared
        .runs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(run) = runs.get(&params.run_id) else {
        return Err(ErrorBody {
            code: "not-active".to_string(),
            message: format!("run {} is not active", params.run_id),
        });
    };
    let _ = run.control.send(Control::StopAll);
    drop(runs);
    if let Ok(store) = Store::open(&shared.state.db_path()) {
        let _ = store.add_event(&params.run_id, "stop requested");
    }
    Ok(json!({ "accepted": true }))
}

fn dispatch(shared: &Arc<Shared>, request: Request) -> Response {
    if request.v != PROTOCOL_VERSION {
        return Response::err(
            &request.id,
            "unsupported-version",
            format!(
                "protocol version {} is not supported; expected {PROTOCOL_VERSION}",
                request.v
            ),
        );
    }
    shared.touch();
    let result = match request.method.as_str() {
        "ping" => Ok(json!({
            "pid": std::process::id(),
            "version": crate::version(),
            "activeRuns": shared.runs.lock().map(|runs| runs.len()).unwrap_or(0),
        })),
        "run.start" => handle_start(shared, request.params),
        "run.resume" => handle_resume(shared, request.params),
        "run.stop" => handle_stop(shared, request.params),
        "shutdown" => {
            shared.shutdown.store(true, Ordering::Relaxed);
            Ok(json!({ "accepted": true }))
        }
        other => Err(ErrorBody {
            code: "unknown-method".to_string(),
            message: format!("unknown method {other:?}"),
        }),
    };
    match result {
        Ok(value) => Response::ok(&request.id, value),
        Err(error) => Response::err(&request.id, &error.code, error.message),
    }
}

fn write_response(stream: &mut UnixStream, response: &Response) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(response).map_err(std::io::Error::other)?;
    line.push(b'\n');
    stream.write_all(&line)
}

fn handle_connection(shared: &Arc<Shared>, mut stream: UnixStream) {
    let _ = stream.set_nonblocking(false);
    let current = state_dir::current_euid();
    if peer_uid(&stream) != Some(current) {
        let _ = write_response(
            &mut stream,
            &Response::err("", "forbidden", "peer user does not match the coordinator"),
        );
        return;
    }
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    loop {
        let line = match read_line_limited(&mut reader) {
            Ok(Some(line)) => line,
            Ok(None) => return,
            Err(too_long) => {
                let message = if too_long {
                    "request line exceeds 1 MiB"
                } else {
                    "request is not valid UTF-8"
                };
                let _ = write_response(&mut stream, &Response::err("", "invalid-request", message));
                return;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => dispatch(shared, request),
            Err(error) => Response::err("", "invalid-request", error.to_string()),
        };
        if write_response(&mut stream, &response).is_err() {
            return;
        }
    }
}

/// 前回のCoordinatorが終了した後に残った未完了Runを照合する(T15)。
/// 稼働中と判定したプロセスは再起動も停止もせず、状態不明として記録する。
pub fn reconcile_orphans(state: &StateDir, store: &mut Store) -> Result<usize, CoordinatorError> {
    let runs = store.recent_runs(1000).map_err(io_err)?;
    let mut count = 0;
    for run in runs {
        if matches!(run.phase, RunPhase::Stopped | RunPhase::Finished) {
            continue;
        }
        count += 1;
        let mut tasks = store.tasks(&run.run_id).map_err(io_err)?;
        for (id, task) in &mut tasks {
            let runtime = &mut task.runtime;
            // AgentはHerdrのpaneで動き続けるため、追跡が切れたことだけを記録する。
            if task.kind == engine::TaskKind::Agent {
                if runtime.state == TaskState::Running {
                    runtime.state = TaskState::Stopped;
                    task.detail = Some(
                        "tracking ended by coordinator restart; the agent may still run in its pane"
                            .to_string(),
                    );
                }
                continue;
            }
            match runtime.state {
                TaskState::Starting | TaskState::Running | TaskState::Stopping => {}
                TaskState::Waiting => {
                    runtime.state = TaskState::Cancelled;
                    task.detail = Some("coordinator stopped before start".to_string());
                    continue;
                }
                _ => continue,
            }
            let Some(attempt) = runtime.attempts.last().map(|attempt| attempt.attempt_id.0) else {
                runtime.state = TaskState::Unknown;
                continue;
            };
            let dir = state.attempt_dir(&run.run_id, &id.0, attempt);
            let launch: Option<engine::LaunchFile> = std::fs::read(dir.join("launch.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok());
            let result: Option<engine::ResultFile> = std::fs::read(dir.join("result.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok());
            let identity = launch.as_ref().map(|launch| {
                process::compare(&launch.child, process::observe(launch.child.pid).as_ref())
            });
            let outcome = match (identity, &result) {
                (Some(IdentityMatch::Same), _) => {
                    task.detail = Some(format!(
                        "process {} is still running without a coordinator; not restarted",
                        launch.as_ref().map_or(0, |launch| launch.child.pid)
                    ));
                    AttemptOutcome::Unknown
                }
                (_, Some(result)) => {
                    task.detail = Some("result imported after coordinator restart".to_string());
                    match (&result.exit_code, &result.signal) {
                        (Some(code), _) => AttemptOutcome::ExitCode(*code),
                        (None, Some(signal)) => AttemptOutcome::Signaled(signal.clone()),
                        (None, None) => AttemptOutcome::Unknown,
                    }
                }
                _ => {
                    task.detail = Some("no result recorded; outcome unknown".to_string());
                    AttemptOutcome::Unknown
                }
            };
            if let Some(last) = runtime.attempts.last().cloned() {
                runtime.record_attempt_result(crate::state::models::AttemptResult {
                    outcome: outcome.clone(),
                    ..last
                });
            }
            runtime.state = match (task.kind, &outcome) {
                (engine::TaskKind::Job, AttemptOutcome::ExitCode(0)) => TaskState::Succeeded,
                (_, AttemptOutcome::Unknown) => TaskState::Unknown,
                _ => TaskState::Failed,
            };
            if task.kind == engine::TaskKind::Service {
                runtime.readiness = ReadinessState::Unready;
            }
        }
        let snapshot = engine::RunSnapshot {
            run_id: run.run_id.clone(),
            phase: RunPhase::Stopped,
            outcome: RunOutcome::Unknown,
            target: run.target.clone().map(TaskId),
            tasks,
            updated_at: logs::rfc3339(SystemTime::now()),
        };
        store.save_snapshot(&snapshot).map_err(io_err)?;
        let _ = store.add_event(
            &run.run_id,
            "coordinator restarted; run reconciled as unknown (use resume or retry)",
        );
    }
    Ok(count)
}

static TERMINATE: AtomicBool = AtomicBool::new(false);

extern "C" fn on_terminate(_: libc::c_int) {
    TERMINATE.store(true, Ordering::Relaxed);
}

fn install_terminate_handler() {
    let handler = on_terminate as extern "C" fn(libc::c_int);
    // SAFETY: ハンドラはアトミック変数への書き込みだけを行う(非同期シグナル安全)。
    unsafe {
        libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
        libc::signal(libc::SIGINT, handler as libc::sighandler_t);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

/// Coordinatorを実行する。稼働中のRunがない状態が`idle_exit`続いたら終了する。
pub fn serve(state: &StateDir, idle_exit: Duration) -> Result<(), CoordinatorError> {
    let socket = state.socket_path();
    check_socket_path(&socket)?;
    let mut lock: Vec<LockGuard> = Vec::new();
    match locks::try_lock_all(state.root(), &[COORDINATOR_LOCK.to_string()], &mut lock)
        .map_err(io_err)?
    {
        LockAttempt::Acquired => {}
        LockAttempt::Busy(_) => return Err(CoordinatorError::AlreadyRunning),
    }
    install_terminate_handler();
    {
        let mut store = Store::open(&state.db_path()).map_err(io_err)?;
        reconcile_orphans(state, &mut store)?;
    }
    // ロックを保持しているので、残っているソケットは前回の残骸。
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).map_err(io_err)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
            .map_err(io_err)?;
    }
    listener.set_nonblocking(true).map_err(io_err)?;
    let shared = Arc::new(Shared {
        state: state.clone(),
        runs: Mutex::new(HashMap::new()),
        last_activity: Mutex::new(Instant::now()),
        shutdown: AtomicBool::new(false),
    });
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || handle_connection(&shared, stream));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(io_err(error)),
        }
        let terminating =
            TERMINATE.load(Ordering::Relaxed) || shared.shutdown.load(Ordering::Relaxed);
        if terminating {
            for run in shared
                .runs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
            {
                let _ = run.control.send(Control::StopAll);
            }
        }
        let active = shared.runs.lock().map(|runs| runs.len()).unwrap_or(0);
        let idle_for = shared
            .last_activity
            .lock()
            .map(|at| at.elapsed())
            .unwrap_or_default();
        if active == 0 && (terminating || idle_for >= idle_exit) {
            break;
        }
    }
    let _ = std::fs::remove_file(&socket);
    drop(lock);
    Ok(())
}

// ---- クライアント ----

pub struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
}

impl Client {
    pub fn connect(state: &StateDir) -> Result<Self, CoordinatorError> {
        let socket = state.socket_path();
        check_socket_path(&socket)?;
        let stream = UnixStream::connect(&socket)
            .map_err(|error| CoordinatorError::Connect(error.to_string()))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .map_err(io_err)?;
        let reader = BufReader::new(stream.try_clone().map_err(io_err)?);
        Ok(Self {
            stream,
            reader,
            next_id: 1,
        })
    }

    /// 接続できなければCoordinatorを起動して接続する。
    pub fn connect_or_spawn(state: &StateDir) -> Result<Self, CoordinatorError> {
        if let Ok(client) = Self::connect(state) {
            return Ok(client);
        }
        spawn_coordinator(state)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match Self::connect(state) {
                Ok(client) => return Ok(client),
                Err(error) if Instant::now() >= deadline => return Err(error),
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, CoordinatorError> {
        let id = format!("c{}", self.next_id);
        self.next_id += 1;
        let request = Request {
            v: PROTOCOL_VERSION,
            id: id.clone(),
            method: method.to_string(),
            params,
        };
        let mut line = serde_json::to_vec(&request).map_err(io_err)?;
        line.push(b'\n');
        self.stream.write_all(&line).map_err(io_err)?;
        let response = read_line_limited(&mut self.reader)
            .map_err(|_| CoordinatorError::Protocol("unreadable response".to_string()))?
            .ok_or_else(|| CoordinatorError::Protocol("connection closed".to_string()))?;
        let response: Response = serde_json::from_str(&response)
            .map_err(|error| CoordinatorError::Protocol(error.to_string()))?;
        if response.id != id && !response.id.is_empty() {
            return Err(CoordinatorError::Protocol(
                "response id mismatch".to_string(),
            ));
        }
        if response.ok {
            Ok(response.result.unwrap_or(Value::Null))
        } else {
            let error = response.error.unwrap_or(ErrorBody {
                code: "unknown".to_string(),
                message: String::new(),
            });
            Err(CoordinatorError::Remote {
                code: error.code,
                message: error.message,
            })
        }
    }
}

/// 自分自身の実行ファイルを`coordinator`サブコマンドで、新しいプロセスグループとして起動する。
fn spawn_coordinator(state: &StateDir) -> Result<(), CoordinatorError> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;
    let exe = std::env::var_os("HERDR_WORKFLOW_BIN")
        .map(PathBuf::from)
        .map_or_else(std::env::current_exe, Ok)
        .map_err(io_err)?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(state.root().join("coordinator.log"))
        .map_err(io_err)?;
    let log_err = log.try_clone().map_err(io_err)?;
    std::process::Command::new(exe)
        .arg("coordinator")
        .env(state_dir::STATE_DIR_ENV, state.root())
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .process_group(0)
        .spawn()
        .map_err(io_err)?;
    Ok(())
}

/// 文字列に変換できる環境変数だけを送る(値は保存しない)。
#[must_use]
pub fn current_env() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn herdr_up_creates_worktree_then_layout_and_agent_after_initialization() {
        use crate::herdr::fake::FakeHerdr;
        use crate::runtime::git::test_repo::{init, run};
        use crate::runtime::state_dir::test_support::TempDir;

        let tmp = TempDir::new("coord-herdr");
        let repo = init(&tmp.0.join("repo"));
        std::fs::create_dir_all(repo.join(".herdr")).unwrap();
        let workflow = "version: 1\nname: t\ninputs:\n  branch:\n    type: string\n    required: true\nworktree:\n  mode: create\n  branch: '${inputs.branch}'\n  base: main\nbootstrap:\n  targets: [init]\ntasks:\n  init:\n    type: command\n    lifecycle: job\n    runner: supervised\n    argv: [/bin/sh, -c, 'touch initialized']\n  developer:\n    type: agent\n    agent:\n      kind: claude\n    dependsOn:\n      - task: init\n        condition: succeeded\nworkspace:\n  label: '${inputs.branch}'\n  tabs:\n    - id: dev\n      label: dev\n      layout: { type: grid, columns: 2, rows: 1 }\n      panes:\n        - id: L\n          label: init\n          view: { type: logs, task: init }\n          placement: { column: 1, row: 1 }\n        - id: A\n          label: developer\n          view: { type: agent, task: developer }\n          placement: { column: 2, row: 1 }\n";
        let config = repo.join(".herdr/workflow.yaml");
        std::fs::write(&config, workflow).unwrap();
        // Herdrが作るはずのcheckoutを先に用意し、偽Herdrはそのパスを返す。
        run(
            &repo,
            &["worktree", "add", "-q", "-b", "feature/x", "../wt"],
        );
        let wt = std::fs::canonicalize(tmp.0.join("wt")).unwrap();
        let socket = PathBuf::from(format!("/tmp/hwf-{}.sock", std::process::id()));
        let fake = FakeHerdr::start(socket.clone(), wt.clone());
        fake.agent_statuses.lock().unwrap().extend(
            ["working", "working", "idle", "done"]
                .into_iter()
                .map(str::to_string),
        );

        let shared = Arc::new(Shared {
            state: StateDir::open(tmp.0.join("state")).unwrap(),
            runs: Mutex::new(HashMap::new()),
            last_activity: Mutex::new(Instant::now()),
            shutdown: AtomicBool::new(false),
        });
        let mut env = current_env();
        env.insert(
            "HERDR_SOCKET_PATH".to_string(),
            socket.display().to_string(),
        );
        env.remove("HERDR_WORKSPACE_ID");
        env.insert(
            "HERDR_WORKFLOW_AGENT_POLL_MS".to_string(),
            "100".to_string(),
        );
        let result = handle_start(
            &shared,
            json!({
                "configPath": config,
                "workflow": workflow,
                "inputs": [["branch", "feature/x"]],
                "checkout": repo,
                "target": null,
                "env": env,
                "herdr": true,
            }),
        )
        .unwrap();
        let run_id = result["runId"].as_str().unwrap().to_string();
        let store = Store::open(&shared.state.db_path()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let record = store.run(&run_id).unwrap();
            if matches!(record.phase, RunPhase::Finished | RunPhase::Stopped) {
                assert_eq!(record.outcome, RunOutcome::Succeeded);
                assert_eq!(record.checkout, wt);
                assert_eq!(record.owned_workspace.as_deref(), Some("w9"));
                break;
            }
            assert!(Instant::now() < deadline, "run did not finish");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(wt.join("initialized").exists());
        let methods: Vec<String> = fake
            .methods()
            .into_iter()
            .filter(|method| method != "agent.get")
            .collect();
        assert_eq!(
            methods,
            [
                "worktree.create",
                "layout.apply",
                "agent.start",
                "workspace.report_metadata"
            ]
        );
        // T10: idleやdoneを観測してもTaskの成功へ変換せず、観測値として記録する。
        let events: Vec<String> = store
            .events(&run_id, 0)
            .unwrap()
            .into_iter()
            .map(|(_, _, message)| message)
            .collect();
        let position = |text: &str| events.iter().position(|event| event.contains(text));
        assert!(
            position("[developer] agent working").is_some(),
            "{events:?}"
        );
        assert!(position("[developer] agent idle") > position("[developer] agent working"));
        assert!(position("[developer] agent done") > position("[developer] agent idle"));
        assert!(
            position("agent exited or was replaced").is_some(),
            "{events:?}"
        );
        let tasks = store.tasks(&run_id).unwrap();
        let developer = &tasks[&TaskId::from("developer")];
        assert_eq!(developer.kind, engine::TaskKind::Agent);
        assert_eq!(developer.runtime.state, TaskState::Stopped);
        let requests = fake.requests.lock().unwrap().clone();
        assert_eq!(requests[0]["params"]["branch"], "feature/x");
        assert_eq!(requests[1]["params"]["workspace_id"], "w9");
        let log_pane = &requests[1]["params"]["root"]["first"];
        assert_eq!(log_pane["command"][1], "logs");
        assert_eq!(requests[2]["params"]["name"], "developer");
        assert_eq!(requests[2]["params"]["kind"], "claude");
        // 自分の作成要求は保留記録で識別でき、イベントで別Runを増やさない(T12)。
        let repo_key = crate::approval::repo_key(&repo);
        assert!(store.acceptance_exists(&format!("pending:{repo_key}:feature/x")));
        let _ = std::fs::remove_file(socket);
    }

    #[test]
    fn long_socket_paths_are_rejected() {
        let long = PathBuf::from(format!("/tmp/{}/coordinator.sock", "x".repeat(200)));
        assert!(matches!(
            check_socket_path(&long),
            Err(CoordinatorError::SocketPathTooLong { .. })
        ));
        assert!(check_socket_path(Path::new("/tmp/s/coordinator.sock")).is_ok());
    }

    #[test]
    fn oversized_lines_are_detected() {
        let mut data = vec![b'a'; MAX_LINE_BYTES + 10];
        data.push(b'\n');
        let mut reader = BufReader::new(data.as_slice());
        assert_eq!(read_line_limited(&mut reader), Err(true));
        let mut ok = BufReader::new(&b"{\"v\":1}\nrest"[..]);
        assert_eq!(
            read_line_limited(&mut ok),
            Ok(Some("{\"v\":1}".to_string()))
        );
    }
}
