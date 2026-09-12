use std::path::Path;

use herdr_workflow::config::load;
use herdr_workflow::config::model::TaskId;
use herdr_workflow::plan::compile;

fn example_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflow.yaml")
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
fn unknown_top_level_key_is_rejected() {
    let base = std::fs::read_to_string(example_path()).unwrap();
    let yaml = format!("{base}\nunknownTopLevelKey: true\n");
    assert!(load::load_str(&yaml).is_err());
}
