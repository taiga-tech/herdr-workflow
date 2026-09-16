use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::config::model::{
    DependencyCondition, DependsOn, OnDependencyLost, PaneConfig, PaneId, PaneView, TabConfig,
    TabId, TaskDef, TaskId, TaskLifecycle, WorkflowSpec,
};
use crate::config::plugin::PluginConfig;
use crate::config::validate::{self, ValidateError};
use crate::plan::graph;
use crate::plan::layout::{self, LayoutError, SplitTree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlannedTaskKind {
    Job,
    Service,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannedTask {
    pub id: TaskId,
    pub kind: PlannedTaskKind,
    pub depends_on: Vec<DependsOn>,
    pub on_dependency_lost: Option<OnDependencyLost>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannedPane {
    pub id: PaneId,
    /// StatusやShellのようにタスクを参照しないpaneは`None`。
    pub task: Option<TaskId>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannedTab {
    pub id: TabId,
    pub panes: Vec<PlannedPane>,
    pub layout: SplitTree,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannedWorkspace {
    pub label: String,
    pub tabs: Vec<PlannedTab>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExecutionPlan {
    pub tasks: BTreeMap<TaskId, PlannedTask>,
    pub topo_order: Vec<TaskId>,
    pub bootstrap_targets: BTreeSet<TaskId>,
    /// `bootstrap.targets`とその`succeeded`依存先の推移閉包(すべてjob)。
    /// worktree準備・ファイルコピー・レイアウト作成の名前付き操作ノード
    /// (docs/architecture.md 39-53行目)は、該当モジュール実装時に別途
    /// グラフへ組み込む。今回は「初期化対象でないタスクはinit_boundaryが
    /// 全succeeded後にのみ実行可」というスケジューリング規則をplan::scheduleに持たせる。
    pub init_boundary: BTreeSet<TaskId>,
    pub workspace: PlannedWorkspace,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CompileError {
    #[error("task {task:?} depends on unknown task {dependency:?}")]
    UnknownDependency { task: TaskId, dependency: TaskId },
    #[error("cyclic dependency: {}", .0.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(" -> "))]
    CyclicDependency(Vec<TaskId>),
    #[error(
        "task {task:?} declares condition {condition:?} on {on:?}, which {on:?} cannot satisfy"
    )]
    UnsatisfiableCondition {
        task: TaskId,
        on: TaskId,
        condition: DependencyCondition,
    },
    #[error("bootstrap target {0:?} must be a job-lifecycle command task")]
    InvalidBootstrapTarget(TaskId),
    #[error("bootstrap target {0:?} does not exist")]
    UnknownBootstrapTarget(TaskId),
    #[error("bootstrap dependency {0:?} must be a job-lifecycle command task")]
    InvalidBootstrapDependency(TaskId),
    #[error("pane {pane:?} in tab {tab:?} references unknown task {task:?}")]
    UnknownPaneTask {
        tab: TabId,
        pane: PaneId,
        task: TaskId,
    },
    #[error("agent pane {pane:?} in tab {tab:?} references non-agent task {task:?}")]
    InvalidAgentPaneTask {
        tab: TabId,
        pane: PaneId,
        task: TaskId,
    },
    #[error("agent task {task:?} must have exactly one agent pane, found {count}")]
    InvalidAgentPaneCount { task: TaskId, count: usize },
    #[error(transparent)]
    Invalid(#[from] ValidateError),
    #[error(transparent)]
    Layout(#[from] LayoutError),
}

pub fn compile(spec: &WorkflowSpec) -> Result<ExecutionPlan, CompileError> {
    compile_with_plugin_config(spec, &PluginConfig::default())
}

pub fn compile_with_plugin_config(
    spec: &WorkflowSpec,
    plugin_config: &PluginConfig,
) -> Result<ExecutionPlan, CompileError> {
    validate::validate(spec)?;

    let mut tasks: BTreeMap<TaskId, PlannedTask> = BTreeMap::new();
    for (id, def) in &spec.tasks {
        let (kind, depends_on, on_dependency_lost) = match def {
            TaskDef::Command {
                lifecycle,
                depends_on,
                on_dependency_lost,
                ..
            } => {
                let kind = match lifecycle {
                    TaskLifecycle::Job => PlannedTaskKind::Job,
                    TaskLifecycle::Service => PlannedTaskKind::Service,
                };
                (kind, depends_on.clone(), *on_dependency_lost)
            }
            TaskDef::Agent { depends_on, .. } => (PlannedTaskKind::Agent, depends_on.clone(), None),
        };
        tasks.insert(
            id.clone(),
            PlannedTask {
                id: id.clone(),
                kind,
                depends_on,
                on_dependency_lost,
            },
        );
    }

    for task in tasks.values() {
        for dependency in &task.depends_on {
            if !tasks.contains_key(&dependency.task) {
                return Err(CompileError::UnknownDependency {
                    task: task.id.clone(),
                    dependency: dependency.task.clone(),
                });
            }
        }
    }

    let all_task_ids: BTreeSet<TaskId> = tasks.keys().cloned().collect();
    let depends_on_map: BTreeMap<TaskId, Vec<TaskId>> = tasks
        .values()
        .map(|task| {
            (
                task.id.clone(),
                task.depends_on.iter().map(|d| d.task.clone()).collect(),
            )
        })
        .collect();
    let topo_order = graph::topo_order(&all_task_ids, &depends_on_map)
        .map_err(CompileError::CyclicDependency)?;

    for task in tasks.values() {
        for dependency in &task.depends_on {
            let target_kind = tasks[&dependency.task].kind;
            let satisfiable = matches!(
                (dependency.condition, target_kind),
                (DependencyCondition::Succeeded, PlannedTaskKind::Job)
                    | (DependencyCondition::Ready, PlannedTaskKind::Service)
                    | (DependencyCondition::Ready, PlannedTaskKind::Agent)
                    | (DependencyCondition::Started, _)
            );
            if !satisfiable {
                return Err(CompileError::UnsatisfiableCondition {
                    task: task.id.clone(),
                    on: dependency.task.clone(),
                    condition: dependency.condition,
                });
            }
        }
    }

    let bootstrap_targets: BTreeSet<TaskId> = spec.bootstrap.targets.iter().cloned().collect();
    for target in &bootstrap_targets {
        let Some(task) = tasks.get(target) else {
            return Err(CompileError::UnknownBootstrapTarget(target.clone()));
        };
        if task.kind != PlannedTaskKind::Job {
            return Err(CompileError::InvalidBootstrapTarget(target.clone()));
        }
    }

    let init_boundary = compute_init_boundary(&tasks, &bootstrap_targets)?;

    validate_agent_panes(&spec.workspace.tabs, &tasks)?;

    let mut planned_tabs = Vec::with_capacity(spec.workspace.tabs.len());
    for tab in &spec.workspace.tabs {
        planned_tabs.push(plan_tab(
            tab,
            &tasks,
            plugin_config.max_grid_dimension(),
            plugin_config.max_panes_per_tab(),
        )?);
    }

    Ok(ExecutionPlan {
        tasks,
        topo_order,
        bootstrap_targets,
        init_boundary,
        workspace: PlannedWorkspace {
            label: spec.workspace.label.clone(),
            tabs: planned_tabs,
        },
    })
}

fn compute_init_boundary(
    tasks: &BTreeMap<TaskId, PlannedTask>,
    bootstrap_targets: &BTreeSet<TaskId>,
) -> Result<BTreeSet<TaskId>, CompileError> {
    let mut boundary: BTreeSet<TaskId> = BTreeSet::new();
    let mut queue: Vec<TaskId> = bootstrap_targets.iter().cloned().collect();
    while let Some(id) = queue.pop() {
        if !boundary.insert(id.clone()) {
            continue;
        }
        // 既存タスクのみが登録される不変条件はUnknownDependencyチェックで保証済み。
        let task = &tasks[&id];
        if task.kind != PlannedTaskKind::Job {
            return Err(CompileError::InvalidBootstrapDependency(id));
        }
        for dependency in &task.depends_on {
            queue.push(dependency.task.clone());
        }
    }
    Ok(boundary)
}

fn validate_agent_panes(
    tabs: &[TabConfig],
    tasks: &BTreeMap<TaskId, PlannedTask>,
) -> Result<(), CompileError> {
    let mut pane_counts: BTreeMap<TaskId, usize> = BTreeMap::new();
    for tab in tabs {
        for pane in &tab.panes {
            let PaneView::Agent { task } = &pane.view else {
                continue;
            };
            let Some(planned_task) = tasks.get(task) else {
                return Err(CompileError::UnknownPaneTask {
                    tab: tab.id.clone(),
                    pane: pane.id.clone(),
                    task: task.clone(),
                });
            };
            if planned_task.kind != PlannedTaskKind::Agent {
                return Err(CompileError::InvalidAgentPaneTask {
                    tab: tab.id.clone(),
                    pane: pane.id.clone(),
                    task: task.clone(),
                });
            }
            *pane_counts.entry(task.clone()).or_default() += 1;
        }
    }

    for task in tasks
        .values()
        .filter(|task| task.kind == PlannedTaskKind::Agent)
    {
        let count = pane_counts.get(&task.id).copied().unwrap_or_default();
        if count != 1 {
            return Err(CompileError::InvalidAgentPaneCount {
                task: task.id.clone(),
                count,
            });
        }
    }
    Ok(())
}

fn plan_tab(
    tab: &TabConfig,
    tasks: &BTreeMap<TaskId, PlannedTask>,
    max_grid_dimension: u32,
    max_panes_per_tab: u32,
) -> Result<PlannedTab, CompileError> {
    let mut panes = Vec::with_capacity(tab.panes.len());
    for pane in &tab.panes {
        panes.push(plan_pane(tab, pane, tasks)?);
    }
    let layout = layout::compile_layout_with_limits(tab, max_grid_dimension, max_panes_per_tab)?;
    Ok(PlannedTab {
        id: tab.id.clone(),
        panes,
        layout,
    })
}

fn plan_pane(
    tab: &TabConfig,
    pane: &PaneConfig,
    tasks: &BTreeMap<TaskId, PlannedTask>,
) -> Result<PlannedPane, CompileError> {
    let task = match &pane.view {
        PaneView::Logs { task } | PaneView::Agent { task } => {
            if !tasks.contains_key(task) {
                return Err(CompileError::UnknownPaneTask {
                    tab: tab.id.clone(),
                    pane: pane.id.clone(),
                    task: task.clone(),
                });
            }
            Some(task.clone())
        }
        PaneView::Status | PaneView::Shell => None,
    };
    Ok(PlannedPane {
        id: pane.id.clone(),
        task,
    })
}

#[cfg(test)]
mod tests;
