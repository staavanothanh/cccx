use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cccx_policy::{
    NormalizedAction, PermissionRequest, PermissionRequestQueue, PermissionRisk, QueueError,
};
use tokio::sync::Mutex as AsyncMutex;

/// Wraps cccx-policy's PermissionRequestQueue for thread-safe use in session.
/// Wraps cccx-policy's PermissionRequestQueue for thread-safe use in session.
// TODO(cccx-phase6): Instantiate in Session::new() and wire into
// exec_policy permission dispatch path.
#[allow(dead_code)]
pub struct CccxPermissionQueue {
    inner: Arc<AsyncMutex<PermissionRequestQueue>>,
    counter: AtomicU64,
}

#[allow(dead_code)]
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
        _agent_display_name: String,
        _task_id: String,
        action: NormalizedAction,
        _reason: String,
    ) -> String {
        let idx = self.counter.fetch_add(1, Ordering::Relaxed);
        let request_id = format!("pr-{idx}");
        let risk = match action.action_type {
            cccx_policy::ActionType::Shell => PermissionRisk::High,
            _ => PermissionRisk::Medium,
        };
        let request = PermissionRequest::new(&request_id, &agent_session_id, action, risk);
        self.inner.lock().await.enqueue(request);
        request_id
    }

    pub async fn approve(&self, request_id: &str, observed_digest: &str) -> Result<(), QueueError> {
        self.inner.lock().await.approve(request_id, observed_digest)
    }

    pub async fn deny(&self, request_id: &str) -> Result<(), QueueError> {
        self.inner.lock().await.deny(request_id)
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

        assert_eq!(
            queue.get(&first).await.map(|request| request.status),
            Some(PermissionRequestStatus::Denied),
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
}
