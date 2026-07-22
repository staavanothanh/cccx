use anyhow::Result;
use cccx_mode::CccxModeState;
use cccx_policy::MainMode;
use cccx_protocol::ActorIdentity;
use cccx_protocol::ArtifactStatus;
use cccx_protocol::EvidenceKind;
use cccx_protocol::ExpectedArtifact;
use cccx_protocol::ForkPolicy;
use cccx_protocol::HarnessKind;
use cccx_protocol::ModelRouteSelection;
use cccx_protocol::ParentCapabilitySet;
use cccx_protocol::PermissionPolicy;
use cccx_protocol::RevisionRef;
use cccx_protocol::ToolGrant;
use cccx_protocol::WorkRole;
use cccx_protocol::WorkScope;
use cccx_protocol::WorkSpec;
use codex_core::cccx_spawn::apply_cccx_harness_to_config;
use codex_core::config::Config;
use codex_features::Feature;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call_with_namespace;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once_match;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event_with_timeout;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

const SPAWN_CALL_ID: &str = "cccx-final-gate-spawn";
const SPAWN_PARENT_PROMPT: &str = "spawn the final-gate CX worker";
const SPAWN_CHILD_PROMPT: &str = "perform the bounded final-gate child task";

fn test_work_spec(harness: HarnessKind, role: WorkRole) -> WorkSpec {
    WorkSpec {
        schema_version: cccx_protocol::CCCX_WORK_SPEC_SCHEMA_VERSION.to_string(),
        work_spec_id: "ws-final-gate-1".to_string(),
        task_id: "task-final-gate-1".to_string(),
        attempt_id: "attempt-final-gate-1".to_string(),
        created_at: "2026-07-21T00:00:00Z".to_string(),
        expires_at: Some("2026-07-21T01:00:00Z".to_string()),
        base_revision: RevisionRef {
            git_head: Some("abc123".to_string()),
            working_tree_digest: Some("sha256:tree".to_string()),
        },
        harness,
        role,
        runtime_identity: None,
        objective: "prove child projection through public core config API".to_string(),
        scope: WorkScope {
            canonical_paths: BTreeSet::from(["D:/workspace/project/src".to_string()]),
            protected_paths: BTreeSet::from(["D:/workspace/project/src/protected".to_string()]),
        },
        allowed_tools: BTreeSet::from(["shell_command".to_string(), "apply_patch".to_string()]),
        forbidden_tools: BTreeSet::from(["tool_search".to_string()]),
        allowed_mcp_tools: BTreeSet::from([
            "mcp:codebase-memory.search_graph".to_string(),
            "mcp:codebase-memory.trace_path".to_string(),
        ]),
        tool_grants: vec![ToolGrant {
            stable_id: "mcp:codebase-memory.search_graph".to_string(),
            server_id: Some("codebase-memory".to_string()),
            schema_digest: Some("sha256:schema".to_string()),
        }],
        permission_policy: PermissionPolicy::Ask,
        model_route: ModelRouteSelection::Explicit {
            route_id: "route/cx-test".to_string(),
        },
        fallback_policy_id: "fail-closed".to_string(),
        fork_policy: ForkPolicy::WorkSpecOnly,
        enforced_constraints: vec!["bounded-scope".to_string()],
        typed_constraints: Vec::new(),
        risk_threshold: None,
        action_classes: None,
        approval_policy: None,
        verification_spec: None,
        audit_depth: None,
        expected_artifact: Some(ExpectedArtifact {
            required_status: ArtifactStatus::Success,
            required_evidence_kinds: vec![EvidenceKind::TestLog],
        }),
    }
}

fn parent_capabilities() -> ParentCapabilitySet {
    ParentCapabilitySet {
        tools: BTreeSet::from([
            "shell_command".to_string(),
            "apply_patch".to_string(),
            "spawn_agent".to_string(),
        ]),
        mcp_tools: BTreeSet::from(["mcp:codebase-memory.search_graph".to_string()]),
        writable_roots: BTreeSet::from(["D:/workspace/project".to_string()]),
        protected_paths: BTreeSet::from(["D:/workspace/project/src/protected".to_string()]),
        model_routes: BTreeSet::from(["route/cx-test".to_string()]),
        permission_ceiling: PermissionPolicy::Ask,
    }
}

