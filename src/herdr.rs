//! HerdrAdapter(docs/integrations/herdr.md)。
//!
//! Herdr v0.9.5タグの公開文書(`socket-api.mdx`)とAPIスキーマ(`herdr-api.schema.json`)に
//! 記載されたメソッドだけを使う。通信は`HERDR_SOCKET_PATH`のUnix domain socketへ
//! 1行1要求のJSONを送り、`{"id","result"}`または`{"id","error"}`を受け取る。
//! HerdrのJSONはこの境界で内部の型へ変換し、Run・Taskへ直接流し込まない。

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::model::PaneId;
use crate::plan::layout::{SplitDirection, SplitNode};

pub const METADATA_SOURCE: &str = "herdr-workflow";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HerdrError {
    #[error("HERDR_SOCKET_PATH is not set; run inside Herdr or use a headless command")]
    NoSocket,
    #[error("could not connect to Herdr at {path}: {message}")]
    Connect { path: PathBuf, message: String },
    #[error("Herdr request {method} failed: {message}")]
    Io { method: String, message: String },
    #[error("Herdr returned {code} for {method}: {message}")]
    Remote {
        method: String,
        code: String,
        message: String,
    },
    #[error("unexpected Herdr response for {method}: {message}")]
    Shape { method: String, message: String },
}

pub struct HerdrClient {
    path: PathBuf,
    next_id: u64,
}

impl HerdrClient {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path, next_id: 1 }
    }

    /// 要求元の環境の`HERDR_SOCKET_PATH`から作る。
    pub fn from_env(env: &BTreeMap<String, String>) -> Result<Self, HerdrError> {
        env.get("HERDR_SOCKET_PATH")
            .filter(|path| !path.is_empty())
            .map(|path| Self::new(PathBuf::from(path)))
            .ok_or(HerdrError::NoSocket)
    }

    /// 1要求ごとに接続する。変更系の要求は応答を失っても同じ要求を再送しない。
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, HerdrError> {
        let io = |message: String| HerdrError::Io {
            method: method.to_string(),
            message,
        };
        let mut stream = UnixStream::connect(&self.path).map_err(|error| HerdrError::Connect {
            path: self.path.clone(),
            message: error.to_string(),
        })?;
        stream
            .set_read_timeout(Some(Duration::from_secs(320)))
            .map_err(|error| io(error.to_string()))?;
        let id = format!("hw{}", self.next_id);
        self.next_id += 1;
        let mut line = serde_json::to_vec(&json!({ "id": id, "method": method, "params": params }))
            .map_err(|error| io(error.to_string()))?;
        line.push(b'\n');
        stream
            .write_all(&line)
            .map_err(|error| io(error.to_string()))?;
        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        reader
            .read_line(&mut response)
            .map_err(|error| io(error.to_string()))?;
        let value: Value = serde_json::from_str(&response).map_err(|error| HerdrError::Shape {
            method: method.to_string(),
            message: error.to_string(),
        })?;
        if let Some(error) = value.get("error") {
            return Err(HerdrError::Remote {
                method: method.to_string(),
                code: error["code"].as_str().unwrap_or("unknown").to_string(),
                message: error["message"].as_str().unwrap_or_default().to_string(),
            });
        }
        value
            .get("result")
            .cloned()
            .ok_or_else(|| HerdrError::Shape {
                method: method.to_string(),
                message: "missing result".to_string(),
            })
    }
}

