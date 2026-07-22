/// Immutable snapshot of one HTTP/SSE transport attempt inside a logical
/// model call. The top-level [`GatewayProvenance`] remains the aggregate/latest
/// compatibility view used by existing Artifact consumers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayAttemptProvenance {
    pub attempt_index: u32,
    pub gateway_request_id: String,
    pub endpoint: Option<String>,
    pub wire_api: Option<String>,
    pub input_digest: Option<String>,
    pub output_digest: Option<String>,
    pub http_status: Option<u16>,
    pub retry_after_ms: Option<u64>,
    pub stream_termination_status: Option<String>,
    pub upstream_request_id: Option<String>,
    pub response_id: Option<String>,
    pub observed_server_model: Option<String>,
}

/// Provenance record for a logical model call through 9router.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GatewayProvenance {
    pub gateway_request_id: String,
    pub requested_route_id: String,
    pub effective_route_id: Option<String>,
    pub effective_provider: Option<String>,
    pub effective_model: Option<String>,
    pub upstream_request_id: Option<String>,
    pub fallback_chain: Vec<String>,
    /// Client-observed transport transitions. This is deliberately separate
    /// from `fallback_chain`, which is reserved for factual router/provider
    /// fallback metadata.
    #[serde(default)]
    pub transport_fallbacks: Vec<String>,
    pub retry_count: u32,
    /// Which `FallbackPolicyId` 9router applied for this request.
    pub fallback_policy_id: Option<crate::FallbackPolicyId>,
    /// The wire API used for this call (e.g. `chat_completions`, `responses`).
    pub wire_api: Option<String>,
    /// Outcome of the stream: `completed`, `truncated`, `timeout`, `error`.
    pub stream_termination_status: Option<String>,
    /// Logical model-call kind observed by cccx (for example `stream`,
    /// `compact`, `memory_summarize`, `classifier`). This is client-side
    /// provenance, not router metadata.
    #[serde(default)]
    pub call_kind: Option<String>,
    /// Router endpoint used by the attempt, such as `/v1/responses` or
    /// `/v1/chat/completions`.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Digest of the exact serialized payload sent by cccx.
    #[serde(default)]
    pub input_digest: Option<String>,
    /// Digest of factual output observed by cccx from the gateway response.
    #[serde(default)]
    pub output_digest: Option<String>,
    /// Response id emitted by the provider/gateway protocol, when present.
    #[serde(default)]
    pub response_id: Option<String>,
    /// Model label observed in response headers/body/stream chunks. This is
    /// protocol-reported output, not 9router effective route metadata.
    #[serde(default)]
    pub observed_server_model: Option<String>,
    /// HTTP status observed for non-stream setup/unary calls.
    #[serde(default)]
    pub http_status: Option<u16>,
    /// Retry-After observed from a real response header, in milliseconds.
    #[serde(default)]
    pub retry_after_ms: Option<u64>,
    /// Attempt-local copy used when snapshotting the journal. The public field
    /// above is also carried as the logical-call aggregate across retries.
    #[serde(skip)]
    current_attempt_retry_after_ms: Option<u64>,
    /// Ordered factual snapshots for every HTTP/SSE attempt. This prevents a
    /// successful retry from erasing the failed attempt that preceded it.
    #[serde(default)]
    pub attempts: Vec<GatewayAttemptProvenance>,
}

impl GatewayProvenance {
    pub fn new(requested_route_id: String) -> Self {
        Self {
            gateway_request_id: uuid_simple(),
            requested_route_id,
            effective_route_id: None,
            effective_provider: None,
            effective_model: None,
            upstream_request_id: None,
            fallback_chain: Vec::new(),
            transport_fallbacks: Vec::new(),
            retry_count: 0,
            fallback_policy_id: None,
            wire_api: None,
            stream_termination_status: None,
            call_kind: None,
            endpoint: None,
            input_digest: None,
            output_digest: None,
            response_id: None,
            observed_server_model: None,
            http_status: None,
            retry_after_ms: None,
            current_attempt_retry_after_ms: None,
            attempts: Vec::new(),
        }
    }

    /// Upsert the current top-level attempt into the durable attempt journal.
    /// Repeated seed/terminal writes update one entry instead of duplicating it.
    pub fn synchronize_attempt_history(&mut self) {
        let existing_index = self
            .attempts
            .iter()
            .position(|attempt| attempt.gateway_request_id == self.gateway_request_id);
        let attempt_index = existing_index
            .map(|index| self.attempts[index].attempt_index)
            .unwrap_or_else(|| u32::try_from(self.attempts.len()).unwrap_or(u32::MAX));
        let snapshot = GatewayAttemptProvenance {
            attempt_index,
            gateway_request_id: self.gateway_request_id.clone(),
            endpoint: self.endpoint.clone(),
            wire_api: self.wire_api.clone(),
            input_digest: self.input_digest.clone(),
            output_digest: self.output_digest.clone(),
            http_status: self.http_status,
            retry_after_ms: self.current_attempt_retry_after_ms,
            stream_termination_status: self.stream_termination_status.clone(),
            upstream_request_id: self.upstream_request_id.clone(),
            response_id: self.response_id.clone(),
            observed_server_model: self.observed_server_model.clone(),
        };
        if let Some(index) = existing_index {
            self.attempts[index] = snapshot;
        } else {
            self.attempts.push(snapshot);
        }
    }

