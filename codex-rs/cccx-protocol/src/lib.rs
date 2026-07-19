//! cccx WorkSpec/Artifact protocol contracts.
//!
//! This crate is intentionally small and independent from `codex-core` so the
//! protocol and child-config safety rules can be tested without building the
//! full runtime.

pub mod plugin;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use thiserror::Error;

pub const CCCX_PROTOCOL_SCHEMA_VERSION: &str = "cccx.protocol.v1";
pub const CCCX_WORK_SPEC_SCHEMA_VERSION: &str = "cccx.work_spec.v1";
pub const CCCX_ARTIFACT_SCHEMA_VERSION: &str = "cccx.artifact.v1";
pub const CCCX_VERIFICATION_RECORD_SCHEMA_VERSION: &str = "cccx.verification_record.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessKind {
    Cc,
    Cx,
}

/// Who is making the current LLM/spawn call.
///
/// architecture.md: "Main CC là scheduler duy nhất. CX/CC subagent không tự spawn."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActorIdentity {
    /// The root Main CC agent — has full scheduling authority.
    MainCc,
    /// A CC-harness subagent — cannot spawn further agents.
    CcSubagent,
    /// A CX-harness subagent — no spawn, no ADR, no sequential-thinking.
    CxSubagent,
}

impl ActorIdentity {
    /// Returns `true` when this actor is permitted to spawn subagents.
    pub fn can_spawn(&self) -> bool {
        matches!(self, Self::MainCc)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkRole {
    Researcher,
    Architect,
    Planner,
    Implementer,
    Tester,
    Auditor,
    Resolver,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStatus {
    Success,
    Blocked,
    Interrupted,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    TestLog,
    BuildLog,
    DiffDigest,
    ReviewFinding,
    RuntimeLog,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPolicy {
    Deny,
    Ask,
    Allow,
}

impl PermissionPolicy {
    fn intersect(self, parent: Self) -> Self {
        use PermissionPolicy::*;
        match (self, parent) {
            (Deny, _) | (_, Deny) => Deny,
            (Ask, _) | (_, Ask) => Ask,
            (Allow, Allow) => Allow,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ForkPolicy {
    WorkSpecOnly,
    FullHistoryDebug,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelRouteSelection {
    InheritMainRequested,
    InheritMainEffective,
    Explicit { route_id: String },
}

impl ModelRouteSelection {
    fn explicit_route(&self) -> Option<&str> {
        match self {
            Self::Explicit { route_id } => Some(route_id.as_str()),
            Self::InheritMainRequested | Self::InheritMainEffective => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkScope {
    pub canonical_paths: BTreeSet<String>,
    #[serde(default)]
    pub protected_paths: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicyKind {
    AutoDeny,
    AskCc,
    AskUser,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RiskThreshold {
    pub max_files_changed: usize,
    pub max_lines_changed: usize,
    #[serde(default)]
    pub forbidden_patterns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActionClass {
    pub action_type: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RevisionRef {
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_head: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_tree_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeIdentity {
    pub logical_session_id: String,
    pub worker_incarnation_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolGrant {
    pub stable_id: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    NoEdit,
    PathAllow,
    PathDeny,
    ActionClass,
    RiskThreshold,
    VerificationBoundary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkConstraint {
    pub id: String,
    pub kind: ConstraintKind,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandSpec {
    pub argv: Vec<String>,
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationSpec {
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandSpec>,
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub protected_digests: BTreeMap<String, String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_policy_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExpectedArtifact {
    pub required_status: ArtifactStatus,
    #[serde(default)]
    pub required_evidence_kinds: Vec<EvidenceKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkSpec {
    pub schema_version: String,
    pub work_spec_id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub created_at: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub base_revision: RevisionRef,
    pub harness: HarnessKind,
    pub role: WorkRole,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_identity: Option<RuntimeIdentity>,
    pub objective: String,
    pub scope: WorkScope,
    #[serde(default)]
    pub allowed_tools: BTreeSet<String>,
    #[serde(default)]
    pub forbidden_tools: BTreeSet<String>,
    #[serde(default)]
    pub allowed_mcp_tools: BTreeSet<String>,
    #[serde(default)]
    pub tool_grants: Vec<ToolGrant>,
    pub permission_policy: PermissionPolicy,
    pub model_route: ModelRouteSelection,
    pub fallback_policy_id: String,
    pub fork_policy: ForkPolicy,
    #[serde(default)]
    pub enforced_constraints: Vec<String>,
    #[serde(default)]
    pub typed_constraints: Vec<WorkConstraint>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_threshold: Option<RiskThreshold>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_classes: Option<Vec<ActionClass>>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<ApprovalPolicyKind>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_spec: Option<VerificationSpec>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_artifact: Option<ExpectedArtifact>,
}

impl WorkSpec {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        require_exact_version(
            "work_spec",
            &self.schema_version,
            CCCX_WORK_SPEC_SCHEMA_VERSION,
        )?;
        require_non_empty("work_spec_id", &self.work_spec_id)?;
        require_non_empty("task_id", &self.task_id)?;
        require_non_empty("attempt_id", &self.attempt_id)?;
        require_non_empty("created_at", &self.created_at)?;
        require_non_empty("objective", &self.objective)?;
        require_non_empty("fallback_policy_id", &self.fallback_policy_id)?;
        validate_role_for_harness(&self.harness, &self.role)?;
        if let Some(identity) = &self.runtime_identity {
            identity.validate()?;
        }
        if self.scope.canonical_paths.is_empty() {
            return Err(ProtocolError::MissingField("scope.canonical_paths"));
        }
        if self.allowed_tools.is_empty()
            && self.allowed_mcp_tools.is_empty()
            && self.tool_grants.is_empty()
        {
            return Err(ProtocolError::MissingField(
                "allowed_tools, allowed_mcp_tools, or tool_grants",
            ));
        }
        validate_tool_grants(&self.tool_grants)?;
        if let Some(tool) = self
            .allowed_tools
            .intersection(&self.forbidden_tools)
            .next()
            .cloned()
        {
            return Err(ProtocolError::ConflictingTool(tool));
        }
        if matches!(self.harness, HarnessKind::Cx)
            && matches!(self.fork_policy, ForkPolicy::FullHistoryDebug)
        {
            return Err(ProtocolError::FullHistoryCxWorker);
        }
        if let Some(route_id) = self.model_route.explicit_route() {
            require_non_empty("model_route.route_id", route_id)?;
        }
        for constraint in &self.typed_constraints {
            constraint.validate()?;
        }
        if let Some(spec) = &self.verification_spec {
            spec.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub summary: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactProvenance {
    pub producer_harness: HarnessKind,
    pub producer_role: WorkRole,
    pub provider_gateway_request_id: String,
    pub model_profile_id: String,
    pub capability_profile_id: String,
    pub fallback_policy_id: String,
    pub requested_route_id: String,
    pub effective_route_id: String,
    pub gateway_request_id: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_model: Option<String>,
    #[serde(default)]
    pub retry_count: usize,
    #[serde(default)]
    pub fallback_chain: Vec<String>,
    pub input_digest: String,
    pub output_digest: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_sequence_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Artifact {
    pub schema_version: String,
    pub artifact_id: String,
    pub task_id: String,
    pub work_spec_id: String,
    pub attempt_id: String,
    pub status: ArtifactStatus,
    pub summary: String,
    #[serde(default)]
    pub changed_paths: BTreeSet<String>,
    #[serde(default)]
    pub changed_files: BTreeSet<String>,
    #[serde(default)]
    pub tests_run: Vec<TestRunRecord>,
    #[serde(default)]
    pub residual_risks: Vec<String>,
    #[serde(default)]
    pub follow_up: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    pub provenance: Option<ArtifactProvenance>,
    pub verification: Option<VerificationRecord>,
}

impl Artifact {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        require_exact_version(
            "artifact",
            &self.schema_version,
            CCCX_ARTIFACT_SCHEMA_VERSION,
        )?;
        require_non_empty("artifact_id", &self.artifact_id)?;
        require_non_empty("task_id", &self.task_id)?;
        require_non_empty("work_spec_id", &self.work_spec_id)?;
        require_non_empty("attempt_id", &self.attempt_id)?;
        require_non_empty("summary", &self.summary)?;

        if self.provenance.is_none() {
            return Err(ProtocolError::MissingField("provenance"));
        }

        if matches!(self.status, ArtifactStatus::Success) && self.evidence.is_empty() {
            return Err(ProtocolError::MissingField("evidence"));
        }

        if let Some(provenance) = &self.provenance {
            provenance.validate()?;
            // Phase 5: reject placeholder digests like sha256:cccx-runtime-*
            provenance.verify_placeholders()?;
        }

        for evidence in &self.evidence {
            evidence.validate()?;
        }

        for test_run in &self.tests_run {
            test_run.validate()?;
        }

        if let Some(verification) = &self.verification {
            verification.validate()?;
        }

        Ok(())
    }
}

impl Evidence {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        require_non_empty("evidence.summary", &self.summary)?;
        require_non_empty("evidence.digest", &self.digest)
    }
}

impl ArtifactProvenance {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        validate_role_for_harness(&self.producer_harness, &self.producer_role)?;
        require_non_empty(
            "provenance.provider_gateway_request_id",
            &self.provider_gateway_request_id,
        )?;
        require_non_empty("provenance.model_profile_id", &self.model_profile_id)?;
        require_non_empty(
            "provenance.capability_profile_id",
            &self.capability_profile_id,
        )?;
        require_non_empty("provenance.fallback_policy_id", &self.fallback_policy_id)?;
        require_non_empty("provenance.requested_route_id", &self.requested_route_id)?;
        require_non_empty("provenance.effective_route_id", &self.effective_route_id)?;
        require_non_empty("provenance.input_digest", &self.input_digest)?;
        require_non_empty("provenance.output_digest", &self.output_digest)
    }

    /// Phase 5: reject provenance entries that carry placeholder-style digests.
    /// architecture.md §12: "Provenance không còn placeholder `sha256:cccx-runtime-*`."
    ///
    /// This is the protocol-layer gate. Runtime code (cccx_runtime::provenance_for_runtime)
    /// already produces real SHA256 digests; this method catches any regression that
    /// re-introduces placeholder patterns.
    pub fn verify_placeholders(&self) -> Result<(), ProtocolError> {
        let fields: [(&str, &str); 4] = [
            ("input_digest", &self.input_digest),
            ("output_digest", &self.output_digest),
            ("requested_route_id", &self.requested_route_id),
            ("effective_route_id", &self.effective_route_id),
        ];
        for (name, value) in &fields {
            if value.contains("cccx-runtime") {
                return Err(ProtocolError::PlaceholderProvenance(
                    name,
                    value.to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Phase 5: verify digest fields use sha256: format with 64 hex chars.
    /// Separate from verify_placeholders() — this is a stricter cryptographic
    /// audit that runtime code can call explicitly.
    pub fn verify_digest_format(&self) -> Result<(), ProtocolError> {
        for (name, value) in [
            ("input_digest", &self.input_digest),
            ("output_digest", &self.output_digest),
        ] {
            let hex = value.strip_prefix("sha256:").ok_or_else(|| {
                ProtocolError::PlaceholderProvenance(
                    name,
                    format!("missing sha256: prefix in '{value}'"),
                )
            })?;
            if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(ProtocolError::PlaceholderProvenance(
                    name,
                    format!("not a valid sha256 digest: {value}"),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationRecord {
    pub schema_version: String,
    pub task_id: String,
    pub attempt_id: String,
    pub verifier: String,
    pub policy_id: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandExecution>,
    pub artifact_digest: String,
    pub result_revision: RevisionRef,
    pub passed: bool,
    pub checked_at: String,
    pub evidence_digest: String,
    #[serde(default)]
    pub findings: Vec<String>,
    pub reason: String,
}

impl VerificationRecord {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        require_exact_version(
            "verification_record",
            &self.schema_version,
            CCCX_VERIFICATION_RECORD_SCHEMA_VERSION,
        )?;
        require_non_empty("verification.task_id", &self.task_id)?;
        require_non_empty("verification.attempt_id", &self.attempt_id)?;
        require_non_empty("verification.verifier", &self.verifier)?;
        require_non_empty("verification.policy_id", &self.policy_id)?;
        require_non_empty("verification.checked_at", &self.checked_at)?;
        require_non_empty("verification.evidence_digest", &self.evidence_digest)?;
        require_non_empty("verification.artifact_digest", &self.artifact_digest)?;
        require_non_empty("verification.reason", &self.reason)?;
        if let Some(command) = &self.command {
            command.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TestRunRecord {
    pub command: String,
    pub exit_code: i32,
    pub log_digest: String,
}

impl TestRunRecord {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        require_non_empty("test_run.command", &self.command)?;
        require_non_empty("test_run.log_digest", &self.log_digest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandExecution {
    pub argv: Vec<String>,
    pub cwd: String,
    pub exit_code: i32,
    pub started_at: String,
    pub finished_at: String,
    pub log_digest: String,
}

impl CommandExecution {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.argv.is_empty() {
            return Err(ProtocolError::MissingField("verification.command.argv"));
        }
        require_non_empty("verification.command.cwd", &self.cwd)?;
        require_non_empty("verification.command.started_at", &self.started_at)?;
        require_non_empty("verification.command.finished_at", &self.finished_at)?;
        require_non_empty("verification.command.log_digest", &self.log_digest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ParentCapabilitySet {
    #[serde(default)]
    pub tools: BTreeSet<String>,
    #[serde(default)]
    pub mcp_tools: BTreeSet<String>,
    #[serde(default)]
    pub writable_roots: BTreeSet<String>,
    #[serde(default)]
    pub protected_paths: BTreeSet<String>,
    #[serde(default)]
    pub model_routes: BTreeSet<String>,
    pub permission_ceiling: PermissionPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChildConfigProjection {
    pub deny_by_default: bool,
    pub harness: HarnessKind,
    pub role: WorkRole,
    pub allowed_tools: BTreeSet<String>,
    pub allowed_mcp_tools: BTreeSet<String>,
    pub writable_scope: BTreeSet<String>,
    pub protected_paths: BTreeSet<String>,
    pub permission_policy: PermissionPolicy,
    pub model_route: ModelRouteSelection,
    pub fallback_policy_id: String,
}

impl ChildConfigProjection {
    pub fn can_use_tool(&self, tool: &str) -> bool {
        self.allowed_tools.contains(tool)
    }

    pub fn can_use_mcp_tool(&self, tool: &str) -> bool {
        self.allowed_mcp_tools.contains(tool)
    }

    pub fn can_write_path(&self, path: &str) -> bool {
        let normalized = normalize_path(path);
        self.writable_scope
            .iter()
            .any(|root| path_within(&normalized, root))
            && !self
                .protected_paths
                .iter()
                .any(|protected| path_within(&normalized, protected))
    }
}

pub fn project_child_config(
    work_spec: &WorkSpec,
    parent: &ParentCapabilitySet,
) -> Result<ChildConfigProjection, ProtocolError> {
    work_spec.validate()?;

    if let Some(route_id) = work_spec.model_route.explicit_route()
        && !parent.model_routes.contains(route_id)
    {
        return Err(ProtocolError::ModelRouteNotAllowed(route_id.to_string()));
    }

    let allowed_tools = work_spec
        .allowed_tools
        .intersection(&parent.tools)
        .filter(|tool| !work_spec.forbidden_tools.contains(*tool))
        .cloned()
        .collect::<BTreeSet<_>>();

    let allowed_mcp_tools = work_spec
        .allowed_mcp_tools
        .intersection(&parent.mcp_tools)
        .cloned()
        .collect::<BTreeSet<_>>();

    let protected_paths = work_spec
        .scope
        .protected_paths
        .iter()
        .chain(parent.protected_paths.iter())
        .map(|path| normalize_path(path))
        .collect::<BTreeSet<_>>();

    let writable_scope = work_spec
        .scope
        .canonical_paths
        .iter()
        .map(|path| normalize_path(path))
        .filter(|scope_path| {
            parent
                .writable_roots
                .iter()
                .map(|root| normalize_path(root))
                .any(|root| path_within(scope_path, &root))
                && !protected_paths
                    .iter()
                    .any(|protected| path_within(scope_path, protected))
        })
        .collect::<BTreeSet<_>>();

    Ok(ChildConfigProjection {
        deny_by_default: true,
        harness: work_spec.harness.clone(),
        role: work_spec.role.clone(),
        allowed_tools,
        allowed_mcp_tools,
        writable_scope,
        protected_paths,
        permission_policy: work_spec
            .permission_policy
            .clone()
            .intersect(parent.permission_ceiling.clone()),
        model_route: work_spec.model_route.clone(),
        fallback_policy_id: work_spec.fallback_policy_id.clone(),
    })
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("unsupported schema version `{0}`")]
    UnsupportedSchemaVersion(String),
    #[error("missing required field `{0}`")]
    MissingField(&'static str),
    #[error("tool `{0}` is both allowed and forbidden")]
    ConflictingTool(String),
    #[error("full-history fork cannot create a CX execution worker")]
    FullHistoryCxWorker,
    #[error("model route `{0}` is not allowed by the parent capability set")]
    ModelRouteNotAllowed(String),
    #[error("role `{role:?}` is not valid for harness `{harness:?}`")]
    RoleHarnessMismatch {
        harness: HarnessKind,
        role: WorkRole,
    },
    #[error("duplicate tool grant stable id `{0}`")]
    DuplicateToolGrant(String),
    /// Phase 5: provenance cannot contain placeholder digests.
    #[error("provenance field `{0}` contains placeholder value `{1}`")]
    PlaceholderProvenance(&'static str, String),
}

fn require_exact_version(
    _schema_name: &'static str,
    version: &str,
    expected: &'static str,
) -> Result<(), ProtocolError> {
    if version == expected {
        Ok(())
    } else {
        Err(ProtocolError::UnsupportedSchemaVersion(version.to_string()))
    }
}

fn require_non_empty(field: &'static str, value: &str) -> Result<(), ProtocolError> {
    if value.trim().is_empty() {
        Err(ProtocolError::MissingField(field))
    } else {
        Ok(())
    }
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_string()
}

fn path_within(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn validate_role_for_harness(harness: &HarnessKind, role: &WorkRole) -> Result<(), ProtocolError> {
    let valid = match harness {
        HarnessKind::Cc => matches!(
            role,
            WorkRole::Researcher | WorkRole::Architect | WorkRole::Planner | WorkRole::Auditor
        ),
        HarnessKind::Cx => matches!(
            role,
            WorkRole::Implementer | WorkRole::Tester | WorkRole::Auditor | WorkRole::Resolver
        ),
    };
    if valid {
        Ok(())
    } else {
        Err(ProtocolError::RoleHarnessMismatch {
            harness: harness.clone(),
            role: role.clone(),
        })
    }
}

fn validate_tool_grants(grants: &[ToolGrant]) -> Result<(), ProtocolError> {
    let mut seen = BTreeSet::new();
    for grant in grants {
        require_non_empty("tool_grants.stable_id", &grant.stable_id)?;
        if !seen.insert(grant.stable_id.clone()) {
            return Err(ProtocolError::DuplicateToolGrant(grant.stable_id.clone()));
        }
        if let Some(server_id) = &grant.server_id {
            require_non_empty("tool_grants.server_id", server_id)?;
        }
        if let Some(schema_digest) = &grant.schema_digest {
            require_non_empty("tool_grants.schema_digest", schema_digest)?;
        }
    }
    Ok(())
}

impl RuntimeIdentity {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        require_non_empty(
            "runtime_identity.logical_session_id",
            &self.logical_session_id,
        )?;
        require_non_empty(
            "runtime_identity.worker_incarnation_id",
            &self.worker_incarnation_id,
        )?;
        require_non_empty("runtime_identity.display_name", &self.display_name)
    }
}

impl WorkConstraint {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        require_non_empty("typed_constraints.id", &self.id)?;
        require_non_empty("typed_constraints.description", &self.description)
    }
}

impl VerificationSpec {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if let Some(command) = &self.command {
            if command.argv.is_empty() {
                return Err(ProtocolError::MissingField(
                    "verification_spec.command.argv",
                ));
            }
            require_non_empty("verification_spec.command.cwd", &command.cwd)?;
        }
        for (path, digest) in &self.protected_digests {
            require_non_empty("verification_spec.protected_digests.path", path)?;
            require_non_empty("verification_spec.protected_digests.digest", digest)?;
        }
        if let Some(policy_id) = &self.verification_policy_id {
            require_non_empty("verification_spec.verification_policy_id", policy_id)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn sample_work_spec() -> WorkSpec {
        WorkSpec {
            schema_version: CCCX_WORK_SPEC_SCHEMA_VERSION.to_string(),
            work_spec_id: "ws-auth-1".to_string(),
            task_id: "T1".to_string(),
            attempt_id: "attempt-1".to_string(),
            created_at: "2026-07-16T00:00:00Z".to_string(),
            expires_at: Some("2026-07-16T01:00:00Z".to_string()),
            base_revision: RevisionRef {
                git_head: Some("abc123".to_string()),
                working_tree_digest: Some("sha256:base".to_string()),
            },
            harness: HarnessKind::Cx,
            role: WorkRole::Implementer,
            runtime_identity: Some(RuntimeIdentity {
                logical_session_id: "worker-auth".to_string(),
                worker_incarnation_id: "worker-auth-1".to_string(),
                display_name: "a1-auth-fix".to_string(),
            }),
            objective: "Implement bounded auth fix".to_string(),
            scope: WorkScope {
                canonical_paths: set(&["D:/repo/src/auth"]),
                protected_paths: set(&["D:/repo/src/auth/generated"]),
            },
            allowed_tools: set(&["apply_patch"]),
            forbidden_tools: set(&["shell"]),
            allowed_mcp_tools: set(&["codebase.search_graph"]),
            tool_grants: vec![ToolGrant {
                stable_id: "apply_patch".to_string(),
                server_id: None,
                schema_digest: Some("sha256:tool".to_string()),
            }],
            permission_policy: PermissionPolicy::Ask,
            model_route: ModelRouteSelection::Explicit {
                route_id: "9router/anthropic/claude-sonnet-4.5".to_string(),
            },
            fallback_policy_id: "compatible".to_string(),
            fork_policy: ForkPolicy::WorkSpecOnly,
            enforced_constraints: vec!["no-out-of-scope-edits".to_string()],
            typed_constraints: vec![WorkConstraint {
                id: "no-out-of-scope-edits".to_string(),
                kind: ConstraintKind::PathAllow,
                description: "Only edit paths in scope".to_string(),
            }],
            risk_threshold: None,
            action_classes: None,
            approval_policy: None,
            verification_spec: Some(VerificationSpec {
                command: Some(CommandSpec {
                    argv: vec!["cargo".to_string(), "test".to_string()],
                    cwd: "D:/repo".to_string(),
                }),
                protected_digests: BTreeMap::from([(
                    "D:/repo/src/auth/generated".to_string(),
                    "sha256:protected".to_string(),
                )]),
                verification_policy_id: Some("focused-tests".to_string()),
            }),
            expected_artifact: Some(ExpectedArtifact {
                required_status: ArtifactStatus::Success,
                required_evidence_kinds: vec![EvidenceKind::TestLog],
            }),
        }
    }

    fn sample_artifact() -> Artifact {
        Artifact {
            schema_version: CCCX_ARTIFACT_SCHEMA_VERSION.to_string(),
            artifact_id: "art-auth-1".to_string(),
            task_id: "T1".to_string(),
            work_spec_id: "ws-auth-1".to_string(),
            attempt_id: "attempt-1".to_string(),
            status: ArtifactStatus::Success,
            summary: "Applied auth fix and ran focused test.".to_string(),
            changed_paths: set(&["D:/repo/src/auth/session.rs"]),
            changed_files: set(&["D:/repo/src/auth/session.rs"]),
            tests_run: vec![TestRunRecord {
                command: "cargo test auth_session_tests".to_string(),
                exit_code: 0,
                log_digest: "sha256:test-log".to_string(),
            }],
            residual_risks: Vec::new(),
            follow_up: Vec::new(),
            evidence: vec![Evidence {
                kind: EvidenceKind::TestLog,
                summary: "auth_session_tests passed".to_string(),
                digest: "sha256:test-log".to_string(),
            }],
            provenance: Some(ArtifactProvenance {
                producer_harness: HarnessKind::Cx,
                producer_role: WorkRole::Implementer,
                provider_gateway_request_id: "gw-1".to_string(),
                model_profile_id: "profile-claude-sonnet".to_string(),
                capability_profile_id: "coding-tools-v1".to_string(),
                fallback_policy_id: "compatible".to_string(),
                requested_route_id: "9router/anthropic/claude-sonnet-4.5".to_string(),
                effective_route_id: "9router/anthropic/claude-sonnet-4.5".to_string(),
                gateway_request_id: Some("gw-1".to_string()),
                requested_model: Some("claude-sonnet-4.5".to_string()),
                effective_model: Some("claude-sonnet-4.5".to_string()),
                retry_count: 0,
                fallback_chain: Vec::new(),
                input_digest: "sha256:input".to_string(),
                output_digest: "sha256:output".to_string(),
                tool_call_sequence_digest: Some("sha256:tool-seq".to_string()),
            }),
            verification: Some(VerificationRecord {
                schema_version: CCCX_VERIFICATION_RECORD_SCHEMA_VERSION.to_string(),
                task_id: "T1".to_string(),
                attempt_id: "attempt-1".to_string(),
                verifier: "cccx:self".to_string(),
                policy_id: "focused-tests".to_string(),
                command: Some(CommandExecution {
                    argv: vec!["cargo".to_string(), "test".to_string()],
                    cwd: "D:/repo".to_string(),
                    exit_code: 0,
                    started_at: "2026-07-16T00:00:00Z".to_string(),
                    finished_at: "2026-07-16T00:01:00Z".to_string(),
                    log_digest: "sha256:test-log".to_string(),
                }),
                artifact_digest: "sha256:artifact".to_string(),
                result_revision: RevisionRef {
                    git_head: Some("abc124".to_string()),
                    working_tree_digest: Some("sha256:result".to_string()),
                },
                passed: true,
                checked_at: "2026-07-16T00:00:00Z".to_string(),
                evidence_digest: "sha256:test-log".to_string(),
                findings: Vec::new(),
                reason: "focused tests passed".to_string(),
            }),
        }
    }

    fn parent_capabilities() -> ParentCapabilitySet {
        ParentCapabilitySet {
            tools: set(&["apply_patch", "shell", "read_file"]),
            mcp_tools: set(&["codebase.search_graph", "codebase.query_graph"]),
            writable_roots: set(&["D:/repo"]),
            protected_paths: set(&["D:/repo/.git"]),
            model_routes: set(&["9router/anthropic/claude-sonnet-4.5"]),
            permission_ceiling: PermissionPolicy::Allow,
        }
    }

    #[test]
    fn workspec_round_trips_with_stable_json_shape() {
        let work_spec = sample_work_spec();

        let value = serde_json::to_value(&work_spec).expect("serialize workspec");

        assert_eq!(
            value,
            json!({
                "schema_version": "cccx.work_spec.v1",
                "work_spec_id": "ws-auth-1",
                "task_id": "T1",
                "attempt_id": "attempt-1",
                "created_at": "2026-07-16T00:00:00Z",
                "expires_at": "2026-07-16T01:00:00Z",
                "base_revision": {
                    "git_head": "abc123",
                    "working_tree_digest": "sha256:base"
                },
                "harness": "cx",
                "role": "implementer",
                "runtime_identity": {
                    "logical_session_id": "worker-auth",
                    "worker_incarnation_id": "worker-auth-1",
                    "display_name": "a1-auth-fix"
                },
                "objective": "Implement bounded auth fix",
                "scope": {
                    "canonical_paths": ["D:/repo/src/auth"],
                    "protected_paths": ["D:/repo/src/auth/generated"]
                },
                "allowed_tools": ["apply_patch"],
                "forbidden_tools": ["shell"],
                "allowed_mcp_tools": ["codebase.search_graph"],
                "tool_grants": [{
                    "stable_id": "apply_patch",
                    "schema_digest": "sha256:tool"
                }],
                "permission_policy": "ask",
                "model_route": {
                    "type": "explicit",
                    "route_id": "9router/anthropic/claude-sonnet-4.5"
                },
                "fallback_policy_id": "compatible",
                "fork_policy": "work_spec_only",
                "enforced_constraints": ["no-out-of-scope-edits"],
                "typed_constraints": [{
                    "id": "no-out-of-scope-edits",
                    "kind": "path_allow",
                    "description": "Only edit paths in scope"
                }],
                "verification_spec": {
                    "command": {
                        "argv": ["cargo", "test"],
                        "cwd": "D:/repo"
                    },
                    "protected_digests": {
                        "D:/repo/src/auth/generated": "sha256:protected"
                    },
                    "verification_policy_id": "focused-tests"
                },
                "expected_artifact": {
                    "required_status": "success",
                    "required_evidence_kinds": ["test_log"]
                }
            })
        );

        let decoded: WorkSpec = serde_json::from_value(value).expect("deserialize workspec");
        assert_eq!(decoded, work_spec);
        decoded.validate().expect("valid workspec");
    }

    #[test]
    fn phase1_workspec_uses_dedicated_schema_version() {
        let mut work_spec = sample_work_spec();
        work_spec.schema_version = CCCX_PROTOCOL_SCHEMA_VERSION.to_string();

        assert_eq!(
            work_spec.validate(),
            Err(ProtocolError::UnsupportedSchemaVersion(
                CCCX_PROTOCOL_SCHEMA_VERSION.to_string()
            ))
        );
    }

    #[test]
    fn phase1_rejects_role_harness_mismatch() {
        let mut work_spec = sample_work_spec();
        work_spec.harness = HarnessKind::Cc;
        work_spec.role = WorkRole::Implementer;

        assert_eq!(
            work_spec.validate(),
            Err(ProtocolError::RoleHarnessMismatch {
                harness: HarnessKind::Cc,
                role: WorkRole::Implementer,
            })
        );
    }

    #[test]
    fn phase1_rejects_duplicate_tool_grant_stable_id() {
        let mut work_spec = sample_work_spec();
        work_spec.tool_grants.push(ToolGrant {
            stable_id: "apply_patch".to_string(),
            server_id: Some("duplicate".to_string()),
            schema_digest: Some("sha256:other".to_string()),
        });

        assert_eq!(
            work_spec.validate(),
            Err(ProtocolError::DuplicateToolGrant("apply_patch".to_string()))
        );
    }

    #[test]
    fn phase1_runtime_identity_requires_stable_names() {
        let mut work_spec = sample_work_spec();
        work_spec.runtime_identity = Some(RuntimeIdentity {
            logical_session_id: "worker-auth".to_string(),
            worker_incarnation_id: "".to_string(),
            display_name: "a1-auth-fix".to_string(),
        });

        assert_eq!(
            work_spec.validate(),
            Err(ProtocolError::MissingField(
                "runtime_identity.worker_incarnation_id"
            ))
        );
    }

    #[test]
    fn artifact_validator_rejects_missing_evidence_or_provenance() {
        let mut artifact = sample_artifact();
        artifact.evidence.clear();
        assert_eq!(
            artifact.validate(),
            Err(ProtocolError::MissingField("evidence"))
        );

        let mut artifact = sample_artifact();
        artifact.provenance = None;
        assert_eq!(
            artifact.validate(),
            Err(ProtocolError::MissingField("provenance"))
        );
    }

    #[test]
    fn artifact_round_trip_keeps_verification_record() {
        let artifact = sample_artifact();
        artifact.validate().expect("valid artifact");

        let encoded = serde_json::to_string(&artifact).expect("serialize artifact");
        let decoded: Artifact = serde_json::from_str(&encoded).expect("deserialize artifact");

        assert_eq!(decoded, artifact);
        assert_eq!(
            decoded.verification.as_ref().map(|record| record.passed),
            Some(true)
        );
    }

    #[test]
    fn phase1_artifact_uses_dedicated_schema_version_and_validates_producer_role() {
        let mut artifact = sample_artifact();
        artifact.schema_version = CCCX_PROTOCOL_SCHEMA_VERSION.to_string();
        assert_eq!(
            artifact.validate(),
            Err(ProtocolError::UnsupportedSchemaVersion(
                CCCX_PROTOCOL_SCHEMA_VERSION.to_string()
            ))
        );

        let mut artifact = sample_artifact();
        artifact.provenance.as_mut().unwrap().producer_harness = HarnessKind::Cc;
        artifact.provenance.as_mut().unwrap().producer_role = WorkRole::Implementer;
        assert_eq!(
            artifact.validate(),
            Err(ProtocolError::RoleHarnessMismatch {
                harness: HarnessKind::Cc,
                role: WorkRole::Implementer,
            })
        );
    }

    #[test]
    fn child_config_is_deny_by_default_and_parent_broad_permission_does_not_leak() {
        let projection = project_child_config(&sample_work_spec(), &parent_capabilities())
            .expect("project child config");

        assert!(projection.deny_by_default);
        assert!(projection.can_use_tool("apply_patch"));
        assert!(!projection.can_use_tool("shell"));
        assert_eq!(projection.permission_policy, PermissionPolicy::Ask);
    }

    #[test]
    fn cx_cannot_use_mcp_outside_workspec_even_when_parent_has_it() {
        let projection = project_child_config(&sample_work_spec(), &parent_capabilities())
            .expect("project child config");

        assert!(projection.can_use_mcp_tool("codebase.search_graph"));
        assert!(!projection.can_use_mcp_tool("codebase.query_graph"));
    }

    #[test]
    fn protected_paths_cannot_be_edited() {
        let projection = project_child_config(&sample_work_spec(), &parent_capabilities())
            .expect("project child config");

        assert!(projection.can_write_path("D:/repo/src/auth/session.rs"));
        assert!(!projection.can_write_path("D:/repo/src/auth/generated/bindings.rs"));
        assert!(!projection.can_write_path("D:/repo/.git/config"));
        assert!(!projection.can_write_path("D:/repo/src/payments/mod.rs"));
    }

    #[test]
    fn full_history_fork_cannot_create_cx_worker() {
        let mut work_spec = sample_work_spec();
        work_spec.fork_policy = ForkPolicy::FullHistoryDebug;

        assert_eq!(
            project_child_config(&work_spec, &parent_capabilities()),
            Err(ProtocolError::FullHistoryCxWorker)
        );
    }

    #[test]
    fn explicit_model_route_must_be_allowed_by_parent() {
        let mut parent = parent_capabilities();
        parent.model_routes.clear();

        assert_eq!(
            project_child_config(&sample_work_spec(), &parent),
            Err(ProtocolError::ModelRouteNotAllowed(
                "9router/anthropic/claude-sonnet-4.5".to_string()
            ))
        );
    }

    #[test]
    fn inherited_model_route_does_not_require_child_specific_allowlist_entry() {
        let mut work_spec = sample_work_spec();
        work_spec.model_route = ModelRouteSelection::InheritMainEffective;
        let mut parent = parent_capabilities();
        parent.model_routes.clear();

        let projection = project_child_config(&work_spec, &parent)
            .expect("inherited model route is resolved by main agent runtime");

        assert_eq!(
            projection.model_route,
            ModelRouteSelection::InheritMainEffective
        );
    }

    #[test]
    fn blocked_artifact_may_carry_no_success_evidence_but_still_needs_provenance() {
        let mut artifact = sample_artifact();
        artifact.status = ArtifactStatus::Blocked;
        artifact.evidence.clear();

        artifact
            .validate()
            .expect("blocked artifact can report inability with provenance only");

        artifact.provenance = None;
        assert_eq!(
            artifact.validate(),
            Err(ProtocolError::MissingField("provenance"))
        );
    }

    // ── Phase 4: ActorIdentity scheduler boundary ──

    #[test]
    fn p4_main_cc_can_spawn() {
        assert!(ActorIdentity::MainCc.can_spawn());
    }

    #[test]
    fn p4_cc_subagent_cannot_spawn() {
        assert!(!ActorIdentity::CcSubagent.can_spawn());
    }

    #[test]
    fn p4_cx_subagent_cannot_spawn() {
        assert!(!ActorIdentity::CxSubagent.can_spawn());
    }

    #[test]
    fn p4_only_main_cc_has_spawn_right() {
        assert!(ActorIdentity::MainCc.can_spawn());
        assert!(!ActorIdentity::CcSubagent.can_spawn());
        assert!(!ActorIdentity::CxSubagent.can_spawn());
    }

    // ── Phase 5: provenance placeholder rejection ──

    /// Phase 5: Artifact::validate() must reject provenance with cccx-runtime placeholders.
    /// task.md Phase 5 exit criteria: "Provenance không còn placeholder sha256:cccx-runtime-*".
    #[test]
    fn p5_artifact_rejects_cccx_runtime_placeholder_in_provenance_digest() {
        // Placeholder in input_digest
        let mut artifact = sample_artifact();
        artifact.provenance.as_mut().unwrap().input_digest =
            "sha256:cccx-runtime-input-ws-x".to_string();
        assert_eq!(
            artifact.validate(),
            Err(ProtocolError::PlaceholderProvenance(
                "input_digest",
                "sha256:cccx-runtime-input-ws-x".to_string()
            ))
        );

        // Placeholder in output_digest
        let mut artifact = sample_artifact();
        artifact.provenance.as_mut().unwrap().output_digest =
            "sha256:cccx-runtime-output-ws-x".to_string();
        assert_eq!(
            artifact.validate(),
            Err(ProtocolError::PlaceholderProvenance(
                "output_digest",
                "sha256:cccx-runtime-output-ws-x".to_string()
            ))
        );

        // Placeholder in requested_route_id
        let mut artifact = sample_artifact();
        artifact.provenance.as_mut().unwrap().requested_route_id =
            "cccx-runtime-default".to_string();
        assert!(artifact.validate().is_err());

        // Placeholder in effective_route_id
        let mut artifact = sample_artifact();
        artifact.provenance.as_mut().unwrap().effective_route_id =
            "cccx-runtime-effective".to_string();
        assert!(artifact.validate().is_err());
    }

    /// Phase 5: ArtifactProvenance::verify_placeholders() passes clean provenance.
    #[test]
    fn p5_clean_provenance_passes_placeholder_check() {
        let artifact = sample_artifact();
        // sample_artifact uses sha256:input/sha256:output — no cccx-runtime
        assert!(artifact.validate().is_ok());
    }
}
