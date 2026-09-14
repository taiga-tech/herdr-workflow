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
mod tests;