    pub fn record_retry(&mut self) {
        self.retry_count += 1;
    }

    pub fn record_transport_fallback(&mut self, transition: &str) {
        if !self
            .transport_fallbacks
            .iter()
            .any(|item| item == transition)
        {
            self.transport_fallbacks.push(transition.to_string());
        }
    }

    pub fn record_effective(&mut self, route_id: String, provider: String, model: String) {
        if self.effective_route_id.is_none() {
            self.effective_route_id = Some(route_id);
            self.effective_provider = Some(provider);
            self.effective_model = Some(model);
        } else {
            self.fallback_chain.push(format!("{provider}:{model}"));
        }
    }

    /// Record observed gateway metadata without inferring downstream provider
    /// identity from the requested route string. Missing fields remain
    /// `None`/unknown until 9router returns real metadata.
    pub fn record_effective_metadata(
        &mut self,
        route_id: Option<String>,
        provider: Option<String>,
        model: Option<String>,
    ) {
        if self.effective_route_id.is_none() {
            self.effective_route_id = route_id;
            self.effective_provider = provider;
            self.effective_model = model;
        } else if let (Some(provider), Some(model)) = (provider, model) {
            self.fallback_chain.push(format!("{provider}:{model}"));
        }
    }

    /// Record a model label observed on the wire without treating it as router
    /// fallback metadata. Responses-style streams can repeat model events; the
    /// first one is the only factual value available unless 9router later
    /// supplies an explicit route/provider transition.
    pub fn record_observed_model(&mut self, model: String) {
        if self.observed_server_model.is_none() {
            self.observed_server_model = Some(model);
        }
    }

    pub fn record_fallback_policy(&mut self, policy: crate::FallbackPolicyId) {
        self.fallback_policy_id = Some(policy);
    }

    pub fn record_wire_api(&mut self, wire_api: &str) {
        self.wire_api = Some(wire_api.to_string());
    }

    pub fn record_stream_termination(&mut self, status: &str) {
        self.stream_termination_status = Some(status.to_string());
    }

    pub fn record_call_site(&mut self, call_kind: &str, endpoint: &str) {
        self.call_kind = Some(call_kind.to_string());
        self.endpoint = Some(endpoint.to_string());
    }

    pub fn record_input_digest(&mut self, digest: String) {
        self.input_digest = Some(digest);
    }

    pub fn record_output_digest(&mut self, digest: String) {
        self.output_digest = Some(digest);
    }

    pub fn record_response_id(&mut self, response_id: String) {
        self.response_id = Some(response_id);
    }

    pub fn record_http_status(&mut self, status: u16) {
        self.http_status = Some(status);
    }

    pub fn record_retry_after_ms(&mut self, retry_after_ms: u64) {
        self.retry_after_ms = Some(retry_after_ms);
        self.current_attempt_retry_after_ms = Some(retry_after_ms);
    }
}

