//! Attempt単位のログ(docs/specs/observability.md「ログ形式」)。
//!
//! 1行を1レコードとし、`runId`・`taskId`・`attemptId`・時刻・stream・連番・データを
//! JSON Linesで保存する。秘密値は保存前に置き換える。上限を超えたら世代を回し、
//! 子プロセスの出力は読み続ける。

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

pub const DEFAULT_MAX_BYTES: u64 = 20 * 1024 * 1024;
pub const DEFAULT_GENERATIONS: u32 = 5;
/// 設定できる世代数の上限。読み取り時はこの世代まで探す。
pub const MAX_GENERATIONS: u32 = 20;
const MASK: &str = "***";
/// 1行の上限。超えた分は次のレコードへ分割する。
const MAX_LINE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stream {
    Stdout,
    Stderr,
}

impl Stream {
    #[must_use]
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Stdout => "stdout.log",
            Self::Stderr => "stderr.log",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRecord {
    pub run_id: String,
    pub task_id: String,
    pub attempt_id: u32,
    pub time: String,
    pub stream: Stream,
    pub seq: u64,
    pub data: String,
}

/// UTCのRFC3339(ミリ秒)。外部crateを使わずに暦日へ変換する。
#[must_use]
pub fn rfc3339(time: SystemTime) -> String {
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnantの`civil_from_days`。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// 表示・照合用に制御文字を除く(タブは空白へ)。
#[must_use]
pub fn display_text(data: &str) -> String {
    let mut out = String::with_capacity(data.len());
    let mut chars = data.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI等の端末制御シーケンスを読み飛ばす。
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        if c == '\t' {
            out.push(' ');
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// 秘密値を置き換える。長い値から順に置換し、部分一致による取りこぼしを減らす。
#[derive(Debug, Clone, Default)]
pub struct Masker {
    values: Vec<String>,
}

impl Masker {
    #[must_use]
    pub fn new(mut values: Vec<String>) -> Self {
        values.retain(|value| !value.is_empty());
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        values.dedup();
        Self { values }
    }

    #[must_use]
    pub fn mask(&self, data: &str) -> String {
        let mut out = data.to_string();
        for value in &self.values {
            if out.contains(value.as_str()) {
                out = out.replace(value.as_str(), MASK);
            }
        }
        out
    }
}

/// 1つのstreamのログファイル。サイズ上限で世代を回す。
pub struct LogWriter {
    path: PathBuf,
    file: File,
    written: u64,
    max_bytes: u64,
    generations: u32,
    degraded: bool,
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

impl LogWriter {
    pub fn create(path: PathBuf, max_bytes: u64, generations: u32) -> io::Result<Self> {
        let file = open_append(&path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            path,
            file,
            written,
            max_bytes: max_bytes.max(1),
            generations: generations.max(1),
            degraded: false,
        })
    }

    /// 書き込みに一度でも失敗したか(`log-degraded`)。
    #[must_use]
    pub fn degraded(&self) -> bool {
        self.degraded
    }

    fn rotate(&mut self) -> io::Result<()> {
        for generation in (1..self.generations).rev() {
            let from = rotated(&self.path, generation - 1);
            let to = rotated(&self.path, generation);
            if from.exists() {
                std::fs::rename(&from, &to)?;
            }
        }
        self.file = open_append(&self.path)?;
        self.written = 0;
        Ok(())
    }

    pub fn write_record(&mut self, record: &LogRecord) {
        let Ok(mut line) = serde_json::to_vec(record) else {
            self.degraded = true;
            return;
        };
        line.push(b'\n');
        let len = u64::try_from(line.len()).unwrap_or(u64::MAX);
        if self.written > 0 && self.written.saturating_add(len) > self.max_bytes {
            if self.generations == 1 {
                // 世代を持たない設定では切り詰めて書き直す。
                if self.file.set_len(0).is_err() {
                    self.degraded = true;
                }
                self.written = 0;
            } else if self.rotate().is_err() {
                self.degraded = true;
            }
        }
        if self.file.write_all(&line).is_err() {
            self.degraded = true;
        } else {
            self.written = self.written.saturating_add(len);
        }
    }
}

/// 世代0は元のパス、1以降は`.1`等を付ける。
#[must_use]
pub fn rotated(path: &Path, generation: u32) -> PathBuf {
    if generation == 0 {
        path.to_path_buf()
    } else {
        let mut name = path.as_os_str().to_os_string();
        name.push(format!(".{generation}"));
        PathBuf::from(name)
    }
}

/// 子プロセスの出力を行単位で読み、保存し、表示用の行を`on_line`へ渡す。
/// 戻り値は読み取った行数。上限超過の行は分割する。
pub fn pump<R: Read>(
    reader: R,
    mut writer: LogWriter,
    template: &LogRecord,
    masker: &Masker,
    mut on_line: impl FnMut(&str),
) -> (u64, bool) {
    let mut reader = BufReader::new(reader);
    let mut seq = 0;
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        let read = (&mut reader)
            .take(u64::try_from(MAX_LINE_BYTES).unwrap_or(u64::MAX))
            .read_until(b'\n', &mut buffer);
        match read {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buffer.last() == Some(&b'\n') {
            buffer.pop();
            if buffer.last() == Some(&b'\r') {
                buffer.pop();
            }
        }
        let text = String::from_utf8_lossy(&buffer);
        let masked = masker.mask(&text);
        seq += 1;
        let record = LogRecord {
            time: rfc3339(SystemTime::now()),
            seq,
            data: masked.clone(),
            ..template.clone()
        };
        writer.write_record(&record);
        on_line(&display_text(&masked));
    }
    (seq, writer.degraded())
}

/// 保存したログを古い世代から順に読む。
pub fn read_records(path: &Path, generations: u32) -> io::Result<Vec<LogRecord>> {
    let mut records = Vec::new();
    for generation in (0..generations.max(1)).rev() {
        let file_path = rotated(path, generation);
        let file = match File::open(&file_path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for line in BufReader::new(file).lines() {
            let line = line?;
            if let Ok(record) = serde_json::from_str::<LogRecord>(&line) {
                records.push(record);
            }
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::state_dir::test_support::TempDir;

    fn template(stream: Stream) -> LogRecord {
        LogRecord {
            run_id: "r1".to_string(),
            task_id: "t".to_string(),
            attempt_id: 1,
            time: String::new(),
            stream,
            seq: 0,
            data: String::new(),
        }
    }

    #[test]
    fn rfc3339_formats_known_instants() {
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(951_782_400_123);
        assert_eq!(rfc3339(t), "2000-02-29T00:00:00.123Z");
        assert_eq!(rfc3339(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn display_text_strips_control_sequences() {
        assert_eq!(display_text("\u{1b}[31mred\u{1b}[0m\tok\u{7}"), "red ok");
    }

    #[test]
    fn masker_replaces_values() {
        let masker = Masker::new(vec!["abc".to_string(), "abcdef".to_string(), String::new()]);
        assert_eq!(masker.mask("x abcdef y abc"), "x *** y ***");
    }

    #[test]
    fn pump_writes_masked_records_and_reports_lines() {
        let tmp = TempDir::new("logs");
        let path = tmp.0.join("stdout.log");
        let writer = LogWriter::create(path.clone(), DEFAULT_MAX_BYTES, 5).unwrap();
        let input = b"hello secret\r\nbad \xff byte\nlast".to_vec();
        let mut lines = Vec::new();
        let (count, degraded) = pump(
            input.as_slice(),
            writer,
            &template(Stream::Stdout),
            &Masker::new(vec!["secret".to_string()]),
            |line| lines.push(line.to_string()),
        );
        assert_eq!(count, 3);
        assert!(!degraded);
        assert_eq!(lines, ["hello ***", "bad \u{fffd} byte", "last"]);
        let records = read_records(&path, 5).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].seq, 1);
        assert_eq!(records[0].data, "hello ***");
        assert!(!std::fs::read_to_string(&path).unwrap().contains("secret"));
    }

    #[test]
    fn writer_rotates_generations() {
        let tmp = TempDir::new("logs");
        let path = tmp.0.join("stdout.log");
        let mut writer = LogWriter::create(path.clone(), 200, 3).unwrap();
        for seq in 1..=20 {
            writer.write_record(&LogRecord {
                seq,
                data: "x".repeat(50),
                ..template(Stream::Stdout)
            });
        }
        assert!(rotated(&path, 1).exists());
        assert!(rotated(&path, 2).exists());
        assert!(!rotated(&path, 3).exists());
        let records = read_records(&path, 3).unwrap();
        assert_eq!(records.last().unwrap().seq, 20);
        // 古い世代は破棄され、残った連番は昇順に並ぶ。
        assert!(records.windows(2).all(|w| w[0].seq < w[1].seq));
    }
}
