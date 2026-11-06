//! 実行承認とHerdrイベントの自動受付(docs/specs/security.md、recovery.md)。
//!
//! 承認は「リポジトリ(共通Gitディレクトリ)＋設定ハッシュ」の組で記録する
//! (未決事項Q09の起点)。設定ハッシュはworkflow設定の本文とプラグイン全体設定から
//! 計算するため、設定を変更すると古い承認では自動実行しない(T25)。

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::model::{EnvValue, TaskDef, WorkflowSpec};
use crate::config::plugin::PluginConfig;
use crate::runtime::git;

/// 承認対象のハッシュ。秘密値は名前だけが設定に現れるため値は含まない。
#[must_use]
pub fn config_hash(workflow_text: &str, plugin_config: &PluginConfig) -> String {
    let mut hasher = Sha256::new();
    hasher.update(workflow_text.as_bytes());
    hasher.update([0]);
    hasher.update(serde_json::to_vec(plugin_config).unwrap_or_default());
    let digest = hasher.finalize();
    let mut hex = String::from("sha256:");
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// 承認のリポジトリ識別子。Gitリポジトリなら共通Gitディレクトリ、そうでなければ設定の場所。
#[must_use]
pub fn repo_key(config_dir: &Path) -> String {
    git::inspect(config_dir).map_or_else(
        |_| {
            std::fs::canonicalize(config_dir)
                .unwrap_or_else(|_| config_dir.to_path_buf())
                .display()
                .to_string()
        },
        |info| info.common_dir.display().to_string(),
    )
}

/// 承認時に表示する実行内容(環境変数の値と秘密値は表示しない)。
#[must_use]
pub fn summary(spec: &WorkflowSpec) -> String {
    let mut out = format!("workflow: {}\n", spec.name);
    if let Some(files) = &spec.files {
        for rule in &files.copy {
            let _ = writeln!(
                out,
                "copy: {} -> {} (Agentを含むcheckout内の処理から読めます)",
                rule.from, rule.to
            );
        }
    }
    for (id, task) in &spec.tasks {
        match task {
            TaskDef::Command {
                argv,
                script,
                env,
                locks,
                wait_for,
                ..
            } => {
                let command = match (argv, script) {
                    (Some(argv), _) => argv.join(" "),
                    (None, Some(script)) => {
                        format!("{} <<script>>\n{}", script.shell.join(" "), script.source)
                    }
                    (None, None) => String::new(),
                };
                let _ = writeln!(out, "task {}: {command}", id.0);
                let secrets: Vec<&str> = env
                    .values()
                    .filter_map(|value| match value {
                        EnvValue::Secret(secret) => Some(secret.secret.as_str()),
                        _ => None,
                    })
                    .collect();
                if !env.is_empty() {
                    let names: Vec<&str> = env.keys().map(String::as_str).collect();
                    let _ = writeln!(out, "  env: {}", names.join(", "));
                }
                if !secrets.is_empty() {
                    let _ = writeln!(out, "  secrets: {}", secrets.join(", "));
                }
                if !locks.is_empty() {
                    let names: Vec<&str> = locks.iter().map(|lock| lock.0.as_str()).collect();
                    let _ = writeln!(out, "  locks: {}", names.join(", "));
                }
                if let Some(crate::config::model::WaitFor::Http { url, .. }) = wait_for.as_deref() {
                    let _ = writeln!(out, "  network: {url}");
                }
            }
            TaskDef::Agent { agent, .. } => {
                let _ = writeln!(out, "agent {}: {:?}", id.0, agent.kind);
            }
        }
    }
    out
}

/// 有効なGitのhook(実行可能で`.sample`でないもの)と、clean・smudge等のフィルター。
/// worktreeの作成やcheckoutでもコマンド実行につながるため、承認時に表示する。
#[must_use]
pub fn git_side_effects(repo: &Path) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let mut found = Vec::new();
    if let Ok(Some(hooks)) = git::git(repo, &["rev-parse", "--git-path", "hooks"]) {
        let hooks = hooks.trim();
        let dir = if Path::new(hooks).is_absolute() {
            PathBuf::from(hooks)
        } else {
            repo.join(hooks)
        };
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut names: Vec<String> = entries
                .flatten()
                .filter(|entry| {
                    entry.metadata().is_ok_and(|metadata| {
                        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                    })
                })
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| !name.ends_with(".sample"))
                .collect();
            names.sort();
            found.extend(names.into_iter().map(|name| format!("git hook: {name}")));
        }
    }
    if let Ok(Some(filters)) = git::git(
        repo,
        &[
            "config",
            "--get-regexp",
            r"^filter\..*\.(clean|smudge|process)$",
        ],
    ) {
        found.extend(
            filters
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| format!("git filter: {line}")),
        );
    }
    found
}

