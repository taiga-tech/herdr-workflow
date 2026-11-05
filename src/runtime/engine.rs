//! 1つのRunを実行するスケジューラ(docs/specs/workflow.md、state-model.md、
//! execution.md、observability.md)。
//!
//! 単一スレッドのイベントループがTaskの状態を所有し、子プロセスの出力・終了・
//! readiness確認は補助スレッドから`Event`として受け取る。状態の更新はこのループだけが
//! 行い、`Recorder`へ渡す(状態ストアの単一書き込み経路)。

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::config::model::{
    DependencyCondition, LogStream, OnDependencyLost, TaskId, TaskLifecycle,
};
use crate::config::secrets::{self, SecretsFile};
use crate::plan::compile::ExecutionPlan;
use crate::plan::schedule;
use crate::runtime::locks::{self, LockAttempt, LockGuard};
use crate::runtime::logs::{self, LogRecord, LogWriter, Masker, Stream};
use crate::runtime::prepare::{self, PreparedTask, Readiness};
use crate::runtime::process::{self, Exit, LaunchSpec, ProcessHandle, ProcessIdentity};
use crate::runtime::state_dir::StateDir;
use crate::state::models::{
    AgentObservation, AttemptId, AttemptOutcome, AttemptResult, LaunchEvidence, LogPosition,
    ReadinessState, TaskRuntimeState, TaskState,
};

const TICK: Duration = Duration::from_millis(50);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunPhase {
    Queued,
    Preparing,
    Active,
    Stopping,
    Stopped,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunOutcome {
    Pending,
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    pub max_jobs: Option<u32>,
    pub max_services: Option<u32>,
}

/// 起動元環境の取得口。秘密値の`fromEnv`はこの環境から読む。
pub type EnvMap = BTreeMap<OsString, OsString>;

pub struct RunConfig {
    pub run_id: String,
    pub plan: ExecutionPlan,
    /// 実行対象のcommandタスク(選択済み)。
    pub tasks: BTreeMap<TaskId, PreparedTask>,
    /// `run --target`の対象。`None`なら全タスクの終端まで続ける。
    pub target: Option<TaskId>,
    pub base_env: EnvMap,
    pub limits: Limits,
    /// 秘密値定義を読み直す関数(Attempt起動直前に呼ぶ)。
    pub load_secrets: Box<dyn Fn() -> Result<SecretsFile, secrets::SecretsError> + Send>,
    /// checkout実行ロックの名前。初期化対象の実行中だけ保持する。
    pub checkout_lock: Option<String>,
    /// 再開・再試行時に引き継ぐTaskの状態と説明。ないTaskは未実行から始める。
    pub initial: BTreeMap<TaskId, (TaskRuntimeState, Option<String>)>,
    /// 初期化の成功後、初期化対象でないタスクを起動する前に一度だけ呼ぶ(画面構築)。
    pub on_init_complete: Option<InitHook>,
    /// Herdrで起動するAgentタスク(Task ID → Agent名)。
    pub agents: BTreeMap<TaskId, String>,
    /// Agentの観測値を照会する関数と、その間隔。
    pub observe_agent: Option<AgentObserver>,
    pub agent_poll: Duration,
}

/// 画面構築の結果。Agentごとの起動結果(成功時はpane ID、失敗時は理由)を返す。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InitOutcome {
    pub message: String,
    pub agents: BTreeMap<TaskId, Result<String, String>>,
}

/// 画面構築等の処理。失敗するとRunを失敗として停止する。
pub type InitHook = Box<dyn FnOnce() -> Result<InitOutcome, String> + Send>;

/// Agentの照会結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentProbe {
    Observed(AgentObservation),
    /// Agentが終了した、または別のプロセスに置き換わった。
    Gone,
    /// Herdrへ照会できない。観測値は`unknown`にする。
    Unreachable(String),
}

pub type AgentObserver = Arc<dyn Fn(&str) -> AgentProbe + Send + Sync>;

/// 状態画面と状態ストアで使うTaskの種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskKind {
    Job,
    Service,
    Agent,
}

