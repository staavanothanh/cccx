use anyhow::Context;
use anyhow::Result;
use cccx_protocol::ActorIdentity;
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
use cccx_task_graph::WorkerStatus;
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
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call_with_namespace;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once_match;
use core_test_support::responses::sse;
use core_test_support::responses::sse_failed;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::local_selections;
use core_test_support::test_codex::test_codex;
use core_test_support::test_codex::turn_permission_fields;
use core_test_support::wait_for_event_with_timeout;
use pretty_assertions::assert_eq;
use serde_json::json;
use serial_test::serial;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

const PARENT_RESUME_PROMPT: &str = "spawn phase1 identity worker and leave it resumable";
const CHILD_RESUME_PROMPT: &str = "phase1 child must checkpoint before interruption";
const RESUME_CALL_ID: &str = "phase1-resume-spawn";
const WORK_SPEC_ID: &str = "ws-phase1-identity";
const TASK_ID: &str = "task-phase1-identity";

const PARENT_DENIAL_PROMPT: &str = "spawn phase1 authority denial worker";
const CHILD_DENIAL_PROMPT: &str = "phase1 child must not spawn a grandchild";
const DENIAL_CALL_ID: &str = "phase1-denial-spawn";
const INJECTED_GRANDCHILD_CALL_ID: &str = "phase1-grandchild-injected";

fn cx_work_spec(project_root: &Path, objective: &str) -> WorkSpec {
    WorkSpec {
        schema_version: cccx_protocol::CCCX_WORK_SPEC_SCHEMA_VERSION.to_string(),
        work_spec_id: WORK_SPEC_ID.to_string(),
        task_id: TASK_ID.to_string(),
        attempt_id: "attempt-phase1-identity".to_string(),
        created_at: "2026-07-21T00:00:00Z".to_string(),
        expires_at: None,
        base_revision: RevisionRef {
            git_head: None,
            working_tree_digest: None,
        },
        harness: HarnessKind::Cx,
        role: WorkRole::Implementer,
        runtime_identity: None,
        objective: objective.to_string(),
        scope: WorkScope {
            canonical_paths: BTreeSet::from([project_root.to_string_lossy().to_string()]),
            protected_paths: BTreeSet::new(),
            advisory_globs: BTreeSet::new(),
        },
        allowed_tools: BTreeSet::from(["shell_command".to_string()]),
        forbidden_tools: BTreeSet::from(["multi_agent_v1__spawn_agent".to_string()]),
        allowed_mcp_tools: BTreeSet::new(),
        tool_grants: Vec::new(),
        permission_policy: PermissionPolicy::Allow,
        model_route: ModelRouteSelection::InheritMainRequested,
        resolved_model_route: None,
        fallback_policy_id: "strict".to_string(),
        fork_policy: ForkPolicy::WorkSpecOnly,
        enforced_constraints: vec!["phase1-bounded-worker".to_string()],
        typed_constraints: Vec::new(),
        advisory_constraints: Vec::new(),
        risk_threshold: None,
        action_classes: None,
        approval_policy: None,
        verification_spec: None,
        audit_depth: None,
        expected_artifact: Some(ExpectedArtifact {
            required_status: ArtifactStatus::Success,
            required_evidence_kinds: vec![EvidenceKind::RuntimeLog],
            required_fields: Vec::new(),
            verification_policy_id: None,
        }),
        checkpoint_id: None,
        base_commit: None,
    }
}

fn request_body_contains(request: &wiremock::Request, needle: &str) -> bool {
    String::from_utf8_lossy(&request.body).contains(needle)
}

async fn wait_for_captured_request(mock_response: &ResponseMock) -> ResponsesRequest {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(request) = mock_response.last_request() {
                break request;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed out waiting for mock request")
}

async fn wait_until_worker_is_resumable(project_root: &Path) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let graph = codex_core::cccx_task_graph_state::load_task_graph(project_root)
                .await
                .expect("load persisted CCCX TaskGraph")
                .expect("spawn must persist a CCCX TaskGraph");
            if graph.tasks[TASK_ID]
                .attempts
                .last()
                .is_some_and(|attempt| attempt.worker.status == WorkerStatus::Resumable)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("timed out waiting for interrupted worker to become resumable")?;
    Ok(())
}

async fn build_cccx_collab_fixture(
    server: &wiremock::MockServer,
) -> Result<core_test_support::test_codex::TestCodex> {
    let mock_base_url = server.uri();
    let mut builder = test_codex().with_config(move |config| {
        config
            .features
            .enable(Feature::Collab)
            .expect("test config should enable native multi-agent tools");
        let mut provider = ModelProviderInfo::create_9router_provider();
        provider.base_url = Some(mock_base_url);
        provider.env_key = None;
        provider.experimental_bearer_token = Some("phase1-identity-test-token".to_string());
        provider.wire_api = WireApi::Responses;
        provider.supports_websockets = false;
        provider.request_max_retries = Some(0);
        provider.stream_max_retries = Some(0);
        config.model_provider = provider;
    });
    builder.build(server).await
}

