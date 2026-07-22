#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Result;
use cccx_protocol::ArtifactStatus;
use cccx_protocol::EvidenceKind;
use cccx_protocol::ExpectedArtifact;
use cccx_protocol::ForkPolicy;
use cccx_protocol::HarnessKind;
use cccx_protocol::ModelRouteSelection;
use cccx_protocol::ParentCapabilitySet;
use cccx_protocol::PermissionPolicy;
use cccx_protocol::RevisionRef;
use cccx_protocol::ToolGrant;
use cccx_protocol::WorkRole;
use cccx_protocol::WorkScope;
use cccx_protocol::WorkSpec;
use codex_core::cccx_spawn::apply_cccx_harness_to_config;
use codex_core::config::ConfigBuilder;
use codex_core_plugins::PluginsManager;
use codex_core_plugins::cccx_extension::cccx_extension_mcp_catalog;
use codex_features::Feature;
use tempfile::TempDir;

const CX_PLUGIN_ID: &str = "cx@test";
const CC_PLUGIN_ID: &str = "cc@test";
const INVALID_PLUGIN_ID: &str = "invalid@test";
const CX_STABLE_ID: &str = "mcp:cx_plugin.search";
const CC_STABLE_ID: &str = "mcp:cc_plugin.secret";

fn plugin_root(home: &TempDir, plugin_id: &str) -> std::path::PathBuf {
    let (name, marketplace) = plugin_id
        .split_once('@')
        .expect("test plugin id should include marketplace");
    home.path()
        .join("plugins/cache")
        .join(marketplace)
        .join(name)
        .join("local")
}

fn write_native_plugin(
    home: &TempDir,
    plugin_id: &str,
    display_name: &str,
    sidecar: &str,
) -> Result<()> {
    let root = plugin_root(home, plugin_id);
    std::fs::create_dir_all(root.join(".codex-plugin"))?;
    std::fs::write(
        root.join(".codex-plugin/plugin.json"),
        format!(r#"{{"name":"{display_name}","description":"runtime plugin"}}"#),
    )?;
    std::fs::write(
        root.join(".mcp.json"),
        r#"{"mcpServers":{"sample":{"command":"echo","args":["ready"]}}}"#,
    )?;
    std::fs::write(root.join("cccx-plugin.json"), sidecar)?;
    Ok(())
}

fn write_config(home: &TempDir) -> Result<()> {
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            "[features]\nplugins = true\n\n\
             [plugins.\"{CX_PLUGIN_ID}\"]\nenabled = true\n\n\
             [plugins.\"{CC_PLUGIN_ID}\"]\nenabled = true\n\n\
             [plugins.\"{INVALID_PLUGIN_ID}\"]\nenabled = true\n"
        ),
    )?;
    Ok(())
}

async fn load_config_and_plugins(
    home: Arc<TempDir>,
) -> Result<(
    codex_core::config::Config,
    codex_core_plugins::PluginLoadOutcome,
)> {
    let mut config = ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .fallback_cwd(Some(home.path().to_path_buf()))
        .build()
        .await?;
    config
        .features
        .enable(Feature::Plugins)
        .expect("test config should allow plugin feature");
    let manager = PluginsManager::new(home.path().to_path_buf());
    let loaded_plugins = manager
        .plugins_for_config(&config.plugins_config_input())
        .await;
    Ok((config, loaded_plugins))
}

fn work_spec(allowed_mcp_tools: &[&str], grants: Vec<ToolGrant>) -> WorkSpec {
    WorkSpec {
        schema_version: cccx_protocol::CCCX_WORK_SPEC_SCHEMA_VERSION.to_string(),
        work_spec_id: "ws-plugin-runtime".to_string(),
        task_id: "task-plugin-runtime".to_string(),
        attempt_id: "attempt-plugin-runtime".to_string(),
        created_at: "2026-07-22T00:00:00Z".to_string(),
        expires_at: None,
        base_revision: RevisionRef {
            git_head: Some("abc123".to_string()),
            working_tree_digest: Some("sha256:tree".to_string()),
        },
        harness: HarnessKind::Cx,
        role: WorkRole::Implementer,
        runtime_identity: None,
        objective: "prove plugin trust and CX isolation".to_string(),
        scope: WorkScope {
            canonical_paths: BTreeSet::from([".".to_string()]),
            protected_paths: BTreeSet::new(),
            advisory_globs: BTreeSet::new(),
        },
        allowed_tools: BTreeSet::from(["shell_command".to_string()]),
        forbidden_tools: BTreeSet::new(),
        allowed_mcp_tools: allowed_mcp_tools
            .iter()
            .map(|tool| (*tool).to_string())
            .collect(),
        tool_grants: grants,
        permission_policy: PermissionPolicy::Ask,
        model_route: ModelRouteSelection::Explicit {
            route_id: "route/cx-plugin".to_string(),
        },
        resolved_model_route: None,
        fallback_policy_id: "strict".to_string(),
        fork_policy: ForkPolicy::WorkSpecOnly,
        enforced_constraints: Vec::new(),
        typed_constraints: Vec::new(),
        advisory_constraints: Vec::new(),
        risk_threshold: None,
        action_classes: None,
        approval_policy: None,
        verification_spec: None,
        audit_depth: None,
        expected_artifact: Some(ExpectedArtifact {
            required_status: ArtifactStatus::Success,
            required_evidence_kinds: vec![EvidenceKind::TestLog],
            required_fields: Vec::new(),
            verification_policy_id: None,
        }),
        checkpoint_id: None,
        base_commit: None,
    }
}

