//! `validate`/`plan`サブコマンド(docs/reference/cli.md)。承認・実行を伴う
//! `trust`/`up`/`run`等は段階B以降のため実装しない。副作用(標準出力・
//! `process::exit`)と実行ロジックを分離し、`execute`は純粋関数にする。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::config::model::{InputId, WorkflowSpec};
use crate::config::{load, substitute};
use crate::plan::compile;

const DEFAULT_CONFIG_PATH: &str = ".herdr/workflow.yaml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Validate {
        config: PathBuf,
        json: bool,
    },
    Plan {
        config: PathBuf,
        inputs: Vec<(String, String)>,
        json: bool,
    },
    Schema {
        output: Option<PathBuf>,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CliError {
    #[error("missing subcommand; expected one of: validate, plan")]
    MissingSubcommand,
    #[error("unknown subcommand {0:?}")]
    UnknownSubcommand(String),
    #[error("unknown flag {0:?}")]
    UnknownFlag(String),
    #[error("flag {0} requires a value")]
    MissingFlagValue(String),
    #[error("--input value must be KEY=VALUE, got {0:?}")]
    MalformedInput(String),
}

pub fn parse_args(args: &[String]) -> Result<Command, CliError> {
    let Some((subcommand, rest)) = args.split_first() else {
        return Err(CliError::MissingSubcommand);
    };

    let mut config = PathBuf::from(DEFAULT_CONFIG_PATH);
    let mut json = false;
    let mut inputs = Vec::new();
    let mut output: Option<PathBuf> = None;

    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::MissingFlagValue("--config".to_string()))?;
                config = PathBuf::from(value);
            }
            "--json" => json = true,
            "--input" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::MissingFlagValue("--input".to_string()))?;
                let (key, val) = value
                    .split_once('=')
                    .ok_or_else(|| CliError::MalformedInput(value.clone()))?;
                inputs.push((key.to_string(), val.to_string()));
            }
            "--output" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::MissingFlagValue("--output".to_string()))?;
                output = Some(PathBuf::from(value));
            }
            other => return Err(CliError::UnknownFlag(other.to_string())),
        }
    }

    match subcommand.as_str() {
        "validate" => Ok(Command::Validate { config, json }),
        "plan" => Ok(Command::Plan {
            config,
            inputs,
            json,
        }),
        "schema" => Ok(Command::Schema { output }),
        other => Err(CliError::UnknownSubcommand(other.to_string())),
    }
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    stage: &'static str,
    message: String,
}

#[derive(Debug, Serialize)]
struct ValidateOutcome {
    ok: bool,
}

#[derive(Debug, Serialize)]
struct PlanOutcome {
    ok: bool,
    plan: PlanReport,
}

#[derive(Debug, Serialize)]
struct ErrorOutcome {
    ok: bool,
    error: ErrorEnvelope,
}

#[derive(Debug, Serialize)]
struct HashInput<'a> {
    spec: &'a WorkflowSpec,
    resolved_inputs: &'a substitute::ResolvedInputs,
}

#[derive(Debug, Serialize)]
struct PlanReport {
    hash: String,
    #[serde(rename = "bootstrapTargets")]
    bootstrap_targets: std::collections::BTreeSet<crate::config::model::TaskId>,
    #[serde(rename = "topoOrder")]
    topo_order: Vec<crate::config::model::TaskId>,
    tasks: std::collections::BTreeMap<crate::config::model::TaskId, compile::PlannedTask>,
    workspace: compile::PlannedWorkspace,
    #[serde(rename = "resolvedInputs")]
    resolved_inputs: substitute::ResolvedInputs,
    #[serde(rename = "resolvedValues")]
    resolved_values: BTreeMap<String, String>,
}

/// (exit_code, stdoutへ出す文字列) を返す純粋関数。プロセスには触れない。
pub fn execute(command: &Command) -> (i32, String) {
    match command {
        Command::Validate { config, json } => run_validate(config, *json),
        Command::Plan {
            config,
            inputs,
            json,
        } => run_plan(config, inputs, *json),
        Command::Schema { output } => run_schema(output.as_deref()),
    }
}

