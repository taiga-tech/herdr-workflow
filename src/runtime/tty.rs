//! 対話コマンド(`runner: pty`)の擬似端末と接続(ADR-0008、Q04)。
//!
//! TaskRunnerが擬似端末(PTY)を確保して子プロセスの制御端末にし、Unix domain socketで
//! 端末クライアント(`herdr-workflow term`)の接続を受ける。クライアントは自分の端末を
//! rawモードにしてキー入力をそのまま送るため、Ctrl+Cは子プロセスの前景プロセスグループへ
//! 端末の規則どおりに届く。端末サイズの変更はPTYへ反映し、子プロセスにはSIGWINCHが届く。
//! paneやクライアントを閉じても子プロセスは止めない。
//!
//! クライアントから送る枠: 種別1バイト、長さ4バイト(big endian)、本体。
//! 種別0はキー入力、種別1は端末サイズ(行・列をそれぞれ2バイト)。
//! TaskRunnerからは出力のバイト列をそのまま送る。

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 接続時に送り直す直近の出力の大きさ。
pub const SCROLLBACK_BYTES: usize = 64 * 1024;
/// 接続を切り離すキー(Ctrl+])。Ctrl+Cは子プロセスへ送るため使わない。
pub const DETACH_KEY: u8 = 0x1d;
const FRAME_DATA: u8 = 0;
const FRAME_RESIZE: u8 = 1;
const MAX_FRAME: usize = 64 * 1024;
/// Unix domain socketのパス長の上限(`sun_path`より短く保つ)。
pub const MAX_SOCKET_PATH: usize = 100;

/// 確保した擬似端末。`master`はTaskRunnerが持ち、`slave`は子プロセスへ渡す。
pub struct Pty {
    pub master: OwnedFd,
    pub slave: OwnedFd,
}

fn cvt(result: libc::c_int) -> io::Result<libc::c_int> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

/// 擬似端末を確保する。初期サイズは24行80列。
pub fn open_pty() -> io::Result<Pty> {
    // SAFETY: 戻り値を検査し、得たfdはOwnedFdへ移して閉じ忘れを防ぐ。
    unsafe {
        let master = cvt(libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY))?;
        let master = OwnedFd::from_raw_fd(master);
        cvt(libc::grantpt(master.as_raw_fd()))?;
        cvt(libc::unlockpt(master.as_raw_fd()))?;
        let name = slave_name(master.as_raw_fd())?;
        let slave = cvt(libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY))?;
        let slave = OwnedFd::from_raw_fd(slave);
        let pty = Pty { master, slave };
        set_size(&pty.master, 24, 80)?;
        Ok(pty)
    }
}

#[cfg(target_os = "linux")]
fn slave_name(master: libc::c_int) -> io::Result<std::ffi::CString> {
    let mut buffer = vec![0 as libc::c_char; 128];
    // SAFETY: バッファの長さを渡し、NUL終端された名前を受け取る。
    let result = unsafe { libc::ptsname_r(master, buffer.as_mut_ptr(), buffer.len()) };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result));
    }
    // SAFETY: ptsname_rが成功したので、バッファはNUL終端されている。
    Ok(unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }.to_owned())
}

#[cfg(not(target_os = "linux"))]
fn slave_name(master: libc::c_int) -> io::Result<std::ffi::CString> {
    // ptsnameは静的領域を返すため、呼び出しを直列にしてすぐ複製する。
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // SAFETY: 戻り値がNULLでなければNUL終端の文字列。複製してから錠を外す。
    let name = unsafe { libc::ptsname(master) };
    if name.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { std::ffi::CStr::from_ptr(name) }.to_owned())
}

/// 端末サイズを設定する(子プロセスの前景プロセスグループにSIGWINCHが届く)。
pub fn set_size(fd: &impl AsRawFd, rows: u16, cols: u16) -> io::Result<()> {
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: winsizeを読むだけのioctl。
    cvt(unsafe { libc::ioctl(fd.as_raw_fd(), libc::TIOCSWINSZ, &raw const size) })?;
    Ok(())
}

/// 端末サイズを読む。端末でなければ`None`。
#[must_use]
pub fn get_size(fd: &impl AsRawFd) -> Option<(u16, u16)> {
    // SAFETY: 出力専用のゼロ初期化領域へ書き込むioctl。
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::ioctl(fd.as_raw_fd(), libc::TIOCGWINSZ, &raw mut size) };
    (result == 0 && size.ws_row > 0 && size.ws_col > 0).then_some((size.ws_row, size.ws_col))
}

/// 子プロセスの起動直前(fork後、exec前)に呼ぶ。新しいセッションを作り、
/// 標準入出力に渡した擬似端末を制御端末にする。
///
/// # Safety
/// `pre_exec`から呼ぶこと。async-signal-safeな関数だけを使う。
pub unsafe fn make_controlling_terminal() -> io::Result<()> {
    // SAFETY: fork後の子プロセスでsetsidとioctlだけを呼ぶ。
    unsafe {
        cvt(libc::setsid())?;
        cvt(libc::ioctl(0, libc::TIOCSCTTY as _, 0))?;
    }
    Ok(())
}