fn parent_capabilities(
    cx_visible_mcp_tools: BTreeSet<String>,
    root: &TempDir,
) -> ParentCapabilitySet {
    ParentCapabilitySet {
        tools: BTreeSet::from(["shell_command".to_string(), "apply_patch".to_string()]),
        mcp_tools: cx_visible_mcp_tools,
        writable_roots: BTreeSet::from([root.path().to_string_lossy().to_string()]),
        protected_paths: BTreeSet::new(),
        model_routes: BTreeSet::from(["route/cx-plugin".to_string()]),
        permission_ceiling: PermissionPolicy::Ask,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cccx_plugin_trust_isolation_projects_only_workspec_granted_cx_sidecars() -> Result<()> {
    let home = Arc::new(TempDir::new()?);
    write_native_plugin(
        home.as_ref(),
        CX_PLUGIN_ID,
        "cx plugin",
        &format!(
            r#"{{
  "cccxSchemaVersion": "1",
  "harness": "cx",
  "mcpTools": [{{"stableId": "{CX_STABLE_ID}", "schemaDigest": "sha256:cx"}}],
  "hooks": [{{"event": "PreToolUse", "command": "echo sidecar-only"}}]
}}"#
        ),
    )?;
    write_native_plugin(
        home.as_ref(),
        CC_PLUGIN_ID,
        "cc plugin",
        &format!(
            r#"{{
  "cccxSchemaVersion": "1",
  "harness": "cc",
  "mcpTools": [{{"stableId": "{CC_STABLE_ID}", "schemaDigest": "sha256:cc"}}]
}}"#
        ),
    )?;
    write_native_plugin(
        home.as_ref(),
        INVALID_PLUGIN_ID,
        "invalid plugin",
        r#"{"cccxSchemaVersion":"1","harness":"cx","unexpected":true}"#,
    )?;
    write_config(home.as_ref())?;

    let (mut child_config, loaded_plugins) = load_config_and_plugins(Arc::clone(&home)).await?;
    let cx_plugin = loaded_plugins
        .plugins()
        .iter()
        .find(|plugin| plugin.config_name == CX_PLUGIN_ID)
        .expect("cx native plugin should load");
    assert!(
        cx_plugin.error.is_none(),
        "cx plugin load error: {:?}",
        cx_plugin.error
    );
    assert!(
        cx_plugin.hook_sources.is_empty(),
        "cccx sidecar hooks must not materialize outside native hook trust flow"
    );

    let invalid_plugin = loaded_plugins
        .plugins()
        .iter()
        .find(|plugin| plugin.config_name == INVALID_PLUGIN_ID)
        .expect("invalid native plugin should be represented");
    assert!(
        invalid_plugin.error.is_some(),
        "invalid cccx sidecar should fail the plugin closed"
    );

    std::fs::write(
        plugin_root(home.as_ref(), CX_PLUGIN_ID).join("cccx-plugin.json"),
        &format!(
            r#"{{"cccxSchemaVersion":"1","harness":"cc","mcpTools":[{{"stableId":"{CX_STABLE_ID}","schemaDigest":"sha256:cx"}}]}}"#
        ),
    )?;

    let extension_catalog = cccx_extension_mcp_catalog(&loaded_plugins);
    assert!(
        extension_catalog
            .extension_stable_ids
            .contains(CX_STABLE_ID)
    );
    assert!(
        extension_catalog
            .extension_stable_ids
            .contains(CC_STABLE_ID)
    );
    assert!(
        extension_catalog
            .cx_compatible_stable_ids
            .contains(CX_STABLE_ID),
        "runtime must use the admitted sidecar snapshot, not the mutated file"
    );
    assert!(!extension_catalog.grant_allowed_for_cx(CC_STABLE_ID));

    let cx_visible_parent_mcp_tools = extension_catalog
        .extension_stable_ids
        .iter()
        .filter(|stable_id| extension_catalog.grant_allowed_for_cx(stable_id))
        .cloned()
        .collect::<BTreeSet<_>>();
    assert!(cx_visible_parent_mcp_tools.contains(CX_STABLE_ID));
    assert!(!cx_visible_parent_mcp_tools.contains(CC_STABLE_ID));

    let parent = parent_capabilities(cx_visible_parent_mcp_tools, home.as_ref());
    apply_cccx_harness_to_config(&mut child_config, &parent, &work_spec(&[], Vec::new()))?;
    assert!(
        !child_config
            .cccx_child_allowed_mcp_tools
            .contains(CX_STABLE_ID),
        "compatible plugin grant must not be visible without a WorkSpec grant"
    );

    let mut child_config_with_grant = child_config.clone();
    apply_cccx_harness_to_config(
        &mut child_config_with_grant,
        &parent,
        &work_spec(
            &[CX_STABLE_ID, CC_STABLE_ID],
            vec![ToolGrant {
                stable_id: CX_STABLE_ID.to_string(),
                server_id: Some("cx_plugin".to_string()),
                schema_digest: Some("sha256:cx".to_string()),
            }],
        ),
    )?;
    assert!(
        child_config_with_grant
            .cccx_child_allowed_mcp_tools
            .contains(CX_STABLE_ID),
        "trusted CX-compatible plugin MCP grant should reach the spawned CX catalog"
    );
    assert!(
        !child_config_with_grant
            .cccx_child_allowed_mcp_tools
            .contains(CC_STABLE_ID),
        "CC-only/native plugin grant must not leak into spawned CX catalog"
    );

    Ok(())
}
