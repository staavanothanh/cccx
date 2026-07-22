use anyhow::Result;
use core_test_support::responses;
use tempfile::TempDir;

const NINE_ROUTER_API_KEY_ENV: &str = "CCCX_API_KEY";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn top_level_cccx_exec_starts_a_main_cc_session() -> Result<()> {
    let home = TempDir::new()?;
    let cwd = TempDir::new()?;
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
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            r#"model_provider = "9router-test"

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
        .arg("exec")
        .arg("--skip-git-repo-check")
        .arg("bootstrap Main CC");
    command.assert().success();

    let request = response.single_request();
    let request_body = request.body_json();
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

    Ok(())
}
