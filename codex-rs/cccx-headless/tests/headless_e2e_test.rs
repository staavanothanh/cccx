//! Phase 10: E2E integration test for cccx headless output contract.
//!
//! Proves: Scenario 9 (Headless — stdout JSONL-only, terminal event last,
//! sequence monotonic, every event has schemaVersion/sessionId/turnId/seq).

use cccx_headless::{
    AgentUpdatePayload, AnswerPayload, ApprovalDeniedPayload, ErrorPayload, HeadlessEvent,
    HeadlessEventEnvelope, ModelRoutePayload, PermissionRequestPayload, TaskUpdatePayload,
    TerminalPayload, TerminalStatus, assert_stdout_is_jsonl_only, deny_permission_by_default,
    render_jsonl, split_headless_streams,
};

// ── Full stream: events + terminal last + sequence validation ──

#[test]
fn headless_full_stream_renders_and_validates() {
    let envelopes = vec![
        HeadlessEventEnvelope::new(
            1,
            HeadlessEvent::Answer(AnswerPayload {
                text: "Hello, cccx!".to_string(),
            }),
        ),
        HeadlessEventEnvelope::new(
            2,
            HeadlessEvent::TaskUpdate(TaskUpdatePayload {
                task_id: "T1".to_string(),
                status: "in_progress".to_string(),
                verified: false,
            }),
        ),
        HeadlessEventEnvelope::new(
            3,
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
        ),
        HeadlessEventEnvelope::new(
            4,
            HeadlessEvent::TaskUpdate(TaskUpdatePayload {
                task_id: "T1".to_string(),
                status: "verified".to_string(),
                verified: true,
            }),
        ),
        HeadlessEventEnvelope::new(
            5,
            HeadlessEvent::Terminal(TerminalPayload {
                status: TerminalStatus::Success,
                exit_code: Some(0),
            }),
        ),
    ];

    // Render to JSONL
    let jsonl = render_jsonl(&envelopes).expect("render jsonl");
    assert!(!jsonl.is_empty());

    // Split into stdout + stderr
    let streams = split_headless_streams(&envelopes, &["diagnostic: reconnecting".to_string()])
        .expect("split streams");
    assert!(!streams.stdout_jsonl.is_empty());
    assert!(streams.stderr_diagnostics.contains("diagnostic"));

    // Validate stdout is JSONL-only
    assert_stdout_is_jsonl_only(&streams.stdout_jsonl).expect("stdout must be jsonl only");

    // Validate each line parses with correct schema
    for line in streams.stdout_jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let envelope: HeadlessEventEnvelope =
            serde_json::from_str(line).expect("each line must parse as HeadlessEventEnvelope");
        assert_eq!(envelope.schema_version, "cccx.exec_event.v1");
        assert!(!envelope.session_id.is_empty());
        assert!(!envelope.turn_id.is_empty());
        assert!(envelope.seq >= 1);
    }

    // Last non-empty line must be Terminal
    let last_line = streams
        .stdout_jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .last()
        .expect("must have terminal event");
    let last: HeadlessEventEnvelope =
        serde_json::from_str(last_line).expect("last line must parse");
    assert!(
        matches!(last.event, HeadlessEvent::Terminal(_)),
        "terminal event must be last, got {:?}",
        last.event
    );
}

// ── Terminal event not at end → render rejects ──

#[test]
fn render_rejects_terminal_not_last() {
    let envelopes = vec![
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
                text: "after terminal".to_string(),
            }),
        ),
    ];

    assert!(
        render_jsonl(&envelopes).is_err(),
        "terminal event not at end must be rejected"
    );
}

// ── Permission denied by default ──

#[test]
fn headless_denies_permission_by_default() {
    let request = PermissionRequestPayload {
        request_id: "pr-1".to_string(),
        agent_display_name: "a1-auth-fix".to_string(),
        action: "edit".to_string(),
        scope: "src/auth/session.ts".to_string(),
        risk: "medium".to_string(),
    };

    let event = deny_permission_by_default(&request);

    match event {
        HeadlessEvent::ApprovalDenied(denied) => {
            assert_eq!(denied.request_id, "pr-1");
            assert!(denied.reason.contains("deny"));
        }
        other => panic!("expected ApprovalDenied, got {other:?}"),
    }
}

// ── Sequence must be monotonic ──

#[test]
fn headless_sequence_is_monotonic() {
    let envelopes = vec![
        HeadlessEventEnvelope::new(
            1,
            HeadlessEvent::Answer(AnswerPayload {
                text: "one".to_string(),
            }),
        ),
        HeadlessEventEnvelope::new(
            2,
            HeadlessEvent::Answer(AnswerPayload {
                text: "two".to_string(),
            }),
        ),
        HeadlessEventEnvelope::new(
            3,
            HeadlessEvent::Terminal(TerminalPayload {
                status: TerminalStatus::Success,
                exit_code: Some(0),
            }),
        ),
    ];

    let jsonl = render_jsonl(&envelopes).expect("render jsonl");

    let mut prev_seq: u64 = 0;
    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let envelope: HeadlessEventEnvelope = serde_json::from_str(line).expect("valid envelope");
        assert!(
            envelope.seq > prev_seq,
            "seq {} must be > prev {}",
            envelope.seq,
            prev_seq
        );
        prev_seq = envelope.seq;
    }
    assert_eq!(prev_seq, 3);
}

// ── stdout JSONL-only validation ──

#[test]
fn headless_stdout_rejects_non_json_content() {
    // Valid JSONL with correct schema
    assert_stdout_is_jsonl_only(
        "{\"schemaVersion\":\"cccx.exec_event.v1\",\"seq\":1,\"sessionId\":\"s\",\"turnId\":\"t\",\"type\":\"answer\",\"payload\":{\"text\":\"hi\"}}\n",
    )
    .expect("valid jsonl");

    // Wrong schema version
    assert!(
        assert_stdout_is_jsonl_only(
            "{\"schemaVersion\":\"codex.v1\",\"seq\":1,\"sessionId\":\"s\",\"turnId\":\"t\",\"type\":\"answer\",\"payload\":{\"text\":\"hi\"}}\n",
        )
        .is_err(),
        "wrong schema version must be rejected"
    );

    // Non-JSON
    assert!(
        assert_stdout_is_jsonl_only("garbage\n").is_err(),
        "non-JSON must be rejected"
    );
}

// ── Error with non-recoverable flag ──

#[test]
fn headless_error_envelope_renders_correctly() {
    let envelope = HeadlessEventEnvelope::new(
        1,
        HeadlessEvent::Error(ErrorPayload {
            message: "stream disconnected".to_string(),
            recoverable: false,
        }),
    );

    let jsonl = render_jsonl(&[envelope]).expect("render error envelope");
    let parsed: HeadlessEventEnvelope = serde_json::from_str(jsonl.trim()).expect("parse back");

    match parsed.event {
        HeadlessEvent::Error(err) => {
            assert!(!err.recoverable);
            assert!(err.message.contains("disconnected"));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}
