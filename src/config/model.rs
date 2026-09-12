use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct TaskId(pub String);

impl From<&str> for TaskId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct InputId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct TabId(pub String);

impl From<&str> for TabId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct PaneId(pub String);

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkflowSpec {
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub inputs: BTreeMap<InputId, InputDef>,
    pub worktree: WorktreeConfig,
    #[serde(default)]
    pub files: Option<FilesConfig>,
    #[serde(default)]
    pub defaults: Option<DefaultsConfig>,
    pub bootstrap: BootstrapConfig,
    pub tasks: BTreeMap<TaskId, TaskDef>,
    pub workspace: WorkspaceConfig,
    #[serde(default)]
    pub execution: ExecutionConfig,
    #[serde(default)]
    pub cleanup: Option<CleanupConfig>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InputDef {
    #[serde(rename = "type")]
    pub input_type: InputType,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<InputDefault>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InputType {
    String,
    Integer,
    Boolean,
}

// 宣言型(InputType)とdefault値の一致検証は config/validate.rs へ先送りする。
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum InputDefault {
    String(String),
    Integer(i64),
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorktreeConfig {
    pub mode: WorktreeMode,
    /// "${inputs.branch}" のような値置換は生文字列のまま保持する。置換処理は後続段階。
    pub branch: String,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub existing_branch: Option<ExistingBranchPolicy>,
    #[serde(default)]
    pub if_already_checked_out: Option<CheckedOutPolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorktreeMode {
    Create,
    // "attach" はexamples/workflow.yamlに現れず、specs/worktree.mdにも
    // 入力キーの定義がまだ無いため今回は実装しない(docs/planning/open-questions.md Q11管理)。
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExistingBranchPolicy {
    Use,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckedOutPolicy {
    Error,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FilesConfig {
    pub source: FilesSource,
    #[serde(default)]
    pub copy: Vec<FileCopyRule>,
    #[serde(default)]
    pub symlinks: SymlinkPolicy,
    #[serde(default)]
    pub require_ignored: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FilesSource {
    Primary,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileCopyRule {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub if_exists: Option<IfExistsPolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum IfExistsPolicy {
    #[default]
    Error,
    Skip,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SymlinkPolicy {
    #[default]
    Reject,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DefaultsConfig {
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub job_timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BootstrapConfig {
    pub targets: Vec<TaskId>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum TaskDef {
    #[serde(rename_all = "camelCase")]
    Command {
        lifecycle: TaskLifecycle,
        runner: RunnerKind,
        argv: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default)]
        depends_on: Vec<DependsOn>,
        #[serde(default)]
        timeout_seconds: Option<u64>,
        #[serde(default)]
        wait_for: Option<WaitFor>,
        #[serde(default)]
        stop: Option<StopConfig>,
        #[serde(default)]
        on_dependency_lost: Option<OnDependencyLost>,
    },
    #[serde(rename_all = "camelCase")]
    Agent {
        agent: AgentSpec,
        #[serde(default)]
        depends_on: Vec<DependsOn>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskLifecycle {
    Job,
    Service,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunnerKind {
    Supervised,
    // "pane" 等の対話Runnerは docs/planning/roadmap.md 段階Eの範囲。
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DependsOn {
    pub task: TaskId,
    pub condition: DependencyCondition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DependencyCondition {
    Succeeded,
    Ready,
    Started,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum WaitFor {
    #[serde(rename_all = "camelCase")]
    Http {
        url: String,
        status: u16,
        timeout_seconds: u64,
        interval_milliseconds: u64,
        consecutive_successes: u32,
    },
    // Tcp/Log/Agentはspecs/observability.mdに確認方式の説明はあるが
    // 具体的なキー名が定義されていないため今回は実装しない
    // (configuration.md「本文にないキーを推測して追加しない」、open-questions.md Q11管理)。
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StopConfig {
    #[serde(default)]
    pub grace_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OnDependencyLost {
    Stop,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSpec {
    pub kind: AgentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentKind {
    Claude,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceConfig {
    pub label: String,
    #[serde(default)]
    pub initial_tab: Option<InitialTabPolicy>,
    pub tabs: Vec<TabConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InitialTabPolicy {
    Preserve,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TabConfig {
    pub id: TabId,
    pub label: String,
    pub layout: LayoutConfig,
    pub panes: Vec<PaneConfig>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum LayoutConfig {
    #[serde(rename_all = "camelCase")]
    Grid { columns: u32, rows: u32 },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PaneConfig {
    pub id: PaneId,
    pub label: String,
    pub view: PaneView,
    pub placement: Placement,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum PaneView {
    #[serde(rename_all = "camelCase")]
    Logs {
        task: TaskId,
    },
    #[serde(rename_all = "camelCase")]
    Agent {
        task: TaskId,
    },
    Status,
    Shell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Placement {
    pub column: u32,
    pub row: u32,
    #[serde(default = "Placement::default_span")]
    pub col_span: u32,
    #[serde(default = "Placement::default_span")]
    pub row_span: u32,
}

impl Placement {
    fn default_span() -> u32 {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct ExecutionConfig {
    pub max_concurrent_jobs: Option<u32>,
    pub max_live_services: Option<u32>,
    pub max_live_agents: Option<u32>,
    pub on_failure: OnFailurePolicy,
    pub retries: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum OnFailurePolicy {
    #[default]
    StopDependents,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CleanupConfig {
    pub on_stop: OnStopConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OnStopConfig {
    pub processes: ProcessesCleanupPolicy,
    pub panes: PreserveOnlyPolicy,
    pub worktree: PreserveOnlyPolicy,
    pub branch: PreserveOnlyPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessesCleanupPolicy {
    StopOwned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PreserveOnlyPolicy {
    Preserve,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_yaml() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflow.yaml");
        std::fs::read_to_string(path).expect("examples/workflow.yaml should be readable")
    }

    #[test]
    fn example_workflow_deserializes_into_workflow_spec() {
        let spec: WorkflowSpec =
            serde_yaml_ng::from_str(&example_yaml()).expect("example config should parse");
        assert_eq!(spec.name, "web-development");
        assert_eq!(spec.tasks.len(), 6);
        assert_eq!(spec.workspace.tabs.len(), 1);
        assert_eq!(spec.workspace.tabs[0].panes.len(), 5);
    }

    #[test]
    fn unknown_top_level_key_is_rejected() {
        let yaml = format!("{}\nunknownTopLevelKey: true\n", example_yaml());
        assert!(serde_yaml_ng::from_str::<WorkflowSpec>(&yaml).is_err());
    }

    #[test]
    fn unknown_key_inside_command_task_is_rejected() {
        let yaml = example_yaml().replacen(
            "toolchain:\n    type: command",
            "toolchain:\n    type: command\n    bogus: 1",
            1,
        );
        assert!(serde_yaml_ng::from_str::<WorkflowSpec>(&yaml).is_err());
    }

    #[test]
    fn service_task_lifecycle_round_trips() {
        let yaml = example_yaml();
        let spec: WorkflowSpec = serde_yaml_ng::from_str(&yaml).expect("should parse");
        match &spec.tasks[&TaskId::from("server")] {
            TaskDef::Command { lifecycle, .. } => assert_eq!(*lifecycle, TaskLifecycle::Service),
            TaskDef::Agent { .. } => panic!("server task should be a command"),
        }
    }
}
