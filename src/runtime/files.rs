//! `files.copy`の実行(docs/specs/worktree.md「ファイルコピー」、T22・T23)。
//!
//! すべての規則を先に検査し、一つでも違反があればファイルを変更しない。コピー中に
//! 失敗した場合は、この操作で記録した変更だけを逆順に戻す。

use std::path::{Component, Path, PathBuf};

use crate::config::model::{FilesConfig, IfExistsPolicy};
use crate::runtime::git::{self, GitError};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CopyError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("files.copy: path {0:?} must be relative and stay inside the checkout")]
    UnsafePath(String),
    #[error("files.copy: {path} contains a symbolic link ({link}); symlinks are rejected")]
    Symlink { path: String, link: PathBuf },
    #[error("files.copy: source {0} does not exist")]
    MissingSource(String),
    #[error("files.copy: source {0} is not a regular file")]
    NotAFile(String),
    #[error("files.copy: source {0} is tracked by Git; only untracked ignored files are copied")]
    SourceTracked(String),
    #[error("files.copy: source {0} is not ignored by Git (requireIgnored)")]
    SourceNotIgnored(String),
    #[error("files.copy: destination {0} is tracked by Git in the checkout")]
    DestinationTracked(String),
    #[error("files.copy: destination {0} already exists (ifExists: error)")]
    DestinationExists(String),
    #[error("files.copy failed at {path}: {message}; rolled back {rolled_back} change(s){}", unrecovered_suffix(.unrecovered))]
    Io {
        path: String,
        message: String,
        rolled_back: usize,
        unrecovered: Vec<String>,
    },
}

fn unrecovered_suffix(unrecovered: &[String]) -> String {
    if unrecovered.is_empty() {
        String::new()
    } else {
        format!("; could not restore: {}", unrecovered.join(", "))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyReport {
    pub copied: Vec<String>,
    pub skipped: Vec<(String, String)>,
}

fn safe_relative(raw: &str) -> Result<PathBuf, CopyError> {
    let path = Path::new(raw);
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            _ => return Err(CopyError::UnsafePath(raw.to_string())),
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(CopyError::UnsafePath(raw.to_string()));
    }
    Ok(normalized)
}

/// 途中の親ディレクトリと対象のどこにもsymlinkがないことを確かめる。
fn reject_symlinks(root: &Path, relative: &Path, raw: &str) -> Result<(), CopyError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(CopyError::Symlink {
                    path: raw.to_string(),
                    link: current,
                });
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    Ok(())
}

struct Planned {
    from: PathBuf,
    to: PathBuf,
    label: String,
    replace: bool,
}

enum Change {
    CreatedDir(PathBuf),
    CreatedFile(PathBuf),
    Replaced { target: PathBuf, backup: PathBuf },
}

fn io_failure(path: &Path, error: &std::io::Error, changes: Vec<Change>) -> CopyError {
    let total = changes.len();
    let unrecovered = rollback(changes);
    CopyError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
        rolled_back: total - unrecovered.len(),
        unrecovered,
    }
}

/// 記録した変更を逆順に戻し、戻せなかったパスを返す。
fn rollback(changes: Vec<Change>) -> Vec<String> {
    let mut unrecovered = Vec::new();
    for change in changes.into_iter().rev() {
        let result = match &change {
            Change::CreatedFile(path) => std::fs::remove_file(path),
            Change::CreatedDir(path) => std::fs::remove_dir(path),
            Change::Replaced { target, backup } => std::fs::rename(backup, target),
        };
        if result.is_err() {
            unrecovered.push(match change {
                Change::CreatedFile(path) | Change::CreatedDir(path) => path.display().to_string(),
                Change::Replaced { target, .. } => target.display().to_string(),
            });
        }
    }
    unrecovered
}

/// `source`(main worktree)から`checkout`へ許可されたファイルをコピーする。
pub fn apply(files: &FilesConfig, source: &Path, checkout: &Path) -> Result<CopyReport, CopyError> {
    apply_with_hook(files, source, checkout, &mut |_| Ok(()))
}