fn field<'a>(value: &'a Value, path: &[&str], method: &str) -> Result<&'a str, HerdrError> {
    let mut current = value;
    for key in path {
        current = current.get(key).ok_or_else(|| HerdrError::Shape {
            method: method.to_string(),
            message: format!("missing {}", path.join(".")),
        })?;
    }
    current.as_str().ok_or_else(|| HerdrError::Shape {
        method: method.to_string(),
        message: format!("{} is not a string", path.join(".")),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedWorktree {
    pub workspace_id: String,
    pub tab_id: String,
    pub root_pane_id: String,
    pub path: PathBuf,
    pub branch: Option<String>,
}

/// `worktree.create`: checkoutとworkspaceをまとめて作る。既存のローカルブランチは
/// そのままcheckoutし、ない場合は`base`(なければHEAD)から作る。
pub fn create_worktree(
    client: &mut HerdrClient,
    repo_cwd: &Path,
    branch: &str,
    base: Option<&str>,
    label: &str,
) -> Result<CreatedWorktree, HerdrError> {
    const METHOD: &str = "worktree.create";
    let result = client.request(
        METHOD,
        json!({
            "cwd": repo_cwd,
            "branch": branch,
            "base": base,
            "label": label,
            "focus": false,
        }),
    )?;
    Ok(CreatedWorktree {
        workspace_id: field(&result, &["workspace", "workspace_id"], METHOD)?.to_string(),
        tab_id: field(&result, &["tab", "tab_id"], METHOD)?.to_string(),
        root_pane_id: field(&result, &["root_pane", "pane_id"], METHOD)?.to_string(),
        path: PathBuf::from(field(&result, &["worktree", "path"], METHOD)?),
        branch: result["worktree"]["branch"].as_str().map(str::to_string),
    })
}

/// 配置する1つのpane。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSpec {
    pub label: String,
    pub cwd: PathBuf,
    /// `None`は通常のshell(AgentとShellのpane)。
    pub command: Option<Vec<String>>,
    pub env: BTreeMap<String, String>,
}

fn layout_node(
    node: &SplitNode,
    panes: &BTreeMap<PaneId, PaneSpec>,
    order: &mut Vec<PaneId>,
) -> Value {
    match node {
        SplitNode::Leaf { pane } => {
            order.push(pane.clone());
            let spec = panes.get(pane);
            let mut value = json!({ "type": "pane" });
            if let Some(spec) = spec {
                value["label"] = json!(spec.label);
                value["cwd"] = json!(spec.cwd);
                if let Some(command) = &spec.command {
                    value["command"] = json!(command);
                }
                if !spec.env.is_empty() {
                    value["env"] = json!(spec.env);
                }
            }
            value
        }
        SplitNode::Split {
            direction,
            first_span,
            total_span,
            first,
            second,
        } => {
            let ratio = f64::from(*first_span) / f64::from((*total_span).max(1));
            json!({
                "type": "split",
                "direction": match direction {
                    SplitDirection::Right => "right",
                    SplitDirection::Down => "down",
                },
                "ratio": ratio,
                "first": layout_node(first, panes, order),
                "second": layout_node(second, panes, order),
            })
        }
    }
}

fn collect_pane_ids(node: &Value, out: &mut Vec<String>) {
    if node["type"] == "split" {
        collect_pane_ids(&node["first"], out);
        collect_pane_ids(&node["second"], out);
    } else if let Some(id) = node["pane_id"].as_str() {
        out.push(id.to_string());
    }
}

/// `layout.apply`: 分割木から新しいtabを作る。既存tabを置換しないよう`tab_id`は渡さない。
/// 戻り値は設定のpane IDからHerdrのpane IDへの対応。
pub fn apply_layout(
    client: &mut HerdrClient,
    workspace_id: &str,
    tab_label: &str,
    root: &SplitNode,
    panes: &BTreeMap<PaneId, PaneSpec>,
) -> Result<BTreeMap<PaneId, String>, HerdrError> {
    const METHOD: &str = "layout.apply";
    let mut order = Vec::new();
    let tree = layout_node(root, panes, &mut order);
    let result = client.request(
        METHOD,
        json!({
            "workspace_id": workspace_id,
            "tab_label": tab_label,
            "focus": false,
            "root": tree,
        }),
    )?;
    let mut ids = Vec::new();
    collect_pane_ids(&result["layout"]["root"], &mut ids);
    if ids.len() != order.len() {
        return Err(HerdrError::Shape {
            method: METHOD.to_string(),
            message: format!("expected {} panes, got {}", order.len(), ids.len()),
        });
    }
    Ok(order.into_iter().zip(ids).collect())
}

/// `agent.start`: 既存のshell paneでAgentを起動する。成功はAgentが入力可能になった後に返る。
pub fn start_agent(
    client: &mut HerdrClient,
    name: &str,
    kind: &str,
    pane_id: &str,
) -> Result<(), HerdrError> {
    client.request(
        "agent.start",
        json!({ "name": name, "kind": kind, "pane_id": pane_id }),
    )?;
    Ok(())
}

/// 画面構築の結果として状態ストアへ保存するHerdr側の識別子。resumeでは
/// これを照合してから使い、照合できないpane IDでは操作しない(T17)。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrState {
    pub workspace_id: String,
    /// `<tab>/<pane>`(設定のID) → Herdrのpane ID。
    pub panes: BTreeMap<String, String>,
    /// Agentタスク → そのAgentを起動したpane ID。
    pub agent_panes: BTreeMap<String, String>,
}

/// `pane.get`: paneが存在するか。`not_found`は`false`。
pub fn pane_exists(client: &mut HerdrClient, pane_id: &str) -> Result<bool, HerdrError> {
    match client.request("pane.get", json!({ "pane_id": pane_id })) {
        Ok(result) => Ok(result["pane"]["pane_id"].as_str().is_some()),
        Err(HerdrError::Remote { code, .. }) if code.contains("not_found") => Ok(false),
        Err(error) => Err(error),
    }
}

