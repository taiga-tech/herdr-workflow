//! `schemars::schema_for!(WorkflowSpec)`で生成したJSON Schemaが実際のJSON Schema仕様に
//! 準拠して`examples/workflow.yaml`を妥当と判定することを確認する(ADR-0005)。
//! あわせて、JSON Schemaは構造検証のみを担い、DAG・gridの意味検証はRust側
//! (`config::validate`・`plan::compile`)が担うという役割分担を、構造的には正しいが
//! 意味的に不正な設定がスキーマ検証を通ることで裏付ける。

use std::path::Path;

use herdr_workflow::config::model::WorkflowSpec;

fn example_spec_as_json() -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflow.yaml");
    let yaml = std::fs::read_to_string(path).expect("examples/workflow.yaml should be readable");
    let spec: WorkflowSpec = serde_yaml_ng::from_str(&yaml).expect("example config should parse");
    serde_json::to_value(&spec).expect("WorkflowSpec should serialize to JSON")
}

fn schema_value() -> serde_json::Value {
    let schema = schemars::schema_for!(WorkflowSpec);
    serde_json::to_value(&schema).expect("schema should serialize to JSON")
}

#[test]
fn example_workflow_is_valid_against_generated_schema() {
    let schema = schema_value();
    let instance = example_spec_as_json();
    let result = jsonschema::validate(&schema, &instance);
    assert!(
        result.is_ok(),
        "example workflow should satisfy the generated schema: {result:?}"
    );
}

#[test]
fn wrong_type_for_version_is_rejected_by_schema() {
    let schema = schema_value();
    let mut instance = example_spec_as_json();
    instance["version"] = serde_json::json!("not-a-number");
    let result = jsonschema::validate(&schema, &instance);
    assert!(
        result.is_err(),
        "a string version should fail structural validation"
    );
}

#[test]
fn cyclic_dependency_is_structurally_valid_but_semantically_wrong() {
    // 循環依存はスキーマ上「オブジェクトの配列」として構造は正しいため、JSON Schemaだけでは
    // 検出できない。この意味検証はplan::compile::compileが担う(役割分担の確認)。
    let schema = schema_value();
    let mut instance = example_spec_as_json();
    instance["tasks"]["toolchain"]["dependsOn"] = serde_json::json!([
        { "task": "dependencies", "condition": "succeeded" }
    ]);
    let result = jsonschema::validate(&schema, &instance);
    assert!(
        result.is_ok(),
        "schema validation only checks structure, not DAG cycles: {result:?}"
    );

    let spec: WorkflowSpec =
        serde_json::from_value(instance).expect("still structurally a WorkflowSpec");
    let compile_result = herdr_workflow::plan::compile::compile(&spec);
    assert!(
        compile_result.is_err(),
        "plan::compile must reject the cycle that the schema alone cannot detect"
    );
}
