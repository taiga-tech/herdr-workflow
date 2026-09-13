use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::config::model::{
    DependencyCondition, DependsOn, OnDependencyLost, PaneConfig, PaneId, PaneView, TabConfig,
    TabId, TaskDef, TaskId, TaskLifecycle, WorkflowSpec,
};
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
    #[error("pane {pane:?} in tab {tab:?} references unknown task {task:?}")]
    UnknownPaneTask {
        tab: TabId,
        pane: PaneId,
        task: TaskId,
    },
    #[error(transparent)]
    Invalid(#[from] ValidateError),
    #[error(transparent)]
    Layout(#[from] LayoutError),
}

pub fn compile(spec: &WorkflowSpec) -> Result<ExecutionPlan, CompileError> {
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
        if tasks[target].kind != PlannedTaskKind::Job {
            return Err(CompileError::InvalidBootstrapTarget(target.clone()));
        }
    }

    let init_boundary = compute_init_boundary(&tasks, &bootstrap_targets);

    let mut planned_tabs = Vec::with_capacity(spec.workspace.tabs.len());
    for tab in &spec.workspace.tabs {
        planned_tabs.push(plan_tab(tab, &tasks)?);
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
) -> BTreeSet<TaskId> {
    let mut boundary: BTreeSet<TaskId> = BTreeSet::new();
    let mut queue: Vec<TaskId> = bootstrap_targets.iter().cloned().collect();
    while let Some(id) = queue.pop() {
        if !boundary.insert(id.clone()) {
            continue;
        }
        for dependency in &tasks[&id].depends_on {
            if dependency.condition == DependencyCondition::Succeeded {
                queue.push(dependency.task.clone());
            }
        }
    }
    boundary
}

fn plan_tab(
    tab: &TabConfig,
    tasks: &BTreeMap<TaskId, PlannedTask>,
) -> Result<PlannedTab, CompileError> {
    let mut panes = Vec::with_capacity(tab.panes.len());
    for pane in &tab.panes {
        panes.push(plan_pane(tab, pane, tasks)?);
    }
    let layout = layout::compile_layout(tab)?;
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
mod tests {
    use super::*;
    use crate::config::load;

    fn example_spec() -> WorkflowSpec {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflow.yaml");
        load::load_file(path).expect("example config should load")
    }

    #[test]
    fn compiles_the_example_workflow_without_error() {
        let plan = compile(&example_spec()).expect("example workflow should compile");
        assert_eq!(plan.tasks.len(), 6);
        assert_eq!(plan.topo_order.len(), 6);
    }

    #[test]
    fn init_boundary_contains_toolchain_dependencies_and_generate_only() {
        let plan = compile(&example_spec()).expect("example workflow should compile");
        let expected: BTreeSet<TaskId> = ["toolchain", "dependencies", "generate"]
            .into_iter()
            .map(TaskId::from)
            .collect();
        assert_eq!(plan.init_boundary, expected);
        assert_eq!(
            plan.bootstrap_targets,
            BTreeSet::from([TaskId::from("generate")])
        );
    }

    #[test]
    fn unknown_dependency_target_is_rejected() {
        let mut spec = example_spec();
        if let Some(TaskDef::Command { depends_on, .. }) =
            spec.tasks.get_mut(&TaskId::from("dependencies"))
        {
            depends_on.push(DependsOn {
                task: TaskId::from("missing"),
                condition: DependencyCondition::Succeeded,
            });
        }
        let err = compile(&spec).expect_err("unknown dependency should be rejected");
        assert_eq!(
            err,
            CompileError::UnknownDependency {
                task: TaskId::from("dependencies"),
                dependency: TaskId::from("missing"),
            }
        );
    }

    #[test]
    fn cyclic_dependency_between_two_tasks_is_rejected() {
        let mut spec = example_spec();
        if let Some(TaskDef::Command { depends_on, .. }) =
            spec.tasks.get_mut(&TaskId::from("toolchain"))
        {
            depends_on.push(DependsOn {
                task: TaskId::from("dependencies"),
                condition: DependencyCondition::Succeeded,
            });
        }
        let err = compile(&spec).expect_err("cycle should be rejected");
        assert!(matches!(err, CompileError::CyclicDependency(_)));
    }

    #[test]
    fn service_task_cannot_be_depended_on_with_succeeded() {
        let mut spec = example_spec();
        if let Some(TaskDef::Command { depends_on, .. }) =
            spec.tasks.get_mut(&TaskId::from("tests"))
        {
            depends_on[0].condition = DependencyCondition::Succeeded;
        }
        let err = compile(&spec).expect_err("service dependency with succeeded should be rejected");
        assert_eq!(
            err,
            CompileError::UnsatisfiableCondition {
                task: TaskId::from("tests"),
                on: TaskId::from("server"),
                condition: DependencyCondition::Succeeded,
            }
        );
    }

    #[test]
    fn agent_task_cannot_be_depended_on_with_succeeded() {
        let mut spec = example_spec();
        spec.tasks.insert(
            TaskId::from("follow_up"),
            TaskDef::Command {
                lifecycle: crate::config::model::TaskLifecycle::Job,
                runner: crate::config::model::RunnerKind::Supervised,
                argv: vec!["true".to_string()],
                env: Default::default(),
                depends_on: vec![DependsOn {
                    task: TaskId::from("developer"),
                    condition: DependencyCondition::Succeeded,
                }],
                timeout_seconds: None,
                wait_for: None,
                stop: None,
                on_dependency_lost: None,
            },
        );
        let err = compile(&spec).expect_err("agent dependency with succeeded should be rejected");
        assert_eq!(
            err,
            CompileError::UnsatisfiableCondition {
                task: TaskId::from("follow_up"),
                on: TaskId::from("developer"),
                condition: DependencyCondition::Succeeded,
            }
        );
    }

    #[test]
    fn job_task_cannot_be_depended_on_with_ready() {
        let mut spec = example_spec();
        if let Some(TaskDef::Command { depends_on, .. }) =
            spec.tasks.get_mut(&TaskId::from("dependencies"))
        {
            depends_on[0].condition = DependencyCondition::Ready;
        }
        let err = compile(&spec).expect_err("job dependency with ready should be rejected");
        assert_eq!(
            err,
            CompileError::UnsatisfiableCondition {
                task: TaskId::from("dependencies"),
                on: TaskId::from("toolchain"),
                condition: DependencyCondition::Ready,
            }
        );
    }

    #[test]
    fn bootstrap_target_must_be_a_job() {
        let mut spec = example_spec();
        spec.bootstrap.targets.push(TaskId::from("server"));
        let err = compile(&spec).expect_err("service bootstrap target should be rejected");
        assert_eq!(
            err,
            CompileError::InvalidBootstrapTarget(TaskId::from("server"))
        );
    }

    #[test]
    fn pane_referencing_unknown_task_is_rejected() {
        let mut spec = example_spec();
        spec.workspace.tabs[0].panes[0].view = PaneView::Logs {
            task: TaskId::from("missing"),
        };
        let err = compile(&spec).expect_err("pane referencing unknown task should be rejected");
        assert_eq!(
            err,
            CompileError::UnknownPaneTask {
                tab: TabId::from("development"),
                pane: crate::config::model::PaneId("A".to_string()),
                task: TaskId::from("missing"),
            }
        );
    }

    #[test]
    fn compile_rejects_spec_with_undefined_input_variable() {
        let mut spec = example_spec();
        spec.worktree.branch = "${inputs.missing}".to_string();
        let err = compile(&spec).expect_err("undefined input variable should be rejected");
        assert!(matches!(err, CompileError::Invalid(_)));
    }

    #[test]
    fn compile_rejects_spec_with_unsliceable_layout() {
        let mut spec = example_spec();
        // A/B/C/D/Eの5枚から6枚のピンホイール配置へ変えてスライス不能にする。
        spec.workspace.tabs[0].layout = crate::config::model::LayoutConfig::Grid {
            columns: 3,
            rows: 3,
        };
        spec.workspace.tabs[0].panes = vec![
            crate::config::model::PaneConfig {
                id: crate::config::model::PaneId("P1".to_string()),
                label: "P1".to_string(),
                view: PaneView::Shell,
                placement: crate::config::model::Placement {
                    column: 1,
                    row: 1,
                    col_span: 2,
                    row_span: 1,
                },
            },
            crate::config::model::PaneConfig {
                id: crate::config::model::PaneId("P2".to_string()),
                label: "P2".to_string(),
                view: PaneView::Shell,
                placement: crate::config::model::Placement {
                    column: 3,
                    row: 1,
                    col_span: 1,
                    row_span: 2,
                },
            },
            crate::config::model::PaneConfig {
                id: crate::config::model::PaneId("P3".to_string()),
                label: "P3".to_string(),
                view: PaneView::Shell,
                placement: crate::config::model::Placement {
                    column: 2,
                    row: 3,
                    col_span: 2,
                    row_span: 1,
                },
            },
            crate::config::model::PaneConfig {
                id: crate::config::model::PaneId("P4".to_string()),
                label: "P4".to_string(),
                view: PaneView::Shell,
                placement: crate::config::model::Placement {
                    column: 1,
                    row: 2,
                    col_span: 1,
                    row_span: 2,
                },
            },
            crate::config::model::PaneConfig {
                id: crate::config::model::PaneId("P5".to_string()),
                label: "P5".to_string(),
                view: PaneView::Shell,
                placement: crate::config::model::Placement {
                    column: 2,
                    row: 2,
                    col_span: 1,
                    row_span: 1,
                },
            },
        ];
        let err = compile(&spec).expect_err("pinwheel layout should be rejected");
        assert!(matches!(
            err,
            CompileError::Layout(LayoutError::NotSliceable { .. })
        ));
    }

    #[test]
    fn compiled_tab_includes_split_tree_for_the_example_workspace() {
        let plan = compile(&example_spec()).expect("example workflow should compile");
        assert_eq!(plan.workspace.tabs.len(), 1);
        assert!(matches!(
            plan.workspace.tabs[0].layout.root,
            crate::plan::layout::SplitNode::Split { .. }
        ));
    }
}