fn load_and_compile(
    config: &PathBuf,
) -> Result<(WorkflowSpec, compile::ExecutionPlan), ErrorEnvelope> {
    let spec = load::load_file(config).map_err(|err| ErrorEnvelope {
        stage: "load",
        message: err.to_string(),
    })?;
    let plan = compile::compile(&spec).map_err(|err| ErrorEnvelope {
        stage: "compile",
        message: err.to_string(),
    })?;
    Ok((spec, plan))
}

fn run_validate(config: &PathBuf, json: bool) -> (i32, String) {
    match load_and_compile(config) {
        Ok(_) => {
            let output = if json {
                serde_json::to_string(&ValidateOutcome { ok: true }).expect("serializable")
            } else {
                "設定は妥当です".to_string()
            };
            (0, output)
        }
        Err(envelope) => (2, error_output(json, envelope)),
    }
}

fn run_plan(config: &PathBuf, provided_inputs: &[(String, String)], json: bool) -> (i32, String) {
    let (spec, plan) = match load_and_compile(config) {
        Ok(pair) => pair,
        Err(envelope) => return (2, error_output(json, envelope)),
    };

    let provided: BTreeMap<InputId, String> = provided_inputs
        .iter()
        .map(|(k, v)| (InputId(k.clone()), v.clone()))
        .collect();

    let resolved = match substitute::resolve_inputs(&spec, &provided) {
        Ok(r) => r,
        Err(err) => {
            return (
                2,
                error_output(
                    json,
                    ErrorEnvelope {
                        stage: "validate",
                        message: err.to_string(),
                    },
                ),
            );
        }
    };

    let resolved_values = match substitute::substitute_all(&spec, &resolved) {
        Ok(v) => v,
        Err(err) => {
            return (
                2,
                error_output(
                    json,
                    ErrorEnvelope {
                        stage: "validate",
                        message: err.to_string(),
                    },
                ),
            );
        }
    };

    let hash = plan_hash(&spec, &resolved);
    let report = PlanReport {
        hash,
        bootstrap_targets: plan.bootstrap_targets.clone(),
        topo_order: plan.topo_order.clone(),
        tasks: plan.tasks.clone(),
        workspace: plan.workspace.clone(),
        resolved_inputs: resolved,
        resolved_values: resolved_values.0,
    };

    let output = if json {
        let outcome = PlanOutcome {
            ok: true,
            plan: report,
        };
        serde_json::to_string(&outcome).expect("serializable")
    } else {
        format!(
            "計画は妥当です(hash={}, タスク数={})",
            report.hash,
            report.tasks.len()
        )
    };
    (0, output)
}

/// `WorkflowSpec`のJSON Schemaを生成する。`output`未指定ならstdoutへ出す文字列を返し、
/// 指定時はファイルへ書き込んで完了メッセージを返す(ADR-0005: 型からスキーマを生成し、
/// 生成物と型を別々に手作業で更新しない)。
fn run_schema(output: Option<&std::path::Path>) -> (i32, String) {
    let schema = schemars::schema_for!(WorkflowSpec);
    let json = serde_json::to_string_pretty(&schema).expect("schema must serialize");
    match output {
        Some(path) => match std::fs::write(path, format!("{json}\n")) {
            Ok(()) => (0, format!("スキーマを書き込みました: {}", path.display())),
            Err(err) => (
                2,
                format!(
                    "エラー(write): failed to write schema to {}: {err}",
                    path.display()
                ),
            ),
        },
        None => (0, json),
    }
}

fn error_output(json: bool, envelope: ErrorEnvelope) -> String {
    if json {
        let outcome = ErrorOutcome {
            ok: false,
            error: envelope,
        };
        serde_json::to_string(&outcome).expect("serializable")
    } else {
        format!("エラー({}): {}", envelope.stage, envelope.message)
    }
}

