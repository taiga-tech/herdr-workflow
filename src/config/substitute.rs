//! `${inputs.branch}`のようなプラグイン独自の値置換(シェル評価ではない)を扱う。
//! configuration.md: 未定義変数・型不一致は実行前にエラーにする。暗黙の実行式は
//! 初期版で扱わない。`${worktree.path}`等の実行時変数(HerdrAdapter接続後にしか
//! 値が確定しない)は、予約名前空間として構文チェックのみ行い、値は解決しない。

use std::collections::BTreeMap;
use std::fmt;

use crate::config::model::{InputDefault, InputId, InputType, TaskDef, WorkflowSpec};
use crate::config::validate::ValidateError;

/// HerdrAdapter接続後(段階C以降)にしか値が確定しない名前空間。
pub const RESERVED_RUNTIME_NAMESPACES: &[&str] = &["worktree"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder<'a> {
    pub namespace: &'a str,
    pub member: &'a str,
    pub raw: &'a str,
    pub start: usize,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SubstituteError {
    #[error("path {path:?}: malformed placeholder {raw:?}")]
    Malformed { path: String, raw: String },
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

/// "${" と "}" の対応を手でスキャンする。ネスト、エスケープは扱わない。
/// 識別子文法: `namespace.member`(それぞれ`[A-Za-z_][A-Za-z0-9_]*`)。
pub fn find_placeholders<'a>(
    path: &str,
    s: &'a str,
) -> Result<Vec<Placeholder<'a>>, SubstituteError> {
    let mut result = Vec::new();
    let mut search_from = 0;
    while let Some(rel_start) = s[search_from..].find("${") {
        let start = search_from + rel_start;
        let after_open = start + 2;
        let Some(rel_close) = s[after_open..].find('}') else {
            return Err(SubstituteError::Malformed {
                path: path.to_string(),
                raw: s[start..].to_string(),
            });
        };
        let close = after_open + rel_close;
        let body = &s[after_open..close];
        let raw = &s[start..=close];

        let malformed = || SubstituteError::Malformed {
            path: path.to_string(),
            raw: raw.to_string(),
        };

        let (namespace, member) = body.split_once('.').ok_or_else(malformed)?;
        let valid_ident = |ident: &str| {
            let mut chars = ident.chars();
            matches!(chars.next(), Some(c) if is_ident_start(c)) && chars.all(is_ident_char)
        };
        if !valid_ident(namespace) || !valid_ident(member) || member.contains('.') {
            return Err(malformed());
        }

        result.push(Placeholder {
            namespace,
            member,
            raw,
            start,
        });
        search_from = close + 1;
    }
    Ok(result)
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(untagged)]
pub enum InputValue {
    String(String),
    Integer(i64),
    Boolean(bool),
}

impl fmt::Display for InputValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InputValue::String(s) => f.write_str(s),
            InputValue::Integer(n) => write!(f, "{n}"),
            InputValue::Boolean(b) => write!(f, "{b}"),
        }
    }
}

pub type ResolvedInputs = BTreeMap<InputId, InputValue>;

/// `spec.inputs`の宣言と、CLIから渡された生文字列値を突き合わせて解決する。
pub fn resolve_inputs(
    spec: &WorkflowSpec,
    provided: &BTreeMap<InputId, String>,
) -> Result<ResolvedInputs, ValidateError> {
    for key in provided.keys() {
        if !spec.inputs.contains_key(key) {
            return Err(ValidateError::UnknownProvidedInput(key.0.clone()));
        }
    }

    let mut resolved = ResolvedInputs::new();
    for (id, def) in &spec.inputs {
        let value = match provided.get(id) {
            Some(raw) => parse_input_value(id, def.input_type, raw)?,
            None => match &def.default {
                Some(default) => default_to_input_value(default),
                None if def.required => {
                    return Err(ValidateError::MissingRequiredInput(id.clone()));
                }
                None => continue,
            },
        };
        resolved.insert(id.clone(), value);
    }
    Ok(resolved)
}

fn default_to_input_value(default: &InputDefault) -> InputValue {
    match default {
        InputDefault::String(s) => InputValue::String(s.clone()),
        InputDefault::Integer(n) => InputValue::Integer(*n),
        InputDefault::Boolean(b) => InputValue::Boolean(*b),
    }
}

fn parse_input_value(
    id: &InputId,
    expected: InputType,
    raw: &str,
) -> Result<InputValue, ValidateError> {
    match expected {
        InputType::String => Ok(InputValue::String(raw.to_string())),
        InputType::Integer => raw.parse::<i64>().map(InputValue::Integer).map_err(|_| {
            ValidateError::InvalidInputValue {
                input: id.clone(),
                expected,
                value: raw.to_string(),
            }
        }),
        InputType::Boolean => raw.parse::<bool>().map(InputValue::Boolean).map_err(|_| {
            ValidateError::InvalidInputValue {
                input: id.clone(),
                expected,
                value: raw.to_string(),
            }
        }),
    }
}

