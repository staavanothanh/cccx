use anyhow::Result;
use cccx_mode::CccxModeState;
use cccx_policy::MainMode;
use cccx_protocol::ActorIdentity;
use cccx_protocol::ArtifactStatus;
use cccx_protocol::CommandSpec;
use cccx_protocol::EvidenceKind;
use cccx_protocol::ExpectedArtifact;
use cccx_protocol::ForkPolicy;
use cccx_protocol::HarnessKind;
use cccx_protocol::ModelRouteSelection;
use cccx_protocol::ParentCapabilitySet;
use cccx_protocol::PermissionPolicy;
use cccx_protocol::RevisionRef;
use cccx_protocol::ToolGrant;
use cccx_protocol::VerificationSpec;
use cccx_protocol::WorkRole;
use cccx_protocol::WorkScope;
use cccx_protocol::WorkSpec;
use codex_core::cccx_spawn::apply_cccx_harness_to_config;
use codex_core::config::Config;
use codex_features::Feature;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::ConversationStartParams;
use codex_protocol::protocol::ConversationStartTransport;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::RealtimeConversationRealtimeEvent;
use codex_protocol::protocol::RealtimeEvent;
use codex_protocol::protocol::RealtimeOutputModality;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_function_call_with_namespace;
use core_test_support::responses::ev_message_item_added;
use core_test_support::responses::ev_output_text_delta;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_response_once_match;
use core_test_support::responses::mount_sse_once_match;
use core_test_support::responses::sse;
use core_test_support::responses::sse_response;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event_with_timeout;
use pretty_assertions::assert_eq;
use serde_json::json;
use serial_test::serial;
use sha2::Digest;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use wiremock::Mock;
use wiremock::Respond;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path_regex;

const SPAWN_CALL_ID: &str = "cccx-final-gate-spawn";
const SPAWN_CHILD_SHELL_CALL_ID: &str = "cccx-final-gate-child-shell";
const SPAWN_PARENT_WAIT_CALL_ID: &str = "cccx-final-gate-parent-wait";
const SPAWN_PARENT_PROMPT: &str =
    "spawn the final-gate CX worker; RAW_PARENT_SENTINEL_MUST_NOT_REACH_CHILD";
const SPAWN_CHILD_PROMPT: &str = "perform the bounded final-gate child task";

struct ParentWaitForSpawnedWorker;

impl Respond for ParentWaitForSpawnedWorker {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let body: serde_json::Value =
            serde_json::from_slice(&request.body).expect("parent follow-up request JSON");
        let agent_id = find_agent_id(&body).expect("spawn output must contain agent_id");
        let arguments = json!({
            "targets": [agent_id],
            "timeout_ms": 10_000,
        })
        .to_string();
        sse_response(sse(vec![
            ev_response_created("cccx-final-gate-parent-wait-response"),
            ev_function_call_with_namespace(
                SPAWN_PARENT_WAIT_CALL_ID,
                "multi_agent_v1",
                "wait_agent",
                &arguments,
            ),
            ev_completed("cccx-final-gate-parent-wait-response"),
        ]))
    }
}

struct DisconnectThenSuccess {
    attempts: AtomicUsize,
}

impl Respond for DisconnectThenSuccess {
    fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            return sse_response(sse(vec![ev_response_created(
                "cccx-gateway-disconnect-before-output",
            )]));
        }
        sse_response(sse(vec![
            ev_response_created("cccx-gateway-retry-success"),
            ev_assistant_message("cccx-gateway-retry-message", "retry completed"),
            ev_completed("cccx-gateway-retry-success"),
        ]))
    }
}

struct RetryAfterThenSuccess {
    attempts: AtomicUsize,
}

impl Respond for RetryAfterThenSuccess {
    fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            return ResponseTemplate::new(429)
                .insert_header("retry-after", "1")
                .set_body_json(json!({"error": {"message": "router busy"}}));
        }
        sse_response(sse(vec![
            ev_response_created("cccx-gateway-rate-limit-success"),
            ev_assistant_message("cccx-gateway-rate-limit-message", "rate limit recovered"),
            ev_completed("cccx-gateway-rate-limit-success"),
        ]))
    }
}

fn chat_text_stream(response_id: &str, text: &str) -> String {
    let chunk = json!({
        "id": response_id,
        "model": "provider-observed/chat-model",
        "choices": [{
            "index": 0,
            "delta": {"role": "assistant", "content": text},
            "finish_reason": "stop"
        }]
    });
    format!("data: {chunk}\n\ndata: [DONE]\n\n")
}

