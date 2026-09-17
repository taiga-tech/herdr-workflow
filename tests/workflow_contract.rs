use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use herdr_workflow::config::load;
use herdr_workflow::config::model::TaskId;
use herdr_workflow::plan::compile;

fn example_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflow.yaml")
}

struct TempConfigDir(PathBuf);

impl TempConfigDir {
    fn new(contents: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "herdr-workflow-cli-contract-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("temporary config directory should be created");
        std::fs::write(path.join("config.yaml"), contents)
            .expect("temporary plugin config should be written");
        Self(path)
    }
}

impl Drop for TempConfigDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn example_workflow_loads_and_compiles() {
    let spec = load::load_file(example_path()).expect("examples/workflow.yaml should parse");
    let plan = compile::compile(&spec).expect("example workflow should compile");

    assert_eq!(
        plan.bootstrap_targets,
        std::collections::BTreeSet::from([TaskId::from("generate")])
    );
    assert_eq!(
        plan.init_boundary,
        ["toolchain", "dependencies", "generate"]
            .into_iter()
            .map(TaskId::from)
            .collect::<std::collections::BTreeSet<_>>()
    );
}

#[test]
fn binary_reads_plugin_config_from_environment() {
    let plugin_config = TempConfigDir::new(
        "version: 1\nlimits:\n  layout:\n    maxGridDimension: 64\n    maxPanesPerTab: 4\n",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-workflow"))
        .args([
            "validate",
            "--config",
            example_path().to_str().expect("example path must be UTF-8"),
            "--json",
        ])
        .env("HERDR_PLUGIN_CONFIG_DIR", &plugin_config.0)
        .output()
        .expect("herdr-workflow binary should run");

    assert_eq!(output.status.code(), Some(2));
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be valid JSON");
    assert_eq!(stdout["ok"], false);
    assert_eq!(stdout["error"]["stage"], "compile");
    assert!(
        stdout["error"]["message"]
            .as_str()
            .expect("error message should be a string")
            .contains("must not contain more than 4 panes")
    );
}

#[test]
fn unknown_top_level_key_is_rejected() {
    let base = std::fs::read_to_string(example_path()).unwrap();
    let yaml = format!("{base}\nunknownTopLevelKey: true\n");
    assert!(load::load_str(&yaml).is_err());
}

#[test]
fn example_workflow_layout_slices_into_expected_split_tree() {
    let spec = load::load_file(example_path()).expect("examples/workflow.yaml should parse");
    let plan = compile::compile(&spec).expect("example workflow should compile");
    assert_eq!(plan.workspace.tabs.len(), 1);
    assert!(matches!(
        plan.workspace.tabs[0].layout.root,
        herdr_workflow::plan::layout::SplitNode::Split { .. }
    ));
}

#[test]
fn duplicate_task_key_in_config_is_rejected_before_compile() {
    let base = std::fs::read_to_string(example_path()).unwrap();
    let yaml = base.replacen(
        "toolchain:\n    type: command",
        "toolchain:\n    type: command\n    lifecycle: job\n    runner: supervised\n    argv: [mise, install]\n  toolchain:\n    type: command",
        1,
    );
    assert!(load::load_str(&yaml).is_err());
}

#[test]
fn undefined_input_variable_in_config_is_rejected() {
    let mut spec = load::load_file(example_path()).expect("examples/workflow.yaml should parse");
    spec.worktree.branch = "${inputs.missing}".to_string();
    let err = compile::compile(&spec).expect_err("undefined input variable should be rejected");
    assert!(matches!(err, compile::CompileError::Invalid(_)));
}

#[test]
fn pinwheel_grid_is_rejected_as_not_sliceable() {
    use herdr_workflow::config::model::{LayoutConfig, PaneConfig, PaneId, PaneView, Placement};

    let mut spec = load::load_file(example_path()).expect("examples/workflow.yaml should parse");
    spec.workspace.tabs[0].layout = LayoutConfig::Grid {
        columns: 3,
        rows: 3,
    };
    let make = |id: &str, column, row, col_span, row_span| PaneConfig {
        id: PaneId(id.to_string()),
        label: id.to_string(),
        view: PaneView::Shell,
        placement: Placement {
            column,
            row,
            col_span,
            row_span,
        },
    };
    spec.workspace.tabs[0].panes = vec![
        make("P1", 1, 1, 2, 1),
        make("P2", 3, 1, 1, 2),
        make("P3", 2, 3, 2, 1),
        make("P4", 1, 2, 1, 2),
        make("P5", 2, 2, 1, 1),
    ];
    spec.workspace.tabs[0].panes[0].view = PaneView::Agent {
        task: TaskId::from("developer"),
    };
    let err = compile::compile(&spec).expect_err("pinwheel layout should be rejected");
    assert!(matches!(
        err,
        compile::CompileError::Layout(
            herdr_workflow::plan::layout::LayoutError::NotSliceable { .. }
        )
    ));
}