/// 枠を一つ書く。
pub fn write_frame(out: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len()).map_err(io::Error::other)?;
    let mut frame = Vec::with_capacity(payload.len() + 5);
    frame.push(kind);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(payload);
    out.write_all(&frame)
}

/// 受け取った枠。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Data(Vec<u8>),
    Resize { rows: u16, cols: u16 },
}

/// 枠を一つ読む。接続が閉じたら`Ok(None)`。
pub fn read_frame(input: &mut impl Read) -> io::Result<Option<Frame>> {
    let mut header = [0u8; 5];
    match input.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut payload = vec![0; len];
    input.read_exact(&mut payload)?;
    match header[0] {
        FRAME_DATA => Ok(Some(Frame::Data(payload))),
        FRAME_RESIZE if payload.len() == 4 => Ok(Some(Frame::Resize {
            rows: u16::from_be_bytes([payload[0], payload[1]]),
            cols: u16::from_be_bytes([payload[2], payload[3]]),
        })),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "unknown frame")),
    }
}

pub fn send_input(out: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    write_frame(out, FRAME_DATA, bytes)
}

pub fn send_resize(out: &mut impl Write, rows: u16, cols: u16) -> io::Result<()> {
    let mut payload = rows.to_be_bytes().to_vec();
    payload.extend_from_slice(&cols.to_be_bytes());
    write_frame(out, FRAME_RESIZE, &payload)
}

/// 接続中のクライアントと直近の出力。
#[derive(Default)]
struct Clients {
    streams: Vec<UnixStream>,
    scrollback: VecDeque<u8>,
}

/// TaskRunner側の端末共有。出力を接続中のクライアントへ配り、入力をPTYへ書く。
#[derive(Clone)]
pub struct Hub {
    clients: Arc<Mutex<Clients>>,
    master: Arc<File>,
}

impl Hub {
    #[must_use]
    pub fn new(master: File) -> Self {
        Self {
            clients: Arc::new(Mutex::new(Clients::default())),
            master: Arc::new(master),
        }
    }

    /// 出力を記録して配る。書けないクライアントは外す。
    pub fn broadcast(&self, bytes: &[u8]) {
        let mut clients = self
            .clients
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clients.scrollback.extend(bytes);
        let excess = clients.scrollback.len().saturating_sub(SCROLLBACK_BYTES);
        clients.scrollback.drain(..excess);
        clients
            .streams
            .retain_mut(|stream| stream.write_all(bytes).is_ok());
    }

    /// 接続を受け付ける。直近の出力を送ってから配信の対象に加える。
    pub fn serve(&self, listener: UnixListener) {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            if !peer_is_same_user(&stream) {
                continue;
            }
            let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
            let Ok(reader) = stream.try_clone() else {
                continue;
            };
            {
                let mut clients = self
                    .clients
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let (front, back) = clients.scrollback.as_slices();
                if stream
                    .write_all(front)
                    .and_then(|()| stream.write_all(back))
                    .is_err()
                {
                    continue;
                }
                clients.streams.push(stream);
            }
            let master = Arc::clone(&self.master);
            std::thread::spawn(move || relay_input(reader, &master));
        }
    }

    /// 子プロセスの終了後、クライアントとの接続を閉じる。
    pub fn close(&self) {
        let mut clients = self
            .clients
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for stream in clients.streams.drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

fn relay_input(mut reader: UnixStream, master: &File) {
    while let Ok(Some(frame)) = read_frame(&mut reader) {
        match frame {
            Frame::Data(bytes) => {
                if (&*master).write_all(&bytes).is_err() {
                    break;
                }
            }
            Frame::Resize { rows, cols } => {
                let _ = set_size(master, rows, cols);
            }
        }
    }
}

/// 接続元が同じ利用者か(Coordinatorの制御ソケットと同じ確認)。
fn peer_is_same_user(stream: &UnixStream) -> bool {
    crate::coordinator::peer_uid(stream)
        .is_some_and(|uid| uid == crate::runtime::state_dir::current_euid())
}

/// Attemptの接続先。`attempt_dir/tty.sock`が長すぎれば状態ディレクトリ直下の短い名前を使う。
#[must_use]
pub fn socket_path(state_root: &Path, attempt_dir: &Path) -> Option<PathBuf> {
    let preferred = attempt_dir.join("tty.sock");
    if preferred.as_os_str().len() <= MAX_SOCKET_PATH {
        return Some(preferred);
    }
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(attempt_dir.as_os_str().as_encoded_bytes());
    let name: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let short = state_root.join("tty").join(format!("{name}.sock"));
    (short.as_os_str().len() <= MAX_SOCKET_PATH).then_some(short)
}

/// 端末のrawモード。破棄時に元の設定へ戻す。
pub struct RawMode {
    fd: libc::c_int,
    original: libc::termios,
}

impl RawMode {
    /// 標準入力が端末ならrawモードにする。端末でなければ`None`。
    #[must_use]
    pub fn enable(fd: libc::c_int) -> Option<Self> {
        // SAFETY: termiosは出力専用のゼロ初期化領域。戻り値を検査する。
        unsafe {
            let mut original: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &raw mut original) != 0 {
                return None;
            }
            let mut raw = original;
            libc::cfmakeraw(&raw mut raw);
            if libc::tcsetattr(fd, libc::TCSANOW, &raw const raw) != 0 {
                return None;
            }
            Some(Self { fd, original })
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // SAFETY: 保存した設定へ戻すだけ。
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &raw const self.original);
        }
    }
}