fn find_agent_id(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => map
            .get("agent_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| map.values().find_map(find_agent_id)),
        serde_json::Value::Array(values) => values.iter().find_map(find_agent_id),
        serde_json::Value::String(encoded)
            if encoded.trim_start().starts_with('{') || encoded.trim_start().starts_with('[') =>
        {
            serde_json::from_str(encoded)
                .ok()
                .and_then(|decoded| find_agent_id(&decoded))
        }
        serde_json::Value::String(_) => None,
        _ => None,
    }
}

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
            advisory_globs: BTreeSet::new(),
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
        resolved_model_route: None,
        fallback_policy_id: "fail-closed".to_string(),
        fork_policy: ForkPolicy::WorkSpecOnly,
        enforced_constraints: vec!["bounded-scope".to_string()],
        typed_constraints: Vec::new(),
        advisory_constraints: Vec::new(),
        risk_threshold: None,
        action_classes: None,
        approval_policy: None,
        verification_spec: None,
        audit_depth: None,
        expected_artifact: Some(ExpectedArtifact {
            required_status: ArtifactStatus::Success,
            required_evidence_kinds: vec![EvidenceKind::TestLog],
            required_fields: Vec::new(),
            verification_policy_id: None,
        }),
        checkpoint_id: None,
        base_commit: None,
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
        advisory_globs: BTreeSet::new(),
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

