use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cccx_policy::{
    NormalizedAction, PermissionRequest, PermissionRequestQueue, PermissionRisk, QueueError,
};
use tokio::sync::Mutex as AsyncMutex;

/// Wraps cccx-policy's PermissionRequestQueue for thread-safe use in session.
pub struct CccxPermissionQueue {
    inner: Arc<AsyncMutex<PermissionRequestQueue>>,
    counter: AtomicU64,
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
        request_id
    }

    pub async fn approve(&self, request_id: &str, observed_digest: &str) -> Result<(), QueueError> {
        self.inner.lock().await.approve(request_id, observed_digest)
    }

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
        if approved {
            let digest = queue
                .get(request_id)
                .ok_or_else(|| QueueError::UnknownRequest(request_id.to_string()))?
                .digest
                .clone();
            queue.approve(request_id, &digest)
        } else {
            queue.deny(request_id)
        }
    }

    pub async fn cancel_agent_requests(&self, agent_session_id: &str) {
        self.inner
            .lock()
            .await
            .cancel_agent_requests(agent_session_id);
    }

    pub async fn active_request_id(&self) -> Option<String> {
        self.inner
            .lock()
            .await
            .active_request_id()
            .map(String::from)
    }

    pub async fn get(&self, request_id: &str) -> Option<PermissionRequest> {
        self.inner.lock().await.get(request_id).cloned()
    }

    /// Batch-approve queued requests that are NOT circuit-breaker/high-risk.
    /// Returns the IDs that were approved.
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
        assert_eq!(queue.active_request_id().await.as_deref(), Some(second.as_str()));
    }
}