/// クライアントの終わり方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detach {
    /// TaskRunnerが接続を閉じた(子プロセスの終了)。
    Closed,
    /// 利用者が切り離した。子プロセスは動き続ける。
    User,
}

static RESIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_winch(_: libc::c_int) {
    RESIZED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// 端末クライアント。標準入力をrawモードにして接続先と中継する。
pub fn attach(socket: &Path) -> io::Result<Detach> {
    let stream = UnixStream::connect(socket)?;
    let mut writer = stream.try_clone()?;
    let stdin_fd = libc::STDIN_FILENO;
    let raw = RawMode::enable(stdin_fd);
    if let Some((rows, cols)) = get_size(&io::stdin()) {
        send_resize(&mut writer, rows, cols)?;
    }
    // SAFETY: フラグを立てるだけの信号処理を登録する。
    unsafe {
        let handler = on_winch as extern "C" fn(libc::c_int);
        libc::signal(libc::SIGWINCH, handler as libc::sighandler_t);
    }
    let detached = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let detached = Arc::clone(&detached);
        let stream = stream.try_clone()?;
        std::thread::spawn(move || {
            let mut stdin = io::stdin().lock();
            let mut buffer = [0u8; 4096];
            loop {
                let read = match stdin.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                if RESIZED.swap(false, std::sync::atomic::Ordering::Relaxed)
                    && let Some((rows, cols)) = get_size(&io::stdin())
                    && send_resize(&mut writer, rows, cols).is_err()
                {
                    break;
                }
                let bytes = &buffer[..read];
                if let Some(position) = bytes.iter().position(|byte| *byte == DETACH_KEY) {
                    let _ = send_input(&mut writer, &bytes[..position]);
                    detached.store(true, std::sync::atomic::Ordering::Relaxed);
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    break;
                }
                if send_input(&mut writer, bytes).is_err() {
                    break;
                }
            }
        });
    }
    // 端末サイズの変化は入力がなくても送る。
    {
        let mut writer = stream.try_clone()?;
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(100));
                if RESIZED.swap(false, std::sync::atomic::Ordering::Relaxed)
                    && let Some((rows, cols)) = get_size(&io::stdin())
                    && send_resize(&mut writer, rows, cols).is_err()
                {
                    break;
                }
            }
        });
    }
    let mut reader = stream;
    let mut stdout = io::stdout().lock();
    let mut buffer = [0u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if stdout
                    .write_all(&buffer[..read])
                    .and_then(|()| stdout.flush())
                    .is_err()
                {
                    break;
                }
            }
        }
    }
    drop(raw);
    Ok(if detached.load(std::sync::atomic::Ordering::Relaxed) {
        Detach::User
    } else {
        Detach::Closed
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buffer = Vec::new();
        send_input(&mut buffer, b"hi\n").unwrap();
        send_resize(&mut buffer, 40, 100).unwrap();
        let mut reader = io::Cursor::new(buffer);
        assert_eq!(
            read_frame(&mut reader).unwrap(),
            Some(Frame::Data(b"hi\n".to_vec()))
        );
        assert_eq!(
            read_frame(&mut reader).unwrap(),
            Some(Frame::Resize {
                rows: 40,
                cols: 100
            })
        );
        assert_eq!(read_frame(&mut reader).unwrap(), None);
        let mut bad = io::Cursor::new(vec![9, 0, 0, 0, 0]);
        assert!(read_frame(&mut bad).is_err());
    }

    #[test]
    fn pty_size_is_visible_on_the_slave() {
        let pty = open_pty().unwrap();
        assert_eq!(get_size(&pty.slave), Some((24, 80)));
        set_size(&pty.master, 40, 100).unwrap();
        assert_eq!(get_size(&pty.slave), Some((40, 100)));
    }

    #[test]
    fn long_attempt_dirs_use_a_short_socket_name() {
        let root = Path::new("/tmp/s");
        assert_eq!(
            socket_path(root, Path::new("/tmp/s/runs/r/attempts/t-1")),
            Some(PathBuf::from("/tmp/s/runs/r/attempts/t-1/tty.sock"))
        );
        let long = root.join("x".repeat(120));
        let short = socket_path(root, &long).unwrap();
        assert!(short.starts_with("/tmp/s/tty"));
        assert!(socket_path(&PathBuf::from("/".to_string() + &"y".repeat(120)), &long).is_none());
    }
}
