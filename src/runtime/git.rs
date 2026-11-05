//! Git CLIによるcheckoutの照合(docs/specs/worktree.md)。
//!
//! `.git`ファイルや管理ディレクトリを独自に読まず、`git rev-parse`、
//! `git worktree list --porcelain`、`git ls-files`、`git check-ignore`の結果だけを使う。

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GitError {
    #[error("failed to run git: {0}")]
    Spawn(String),
    #[error("{path} is not a Git working tree")]
    NotWorkTree { path: PathBuf },
    #[error("checkout {checkout} belongs to a different repository than the workflow config")]
    DifferentRepository { checkout: PathBuf },
    #[error(
        "checkout {checkout} is in detached HEAD; worktree.branch {expected:?} cannot be verified"
    )]
    Detached { checkout: PathBuf, expected: String },
    #[error("checkout {checkout} is on branch {actual:?}, expected {expected:?}")]
    BranchMismatch {
        checkout: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("cannot determine the primary worktree of {path} (bare repository?)")]
    NoPrimary { path: PathBuf },
}

/// gitを実行し、成功時の標準出力を返す。失敗時は`Ok(None)`。
pub fn git(dir: &Path, args: &[&str]) -> Result<Option<String>, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| GitError::Spawn(error.to_string()))?;
    if output.status.success() {
        Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
    } else {
        Ok(None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutInfo {
    pub toplevel: PathBuf,
    pub common_dir: PathBuf,
    /// detached HEADでは`None`。
    pub branch: Option<String>,
    /// Gitが認識するmain worktree。bareリポジトリ等では`None`。
    pub primary: Option<PathBuf>,
}

fn canonical(base: &Path, value: &str) -> PathBuf {
    let path = Path::new(value.trim());
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// checkoutの最上位、共通Gitディレクトリ、ブランチ、main worktreeを調べる。
pub fn inspect(path: &Path) -> Result<CheckoutInfo, GitError> {
    let not_tree = || GitError::NotWorkTree {
        path: path.to_path_buf(),
    };
    let inside = git(path, &["rev-parse", "--is-inside-work-tree"])?.ok_or_else(not_tree)?;
    if inside.trim() != "true" {
        return Err(not_tree());
    }
    let toplevel = git(path, &["rev-parse", "--show-toplevel"])?.ok_or_else(not_tree)?;
    let toplevel = canonical(path, &toplevel);
    let common = git(path, &["rev-parse", "--git-common-dir"])?.ok_or_else(not_tree)?;
    let common_dir = canonical(path, &common);
    let branch = git(path, &["symbolic-ref", "-q", "--short", "HEAD"])?
        .map(|branch| branch.trim().to_string())
        .filter(|branch| !branch.is_empty());
    let primary = git(path, &["worktree", "list", "--porcelain"])?.and_then(|list| {
        let mut lines = list.lines();
        let first = lines.next()?.strip_prefix("worktree ")?.to_string();
        // 最初の項目がbareならコピー元を確定できない。
        let bare = lines
            .take_while(|line| !line.is_empty())
            .any(|line| line == "bare");
        (!bare).then(|| canonical(path, &first))
    });
    Ok(CheckoutInfo {
        toplevel,
        common_dir,
        branch,
        primary,
    })
}

/// `attach`の対象を確認する(T37)。`config_dir`がGitリポジトリ内なら同じリポジトリか、
/// `expected_branch`があれば現在のブランチと一致するかを確かめる。
pub fn validate_attach(
    checkout: &Path,
    config_dir: Option<&Path>,
    expected_branch: Option<&str>,
) -> Result<CheckoutInfo, GitError> {
    let info = inspect(checkout)?;
    if let Some(config_dir) = config_dir
        && let Ok(config_repo) = inspect(config_dir)
        && config_repo.common_dir != info.common_dir
    {
        return Err(GitError::DifferentRepository {
            checkout: info.toplevel,
        });
    }
    if let Some(expected) = expected_branch {
        match &info.branch {
            None => {
                return Err(GitError::Detached {
                    checkout: info.toplevel,
                    expected: expected.to_string(),
                });
            }
            Some(actual) if actual != expected => {
                return Err(GitError::BranchMismatch {
                    checkout: info.toplevel.clone(),
                    expected: expected.to_string(),
                    actual: actual.clone(),
                });
            }
            Some(_) => {}
        }
    }
    Ok(info)
}

/// `relative`がGitの追跡対象か。
pub fn is_tracked(repo: &Path, relative: &Path) -> Result<bool, GitError> {
    let relative = relative.to_string_lossy();
    Ok(git(repo, &["ls-files", "--error-unmatch", "--", &relative])?.is_some())
}

/// `relative`がignore対象か。
pub fn is_ignored(repo: &Path, relative: &Path) -> Result<bool, GitError> {
    let relative = relative.to_string_lossy();
    Ok(git(repo, &["check-ignore", "-q", "--no-index", "--", &relative])?.is_some())
}

/// 状態の要約(削除前の確認に使う)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeChanges {
    pub tracked_changes: Vec<String>,
    pub untracked: Vec<String>,
    pub ignored: Vec<String>,
}

impl WorktreeChanges {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.tracked_changes.is_empty() && self.untracked.is_empty() && self.ignored.is_empty()
    }
}

/// 追跡ファイルの変更、未追跡ファイル、ignore対象ファイルを列挙する。
pub fn changes(checkout: &Path) -> Result<WorktreeChanges, GitError> {
    let status = git(
        checkout,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignored=matching",
        ],
    )?
    .ok_or_else(|| GitError::NotWorkTree {
        path: checkout.to_path_buf(),
    })?;
    let mut changes = WorktreeChanges::default();
    for line in status.lines() {
        if line.len() < 4 {
            continue;
        }
        let (code, path) = line.split_at(3);
        let path = path.to_string();
        match &code[..2] {
            "??" => changes.untracked.push(path),
            "!!" => changes.ignored.push(path),
            _ => changes.tracked_changes.push(path),
        }
    }
    Ok(changes)
}