/// `agent.get`: Agentの観測値とpane ID。名前が解決できなければ`None`。
pub fn agent_info(
    client: &mut HerdrClient,
    target: &str,
) -> Result<Option<(String, String)>, HerdrError> {
    match client.request("agent.get", json!({ "target": target })) {
        Ok(result) => Ok(Some((
            result["agent"]["agent_status"]
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
            result["agent"]["pane_id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        ))),
        Err(HerdrError::Remote { code, .. }) if code.contains("not_found") => Ok(None),
        Err(error) => Err(error),
    }
}

/// `agent.get`: Agentの観測値(`agent_status`)を返す。Agentが終了・置換されて名前が
/// 解決できない場合(`not_found`)は`None`。
pub fn agent_status(client: &mut HerdrClient, target: &str) -> Result<Option<String>, HerdrError> {
    match client.request("agent.get", json!({ "target": target })) {
        Ok(result) => Ok(result["agent"]["agent_status"].as_str().map(str::to_string)),
        Err(HerdrError::Remote { code, .. }) if code.contains("not_found") => Ok(None),
        Err(error) => Err(error),
    }
}

/// `workspace.report_metadata`: workspace単位の集計値を報告する(表示専用)。
pub fn report_metadata(
    client: &mut HerdrClient,
    workspace_id: &str,
    tokens: &BTreeMap<String, String>,
) -> Result<(), HerdrError> {
    client.request(
        "workspace.report_metadata",
        json!({ "workspace_id": workspace_id, "source": METADATA_SOURCE, "tokens": tokens }),
    )?;
    Ok(())
}

/// サイドバーに表示できるworkspaceのtoken(Q03)。利用者がHerdrのSpace行に`$flow_state`等を
/// 書くと表示される。Herdrの規則(名前は1〜32文字、値は80文字まで)に収まる値だけを返す。
#[must_use]
pub fn sidebar_tokens(snapshot: &crate::runtime::engine::RunSnapshot) -> BTreeMap<String, String> {
    use crate::runtime::engine::{RunOutcome, RunPhase};
    use crate::state::models::TaskState;
    let state = match (snapshot.phase, snapshot.outcome) {
        (RunPhase::Finished | RunPhase::Stopped, RunOutcome::Succeeded) => "succeeded",
        (RunPhase::Finished | RunPhase::Stopped, RunOutcome::Failed) => "failed",
        (RunPhase::Finished | RunPhase::Stopped, RunOutcome::Cancelled) => "cancelled",
        (RunPhase::Finished | RunPhase::Stopped, _) => "unknown",
        (RunPhase::Queued, _) => "queued",
        (RunPhase::Preparing, _) => "preparing",
        (RunPhase::Active, _) => "active",
        (RunPhase::Stopping, _) => "stopping",
    };
    let count = |states: &[TaskState]| {
        snapshot
            .tasks
            .values()
            .filter(|task| states.contains(&task.runtime.state))
            .count()
    };
    BTreeMap::from([
        ("flow_state".to_string(), state.to_string()),
        (
            "flow_running".to_string(),
            count(&[TaskState::Starting, TaskState::Running]).to_string(),
        ),
        (
            "flow_failed".to_string(),
            count(&[TaskState::Failed, TaskState::Unknown]).to_string(),
        ),
        (
            "flow_progress".to_string(),
            format!(
                "{}/{}",
                count(&[TaskState::Succeeded]),
                snapshot.tasks.len()
            ),
        ),
    ])
}

/// Agent名はHerdrの規則`[a-z][a-z0-9_-]{0,31}`に合わせる。
#[must_use]
pub fn agent_name(task: &str) -> String {
    let mut name: String = task
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    if !name.starts_with(|c: char| c.is_ascii_lowercase()) {
        name.insert(0, 'a');
    }
    name.truncate(32);
    name
}

#[cfg(test)]
pub(crate) mod fake {
    //! 公開スキーマの応答形を返す偽のHerdrソケット(試験用)。
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};

    pub struct FakeHerdr {
        pub path: PathBuf,
        pub requests: Arc<Mutex<Vec<Value>>>,
        /// `agent.get`へ順に返す`agent_status`。尽きたら`not_found`を返す。
        pub agent_statuses: Arc<Mutex<std::collections::VecDeque<String>>>,
        /// `pane.get`で`not_found`を返すpane ID。
        pub missing_panes: Arc<Mutex<Vec<String>>>,
    }

    fn pane_ids(node: &Value, next: &mut u32) -> Value {
        if node["type"] == "split" {
            json!({
                "type": "split",
                "direction": node["direction"],
                "ratio": node["ratio"],
                "first": pane_ids(&node["first"], next),
                "second": pane_ids(&node["second"], next),
            })
        } else {
            *next += 1;
            json!({ "type": "pane", "pane_id": format!("w9:p{next}"), "label": node["label"] })
        }
    }

    impl FakeHerdr {
        pub fn start(path: PathBuf, worktree_path: PathBuf) -> Self {
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let log = Arc::clone(&requests);
            let agent_statuses = Arc::new(Mutex::new(std::collections::VecDeque::new()));
            let statuses = Arc::clone(&agent_statuses);
            let missing_panes = Arc::new(Mutex::new(Vec::new()));
            let missing = Arc::clone(&missing_panes);
            std::thread::spawn(move || {
                use std::io::{BufRead, BufReader, Write};
                let mut next = 0;
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { break };
                    let mut line = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        continue;
                    }
                    let request: Value = serde_json::from_str(&line).unwrap();
                    log.lock().unwrap().push(request.clone());
                    if request["method"] == "pane.get" {
                        let pane = request["params"]["pane_id"].as_str().unwrap_or_default();
                        let response = if missing.lock().unwrap().iter().any(|id| id == pane) {
                            json!({ "id": request["id"], "error": { "code": "not_found", "message": "pane not found" } })
                        } else {
                            json!({ "id": request["id"], "result": { "type": "pane_info", "pane": { "pane_id": pane } } })
                        };
                        let _ = stream.write_all(format!("{response}\n").as_bytes());
                        continue;
                    }
                    if request["method"] == "agent.get" {
                        let response = match statuses.lock().unwrap().pop_front() {
                            Some(status) => json!({ "id": request["id"], "result": {
                                "type": "agent_info",
                                "agent": { "pane_id": "w9:p2", "agent_status": status, "name": request["params"]["target"] }
                            }}),
                            None => {
                                json!({ "id": request["id"], "error": { "code": "not_found", "message": "agent not found" } })
                            }
                        };
                        let _ = stream.write_all(format!("{response}\n").as_bytes());
                        continue;
                    }
                    let result = match request["method"].as_str().unwrap() {
                        "worktree.create" => json!({
                            "type": "worktree_created",
                            "workspace": { "workspace_id": "w9", "label": "x" },
                            "tab": { "tab_id": "w9:t1" },
                            "root_pane": { "pane_id": "w9:p0" },
                            "worktree": { "path": worktree_path, "branch": request["params"]["branch"], "is_bare": false, "is_detached": false, "is_prunable": false, "is_linked_worktree": true, "label": "x" }
                        }),
                        "layout.apply" => json!({
                            "type": "layout_apply",
                            "layout": { "workspace_id": "w9", "tab_id": "w9:t2", "zoomed": false, "focused_pane_id": "w9:p1", "root": pane_ids(&request["params"]["root"], &mut next) }
                        }),
                        "agent.start" => {
                            json!({ "type": "agent_started", "agent": { "pane_id": request["params"]["pane_id"] }, "argv": ["claude"] })
                        }
                        _ => json!({ "type": "ok" }),
                    };
                    let response = json!({ "id": request["id"], "result": result });
                    let _ = stream.write_all(format!("{response}\n").as_bytes());
                }
            });
            Self {
                path,
                requests,
                agent_statuses,
                missing_panes,
            }
        }

        pub fn methods(&self) -> Vec<String> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|request| request["method"].as_str().unwrap().to_string())
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeHerdr;
    use super::*;
    use crate::runtime::state_dir::test_support::TempDir;

    #[test]
    fn sidebar_tokens_summarize_the_run() {
        use crate::config::model::TaskId;
        use crate::runtime::engine::{RunOutcome, RunPhase, RunSnapshot, TaskKind, TaskSnapshot};
        use crate::state::models::{TaskRuntimeState, TaskState};
        let task = |state| TaskSnapshot {
            kind: TaskKind::Job,
            runtime: TaskRuntimeState {
                state,
                ..Default::default()
            },
            detail: None,
        };
        let mut snapshot = RunSnapshot {
            run_id: "r".to_string(),
            phase: RunPhase::Active,
            outcome: RunOutcome::Pending,
            target: None,
            tasks: BTreeMap::from([
                (TaskId::from("a"), task(TaskState::Succeeded)),
                (TaskId::from("b"), task(TaskState::Running)),
                (TaskId::from("c"), task(TaskState::Failed)),
                (TaskId::from("d"), task(TaskState::Waiting)),
            ]),
            updated_at: String::new(),
        };
        let tokens = sidebar_tokens(&snapshot);
        assert_eq!(tokens["flow_state"], "active");
        assert_eq!(tokens["flow_running"], "1");
        assert_eq!(tokens["flow_failed"], "1");
        assert_eq!(tokens["flow_progress"], "1/4");
        snapshot.phase = RunPhase::Finished;
        snapshot.outcome = RunOutcome::Failed;
        assert_eq!(sidebar_tokens(&snapshot)["flow_state"], "failed");
        for (name, value) in &tokens {
            assert!(name.len() <= 32 && value.len() <= 80);
        }
    }

    #[test]
    fn layout_is_sent_as_a_new_tab_and_pane_ids_are_mapped() {
        let tmp = TempDir::new("herdr");
        let fake = FakeHerdr::start(tmp.0.join("h.sock"), tmp.0.join("wt"));
        let mut client = HerdrClient::new(fake.path.clone());
        let root = SplitNode::Split {
            direction: SplitDirection::Right,
            first_span: 1,
            total_span: 3,
            first: Box::new(SplitNode::Leaf {
                pane: PaneId("A".to_string()),
            }),
            second: Box::new(SplitNode::Leaf {
                pane: PaneId("B".to_string()),
            }),
        };
        let panes = BTreeMap::from([
            (
                PaneId("A".to_string()),
                PaneSpec {
                    label: "server".to_string(),
                    cwd: PathBuf::from("/w"),
                    command: Some(vec!["hw".to_string(), "logs".to_string()]),
                    env: BTreeMap::new(),
                },
            ),
            (
                PaneId("B".to_string()),
                PaneSpec {
                    label: "shell".to_string(),
                    cwd: PathBuf::from("/w"),
                    command: None,
                    env: BTreeMap::new(),
                },
            ),
        ]);
        let ids = apply_layout(&mut client, "w9", "development", &root, &panes).unwrap();
        assert_eq!(ids[&PaneId("A".to_string())], "w9:p1");
        assert_eq!(ids[&PaneId("B".to_string())], "w9:p2");
        let request = fake.requests.lock().unwrap()[0].clone();
        assert_eq!(request["method"], "layout.apply");
        // 既存tabを置換しないため、tab_idを渡さない。
        assert!(request["params"].get("tab_id").is_none());
        let sent = &request["params"]["root"];
        assert_eq!(sent["direction"], "right");
        assert!((sent["ratio"].as_f64().unwrap() - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(sent["first"]["command"], json!(["hw", "logs"]));
        assert!(sent["second"].get("command").is_none());
    }

    #[test]
    fn worktree_create_maps_the_documented_response() {
        let tmp = TempDir::new("herdr");
        let fake = FakeHerdr::start(tmp.0.join("h.sock"), tmp.0.join("wt"));
        let mut client = HerdrClient::new(fake.path.clone());
        let created = create_worktree(
            &mut client,
            Path::new("/repo"),
            "feature/x",
            Some("main"),
            "x",
        )
        .unwrap();
        assert_eq!(created.workspace_id, "w9");
        assert_eq!(created.path, tmp.0.join("wt"));
        assert_eq!(created.branch.as_deref(), Some("feature/x"));
        let request = fake.requests.lock().unwrap()[0].clone();
        assert_eq!(request["params"]["focus"], false);
        assert_eq!(request["params"]["base"], "main");
    }

    #[test]
    fn agent_status_maps_not_found_to_none() {
        let tmp = TempDir::new("herdr");
        let fake = FakeHerdr::start(tmp.0.join("h.sock"), tmp.0.join("wt"));
        fake.agent_statuses
            .lock()
            .unwrap()
            .push_back("working".to_string());
        let mut client = HerdrClient::new(fake.path.clone());
        assert_eq!(
            agent_status(&mut client, "developer").unwrap().as_deref(),
            Some("working")
        );
        assert_eq!(agent_status(&mut client, "developer").unwrap(), None);
        let request = fake.requests.lock().unwrap()[0].clone();
        assert_eq!(request["params"]["target"], "developer");
    }

    #[test]
    fn missing_socket_is_reported() {
        assert_eq!(
            HerdrClient::from_env(&BTreeMap::new()).err(),
            Some(HerdrError::NoSocket)
        );
    }

    #[test]
    fn agent_names_follow_herdr_rules() {
        assert_eq!(agent_name("developer"), "developer");
        assert_eq!(agent_name("Dev.Task"), "dev-task");
        assert_eq!(agent_name("1st"), "a1st");
    }
}
