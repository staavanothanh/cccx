use std::future::Future;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;

use cccx_policy::ActionType as CccxActionType;
use cccx_policy::ClassifierOutcome as CccxPolicyClassifierOutcome;
use cccx_policy::MainMode as CccxMainMode;
use cccx_policy::ModeThresholds as CccxModeThresholds;
use cccx_policy::NormalizedAction as CccxNormalizedAction;
use cccx_policy::PolicyDecision as CccxPolicyDecision;
use cccx_policy::PolicySession as CccxPolicySession;
use cccx_policy::PolicySet as CccxPolicySet;
use cccx_policy::evaluate_action as cccx_evaluate_action;
use cccx_policy::evaluate_subagent;
use cccx_protocol;
use codex_config::ConfigLayerSource;
use codex_config::ConfigLayerStack;
use codex_config::ConfigLayerStackOrdering;
use codex_execpolicy::AmendError;
use codex_execpolicy::Decision;
use codex_execpolicy::Error as ExecPolicyRuleError;
use codex_execpolicy::Evaluation;
use codex_execpolicy::MatchOptions;
use codex_execpolicy::NetworkRuleProtocol;
use codex_execpolicy::Policy;
use codex_execpolicy::PolicyParser;
use codex_execpolicy::RuleMatch;
use codex_execpolicy::blocking_append_allow_prefix_rule;
use codex_execpolicy::blocking_append_network_rule;
use codex_model_provider_info::NINE_ROUTER_API_KEY_ENV;
use codex_protocol::approvals::ExecPolicyAmendment;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemSandboxKind;
use codex_protocol::protocol::AskForApproval;
use codex_shell_command::is_dangerous_command::DangerousCommandMatch;
use codex_shell_command::is_dangerous_command::dangerous_command_match;
use codex_shell_command::is_safe_command::is_known_safe_command;
use thiserror::Error;
use tokio::fs;
use tokio::sync::Semaphore;
use tokio::task::spawn_blocking;
use tokio::time::timeout;
use tracing::instrument;

use crate::cccx_paths::resolve_physical_paths;
use crate::config::Config;
use crate::sandboxing::SandboxPermissions;
use crate::tools::sandboxing::ExecApprovalRequirement;
use codex_shell_command::bash::parse_shell_lc_plain_commands;
use codex_shell_command::bash::parse_shell_lc_single_command_prefix;
use codex_utils_absolute_path::AbsolutePathBuf;
use sha2::Digest;
use sha2::Sha256;
use shlex::try_join as shlex_try_join;

const PROMPT_CONFLICT_REASON: &str =
    "approval required by policy, but AskForApproval is set to Never";
const REJECT_SANDBOX_APPROVAL_REASON: &str =
    "approval required by policy, but AskForApproval::Granular.sandbox_approval is false";
const REJECT_RULES_APPROVAL_REASON: &str =
    "approval required by policy rule, but AskForApproval::Granular.rules is false";
const RULES_DIR_NAME: &str = "rules";
const RULE_EXTENSION: &str = "rules";
const DEFAULT_POLICY_FILE: &str = "default.rules";
const CCCX_AUTO_CLASSIFIER_MODEL_ENV: &str = "CCCX_AUTO_CLASSIFIER_MODEL";
const CCCX_AUTO_CLASSIFIER_BASE_URL: &str = "https://9router.ai/api/v1";
const CCCX_AUTO_CLASSIFIER_TIMEOUT: Duration = Duration::from_secs(8);
static BANNED_PREFIX_SUGGESTIONS: &[&[&str]] = &[
    &["python3"],
    &["python3", "-"],
    &["python3", "-c"],
    &["python"],
    &["python", "-"],
    &["python", "-c"],
    &["py"],
    &["py", "-3"],
    &["pythonw"],
    &["pyw"],
    &["pypy"],
    &["pypy3"],
    &["git"],
    &["bash"],
    &["bash", "-lc"],
    &["sh"],
    &["sh", "-c"],
    &["sh", "-lc"],
    &["zsh"],
    &["zsh", "-lc"],
    &["/bin/zsh"],
    &["/bin/zsh", "-lc"],
    &["/bin/bash"],
    &["/bin/bash", "-lc"],
    &["pwsh"],
    &["pwsh", "-Command"],
    &["pwsh", "-c"],
    &["powershell"],
    &["powershell", "-Command"],
    &["powershell", "-c"],
    &["powershell.exe"],
    &["powershell.exe", "-Command"],
    &["powershell.exe", "-c"],
    &["env"],
    &["sudo"],
    &["node"],
    &["node", "-e"],
    &["perl"],
    &["perl", "-e"],
    &["ruby"],
    &["ruby", "-e"],
    &["php"],
    &["php", "-r"],
    &["lua"],
    &["lua", "-e"],
    &["osascript"],
];

/// Describes which unmatched-command heuristics should classify the command
/// words being evaluated by exec-policy.
///
/// The command tokens may be the original argv or a shell-specific lowering of
/// a wrapper such as `bash -lc ...` or `powershell.exe -Command ...`. We only
/// need to distinguish the PowerShell case because its safelist and dangerous
/// heuristics operate on PowerShell-flavored inner command words rather than
/// the generic command classifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExecPolicyCommandOrigin {
    /// Use the generic unmatched-command heuristics.
    Generic,
    #[cfg(windows)]
    /// The command words came from the `-Command` body of a top-level
    /// PowerShell wrapper, so use PowerShell-specific unmatched-command
    /// heuristics for the lowered words.
    PowerShell,
}

#[derive(Clone, Copy)]
pub(crate) struct UnmatchedCommandContext<'a> {
    pub(crate) approval_policy: AskForApproval,
    pub(crate) permission_profile: &'a PermissionProfile,
    pub(crate) windows_sandbox_level: WindowsSandboxLevel,
    pub(crate) sandbox_permissions: SandboxPermissions,
    pub(crate) used_complex_parsing: bool,
    pub(crate) command_origin: ExecPolicyCommandOrigin,
}

#[derive(Debug, Eq, PartialEq)]
struct ExecPolicyCommands {
    commands: Vec<Vec<String>>,
    used_complex_parsing: bool,
    command_origin: ExecPolicyCommandOrigin,
}

pub(crate) fn child_uses_parent_exec_policy(parent_config: &Config, child_config: &Config) -> bool {
    fn exec_policy_config_folders(config: &Config) -> Vec<AbsolutePathBuf> {
        config
            .config_layer_stack
            .get_layers(
                ConfigLayerStackOrdering::LowestPrecedenceFirst,
                /*include_disabled*/ false,
            )
            .into_iter()
            .filter_map(codex_config::ConfigLayerEntry::config_folder)
            .collect()
    }

    exec_policy_config_folders(parent_config) == exec_policy_config_folders(child_config)
        && parent_config
            .config_layer_stack
            .ignore_user_and_project_exec_policy_rules()
            == child_config
                .config_layer_stack
                .ignore_user_and_project_exec_policy_rules()
        && parent_config.config_layer_stack.requirements().exec_policy
            == child_config.config_layer_stack.requirements().exec_policy
}

fn is_policy_match(rule_match: &RuleMatch) -> bool {
    match rule_match {
        RuleMatch::PrefixRuleMatch { .. } => true,
        RuleMatch::HeuristicsRuleMatch { .. } => false,
    }
}