/// Herdrが渡すイベントから取り出したworktree情報。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEvent {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub is_detached: bool,
    pub is_linked_worktree: Option<bool>,
}

fn find_worktree(value: &Value) -> Option<&Value> {
    if let Some(worktree) = value.get("worktree")
        && worktree.get("path").is_some()
    {
        return Some(worktree);
    }
    match value {
        Value::Object(map) => map.values().find_map(find_worktree),
        _ => None,
    }
}

/// `HERDR_PLUGIN_EVENT_JSON`から`worktree`記録(`path`、`branch`、`is_detached`)を探す。
/// Herdr v0.9.5の公開スキーマの`WorktreeInfo`に従い、外側の包み方には依存しない。
#[must_use]
pub fn parse_worktree_event(json: &str) -> Option<WorktreeEvent> {
    let value: Value = serde_json::from_str(json).ok()?;
    let worktree = find_worktree(&value)?;
    Some(WorktreeEvent {
        path: PathBuf::from(worktree.get("path")?.as_str()?),
        branch: worktree
            .get("branch")
            .and_then(Value::as_str)
            .map(str::to_string),
        is_detached: worktree
            .get("is_detached")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        is_linked_worktree: worktree.get("is_linked_worktree").and_then(Value::as_bool),
    })
}

/// 自動受付の重複判定キー(checkout世代＋workflow＋設定ハッシュ)。同じパスに作り直された
/// checkoutは、ディレクトリのinodeと作成時刻(取得できる場合)で別の世代として扱う。
/// 中身の変更で変わるctimeやmtimeは使わない。
#[must_use]
pub fn acceptance_key(checkout: &Path, workflow: &str, config_hash: &str) -> String {
    use std::os::unix::fs::MetadataExt;
    let generation = std::fs::metadata(checkout)
        .map(|metadata| {
            let created = metadata
                .created()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_nanos());
            format!("{}:{}:{created}", metadata.dev(), metadata.ino())
        })
        .unwrap_or_default();
    let mut hasher = Sha256::new();
    for part in [
        checkout.display().to_string().as_str(),
        generation.as_str(),
        workflow,
        config_hash,
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    digest.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_changes_with_config_text() {
        let plugin = PluginConfig::default();
        assert_ne!(config_hash("a", &plugin), config_hash("b", &plugin));
        assert_eq!(config_hash("a", &plugin), config_hash("a", &plugin));
    }

    #[test]
    fn git_hooks_and_filters_are_listed() {
        use crate::runtime::git::test_repo::{init, run};
        use crate::runtime::state_dir::test_support::TempDir;
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new("approval");
        let repo = init(&tmp.0.join("repo"));
        // 利用者全体の設定(例: git-lfs)のフィルターも対象になるため、差分で確かめる。
        let before = git_side_effects(&repo);
        assert!(!before.iter().any(|item| item.starts_with("git hook:")));
        let hook = repo.join(".git/hooks/post-checkout");
        std::fs::write(&hook, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        run(
            &repo,
            &["config", "filter.custom.smudge", "custom-smudge %f"],
        );
        let after = git_side_effects(&repo);
        assert!(
            after.contains(&"git hook: post-checkout".to_string()),
            "{after:?}"
        );
        assert!(
            after
                .iter()
                .any(|item| item == "git filter: filter.custom.smudge custom-smudge %f"),
            "{after:?}"
        );
        assert_eq!(after.len(), before.len() + 2);
    }

    #[test]
    fn worktree_event_is_found_in_documented_shape() {
        let json = r#"{"type":"worktree_created","workspace":{"workspace_id":"w2"},"worktree":{"path":"/r/wt","branch":"feature/x","is_bare":false,"is_detached":false,"is_prunable":false,"is_linked_worktree":true,"label":"x"}}"#;
        let event = parse_worktree_event(json).unwrap();
        assert_eq!(event.path, PathBuf::from("/r/wt"));
        assert_eq!(event.branch.as_deref(), Some("feature/x"));
        assert_eq!(event.is_linked_worktree, Some(true));
        let wrapped = format!(r#"{{"event":"worktree.created","data":{json}}}"#);
        assert_eq!(parse_worktree_event(&wrapped), Some(event));
        assert_eq!(parse_worktree_event("{}"), None);
    }
}
