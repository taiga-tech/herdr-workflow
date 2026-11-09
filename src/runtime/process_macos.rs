//! macOSのプロセス情報(docs/specs/persistence.md#process-identity)。
//!
//! 起動時刻と親PID・状態は`proc_pidinfo(PROC_PIDTBSDINFO)`、ブートの識別には
//! `sysctl kern.boottime`、プロセス一覧には`proc_listallpids`を使う。`std`と`libc`
//! 以外に依存しない(このファイル単体でdarwin向けにコンパイル検査できるようにする)。

use std::io;

fn bsd_info(pid: u32) -> io::Result<libc::proc_bsdinfo> {
    let pid = libc::c_int::try_from(pid).map_err(io::Error::other)?;
    // SAFETY: infoは出力専用のゼロ初期化領域で、渡す大きさはその型の大きさ。
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>())
        .map_err(io::Error::other)?;
    let written =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    if written == size {
        Ok(info)
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Err(io::Error::from(io::ErrorKind::NotFound))
        } else {
            Err(error)
        }
    }
}

/// 起動時刻(エポックからのマイクロ秒)。プロセスがなければ`Err(NotFound)`。
pub(super) fn start_time(pid: u32) -> io::Result<Option<u64>> {
    let info = bsd_info(pid)?;
    Ok(info
        .pbi_start_tvsec
        .checked_mul(1_000_000)
        .and_then(|micros| micros.checked_add(info.pbi_start_tvusec)))
}

/// 親PID。
pub(super) fn parent_pid(pid: u32) -> Option<u32> {
    bsd_info(pid).ok().map(|info| info.pbi_ppid)
}

/// 終了済みで回収待ち(`SZOMB`)か。
pub(super) fn is_zombie(pid: u32) -> bool {
    const SZOMB: u32 = 5;
    bsd_info(pid).is_ok_and(|info| info.pbi_status == SZOMB)
}

/// `kern.boottime`(秒.マイクロ秒)。再起動すると変わる。
pub(super) fn boot_id() -> Option<String> {
    // SAFETY: tvは出力専用の領域で、lenはその大きさ。名前はNUL終端の定数。
    let mut tv: libc::timeval = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::timeval>();
    let result = unsafe {
        libc::sysctlbyname(
            c"kern.boottime".as_ptr(),
            (&raw mut tv).cast(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (result == 0 && len == std::mem::size_of::<libc::timeval>())
        .then(|| format!("{}.{:06}", tv.tv_sec, tv.tv_usec))
}

/// 稼働中の全プロセスのPID。
pub(super) fn all_pids() -> Vec<u32> {
    // SAFETY: 大きさを問い合わせるだけで、バッファは渡さない。
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let Ok(count) = usize::try_from(count) else {
        return Vec::new();
    };
    // 問い合わせ後に増えた分の余裕を持たせる。
    let mut pids: Vec<libc::c_int> = vec![0; count + 64];
    let Ok(bytes) = libc::c_int::try_from(pids.len() * std::mem::size_of::<libc::c_int>()) else {
        return Vec::new();
    };
    // SAFETY: pidsは書き込み可能な領域で、bytesはその大きさ。
    let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    let Ok(filled) = usize::try_from(filled) else {
        return Vec::new();
    };
    pids.truncate(filled.min(pids.len()));
    pids.into_iter()
        .filter_map(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid != 0)
        .collect()
}
