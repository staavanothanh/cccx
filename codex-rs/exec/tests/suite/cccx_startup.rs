#![allow(clippy::unwrap_used)]

use core_test_support::responses;
use core_test_support::test_codex_exec::test_codex_exec;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cccx_exec_startup_bootstraps_main_cc_harness() -> anyhow::Result<()> {
    let test = test_codex_exec();

    let server = responses::start_mock_server().await;
    let body = responses::sse(vec![
        responses::ev_response_created("resp1"),
        responses::ev_assistant_message("m1", "fixture hello"),
        responses::ev_completed("resp1"),
    ]);
    let response_mock = responses::mount_sse_once(&server, body).await;

    test.cmd_with_server(&server)
        .arg("--skip-git-repo-check")
        .arg("bootstrap check")
        .assert()
        .success();

    let request = response_mock.single_request();
    let request_body = request.body_json();
    assert!(
        request.body_contains_text("You are the root Main CC agent for cccx."),
        "exec startup must expose MainCc control-plane instructions, got: {request_body}"
    );
    assert!(
        request.body_contains_text("Main CC uses CC harness"),
        "exec startup must bind MainCc to the CC harness, got: {request_body}"
    );
    assert!(
        !request.body_contains_text("You are a bounded CC worker for cccx."),
        "root startup must not use child CC worker prompt, got: {request_body}"
    );
    assert!(
        !request.body_contains_text("You are a bounded CX worker for cccx."),
        "root startup must not use child CX worker prompt, got: {request_body}"
    );

    Ok(())
}
