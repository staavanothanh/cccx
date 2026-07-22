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
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::TestCodex;
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
use std::time::Instant;

const PARENT_AUTHORITY_PROMPT: &str = "phase1 authority runtime parent";
const CHILD_AUTHORITY_PROMPT: &str = "phase1 authority runtime child";
const SPAWN_CALL_ID: &str = "phase1-authority-spawn";
const GRANDCHILD_CALL_ID: &str = "phase1-authority-grandchild";
const ADR_CALL_ID: &str = "phase1-authority-child-adr";
const WORK_SPEC_ID: &str = "ws-phase1-authority";
const TASK_ID: &str = "task-phase1-authority";

fn authority_work_spec(project_root: &Path, objective: &str) -> WorkSpec {
    authority_work_spec_with_harness(
        project_root,
        objective,
        HarnessKind::Cx,
        WorkRole::Implementer,
    )
}

fn authority_work_spec_with_harness(
    project_root: &Path,
    objective: &str,
    harness: HarnessKind,
    role: WorkRole,
) -> WorkSpec {
    WorkSpec {
        schema_version: cccx_protocol::CCCX_WORK_SPEC_SCHEMA_VERSION.to_string(),
        work_spec_id: WORK_SPEC_ID.to_string(),
        task_id: TASK_ID.to_string(),
        attempt_id: "attempt-phase1-authority".to_string(),
        created_at: "2026-07-21T00:00:00Z".to_string(),
        expires_at: None,
        base_revision: RevisionRef {
            git_head: None,
            working_tree_digest: None,
        },
        harness,
        role,
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
        enforced_constraints: vec!["phase1-authority-worker".to_string()],
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

async fn build_authority_fixture(server: &wiremock::MockServer) -> Result<TestCodex> {
    let mock_base_url = server.uri();
    let mut builder = test_codex().with_config(move |config| {
        config
            .features
            .enable(Feature::Collab)
            .expect("test config should enable native multi-agent tools");
        let mut provider = ModelProviderInfo::create_9router_provider();
        provider.base_url = Some(format!("{}/v1", mock_base_url.trim_end_matches('/')));
        provider.env_key = None;
        provider.experimental_bearer_token = Some("phase1-authority-test-token".to_string());
        provider.wire_api = WireApi::Responses;
        provider.supports_websockets = false;
        provider.request_max_retries = Some(0);
        provider.stream_max_retries = Some(0);
        config.model_provider = provider;
    });
    builder.build(server).await
}

async fn submit_default_disabled_turn(test: &TestCodex, prompt: &str) -> Result<()> {
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

async fn spawn_authority_child(
    test: &TestCodex,
    server: &wiremock::MockServer,
    child_stream: Vec<serde_json::Value>,
    created_threads: &mut tokio::sync::broadcast::Receiver<codex_protocol::ThreadId>,
) -> Result<(codex_protocol::ThreadId, ResponsesRequest)> {
    let work_spec = authority_work_spec(test.cwd_path(), "prove child authority is bounded");
    spawn_authority_child_with_work_spec(test, server, work_spec, child_stream, created_threads)
        .await
}

async fn spawn_authority_child_with_work_spec(
    test: &TestCodex,
    server: &wiremock::MockServer,
    work_spec: WorkSpec,
    child_stream: Vec<serde_json::Value>,
    created_threads: &mut tokio::sync::broadcast::Receiver<codex_protocol::ThreadId>,
) -> Result<(codex_protocol::ThreadId, ResponsesRequest)> {
    let spawn_args = serde_json::to_string(&json!({
        "message": CHILD_AUTHORITY_PROMPT,
        "work_spec": work_spec,
    }))?;

    mount_sse_once_match(
        server,
        |request: &wiremock::Request| request_body_contains(request, PARENT_AUTHORITY_PROMPT),
        sse(vec![
            ev_response_created("phase1-authority-parent-spawn"),
            ev_function_call_with_namespace(
                SPAWN_CALL_ID,
                "multi_agent_v1",
                "spawn_agent",
                &spawn_args,
            ),
            ev_completed("phase1-authority-parent-spawn"),
        ]),
    )
    .await;
    let child_first_request = mount_sse_once_match(
        server,
        |request: &wiremock::Request| {
            request_body_contains(request, CHILD_AUTHORITY_PROMPT)
                && !request_body_contains(request, SPAWN_CALL_ID)
        },
        sse(child_stream),
    )
    .await;
    mount_sse_once_match(
        server,
        |request: &wiremock::Request| request_body_contains(request, SPAWN_CALL_ID),
        sse(vec![
            ev_response_created("phase1-authority-parent-follow-up"),
            ev_assistant_message("phase1-authority-parent-follow-up-msg", "child started"),
            ev_completed("phase1-authority-parent-follow-up"),
        ]),
    )
    .await;

    submit_default_disabled_turn(test, PARENT_AUTHORITY_PROMPT).await?;
    let child_thread_id = tokio::time::timeout(Duration::from_secs(10), created_threads.recv())
        .await
        .context("timed out waiting for spawned authority child")?
        .context("thread-created channel closed")?;
    let child_request = wait_for_captured_request(&child_first_request).await;
    Ok((child_thread_id, child_request))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn spawned_cc_injected_spawn_is_denied_at_runtime() -> Result<()> {
    let server = start_mock_server().await;
    let test = build_authority_fixture(&server).await?;
    let work_spec = authority_work_spec_with_harness(
        test.cwd_path(),
        "prove CC child authority is bounded",
        HarnessKind::Cc,
        WorkRole::Planner,
    );
    let injected_spawn_args = serde_json::to_string(&json!({
        "message": "grandchild should never start",
        "work_spec": authority_work_spec(test.cwd_path(), "grandchild injection attempt"),
    }))?;

    let mut created_threads = test.thread_manager.subscribe_thread_created();
    let (child_thread_id, child_request) = spawn_authority_child_with_work_spec(
        &test,
        &server,
        work_spec,
        vec![
            ev_response_created("phase1-authority-cc-child-injected"),
            ev_function_call_with_namespace(
                GRANDCHILD_CALL_ID,
                "multi_agent_v1",
                "spawn_agent",
                &injected_spawn_args,
            ),
            ev_completed("phase1-authority-cc-child-injected"),
        ],
        &mut created_threads,
    )
    .await?;

    assert!(
        child_request
            .tool_by_name("multi_agent_v1", "spawn_agent")
            .is_none(),
        "CC child must not see scheduler/spawn in its model-visible catalog"
    );
    let no_grandchild =
        tokio::time::timeout(Duration::from_millis(300), created_threads.recv()).await;
    assert!(
        no_grandchild.is_err(),
        "injected CC child-side spawn unexpectedly created a grandchild thread"
    );

    let child = test.thread_manager.get_thread(child_thread_id).await?;
    let child_config = child.config().await;
    assert_eq!(child_config.cccx_actor_identity, ActorIdentity::CcSubagent);
    assert_eq!(child_config.cccx_harness, HarnessKind::Cc);
    child.submit(Op::Interrupt).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn spawned_cx_injected_spawn_and_reroute_are_denied_at_runtime() -> Result<()> {
    let server = start_mock_server().await;
    let test = build_authority_fixture(&server).await?;
    let injected_spawn_args = serde_json::to_string(&json!({
        "message": "grandchild should never start",
        "work_spec": authority_work_spec(test.cwd_path(), "grandchild injection attempt"),
    }))?;

    let mut created_threads = test.thread_manager.subscribe_thread_created();
    let (child_thread_id, child_request) = spawn_authority_child(
        &test,
        &server,
        vec![
            ev_response_created("phase1-authority-child-injected"),
            ev_function_call_with_namespace(
                GRANDCHILD_CALL_ID,
                "multi_agent_v1",
                "spawn_agent",
                &injected_spawn_args,
            ),
            ev_completed("phase1-authority-child-injected"),
        ],
        &mut created_threads,
    )
    .await?;
    assert!(
        child_request
            .tool_by_name("multi_agent_v1", "spawn_agent")
            .is_none(),
        "CX child must not see scheduler/spawn in its model-visible catalog"
    );

    let no_grandchild =
        tokio::time::timeout(Duration::from_millis(300), created_threads.recv()).await;
    assert!(
        no_grandchild.is_err(),
        "injected child-side spawn unexpectedly created a grandchild thread"
    );

    let child = test.thread_manager.get_thread(child_thread_id).await?;
    let child_config = child.config().await;
    assert_eq!(child_config.cccx_actor_identity, ActorIdentity::CxSubagent);
    assert_eq!(child_config.cccx_harness, HarnessKind::Cx);
    test.codex
        .set_cccx_agent_model_route("cx_implementer", "oc/deepseek-v4-flash-free")
        .await
        .expect("MainCc route configuration should be allowed");
    let result = child
        .set_cccx_agent_model_route("cx_implementer", "oc/child-forbidden-route")
        .await;
    assert!(
        result.is_err(),
        "CX child must not be able to mutate agent model-route/reroute state"
    );

    wait_for_event_with_timeout(
        &child,
        |event| matches!(event, EventMsg::TurnComplete(_) | EventMsg::TurnAborted(_)),
        Duration::from_secs(10),
    )
    .await;
    child.submit(Op::Interrupt).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn spawned_cx_adr_commit_returns_structured_proposal_without_mcp_transport() -> Result<()> {
    let server = start_mock_server().await;
    let test = build_authority_fixture(&server).await?;
    let adr_args = serde_json::to_string(&json!({
        "title": "Keep 9router as the only model backend",
        "decision": "Child agents may propose ADRs but must not commit them",
    }))?;

    let _child_follow_up = mount_sse_once_match(
        &server,
        |request: &wiremock::Request| {
            request_body_contains(request, "You are a bounded CX worker for cccx.")
                && request_body_contains(request, ADR_CALL_ID)
        },
        sse(vec![
            ev_response_created("phase1-authority-child-adr-follow-up"),
            ev_assistant_message(
                "phase1-authority-child-adr-follow-up-msg",
                "ADR proposal returned to Main CC",
            ),
            ev_completed("phase1-authority-child-adr-follow-up"),
        ]),
    )
    .await;

    let mut created_threads = test.thread_manager.subscribe_thread_created();
    let (child_thread_id, child_request) = spawn_authority_child(
        &test,
        &server,
        vec![
            ev_response_created("phase1-authority-child-adr"),
            ev_function_call_with_namespace(
                ADR_CALL_ID,
                "mcp__codebase_memory_mcp",
                "manage_adr",
                &adr_args,
            ),
            ev_completed("phase1-authority-child-adr"),
        ],
        &mut created_threads,
    )
    .await?;

    assert!(
        child_request
            .tool_by_name("mcp__codebase_memory_mcp", "manage_adr")
            .is_none(),
        "CX child must not receive direct model-visible ADR commit capability"
    );

    let child_thread_id_string = child_thread_id.to_string();
    let deadline = Instant::now() + Duration::from_secs(10);
    let follow_up_body = loop {
        let requests = server.received_requests().await.unwrap_or_default();
        if let Some(body) = requests.into_iter().find_map(|request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).ok()?;
            let body_text = body.to_string();
            (body["client_metadata"]["thread_id"] == child_thread_id_string
                && body_text.contains(ADR_CALL_ID)
                && body_text.contains("cccx.adr_proposal.v1"))
            .then_some(body_text)
        }) {
            break body;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the spawned child ADR proposal follow-up"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(
        follow_up_body.contains("cccx.adr_proposal.v1"),
        "child follow-up must carry the structured ADR proposal: {follow_up_body}"
    );
    assert!(
        follow_up_body.contains("cx_subagent")
            && follow_up_body.contains("codebase_memory_mcp")
            && follow_up_body.contains("manage_adr")
            && follow_up_body.contains("Keep 9router as the only model backend"),
        "ADR proposal lost authority or argument fields before the model follow-up: {follow_up_body}"
    );

    let child = test.thread_manager.get_thread(child_thread_id).await?;
    child.submit(Op::Interrupt).await?;
    Ok(())
}
