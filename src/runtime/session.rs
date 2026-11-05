//! 設定・入力・対象checkoutからRunの実行構成を組み立てる。
//!
//! 外部操作(子プロセスの起動)の前に、入力値、値置換、秘密値参照、実行対象を
//! 確定する。単独CLI(headless)ではAgentとpane依存のタスクを実行しない
//! (docs/specs/workflow.md「初期化と画面構築の順序」)。

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::config::model::{InputId, TaskDef, TaskId, WorkflowSpec, WorktreeMode};
use crate::config::plugin::PluginConfig;
use crate::config::secrets::{self, SecretsFile};
use crate::config::substitute;
use crate::plan::compile::{self, CompileError, ExecutionPlan, PlannedTaskKind};
use crate::runtime::engine::{EnvMap, Limits, RunConfig};
use crate::runtime::git;
use crate::runtime::prepare::{self, PrepareError};

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Compile(#[from] CompileError),
    #[error(transparent)]
    Validate(#[from] crate::config::validate::ValidateError),
    #[error(transparent)]
    Prepare(#[from] PrepareError),
    #[error(transparent)]
    Secrets(#[from] secrets::SecretsError),
    #[error("unknown target task {0:?}")]
    UnknownTarget(String),
    #[error(
        "task {0:?} is an agent task; a headless run cannot start agents (use a target whose dependencies are commands only)"
    )]
    HeadlessAgent(String),
    #[error(transparent)]
    Git(#[from] git::GitError),
    #[error(
        "agent task {task:?} depends on {dependency:?}, which is not a bootstrap task; agents start when the layout is created after initialization"
    )]
    AgentDependency { task: String, dependency: String },
    #[error("checkout {path} is not usable: {message}")]
    Checkout { path: PathBuf, message: String },
}

/// 秘密値定義の読み込み口。
pub type SecretsLoader =
    std::sync::Arc<dyn Fn() -> Result<SecretsFile, secrets::SecretsError> + Send + Sync>;

pub struct RunRequest<'a> {
    pub spec: &'a WorkflowSpec,
    pub plugin_config: &'a PluginConfig,
    pub inputs: &'a BTreeMap<InputId, String>,
    pub checkout: &'a Path,
    pub target: Option<TaskId>,
    /// 自動受付の初期化だけを行う(`bootstrap.targets`とその依存先)。
    pub bootstrap_only: bool,
    /// Herdr連携で実行する。AgentタスクはHerdrの画面構築時に起動する。
    pub herdr: bool,
    pub base_env: EnvMap,
    /// 秘密値定義の読み込み口(Run開始時とAttempt起動直前に呼ぶ)。
    pub load_secrets: SecretsLoader,
    /// workflow設定のあるディレクトリ(同じリポジトリかの確認に使う)。
    pub config_dir: Option<PathBuf>,
}

/// Run IDを生成する。時刻(ミリ秒)、PID、連番から作り、辞書順が時刻順になる。
#[must_use]
pub fn new_run_id() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    format!(
        "{millis:013x}-{:06x}-{:04x}",
        std::process::id() & 0x00ff_ffff,
        COUNTER.fetch_add(1, Ordering::Relaxed) & 0xffff
    )
}

/// 対象タスクと、その依存先・初期化対象の推移閉包。
fn select_tasks(
    plan: &ExecutionPlan,
    target: Option<&TaskId>,
    bootstrap_only: bool,
) -> BTreeSet<TaskId> {
    if bootstrap_only {
        return plan.init_boundary.clone();
    }
    let Some(target) = target else {
        return plan.tasks.keys().cloned().collect();
    };
    let mut selected = BTreeSet::new();
    let mut stack: Vec<TaskId> = std::iter::once(target.clone())
        .chain(plan.init_boundary.iter().cloned())
        .collect();
    while let Some(id) = stack.pop() {
        if !selected.insert(id.clone()) {
            continue;
        }
        if let Some(task) = plan.tasks.get(&id) {
            stack.extend(
                task.depends_on
                    .iter()
                    .map(|dependency| dependency.task.clone()),
            );
        }
    }
    selected
}

pub struct PreparedRun {
    pub config: RunConfig,
    pub plan_hash: String,
    pub checkout: PathBuf,
    /// `files.copy`の設定とコピー元(main worktree)。外部操作なので実行時に行う。
    pub files: Option<(crate::config::model::FilesConfig, PathBuf)>,
    /// `attach`で採用した借用checkout。後片付けで削除しない。
    pub borrowed: bool,
    /// Herdr連携で起動するAgentタスク。
    pub agents: Vec<TaskId>,
}

