use super::*;
use std::sync::Arc;

use crate::exec_policy::CccxAutoClassifier;
use crate::exec_policy::CccxAutoClassifierFuture;
use crate::exec_policy::CccxClassifierOutcome;
use crate::exec_policy::ExecPolicyManager;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;

use tempfile::tempdir;

#[derive(Debug)]
struct ApplyPatchAutoClassifier {
    outcome: CccxClassifierOutcome,
}

impl CccxAutoClassifier for ApplyPatchAutoClassifier {
    fn classify(&self, _action: cccx_policy::NormalizedAction) -> CccxAutoClassifierFuture<'_> {
        let outcome = self.outcome.clone();
        Box::pin(async move { outcome })
    }
}

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

#[tokio::test]
async fn apply_patch_auto_mode_uses_session_classifier_before_legacy_safety() {
    let (mut session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    session.services.exec_policy = Arc::new(
        ExecPolicyManager::default().with_cccx_auto_classifier_for_test(Arc::new(
            ApplyPatchAutoClassifier {
                outcome: CccxClassifierOutcome::Available(cccx_policy::ClassifierOutcome::Allow),
            },
        )),
    );
    turn_context.cccx_main_mode = cccx_policy::MainMode::Auto;
    let tmp = tempdir().expect("tmp");
    let path = tmp.path().join("small.txt");
    let path_uri = PathUri::from_host_native_path(&path).expect("absolute test path");
    let action = ApplyPatchAction::new_add_for_test(&path_uri, "hello".to_string());

    let invocation = apply_patch(
        &session,
        &turn_context,
        &FileSystemSandboxPolicy::unrestricted(),
        action,
        vec![path.to_string_lossy().to_string()],
    )
    .await;

    let InternalApplyPatchInvocation::DelegateToRuntime(runtime_invocation) = invocation else {
        panic!("classifier-allowed Auto patch should delegate to runtime without approval");
    };
    let ExecApprovalRequirement::NeedsApproval { reason, .. } =
        runtime_invocation.exec_approval_requirement
    else {
        panic!("classifier-allowed Auto patch should fall through to legacy patch safety approval");
    };
    assert_eq!(
        reason, None,
        "Auto patch must use session classifier allow before legacy safety; cccx classifier gate should not fail closed"
    );
}

#[tokio::test]
async fn apply_patch_auto_mode_fails_closed_when_session_classifier_unavailable() {
    let (mut session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    session.services.exec_policy = Arc::new(
        ExecPolicyManager::default().with_cccx_auto_classifier_for_test(Arc::new(
            ApplyPatchAutoClassifier {
                outcome: CccxClassifierOutcome::Unavailable,
            },
        )),
    );
    turn_context.cccx_main_mode = cccx_policy::MainMode::Auto;
    let tmp = tempdir().expect("tmp");
    let path = tmp.path().join("small.txt");
    let path_uri = PathUri::from_host_native_path(&path).expect("absolute test path");
    let action = ApplyPatchAction::new_add_for_test(&path_uri, "hello".to_string());

    let invocation = apply_patch(
        &session,
        &turn_context,
        &FileSystemSandboxPolicy::unrestricted(),
        action,
        vec![path.to_string_lossy().to_string()],
    )
    .await;

    let InternalApplyPatchInvocation::DelegateToRuntime(runtime_invocation) = invocation else {
        panic!("classifier-unavailable Auto patch should require approval");
    };
    let ExecApprovalRequirement::NeedsApproval {
        reason: Some(reason),
        ..
    } = runtime_invocation.exec_approval_requirement
    else {
        panic!("classifier-unavailable Auto patch should carry fail-closed reason");
    };
    assert_eq!(reason, "classifier_unavailable");
}
