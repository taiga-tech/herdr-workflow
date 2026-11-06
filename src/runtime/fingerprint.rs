//! 再開時に比べるcheckoutの記録(docs/specs/recovery.md#resume-changes、Q15)。
//!
//! Run開始時と再開時に、checkoutの世代、HEAD、依存関係とツールの定義ファイルを記録する。
//! 再開時に記録と現在を比べ、変更を利用者へ示す。成功済みという記録だけで、
//! lockfile等が変わった後の初期化を省略しない。

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::runtime::git;

/// 変更を検出するcheckout直下のファイル(依存関係のlockfileとツールの定義)。
pub const WATCHED_FILES: &[&str] = &[
    "pnpm-lock.yaml",
    "package-lock.json",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "Cargo.lock",
    "go.sum",
    "uv.lock",
    "poetry.lock",
    "Gemfile.lock",
    "composer.lock",
    "mise.toml",
    ".mise.toml",
    "mise.lock",
    ".tool-versions",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutFingerprint {
    /// ディレクトリの世代(dev:inode:作成時刻)。同じパスに作り直すと変わる。
    pub generation: String,
    /// HEADのコミット。Gitでなければ`None`。
    pub head: Option<String>,
    /// 存在した監視対象ファイルの内容ハッシュ。
    pub files: BTreeMap<String, String>,
}

/// checkoutディレクトリの世代。中身の変更で変わるctimeやmtimeは使わない。
/// 取得できなければ空文字列。
#[must_use]
pub fn checkout_generation(checkout: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(checkout)
        .map(|metadata| {
            let created = metadata
                .created()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_nanos());
            format!("{}:{}:{created}", metadata.dev(), metadata.ino())
        })
        .unwrap_or_default()
}

fn file_hash(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let digest = Sha256::digest(&bytes);
    Some(digest.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    }))
}

#[must_use]
pub fn capture(checkout: &Path) -> CheckoutFingerprint {
    let head = git::git(checkout, &["rev-parse", "--verify", "-q", "HEAD"])
        .ok()
        .flatten()
        .map(|head| head.trim().to_string())
        .filter(|head| !head.is_empty());
    let files = WATCHED_FILES
        .iter()
        .filter_map(|name| file_hash(&checkout.join(name)).map(|hash| ((*name).to_string(), hash)))
        .collect();
    CheckoutFingerprint {
        generation: checkout_generation(checkout),
        head,
        files,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeKind {
    /// checkoutが同じパスに作り直された。
    Recreated,
    /// lockfile等の監視対象ファイルが変わった。
    Inputs,
    /// workflow設定のファイルが記録した本文と異なる。
    Config,
    /// 記録したHerdrのpaneが見つからない。
    Pane,
    /// HEADが変わった(表示のみ)。
    Head,
}

impl ChangeKind {
    /// 再開前に利用者の確認を必要とする変更か。HEADの移動はAgentの作業で日常的に起こるため、
    /// 表示だけにする。
    #[must_use]
    pub fn needs_confirmation(self) -> bool {
        !matches!(self, Self::Head)
    }

    /// 成功済みのタスクを再実行させる変更か。
    #[must_use]
    pub fn invalidates_success(self) -> bool {
        matches!(self, Self::Recreated | Self::Inputs)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub kind: ChangeKind,
    pub detail: String,
}

/// 記録(`before`)と現在(`after`)の差分。記録がない古いRunや、checkoutが読めない場合は比べない。
#[must_use]
pub fn compare(before: &CheckoutFingerprint, after: &CheckoutFingerprint) -> Vec<Change> {
    let mut changes = Vec::new();
    if after.generation.is_empty() {
        return changes;
    }
    if !before.generation.is_empty() && before.generation != after.generation {
        changes.push(Change {
            kind: ChangeKind::Recreated,
            detail: "checkout was re-created at the same path since the run started".to_string(),
        });
    }
    for name in WATCHED_FILES {
        let detail = match (before.files.get(*name), after.files.get(*name)) {
            (Some(old), Some(new)) if old != new => format!("{name} changed"),
            (Some(_), None) => format!("{name} was removed"),
            (None, Some(_)) => format!("{name} was added"),
            _ => continue,
        };
        changes.push(Change {
            kind: ChangeKind::Inputs,
            detail,
        });
    }
    if before.head != after.head {
        let short = |head: &Option<String>| {
            head.as_deref().map_or_else(
                || "(none)".to_string(),
                |head| head.chars().take(12).collect(),
            )
        };
        changes.push(Change {
            kind: ChangeKind::Head,
            detail: format!(
                "HEAD moved {} -> {}",
                short(&before.head),
                short(&after.head)
            ),
        });
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::git::test_repo::{init, run};
    use crate::runtime::state_dir::test_support::TempDir;

    #[test]
    fn lockfile_and_head_changes_are_reported() {
        let tmp = TempDir::new("fingerprint");
        let repo = init(&tmp.0.join("repo"));
        std::fs::write(repo.join("pnpm-lock.yaml"), "a").unwrap();
        let before = capture(&repo);
        assert!(before.head.is_some());
        assert!(compare(&before, &capture(&repo)).is_empty());

        std::fs::write(repo.join("pnpm-lock.yaml"), "b").unwrap();
        std::fs::write(repo.join("mise.toml"), "[tools]\n").unwrap();
        let changes = compare(&before, &capture(&repo));
        let details: Vec<&str> = changes.iter().map(|c| c.detail.as_str()).collect();
        assert_eq!(details, ["pnpm-lock.yaml changed", "mise.toml was added"]);
        assert!(changes.iter().all(|c| c.kind.invalidates_success()));

        std::fs::write(repo.join("README"), "x").unwrap();
        run(&repo, &["add", "README"]);
        run(&repo, &["commit", "-q", "-m", "next"]);
        let after = capture(&repo);
        let head: Vec<Change> = compare(&before, &after)
            .into_iter()
            .filter(|c| c.kind == ChangeKind::Head)
            .collect();
        assert_eq!(head.len(), 1);
        assert!(!head[0].kind.needs_confirmation());
        assert!(!head[0].kind.invalidates_success());
    }

    #[test]
    fn recreated_checkout_is_detected_and_missing_one_is_not_compared() {
        let tmp = TempDir::new("fingerprint");
        let dir = tmp.0.join("work");
        std::fs::create_dir(&dir).unwrap();
        let before = capture(&dir);
        // inodeと作成時刻を取得できないファイルシステムでは世代を区別できないため、
        // 区別できた場合だけ確かめる。
        let keep = tmp.0.join("keep");
        std::fs::create_dir(&keep).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        std::fs::create_dir(&dir).unwrap();
        let after = capture(&dir);
        if before.generation != after.generation {
            let changes = compare(&before, &after);
            assert_eq!(changes[0].kind, ChangeKind::Recreated);
            assert!(changes[0].kind.needs_confirmation());
        }
        std::fs::remove_dir(&dir).unwrap();
        assert!(compare(&before, &capture(&dir)).is_empty());
    }
}
