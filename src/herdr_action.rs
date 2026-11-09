//! 他のHerdrプラグインのactionを一つのjobとして実行するアダプター(Q08、F13)。
//!
//! `plugin.action.invoke`は呼び出しの受付とコマンドの開始を返すだけなので、それだけでは
//! 完了と判定しない。返された実行記録のIDを`plugin.log.list`で照会し、記録が`succeeded`・
//! `failed`になったときの終了コードをこのjobの結果にする。記録の標準出力と標準エラーは
//! 完了時にこのjobのログへ写す。Herdr v0.9.5の公開APIにはactionのコマンドを止める
//! 手段がないため、停止要求では照会をやめて終了し、action側が動き続けうることを示す。
//!
//! タスクの`herdrAction`は、`herdr-workflow herdr-action '<JSON>'`を起動するjobとして準備する
//! (`runtime::prepare`)。実行期限、再試行、ロック、ログは通常のjobと同じ扱いになる。

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::model::HerdrActionSpec;
use crate::herdr::{HerdrClient, HerdrError};

/// 停止要求で照会をやめたときの終了コード(SIGTERMによる終了と同じ値)。
pub const EXIT_STOPPED: i32 = 143;

/// 実行記録の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Succeeded,
    Failed(i32),
    Stopped,
}

fn record_state(record: &Value) -> Option<Outcome> {
    match record["status"].as_str()? {
        "succeeded" => Some(Outcome::Succeeded),
        "failed" => Some(Outcome::Failed(
            record["exit_code"]
                .as_i64()
                .and_then(|code| i32::try_from(code).ok())
                .filter(|code| *code != 0)
                .unwrap_or(1),
        )),
        _ => None,
    }
}

fn copy_output(record: &Value, out: &mut dyn Write, err: &mut dyn Write) {
    if let Some(text) = record["stdout"].as_str().filter(|text| !text.is_empty()) {
        let _ = writeln!(out, "{}", text.trim_end_matches('\n'));
    }
    if let Some(text) = record["stderr"].as_str().filter(|text| !text.is_empty()) {
        let _ = writeln!(err, "{}", text.trim_end_matches('\n'));
    }
    if let Some(text) = record["error"].as_str().filter(|text| !text.is_empty()) {
        let _ = writeln!(err, "herdr: {text}");
    }
}

