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

    pub fn end_call(
        &self,
        provenance: &mut Option<GatewayProvenance>,
        effective_route_id: Option<&str>,
        effective_provider: Option<&str>,
        effective_model: Option<&str>,
    ) {
        if let Some(prov) = provenance.as_mut() {
            if let (Some(route), Some(provider), Some(model)) =
                (effective_route_id, effective_provider, effective_model)
            {
                prov.record_effective(route.to_string(), provider.to_string(), model.to_string());
            }
        }
    }

    pub fn resolve_route(&self, provider_info: &ModelProviderInfo, model_slug: &str) -> String {
        if self.enabled && provider_info.is_9router() {
            format!("9router/{}", model_slug)
        } else {
            model_slug.to_string()
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
    fn enabled_gateway_records_effective_9router_route() {
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
    fn route_resolution_prefixes_only_canonical_9router_provider() {
        let bridge = ProviderGatewayBridge::new(true);
        let router = ModelProviderInfo::create_9router_provider();
        let openai = ModelProviderInfo::create_openai_provider(None);

        assert_eq!(
            bridge.resolve_route(&router, "oc/deepseek-v4-flash-free"),
            "9router/oc/deepseek-v4-flash-free"
        );
        assert_eq!(bridge.resolve_route(&openai, "gpt-5.6-sol"), "gpt-5.6-sol");
    }
}
