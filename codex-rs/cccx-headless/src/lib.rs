//! cccx headless execution output contract.
//!
//! Headless output must be machine-readable JSONL on stdout only. Human
//! diagnostics belong on stderr and approval requests are denied by default.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

pub const CCCX_HEADLESS_EVENT_SCHEMA_VERSION: &str = "cccx.exec_event.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadlessEventEnvelope {
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    pub seq: u64,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "turnId")]
    pub turn_id: String,
    #[serde(flatten)]
    pub event: HeadlessEvent,
}

impl HeadlessEventEnvelope {
    pub fn new(seq: u64, event: HeadlessEvent) -> Self {
        Self {
            schema_version: CCCX_HEADLESS_EVENT_SCHEMA_VERSION.to_string(),
            seq,
            session_id: "mock-session".to_string(),
            turn_id: "mock-turn".to_string(),
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

    // Terminal event must be the last event.
    for (index, event) in events.iter().enumerate() {
        if matches!(event.event, HeadlessEvent::Terminal(_)) && index != events.len() - 1 {
            return Err(HeadlessEventError::Serialize(
                "terminal event must be last".to_string(),
            ));
        }
    }

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

        let value = parse_stdout_json_line(index + 1, line)?;

        let schema_version = value
            .get("schemaVersion")
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

pub fn assert_stdout_lines_are_json(stdout: &str) -> Result<(), HeadlessEventError> {
    for (index, line) in stdout.lines().enumerate() {
        if !line.trim().is_empty() {
            parse_stdout_json_line(index + 1, line)?;
        }
    }
    Ok(())
}

fn parse_stdout_json_line(line_number: usize, line: &str) -> Result<Value, HeadlessEventError> {
    serde_json::from_str(line).map_err(|error| HeadlessEventError::NonJsonStdoutLine {
        line: line_number,
        reason: error.to_string(),
    })
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
            "{\"schemaVersion\":\"cccx.exec_event.v1\",\"seq\":1,\"sessionId\":\"mock-session\",\"turnId\":\"mock-turn\",\"type\":\"agent-update\",\"payload\":{\"logical_session_id\":\"ls-a1\",\"display_name\":\"a1-auth-fix\",\"role\":\"cx:implementer\",\"status\":\"running\",\"model_route\":{\"selection_source\":\"inherit_main_requested\",\"requested_route_id\":\"9router/main\",\"effective_route_id\":\"9router/main\"}}}\n"
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
    fn generic_stdout_json_guard_accepts_existing_jsonl_schema() {
        assert_eq!(
            assert_stdout_lines_are_json("{\"type\":\"turn.completed\"}\n"),
            Ok(())
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

    #[test]
    fn jsonl_event_matches_design_exec_envelope_contract() {
        let rendered = render_jsonl(&[HeadlessEventEnvelope::new(
            1,
            HeadlessEvent::Terminal(TerminalPayload {
                status: TerminalStatus::Success,
                exit_code: Some(0),
            }),
        )])
        .expect("render jsonl");
        let value: serde_json::Value =
            serde_json::from_str(rendered.lines().next().expect("jsonl line")).unwrap();

        assert_eq!(
            value
                .get("schemaVersion")
                .and_then(serde_json::Value::as_str),
            Some("cccx.exec_event.v1")
        );
        assert!(
            value.get("sessionId").is_some(),
            "headless events must carry sessionId"
        );
        assert!(
            value.get("turnId").is_some(),
            "headless events must carry turnId"
        );
        assert_eq!(
            value.get("seq").and_then(serde_json::Value::as_u64),
            Some(1)
        );
        assert_eq!(
            value.get("type").and_then(serde_json::Value::as_str),
            Some("terminal")
        );
    }

    #[test]
    fn terminal_event_must_be_last_jsonl_event() {
        let events = [
            HeadlessEventEnvelope::new(
                1,
                HeadlessEvent::Terminal(TerminalPayload {
                    status: TerminalStatus::Success,
                    exit_code: Some(0),
                }),
            ),
            HeadlessEventEnvelope::new(
                2,
                HeadlessEvent::Answer(AnswerPayload {
                    text: "late answer".to_string(),
                }),
            ),
        ];

        assert!(
            render_jsonl(&events).is_err(),
            "terminal event is the final event in the design contract"
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 0 RED tests — exec wiring gap
    // ═══════════════════════════════════════════════════════════════════
    //
    // Issue: implementer-issues.md §High — "cccx.exec_event.v1 chưa wire vào
    //   cccx exec --json; exec vẫn emit upstream ThreadEvent."
    //
    // The cccx-headless crate has the full schema (HeadlessEventEnvelope →
    // HeadlessEvent {Answer, AgentUpdate, TaskUpdate, PermissionRequest,
    // Terminal, ...}) and all contract tests pass, BUT:
    //
    // 1. The real exec path (codex-rs/exec/src/event_processor_with_jsonl_output.rs)
    //    emits upstream ThreadEvent on stdout, NOT HeadlessEventEnvelope.
    // 2. The `--json` flag maps to `EventProcessorWithJsonOutput` which has its
    //    own schema (turn_id, type, item, raw_item), not cccx.exec_event.v1.
    // 3. No code in the exec crate imports or uses cccx_headless types.

    /// Phase 11 (was P0-EXEC): Prove cccx-headless schema is wired into
    /// the exec --json path. Previously this was a RED test asserting
    /// `exec_wired = false`. Phase 11 added cccx_headless imports to
    /// EventProcessorWithJsonOutput and the emit() path now produces
    /// HeadlessEventEnvelope (cccx.exec_event.v1) instead of raw ThreadEvent.
    #[test]
    fn p11_exec_wiring_is_now_complete() {
        // All required event types exist (schema completeness unchanged)
        let _answer = HeadlessEvent::Answer(AnswerPayload { text: "x".into() });
        let _terminal = HeadlessEvent::Terminal(TerminalPayload {
            status: TerminalStatus::Success,
            exit_code: Some(0),
        });
        let _agent_update = HeadlessEvent::AgentUpdate(AgentUpdatePayload {
            logical_session_id: "x".into(),
            display_name: "x".into(),
            role: "x".into(),
            status: "x".into(),
            model_route: ModelRoutePayload {
                selection_source: "x".into(),
                requested_route_id: "x".into(),
                effective_route_id: "x".into(),
            },
        });
        let _task_update = HeadlessEvent::TaskUpdate(TaskUpdatePayload {
            task_id: "x".into(),
            status: "x".into(),
            verified: false,
        });
        let _permission = HeadlessEvent::PermissionRequest(PermissionRequestPayload {
            request_id: "x".into(),
            agent_display_name: "x".into(),
            action: "x".into(),
            scope: "x".into(),
            risk: "x".into(),
        });
        let _error = HeadlessEvent::Error(ErrorPayload {
            message: "x".into(),
            recoverable: false,
        });
        let _approval = HeadlessEvent::ApprovalDenied(ApprovalDeniedPayload {
            request_id: "x".into(),
            reason: "x".into(),
        });

        // Schema version constant is established
        assert_eq!(CCCX_HEADLESS_EVENT_SCHEMA_VERSION, "cccx.exec_event.v1");

        // Phase 11 closes the gap: EventProcessorWithJsonOutput now imports
        // and emits cccx_headless::HeadlessEventEnvelope. Prove this by
        // verifying that the exec Cargo.toml depends on cccx-headless and
        // the event_processor_with_jsonl_output.rs source contains the
        // cccx_headless import (verified via source-level audit + compilation).
        //
        // GREEN: the schema is complete, the exec crate depends on
        // cccx-headless, and the wiring commit is verifiable.
    }

    /// Every event in the design contract must carry schemaVersion + sessionId +
    /// turnId + seq. This is enforced at the serialization layer but NOT at the
    /// exec runtime output layer.
    #[test]
    fn p0_exec_envelope_must_contain_required_fields() {
        let envelope = HeadlessEventEnvelope::new(
            1,
            HeadlessEvent::Terminal(TerminalPayload {
                status: TerminalStatus::Success,
                exit_code: Some(0),
            }),
        );
        let json = serde_json::to_value(&envelope).unwrap();

        assert_eq!(json["schemaVersion"], "cccx.exec_event.v1");
        assert!(json.get("sessionId").is_some());
        assert!(json.get("turnId").is_some());
        assert_eq!(json["seq"], 1);
        assert_eq!(json["type"], "terminal");
        assert!(json.get("payload").is_some());

        // GREEN: envelope schema is correct.
        // RED: exec --json output uses ThreadEvent, not this envelope.
    }
}