async fn submit_default_disabled_turn(
    test: &core_test_support::test_codex::TestCodex,
    prompt: &str,
) -> Result<()> {
    let session_model = test.session_configured.model.clone();
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, test.config.cwd.as_path());
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
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Default,
                    settings: Settings {
                        model: session_model,
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            },
        })
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn spawned_cx_checkpoint_resume_preserves_typed_identity_and_single_harness() -> Result<()> {
    let server = start_mock_server().await;
    let test = build_cccx_collab_fixture(&server).await?;
    let work_spec = cx_work_spec(test.cwd_path(), "prove runtime identity survives resume");
    let spawn_args = serde_json::to_string(&json!({
        "message": CHILD_RESUME_PROMPT,
        "work_spec": work_spec,
    }))?;

    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_body_contains(request, PARENT_RESUME_PROMPT),
        sse(vec![
            ev_response_created("phase1-parent-spawn"),
            ev_function_call_with_namespace(
                RESUME_CALL_ID,
                "multi_agent_v1",
                "spawn_agent",
                &spawn_args,
            ),
            ev_completed("phase1-parent-spawn"),
        ]),
    )
    .await;
    let child_first_request = mount_sse_once_match(
        &server,
        |request: &wiremock::Request| {
            request_body_contains(request, CHILD_RESUME_PROMPT)
                && !request_body_contains(request, RESUME_CALL_ID)
        },
        sse_failed(
            "phase1-child-interrupted",
            "provider_disconnected",
            "synthetic child disconnect",
        ),
    )
    .await;
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_body_contains(request, RESUME_CALL_ID),
        sse(vec![
            ev_response_created("phase1-parent-spawn-follow-up"),
            ev_assistant_message(
                "phase1-parent-spawn-follow-up-msg",
                "identity worker started",
            ),
            ev_completed("phase1-parent-spawn-follow-up"),
        ]),
    )
    .await;

    let mut created_threads = test.thread_manager.subscribe_thread_created();
    submit_default_disabled_turn(&test, PARENT_RESUME_PROMPT).await?;
    let child_thread_id = tokio::time::timeout(Duration::from_secs(10), created_threads.recv())
        .await
        .context("timed out waiting for spawned child thread")?
        .context("thread-created channel closed")?;
    let child = test.thread_manager.get_thread(child_thread_id).await?;
    let child_config = child.config().await;
    assert_eq!(child_config.cccx_actor_identity, ActorIdentity::CxSubagent);
    assert_eq!(child_config.cccx_harness, HarnessKind::Cx);
    assert_eq!(
        child_config.cccx_child_allowed_tools,
        BTreeSet::from(["shell_command".to_string()])
    );

    let child_request = wait_for_captured_request(&child_first_request).await;
    assert!(
        child_request
            .tool_by_name("multi_agent_v1", "spawn_agent")
            .is_none(),
        "spawn must be proactively absent from a CX child model-visible catalog"
    );
    assert!(
        child_request.tool_by_name("", "shell_command").is_some()
            || child_request.body_contains_text("shell_command"),
        "child catalog/request should still expose the one WorkSpec-granted shell tool"
    );

    wait_for_event_with_timeout(
        &child,
        |event| matches!(event, EventMsg::TurnComplete(_) | EventMsg::TurnAborted(_)),
        Duration::from_secs(10),
    )
    .await;
    wait_until_worker_is_resumable(test.cwd_path()).await?;

    let graph = codex_core::cccx_task_graph_state::load_task_graph(test.cwd_path())
        .await?
        .context("spawn must persist a CCCX TaskGraph")?;
    let attempt = &graph.tasks[TASK_ID].attempts[0];
    assert_eq!(attempt.worker.logical_session_id, WORK_SPEC_ID);
    assert_eq!(attempt.attempt_id, "attempt-phase1-identity");
    assert_eq!(
        attempt.worker.worker_incarnation_id,
        child_thread_id.to_string()
    );

    test.codex.submit(Op::Interrupt).await?;
    wait_for_event_with_timeout(
        &test.codex,
        |event| matches!(event, EventMsg::TurnAborted(_) | EventMsg::TurnComplete(_)),
        Duration::from_secs(10),
    )
    .await;

    let resumed_child_request = mount_sse_once_match(
        &server,
        |request: &wiremock::Request| {
            request_body_contains(request, "Resume the existing CCCX worker")
        },
        sse(vec![
            ev_response_created("phase1-resumed-child"),
            ev_assistant_message("phase1-resumed-child-msg", "resumed child done"),
            ev_completed("phase1-resumed-child"),
        ]),
    )
    .await;
    let resumed_thread_id = test
        .codex
        .resume_cccx_worker(WORK_SPEC_ID, true)
        .await
        .map_err(anyhow::Error::msg)?;
    assert_ne!(
        resumed_thread_id, child_thread_id,
        "resume must create a fresh worker incarnation"
    );
    let resumed = test.thread_manager.get_thread(resumed_thread_id).await?;
    let resumed_config = resumed.config().await;
    assert_eq!(
        resumed_config.cccx_actor_identity,
        ActorIdentity::CxSubagent
    );
    assert_eq!(resumed_config.cccx_harness, HarnessKind::Cx);
    assert_eq!(
        resumed_config.cccx_child_allowed_tools,
        BTreeSet::from(["shell_command".to_string()]),
        "resume must re-apply the pinned WorkSpec projection, not inherit root authority"
    );
    let resumed_request = wait_for_captured_request(&resumed_child_request).await;
    assert!(resumed_request.body_contains_text("cccx.spawn_input.v1"));
    assert!(resumed_request.body_contains_text("cccx.work_spec.v1"));
    assert!(resumed_request.body_contains_text("checkpoint-"));
    assert!(resumed_request.body_contains_text("attempt-phase1-identity"));
    assert!(
        !resumed_request.body_contains_text(PARENT_RESUME_PROMPT),
        "resume must not fork the raw parent history into the child"
    );
    wait_for_event_with_timeout(
        &resumed,
        |event| matches!(event, EventMsg::TurnComplete(_)),
        Duration::from_secs(10),
    )
    .await;
    let resumed_graph = codex_core::cccx_task_graph_state::load_task_graph(test.cwd_path())
        .await?
        .context("resume must preserve the CCCX TaskGraph")?;
    let attempts = &resumed_graph.tasks[TASK_ID].attempts;
    assert_eq!(attempts.len(), 1, "resume must not create a second attempt");
    assert_eq!(attempts[0].attempt_id, "attempt-phase1-identity");
    assert_eq!(attempts[0].worker.logical_session_id, WORK_SPEC_ID);
    assert_eq!(
        attempts[0].worker.worker_incarnation_id,
        resumed_thread_id.to_string()
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn spawned_cx_hides_spawn_catalog_and_injected_spawn_fails_closed() -> Result<()> {
    let server = start_mock_server().await;
    let test = build_cccx_collab_fixture(&server).await?;
    let work_spec = cx_work_spec(test.cwd_path(), "prove child cannot spawn a grandchild");
    let spawn_args = serde_json::to_string(&json!({
        "message": CHILD_DENIAL_PROMPT,
        "work_spec": work_spec,
    }))?;
    let injected_spawn_args = serde_json::to_string(&json!({
        "message": "grandchild should never start",
        "work_spec": cx_work_spec(test.cwd_path(), "grandchild injection attempt"),
    }))?;

    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_body_contains(request, PARENT_DENIAL_PROMPT),
        sse(vec![
            ev_response_created("phase1-denial-parent-spawn"),
            ev_function_call_with_namespace(
                DENIAL_CALL_ID,
                "multi_agent_v1",
                "spawn_agent",
                &spawn_args,
            ),
            ev_completed("phase1-denial-parent-spawn"),
        ]),
    )
    .await;
    let child_first_request = mount_sse_once_match(
        &server,
        |request: &wiremock::Request| {
            request_body_contains(request, CHILD_DENIAL_PROMPT)
                && !request_body_contains(request, DENIAL_CALL_ID)
        },
        sse(vec![
            ev_response_created("phase1-denial-child-injected"),
            ev_function_call_with_namespace(
                INJECTED_GRANDCHILD_CALL_ID,
                "multi_agent_v1",
                "spawn_agent",
                &injected_spawn_args,
            ),
            ev_completed("phase1-denial-child-injected"),
        ]),
    )
    .await;
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_body_contains(request, DENIAL_CALL_ID),
        sse(vec![
            ev_response_created("phase1-denial-parent-follow-up"),
            ev_assistant_message(
                "phase1-denial-parent-follow-up-msg",
                "authority denial worker started",
            ),
            ev_completed("phase1-denial-parent-follow-up"),
        ]),
    )
    .await;
    let mut created_threads = test.thread_manager.subscribe_thread_created();
    submit_default_disabled_turn(&test, PARENT_DENIAL_PROMPT).await?;
    let child_thread_id = tokio::time::timeout(Duration::from_secs(10), created_threads.recv())
        .await
        .context("timed out waiting for spawned denial child")?
        .context("thread-created channel closed")?;
    let child = test.thread_manager.get_thread(child_thread_id).await?;
    let child_config = child.config().await;
    assert_eq!(child_config.cccx_actor_identity, ActorIdentity::CxSubagent);
    assert_eq!(child_config.cccx_harness, HarnessKind::Cx);

    let child_request = wait_for_captured_request(&child_first_request).await;
    assert!(
        child_request
            .tool_by_name("multi_agent_v1", "spawn_agent")
            .is_none(),
        "CX child must not see the spawn tool in its model-visible catalog"
    );

    let no_grandchild =
        tokio::time::timeout(Duration::from_millis(300), created_threads.recv()).await;
    assert!(
        no_grandchild.is_err(),
        "injected child-side spawn unexpectedly created a grandchild thread"
    );

    // The child/parent turns may already have consumed terminal events after
    // the child-side injected call is rejected. Cleanup must not turn that
    // benign race into a gate failure after the authority invariant is proven.
    child.submit(Op::Interrupt).await?;
    test.codex.submit(Op::Interrupt).await?;

    Ok(())
}
