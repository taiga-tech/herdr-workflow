//! ブランチ名による構成の選択(docs/integrations/external-tools.md#profiles、Q08)。
//!
//! `profiles`を上から順に照合し、最初に一致したものを適用する。適用は入力の既定値と
//! `bootstrap.targets`の置き換えに限り、適用後の構成で計画を作る。選んだprofileは計画に表示し、
//! 実行計画ハッシュにも含まれる(適用後の構成から計算するため)。

use std::collections::BTreeMap;
use std::path::Path;

use crate::config::model::{BootstrapConfig, InputId, ProfileDef, WorkflowSpec};
use crate::config::substitute;

/// `*`・`**`・`?`を使うブランチ名のパターン照合。
#[must_use]
pub fn branch_matches(pattern: &str, branch: &str) -> bool {
    fn go(pattern: &[u8], text: &[u8]) -> bool {
        match pattern {
            [] => text.is_empty(),
            [b'*', b'*', rest @ ..] => (0..=text.len()).any(|skip| go(rest, &text[skip..])),
            [b'*', rest @ ..] => {
                let limit = text.iter().position(|c| *c == b'/').unwrap_or(text.len());
                (0..=limit).any(|skip| go(rest, &text[skip..]))
            }
            [b'?', rest @ ..] => text.first().is_some_and(|c| *c != b'/') && go(rest, &text[1..]),
            [c, rest @ ..] => text.first() == Some(c) && go(rest, &text[1..]),
        }
    }
    go(pattern.as_bytes(), branch.as_bytes())
}

/// 照合に使うブランチ名。`worktree.branch`の値置換の結果、なければcheckoutの現在のブランチ。
#[must_use]
pub fn branch_for(
    spec: &WorkflowSpec,
    inputs: &BTreeMap<InputId, String>,
    checkout: Option<&Path>,
) -> Option<String> {
    let from_config = substitute::resolve_inputs(spec, inputs)
        .ok()
        .and_then(|resolved| substitute::substitute_all(spec, &resolved).ok())
        .and_then(|values| values.0.get("worktree.branch").cloned())
        .filter(|branch| !branch.contains("${"));
    from_config.or_else(|| {
        checkout
            .and_then(|checkout| crate::runtime::git::inspect(checkout).ok())
            .and_then(|info| info.branch)
    })
}

/// 最初に一致したprofile。
#[must_use]
pub fn select<'a>(spec: &'a WorkflowSpec, branch: &str) -> Option<&'a ProfileDef> {
    spec.profiles
        .iter()
        .find(|profile| branch_matches(&profile.branch, branch))
}

/// profileを適用した構成。`profiles`自体は選択結果の表示のために残す。
#[must_use]
pub fn apply(spec: &WorkflowSpec, profile: &ProfileDef) -> WorkflowSpec {
    let mut applied = spec.clone();
    for (input, value) in &profile.inputs {
        if let Some(def) = applied.inputs.get_mut(input) {
            def.default = Some(value.clone());
        }
    }
    if let Some(targets) = &profile.bootstrap_targets {
        applied.bootstrap = BootstrapConfig {
            targets: targets.clone(),
        };
    }
    applied
}

/// 選択と適用をまとめて行う。選んだprofileのIDを返す。
#[must_use]
pub fn resolve(
    spec: &WorkflowSpec,
    inputs: &BTreeMap<InputId, String>,
    checkout: Option<&Path>,
) -> (WorkflowSpec, Option<String>) {
    if spec.profiles.is_empty() {
        return (spec.clone(), None);
    }
    let Some(branch) = branch_for(spec, inputs, checkout) else {
        return (spec.clone(), None);
    };
    match select(spec, &branch) {
        Some(profile) => (apply(spec, profile), Some(profile.id.clone())),
        None => (spec.clone(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{InputDefault, TaskId};

    #[test]
    fn patterns_follow_path_segments() {
        assert!(branch_matches("docs/*", "docs/readme"));
        assert!(!branch_matches("docs/*", "docs/a/b"));
        assert!(branch_matches("docs/**", "docs/a/b"));
        assert!(branch_matches("**", "anything/at/all"));
        assert!(branch_matches("release-?.?", "release-1.2"));
        assert!(!branch_matches("release-?", "release-/"));
        assert!(branch_matches("main", "main"));
        assert!(!branch_matches("main", "main2"));
        assert!(branch_matches("*-hotfix", "x-hotfix"));
    }

    #[test]
    fn first_matching_profile_replaces_defaults_and_bootstrap() {
        let mut spec =
            crate::config::load::load_str(include_str!("../../examples/workflow.yaml")).unwrap();
        spec.profiles = vec![
            ProfileDef {
                id: "docs".to_string(),
                branch: "docs/**".to_string(),
                inputs: BTreeMap::from([(
                    InputId("port".to_string()),
                    InputDefault::Integer(4000),
                )]),
                bootstrap_targets: Some(vec![TaskId::from("toolchain")]),
            },
            ProfileDef {
                id: "all".to_string(),
                branch: "**".to_string(),
                inputs: BTreeMap::new(),
                bootstrap_targets: None,
            },
        ];
        crate::config::validate::validate(&spec).unwrap();
        let inputs =
            |branch: &str| BTreeMap::from([(InputId("branch".to_string()), branch.to_string())]);
        let (applied, chosen) = resolve(&spec, &inputs("docs/guide"), None);
        assert_eq!(chosen.as_deref(), Some("docs"));
        assert_eq!(applied.bootstrap.targets, vec![TaskId::from("toolchain")]);
        assert_eq!(
            applied.inputs[&InputId("port".to_string())].default,
            Some(InputDefault::Integer(4000))
        );
        let (applied, chosen) = resolve(&spec, &inputs("feature/x"), None);
        assert_eq!(chosen.as_deref(), Some("all"));
        assert_eq!(applied.bootstrap, spec.bootstrap);
    }

    #[test]
    fn profiles_cannot_change_the_branch_or_unknown_inputs() {
        let mut spec =
            crate::config::load::load_str(include_str!("../../examples/workflow.yaml")).unwrap();
        let profile = |inputs: BTreeMap<InputId, InputDefault>| ProfileDef {
            id: "p".to_string(),
            branch: "**".to_string(),
            inputs,
            bootstrap_targets: None,
        };
        spec.profiles = vec![profile(BTreeMap::from([(
            InputId("branch".to_string()),
            InputDefault::String("x".to_string()),
        )]))];
        assert!(crate::config::validate::validate(&spec).is_err());
        spec.profiles = vec![profile(BTreeMap::from([(
            InputId("nope".to_string()),
            InputDefault::Integer(1),
        )]))];
        assert!(crate::config::validate::validate(&spec).is_err());
        spec.profiles = vec![profile(BTreeMap::from([(
            InputId("port".to_string()),
            InputDefault::String("x".to_string()),
        )]))];
        assert!(crate::config::validate::validate(&spec).is_err());
    }
}