/// checkout実行ロックの名前(正規化したパスのハッシュ)。
#[must_use]
pub fn checkout_lock_name(checkout: &Path) -> String {
    let digest = Sha256::digest(checkout.as_os_str().as_encoded_bytes());
    let hex: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("checkout-{hex}")
}

pub fn prepare_run(request: RunRequest<'_>, run_id: String) -> Result<PreparedRun, SessionError> {
    let checkout =
        std::fs::canonicalize(request.checkout).map_err(|error| SessionError::Checkout {
            path: request.checkout.to_path_buf(),
            message: error.to_string(),
        })?;
    if !checkout.is_dir() {
        return Err(SessionError::Checkout {
            path: checkout,
            message: "not a directory".to_string(),
        });
    }
    let plan = compile::compile_with_plugin_config(request.spec, request.plugin_config)?;
    if let Some(target) = &request.target
        && !plan.tasks.contains_key(target)
    {
        return Err(SessionError::UnknownTarget(target.0.clone()));
    }
    let resolved_inputs = substitute::resolve_inputs(request.spec, request.inputs)?;
    let resolved_values = substitute::substitute_all(request.spec, &resolved_inputs)?;

    // attachは既存checkoutをGit CLIで照合してから採用する(T37)。
    let expected_branch = resolved_values
        .0
        .get("worktree.branch")
        .filter(|branch| !branch.contains("${"))
        .cloned();
    let borrowed = request.spec.worktree.mode == WorktreeMode::Attach;
    let git_info = if borrowed {
        Some(git::validate_attach(
            &checkout,
            request.config_dir.as_deref(),
            expected_branch.as_deref(),
        )?)
    } else {
        git::inspect(&checkout).ok()
    };
    let files = match &request.spec.files {
        Some(files) if !files.copy.is_empty() => {
            let primary = git_info
                .as_ref()
                .and_then(|info| info.primary.clone())
                .ok_or_else(|| git::GitError::NoPrimary {
                    path: checkout.clone(),
                })?;
            Some((files.clone(), primary))
        }
        _ => None,
    };

    let selected = select_tasks(&plan, request.target.as_ref(), request.bootstrap_only);
    let mut agents = Vec::new();
    for id in &selected {
        if plan.tasks[id].kind == PlannedTaskKind::Agent {
            if !request.herdr {
                return Err(SessionError::HeadlessAgent(id.0.clone()));
            }
            // Agentは画面構築時(初期化の成功後)に起動するため、依存先は初期化対象に限る。
            if let Some(dependency) = plan.tasks[id]
                .depends_on
                .iter()
                .find(|dependency| !plan.init_boundary.contains(&dependency.task))
            {
                return Err(SessionError::AgentDependency {
                    task: id.0.clone(),
                    dependency: dependency.task.0.clone(),
                });
            }
            agents.push(id.clone());
        }
    }
    let selected: BTreeSet<TaskId> = selected
        .into_iter()
        .filter(|id| plan.tasks[id].kind != PlannedTaskKind::Agent)
        .collect();

    let mut tasks = BTreeMap::new();
    for id in &selected {
        tasks.insert(
            id.clone(),
            prepare::prepare_task(request.spec, id, &resolved_values, &checkout)?,
        );
    }

    // Run開始時に、参照した秘密値がすべて定義済みであることを外部操作の前に確認する。
    let names: BTreeSet<&str> = tasks
        .values()
        .flat_map(prepare::PreparedTask::secret_names)
        .collect();
    if !names.is_empty() {
        let file = (request.load_secrets)()?;
        let base = &request.base_env;
        file.check_references(names.iter().copied(), |name: &str| {
            base.get(&OsString::from(name)).cloned()
        })?;
    }

    let hash = crate::cli::plan_hash(request.spec, request.plugin_config, &resolved_inputs);
    let execution = &request.spec.execution;
    let load_secrets = request.load_secrets;
    let config = RunConfig {
        run_id,
        plan,
        tasks,
        target: request.target,
        base_env: request.base_env,
        limits: Limits {
            max_jobs: execution.max_concurrent_jobs,
            max_services: execution.max_live_services,
        },
        load_secrets: Box::new(move || load_secrets()),
        checkout_lock: Some(checkout_lock_name(&checkout)),
        initial: BTreeMap::new(),
        on_init_complete: None,
    };
    Ok(PreparedRun {
        config,
        plan_hash: hash,
        checkout,
        files,
        borrowed,
        agents,
    })
}

/// 計画に含まれるcommandタスクの一覧(表示用)。
#[must_use]
pub fn command_tasks(spec: &WorkflowSpec) -> Vec<TaskId> {
    spec.tasks
        .iter()
        .filter(|(_, def)| matches!(def, TaskDef::Command { .. }))
        .map(|(id, _)| id.clone())
        .collect()
}
