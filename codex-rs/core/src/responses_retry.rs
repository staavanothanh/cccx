//! Shared retry and transport fallback decisions for Responses requests.

use std::time::Duration;

use crate::client::ModelClientSession;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::util::backoff;
use codex_protocol::error::CodexErr;
use codex_protocol::error::RetryLimitReachedError;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WarningEvent;
use tracing::warn;

const CCCX_GATEWAY_MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy)]
pub(crate) enum ResponsesStreamRequest {
    Sampling,
    RemoteCompactionV2,
}

/// Handles a retryable stream error and returns `Ok(())` when the caller should
/// retry the request loop.
pub(crate) async fn handle_retryable_response_stream_error(
    retries: &mut u64,
    max_retries: u64,
    err: CodexErr,
    client_session: &mut ModelClientSession,
    sess: &Session,
    turn_context: &TurnContext,
    request: ResponsesStreamRequest,
) -> Result<(), CodexErr> {
    if *retries >= max_retries
        && client_session.try_switch_fallback_transport(
            &turn_context.session_telemetry,
            &turn_context.model_info,
        )
    {
        client_session.record_gateway_transport_fallback().await;
        sess.send_event(
            turn_context,
            EventMsg::Warning(WarningEvent {
                message: format!("Falling back from WebSockets to HTTPS transport. {err:#}"),
            }),
        )
        .await;
        *retries = 0;
        return Ok(());
    }

    if *retries < max_retries {
        *retries += 1;
        let retry_count = *retries;
        let observed_retry_after = match &err {
            CodexErr::Stream(_, retry_after) => *retry_after,
            _ => None,
        };
        client_session
            .record_gateway_retry_with_retry_after(observed_retry_after)
            .await;
        let delay = retry_delay_for_stream_error(
            &err,
            retry_count,
            codex_features::cccx_mvp::provider_boundary_active(),
        );
        log_retry(request, turn_context, &err, retry_count, max_retries, delay);

        // In release builds, hide the first websocket retry notification to reduce noisy
        // transient reconnect messages. In debug builds, keep full visibility for diagnosis.
        let report_error = retry_count > 1
            || cfg!(debug_assertions)
            || !sess.services.model_client.responses_websocket_enabled();
        if report_error {
            // Surface retry information to any UI/front-end so the user understands what is
            // happening instead of staring at a seemingly frozen screen.
            sess.notify_stream_error(
                turn_context,
                format!("Reconnecting... {retry_count}/{max_retries}"),
                err,
            )
            .await;
        }
        tokio::time::sleep(delay).await;
        return Ok(());
    }

    if codex_features::cccx_mvp::provider_boundary_active()
        && let Some(request_id) = client_session.gateway_rate_limit_request_id().await
    {
        return Err(CodexErr::RetryLimit(RetryLimitReachedError {
            status: http::StatusCode::TOO_MANY_REQUESTS,
            request_id,
        }));
    }

    Err(err)
}

fn retry_delay_for_stream_error(
    err: &CodexErr,
    retry_count: u64,
    cccx_provider_boundary_active: bool,
) -> Duration {
    if let CodexErr::Stream(_, Some(requested_delay)) = err {
        return if cccx_provider_boundary_active {
            (*requested_delay).min(CCCX_GATEWAY_MAX_RETRY_AFTER)
        } else {
            *requested_delay
        };
    }
    if cccx_provider_boundary_active {
        cccx_gateway_backoff(retry_count)
    } else {
        backoff(retry_count)
    }
}

fn cccx_gateway_backoff(attempt: u64) -> Duration {
    match attempt {
        0 | 1 => Duration::from_secs(5),
        2 => Duration::from_secs(15),
        _ => Duration::from_secs(45),
    }
}

fn log_retry(
    request: ResponsesStreamRequest,
    turn_context: &TurnContext,
    err: &CodexErr,
    retries: u64,
    max_retries: u64,
    delay: Duration,
) {
    match request {
        ResponsesStreamRequest::Sampling => {
            warn!(
                "stream disconnected - retrying sampling request ({retries}/{max_retries} in {delay:?})...",
            );
        }
        ResponsesStreamRequest::RemoteCompactionV2 => {
            warn!(
                turn_id = %turn_context.sub_id,
                retries,
                max_retries,
                compact_error = %err,
                "remote compaction v2 stream failed; retrying request after delay"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cccx_gateway_backoff_is_5_15_45() {
        assert_eq!(cccx_gateway_backoff(1), Duration::from_secs(5));
        assert_eq!(cccx_gateway_backoff(2), Duration::from_secs(15));
        assert_eq!(cccx_gateway_backoff(3), Duration::from_secs(45));
        assert_eq!(cccx_gateway_backoff(4), Duration::from_secs(45));
    }

    #[test]
    fn retry_after_overrides_cccx_gateway_backoff() {
        let err = CodexErr::Stream("rate limited".to_string(), Some(Duration::from_secs(23)));

        assert_eq!(
            retry_delay_for_stream_error(&err, 1, true),
            Duration::from_secs(23)
        );
    }

    #[test]
    fn cccx_retry_after_is_clamped_to_a_local_maximum() {
        let err = CodexErr::Stream(
            "rate limited".to_string(),
            Some(Duration::from_secs(3_153_600_000)),
        );

        assert_eq!(
            retry_delay_for_stream_error(&err, 1, true),
            Duration::from_secs(60)
        );
    }
}
