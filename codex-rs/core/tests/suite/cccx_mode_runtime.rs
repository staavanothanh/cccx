use anyhow::Result;
use codex_features::Feature;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Settings;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::ThreadSettingsOverrides;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::submit_thread_settings;
use core_test_support::test_codex::local_selections;
use core_test_support::test_codex::test_codex;
use core_test_support::test_codex::turn_permission_fields;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_with_timeout;
use serde_json::json;
use serial_test::serial;
use std::time::Duration;

fn mode(mode: ModeKind, model: String) -> CollaborationMode {
    CollaborationMode {
        mode,
        settings: Settings {
            model,
            reasoning_effort: None,
            developer_instructions: None,
        },
    }
}

async fn build_mode_test(
    server: &wiremock::MockServer,
) -> Result<core_test_support::test_codex::TestCodex> {
    let mut builder = test_codex().with_config(|config| {
        config.model_provider.name = "9router".to_string();
        config
            .features
            .enable(Feature::CollaborationModes)
            .expect("test config should allow CollaborationModes");
    });
    builder.build(server).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn cccx_new_session_uses_effective_configured_startup_mode() -> Result<()> {
    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config.model_provider.name = "9router".to_string();
        config.cccx_default_mode = ModeKind::Auto;
        config
            .features
            .enable(Feature::CollaborationModes)
            .expect("test config should allow CollaborationModes");
    });
    let test = builder.build(&server).await?;
    let call_id = "phase1-configured-auto-shell";
    let args = json!({
        "command": "echo configured-auto-mode > cccx-configured-auto.txt",
        "timeout_ms": 1_000,
    });
    mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-configured-auto-1"),
                ev_function_call(call_id, "shell_command", &serde_json::to_string(&args)?),
                ev_completed("resp-configured-auto-1"),
            ]),
            sse(vec![
                ev_assistant_message("msg-configured-auto-2", "done"),
                ev_completed("resp-configured-auto-2"),
            ]),
        ],
    )
    .await;

    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, test.config.cwd.as_path());
    let submission_id = test
        .codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "configured mode should govern the first turn".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: ThreadSettingsOverrides {
                environments: Some(local_selections(test.config.cwd.clone())),
                approval_policy: Some(AskForApproval::OnRequest),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: None,
                ..Default::default()
            },
        })
        .await?;

    let event = wait_for_submission_event(&test, &submission_id, |event| {
        matches!(event, EventMsg::ExecApprovalRequest(_))
    })
    .await;
    let EventMsg::ExecApprovalRequest(approval) = event else {
        unreachable!("predicate only accepts execution approval requests");
    };
    test.codex
        .submit(Op::ExecApproval {
            id: approval.effective_approval_id(),
            turn_id: None,
            decision: ReviewDecision::Denied,
        })
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    Ok(())
}

async fn wait_for_submission_event(
    test: &core_test_support::test_codex::TestCodex,
    submission_id: &str,
    mut predicate: impl FnMut(&EventMsg) -> bool,
) -> EventMsg {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = test
                .codex
                .next_event()
                .await
                .expect("event stream ended unexpectedly");
            if event.id == submission_id && predicate(&event.msg) {
                return event.msg;
            }
        }
    })
    .await
    .expect("timeout waiting for submission event")
}

