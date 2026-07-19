use super::*;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;

use tempfile::tempdir;

#[test]
fn convert_apply_patch_maps_add_variant() {
    let tmp = tempdir().expect("tmp");
    let path = tmp.path().join("a.txt");
    let path_uri = PathUri::from_host_native_path(&path).expect("absolute test path");
    let action = ApplyPatchAction::new_add_for_test(&path_uri, "hello".to_string());

    let got = convert_apply_patch_to_protocol(&action);

    assert_eq!(
        got.get(path.as_path()),
        Some(&FileChange::Add {
            content: "hello".to_string()
        })
    );
}

#[tokio::test]
async fn apply_patch_accept_edits_uses_cumulative_threshold_before_runtime_delegate() {
    let (session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    turn_context.cccx_main_mode = cccx_policy::MainMode::AcceptEdits;
    let tmp = tempdir().expect("tmp");
    let path = tmp.path().join("large.txt");
    let path_uri = PathUri::from_host_native_path(&path).expect("absolute test path");
    let content = (0..21)
        .map(|i| format!("line-{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let action = ApplyPatchAction::new_add_for_test(&path_uri, content);

    let invocation = apply_patch(
        &session,
        &turn_context,
        &FileSystemSandboxPolicy::unrestricted(),
        action,
        vec![path.to_string_lossy().to_string()],
    )
    .await;

    let InternalApplyPatchInvocation::DelegateToRuntime(runtime_invocation) = invocation else {
        panic!("large AcceptEdits patch should be delegated to runtime approval");
    };
    let ExecApprovalRequirement::NeedsApproval { reason, .. } =
        runtime_invocation.exec_approval_requirement
    else {
        panic!("large AcceptEdits patch should require approval/reroute");
    };
    assert!(
        reason.is_some_and(|reason| reason.contains("mode-threshold-exceeded")),
        "large AcceptEdits patch must include cccx threshold reason"
    );
}