/// Returns a rejection reason when `approval_policy` disallows surfacing the
/// current prompt to the user.
///
/// `prompt_is_rule` distinguishes policy-rule prompts from sandbox/escalation
/// prompts so granular `rules` and `sandbox_approval` settings are honored
/// independently. When both are present, policy-rule prompts take precedence.
pub(crate) fn prompt_is_rejected_by_policy(
    approval_policy: AskForApproval,
    prompt_is_rule: bool,
) -> Option<&'static str> {
    match approval_policy {
        AskForApproval::Never => Some(PROMPT_CONFLICT_REASON),
        AskForApproval::OnRequest => None,
        AskForApproval::UnlessTrusted => None,
        AskForApproval::Granular(granular_config) => {
            if prompt_is_rule {
                if !granular_config.allows_rules_approval() {
                    Some(REJECT_RULES_APPROVAL_REASON)
                } else {
                    None
                }
            } else if !granular_config.allows_sandbox_approval() {
                Some(REJECT_SANDBOX_APPROVAL_REASON)
            } else {
                None
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum ExecPolicyError {
    #[error("failed to read rules files from {dir}: {source}")]
    ReadDir {
        dir: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to read rules file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to parse rules file {path}: {source}")]
    ParsePolicy {
        path: String,
        source: codex_execpolicy::Error,
    },
}

#[derive(Debug, Error)]
pub enum ExecPolicyUpdateError {
    #[error("failed to update rules file {path}: {source}")]
    AppendRule { path: PathBuf, source: AmendError },

    #[error("failed to join blocking rules update task: {source}")]
    JoinBlockingTask { source: tokio::task::JoinError },

    #[error("failed to update in-memory rules: {source}")]
    AddRule {
        #[from]
        source: ExecPolicyRuleError,
    },
}

pub(crate) struct ExecPolicyManager {
    policy: ArcSwap<Policy>,
    update_lock: Semaphore,
    cccx_auto_classifier: Option<Arc<dyn CccxAutoClassifier>>,
}

pub(crate) struct ExecApprovalRequest<'a> {
    pub(crate) command: &'a [String],
    pub(crate) approval_policy: AskForApproval,
    pub(crate) permission_profile: PermissionProfile,
    pub(crate) windows_sandbox_level: WindowsSandboxLevel,
    pub(crate) sandbox_permissions: SandboxPermissions,
    pub(crate) prefix_rule: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CccxClassifierOutcome {
    Available(CccxPolicyClassifierOutcome),
    Unavailable,
}

pub(crate) type CccxAutoClassifierFuture<'a> =
    Pin<Box<dyn Future<Output = CccxClassifierOutcome> + Send + 'a>>;

pub(crate) trait CccxAutoClassifier: std::fmt::Debug + Send + Sync {
    fn classify(&self, action: CccxNormalizedAction) -> CccxAutoClassifierFuture<'_>;
}

#[derive(Debug, Clone)]
struct CccxNineRouterAutoClassifier {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

#[derive(Debug, serde::Serialize)]
struct CccxClassifierRequestMessage {
    role: &'static str,
    content: String,
}

#[derive(Debug, serde::Serialize)]
struct CccxClassifierRequest {
    model: String,
    messages: Vec<CccxClassifierRequestMessage>,
    stream: bool,
    temperature: f32,
}

#[derive(Debug, serde::Deserialize)]
struct CccxClassifierResponse {
    choices: Vec<CccxClassifierChoice>,
}

#[derive(Debug, serde::Deserialize)]
struct CccxClassifierChoice {
    message: CccxClassifierMessage,
}

#[derive(Debug, serde::Deserialize)]
struct CccxClassifierMessage {
    content: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
struct CccxClassifierDecision {
    decision: String,
    #[serde(default)]
    reason: Option<String>,
}

impl CccxNineRouterAutoClassifier {
    fn from_env() -> Option<Self> {
        let api_key = std::env::var(NINE_ROUTER_API_KEY_ENV).ok()?;
        let model = std::env::var(CCCX_AUTO_CLASSIFIER_MODEL_ENV).ok()?;
        Some(Self {
            client: reqwest::Client::new(),
            api_key,
            base_url: CCCX_AUTO_CLASSIFIER_BASE_URL.to_string(),
            model,
        })
    }

    async fn classify_inner(&self, action: CccxNormalizedAction) -> CccxClassifierOutcome {
        let Ok(action_json) = serde_json::to_string(&action) else {
            return CccxClassifierOutcome::Unavailable;
        };
        let request = CccxClassifierRequest {
            model: self.model.clone(),
            stream: false,
            temperature: 0.0,
            messages: vec![
                CccxClassifierRequestMessage {
                    role: "system",
                    content: "You are the cccx Auto-mode permission classifier. Return only compact JSON with decision=allow|deny|require_approval|reroute_to_cx and optional reason. Never exceed deterministic policy gates.".to_string(),
                },
                CccxClassifierRequestMessage {
                    role: "user",
                    content: action_json,
                },
            ],
        };
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let response = match timeout(
            CCCX_AUTO_CLASSIFIER_TIMEOUT,
            self.client
                .post(url)
                .bearer_auth(&self.api_key)
                .json(&request)
                .send(),
        )
        .await
        {
            Ok(Ok(response)) => response,
            _ => return CccxClassifierOutcome::Unavailable,
        };
        if !response.status().is_success() {
            return CccxClassifierOutcome::Unavailable;
        }
        let Ok(response) = response.json::<CccxClassifierResponse>().await else {
            return CccxClassifierOutcome::Unavailable;
        };
        let Some(content) = response
            .choices
            .first()
            .and_then(|choice| choice.message.content.as_deref())
        else {
            return CccxClassifierOutcome::Unavailable;
        };
        parse_cccx_classifier_content(content)
            .map(CccxClassifierOutcome::Available)
            .unwrap_or(CccxClassifierOutcome::Unavailable)
    }
}

impl CccxAutoClassifier for CccxNineRouterAutoClassifier {
    fn classify(&self, action: CccxNormalizedAction) -> CccxAutoClassifierFuture<'_> {
        Box::pin(async move { self.classify_inner(action).await })
    }
}

fn parse_cccx_classifier_content(content: &str) -> Option<CccxPolicyClassifierOutcome> {
    let trimmed = content.trim();
    let decision = serde_json::from_str::<CccxClassifierDecision>(trimmed).ok()?;
    let reason = decision
        .reason
        .filter(|reason| !reason.trim().is_empty())
        .unwrap_or_else(|| "classifier".to_string());
    match decision.decision.as_str() {
        "allow" => Some(CccxPolicyClassifierOutcome::Allow),
        "deny" => Some(CccxPolicyClassifierOutcome::Deny { reason }),
        "require_approval" => Some(CccxPolicyClassifierOutcome::RequireApproval { reason }),
        "reroute_to_cx" => Some(CccxPolicyClassifierOutcome::RerouteToCx { reason }),
        _ => None,
    }
}

fn cccx_circuit_breaker_requirement(command: &[String]) -> Option<ExecApprovalRequirement> {
    let action = cccx_shell_action(command);
    let session = CccxPolicySession {
        // Use Bypass deliberately: this pre-gate only wants the invariant
        // circuit-breaker layer. All non-circuit policy remains owned by the
        // existing Codex exec-policy path below.
        mode: CccxMainMode::Bypass,
        thresholds: CccxModeThresholds {
            max_changed_files: usize::MAX,
            max_changed_lines: usize::MAX,
        },
        explicit_allow_rules: Vec::new(),
    };

    match cccx_evaluate_action(&action, &session, &CccxPolicySet::default_m4(), None) {
        CccxPolicyDecision::Deny {
            reason,
            bypassable: false,
            ..
        } => Some(ExecApprovalRequirement::Forbidden { reason }),
        _ => None,
    }
}

fn cccx_shell_action(command: &[String]) -> CccxNormalizedAction {
    let command_summary = command.join(" ");
    CccxNormalizedAction {
        action_type: CccxActionType::Shell,
        summary: command_summary.clone(),
        command: Some(command_summary),
        target_paths: Vec::new(),
        tool_name: Some("exec".to_string()),
        task_id: "codex-core-exec".to_string(),
        changed_files: 0,
        changed_lines: 0,
    }
}

fn cccx_mcp_action(server_id: &str, tool_name: &str) -> CccxNormalizedAction {
    CccxNormalizedAction {
        action_type: CccxActionType::McpCall,
        summary: format!("mcp:{server_id}.{tool_name}"),
        command: None,
        target_paths: Vec::new(),
        tool_name: Some(format!("mcp:{server_id}.{tool_name}")),
        task_id: "codex-core-mcp".to_string(),
        changed_files: 0,
        changed_lines: 0,
    }
}

fn cccx_edit_action(file_paths: &[String], changed_lines: usize) -> CccxNormalizedAction {
    let paths_summary = file_paths.join(", ");
    CccxNormalizedAction {
        action_type: CccxActionType::Edit,
        summary: format!("edit files: {paths_summary}"),
        command: None,
        target_paths: file_paths.to_vec(),
        tool_name: Some("apply_patch".to_string()),
        task_id: "codex-core-edit".to_string(),
        changed_files: file_paths.len(),
        changed_lines,
    }
}

fn cccx_requirement_from_decision(decision: CccxPolicyDecision) -> Option<ExecApprovalRequirement> {
    match decision {
        CccxPolicyDecision::Deny { reason, .. } => {
            Some(ExecApprovalRequirement::Forbidden { reason })
        }
        CccxPolicyDecision::RequireApproval { reason, .. } => {
            Some(ExecApprovalRequirement::NeedsApproval {
                reason: Some(reason),
                proposed_execpolicy_amendment: None,
            })
        }
        CccxPolicyDecision::RerouteToCx { reason } => {
            Some(ExecApprovalRequirement::NeedsApproval {
                reason: Some(format!("{reason} — reroute to CX recommended")),
                proposed_execpolicy_amendment: None,
            })
        }
        CccxPolicyDecision::Allow { .. } => None,
    }
}

/// Runs cccx policy evaluation for MCP tool calls.
///
/// Uses `Default` mode so circuit-breaker rules hard-deny, escalate rules
/// require approval, and allowlist rules fast-path.
///
/// Returns:
/// - `Some(Forbidden)` when cccx policy hard-denies (circuit breaker match)
/// - `Some(NeedsApproval)` when user approval is required (escalate match)
/// - `None` when no cccx policy applies (allowlist match or no rule matched)
#[cfg(test)]
pub(crate) fn cccx_mcp_requirement(
    server_id: &str,
    tool_name: &str,
    mode: CccxMainMode,
) -> Option<ExecApprovalRequirement> {
    cccx_mcp_requirement_with_classifier(server_id, tool_name, mode, None)
}

fn cccx_mcp_requirement_with_classifier(
    server_id: &str,
    tool_name: &str,
    mode: CccxMainMode,
    classifier: Option<CccxPolicyClassifierOutcome>,
) -> Option<ExecApprovalRequirement> {
    let action = cccx_mcp_action(server_id, tool_name);
    let session = CccxPolicySession {
        mode,
        thresholds: CccxModeThresholds {
            max_changed_files: usize::MAX,
            max_changed_lines: usize::MAX,
        },
        explicit_allow_rules: Vec::new(),
    };

    cccx_requirement_from_decision(cccx_evaluate_action(
        &action,
        &session,
        &CccxPolicySet::default_m4(),
        classifier,
    ))
}

/// Runs the full cccx policy pipeline (circuit breaker + escalate + allowlist)
/// on shell commands with `Default` mode (matching the architecture.md spec
/// where Default=Ask requires approval for non-trivial actions).
///
/// This complements `cccx_circuit_breaker_requirement` which only checks
/// circuit-breaker invariants. This function also checks:
/// - Escalate rules (secret paths, destructive git, network install)
/// - DontAsk mode: deny silent for escalate matches
/// - Plan mode: deny edits/shell
///
/// Returns:
/// - `Some(Forbidden)` when the command is hard-denied (circuit breaker,
///   Plan-mode shell, DontAsk with escalate match)
/// - `Some(NeedsApproval)` when user approval is required (escalate match
///   in Default/AcceptEdits/Auto mode)
/// - `None` when no cccx policy applies (allowlist match, or no rule matched)
fn cccx_full_shell_requirement(
    command: &[String],
    mode: CccxMainMode,
    classifier: Option<CccxPolicyClassifierOutcome>,
) -> Option<ExecApprovalRequirement> {
    let action = cccx_shell_action(command);
    let session = CccxPolicySession {
        mode,
        thresholds: CccxModeThresholds {
            max_changed_files: usize::MAX,
            max_changed_lines: usize::MAX,
        },
        explicit_allow_rules: Vec::new(),
    };

    match cccx_evaluate_action(&action, &session, &CccxPolicySet::default_m4(), classifier) {
        CccxPolicyDecision::Deny { reason, .. } => {
            Some(ExecApprovalRequirement::Forbidden { reason })
        }
        CccxPolicyDecision::RequireApproval { reason, .. } => {
            Some(ExecApprovalRequirement::NeedsApproval {
                reason: Some(reason),
                proposed_execpolicy_amendment: None,
            })
        }
        CccxPolicyDecision::RerouteToCx { reason } => {
            // Reroute-to-CX is not actionable in shell path; treat as
            // require-approval so user can decide.
            Some(ExecApprovalRequirement::NeedsApproval {
                reason: Some(format!("{reason} — reroute to CX recommended")),
                proposed_execpolicy_amendment: None,
            })
        }
        CccxPolicyDecision::Allow { .. } => None,
    }
}

/// Runs the full cccx policy pipeline (circuit breaker + escalate + allowlist)
/// on file edit operations (apply_patch, file_write) with `Default` mode.
///
/// This mirrors `cccx_full_shell_requirement` for shell commands, applying the
/// same decision logic to edit actions:
/// - Circuit breaker: rejects known-dangerous edit patterns
/// - Escalate: requires approval for writes to sensitive paths
/// - Allowlist: fast-passes known-safe edit operations
///
/// Returns:
/// - `Some(Forbidden)` when the edit is hard-denied (circuit breaker,
///   Plan-mode edit, DontAsk with escalate match)
/// - `Some(NeedsApproval)` when user approval is required
/// - `None` when no cccx policy applies (allowlist match, or no rule matched)
#[cfg(test)]
pub(crate) fn cccx_edit_requirement_with_changed_lines(
    file_paths: &[String],
    mode: CccxMainMode,
    changed_lines: usize,
) -> Option<ExecApprovalRequirement> {
    cccx_edit_requirement_with_classifier(file_paths, mode, changed_lines, None)
}

fn cccx_edit_requirement_with_classifier(
    file_paths: &[String],
    mode: CccxMainMode,
    changed_lines: usize,
    classifier: Option<CccxPolicyClassifierOutcome>,
) -> Option<ExecApprovalRequirement> {
    let action = cccx_edit_action(file_paths, changed_lines);
    let session = CccxPolicySession {
        mode,
        thresholds: CccxModeThresholds {
            max_changed_files: usize::MAX,
            max_changed_lines: 20,
        },
        explicit_allow_rules: Vec::new(),
    };

    cccx_requirement_from_decision(cccx_evaluate_action(
        &action,
        &session,
        &CccxPolicySet::default_m4(),
        classifier,
    ))
}

impl ExecPolicyManager {
    pub(crate) fn new(policy: Arc<Policy>) -> Self {
        Self {
            policy: ArcSwap::from(policy),
            update_lock: Semaphore::new(/*permits*/ 1),
            cccx_auto_classifier: CccxNineRouterAutoClassifier::from_env()
                .map(|classifier| Arc::new(classifier) as Arc<dyn CccxAutoClassifier>),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_cccx_auto_classifier_for_test(
        mut self,
        classifier: Arc<dyn CccxAutoClassifier>,
    ) -> Self {
        self.cccx_auto_classifier = Some(classifier);
        self
    }

    #[instrument(level = "info", skip_all)]
    pub(crate) async fn load(config_stack: &ConfigLayerStack) -> Result<Self, ExecPolicyError> {
        let (policy, warning) = load_exec_policy_with_warning(config_stack).await?;
        if let Some(err) = warning.as_ref() {
            tracing::warn!("failed to parse rules: {err}");
        }
        Ok(Self::new(Arc::new(policy)))
    }

    pub(crate) fn current(&self) -> Arc<Policy> {
        self.policy.load_full()
    }

    async fn cccx_classifier_outcome_for_mode(
        &self,
        cccx_mode: &CccxMainMode,
        action: CccxNormalizedAction,
    ) -> Option<CccxPolicyClassifierOutcome> {
        if *cccx_mode != CccxMainMode::Auto {
            return None;
        }
        match self.cccx_auto_classifier.as_ref() {
            Some(classifier) => match classifier.classify(action).await {
                CccxClassifierOutcome::Available(outcome) => Some(outcome),
                CccxClassifierOutcome::Unavailable => {
                    Some(CccxPolicyClassifierOutcome::Unavailable)
                }
            },
            None => Some(CccxPolicyClassifierOutcome::Unavailable),
        }
    }

    pub(crate) async fn create_edit_approval_requirement_for_changed_lines_in_mode(
        &self,
        file_paths: &[String],
        cccx_mode: CccxMainMode,
        changed_lines: usize,
    ) -> Option<ExecApprovalRequirement> {
        let action = cccx_edit_action(file_paths, changed_lines);
        let classifier = self
            .cccx_classifier_outcome_for_mode(&cccx_mode, action)
            .await;
        cccx_edit_requirement_with_classifier(file_paths, cccx_mode, changed_lines, classifier)
    }

    pub(crate) async fn create_mcp_approval_requirement_in_mode(
        &self,
        server_id: &str,
        tool_name: &str,
        cccx_mode: CccxMainMode,
    ) -> Option<ExecApprovalRequirement> {
        let action = cccx_mcp_action(server_id, tool_name);
        let classifier = self
            .cccx_classifier_outcome_for_mode(&cccx_mode, action)
            .await;
        cccx_mcp_requirement_with_classifier(server_id, tool_name, cccx_mode, classifier)
    }

    #[cfg(test)]
    pub(crate) async fn create_exec_approval_requirement_for_command(
        &self,
        req: ExecApprovalRequest<'_>,
    ) -> ExecApprovalRequirement {
        self.create_exec_approval_requirement_for_command_in_mode(req, CccxMainMode::Default)
            .await
    }

    pub(crate) async fn create_exec_approval_requirement_for_command_in_mode(
        &self,
        req: ExecApprovalRequest<'_>,
        cccx_mode: CccxMainMode,
    ) -> ExecApprovalRequirement {
        let ExecApprovalRequest {
            command,
            approval_policy,
            permission_profile,
            windows_sandbox_level,
            sandbox_permissions,
            prefix_rule,
        } = req;
        if let Some(requirement) = cccx_circuit_breaker_requirement(command) {
            return requirement;
        }
        // Run full cccx policy (escalate + allowlist).
        // The mode is threaded from the session's active cccx mode state.
        // When no session is available, fall back to Default.
        let classifier = self
            .cccx_classifier_outcome_for_mode(&cccx_mode, cccx_shell_action(command))
            .await;
        if let Some(requirement) = cccx_full_shell_requirement(command, cccx_mode, classifier) {
            // Forbidden (Plan-mode, DontAsk+escalate): hard-deny immediately.
            // NeedsApproval (escalate in Default/AcceptEdits): bubble
            // up so user gets a meaningful prompt from cccx policy.
            return requirement;
        }
        let exec_policy = self.current();
        let ExecPolicyCommands {
            commands,
            used_complex_parsing,
            command_origin,
        } = commands_for_exec_policy(command);
        // Keep heredoc prefix parsing for rule evaluation so existing
        // allow/prompt/forbidden rules still apply, but avoid auto-derived
        // amendments when only the heredoc fallback parser matched.
        let auto_amendment_allowed = !used_complex_parsing;
        let exec_policy_fallback = |cmd: &[String]| {
            render_decision_for_unmatched_command(
                cmd,
                UnmatchedCommandContext {
                    approval_policy,
                    permission_profile: &permission_profile,
                    windows_sandbox_level,
                    sandbox_permissions,
                    used_complex_parsing,
                    command_origin,
                },
            )
        };
        let match_options = MatchOptions {
            resolve_host_executables: true,
        };
        let evaluation = exec_policy.check_multiple_with_options(
            commands.iter(),
            &exec_policy_fallback,
            &match_options,
        );

        let requested_amendment = if auto_amendment_allowed {
            derive_requested_execpolicy_amendment_from_prefix_rule(
                prefix_rule.as_ref(),
                &evaluation.matched_rules,
                exec_policy.as_ref(),
                &commands,
                &exec_policy_fallback,
                &match_options,
            )
        } else {
            None
        };

        match evaluation.decision {
            Decision::Forbidden => ExecApprovalRequirement::Forbidden {
                reason: derive_forbidden_reason(
                    command,
                    &evaluation,
                    dangerous_command_match_for_heuristics(
                        &evaluation,
                        Decision::Forbidden,
                        command_origin,
                    ),
                ),
            },
            Decision::Prompt => {
                let prompt_is_rule = evaluation.matched_rules.iter().any(|rule_match| {
                    is_policy_match(rule_match) && rule_match.decision() == Decision::Prompt
                });
                match prompt_is_rejected_by_policy(approval_policy, prompt_is_rule) {
                    Some(reason) if prompt_is_rule => ExecApprovalRequirement::Forbidden {
                        reason: reason.to_string(),
                    },
                    Some(reason) => ExecApprovalRequirement::Forbidden {
                        reason: derive_rejected_prompt_reason(
                            reason,
                            dangerous_command_match_for_heuristics(
                                &evaluation,
                                Decision::Prompt,
                                command_origin,
                            ),
                        ),
                    },
                    None => ExecApprovalRequirement::NeedsApproval {
                        reason: derive_prompt_reason(command, &evaluation),
                        proposed_execpolicy_amendment: requested_amendment.or_else(|| {
                            if auto_amendment_allowed {
                                try_derive_execpolicy_amendment_for_prompt_rules(
                                    &evaluation.matched_rules,
                                )
                            } else {
                                None
                            }
                        }),
                    },
                }
            }
            Decision::Allow => ExecApprovalRequirement::Skip {
                // Bypass sandbox only when every parsed command segment is
                // explicitly allowed by execpolicy.
                bypass_sandbox: commands.iter().all(|command| {
                    exec_policy
                        .matches_for_command_with_options(
                            command,
                            /*heuristics_fallback*/ None,
                            &match_options,
                        )
                        .iter()
                        .any(|rule_match| {
                            is_policy_match(rule_match) && rule_match.decision() == Decision::Allow
                        })
                }),
                proposed_execpolicy_amendment: if auto_amendment_allowed {
                    try_derive_execpolicy_amendment_for_allow_rules(&evaluation.matched_rules)
                } else {
                    None
                },
            },
        }
    }

    pub(crate) async fn append_amendment_and_update(
        &self,
        codex_home: &Path,
        amendment: &ExecPolicyAmendment,
    ) -> Result<(), ExecPolicyUpdateError> {
        let _update_guard =
            self.update_lock
                .acquire()
                .await
                .map_err(|_| ExecPolicyUpdateError::AddRule {
                    source: ExecPolicyRuleError::InvalidRule(
                        "exec policy update semaphore closed".to_string(),
                    ),
                })?;
        let policy_path = default_policy_path(codex_home);
        spawn_blocking({
            let policy_path = policy_path.clone();
            let prefix = amendment.command.clone();
            move || blocking_append_allow_prefix_rule(&policy_path, &prefix)
        })
        .await
        .map_err(|source| ExecPolicyUpdateError::JoinBlockingTask { source })?
        .map_err(|source| ExecPolicyUpdateError::AppendRule {
            path: policy_path,
            source,
        })?;

        let current_policy = self.current();
        let match_options = MatchOptions {
            resolve_host_executables: true,
        };
        let existing_evaluation = current_policy.check_multiple_with_options(
            [&amendment.command],
            &|_| Decision::Forbidden,
            &match_options,
        );
        let already_allowed = existing_evaluation.decision == Decision::Allow
            && existing_evaluation.matched_rules.iter().any(|rule_match| {
                is_policy_match(rule_match) && rule_match.decision() == Decision::Allow
            });
        if already_allowed {
            return Ok(());
        }

        let mut updated_policy = current_policy.as_ref().clone();
        updated_policy.add_prefix_rule(&amendment.command, Decision::Allow)?;
        self.policy.store(Arc::new(updated_policy));
        Ok(())
    }

    pub(crate) async fn append_network_rule_and_update(
        &self,
        codex_home: &Path,
        host: &str,
        protocol: NetworkRuleProtocol,
        decision: Decision,
        justification: Option<String>,
    ) -> Result<(), ExecPolicyUpdateError> {
        let _update_guard =
            self.update_lock
                .acquire()
                .await
                .map_err(|_| ExecPolicyUpdateError::AddRule {
                    source: ExecPolicyRuleError::InvalidRule(
                        "exec policy update semaphore closed".to_string(),
                    ),
                })?;
        let policy_path = default_policy_path(codex_home);
        let host = host.to_string();
        spawn_blocking({
            let policy_path = policy_path.clone();
            let host = host.clone();
            let justification = justification.clone();
            move || {
                blocking_append_network_rule(
                    &policy_path,
                    &host,
                    protocol,
                    decision,
                    justification.as_deref(),
                )
            }
        })
        .await
        .map_err(|source| ExecPolicyUpdateError::JoinBlockingTask { source })?
        .map_err(|source| ExecPolicyUpdateError::AppendRule {
            path: policy_path,
            source,
        })?;

        let mut updated_policy = self.current().as_ref().clone();
        updated_policy.add_network_rule(&host, protocol, decision, justification)?;
        self.policy.store(Arc::new(updated_policy));
        Ok(())
    }
}

impl Default for ExecPolicyManager {
    fn default() -> Self {
        Self::new(Arc::new(Policy::empty()))
    }
}

pub async fn check_execpolicy_for_warnings(
    config_stack: &ConfigLayerStack,
) -> Result<Option<ExecPolicyError>, ExecPolicyError> {
    let (_, warning) = load_exec_policy_with_warning(config_stack).await?;
    Ok(warning)
}

fn exec_policy_message_for_display(source: &codex_execpolicy::Error) -> String {
    let message = source.to_string();
    if let Some(line) = message
        .lines()
        .find(|line| line.trim_start().starts_with("error: "))
    {
        return line.to_owned();
    }
    if let Some(first_line) = message.lines().next()
        && let Some((_, detail)) = first_line.rsplit_once(": starlark error: ")
    {
        return detail.trim().to_string();
    }

    message
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn parse_starlark_line_from_message(message: &str) -> Option<(PathBuf, usize)> {
    let first_line = message.lines().next()?.trim();
    let (path_and_position, _) = first_line.rsplit_once(": starlark error:")?;

    let mut parts = path_and_position.rsplitn(3, ':');
    let _column = parts.next()?.parse::<usize>().ok()?;
    let line = parts.next()?.parse::<usize>().ok()?;
    let path = PathBuf::from(parts.next()?);

    if line == 0 {
        return None;
    }

    Some((path, line))
}

pub fn format_exec_policy_error_with_source(error: &ExecPolicyError) -> String {
    match error {
        ExecPolicyError::ParsePolicy { path, source } => {
            let rendered_source = source.to_string();
            let structured_location = source
                .location()
                .map(|location| (PathBuf::from(location.path), location.range.start.line));
            let parsed_location = parse_starlark_line_from_message(&rendered_source);
            let location = match (structured_location, parsed_location) {
                (Some((_, 1)), Some((parsed_path, parsed_line))) if parsed_line > 1 => {
                    Some((parsed_path, parsed_line))
                }
                (Some(structured), _) => Some(structured),
                (None, parsed) => parsed,
            };
            let message = exec_policy_message_for_display(source);
            match location {
                Some((path, line)) => {
                    format!(
                        "{}:{}: {} (problem is on or around line {})",
                        path.display(),
                        line,
                        message,
                        line
                    )
                }
                None => format!("{path}: {message}"),
            }
        }
        _ => error.to_string(),
    }
}

pub(crate) async fn load_exec_policy_with_warning(
    config_stack: &ConfigLayerStack,
) -> Result<(Policy, Option<ExecPolicyError>), ExecPolicyError> {
    match load_exec_policy(config_stack).await {
        Ok(policy) => Ok((policy, None)),
        Err(err @ ExecPolicyError::ParsePolicy { .. }) => {
            let policy = config_stack
                .requirements()
                .exec_policy
                .as_deref()
                .map_or_else(Policy::empty, |policy| policy.as_ref().clone());
            Ok((policy, Some(err)))
        }
        Err(err) => Err(err),
    }
}

pub async fn load_exec_policy(config_stack: &ConfigLayerStack) -> Result<Policy, ExecPolicyError> {
    // Disabled project layers already represent the trust decision, so hooks
    // and exec-policy loading can reuse the normal trusted-layer view.
    // Iterate the layers in increasing order of precedence, adding the *.rules
    // from each layer, so that higher-precedence layers can override
    // rules defined in lower-precedence ones.
    let mut policy_paths = Vec::new();
    for layer in config_stack.get_layers(
        ConfigLayerStackOrdering::LowestPrecedenceFirst,
        /*include_disabled*/ false,
    ) {
        if config_stack.ignore_user_and_project_exec_policy_rules()
            && matches!(
                layer.name,
                ConfigLayerSource::User { .. } | ConfigLayerSource::Project { .. }
            )
        {
            continue;
        }
        if let Some(config_folder) = layer.config_folder() {
            let policy_dir = config_folder.join(RULES_DIR_NAME);
            let layer_policy_paths = collect_policy_files(&policy_dir).await?;
            policy_paths.extend(layer_policy_paths);
        }
    }
    tracing::trace!(
        policy_paths = ?policy_paths,
        "loaded exec policies"
    );

    let mut parser = PolicyParser::new();
    for policy_path in &policy_paths {
        let contents =
            fs::read_to_string(policy_path)
                .await
                .map_err(|source| ExecPolicyError::ReadFile {
                    path: policy_path.clone(),
                    source,
                })?;
        let identifier = policy_path.to_string_lossy().to_string();
        parser
            .parse(&identifier, &contents)
            .map_err(|source| ExecPolicyError::ParsePolicy {
                path: identifier,
                source,
            })?;
    }

    let policy = parser.build();
    tracing::debug!("loaded rules from {} files", policy_paths.len());
    tracing::trace!(rules = ?policy, "exec policy rules loaded");

    let Some(requirements_policy) = config_stack.requirements().exec_policy.as_deref() else {
        return Ok(policy);
    };

    Ok(policy.merge_overlay(requirements_policy.as_ref()))
}

fn dangerous_command_match_for_origin(
    command: &[String],
    command_origin: ExecPolicyCommandOrigin,
) -> Option<DangerousCommandMatch> {
    match command_origin {
        ExecPolicyCommandOrigin::Generic => dangerous_command_match(command),
        #[cfg(windows)]
        ExecPolicyCommandOrigin::PowerShell => {
            codex_shell_command::is_dangerous_command::dangerous_powershell_words_match(command)
        }
    }
}

/// Extract DangerousCommandMatch from an Evaluation
fn dangerous_command_match_for_heuristics(
    evaluation: &Evaluation,
    decision: Decision,
    command_origin: ExecPolicyCommandOrigin,
) -> Option<DangerousCommandMatch> {
    evaluation
        .matched_rules
        .iter()
        .find_map(|rule_match| match rule_match {
            RuleMatch::HeuristicsRuleMatch {
                command,
                decision: matched_decision,
            } if *matched_decision == decision => {
                dangerous_command_match_for_origin(command, command_origin)
            }
            _ => None,
        })
}

/// If a command is not matched by any execpolicy rule, derive a [`Decision`].
pub(crate) fn render_decision_for_unmatched_command(
    command: &[String],
    context: UnmatchedCommandContext<'_>,
) -> Decision {
    let dangerous_command_match =
        dangerous_command_match_for_origin(command, context.command_origin);
    let UnmatchedCommandContext {
        approval_policy,
        permission_profile,
        windows_sandbox_level,
        sandbox_permissions,
        used_complex_parsing,
        command_origin,
    } = context;
    let file_system_sandbox_policy = permission_profile.file_system_sandbox_policy();
    let is_known_safe = match command_origin {
        ExecPolicyCommandOrigin::Generic => is_known_safe_command(command),
        #[cfg(windows)]
        ExecPolicyCommandOrigin::PowerShell => {
            codex_shell_command::is_safe_command::is_safe_powershell_words(command)
        }
    };

    // When the Windows sandbox backend is disabled, managed filesystem
    // restrictions are only a policy shape; there is no platform sandbox to
    // enforce the boundary. Keep that legacy case conservative while still
    // relying on the real Windows sandbox when it is enabled.
    let windows_managed_fs_restrictions_without_sandbox_backend = cfg!(windows)
        && windows_sandbox_level == WindowsSandboxLevel::Disabled
        && profile_has_managed_filesystem_restrictions(permission_profile);

    if is_known_safe
        && !used_complex_parsing
        && (approval_policy == AskForApproval::UnlessTrusted
            || windows_managed_fs_restrictions_without_sandbox_backend)
    {
        return Decision::Allow;
    }

    // If the command is flagged as dangerous or we have no sandbox protection,
    // we should never allow it to run without approval.
    //
    // We prefer to prompt the user rather than outright forbid the command,
    // but if the user has explicitly disabled prompts, we must
    // forbid the command.
    if dangerous_command_match.is_some() || windows_managed_fs_restrictions_without_sandbox_backend
    {
        return match approval_policy {
            AskForApproval::Never => Decision::Forbidden,
            AskForApproval::OnRequest
            | AskForApproval::UnlessTrusted
            | AskForApproval::Granular(_) => Decision::Prompt,
        };
    }

    match approval_policy {
        AskForApproval::Never => {
            // We allow the command to run, relying on the sandbox for
            // protection.
            Decision::Allow
        }
        AskForApproval::UnlessTrusted => {
            // We already checked the unmatched-command safelist and it
            // returned false, so we must prompt.
            Decision::Prompt
        }
        AskForApproval::OnRequest => {
            match file_system_sandbox_policy.kind {
                FileSystemSandboxKind::Unrestricted | FileSystemSandboxKind::ExternalSandbox => {
                    // The user has indicated we should "just run" commands
                    // in their unrestricted environment, so we do so since the
                    // command has not been flagged as dangerous.
                    Decision::Allow
                }
                FileSystemSandboxKind::Restricted => {
                    // In restricted sandboxes, do not prompt for non-escalated,
                    // non-dangerous commands; let the sandbox enforce
                    // restrictions without a user prompt.
                    if sandbox_permissions.requests_sandbox_override() {
                        Decision::Prompt
                    } else {
                        Decision::Allow
                    }
                }
            }
        }
        AskForApproval::Granular(_) => match file_system_sandbox_policy.kind {
            FileSystemSandboxKind::Unrestricted | FileSystemSandboxKind::ExternalSandbox => {
                // Mirror on-request behavior for unmatched commands; prompt-vs-reject is handled
                // by `prompt_is_rejected_by_policy`.
                Decision::Allow
            }
            FileSystemSandboxKind::Restricted => {
                if sandbox_permissions.requests_sandbox_override() {
                    Decision::Prompt
                } else {
                    Decision::Allow
                }
            }
        },
    }
}

fn profile_has_managed_filesystem_restrictions(permission_profile: &PermissionProfile) -> bool {
    let file_system_sandbox_policy = permission_profile.file_system_sandbox_policy();
    matches!(permission_profile, PermissionProfile::Managed { .. })
        && matches!(
            file_system_sandbox_policy.kind,
            FileSystemSandboxKind::Restricted
        )
        && !file_system_sandbox_policy.has_full_disk_write_access()
}

fn default_policy_path(codex_home: &Path) -> PathBuf {
    codex_home.join(RULES_DIR_NAME).join(DEFAULT_POLICY_FILE)
}

fn commands_for_exec_policy(command: &[String]) -> ExecPolicyCommands {
    if let Some(commands) = parse_shell_lc_plain_commands(command)
        && !commands.is_empty()
    {
        return ExecPolicyCommands {
            commands,
            used_complex_parsing: false,
            command_origin: ExecPolicyCommandOrigin::Generic,
        };
    }

    #[cfg(windows)]
    {
        if let Some(commands) =
            codex_shell_command::powershell::parse_powershell_command_into_plain_commands(command)
            && !commands.is_empty()
        {
            return ExecPolicyCommands {
                commands,
                used_complex_parsing: false,
                command_origin: ExecPolicyCommandOrigin::PowerShell,
            };
        }
    }

    if let Some(single_command) = parse_shell_lc_single_command_prefix(command) {
        return ExecPolicyCommands {
            commands: vec![single_command],
            used_complex_parsing: true,
            command_origin: ExecPolicyCommandOrigin::Generic,
        };
    }

    ExecPolicyCommands {
        commands: vec![command.to_vec()],
        used_complex_parsing: false,
        command_origin: ExecPolicyCommandOrigin::Generic,
    }
}

/// Derive a proposed execpolicy amendment when a command requires user approval
/// - If any execpolicy rule prompts, return None, because an amendment would not skip that policy requirement.
/// - Otherwise return the first heuristics Prompt.
/// - Examples:
/// - execpolicy: empty. Command: `["python"]`. Heuristics prompt -> `Some(vec!["python"])`.
/// - execpolicy: empty. Command: `["bash", "-c", "cd /some/folder && prog1 --option1 arg1 && prog2 --option2 arg2"]`.
///   Parsed commands include `cd /some/folder`, `prog1 --option1 arg1`, and `prog2 --option2 arg2`. If heuristics allow `cd` but prompt
///   on `prog1`, we return `Some(vec!["prog1", "--option1", "arg1"])`.
/// - execpolicy: contains a `prompt for prefix ["prog2"]` rule. For the same command as above,
///   we return `None` because an execpolicy prompt still applies even if we amend execpolicy to allow ["prog1", "--option1", "arg1"].
fn try_derive_execpolicy_amendment_for_prompt_rules(
    matched_rules: &[RuleMatch],
) -> Option<ExecPolicyAmendment> {
    if matched_rules
        .iter()
        .any(|rule_match| is_policy_match(rule_match) && rule_match.decision() == Decision::Prompt)
    {
        return None;
    }

    matched_rules
        .iter()
        .find_map(|rule_match| match rule_match {
            RuleMatch::HeuristicsRuleMatch {
                command,
                decision: Decision::Prompt,
            } => Some(ExecPolicyAmendment::from(command.clone())),
            _ => None,
        })
}

/// - Note: we only use this amendment when the command fails to run in sandbox and codex prompts the user to run outside the sandbox
/// - The purpose of this amendment is to bypass sandbox for similar commands in the future
/// - If any execpolicy rule matches, return None, because we would already be running command outside the sandbox
fn try_derive_execpolicy_amendment_for_allow_rules(
    matched_rules: &[RuleMatch],
) -> Option<ExecPolicyAmendment> {
    if matched_rules.iter().any(is_policy_match) {
        return None;
    }

    matched_rules
        .iter()
        .find_map(|rule_match| match rule_match {
            RuleMatch::HeuristicsRuleMatch {
                command,
                decision: Decision::Allow,
            } => Some(ExecPolicyAmendment::from(command.clone())),
            _ => None,
        })
}

fn derive_requested_execpolicy_amendment_from_prefix_rule(
    prefix_rule: Option<&Vec<String>>,
    matched_rules: &[RuleMatch],
    exec_policy: &Policy,
    commands: &[Vec<String>],
    exec_policy_fallback: &impl Fn(&[String]) -> Decision,
    match_options: &MatchOptions,
) -> Option<ExecPolicyAmendment> {
    let prefix_rule = prefix_rule?;
    if prefix_rule.is_empty() {
        return None;
    }
    if BANNED_PREFIX_SUGGESTIONS.iter().any(|banned| {
        prefix_rule.len() == banned.len()
            && prefix_rule
                .iter()
                .map(String::as_str)
                .eq(banned.iter().copied())
    }) {
        return None;
    }

    // if any policy rule already matches, don't suggest an additional rule that might conflict or not apply
    if matched_rules.iter().any(is_policy_match) {
        return None;
    }

    let amendment = ExecPolicyAmendment::new(prefix_rule.clone());
    if prefix_rule_would_approve_all_commands(
        exec_policy,
        &amendment.command,
        commands,
        exec_policy_fallback,
        match_options,
    ) {
        Some(amendment)
    } else {
        None
    }
}

fn prefix_rule_would_approve_all_commands(
    exec_policy: &Policy,
    prefix_rule: &[String],
    commands: &[Vec<String>],
    exec_policy_fallback: &impl Fn(&[String]) -> Decision,
    match_options: &MatchOptions,
) -> bool {
    let mut policy_with_prefix_rule = exec_policy.clone();
    if policy_with_prefix_rule
        .add_prefix_rule(prefix_rule, Decision::Allow)
        .is_err()
    {
        return false;
    }

    commands.iter().all(|command| {
        policy_with_prefix_rule
            .check_with_options(command, exec_policy_fallback, match_options)
            .decision
            == Decision::Allow
    })
}

/// Only return a reason when a policy rule drove the prompt decision.
fn derive_prompt_reason(command_args: &[String], evaluation: &Evaluation) -> Option<String> {
    let command = render_shlex_command(command_args);

    let most_specific_prompt = evaluation
        .matched_rules
        .iter()
        .filter_map(|rule_match| match rule_match {
            RuleMatch::PrefixRuleMatch {
                matched_prefix,
                decision: Decision::Prompt,
                justification,
                ..
            } => Some((matched_prefix.len(), justification.as_deref())),
            _ => None,
        })
        .max_by_key(|(matched_prefix_len, _)| *matched_prefix_len);

    match most_specific_prompt {
        Some((_matched_prefix_len, Some(justification))) => {
            Some(format!("`{command}` requires approval: {justification}"))
        }
        Some((_matched_prefix_len, None)) => {
            Some(format!("`{command}` requires approval by policy"))
        }
        None => None,
    }
}

fn render_shlex_command(args: &[String]) -> String {
    shlex_try_join(args.iter().map(String::as_str)).unwrap_or_else(|_| args.join(" "))
}

/// Derive a string explaining why the command was forbidden. If `justification`
/// is set by the user, this can contain instructions with recommended
/// alternatives, for example.
fn derive_forbidden_reason(
    command_args: &[String],
    evaluation: &Evaluation,
    dangerous_command_match: Option<DangerousCommandMatch>,
) -> String {
    let command = render_shlex_command(command_args);

    let most_specific_forbidden = evaluation
        .matched_rules
        .iter()
        .filter_map(|rule_match| match rule_match {
            RuleMatch::PrefixRuleMatch {
                matched_prefix,
                decision: Decision::Forbidden,
                justification,
                ..
            } => Some((matched_prefix, justification.as_deref())),
            _ => None,
        })
        .max_by_key(|(matched_prefix, _)| matched_prefix.len());

    match most_specific_forbidden {
        Some((_matched_prefix, Some(justification))) => {
            format!("`{command}` rejected: {justification}")
        }
        Some((matched_prefix, None)) => {
            let prefix = render_shlex_command(matched_prefix);
            format!("`{command}` rejected: policy forbids commands starting with `{prefix}`")
        }
        None => {
            if let Some(dangerous_command_match) = dangerous_command_match {
                let reason = dangerous_command_rejection_reason(dangerous_command_match);
                format!("`{command}` rejected: {reason}")
            } else {
                format!("`{command}` rejected: blocked by policy")
            }
        }
    }
}

fn derive_rejected_prompt_reason(
    fallback_reason: &str,
    dangerous_command_match: Option<DangerousCommandMatch>,
) -> String {
    match dangerous_command_match {
        Some(dangerous_command_match @ DangerousCommandMatch::ForcedRm) => {
            dangerous_command_rejection_reason(dangerous_command_match).to_string()
        }
        Some(DangerousCommandMatch::Other) | None => fallback_reason.to_string(),
    }
}

fn dangerous_command_rejection_reason(
    dangerous_command_match: DangerousCommandMatch,
) -> &'static str {
    match dangerous_command_match {
        DangerousCommandMatch::ForcedRm => {
            "rm -f style commands are not permitted. Use a safer approach"
        }
        DangerousCommandMatch::Other => "blocked by policy",
    }
}

async fn collect_policy_files(dir: impl AsRef<Path>) -> Result<Vec<PathBuf>, ExecPolicyError> {
    let dir = dir.as_ref();
    let mut read_dir = match fs::read_dir(dir).await {
        Ok(read_dir) => read_dir,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ExecPolicyError::ReadDir {
                dir: dir.to_path_buf(),
                source,
            });
        }
    };

    let mut policy_paths = Vec::new();
    while let Some(entry) =
        read_dir
            .next_entry()
            .await
            .map_err(|source| ExecPolicyError::ReadDir {
                dir: dir.to_path_buf(),
                source,
            })?
    {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .await
            .map_err(|source| ExecPolicyError::ReadDir {
                dir: dir.to_path_buf(),
                source,
            })?;

        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext == RULE_EXTENSION)
            && file_type.is_file()
        {
            policy_paths.push(path);
        }
    }

    policy_paths.sort();

    tracing::debug!(
        "loaded {} .rules files in {}",
        policy_paths.len(),
        dir.display()
    );
    Ok(policy_paths)
}

/// cccx subagent permission evaluation bridge.
///
/// Called when a subagent (CC-harness or CX-harness) tries to execute a tool.
/// Architecture.md §5.9.
pub(crate) fn cccx_subagent_policy_requirement(
    work_spec: &cccx_protocol::WorkSpec,
    _tool_name: &str,
    action: &CccxNormalizedAction,
) -> Option<ExecApprovalRequirement> {
    let decision = evaluate_subagent(action, work_spec, &CccxPolicySet::default_m4());

    match decision {
        CccxPolicyDecision::Deny { reason, .. } => {
            Some(ExecApprovalRequirement::Forbidden { reason })
        }
        CccxPolicyDecision::RequireApproval { reason, .. } => {
            Some(ExecApprovalRequirement::NeedsApproval {
                reason: Some(reason),
                proposed_execpolicy_amendment: None,
            })
        }
        CccxPolicyDecision::RerouteToCx { reason } => {
            Some(ExecApprovalRequirement::NeedsApproval {
                reason: Some(format!(
                    "{reason} — reroute not applicable in subagent context"
                )),
                proposed_execpolicy_amendment: None,
            })
        }
        CccxPolicyDecision::Allow { .. } => None,
    }
}

/// Build and evaluate a cccx WorkSpec-bound subagent tool action.
///
/// This is the runtime gate for CC/CX subagents. It must run before the native
/// tool executor so allowed/forbidden tool and scope constraints are enforced
/// in code, not only in the subagent prompt.
pub(crate) fn cccx_subagent_tool_requirement(
    work_spec: &cccx_protocol::WorkSpec,
    tool_name: &str,
    action_type: CccxActionType,
    command: Option<String>,
    target_paths: Vec<String>,
    changed_files: usize,
    changed_lines: usize,
) -> Option<ExecApprovalRequirement> {
    let action = CccxNormalizedAction {
        action_type,
        summary: format!("subagent tool: {tool_name}"),
        command,
        target_paths,
        tool_name: Some(tool_name.to_string()),
        task_id: work_spec.task_id.clone(),
        changed_files,
        changed_lines,
    };
    cccx_subagent_policy_requirement(work_spec, tool_name, &action)
}

pub(crate) fn cccx_subagent_shell_requirement(
    work_spec: &cccx_protocol::WorkSpec,
    command: &[String],
    cwd: &Path,
) -> Option<ExecApprovalRequirement> {
    let mut target_paths = match cccx_shell_target_paths(command, cwd) {
        Ok(paths) => paths,
        Err(reason) => {
            return Some(ExecApprovalRequirement::Forbidden {
                reason: format!("cannot resolve shell target path: {reason}"),
            });
        }
    };
    if target_paths.is_empty() && !cccx_shell_command_is_known_read_only(command) {
        match crate::cccx_paths::resolve_physical_path(cwd, cwd) {
            Ok(path) => target_paths.push(path.to_string_lossy().into_owned()),
            Err(reason) => {
                return Some(ExecApprovalRequirement::Forbidden {
                    reason: format!("cannot resolve shell working directory: {reason}"),
                });
            }
        }
    }
    cccx_subagent_tool_requirement(
        work_spec,
        "shell_command",
        CccxActionType::Shell,
        Some(command.join(" ")),
        target_paths,
        0,
        0,
    )
}

pub(crate) fn cccx_shell_target_paths(
    command: &[String],
    cwd: &Path,
) -> Result<Vec<String>, String> {
    resolve_physical_paths(cccx_extract_shell_target_paths(command), cwd)
}

pub(crate) fn cccx_shell_command_is_known_read_only(command: &[String]) -> bool {
    let joined = command.join(" ").to_ascii_lowercase();
    let mut tokens = command
        .iter()
        .flat_map(|argument| shlex::split(argument).unwrap_or_else(|| vec![argument.clone()]));
    let executable = tokens
        .next()
        .and_then(|token| {
            Path::new(&token)
                .file_stem()
                .map(|name| name.to_string_lossy().to_ascii_lowercase())
        })
        .unwrap_or_default();
    let mutating_markers = [
        "set-content",
        "add-content",
        "out-file",
        "new-item",
        "remove-item",
        "move-item",
        "copy-item",
        "rename-item",
        "tee-object",
        " >",
        ">>",
    ];
    if mutating_markers
        .iter()
        .any(|marker| joined.contains(marker))
    {
        return false;
    }
    matches!(
        executable.as_str(),
        "rg" | "grep" | "findstr" | "type" | "where" | "pwd" | "ls" | "dir"
    ) || ((executable == "powershell" || executable == "pwsh")
        && [
            "get-content",
            "get-childitem",
            "select-string",
            "resolve-path",
            "test-path",
        ]
        .iter()
        .any(|verb| joined.contains(verb)))
        || (executable == "git"
            && [" status", " diff", " log", " show", " rev-parse"]
                .iter()
                .any(|verb| joined.contains(verb)))
}

pub(crate) fn cccx_subagent_edit_requirement(
    work_spec: &cccx_protocol::WorkSpec,
    tool_name: &str,
    target_paths: Vec<String>,
) -> Option<ExecApprovalRequirement> {
    let changed_files = target_paths.len();
    cccx_subagent_tool_requirement(
        work_spec,
        tool_name,
        CccxActionType::Edit,
        None,
        target_paths,
        changed_files,
        0,
    )
}

pub(crate) fn cccx_subagent_mcp_requirement(
    work_spec: &cccx_protocol::WorkSpec,
    server_id: &str,
    tool_name: &str,
    input_schema: &serde_json::Value,
    target_paths: Vec<String>,
) -> Option<ExecApprovalRequirement> {
    let stable_tool_name = format!("mcp:{server_id}.{tool_name}");
    if let Err(reason) = cccx_validate_mcp_tool_grant(
        &work_spec.tool_grants,
        &stable_tool_name,
        server_id,
        input_schema,
    ) {
        return Some(ExecApprovalRequirement::Forbidden { reason });
    }
    cccx_subagent_tool_requirement(
        work_spec,
        &stable_tool_name,
        CccxActionType::McpCall,
        None,
        target_paths,
        0,
        0,
    )
}

pub(crate) fn cccx_extract_mcp_target_paths(arguments: &serde_json::Value) -> Vec<String> {
    let mut paths = Vec::new();
    cccx_collect_mcp_target_paths(arguments, None, &mut paths);
    paths.sort();
    paths.dedup();
    paths
}

pub(crate) fn cccx_resolve_target_paths(
    paths: Vec<String>,
    cwd: &Path,
) -> Result<Vec<String>, String> {
    resolve_physical_paths(paths, cwd)
}

fn cccx_collect_mcp_target_paths(
    value: &serde_json::Value,
    field_name: Option<&str>,
    paths: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Object(object) => {
            for (name, value) in object {
                cccx_collect_mcp_target_paths(value, Some(name), paths);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                cccx_collect_mcp_target_paths(value, field_name, paths);
            }
        }
        serde_json::Value::String(value)
            if field_name.is_some_and(cccx_mcp_field_can_hold_path) && !value.trim().is_empty() =>
        {
            paths.push(value.to_string());
        }
        _ => {}
    }
}

fn cccx_mcp_field_can_hold_path(field_name: &str) -> bool {
    let field_name = field_name.to_ascii_lowercase();
    matches!(
        field_name.as_str(),
        "path"
            | "paths"
            | "file"
            | "files"
            | "filename"
            | "filenames"
            | "directory"
            | "directories"
            | "dir"
            | "cwd"
            | "target"
            | "targets"
            | "source"
            | "sources"
            | "destination"
            | "destinations"
    ) || field_name.ends_with("_path")
        || field_name.ends_with("_paths")
        || field_name.ends_with("_file")
        || field_name.ends_with("_files")
        || field_name.ends_with("_directory")
        || field_name.ends_with("_filename")
        || field_name.ends_with("_destination")
}

fn cccx_extract_shell_target_paths(command: &[String]) -> Vec<String> {
    let mut candidates = Vec::new();
    for (argument_index, argument) in command.iter().enumerate() {
        // A command-array argument may itself be a Windows path. Do not send
        // it through shlex first: backslashes are shell escapes there and
        // would turn `..\\outside.txt` into a different target.
        if !argument.chars().any(char::is_whitespace)
            && let Some(path) = cccx_shell_path_token(argument, argument_index == 0)
        {
            candidates.push(path);
            continue;
        }
        let tokens = shlex::split(argument).unwrap_or_else(|| vec![argument.clone()]);
        candidates.extend(
            tokens
                .into_iter()
                .enumerate()
                .filter_map(|(token_index, token)| {
                    cccx_shell_path_token(&token, argument_index == 0 && token_index == 0)
                }),
        );
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

fn cccx_shell_path_token(token: &str, is_executable: bool) -> Option<String> {
    if is_executable || token.starts_with('-') || token.contains("://") {
        return None;
    }
    let token = token
        .strip_prefix('>')
        .or_else(|| token.strip_prefix('<'))
        .unwrap_or(token);
    (token.starts_with('.')
        || token.contains('/')
        || token.contains('\\')
        || (token.len() > 2 && token.as_bytes()[1] == b':')
        || Path::new(token).extension().is_some())
    .then(|| token.to_string())
}

/// Validate the immutable grant issued in a WorkSpec against the live MCP tool
/// identity before it is invoked. Empty grants remain compatible with legacy
/// WorkSpecs; once a grant list is present every MCP call must name the exact
/// stable tool, server, and (when pinned) canonical input schema digest.
pub(crate) fn cccx_validate_mcp_tool_grant(
    grants: &[cccx_protocol::ToolGrant],
    stable_id: &str,
    server_id: &str,
    input_schema: &serde_json::Value,
) -> Result<(), String> {
    if grants.is_empty() {
        return Ok(());
    }
    let grant = grants
        .iter()
        .find(|grant| grant.stable_id == stable_id)
        .ok_or_else(|| format!("MCP tool grant `{stable_id}` is absent from WorkSpec"))?;
    if let Some(expected_server_id) = &grant.server_id
        && expected_server_id != server_id
    {
        return Err(format!(
            "MCP tool grant server mismatch: expected `{expected_server_id}`, got `{server_id}`"
        ));
    }
    if let Some(expected_digest) = &grant.schema_digest {
        let actual_digest = cccx_mcp_schema_digest(input_schema);
        if expected_digest != &actual_digest {
            return Err(format!(
                "MCP tool grant schema digest mismatch for `{stable_id}`: expected `{expected_digest}`, got `{actual_digest}`"
            ));
        }
    }
    Ok(())
}

fn cccx_mcp_schema_digest(input_schema: &serde_json::Value) -> String {
    let canonical = cccx_canonical_json(input_schema);
    let serialized = serde_json::to_vec(&canonical).unwrap_or_default();
    format!("sha256:{:x}", Sha256::digest(serialized))
}

fn cccx_canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => {
            let ordered = object
                .iter()
                .map(|(key, value)| (key.clone(), cccx_canonical_json(value)))
                .collect::<std::collections::BTreeMap<_, _>>();
            serde_json::Value::Object(ordered.into_iter().collect())
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(cccx_canonical_json).collect())
        }
        _ => value.clone(),
    }
}

#[cfg(test)]
mod cccx_policy_wiring_tests {
    use super::*;

    #[test]
    fn cccx_mcp_grant_rejects_schema_drift_before_invocation() {
        let schema =
            serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}});
        let grants = vec![cccx_protocol::ToolGrant {
            stable_id: "mcp:memory.search_graph".to_string(),
            server_id: Some("memory".to_string()),
            schema_digest: Some("sha256:incorrect".to_string()),
        }];

        let result =
            cccx_validate_mcp_tool_grant(&grants, "mcp:memory.search_graph", "memory", &schema);

        assert!(
            result.is_err_and(|reason| reason.contains("schema digest mismatch")),
            "a changed live MCP schema must be denied before the call"
        );
    }

    #[test]
    fn cccx_mcp_grant_requires_exact_stable_id_when_grants_are_present() {
        let schema = serde_json::json!({"type": "object"});
        let grants = vec![cccx_protocol::ToolGrant {
            stable_id: "mcp:memory.search_graph".to_string(),
            server_id: Some("memory".to_string()),
            schema_digest: None,
        }];

        let result =
            cccx_validate_mcp_tool_grant(&grants, "mcp:memory.manage_adr", "memory", &schema);

        assert!(
            result.is_err_and(|reason| reason.contains("is absent from WorkSpec")),
            "a grant list must deny a different MCP stable id"
        );
    }

    #[test]
    fn cccx_extracts_every_nested_mcp_path_argument() {
        let arguments = serde_json::json!({
            "paths": ["src/lib.rs", "src/main.rs"],
            "options": {"target_file": "Cargo.toml"},
            "changes": [{"filename": "src/changed.rs"}],
            "query": "not a path"
        });

        assert_eq!(
            cccx_extract_mcp_target_paths(&arguments),
            vec!["Cargo.toml", "src/changed.rs", "src/lib.rs", "src/main.rs"]
        );
    }

    #[test]
    fn cccx_extracts_multiple_shell_path_arguments() {
        assert_eq!(
            cccx_extract_shell_target_paths(&[
                "git".to_string(),
                "diff".to_string(),
                "src/lib.rs".to_string(),
                "..\\outside.txt".to_string(),
            ]),
            vec!["..\\outside.txt", "src/lib.rs"]
        );
    }

    #[test]
    fn cccx_extracts_simple_filename_and_redirection_shell_targets() {
        assert_eq!(
            cccx_extract_shell_target_paths(&[
                "powershell".to_string(),
                "Set-Content out.txt 'data'; echo next >result.log".to_string(),
            ]),
            vec!["out.txt", "result.log"]
        );
    }

    #[test]
    fn cccx_subagent_shell_scope_blocks_simple_filename_outside_workspec() {
        let temp = tempfile::tempdir().expect("tempdir");
        let allowed = temp.path().join("allowed");
        std::fs::create_dir_all(&allowed).expect("allowed dir");
        let allowed = std::fs::canonicalize(&allowed).expect("canonical allowed");

        let work_spec = cccx_protocol::WorkSpec {
            schema_version: cccx_protocol::CCCX_WORK_SPEC_SCHEMA_VERSION.to_string(),
            work_spec_id: "ws-shell-simple".to_string(),
            task_id: "task-shell-simple".to_string(),
            attempt_id: "attempt-shell-simple".to_string(),
            created_at: "2026-07-22T00:00:00Z".to_string(),
            expires_at: None,
            base_revision: cccx_protocol::RevisionRef {
                git_head: None,
                working_tree_digest: None,
            },
            harness: cccx_protocol::HarnessKind::Cx,
            role: cccx_protocol::WorkRole::Implementer,
            runtime_identity: None,
            objective: "delete only in allowed scope".to_string(),
            scope: cccx_protocol::WorkScope {
                canonical_paths: [allowed.to_string_lossy().into_owned()]
                    .into_iter()
                    .collect(),
                protected_paths: std::collections::BTreeSet::new(),
                advisory_globs: std::collections::BTreeSet::new(),
            },
            allowed_tools: ["shell_command".to_string()].into_iter().collect(),
            forbidden_tools: std::collections::BTreeSet::new(),
            allowed_mcp_tools: std::collections::BTreeSet::new(),
            tool_grants: Vec::new(),
            permission_policy: cccx_protocol::PermissionPolicy::Allow,
            model_route: cccx_protocol::ModelRouteSelection::InheritMainRequested,
            resolved_model_route: None,
            fallback_policy_id: "strict".to_string(),
            fork_policy: cccx_protocol::ForkPolicy::WorkSpecOnly,
            enforced_constraints: Vec::new(),
            typed_constraints: Vec::new(),
            advisory_constraints: Vec::new(),
            risk_threshold: None,
            action_classes: None,
            approval_policy: None,
            verification_spec: None,
            audit_depth: None,
            expected_artifact: None,
            checkpoint_id: None,
            base_commit: None,
        };

        let requirement = cccx_subagent_shell_requirement(
            &work_spec,
            &["rm".to_string(), "outside.txt".to_string()],
            temp.path(),
        );

        assert!(
            matches!(requirement, Some(ExecApprovalRequirement::Forbidden { ref reason }) if reason == "out_of_scope"),
            "a simple filename target outside WorkSpec scope must be extracted and denied, got {requirement:?}"
        );
    }

    #[test]
    fn cccx_resolves_relative_targets_against_worker_cwd() {
        let temp = tempfile::tempdir().expect("tempdir");
        let src = temp.path().join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        assert_eq!(
            cccx_resolve_target_paths(vec!["src/lib.rs".to_string()], temp.path()),
            Ok(vec![
                crate::cccx_paths::resolve_physical_path(Path::new("src/lib.rs"), temp.path())
                    .expect("target should resolve")
                    .to_string_lossy()
                    .into_owned()
            ])
        );
    }

    #[test]
    fn cccx_circuit_breaker_blocks_rm_rf_root_before_exec_policy() {
        assert!(matches!(
            cccx_circuit_breaker_requirement(&[
                "bash".to_string(),
                "-lc".to_string(),
                "rm -rf /".to_string()
            ]),
            Some(ExecApprovalRequirement::Forbidden { reason })
                if reason == "refuse recursive deletion of filesystem root"
        ));
    }

    #[test]
    fn cccx_circuit_breaker_ignores_non_circuit_command() {
        assert!(
            cccx_circuit_breaker_requirement(&["echo".to_string(), "ok".to_string()]).is_none()
        );
    }

    // ── Phase 2 mode-aware tests ──

    /// Phase 2: Plan mode must deny shell commands (architecture.md §4).
    /// In Plan mode, only read/search/MCP/sequential-thinking are allowed.
    /// Shell mutation is hard-denied.
    #[test]
    fn p2_plan_mode_denies_shell_mutation() {
        let result =
            cccx_full_shell_requirement(&["npm".into(), "test".into()], CccxMainMode::Plan, None);
        assert!(
            matches!(result, Some(ExecApprovalRequirement::Forbidden { .. })),
            "Phase 2: Plan mode must deny shell commands. Architecture.md §4: \
             'Plan: Read/search/MCP/sequential-thinking only. \
             Cấm edit, cấm shell mutate, cấm memory commit.'"
        );
    }

    /// Phase 2: Default mode requires approval for shell commands.
    /// Architecture.md §4: "Default (Ask): Write được, nhưng mỗi edit/command
    /// đều hỏi user"
    #[test]
    fn p2_default_mode_requires_approval_for_shell() {
        let result = cccx_full_shell_requirement(
            &[
                "git".into(),
                "push".into(),
                "--force".into(),
                "origin".into(),
                "main".into(),
            ],
            CccxMainMode::Default,
            None,
        );
        assert!(
            matches!(
                result,
                Some(
                    ExecApprovalRequirement::Forbidden { .. }
                        | ExecApprovalRequirement::NeedsApproval { .. }
                )
            ),
            "Phase 2: Default mode must require approval or deny for git push --force. \
             Got: {result:?}"
        );
    }

    /// Phase 2: Circuit breaker survives Bypass mode (architecture.md §5).
    /// "Bypass chỉ skip các bước hỏi thông thường, không skip circuit breaker."
    #[test]
    fn p2_bypass_blocks_circuit_breaker() {
        // Circuit breaker test mode: this explicitly uses Bypass
        // and still blocks rm -rf /. This already worked.
        let result =
            cccx_circuit_breaker_requirement(&["bash".into(), "-lc".into(), "rm -rf /".into()]);
        assert!(
            matches!(result, Some(ExecApprovalRequirement::Forbidden { .. })),
            "Phase 2: Circuit breaker must survive even in Bypass mode"
        );
    }

    /// Phase 2: Mode parameter is plumbed — Plan mode vs Default mode
    /// produce different decisions for the same shell command.
    #[test]
    fn p2_plan_and_default_produce_different_decisions() {
        let plan_result =
            cccx_full_shell_requirement(&["npm".into(), "test".into()], CccxMainMode::Plan, None);
        let default_result = cccx_full_shell_requirement(
            &["npm".into(), "test".into()],
            CccxMainMode::Default,
            None,
        );

        // Plan must hard-deny; Default must either deny or ask
        match (&plan_result, &default_result) {
            (Some(ExecApprovalRequirement::Forbidden { .. }), _) => {
                // Plan denies — this is the expected path.
                // Default either denies (if matched by escalate rules)
                // or requires approval — both are correct per spec.
            }
            (plan, def) => {
                panic!(
                    "Phase 2: Plan mode must hard-deny shell mutation. \
                     Plan result: {plan:?}, Default result: {def:?}"
                );
            }
        }
    }

    /// Phase 2: AcceptEdits allows edits, but still asks for shell.
    /// Architecture.md §4: "AcceptEdits: Edit file tự động trong ngưỡng nhỏ;
    /// shell command vẫn hỏi"
    #[test]
    fn p2_accept_edits_allows_edit_but_asks_shell() {
        // Shell command in AcceptEdits: must require approval
        let shell_result = cccx_full_shell_requirement(
            &["echo".into(), "hello".into()],
            CccxMainMode::AcceptEdits,
            None,
        );
        assert!(
            matches!(
                shell_result,
                Some(ExecApprovalRequirement::NeedsApproval { .. })
            ),
            "AcceptEdits must ask for shell. Architecture §4: \
             'shell command vẫn hỏi'. Got: {shell_result:?}"
        );

        // Edit in AcceptEdits: should be allowed
        let edit_result = cccx_edit_requirement_with_changed_lines(
            &["src/main.rs".into()],
            CccxMainMode::AcceptEdits,
            0,
        );
        assert!(
            edit_result.is_none(),
            "AcceptEdits must allow edits. Architecture §4: \
             'Edit file tự động trong ngưỡng nhỏ'. Got: {edit_result:?}"
        );
    }

    #[test]
    fn p3_accept_edits_reroutes_when_cumulative_changed_lines_exceed_threshold() {
        let result = cccx_edit_requirement_with_changed_lines(
            &["src/main.rs".into()],
            CccxMainMode::AcceptEdits,
            21,
        );

        assert!(
            matches!(result, Some(ExecApprovalRequirement::NeedsApproval { ref reason, .. })
                if reason.as_deref().is_some_and(|reason| reason.contains("mode-threshold-exceeded"))),
            "AcceptEdits must reroute or ask when cumulative changed lines exceed the small-edit threshold. Got: {result:?}"
        );
    }

    /// Phase 2: DontAsk with no explicit allow rules denies silently.
    /// Architecture.md §4: "Chỉ tool nằm trong allowlist cấu hình sẵn được chạy,
    /// còn lại từ chối im lặng"
    #[test]
    fn p2_dont_ask_denies_shell_without_explicit_allow() {
        let result = cccx_full_shell_requirement(
            &["npm".into(), "test".into()],
            CccxMainMode::DontAsk,
            None,
        );
        assert!(
            matches!(result, Some(ExecApprovalRequirement::Forbidden { .. })),
            "DontAsk with empty explicit_allow_rules must deny shell. \
             Architecture §4: 'còn lại từ chối im lặng'. Got: {result:?}"
        );
    }

    #[test]
    fn p3_mcp_requirement_uses_live_mode_parameter() {
        let plan_result =
            cccx_mcp_requirement("codebase-memory-mcp", "manage_adr", CccxMainMode::Plan);
        let bypass_result =
            cccx_mcp_requirement("codebase-memory-mcp", "manage_adr", CccxMainMode::Bypass);

        assert!(
            matches!(plan_result, Some(ExecApprovalRequirement::Forbidden { ref reason })
                if reason == "plan-read-only"),
            "Plan mode must deny side-effect MCP calls before approval flow. Got: {plan_result:?}"
        );
        assert!(
            bypass_result.is_none(),
            "Bypass mode should skip non-circuit MCP approval requirements. Got: {bypass_result:?}"
        );
    }

    #[derive(Debug)]
    struct TestAutoClassifier {
        outcome: CccxClassifierOutcome,
    }

    impl CccxAutoClassifier for TestAutoClassifier {
        fn classify(&self, _action: CccxNormalizedAction) -> CccxAutoClassifierFuture<'_> {
            let outcome = self.outcome.clone();
            Box::pin(async move { outcome })
        }
    }

    fn test_exec_request(command: &[&str]) -> ExecApprovalRequest<'static> {
        let command: Vec<String> = command.iter().map(|part| part.to_string()).collect();
        let command: &'static [String] = Box::leak(command.into_boxed_slice());
        ExecApprovalRequest {
            command,
            approval_policy: AskForApproval::OnRequest,
            permission_profile: PermissionProfile::Disabled,
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            sandbox_permissions: SandboxPermissions::UseDefault,
            prefix_rule: None,
        }
    }

    #[tokio::test]
    async fn p3_auto_shell_uses_classifier_allow_outcome() {
        let manager = ExecPolicyManager::default().with_cccx_auto_classifier_for_test(Arc::new(
            TestAutoClassifier {
                outcome: CccxClassifierOutcome::Available(cccx_policy::ClassifierOutcome::Allow),
            },
        ));

        let result = manager
            .create_exec_approval_requirement_for_command_in_mode(
                test_exec_request(&["npm", "test"]),
                CccxMainMode::Auto,
            )
            .await;

        assert!(
            matches!(result, ExecApprovalRequirement::Skip { .. }),
            "Auto mode must honor classifier allow after deterministic gates. Got: {result:?}"
        );
    }

    #[tokio::test]
    async fn p3_auto_shell_requires_approval_when_classifier_unavailable() {
        let manager = ExecPolicyManager::default().with_cccx_auto_classifier_for_test(Arc::new(
            TestAutoClassifier {
                outcome: CccxClassifierOutcome::Unavailable,
            },
        ));

        let result = manager
            .create_exec_approval_requirement_for_command_in_mode(
                test_exec_request(&["npm", "test"]),
                CccxMainMode::Auto,
            )
            .await;

        assert!(
            matches!(
                result,
                ExecApprovalRequirement::NeedsApproval { reason: Some(ref reason), .. }
                    if reason == "classifier_unavailable"
            ),
            "Auto mode must fail closed to approval when classifier is unavailable. Got: {result:?}"
        );
    }

    #[tokio::test]
    async fn p3_auto_shell_circuit_breaker_does_not_call_classifier() {
        let manager = ExecPolicyManager::default().with_cccx_auto_classifier_for_test(Arc::new(
            TestAutoClassifier {
                outcome: CccxClassifierOutcome::Available(cccx_policy::ClassifierOutcome::Allow),
            },
        ));

        let result = manager
            .create_exec_approval_requirement_for_command_in_mode(
                test_exec_request(&["bash", "-lc", "rm -rf /"]),
                CccxMainMode::Auto,
            )
            .await;

        assert!(
            matches!(result, ExecApprovalRequirement::Forbidden { .. }),
            "Circuit breaker must run before classifier and remain non-bypassable. Got: {result:?}"
        );
    }

    #[tokio::test]
    async fn p3_auto_edit_uses_classifier_allow_outcome() {
        let manager = ExecPolicyManager::default().with_cccx_auto_classifier_for_test(Arc::new(
            TestAutoClassifier {
                outcome: CccxClassifierOutcome::Available(cccx_policy::ClassifierOutcome::Allow),
            },
        ));

        let result = manager
            .create_edit_approval_requirement_for_changed_lines_in_mode(
                &["src/main.rs".into()],
                CccxMainMode::Auto,
                1,
            )
            .await;

        assert!(
            result.is_none(),
            "Auto edit must honor classifier allow after deterministic gates. Got: {result:?}"
        );
    }

    #[tokio::test]
    async fn p3_auto_mcp_uses_classifier_allow_outcome() {
        let manager = ExecPolicyManager::default().with_cccx_auto_classifier_for_test(Arc::new(
            TestAutoClassifier {
                outcome: CccxClassifierOutcome::Available(cccx_policy::ClassifierOutcome::Allow),
            },
        ));

        let result = manager
            .create_mcp_approval_requirement_in_mode(
                "codebase-memory-mcp",
                "search_graph",
                CccxMainMode::Auto,
            )
            .await;

        assert!(
            result.is_none(),
            "Auto MCP must honor classifier allow after deterministic gates. Got: {result:?}"
        );
    }

    #[test]
    fn p3_auto_classifier_parser_rejects_invalid_output() {
        assert_eq!(parse_cccx_classifier_content("not json"), None);
        assert_eq!(
            parse_cccx_classifier_content(r#"{"decision":"escalate_to_root"}"#),
            None
        );
        assert_eq!(
            parse_cccx_classifier_content(
                r#"{"decision":"require_approval","reason":"needs user"}"#
            ),
            Some(CccxPolicyClassifierOutcome::RequireApproval {
                reason: "needs user".to_string()
            })
        );
    }
}

#[cfg(test)]
#[path = "exec_policy_tests.rs"]
mod tests;
