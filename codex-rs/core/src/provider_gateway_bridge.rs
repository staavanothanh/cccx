use cccx_provider_gateway::provenance::GatewayProvenance;
use codex_model_provider_info::ModelProviderInfo;

pub struct ProviderGatewayBridge {
    enabled: bool,
}

impl ProviderGatewayBridge {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub fn begin_call(&self, requested_route_id: &str) -> Option<GatewayProvenance> {
        if !self.enabled {
            return None;
        }
        Some(GatewayProvenance::new(requested_route_id.to_string()))
    }

    pub fn begin_call_with_wire_api(
        &self,
        requested_route_id: &str,
        wire_api: &str,
    ) -> Option<GatewayProvenance> {
        let mut provenance = self.begin_call(requested_route_id)?;
        provenance.record_wire_api(wire_api);
        Some(provenance)
    }

    pub fn record_stream_termination(
        &self,
        provenance: &mut Option<GatewayProvenance>,
        status: &str,
    ) {
        if let Some(prov) = provenance.as_mut() {
            prov.record_stream_termination(status);
        }
    }

    /// Returns `false` when the cccx surface gate is enabled but the resolved
    /// provider is NOT 9router — this is a fail-closed signal that callers must
    /// surface as a hard error.
    pub fn check_fail_closed(&self, provider_info: &ModelProviderInfo) -> bool {
        if self.enabled && !provider_info.is_9router() {
            return false;
        }
        true
    }

    pub fn end_call(
        &self,
        provenance: &mut Option<GatewayProvenance>,
        effective_route_id: Option<&str>,
        effective_provider: Option<&str>,
        effective_model: Option<&str>,
    ) {
        if let Some(prov) = provenance.as_mut() {
            prov.record_effective_metadata(
                effective_route_id.map(str::to_string),
                effective_provider.map(str::to_string),
                effective_model.map(str::to_string),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_gateway_records_no_request_provenance() {
        let bridge = ProviderGatewayBridge::new(false);

        assert!(bridge.begin_call("oc/deepseek-v4-flash-free").is_none());
    }

    #[test]
    fn enabled_gateway_records_effective_metadata_when_supplied() {
        let bridge = ProviderGatewayBridge::new(true);
        let mut provenance = bridge.begin_call("oc/deepseek-v4-flash-free");

        bridge.end_call(
            &mut provenance,
            Some("opencode/deepseek-v4-flash-free"),
            Some("opencode"),
            Some("deepseek-v4-flash-free"),
        );

        let provenance = provenance.expect("enabled bridge creates provenance");
        assert_eq!(provenance.requested_route_id, "oc/deepseek-v4-flash-free");
        assert_eq!(
            provenance.effective_route_id.as_deref(),
            Some("opencode/deepseek-v4-flash-free")
        );
        assert_eq!(provenance.effective_provider.as_deref(), Some("opencode"));
        assert_eq!(
            provenance.effective_model.as_deref(),
            Some("deepseek-v4-flash-free")
        );
    }

    #[test]
    fn enabled_gateway_does_not_infer_downstream_route_without_metadata() {
        let bridge = ProviderGatewayBridge::new(true);
        let mut provenance = bridge.begin_call("oc/deepseek-v4-flash-free");

        bridge.end_call(&mut provenance, None, None, None);

        let provenance = provenance.expect("enabled bridge creates provenance");
        assert_eq!(provenance.requested_route_id, "oc/deepseek-v4-flash-free");
        assert_eq!(provenance.effective_route_id, None);
        assert_eq!(provenance.effective_provider, None);
        assert_eq!(provenance.effective_model, None);
    }

    #[test]
    fn enabled_gateway_records_wire_api_and_stream_termination_in_provenance() {
        let bridge = ProviderGatewayBridge::new(true);
        let mut provenance =
            bridge.begin_call_with_wire_api("oc/deepseek-v4-flash-free", "chat_completions");

        bridge.record_stream_termination(&mut provenance, "completed");

        let provenance = provenance.expect("enabled bridge creates provenance");
        assert_eq!(provenance.wire_api.as_deref(), Some("chat_completions"));
        assert_eq!(
            provenance.stream_termination_status.as_deref(),
            Some("completed")
        );
    }

    #[test]
    fn fail_closed_detects_non_9router_provider_when_cccx_gate_enabled() {
        let bridge = ProviderGatewayBridge::new(true);
        let openai = ModelProviderInfo::create_openai_provider(None);
        let router = ModelProviderInfo::create_9router_provider();
        let mut spoofed_router = ModelProviderInfo::create_9router_provider();
        spoofed_router.base_url = Some("https://attacker.example/v1".to_string());

        // When cccx gate is on, non-9router provider must fail-closed
        assert!(!bridge.check_fail_closed(&openai));
        // A custom provider cannot pass just by naming itself "9router".
        assert!(!bridge.check_fail_closed(&spoofed_router));
        // 9router provider passes
        assert!(bridge.check_fail_closed(&router));
    }

    #[test]
    fn fail_closed_is_noop_when_gate_disabled() {
        let bridge = ProviderGatewayBridge::new(false);
        let openai = ModelProviderInfo::create_openai_provider(None);

        // When gate disabled, check always passes
        assert!(bridge.check_fail_closed(&openai));
    }
}
