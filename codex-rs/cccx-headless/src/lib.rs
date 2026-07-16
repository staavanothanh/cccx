//! cccx headless execution output contract.
//!
//! Headless output must be machine-readable JSONL on stdout only. Human
//! diagnostics belong on stderr and approval requests are denied by default.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

pub const CCCX_HEADLESS_EVENT_SCHEMA_VERSION: &str = "cccx.headless_event.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadlessEventEnvelope {
    pub schema_version: String,
    pub seq: u64,
    #[serde(flatten)]
    pub event: HeadlessEvent,
}

impl HeadlessEventEnvelope {
    pub fn new(seq: u64, event: HeadlessEvent) -> Self {
        Self {
            schema_version: CCCX_HEADLESS_EVENT_SCHEMA_VERSION.to_string(),
            seq,
            event,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "kebab-case")]
pub enum HeadlessEvent {
    Answer(AnswerPayload),
    ApprovalDenied(ApprovalDeniedPayload),
    TaskUpdate(TaskUpdatePayload),
    AgentUpdate(AgentUpdatePayload),
    PermissionRequest(PermissionRequestPayload),
    Error(ErrorPayload),
    Terminal(TerminalPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerPayload {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalDeniedPayload {
    pub request_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskUpdatePayload {
    pub task_id: String,
    pub status: String,
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentUpdatePayload {
    pub logical_session_id: String,
    pub display_name: String,
    pub role: String,
    pub status: String,
    pub model_route: ModelRoutePayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRoutePayload {
    pub selection_source: String,
    pub requested_route_id: String,
    pub effective_route_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRequestPayload {
    pub request_id: String,
    pub agent_display_name: String,
    pub action: String,
    pub scope: String,
    pub risk: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub message: String,
    pub recoverable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPayload {
    pub status: TerminalStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    Success,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessStreams {
    pub stdout_jsonl: String,
    pub stderr_diagnostics: String,
}

pub fn render_jsonl(events: &[HeadlessEventEnvelope]) -> Result<String, HeadlessEventError> {
    validate_sequence(events)?;

    let mut output = String::new();
    for event in events {
        output.push_str(&serde_json::to_string(event)?);
        output.push('\n');
    }
    Ok(output)
}

pub fn split_headless_streams(
    events: &[HeadlessEventEnvelope],
    diagnostics: &[String],
) -> Result<HeadlessStreams, HeadlessEventError> {
    let stdout_jsonl = render_jsonl(events)?;
    assert_stdout_is_jsonl_only(&stdout_jsonl)?;

    let stderr_diagnostics = if diagnostics.is_empty() {
        String::new()
    } else {
        format!("{}\n", diagnostics.join("\n"))
    };

    Ok(HeadlessStreams {
        stdout_jsonl,
        stderr_diagnostics,
    })
}

pub fn deny_permission_by_default(request: &PermissionRequestPayload) -> HeadlessEvent {
    HeadlessEvent::ApprovalDenied(ApprovalDeniedPayload {
        request_id: request.request_id.clone(),
        reason: "headless approval default is deny".to_string(),
    })
}

pub fn assert_stdout_is_jsonl_only(stdout: &str) -> Result<(), HeadlessEventError> {
    for (index, line) in stdout.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }

        let value: Value =
            serde_json::from_str(line).map_err(|error| HeadlessEventError::NonJsonStdoutLine {
                line: index + 1,
                reason: error.to_string(),
            })?;

        let schema_version = value
            .get("schema_version")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if schema_version != CCCX_HEADLESS_EVENT_SCHEMA_VERSION {
            return Err(HeadlessEventError::UnsupportedSchemaVersion {
                line: index + 1,
                schema_version: schema_version.to_string(),
            });
        }
    }

    Ok(())
}

fn validate_sequence(events: &[HeadlessEventEnvelope]) -> Result<(), HeadlessEventError> {
    for (index, event) in events.iter().enumerate() {
        let expected = index as u64 + 1;
        if event.seq != expected {
            return Err(HeadlessEventError::NonContiguousSequence {
                expected,
                actual: event.seq,
            });
        }
    }
    Ok(())
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HeadlessEventError {
    #[error("failed to serialize headless event: {0}")]
    Serialize(String),
    #[error("event sequence is not contiguous: expected {expected}, actual {actual}")]
    NonContiguousSequence { expected: u64, actual: u64 },
    #[error("stdout line {line} is not JSON: {reason}")]
    NonJsonStdoutLine { line: usize, reason: String },
    #[error("stdout line {line} has unsupported schema version `{schema_version}`")]
    UnsupportedSchemaVersion { line: usize, schema_version: String },
}

impl From<serde_json::Error> for HeadlessEventError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn permission_request() -> PermissionRequestPayload {
        PermissionRequestPayload {
            request_id: "pr-17".to_string(),
            agent_display_name: "a1-auth-fix".to_string(),
            action: "edit".to_string(),
            scope: "src/auth/session.ts".to_string(),
            risk: "medium".to_string(),
        }
    }

    #[test]
    fn jsonl_event_schema_snapshot() {
        let event = HeadlessEventEnvelope::new(
            1,
            HeadlessEvent::AgentUpdate(AgentUpdatePayload {
                logical_session_id: "ls-a1".to_string(),
                display_name: "a1-auth-fix".to_string(),
                role: "cx:implementer".to_string(),
                status: "running".to_string(),
                model_route: ModelRoutePayload {
                    selection_source: "inherit_main_requested".to_string(),
                    requested_route_id: "9router/main".to_string(),
                    effective_route_id: "9router/main".to_string(),
                },
            }),
        );

        let rendered = render_jsonl(&[event]).expect("render jsonl");

        assert_eq!(
            rendered,
            "{\"schema_version\":\"cccx.headless_event.v1\",\"seq\":1,\"type\":\"agent-update\",\"payload\":{\"logical_session_id\":\"ls-a1\",\"display_name\":\"a1-auth-fix\",\"role\":\"cx:implementer\",\"status\":\"running\",\"model_route\":{\"selection_source\":\"inherit_main_requested\",\"requested_route_id\":\"9router/main\",\"effective_route_id\":\"9router/main\"}}}\n"
        );
    }

    #[test]
    fn stdout_accepts_jsonl_and_stderr_keeps_diagnostics() {
        let streams = split_headless_streams(
            &[
                HeadlessEventEnvelope::new(
                    1,
                    HeadlessEvent::Answer(AnswerPayload {
                        text: "done".to_string(),
                    }),
                ),
                HeadlessEventEnvelope::new(
                    2,
                    HeadlessEvent::Terminal(TerminalPayload {
                        status: TerminalStatus::Success,
                        exit_code: Some(0),
                    }),
                ),
            ],
            &["debug: compacted context".to_string()],
        )
        .expect("split streams");

        assert!(
            streams
                .stdout_jsonl
                .lines()
                .all(|line| { serde_json::from_str::<HeadlessEventEnvelope>(line).is_ok() })
        );
        assert_eq!(streams.stderr_diagnostics, "debug: compacted context\n");
    }

    #[test]
    fn stdout_rejects_prompt_spinner_or_banner_text() {
        assert_eq!(
            assert_stdout_is_jsonl_only("cccx> thinking...\n"),
            Err(HeadlessEventError::NonJsonStdoutLine {
                line: 1,
                reason: "expected value at line 1 column 1".to_string()
            })
        );
    }

    #[test]
    fn headless_permission_request_is_denied_by_default() {
        assert_eq!(
            deny_permission_by_default(&permission_request()),
            HeadlessEvent::ApprovalDenied(ApprovalDeniedPayload {
                request_id: "pr-17".to_string(),
                reason: "headless approval default is deny".to_string()
            })
        );
    }

    #[test]
    fn jsonl_sequence_must_be_contiguous() {
        assert_eq!(
            render_jsonl(&[HeadlessEventEnvelope::new(
                2,
                HeadlessEvent::Terminal(TerminalPayload {
                    status: TerminalStatus::Error,
                    exit_code: Some(1)
                })
            )]),
            Err(HeadlessEventError::NonContiguousSequence {
                expected: 1,
                actual: 2
            })
        );
    }
}
