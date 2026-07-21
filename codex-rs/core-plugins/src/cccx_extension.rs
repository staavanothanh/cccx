use std::fs;

use cccx_protocol::plugin::CccxPluginManifest;
use codex_utils_absolute_path::AbsolutePathBuf;

use crate::PluginLoadOutcome;

const CCCX_PLUGIN_MANIFEST_FILE: &str = "cccx-plugin.json";

/// CCCX-only metadata attached to a plugin which the native loader has already
/// admitted. This is intentionally a sidecar: it never materializes skills,
/// MCP servers, apps, or hooks outside Codex's native plugin pipeline.
#[derive(Debug, Clone)]
pub struct LoadedCccxExtension {
    pub config_name: String,
    pub display_name: String,
    pub root: AbsolutePathBuf,
    pub manifest: CccxPluginManifest,
}

impl LoadedCccxExtension {
    /// Bounded Main-CC context for the extension. Rules are accepted only when
    /// their canonical path stays inside the installed plugin root.
    pub fn main_cc_context_fragments(&self) -> Vec<String> {
        if !self.manifest.is_compatible_with_main_cc() {
            return Vec::new();
        }

        let canonical_root = match dunce::canonicalize(self.root.as_path()) {
            Ok(root) => root,
            Err(_) => return Vec::new(),
        };
        let mut fragments = Vec::new();
        for rule in &self.manifest.rules {
            let candidate = match dunce::canonicalize(self.root.join(rule)) {
                Ok(path) if path.starts_with(&canonical_root) => path,
                _ => continue,
            };
            let Ok(contents) = fs::read_to_string(&candidate) else {
                continue;
            };
            let contents = contents.chars().take(16_000).collect::<String>();
            if !contents.trim().is_empty() {
                fragments.push(format!(
                    "<cccx_plugin_rule plugin=\"{}\" path=\"{}\">\n{}\n</cccx_plugin_rule>",
                    self.display_name,
                    rule,
                    contents
                ));
            }
        }

        if !self.manifest.agents.is_empty() || !self.manifest.mcp_tools.is_empty() {
            let preset_roles = self
                .manifest
                .agents
                .iter()
                .map(|preset| format!("{:?}", preset.role))
                .collect::<Vec<_>>();
            let stable_ids = self.manifest.mcp_stable_ids();
            fragments.push(format!(
                "<cccx_plugin_capabilities plugin=\"{}\">\nagent_presets: {}\nmcp_stable_ids: {}\nThese are Main CC planning metadata only. Grant child access solely through a WorkSpec.\n</cccx_plugin_capabilities>",
                self.display_name,
                preset_roles.join(", "),
                stable_ids.join(", "),
            ));
        }
        fragments
    }
}

/// Re-read strict CCCX sidecars only for native plugins which are active in
/// this runtime snapshot. A changed or malformed sidecar is ignored here: the
/// native loader will report it as an inactive plugin on the next refresh.
pub fn active_cccx_extensions(loaded_plugins: &PluginLoadOutcome) -> Vec<LoadedCccxExtension> {
    loaded_plugins
        .plugins()
        .iter()
        .filter(|plugin| plugin.is_active())
        .filter_map(|plugin| {
            let path = plugin.root.join(CCCX_PLUGIN_MANIFEST_FILE);
            let raw = fs::read_to_string(path).ok()?;
            let manifest = serde_json::from_str::<CccxPluginManifest>(&raw).ok()?;
            manifest.validate().ok()?;
            Some(LoadedCccxExtension {
                config_name: plugin.config_name.clone(),
                display_name: plugin.display_name().to_string(),
                root: plugin.root.clone(),
                manifest,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cccx_protocol::plugin::PluginHarness;
    use std::collections::HashMap;
    use std::collections::HashSet;

    fn manifest(harness: PluginHarness, rules: Vec<String>) -> CccxPluginManifest {
        CccxPluginManifest {
            cccx_schema_version: "1".to_string(),
            harness,
            rules,
            agents: Vec::new(),
            mcp_tools: Vec::new(),
            skills: Vec::new(),
            mcp_servers: Vec::new(),
            hooks: Vec::new(),
            apps: Vec::new(),
        }
    }

    #[test]
    fn absent_or_invalid_sidecars_do_not_become_active_extensions() {
        // The native loader is responsible for emitting diagnostics. This
        // catalog must remain fail-closed if a sidecar changes after load.
        assert!(active_cccx_extensions(&PluginLoadOutcome::default()).is_empty());
    }

    #[test]
    fn main_cc_context_reads_only_in_root_rules_and_hides_cx_extensions() {
        let root = tempfile::tempdir().expect("temporary plugin root");
        let rule_path = root.path().join("rules.md");
        fs::write(&rule_path, "Use the plugin rule.").expect("write rule");
        let extension = LoadedCccxExtension {
            config_name: "example@test".to_string(),
            display_name: "example".to_string(),
            root: AbsolutePathBuf::try_from(root.path()).expect("absolute root"),
            manifest: manifest(PluginHarness::Cc, vec!["rules.md".to_string()]),
        };

        let fragments = extension.main_cc_context_fragments();
        assert_eq!(fragments.len(), 1);
        assert!(fragments[0].contains("Use the plugin rule."));

        let cx_extension = LoadedCccxExtension {
            manifest: manifest(PluginHarness::Cx, vec!["rules.md".to_string()]),
            ..extension
        };
        assert!(cx_extension.main_cc_context_fragments().is_empty());
    }

    #[test]
    fn catalog_admits_only_native_active_plugins() {
        let active_root = tempfile::tempdir().expect("active plugin root");
        let disabled_root = tempfile::tempdir().expect("disabled plugin root");
        let manifest = r#"{"cccxSchemaVersion":"1","harness":"cc"}"#;
        fs::write(active_root.path().join(CCCX_PLUGIN_MANIFEST_FILE), manifest)
            .expect("write active sidecar");
        fs::write(disabled_root.path().join(CCCX_PLUGIN_MANIFEST_FILE), manifest)
            .expect("write disabled sidecar");
        let plugin = |name: &str, root: &std::path::Path, enabled: bool| crate::LoadedPlugin {
            config_name: name.to_string(),
            manifest_name: Some(name.to_string()),
            plugin_namespace: Some(name.to_string()),
            manifest_description: None,
            root: AbsolutePathBuf::try_from(root).expect("absolute plugin root"),
            enabled,
            skill_roots: Vec::new(),
            disabled_skill_paths: HashSet::new(),
            has_enabled_skills: false,
            mcp_servers: HashMap::new(),
            apps: Vec::new(),
            hook_sources: Vec::new(),
            hook_load_warnings: Vec::new(),
            error: None,
        };
        let outcome = PluginLoadOutcome::from_plugins(vec![
            plugin("active@test", active_root.path(), true),
            plugin("disabled@test", disabled_root.path(), false),
        ]);

        let extensions = active_cccx_extensions(&outcome);
        assert_eq!(extensions.len(), 1);
        assert_eq!(extensions[0].config_name, "active@test");
    }
}