fn uuid_simple() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("gw-{}-{}", d.as_secs(), d.subsec_nanos()))
        .unwrap_or_else(|_| "gw-unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_records_requested_effective_retry_and_fallback_chain() {
        let mut provenance = GatewayProvenance::new("oc/deepseek-v4-flash-free".to_string());
        provenance.record_retry();
        provenance.record_effective(
            "opencode/deepseek-v4-flash-free".to_string(),
            "opencode".to_string(),
            "deepseek-v4-flash-free".to_string(),
        );
        provenance.record_effective(
            "fallback/other".to_string(),
            "fallback-provider".to_string(),
            "other-model".to_string(),
        );

        assert!(provenance.gateway_request_id.starts_with("gw-"));
        assert_eq!(provenance.requested_route_id, "oc/deepseek-v4-flash-free");
        assert_eq!(
            provenance.effective_route_id.as_deref(),
            Some("opencode/deepseek-v4-flash-free")
        );
        assert_eq!(provenance.retry_count, 1);
        assert_eq!(
            provenance.fallback_chain,
            vec!["fallback-provider:other-model"]
        );
    }

    #[test]
    fn provenance_records_fallback_policy_wire_api_and_stream_termination() {
        let mut provenance = GatewayProvenance::new("oc/deepseek-v4-flash-free".to_string());
        provenance.record_fallback_policy(crate::FallbackPolicyId::Strict);
        provenance.record_wire_api("chat_completions");
        provenance.record_transport_fallback("responses_websocket->responses_http");
        provenance.record_stream_termination("completed");

        assert_eq!(
            provenance.fallback_policy_id,
            Some(crate::FallbackPolicyId::Strict)
        );
        assert_eq!(provenance.wire_api.as_deref(), Some("chat_completions"));
        assert_eq!(
            provenance.stream_termination_status.as_deref(),
            Some("completed")
        );
        assert_eq!(
            provenance.transport_fallbacks,
            vec!["responses_websocket->responses_http"]
        );
    }

    #[test]
    fn provenance_does_not_infer_effective_metadata_when_router_does_not_report_it() {
        let mut provenance = GatewayProvenance::new("oc/deepseek-v4-flash-free".to_string());

        provenance.record_effective_metadata(None, None, None);

        assert_eq!(provenance.effective_route_id, None);
        assert_eq!(provenance.effective_provider, None);
        assert_eq!(provenance.effective_model, None);
        assert!(provenance.fallback_chain.is_empty());
    }

    #[test]
    fn observed_stream_models_preserve_the_first_value_without_inventing_fallback() {
        let mut provenance = GatewayProvenance::new("9router/requested".to_string());
        provenance.record_observed_model("first-observed".to_string());
        provenance.record_observed_model("later-observed".to_string());

        assert_eq!(
            provenance.observed_server_model.as_deref(),
            Some("first-observed")
        );
        assert!(provenance.fallback_chain.is_empty());
        assert!(provenance.effective_route_id.is_none());
        assert!(provenance.effective_provider.is_none());
        assert!(provenance.effective_model.is_none());
    }

    #[test]
    fn attempt_history_upserts_one_snapshot_per_gateway_request() {
        let mut provenance = GatewayProvenance::new("route/requested".to_string());
        provenance.record_call_site("stream", "/v1/chat/completions");
        provenance.record_input_digest("sha256:first".to_string());
        provenance.synchronize_attempt_history();
        provenance.record_stream_termination("error");
        provenance.synchronize_attempt_history();

        assert_eq!(provenance.attempts.len(), 1);
        assert_eq!(provenance.attempts[0].attempt_index, 0);
        assert_eq!(
            provenance.attempts[0].stream_termination_status.as_deref(),
            Some("error")
        );

        let prior_attempts = provenance.attempts.clone();
        let mut retry = GatewayProvenance::new("route/requested".to_string());
        retry.attempts = prior_attempts;
        retry.record_call_site("stream", "/v1/chat/completions");
        retry.synchronize_attempt_history();

        assert_eq!(retry.attempts.len(), 2);
        assert_eq!(retry.attempts[1].attempt_index, 1);
        assert_ne!(
            retry.attempts[0].gateway_request_id,
            retry.attempts[1].gateway_request_id
        );
    }

    #[test]
    fn provenance_stream_termination_distinguishes_completed_from_truncated() {
        let completed = {
            let mut p = GatewayProvenance::new("route-1".to_string());
            p.record_stream_termination("completed");
            p
        };
        let truncated = {
            let mut p = GatewayProvenance::new("route-2".to_string());
            p.record_stream_termination("truncated");
            p
        };

        assert_eq!(
            completed.stream_termination_status.as_deref(),
            Some("completed")
        );
        assert_eq!(
            truncated.stream_termination_status.as_deref(),
            Some("truncated")
        );
    }

    #[test]
    fn provenance_records_factual_call_site_and_digests() {
        let mut provenance = GatewayProvenance::new("route-1".to_string());

        provenance.record_call_site("stream", "/v1/chat/completions");
        provenance.record_input_digest("sha256:input".to_string());
        provenance.record_output_digest("sha256:output".to_string());
        provenance.record_response_id("resp-1".to_string());
        provenance.record_observed_model("server-model".to_string());
        provenance.record_http_status(429);
        provenance.record_retry_after_ms(23_000);

        assert_eq!(provenance.call_kind.as_deref(), Some("stream"));
        assert_eq!(provenance.endpoint.as_deref(), Some("/v1/chat/completions"));
        assert_eq!(provenance.input_digest.as_deref(), Some("sha256:input"));
        assert_eq!(provenance.output_digest.as_deref(), Some("sha256:output"));
        assert_eq!(provenance.response_id.as_deref(), Some("resp-1"));
        assert_eq!(
            provenance.observed_server_model.as_deref(),
            Some("server-model")
        );
        assert_eq!(provenance.http_status, Some(429));
        assert_eq!(provenance.retry_after_ms, Some(23_000));
    }
}