/// actionを呼び出し、実行記録が確定するまで照会する。
///
/// # Errors
/// Herdrへの要求が失敗した場合、または実行記録が見つからなくなった場合。
pub fn run(
    spec: &HerdrActionSpec,
    client: &mut HerdrClient,
    workspace_id: Option<&str>,
    cwd: &Path,
    stop: &AtomicBool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<Outcome, HerdrError> {
    let mut context = json!({
        "invocation_source": "herdr-workflow",
        "workspace_cwd": cwd,
    });
    if let Some(workspace) = workspace_id {
        context["workspace_id"] = json!(workspace);
    }
    let mut params = json!({ "action_id": spec.action, "context": context });
    if let Some(plugin) = &spec.plugin {
        params["plugin_id"] = json!(plugin);
    }
    let invoked = client.request("plugin.action.invoke", params)?;
    let log = &invoked["log"];
    let log_id = log["log_id"].as_str().unwrap_or_default().to_string();
    let plugin_id = log["plugin_id"]
        .as_str()
        .map(str::to_string)
        .or_else(|| spec.plugin.clone());
    let _ = writeln!(out, "invoked Herdr action {} (log {log_id})", spec.action);
    if let Some(outcome) = record_state(log) {
        copy_output(log, out, err);
        return Ok(outcome);
    }
    let interval = Duration::from_millis(spec.poll_interval_milliseconds);
    loop {
        let deadline = std::time::Instant::now() + interval;
        while std::time::Instant::now() < deadline {
            if stop.load(Ordering::Relaxed) {
                let _ = writeln!(
                    err,
                    "stop requested; Herdr has no API to cancel plugin action commands, so action {} may still be running",
                    spec.action
                );
                return Ok(Outcome::Stopped);
            }
            std::thread::sleep(Duration::from_millis(50).min(interval));
        }
        let listed = client.request(
            "plugin.log.list",
            json!({ "plugin_id": plugin_id, "limit": 100 }),
        )?;
        let record = listed["logs"]
            .as_array()
            .and_then(|logs| {
                logs.iter()
                    .find(|record| record["log_id"] == log_id.as_str())
            })
            .cloned();
        let Some(record) = record else {
            return Err(HerdrError::Io {
                method: "plugin.log.list".to_string(),
                message: format!("command log {log_id} is no longer listed; its result is unknown"),
            });
        };
        if let Some(outcome) = record_state(&record) {
            copy_output(&record, out, err);
            return Ok(outcome);
        }
    }
}

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// `herdr-workflow herdr-action '<JSON>'`の入口。
pub fn main(args: &[String]) -> i32 {
    let Some(spec) = args
        .first()
        .and_then(|text| serde_json::from_str::<HerdrActionSpec>(text).ok())
    else {
        eprintln!("usage: herdr-workflow herdr-action '<herdrAction JSON>'");
        return 2;
    };
    // SAFETY: フラグを立てるだけの信号処理を登録する。
    unsafe {
        let handler = on_term as extern "C" fn(libc::c_int);
        libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
        libc::signal(libc::SIGINT, handler as libc::sighandler_t);
    }
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let mut client = match HerdrClient::from_env(&env) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("herdr-action: {error}");
            return 1;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let workspace = env
        .get("HERDR_WORKSPACE_ID")
        .filter(|id| !id.is_empty())
        .cloned();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    match run(
        &spec,
        &mut client,
        workspace.as_deref(),
        &cwd,
        &STOP,
        &mut stdout,
        &mut stderr,
    ) {
        Ok(Outcome::Succeeded) => 0,
        Ok(Outcome::Failed(code)) => code,
        Ok(Outcome::Stopped) => EXIT_STOPPED,
        Err(error) => {
            eprintln!("herdr-action: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::fake::FakeHerdr;
    use crate::runtime::state_dir::test_support::TempDir;

    fn spec() -> HerdrActionSpec {
        HerdrActionSpec {
            action: "example.bootstrap.setup".to_string(),
            plugin: Some("example.bootstrap".to_string()),
            poll_interval_milliseconds: 10,
        }
    }

    #[test]
    fn completion_is_observed_from_the_command_log_not_the_invoke_reply() {
        let tmp = TempDir::new("herdr-action");
        let socket = PathBuf::from(format!("/tmp/hwa-{}.sock", std::process::id()));
        let fake = FakeHerdr::start(socket.clone(), tmp.0.clone());
        fake.plugin_logs.lock().unwrap().extend([
            json!({"status": "running"}),
            json!({"status": "succeeded", "exit_code": 0, "stdout": "done\n"}),
        ]);
        let mut client = HerdrClient::new(socket.clone());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let outcome = run(
            &spec(),
            &mut client,
            Some("w9"),
            &tmp.0,
            &AtomicBool::new(false),
            &mut out,
            &mut err,
        )
        .unwrap();
        assert_eq!(outcome, Outcome::Succeeded);
        assert!(String::from_utf8(out).unwrap().contains("done"));
        let methods = fake.methods();
        assert_eq!(methods[0], "plugin.action.invoke");
        assert_eq!(
            methods.iter().filter(|m| *m == "plugin.log.list").count(),
            2,
            "{methods:?}"
        );
        let invoke = &fake.requests.lock().unwrap()[0];
        assert_eq!(invoke["params"]["action_id"], "example.bootstrap.setup");
        assert_eq!(invoke["params"]["context"]["workspace_id"], "w9");
        let _ = std::fs::remove_file(socket);
    }

    #[test]
    fn failure_exit_code_and_stop_are_reported() {
        let tmp = TempDir::new("herdr-action");
        let socket = PathBuf::from(format!("/tmp/hwb-{}.sock", std::process::id()));
        let fake = FakeHerdr::start(socket.clone(), tmp.0.clone());
        fake.plugin_logs
            .lock()
            .unwrap()
            .push_back(json!({"status": "failed", "exit_code": 7, "stderr": "boom"}));
        let mut client = HerdrClient::new(socket.clone());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let stop = AtomicBool::new(false);
        let outcome = run(
            &spec(),
            &mut client,
            None,
            &tmp.0,
            &stop,
            &mut out,
            &mut err,
        )
        .unwrap();
        assert_eq!(outcome, Outcome::Failed(7));
        assert!(String::from_utf8(err).unwrap().contains("boom"));

        // 実行中のまま停止を求められたら、照会をやめて止められないことを示す。
        fake.plugin_logs
            .lock()
            .unwrap()
            .push_back(json!({"status": "running"}));
        stop.store(true, Ordering::Relaxed);
        let mut err = Vec::new();
        let outcome = run(
            &spec(),
            &mut client,
            None,
            &tmp.0,
            &stop,
            &mut out,
            &mut err,
        )
        .unwrap();
        assert_eq!(outcome, Outcome::Stopped);
        assert!(
            String::from_utf8(err)
                .unwrap()
                .contains("may still be running")
        );
        let _ = std::fs::remove_file(socket);
    }

    use std::path::PathBuf;
}
