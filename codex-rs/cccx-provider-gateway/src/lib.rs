//! cccx 9router provider gateway.
//!
//! This crate owns cccx model-route resolution and legacy 9router request provenance.
//! Route IDs are opaque: cccx never parses provider identity out of model IDs.
//!
//! Important runtime boundary:
//! - The active cccx runtime uses the codex-core chat-completions client path for
//!   9router/OpenAI-compatible traffic.
//! - The HTTP prompt helpers in this crate remain for route-resolution tests and
//!   older experiments, but they are not the canonical live inference boundary.

use reqwest::Client;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::env;
use std::time::Duration;
use thiserror::Error;
use url::Url;

pub const DEFAULT_9ROUTER_BASE_URL: &str = "https://9router.ai/api";
pub const DEFAULT_9ROUTER_API_KEY_ENV: &str = "CCCX_API_KEY";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackPolicyId {
    Strict,
    Compatible,
    RouterDefault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRouteSelectionSource {
    UserOverride,
    RolePreset,
    AgentSuggested,
    InheritMainRequested,
    InheritMainEffective,
    GlobalDefault,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRoute {
    pub route_id: String,
    pub model_id: String,
    pub model_profile_id: String,
    pub capability_profile_id: String,
    pub fallback_policy_id: FallbackPolicyId,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelRouteCatalog {
    routes: BTreeMap<String, ModelRoute>,
    global_default_route_id: Option<String>,
}

impl ModelRouteCatalog {
    pub fn new(routes: impl IntoIterator<Item = ModelRoute>) -> Self {
        Self {
            routes: routes
                .into_iter()
                .map(|route| (route.route_id.clone(), route))
                .collect(),
            global_default_route_id: None,
        }
    }

    pub fn with_global_default_route_id(mut self, route_id: impl Into<String>) -> Self {
        self.global_default_route_id = Some(route_id.into());
        self
    }

    pub fn get(&self, route_id: &str) -> Option<&ModelRoute> {
        self.routes.get(route_id)
    }

    pub fn global_default(&self) -> Option<&ModelRoute> {
        self.global_default_route_id
            .as_deref()
            .and_then(|route_id| self.get(route_id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRouteProvenance {
    pub selection_source: ModelRouteSelectionSource,
    pub requested_route_id: String,
    pub effective_route_id: Option<String>,
    pub model_profile_id: String,
    pub capability_profile_id: String,
    pub fallback_policy_id: FallbackPolicyId,
    pub parent_requested_route_id: Option<String>,
    pub parent_effective_route_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModelRoute {
    pub route: ModelRoute,
    pub provenance: ModelRouteProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentModelRouteRequest {
    RouteId(String),
    InheritMainRequested,
    InheritMainEffective,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelRouteResolveRequest {
    pub user_override_route_id: Option<String>,
    pub role_preset_route_id: Option<String>,
    pub agent_suggested_route_id: Option<String>,
    pub session_default_subagent_model: Option<SubagentModelRouteRequest>,
    pub main_requested_route_id: Option<String>,
    pub main_effective_route_id: Option<String>,
}

pub struct ModelRouteResolver<'a> {
    catalog: &'a ModelRouteCatalog,
}

impl<'a> ModelRouteResolver<'a> {
    pub fn new(catalog: &'a ModelRouteCatalog) -> Self {
        Self { catalog }
    }

    pub fn resolve_subagent_route(
        &self,
        request: ModelRouteResolveRequest,
    ) -> Result<ResolvedModelRoute, ProviderGatewayError> {
        let (source, requested_route_id) =
            if let Some(route_id) = request.user_override_route_id.clone() {
                (ModelRouteSelectionSource::UserOverride, route_id)
            } else if let Some(route_id) = request.role_preset_route_id.clone() {
                (ModelRouteSelectionSource::RolePreset, route_id)
            } else if let Some(route_id) = request.agent_suggested_route_id.clone() {
                (ModelRouteSelectionSource::AgentSuggested, route_id)
            } else if let Some(default) = request.session_default_subagent_model.clone() {
                match default {
                    SubagentModelRouteRequest::RouteId(route_id) => {
                        (ModelRouteSelectionSource::GlobalDefault, route_id)
                    }
                    SubagentModelRouteRequest::InheritMainRequested => (
                        ModelRouteSelectionSource::InheritMainRequested,
                        request.main_requested_route_id.clone().ok_or(
                            ProviderGatewayError::MissingInheritedRoute {
                                selection_source: ModelRouteSelectionSource::InheritMainRequested,
                            },
                        )?,
                    ),
                    SubagentModelRouteRequest::InheritMainEffective => (
                        ModelRouteSelectionSource::InheritMainEffective,
                        request.main_effective_route_id.clone().ok_or(
                            ProviderGatewayError::MissingInheritedRoute {
                                selection_source: ModelRouteSelectionSource::InheritMainEffective,
                            },
                        )?,
                    ),
                }
            } else if let Some(route_id) = request.main_requested_route_id.clone() {
                (ModelRouteSelectionSource::InheritMainRequested, route_id)
            } else {
                let route = self
                    .catalog
                    .global_default()
                    .ok_or(ProviderGatewayError::MissingGlobalDefaultRoute)?;
                (
                    ModelRouteSelectionSource::GlobalDefault,
                    route.route_id.clone(),
                )
            };

        let route = self.catalog.get(&requested_route_id).cloned().ok_or(
            ProviderGatewayError::UnknownRouteId {
                route_id: requested_route_id.clone(),
            },
        )?;

        Ok(ResolvedModelRoute {
            provenance: ModelRouteProvenance {
                selection_source: source,
                requested_route_id,
                effective_route_id: None,
                model_profile_id: route.model_profile_id.clone(),
                capability_profile_id: route.capability_profile_id.clone(),
                fallback_policy_id: route.fallback_policy_id,
                parent_requested_route_id: request.main_requested_route_id,
                parent_effective_route_id: request.main_effective_route_id,
            },
            route,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ProviderGatewayConfig {
    pub base_url: Url,
    pub api_key_env: String,
    pub request_timeout: Duration,
}

impl ProviderGatewayConfig {
    pub fn new(base_url: Url) -> Self {
        Self {
            base_url,
            api_key_env: DEFAULT_9ROUTER_API_KEY_ENV.to_string(),
            request_timeout: Duration::from_secs(120),
        }
    }

    #[deprecated(
        note = "cccx runtime should use codex-core's chat-completions boundary for 9router; this helper is legacy-only"
    )]
    pub fn default_9router() -> Result<Self, ProviderGatewayError> {
        Ok(Self::new(Url::parse(DEFAULT_9ROUTER_BASE_URL)?))
    }
}

pub struct ProviderGateway {
    config: ProviderGatewayConfig,
    client: Client,
}

impl ProviderGateway {
    #[deprecated(
        note = "cccx runtime should use codex-core's chat-completions boundary for 9router; this gateway is legacy-only"
    )]
    pub fn new(config: ProviderGatewayConfig) -> Result<Self, ProviderGatewayError> {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(ProviderGatewayError::HttpClient)?;
        Ok(Self { config, client })
    }

    #[deprecated(
        note = "cccx runtime should use codex-core's chat-completions boundary for 9router; this helper is legacy-only"
    )]
    pub async fn send_prompt(
        &self,
        resolved_route: &ResolvedModelRoute,
        prompt: &str,
    ) -> Result<GatewayResponse, ProviderGatewayError> {
        let api_key = env::var(&self.config.api_key_env).map_err(|_| {
            ProviderGatewayError::MissingApiKeyEnv {
                env_var: self.config.api_key_env.clone(),
            }
        })?;
        #[allow(deprecated)]
        {
            self.send_prompt_with_api_key(resolved_route, prompt, &api_key)
                .await
        }
    }

    #[deprecated(
        note = "cccx runtime should use codex-core's chat-completions boundary for 9router; this helper is legacy-only"
    )]
    pub async fn send_prompt_with_api_key(
        &self,
        resolved_route: &ResolvedModelRoute,
        prompt: &str,
        api_key: &str,
    ) -> Result<GatewayResponse, ProviderGatewayError> {
        let url = self.config.base_url.join("v1/responses")?;
        let request = GatewayRequest {
            model: &resolved_route.route.model_id,
            input: prompt,
        };
        let response = self
            .client
            .post(url)
            .bearer_auth(api_key)
            .json(&request)
            .send()
            .await
            .map_err(ProviderGatewayError::HttpRequest)?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderGatewayError::HttpStatus {
                status: status.as_u16(),
                body,
            });
        }

        let body: GatewayResponseBody = response
            .json()
            .await
            .map_err(ProviderGatewayError::HttpRequest)?;
        let mut provenance = resolved_route.provenance.clone();
        provenance.effective_route_id = Some(resolved_route.route.route_id.clone());
        Ok(GatewayResponse {
            response_id: body.id,
            output_text: body.output_text.unwrap_or_default(),
            provenance,
        })
    }
}

#[derive(Debug, Serialize)]
struct GatewayRequest<'a> {
    model: &'a str,
    input: &'a str,
}

#[derive(Debug, Deserialize)]
struct GatewayResponseBody {
    id: String,
    #[serde(default)]
    output_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayResponse {
    pub response_id: String,
    pub output_text: String,
    pub provenance: ModelRouteProvenance,
}

#[derive(Debug, Error)]
pub enum ProviderGatewayError {
    #[error("missing global default model route")]
    MissingGlobalDefaultRoute,
    #[error("missing inherited model route for {selection_source:?}")]
    MissingInheritedRoute {
        selection_source: ModelRouteSelectionSource,
    },
    #[error("unknown model route id `{route_id}`")]
    UnknownRouteId { route_id: String },
    #[error("missing 9router API key environment variable `{env_var}`")]
    MissingApiKeyEnv { env_var: String },
    #[error("invalid 9router base URL: {0}")]
    InvalidBaseUrl(#[from] url::ParseError),
    #[error("failed to create HTTP client: {0}")]
    HttpClient(reqwest::Error),
    #[error("9router request failed: {0}")]
    HttpRequest(reqwest::Error),
    #[error("9router returned HTTP {status}: {body}")]
    HttpStatus { status: u16, body: String },
}

pub mod provenance;

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_json;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    fn route(route_id: &str, model_id: &str) -> ModelRoute {
        ModelRoute {
            route_id: route_id.to_string(),
            model_id: model_id.to_string(),
            model_profile_id: "default-profile".to_string(),
            capability_profile_id: "tools-streaming".to_string(),
            fallback_policy_id: FallbackPolicyId::Compatible,
        }
    }

    fn catalog() -> ModelRouteCatalog {
        ModelRouteCatalog::new([
            route("main", "provider-a/model"),
            route("preset", "provider-b/model"),
            route("explicit", "provider-c/model"),
        ])
        .with_global_default_route_id("main")
    }

    #[test]
    fn explicit_route_wins() {
        let catalog = catalog();
        let resolved = ModelRouteResolver::new(&catalog)
            .resolve_subagent_route(ModelRouteResolveRequest {
                user_override_route_id: Some("explicit".to_string()),
                role_preset_route_id: Some("preset".to_string()),
                main_requested_route_id: Some("main".to_string()),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(resolved.route.route_id, "explicit");
        assert_eq!(
            resolved.provenance.selection_source,
            ModelRouteSelectionSource::UserOverride
        );
    }

    #[test]
    fn role_preset_wins_when_no_user_override() {
        let catalog = catalog();
        let resolved = ModelRouteResolver::new(&catalog)
            .resolve_subagent_route(ModelRouteResolveRequest {
                role_preset_route_id: Some("preset".to_string()),
                main_requested_route_id: Some("main".to_string()),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(resolved.route.route_id, "preset");
        assert_eq!(
            resolved.provenance.selection_source,
            ModelRouteSelectionSource::RolePreset
        );
    }

    #[test]
    fn default_inherits_main_requested_route() {
        let catalog = catalog();
        let resolved = ModelRouteResolver::new(&catalog)
            .resolve_subagent_route(ModelRouteResolveRequest {
                main_requested_route_id: Some("main".to_string()),
                main_effective_route_id: Some("preset".to_string()),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(resolved.route.route_id, "main");
        assert_eq!(
            resolved.provenance.selection_source,
            ModelRouteSelectionSource::InheritMainRequested
        );
    }

    #[test]
    fn invalid_route_fails_closed() {
        let catalog = catalog();
        let err = ModelRouteResolver::new(&catalog)
            .resolve_subagent_route(ModelRouteResolveRequest {
                user_override_route_id: Some("missing".to_string()),
                main_requested_route_id: Some("main".to_string()),
                ..Default::default()
            })
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderGatewayError::UnknownRouteId { route_id } if route_id == "missing"
        ));
    }

    #[test]
    fn session_default_can_inherit_main_effective_route() {
        let catalog = catalog();
        let resolved = ModelRouteResolver::new(&catalog)
            .resolve_subagent_route(ModelRouteResolveRequest {
                session_default_subagent_model: Some(
                    SubagentModelRouteRequest::InheritMainEffective,
                ),
                main_requested_route_id: Some("main".to_string()),
                main_effective_route_id: Some("preset".to_string()),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(resolved.route.route_id, "preset");
        assert_eq!(
            resolved.provenance.selection_source,
            ModelRouteSelectionSource::InheritMainEffective
        );
        assert_eq!(
            resolved.provenance.parent_requested_route_id,
            Some("main".to_string())
        );
        assert_eq!(
            resolved.provenance.parent_effective_route_id,
            Some("preset".to_string())
        );
    }

    #[test]
    fn agent_suggested_route_has_lower_priority_than_role_preset() {
        let catalog = catalog();
        let resolved = ModelRouteResolver::new(&catalog)
            .resolve_subagent_route(ModelRouteResolveRequest {
                role_preset_route_id: Some("preset".to_string()),
                agent_suggested_route_id: Some("explicit".to_string()),
                main_requested_route_id: Some("main".to_string()),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(resolved.route.route_id, "preset");
        assert_eq!(
            resolved.provenance.selection_source,
            ModelRouteSelectionSource::RolePreset
        );
    }

    #[allow(deprecated)]
    #[tokio::test]
    async fn mock_9router_records_effective_route_provenance() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(header("authorization", "Bearer test-key"))
            .and(body_json(serde_json::json!({
                "model": "provider-a/model",
                "input": "hello"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "resp_1",
                "output_text": "world"
            })))
            .mount(&server)
            .await;

        let config = ProviderGatewayConfig::new(Url::parse(&server.uri()).unwrap());
        let gateway = ProviderGateway::new(config).unwrap();
        let catalog = catalog();
        let resolved = ModelRouteResolver::new(&catalog)
            .resolve_subagent_route(ModelRouteResolveRequest {
                main_requested_route_id: Some("main".to_string()),
                ..Default::default()
            })
            .unwrap();

        let response = gateway
            .send_prompt_with_api_key(&resolved, "hello", "test-key")
            .await
            .unwrap();

        assert_eq!(response.response_id, "resp_1");
        assert_eq!(response.output_text, "world");
        assert_eq!(
            response.provenance.effective_route_id,
            Some("main".to_string())
        );
        assert_eq!(response.provenance.requested_route_id, "main".to_string());
    }
}
