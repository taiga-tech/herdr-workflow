use super::*;
use crate::config::load;
use crate::plan::compile;
use crate::state::models::{AttemptId, AttemptOutcome, AttemptResult, LaunchEvidence, LogPosition};

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

#[test]
fn started_dependency_inside_bootstrap_does_not_deadlock_initial_schedule() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflow.yaml");
    let mut spec = load::load_file(path).expect("example config should load");
    if let Some(crate::config::model::TaskDef::Command { depends_on, .. }) =
        spec.tasks.get_mut(&TaskId::from("generate"))
    {
        depends_on[0].condition = DependencyCondition::Started;
    }
    let plan = compile::compile(&spec).expect("started job dependency should compile");

    let runnable = runnable_tasks(&plan, &TaskStates::new());

    assert_eq!(runnable, BTreeSet::from([TaskId::from("toolchain")]));
}

#[test]
fn started_condition_is_not_met_when_launch_failed() {
    let mut runtime = TaskRuntimeState::default();
    runtime.record_attempt_result(AttemptResult {
        attempt_id: AttemptId(1),
        launch_evidence: LaunchEvidence {
            pid: None,
            started_at: None,
            command_summary: "mise install".to_string(),
        },
        outcome: AttemptOutcome::LaunchFailed("command not found".to_string()),
        log_position: LogPosition {
            byte_offset: 0,
            line_number: 0,
        },
    });

    assert!(!condition_met(DependencyCondition::Started, &runtime));
}

#[test]
fn started_condition_is_not_met_when_latest_launch_is_unknown() {
    // 過去に起動済みの世代があっても、最新世代が起動未確認なら依存を解放しない。
    let mut runtime = TaskRuntimeState::default();
    for (attempt_id, outcome) in [
        (1, AttemptOutcome::ExitCode(0)),
        (2, AttemptOutcome::Unknown),
    ] {
        runtime.record_attempt_result(AttemptResult {
            attempt_id: AttemptId(attempt_id),
            launch_evidence: LaunchEvidence {
                pid: None,
                started_at: None,
                command_summary: "mise install".to_string(),
            },
            outcome,
            log_position: LogPosition {
                byte_offset: 0,
                line_number: 0,
            },
        });
    }

    assert!(!condition_met(DependencyCondition::Started, &runtime));
}

#[test]
fn started_condition_is_met_once_an_attempt_actually_launched() {
    let mut runtime = TaskRuntimeState::default();
    runtime.record_attempt_result(AttemptResult {
        attempt_id: AttemptId(1),
        launch_evidence: LaunchEvidence {
            pid: Some(1234),
            started_at: None,
            command_summary: "mise install".to_string(),
        },
        outcome: AttemptOutcome::Pending,
        log_position: LogPosition {
            byte_offset: 0,
            line_number: 0,
        },
    });

    assert!(condition_met(DependencyCondition::Started, &runtime));
}

// --- 2. serviceのready後にE2Eを実行する ------------------------------------

#[test]
fn ready_condition_requires_a_running_service() {
    // readinessの古い値が残っていても、生存中でなければ依存を解放しない。
    for state in [
        TaskState::Waiting,
        TaskState::Starting,
        TaskState::Succeeded,
        TaskState::Failed,
        TaskState::Stopping,
        TaskState::Stopped,
        TaskState::Cancelled,
        TaskState::Skipped,
        TaskState::Unknown,
    ] {
        let runtime = TaskRuntimeState {
            state,
            ..running_and_ready()
        };
        assert!(
            !condition_met(DependencyCondition::Ready, &runtime),
            "ready must not be met in {state:?}"
        );
    }

    assert!(condition_met(
        DependencyCondition::Ready,
        &running_and_ready()
    ));
}

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
