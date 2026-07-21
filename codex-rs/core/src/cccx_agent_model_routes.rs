use cccx_protocol::ModelRouteSelection;
use cccx_protocol::WorkRole;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;

const ROUTES_FILE: &str = "cccx-agent-model-routes.json";

#[derive(Debug, Error)]
pub enum AgentModelRouteStoreError {
    #[error("invalid agent preset `{0}`")]
    InvalidPreset(String),
    #[error("invalid model route `{0}`")]
    InvalidRoute(String),
    #[error("failed to read CCCX agent model routes: {0}")]
    Read(#[from] std::io::Error),
    #[error("failed to parse CCCX agent model routes: {0}")]
    Parse(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct AgentModelRoutes {
    #[serde(default)]
    routes: BTreeMap<String, String>,
}

pub async fn set_agent_model_route(
    codex_home: &Path,
    preset: &str,
    route: &str,
) -> Result<(), AgentModelRouteStoreError> {
    validate_key(preset).map_err(|_| AgentModelRouteStoreError::InvalidPreset(preset.into()))?;
    validate_key(route).map_err(|_| AgentModelRouteStoreError::InvalidRoute(route.into()))?;
    let path = codex_home.join(ROUTES_FILE);
    let mut routes = read_routes_file(&path).await?;
    routes
        .routes
        .insert(normalize_preset(preset), route.to_string());
    tokio::fs::create_dir_all(codex_home).await?;
    let data = serde_json::to_vec_pretty(&routes)?;
    tokio::fs::write(path, data).await?;
    Ok(())
}

pub async fn route_for_work_role(
    codex_home: &Path,
    role: &WorkRole,
) -> Result<Option<ModelRouteSelection>, AgentModelRouteStoreError> {
    let path = codex_home.join(ROUTES_FILE);
    if tokio::fs::metadata(&path).await.is_err() {
        return Ok(None);
    }
    let routes = read_routes_file(&path).await?;
    let role_key = normalize_work_role(role);
    Ok(routes
        .routes
        .get(&role_key)
        .cloned()
        .map(|route_id| ModelRouteSelection::Explicit { route_id }))
}

/// Resolves the role-preset layer without overriding a direct route selected
/// for this spawn. The WorkSpec is the typed spawn request, so an explicit
/// route there is the highest-priority caller input; a role preset only
/// supplies the configured fallback for inherited routes.
pub fn apply_role_preset(
    requested: ModelRouteSelection,
    role_preset: Option<ModelRouteSelection>,
) -> ModelRouteSelection {
    match requested {
        ModelRouteSelection::Explicit { .. } => requested,
        ModelRouteSelection::InheritMainRequested | ModelRouteSelection::InheritMainEffective => {
            role_preset.unwrap_or(requested)
        }
    }
}

fn validate_key(value: &str) -> Result<(), ()> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return Err(());
    }
    Ok(())
}

fn normalize_work_role(role: &WorkRole) -> String {
    format!("{role:?}").to_ascii_lowercase()
}

fn normalize_preset(preset: &str) -> String {
    preset
        .trim()
        .trim_start_matches("cc:")
        .trim_start_matches("cx:")
        .trim_start_matches("cc_")
        .trim_start_matches("cx_")
        .to_ascii_lowercase()
}

async fn read_routes_file(path: &Path) -> Result<AgentModelRoutes, AgentModelRouteStoreError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(AgentModelRoutes::default())
        }
        Err(error) => Err(AgentModelRouteStoreError::Read(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn route_for_work_role_reads_normalized_preset() {
        let home = tempdir().unwrap();
        set_agent_model_route(home.path(), "cx_implementer", "oc/deepseek-v4")
            .await
            .unwrap();

        assert_eq!(
            route_for_work_role(home.path(), &WorkRole::Implementer)
                .await
                .unwrap(),
            Some(ModelRouteSelection::Explicit {
                route_id: "oc/deepseek-v4".to_string()
            })
        );
    }

    #[tokio::test]
    async fn route_store_rejects_empty_or_control_values() {
        let home = tempdir().unwrap();
        assert!(
            set_agent_model_route(home.path(), "", "oc/model")
                .await
                .is_err()
        );
        assert!(
            set_agent_model_route(home.path(), "implementer", "oc/\nmodel")
                .await
                .is_err()
        );
    }

    #[test]
    fn explicit_work_spec_route_wins_over_role_preset() {
        let explicit = ModelRouteSelection::Explicit {
            route_id: "9router/user-override".to_string(),
        };
        let preset = Some(ModelRouteSelection::Explicit {
            route_id: "9router/role-preset".to_string(),
        });

        assert_eq!(apply_role_preset(explicit.clone(), preset), explicit);
    }

    #[test]
    fn role_preset_replaces_inherited_route() {
        let preset = ModelRouteSelection::Explicit {
            route_id: "9router/role-preset".to_string(),
        };

        assert_eq!(
            apply_role_preset(
                ModelRouteSelection::InheritMainRequested,
                Some(preset.clone())
            ),
            preset
        );
    }
}
