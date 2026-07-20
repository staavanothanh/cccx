/// Provenance record for a single model call through 9router.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GatewayProvenance {
    pub gateway_request_id: String,
    pub requested_route_id: String,
    pub effective_route_id: Option<String>,
    pub effective_provider: Option<String>,
    pub effective_model: Option<String>,
    pub upstream_request_id: Option<String>,
    pub fallback_chain: Vec<String>,
    pub retry_count: u32,
    /// Which `FallbackPolicyId` 9router applied for this request.
    pub fallback_policy_id: Option<crate::FallbackPolicyId>,
    /// The wire API used for this call (e.g. `chat_completions`, `responses`).
    pub wire_api: Option<String>,
    /// Outcome of the stream: `completed`, `truncated`, `timeout`, `error`.
    pub stream_termination_status: Option<String>,
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
            retry_count: 0,
            fallback_policy_id: None,
            wire_api: None,
            stream_termination_status: None,
        }
    }

    pub fn record_retry(&mut self) {
        self.retry_count += 1;
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

    pub fn record_fallback_policy(&mut self, policy: crate::FallbackPolicyId) {
        self.fallback_policy_id = Some(policy);
    }

    pub fn record_wire_api(&mut self, wire_api: &str) {
        self.wire_api = Some(wire_api.to_string());
    }

    pub fn record_stream_termination(&mut self, status: &str) {
        self.stream_termination_status = Some(status.to_string());
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
}