/// `before_copy`はコピーごとに呼ばれる(試験で途中失敗を作るため)。
fn apply_with_hook(
    files: &FilesConfig,
    source: &Path,
    checkout: &Path,
    before_copy: &mut dyn FnMut(usize) -> std::io::Result<()>,
) -> Result<CopyReport, CopyError> {
    let mut report = CopyReport::default();
    let source = std::fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    let checkout = std::fs::canonicalize(checkout).unwrap_or_else(|_| checkout.to_path_buf());
    if source == checkout {
        for rule in &files.copy {
            report.skipped.push((
                rule.to.clone(),
                "source and checkout are the same".to_string(),
            ));
        }
        return Ok(report);
    }

    // 1段階目: すべて検査する。ここではファイルを変更しない。
    let mut planned = Vec::new();
    for rule in &files.copy {
        let from_rel = safe_relative(&rule.from)?;
        let to_rel = safe_relative(&rule.to)?;
        reject_symlinks(&source, &from_rel, &rule.from)?;
        reject_symlinks(&checkout, &to_rel, &rule.to)?;
        let from = source.join(&from_rel);
        let metadata = match std::fs::symlink_metadata(&from) {
            Ok(metadata) => metadata,
            Err(_) if rule.optional => {
                report
                    .skipped
                    .push((rule.to.clone(), "optional source is missing".to_string()));
                continue;
            }
            Err(_) => return Err(CopyError::MissingSource(rule.from.clone())),
        };
        if !metadata.is_file() {
            return Err(CopyError::NotAFile(rule.from.clone()));
        }
        if git::is_tracked(&source, &from_rel)? {
            return Err(CopyError::SourceTracked(rule.from.clone()));
        }
        if files.require_ignored && !git::is_ignored(&source, &from_rel)? {
            return Err(CopyError::SourceNotIgnored(rule.from.clone()));
        }
        if git::is_tracked(&checkout, &to_rel)? {
            return Err(CopyError::DestinationTracked(rule.to.clone()));
        }
        let to = checkout.join(&to_rel);
        let exists = std::fs::symlink_metadata(&to).is_ok();
        let replace = match (exists, rule.if_exists.unwrap_or_default()) {
            (false, _) => false,
            (true, IfExistsPolicy::Error) => {
                return Err(CopyError::DestinationExists(rule.to.clone()));
            }
            (true, IfExistsPolicy::Skip) => {
                report.skipped.push((
                    rule.to.clone(),
                    "destination exists (ifExists: skip)".to_string(),
                ));
                continue;
            }
            (true, IfExistsPolicy::Replace) => true,
        };
        planned.push(Planned {
            from,
            to,
            label: rule.to.clone(),
            replace,
        });
    }

    // 2段階目: 一時ファイルへ準備してから置き換え、変更を記録する。
    let mut changes = Vec::new();
    for (index, item) in planned.into_iter().enumerate() {
        if let Err(error) = before_copy(index) {
            return Err(io_failure(&item.to, &error, changes));
        }
        let Some(parent) = item.to.parent() else {
            continue;
        };
        let mut missing = Vec::new();
        let mut cursor = parent.to_path_buf();
        while !cursor.exists() {
            missing.push(cursor.clone());
            if !cursor.pop() {
                break;
            }
        }
        for dir in missing.into_iter().rev() {
            if let Err(error) = std::fs::create_dir(&dir) {
                return Err(io_failure(&dir, &error, changes));
            }
            changes.push(Change::CreatedDir(dir));
        }
        // 検査後の差し替えに備え、操作直前にもsymlinkを確認する。
        if std::fs::symlink_metadata(&item.from).is_ok_and(|m| m.file_type().is_symlink())
            || std::fs::symlink_metadata(&item.to).is_ok_and(|m| m.file_type().is_symlink())
        {
            let unrecovered = rollback(changes);
            return Err(CopyError::Io {
                path: item.label,
                message: "path became a symbolic link during the copy".to_string(),
                rolled_back: 0,
                unrecovered,
            });
        }
        let name = item
            .to
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tmp = parent.join(format!(".{name}.herdr-workflow.tmp"));
        if let Err(error) = std::fs::copy(&item.from, &tmp) {
            let _ = std::fs::remove_file(&tmp);
            return Err(io_failure(&item.to, &error, changes));
        }
        if item.replace {
            let backup = parent.join(format!(".{name}.herdr-workflow.bak"));
            if let Err(error) = std::fs::rename(&item.to, &backup) {
                let _ = std::fs::remove_file(&tmp);
                return Err(io_failure(&item.to, &error, changes));
            }
            changes.push(Change::Replaced {
                target: item.to.clone(),
                backup,
            });
        }
        if let Err(error) = std::fs::rename(&tmp, &item.to) {
            let _ = std::fs::remove_file(&tmp);
            return Err(io_failure(&item.to, &error, changes));
        }
        if !item.replace {
            changes.push(Change::CreatedFile(item.to.clone()));
        }
        report.copied.push(item.label);
    }
    // 成功したら置換前のバックアップを片付ける。
    for change in changes {
        if let Change::Replaced { backup, .. } = change {
            let _ = std::fs::remove_file(backup);
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{FileCopyRule, FilesSource, SymlinkPolicy};
    use crate::runtime::git::test_repo::{init, run};
    use crate::runtime::state_dir::test_support::TempDir;

    fn config(rules: Vec<FileCopyRule>) -> FilesConfig {
        FilesConfig {
            source: FilesSource::Primary,
            copy: rules,
            symlinks: SymlinkPolicy::Reject,
            require_ignored: true,
        }
    }

    fn rule(from: &str, to: &str, if_exists: Option<IfExistsPolicy>) -> FileCopyRule {
        FileCopyRule {
            from: from.to_string(),
            to: to.to_string(),
            optional: false,
            if_exists,
        }
    }

    fn setup() -> (TempDir, PathBuf, PathBuf) {
        let tmp = TempDir::new("files");
        let repo = init(&tmp.0.join("repo"));
        run(&repo, &["worktree", "add", "-q", "-b", "feature", "../wt"]);
        let wt = std::fs::canonicalize(tmp.0.join("wt")).unwrap();
        std::fs::write(repo.join(".env.local"), "TOKEN=1\n").unwrap();
        (tmp, repo, wt)
    }

    #[test]
    fn copies_ignored_untracked_file() {
        let (_tmp, repo, wt) = setup();
        let report = apply(
            &config(vec![rule(".env.local", ".env.local", None)]),
            &repo,
            &wt,
        )
        .unwrap();
        assert_eq!(report.copied, [".env.local"]);
        assert_eq!(
            std::fs::read_to_string(wt.join(".env.local")).unwrap(),
            "TOKEN=1\n"
        );
    }

    #[test]
    fn rejections_leave_files_unchanged() {
        // T22: 追跡済みのコピー先、パス脱出、symlinkは変更せずに拒否する。
        let (_tmp, repo, wt) = setup();
        let rules = vec![
            rule(".env.local", ".env.local", None),
            rule(".env.local", "README", None),
        ];
        assert_eq!(
            apply(&config(rules), &repo, &wt),
            Err(CopyError::DestinationTracked("README".to_string()))
        );
        assert!(!wt.join(".env.local").exists());
        assert_eq!(
            apply(&config(vec![rule("../x", ".env", None)]), &repo, &wt),
            Err(CopyError::UnsafePath("../x".to_string()))
        );
        std::os::unix::fs::symlink(repo.join(".env.local"), repo.join(".env.link")).unwrap();
        assert!(matches!(
            apply(
                &config(vec![rule(".env.link", ".env.link", None)]),
                &repo,
                &wt
            ),
            Err(CopyError::Symlink { .. })
        ));
        std::fs::write(repo.join("notes.txt"), "n").unwrap();
        assert_eq!(
            apply(
                &config(vec![rule("notes.txt", "notes.txt", None)]),
                &repo,
                &wt
            ),
            Err(CopyError::SourceNotIgnored("notes.txt".to_string()))
        );
        assert_eq!(
            apply(
                &config(vec![rule("README", "local/readme", None)]),
                &repo,
                &wt
            ),
            Err(CopyError::SourceTracked("README".to_string()))
        );
        assert!(!wt.join("local").exists());
    }

    #[test]
    fn if_exists_policies() {
        let (_tmp, repo, wt) = setup();
        std::fs::write(wt.join(".env.local"), "OLD\n").unwrap();
        assert_eq!(
            apply(
                &config(vec![rule(".env.local", ".env.local", None)]),
                &repo,
                &wt
            ),
            Err(CopyError::DestinationExists(".env.local".to_string()))
        );
        let skipped = apply(
            &config(vec![rule(
                ".env.local",
                ".env.local",
                Some(IfExistsPolicy::Skip),
            )]),
            &repo,
            &wt,
        )
        .unwrap();
        assert_eq!(skipped.skipped.len(), 1);
        assert_eq!(
            std::fs::read_to_string(wt.join(".env.local")).unwrap(),
            "OLD\n"
        );
        apply(
            &config(vec![rule(
                ".env.local",
                ".env.local",
                Some(IfExistsPolicy::Replace),
            )]),
            &repo,
            &wt,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(wt.join(".env.local")).unwrap(),
            "TOKEN=1\n"
        );
        assert!(!wt.join("..env.local.herdr-workflow.bak").exists());
    }

    #[test]
    fn failure_during_copy_rolls_back_recorded_changes() {
        // T23: 途中で失敗したら記録した変更だけを戻し、利用者の既存ファイルは残す。
        let (_tmp, repo, wt) = setup();
        std::fs::write(repo.join(".env.second"), "2\n").unwrap();
        std::fs::create_dir_all(wt.join("local")).unwrap();
        std::fs::write(wt.join("local/keep"), "user\n").unwrap();
        let rules = vec![
            rule(".env.local", "local/new/.env.local", None),
            rule(".env.second", "local/.env.second", None),
        ];
        let result = apply_with_hook(&config(rules), &repo, &wt, &mut |index| {
            if index == 1 {
                Err(std::io::Error::other("disk full"))
            } else {
                Ok(())
            }
        });
        assert!(
            matches!(&result, Err(CopyError::Io { rolled_back: 2, unrecovered, .. }) if unrecovered.is_empty()),
            "{result:?}"
        );
        assert!(!wt.join("local/new").exists());
        assert!(!wt.join("local/.env.second").exists());
        assert_eq!(
            std::fs::read_to_string(wt.join("local/keep")).unwrap(),
            "user\n"
        );
    }

    #[test]
    fn same_source_and_checkout_is_skipped() {
        let (_tmp, repo, _wt) = setup();
        let report = apply(
            &config(vec![rule(".env.local", ".env.local", None)]),
            &repo,
            &repo,
        )
        .unwrap();
        assert!(report.copied.is_empty());
        assert_eq!(report.skipped.len(), 1);
    }
}
