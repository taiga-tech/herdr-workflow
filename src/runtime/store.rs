//! SQLiteの状態ストア(docs/specs/persistence.md)。
//!
//! 書き込みはRunを実行するCoordinatorの経路だけが行う。秘密値と環境変数は保存しない。
//! スキーマ版は`PRAGMA user_version`で管理し、未知の新しい版は読まずにエラーにする。

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::runtime::engine::{Recorder, RunOutcome, RunPhase, RunSnapshot};
use crate::runtime::logs;

pub const SCHEMA_VERSION: i64 = 6;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("state database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(
        "state database {path} has schema version {found}, newer than supported version {SCHEMA_VERSION}"
    )]
    NewerSchema { path: PathBuf, found: i64 },
    #[error("failed to back up state database before migration: {0}")]
    Backup(String),
    #[error("run {0} not found")]
    RunNotFound(String),
    #[error("stored value is invalid: {0}")]
    Invalid(String),
}

/// Runを再開・表示するために保存する定義。入力値は保存するが、秘密値は名前だけ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub run_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub plan_hash: String,
    pub config_path: PathBuf,
    pub checkout: PathBuf,
    pub target: Option<String>,
    pub phase: RunPhase,
    pub outcome: RunOutcome,
    pub workflow: String,
    pub inputs: Vec<(String, String)>,
    /// Runを実行しているCoordinatorのPID(診断用)。
    pub coordinator_pid: Option<u32>,
    /// Run開始時に固定したプラグイン全体設定(JSON)。
    pub plugin_config: String,
    /// 本プラグインがHerdrへ作成を依頼したworkspace。`None`のcheckoutは借用として扱う。
    pub owned_workspace: Option<String>,
    /// Herdr連携で配置したpaneとAgentの対応(JSON、`herdr::HerdrState`)。
    pub herdr_state: Option<String>,
    /// Run開始時(再開後は再開時)のcheckoutの記録(JSON、`fingerprint::CheckoutFingerprint`)。
    pub fingerprint: Option<String>,
}

pub struct Store {
    conn: Connection,
}

/// 承認の照合結果(docs/specs/security.md#approval)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalStatus {
    Approved,
    /// 同じリポジトリと設定ハッシュの承認がない。
    Missing,
    /// 承認後にGitのhookやフィルターが変わった。
    SideEffectsChanged,
}

