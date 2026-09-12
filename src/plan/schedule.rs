//! Coordinator/TaskRunner本体(段階B)を実装する前に、依存判定だけを副作用なしの
//! 純粋関数として固定するモジュール。docs/architecture.md 77-81行目が要求する
//! 4つの動作(初期化失敗時のAgent起動抑止、service ready後のE2E実行、再開時の
//! 二重起動防止、resumeでのタブ非置換)をここでテストする。

use std::collections::{BTreeMap, BTreeSet};

use crate::config::model::{DependencyCondition, TabId, TaskId};
use crate::plan::compile::{ExecutionPlan, PlannedTab};
use crate::state::models::{ReadinessState, TaskRuntimeState, TaskState};

pub type TaskStates = BTreeMap<TaskId, TaskRuntimeState>;

fn state_of(states: &TaskStates, id: &TaskId) -> TaskRuntimeState {
    states.get(id).cloned().unwrap_or_default()
}

fn condition_met(condition: DependencyCondition, dependency_state: &TaskRuntimeState) -> bool {
    match condition {
        DependencyCondition::Succeeded => dependency_state.state == TaskState::Succeeded,
        DependencyCondition::Ready => dependency_state.readiness == ReadinessState::Ready,
        DependencyCondition::Started => dependency_state.current_attempt().is_some(),
    }
}

/// 現在の状態集合から見て、新たに`starting`へ進めてよいTaskIdの集合を返す。
///
/// - `waiting`(未記録を含む)状態のタスクだけを対象にする
///   -> 既にsucceeded/running/starting等のタスクは対象に含まれない(二重起動防止)。
/// - 非init境界タスク(serviceとAgentを含む)は、init_boundaryの全タスクが
///   succeededでない限り対象外とする(初期化失敗時の起動抑止)。
/// - 各depends_onの条件は依存先タスクの現在状態から判定する。
pub fn runnable_tasks(plan: &ExecutionPlan, states: &TaskStates) -> BTreeSet<TaskId> {
    let init_complete = plan
        .init_boundary
        .iter()
        .all(|id| state_of(states, id).state == TaskState::Succeeded);

    plan.tasks
        .values()
        .filter(|task| state_of(states, &task.id).state == TaskState::Waiting)
        .filter(|task| plan.init_boundary.contains(&task.id) || init_complete)
        .filter(|task| {
            task.depends_on.iter().all(|dependency| {
                condition_met(dependency.condition, &state_of(states, &dependency.task))
            })
        })
        .map(|task| task.id.clone())
        .collect()
}

/// resume時に既存のtab所有情報と突き合わせ、新規作成すべきtabだけを返す。
/// 既にownedなtab(=既存のtab)は初期化対象に含めない(既存タブを置換しない)。
#[derive(Debug, Clone, Default)]
pub struct ResumedWorkspace {
    pub existing_tabs: BTreeSet<TabId>,
}

