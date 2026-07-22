use anyhow::Context;
use anyhow::Result;
use cccx_policy::PermissionRequest;
use cccx_policy::PermissionRequestStatus;
use cccx_protocol::ArtifactStatus;
use cccx_protocol::EvidenceKind;
use cccx_protocol::ExpectedArtifact;
use cccx_protocol::ForkPolicy;
use cccx_protocol::HarnessKind;
use cccx_protocol::ModelRouteSelection;
use cccx_protocol::PermissionPolicy;
use cccx_protocol::RevisionRef;
use cccx_protocol::WorkRole;
use cccx_protocol::WorkScope;
use cccx_protocol::WorkSpec;
use codex_features::Feature;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Settings;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ThreadSettingsOverrides;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ResponseMock;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_function_call_with_namespace;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once_match;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::local_selections;
use core_test_support::test_codex::test_codex;
use core_test_support::test_codex::turn_permission_fields;
use core_test_support::wait_for_event_with_timeout;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn request_body_contains(request: &wiremock::Request, needle: &str) -> bool {
    String::from_utf8_lossy(&request.body).contains(needle)
}

fn worker_spec(project_root: &Path, index: usize) -> WorkSpec {
    WorkSpec {
        schema_version: cccx_protocol::CCCX_WORK_SPEC_SCHEMA_VERSION.to_string(),
        work_spec_id: format!("ws-permission-{index}"),
        task_id: format!("task-permission-{index}"),
        attempt_id: format!("attempt-permission-{index}"),
        created_at: "2026-07-22T00:00:00Z".to_string(),
        expires_at: None,
        base_revision: RevisionRef {
            git_head: None,
            working_tree_digest: None,
        },
        harness: HarnessKind::Cc,
        role: WorkRole::Researcher,
        runtime_identity: None,
        objective: format!("request one bounded network permission from worker {index}"),
        scope: WorkScope {
            canonical_paths: BTreeSet::from([project_root.to_string_lossy().to_string()]),
            protected_paths: BTreeSet::new(),
        },
        allowed_tools: BTreeSet::from(["request_permissions".to_string()]),
        forbidden_tools: BTreeSet::from(["multi_agent_v1__spawn_agent".to_string()]),
        allowed_mcp_tools: BTreeSet::new(),
        tool_grants: Vec::new(),
        permission_policy: PermissionPolicy::Ask,
        model_route: ModelRouteSelection::InheritMainRequested,
        fallback_policy_id: "strict".to_string(),
        fork_policy: ForkPolicy::WorkSpecOnly,
        enforced_constraints: vec!["permission-queue-runtime-test".to_string()],
        typed_constraints: Vec::new(),
        risk_threshold: None,
        action_classes: None,
        approval_policy: None,
        verification_spec: None,
        audit_depth: None,
        expected_artifact: Some(ExpectedArtifact {
            required_status: ArtifactStatus::Success,
            required_evidence_kinds: vec![EvidenceKind::RuntimeLog],
        }),
    }
}

async fn build_fixture(server: &wiremock::MockServer) -> Result<TestCodex> {
    let mock_base_url = server.uri();
    let mut builder = test_codex().with_config(move |config| {
        config
            .features
            .enable(Feature::Collab)
            .expect("test config should enable native multi-agent tools");
        config
            .features
            .enable(Feature::RequestPermissionsTool)
            .expect("test config should enable request_permissions");
        config.cccx_default_mode = ModeKind::Auto;
        let mut provider = ModelProviderInfo::create_9router_provider();
        provider.base_url = Some(mock_base_url);
        provider.env_key = None;
        provider.experimental_bearer_token = Some("permission-queue-test-token".to_string());
        provider.wire_api = WireApi::Responses;
        provider.supports_websockets = false;
        provider.request_max_retries = Some(0);
        provider.stream_max_retries = Some(0);
        config.model_provider = provider;
    });
    builder.build(server).await
}

async fn submit_parent_spawn(test: &TestCodex, prompt: &str) -> Result<()> {
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::read_only(), test.config.cwd.as_path());
    test.codex
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
                approval_policy: Some(AskForApproval::OnRequest),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Auto,
                    settings: Settings {
                        model: test.session_configured.model.clone(),
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            },
        })
        .await?;
    wait_for_event_with_timeout(
        &test.codex,
        |event| matches!(event, EventMsg::TurnComplete(_)),
        Duration::from_secs(10),
    )
    .await;
    Ok(())
}