async fn submit_shell_turn(
    server: &wiremock::MockServer,
    test: &core_test_support::test_codex::TestCodex,
    prompt: &str,
    command: &str,
    call_id: &str,
    mode_kind: ModeKind,
    approval_policy: AskForApproval,
    permission_profile: PermissionProfile,
) -> Result<String> {
    let args = json!({
        "command": command,
        "timeout_ms": 1_000,
    });
    mount_sse_sequence(
        server,
        vec![
            sse(vec![
                ev_response_created(&format!("resp-{call_id}-1")),
                ev_function_call(call_id, "shell_command", &serde_json::to_string(&args)?),
                ev_completed(&format!("resp-{call_id}-1")),
            ]),
            sse(vec![
                ev_assistant_message(&format!("msg-{call_id}-2"), "done"),
                ev_completed(&format!("resp-{call_id}-2")),
            ]),
        ],
    )
    .await;

    let session_model = test.session_configured.model.clone();
    submit_thread_settings(
        &test.codex,
        ThreadSettingsOverrides {
            collaboration_mode: Some(mode(mode_kind, session_model)),
            ..Default::default()
        },
    )
    .await?;
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(permission_profile, test.config.cwd.as_path());
    let submission_id = test
        .codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: prompt.to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: ThreadSettingsOverrides {
                environments: Some(local_selections(test.config.cwd.clone())),
                approval_policy: Some(approval_policy),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: None,
                ..Default::default()
            },
        })
        .await?;
    Ok(submission_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn cccx_plan_and_dontask_modes_hard_deny_shell_without_native_prompt() -> Result<()> {
    for (mode_kind, prompt, call_id, expected) in [
        (
            ModeKind::Plan,
            "plan mode should not run shell",
            "phase1-plan-deny-shell",
            "plan",
        ),
        (
            ModeKind::DontAsk,
            "dont ask should deny unallowlisted shell",
            "phase1-dontask-deny-shell",
            "dont-ask-no-explicit-allow",
        ),
    ] {
        let server = start_mock_server().await;
        let test = build_mode_test(&server).await?;
        let args = json!({
            "command": "npm test",
            "timeout_ms": 1_000,
        });
        mount_sse_sequence(
            &server,
            vec![
                sse(vec![
                    ev_response_created(&format!("resp-{call_id}-1")),
                    ev_function_call(call_id, "shell_command", &serde_json::to_string(&args)?),
                    ev_completed(&format!("resp-{call_id}-1")),
                ]),
                sse(vec![
                    ev_assistant_message(&format!("msg-{call_id}-2"), "done"),
                    ev_completed(&format!("resp-{call_id}-2")),
                ]),
            ],
        )
        .await;

        let session_model = test.session_configured.model.clone();
        submit_thread_settings(
            &test.codex,
            ThreadSettingsOverrides {
                collaboration_mode: Some(mode(mode_kind, session_model)),
                ..Default::default()
            },
        )
        .await?;
        let (sandbox_policy, permission_profile) =
            turn_permission_fields(PermissionProfile::Disabled, test.config.cwd.as_path());
        let submission_id = test
            .codex
            .submit(Op::UserInput {
                items: vec![UserInput::Text {
                    text: prompt.to_string(),
                    text_elements: Vec::new(),
                }],
                final_output_json_schema: None,
                responsesapi_client_metadata: None,
                additional_context: Default::default(),
                thread_settings: ThreadSettingsOverrides {
                    environments: Some(local_selections(test.config.cwd.clone())),
                    approval_policy: Some(AskForApproval::Never),
                    sandbox_policy: Some(sandbox_policy),
                    permission_profile,
                    collaboration_mode: None,
                    ..Default::default()
                },
            })
            .await?;

        let mut saw_approval = false;
        let mut denial_output = None;
        wait_for_submission_event(&test, &submission_id, |event| {
            saw_approval |= matches!(event, EventMsg::ExecApprovalRequest(_));
            if let EventMsg::ExecCommandEnd(end) = event {
                denial_output = Some(end.aggregated_output.to_lowercase());
            }
            matches!(event, EventMsg::TurnComplete(_))
        })
        .await;
        assert!(
            !saw_approval,
            "mode {mode_kind:?} must hard-deny without native approval prompt"
        );
        let output = denial_output.expect("hard-denied shell must emit an execution result");
        assert!(
            output.contains(expected),
            "mode {mode_kind:?} returned unexpected denial output: {output}"
        );
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn cccx_default_accept_edits_and_auto_modes_request_shell_approval() -> Result<()> {
    for (mode_kind, command, call_id) in [
        (
            ModeKind::Default,
            "echo default-mode-approval > cccx-default-mode.txt",
            "phase1-default-ask-shell",
        ),
        (
            ModeKind::AcceptEdits,
            "echo accept-edits-mode-approval > cccx-accept-mode.txt",
            "phase1-accept-edits-ask-shell",
        ),
        (
            ModeKind::Auto,
            "echo auto-mode-approval > cccx-auto-mode.txt",
            "phase1-auto-ask-shell",
        ),
    ] {
        let server = start_mock_server().await;
        let test = build_mode_test(&server).await?;
        let _submission_id = submit_shell_turn(
            &server,
            &test,
            "mode should ask before shell",
            command,
            call_id,
            mode_kind,
            AskForApproval::OnRequest,
            PermissionProfile::read_only(),
        )
        .await?;

        let approval_event = wait_for_event(&test.codex, |event| {
            matches!(
                event,
                EventMsg::ExecApprovalRequest(_) | EventMsg::TurnComplete(_)
            )
        })
        .await;
        let EventMsg::ExecApprovalRequest(approval) = approval_event else {
            panic!("mode {mode_kind:?} completed without requesting shell approval");
        };
        assert_eq!(approval.command.last().map(String::as_str), Some(command));

        test.codex
            .submit(Op::ExecApproval {
                id: approval.effective_approval_id(),
                turn_id: None,
                decision: ReviewDecision::Denied,
            })
            .await?;
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::TurnComplete(_))
        })
        .await;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn cccx_bypass_mode_without_backend_attestation_is_rejected() -> Result<()> {
    let server = start_mock_server().await;
    let test = build_mode_test(&server).await?;
    let session_model = test.session_configured.model.clone();
    let submission_id = test
        .codex
        .submit(Op::ThreadSettings {
            thread_settings: ThreadSettingsOverrides {
                collaboration_mode: Some(mode(ModeKind::Bypass, session_model)),
                ..Default::default()
            },
        })
        .await?;

    let event = wait_for_event_with_timeout(
        &test.codex,
        |event| {
            matches!(
                event,
                EventMsg::ThreadSettingsApplied(_) | EventMsg::Error(_)
            )
        },
        Duration::from_secs(10),
    )
    .await;
    match event {
        EventMsg::Error(error) if error.message.contains("Bypass") => {}
        EventMsg::Error(error) => panic!("unexpected bypass rejection error: {}", error.message),
        other => {
            panic!("bypass update {submission_id:?} unexpectedly succeeded or emitted {other:?}")
        }
    }

    Ok(())
}
