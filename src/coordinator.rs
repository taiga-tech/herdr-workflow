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
/// 状態不明のTaskの結果を取り込む間隔。
const IMPORT_INTERVAL: Duration = Duration::from_secs(1);
/// ログの総容量を確かめる間隔。
const TOTAL_CHECK_INTERVAL: Duration = Duration::from_secs(60);
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
    runner: crate::runtime::runner::Launcher,
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
    /// resume時に照合する画面の記録と、Agentごとの扱い。
    herdr_resume: Option<crate::herdr::HerdrState>,
    agent_plans: BTreeMap<TaskId, AgentPlan>,
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

/// 画面構築・再開時のAgentの扱い。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentPlan {
    /// まだ起動していない。記録済み(または新しく作った)paneで起動する。
    Start,
    /// 以前起動した。記録と同じpaneで稼働していれば追跡を再開し、起動し直さない。
    AdoptOnly,
    /// `retry --task`で明示された。照合できたpaneで起動し直す。
    Restart,
}

fn agent_kind(spec: &crate::config::model::WorkflowSpec, task: &TaskId) -> Option<String> {
    match spec.tasks.get(task) {
        Some(crate::config::model::TaskDef::Agent { agent, .. }) => {
            serde_json::to_value(agent.kind)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
        }
        _ => None,
    }
}

