//! `WorkflowSpec`にデシリアライズが成功した後でないと判定できない意味検証。
//! 未知キー・固定フィールドの重複キーはserde側(`deny_unknown_fields`とderive)、
//! mapフィールドの重複キーは`config::model::no_duplicate_map`で既に検出済みなので
//! ここでは扱わない。

use std::collections::HashSet;
use std::path::{Component, Path};

use crate::config::model::{
    EnvValue, InputId, InputType, PaneId, PortValue, TabId, TaskDef, TaskId, TaskLifecycle,
    WaitFor, WorkflowSpec, WorktreeMode,
};
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

    #[error("execution.retries: must be between 0 and {max} (got {value})")]
    ExecutionRetriesOutOfRange { value: u32, max: u32 },

    #[error("worktree.branch: required when worktree.mode is create")]
    MissingWorktreeBranch,

    #[error("worktree.{field}: not allowed when worktree.mode is attach")]
    AttachDisallowedField { field: &'static str },

    #[error("tasks.{task:?}: argv and script cannot be specified together")]
    ArgvScriptConflict { task: TaskId },

    #[error("tasks.{task:?}: either argv or script is required")]
    MissingCommand { task: TaskId },

    #[error("tasks.{task:?}.{field}: must not be empty")]
    EmptyCommand { task: TaskId, field: &'static str },

    #[error("{path}: invalid environment variable name {name:?}")]
    InvalidEnvName { path: String, name: String },

    #[error("{path}: environment variable {name:?} is reserved (HERDR_ prefix)")]
    ReservedEnvName { path: String, name: String },

    #[error("{path}: invalid secret name {name:?} (expected [a-z0-9][a-z0-9-]*)")]
    InvalidSecretName { path: String, name: String },

    #[error("tasks.{task:?}.locks: invalid lock name {lock:?} (expected [a-z0-9][a-z0-9-]*)")]
    InvalidLockName { task: TaskId, lock: String },

    #[error("tasks.{task:?}.locks: duplicate lock name {lock:?}")]
    DuplicateLockName { task: TaskId, lock: String },

    #[error("tasks.{task:?}.retry: only job-lifecycle command tasks can be retried")]
    RetryOnNonJob { task: TaskId },

    #[error("tasks.{task:?}.retry.{field}: must be between {min} and {max} (got {value})")]
    RetryOutOfRange {
        task: TaskId,
        field: &'static str,
        value: u64,
        min: u64,
        max: u64,
    },

    #[error("tasks.{task:?}.waitFor: only service-lifecycle command tasks can declare waitFor")]
    WaitForOnNonService { task: TaskId },

    #[error("tasks.{task:?}.waitFor.{field}: must be greater than zero")]
    ZeroWaitForValue { task: TaskId, field: &'static str },

    #[error("{path}: TCP port must be an integer between 1 and 65535 (got {value:?})")]
    InvalidTcpPort { path: String, value: String },

    #[error("tasks.{task:?}.waitFor.pattern: must be at most {max} characters")]
    LogPatternTooLong { task: TaskId, max: usize },

    #[error("tasks.{task:?}.waitFor.pattern: invalid regular expression: {message}")]
    InvalidLogPattern { task: TaskId, message: String },
}

/// `execution.retries`の上限。`retry.maxAttempts`の上限10から初回分を引いた値。
pub const MAX_EXECUTION_RETRIES: u32 = 9;
pub const RETRY_MAX_ATTEMPTS_RANGE: (u32, u32) = (1, 10);
pub const RETRY_MAX_DELAY_SECONDS: u64 = 3600;
pub const LOG_PATTERN_MAX_CHARS: usize = 1024;
const RESERVED_ENV_PREFIX: &str = "HERDR_";

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
    validate_worktree(spec)?;
    validate_tasks(spec)?;
    Ok(())
}

fn validate_worktree(spec: &WorkflowSpec) -> Result<(), ValidateError> {
    let worktree = &spec.worktree;
    match worktree.mode {
        WorktreeMode::Create => {
            if worktree.branch.is_none() {
                return Err(ValidateError::MissingWorktreeBranch);
            }
        }
        WorktreeMode::Attach => {
            for (present, field) in [
                (worktree.base.is_some(), "base"),
                (worktree.existing_branch.is_some(), "existingBranch"),
                (
                    worktree.if_already_checked_out.is_some(),
                    "ifAlreadyCheckedOut",
                ),
            ] {
                if present {
                    return Err(ValidateError::AttachDisallowedField { field });
                }
            }
        }
    }
    Ok(())
}

/// 秘密値名とロック名の共通規則`[a-z0-9][a-z0-9-]*`。
fn is_valid_resource_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn validate_env(
    path: &str,
    env: &std::collections::BTreeMap<String, EnvValue>,
) -> Result<(), ValidateError> {
    for (name, value) in env {
        if name.is_empty() || name.contains('=') || name.contains('\0') {
            return Err(ValidateError::InvalidEnvName {
                path: path.to_string(),
                name: name.clone(),
            });
        }
        if name.starts_with(RESERVED_ENV_PREFIX) {
            return Err(ValidateError::ReservedEnvName {
                path: path.to_string(),
                name: name.clone(),
            });
        }
        if let EnvValue::Secret(secret) = value
            && !is_valid_resource_name(&secret.secret)
        {
            return Err(ValidateError::InvalidSecretName {
                path: format!("{path}.{name}"),
                name: secret.secret.clone(),
            });
        }
    }
    Ok(())
}

fn validate_tasks(spec: &WorkflowSpec) -> Result<(), ValidateError> {
    if let Some(defaults) = &spec.defaults {
        validate_env("defaults.env", &defaults.env)?;
    }
    for (task_id, def) in &spec.tasks {
        let TaskDef::Command {
            lifecycle,
            argv,
            script,
            env,
            wait_for,
            locks,
            retry,
            ..
        } = def
        else {
            continue;
        };
        match (argv, script) {
            (Some(_), Some(_)) => {
                return Err(ValidateError::ArgvScriptConflict {
                    task: task_id.clone(),
                });
            }
            (None, None) => {
                return Err(ValidateError::MissingCommand {
                    task: task_id.clone(),
                });
            }
            (Some(argv), None) if argv.is_empty() => {
                return Err(ValidateError::EmptyCommand {
                    task: task_id.clone(),
                    field: "argv",
                });
            }
            (None, Some(script)) if script.shell.is_empty() => {
                return Err(ValidateError::EmptyCommand {
                    task: task_id.clone(),
                    field: "script.shell",
                });
            }
            _ => {}
        }

        validate_env(&format!("tasks.{}.env", task_id.0), env)?;

        let mut seen_locks = HashSet::new();
        for lock in locks {
            if !is_valid_resource_name(&lock.0) {
                return Err(ValidateError::InvalidLockName {
                    task: task_id.clone(),
                    lock: lock.0.clone(),
                });
            }
            if !seen_locks.insert(&lock.0) {
                return Err(ValidateError::DuplicateLockName {
                    task: task_id.clone(),
                    lock: lock.0.clone(),
                });
            }
        }

        if let Some(retry) = retry {
            if *lifecycle != TaskLifecycle::Job {
                return Err(ValidateError::RetryOnNonJob {
                    task: task_id.clone(),
                });
            }
            let (min, max) = RETRY_MAX_ATTEMPTS_RANGE;
            if !(min..=max).contains(&retry.max_attempts) {
                return Err(ValidateError::RetryOutOfRange {
                    task: task_id.clone(),
                    field: "maxAttempts",
                    value: u64::from(retry.max_attempts),
                    min: u64::from(min),
                    max: u64::from(max),
                });
            }
            if retry.delay_seconds > RETRY_MAX_DELAY_SECONDS {
                return Err(ValidateError::RetryOutOfRange {
                    task: task_id.clone(),
                    field: "delaySeconds",
                    value: retry.delay_seconds,
                    min: 0,
                    max: RETRY_MAX_DELAY_SECONDS,
                });
            }
        }

        if let Some(wait_for) = wait_for {
            if *lifecycle != TaskLifecycle::Service {
                return Err(ValidateError::WaitForOnNonService {
                    task: task_id.clone(),
                });
            }
            validate_wait_for(task_id, wait_for)?;
        }
    }
    Ok(())
}

fn validate_wait_for(task_id: &TaskId, wait_for: &WaitFor) -> Result<(), ValidateError> {
    let zero = |field: &'static str| ValidateError::ZeroWaitForValue {
        task: task_id.clone(),
        field,
    };
    let monitor = match wait_for {
        // httpの既存キーは従来どおり型の範囲だけを検査し、追加したmonitorだけを確認する。
        WaitFor::Http { monitor, .. } => monitor,
        WaitFor::Tcp {
            port,
            timeout_seconds,
            interval_milliseconds,
            consecutive_successes,
            monitor,
            ..
        } => {
            if *timeout_seconds == 0 {
                return Err(zero("timeoutSeconds"));
            }
            if *interval_milliseconds == 0 {
                return Err(zero("intervalMilliseconds"));
            }
            if *consecutive_successes == 0 {
                return Err(zero("consecutiveSuccesses"));
            }
            // 値置換を含む文字列は置換後にsubstitute_allで検査する。
            if let PortValue::Number(number) = port
                && !(1_i64..=65_535).contains(number)
            {
                return Err(ValidateError::InvalidTcpPort {
                    path: format!("tasks.{}.waitFor.port", task_id.0),
                    value: number.to_string(),
                });
            }
            monitor
        }
        WaitFor::Log {
            pattern,
            timeout_seconds,
            ..
        } => {
            if *timeout_seconds == 0 {
                return Err(zero("timeoutSeconds"));
            }
            if pattern.chars().count() > LOG_PATTERN_MAX_CHARS {
                return Err(ValidateError::LogPatternTooLong {
                    task: task_id.clone(),
                    max: LOG_PATTERN_MAX_CHARS,
                });
            }
            if let Err(err) = regex::Regex::new(pattern) {
                return Err(ValidateError::InvalidLogPattern {
                    task: task_id.clone(),
                    message: err.to_string(),
                });
            }
            return Ok(());
        }
    };
    if let Some(monitor) = monitor {
        if monitor.interval_seconds == 0 {
            return Err(zero("monitor.intervalSeconds"));
        }
        if monitor.failure_threshold == 0 {
            return Err(zero("monitor.failureThreshold"));
        }
    }
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
    if spec.execution.retries > MAX_EXECUTION_RETRIES {
        return Err(ValidateError::ExecutionRetriesOutOfRange {
            value: spec.execution.retries,
            max: MAX_EXECUTION_RETRIES,
        });
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
    // componentsで区切りの表記揺れを吸収し、先頭のCurDirも明示的に除く。
    let a: Vec<_> = Path::new(a)
        .components()
        .filter(|part| *part != Component::CurDir)
        .collect();
    let b: Vec<_> = Path::new(b)
        .components()
        .filter(|part| *part != Component::CurDir)
        .collect();
    a.starts_with(&b) || b.starts_with(&a)
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
        spec.worktree.branch = Some("${inputs.missing}".to_string());
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
        spec.worktree.branch = Some("${bogus.member}".to_string());
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
    fn normalized_overlapping_copy_destinations_are_rejected() {
        // 同一パスの表記揺れと親子関係は、指定順を入れ替えても拒否する。
        for (a, b) in [
            (".env", "./.env"),
            ("config/", "config/local.env"),
            ("config/local.env", "config//local.env"),
            ("./config/./", "config/local.env"),
        ] {
            for (first, second) in [(a, b), (b, a)] {
                let spec = spec_with_copy_rules(vec![
                    copy_rule("source-a", first),
                    copy_rule("source-b", second),
                ]);
                assert_eq!(
                    validate(&spec),
                    Err(ValidateError::OverlappingCopyDestination {
                        first: 0,
                        second: 1,
                    })
                );
            }
        }
    }

    #[test]
    fn copy_destinations_with_only_textual_prefix_are_allowed() {
        // 文字列の接頭辞が同じでも、要素が異なるパスや兄弟ファイルは衝突しない。
        for (a, b) in [
            ("config", "config-old/local.env"),
            ("config/a", "config/ab"),
        ] {
            let spec =
                spec_with_copy_rules(vec![copy_rule("source-a", a), copy_rule("source-b", b)]);
            assert_eq!(validate(&spec), Ok(()));
        }
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

/// Q11で追加した入力形式の意味検証(T39)。YAML断片から組み立て、外部操作なしで拒否されることを確認する。
#[cfg(test)]
mod q11_tests {
    use super::*;

    const TOOLCHAIN_TAIL: &str = "argv: [mise, install]\n    timeoutSeconds: 600";
    /// 設定例のserverのwaitFor(monitorを除く部分)。
    const SERVER_HTTP_BASE: &str = "    waitFor:\n      type: http\n      url: 'http://127.0.0.1:${inputs.port}/'\n      status: 200\n      timeoutSeconds: 120\n      intervalMilliseconds: 500\n      consecutiveSuccesses: 2\n";
    /// 設定例のserverのwaitFor全体(コメントとmonitorを含む)。
    const SERVER_WAIT_FOR: &str = "    waitFor:\n      type: http\n      url: 'http://127.0.0.1:${inputs.port}/'\n      status: 200\n      timeoutSeconds: 120\n      intervalMilliseconds: 500\n      consecutiveSuccesses: 2\n      # ready後も確認を続け、3回連続で失敗したらunreadyにする。\n      monitor:\n        intervalSeconds: 10\n        failureThreshold: 3\n";

    fn example_yaml() -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflow.yaml"),
        )
        .unwrap()
    }

    fn parse(yaml: &str) -> Result<WorkflowSpec, serde_yaml_ng::Error> {
        serde_yaml_ng::from_str(yaml)
    }

    /// toolchainタスク(job)の末尾にキーを追加する。`extra`は4字下げ済みの行。
    fn with_toolchain(extra: &str) -> String {
        let yaml = example_yaml();
        assert!(yaml.contains(TOOLCHAIN_TAIL), "example layout changed");
        yaml.replacen(TOOLCHAIN_TAIL, &format!("{TOOLCHAIN_TAIL}\n{extra}"), 1)
    }

    /// serverタスク(service)のwaitForを置き換える。`wait_for`は4字下げの`waitFor:`から始める。
    fn with_server_wait_for(wait_for: &str) -> String {
        let yaml = example_yaml();
        assert!(yaml.contains(SERVER_WAIT_FOR), "example layout changed");
        yaml.replacen(SERVER_WAIT_FOR, wait_for, 1)
    }

    fn validate_yaml(yaml: &str) -> Result<(), ValidateError> {
        validate(&parse(yaml).expect("yaml should deserialize"))
    }

    fn toolchain() -> TaskId {
        TaskId::from("toolchain")
    }

    #[test]
    fn reserved_herdr_env_name_is_rejected() {
        let yaml = with_toolchain("    env:\n      HERDR_PANE_ID: x");
        assert_eq!(
            validate_yaml(&yaml),
            Err(ValidateError::ReservedEnvName {
                path: "tasks.toolchain.env".to_string(),
                name: "HERDR_PANE_ID".to_string(),
            })
        );
    }

    #[test]
    fn reserved_env_name_in_defaults_is_rejected() {
        let yaml = example_yaml().replacen(
            "env:\n    NODE_ENV: development",
            "env:\n    NODE_ENV: development\n    HERDR_WORKFLOW_STATE_DIR: /tmp/x",
            1,
        );
        assert!(matches!(
            validate_yaml(&yaml),
            Err(ValidateError::ReservedEnvName { .. })
        ));
    }

    #[test]
    fn env_name_with_equals_sign_is_rejected() {
        let yaml = with_toolchain("    env:\n      'A=B': x");
        assert!(matches!(
            validate_yaml(&yaml),
            Err(ValidateError::InvalidEnvName { .. })
        ));
    }

    #[test]
    fn null_env_value_and_secret_reference_are_accepted() {
        let yaml = with_toolchain(
            "    env:\n      NODE_ENV: null\n      DATABASE_URL: { secret: db-url }",
        );
        let spec = parse(&yaml).unwrap();
        let TaskDef::Command { env, .. } = &spec.tasks[&toolchain()] else {
            panic!("toolchain should be a command");
        };
        assert_eq!(env["NODE_ENV"], EnvValue::Unset);
        assert_eq!(
            env["DATABASE_URL"],
            EnvValue::Secret(crate::config::model::SecretRef {
                secret: "db-url".to_string()
            })
        );
        assert_eq!(validate(&spec), Ok(()));
    }

    #[test]
    fn invalid_secret_name_is_rejected() {
        let yaml = with_toolchain("    env:\n      TOKEN: { secret: Bad_Name }");
        assert_eq!(
            validate_yaml(&yaml),
            Err(ValidateError::InvalidSecretName {
                path: "tasks.toolchain.env.TOKEN".to_string(),
                name: "Bad_Name".to_string(),
            })
        );
    }

    #[test]
    fn secret_reference_with_unknown_key_is_rejected_by_deserializer() {
        let yaml = with_toolchain("    env:\n      TOKEN: { secret: token, value: leaked }");
        assert!(parse(&yaml).is_err());
    }

    #[test]
    fn argv_and_script_together_are_rejected() {
        let yaml =
            with_toolchain("    script:\n      shell: [bash, -c]\n      source: mise install");
        assert_eq!(
            validate_yaml(&yaml),
            Err(ValidateError::ArgvScriptConflict { task: toolchain() })
        );
    }

    #[test]
    fn script_without_argv_is_accepted() {
        let yaml = example_yaml().replacen(
            TOOLCHAIN_TAIL,
            "script:\n      shell: [bash, -euo, pipefail, -c]\n      source: 'mise install && echo ${HOME}'\n    timeoutSeconds: 600",
            1,
        );
        // sourceの`${HOME}`は値置換の対象外なので、未知の名前空間として拒否されない。
        assert_eq!(validate_yaml(&yaml), Ok(()));
    }

    #[test]
    fn missing_argv_and_script_is_rejected() {
        let yaml = example_yaml().replacen(TOOLCHAIN_TAIL, "timeoutSeconds: 600", 1);
        assert_eq!(
            validate_yaml(&yaml),
            Err(ValidateError::MissingCommand { task: toolchain() })
        );
    }

    #[test]
    fn empty_argv_and_empty_shell_are_rejected() {
        let yaml = example_yaml().replacen(TOOLCHAIN_TAIL, "argv: []\n    timeoutSeconds: 600", 1);
        assert_eq!(
            validate_yaml(&yaml),
            Err(ValidateError::EmptyCommand {
                task: toolchain(),
                field: "argv"
            })
        );
        let yaml = example_yaml().replacen(
            TOOLCHAIN_TAIL,
            "script:\n      shell: []\n      source: x\n    timeoutSeconds: 600",
            1,
        );
        assert_eq!(
            validate_yaml(&yaml),
            Err(ValidateError::EmptyCommand {
                task: toolchain(),
                field: "script.shell"
            })
        );
    }

    #[test]
    fn invalid_and_duplicate_lock_names_are_rejected() {
        let yaml = with_toolchain("    locks: [shared-db, Shared]");
        assert_eq!(
            validate_yaml(&yaml),
            Err(ValidateError::InvalidLockName {
                task: toolchain(),
                lock: "Shared".to_string(),
            })
        );
        let yaml = with_toolchain("    locks: [shared-db, shared-db]");
        assert_eq!(
            validate_yaml(&yaml),
            Err(ValidateError::DuplicateLockName {
                task: toolchain(),
                lock: "shared-db".to_string(),
            })
        );
        let yaml = with_toolchain("    locks: [shared-db, cache-1]");
        assert_eq!(validate_yaml(&yaml), Ok(()));
    }

    #[test]
    fn retry_on_job_within_range_is_accepted() {
        let yaml = with_toolchain("    retry:\n      maxAttempts: 10\n      delaySeconds: 3600");
        assert_eq!(validate_yaml(&yaml), Ok(()));
        let yaml = with_toolchain("    retry:\n      maxAttempts: 1");
        assert_eq!(validate_yaml(&yaml), Ok(()));
    }

    #[test]
    fn retry_out_of_range_is_rejected() {
        for (retry, field, value) in [
            ("maxAttempts: 0", "maxAttempts", 0),
            ("maxAttempts: 11", "maxAttempts", 11),
            (
                "maxAttempts: 2\n      delaySeconds: 3601",
                "delaySeconds",
                3601,
            ),
        ] {
            let yaml = with_toolchain(&format!("    retry:\n      {retry}"));
            assert!(
                matches!(
                    validate_yaml(&yaml),
                    Err(ValidateError::RetryOutOfRange { field: f, value: v, .. })
                        if f == field && v == value
                ),
                "{retry}"
            );
        }
    }

    #[test]
    fn retry_on_service_is_rejected() {
        let yaml = example_yaml().replacen(
            "    stop:\n      graceSeconds: 10",
            "    stop:\n      graceSeconds: 10\n    retry:\n      maxAttempts: 2",
            1,
        );
        assert_eq!(
            validate_yaml(&yaml),
            Err(ValidateError::RetryOnNonJob {
                task: TaskId::from("server")
            })
        );
    }

    #[test]
    fn execution_retries_above_nine_is_rejected() {
        let yaml = example_yaml().replacen("  retries: 0", "  retries: 10", 1);
        assert_eq!(
            validate_yaml(&yaml),
            Err(ValidateError::ExecutionRetriesOutOfRange { value: 10, max: 9 })
        );
        let yaml = example_yaml().replacen("  retries: 0", "  retries: 9", 1);
        assert_eq!(validate_yaml(&yaml), Ok(()));
    }

    #[test]
    fn wait_for_on_job_is_rejected() {
        let yaml = with_toolchain(
            "    waitFor:\n      type: tcp\n      port: 3000\n      timeoutSeconds: 5\n      intervalMilliseconds: 100\n      consecutiveSuccesses: 1",
        );
        assert_eq!(
            validate_yaml(&yaml),
            Err(ValidateError::WaitForOnNonService { task: toolchain() })
        );
    }

    #[test]
    fn tcp_wait_for_defaults_host_and_accepts_monitor() {
        let yaml = with_server_wait_for(
            "    waitFor:\n      type: tcp\n      port: '${inputs.port}'\n      timeoutSeconds: 60\n      intervalMilliseconds: 250\n      consecutiveSuccesses: 2\n      monitor:\n        intervalSeconds: 10\n        failureThreshold: 3\n",
        );
        let spec = parse(&yaml).unwrap();
        let TaskDef::Command {
            wait_for: Some(WaitFor::Tcp { host, monitor, .. }),
            ..
        } = &spec.tasks[&TaskId::from("server")]
        else {
            panic!("server should have a tcp waitFor");
        };
        assert_eq!(host, "127.0.0.1");
        assert!(monitor.is_some());
        assert_eq!(validate(&spec), Ok(()));
    }

    #[test]
    fn tcp_port_out_of_range_is_rejected() {
        let yaml = with_server_wait_for(
            "    waitFor:\n      type: tcp\n      port: 70000\n      timeoutSeconds: 60\n      intervalMilliseconds: 250\n      consecutiveSuccesses: 2\n",
        );
        assert_eq!(
            validate_yaml(&yaml),
            Err(ValidateError::InvalidTcpPort {
                path: "tasks.server.waitFor.port".to_string(),
                value: "70000".to_string(),
            })
        );
    }

    #[test]
    fn zero_tcp_values_and_monitor_values_are_rejected() {
        let yaml = with_server_wait_for(
            "    waitFor:\n      type: tcp\n      port: 3000\n      timeoutSeconds: 0\n      intervalMilliseconds: 250\n      consecutiveSuccesses: 2\n",
        );
        assert!(matches!(
            validate_yaml(&yaml),
            Err(ValidateError::ZeroWaitForValue {
                field: "timeoutSeconds",
                ..
            })
        ));
        let yaml = with_server_wait_for(&format!(
            "{SERVER_HTTP_BASE}      monitor:\n        intervalSeconds: 10\n        failureThreshold: 0\n"
        ));
        assert!(matches!(
            validate_yaml(&yaml),
            Err(ValidateError::ZeroWaitForValue {
                field: "monitor.failureThreshold",
                ..
            })
        ));
    }

    #[test]
    fn monitor_requires_both_keys() {
        let yaml = with_server_wait_for(&format!(
            "{SERVER_HTTP_BASE}      monitor:\n        intervalSeconds: 10\n"
        ));
        assert!(parse(&yaml).is_err());
    }

    #[test]
    fn log_wait_for_compiles_pattern_and_rejects_monitor() {
        let log = "    waitFor:\n      type: log\n      stream: stdout\n      pattern: 'Ready in \\d+ms'\n      timeoutSeconds: 120\n";
        assert_eq!(validate_yaml(&with_server_wait_for(log)), Ok(()));

        let with_monitor = format!(
            "{log}      monitor:\n        intervalSeconds: 10\n        failureThreshold: 3\n"
        );
        assert!(parse(&with_server_wait_for(&with_monitor)).is_err());
    }

    #[test]
    fn invalid_or_too_long_log_pattern_is_rejected() {
        let yaml = with_server_wait_for(
            "    waitFor:\n      type: log\n      pattern: '(unclosed'\n      timeoutSeconds: 120\n",
        );
        assert!(matches!(
            validate_yaml(&yaml),
            Err(ValidateError::InvalidLogPattern { .. })
        ));

        let long = "a".repeat(LOG_PATTERN_MAX_CHARS + 1);
        let yaml = with_server_wait_for(&format!(
            "    waitFor:\n      type: log\n      pattern: '{long}'\n      timeoutSeconds: 120\n"
        ));
        assert_eq!(
            validate_yaml(&yaml),
            Err(ValidateError::LogPatternTooLong {
                task: TaskId::from("server"),
                max: LOG_PATTERN_MAX_CHARS,
            })
        );
    }

    #[test]
    fn attach_mode_rejects_create_only_fields_and_allows_missing_branch() {
        // 例はbase・existingBranch・ifAlreadyCheckedOutを持つので、attachでは最初のbaseで拒否される。
        let yaml = example_yaml().replacen("mode: create", "mode: attach", 1);
        assert_eq!(
            validate_yaml(&yaml),
            Err(ValidateError::AttachDisallowedField { field: "base" })
        );

        let yaml = example_yaml().replacen(
            "  mode: create\n  branch: '${inputs.branch}'\n  base: main\n  existingBranch: use\n  ifAlreadyCheckedOut: error\n",
            "  mode: attach\n",
            1,
        );
        assert_eq!(validate_yaml(&yaml), Ok(()));
    }

    #[test]
    fn create_mode_requires_branch() {
        let yaml = example_yaml().replacen("  branch: '${inputs.branch}'\n", "", 1);
        assert_eq!(
            validate_yaml(&yaml),
            Err(ValidateError::MissingWorktreeBranch)
        );
    }
}