fn enum_text<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn enum_from<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T, StoreError> {
    serde_json::from_value(serde_json::Value::String(text.to_string()))
        .map_err(|_| StoreError::Invalid(text.to_string()))
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if !path.exists() {
            // 既定のumaskで作らせず、最初から0600で作る。
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(path)
                .map_err(|error| StoreError::Invalid(format!("{}: {error}", path.display())))?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::NewerSchema {
                path: path.to_path_buf(),
                found: version,
            });
        }
        if version > 0 && version < SCHEMA_VERSION {
            // 既存の版から更新する前にバックアップを作る。
            let backup = path.with_extension(format!("v{version}.bak"));
            std::fs::copy(path, &backup).map_err(|error| StoreError::Backup(error.to_string()))?;
        }
        if version < 1 {
            conn.execute_batch(
                "BEGIN;
                CREATE TABLE IF NOT EXISTS runs (
                    id TEXT PRIMARY KEY,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    plan_hash TEXT NOT NULL,
                    config_path TEXT NOT NULL,
                    checkout TEXT NOT NULL,
                    target TEXT,
                    phase TEXT NOT NULL,
                    outcome TEXT NOT NULL,
                    workflow TEXT NOT NULL,
                    inputs TEXT NOT NULL,
                    coordinator_pid INTEGER,
                    plugin_config TEXT NOT NULL DEFAULT ''
                );
                CREATE TABLE IF NOT EXISTS tasks (
                    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
                    task_id TEXT NOT NULL,
                    snapshot TEXT NOT NULL,
                    PRIMARY KEY (run_id, task_id)
                );
                CREATE TABLE IF NOT EXISTS events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
                    time TEXT NOT NULL,
                    message TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS events_by_run ON events(run_id, id);
                PRAGMA user_version = 1;
                COMMIT;",
            )?;
        }
        if version < 2 {
            // 版2: 承認と自動受付の重複判定(docs/specs/security.md、recovery.md)。
            conn.execute_batch(
                "BEGIN;
                CREATE TABLE IF NOT EXISTS approvals (
                    repo TEXT NOT NULL,
                    config_hash TEXT NOT NULL,
                    approved_at TEXT NOT NULL,
                    summary TEXT NOT NULL,
                    PRIMARY KEY (repo, config_hash)
                );
                CREATE TABLE IF NOT EXISTS acceptances (
                    key TEXT PRIMARY KEY,
                    run_id TEXT,
                    hold_reason TEXT,
                    received_at TEXT NOT NULL
                );
                PRAGMA user_version = 2;
                COMMIT;",
            )?;
        }
        if version < 3 {
            // 版3: Herdr経由で作成したworkspace(所有するworktree)の記録。
            conn.execute_batch(
                "BEGIN;
                ALTER TABLE runs ADD COLUMN owned_workspace TEXT;
                PRAGMA user_version = 3;
                COMMIT;",
            )?;
        }
        if version < 4 {
            // 版4: Herdr連携で作成したpaneとAgentの対応(resumeで照合する)。
            conn.execute_batch(
                "BEGIN;
                ALTER TABLE runs ADD COLUMN herdr_state TEXT;
                PRAGMA user_version = 4;
                COMMIT;",
            )?;
        }
        if version < 5 {
            // 版5: 承認時に表示したGitのhook・フィルターの要約(Q13)。版4以前の承認は
            // 要約を持たないため一致せず、再承認が必要になる。
            conn.execute_batch(
                "BEGIN;
                ALTER TABLE approvals ADD COLUMN side_effects TEXT NOT NULL DEFAULT '';
                PRAGMA user_version = 5;
                COMMIT;",
            )?;
        }
        if version < 6 {
            // 版6: 再開時に比べるcheckoutの記録(Q15、`fingerprint::CheckoutFingerprint`)。
            conn.execute_batch(
                "BEGIN;
                ALTER TABLE runs ADD COLUMN fingerprint TEXT;
                PRAGMA user_version = 6;
                COMMIT;",
            )?;
        }
        Ok(Self { conn })
    }

    pub fn insert_run(&self, record: &RunRecord) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO runs (id, created_at, updated_at, plan_hash, config_path, checkout, target,
                phase, outcome, workflow, inputs, coordinator_pid, plugin_config, owned_workspace,
                herdr_state, fingerprint)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                record.run_id,
                record.created_at,
                record.updated_at,
                record.plan_hash,
                record.config_path.to_string_lossy(),
                record.checkout.to_string_lossy(),
                record.target,
                enum_text(&record.phase),
                enum_text(&record.outcome),
                record.workflow,
                serde_json::to_string(&record.inputs).unwrap_or_default(),
                record.coordinator_pid,
                record.plugin_config,
                record.owned_workspace,
                record.herdr_state,
                record.fingerprint,
            ],
        )?;
        Ok(())
    }

    pub fn save_snapshot(&mut self, snapshot: &RunSnapshot) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "UPDATE runs SET phase = ?2, outcome = ?3, updated_at = ?4 WHERE id = ?1",
            params![
                snapshot.run_id,
                enum_text(&snapshot.phase),
                enum_text(&snapshot.outcome),
                snapshot.updated_at
            ],
        )?;
        if changed == 0 {
            return Err(StoreError::RunNotFound(snapshot.run_id.clone()));
        }
        for (task_id, task) in &snapshot.tasks {
            tx.execute(
                "INSERT INTO tasks (run_id, task_id, snapshot) VALUES (?1, ?2, ?3)
                 ON CONFLICT(run_id, task_id) DO UPDATE SET snapshot = excluded.snapshot",
                params![
                    snapshot.run_id,
                    task_id.0,
                    serde_json::to_string(task).unwrap_or_default()
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn add_event(&self, run_id: &str, message: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO events (run_id, time, message) VALUES (?1, ?2, ?3)",
            params![run_id, logs::rfc3339(std::time::SystemTime::now()), message],
        )?;
        Ok(())
    }

    fn run_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(RunRecord, String, String)> {
        let inputs: String = row.get(10)?;
        Ok((
            RunRecord {
                run_id: row.get(0)?,
                created_at: row.get(1)?,
                updated_at: row.get(2)?,
                plan_hash: row.get(3)?,
                config_path: PathBuf::from(row.get::<_, String>(4)?),
                checkout: PathBuf::from(row.get::<_, String>(5)?),
                target: row.get(6)?,
                phase: RunPhase::Queued,
                outcome: RunOutcome::Pending,
                workflow: row.get(9)?,
                inputs: serde_json::from_str(&inputs).unwrap_or_default(),
                coordinator_pid: row.get(11)?,
                plugin_config: row.get(12)?,
                owned_workspace: row.get(13)?,
                herdr_state: row.get(14)?,
                fingerprint: row.get(15)?,
            },
            row.get(7)?,
            row.get(8)?,
        ))
    }

    fn finish_run(raw: (RunRecord, String, String)) -> Result<RunRecord, StoreError> {
        let (mut record, phase, outcome) = raw;
        record.phase = enum_from(&phase)?;
        record.outcome = enum_from(&outcome)?;
        Ok(record)
    }

    const RUN_COLUMNS: &'static str = "id, created_at, updated_at, plan_hash, config_path, checkout, target, phase, outcome, workflow, inputs, coordinator_pid, plugin_config, owned_workspace, herdr_state, fingerprint";

    pub fn run(&self, run_id: &str) -> Result<RunRecord, StoreError> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {} FROM runs WHERE id = ?1", Self::RUN_COLUMNS),
                params![run_id],
                Self::run_from_row,
            )
            .optional()?
            .ok_or_else(|| StoreError::RunNotFound(run_id.to_string()))?;
        Self::finish_run(raw)
    }

    pub fn recent_runs(&self, limit: u32) -> Result<Vec<RunRecord>, StoreError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {} FROM runs ORDER BY created_at DESC, id DESC LIMIT ?1",
            Self::RUN_COLUMNS
        ))?;
        let rows = statement.query_map(params![limit], Self::run_from_row)?;
        let mut runs = Vec::new();
        for row in rows {
            runs.push(Self::finish_run(row?)?);
        }
        Ok(runs)
    }

    pub fn tasks(
        &self,
        run_id: &str,
    ) -> Result<
        std::collections::BTreeMap<
            crate::config::model::TaskId,
            crate::runtime::engine::TaskSnapshot,
        >,
        StoreError,
    > {
        let mut statement = self
            .conn
            .prepare("SELECT task_id, snapshot FROM tasks WHERE run_id = ?1 ORDER BY task_id")?;
        let rows = statement.query_map(params![run_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut tasks = std::collections::BTreeMap::new();
        for row in rows {
            let (task_id, snapshot) = row?;
            let snapshot = serde_json::from_str(&snapshot)
                .map_err(|error| StoreError::Invalid(error.to_string()))?;
            tasks.insert(crate::config::model::TaskId(task_id), snapshot);
        }
        Ok(tasks)
    }

    pub fn events(
        &self,
        run_id: &str,
        after: i64,
    ) -> Result<Vec<(i64, String, String)>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT id, time, message FROM events WHERE run_id = ?1 AND id > ?2 ORDER BY id",
        )?;
        let rows = statement.query_map(params![run_id, after], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Coordinatorの異常終了で残った未完了Runの状態を更新する(照合後)。
    pub fn set_run_state(
        &self,
        run_id: &str,
        phase: RunPhase,
        outcome: RunOutcome,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE runs SET phase = ?2, outcome = ?3, updated_at = ?4 WHERE id = ?1",
            params![
                run_id,
                enum_text(&phase),
                enum_text(&outcome),
                logs::rfc3339(std::time::SystemTime::now())
            ],
        )?;
        Ok(())
    }
}

