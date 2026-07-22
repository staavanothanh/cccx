use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cccx_policy::{
    NormalizedAction, PermissionRequest, PermissionRequestQueue, PermissionRequestStatus,
    PermissionRisk, QueueError,
};
use codex_protocol::mcp::RequestId as ProtocolMcpRequestId;
use codex_protocol::request_permissions::RequestPermissionProfile;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::Notify;

/// Wraps cccx-policy's PermissionRequestQueue for thread-safe use in session.
pub struct CccxPermissionQueue {
    inner: Arc<AsyncMutex<PermissionRequestQueue>>,
    /// The queue owns the correlation between its stable `pr-*` identity and
    /// the native Codex approval waiter that must receive the decision.
    native_approvals: Arc<AsyncMutex<BTreeMap<String, NativeApproval>>>,
    state_changed: Arc<Notify>,
    counter: AtomicU64,
}

/// Native approval waiter correlated with a CCCX permission request.
///
/// This deliberately stores only opaque IDs. The decision is still delivered
/// through the worker session's normal `Op::{ExecApproval, PatchApproval}`
/// path, so one queued request cannot resolve another worker's prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NativeApproval {
    Exec {
        thread_id: String,
        approval_id: String,
        turn_id: String,
    },
    Patch {
        thread_id: String,
        approval_id: String,
    },
    RequestPermissions {
        thread_id: String,
        call_id: String,
        requested_permissions: RequestPermissionProfile,
    },
    McpUserInput {
        thread_id: String,
        turn_id: String,
        question_id: String,
        approve_answer: String,
        deny_answer: String,
    },
    McpElicitation {
        thread_id: String,
        server_name: String,
        request_id: ProtocolMcpRequestId,
    },
}

