//! resource lockとcheckout実行ロック(docs/specs/execution.md#resource-lock)。
//!
//! 状態ディレクトリの`locks/<名前>.lock`をOSのファイルロック(flock)で排他する。
//! 同じ状態ディレクトリを使う全Runで共有され、プロセスが終了すればOSが解放する。
//! 列挙したロックはすべて同時に取得できる場合だけ取得し、一部だけを保持しない。

use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// 保持中のロック。破棄すると解放する。
#[derive(Debug)]
pub struct LockGuard {
    name: String,
    _file: File,
}

impl LockGuard {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

fn try_lock(dir: &Path, name: &str) -> std::io::Result<Option<LockGuard>> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(dir.join(format!("{name}.lock")))?;
    // SAFETY: 有効なファイル記述子に対する非ブロッキングの排他ロック。
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        Ok(Some(LockGuard {
            name: name.to_string(),
            _file: file,
        }))
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Ok(None)
        } else {
            Err(error)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum LockAttempt {
    Acquired,
    /// 保持されているロック名。取得済みの分は解放した。
    Busy(String),
}

/// すべて取得できた場合だけ`guards`へ追加する。
pub fn try_lock_all(
    dir: &Path,
    names: &[String],
    guards: &mut Vec<LockGuard>,
) -> std::io::Result<LockAttempt> {
    let mut acquired = Vec::with_capacity(names.len());
    for name in names {
        match try_lock(dir, name)? {
            Some(guard) => acquired.push(guard),
            None => return Ok(LockAttempt::Busy(name.clone())),
        }
    }
    guards.extend(acquired);
    Ok(LockAttempt::Acquired)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::state_dir::test_support::TempDir;

    #[test]
    fn locks_are_exclusive_and_all_or_nothing() {
        let tmp = TempDir::new("locks");
        let mut first = Vec::new();
        assert_eq!(
            try_lock_all(&tmp.0, &["db".to_string()], &mut first).unwrap(),
            LockAttempt::Acquired
        );
        let mut second = Vec::new();
        assert_eq!(
            try_lock_all(
                &tmp.0,
                &["cache".to_string(), "db".to_string()],
                &mut second
            )
            .unwrap(),
            LockAttempt::Busy("db".to_string())
        );
        assert!(second.is_empty());
        // 一部だけを保持しないので、cacheは別の要求者が取得できる。
        let third = acquire_eventually(&tmp.0, "cache");
        assert_eq!(third[0].name(), "cache");
        drop(first);
        let fourth = acquire_eventually(&tmp.0, "db");
        assert_eq!(fourth[0].name(), "db");
    }

    /// 解放直後は、並行する試験がforkした子プロセスがexecまでの間だけ記述子を
    /// 引き継いでいることがある。エンジンと同じく、少し待って取り直す。
    fn acquire_eventually(dir: &Path, name: &str) -> Vec<LockGuard> {
        for _ in 0..100 {
            let mut guards = Vec::new();
            if try_lock_all(dir, &[name.to_string()], &mut guards).unwrap() == LockAttempt::Acquired
            {
                return guards;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("lock {name} was not released");
    }
}
