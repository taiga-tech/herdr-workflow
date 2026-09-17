use serde::{Deserialize, Serialize};

/// docs/specs/state-model.md の10状態。readinessやAgent観測値とは別フィールドで持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TaskState {
    #[default]
    Waiting,
    Starting,
    Running,
    Succeeded,
    Failed,
    Stopping,
    Stopped,
    Cancelled,
    Skipped,
    Unknown,
}

/// serviceはreadinessが整っても`succeeded`にはならず`running`のままとする
/// (state-model.md)。これは`TaskState`とは独立したフィールドで表現する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ReadinessState {
    #[default]
    NotApplicable,
    Pending,
    Ready,
    Unready,
}

/// Agentの表示上の観測値。Taskの終了結果とは統合しない(state-model.md)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentObservation {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

/// 再試行ごとに発行される起動世代の識別子。値が大きいほど新しい世代を表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct AttemptId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LaunchEvidence {
    pub pid: Option<u32>,
    /// RFC3339文字列。日時計算は今回のスコープ外のため`time`crateは追加しない。
    pub started_at: Option<String>,
    pub command_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum AttemptOutcome {
    Pending,
    ExitCode(i32),
    UnexpectedExit,
    Signaled(String),
    LaunchFailed(String),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct LogPosition {
    pub byte_offset: u64,
    pub line_number: u64,
}

/// TaskRunnerが保存するAttempt単位の起動証跡・終了結果・ログ位置
/// (docs/architecture.md TaskRunnerの責務)。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AttemptResult {
    pub attempt_id: AttemptId,
    pub launch_evidence: LaunchEvidence,
    pub outcome: AttemptOutcome,
    pub log_position: LogPosition,
}

/// スケジューリング判定に必要なTaskの実行時状態。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
pub struct TaskRuntimeState {
    pub state: TaskState,
    pub readiness: ReadinessState,
    pub agent: Option<AgentObservation>,
    /// AttemptId昇順。最新は`current_attempt()`で取得する。
    pub attempts: Vec<AttemptResult>,
}

impl TaskRuntimeState {
    pub fn current_attempt(&self) -> Option<&AttemptResult> {
        self.attempts.last()
    }

    /// 古いAttemptの遅延通知や同じAttemptの重複通知による上書きを防ぐ
    /// (state-model.md「起動世代」、T16)。
    pub fn record_attempt_result(&mut self, result: AttemptResult) {
        let is_stale = self
            .current_attempt()
            .is_some_and(|latest| result.attempt_id <= latest.attempt_id);
        if !is_stale {
            self.attempts.push(result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_result(attempt_id: u32) -> AttemptResult {
        AttemptResult {
            attempt_id: AttemptId(attempt_id),
            launch_evidence: LaunchEvidence {
                pid: Some(1234),
                started_at: None,
                command_summary: "mise install".to_string(),
            },
            outcome: AttemptOutcome::Pending,
            log_position: LogPosition {
                byte_offset: 0,
                line_number: 0,
            },
        }
    }

    #[test]
    fn readiness_and_task_state_are_independent_fields() {
        let running_but_ready = TaskRuntimeState {
            state: TaskState::Running,
            readiness: ReadinessState::Ready,
            agent: None,
            attempts: Vec::new(),
        };
        assert_eq!(running_but_ready.state, TaskState::Running);
        assert_eq!(running_but_ready.readiness, ReadinessState::Ready);
    }

    #[test]
    fn record_attempt_result_accepts_new_generation() {
        let mut runtime = TaskRuntimeState::default();
        runtime.record_attempt_result(sample_result(1));
        runtime.record_attempt_result(sample_result(2));
        assert_eq!(runtime.current_attempt().unwrap().attempt_id, AttemptId(2));
        assert_eq!(runtime.attempts.len(), 2);
    }

    #[test]
    fn record_attempt_result_ignores_stale_attempt() {
        let mut runtime = TaskRuntimeState::default();
        runtime.record_attempt_result(sample_result(2));
        runtime.record_attempt_result(sample_result(1));
        assert_eq!(runtime.current_attempt().unwrap().attempt_id, AttemptId(2));
        assert_eq!(runtime.attempts.len(), 1);
    }

    #[test]
    fn record_attempt_result_ignores_duplicate_attempt_without_overwriting_result() {
        // 同じ世代の再送・遅延通知は、確定済みの結果と起動証跡を上書きしない。
        let mut runtime = TaskRuntimeState::default();
        let mut completed = sample_result(2);
        completed.outcome = AttemptOutcome::ExitCode(0);
        completed.log_position.byte_offset = 100;
        runtime.record_attempt_result(completed.clone());
        runtime.record_attempt_result(completed.clone());
        runtime.record_attempt_result(sample_result(2));

        assert_eq!(runtime.current_attempt(), Some(&completed));
        assert_eq!(runtime.attempts, vec![completed]);
    }
}