impl Default for CccxPermissionQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl CccxPermissionQueue {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AsyncMutex::new(PermissionRequestQueue::default())),
            native_approvals: Arc::new(AsyncMutex::new(BTreeMap::new())),
            state_changed: Arc::new(Notify::new()),
            counter: AtomicU64::new(0),
        }
    }

    pub async fn enqueue(
        &self,
        agent_session_id: String,
        agent_display_name: String,
        task_id: String,
        action: NormalizedAction,
        reason: String,
    ) -> String {
        let idx = self.counter.fetch_add(1, Ordering::Relaxed);
        let request_id = format!("pr-{idx}");
        let risk = match action.action_type {
            cccx_policy::ActionType::Shell => PermissionRisk::High,
            _ => PermissionRisk::Medium,
        };
        let request = PermissionRequest::new(&request_id, &agent_session_id, action, risk)
            .with_context(agent_display_name, task_id, reason);
        self.inner.lock().await.enqueue(request);
        self.state_changed.notify_waiters();
        request_id
    }

    #[cfg(test)]
    pub async fn approve(&self, request_id: &str, observed_digest: &str) -> Result<(), QueueError> {
        self.inner.lock().await.approve(request_id, observed_digest)
    }

    #[cfg(test)]
    pub async fn deny(&self, request_id: &str) -> Result<(), QueueError> {
        self.inner.lock().await.deny(request_id)
    }

    /// Resolve the cccx queue entry when the native Codex approval UI has
    /// returned. Approval always reuses the digest captured at enqueue time;
    /// a stale or cancelled entry remains rejected by the policy queue.
    pub async fn resolve_native_decision(
        &self,
        request_id: &str,
        approved: bool,
    ) -> Result<(), QueueError> {
        let mut queue = self.inner.lock().await;
        let result = if approved {
            let digest = queue
                .get(request_id)
                .ok_or_else(|| QueueError::UnknownRequest(request_id.to_string()))?
                .digest
                .clone();
            queue.approve(request_id, &digest)
        } else {
            queue.deny(request_id)
        };
        drop(queue);
        self.state_changed.notify_waiters();
        result
    }

    /// Wait until a request becomes the single active prompt that may be
    /// rendered to the user. Returns `false` when the request reaches any
    /// terminal/non-queued state before activation.
    pub async fn wait_until_active(&self, request_id: &str) -> bool {
        loop {
            let notified = self.state_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let queue = self.inner.lock().await;
                match queue.get(request_id).map(|request| &request.status) {
                    Some(PermissionRequestStatus::Active) => return true,
                    Some(PermissionRequestStatus::Queued) => {}
                    Some(_) | None => return false,
                }
            }
            notified.await;
        }
    }

    pub async fn register_native_approval(
        &self,
        request_id: String,
        native_approval: NativeApproval,
    ) {
        self.native_approvals
            .lock()
            .await
            .insert(request_id, native_approval);
    }

    pub async fn native_approval(&self, request_id: &str) -> Option<NativeApproval> {
        self.native_approvals.lock().await.get(request_id).cloned()
    }

    pub async fn forget_native_approval(&self, request_id: &str) {
        self.native_approvals.lock().await.remove(request_id);
    }

    pub async fn cancel_agent_requests(&self, agent_session_id: &str) {
        let request_ids = self
            .inner
            .lock()
            .await
            .list_all()
            .into_iter()
            .filter(|request| request.agent_id == agent_session_id)
            .map(|request| request.request_id)
            .collect::<Vec<_>>();
        self.inner
            .lock()
            .await
            .cancel_agent_requests(agent_session_id);
        self.state_changed.notify_waiters();
        let mut native_approvals = self.native_approvals.lock().await;
        for request_id in request_ids {
            native_approvals.remove(&request_id);
        }
    }

    pub async fn cancel_request(&self, request_id: &str) {
        if let Err(error) = self.inner.lock().await.cancel_request(request_id) {
            tracing::warn!(%request_id, %error, "cccx permission request cancellation was stale");
        }
        self.native_approvals.lock().await.remove(request_id);
        self.state_changed.notify_waiters();
    }

    #[cfg(test)]
    pub async fn active_request_id(&self) -> Option<String> {
        self.inner
            .lock()
            .await
            .active_request_id()
            .map(String::from)
    }

    pub async fn ensure_active(&self, request_id: &str) -> Result<(), String> {
        let queue = self.inner.lock().await;
        let request = queue
            .get(request_id)
            .ok_or_else(|| format!("unknown permission request `{request_id}`"))?;
        if request.status == PermissionRequestStatus::Active {
            Ok(())
        } else {
            Err(format!(
                "permission request `{request_id}` is not active; status is {:?}",
                request.status
            ))
        }
    }

    /// Return the only request that may be auto-approved by a batch
    /// controller. Queued requests must wait until the native decision for
    /// the current active request completes and promotes them.
    pub async fn active_safe_request_id(&self) -> Option<String> {
        let queue = self.inner.lock().await;
        let request_id = queue.active_request_id()?;
        let request = queue.get(request_id)?;
        if request.risk == PermissionRisk::High || request.risk == PermissionRisk::CircuitBreaker {
            return None;
        }
        let request_id = request_id.to_string();
        drop(queue);
        matches!(
            self.native_approval(&request_id).await,
            Some(NativeApproval::Patch { .. })
        )
        .then_some(request_id)
    }

    pub async fn validate_batch_request_ids(&self, request_ids: &[String]) -> Result<(), String> {
        if request_ids.is_empty() {
            return Err("batch approval requires at least one request id".to_string());
        }
        let queue = self.inner.lock().await;
        let mut seen = BTreeSet::new();
        for request_id in request_ids {
            if !seen.insert(request_id.as_str()) {
                return Err(format!(
                    "batch approval contains duplicate permission request `{request_id}`"
                ));
            }
            let request = queue
                .get(request_id)
                .ok_or_else(|| format!("unknown permission request `{request_id}`"))?;
            if !matches!(
                request.status,
                PermissionRequestStatus::Active | PermissionRequestStatus::Queued
            ) {
                return Err(format!(
                    "permission request `{request_id}` is not pending; status is {:?}",
                    request.status
                ));
            }
            if matches!(
                request.risk,
                PermissionRisk::High | PermissionRisk::CircuitBreaker
            ) {
                return Err(format!(
                    "permission request `{request_id}` is {:?} risk and cannot be batch approved",
                    request.risk
                ));
            }
        }
        let Some(active_id) = queue.active_request_id() else {
            return Err("permission queue has no active request".to_string());
        };
        if request_ids.first().map(String::as_str) != Some(active_id) {
            return Err(format!(
                "batch approval must start with the current active request `{active_id}`"
            ));
        }
        let pending_ids = queue.pending_request_ids_ordered();
        let expected_prefix = pending_ids
            .iter()
            .take(request_ids.len())
            .map(String::as_str)
            .collect::<Vec<_>>();
        let requested = request_ids.iter().map(String::as_str).collect::<Vec<_>>();
        if requested != expected_prefix {
            return Err(format!(
                "batch approval request ids must be the exact active-queue prefix: {}",
                expected_prefix.join(", ")
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub async fn get(&self, request_id: &str) -> Option<PermissionRequest> {
        self.inner.lock().await.get(request_id).cloned()
    }

    /// Test-only access to the policy queue's batch primitive. Production uses
    /// the sequential native-approval controller above instead, so a worker's
    /// native waiter is never orphaned.
    #[cfg(test)]
    pub async fn batch_approve_safe(&self) -> Vec<String> {
        self.inner.lock().await.batch_approve_safe()
    }

    /// List all active + queued requests for `/permissions list` display.
    pub async fn list_all(&self) -> Vec<PermissionRequest> {
        self.inner.lock().await.list_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cccx_policy::ActionType;
    use cccx_policy::PermissionRequestStatus;

    fn action(task_id: &str) -> NormalizedAction {
        NormalizedAction {
            action_type: ActionType::Edit,
            summary: "edit source".to_string(),
            command: None,
            target_paths: vec!["D:/repo/src/lib.rs".to_string()],
            tool_name: Some("apply_patch".to_string()),
            task_id: task_id.to_string(),
            changed_files: 1,
            changed_lines: 4,
        }
    }

    #[tokio::test]
    async fn denying_one_prompt_must_not_cancel_other_prompts_from_same_agent() {
        let queue = CccxPermissionQueue::new();
        let first = queue
            .enqueue(
                "agent-1".to_string(),
                "worker".to_string(),
                "T1".to_string(),
                action("T1"),
                "first edit".to_string(),
            )
            .await;
        let second = queue
            .enqueue(
                "agent-1".to_string(),
                "worker".to_string(),
                "T2".to_string(),
                action("T2"),
                "second edit".to_string(),
            )
            .await;

        queue.deny(&first).await.expect("deny active prompt");

        let first_request = queue.get(&first).await.expect("first request exists");
        assert_eq!(first_request.agent_display_name, "worker");
        assert_eq!(first_request.task_id, "T1");
        assert_eq!(first_request.reason, "first edit");
        assert_eq!(
            first_request.status,
            PermissionRequestStatus::Denied,
            "denied request should be marked denied, not bulk-cancelled"
        );
        assert_eq!(
            queue.active_request_id().await.as_deref(),
            Some(second.as_str()),
            "queue should promote the next prompt instead of cancelling same-agent prompts"
        );
        assert_eq!(
            queue.get(&second).await.map(|request| request.status),
            Some(PermissionRequestStatus::Active)
        );
    }

    // ── Phase 7 tests ──

    /// Phase 7: approve with wrong digest must reject.
    /// task.md: "approve với digest mismatch → reject."
    #[tokio::test]
    async fn p7_approve_rejects_digest_mismatch() {
        let queue = CccxPermissionQueue::new();
        let request_id = queue
            .enqueue(
                "agent-1".to_string(),
                "worker".to_string(),
                "T1".to_string(),
                action("T1"),
                "edit file".to_string(),
            )
            .await;

        // Approve with wrong digest → QueueError::DigestMismatch
        let result = queue.approve(&request_id, "wrong-digest").await;
        assert!(
            matches!(result, Err(QueueError::DigestMismatch)),
            "Phase 7: approve with digest mismatch must return DigestMismatch. Got: {:?}",
            result
        );

        // The request must be expired (not approved)
        let request = queue.get(&request_id).await.unwrap();
        assert_eq!(
            request.status,
            PermissionRequestStatus::Expired,
            "Phase 7: digest mismatch must expire the request"
        );
    }

    /// Phase 7: batch_approve_safe must NOT approve High-risk requests.
    /// task.md: "batch approve không nhận high-risk/circuit breaker."
    #[tokio::test]
    async fn p7_batch_approve_safe_rejects_high_risk() {
        let queue = CccxPermissionQueue::new();

        // First request becomes Active (Edit → Medium risk)
        let active_id = queue
            .enqueue(
                "agent-1".to_string(),
                "worker".to_string(),
                "T1".to_string(),
                action("T1"), // ActionType::Edit → Medium risk
                "edit file".to_string(),
            )
            .await;

        // Second request: Queued (Edit → Medium risk)
        let queued_medium = queue
            .enqueue(
                "agent-1".to_string(),
                "worker".to_string(),
                "T2".to_string(),
                action("T2"), // Edit → Medium
                "another edit".to_string(),
            )
            .await;

        // Third request: Queued (Shell → High risk via enqueue scoring)
        let shell_action = NormalizedAction {
            action_type: ActionType::Shell,
            summary: "run command".to_string(),
            command: Some("npm test".to_string()),
            target_paths: vec![],
            tool_name: Some("shell_command".to_string()),
            task_id: "T3".to_string(),
            changed_files: 0,
            changed_lines: 0,
        };
        let _queued_high = queue
            .enqueue(
                "agent-1".to_string(),
                "worker".to_string(),
                "T3".to_string(),
                shell_action,
                "run tests".to_string(),
            )
            .await;

        // Release the Active so both queued are now Active + Queued
        queue.deny(&active_id).await.unwrap();
        // Now: T2 is Active (promoted), T3 is Queued (High risk)
        // batch_approve_safe only processes status==Queued AND risk!=High
        let approved = queue.batch_approve_safe().await;

        // Only the Medium-risk queued (T3 stayed Queued & High) should return here
        // T2 became Active (not matching status==Queued), so batch should see only T3
        // T3 is High risk → excluded
        assert!(
            approved.is_empty(),
            "Phase 7: batch_approve_safe must not include High-risk Shell. Got {:?}",
            approved
        );

        // Verify: the queued Medium request was promoted to Active (not via batch)
        assert_eq!(
            queue.active_request_id().await.unwrap(),
            queued_medium,
            "Phase 7: after deny, queued Medium should be promoted to Active"
        );
    }

    /// Phase 7: single active request invariant — only one prompt at a time.
    /// task.md: "Chỉ một prompt active tại một thời điểm."
    #[tokio::test]
    async fn p7_only_one_active_prompt_at_a_time() {
        let queue = CccxPermissionQueue::new();
        let first = queue
            .enqueue(
                "agent-1".to_string(),
                "w1".to_string(),
                "T1".to_string(),
                action("T1"),
                "first".to_string(),
            )
            .await;
        let second = queue
            .enqueue(
                "agent-1".to_string(),
                "w1".to_string(),
                "T2".to_string(),
                action("T2"),
                "second".to_string(),
            )
            .await;
        let _third = queue
            .enqueue(
                "agent-2".to_string(),
                "w2".to_string(),
                "T3".to_string(),
                action("T3"),
                "third".to_string(),
            )
            .await;

        // Only one active
        assert_eq!(
            queue.active_request_id().await.unwrap(),
            first,
            "Phase 7: first request must be active"
        );

        // After denying the active one, second gets promoted
        queue.deny(&first).await.unwrap();
        let active_after = queue.active_request_id().await;
        assert_eq!(
            active_after.as_deref(),
            Some(second.as_str()),
            "Phase 7: after deny, second request must be promoted to active"
        );
    }

    #[tokio::test]
    async fn native_approval_resolution_clears_the_active_queue_entry() {
        let queue = CccxPermissionQueue::new();
        let first = queue
            .enqueue(
                "agent-1".to_string(),
                "worker-1".to_string(),
                "T1".to_string(),
                action("T1"),
                "first approval".to_string(),
            )
            .await;
        let second = queue
            .enqueue(
                "agent-2".to_string(),
                "worker-2".to_string(),
                "T2".to_string(),
                action("T2"),
                "second approval".to_string(),
            )
            .await;

        queue
            .resolve_native_decision(&first, true)
            .await
            .expect("native approval should resolve the active queue request");

        assert_eq!(
            queue.get(&first).await.map(|request| request.status),
            Some(PermissionRequestStatus::Approved)
        );
        assert_eq!(
            queue.active_request_id().await.as_deref(),
            Some(second.as_str())
        );
    }

    #[tokio::test]
    async fn native_approval_mapping_is_isolated_and_removed_on_worker_cancel() {
        let queue = CccxPermissionQueue::new();
        let request_id = queue
            .enqueue(
                "worker-a".to_string(),
                "worker A".to_string(),
                "T1".to_string(),
                action("T1"),
                "needs approval".to_string(),
            )
            .await;
        let native = NativeApproval::Exec {
            thread_id: "worker-a".to_string(),
            approval_id: "approval-a".to_string(),
            turn_id: "turn-a".to_string(),
        };
        queue
            .register_native_approval(request_id.clone(), native.clone())
            .await;

        assert_eq!(queue.native_approval(&request_id).await, Some(native));
        queue.cancel_agent_requests("worker-a").await;
        assert_eq!(queue.native_approval(&request_id).await, None);
    }

    #[tokio::test]
    async fn active_safe_request_excludes_high_risk_native_waiters() {
        let queue = CccxPermissionQueue::new();
        let shell = NormalizedAction {
            action_type: ActionType::Shell,
            summary: "unsafe command".to_string(),
            command: Some("Remove-Item".to_string()),
            target_paths: Vec::new(),
            tool_name: Some("shell_command".to_string()),
            task_id: "T1".to_string(),
            changed_files: 0,
            changed_lines: 0,
        };
        queue
            .enqueue(
                "worker-a".to_string(),
                "worker A".to_string(),
                "T1".to_string(),
                shell,
                "requires shell approval".to_string(),
            )
            .await;
        assert_eq!(queue.active_safe_request_id().await, None);
    }

    #[tokio::test]
    async fn queued_native_prompt_waits_until_it_is_active() {
        let queue = CccxPermissionQueue::new();
        let first = queue
            .enqueue(
                "worker-a".to_string(),
                "worker A".to_string(),
                "T1".to_string(),
                action("T1"),
                "first".to_string(),
            )
            .await;
        let second = queue
            .enqueue(
                "worker-b".to_string(),
                "worker B".to_string(),
                "T2".to_string(),
                action("T2"),
                "second".to_string(),
            )
            .await;

        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                queue.wait_until_active(&second),
            )
            .await
            .is_err(),
            "queued native prompt must not render before it becomes active"
        );

        queue
            .resolve_native_decision(&first, false)
            .await
            .expect("deny active");
        assert!(queue.wait_until_active(&second).await);
    }

    #[tokio::test]
    async fn queued_native_prompt_aborts_when_cancelled_before_activation() {
        let queue = CccxPermissionQueue::new();
        queue
            .enqueue(
                "worker-a".to_string(),
                "worker A".to_string(),
                "T1".to_string(),
                action("T1"),
                "first".to_string(),
            )
            .await;
        let second = queue
            .enqueue(
                "worker-b".to_string(),
                "worker B".to_string(),
                "T2".to_string(),
                action("T2"),
                "second".to_string(),
            )
            .await;

        queue.cancel_agent_requests("worker-b").await;

        assert!(
            !queue.wait_until_active(&second).await,
            "cancelled queued prompt must fail closed instead of rendering later"
        );
    }

    #[tokio::test]
    async fn cancelling_one_request_preserves_same_worker_sibling() {
        let queue = CccxPermissionQueue::new();
        let first = queue
            .enqueue(
                "worker-a".to_string(),
                "worker A".to_string(),
                "T1".to_string(),
                action("T1"),
                "first".to_string(),
            )
            .await;
        let second = queue
            .enqueue(
                "worker-a".to_string(),
                "worker A".to_string(),
                "T2".to_string(),
                action("T2"),
                "second".to_string(),
            )
            .await;

        queue.cancel_request(&first).await;

        assert_eq!(
            queue.get(&first).await.map(|request| request.status),
            Some(PermissionRequestStatus::Cancelled)
        );
        assert_eq!(
            queue.active_request_id().await.as_deref(),
            Some(second.as_str())
        );
    }

    #[tokio::test]
    async fn batch_validation_requires_explicit_active_safe_ids() {
        let queue = CccxPermissionQueue::new();
        let active = queue
            .enqueue(
                "worker-a".to_string(),
                "worker A".to_string(),
                "T1".to_string(),
                action("T1"),
                "first".to_string(),
            )
            .await;
        let queued = queue
            .enqueue(
                "worker-b".to_string(),
                "worker B".to_string(),
                "T2".to_string(),
                action("T2"),
                "second".to_string(),
            )
            .await;

        assert!(
            queue
                .validate_batch_request_ids(std::slice::from_ref(&queued))
                .await
                .expect_err("batch must start with current active request")
                .contains(&active)
        );
        queue
            .validate_batch_request_ids(&[active.clone(), queued])
            .await
            .expect("active and queued medium-risk IDs are eligible");

        let high = NormalizedAction {
            action_type: ActionType::Shell,
            summary: "run command".to_string(),
            command: Some("npm test".to_string()),
            target_paths: vec![],
            tool_name: Some("shell_command".to_string()),
            task_id: "T3".to_string(),
            changed_files: 0,
            changed_lines: 0,
        };
        queue.resolve_native_decision(&active, false).await.unwrap();
        let high_id = queue
            .enqueue(
                "worker-c".to_string(),
                "worker C".to_string(),
                "T3".to_string(),
                high,
                "run tests".to_string(),
            )
            .await;
        assert!(
            queue
                .validate_batch_request_ids(&[high_id])
                .await
                .expect_err("high risk cannot be batch-approved")
                .contains("High")
        );
    }

    #[tokio::test]
    async fn batch_validation_rejects_duplicate_reordered_or_noncontiguous_ids() {
        let queue = CccxPermissionQueue::new();
        let active = queue
            .enqueue(
                "worker-a".to_string(),
                "worker A".to_string(),
                "T1".to_string(),
                action("T1"),
                "first".to_string(),
            )
            .await;
        let second = queue
            .enqueue(
                "worker-b".to_string(),
                "worker B".to_string(),
                "T2".to_string(),
                action("T2"),
                "second".to_string(),
            )
            .await;
        let third = queue
            .enqueue(
                "worker-c".to_string(),
                "worker C".to_string(),
                "T3".to_string(),
                action("T3"),
                "third".to_string(),
            )
            .await;

        assert!(
            queue
                .validate_batch_request_ids(&[active.clone(), active.clone()])
                .await
                .expect_err("duplicate request ids must be rejected")
                .contains("duplicate")
        );
        assert!(
            queue
                .validate_batch_request_ids(&[second.clone(), active.clone()])
                .await
                .expect_err("reordered request ids must be rejected")
                .contains(&active)
        );
        assert!(
            queue
                .validate_batch_request_ids(&[active.clone(), third])
                .await
                .expect_err("batch ids must be a contiguous active-queue prefix")
                .contains(&second)
        );
        queue
            .validate_batch_request_ids(&[active, second])
            .await
            .expect("active-queue prefix should be eligible");
    }
}
