/// Provenance record for a single model call through 9router.
#[derive(Debug, Clone)]
pub struct GatewayProvenance {
    pub gateway_request_id: String,
    pub requested_route_id: String,
    pub effective_route_id: Option<String>,
    pub effective_provider: Option<String>,
    pub effective_model: Option<String>,
    pub upstream_request_id: Option<String>,
    pub fallback_chain: Vec<String>,
    pub retry_count: u32,
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
}
