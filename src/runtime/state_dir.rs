//! 状態ディレクトリの決定と権限確認(docs/specs/persistence.md#state-directory)。
//!
//! Herdrの`HERDR_PLUGIN_STATE_DIR`は使わない(ADR-0007)。既存ディレクトリの権限は
//! 変更せず、条件を満たさなければエラーにする。

use std::ffi::OsString;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

pub const STATE_DIR_ENV: &str = "HERDR_WORKFLOW_STATE_DIR";
const APP_DIR: &str = "herdr-workflow";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StateDirError {
    #[error(
        "state directory is not determined: set HERDR_WORKFLOW_STATE_DIR, XDG_STATE_HOME or HOME to an absolute path"
    )]
    Undetermined,
    #[error("{var} must be an absolute path, got {value:?}")]
    Relative { var: &'static str, value: String },
    #[error("state path {path} is not a directory")]
    NotDirectory { path: PathBuf },
    #[error(
        "state directory {path} is owned by uid {owner}, not the current user (uid {current}); permissions were not changed"
    )]
    NotOwned {
        path: PathBuf,
        owner: u32,
        current: u32,
    },
    #[error(
        "state directory {path} is writable by group or others (mode {mode:o}); permissions were not changed"
    )]
    InsecurePermissions { path: PathBuf, mode: u32 },
    #[error("failed to prepare state directory {path}: {message}")]
    Io { path: PathBuf, message: String },
}

/// 環境変数の読み取りを注入できる形で状態ディレクトリを決める。
pub fn resolve_with<F>(lookup: F) -> Result<PathBuf, StateDirError>
where
    F: Fn(&str) -> Option<OsString>,
{
    let get = |name: &str| lookup(name).filter(|value| !value.is_empty());
    let absolute = |var: &'static str, value: OsString| {
        let path = PathBuf::from(&value);
        if path.is_absolute() {
            Ok(path)
        } else {
            Err(StateDirError::Relative {
                var,
                value: value.to_string_lossy().into_owned(),
            })
        }
    };
    if let Some(value) = get(STATE_DIR_ENV) {
        return absolute(STATE_DIR_ENV, value);
    }
    if let Some(value) = get("XDG_STATE_HOME") {
        return Ok(absolute("XDG_STATE_HOME", value)?.join(APP_DIR));
    }
    if let Some(value) = get("HOME") {
        return Ok(absolute("HOME", value)?
            .join(".local")
            .join("state")
            .join(APP_DIR));
    }
    Err(StateDirError::Undetermined)
}

pub fn resolve() -> Result<PathBuf, StateDirError> {
    resolve_with(|name| std::env::var_os(name))
}

pub(crate) fn current_euid() -> u32 {
    // SAFETY: geteuid は引数を取らず、常に成功し、メモリへ触れない。
    unsafe { libc::geteuid() }
}

fn io_error(path: &Path, error: &std::io::Error) -> StateDirError {
    StateDirError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

/// 既存ディレクトリの所有者と書き込み権限を確認する。
pub fn check_private_dir(path: &Path) -> Result<(), StateDirError> {
    let metadata = std::fs::metadata(path).map_err(|error| io_error(path, &error))?;
    if !metadata.is_dir() {
        return Err(StateDirError::NotDirectory {
            path: path.to_path_buf(),
        });
    }
    let current = current_euid();
    if metadata.uid() != current {
        return Err(StateDirError::NotOwned {
            path: path.to_path_buf(),
            owner: metadata.uid(),
            current,
        });
    }
    if metadata.mode() & 0o022 != 0 {
        return Err(StateDirError::InsecurePermissions {
            path: path.to_path_buf(),
            mode: metadata.mode() & 0o7777,
        });
    }
    Ok(())
}

/// ディレクトリを`0700`で作成し(既存なら権限を確認し)、パスを返す。
pub fn ensure_private_dir(path: &Path) -> Result<(), StateDirError> {
    match std::fs::DirBuilder::new()
        .recursive(false)
        .mode(0o700)
        .create(path)
    {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // 親ディレクトリ(`~/.local/state`等)は利用者の領域なので0700で作るだけにする。
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .map_err(|error| io_error(path, &error))?;
        }
        Err(error) => return Err(io_error(path, &error)),
    }
    check_private_dir(path)
}

/// 状態ディレクトリと、その配下のパス規則。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDir {
    root: PathBuf,
}

impl StateDir {
    /// 状態ディレクトリを作成・確認して開く。
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StateDirError> {
        let root = root.into();
        ensure_private_dir(&root)?;
        Ok(Self { root })
    }

    pub fn open_default() -> Result<Self, StateDirError> {
        Self::open(resolve()?)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn db_path(&self) -> PathBuf {
        self.root.join("state.db")
    }

    #[must_use]
    pub fn socket_path(&self) -> PathBuf {
        self.root.join("coordinator.sock")
    }

    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.root.join("coordinator.lock")
    }

    #[must_use]
    pub fn resource_locks_dir(&self) -> PathBuf {
        self.root.join("locks")
    }

    #[must_use]
    pub fn run_dir(&self, run_id: &str) -> PathBuf {
        self.root.join("runs").join(run_id)
    }

    #[must_use]
    pub fn attempt_dir(&self, run_id: &str, task_id: &str, attempt: u32) -> PathBuf {
        self.run_dir(run_id)
            .join("attempts")
            .join(format!("{task_id}-{attempt}"))
    }

    /// 配下のディレクトリを`0700`で作成する。
    pub fn ensure_subdir(&self, path: &Path) -> Result<(), StateDirError> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(|error| io_error(path, &error))
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 試験ごとに別の一時ディレクトリを作り、終了時に削除する。
    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new(prefix: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "hw-{prefix}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::TempDir;
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn resolution_order_follows_the_spec() {
        assert_eq!(
            resolve_with(env(&[
                ("HERDR_WORKFLOW_STATE_DIR", "/s"),
                ("XDG_STATE_HOME", "/x"),
                ("HOME", "/h")
            ])),
            Ok(PathBuf::from("/s"))
        );
        assert_eq!(
            resolve_with(env(&[
                ("HERDR_WORKFLOW_STATE_DIR", ""),
                ("XDG_STATE_HOME", "/x"),
                ("HOME", "/h")
            ])),
            Ok(PathBuf::from("/x/herdr-workflow"))
        );
        assert_eq!(
            resolve_with(env(&[("HOME", "/h")])),
            Ok(PathBuf::from("/h/.local/state/herdr-workflow"))
        );
        assert_eq!(resolve_with(env(&[])), Err(StateDirError::Undetermined));
    }

    #[test]
    fn relative_paths_are_rejected() {
        assert!(matches!(
            resolve_with(env(&[("XDG_STATE_HOME", "state")])),
            Err(StateDirError::Relative { .. })
        ));
        assert!(matches!(
            resolve_with(env(&[("HERDR_WORKFLOW_STATE_DIR", "./s")])),
            Err(StateDirError::Relative { .. })
        ));
    }

    #[test]
    fn new_directory_is_private() {
        let tmp = TempDir::new("statedir");
        let root = tmp.0.join("a/b/herdr-workflow");
        let dir = StateDir::open(&root).unwrap();
        let mode = std::fs::metadata(dir.root()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn group_writable_directory_is_rejected_without_change() {
        let tmp = TempDir::new("statedir");
        let root = tmp.0.join("shared");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o770)).unwrap();
        assert!(matches!(
            StateDir::open(&root),
            Err(StateDirError::InsecurePermissions { .. })
        ));
        let mode = std::fs::metadata(&root).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o770);
    }
}