impl Store {
    /// 承認を記録する(同じ組は上書き)。
    pub fn set_herdr_state(&self, run_id: &str, state: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE runs SET herdr_state = ?2 WHERE id = ?1",
            params![run_id, state],
        )?;
        Ok(())
    }

    /// 承認を記録する。`side_effects`は承認時のGitのhook・フィルターの要約。
    pub fn set_fingerprint(&self, run_id: &str, fingerprint: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE runs SET fingerprint = ?2 WHERE id = ?1",
            params![run_id, fingerprint],
        )?;
        Ok(())
    }

    /// 承認を記録する。`side_effects`は承認時のGitのhook・フィルターの要約。
    pub fn approve(
        &self,
        repo: &str,
        config_hash: &str,
        side_effects: &str,
        summary: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO approvals (repo, config_hash, approved_at, summary, side_effects)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(repo, config_hash) DO UPDATE SET approved_at = excluded.approved_at,
                summary = excluded.summary, side_effects = excluded.side_effects",
            params![
                repo,
                config_hash,
                logs::rfc3339(std::time::SystemTime::now()),
                summary,
                side_effects
            ],
        )?;
        Ok(())
    }

    /// 承認の照合。設定ハッシュが一致しても、Gitのhook・フィルターが変わっていれば無効。
    pub fn approval_status(
        &self,
        repo: &str,
        config_hash: &str,
        side_effects: &str,
    ) -> Result<ApprovalStatus, StoreError> {
        let recorded: Option<String> = self
            .conn
            .query_row(
                "SELECT side_effects FROM approvals WHERE repo = ?1 AND config_hash = ?2",
                params![repo, config_hash],
                |row| row.get(0),
            )
            .optional()?;
        Ok(match recorded {
            None => ApprovalStatus::Missing,
            Some(recorded) if recorded == side_effects => ApprovalStatus::Approved,
            Some(_) => ApprovalStatus::SideEffectsChanged,
        })
    }

    /// 自動受付を一度だけ記録する。既に同じキーがあれば`false`(重複)。
    pub fn accept_once(&self, key: &str, hold_reason: Option<&str>) -> Result<bool, StoreError> {
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO acceptances (key, run_id, hold_reason, received_at)
             VALUES (?1, NULL, ?2, ?3)",
            params![
                key,
                hold_reason,
                logs::rfc3339(std::time::SystemTime::now())
            ],
        )?;
        Ok(inserted == 1)
    }

    pub fn acceptance_exists(&self, key: &str) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM acceptances WHERE key = ?1",
                params![key],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// 保留していた受付を、承認後の再受付のために取り除く。
    pub fn release_held(&self, key: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM acceptances WHERE key = ?1 AND hold_reason IS NOT NULL",
            params![key],
        )?;
        Ok(())
    }

    pub fn set_acceptance_run(&self, key: &str, run_id: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE acceptances SET run_id = ?2, hold_reason = NULL WHERE key = ?1",
            params![key, run_id],
        )?;
        Ok(())
    }
}

