use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use cccx_protocol::plugin::CccxPluginManifest;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::collections::BTreeSet;

use crate::PluginLoadOutcome;

static VALIDATED_EXTENSION_SNAPSHOTS: OnceLock<
    RwLock<std::collections::HashMap<PathBuf, CccxPluginManifest>>,
> = OnceLock::new();

fn extension_snapshots() -> &'static RwLock<std::collections::HashMap<PathBuf, CccxPluginManifest>>
{
    VALIDATED_EXTENSION_SNAPSHOTS.get_or_init(|| RwLock::new(std::collections::HashMap::new()))
}

pub(crate) fn replace_validated_extension_snapshot(
    root: &Path,
    manifest: Option<CccxPluginManifest>,
) {
    let key = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut snapshots = extension_snapshots()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    snapshots.remove(&key);
    if let Some(manifest) = manifest {
        snapshots.insert(key, manifest);
    }
}

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
                    self.display_name, rule, contents
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

/// Return only sidecars validated in the same native plugin load snapshot.
/// Runtime projection never re-reads mutable sidecar files from disk.
pub fn active_cccx_extensions(loaded_plugins: &PluginLoadOutcome) -> Vec<LoadedCccxExtension> {
    loaded_plugins
        .plugins()
        .iter()
        .filter(|plugin| plugin.is_active())
        .filter_map(|plugin| {
            let key = dunce::canonicalize(plugin.root.as_path())
                .unwrap_or_else(|_| plugin.root.as_path().to_path_buf());
            let manifest = extension_snapshots()
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&key)
                .cloned()?;
            Some(LoadedCccxExtension {
                config_name: plugin.config_name.clone(),
                display_name: plugin.display_name().to_string(),
                root: plugin.root.clone(),
                manifest,
            })
        })
        .collect()
}

/// Stable MCP ids declared by active CCCX extension sidecars.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CccxExtensionMcpCatalog {
    /// Every extension-provided stable MCP id from active, valid CCCX sidecars.
    pub extension_stable_ids: BTreeSet<String>,
    /// The subset compatible with CX subagent projection.
    pub cx_compatible_stable_ids: BTreeSet<String>,
}

impl CccxExtensionMcpCatalog {
    /// Gate one MCP grant through CCCX extension harness compatibility.
    ///
    /// Non-extension grants are left untouched: project/global MCP servers are
    /// still governed by WorkSpec + parent live catalog projection. Extension
    /// grants fail closed unless the declaring sidecar is CX-compatible.
    pub fn grant_allowed_for_cx(&self, grant: &str) -> bool {
        !self.extension_stable_ids.contains(grant) || self.cx_compatible_stable_ids.contains(grant)
    }
}

