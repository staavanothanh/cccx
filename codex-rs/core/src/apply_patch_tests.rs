use super::*;
use std::sync::Arc;

use crate::exec_policy::CccxAutoClassifier;
use crate::exec_policy::CccxAutoClassifierFuture;
use crate::exec_policy::CccxClassifierOutcome;
use crate::exec_policy::ExecPolicyManager;
use codex_apply_patch::MaybeApplyPatchVerified;
use codex_exec_server::LOCAL_FS;
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

#[test]
fn cccx_child_write_scope_rejects_target_outside_projected_root() {
    let tmp = tempdir().expect("tmp");
    let allowed = tmp.path().join("allowed");
    let outside = tmp.path().join("outside").join("escape.txt");
    std::fs::create_dir_all(&allowed).expect("allowed root");
    std::fs::create_dir_all(outside.parent().expect("outside parent")).expect("outside root");
    let action = ApplyPatchAction::new_add_for_test(
        &PathUri::from_host_native_path(&outside).expect("absolute path"),
        "blocked".to_string(),
    );

    let result = cccx_validate_write_scope(
        &[allowed.to_string_lossy().to_string()]
            .into_iter()
            .collect(),
        &action,
    );

    assert!(
        result.is_err_and(|reason| reason.contains("outside the projected writable scope")),
        "child executor must reject a patch target outside its projected writable root"
    );
}

#[test]
fn cccx_child_write_scope_rejects_parent_traversal() {
    let tmp = tempdir().expect("tmp");
    let allowed = tmp.path().join("allowed");
    std::fs::create_dir_all(&allowed).expect("allowed root");
    let escaped = allowed.join("..").join("outside.txt");
    let action = ApplyPatchAction::new_add_for_test(
        &PathUri::from_host_native_path(&escaped).expect("absolute path"),
        "blocked".to_string(),
    );

    let result = cccx_validate_write_scope(
        &[allowed.to_string_lossy().to_string()]
            .into_iter()
            .collect(),
        &action,
    );

    assert!(
        result.is_err(),
        "path traversal must not escape child scope"
    );
}

#[tokio::test]
async fn cccx_child_write_scope_rejects_move_destination_outside_projected_root() {
    let tmp = tempdir().expect("tmp");
    let allowed = tmp.path().join("allowed");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&allowed).expect("allowed root");
    std::fs::create_dir_all(&outside).expect("outside root");
    std::fs::write(allowed.join("source.txt"), "before\n").expect("source file");
    let patch = r#"*** Begin Patch
*** Update File: allowed/source.txt
*** Move to: outside/escaped.txt
@@
-before
+after
*** End Patch"#;
    let cwd = PathUri::from_host_native_path(tmp.path()).expect("absolute cwd");
    let action = match codex_apply_patch::maybe_parse_apply_patch_verified(
        &["apply_patch".to_string(), patch.to_string()],
        &cwd,
        LOCAL_FS.as_ref(),
        None,
    )
    .await
    {
        MaybeApplyPatchVerified::Body(action) => action,
        other => panic!("expected verified patch, got {other:?}"),
    };

    let result = cccx_validate_write_scope(
        &[allowed.to_string_lossy().to_string()]
            .into_iter()
            .collect(),
        &action,
    );

    assert!(
        result.is_err_and(|reason| reason.contains("outside the projected writable scope")),
        "a move destination outside the child scope must be rejected"
    );
}

#[tokio::test]
async fn cccx_stale_read_gate_blocks_patch_before_runtime_execution() {
    let (session, turn_context) = crate::session::tests::make_session_and_context().await;
    let tmp = tempdir().expect("tmp");
    let file = tmp.path().join("tracked.txt");
    std::fs::write(&file, "before\n").expect("initial file");
    let patch = r#"*** Begin Patch
*** Update File: tracked.txt
@@
-before
+after
*** End Patch"#;
    let cwd = PathUri::from_host_native_path(tmp.path()).expect("absolute cwd");
    let action = match codex_apply_patch::maybe_parse_apply_patch_verified(
        &["apply_patch".to_string(), patch.to_string()],
        &cwd,
        LOCAL_FS.as_ref(),
        None,
    )
    .await
    {
        MaybeApplyPatchVerified::Body(action) => action,
        other => panic!("expected verified patch, got {other:?}"),
    };
    turn_context
        .record_cccx_read(
            std::fs::canonicalize(&file).expect("canonical file"),
            b"before\n",
        )
        .await;
    std::fs::write(&file, "changed elsewhere\n").expect("external change");

    let result = apply_patch(
        &session,
        &turn_context,
        &FileSystemSandboxPolicy::unrestricted(),
        action,
        vec![file.to_string_lossy().to_string()],
    )
    .await;

    assert!(
        matches!(result, InternalApplyPatchInvocation::Output(Err(FunctionCallError::RespondToModel(message))) if message.contains("stale-read gate")),
        "a direct re-read mismatch must stop the patch before it reaches the runtime"
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
