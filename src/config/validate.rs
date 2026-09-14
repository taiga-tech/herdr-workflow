//! `WorkflowSpec`にデシリアライズが成功した後でないと判定できない意味検証。
//! 未知キー・固定フィールドの重複キーはserde側(`deny_unknown_fields`とderive)、
//! mapフィールドの重複キーは`config::model::no_duplicate_map`で既に検出済みなので
//! ここでは扱わない。

use std::collections::HashSet;

use crate::config::model::{InputId, InputType, PaneId, TabId, WorkflowSpec};
use crate::config::substitute::{self, RESERVED_RUNTIME_NAMESPACES};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ValidateError {
    #[error(
        "input {input:?} declares type {expected:?} but its default value has a different type"
    )]
    DefaultTypeMismatch { input: InputId, expected: InputType },

    #[error("path {path:?}: placeholder {raw:?} is malformed")]
    MalformedPlaceholder { path: String, raw: String },

    #[error("path {path:?}: undefined input {name:?}")]
    UndefinedInput { path: String, name: InputId },

    #[error("path {path:?}: unknown variable namespace {namespace:?}")]
    UnknownNamespace { path: String, namespace: String },

    #[error("duplicate tab id {0:?}")]
    DuplicateTabId(TabId),

    #[error("tab {tab:?} has duplicate pane id {pane:?}")]
    DuplicatePaneId { tab: TabId, pane: PaneId },

    #[error("files.copy[{index}].{field:?}: absolute paths are not allowed ({path:?})")]
    AbsoluteCopyPath {
        index: usize,
        field: CopyPathField,
        path: String,
    },

    #[error("files.copy[{index}].{field:?}: path escapes the worktree via `..` ({path:?})")]
    PathEscapesWorktree {
        index: usize,
        field: CopyPathField,
        path: String,
    },

    #[error("files.copy[{first}] and files.copy[{second}] have overlapping destinations")]
    OverlappingCopyDestination { first: usize, second: usize },

    #[error("unknown input {0:?} was provided")]
    UnknownProvidedInput(String),
    #[error("required input {0:?} was not provided and has no default")]
    MissingRequiredInput(InputId),
    #[error("input {input:?} expects type {expected:?} but got {value:?}")]
    InvalidInputValue {
        input: InputId,
        expected: InputType,
        value: String,
    },

    #[error("execution.{field}: must be greater than zero when specified")]
    ZeroExecutionLimit { field: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyPathField {
    From,
    To,
}

pub fn validate(spec: &WorkflowSpec) -> Result<(), ValidateError> {
    validate_input_defaults(spec)?;
    validate_placeholders(spec)?;
    validate_unique_ids(spec)?;
    validate_copy_paths(spec)?;
    validate_execution_limits(spec)?;
    Ok(())
}

/// `None`は「制限なし」を意味し許容する。`u32`により負数は型レベルで排除済みなので、
/// ここでは指定時に`0`(=並行実行を一切許可しない、DAGが進行不能になる)だけを拒否する。
fn validate_execution_limits(spec: &WorkflowSpec) -> Result<(), ValidateError> {
    for (value, field) in [
        (spec.execution.max_concurrent_jobs, "maxConcurrentJobs"),
        (spec.execution.max_live_services, "maxLiveServices"),
        (spec.execution.max_live_agents, "maxLiveAgents"),
    ] {
        if value == Some(0) {
            return Err(ValidateError::ZeroExecutionLimit { field });
        }
    }
    Ok(())
}

fn validate_input_defaults(spec: &WorkflowSpec) -> Result<(), ValidateError> {
    for (id, def) in &spec.inputs {
        let Some(default) = &def.default else {
            continue;
        };
        let matches = matches!(
            (def.input_type, default),
            (
                InputType::String,
                crate::config::model::InputDefault::String(_)
            ) | (
                InputType::Integer,
                crate::config::model::InputDefault::Integer(_)
            ) | (
                InputType::Boolean,
                crate::config::model::InputDefault::Boolean(_)
            )
        );
        if !matches {
            return Err(ValidateError::DefaultTypeMismatch {
                input: id.clone(),
                expected: def.input_type,
            });
        }
    }
    Ok(())
}

fn validate_placeholders(spec: &WorkflowSpec) -> Result<(), ValidateError> {
    for (path, raw) in substitute::template_fields(spec) {
        let placeholders = substitute::find_placeholders(&path, raw).map_err(|err| match err {
            substitute::SubstituteError::Malformed { path, raw } => {
                ValidateError::MalformedPlaceholder { path, raw }
            }
        })?;
        for placeholder in placeholders {
            if placeholder.namespace == "inputs" {
                let input_id = InputId(placeholder.member.to_string());
                if !spec.inputs.contains_key(&input_id) {
                    return Err(ValidateError::UndefinedInput {
                        path: path.clone(),
                        name: input_id,
                    });
                }
            } else if !RESERVED_RUNTIME_NAMESPACES.contains(&placeholder.namespace) {
                return Err(ValidateError::UnknownNamespace {
                    path: path.clone(),
                    namespace: placeholder.namespace.to_string(),
                });
            }
        }
    }
    Ok(())
}

fn validate_unique_ids(spec: &WorkflowSpec) -> Result<(), ValidateError> {
    let mut tab_ids: HashSet<&TabId> = HashSet::new();
    for tab in &spec.workspace.tabs {
        if !tab_ids.insert(&tab.id) {
            return Err(ValidateError::DuplicateTabId(tab.id.clone()));
        }
        let mut pane_ids: HashSet<&PaneId> = HashSet::new();
        for pane in &tab.panes {
            if !pane_ids.insert(&pane.id) {
                return Err(ValidateError::DuplicatePaneId {
                    tab: tab.id.clone(),
                    pane: pane.id.clone(),
                });
            }
        }
    }
    Ok(())
}

fn is_absolute_or_escaping(path: &str) -> (bool, bool) {
    let is_absolute = path.starts_with('/') || path.starts_with('\\');
    let escapes = std::path::Path::new(path)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir));
    (is_absolute, escapes)
}

