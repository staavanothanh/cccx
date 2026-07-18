use crate::WorkRole;
use serde::{Deserialize, Serialize};

/// cccx extended plugin manifest.
/// Architecture.md §15 — separate from Codex native plugin.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CccxPluginManifest {
    /// Must be "1" for this schema version.
    pub cccx_schema_version: String,

    /// Which harness(es) this plugin applies to.
    #[serde(default)]
    pub harness: PluginHarness,

    /// Paths to rule files bundled with the plugin.
    #[serde(default)]
    pub rules: Vec<String>,

    /// Agent presets (WorkSpec templates).
    #[serde(default)]
    pub agents: Vec<AgentPreset>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginHarness {
    Cc,
    Cx,
    Both,
}

impl Default for PluginHarness {
    fn default() -> Self {
        PluginHarness::Cc
    }
}

/// Agent preset — a WorkSpec template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPreset {
    pub role: WorkRole,
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub allowed_model_routes: Vec<String>,
    #[serde(default)]
    pub fallback_policy_id: Option<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
}

impl CccxPluginManifest {
    /// Validate the manifest. Returns Ok(()) or an error message.
    pub fn validate(&self) -> Result<(), String> {
        if self.cccx_schema_version != "1" {
            return Err(format!(
                "unsupported cccx schema version '{}'; expected '1'",
                self.cccx_schema_version
            ));
        }
        for agent in &self.agents {
            if agent.allowed_tools.is_empty() {
                return Err("agent preset must have at least one allowed_tool".to_string());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cccx_plugin_manifest_uses_design_schema_field_names() {
        let manifest: CccxPluginManifest = serde_json::from_value(serde_json::json!({
            "cccxSchemaVersion": "1",
            "agents": [{
                "role": "implementer",
                "allowedTools": ["apply_patch"],
                "model": "inherit-main-requested",
                "allowedModelRoutes": ["oc/deepseek-v4-flash-free"],
                "fallbackPolicyId": "compatible",
                "constraints": ["no-out-of-scope-edits"]
            }]
        }))
        .expect("design manifest should deserialize");

        assert!(matches!(manifest.harness, PluginHarness::Cc));
        manifest
            .validate()
            .expect("design manifest should validate");
    }

    #[test]
    fn cccx_plugin_manifest_rejects_unknown_fields() {
        let result = serde_json::from_value::<CccxPluginManifest>(serde_json::json!({
            "cccxSchemaVersion": "1",
            "harness": "cc",
            "unexpectedCodexField": true
        }));

        assert!(
            result.is_err(),
            "cccx extended manifest must fail closed on unknown fields"
        );
    }

    #[test]
    fn cccx_plugin_manifest_rejects_unsupported_schema_version() {
        let manifest = CccxPluginManifest {
            cccx_schema_version: "2".to_string(),
            harness: PluginHarness::Cc,
            rules: Vec::new(),
            agents: Vec::new(),
        };

        assert!(
            manifest
                .validate()
                .unwrap_err()
                .contains("unsupported cccx schema version")
        );
    }
}
