use std::sync::Arc;
use std::sync::Weak;

use axum::http::HeaderValue;
use codex_app_server_protocol::AttestationGenerateParams;
use codex_app_server_protocol::AttestationGenerateResponse;
use codex_app_server_protocol::ServerRequestPayload;
use codex_core::AttestationContext;
use codex_core::AttestationProvider;
use codex_core::BypassSandboxAttestationFuture;
use codex_core::GenerateAttestationFuture;
use serde::Serialize;
use tokio::time::Duration;
use tokio::time::timeout;
use tracing::warn;

use crate::outgoing_message::OutgoingMessageSender;
use crate::thread_state::ThreadStateManager;

const ATTESTATION_GENERATE_TIMEOUT: Duration = Duration::from_millis(100);

pub(crate) fn app_server_attestation_provider(
    outgoing: Arc<OutgoingMessageSender>,
    thread_state_manager: ThreadStateManager,
) -> Arc<dyn AttestationProvider> {
    Arc::new(AppServerAttestationProvider {
        outgoing: Arc::downgrade(&outgoing),
        thread_state_manager,
    })
}

struct AppServerAttestationProvider {
    outgoing: Weak<OutgoingMessageSender>,
    thread_state_manager: ThreadStateManager,
}

impl std::fmt::Debug for AppServerAttestationProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppServerAttestationProvider")
            .finish()
    }
}

impl AttestationProvider for AppServerAttestationProvider {
    fn header_for_request(&self, context: AttestationContext) -> GenerateAttestationFuture<'_> {
        let Some(outgoing) = self.outgoing.upgrade() else {
            return Box::pin(async { None });
        };
        let thread_state_manager = self.thread_state_manager.clone();
        Box::pin(async move {
            request_attestation_header_value_with_timeout(
                outgoing,
                thread_state_manager,
                context.thread_id,
                ATTESTATION_GENERATE_TIMEOUT,
            )
            .await
            .and_then(|value| HeaderValue::from_bytes(value.as_bytes()).ok())
        })
    }

    fn bypass_sandbox_attestation_for(
        &self,
        _context: AttestationContext,
    ) -> BypassSandboxAttestationFuture<'_> {
        // The app-server `AttestationGenerate` capability produces an opaque
        // upstream model-request attestation header. It does not prove that the
        // local tool executor for this thread is isolated by an OS sandbox.
        // Until the app-server protocol exposes a backend-verified sandbox
        // isolation proof bound to the current thread/session, Bypass must stay
        // fail-closed rather than treating model attestation as local sandbox
        // attestation.
        Box::pin(async { None })
    }
}

async fn request_attestation_header_value_with_timeout(
    outgoing: Arc<OutgoingMessageSender>,
    thread_state_manager: ThreadStateManager,
    thread_id: codex_protocol::ThreadId,
    timeout_duration: Duration,
) -> Option<String> {
    let connection_id = thread_state_manager
        .first_attestation_capable_connection_for_thread(thread_id)
        .await?;

    let connection_ids = [connection_id];
    let (request_id, rx) = outgoing
        .send_request_to_connections(
            Some(&connection_ids),
            ServerRequestPayload::AttestationGenerate(AttestationGenerateParams {}),
            /*thread_id*/ None,
        )
        .await;

    let result = match timeout(timeout_duration, rx).await {
        Ok(Ok(Ok(result))) => result,
        Ok(Ok(Err(err))) => {
            warn!(
                code = err.code,
                message = %err.message,
                "attestation generation request failed"
            );
            return app_server_attestation_header_value(
                AppServerAttestationStatus::RequestFailed,
                /*token*/ None,
            );
        }
        Ok(Err(err)) => {
            warn!("attestation generation request canceled: {err}");
            return app_server_attestation_header_value(
                AppServerAttestationStatus::RequestCanceled,
                /*token*/ None,
            );
        }
        Err(_) => {
            let _canceled = outgoing.cancel_request(&request_id).await;
            warn!(
                timeout_seconds = timeout_duration.as_secs(),
                "attestation generation request timed out"
            );
            return app_server_attestation_header_value(
                AppServerAttestationStatus::Timeout,
                /*token*/ None,
            );
        }
    };

    match serde_json::from_value::<AttestationGenerateResponse>(result) {
        Ok(response) => app_server_attestation_header_value(
            AppServerAttestationStatus::Ok,
            Some(&response.token),
        ),
        Err(err) => {
            warn!("failed to deserialize attestation generation response: {err}");
            app_server_attestation_header_value(
                AppServerAttestationStatus::MalformedResponse,
                /*token*/ None,
            )
        }
    }
}

#[derive(Clone, Copy)]
enum AppServerAttestationStatus {
    Ok,
    Timeout,
    RequestFailed,
    RequestCanceled,
    MalformedResponse,
}

impl AppServerAttestationStatus {
    const fn code(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Timeout => 1,
            Self::RequestFailed => 2,
            Self::RequestCanceled => 3,
            Self::MalformedResponse => 4,
        }
    }
}

#[derive(Serialize)]
struct AppServerAttestationEnvelope<'a> {
    v: u8,
    s: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    t: Option<&'a str>,
}

fn app_server_attestation_header_value(
    status: AppServerAttestationStatus,
    token: Option<&str>,
) -> Option<String> {
    serde_json::to_string(&AppServerAttestationEnvelope {
        v: 1,
        s: status.code(),
        t: token,
    })
    .map_err(|err| warn!("failed to serialize app-server attestation envelope: {err}"))
    .ok()
}

#[cfg(test)]
mod tests {
    use super::AppServerAttestationStatus;
    use super::AppServerAttestationProvider;
    use super::app_server_attestation_header_value;
    use crate::thread_state::ThreadStateManager;
    use codex_core::AttestationContext;
    use codex_core::AttestationProvider;
    use codex_protocol::ThreadId;
    use pretty_assertions::assert_eq;
    use std::sync::Weak;

    #[test]
    fn app_server_attestation_header_value_wraps_opaque_client_payloads() {
        assert_eq!(
            app_server_attestation_header_value(
                AppServerAttestationStatus::Ok,
                Some("v1.opaque-client-payload"),
            ),
            Some(r#"{"v":1,"s":0,"t":"v1.opaque-client-payload"}"#.to_string())
        );
    }

    #[test]
    fn app_server_attestation_header_value_reports_app_server_failures() {
        assert_eq!(
            app_server_attestation_header_value(
                AppServerAttestationStatus::Timeout,
                /*token*/ None,
            ),
            Some(r#"{"v":1,"s":1}"#.to_string())
        );
        assert_eq!(
            app_server_attestation_header_value(
                AppServerAttestationStatus::RequestFailed,
                /*token*/ None,
            ),
            Some(r#"{"v":1,"s":2}"#.to_string())
        );
        assert_eq!(
            app_server_attestation_header_value(
                AppServerAttestationStatus::RequestCanceled,
                /*token*/ None,
            ),
            Some(r#"{"v":1,"s":3}"#.to_string())
        );
        assert_eq!(
            app_server_attestation_header_value(
                AppServerAttestationStatus::MalformedResponse,
                /*token*/ None
            ),
            Some(r#"{"v":1,"s":4}"#.to_string())
        );
    }

    #[tokio::test]
    async fn app_server_provider_does_not_mint_bypass_sandbox_attestation() {
        let provider = AppServerAttestationProvider {
            outgoing: Weak::new(),
            thread_state_manager: ThreadStateManager::new(),
        };

        let attestation = provider
            .bypass_sandbox_attestation_for(AttestationContext {
                thread_id: ThreadId::default(),
            })
            .await;

        assert_eq!(attestation, None);
    }
}