async fn spawn_approval_worker(
    test: &TestCodex,
    server: &wiremock::MockServer,
    index: usize,
    created_threads: &mut tokio::sync::broadcast::Receiver<codex_protocol::ThreadId>,
) -> Result<Arc<codex_core::CodexThread>> {
    let parent_prompt = format!("spawn permission worker {index}");
    let child_prompt = format!("permission worker {index} request network permission");
    let spawn_call_id = format!("permission-spawn-{index}");
    let permission_call_id = format!("permission-request-{index}");
    let spawn_args = serde_json::to_string(&json!({
        "message": child_prompt,
        "work_spec": worker_spec(test.cwd_path(), index),
    }))?;

    mount_sse_once_match(
        server,
        {
            let parent_prompt = parent_prompt.clone();
            let spawn_call_id = spawn_call_id.clone();
            move |request: &wiremock::Request| {
                request_body_contains(request, &parent_prompt)
                    && !request_body_contains(request, &spawn_call_id)
            }
        },
        sse(vec![
            ev_response_created(&format!("permission-parent-{index}")),
            ev_function_call_with_namespace(
                &spawn_call_id,
                "multi_agent_v1",
                "spawn_agent",
                &spawn_args,
            ),
            ev_completed(&format!("permission-parent-{index}")),
        ]),
    )
    .await;
    let child_response: ResponseMock = mount_sse_once_match(
        server,
        {
            let child_prompt = child_prompt.clone();
            let spawn_call_id = spawn_call_id.clone();
            move |request: &wiremock::Request| {
                request_body_contains(request, &child_prompt)
                    && !request_body_contains(request, &spawn_call_id)
            }
        },
        sse(vec![
            ev_response_created(&format!("permission-child-{index}")),
            ev_function_call(
                &permission_call_id,
                "request_permissions",
                &serde_json::to_string(&json!({
                    "reason": format!("permission worker {index} needs network"),
                    "permissions": {
                        "network": { "enabled": true }
                    }
                }))?,
            ),
            ev_completed(&format!("permission-child-{index}")),
        ]),
    )
    .await;
    mount_sse_once_match(
        server,
        {
            let spawn_call_id = spawn_call_id.clone();
            move |request: &wiremock::Request| request_body_contains(request, &spawn_call_id)
        },
        sse(vec![
            ev_response_created(&format!("permission-parent-follow-up-{index}")),
            ev_assistant_message(
                &format!("permission-parent-message-{index}"),
                "worker started",
            ),
            ev_completed(&format!("permission-parent-follow-up-{index}")),
        ]),
    )
    .await;

    submit_parent_spawn(test, &parent_prompt).await?;
    let thread_id = tokio::time::timeout(Duration::from_secs(10), created_threads.recv())
        .await
        .context("timed out waiting for permission worker")?
        .context("thread-created channel closed")?;
    let child = test.thread_manager.get_thread(thread_id).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if child_response.last_request().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("timed out waiting for worker model request")?;
    Ok(child)
}

async fn wait_for_pending(test: &TestCodex, count: usize) -> Result<Vec<PermissionRequest>> {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = test
                .codex
                .list_cccx_permissions()
                .await
                .into_iter()
                .filter(|request| {
                    matches!(
                        request.status,
                        PermissionRequestStatus::Active | PermissionRequestStatus::Queued
                    )
                })
                .collect::<Vec<_>>();
            if pending.len() == count {
                return pending;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    match result {
        Ok(pending) => Ok(pending),
        Err(error) => {
            let snapshot = test.codex.list_cccx_permissions().await;
            Err(anyhow::Error::new(error).context(format!(
                "timed out waiting for {count} central permission request(s); snapshot={snapshot:?}"
            )))
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_workers_share_one_active_permission_prompt_and_exact_request_ids() -> Result<()> {
    let server = start_mock_server().await;
    let test = build_fixture(&server).await?;
    let mut created_threads = test.thread_manager.subscribe_thread_created();

    let first = spawn_approval_worker(&test, &server, 1, &mut created_threads).await?;
    let first_request = wait_for_pending(&test, 1).await?.remove(0);
    assert_eq!(first_request.status, PermissionRequestStatus::Active);

    let second = spawn_approval_worker(&test, &server, 2, &mut created_threads).await?;
    let pending = wait_for_pending(&test, 2).await?;
    let active = pending
        .iter()
        .find(|request| request.status == PermissionRequestStatus::Active)
        .context("one request must be active")?;
    let queued = pending
        .iter()
        .find(|request| request.status == PermissionRequestStatus::Queued)
        .context("second request must remain queued")?;
    assert_eq!(active.request_id, first_request.request_id);
    assert_ne!(active.agent_id, queued.agent_id);

    let first_event = wait_for_event_with_timeout(
        &first,
        |event| matches!(event, EventMsg::RequestPermissions(_)),
        Duration::from_secs(20),
    )
    .await;
    assert!(matches!(first_event, EventMsg::RequestPermissions(_)));
    let second_prompt_before_promotion = tokio::time::timeout(
        Duration::from_millis(300),
        wait_for_event_with_timeout(
            &second,
            |event| matches!(event, EventMsg::RequestPermissions(_)),
            Duration::from_secs(10),
        ),
    )
    .await;
    assert!(
        second_prompt_before_promotion.is_err(),
        "queued worker must not render a second active permission prompt"
    );

    test.codex
        .resolve_cccx_permission(&active.request_id, false)
        .await
        .map_err(anyhow::Error::msg)?;
    let second_event = wait_for_event_with_timeout(
        &second,
        |event| matches!(event, EventMsg::RequestPermissions(_)),
        Duration::from_secs(20),
    )
    .await;
    assert!(matches!(second_event, EventMsg::RequestPermissions(_)));

    let promoted = wait_for_pending(&test, 1).await?.remove(0);
    assert_eq!(promoted.request_id, queued.request_id);
    assert_eq!(promoted.status, PermissionRequestStatus::Active);
    assert!(
        test.codex
            .resolve_cccx_permission("cccx-perm-does-not-exist", true)
            .await
            .is_err(),
        "unknown/stale request IDs must fail closed"
    );
    test.codex
        .resolve_cccx_permission(&promoted.request_id, false)
        .await
        .map_err(anyhow::Error::msg)?;

    Ok(())
}
