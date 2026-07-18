//! cccx permission policy core.
//!
//! This crate owns the deterministic permission decision contract. It is kept
//! independent from `codex-core` so the safety semantics can be validated
//! quickly before runtime wiring.

use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    Read,
    Edit,
    Shell,
    McpCall,
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MainMode {
    Plan,
    Default,
    AcceptEdits,
    Auto,
    DontAsk,
    Bypass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleTier {
    Allowlist,
    Escalate,
    CircuitBreaker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticRule {
    pub id: String,
    pub tier: RuleTier,
    pub action_type: ActionType,
    pub pattern: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedAction {
    pub action_type: ActionType,
    pub summary: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub target_paths: Vec<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    pub task_id: String,
    #[serde(default)]
    pub changed_files: usize,
    #[serde(default)]
    pub changed_lines: usize,
}

impl NormalizedAction {
    pub fn digest(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.action_type.hash(&mut hasher);
        self.summary.hash(&mut hasher);
        self.command.hash(&mut hasher);
        self.target_paths.hash(&mut hasher);
        self.tool_name.hash(&mut hasher);
        self.task_id.hash(&mut hasher);
        self.changed_files.hash(&mut hasher);
        self.changed_lines.hash(&mut hasher);
        format!("cccx-m4-{:016x}", hasher.finish())
    }
}

impl Hash for ActionType {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeThresholds {
    pub max_changed_files: usize,
    pub max_changed_lines: usize,
}

impl ModeThresholds {
    pub fn exceeded_by(&self, action: &NormalizedAction) -> bool {
        action.changed_files > self.max_changed_files
            || action.changed_lines > self.max_changed_lines
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicySession {
    pub mode: MainMode,
    pub thresholds: ModeThresholds,
    #[serde(default)]
    pub explicit_allow_rules: Vec<StaticRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicySet {
    #[serde(default)]
    pub rules: Vec<StaticRule>,
}

impl PolicySet {
    pub fn default_m4() -> Self {
        Self {
            rules: vec![
                StaticRule::contains(
                    "cb-rm-rf-root",
                    RuleTier::CircuitBreaker,
                    ActionType::Shell,
                    "rm -rf /",
                    "refuse recursive deletion of filesystem root",
                ),
                StaticRule::contains(
                    "cb-format",
                    RuleTier::CircuitBreaker,
                    ActionType::Shell,
                    "format ",
                    "refuse filesystem format/wipe command",
                ),
                StaticRule::contains(
                    "cb-git-force-main",
                    RuleTier::CircuitBreaker,
                    ActionType::Shell,
                    "git push",
                    "refuse unreviewed force push to protected branch",
                ),
                StaticRule::contains(
                    "secret-path",
                    RuleTier::Escalate,
                    ActionType::Edit,
                    ".env",
                    "secret-like path requires approval",
                ),
                StaticRule::contains(
                    "secret-pem",
                    RuleTier::Escalate,
                    ActionType::Edit,
                    ".pem",
                    "credential file requires approval",
                ),
                StaticRule::contains(
                    "secret-key",
                    RuleTier::Escalate,
                    ActionType::Edit,
                    ".key",
                    "credential file requires approval",
                ),
                StaticRule::contains(
                    "secret-id-rsa",
                    RuleTier::Escalate,
                    ActionType::Edit,
                    "id_rsa",
                    "credential file requires approval",
                ),
                StaticRule::contains(
                    "secret-credentials",
                    RuleTier::Escalate,
                    ActionType::Edit,
                    "credentials",
                    "credential file requires approval",
                ),
                StaticRule::contains(
                    "secret-config-secrets",
                    RuleTier::Escalate,
                    ActionType::Edit,
                    "secrets",
                    "credential-like path requires approval",
                ),
                StaticRule::contains(
                    "mcp-side-effect",
                    RuleTier::Escalate,
                    ActionType::McpCall,
                    "manage_adr",
                    "potential side-effect MCP tool requires approval",
                ),
                StaticRule::contains(
                    "safe-read",
                    RuleTier::Allowlist,
                    ActionType::Read,
                    "",
                    "read-only action",
                ),
            ],
        }
    }
}

impl StaticRule {
    pub fn contains(
        id: impl Into<String>,
        tier: RuleTier,
        action_type: ActionType,
        pattern: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            tier,
            action_type,
            pattern: pattern.into(),
            reason: reason.into(),
        }
    }

    fn matches(&self, action: &NormalizedAction) -> bool {
        if self.action_type != action.action_type {
            return false;
        }

        // --- cb-git-force-main: force push to protected branches ---
        if self.id == "cb-git-force-main" {
            let Some(cmd) = action.command.as_deref() else {
                return false;
            };
            let cmd = cmd.to_ascii_lowercase();
            if !cmd.contains("git push") {
                return false;
            }
            let has_force =
                cmd.contains("--force") || cmd.contains("-f") || cmd.contains("--force-with-lease");
            if !has_force {
                return false;
            }
            let targets_protected = cmd.contains(" origin main")
                || cmd.contains(" origin master")
                || cmd.contains(" origin prod")
                || cmd.contains(" origin production")
                || cmd.contains("--mirror");
            return targets_protected;
        }

        // --- cb-rm-rf-root: destructive recursive deletion ---
        if self.id == "cb-rm-rf-root" {
            let Some(cmd) = action.command.as_deref() else {
                return false;
            };
            let cmd = cmd.to_ascii_lowercase();
            if !cmd.contains("rm") {
                return false;
            }
            let destructive_flags = cmd.contains("-rf")
                || cmd.contains("-r -f")
                || cmd.contains("-fr")
                || cmd.contains("--recursive")
                || cmd.contains("--force");
            if !destructive_flags {
                return false;
            }
            let dangerous_targets = [
                " /",
                " /*",
                " ~",
                " /home",
                "$home",
                "\"c:\\",
                "c:\\windows",
            ];
            return dangerous_targets.iter().any(|t| cmd.contains(t));
        }

        // --- cb-format: disk format / wipe ---
        if self.id == "cb-format" {
            let Some(cmd) = action.command.as_deref() else {
                return false;
            };
            let cmd = cmd.to_ascii_lowercase();
            if cmd.contains("format ")
                || cmd.contains("mkfs")
                || (cmd.contains("dd if=") && cmd.contains("of="))
                || cmd.contains("diskpart")
                || (cmd.contains("wmic") && cmd.contains("diskdrive") && cmd.contains("clean"))
            {
                return true;
            }
            return false;
        }

        // --- generic contains-based matching ---
        let pattern = self.pattern.to_ascii_lowercase();
        if pattern.is_empty() {
            return true;
        }
        action
            .command
            .iter()
            .chain(action.target_paths.iter())
            .chain(action.tool_name.iter())
            .chain(std::iter::once(&action.summary))
            .any(|value| value.to_ascii_lowercase().contains(&pattern))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClassifierOutcome {
    Allow,
    Deny { reason: String },
    RequireApproval { reason: String },
    RerouteToCx { reason: String },
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyDecision {
    Allow {
        tier: String,
    },
    Deny {
        reason: String,
        bypassable: bool,
        silent: bool,
    },
    RequireApproval {
        reason: String,
        digest: String,
    },
    RerouteToCx {
        reason: String,
    },
}

pub fn evaluate_action(
    action: &NormalizedAction,
    session: &PolicySession,
    policies: &PolicySet,
    classifier: Option<ClassifierOutcome>,
) -> PolicyDecision {
    if let Some(rule) = first_match(policies, action, RuleTier::CircuitBreaker) {
        return PolicyDecision::Deny {
            reason: rule.reason.clone(),
            bypassable: false,
            silent: false,
        };
    }

    if matches!(session.mode, MainMode::Bypass) {
        return PolicyDecision::Allow {
            tier: "bypass".to_string(),
        };
    }

    if let Some(rule) = first_match(policies, action, RuleTier::Escalate) {
        return match session.mode {
            MainMode::DontAsk => PolicyDecision::Deny {
                reason: rule.reason.clone(),
                bypassable: true,
                silent: true,
            },
            MainMode::Plan => PolicyDecision::Deny {
                reason: "plan-read-only".to_string(),
                bypassable: true,
                silent: false,
            },
            _ => PolicyDecision::RequireApproval {
                reason: rule.reason.clone(),
                digest: action.digest(),
            },
        };
    }

    if first_match(policies, action, RuleTier::Allowlist).is_some() {
        return PolicyDecision::Allow {
            tier: "static_allowlist".to_string(),
        };
    }

    if session.thresholds.exceeded_by(action) {
        return match session.mode {
            MainMode::AcceptEdits => PolicyDecision::RerouteToCx {
                reason: "mode-threshold-exceeded".to_string(),
            },
            MainMode::Auto => classify_or_escalate(action, classifier),
            MainMode::DontAsk => PolicyDecision::Deny {
                reason: "mode-threshold-exceeded".to_string(),
                bypassable: true,
                silent: true,
            },
            _ => PolicyDecision::RequireApproval {
                reason: "mode-threshold-exceeded".to_string(),
                digest: action.digest(),
            },
        };
    }

    match session.mode {
        MainMode::Plan => PolicyDecision::Deny {
            reason: "plan-mode".to_string(),
            bypassable: true,
            silent: false,
        },
        MainMode::Default => PolicyDecision::RequireApproval {
            reason: "default-mode".to_string(),
            digest: action.digest(),
        },
        MainMode::AcceptEdits => {
            if matches!(action.action_type, ActionType::Edit) {
                PolicyDecision::Allow {
                    tier: "accept_edits".to_string(),
                }
            } else {
                PolicyDecision::RequireApproval {
                    reason: "accept-edits-only-covers-edit".to_string(),
                    digest: action.digest(),
                }
            }
        }
        MainMode::Auto => classify_or_escalate(action, classifier),
        MainMode::DontAsk => {
            if session
                .explicit_allow_rules
                .iter()
                .any(|rule| rule.matches(action))
            {
                PolicyDecision::Allow {
                    tier: "dont_ask_explicit_allow".to_string(),
                }
            } else {
                PolicyDecision::Deny {
                    reason: "dont-ask-no-explicit-allow".to_string(),
                    bypassable: true,
                    silent: true,
                }
            }
        }
        MainMode::Bypass => unreachable!("bypass handled before static rules"),
    }
}

fn classify_or_escalate(
    action: &NormalizedAction,
    classifier: Option<ClassifierOutcome>,
) -> PolicyDecision {
    match classifier.unwrap_or(ClassifierOutcome::Unavailable) {
        ClassifierOutcome::Allow => PolicyDecision::Allow {
            tier: "classifier".to_string(),
        },
        ClassifierOutcome::Deny { reason } => PolicyDecision::Deny {
            reason,
            bypassable: true,
            silent: false,
        },
        ClassifierOutcome::RequireApproval { reason } => PolicyDecision::RequireApproval {
            reason,
            digest: action.digest(),
        },
        ClassifierOutcome::RerouteToCx { reason } => PolicyDecision::RerouteToCx { reason },
        ClassifierOutcome::Unavailable => PolicyDecision::RequireApproval {
            reason: "classifier_unavailable".to_string(),
            digest: action.digest(),
        },
    }
}

pub fn evaluate_subagent(
    action: &NormalizedAction,
    work_spec: &cccx_protocol::WorkSpec,
    policies: &PolicySet,
) -> PolicyDecision {
    if let Some(rule) = first_match(policies, action, RuleTier::CircuitBreaker) {
        return PolicyDecision::Deny {
            reason: rule.reason.clone(),
            bypassable: false,
            silent: false,
        };
    }

    if let Some(ref tool_name) = action.tool_name {
        let is_mcp = matches!(action.action_type, ActionType::McpCall);
        let allowed = if is_mcp {
            work_spec.allowed_mcp_tools.contains(tool_name)
        } else {
            work_spec.allowed_tools.contains(tool_name)
        };
        if !allowed {
            return PolicyDecision::Deny {
                reason: format!("tool_not_allowed: {}", tool_name),
                bypassable: false,
                silent: false,
            };
        }
    }

    if let Some(ref tool_name) = action.tool_name {
        if work_spec.forbidden_tools.contains(tool_name) {
            return PolicyDecision::Deny {
                reason: format!("tool_forbidden: {}", tool_name),
                bypassable: false,
                silent: false,
            };
        }
    }

    if matches!(action.action_type, ActionType::Edit) && !action.target_paths.is_empty() {
        let in_scope = action.target_paths.iter().any(|target| {
            let normalized = target.replace('\\', "/");
            work_spec
                .scope
                .canonical_paths
                .iter()
                .any(|scope_path| normalized.starts_with(&scope_path.replace('\\', "/")))
        });
        if !in_scope {
            return PolicyDecision::Deny {
                reason: "out_of_scope".to_string(),
                bypassable: false,
                silent: false,
            };
        }
    }

    for constraint in &work_spec.enforced_constraints {
        if violates_subagent_constraint(action, constraint) {
            return PolicyDecision::Deny {
                reason: format!("constraint_violation: {}", constraint),
                bypassable: false,
                silent: false,
            };
        }
    }

    if let Some(ref threshold) = work_spec.risk_threshold {
        if action.changed_files > threshold.max_files_changed {
            return escalate_subagent(action, &work_spec.approval_policy, "max_files_exceeded");
        }
        if action.changed_lines > threshold.max_lines_changed {
            return escalate_subagent(action, &work_spec.approval_policy, "max_lines_exceeded");
        }
        for pattern in &threshold.forbidden_patterns {
            if action.target_paths.iter().any(|p| {
                p.to_ascii_lowercase()
                    .contains(&pattern.to_ascii_lowercase())
            }) {
                return escalate_subagent(action, &work_spec.approval_policy, "forbidden_pattern");
            }
        }
    }

    if let Some(ref action_classes) = work_spec.action_classes {
        let at = action_type_str(action);
        if action_classes.iter().any(|ac| ac.action_type == at) {
            return escalate_subagent(
                action,
                &work_spec.approval_policy,
                "action_class_requires_approval",
            );
        }
    }

    match work_spec.approval_policy {
        Some(cccx_protocol::ApprovalPolicyKind::AutoDeny) => PolicyDecision::Deny {
            reason: "auto_deny_policy".to_string(),
            bypassable: true,
            silent: false,
        },
        Some(cccx_protocol::ApprovalPolicyKind::AskCc) => PolicyDecision::RequireApproval {
            reason: "subagent-require-cc-approval".to_string(),
            digest: action.digest(),
        },
        Some(cccx_protocol::ApprovalPolicyKind::AskUser) | None => {
            PolicyDecision::RequireApproval {
                reason: "subagent-require-approval".to_string(),
                digest: action.digest(),
            }
        }
    }
}

fn escalate_subagent(
    action: &NormalizedAction,
    policy: &Option<cccx_protocol::ApprovalPolicyKind>,
    reason: &str,
) -> PolicyDecision {
    match policy {
        Some(cccx_protocol::ApprovalPolicyKind::AutoDeny) => PolicyDecision::Deny {
            reason: reason.to_string(),
            bypassable: true,
            silent: false,
        },
        _ => PolicyDecision::RequireApproval {
            reason: reason.to_string(),
            digest: action.digest(),
        },
    }
}

fn violates_subagent_constraint(action: &NormalizedAction, constraint: &str) -> bool {
    if constraint == "no-out-of-scope-edits" {
        return false;
    }
    let pattern = constraint.to_ascii_lowercase();
    action
        .command
        .iter()
        .chain(action.tool_name.iter())
        .chain(action.target_paths.iter())
        .any(|value| value.to_ascii_lowercase().contains(&pattern))
}

fn action_type_str(action: &NormalizedAction) -> &str {
    match action.action_type {
        ActionType::Read => "read",
        ActionType::Edit => "edit",
        ActionType::Shell => "shell",
        ActionType::McpCall => "mcp_call",
        ActionType::Network => "network",
    }
}

fn first_match<'a>(
    policies: &'a PolicySet,
    action: &NormalizedAction,
    tier: RuleTier,
) -> Option<&'a StaticRule> {
    policies
        .rules
        .iter()
        .filter(|rule| rule.tier == tier)
        .find(|rule| rule.matches(action))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionRisk {
    Low,
    Medium,
    High,
    CircuitBreaker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionRequestStatus {
    Queued,
    Active,
    Approved,
    Denied,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub request_id: String,
    pub agent_id: String,
    pub action: NormalizedAction,
    pub digest: String,
    pub risk: PermissionRisk,
    pub status: PermissionRequestStatus,
}

impl PermissionRequest {
    pub fn new(
        request_id: impl Into<String>,
        agent_id: impl Into<String>,
        action: NormalizedAction,
        risk: PermissionRisk,
    ) -> Self {
        let digest = action.digest();
        Self {
            request_id: request_id.into(),
            agent_id: agent_id.into(),
            action,
            digest,
            risk,
            status: PermissionRequestStatus::Queued,
        }
    }
}

#[derive(Debug, Default)]
pub struct PermissionRequestQueue {
    requests: BTreeMap<String, PermissionRequest>,
    active_request_id: Option<String>,
    queue_order: Vec<String>,
}

impl PermissionRequestQueue {
    pub fn enqueue(&mut self, mut request: PermissionRequest) {
        if self.active_request_id.is_none() {
            request.status = PermissionRequestStatus::Active;
            self.active_request_id = Some(request.request_id.clone());
        } else {
            request.status = PermissionRequestStatus::Queued;
            self.queue_order.push(request.request_id.clone());
        }
        self.requests.insert(request.request_id.clone(), request);
    }

    pub fn active_request_id(&self) -> Option<&str> {
        self.active_request_id.as_deref()
    }

    pub fn approve(&mut self, request_id: &str, observed_digest: &str) -> Result<(), QueueError> {
        let request = self
            .requests
            .get_mut(request_id)
            .ok_or_else(|| QueueError::UnknownRequest(request_id.to_string()))?;

        if !matches!(request.status, PermissionRequestStatus::Active) {
            return Err(QueueError::NotActive(request_id.to_string()));
        }
        if request.digest != observed_digest {
            request.status = PermissionRequestStatus::Expired;
            self.promote_next();
            return Err(QueueError::DigestMismatch);
        }
        if matches!(request.risk, PermissionRisk::CircuitBreaker) {
            request.status = PermissionRequestStatus::Denied;
            self.promote_next();
            return Err(QueueError::CircuitBreakerCannotBeApproved);
        }

        request.status = PermissionRequestStatus::Approved;
        self.promote_next();
        Ok(())
    }

    pub fn cancel_agent_requests(&mut self, agent_id: &str) {
        let affected = self
            .requests
            .values_mut()
            .filter(|request| request.agent_id == agent_id)
            .map(|request| {
                request.status = PermissionRequestStatus::Cancelled;
                request.request_id.clone()
            })
            .collect::<BTreeSet<_>>();

        self.queue_order
            .retain(|request_id| !affected.contains(request_id));
        if self
            .active_request_id
            .as_ref()
            .is_some_and(|request_id| affected.contains(request_id))
        {
            self.active_request_id = None;
            self.promote_next();
        }
    }

    pub fn get(&self, request_id: &str) -> Option<&PermissionRequest> {
        self.requests.get(request_id)
    }

    fn promote_next(&mut self) {
        if let Some(active_id) = self.active_request_id.take()
            && let Some(active) = self.requests.get(&active_id)
            && matches!(active.status, PermissionRequestStatus::Active)
        {
            self.active_request_id = Some(active_id);
            return;
        }

        while let Some(next_id) = self.queue_order.first().cloned() {
            self.queue_order.remove(0);
            if let Some(next) = self.requests.get_mut(&next_id)
                && matches!(next.status, PermissionRequestStatus::Queued)
            {
                next.status = PermissionRequestStatus::Active;
                self.active_request_id = Some(next_id);
                return;
            }
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum QueueError {
    #[error("unknown permission request `{0}`")]
    UnknownRequest(String),
    #[error("permission request `{0}` is not active")]
    NotActive(String),
    #[error("permission request digest mismatch")]
    DigestMismatch,
    #[error("circuit breaker request cannot be approved")]
    CircuitBreakerCannotBeApproved,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn session(mode: MainMode) -> PolicySession {
        PolicySession {
            mode,
            thresholds: ModeThresholds {
                max_changed_files: 1,
                max_changed_lines: 20,
            },
            explicit_allow_rules: Vec::new(),
        }
    }

    fn read_env_action() -> NormalizedAction {
        NormalizedAction {
            action_type: ActionType::Read,
            summary: "read env file".to_string(),
            command: None,
            target_paths: vec!["D:/repo/.env".to_string()],
            tool_name: Some("read_file".to_string()),
            task_id: "T1".to_string(),
            changed_files: 0,
            changed_lines: 0,
        }
    }

    fn edit_action() -> NormalizedAction {
        NormalizedAction {
            action_type: ActionType::Edit,
            summary: "edit source".to_string(),
            command: None,
            target_paths: vec!["D:/repo/src/lib.rs".to_string()],
            tool_name: Some("apply_patch".to_string()),
            task_id: "T1".to_string(),
            changed_files: 1,
            changed_lines: 8,
        }
    }

    fn dangerous_shell(command: &str) -> NormalizedAction {
        NormalizedAction {
            action_type: ActionType::Shell,
            summary: command.to_string(),
            command: Some(command.to_string()),
            target_paths: Vec::new(),
            tool_name: Some("shell".to_string()),
            task_id: "T1".to_string(),
            changed_files: 0,
            changed_lines: 0,
        }
    }

    fn edit_env_action() -> NormalizedAction {
        NormalizedAction {
            action_type: ActionType::Edit,
            summary: "edit env file".to_string(),
            command: None,
            target_paths: vec!["D:/repo/.env".to_string()],
            tool_name: Some("apply_patch".to_string()),
            task_id: "T1".to_string(),
            changed_files: 1,
            changed_lines: 1,
        }
    }

    fn set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn cx_work_spec() -> cccx_protocol::WorkSpec {
        cccx_protocol::WorkSpec {
            schema_version: cccx_protocol::CCCX_PROTOCOL_SCHEMA_VERSION.to_string(),
            work_spec_id: "ws-1".to_string(),
            task_id: "T1".to_string(),
            harness: cccx_protocol::HarnessKind::Cx,
            role: cccx_protocol::WorkRole::Implementer,
            objective: "bounded implementation".to_string(),
            scope: cccx_protocol::WorkScope {
                canonical_paths: set(&["D:/repo/src/auth"]),
                protected_paths: BTreeSet::new(),
            },
            allowed_tools: set(&["apply_patch"]),
            forbidden_tools: set(&["spawn_agent"]),
            allowed_mcp_tools: set(&["mcp:codebase-memory-mcp.search_graph"]),
            permission_policy: cccx_protocol::PermissionPolicy::Ask,
            model_route: cccx_protocol::ModelRouteSelection::InheritMainRequested,
            fallback_policy_id: "compatible".to_string(),
            fork_policy: cccx_protocol::ForkPolicy::WorkSpecOnly,
            enforced_constraints: vec!["no-out-of-scope-edits".to_string()],
            risk_threshold: None,
            action_classes: None,
            approval_policy: Some(cccx_protocol::ApprovalPolicyKind::AskCc),
        }
    }

    #[test]
    fn static_deny_beats_static_allow() {
        let decision = evaluate_action(
            &edit_env_action(),
            &session(MainMode::Default),
            &PolicySet::default_m4(),
            None,
        );

        assert!(matches!(
            decision,
            PolicyDecision::RequireApproval { reason, .. }
                if reason == "secret-like path requires approval"
        ));
    }

    #[test]
    fn circuit_breaker_denies_even_in_bypass() {
        let decision = evaluate_action(
            &dangerous_shell("rm -rf /"),
            &session(MainMode::Bypass),
            &PolicySet::default_m4(),
            None,
        );

        assert_eq!(
            decision,
            PolicyDecision::Deny {
                reason: "refuse recursive deletion of filesystem root".to_string(),
                bypassable: false,
                silent: false
            }
        );
    }

    #[test]
    fn bypass_skips_non_circuit_static_escalate() {
        let decision = evaluate_action(
            &read_env_action(),
            &session(MainMode::Bypass),
            &PolicySet::default_m4(),
            None,
        );

        assert_eq!(
            decision,
            PolicyDecision::Allow {
                tier: "bypass".to_string()
            }
        );
    }

    #[test]
    fn accept_edits_reroutes_when_threshold_exceeded() {
        let mut action = edit_action();
        action.changed_lines = 100;

        let decision = evaluate_action(
            &action,
            &session(MainMode::AcceptEdits),
            &PolicySet::default_m4(),
            None,
        );

        assert_eq!(
            decision,
            PolicyDecision::RerouteToCx {
                reason: "mode-threshold-exceeded".to_string()
            }
        );
    }

    #[test]
    fn classifier_failure_requires_approval_in_auto() {
        let decision = evaluate_action(
            &edit_action(),
            &session(MainMode::Auto),
            &PolicySet::default_m4(),
            Some(ClassifierOutcome::Unavailable),
        );

        assert!(matches!(
            decision,
            PolicyDecision::RequireApproval { reason, .. }
                if reason == "classifier_unavailable"
        ));
    }

    #[test]
    fn git_force_push_to_protected_branch_is_circuit_breaker() {
        let decision = evaluate_action(
            &dangerous_shell("git push origin main --force"),
            &session(MainMode::Default),
            &PolicySet::default_m4(),
            None,
        );

        assert!(matches!(
            decision,
            PolicyDecision::Deny {
                bypassable: false,
                ..
            }
        ));
    }

    #[test]
    fn permission_queue_has_single_active_prompt() {
        let mut queue = PermissionRequestQueue::default();
        let first = PermissionRequest::new("pr-1", "a1", edit_action(), PermissionRisk::Medium);
        let second = PermissionRequest::new("pr-2", "a2", edit_action(), PermissionRisk::Medium);

        queue.enqueue(first);
        queue.enqueue(second);

        assert_eq!(queue.active_request_id(), Some("pr-1"));
        assert_eq!(
            queue.get("pr-2").map(|request| &request.status),
            Some(&PermissionRequestStatus::Queued)
        );
    }

    #[test]
    fn stale_permission_digest_cannot_be_approved_and_promotes_next() {
        let mut queue = PermissionRequestQueue::default();
        queue.enqueue(PermissionRequest::new(
            "pr-1",
            "a1",
            edit_action(),
            PermissionRisk::Medium,
        ));
        queue.enqueue(PermissionRequest::new(
            "pr-2",
            "a2",
            edit_action(),
            PermissionRisk::Medium,
        ));

        assert_eq!(
            queue.approve("pr-1", "wrong-digest"),
            Err(QueueError::DigestMismatch)
        );

        assert_eq!(
            queue.get("pr-1").map(|request| &request.status),
            Some(&PermissionRequestStatus::Expired)
        );
        assert_eq!(queue.active_request_id(), Some("pr-2"));
    }

    #[test]
    fn cancelled_agent_permission_cannot_be_approved() {
        let mut queue = PermissionRequestQueue::default();
        let request = PermissionRequest::new("pr-1", "a1", edit_action(), PermissionRisk::Medium);
        let digest = request.digest.clone();
        queue.enqueue(request);
        queue.cancel_agent_requests("a1");

        assert_eq!(
            queue.approve("pr-1", &digest),
            Err(QueueError::NotActive("pr-1".to_string()))
        );
        assert_eq!(
            queue.get("pr-1").map(|request| &request.status),
            Some(&PermissionRequestStatus::Cancelled)
        );
    }

    #[test]
    fn subagent_scope_check_requires_real_path_boundary() {
        let mut action = edit_action();
        action.target_paths = vec!["D:/repo/src/authentication/session.rs".to_string()];

        let decision = evaluate_subagent(&action, &cx_work_spec(), &PolicySet::default_m4());

        assert_eq!(
            decision,
            PolicyDecision::Deny {
                reason: "out_of_scope".to_string(),
                bypassable: false,
                silent: false
            }
        );
    }

    #[test]
    fn main_bypass_does_not_expand_subagent_tools() {
        let action = NormalizedAction {
            action_type: ActionType::McpCall,
            summary: "commit ADR".to_string(),
            command: None,
            target_paths: Vec::new(),
            tool_name: Some("mcp:codebase-memory-mcp.manage_adr".to_string()),
            task_id: "T1".to_string(),
            changed_files: 0,
            changed_lines: 0,
        };

        let decision = evaluate_subagent(&action, &cx_work_spec(), &PolicySet::default_m4());

        assert!(matches!(
            decision,
            PolicyDecision::Deny { reason, bypassable: false, .. }
                if reason == "tool_not_allowed: mcp:codebase-memory-mcp.manage_adr"
        ));
    }
}