/// エンジンの記録をSQLiteへ保存し、通知を任意の出力先へも渡す。
pub struct StoreRecorder<'a> {
    pub store: &'a mut Store,
    pub run_id: String,
    pub on_notice: Box<dyn FnMut(&str) + 'a>,
}

impl Recorder for StoreRecorder<'_> {
    fn record(&mut self, snapshot: &RunSnapshot) -> Result<(), String> {
        self.store
            .save_snapshot(snapshot)
            .map_err(|error| error.to_string())
    }

    fn notice(&mut self, message: &str) {
        let _ = self.store.add_event(&self.run_id, message);
        (self.on_notice)(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::state_dir::test_support::TempDir;

    fn record(id: &str) -> RunRecord {
        RunRecord {
            run_id: id.to_string(),
            created_at: logs::rfc3339(std::time::SystemTime::now()),
            updated_at: String::new(),
            plan_hash: "sha256:x".to_string(),
            config_path: PathBuf::from("/c/workflow.yaml"),
            checkout: PathBuf::from("/w"),
            target: Some("tests".to_string()),
            phase: RunPhase::Queued,
            outcome: RunOutcome::Pending,
            workflow: "name: x".to_string(),
            inputs: vec![("port".to_string(), "3001".to_string())],
            coordinator_pid: Some(1),
            plugin_config: String::new(),
            owned_workspace: None,
            herdr_state: None,
            fingerprint: None,
        }
    }

    #[test]
    fn runs_and_snapshots_round_trip() {
        let tmp = TempDir::new("store");
        let mut store = Store::open(&tmp.0.join("state.db")).unwrap();
        store.insert_run(&record("r1")).unwrap();
        let snapshot = RunSnapshot {
            run_id: "r1".to_string(),
            phase: RunPhase::Active,
            outcome: RunOutcome::Failed,
            target: None,
            tasks: std::collections::BTreeMap::new(),
            updated_at: "t".to_string(),
        };
        store.save_snapshot(&snapshot).unwrap();
        store.add_event("r1", "hello").unwrap();
        let run = store.run("r1").unwrap();
        assert_eq!(run.phase, RunPhase::Active);
        assert_eq!(run.outcome, RunOutcome::Failed);
        assert_eq!(run.inputs, vec![("port".to_string(), "3001".to_string())]);
        assert_eq!(store.recent_runs(10).unwrap().len(), 1);
        assert_eq!(store.events("r1", 0).unwrap().len(), 1);
        assert!(matches!(store.run("nope"), Err(StoreError::RunNotFound(_))));
    }

    #[test]
    fn newer_schema_is_rejected_without_change() {
        let tmp = TempDir::new("store");
        let path = tmp.0.join("state.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", 99).unwrap();
        }
        assert!(matches!(
            Store::open(&path),
            Err(StoreError::NewerSchema { found: 99, .. })
        ));
        let conn = Connection::open(&path).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 99);
    }

    #[test]
    fn version_one_database_is_backed_up_and_migrated() {
        let tmp = TempDir::new("store");
        let path = tmp.0.join("state.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE runs (id TEXT); PRAGMA user_version = 1;")
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert!(path.with_extension("v1.bak").exists());
        assert_eq!(
            store.approval_status("repo", "h", "s").unwrap(),
            ApprovalStatus::Missing
        );
        store.approve("repo", "h", "s", "summary").unwrap();
        assert_eq!(
            store.approval_status("repo", "h", "s").unwrap(),
            ApprovalStatus::Approved
        );
        assert_eq!(
            store.approval_status("repo", "h", "other").unwrap(),
            ApprovalStatus::SideEffectsChanged
        );
        assert_eq!(
            store.approval_status("repo", "h2", "s").unwrap(),
            ApprovalStatus::Missing
        );
        assert!(store.accept_once("k", None).unwrap());
        assert!(!store.accept_once("k", None).unwrap());
    }

    #[test]
    fn database_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new("store");
        let path = tmp.0.join("state.db");
        Store::open(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