fn runtime_work_spec(project_root: &Path) -> WorkSpec {
    let mut work_spec = test_work_spec(HarnessKind::Cx, WorkRole::Implementer);
    work_spec.created_at = "2026-07-21T00:00:00Z".to_string();
    work_spec.expires_at = None;
    work_spec.scope = WorkScope {
        canonical_paths: BTreeSet::from([project_root.to_string_lossy().to_string()]),
        protected_paths: BTreeSet::new(),
    };
    work_spec.allowed_tools = BTreeSet::from(["shell_command".to_string()]);
    work_spec.forbidden_tools.clear();
    work_spec.allowed_mcp_tools.clear();
    work_spec.tool_grants.clear();
    work_spec.model_route = ModelRouteSelection::InheritMainRequested;
    work_spec.fallback_policy_id = "strict".to_string();
    work_spec
}

fn request_body_contains(request: &wiremock::Request, needle: &str) -> bool {
    String::from_utf8_lossy(&request.body).contains(needle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_session_bootstrap_is_typed_main_cc_cc_harness() -> Result<()> {
    let server = start_mock_server().await;
    let test = test_codex().build(&server).await?;

    assert_eq!(test.config.cccx_actor_identity, ActorIdentity::MainCc);
    assert_eq!(test.config.cccx_harness, HarnessKind::Cc);
    assert!(
        test.config.cccx_child_permission_policy.is_none(),
        "root startup must not carry a child-only permission projection"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn main_cc_tool_spawn_creates_a_typed_cx_runtime_child() -> Result<()> {
    let server = start_mock_server().await;
    let mock_base_url = server.uri();
    let mut builder = test_codex().with_config(move |config| {
        config
            .features
            .enable(Feature::Collab)
            .expect("test config should enable the native multi-agent tool");
        let mut provider = ModelProviderInfo::create_9router_provider();
        provider.base_url = Some(mock_base_url);
        provider.env_key = None;
        provider.experimental_bearer_token = Some("final-gate-test-token".to_string());
        provider.wire_api = WireApi::Responses;
        provider.supports_websockets = false;
        config.model_provider = provider;
    });
    let test = builder.build(&server).await?;

    let live_work_spec = runtime_work_spec(test.cwd_path());
    let live_spawn_args = serde_json::to_string(&json!({
        "message": SPAWN_CHILD_PROMPT,
        "work_spec": live_work_spec,
    }))?;
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_body_contains(request, SPAWN_PARENT_PROMPT),
        sse(vec![
            ev_response_created("cccx-final-gate-parent-live-spawn"),
            ev_function_call_with_namespace(
                SPAWN_CALL_ID,
                "multi_agent_v1",
                "spawn_agent",
                &live_spawn_args,
            ),
            ev_completed("cccx-final-gate-parent-live-spawn"),
        ]),
    )
    .await;
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| {
            request_body_contains(request, SPAWN_CHILD_PROMPT)
                && !request_body_contains(request, SPAWN_CALL_ID)
        },
        sse(vec![
            ev_response_created("cccx-final-gate-child"),
            ev_assistant_message("cccx-final-gate-child-message", "child completed"),
            ev_completed("cccx-final-gate-child"),
        ]),
    )
    .await;
    // Mount this last because the follow-up also contains the original parent
    // prompt in history; wiremock gives the latest matching mock priority.
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_body_contains(request, SPAWN_CALL_ID),
        sse(vec![
            ev_response_created("cccx-final-gate-parent-follow-up"),
            ev_assistant_message("cccx-final-gate-parent-message", "child started"),
            ev_completed("cccx-final-gate-parent-follow-up"),
        ]),
    )
    .await;

    let mut created_threads = test.thread_manager.subscribe_thread_created();
    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: SPAWN_PARENT_PROMPT.to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    let child_thread_id = tokio::time::timeout(Duration::from_secs(10), created_threads.recv())
        .await
        .expect("timed out waiting for the real child thread")
        .expect("thread-created channel closed unexpectedly");
    let child = test.thread_manager.get_thread(child_thread_id).await?;
    let child_config = child.config().await;

    assert_eq!(child_config.cccx_actor_identity, ActorIdentity::CxSubagent);
    assert_eq!(child_config.cccx_harness, HarnessKind::Cx);
    assert_eq!(
        child_config.cccx_child_permission_policy,
        Some(PermissionPolicy::Ask),
        "the child Ask policy must be intersected with the live turn ceiling"
    );
    assert!(
        child_config.cccx_child_writable_scope.is_empty(),
        "the default test turn has no writable parent capability, so the spawned child must stay read-only"
    );
    let workers = test
        .codex
        .list_cccx_workers()
        .await
        .map_err(anyhow::Error::msg)?;
    assert!(workers.iter().any(|worker| {
        worker.logical_session_id == "ws-final-gate-1"
            && worker.task_id == "task-final-gate-1"
            && worker.worker_incarnation_id == child_thread_id.to_string()
    }));
    wait_for_event_with_timeout(
        &child,
        |event| matches!(event, EventMsg::TurnComplete(_)),
        Duration::from_secs(10),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let requests = server
        .received_requests()
        .await
        .expect("mock server should expose received requests");
    assert!(
        requests
            .iter()
            .any(|request| request_body_contains(request, SPAWN_CALL_ID)),
        "parent never sent a tool-result follow-up after the real spawn; requests={:?}",
        requests
            .iter()
            .map(|request| String::from_utf8_lossy(&request.body).into_owned())
            .collect::<Vec<_>>()
    );
    // The spawned task is deliberately unverified, so the StopHook must keep
    // the parent turn open. Interrupt only to tear down this focused Phase 1
    // fixture after the real spawn and child lifecycle have been observed.
    test.codex.submit(Op::Interrupt).await?;
    let parent_terminal = wait_for_event_with_timeout(
        &test.codex,
        |event| matches!(event, EventMsg::TurnAborted(_)),
        Duration::from_secs(10),
    )
    .await;
    assert!(
        matches!(parent_terminal, EventMsg::TurnAborted(_)),
        "focused fixture cleanup must abort the still-unverified parent turn"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_core_config_is_typed_main_cc_cc_identity() -> Result<()> {
    let server = start_mock_server().await;
    let test = test_codex().build(&server).await?;

    assert_eq!(test.config.cccx_actor_identity, ActorIdentity::MainCc);
    assert_eq!(test.config.cccx_harness, HarnessKind::Cc);
    assert!(test.config.cccx_child_permission_policy.is_none());
    assert!(test.config.cccx_child_allowed_tools.is_empty());
    assert!(test.config.cccx_child_allowed_mcp_tools.is_empty());
    assert!(test.config.cccx_child_writable_scope.is_empty());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_config_projection_sets_typed_identity_harness_and_denies_by_intersection()
-> Result<()> {
    let server = start_mock_server().await;
    let test = test_codex().build(&server).await?;
    let mut child_config: Config = test.config.clone();
    let work_spec = test_work_spec(HarnessKind::Cx, WorkRole::Implementer);

    apply_cccx_harness_to_config(&mut child_config, &parent_capabilities(), &work_spec)?;

    assert_eq!(child_config.cccx_actor_identity, ActorIdentity::CxSubagent);
    assert_eq!(child_config.cccx_harness, HarnessKind::Cx);
    assert_eq!(
        child_config.cccx_child_permission_policy,
        Some(PermissionPolicy::Ask)
    );
    assert_eq!(
        child_config.cccx_child_allowed_tools,
        BTreeSet::from(["apply_patch".to_string(), "shell_command".to_string()])
    );
    assert_eq!(
        child_config.cccx_child_allowed_mcp_tools,
        BTreeSet::from(["mcp:codebase-memory.search_graph".to_string()])
    );
    assert_eq!(
        child_config.cccx_child_writable_scope,
        BTreeSet::from(["D:/workspace/project/src".to_string()])
    );
    assert_eq!(child_config.model.as_deref(), Some("route/cx-test"));
    assert!(!child_config.orchestrator_mcp_enabled);
    assert!(!child_config.orchestrator_skills_enabled);
    assert!(
        child_config
            .developer_instructions
            .as_deref()
            .is_some_and(|text| text.contains("CX"))
    );

    Ok(())
}

#[test]
fn typed_actor_identity_is_the_authority_boundary_for_spawn() {
    assert!(ActorIdentity::MainCc.can_spawn());
    assert!(!ActorIdentity::CcSubagent.can_spawn());
    assert!(!ActorIdentity::CxSubagent.can_spawn());
}

#[test]
fn safe_mode_cycle_excludes_dont_ask_and_bypass() {
    let mut state = CccxModeState::new(MainMode::Plan);
    let mut observed = Vec::new();

    for _ in 0..8 {
        let next = state.next_in_cycle();
        observed.push(next.clone());
        state
            .change_mode(next)
            .expect("safe cycle should transition");
    }

    assert_eq!(
        observed,
        vec![
            MainMode::Default,
            MainMode::AcceptEdits,
            MainMode::Auto,
            MainMode::Plan,
            MainMode::Default,
            MainMode::AcceptEdits,
            MainMode::Auto,
            MainMode::Plan,
        ]
    );
    assert!(!observed.contains(&MainMode::DontAsk));
    assert!(!observed.contains(&MainMode::Bypass));
}
