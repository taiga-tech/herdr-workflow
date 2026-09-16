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
    if let Some(TaskDef::Command { depends_on, .. }) = spec.tasks.get_mut(&TaskId::from("tests")) {
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
fn unknown_bootstrap_target_is_rejected_without_panicking() {
    let mut spec = example_spec();
    spec.bootstrap.targets = vec![TaskId::from("missing")];
    let err = compile(&spec).expect_err("unknown bootstrap target should be rejected");
    assert_eq!(
        err,
        CompileError::UnknownBootstrapTarget(TaskId::from("missing"))
    );
}

#[test]
fn started_job_dependency_is_included_in_init_boundary() {
    let mut spec = example_spec();
    if let Some(TaskDef::Command { depends_on, .. }) = spec.tasks.get_mut(&TaskId::from("generate"))
    {
        depends_on[0].condition = DependencyCondition::Started;
    }
    let plan = compile(&spec).expect("job bootstrap dependency should compile");
    let expected: BTreeSet<TaskId> = ["toolchain", "dependencies", "generate"]
        .into_iter()
        .map(TaskId::from)
        .collect();
    assert_eq!(plan.init_boundary, expected);
}

#[test]
fn service_in_bootstrap_dependency_closure_is_rejected() {
    let mut spec = example_spec();
    if let Some(TaskDef::Command { depends_on, .. }) = spec.tasks.get_mut(&TaskId::from("server")) {
        depends_on.clear();
    }
    if let Some(TaskDef::Command { depends_on, .. }) = spec.tasks.get_mut(&TaskId::from("generate"))
    {
        depends_on[0] = DependsOn {
            task: TaskId::from("server"),
            condition: DependencyCondition::Started,
        };
    }
    let err = compile(&spec).expect_err("service bootstrap dependency should be rejected");
    assert_eq!(
        err,
        CompileError::InvalidBootstrapDependency(TaskId::from("server"))
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
fn agent_pane_referencing_command_task_is_rejected() {
    let mut spec = example_spec();
    spec.workspace.tabs[0].panes[1].view = PaneView::Agent {
        task: TaskId::from("server"),
    };
    let err = compile(&spec).expect_err("agent pane must reference an agent task");
    assert_eq!(
        err,
        CompileError::InvalidAgentPaneTask {
            tab: TabId::from("development"),
            pane: crate::config::model::PaneId("B".to_string()),
            task: TaskId::from("server"),
        }
    );
}

#[test]
fn agent_task_without_agent_pane_is_rejected() {
    let mut spec = example_spec();
    spec.workspace.tabs[0].panes[1].view = PaneView::Shell;
    let err = compile(&spec).expect_err("agent task must have one agent pane");
    assert_eq!(
        err,
        CompileError::InvalidAgentPaneCount {
            task: TaskId::from("developer"),
            count: 0,
        }
    );
}

#[test]
fn agent_task_with_multiple_agent_panes_is_rejected() {
    let mut spec = example_spec();
    spec.workspace.tabs[0].panes[0].view = PaneView::Agent {
        task: TaskId::from("developer"),
    };
    let err = compile(&spec).expect_err("agent task must not have multiple agent panes");
    assert_eq!(
        err,
        CompileError::InvalidAgentPaneCount {
            task: TaskId::from("developer"),
            count: 2,
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
    // exampleの配置をP1-P5のピンホイール配置へ変えてスライス不能にする。
    spec.workspace.tabs[0].layout = crate::config::model::LayoutConfig::Grid {
        columns: 3,
        rows: 3,
    };
    spec.workspace.tabs[0].panes = vec![
        crate::config::model::PaneConfig {
            id: crate::config::model::PaneId("P1".to_string()),
            label: "P1".to_string(),
            view: PaneView::Agent {
                task: TaskId::from("developer"),
            },
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

#[test]
fn compile_rejects_spec_with_zero_execution_limit() {
    let mut spec = example_spec();
    spec.execution.max_concurrent_jobs = Some(0);
    let err = compile(&spec).expect_err("zero execution limit should be rejected");
    assert!(matches!(
        err,
        CompileError::Invalid(crate::config::validate::ValidateError::ZeroExecutionLimit {
            field: "maxConcurrentJobs"
        })
    ));
}
