use anyhow::Result;
use codex_core::project_memory::ProjectMemory;
use core_test_support::responses;
use tempfile::TempDir;

const NINE_ROUTER_API_KEY_ENV: &str = "CCCX_API_KEY";
const OPAQUE_9ROUTER_MODEL_ID: &str = "route_opaque/main-cc:model@2026-07-22";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn top_level_cccx_exec_starts_a_main_cc_session() -> Result<()> {
    let home = TempDir::new()?;
    let cwd = TempDir::new()?;
    let empty_path = TempDir::new()?;
    let server = responses::start_mock_server().await;
    let response = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("cccx-cli-startup"),
            responses::ev_assistant_message("cccx-cli-startup-msg", "fixture complete"),
            responses::ev_completed("cccx-cli-startup"),
        ]),
    )
    .await;
    let base_url = format!("{}/v1", server.uri());
    ProjectMemory::new(home.path(), cwd.path())
        .remember("startup e2e project memory sentinel")
        .await?;
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            r#"model = "{OPAQUE_9ROUTER_MODEL_ID}"
model_provider = "9router-test"

[model_providers.9router-test]
name = "9router"
base_url = {}
env_key = "{}"
wire_api = "responses"
"#,
            serde_json::to_string(&base_url)?,
            NINE_ROUTER_API_KEY_ENV,
        ),
    )?;

    let mut command = assert_cmd::Command::new(codex_utils_cargo_bin::cargo_bin("cccx")?);
    command
        .current_dir(cwd.path())
        .env("CCCX_HOME", home.path())
        .env("CODEX_HOME", home.path())
        .env("CODEX_SQLITE_HOME", home.path())
        .env(NINE_ROUTER_API_KEY_ENV, "fixture-key")
        .env("PATH", empty_path.path())
        .arg("exec")
        .arg("--skip-git-repo-check")
        .arg("bootstrap Main CC");
    command.assert().success();

    let request = response.single_request();
    let request_body = request.body_json();
    assert_eq!(
        request_body
            .get("model")
            .and_then(serde_json::Value::as_str),
        Some(OPAQUE_9ROUTER_MODEL_ID),
        "top-level cccx startup did not send the opaque 9router model id: {request_body}"
    );
    assert!(
        request.body_contains_text("You are the root Main CC agent for cccx."),
        "top-level cccx startup did not select MainCc instructions: {request_body}"
    );
    assert!(
        request.body_contains_text("Main CC uses CC harness"),
        "top-level cccx startup did not bind the CC harness: {request_body}"
    );
    assert!(
        !request.body_contains_text("You are a bounded CC worker for cccx.")
            && !request.body_contains_text("You are a bounded CX worker for cccx."),
        "top-level cccx startup selected a child harness prompt: {request_body}"
    );
    assert!(
        request.body_contains_text("startup e2e project memory sentinel"),
        "top-level cccx startup did not load Project Memory into durable context: {request_body}"
    );
    assert!(
        request.body_contains_text("\"mcp_registry_health\""),
        "top-level cccx startup did not include MCP registry health in durable context: {request_body}"
    );
    assert!(
        request.body_contains_text("\"server_id\":\"codebase-memory-mcp\"")
            && request.body_contains_text("\"server_id\":\"sequential-thinking\""),
        "top-level cccx startup did not include both default MCP servers: {request_body}"
    );
    assert!(
        request.body_contains_text("\"observed_status\":\"unavailable\""),
        "top-level cccx startup did not report degraded MCP startup state: {request_body}"
    );

    Ok(())
}
