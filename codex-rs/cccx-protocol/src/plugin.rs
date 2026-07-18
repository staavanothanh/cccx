use crate::WorkRole;
use serde::{Deserialize, Serialize};

/// cccx extended plugin manifest.
/// Architecture.md §15 — separate from Codex native plugin.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CccxPluginManifest {
    /// Must be "1" for this schema version.
    #[serde(rename = "cccxSchemaVersion")]
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

    /// MCP tools this plugin provides, with stable identifiers and
    /// schema digests for WorkSpec gating.
    #[serde(default, rename = "mcpTools")]
    pub mcp_tools: Vec<McpToolEntry>,
}

/// MCP tool entry with stable id and schema digest.
/// architecture.md §15: WorkSpec grants MCP access via stable id/schema digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolEntry {
    /// Stable tool identifier used in WorkSpec allowed_mcp_tools.
    #[serde(rename = "stableId")]
    pub stable_id: String,
    /// Schema digest for integrity verification at plugin load time.
    #[serde(rename = "schemaDigest")]
    pub schema_digest: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    #[serde(rename = "role")]
    pub role: WorkRole,
    #[serde(rename = "allowedTools")]
    pub allowed_tools: Vec<String>,
    #[serde(default, rename = "model")]
    pub model: Option<String>,
    #[serde(default, rename = "allowedModelRoutes")]
    pub allowed_model_routes: Vec<String>,
    #[serde(default, rename = "fallbackPolicyId")]
    pub fallback_policy_id: Option<String>,
    #[serde(default, rename = "constraints")]
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
        for mcp_tool in &self.mcp_tools {
            if mcp_tool.stable_id.is_empty() {
                return Err("mcp_tools[].stableId must not be empty".to_string());
            }
            if mcp_tool.schema_digest.is_empty() {
                return Err("mcp_tools[].schemaDigest must not be empty".to_string());
            }
        }
        Ok(())
    }

    /// Returns `true` when this plugin applies to the Main CC harness.
    pub fn is_compatible_with_main_cc(&self) -> bool {
        matches!(self.harness, PluginHarness::Cc | PluginHarness::Both)
    }

    /// Returns `true` when this plugin applies to CX subagent harness.
    /// CX plugins require explicit MCP stable ids for WorkSpec gating.
    pub fn is_compatible_with_cx_subagent(&self) -> bool {
        matches!(self.harness, PluginHarness::Cx | PluginHarness::Both)
    }

    /// Returns the stable MCP tool ids that this plugin exposes.
    /// Used in WorkSpec projection to decide which MCP tools a
    /// subagent can access.
    pub fn mcp_stable_ids(&self) -> Vec<&str> {
        self.mcp_tools
            .iter()
            .map(|entry| entry.stable_id.as_str())
            .collect()
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
            mcp_tools: Vec::new(),
        };

        assert!(
            manifest
                .validate()
                .unwrap_err()
                .contains("unsupported cccx schema version")
        );
    }

    #[test]
    fn cx_plugged_plugin_is_not_compatible_with_main_cc() {
        let manifest = CccxPluginManifest {
            cccx_schema_version: "1".to_string(),
            harness: PluginHarness::Cx,
            rules: vec!["cx-audit-rules.md".to_string()],
            agents: vec![AgentPreset {
                role: WorkRole::Auditor,
                allowed_tools: vec!["apply_patch".to_string()],
                ..default_agent_preset()
            }],
            mcp_tools: Vec::new(),
        };
        manifest.validate().expect("valid cx manifest");

        assert!(!manifest.is_compatible_with_main_cc());
        assert!(manifest.is_compatible_with_cx_subagent());
    }

    #[test]
    fn cc_plugged_plugin_is_not_compatible_with_cx_subagent() {
        let manifest = CccxPluginManifest {
            cccx_schema_version: "1".to_string(),
            harness: PluginHarness::Cc,
            rules: Vec::new(),
            agents: vec![AgentPreset {
                role: WorkRole::Planner,
                allowed_tools: vec!["spawn_agent".to_string()],
                ..default_agent_preset()
            }],
            mcp_tools: Vec::new(),
        };
        manifest.validate().expect("valid cc manifest");

        assert!(manifest.is_compatible_with_main_cc());
        assert!(!manifest.is_compatible_with_cx_subagent());
    }

    #[test]
    fn both_harness_plugin_is_compatible_with_both() {
        let manifest = CccxPluginManifest {
            cccx_schema_version: "1".to_string(),
            harness: PluginHarness::Both,
            rules: Vec::new(),
            agents: Vec::new(),
            mcp_tools: Vec::new(),
        };
        manifest.validate().expect("valid both manifest");

        assert!(manifest.is_compatible_with_main_cc());
        assert!(manifest.is_compatible_with_cx_subagent());
    }

    #[test]
    fn mcp_tool_entries_expose_stable_ids_for_workspec_gating() {
        let manifest = CccxPluginManifest {
            cccx_schema_version: "1".to_string(),
            harness: PluginHarness::Both,
            rules: Vec::new(),
            agents: Vec::new(),
            mcp_tools: vec![
                McpToolEntry {
                    stable_id: "codebase-memory-mcp".to_string(),
                    schema_digest: "sha256:abc123".to_string(),
                },
                McpToolEntry {
                    stable_id: "sequential-thinking".to_string(),
                    schema_digest: "sha256:def456".to_string(),
                },
            ],
        };
        manifest.validate().expect("valid mcp plugin");

        assert_eq!(
            manifest.mcp_stable_ids(),
            vec!["codebase-memory-mcp", "sequential-thinking"]
        );
    }

    #[test]
    fn mcp_tool_validation_rejects_empty_stable_id() {
        let manifest = CccxPluginManifest {
            cccx_schema_version: "1".to_string(),
            harness: PluginHarness::Both,
            rules: Vec::new(),
            agents: Vec::new(),
            mcp_tools: vec![McpToolEntry {
                stable_id: String::new(),
                schema_digest: "sha256:abc".to_string(),
            }],
        };
        assert!(
            manifest
                .validate()
                .unwrap_err()
                .contains("stableId must not be empty")
        );
    }

    fn default_agent_preset() -> AgentPreset {
        AgentPreset {
            role: WorkRole::Implementer,
            allowed_tools: vec!["apply_patch".to_string()],
            model: None,
            allowed_model_routes: Vec::new(),
            fallback_policy_id: None,
            constraints: Vec::new(),
        }
    }
}