/// Herdrによる画面構築とAgent起動(@layout.create)。初期化の成功後に一度だけ呼ばれる。
/// `resume`があれば画面を作り直さず(T20)、記録したpaneを照合してから使う(T17)。
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
    plans: BTreeMap<TaskId, AgentPlan>,
    resume: Option<crate::herdr::HerdrState>,
) -> engine::InitHook {
    Box::new(move || {
        use crate::config::model::PaneView;
        use crate::herdr::{self, HerdrClient, HerdrState, PaneSpec};
        use engine::AgentStart;
        let mut client = HerdrClient::new(socket);
        let mut summary = Vec::new();
        let resuming = resume.is_some();
        let state = if let Some(state) = resume {
            summary.push("existing layout kept".to_string());
            state
        } else {
            let exe = std::env::var_os("HERDR_WORKFLOW_BIN")
                .map(PathBuf::from)
                .map_or_else(std::env::current_exe, Ok)
                .map_err(|error| error.to_string())?;
            let exe = exe.display().to_string();
            let mut state = HerdrState {
                workspace_id: workspace_id.clone(),
                ..HerdrState::default()
            };
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
                            follow(&["logs", &run_id, "--task", &task.0, "--follow", "--keep"])
                        }
                        PaneView::Status => follow(&["status", &run_id, "--watch", "--keep"]),
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
                for (pane, herdr_pane) in &ids {
                    state
                        .panes
                        .insert(format!("{}/{}", tab.id.0, pane.0), herdr_pane.clone());
                }
                for (pane, task) in agent_panes {
                    if let Some(herdr_pane) = ids.get(&pane) {
                        state.agent_panes.insert(task.0.clone(), herdr_pane.clone());
                    }
                }
            }
            // resumeで照合できるよう、作成したpaneを状態ストアへ記録する。
            if let Ok(json) = serde_json::to_string(&state)
                && let Ok(store) = Store::open(&state_root.join("state.db"))
            {
                let _ = store.set_herdr_state(&run_id, &json);
            }
            state
        };

        let mut results = BTreeMap::new();
        for (task, plan) in &plans {
            let Some(pane) = state.agent_panes.get(&task.0).cloned() else {
                results.insert(
                    task.clone(),
                    AgentStart::Failed("no agent pane is recorded for this task".to_string()),
                );
                continue;
            };
            let Some(kind) = agent_kind(&spec, task) else {
                continue;
            };
            let name = herdr::agent_name(&task.0);
            if resuming {
                // 古いpane IDを照合せずに操作しない(T17)。
                match herdr::pane_exists(&mut client, &pane) {
                    Ok(true) => {}
                    Ok(false) => {
                        results.insert(
                            task.clone(),
                            AgentStart::Failed(format!(
                                "recorded pane {pane} no longer exists; the layout is not recreated"
                            )),
                        );
                        continue;
                    }
                    Err(error) => {
                        results.insert(task.clone(), AgentStart::Failed(error.to_string()));
                        continue;
                    }
                }
            }
            let result = match plan {
                AgentPlan::AdoptOnly => match herdr::agent_info(&mut client, &name) {
                    Ok(Some((_, current))) if current == pane => AgentStart::Adopted(pane.clone()),
                    Ok(Some((_, current))) => AgentStart::NotRestarted(format!(
                        "agent {name} now runs in pane {current}, not the recorded pane {pane}; not adopted"
                    )),
                    Ok(None) => AgentStart::NotRestarted(
                        "agent is not running; resume does not restart it (use retry --task)"
                            .to_string(),
                    ),
                    Err(error) => AgentStart::Failed(error.to_string()),
                },
                // プロンプトは送らない(設定で明示した場合のみ。初期版は起動だけ)。
                AgentPlan::Start | AgentPlan::Restart => {
                    match herdr::start_agent(&mut client, &name, &kind, &pane) {
                        Ok(()) => {
                            summary.push(format!("agent {} started in {pane}", task.0));
                            AgentStart::Started(pane.clone())
                        }
                        Err(error) => AgentStart::Failed(error.to_string()),
                    }
                }
            };
            results.insert(task.clone(), result);
        }
        let tokens = BTreeMap::from([("flow_state".to_string(), "active".to_string())]);
        if let Err(error) = herdr::report_metadata(&mut client, &state.workspace_id, &tokens) {
            summary.push(format!("metadata not reported: {error}"));
        }
        Ok(engine::InitOutcome {
            message: format!("@layout.create: {}", summary.join(", ")),
            agents: results,
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
        if let Some(state) = &start.herdr_resume {
            workspace_id = Some(state.workspace_id.clone());
        } else if !start.is_new {
            workspace_id = start.record.owned_workspace.clone().or(workspace_id);
        }
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
            // 別のworktreeで既にcheckout済みのブランチには強制実行しない(既定は
            // `ifAlreadyCheckedOut: error`)。作成要求の前にGitの実状態を照合する(T13)。
            if let Ok(Some(existing)) = crate::runtime::git::worktree_for_branch(&repo_dir, &branch)
            {
                return Err(ErrorBody {
                    code: "conflict".to_string(),
                    message: format!(
                        "branch {branch} is already checked out at {}; use attach --herdr --checkout {} to adopt it",
                        existing.display(),
                        existing.display()
                    ),
                });
            }
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
    // TaskRunnerは別プロセスで起動し、Coordinatorが終了しても子プロセスを止めない(ADR-0002)。
    prepared.config.runner = shared.runner.clone();
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
            prepared
                .agents
                .iter()
                .map(|task| {
                    (
                        task.clone(),
                        start
                            .agent_plans
                            .get(task)
                            .copied()
                            .unwrap_or(AgentPlan::Start),
                    )
                })
                .collect(),
            start.herdr_resume.clone(),
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
        herdr_state: None,
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
            herdr_resume: None,
            agent_plans: BTreeMap::new(),
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
    // Agentを含むRun、または画面を記録したRunはHerdr連携で再開する。
    let herdr_resume: Option<crate::herdr::HerdrState> = record
        .herdr_state
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok());
    let has_agents = tasks
        .values()
        .any(|task| task.kind == engine::TaskKind::Agent);
    let herdr = has_agents || herdr_resume.is_some();
    if let Some(task) = &retry_task
        && tasks
            .get(task)
            .is_some_and(|snapshot| snapshot.kind == engine::TaskKind::Agent)
    {
        // 明示した再試行でも、稼働中のAgentを止めて起動し直すことはしない。
        let mut client = crate::herdr::HerdrClient::from_env(&params.env).map_err(herdr_error)?;
        if crate::herdr::agent_info(&mut client, &crate::herdr::agent_name(&task.0))
            .map_err(herdr_error)?
            .is_some()
        {
            return Err(ErrorBody {
                code: "conflict".to_string(),
                message: format!(
                    "agent {} is still running in Herdr; exit it there before retrying",
                    task.0
                ),
            });
        }
    }
    let mut initial = BTreeMap::new();
    let mut agent_plans = BTreeMap::new();
    for (id, snapshot) in tasks {
        let mut runtime = snapshot.runtime;
        let retrying = retry_task.as_ref() == Some(&id);
        if snapshot.kind == engine::TaskKind::Agent {
            let started_before = runtime
                .attempts
                .iter()
                .any(|attempt| !matches!(attempt.outcome, AttemptOutcome::LaunchFailed(_)));
            let plan = if retrying {
                AgentPlan::Restart
            } else if started_before {
                AgentPlan::AdoptOnly
            } else {
                AgentPlan::Start
            };
            agent_plans.insert(id.clone(), plan);
            runtime.state = TaskState::Waiting;
            runtime.readiness = ReadinessState::Pending;
            initial.insert(id, (runtime, None));
            continue;
        }
        let keep = runtime.state == TaskState::Succeeded && !retrying;
        let unknown = runtime.state == TaskState::Unknown && !retrying;
        if keep || unknown {
            initial.insert(id, (runtime, snapshot.detail));
            continue;
        }
        runtime.state = TaskState::Waiting;
        runtime.readiness = match snapshot.kind {
            engine::TaskKind::Job => ReadinessState::NotApplicable,
            engine::TaskKind::Service | engine::TaskKind::Agent => ReadinessState::Pending,
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
            herdr,
            herdr_resume,
            agent_plans,
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
                    // T14: 起動後・記録前に終了した場合は、次のAttemptの起動証跡が残る。
                    // その場合は起動済みとして照合し、未起動扱いで再実行しない。
                    let next = runtime
                        .attempts
                        .last()
                        .map_or(1, |attempt| attempt.attempt_id.0 + 1);
                    let launch: Option<engine::LaunchFile> = std::fs::read(
                        state
                            .attempt_dir(&run.run_id, &id.0, next)
                            .join("launch.json"),
                    )
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok());
                    let Some(launch) = launch else {
                        runtime.state = TaskState::Cancelled;
                        task.detail = Some("coordinator stopped before start".to_string());
                        continue;
                    };
                    runtime.record_attempt_result(crate::state::models::AttemptResult {
                        attempt_id: crate::state::models::AttemptId(next),
                        launch_evidence: crate::state::models::LaunchEvidence {
                            pid: Some(launch.child.pid),
                            started_at: Some(launch.started_at.clone()),
                            command_summary: launch.command.clone(),
                        },
                        outcome: AttemptOutcome::Pending,
                        log_position: crate::state::models::LogPosition {
                            byte_offset: 0,
                            line_number: 0,
                        },
                    });
                    runtime.state = TaskState::Running;
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
                        "process {} is still running under its task runner; not restarted (the result is imported when it finishes)",
                        launch.as_ref().map_or(0, |launch| launch.child.pid)
                    ));
                    AttemptOutcome::Unknown
                }
                (_, Some(result)) => {
                    task.detail = Some("result imported after coordinator restart".to_string());
                    outcome_from_result(result)
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

fn outcome_from_result(result: &engine::ResultFile) -> AttemptOutcome {
    match (&result.exit_code, &result.signal) {
        (Some(code), _) => AttemptOutcome::ExitCode(*code),
        (None, Some(signal)) => AttemptOutcome::Signaled(signal.clone()),
        (None, None) => AttemptOutcome::Unknown,
    }
}

/// Coordinatorの再起動で状態不明として残したTaskのうち、TaskRunnerが後から
/// `result.json`を確定したものの結果を取り込む(T15)。再実行はしない。
/// 取り込んだTaskの数を返す。
pub fn import_finished_results(
    state: &StateDir,
    store: &mut Store,
) -> Result<usize, CoordinatorError> {
    let mut imported = 0;
    for run in store.recent_runs(1000).map_err(io_err)? {
        if !matches!(run.phase, RunPhase::Stopped | RunPhase::Finished)
            || run.outcome != RunOutcome::Unknown
        {
            continue;
        }
        let mut tasks = store.tasks(&run.run_id).map_err(io_err)?;
        let mut messages = Vec::new();
        for (id, task) in &mut tasks {
            if task.runtime.state != TaskState::Unknown || task.kind == engine::TaskKind::Agent {
                continue;
            }
            let Some(last) = task.runtime.attempts.last().cloned() else {
                continue;
            };
            let dir = state.attempt_dir(&run.run_id, &id.0, last.attempt_id.0);
            let Some(result) = std::fs::read(dir.join("result.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<engine::ResultFile>(&bytes).ok())
            else {
                continue;
            };
            let outcome = outcome_from_result(&result);
            if outcome == AttemptOutcome::Unknown {
                continue;
            }
            // 記録上は確定済み(Unknown)のAttemptを、確定した結果で置き換える。
            if let Some(entry) = task
                .runtime
                .attempts
                .iter_mut()
                .find(|attempt| attempt.attempt_id == last.attempt_id)
            {
                entry.outcome = outcome.clone();
            }
            task.runtime.state = match (task.kind, &outcome) {
                (engine::TaskKind::Job, AttemptOutcome::ExitCode(0)) => TaskState::Succeeded,
                _ => TaskState::Failed,
            };
            task.detail =
                Some("result imported from the task runner after coordinator restart".to_string());
            messages.push(format!("[{}] result imported: {outcome:?}", id.0));
            imported += 1;
        }
        if messages.is_empty() {
            continue;
        }
        let states: Vec<TaskState> = tasks.values().map(|task| task.runtime.state).collect();
        let outcome = if states.contains(&TaskState::Unknown) {
            RunOutcome::Unknown
        } else if states
            .iter()
            .any(|state| matches!(state, TaskState::Failed | TaskState::Skipped))
        {
            RunOutcome::Failed
        } else {
            RunOutcome::Succeeded
        };
        store
            .save_snapshot(&engine::RunSnapshot {
                run_id: run.run_id.clone(),
                phase: run.phase,
                outcome,
                target: run.target.clone().map(TaskId),
                tasks,
                updated_at: logs::rfc3339(SystemTime::now()),
            })
            .map_err(io_err)?;
        for message in messages {
            let _ = store.add_event(&run.run_id, &message);
        }
    }
    Ok(imported)
}

/// 保持日数を過ぎた、終了・停止済みRunのAttemptファイル(ログ、起動証跡、結果)を削除する。
/// 状態ストアのRunとTaskの記録は残す。稼働中・状態不明のRunは対象にしない。
pub fn prune_logs(
    state: &StateDir,
    store: &Store,
    retention_days: u32,
    now: SystemTime,
) -> Result<Vec<String>, CoordinatorError> {
    let cutoff = now
        .checked_sub(Duration::from_secs(u64::from(retention_days) * 86_400))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let cutoff = logs::rfc3339(cutoff);
    let mut removed = Vec::new();
    for run in store.recent_runs(u32::MAX).map_err(io_err)? {
        if !matches!(run.phase, RunPhase::Stopped | RunPhase::Finished)
            || run.outcome == RunOutcome::Unknown
            || run.updated_at >= cutoff
        {
            continue;
        }
        let dir = state.run_dir(&run.run_id);
        if !dir.exists() {
            continue;
        }
        std::fs::remove_dir_all(&dir).map_err(io_err)?;
        let _ = store.add_event(
            &run.run_id,
            &format!("logs removed after {retention_days} day(s) of retention"),
        );
        removed.push(run.run_id);
    }
    Ok(removed)
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_size(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

/// Attemptファイルの合計が上限を超えていれば、終了・停止済みRunを古い順に削除する。
/// 稼働中と状態不明のRunは数えるが削除しない。状態ストアの記録は残す。
pub fn prune_by_total(
    state: &StateDir,
    store: &Store,
    max_total_bytes: u64,
) -> Result<Vec<String>, CoordinatorError> {
    let mut runs: Vec<(RunRecord, u64)> = store
        .recent_runs(u32::MAX)
        .map_err(io_err)?
        .into_iter()
        .map(|run| {
            let size = dir_size(&state.run_dir(&run.run_id));
            (run, size)
        })
        .collect();
    let mut total: u64 = runs.iter().map(|(_, size)| *size).sum();
    // 古いものから消すため、最終更新の昇順に並べる。
    runs.sort_by(|a, b| a.0.updated_at.cmp(&b.0.updated_at));
    let mut removed = Vec::new();
    for (run, size) in runs {
        if total <= max_total_bytes {
            break;
        }
        if size == 0
            || !matches!(run.phase, RunPhase::Stopped | RunPhase::Finished)
            || run.outcome == RunOutcome::Unknown
        {
            continue;
        }
        std::fs::remove_dir_all(state.run_dir(&run.run_id)).map_err(io_err)?;
        total = total.saturating_sub(size);
        let _ = store.add_event(
            &run.run_id,
            &format!("logs removed to keep the total under {max_total_bytes} bytes"),
        );
        removed.push(run.run_id);
    }
    Ok(removed)
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
        import_finished_results(state, &mut store)?;
        // 保持日数と総容量はCoordinatorの起動元のプラグイン全体設定から読む。
        let plugin = crate::config::plugin::load_from_env().ok();
        if let Some(days) = plugin
            .as_ref()
            .and_then(|config| config.log_retention_days())
        {
            prune_logs(state, &store, days, SystemTime::now())?;
        }
        if let Some(total) = plugin
            .as_ref()
            .and_then(|config| config.log_max_total_bytes())
        {
            prune_by_total(state, &store, total)?;
        }
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
        runner: crate::runtime::runner::Launcher::current().map_err(io_err)?,
        runs: Mutex::new(HashMap::new()),
        last_activity: Mutex::new(Instant::now()),
        shutdown: AtomicBool::new(false),
    });
    // 状態不明のTaskの結果を、TaskRunnerが確定し次第取り込む(T15)。
    let mut import_store = Store::open(&state.db_path()).map_err(io_err)?;
    let mut next_import = Instant::now() + IMPORT_INTERVAL;
    let total_limit = crate::config::plugin::load_from_env()
        .ok()
        .and_then(|config| config.log_max_total_bytes());
    let mut next_total_check = Instant::now() + TOTAL_CHECK_INTERVAL;
    loop {
        if Instant::now() >= next_import {
            next_import = Instant::now() + IMPORT_INTERVAL;
            let _ = import_finished_results(state, &mut import_store);
        }
        if let Some(limit) = total_limit
            && Instant::now() >= next_total_check
        {
            next_total_check = Instant::now() + TOTAL_CHECK_INTERVAL;
            let _ = prune_by_total(state, &import_store, limit);
        }
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
        // ブランチを付けるとcheckout済みとして拒否されるため、ここではdetachedで作る。
        run(&repo, &["worktree", "add", "-q", "--detach", "../wt"]);
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
            runner: crate::runtime::runner::Launcher::Thread,
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

    fn wait_until(store: &Store, run_id: &str, what: impl Fn(&RunRecord, &[String]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let record = store.run(run_id).unwrap();
            let events: Vec<String> = store
                .events(run_id, 0)
                .unwrap()
                .into_iter()
                .map(|(_, _, message)| message)
                .collect();
            if what(&record, &events) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out: {events:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn done(record: &RunRecord, _: &[String]) -> bool {
        matches!(record.phase, RunPhase::Finished | RunPhase::Stopped)
    }

    #[test]
    fn herdr_resume_keeps_layout_and_retry_restarts_agent_only_in_verified_pane() {
        use crate::herdr::fake::FakeHerdr;
        use crate::runtime::git::test_repo::init;
        use crate::runtime::state_dir::test_support::TempDir;

        let tmp = TempDir::new("coord-resume");
        let repo = init(&tmp.0.join("repo"));
        std::fs::create_dir_all(repo.join(".herdr")).unwrap();
        let workflow = "version: 1\nname: t\nworktree:\n  mode: attach\nbootstrap:\n  targets: [init]\ntasks:\n  init:\n    type: command\n    lifecycle: job\n    runner: supervised\n    argv: [/bin/sh, -c, 'echo x >> init-count']\n  developer:\n    type: agent\n    agent:\n      kind: claude\n    dependsOn:\n      - task: init\n        condition: succeeded\nworkspace:\n  label: t\n  tabs:\n    - id: dev\n      label: dev\n      layout: { type: grid, columns: 2, rows: 1 }\n      panes:\n        - id: L\n          label: init\n          view: { type: logs, task: init }\n          placement: { column: 1, row: 1 }\n        - id: A\n          label: developer\n          view: { type: agent, task: developer }\n          placement: { column: 2, row: 1 }\n";
        let config = repo.join(".herdr/workflow.yaml");
        std::fs::write(&config, workflow).unwrap();
        let socket = PathBuf::from(format!("/tmp/hwr-{}.sock", std::process::id()));
        let fake = FakeHerdr::start(socket.clone(), repo.clone());
        let shared = Arc::new(Shared {
            state: StateDir::open(tmp.0.join("state")).unwrap(),
            runner: crate::runtime::runner::Launcher::Thread,
            runs: Mutex::new(HashMap::new()),
            last_activity: Mutex::new(Instant::now()),
            shutdown: AtomicBool::new(false),
        });
        let mut env = current_env();
        env.insert(
            "HERDR_SOCKET_PATH".to_string(),
            socket.display().to_string(),
        );
        env.insert("HERDR_WORKSPACE_ID".to_string(), "w9".to_string());
        // 照会は起動直後に1回だけ行い、以後は試験中に発生させない。
        env.insert(
            "HERDR_WORKFLOW_AGENT_POLL_MS".to_string(),
            "600000".to_string(),
        );
        let push = |status: &str| {
            fake.agent_statuses
                .lock()
                .unwrap()
                .push_back(status.to_string());
        };
        let count = |method: &str| fake.methods().iter().filter(|m| *m == method).count();
        let store = Store::open(&shared.state.db_path()).unwrap();
        let stop = |run_id: &str| {
            handle_stop(&shared, json!({ "runId": run_id })).unwrap();
            wait_until(&store, run_id, done);
        };
        let resume = |run_id: &str, retry: Option<&str>| {
            handle_resume(
                &shared,
                json!({ "runId": run_id, "retryTask": retry, "env": env }),
            )
        };
        let developer =
            |run_id: &str| store.tasks(run_id).unwrap()[&TaskId::from("developer")].clone();

        // 1. 新しいRun: 初期化、画面構築、Agent起動。
        push("working");
        let result = handle_start(
            &shared,
            json!({
                "configPath": config, "workflow": workflow, "inputs": [],
                "checkout": repo, "target": null, "env": env, "herdr": true,
            }),
        )
        .unwrap();
        let run_id = result["runId"].as_str().unwrap().to_string();
        wait_until(&store, &run_id, |_, events| {
            events.iter().any(|event| event.contains("agent working"))
        });
        let recorded: crate::herdr::HerdrState =
            serde_json::from_str(store.run(&run_id).unwrap().herdr_state.as_deref().unwrap())
                .unwrap();
        assert_eq!(recorded.agent_panes["developer"], "w9:p2");
        stop(&run_id);
        assert_eq!(developer(&run_id).runtime.state, TaskState::Stopped);

        // 2. resume: Agentが同じpaneで稼働中なら追跡だけを再開し、画面もAgentも作り直さない(T20)。
        push("idle");
        push("idle");
        resume(&run_id, None).unwrap();
        wait_until(&store, &run_id, |_, events| {
            events
                .iter()
                .any(|event| event.contains("re-adopted running agent"))
        });
        assert_eq!(count("layout.apply"), 1);
        assert_eq!(count("agent.start"), 1);
        assert!(fake.methods().contains(&"pane.get".to_string()));
        stop(&run_id);
        assert_eq!(
            std::fs::read_to_string(repo.join("init-count")).unwrap(),
            "x\n",
            "succeeded initialization is not repeated"
        );

        // 3. resume: 終了済みのAgentは起動し直さない。
        resume(&run_id, None).unwrap();
        wait_until(&store, &run_id, done);
        assert_eq!(count("agent.start"), 1);
        let agent = developer(&run_id);
        assert_eq!(agent.runtime.state, TaskState::Stopped);
        assert!(agent.detail.unwrap().contains("does not restart"));

        // 4. retry: 稼働中のAgentは止めずに拒否する。
        push("working");
        let error = resume(&run_id, Some("developer")).unwrap_err();
        assert_eq!(error.code, "conflict");
        assert_eq!(count("agent.start"), 1);

        // 5. retry: 照合できた記録済みpaneで起動し直す(新しいAttempt)。
        resume(&run_id, Some("developer")).unwrap();
        wait_until(&store, &run_id, done);
        assert_eq!(count("agent.start"), 2);
        assert_eq!(count("layout.apply"), 1);
        assert_eq!(store.run(&run_id).unwrap().outcome, RunOutcome::Succeeded);
        let attempts = developer(&run_id).runtime.attempts;
        assert_eq!(attempts.last().unwrap().attempt_id.0, 2);

        // 6. retry: 記録したpaneが消えていたら、そのpane IDでは操作しない(T17)。
        fake.missing_panes.lock().unwrap().push("w9:p2".to_string());
        resume(&run_id, Some("developer")).unwrap();
        wait_until(&store, &run_id, done);
        assert_eq!(count("agent.start"), 2);
        assert_eq!(count("layout.apply"), 1);
        let agent = developer(&run_id);
        assert_eq!(agent.runtime.state, TaskState::Failed);
        assert!(agent.detail.unwrap().contains("no longer exists"));
        assert_eq!(store.run(&run_id).unwrap().outcome, RunOutcome::Failed);
        let _ = std::fs::remove_file(socket);
    }

    #[test]
    fn retention_removes_only_old_finished_run_files() {
        use crate::runtime::state_dir::test_support::TempDir;
        let tmp = TempDir::new("coord-prune");
        let state = StateDir::open(tmp.0.join("state")).unwrap();
        let mut store = Store::open(&state.db_path()).unwrap();
        let record = |id: &str| RunRecord {
            run_id: id.to_string(),
            created_at: "2026-01-01T00:00:00.000Z".to_string(),
            updated_at: "2026-01-01T00:00:00.000Z".to_string(),
            plan_hash: String::new(),
            config_path: PathBuf::from("/c"),
            checkout: PathBuf::from("/w"),
            target: None,
            phase: RunPhase::Queued,
            outcome: RunOutcome::Pending,
            workflow: String::new(),
            inputs: Vec::new(),
            coordinator_pid: None,
            plugin_config: String::new(),
            owned_workspace: None,
            herdr_state: None,
        };
        let finish = |store: &mut Store, id: &str, phase, outcome, at: &str| {
            store
                .save_snapshot(&engine::RunSnapshot {
                    run_id: id.to_string(),
                    phase,
                    outcome,
                    target: None,
                    tasks: BTreeMap::new(),
                    updated_at: at.to_string(),
                })
                .unwrap();
        };
        for id in ["old", "recent", "active", "unknown"] {
            store.insert_run(&record(id)).unwrap();
            let dir = state.attempt_dir(id, "t", 1);
            state.ensure_subdir(&dir).unwrap();
            std::fs::write(dir.join("stdout.log"), "x").unwrap();
        }
        finish(
            &mut store,
            "old",
            RunPhase::Finished,
            RunOutcome::Succeeded,
            "2026-01-01T00:00:00.000Z",
        );
        finish(
            &mut store,
            "recent",
            RunPhase::Finished,
            RunOutcome::Failed,
            "2026-03-09T00:00:00.000Z",
        );
        finish(
            &mut store,
            "active",
            RunPhase::Active,
            RunOutcome::Pending,
            "2026-01-01T00:00:00.000Z",
        );
        finish(
            &mut store,
            "unknown",
            RunPhase::Stopped,
            RunOutcome::Unknown,
            "2026-01-01T00:00:00.000Z",
        );
        // 2026-03-10から7日前より古いものだけが対象。
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_773_100_800);
        let removed = prune_logs(&state, &store, 7, now).unwrap();
        assert_eq!(removed, ["old"]);
        assert!(!state.run_dir("old").exists());
        for id in ["recent", "active", "unknown"] {
            assert!(state.run_dir(id).exists(), "{id}");
        }
        // Runの記録は残る。
        assert_eq!(store.run("old").unwrap().outcome, RunOutcome::Succeeded);
    }

    #[test]
    fn total_limit_removes_oldest_finished_runs_first() {
        use crate::runtime::state_dir::test_support::TempDir;
        let tmp = TempDir::new("coord-total");
        let state = StateDir::open(tmp.0.join("state")).unwrap();
        let mut store = Store::open(&state.db_path()).unwrap();
        let runs = [
            (
                "a",
                RunPhase::Finished,
                RunOutcome::Succeeded,
                "2026-01-01T00:00:00.000Z",
            ),
            (
                "b",
                RunPhase::Active,
                RunOutcome::Pending,
                "2026-01-02T00:00:00.000Z",
            ),
            (
                "c",
                RunPhase::Stopped,
                RunOutcome::Cancelled,
                "2026-01-03T00:00:00.000Z",
            ),
            (
                "d",
                RunPhase::Finished,
                RunOutcome::Failed,
                "2026-01-04T00:00:00.000Z",
            ),
        ];
        for (id, phase, outcome, at) in runs {
            store
                .insert_run(&RunRecord {
                    run_id: id.to_string(),
                    created_at: at.to_string(),
                    updated_at: at.to_string(),
                    plan_hash: String::new(),
                    config_path: PathBuf::from("/c"),
                    checkout: PathBuf::from("/w"),
                    target: None,
                    phase,
                    outcome,
                    workflow: String::new(),
                    inputs: Vec::new(),
                    coordinator_pid: None,
                    plugin_config: String::new(),
                    owned_workspace: None,
                    herdr_state: None,
                })
                .unwrap();
            store
                .save_snapshot(&engine::RunSnapshot {
                    run_id: id.to_string(),
                    phase,
                    outcome,
                    target: None,
                    tasks: BTreeMap::new(),
                    updated_at: at.to_string(),
                })
                .unwrap();
            let dir = state.attempt_dir(id, "t", 1);
            state.ensure_subdir(&dir).unwrap();
            std::fs::write(dir.join("stdout.log"), vec![b'x'; 1000]).unwrap();
        }
        // 4000バイトを2500バイト以下にする: 稼働中のbを飛ばし、古いaとcを消す。
        let removed = prune_by_total(&state, &store, 2500).unwrap();
        assert_eq!(removed, ["a", "c"]);
        assert!(state.run_dir("b").exists());
        assert!(state.run_dir("d").exists());
        assert!(prune_by_total(&state, &store, 2500).unwrap().is_empty());
    }

    #[test]
    fn launch_recorded_only_by_the_runner_is_not_treated_as_unstarted() {
        // T14: 状態ストアに起動を記録する前にCoordinatorが終了しても、起動証跡から
        // 起動済みと判断し、結果を取り込む(未起動として再実行しない)。
        use crate::runtime::state_dir::test_support::TempDir;
        let tmp = TempDir::new("coord-t14");
        let state = StateDir::open(tmp.0.join("state")).unwrap();
        let mut store = Store::open(&state.db_path()).unwrap();
        store
            .insert_run(&RunRecord {
                run_id: "r".to_string(),
                created_at: "2026-01-01T00:00:00.000Z".to_string(),
                updated_at: "2026-01-01T00:00:00.000Z".to_string(),
                plan_hash: String::new(),
                config_path: PathBuf::from("/c"),
                checkout: PathBuf::from("/w"),
                target: None,
                phase: RunPhase::Active,
                outcome: RunOutcome::Pending,
                workflow: String::new(),
                inputs: Vec::new(),
                coordinator_pid: None,
                plugin_config: String::new(),
                owned_workspace: None,
                herdr_state: None,
            })
            .unwrap();
        let waiting = |kind| engine::TaskSnapshot {
            kind,
            runtime: crate::state::models::TaskRuntimeState::default(),
            detail: None,
        };
        store
            .save_snapshot(&engine::RunSnapshot {
                run_id: "r".to_string(),
                phase: RunPhase::Active,
                outcome: RunOutcome::Pending,
                target: None,
                tasks: BTreeMap::from([
                    (TaskId::from("migrate"), waiting(engine::TaskKind::Job)),
                    (TaskId::from("later"), waiting(engine::TaskKind::Job)),
                ]),
                updated_at: "2026-01-01T00:00:00.000Z".to_string(),
            })
            .unwrap();
        let dir = state.attempt_dir("r", "migrate", 1);
        state.ensure_subdir(&dir).unwrap();
        // 既に終了したプロセスの起動証跡と、TaskRunnerが確定した結果。
        let launch = engine::LaunchFile {
            run_id: "r".to_string(),
            task_id: "migrate".to_string(),
            attempt_id: 1,
            command: "migrate".to_string(),
            cwd: PathBuf::from("/w"),
            child: process::ProcessIdentity {
                pid: u32::MAX - 1,
                start_time: Some(1),
                pgid: Some(u32::MAX - 1),
                boot_id: Some("old-boot".to_string()),
            },
            runner: None,
            started_at: "2026-01-01T00:00:00.000Z".to_string(),
        };
        std::fs::write(
            dir.join("launch.json"),
            serde_json::to_vec(&launch).unwrap(),
        )
        .unwrap();
        let result = engine::ResultFile {
            run_id: "r".to_string(),
            task_id: "migrate".to_string(),
            attempt_id: 1,
            exit_code: Some(0),
            signal: None,
            finished_at: "2026-01-01T00:00:01.000Z".to_string(),
        };
        std::fs::write(
            dir.join("result.json"),
            serde_json::to_vec(&result).unwrap(),
        )
        .unwrap();

        reconcile_orphans(&state, &mut store).unwrap();
        let tasks = store.tasks("r").unwrap();
        let migrate = &tasks[&TaskId::from("migrate")];
        assert_eq!(migrate.runtime.state, TaskState::Succeeded, "{migrate:?}");
        assert_eq!(migrate.runtime.attempts.len(), 1);
        assert_eq!(
            tasks[&TaskId::from("later")].runtime.state,
            TaskState::Cancelled
        );
    }

    #[test]
    fn herdr_up_refuses_a_branch_already_checked_out_elsewhere() {
        use crate::herdr::fake::FakeHerdr;
        use crate::runtime::git::test_repo::{init, run};
        use crate::runtime::state_dir::test_support::TempDir;

        let tmp = TempDir::new("coord-checked-out");
        let repo = init(&tmp.0.join("repo"));
        run(
            &repo,
            &["worktree", "add", "-q", "-b", "feature/x", "../wt"],
        );
        let workflow = "version: 1\nname: t\ninputs:\n  branch:\n    type: string\n    required: true\nworktree:\n  mode: create\n  branch: '${inputs.branch}'\nbootstrap:\n  targets: []\ntasks:\n  init:\n    type: command\n    lifecycle: job\n    runner: supervised\n    argv: [/bin/true]\nworkspace:\n  label: t\n  tabs:\n    - id: dev\n      label: dev\n      layout: { type: grid, columns: 1, rows: 1 }\n      panes:\n        - id: S\n          label: status\n          view: { type: status }\n          placement: { column: 1, row: 1 }\n";
        let config = repo.join("workflow.yaml");
        std::fs::write(&config, workflow).unwrap();
        let socket = PathBuf::from(format!("/tmp/hwc-{}.sock", std::process::id()));
        let fake = FakeHerdr::start(socket.clone(), tmp.0.join("other"));
        let shared = Arc::new(Shared {
            state: StateDir::open(tmp.0.join("state")).unwrap(),
            runner: crate::runtime::runner::Launcher::Thread,
            runs: Mutex::new(HashMap::new()),
            last_activity: Mutex::new(Instant::now()),
            shutdown: AtomicBool::new(false),
        });
        let mut env = current_env();
        env.insert(
            "HERDR_SOCKET_PATH".to_string(),
            socket.display().to_string(),
        );
        let error = handle_start(
            &shared,
            json!({
                "configPath": config, "workflow": workflow,
                "inputs": [["branch", "feature/x"]],
                "checkout": repo, "target": null, "env": env, "herdr": true,
            }),
        )
        .unwrap_err();
        assert_eq!(error.code, "conflict");
        assert!(
            error.message.contains("already checked out"),
            "{}",
            error.message
        );
        // 作成要求を送らず、worktreeを増やさない。
        assert!(fake.methods().is_empty());
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
