use codex_http_client::Request;
use codex_http_client::TransportError;
use rand::Rng;
use std::future::Future;
use std::time::Duration;
use std::time::SystemTime;
use tokio::time::sleep;

const MAX_RETRY_AFTER_DELAY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u64,
    pub base_delay: Duration,
    pub retry_on: RetryOn,
}

#[derive(Debug, Clone)]
pub struct RetryOn {
    pub retry_429: bool,
    pub retry_5xx: bool,
    pub retry_transport: bool,
}

impl RetryOn {
    pub fn should_retry(&self, err: &TransportError, attempt: u64, max_attempts: u64) -> bool {
        if attempt >= max_attempts {
            return false;
        }
        match err {
            TransportError::Http { status, .. } => {
                (self.retry_429 && status.as_u16() == 429)
                    || (self.retry_5xx && status.is_server_error())
            }
            TransportError::Timeout | TransportError::Network(_) => self.retry_transport,
            _ => false,
        }
    }
}

pub fn backoff(base: Duration, attempt: u64) -> Duration {
    if attempt == 0 {
        return base;
    }
    let exp = 2u64.saturating_pow(attempt as u32 - 1);
    let millis = base.as_millis() as u64;
    let raw = millis.saturating_mul(exp);
    let jitter: f64 = rand::rng().random_range(0.9..1.1);
    Duration::from_millis((raw as f64 * jitter) as u64)
}

pub async fn run_with_retry<T, F, Fut>(
    policy: RetryPolicy,
    mut make_req: impl FnMut() -> Request,
    op: F,
) -> Result<T, TransportError>
where
    F: Fn(Request, u64) -> Fut,
    Fut: Future<Output = Result<T, TransportError>>,
{
    for attempt in 0..=policy.max_attempts {
        let req = make_req();
        match op(req, attempt).await {
            Ok(resp) => return Ok(resp),
            Err(err)
                if policy
                    .retry_on
                    .should_retry(&err, attempt, policy.max_attempts) =>
            {
                let fallback = backoff(policy.base_delay, attempt + 1);
                sleep(retry_after_delay(&err, fallback)).await;
            }
            Err(err) => return Err(err),
        }
    }
    Err(TransportError::RetryLimit)
}

fn retry_after_delay(err: &TransportError, fallback: Duration) -> Duration {
    let TransportError::Http {
        status, headers, ..
    } = err
    else {
        return fallback;
    };
    if status.as_u16() != 429 {
        return fallback;
    }
    let Some(value) = headers
        .as_ref()
        .and_then(|headers| headers.get(http::header::RETRY_AFTER))
        .and_then(|value| value.to_str().ok())
    else {
        return fallback;
    };
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Duration::from_secs(seconds).min(MAX_RETRY_AFTER_DELAY);
    }
    httpdate::parse_http_date(value)
        .ok()
        .and_then(|deadline| deadline.duration_since(SystemTime::now()).ok())
        .map(|delay| delay.min(MAX_RETRY_AFTER_DELAY))
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;

    #[test]
    fn retry_after_is_clamped_to_a_local_maximum() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::RETRY_AFTER,
            http::HeaderValue::from_static("3153600000"),
        );
        let error = TransportError::Http {
            status: http::StatusCode::TOO_MANY_REQUESTS,
            url: Some("https://example.com/v1/responses".to_string()),
            headers: Some(headers),
            body: None,
        };

        assert_eq!(
            retry_after_delay(&error, Duration::from_secs(1)),
            Duration::from_secs(60)
        );
    }
}