fn request_tool_names(request: &wiremock::Request) -> BTreeSet<String> {
    serde_json::from_slice::<serde_json::Value>(&request.body)
        .ok()
        .and_then(|value| {
            value
                .get("tools")
                .and_then(|tools| tools.as_array())
                .cloned()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|tool| {
            tool.get("name")
                .and_then(|name| name.as_str())
                .map(str::to_string)
        })
        .collect()
}

fn configure_mock_9router(config: &mut Config, base_url: String) {
    config.model = Some("oc/deepseek-v4-flash-free".to_string());
    let mut provider = ModelProviderInfo::create_9router_provider();
    provider.base_url = Some(format!("{}/v1", base_url.trim_end_matches('/')));
    provider.env_key = None;
    provider.experimental_bearer_token = Some("final-gate-test-token".to_string());
    provider.wire_api = WireApi::Responses;
    provider.supports_websockets = false;
    config.model_provider = provider;
}

fn realtime_start_params(transport: Option<ConversationStartTransport>) -> ConversationStartParams {
    ConversationStartParams {
        client_managed_handoffs: false,
        flush_transcript_tail_on_session_end: false,
        codex_responses_as_items: false,
        codex_response_item_prefix: None,
        codex_response_handoff_prefix: None,
        model: None,
        output_modality: RealtimeOutputModality::Audio,
        include_startup_context: true,
        prompt: Some(Some("cccx final-gate realtime start".to_string())),
        realtime_session_id: None,
        transport,
        version: None,
        voice: None,
    }
}

fn gateway_provenance_json(test: &core_test_support::test_codex::TestCodex) -> serde_json::Value {
    let thread_id = test.session_configured.thread_id.to_string();
    let path = test
        .cwd_path()
        .join(".cccx")
        .join("state")
        .join("gateway-provenance")
        .join(format!("{thread_id}.json"));
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn realtime_error_message(event: &EventMsg) -> Option<String> {
    match event {
        EventMsg::Error(err) => Some(err.message.clone()),
        EventMsg::RealtimeConversationRealtime(RealtimeConversationRealtimeEvent {
            payload: RealtimeEvent::Error(message),
        }) => Some(message.clone()),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn realtime_websocket_fails_closed_in_9router_only_mode() -> Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config({
            let uri = server.uri();
            move |config| {
                configure_mock_9router(config, uri.clone());
                config.experimental_realtime_ws_base_url = Some(uri.clone());
            }
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::RealtimeConversationStart(realtime_start_params(Some(
            ConversationStartTransport::Websocket,
        ))))
        .await?;

    let err = wait_for_event_with_timeout(
        &test.codex,
        |event| realtime_error_message(event).is_some(),
        Duration::from_secs(5),
    )
    .await;
    let err = realtime_error_message(&err).expect("realtime error message");

    assert!(err.contains("cccx: realtime conversations are disabled in 9router-only mode"));
    assert!(
        err.contains("WebSocket/WebRTC"),
        "error must name the disabled realtime transports"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn realtime_webrtc_fails_closed_before_transport_in_9router_only_mode() -> Result<()> {
    let server = start_mock_server().await;
    Mock::given(method("POST"))
        .and(path_regex(".*/realtime/calls$"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let test = test_codex()
        .with_config({
            let uri = server.uri();
            move |config| {
                configure_mock_9router(config, uri.clone());
                config.experimental_realtime_webrtc_call_base_url =
                    Some(format!("{}/v1", uri.trim_end_matches('/')));
            }
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::RealtimeConversationStart(realtime_start_params(Some(
            ConversationStartTransport::Webrtc {
                sdp: "v=0\r\n".to_string(),
            },
        ))))
        .await?;

    let err = wait_for_event_with_timeout(
        &test.codex,
        |event| realtime_error_message(event).is_some(),
        Duration::from_secs(5),
    )
    .await;
    let err = realtime_error_message(&err).expect("realtime error message");

    assert!(err.contains("cccx: realtime conversations are disabled in 9router-only mode"));
    Ok(())
}

async fn submit_gateway_prompt(
    test: &core_test_support::test_codex::TestCodex,
    prompt: &str,
) -> Result<()> {
    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: prompt.to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    wait_for_event_with_timeout(
        &test.codex,
        |event| matches!(event, EventMsg::TurnComplete(_)),
        Duration::from_secs(20),
    )
    .await;
    Ok(())
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
#[serial(cccx_runtime)]
async fn mock_9router_success_persists_factual_unknown_route_provenance() -> Result<()> {
    const PROMPT: &str = "phase5 successful 9router stream";
    let server = start_mock_server().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path("/v1/responses"))
        .and(|request: &wiremock::Request| request_body_contains(request, PROMPT))
        .respond_with(
            sse_response(sse(vec![
                ev_response_created("cccx-gateway-success"),
                ev_assistant_message("cccx-gateway-success-message", "gateway completed"),
                ev_completed("cccx-gateway-success"),
            ]))
            .insert_header("openai-model", "provider-observed/model-v1"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mock_base_url = server.uri();
    let test = test_codex()
        .with_config(move |config| configure_mock_9router(config, mock_base_url))
        .build(&server)
        .await?;

    submit_gateway_prompt(&test, PROMPT).await?;

    let record = gateway_provenance_json(&test);
    let provenance = &record["provenance"];
    assert_eq!(
        provenance["requested_route_id"],
        "oc/deepseek-v4-flash-free"
    );
    assert!(provenance["effective_route_id"].is_null());
    assert!(provenance["effective_provider"].is_null());
    assert!(provenance["effective_model"].is_null());
    assert_eq!(
        provenance["observed_server_model"],
        "provider-observed/model-v1"
    );
    assert_eq!(provenance["stream_termination_status"], "completed");
    assert_eq!(provenance["response_id"], "cccx-gateway-success");
    assert!(
        provenance["input_digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:"))
    );
    assert!(
        provenance["output_digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:"))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn mock_9router_chat_completions_stream_uses_the_same_gateway_boundary() -> Result<()> {
    const PROMPT: &str = "phase5 chat completions compatibility";
    let server = start_mock_server().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path("/v1/chat/completions"))
        .and(|request: &wiremock::Request| request_body_contains(request, PROMPT))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .insert_header("x-request-id", "chat-router-request")
                .set_body_string(chat_text_stream(
                    "chatcmpl-phase5",
                    "chat transport completed",
                )),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mock_base_url = server.uri();
    let test = test_codex()
        .with_config(move |config| {
            configure_mock_9router(config, mock_base_url);
            config.model_provider.wire_api = WireApi::ChatCompletions;
        })
        .build(&server)
        .await?;

    submit_gateway_prompt(&test, PROMPT).await?;

    let record = gateway_provenance_json(&test);
    let provenance = &record["provenance"];
    assert_eq!(provenance["wire_api"], "chat_completions");
    // The configured base URL owns the `/v1` API root; provenance records the
    // transport's endpoint suffix. The exact wire path is enforced by the
    // wiremock matcher above.
    assert_eq!(provenance["endpoint"], "/chat/completions");
    assert_eq!(provenance["stream_termination_status"], "completed");
    assert_eq!(provenance["response_id"], "chatcmpl-phase5");
    assert_eq!(provenance["upstream_request_id"], "chat-router-request");
    assert_eq!(
        provenance["observed_server_model"],
        "provider-observed/chat-model"
    );
    assert!(provenance["effective_route_id"].is_null());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn mock_9router_disconnect_before_output_retries_once_with_provenance() -> Result<()> {
    const PROMPT: &str = "phase5 retry safe disconnect";
    let server = start_mock_server().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path("/v1/responses"))
        .and(|request: &wiremock::Request| request_body_contains(request, PROMPT))
        .respond_with(DisconnectThenSuccess {
            attempts: AtomicUsize::new(0),
        })
        .expect(2)
        .mount(&server)
        .await;
    let mock_base_url = server.uri();
    let test = test_codex()
        .with_config(move |config| configure_mock_9router(config, mock_base_url))
        .build(&server)
        .await?;

    submit_gateway_prompt(&test, PROMPT).await?;

    let requests = server.received_requests().await.unwrap_or_default();
    let request_bodies = requests
        .iter()
        .filter(|request| request.url.path() == "/v1/responses")
        .map(|request| {
            serde_json::from_slice::<serde_json::Value>(&request.body).expect("retry request JSON")
        })
        .collect::<Vec<_>>();
    assert_eq!(request_bodies.len(), 2);
    assert_eq!(
        request_bodies[0], request_bodies[1],
        "transparent retry must replay the immutable request payload"
    );

    let record = gateway_provenance_json(&test);
    let provenance = &record["provenance"];
    assert_eq!(provenance["retry_count"], 1);
    assert_eq!(provenance["stream_termination_status"], "completed");
    assert_eq!(provenance["response_id"], "cccx-gateway-retry-success");
    let attempts = provenance["attempts"]
        .as_array()
        .expect("per-attempt gateway journal");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["streamTerminationStatus"], "error");
    assert_eq!(attempts[1]["streamTerminationStatus"], "completed");
    assert_eq!(attempts[0]["inputDigest"], attempts[1]["inputDigest"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn mock_9router_retry_after_is_honored_and_persisted() -> Result<()> {
    const PROMPT: &str = "phase5 retry after";
    let server = start_mock_server().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path("/v1/responses"))
        .and(|request: &wiremock::Request| request_body_contains(request, PROMPT))
        .respond_with(RetryAfterThenSuccess {
            attempts: AtomicUsize::new(0),
        })
        .expect(2)
        .mount(&server)
        .await;
    let mock_base_url = server.uri();
    let test = test_codex()
        .with_config(move |config| configure_mock_9router(config, mock_base_url))
        .build(&server)
        .await?;

    submit_gateway_prompt(&test, PROMPT).await?;

    let record = gateway_provenance_json(&test);
    let provenance = &record["provenance"];
    assert_eq!(provenance["retry_count"], 1);
    assert_eq!(provenance["retry_after_ms"], 1_000);
    assert_eq!(provenance["stream_termination_status"], "completed");
    let attempts = provenance["attempts"]
        .as_array()
        .expect("per-attempt gateway journal");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["httpStatus"], 429);
    assert_eq!(attempts[0]["retryAfterMs"], 1_000);
    assert_eq!(attempts[1]["streamTerminationStatus"], "completed");
    assert_eq!(attempts[0]["inputDigest"], attempts[1]["inputDigest"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn mock_9router_exhausted_429_preserves_typed_status() -> Result<()> {
    const PROMPT: &str = "phase5 exhausted rate limit";
    let server = start_mock_server().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path("/v1/responses"))
        .and(|request: &wiremock::Request| request_body_contains(request, PROMPT))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "1")
                .insert_header("x-request-id", "router-rate-limit-terminal")
                .set_body_json(json!({"error": {"message": "router still busy"}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mock_base_url = server.uri();
    let test = test_codex()
        .with_config(move |config| {
            configure_mock_9router(config, mock_base_url);
            config.model_provider.stream_max_retries = Some(0);
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: PROMPT.to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    let event = wait_for_event_with_timeout(
        &test.codex,
        |event| {
            matches!(
                event,
                EventMsg::Error(error)
                    if matches!(
                        error.codex_error_info.as_ref(),
                        Some(CodexErrorInfo::ResponseTooManyFailedAttempts {
                            http_status_code: Some(429)
                        })
                    )
            )
        },
        Duration::from_secs(10),
    )
    .await;
    assert!(matches!(
        event,
        EventMsg::Error(error)
            if matches!(
                error.codex_error_info,
                Some(CodexErrorInfo::ResponseTooManyFailedAttempts {
                    http_status_code: Some(429)
                })
            )
    ));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn mock_9router_disconnect_after_tool_intent_does_not_replay_request() -> Result<()> {
    const PROMPT: &str = "phase5 do not replay tool intent";
    let server = start_mock_server().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path("/v1/responses"))
        .and(|request: &wiremock::Request| request_body_contains(request, PROMPT))
        .respond_with(sse_response(sse(vec![
            ev_response_created("cccx-gateway-tool-disconnect"),
            ev_function_call(
                "cccx-gateway-tool-intent",
                "update_plan",
                &json!({
                    "plan": [{"step": "observe tool intent", "status": "in_progress"}]
                })
                .to_string(),
            ),
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let mock_base_url = server.uri();
    let test = test_codex()
        .with_config(move |config| configure_mock_9router(config, mock_base_url))
        .build(&server)
        .await?;

    submit_gateway_prompt(&test, PROMPT).await?;

    let record = gateway_provenance_json(&test);
    let provenance = &record["provenance"];
    assert_eq!(provenance["retry_count"], 0);
    assert_eq!(provenance["stream_termination_status"], "error");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn mock_9router_partial_text_before_disconnect_is_digested_without_replay() -> Result<()> {
    const PROMPT: &str = "phase5 partial text provenance";
    let server = start_mock_server().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path("/v1/responses"))
        .and(|request: &wiremock::Request| request_body_contains(request, PROMPT))
        .respond_with(sse_response(sse(vec![
            ev_response_created("cccx-gateway-partial-text"),
            ev_message_item_added("cccx-partial-text-item", ""),
            ev_output_text_delta("partial visible output"),
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let mock_base_url = server.uri();
    let test = test_codex()
        .with_config(move |config| configure_mock_9router(config, mock_base_url))
        .build(&server)
        .await?;

    submit_gateway_prompt(&test, PROMPT).await?;

    let provenance = gateway_provenance_json(&test)["provenance"].clone();
    let empty_digest = format!("sha256:{:x}", sha2::Sha256::digest(b"[]"));
    assert_eq!(provenance["retry_count"], 0);
    assert_eq!(provenance["stream_termination_status"], "error");
    assert_ne!(provenance["output_digest"], empty_digest);
    assert_eq!(provenance["attempts"].as_array().map(Vec::len), Some(1));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn mock_9router_partial_tool_arguments_are_digested_without_replay() -> Result<()> {
    const PROMPT: &str = "phase5 partial tool arguments provenance";
    let server = start_mock_server().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path("/v1/responses"))
        .and(|request: &wiremock::Request| request_body_contains(request, PROMPT))
        .respond_with(sse_response(sse(vec![
            ev_response_created("cccx-gateway-partial-tool"),
            json!({
                "type": "response.output_item.added",
                "item": {
                    "type": "function_call",
                    "id": "cccx-partial-tool-item",
                    "call_id": "cccx-partial-tool-call",
                    "name": "update_plan",
                    "arguments": "",
                },
            }),
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": "cccx-partial-tool-item",
                "call_id": "cccx-partial-tool-call",
                "delta": "{\"plan\":[",
            }),
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let mock_base_url = server.uri();
    let test = test_codex()
        .with_config(move |config| configure_mock_9router(config, mock_base_url))
        .build(&server)
        .await?;

    submit_gateway_prompt(&test, PROMPT).await?;

    let provenance = gateway_provenance_json(&test)["provenance"].clone();
    let empty_digest = format!("sha256:{:x}", sha2::Sha256::digest(b"[]"));
    assert_eq!(provenance["retry_count"], 0);
    assert_eq!(provenance["stream_termination_status"], "error");
    assert_ne!(provenance["output_digest"], empty_digest);
    assert_eq!(provenance["attempts"].as_array().map(Vec::len), Some(1));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn mock_9router_auth_failure_is_terminal_without_oauth_retry() -> Result<()> {
    const PROMPT: &str = "phase5 auth failure";
    let server = start_mock_server().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path("/v1/responses"))
        .and(|request: &wiremock::Request| request_body_contains(request, PROMPT))
        .respond_with(
            ResponseTemplate::new(401)
                .insert_header("x-request-id", "router-auth-request")
                .set_body_json(json!({"error": {"message": "invalid router token"}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mock_base_url = server.uri();
    let test = test_codex()
        .with_config(move |config| configure_mock_9router(config, mock_base_url))
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: PROMPT.to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    wait_for_event_with_timeout(
        &test.codex,
        |event| matches!(event, EventMsg::Error(_)),
        Duration::from_secs(20),
    )
    .await;

    let record = gateway_provenance_json(&test);
    let provenance = &record["provenance"];
    assert_eq!(provenance["http_status"], 401);
    assert_eq!(provenance["retry_count"], 0);
    assert_eq!(provenance["stream_termination_status"], "error");
    assert_eq!(provenance["upstream_request_id"], "router-auth-request");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn main_cc_tool_spawn_creates_a_typed_cx_runtime_child() -> Result<()> {
    let server = start_mock_server().await;
    let mock_base_url = server.uri();
    let mut builder = test_codex().with_config(move |config| {
        config
            .features
            .enable(Feature::Collab)
            .expect("test config should enable the native multi-agent tool");
        let mut provider = ModelProviderInfo::create_9router_provider();
        provider.base_url = Some(format!("{}/v1", mock_base_url.trim_end_matches('/')));
        provider.env_key = None;
        provider.experimental_bearer_token = Some("final-gate-test-token".to_string());
        provider.wire_api = WireApi::Responses;
        provider.supports_websockets = false;
        config.model_provider = provider;
        config
            .set_legacy_sandbox_policy(SandboxPolicy::WorkspaceWrite {
                writable_roots: vec![config.cwd.clone()],
                network_access: false,
                exclude_tmpdir_env_var: true,
                exclude_slash_tmp: true,
            })
            .expect("parent test sandbox should accept its cwd as writable root");
    });
    let test = builder.build(&server).await?;

    let mut live_work_spec = runtime_work_spec(test.cwd_path());
    live_work_spec.verification_spec = Some(VerificationSpec {
        command: Some(CommandSpec {
            argv: if cfg!(windows) {
                vec![
                    "cmd".to_string(),
                    "/C".to_string(),
                    "echo cccx-final-gate".to_string(),
                ]
            } else {
                vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "printf cccx-final-gate".to_string(),
                ]
            },
            cwd: ".".to_string(),
            timeout_ms: Some(5_000),
        }),
        commands: Vec::new(),
        protected_paths: Default::default(),
        protected_digests: Default::default(),
        verification_policy_id: Some("final-gate-live".to_string()),
    });
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
            ev_function_call(
                SPAWN_CHILD_SHELL_CALL_ID,
                "shell_command",
                &json!({"command": "Get-ChildItem", "timeout_ms": 5_000}).to_string(),
            ),
            ev_completed("cccx-final-gate-child"),
        ]),
    )
    .await;
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_body_contains(request, SPAWN_CHILD_SHELL_CALL_ID),
        sse(vec![
            ev_response_created("cccx-final-gate-child-follow-up"),
            ev_assistant_message(
                "cccx-final-gate-child-message",
                "bounded read tool completed",
            ),
            ev_completed("cccx-final-gate-child-follow-up"),
        ]),
    )
    .await;
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_body_contains(request, SPAWN_PARENT_WAIT_CALL_ID),
        sse(vec![
            ev_response_created("cccx-final-gate-parent-follow-up"),
            ev_assistant_message("cccx-final-gate-parent-message", "child verified"),
            ev_completed("cccx-final-gate-parent-follow-up"),
        ]),
    )
    .await;
    // The parent must explicitly wait for the child. This removes the timing
    // race where terminal prose arrived before the child Artifact transaction.
    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .and(|request: &wiremock::Request| request_body_contains(request, SPAWN_CALL_ID))
        .respond_with(ParentWaitForSpawnedWorker)
        .up_to_n_times(1)
        .mount(&server)
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
    assert_eq!(child_config.cccx_child_writable_scope.len(), 1);
    let child_sandbox = child_config.legacy_sandbox_policy();
    assert!(
        matches!(
            child_sandbox,
            SandboxPolicy::WorkspaceWrite {
                network_access: false,
                ..
            }
        ),
        "spawned child sandbox did not preserve the projected writable root: {child_sandbox:?}"
    );
    let projected_root = child_config
        .cccx_child_writable_scope
        .iter()
        .next()
        .expect("one projected root");
    let effective_roots = child_config.effective_workspace_roots();
    assert_eq!(effective_roots.len(), 1);
    assert_eq!(
        std::fs::canonicalize(effective_roots[0].as_path())?,
        Path::new(projected_root)
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
        Duration::from_secs(20),
    )
    .await;
    let parent_terminal = wait_for_event_with_timeout(
        &test.codex,
        |event| matches!(event, EventMsg::TurnComplete(_)),
        Duration::from_secs(20),
    )
    .await;
    assert!(matches!(parent_terminal, EventMsg::TurnComplete(_)));
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
    let child_request = requests
        .iter()
        .find(|request| {
            request_body_contains(request, SPAWN_CHILD_PROMPT)
                && !request_body_contains(request, SPAWN_CALL_ID)
        })
        .expect("spawned child must send a model request");
    let child_tools = request_tool_names(child_request);
    assert!(request_body_contains(
        child_request,
        cccx_protocol::CCCX_WORK_SPEC_SCHEMA_VERSION
    ));
    assert!(request_body_contains(child_request, "ws-final-gate-1"));
    assert!(request_body_contains(child_request, "task-final-gate-1"));
    assert!(!request_body_contains(
        child_request,
        "RAW_PARENT_SENTINEL_MUST_NOT_REACH_CHILD"
    ));
    assert!(child_tools.contains("shell_command"));
    assert!(!child_tools.contains("apply_patch"));
    assert!(!child_tools.contains("tool_search"));
    assert!(!child_tools.iter().any(|tool| tool.starts_with("mcp__")));
    assert!(!child_tools.iter().any(|tool| tool.contains("spawn_agent")));
    let graph = codex_core::cccx_task_graph_state::load_task_graph(test.cwd_path())
        .await?
        .expect("the live spawn must persist a TaskGraph");
    let task = graph
        .tasks
        .get("task-final-gate-1")
        .expect("the WorkSpec task must exist");
    let attempt = task.attempts.last().expect("the child attempt must exist");
    let artifact_id = attempt
        .artifact_ids
        .last()
        .expect("the terminal child must return an Artifact");
    let artifact = graph
        .artifacts
        .get(artifact_id)
        .expect("the Artifact must be bound into the graph");
    assert_eq!(artifact.task_id, task.task_id);
    assert_eq!(artifact.work_spec_id, attempt.work_spec_id);
    assert_eq!(artifact.attempt_id, attempt.attempt_id);
    assert!(task.is_verified());
    assert!(attempt.verification_records.iter().any(|record| {
        record.passed
            && record
                .command
                .as_ref()
                .is_some_and(|command| command.exit_code == 0)
    }));
    assert!(
        codex_core::cccx_task_graph_state::stop_guard(&graph).is_ok(),
        "the real root StopHook predicate must allow the verified graph"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(cccx_runtime)]
async fn spawned_cx_empty_intersections_are_read_only_and_fail_closed() -> Result<()> {
    const PARENT_PROMPT: &str = "spawn the deny-by-default CX worker";
    const CHILD_PROMPT: &str = "inspect the deny-by-default child boundary";
    const CALL_ID: &str = "cccx-deny-by-default-spawn";
    const UNKNOWN_CALL_ID: &str = "cccx-injected-unknown-tool";
    let server = start_mock_server().await;
    let mock_base_url = server.uri();
    let test = test_codex()
        .with_config(move |config| {
            config
                .features
                .enable(Feature::Collab)
                .expect("enable native multi-agent tool");
            let mut provider = ModelProviderInfo::create_9router_provider();
            provider.base_url = Some(format!("{}/v1", mock_base_url.trim_end_matches('/')));
            provider.env_key = None;
            provider.experimental_bearer_token = Some("final-gate-test-token".to_string());
            provider.wire_api = WireApi::Responses;
            provider.supports_websockets = false;
            config.model_provider = provider;
        })
        .build(&server)
        .await?;
    let mut work_spec = runtime_work_spec(test.cwd_path());
    work_spec.work_spec_id = "ws-final-gate-deny".to_string();
    work_spec.task_id = "task-final-gate-deny".to_string();
    work_spec.attempt_id = "attempt-final-gate-deny".to_string();
    work_spec.allowed_tools = BTreeSet::from(["future_unknown_tool".to_string()]);
    work_spec.allowed_mcp_tools = BTreeSet::from(["mcp:plugin.cc_only".to_string()]);
    let spawn_args = serde_json::to_string(&json!({
        "message": CHILD_PROMPT,
        "work_spec": work_spec,
    }))?;
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_body_contains(request, PARENT_PROMPT),
        sse(vec![
            ev_response_created("cccx-deny-parent"),
            ev_function_call_with_namespace(CALL_ID, "multi_agent_v1", "spawn_agent", &spawn_args),
            ev_completed("cccx-deny-parent"),
        ]),
    )
    .await;
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| {
            request_body_contains(request, CHILD_PROMPT) && !request_body_contains(request, CALL_ID)
        },
        sse(vec![
            ev_response_created("cccx-deny-child"),
            ev_function_call(UNKNOWN_CALL_ID, "future_unknown_tool", "{}"),
            ev_completed("cccx-deny-child"),
        ]),
    )
    .await;
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_body_contains(request, UNKNOWN_CALL_ID),
        sse(vec![
            ev_response_created("cccx-deny-child-follow-up"),
            ev_assistant_message(
                "cccx-deny-child-follow-up-message",
                "unknown tool stayed denied",
            ),
            ev_completed("cccx-deny-child-follow-up"),
        ]),
    )
    .await;
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_body_contains(request, CALL_ID),
        sse(vec![
            ev_response_created("cccx-deny-follow-up"),
            ev_assistant_message("cccx-deny-follow-up-message", "child started"),
            ev_completed("cccx-deny-follow-up"),
        ]),
    )
    .await;
    mount_response_once_match(
        &server,
        |request: &wiremock::Request| {
            request_body_contains(request, "cccx StopGuard blocked completion")
        },
        sse_response(sse(vec![ev_response_created(
            "cccx-deny-stopguard-blocked",
        )]))
        .set_delay(Duration::from_secs(30)),
    )
    .await;

    let mut created_threads = test.thread_manager.subscribe_thread_created();
    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: PARENT_PROMPT.to_string(),
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
        .expect("timed out waiting for deny-by-default child")
        .expect("thread-created channel closed");
    let child = test.thread_manager.get_thread(child_thread_id).await?;
    let child_config = child.config().await;
    assert!(child_config.cccx_child_writable_scope.is_empty());
    assert!(child_config.cccx_child_allowed_tools.is_empty());
    assert!(child_config.cccx_child_allowed_mcp_tools.is_empty());
    assert_eq!(
        child_config.legacy_sandbox_policy(),
        SandboxPolicy::ReadOnly {
            network_access: false
        }
    );
    wait_for_event_with_timeout(
        &child,
        |event| matches!(event, EventMsg::TurnComplete(_)),
        Duration::from_secs(10),
    )
    .await;
    let requests = server.received_requests().await.unwrap_or_default();
    let child_request = requests
        .iter()
        .find(|request| {
            request_body_contains(request, CHILD_PROMPT) && !request_body_contains(request, CALL_ID)
        })
        .expect("deny-by-default child must send a model request");
    assert!(
        request_tool_names(child_request).is_empty(),
        "an empty child capability intersection must expose zero tools"
    );
    let unknown_follow_up = requests
        .iter()
        .find(|request| request_body_contains(request, UNKNOWN_CALL_ID))
        .expect("the injected unknown tool must produce a fail-closed child follow-up");
    let unknown_follow_up_body = String::from_utf8_lossy(&unknown_follow_up.body);
    assert!(
        unknown_follow_up_body.contains("not found")
            || unknown_follow_up_body.contains("unsupported")
            || unknown_follow_up_body.contains("unavailable"),
        "unknown tool denial must be returned to the child model: {unknown_follow_up_body}"
    );

    let mut stop_guard_request_seen = false;
    for _ in 0..100 {
        let requests = server.received_requests().await.unwrap_or_default();
        if requests
            .iter()
            .any(|request| request_body_contains(request, "cccx StopGuard blocked completion"))
        {
            stop_guard_request_seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        stop_guard_request_seen,
        "the root turn must be continued by the production StopHook while its child task is unverified"
    );
    let graph = codex_core::cccx_task_graph_state::load_task_graph(test.cwd_path())
        .await?
        .expect("the unverified child must persist a TaskGraph");
    assert!(codex_core::cccx_task_graph_state::stop_guard(&graph).is_err());

    test.codex.submit(Op::Interrupt).await?;
    let _ = wait_for_event_with_timeout(
        &test.codex,
        |event| matches!(event, EventMsg::TurnAborted(_)),
        Duration::from_secs(10),
    )
    .await;
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
    assert_eq!(child_config.cccx_child_writable_scope.len(), 1);
    assert!(
        child_config
            .cccx_child_writable_scope
            .iter()
            .all(|path| Path::new(path).is_absolute())
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