/// 置換対象になり得る文字列フィールドを`(構造パス, 生文字列)`として収集する唯一の窓口。
/// 新しいフィールドを置換対象に加えたい場合はここだけ拡張する。
pub fn template_fields(spec: &WorkflowSpec) -> Vec<(String, &str)> {
    let mut fields = Vec::new();

    fields.push(("worktree.branch".to_string(), spec.worktree.branch.as_str()));
    if let Some(base) = &spec.worktree.base {
        fields.push(("worktree.base".to_string(), base.as_str()));
    }

    if let Some(defaults) = &spec.defaults {
        if let Some(cwd) = &defaults.cwd {
            fields.push(("defaults.cwd".to_string(), cwd.as_str()));
        }
        for (key, value) in &defaults.env {
            fields.push((format!("defaults.env.{key}"), value.as_str()));
        }
    }

    fields.push(("workspace.label".to_string(), spec.workspace.label.as_str()));
    for tab in &spec.workspace.tabs {
        fields.push((
            format!("workspace.tabs.{}.label", tab.id.0),
            tab.label.as_str(),
        ));
        for pane in &tab.panes {
            fields.push((
                format!("workspace.tabs.{}.panes.{}.label", tab.id.0, pane.id.0),
                pane.label.as_str(),
            ));
        }
    }

    for (task_id, def) in &spec.tasks {
        if let TaskDef::Command {
            argv,
            env,
            wait_for,
            ..
        } = def
        {
            for (index, arg) in argv.iter().enumerate() {
                fields.push((format!("tasks.{}.argv.{index}", task_id.0), arg.as_str()));
            }
            for (key, value) in env {
                fields.push((format!("tasks.{}.env.{key}", task_id.0), value.as_str()));
            }
            if let Some(crate::config::model::WaitFor::Http { url, .. }) = wait_for {
                fields.push((format!("tasks.{}.waitFor.url", task_id.0), url.as_str()));
            }
        }
    }

    fields
}

/// パス文字列 -> 解決後の文字列。`${worktree.*}`のような実行時変数は未解決のまま残す。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResolvedValues(pub BTreeMap<String, String>);

/// `template_fields`が収集した各文字列のプレースホルダーを解決する。`${inputs.x}`は
/// `resolved`から引いて置換し、予約名前空間(`worktree`)はそのまま残す。argv要素は
/// 1つの文字列として置換し、置換後に分割しない(configuration.md)。
pub fn substitute_all(
    spec: &WorkflowSpec,
    resolved: &ResolvedInputs,
) -> Result<ResolvedValues, ValidateError> {
    let mut values = BTreeMap::new();
    for (path, raw) in template_fields(spec) {
        let placeholders = find_placeholders(&path, raw).map_err(substitute_to_validate_error)?;
        let mut result = String::new();
        let mut cursor = 0;
        for placeholder in &placeholders {
            result.push_str(&raw[cursor..placeholder.start]);
            if placeholder.namespace == "inputs" {
                let input_id = InputId(placeholder.member.to_string());
                let value =
                    resolved
                        .get(&input_id)
                        .ok_or_else(|| ValidateError::UndefinedInput {
                            path: path.clone(),
                            name: input_id.clone(),
                        })?;
                result.push_str(&value.to_string());
            } else if RESERVED_RUNTIME_NAMESPACES.contains(&placeholder.namespace) {
                result.push_str(placeholder.raw);
            } else {
                return Err(ValidateError::UnknownNamespace {
                    path: path.clone(),
                    namespace: placeholder.namespace.to_string(),
                });
            }
            cursor = placeholder.start + placeholder.raw.len();
        }
        result.push_str(&raw[cursor..]);
        values.insert(path, result);
    }
    Ok(ResolvedValues(values))
}