fn plan_hash(spec: &WorkflowSpec, resolved: &substitute::ResolvedInputs) -> String {
    let hash_input = HashInput {
        spec,
        resolved_inputs: resolved,
    };
    let canonical = serde_json::to_string(&hash_input).expect("WorkflowSpec must serialize");
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut hex, "{byte:02x}").expect("writing into a String never fails");
    }
    format!("sha256:{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_config() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/workflow.yaml")
    }

    #[test]
    fn parses_validate_with_config_flag() {
        let args = vec![
            "validate".to_string(),
            "--config".to_string(),
            "foo.yaml".to_string(),
        ];
        let command = parse_args(&args).unwrap();
        assert_eq!(
            command,
            Command::Validate {
                config: PathBuf::from("foo.yaml"),
                json: false
            }
        );
    }

    #[test]
    fn parses_plan_with_multiple_inputs_and_json_flag() {
        let args = vec![
            "plan".to_string(),
            "--input".to_string(),
            "branch=feature/x".to_string(),
            "--input".to_string(),
            "port=3001".to_string(),
            "--json".to_string(),
        ];
        let command = parse_args(&args).unwrap();
        assert_eq!(
            command,
            Command::Plan {
                config: PathBuf::from(DEFAULT_CONFIG_PATH),
                inputs: vec![
                    ("branch".to_string(), "feature/x".to_string()),
                    ("port".to_string(), "3001".to_string()),
                ],
                json: true,
            }
        );
    }

    #[test]
    fn rejects_input_without_equals_sign() {
        let args = vec![
            "plan".to_string(),
            "--input".to_string(),
            "branch".to_string(),
        ];
        let err = parse_args(&args).unwrap_err();
        assert_eq!(err, CliError::MalformedInput("branch".to_string()));
    }

    #[test]
    fn rejects_unknown_flag() {
        let args = vec!["validate".to_string(), "--bogus".to_string()];
        let err = parse_args(&args).unwrap_err();
        assert_eq!(err, CliError::UnknownFlag("--bogus".to_string()));
    }

    #[test]
    fn missing_subcommand_is_rejected() {
        let err = parse_args(&[]).unwrap_err();
        assert_eq!(err, CliError::MissingSubcommand);
    }

    #[test]
    fn validate_command_succeeds_for_example_workflow() {
        let (code, output) = execute(&Command::Validate {
            config: example_config(),
            json: true,
        });
        assert_eq!(code, 0);
        assert!(output.contains("\"ok\":true"));
    }

    #[test]
    fn validate_command_reports_error_for_broken_config() {
        let (code, output) = execute(&Command::Validate {
            config: PathBuf::from("does/not/exist.yaml"),
            json: true,
        });
        assert_eq!(code, 2);
        assert!(output.contains("\"stage\":\"load\""));
    }

    #[test]
    fn plan_command_produces_hash_and_resolved_inputs() {
        let (code, output) = execute(&Command::Plan {
            config: example_config(),
            inputs: vec![("branch".to_string(), "feature/x".to_string())],
            json: true,
        });
        assert_eq!(code, 0);
        assert!(output.contains("\"hash\":\"sha256:"));
        assert!(output.contains("feature/x"));
    }

    #[test]
    fn plan_command_missing_required_input_exits_with_config_error() {
        let (code, output) = execute(&Command::Plan {
            config: example_config(),
            inputs: vec![],
            json: true,
        });
        assert_eq!(code, 2);
        assert!(output.contains("\"stage\":\"validate\""));
    }

    #[test]
    fn parses_schema_with_output_flag() {
        let args = vec![
            "schema".to_string(),
            "--output".to_string(),
            "schema/workflow.schema.json".to_string(),
        ];
        let command = parse_args(&args).unwrap();
        assert_eq!(
            command,
            Command::Schema {
                output: Some(PathBuf::from("schema/workflow.schema.json")),
            }
        );
    }

    #[test]
    fn parses_schema_without_output_flag() {
        let command = parse_args(&["schema".to_string()]).unwrap();
        assert_eq!(command, Command::Schema { output: None });
    }

    #[test]
    fn schema_command_prints_valid_json() {
        let (code, output) = execute(&Command::Schema { output: None });
        assert_eq!(code, 0);
        let value: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert!(value.get("properties").is_some());
    }

    #[test]
    fn schema_command_writes_to_output_file() {
        let dir =
            std::env::temp_dir().join(format!("herdr-workflow-schema-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("workflow.schema.json");
        let (code, message) = execute(&Command::Schema {
            output: Some(path.clone()),
        });
        assert_eq!(code, 0);
        assert!(message.contains(&path.display().to_string()));
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&written).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