fn validate_copy_paths(spec: &WorkflowSpec) -> Result<(), ValidateError> {
    let Some(files) = &spec.files else {
        return Ok(());
    };
    let mut destinations: Vec<(usize, &str)> = Vec::new();
    for (index, rule) in files.copy.iter().enumerate() {
        for (field, path) in [
            (CopyPathField::From, &rule.from),
            (CopyPathField::To, &rule.to),
        ] {
            let (is_absolute, escapes) = is_absolute_or_escaping(path);
            if is_absolute {
                return Err(ValidateError::AbsoluteCopyPath {
                    index,
                    field,
                    path: path.clone(),
                });
            }
            if escapes {
                return Err(ValidateError::PathEscapesWorktree {
                    index,
                    field,
                    path: path.clone(),
                });
            }
        }
        destinations.push((index, rule.to.as_str()));
    }
    for i in 0..destinations.len() {
        for j in (i + 1)..destinations.len() {
            let (first_index, first_path) = destinations[i];
            let (second_index, second_path) = destinations[j];
            if paths_conflict(first_path, second_path) {
                return Err(ValidateError::OverlappingCopyDestination {
                    first: first_index,
                    second: second_index,
                });
            }
        }
    }
    Ok(())
}

fn paths_conflict(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let a_prefix = format!("{a}/");
    let b_prefix = format!("{b}/");
    a.starts_with(&b_prefix) || b.starts_with(&a_prefix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{InputDef, InputType};

    fn example_spec() -> WorkflowSpec {
        let yaml = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflow.yaml"),
        )
        .unwrap();
        serde_yaml_ng::from_str(&yaml).unwrap()
    }

    #[test]
    fn example_workflow_passes_validation() {
        assert_eq!(validate(&example_spec()), Ok(()));
    }

    #[test]
    fn undefined_input_variable_is_rejected() {
        let mut spec = example_spec();
        spec.worktree.branch = "${inputs.missing}".to_string();
        let err = validate(&spec).unwrap_err();
        assert_eq!(
            err,
            ValidateError::UndefinedInput {
                path: "worktree.branch".to_string(),
                name: InputId("missing".to_string()),
            }
        );
    }

    #[test]
    fn unknown_namespace_in_placeholder_is_rejected() {
        let mut spec = example_spec();
        spec.worktree.branch = "${bogus.member}".to_string();
        let err = validate(&spec).unwrap_err();
        assert_eq!(
            err,
            ValidateError::UnknownNamespace {
                path: "worktree.branch".to_string(),
                namespace: "bogus".to_string(),
            }
        );
    }

    #[test]
    fn input_default_type_mismatch_is_rejected() {
        let mut spec = example_spec();
        spec.inputs.insert(
            InputId("verbose".to_string()),
            InputDef {
                input_type: InputType::Boolean,
                required: false,
                default: Some(crate::config::model::InputDefault::String(
                    "nope".to_string(),
                )),
            },
        );
        let err = validate(&spec).unwrap_err();
        assert_eq!(
            err,
            ValidateError::DefaultTypeMismatch {
                input: InputId("verbose".to_string()),
                expected: InputType::Boolean,
            }
        );
    }

    #[test]
    fn input_default_matching_type_is_accepted() {
        let mut spec = example_spec();
        spec.inputs.insert(
            InputId("verbose".to_string()),
            InputDef {
                input_type: InputType::Boolean,
                required: false,
                default: Some(crate::config::model::InputDefault::Boolean(true)),
            },
        );
        assert_eq!(validate(&spec), Ok(()));
    }

    #[test]
    fn duplicate_tab_id_is_rejected() {
        let mut spec = example_spec();
        let tab = spec.workspace.tabs[0].clone();
        spec.workspace.tabs.push(tab);
        let err = validate(&spec).unwrap_err();
        assert_eq!(
            err,
            ValidateError::DuplicateTabId(TabId::from("development"))
        );
    }

    #[test]
    fn duplicate_pane_id_within_tab_is_rejected() {
        let mut spec = example_spec();
        let pane = spec.workspace.tabs[0].panes[0].clone();
        spec.workspace.tabs[0].panes.push(pane);
        let err = validate(&spec).unwrap_err();
        assert_eq!(
            err,
            ValidateError::DuplicatePaneId {
                tab: TabId::from("development"),
                pane: crate::config::model::PaneId("A".to_string()),
            }
        );
    }

    fn spec_with_copy_rules(rules: Vec<crate::config::model::FileCopyRule>) -> WorkflowSpec {
        let mut spec = example_spec();
        spec.files = Some(crate::config::model::FilesConfig {
            source: crate::config::model::FilesSource::Primary,
            copy: rules,
            symlinks: crate::config::model::SymlinkPolicy::Reject,
            require_ignored: true,
        });
        spec
    }

    fn copy_rule(from: &str, to: &str) -> crate::config::model::FileCopyRule {
        crate::config::model::FileCopyRule {
            from: from.to_string(),
            to: to.to_string(),
            optional: false,
            if_exists: None,
        }
    }

    #[test]
    fn absolute_copy_path_is_rejected() {
        let spec = spec_with_copy_rules(vec![copy_rule("/etc/passwd", ".env.local")]);
        let err = validate(&spec).unwrap_err();
        assert_eq!(
            err,
            ValidateError::AbsoluteCopyPath {
                index: 0,
                field: CopyPathField::From,
                path: "/etc/passwd".to_string(),
            }
        );
    }

    #[test]
    fn copy_path_escaping_worktree_is_rejected() {
        let spec = spec_with_copy_rules(vec![copy_rule("../secrets/.env", ".env.local")]);
        let err = validate(&spec).unwrap_err();
        assert_eq!(
            err,
            ValidateError::PathEscapesWorktree {
                index: 0,
                field: CopyPathField::From,
                path: "../secrets/.env".to_string(),
            }
        );
    }

    #[test]
    fn overlapping_copy_destinations_are_rejected() {
        let spec = spec_with_copy_rules(vec![
            copy_rule(".env.local", "config/.env.local"),
            copy_rule(".env.other", "config/.env.local"),
        ]);
        let err = validate(&spec).unwrap_err();
        assert_eq!(
            err,
            ValidateError::OverlappingCopyDestination {
                first: 0,
                second: 1
            }
        );
    }

    #[test]
    fn zero_max_concurrent_jobs_is_rejected() {
        let mut spec = example_spec();
        spec.execution.max_concurrent_jobs = Some(0);
        let err = validate(&spec).unwrap_err();
        assert_eq!(
            err,
            ValidateError::ZeroExecutionLimit {
                field: "maxConcurrentJobs"
            }
        );
    }

    #[test]
    fn zero_max_live_services_is_rejected() {
        let mut spec = example_spec();
        spec.execution.max_live_services = Some(0);
        let err = validate(&spec).unwrap_err();
        assert_eq!(
            err,
            ValidateError::ZeroExecutionLimit {
                field: "maxLiveServices"
            }
        );
    }

    #[test]
    fn zero_max_live_agents_is_rejected() {
        let mut spec = example_spec();
        spec.execution.max_live_agents = Some(0);
        let err = validate(&spec).unwrap_err();
        assert_eq!(
            err,
            ValidateError::ZeroExecutionLimit {
                field: "maxLiveAgents"
            }
        );
    }

    #[test]
    fn none_execution_limits_are_accepted() {
        let mut spec = example_spec();
        spec.execution.max_concurrent_jobs = None;
        spec.execution.max_live_services = None;
        spec.execution.max_live_agents = None;
        assert_eq!(validate(&spec), Ok(()));
    }

    #[test]
    fn retries_zero_is_accepted() {
        let mut spec = example_spec();
        spec.execution.retries = 0;
        assert_eq!(validate(&spec), Ok(()));
    }
}