#[cfg(test)]
pub(crate) mod test_repo {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?}: {status:?}");
    }

    /// mainブランチに1コミットを持つリポジトリを作る。
    pub fn init(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        run(dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join(".gitignore"), ".env*\nlocal/\n").unwrap();
        std::fs::write(dir.join("README"), "x\n").unwrap();
        run(dir, &["add", "."]);
        run(dir, &["commit", "-q", "-m", "init"]);
        std::fs::canonicalize(dir).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::test_repo::{init, run};
    use super::*;
    use crate::runtime::state_dir::test_support::TempDir;

    #[test]
    fn inspect_reports_branch_and_primary() {
        let tmp = TempDir::new("git");
        let repo = init(&tmp.0.join("repo"));
        run(
            &repo,
            &["worktree", "add", "-q", "-b", "feature/x", "../wt"],
        );
        let wt = std::fs::canonicalize(tmp.0.join("wt")).unwrap();
        let info = inspect(&wt).unwrap();
        assert_eq!(info.toplevel, wt);
        assert_eq!(info.branch.as_deref(), Some("feature/x"));
        assert_eq!(info.primary.as_deref(), Some(repo.as_path()));
        assert_eq!(info.common_dir, inspect(&repo).unwrap().common_dir);
    }

    #[test]
    fn attach_rejects_mismatch_detached_and_other_repository() {
        // T37
        let tmp = TempDir::new("git");
        let repo = init(&tmp.0.join("repo"));
        run(
            &repo,
            &["worktree", "add", "-q", "-b", "feature/x", "../wt"],
        );
        let wt = tmp.0.join("wt");
        assert!(validate_attach(&wt, Some(&repo), Some("feature/x")).is_ok());
        assert!(matches!(
            validate_attach(&wt, Some(&repo), Some("feature/y")),
            Err(GitError::BranchMismatch { .. })
        ));
        run(&wt, &["checkout", "-q", "--detach"]);
        assert!(matches!(
            validate_attach(&wt, Some(&repo), Some("feature/x")),
            Err(GitError::Detached { .. })
        ));
        // ブランチを指定しなければdetachedでも採用できる。
        assert!(validate_attach(&wt, Some(&repo), None).is_ok());
        let other = init(&tmp.0.join("other"));
        assert!(matches!(
            validate_attach(&other, Some(&repo), None),
            Err(GitError::DifferentRepository { .. })
        ));
        let plain = tmp.0.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(matches!(
            validate_attach(&plain, None, None),
            Err(GitError::NotWorkTree { .. })
        ));
    }

    #[test]
    fn changes_lists_tracked_untracked_and_ignored() {
        let tmp = TempDir::new("git");
        let repo = init(&tmp.0.join("repo"));
        assert!(changes(&repo).unwrap().is_clean());
        std::fs::write(repo.join("README"), "changed\n").unwrap();
        std::fs::write(repo.join("new.txt"), "n\n").unwrap();
        std::fs::write(repo.join(".env"), "SECRET=1\n").unwrap();
        let found = changes(&repo).unwrap();
        assert_eq!(found.tracked_changes, ["README"]);
        assert_eq!(found.untracked, ["new.txt"]);
        assert_eq!(found.ignored, [".env"]);
        assert!(is_tracked(&repo, Path::new("README")).unwrap());
        assert!(!is_tracked(&repo, Path::new(".env")).unwrap());
        assert!(is_ignored(&repo, Path::new(".env")).unwrap());
        assert!(!is_ignored(&repo, Path::new("new.txt")).unwrap());
    }
}