fn substitute_to_validate_error(err: SubstituteError) -> ValidateError {
    match err {
        SubstituteError::Malformed { path, raw } => {
            ValidateError::MalformedPlaceholder { path, raw }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::InputId;

    #[test]
    fn finds_single_placeholder_in_plain_string() {
        let placeholders = find_placeholders("p", "${inputs.branch}").unwrap();
        assert_eq!(placeholders.len(), 1);
        assert_eq!(placeholders[0].namespace, "inputs");
        assert_eq!(placeholders[0].member, "branch");
        assert_eq!(placeholders[0].raw, "${inputs.branch}");
        assert_eq!(placeholders[0].start, 0);
    }

    #[test]
    fn finds_placeholder_embedded_in_larger_string() {
        let placeholders = find_placeholders("p", "http://127.0.0.1:${inputs.port}/").unwrap();
        assert_eq!(placeholders.len(), 1);
        assert_eq!(placeholders[0].member, "port");
        assert_eq!(placeholders[0].start, 17);
    }

    #[test]
    fn malformed_placeholder_missing_closing_brace_is_rejected() {
        let err = find_placeholders("p", "${inputs.branch").unwrap_err();
        assert!(matches!(err, SubstituteError::Malformed { .. }));
    }

    #[test]
    fn malformed_placeholder_empty_body_is_rejected() {
        let err = find_placeholders("p", "${}").unwrap_err();
        assert!(matches!(err, SubstituteError::Malformed { .. }));
    }

    #[test]
    fn malformed_placeholder_without_member_is_rejected() {
        let err = find_placeholders("p", "${inputs}").unwrap_err();
        assert!(matches!(err, SubstituteError::Malformed { .. }));
    }

    fn spec_with_inputs() -> WorkflowSpec {
        let yaml = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflow.yaml"),
        )
        .unwrap();
        serde_yaml_ng::from_str(&yaml).unwrap()
    }

    #[test]
    fn resolves_declared_input_with_provided_value() {
        let spec = spec_with_inputs();
        let provided = BTreeMap::from([(InputId("branch".to_string()), "feature/x".to_string())]);
        let resolved = resolve_inputs(&spec, &provided).unwrap();
        assert_eq!(
            resolved[&InputId("branch".to_string())],
            InputValue::String("feature/x".to_string())
        );
        assert_eq!(
            resolved[&InputId("port".to_string())],
            InputValue::Integer(3000)
        );
    }

    #[test]
    fn resolves_declared_input_using_default_when_not_provided() {
        let spec = spec_with_inputs();
        let provided = BTreeMap::from([(InputId("branch".to_string()), "feature/x".to_string())]);
        let resolved = resolve_inputs(&spec, &provided).unwrap();
        assert_eq!(
            resolved[&InputId("port".to_string())],
            InputValue::Integer(3000)
        );
    }

    #[test]
    fn missing_required_input_without_default_is_rejected() {
        let spec = spec_with_inputs();
        let err = resolve_inputs(&spec, &BTreeMap::new()).unwrap_err();
        assert_eq!(
            err,
            ValidateError::MissingRequiredInput(InputId("branch".to_string()))
        );
    }

    #[test]
    fn unknown_provided_input_key_is_rejected() {
        let spec = spec_with_inputs();
        let provided = BTreeMap::from([
            (InputId("branch".to_string()), "feature/x".to_string()),
            (InputId("bogus".to_string()), "1".to_string()),
        ]);
        let err = resolve_inputs(&spec, &provided).unwrap_err();
        assert_eq!(
            err,
            ValidateError::UnknownProvidedInput("bogus".to_string())
        );
    }

    #[test]
    fn invalid_integer_input_value_is_rejected() {
        let spec = spec_with_inputs();
        let provided = BTreeMap::from([
            (InputId("branch".to_string()), "feature/x".to_string()),
            (InputId("port".to_string()), "not-a-number".to_string()),
        ]);
        let err = resolve_inputs(&spec, &provided).unwrap_err();
        assert!(matches!(err, ValidateError::InvalidInputValue { .. }));
    }

    #[test]
    fn invalid_boolean_input_value_is_rejected() {
        let spec = spec_with_inputs();
        let mut spec = spec;
        spec.inputs.insert(
            InputId("verbose".to_string()),
            crate::config::model::InputDef {
                input_type: InputType::Boolean,
                required: false,
                default: None,
            },
        );
        let provided = BTreeMap::from([
            (InputId("branch".to_string()), "feature/x".to_string()),
            (InputId("verbose".to_string()), "yes".to_string()),
        ]);
        let err = resolve_inputs(&spec, &provided).unwrap_err();
        assert!(matches!(err, ValidateError::InvalidInputValue { .. }));
    }

    #[test]
    fn worktree_namespace_placeholder_is_left_unresolved_after_substitution() {
        let spec = spec_with_inputs();
        let provided = BTreeMap::from([(InputId("branch".to_string()), "feature/x".to_string())]);
        let resolved = resolve_inputs(&spec, &provided).unwrap();
        let values = substitute_all(&spec, &resolved).unwrap();
        assert_eq!(values.0["defaults.cwd"], "${worktree.path}");
    }

    #[test]
    fn argv_placeholder_is_replaced_without_splitting_the_element() {
        let spec = spec_with_inputs();
        let provided = BTreeMap::from([(InputId("branch".to_string()), "feature/x".to_string())]);
        let resolved = resolve_inputs(&spec, &provided).unwrap();
        let values = substitute_all(&spec, &resolved).unwrap();
        assert_eq!(values.0["tasks.server.argv.10"], "3000");
    }

    #[test]
    fn inputs_branch_placeholder_is_replaced_in_worktree_branch() {
        let spec = spec_with_inputs();
        let provided = BTreeMap::from([(InputId("branch".to_string()), "feature/x".to_string())]);
        let resolved = resolve_inputs(&spec, &provided).unwrap();
        let values = substitute_all(&spec, &resolved).unwrap();
        assert_eq!(values.0["worktree.branch"], "feature/x");
    }
}