pub fn tabs_to_create<'p>(
    plan: &'p ExecutionPlan,
    resumed: &ResumedWorkspace,
) -> Vec<&'p PlannedTab> {
    plan.workspace
        .tabs
        .iter()
        .filter(|tab| !resumed.existing_tabs.contains(&tab.id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load;
    use crate::plan::compile;
    use crate::state::models::{
        AttemptId, AttemptOutcome, AttemptResult, LaunchEvidence, LogPosition,
    };

    fn example_plan() -> ExecutionPlan {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflow.yaml");
        let spec = load::load_file(path).expect("example config should load");
        compile::compile(&spec).expect("example workflow should compile")
    }

    fn succeeded() -> TaskRuntimeState {
        TaskRuntimeState {
            state: TaskState::Succeeded,
            ..Default::default()
        }
    }

    fn failed() -> TaskRuntimeState {
        TaskRuntimeState {
            state: TaskState::Failed,
            ..Default::default()
        }
    }

    fn running() -> TaskRuntimeState {
        TaskRuntimeState {
            state: TaskState::Running,
            ..Default::default()
        }
    }

    fn running_and_ready() -> TaskRuntimeState {
        TaskRuntimeState {
            state: TaskState::Running,
            readiness: ReadinessState::Ready,
            ..Default::default()
        }
    }

    // --- 1. 初期化失敗時はAgentを起動しない -------------------------------------

    #[test]
    fn bootstrap_failure_blocks_all_post_init_tasks_including_agent() {
        let plan = example_plan();
        let mut states = TaskStates::new();
        states.insert(TaskId::from("toolchain"), failed());

        let runnable = runnable_tasks(&plan, &states);

        assert!(
            !runnable.contains(&TaskId::from("developer")),
            "agent must not start"
        );
        assert!(
            !runnable.contains(&TaskId::from("server")),
            "service must not start"
        );
        assert!(
            runnable.is_empty(),
            "no post-toolchain task should be runnable"
        );
    }

    #[test]
    fn init_boundary_success_unblocks_post_init_tasks() {
        let plan = example_plan();
        let mut states = TaskStates::new();
        states.insert(TaskId::from("toolchain"), succeeded());
        states.insert(TaskId::from("dependencies"), succeeded());
        states.insert(TaskId::from("generate"), succeeded());

        let runnable = runnable_tasks(&plan, &states);

        assert!(runnable.contains(&TaskId::from("server")));
        assert!(runnable.contains(&TaskId::from("developer")));
    }

    #[test]
    fn blanket_block_holds_even_without_direct_dependency_chain() {
        // developerはtoolchainに直接依存していないが、init境界のどれか1つでも
        // 失敗すれば非initタスクは一律で止まる、という一般則を確認する。
        let plan = example_plan();
        let mut states = TaskStates::new();
        states.insert(TaskId::from("toolchain"), succeeded());
        states.insert(TaskId::from("dependencies"), succeeded());
        states.insert(TaskId::from("generate"), failed());

        let runnable = runnable_tasks(&plan, &states);

        assert!(!runnable.contains(&TaskId::from("developer")));
        assert!(!runnable.contains(&TaskId::from("server")));
    }

    // --- 2. serviceのready後にE2Eを実行する ------------------------------------

    #[test]
    fn tests_task_waits_for_server_readiness() {
        let plan = example_plan();
        let mut states = TaskStates::new();
        states.insert(TaskId::from("toolchain"), succeeded());
        states.insert(TaskId::from("dependencies"), succeeded());
        states.insert(TaskId::from("generate"), succeeded());
        states.insert(TaskId::from("server"), running());

        let runnable = runnable_tasks(&plan, &states);

        assert!(!runnable.contains(&TaskId::from("tests")));
    }

    #[test]
    fn tests_task_becomes_runnable_once_server_is_ready() {
        let plan = example_plan();
        let mut states = TaskStates::new();
        states.insert(TaskId::from("toolchain"), succeeded());
        states.insert(TaskId::from("dependencies"), succeeded());
        states.insert(TaskId::from("generate"), succeeded());
        states.insert(TaskId::from("server"), running_and_ready());

        let runnable = runnable_tasks(&plan, &states);

        assert!(runnable.contains(&TaskId::from("tests")));
    }

    #[test]
    fn server_never_reports_succeeded_even_when_ready() {
        // state-model.md: serviceは準備完了しても`succeeded`にはならない。
        // `tests`がcondition: readyで待っている限り、succeeded判定に化けないことを確認する。
        let ready_but_running = running_and_ready();
        assert!(!condition_met(
            DependencyCondition::Succeeded,
            &ready_but_running
        ));
        assert!(condition_met(
            DependencyCondition::Ready,
            &ready_but_running
        ));
    }

    // --- 3. 再開時に既存タスクを二重起動しない ----------------------------------

    #[test]
    fn resuming_with_partial_progress_does_not_restart_finished_or_running_tasks() {
        let plan = example_plan();
        let mut states = TaskStates::new();
        states.insert(TaskId::from("toolchain"), succeeded());
        states.insert(TaskId::from("dependencies"), succeeded());
        states.insert(TaskId::from("generate"), succeeded());
        states.insert(TaskId::from("server"), running_and_ready());
        states.insert(TaskId::from("tests"), running());

        let runnable = runnable_tasks(&plan, &states);

        assert_eq!(runnable, BTreeSet::from([TaskId::from("developer")]));
    }

    #[test]
    fn runnable_tasks_is_idempotent_given_unchanged_states() {
        let plan = example_plan();
        let mut states = TaskStates::new();
        states.insert(TaskId::from("toolchain"), succeeded());

        let first = runnable_tasks(&plan, &states);
        let second = runnable_tasks(&plan, &states);
        assert_eq!(first, second);

        // 起動記録を追加しても、再度waitingへ戻していない限り再起動対象にならない。
        let mut runtime = states.get(&TaskId::from("toolchain")).unwrap().clone();
        runtime.record_attempt_result(AttemptResult {
            attempt_id: AttemptId(1),
            launch_evidence: LaunchEvidence {
                pid: Some(1),
                started_at: None,
                command_summary: "mise install".to_string(),
            },
            outcome: AttemptOutcome::ExitCode(0),
            log_position: LogPosition {
                byte_offset: 0,
                line_number: 0,
            },
        });
        states.insert(TaskId::from("toolchain"), runtime);
        let third = runnable_tasks(&plan, &states);
        assert_eq!(first, third);
    }

    // --- 4. resumeで既存タブを置換しない ----------------------------------------

    #[test]
    fn resume_does_not_recreate_existing_tab() {
        let plan = example_plan();
        let resumed = ResumedWorkspace {
            existing_tabs: BTreeSet::from([TabId::from("development")]),
        };

        assert!(tabs_to_create(&plan, &resumed).is_empty());
    }

    #[test]
    fn fresh_run_proposes_the_declared_tab() {
        let plan = example_plan();
        let resumed = ResumedWorkspace::default();

        let tabs = tabs_to_create(&plan, &resumed);

        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].id, TabId::from("development"));
    }
}