impl From<TaskLifecycle> for TaskKind {
    fn from(lifecycle: TaskLifecycle) -> Self {
        match lifecycle {
            TaskLifecycle::Job => Self::Job,
            TaskLifecycle::Service => Self::Service,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    pub kind: TaskKind,
    pub runtime: TaskRuntimeState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSnapshot {
    pub run_id: String,
    pub phase: RunPhase,
    pub outcome: RunOutcome,
    pub target: Option<TaskId>,
    pub tasks: BTreeMap<TaskId, TaskSnapshot>,
    pub updated_at: String,
}

/// 状態の保存先と進捗の通知先。保存に失敗したら新しい起動を止める。
pub trait Recorder {
    fn record(&mut self, snapshot: &RunSnapshot) -> Result<(), String>;
    fn notice(&mut self, _message: &str) {}
}

#[derive(Debug, Default)]
pub struct MemoryRecorder {
    pub snapshots: Vec<RunSnapshot>,
    pub notices: Vec<String>,
}

impl Recorder for MemoryRecorder {
    fn record(&mut self, snapshot: &RunSnapshot) -> Result<(), String> {
        self.snapshots.push(snapshot.clone());
        Ok(())
    }

    fn notice(&mut self, message: &str) {
        self.notices.push(message.to_string());
    }
}

/// 外部からの制御要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    StopAll,
}

#[derive(Debug)]
enum Event {
    Line {
        task: TaskId,
        attempt: u32,
        stream: Stream,
        text: String,
    },
    Exited {
        task: TaskId,
        attempt: u32,
        exit: Result<Exit, String>,
    },
    StreamClosed {
        task: TaskId,
        attempt: u32,
        lines: u64,
        degraded: bool,
    },
    Probe {
        task: TaskId,
        attempt: u32,
        ok: bool,
    },
    Agent {
        task: TaskId,
        probe: AgentProbe,
    },
}

/// HerdrのAgent。プロセスを所有しないため停止信号は送らない。
struct AgentSlot {
    name: String,
    runtime: TaskRuntimeState,
    detail: Option<String>,
    cancel: Arc<AtomicBool>,
}

impl AgentSlot {
    fn is_terminal(&self) -> bool {
        !matches!(
            self.runtime.state,
            TaskState::Waiting | TaskState::Starting | TaskState::Running | TaskState::Stopping
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopKind {
    Timeout,
    ReadinessTimeout,
    User,
    DependencyLost,
}

struct LiveAttempt {
    attempt: u32,
    handle: ProcessHandle,
    deadline: Option<Instant>,
    ready_deadline: Option<Instant>,
    successes: u32,
    failures: u32,
    stop: Option<StopKind>,
    signaled_at: Option<Instant>,
    killed: bool,
    /// 停止信号を送った時点の子孫(グループ外へ移ったものを含む)。
    descendants: Vec<ProcessIdentity>,
    probe_cancel: Arc<AtomicBool>,
    probe_interval: Arc<AtomicU64>,
    want_lines: Arc<AtomicBool>,
    guards: Vec<LockGuard>,
}

struct Slot {
    prepared: PreparedTask,
    runtime: TaskRuntimeState,
    detail: Option<String>,
    live: Option<LiveAttempt>,
    retry_at: Option<Instant>,
    /// 逆順停止の対象。
    pending_stop: Option<StopKind>,
}

impl Slot {
    fn is_live(&self) -> bool {
        self.live.is_some()
    }

    fn is_terminal(&self) -> bool {
        !matches!(
            self.runtime.state,
            TaskState::Waiting | TaskState::Starting | TaskState::Running | TaskState::Stopping
        )
    }
}

/// `result.json`(Attemptの終了結果)。一時ファイルからの置換で確定する。
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

pub(crate) fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
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

pub struct Engine<'r> {
    config: RunConfig,
    state_dir: StateDir,
    recorder: &'r mut dyn Recorder,
    slots: BTreeMap<TaskId, Slot>,
    events_tx: Sender<Event>,
    events_rx: Receiver<Event>,
    control: Receiver<Control>,
    threads: Vec<JoinHandle<()>>,
    phase: RunPhase,
    outcome: RunOutcome,
    stopping_all: Option<StopKind>,
    store_degraded: bool,
    checkout_guard: Vec<LockGuard>,
    init_hook: Option<InitHook>,
    agents: BTreeMap<TaskId, AgentSlot>,
    dirty: bool,
}

/// 実行結果の要約。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    pub snapshot: RunSnapshot,
}

impl<'r> Engine<'r> {
    pub fn new(
        config: RunConfig,
        state_dir: StateDir,
        recorder: &'r mut dyn Recorder,
        control: Receiver<Control>,
    ) -> Self {
        let mut config = config;
        let init_hook = config.on_init_complete.take();
        let agents = config
            .agents
            .iter()
            .map(|(id, name)| {
                (
                    id.clone(),
                    AgentSlot {
                        name: name.clone(),
                        runtime: TaskRuntimeState::default(),
                        detail: None,
                        cancel: Arc::new(AtomicBool::new(false)),
                    },
                )
            })
            .collect();
        let (events_tx, events_rx) = mpsc::channel();
        let slots = config
            .tasks
            .iter()
            .map(|(id, prepared)| {
                let readiness = match prepared.lifecycle {
                    TaskLifecycle::Job => ReadinessState::NotApplicable,
                    TaskLifecycle::Service => ReadinessState::Pending,
                };
                let (runtime, detail) = config.initial.get(id).cloned().unwrap_or((
                    TaskRuntimeState {
                        readiness,
                        ..TaskRuntimeState::default()
                    },
                    None,
                ));
                (
                    id.clone(),
                    Slot {
                        prepared: prepared.clone(),
                        runtime,
                        detail,
                        live: None,
                        retry_at: None,
                        pending_stop: None,
                    },
                )
            })
            .collect();
        Self {
            config,
            state_dir,
            recorder,
            slots,
            events_tx,
            events_rx,
            control,
            threads: Vec::new(),
            phase: RunPhase::Queued,
            outcome: RunOutcome::Pending,
            stopping_all: None,
            store_degraded: false,
            checkout_guard: Vec::new(),
            init_hook,
            agents,
            dirty: true,
        }
    }

    fn notice(&mut self, message: String) {
        self.recorder.notice(&message);
    }

    fn snapshot(&self) -> RunSnapshot {
        RunSnapshot {
            run_id: self.config.run_id.clone(),
            phase: self.phase,
            outcome: self.outcome,
            target: self.config.target.clone(),
            tasks: self
                .slots
                .iter()
                .map(|(id, slot)| {
                    (
                        id.clone(),
                        TaskSnapshot {
                            kind: slot.prepared.lifecycle.into(),
                            runtime: slot.runtime.clone(),
                            detail: slot.detail.clone(),
                        },
                    )
                })
                .chain(self.agents.iter().map(|(id, agent)| {
                    (
                        id.clone(),
                        TaskSnapshot {
                            kind: TaskKind::Agent,
                            runtime: agent.runtime.clone(),
                            detail: agent.detail.clone(),
                        },
                    )
                }))
                .collect(),
            updated_at: logs::rfc3339(SystemTime::now()),
        }
    }

    fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let snapshot = self.snapshot();
        if let Err(error) = self.recorder.record(&snapshot)
            && !self.store_degraded
        {
            self.store_degraded = true;
            self.notice(format!(
                "状態ストアへの書き込みに失敗したため、新しいタスクを起動しません: {error}"
            ));
        }
    }

    fn states(&self) -> schedule::TaskStates {
        self.slots
            .iter()
            .map(|(id, slot)| (id.clone(), slot.runtime.clone()))
            .chain(
                self.agents
                    .iter()
                    .map(|(id, agent)| (id.clone(), agent.runtime.clone())),
            )
            .collect()
    }

    /// 実行計画を最後まで進め、結果を返す。
    pub fn run(mut self) -> RunReport {
        self.phase = RunPhase::Preparing;
        self.dirty = true;
        loop {
            match self.control.try_recv() {
                Ok(Control::StopAll) => self.request_stop_all(StopKind::User),
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => {}
            }
            match self.events_rx.recv_timeout(TICK) {
                Ok(event) => {
                    self.handle(event);
                    while let Ok(event) = self.events_rx.try_recv() {
                        self.handle(event);
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
            }
            self.check_deadlines();
            self.propagate();
            self.check_dependency_lost();
            self.advance_stops();
            if self.stopping_all.is_none() {
                self.launch_runnable();
            }
            self.update_phase();
            self.flush();
            if self.finished() {
                break;
            }
        }
        self.checkout_guard.clear();
        for thread in std::mem::take(&mut self.threads) {
            let _ = thread.join();
        }
        self.update_phase();
        self.dirty = true;
        self.flush();
        RunReport {
            snapshot: self.snapshot(),
        }
    }

    fn finished(&self) -> bool {
        matches!(self.phase, RunPhase::Finished | RunPhase::Stopped)
    }

    fn init_done(&self) -> bool {
        self.config
            .plan
            .init_boundary
            .iter()
            .filter(|id| self.slots.contains_key(*id))
            .all(|id| self.slots[id].runtime.state == TaskState::Succeeded)
    }

    fn init_failed(&self) -> bool {
        self.config.plan.init_boundary.iter().any(|id| {
            self.slots.get(id).is_some_and(|slot| {
                slot.is_terminal() && slot.runtime.state != TaskState::Succeeded
            })
        })
    }

    fn update_phase(&mut self) {
        let before = (self.phase, self.outcome);
        let any_live = self.slots.values().any(Slot::is_live);
        let all_terminal = self.slots.values().all(Slot::is_terminal)
            && self.agents.values().all(AgentSlot::is_terminal);
        let any_failed = self
            .slots
            .values()
            .map(|slot| slot.runtime.state)
            .chain(self.agents.values().map(|agent| agent.runtime.state))
            .any(|state| {
                matches!(
                    state,
                    TaskState::Failed | TaskState::Unknown | TaskState::Skipped
                )
            });

        if let Some(kind) = self.stopping_all {
            if !any_live && self.slots.values().all(|slot| slot.retry_at.is_none()) {
                self.phase = if kind == StopKind::User {
                    RunPhase::Stopped
                } else {
                    RunPhase::Finished
                };
                if self.outcome == RunOutcome::Pending {
                    self.outcome = if any_failed {
                        RunOutcome::Failed
                    } else if kind == StopKind::User {
                        RunOutcome::Cancelled
                    } else {
                        RunOutcome::Succeeded
                    };
                }
            } else {
                self.phase = RunPhase::Stopping;
            }
        } else if let Some(target) = self.config.target.clone() {
            let slot = &self.slots[&target];
            let target_success = match slot.prepared.lifecycle {
                TaskLifecycle::Job => slot.runtime.state == TaskState::Succeeded,
                TaskLifecycle::Service => {
                    slot.runtime.state == TaskState::Running
                        && slot.runtime.readiness == ReadinessState::Ready
                }
            };
            if target_success || slot.is_terminal() {
                self.outcome = if target_success {
                    RunOutcome::Succeeded
                } else {
                    RunOutcome::Failed
                };
                // 対象の結果が確定したら、残りの所有タスクを逆順に停止して終える。
                self.request_stop_all(StopKind::DependencyLost);
                self.phase = if self.slots.values().any(Slot::is_live) {
                    RunPhase::Stopping
                } else {
                    RunPhase::Finished
                };
            } else {
                self.phase = if self.init_done() {
                    RunPhase::Active
                } else {
                    RunPhase::Preparing
                };
            }
        } else if all_terminal {
            self.phase = RunPhase::Finished;
            self.outcome = if any_failed {
                RunOutcome::Failed
            } else {
                RunOutcome::Succeeded
            };
        } else {
            self.phase = if self.init_done() {
                RunPhase::Active
            } else {
                RunPhase::Preparing
            };
            if any_failed {
                self.outcome = RunOutcome::Failed;
            }
        }
        if (self.phase, self.outcome) != before {
            self.dirty = true;
        }
    }

    // ---- 停止 ----

    fn request_stop_all(&mut self, kind: StopKind) {
        if self.stopping_all.is_some() {
            return;
        }
        self.stopping_all = Some(kind);
        let reason = match kind {
            StopKind::User => "stop requested",
            _ => "run finished",
        };
        for slot in self.slots.values_mut() {
            if slot.is_live() {
                slot.pending_stop.get_or_insert(kind);
            } else if !slot.is_terminal() {
                slot.runtime.state = TaskState::Cancelled;
                slot.retry_at = None;
                slot.detail = Some(reason.to_string());
            }
        }
        // Agentのプロセスとpaneは止めず、追跡だけを終える(paneを閉じる操作と停止を分ける)。
        for agent in self.agents.values_mut() {
            agent.cancel.store(true, Ordering::Relaxed);
            match agent.runtime.state {
                TaskState::Running => {
                    agent.runtime.state = TaskState::Stopped;
                    agent.detail = Some(
                        "tracking ended; the agent keeps running in its Herdr pane".to_string(),
                    );
                }
                TaskState::Waiting => {
                    agent.runtime.state = TaskState::Cancelled;
                    agent.detail = Some(reason.to_string());
                }
                _ => {}
            }
        }
        self.dirty = true;
    }

    /// 依存の逆順に停止する。稼働中の利用側が残っている間は依存先を止めない。
    fn advance_stops(&mut self) {
        let live: BTreeSet<TaskId> = self
            .slots
            .iter()
            .filter(|(_, slot)| slot.is_live())
            .map(|(id, _)| id.clone())
            .collect();
        let ids: Vec<TaskId> = self.slots.keys().cloned().collect();
        let now = Instant::now();
        for id in ids {
            let Some(kind) = self.slots[&id].pending_stop else {
                continue;
            };
            let has_live_dependent = live.iter().any(|other| {
                other != &id
                    && self.slots[other]
                        .prepared
                        .depends_on
                        .iter()
                        .any(|dependency| dependency.task == id)
            });
            let slot = self.slots.get_mut(&id).expect("slot exists");
            let Some(live_attempt) = slot.live.as_mut() else {
                slot.pending_stop = None;
                continue;
            };
            if live_attempt.signaled_at.is_none() {
                if has_live_dependent && kind != StopKind::Timeout {
                    continue;
                }
                live_attempt.stop.get_or_insert(kind);
                live_attempt.signaled_at = Some(now);
                live_attempt.descendants = process::descendants(live_attempt.handle.pid());
                live_attempt.probe_cancel.store(true, Ordering::Relaxed);
                live_attempt.handle.signal_group(libc::SIGTERM);
                slot.runtime.state = TaskState::Stopping;
                self.dirty = true;
            } else if !live_attempt.killed
                && live_attempt
                    .signaled_at
                    .is_some_and(|at| now.duration_since(at) >= slot.prepared.grace)
            {
                live_attempt.killed = true;
                live_attempt.handle.signal_group(libc::SIGKILL);
            }
        }
    }

    fn stop_task(&mut self, id: &TaskId, kind: StopKind) {
        if let Some(slot) = self.slots.get_mut(id)
            && slot.is_live()
            && slot.pending_stop.is_none()
        {
            slot.pending_stop = Some(kind);
            self.dirty = true;
        }
    }

    // ---- 期限 ----

    fn check_deadlines(&mut self) {
        let now = Instant::now();
        let mut to_stop = Vec::new();
        let mut retry_due = false;
        for (id, slot) in &mut self.slots {
            if let Some(at) = slot.retry_at
                && at <= now
            {
                slot.retry_at = None;
                retry_due = true;
            }
            let Some(live) = &slot.live else { continue };
            if live.stop.is_some() || slot.pending_stop.is_some() {
                continue;
            }
            if live.deadline.is_some_and(|deadline| deadline <= now) {
                to_stop.push((id.clone(), StopKind::Timeout, "timeout"));
            } else if slot.runtime.readiness == ReadinessState::Pending
                && live.ready_deadline.is_some_and(|deadline| deadline <= now)
            {
                to_stop.push((id.clone(), StopKind::ReadinessTimeout, "readiness timeout"));
            }
        }
        if retry_due {
            self.dirty = true;
        }
        for (id, kind, reason) in to_stop {
            if let Some(slot) = self.slots.get_mut(&id) {
                slot.detail = Some(reason.to_string());
                if let Some(live) = slot.live.as_mut() {
                    live.stop = Some(kind);
                }
                // 期限切れは利用側の有無にかかわらず止める。
                slot.pending_stop = Some(StopKind::Timeout);
            }
            self.notice(format!("[{}] {reason}", id.0));
            self.dirty = true;
        }
    }

    // ---- 失敗伝播と依存喪失 ----

    fn propagate(&mut self) {
        let init_failed = self.init_failed();
        loop {
            let states = self.states();
            let mut skip = Vec::new();
            for (id, slot) in &self.slots {
                if slot.runtime.state != TaskState::Waiting || slot.retry_at.is_some() {
                    continue;
                }
                let blocked_by_init = init_failed && !self.config.plan.init_boundary.contains(id);
                let blocked = slot.prepared.depends_on.iter().find(|dependency| {
                    let terminal = match (
                        self.slots.get(&dependency.task),
                        self.agents.get(&dependency.task),
                    ) {
                        (Some(dep), _) => dep.is_terminal(),
                        (None, Some(agent)) => agent.is_terminal(),
                        (None, None) => return false,
                    };
                    terminal && !condition_met(dependency.condition, &states[&dependency.task])
                });
                if blocked_by_init {
                    skip.push((id.clone(), "initialization failed".to_string()));
                } else if let Some(dependency) = blocked {
                    skip.push((
                        id.clone(),
                        format!(
                            "dependency {} did not reach {:?}",
                            dependency.task.0, dependency.condition
                        ),
                    ));
                }
            }
            if init_failed {
                for agent in self.agents.values_mut() {
                    if agent.runtime.state == TaskState::Waiting {
                        agent.runtime.state = TaskState::Skipped;
                        agent.detail = Some("initialization failed".to_string());
                        self.dirty = true;
                    }
                }
            }
            if skip.is_empty() {
                break;
            }
            for (id, reason) in skip {
                let slot = self.slots.get_mut(&id).expect("slot exists");
                slot.runtime.state = TaskState::Skipped;
                slot.detail = Some(reason.clone());
                self.recorder
                    .notice(&format!("[{}] skipped: {reason}", id.0));
            }
            self.dirty = true;
        }
    }

    fn check_dependency_lost(&mut self) {
        let states = self.states();
        let mut lost = Vec::new();
        for (id, slot) in &self.slots {
            if !slot.is_live() || slot.pending_stop.is_some() {
                continue;
            }
            if slot.prepared.on_dependency_lost != OnDependencyLost::Stop {
                continue;
            }
            let broken = slot.prepared.depends_on.iter().find(|dependency| {
                dependency.condition == DependencyCondition::Ready
                    && states
                        .get(&dependency.task)
                        .is_some_and(|state| !condition_met(DependencyCondition::Ready, state))
            });
            if let Some(dependency) = broken {
                lost.push((id.clone(), dependency.task.clone()));
            }
        }
        for (id, dependency) in lost {
            if let Some(slot) = self.slots.get_mut(&id) {
                slot.detail = Some(format!("dependency {} lost readiness", dependency.0));
            }
            self.notice(format!(
                "[{}] stopping: dependency {} is no longer ready",
                id.0, dependency.0
            ));
            self.stop_task(&id, StopKind::DependencyLost);
        }
    }

    // ---- 起動 ----

    fn live_count(&self, lifecycle: TaskLifecycle) -> u32 {
        let count = self
            .slots
            .values()
            .filter(|slot| slot.is_live() && slot.prepared.lifecycle == lifecycle)
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }

    fn launch_runnable(&mut self) {
        if self.store_degraded {
            return;
        }
        let init_pending = !self.init_done();
        if !init_pending
            && !self.init_failed()
            && let Some(hook) = self.init_hook.take()
        {
            match hook() {
                Ok(outcome) => {
                    self.notice(outcome.message.clone());
                    self.start_agent_tracking(outcome.agents);
                }
                Err(message) => {
                    self.notice(format!("@layout.create failed: {message}"));
                    self.outcome = RunOutcome::Failed;
                    self.request_stop_all(StopKind::DependencyLost);
                    return;
                }
            }
        }
        let states = self.states();
        let runnable = schedule::runnable_tasks(&self.config.plan, &states);
        for id in runnable {
            let Some(slot) = self.slots.get(&id) else {
                continue;
            };
            if slot.retry_at.is_some() {
                continue;
            }
            let limit = match slot.prepared.lifecycle {
                TaskLifecycle::Job => self.config.limits.max_jobs,
                TaskLifecycle::Service => self.config.limits.max_services,
            };
            if limit.is_some_and(|limit| self.live_count(slot.prepared.lifecycle) >= limit) {
                continue;
            }
            // 同じcheckoutの初期化は一つに制限する。
            if init_pending
                && self.config.plan.init_boundary.contains(&id)
                && self.checkout_guard.is_empty()
                && let Some(name) = self.config.checkout_lock.clone()
            {
                match locks::try_lock_all(
                    &self.state_dir.resource_locks_dir(),
                    std::slice::from_ref(&name),
                    &mut self.checkout_guard,
                ) {
                    Ok(LockAttempt::Acquired) => {}
                    Ok(LockAttempt::Busy(_)) => {
                        self.set_detail(&id, "waiting for checkout lock".to_string());
                        continue;
                    }
                    Err(error) => {
                        self.fail_launch(&id, format!("checkout lock: {error}"));
                        continue;
                    }
                }
            }
            self.launch(&id);
        }
        if !init_pending || self.init_failed() {
            self.checkout_guard.clear();
        }
    }

    fn set_detail(&mut self, id: &TaskId, detail: String) {
        if let Some(slot) = self.slots.get_mut(id)
            && slot.detail.as_deref() != Some(detail.as_str())
        {
            slot.detail = Some(detail);
            self.dirty = true;
        }
    }

    fn next_attempt(&self, id: &TaskId) -> u32 {
        self.slots[id]
            .runtime
            .attempts
            .last()
            .map_or(1, |attempt| attempt.attempt_id.0 + 1)
    }

    fn fail_launch(&mut self, id: &TaskId, reason: String) {
        let attempt = self.next_attempt(id);
        let slot = self.slots.get_mut(id).expect("slot exists");
        slot.runtime.record_attempt_result(AttemptResult {
            attempt_id: AttemptId(attempt),
            launch_evidence: LaunchEvidence {
                pid: None,
                started_at: Some(logs::rfc3339(SystemTime::now())),
                command_summary: slot.prepared.command_summary(),
            },
            outcome: AttemptOutcome::LaunchFailed(reason.clone()),
            log_position: LogPosition {
                byte_offset: 0,
                line_number: 0,
            },
        });
        slot.runtime.state = TaskState::Failed;
        slot.detail = Some(reason.clone());
        self.dirty = true;
        self.notice(format!("[{}] launch failed: {reason}", id.0));
    }

    fn launch(&mut self, id: &TaskId) {
        let attempt = self.next_attempt(id);
        let prepared = self.slots[id].prepared.clone();

        let mut guards = Vec::new();
        if !prepared.locks.is_empty() {
            match locks::try_lock_all(
                &self.state_dir.resource_locks_dir(),
                &prepared.locks,
                &mut guards,
            ) {
                Ok(LockAttempt::Acquired) => {}
                Ok(LockAttempt::Busy(name)) => {
                    self.set_detail(id, format!("waiting for lock {name}"));
                    return;
                }
                Err(error) => {
                    self.fail_launch(id, format!("lock: {error}"));
                    return;
                }
            }
        }

        // 既に使用中のポートをreadyと誤認しない(T07)。
        if let Readiness::Poll { probe, .. } = &prepared.readiness
            && let Some((host, port)) = probe.loopback_endpoint()
            && crate::runtime::readiness::tcp_probe(host, port, Duration::from_millis(300))
        {
            self.fail_launch(id, format!("port {host}:{port} is already in use"));
            return;
        }

        let secrets_file = if prepared.secret_names().is_empty() {
            SecretsFile::default()
        } else {
            match (self.config.load_secrets)() {
                Ok(file) => file,
                Err(error) => {
                    self.fail_launch(id, error.to_string());
                    return;
                }
            }
        };
        let base_env = self.config.base_env.clone();
        let lookup = |name: &str| base_env.get(&OsString::from(name)).cloned();
        let (env, masked) =
            match prepare::build_env(&self.config.base_env, &prepared.env_ops, |name| {
                secrets_file.resolve(name, lookup)
            }) {
                Ok(pair) => pair,
                Err(error) => {
                    self.fail_launch(id, error.to_string());
                    return;
                }
            };
        let masker = Masker::new(masked);

        let attempt_dir = self
            .state_dir
            .attempt_dir(&self.config.run_id, &id.0, attempt);
        if let Err(error) = self.state_dir.ensure_subdir(&attempt_dir) {
            self.fail_launch(id, error.to_string());
            return;
        }
        let writers = (
            LogWriter::create(
                attempt_dir.join(Stream::Stdout.file_name()),
                logs::DEFAULT_MAX_BYTES,
                logs::DEFAULT_GENERATIONS,
            ),
            LogWriter::create(
                attempt_dir.join(Stream::Stderr.file_name()),
                logs::DEFAULT_MAX_BYTES,
                logs::DEFAULT_GENERATIONS,
            ),
        );
        let (stdout_writer, stderr_writer) = match writers {
            (Ok(out), Ok(err)) => (out, err),
            (Err(error), _) | (_, Err(error)) => {
                self.fail_launch(id, format!("log file: {error}"));
                return;
            }
        };

        let spec = LaunchSpec {
            program: prepared.program.clone(),
            args: prepared.args.clone(),
            cwd: prepared.cwd.clone(),
            env,
        };
        let spawned = match process::spawn(&spec) {
            Ok(spawned) => spawned,
            Err(error) => {
                self.fail_launch(id, format!("{}: {error}", prepared.program));
                return;
            }
        };
        let started_at = logs::rfc3339(SystemTime::now());
        let launch_file = LaunchFile {
            run_id: self.config.run_id.clone(),
            task_id: id.0.clone(),
            attempt_id: attempt,
            command: prepared.command_summary(),
            cwd: prepared.cwd.clone(),
            child: spawned.identity.clone(),
            runner: process::observe(std::process::id()),
            started_at: started_at.clone(),
        };
        let launch_degraded = serde_json::to_vec_pretty(&launch_file)
            .ok()
            .and_then(|bytes| write_atomic(&attempt_dir.join("launch.json"), &bytes).ok())
            .is_none();

        let want_lines = Arc::new(AtomicBool::new(matches!(
            prepared.readiness,
            Readiness::Log { .. }
        )));
        let template = |stream| LogRecord {
            run_id: self.config.run_id.clone(),
            task_id: id.0.clone(),
            attempt_id: attempt,
            time: String::new(),
            stream,
            seq: 0,
            data: String::new(),
        };
        for (stream, reader, writer) in [
            (
                Stream::Stdout,
                Box::new(spawned.stdout) as Box<dyn std::io::Read + Send>,
                stdout_writer,
            ),
            (Stream::Stderr, Box::new(spawned.stderr), stderr_writer),
        ] {
            let tx = self.events_tx.clone();
            let task = id.clone();
            let want = Arc::clone(&want_lines);
            let record = template(stream);
            let masker = masker.clone();
            self.threads.push(std::thread::spawn(move || {
                let line_tx = tx.clone();
                let line_task = task.clone();
                let (lines, degraded) = logs::pump(reader, writer, &record, &masker, |line| {
                    if want.load(Ordering::Relaxed) {
                        let _ = line_tx.send(Event::Line {
                            task: line_task.clone(),
                            attempt,
                            stream,
                            text: line.to_string(),
                        });
                    }
                });
                let _ = tx.send(Event::StreamClosed {
                    task,
                    attempt,
                    lines,
                    degraded,
                });
            }));
        }

        let handle = spawned.handle.clone();
        {
            let tx = self.events_tx.clone();
            let task = id.clone();
            let run_id = self.config.run_id.clone();
            let result_path = attempt_dir.join("result.json");
            let waiter = spawned.waiter;
            self.threads.push(std::thread::spawn(move || {
                let exit = waiter
                    .wait()
                    .map(process::classify_exit)
                    .map_err(|error| error.to_string());
                if let Ok(exit) = &exit {
                    let (exit_code, signal) = match exit {
                        Exit::Code(code) => (Some(*code), None),
                        Exit::Signal(signal) => (None, Some(signal.clone())),
                    };
                    let result = ResultFile {
                        run_id,
                        task_id: task.0.clone(),
                        attempt_id: attempt,
                        exit_code,
                        signal,
                        finished_at: logs::rfc3339(SystemTime::now()),
                    };
                    if let Ok(bytes) = serde_json::to_vec_pretty(&result) {
                        let _ = write_atomic(&result_path, &bytes);
                    }
                }
                let _ = tx.send(Event::Exited {
                    task,
                    attempt,
                    exit,
                });
            }));
        }

        let probe_cancel = Arc::new(AtomicBool::new(false));
        let probe_interval = Arc::new(AtomicU64::new(0));
        let now = Instant::now();
        let mut ready_deadline = None;
        match &prepared.readiness {
            Readiness::Poll {
                probe,
                timeout,
                interval,
                ..
            } => {
                ready_deadline = Some(now + *timeout);
                probe_interval.store(
                    u64::try_from(interval.as_millis()).unwrap_or(u64::MAX),
                    Ordering::Relaxed,
                );
                let tx = self.events_tx.clone();
                let task = id.clone();
                let probe = probe.clone();
                let cancel = Arc::clone(&probe_cancel);
                let interval = Arc::clone(&probe_interval);
                self.threads.push(std::thread::spawn(move || {
                    while !cancel.load(Ordering::Relaxed) {
                        let wait = Duration::from_millis(interval.load(Ordering::Relaxed));
                        sleep_unless(&cancel, wait);
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let ok = probe.check(PROBE_TIMEOUT.min(wait.max(TICK)));
                        if tx
                            .send(Event::Probe {
                                task: task.clone(),
                                attempt,
                                ok,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }));
            }
            Readiness::Log { timeout, .. } => ready_deadline = Some(now + *timeout),
            Readiness::Started => {}
        }

        let slot = self.slots.get_mut(id).expect("slot exists");
        slot.runtime.state = TaskState::Running;
        slot.runtime.readiness = match (prepared.lifecycle, &prepared.readiness) {
            (TaskLifecycle::Job, _) => ReadinessState::NotApplicable,
            (TaskLifecycle::Service, Readiness::Started) => ReadinessState::Ready,
            (TaskLifecycle::Service, _) => ReadinessState::Pending,
        };
        slot.detail = Some(format!("attempt {attempt}"));
        slot.runtime.record_attempt_result(AttemptResult {
            attempt_id: AttemptId(attempt),
            launch_evidence: LaunchEvidence {
                pid: Some(handle.pid()),
                started_at: Some(started_at),
                command_summary: prepared.command_summary(),
            },
            outcome: AttemptOutcome::Pending,
            log_position: LogPosition {
                byte_offset: 0,
                line_number: 0,
            },
        });
        slot.live = Some(LiveAttempt {
            attempt,
            handle,
            deadline: prepared.timeout.map(|timeout| now + timeout),
            ready_deadline,
            successes: 0,
            failures: 0,
            stop: None,
            signaled_at: None,
            killed: false,
            descendants: Vec::new(),
            probe_cancel,
            probe_interval,
            want_lines,
            guards,
        });
        self.dirty = true;
        self.notice(format!(
            "[{}] started attempt {attempt} (pid {})",
            id.0,
            slot_pid(&self.slots[id])
        ));
        if launch_degraded {
            self.notice(format!("[{}] failed to write launch.json", id.0));
        }
    }

    // ---- イベント ----

    fn handle(&mut self, event: Event) {
        match event {
            Event::Line {
                task,
                attempt,
                stream,
                text,
            } => self.on_line(&task, attempt, stream, &text),
            Event::Probe { task, attempt, ok } => self.on_probe(&task, attempt, ok),
            Event::Agent { task, probe } => self.on_agent(&task, probe),
            Event::Exited {
                task,
                attempt,
                exit,
            } => self.on_exit(&task, attempt, exit),
            Event::StreamClosed {
                task,
                attempt,
                lines,
                degraded,
            } => {
                if degraded {
                    self.notice(format!("[{}] log-degraded (attempt {attempt})", task.0));
                }
                if let Some(slot) = self.slots.get_mut(&task)
                    && let Some(result) = slot
                        .runtime
                        .attempts
                        .iter_mut()
                        .find(|result| result.attempt_id.0 == attempt)
                {
                    result.log_position.line_number += lines;
                    self.dirty = true;
                }
            }
        }
    }

    /// 画面構築でのAgent起動結果を反映し、観測値の照会を始める。
    fn start_agent_tracking(&mut self, results: BTreeMap<TaskId, Result<String, String>>) {
        let now = logs::rfc3339(SystemTime::now());
        let mut notices = Vec::new();
        for (id, agent) in &mut self.agents {
            match results.get(id) {
                Some(Ok(pane)) => {
                    // Herdrの`agent.start`は入力準備ができてから成功を返すため、readyとする。
                    agent.runtime.state = TaskState::Running;
                    agent.runtime.readiness = ReadinessState::Ready;
                    agent.runtime.record_attempt_result(AttemptResult {
                        attempt_id: AttemptId(1),
                        launch_evidence: LaunchEvidence {
                            pid: None,
                            started_at: Some(now.clone()),
                            command_summary: format!("herdr agent {} in {pane}", agent.name),
                        },
                        outcome: AttemptOutcome::Pending,
                        log_position: LogPosition {
                            byte_offset: 0,
                            line_number: 0,
                        },
                    });
                    agent.detail = Some(format!("pane {pane}"));
                    if let Some(observe) = self.config.observe_agent.clone() {
                        let tx = self.events_tx.clone();
                        let task = id.clone();
                        let name = agent.name.clone();
                        let cancel = Arc::clone(&agent.cancel);
                        let interval = self.config.agent_poll;
                        self.threads.push(std::thread::spawn(move || {
                            while !cancel.load(Ordering::Relaxed) {
                                let probe = observe(&name);
                                let gone = probe == AgentProbe::Gone;
                                if tx
                                    .send(Event::Agent {
                                        task: task.clone(),
                                        probe,
                                    })
                                    .is_err()
                                    || gone
                                {
                                    break;
                                }
                                sleep_unless(&cancel, interval);
                            }
                        }));
                    }
                }
                Some(Err(reason)) => {
                    agent.runtime.state = TaskState::Failed;
                    agent.runtime.readiness = ReadinessState::Unready;
                    agent.runtime.record_attempt_result(AttemptResult {
                        attempt_id: AttemptId(1),
                        launch_evidence: LaunchEvidence {
                            pid: None,
                            started_at: Some(now.clone()),
                            command_summary: format!("herdr agent {}", agent.name),
                        },
                        outcome: AttemptOutcome::LaunchFailed(reason.clone()),
                        log_position: LogPosition {
                            byte_offset: 0,
                            line_number: 0,
                        },
                    });
                    agent.detail = Some(format!("agent start failed: {reason}"));
                    notices.push(format!("[{}] agent start failed: {reason}", id.0));
                }
                None => {
                    agent.runtime.state = TaskState::Skipped;
                    agent.detail = Some("no agent pane was created".to_string());
                }
            }
        }
        self.dirty = true;
        for notice in notices {
            self.notice(notice);
        }
    }

    /// 観測値は別フィールドへ保存し、`idle`や`done`をTaskの成功へ変換しない(T10)。
    fn on_agent(&mut self, task: &TaskId, probe: AgentProbe) {
        let Some(agent) = self.agents.get_mut(task) else {
            return;
        };
        if agent.runtime.state != TaskState::Running {
            return;
        }
        let message = match probe {
            AgentProbe::Observed(observation) => {
                if agent.runtime.agent == Some(observation) {
                    return;
                }
                agent.runtime.agent = Some(observation);
                agent.detail = Some(format!("observed: {}", observation_text(observation)));
                format!("[{}] agent {}", task.0, observation_text(observation))
            }
            AgentProbe::Unreachable(reason) => {
                let changed = agent.runtime.agent != Some(AgentObservation::Unknown);
                agent.runtime.agent = Some(AgentObservation::Unknown);
                agent.detail = Some(format!("Herdr unreachable: {reason}"));
                if !changed {
                    self.dirty = true;
                    return;
                }
                format!("[{}] agent state unknown: {reason}", task.0)
            }
            AgentProbe::Gone => {
                agent.cancel.store(true, Ordering::Relaxed);
                agent.runtime.state = TaskState::Stopped;
                agent.runtime.readiness = ReadinessState::Unready;
                agent.runtime.agent = None;
                if let Some(last) = agent.runtime.attempts.last().cloned() {
                    agent.runtime.record_attempt_result(AttemptResult {
                        outcome: AttemptOutcome::Unknown,
                        ..last
                    });
                }
                agent.detail = Some("agent exited or was replaced in its pane".to_string());
                format!("[{}] agent exited or was replaced", task.0)
            }
        };
        self.dirty = true;
        self.notice(message);
    }

    fn live_mut(&mut self, task: &TaskId, attempt: u32) -> Option<(&mut Slot, bool)> {
        let slot = self.slots.get_mut(task)?;
        // 古いAttemptの通知で現在の状態を更新しない(T16)。
        let current = slot
            .live
            .as_ref()
            .is_some_and(|live| live.attempt == attempt);
        Some((slot, current))
    }

    fn on_line(&mut self, task: &TaskId, attempt: u32, stream: Stream, text: &str) {
        let Some((slot, true)) = self.live_mut(task, attempt) else {
            return;
        };
        let Readiness::Log {
            stream: wanted,
            pattern,
            ..
        } = &slot.prepared.readiness
        else {
            return;
        };
        let stream_matches = match wanted {
            LogStream::Any => true,
            LogStream::Stdout => stream == Stream::Stdout,
            LogStream::Stderr => stream == Stream::Stderr,
        };
        if slot.runtime.readiness == ReadinessState::Pending
            && slot.runtime.state == TaskState::Running
            && stream_matches
            && pattern.is_match(text)
        {
            slot.runtime.readiness = ReadinessState::Ready;
            if let Some(live) = &slot.live {
                live.want_lines.store(false, Ordering::Relaxed);
            }
            self.dirty = true;
            self.notice(format!("[{}] ready (log matched)", task.0));
        }
    }

    fn on_probe(&mut self, task: &TaskId, attempt: u32, ok: bool) {
        let Some((slot, true)) = self.live_mut(task, attempt) else {
            return;
        };
        if slot.runtime.state != TaskState::Running {
            return;
        }
        let Readiness::Poll {
            successes, monitor, ..
        } = slot.prepared.readiness.clone()
        else {
            return;
        };
        let live = slot.live.as_mut().expect("live attempt");
        let mut message = None;
        match slot.runtime.readiness {
            ReadinessState::Pending => {
                live.successes = if ok { live.successes + 1 } else { 0 };
                if live.successes >= successes {
                    slot.runtime.readiness = ReadinessState::Ready;
                    live.failures = 0;
                    match monitor {
                        Some(monitor) => live.probe_interval.store(
                            u64::try_from(monitor.interval.as_millis()).unwrap_or(u64::MAX),
                            Ordering::Relaxed,
                        ),
                        None => live.probe_cancel.store(true, Ordering::Relaxed),
                    }
                    message = Some(format!("[{}] ready", task.0));
                }
            }
            ReadinessState::Ready | ReadinessState::Unready => {
                let Some(monitor) = monitor else { return };
                if ok {
                    live.failures = 0;
                    if slot.runtime.readiness == ReadinessState::Unready {
                        slot.runtime.readiness = ReadinessState::Ready;
                        message = Some(format!("[{}] ready again", task.0));
                    }
                } else {
                    live.failures += 1;
                    if live.failures >= monitor.failure_threshold
                        && slot.runtime.readiness == ReadinessState::Ready
                    {
                        slot.runtime.readiness = ReadinessState::Unready;
                        message = Some(format!(
                            "[{}] unready after {} failed checks",
                            task.0, live.failures
                        ));
                    }
                }
            }
            ReadinessState::NotApplicable => {}
        }
        if let Some(message) = message {
            self.dirty = true;
            self.notice(message);
        }
    }

    fn on_exit(&mut self, task: &TaskId, attempt: u32, exit: Result<Exit, String>) {
        let stopping_all = self.stopping_all.is_some();
        let Some((slot, true)) = self.live_mut(task, attempt) else {
            return;
        };
        let live = slot.live.take().expect("live attempt");
        live.probe_cancel.store(true, Ordering::Relaxed);
        drop(live.guards);
        slot.pending_stop = None;
        // T26: 停止時に記録した子孫のうち、まだ同一と照合できるものを止めて報告する。
        let survivors: Vec<u32> = live
            .descendants
            .iter()
            .filter(|identity| process::signal_if_same(identity, libc::SIGKILL))
            .map(|identity| identity.pid)
            .collect();
        let survivor_notice = (!survivors.is_empty()).then(|| {
            format!(
                "[{}] killed {} descendant process(es) left outside the process group: {:?}",
                task.0,
                survivors.len(),
                survivors
            )
        });

        let stop = live.stop;
        let lifecycle = slot.prepared.lifecycle;
        let (outcome, state, retryable, detail) = match (&exit, stop) {
            (Err(error), _) => (
                AttemptOutcome::Unknown,
                TaskState::Unknown,
                false,
                format!("wait failed: {error}"),
            ),
            (Ok(_), Some(StopKind::Timeout)) => (
                AttemptOutcome::TimedOut,
                TaskState::Failed,
                lifecycle == TaskLifecycle::Job,
                "timed out".to_string(),
            ),
            (Ok(_), Some(StopKind::ReadinessTimeout)) => (
                AttemptOutcome::TimedOut,
                TaskState::Failed,
                false,
                "readiness timed out".to_string(),
            ),
            (Ok(exit), Some(StopKind::User | StopKind::DependencyLost)) => {
                let outcome = attempt_outcome(exit);
                let state = match (lifecycle, stop) {
                    (TaskLifecycle::Service, Some(StopKind::User))
                    | (TaskLifecycle::Service, Some(StopKind::DependencyLost))
                        if slot.runtime.state != TaskState::Failed =>
                    {
                        TaskState::Stopped
                    }
                    _ => TaskState::Cancelled,
                };
                (
                    outcome,
                    state,
                    false,
                    format!("stopped ({})", exit_text(exit)),
                )
            }
            (Ok(exit), None) => match lifecycle {
                TaskLifecycle::Job => match exit {
                    Exit::Code(0) => (
                        AttemptOutcome::ExitCode(0),
                        TaskState::Succeeded,
                        false,
                        "exit 0".to_string(),
                    ),
                    Exit::Code(code) => (
                        AttemptOutcome::ExitCode(*code),
                        TaskState::Failed,
                        true,
                        format!("exit {code}"),
                    ),
                    Exit::Signal(signal) => (
                        AttemptOutcome::Signaled(signal.clone()),
                        TaskState::Failed,
                        false,
                        format!("killed by {signal}"),
                    ),
                },
                // serviceの予期しない終了は終了コード0でも失敗とする。
                TaskLifecycle::Service => (
                    AttemptOutcome::UnexpectedExit,
                    TaskState::Failed,
                    false,
                    format!("unexpected exit ({})", exit_text(exit)),
                ),
            },
        };

        if let Some(result) = slot
            .runtime
            .attempts
            .iter()
            .find(|result| result.attempt_id.0 == attempt)
            .cloned()
        {
            slot.runtime
                .record_attempt_result(AttemptResult { outcome, ..result });
        }
        if lifecycle == TaskLifecycle::Service {
            slot.runtime.readiness = ReadinessState::Unready;
        }
        let can_retry = retryable && !stopping_all && attempt < slot.prepared.max_attempts;
        if can_retry {
            slot.runtime.state = TaskState::Waiting;
            slot.retry_at = Some(Instant::now() + slot.prepared.retry_delay);
            slot.detail = Some(format!(
                "{detail}; retrying in {}s (attempt {}/{})",
                slot.prepared.retry_delay.as_secs(),
                attempt + 1,
                slot.prepared.max_attempts
            ));
        } else {
            slot.runtime.state = state;
            slot.detail = Some(detail);
        }
        let message = format!(
            "[{}] attempt {attempt}: {}",
            task.0,
            slot.detail.clone().unwrap_or_default()
        );
        self.dirty = true;
        self.notice(message);
        if let Some(notice) = survivor_notice {
            self.notice(notice);
        }
    }
}

fn observation_text(observation: AgentObservation) -> &'static str {
    match observation {
        AgentObservation::Idle => "idle",
        AgentObservation::Working => "working",
        AgentObservation::Blocked => "blocked",
        AgentObservation::Done => "done",
        AgentObservation::Unknown => "unknown",
    }
}

fn slot_pid(slot: &Slot) -> u32 {
    slot.live.as_ref().map_or(0, |live| live.handle.pid())
}

fn sleep_unless(cancel: &AtomicBool, duration: Duration) {
    let end = Instant::now() + duration;
    while !cancel.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now >= end {
            break;
        }
        std::thread::sleep((end - now).min(TICK));
    }
}

fn exit_text(exit: &Exit) -> String {
    match exit {
        Exit::Code(code) => format!("exit {code}"),
        Exit::Signal(signal) => signal.clone(),
    }
}

fn attempt_outcome(exit: &Exit) -> AttemptOutcome {
    match exit {
        Exit::Code(code) => AttemptOutcome::ExitCode(*code),
        Exit::Signal(signal) => AttemptOutcome::Signaled(signal.clone()),
    }
}

/// `plan::schedule`と同じ依存条件の判定(失敗伝播と依存喪失に使う)。
fn condition_met(condition: DependencyCondition, state: &TaskRuntimeState) -> bool {
    match condition {
        DependencyCondition::Succeeded => state.state == TaskState::Succeeded,
        DependencyCondition::Ready => {
            state.state == TaskState::Running && state.readiness == ReadinessState::Ready
        }
        DependencyCondition::Started => state.current_attempt().is_some_and(|attempt| {
            !matches!(
                attempt.outcome,
                AttemptOutcome::LaunchFailed(_) | AttemptOutcome::Unknown
            )
        }),
    }
}

#[cfg(test)]
mod tests;