pub fn cccx_extension_mcp_catalog(loaded_plugins: &PluginLoadOutcome) -> CccxExtensionMcpCatalog {
    let mut catalog = CccxExtensionMcpCatalog::default();
    for extension in active_cccx_extensions(loaded_plugins) {
        for stable_id in extension.manifest.mcp_stable_ids() {
            catalog.extension_stable_ids.insert(stable_id.to_string());
            if extension.manifest.is_compatible_with_cx_subagent() {
                catalog
                    .cx_compatible_stable_ids
                    .insert(stable_id.to_string());
            }
        }
    }
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;
    use cccx_protocol::plugin::PluginHarness;
    use std::collections::HashMap;
    use std::collections::HashSet;

    const CCCX_PLUGIN_MANIFEST_FILE: &str = "cccx-plugin.json";

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
        fs::write(
            disabled_root.path().join(CCCX_PLUGIN_MANIFEST_FILE),
            manifest,
        )
        .expect("write disabled sidecar");
        crate::loader::load_cccx_extension(active_root.path()).expect("load active sidecar");
        crate::loader::load_cccx_extension(disabled_root.path()).expect("load disabled sidecar");
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

    #[test]
    fn runtime_projection_uses_validated_sidecar_snapshot_after_disk_mutation() {
        let root = tempfile::tempdir().expect("plugin root");
        fs::write(
            root.path().join(CCCX_PLUGIN_MANIFEST_FILE),
            r#"{"cccxSchemaVersion":"1","harness":"cc","mcpTools":[{"stableId":"mcp:plugin.secret","schemaDigest":"sha256:cc"}]}"#,
        )
        .expect("write initial sidecar");
        crate::loader::load_cccx_extension(root.path()).expect("validate initial sidecar");
        fs::write(
            root.path().join(CCCX_PLUGIN_MANIFEST_FILE),
            r#"{"cccxSchemaVersion":"1","harness":"cx","mcpTools":[{"stableId":"mcp:plugin.secret","schemaDigest":"sha256:cc"}]}"#,
        )
        .expect("mutate sidecar");
        let outcome = PluginLoadOutcome::from_plugins(vec![crate::LoadedPlugin {
            config_name: "plugin@test".to_string(),
            manifest_name: Some("plugin".to_string()),
            plugin_namespace: Some("plugin".to_string()),
            manifest_description: None,
            root: AbsolutePathBuf::try_from(root.path()).expect("absolute root"),
            enabled: true,
            skill_roots: Vec::new(),
            disabled_skill_paths: HashSet::new(),
            has_enabled_skills: false,
            mcp_servers: HashMap::new(),
            apps: Vec::new(),
            hook_sources: Vec::new(),
            hook_load_warnings: Vec::new(),
            error: None,
        }]);

        let catalog = cccx_extension_mcp_catalog(&outcome);
        assert!(catalog.extension_stable_ids.contains("mcp:plugin.secret"));
        assert!(
            !catalog
                .cx_compatible_stable_ids
                .contains("mcp:plugin.secret")
        );
    }

    #[test]
    fn cx_mcp_catalog_keeps_only_cx_compatible_extension_grants() {
        let cc_root = tempfile::tempdir().expect("cc plugin root");
        let cx_root = tempfile::tempdir().expect("cx plugin root");
        fs::write(
            cc_root.path().join(CCCX_PLUGIN_MANIFEST_FILE),
            r#"{"cccxSchemaVersion":"1","harness":"cc","mcpTools":[{"stableId":"mcp:cc.secret","schemaDigest":"sha256:cc"}]}"#,
        )
        .expect("write cc sidecar");
        fs::write(
            cx_root.path().join(CCCX_PLUGIN_MANIFEST_FILE),
            r#"{"cccxSchemaVersion":"1","harness":"cx","mcpTools":[{"stableId":"mcp:cx.read","schemaDigest":"sha256:cx"}]}"#,
        )
        .expect("write cx sidecar");
        crate::loader::load_cccx_extension(cc_root.path()).expect("load cc sidecar");
        crate::loader::load_cccx_extension(cx_root.path()).expect("load cx sidecar");
        let plugin = |name: &str, root: &std::path::Path| crate::LoadedPlugin {
            config_name: name.to_string(),
            manifest_name: Some(name.to_string()),
            plugin_namespace: Some(name.to_string()),
            manifest_description: None,
            root: AbsolutePathBuf::try_from(root).expect("absolute plugin root"),
            enabled: true,
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
            plugin("cc@test", cc_root.path()),
            plugin("cx@test", cx_root.path()),
        ]);

        let catalog = cccx_extension_mcp_catalog(&outcome);
        assert!(catalog.extension_stable_ids.contains("mcp:cc.secret"));
        assert!(catalog.extension_stable_ids.contains("mcp:cx.read"));
        assert!(!catalog.grant_allowed_for_cx("mcp:cc.secret"));
        assert!(catalog.grant_allowed_for_cx("mcp:cx.read"));
        assert!(
            catalog.grant_allowed_for_cx("mcp:project_configured.search"),
            "non-extension MCP grants remain governed by WorkSpec and parent live catalog"
        );
    }
}
