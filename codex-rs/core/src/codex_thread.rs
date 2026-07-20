use crate::agent::AgentStatus;
use crate::agent::control::SpawnAgentOptions;
use crate::cccx_runtime::CccxRuntimeRegistry;
use crate::cccx_runtime::CccxSpawnRuntime;
use crate::cccx_runtime::parent_capability_set_for_work_spec;
use crate::config::ConstraintResult;
use crate::elicitation::ElicitationRegistration;
use crate::session::SessionIo;
use crate::session::SessionSettingsUpdate;
use crate::session::SteerInputError;
use crate::session::session::Session;
use codex_exec_server::SelectedCapabilityRootsStatus;
use codex_features::Feature;
use codex_otel::SessionTelemetry;
use codex_protocol::ThreadId;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::mcp::CallToolResult;
use codex_protocol::models::ActivePermissionProfile;
use codex_protocol::models::ContentItem;
use codex_protocol::models::PermissionProfile;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AdditionalContextEntry;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::MultiAgentVersion;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionConfiguredEvent;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::protocol::Submission;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_protocol::protocol::ThreadMemoryMode;
use codex_protocol::protocol::ThreadSource;
use codex_protocol::protocol::TokenUsageInfo;
use codex_protocol::protocol::TurnEnvironmentSelection;
use codex_protocol::protocol::TurnEnvironmentSelections;
use codex_protocol::protocol::W3cTraceContext;
use codex_protocol::user_input::UserInput;
use codex_thread_store::StoredThread;
use codex_thread_store::StoredThreadHistory;
use codex_thread_store::ThreadMetadataPatch;
use codex_thread_store::ThreadStoreError;
use codex_thread_store::ThreadStoreResult;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::LegacyAppPathString;
use codex_utils_path_uri::PathUri;
use rmcp::model::ReadResourceRequestParams;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::watch;

use codex_rollout::state_db::StateDbHandle;

#[derive(Clone, Debug)]
pub struct ThreadConfigSnapshot {
    pub model: String,
    pub model_provider_id: String,
    pub service_tier: Option<String>,
    pub approval_policy: AskForApproval,
    pub approvals_reviewer: ApprovalsReviewer,
    pub permission_profile: PermissionProfile,
    pub active_permission_profile: Option<ActivePermissionProfile>,
    pub environments: TurnEnvironmentSelections,
    pub workspace_roots: Vec<AbsolutePathBuf>,
    pub profile_workspace_roots: Vec<AbsolutePathBuf>,
    pub ephemeral: bool,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub reasoning_summary: Option<ReasoningSummary>,
    pub personality: Option<Personality>,
    pub collaboration_mode: CollaborationMode,
    pub session_source: SessionSource,
    pub history_mode: ThreadHistoryMode,
    pub forked_from_thread_id: Option<ThreadId>,
    pub parent_thread_id: Option<ThreadId>,
    pub thread_source: Option<ThreadSource>,
    pub originator: String,
}

/// Sanitized read-only projection of a persisted CCCX worker attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CccxWorkerView {
    pub logical_session_id: String,
    pub worker_incarnation_id: String,
    pub display_name: String,
    pub role: String,
    pub task_id: String,
    pub objective: String,
    pub status: String,
    pub model_route_summary: String,
}

/// Sanitized read-only log projection for a CCCX logical worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CccxWorkerLogEntry {
    pub logical_session_id: String,
    pub display_name: String,
    pub task_id: String,
    pub attempt_id: String,
    pub artifact_id: String,
    pub status: String,
    pub summary: String,
    pub changed_paths_count: usize,
    pub tests_run_count: usize,
    pub evidence_count: usize,
}

fn sanitize_cccx_worker_log_text(input: &str) -> String {
    const MAX_CHARS: usize = 512;
    let mut output = String::new();
    for ch in input.chars() {
        if output.chars().count() >= MAX_CHARS {
            output.push_str("...");
            break;
        }
        if ch.is_control() {
            output.push(' ');
        } else {
            output.push(ch);
        }
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Explains why `CodexThread::try_start_turn_if_idle` rejected an automatic
/// idle turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TryStartTurnIfIdleRejectionReason {
    /// User/client-triggered mailbox work is already queued and must take
    /// priority over extension-initiated idle work.
    PendingTriggerTurn,
    /// The thread is in Plan mode, where automatic idle work must not start a
    /// new model turn.
    PlanMode,
    /// Another turn or task is active, or the idle reservation was lost before
    /// the automatic turn could start.
    Busy,
}

/// Rejection returned when an extension asks to start automatic idle work but
/// the thread is not eligible to run it.
#[derive(Debug)]
pub struct TryStartTurnIfIdleError {
    reason: TryStartTurnIfIdleRejectionReason,
    input: Vec<ResponseItem>,
}

impl TryStartTurnIfIdleError {
    pub(crate) fn new(reason: TryStartTurnIfIdleRejectionReason, input: Vec<ResponseItem>) -> Self {
        Self { reason, input }
    }

    /// Returns the stable reason the automatic idle turn was rejected.
    pub fn reason(&self) -> TryStartTurnIfIdleRejectionReason {
        self.reason
    }

    /// Consumes the rejection and returns the original model-visible input
    /// unchanged, so callers can retry, drop, or log it explicitly.
    pub fn into_input(self) -> Vec<ResponseItem> {
        self.input
    }
}

impl ThreadConfigSnapshot {
    pub fn cwd(&self) -> &AbsolutePathBuf {
        &self.environments.legacy_fallback_cwd
    }

    pub fn environment_selections(&self) -> &[TurnEnvironmentSelection] {
        &self.environments.environments
    }

    pub fn sandbox_policy(&self) -> SandboxPolicy {
        codex_sandboxing::compatibility_sandbox_policy_for_permission_profile(
            &self.permission_profile,
            self.cwd().as_path(),
        )
    }
}

/// Thread settings overrides that app-server validates before starting a turn.
#[derive(Clone, Default)]
pub struct CodexThreadSettingsOverrides {
    pub environments: Option<TurnEnvironmentSelections>,
    pub profile_workspace_roots: Option<Vec<AbsolutePathBuf>>,
    pub approval_policy: Option<AskForApproval>,
    pub approvals_reviewer: Option<ApprovalsReviewer>,
    pub sandbox_policy: Option<SandboxPolicy>,
    pub permission_profile: Option<PermissionProfile>,
    pub active_permission_profile: Option<ActivePermissionProfile>,
    pub windows_sandbox_level: Option<WindowsSandboxLevel>,
    pub model: Option<String>,
    pub effort: Option<Option<ReasoningEffort>>,
    pub summary: Option<ReasoningSummary>,
    pub service_tier: Option<Option<String>>,
    pub collaboration_mode: Option<CollaborationMode>,
    pub personality: Option<Personality>,
}

pub struct CodexThread {
    pub(crate) session: Arc<Session>,
    pub(crate) io: SessionIo,
    pub(crate) session_source: SessionSource,
    session_configured: SessionConfiguredEvent,
    rollout_path: Option<PathBuf>,
    out_of_band_elicitations: Mutex<OutOfBandElicitations>,
}

#[derive(Default)]
struct OutOfBandElicitations {
    count: i64,
    registration: Option<ElicitationRegistration>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct BackgroundTerminalInfo {
    pub item_id: String,
    pub process_id: String,
    pub command: String,
    pub cwd: PathUri,
}

/// Conduit for the bidirectional stream of messages that compose a thread
/// (formerly called a conversation) in Codex.
impl CodexThread {
    /// List only CCCX-owned worker state persisted under this root project.
    pub async fn list_cccx_workers(&self) -> Result<Vec<CccxWorkerView>, String> {
        let config = self.config().await;
        let graph = crate::cccx_task_graph_state::load_task_graph(&config.cwd)
            .await
            .map_err(|error| format!("failed to load CCCX task graph: {error}"))?;
        let Some(graph) = graph else {
            return Ok(Vec::new());
        };
        let mut workers = graph
            .tasks
            .values()
            .flat_map(|task| {
                task.attempts.iter().map(move |attempt| CccxWorkerView {
                    logical_session_id: attempt.worker.logical_session_id.clone(),
                    worker_incarnation_id: attempt.attempt_id.clone(),
                    display_name: attempt.worker.display_name.clone(),
                    role: attempt.worker.role.clone(),
                    task_id: task.task_id.clone(),
                    objective: task.objective.clone(),
                    status: format!("{:?}", attempt.worker.status).to_lowercase(),
                    model_route_summary: attempt.worker.model_route_summary.clone(),
                })
            })
            .collect::<Vec<_>>();
        workers.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        Ok(workers)
    }

    /// Resolve a stable CCCX worker identity to its current native incarnation.
    /// This is root-scoped: callers cannot select an arbitrary native thread.
    pub async fn cccx_worker_thread_id(
        &self,
        worker_identifier: &str,
    ) -> Result<ThreadId, String> {
        let config = self.config().await;
        let graph = crate::cccx_task_graph_state::load_task_graph(&config.cwd)
            .await
            .map_err(|error| format!("failed to load CCCX task graph: {error}"))?
            .ok_or_else(|| "CCCX task graph is missing".to_string())?;
        let (_, attempt) = graph
            .find_worker_attempt(worker_identifier)
            .map_err(|error| error.to_string())?;
        ThreadId::from_string(&attempt.attempt_id)
            .map_err(|error| format!("CCCX worker incarnation is invalid: {error}"))
    }

    pub async fn cccx_worker_logs(
        &self,
        worker_identifier: &str,
        tail: usize,
    ) -> Result<Vec<CccxWorkerLogEntry>, String> {
        let config = self.config().await;
        let graph = crate::cccx_task_graph_state::load_task_graph(&config.cwd)
            .await
            .map_err(|error| format!("failed to load CCCX task graph: {error}"))?
            .ok_or_else(|| "CCCX task graph is missing".to_string())?;
        let tail = tail.clamp(1, 80);
        let matched_logical_id = {
            let logical_matches = graph
                .tasks
                .values()
                .flat_map(|task| task.attempts.iter())
                .filter(|attempt| attempt.worker.logical_session_id == worker_identifier)
                .map(|attempt| attempt.worker.logical_session_id.clone())
                .collect::<std::collections::BTreeSet<_>>();
            if logical_matches.len() == 1 {
                logical_matches.into_iter().next().unwrap_or_default()
            } else if logical_matches.len() > 1 {
                return Err(format!(
                    "ambiguous CCCX worker logicalSessionId `{worker_identifier}`"
                ));
            } else {
                let display_matches = graph
                    .tasks
                    .values()
                    .flat_map(|task| task.attempts.iter())
                    .filter(|attempt| attempt.worker.display_name == worker_identifier)
                    .map(|attempt| attempt.worker.logical_session_id.clone())
                    .collect::<std::collections::BTreeSet<_>>();
                match display_matches.len() {
                    0 => return Err(format!("unknown CCCX worker `{worker_identifier}`")),
                    1 => display_matches.into_iter().next().unwrap_or_default(),
                    _ => {
                        return Err(format!(
                            "ambiguous CCCX worker display name `{worker_identifier}`; use logicalSessionId"
                        ));
                    }
                }
            }
        };

        let mut entries = Vec::new();
        for task in graph.tasks.values() {
            for attempt in &task.attempts {
                if attempt.worker.logical_session_id != matched_logical_id {
                    continue;
                }
                for artifact_id in &attempt.artifact_ids {
                    let Some(artifact) = graph.artifacts.get(artifact_id) else {
                        continue;
                    };
                    entries.push(CccxWorkerLogEntry {
                        logical_session_id: attempt.worker.logical_session_id.clone(),
                        display_name: attempt.worker.display_name.clone(),
                        task_id: task.task_id.clone(),
                        attempt_id: attempt.attempt_id.clone(),
                        artifact_id: artifact.artifact_id.clone(),
                        status: format!("{:?}", artifact.status).to_lowercase(),
                        summary: sanitize_cccx_worker_log_text(&artifact.summary),
                        changed_paths_count: artifact.changed_paths.len(),
                        tests_run_count: artifact.tests_run.len(),
                        evidence_count: artifact.evidence.len(),
                    });
                }
            }
        }
        let start = entries.len().saturating_sub(tail);
        Ok(entries.split_off(start))
    }

    pub async fn set_cccx_agent_model_route(
        &self,
        preset: &str,
        route: &str,
    ) -> Result<(), String> {
        let config = self.config().await;
        crate::cccx_agent_model_routes::set_agent_model_route(&config.codex_home, preset, route)
            .await
            .map_err(|error| error.to_string())
    }

    /// Resume a CCCX worker as a fresh native incarnation while preserving its
    /// logical identity and the exact checkpointed WorkSpec.
    pub async fn resume_cccx_worker(
        &self,
        worker_identifier: &str,
        revalidate: bool,
    ) -> Result<ThreadId, String> {
        let mut config = (*self.config().await).clone();
        let current_head = codex_git_utils::get_head_commit_hash(&config.cwd)
            .await
            .map(|sha| sha.0)
            .unwrap_or_default();
        let current_working_tree_digest = cccx_current_working_tree_digest(config.cwd.as_path()).await;
        let checkpoint = CccxRuntimeRegistry::prepare_resume(
            &config.cwd,
            worker_identifier,
            &current_head,
            current_working_tree_digest.as_deref(),
            60,
            revalidate,
        )
        .await?;
        let work_spec = checkpoint
            .work_spec
            .clone()
            .ok_or_else(|| "checkpoint is missing the pinned WorkSpec".to_string())?;
        let display_name = checkpoint
            .display_name
            .clone()
            .ok_or_else(|| "checkpoint is missing the worker display name".to_string())?;
        let model_route_summary = config
            .model
            .clone()
            .unwrap_or_else(|| "inherit-main".to_string());
        let parent = parent_capability_set_for_work_spec(
            &config.cwd,
            &model_route_summary,
            &work_spec,
            work_spec.allowed_tools.clone(),
            work_spec.allowed_mcp_tools.clone(),
        );
        crate::cccx_spawn::apply_cccx_harness_to_config(&mut config, &parent, &work_spec)
            .map_err(|error| error.to_string())?;
        let resume_input = format!(
            "Resume the existing CCCX worker. Continue from checkpoint only; do not expand scope.\n\
             Last completed step: {}\nPending steps: {}\nPartial diff: {}",
            checkpoint.last_completed_step,
            checkpoint.pending_steps.join("; "),
            checkpoint.partial_diff,
        );
        let source = SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id: self.session.thread_id,
            depth: 1,
            agent_path: None,
            agent_nickname: None,
            agent_role: Some(format!("{:?}", work_spec.role)),
        });
        let runtime = Arc::new(CccxSpawnRuntime {
            project_root: config.cwd.to_path_buf(),
            work_spec,
            logical_session_id: checkpoint.logical_session_id.clone(),
            display_name,
            model_route_summary,
            gateway_provenance: None,
        });
        let spawned = self
            .session
            .services
            .agent_control
            .spawn_agent_with_metadata(
                config,
                vec![UserInput::Text {
                    text: resume_input,
                    text_elements: Vec::new(),
                }],
                Some(source),
                SpawnAgentOptions {
                    parent_thread_id: Some(self.session.thread_id),
                    cccx_runtime: Some(runtime),
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(spawned.thread_id)
    }

    /// Resolve a queued CCCX permission request through its root-owned
    /// control plane. The request itself chooses the target worker; callers
    /// cannot provide an arbitrary approval id or thread id.
    pub async fn resolve_cccx_permission(
        &self,
        request_id: &str,
        approve: bool,
    ) -> Result<(), String> {
        self.session
            .services
            .agent_control
            .resolve_cccx_permission(request_id, approve)
            .await
    }

    /// Read the CCCX permission queue shared by this root's worker tree.
    pub async fn list_cccx_permissions(&self) -> Vec<cccx_policy::PermissionRequest> {
        self.session
            .services
            .agent_control
            .list_cccx_permissions()
            .await
    }

    /// Start a sequential batch for safe CCCX permission requests.
    pub async fn begin_cccx_safe_permission_batch(&self) -> Result<bool, String> {
        self.session
            .services
            .agent_control
            .begin_cccx_safe_batch()
            .await
    }

    pub(crate) fn new(
        session: Arc<Session>,
        io: SessionIo,
        session_configured: SessionConfiguredEvent,
        rollout_path: Option<PathBuf>,
        session_source: SessionSource,
    ) -> Self {
        Self {
            session,
            io,
            session_source,
            session_configured,
            rollout_path,
            out_of_band_elicitations: Mutex::new(OutOfBandElicitations::default()),
        }
    }

    pub async fn submit(&self, op: Op) -> CodexResult<String> {
        self.io.submit(op).await
    }

    /// Returns the session telemetry handle for thread-scoped production instrumentation.
    pub fn session_telemetry(&self) -> SessionTelemetry {
        self.session.services.session_telemetry.clone()
    }

    pub async fn shutdown_and_wait(&self) -> CodexResult<()> {
        self.io.shutdown_and_wait().await
    }

    /// Wait until the underlying session loop has terminated.
    pub async fn wait_until_terminated(&self) {
        self.io.session_loop_termination.clone().await;
    }

    pub(crate) async fn emit_thread_resume_lifecycle(&self) {
        for contributor in self
            .session
            .services
            .extensions
            .thread_lifecycle_contributors()
        {
            contributor
                .on_thread_resume(codex_extension_api::ThreadResumeInput {
                    session_store: &self.session.services.session_extension_data,
                    thread_store: &self.session.services.thread_extension_data,
                })
                .await;
        }
    }

    pub async fn emit_thread_idle_lifecycle_if_idle(&self) {
        self.session.emit_thread_idle_lifecycle_if_idle().await;
    }

    #[doc(hidden)]
    pub async fn ensure_rollout_materialized(&self) {
        self.session.ensure_rollout_materialized().await;
    }

    #[doc(hidden)]
    pub async fn flush_rollout(&self) -> std::io::Result<()> {
        self.session.flush_rollout().await
    }

    pub async fn submit_with_trace(
        &self,
        op: Op,
        trace: Option<W3cTraceContext>,
    ) -> CodexResult<String> {
        self.io.submit_with_trace(op, trace).await
    }

    pub async fn submit_user_input_with_client_user_message_id(
        &self,
        op: Op,
        trace: Option<W3cTraceContext>,
        client_user_message_id: Option<String>,
    ) -> CodexResult<String> {
        self.session
            .services
            .agent_control
            .ensure_execution_capacity_for_op(self.session.thread_id(), &op)
            .await?;
        self.io
            .submit_user_input_with_client_user_message_id(op, trace, client_user_message_id)
            .await
    }

    /// Persist whether this thread is eligible for future memory generation.
    pub async fn set_thread_memory_mode(&self, mode: ThreadMemoryMode) -> anyhow::Result<()> {
        self.session.set_thread_memory_mode(mode).await
    }

    pub async fn steer_input(
        &self,
        input: Vec<UserInput>,
        additional_context: BTreeMap<String, AdditionalContextEntry>,
        expected_turn_id: Option<&str>,
        client_user_message_id: Option<String>,
        responsesapi_client_metadata: Option<HashMap<String, String>>,
    ) -> Result<String, SteerInputError> {
        self.session
            .steer_input(
                input,
                additional_context,
                expected_turn_id,
                client_user_message_id,
                responsesapi_client_metadata,
            )
            .await
    }

    /// Injects model-visible items into the currently active turn.
    ///
    /// This is the thread-level bridge to `Session::inject_if_running` for
    /// callers that only hold a `CodexThread`.
    /// It returns the unchanged items when this thread has no active turn.
    pub async fn inject_if_running(
        &self,
        items: Vec<ResponseItem>,
    ) -> Result<(), Vec<ResponseItem>> {
        self.session.inject_if_running(items).await
    }

    /// Starts an automatic regular turn with model-visible items only when idle
    /// work is allowed for this thread.
    ///
    /// This is the required entry point for extensions that want to launch
    /// model-visible work from `ThreadLifecycleContributor::on_thread_idle`.
    /// The call succeeds only if no user/client-triggered turn is queued, no
    /// task is currently active, and the thread is not in Plan mode. Active
    /// Review tasks are rejected by the active-task check because Review turns
    /// are not steerable.
    ///
    /// On rejection, the returned error includes a stable reason and carries
    /// the original `items` unchanged so the caller can decide whether to drop
    /// them, retry later, or log why no automatic turn was started.
    pub async fn try_start_turn_if_idle(
        &self,
        items: Vec<ResponseItem>,
    ) -> Result<(), TryStartTurnIfIdleError> {
        self.session.try_start_turn_if_idle(items).await
    }

    pub async fn set_app_server_client_info(
        &self,
        app_server_client_name: Option<String>,
        app_server_client_version: Option<String>,
        mcp_elicitations_auto_deny: bool,
    ) -> ConstraintResult<()> {
        self.session
            .set_app_server_client_info(
                app_server_client_name,
                app_server_client_version,
                mcp_elicitations_auto_deny,
            )
            .await
    }

    pub async fn set_openai_form_elicitation_support(&self, supported: bool) -> anyhow::Result<()> {
        self.session
            .set_openai_form_elicitation_support(supported)
            .await
    }

    /// Preview persistent thread settings overrides without committing them.
    pub async fn preview_thread_settings_overrides(
        &self,
        overrides: CodexThreadSettingsOverrides,
    ) -> ConstraintResult<ThreadConfigSnapshot> {
        let updates = self.thread_settings_update(overrides).await;
        self.session.preview_settings(&updates).await
    }

    async fn thread_settings_update(
        &self,
        overrides: CodexThreadSettingsOverrides,
    ) -> SessionSettingsUpdate {
        let CodexThreadSettingsOverrides {
            environments,
            profile_workspace_roots,
            approval_policy,
            approvals_reviewer,
            sandbox_policy,
            permission_profile,
            active_permission_profile,
            windows_sandbox_level,
            model,
            effort,
            summary,
            service_tier,
            collaboration_mode,
            personality,
        } = overrides;
        let collaboration_mode = if let Some(collaboration_mode) = collaboration_mode {
            collaboration_mode
        } else {
            self.session
                .collaboration_mode()
                .await
                .with_updates(model, effort, /*developer_instructions*/ None)
        };

        SessionSettingsUpdate {
            environments,
            profile_workspace_roots,
            approval_policy,
            approvals_reviewer,
            sandbox_policy,
            permission_profile,
            active_permission_profile,
            windows_sandbox_level,
            collaboration_mode: Some(collaboration_mode),
            reasoning_summary: summary,
            service_tier,
            personality,
            ..Default::default()
        }
    }

    /// Use sparingly: this is intended to be removed soon.
    pub async fn submit_with_id(&self, sub: Submission) -> CodexResult<()> {
        self.io.submit_with_id(sub).await
    }

    pub async fn next_event(&self) -> CodexResult<Event> {
        self.io.next_event().await
    }

    pub async fn agent_status(&self) -> AgentStatus {
        self.io.agent_status().await
    }

    pub async fn list_background_terminals(&self) -> Vec<BackgroundTerminalInfo> {
        self.session.list_background_terminals().await
    }

    pub async fn terminate_background_terminal(&self, process_id: i32) -> bool {
        self.session.terminate_background_terminal(process_id).await
    }

    pub(crate) fn subscribe_status(&self) -> watch::Receiver<AgentStatus> {
        self.io.agent_status.clone()
    }

    /// Returns the complete token usage snapshot currently cached for this thread.
    ///
    /// This accessor is intentionally narrower than direct session access: it lets
    /// app-server lifecycle paths replay restored usage after resume or fork without
    /// exposing broader session mutation authority. A caller that only reads
    /// `total_token_usage` would drop last-turn usage and make the v2
    /// `thread/tokenUsage/updated` payload incomplete.
    pub async fn token_usage_info(&self) -> Option<TokenUsageInfo> {
        self.session.token_usage_info().await
    }

    /// Records a user-role session-prefix message without creating a new user turn boundary.
    pub(crate) async fn inject_user_message_without_turn(&self, message: String) {
        let item = ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText { text: message }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        };
        self.session
            .inject_no_new_turn(vec![item], /*current_turn_context*/ None)
            .await;
    }

    /// Record raw Responses API items without starting a new turn.
    pub async fn inject_response_items(&self, items: Vec<ResponseItem>) -> CodexResult<()> {
        if items.is_empty() {
            return Err(CodexErr::InvalidRequest(
                "items must not be empty".to_string(),
            ));
        }

        let turn_context = self.session.new_default_turn().await;
        if self.session.reference_context_item().await.is_none() {
            // This history-only API runs without run_turn, so it owns its initial step.
            let step_context = self
                .session
                .capture_step_context(Arc::clone(&turn_context))
                .await;
            self.session
                .record_context_updates_and_set_reference_context_item(step_context.as_ref())
                .await;
        }
        self.session
            .inject_no_new_turn(items, Some(turn_context.as_ref()))
            .await;
        self.session.flush_rollout().await?;
        Ok(())
    }

    pub fn rollout_path(&self) -> Option<PathBuf> {
        self.rollout_path.clone()
    }

    pub fn session_configured(&self) -> SessionConfiguredEvent {
        self.session_configured.clone()
    }

    pub(crate) fn is_running(&self) -> bool {
        !self.io.tx_sub.is_closed()
    }

    pub async fn guardian_trunk_rollout_path(&self) -> Option<PathBuf> {
        self.session
            .guardian_review_session
            .trunk_rollout_path()
            .await
    }

    pub async fn load_history(
        &self,
        include_archived: bool,
    ) -> ThreadStoreResult<StoredThreadHistory> {
        let live_thread = self
            .session
            .live_thread_for_persistence("load history")
            .map_err(|err| ThreadStoreError::Internal {
                message: err.to_string(),
            })?;
        live_thread.load_history(include_archived).await
    }

    pub async fn read_thread(
        &self,
        include_archived: bool,
        include_history: bool,
    ) -> ThreadStoreResult<StoredThread> {
        let live_thread = self
            .session
            .live_thread_for_persistence("read thread")
            .map_err(|err| ThreadStoreError::Internal {
                message: err.to_string(),
            })?;
        live_thread
            .read_thread(include_archived, include_history)
            .await
    }

    pub async fn update_thread_metadata(
        &self,
        patch: ThreadMetadataPatch,
        include_archived: bool,
    ) -> ThreadStoreResult<StoredThread> {
        let live_thread = self
            .session
            .live_thread_for_persistence("update thread metadata")
            .map_err(|err| ThreadStoreError::Internal {
                message: err.to_string(),
            })?;
        live_thread.update_metadata(patch, include_archived).await
    }

    /// Appends rollout items through the live thread so derived metadata stays in sync.
    pub async fn append_rollout_items(&self, items: &[RolloutItem]) -> ThreadStoreResult<()> {
        let live_thread = self
            .session
            .live_thread_for_persistence("append rollout items")
            .map_err(|err| ThreadStoreError::Internal {
                message: err.to_string(),
            })?;
        live_thread.append_items(items).await
    }

    pub fn state_db(&self) -> Option<StateDbHandle> {
        self.session.state_db()
    }

    pub async fn config_snapshot(&self) -> ThreadConfigSnapshot {
        self.session.thread_config_snapshot().await
    }

    /// Returns the files that supplied the thread's loaded model instructions.
    pub async fn instruction_sources(&self) -> Vec<PathUri> {
        self.session.instruction_sources().await
    }

    /// Returns loaded instruction sources rendered as legacy app-server path strings.
    pub async fn legacy_instruction_sources(&self) -> Vec<LegacyAppPathString> {
        self.instruction_sources()
            .await
            .into_iter()
            .map(Into::into)
            .collect()
    }

    pub async fn config(&self) -> Arc<crate::config::Config> {
        self.session.get_config().await
    }

    /// Resolves the MCP runtime configuration using this thread's extension data.
    pub async fn runtime_mcp_config(&self, config: &crate::config::Config) -> codex_mcp::McpConfig {
        self.session.runtime_mcp_config(config).await
    }

    /// Returns the exact MCP config, environment bindings, and manager most recently published.
    pub async fn current_mcp_runtime(&self) -> Arc<crate::session::McpRuntimeSnapshot> {
        let turn_context = self.session.new_default_turn().await;
        self.session
            .capture_step_context(turn_context)
            .await
            .mcp
            .clone()
    }

    pub fn multi_agent_version(&self) -> Option<MultiAgentVersion> {
        self.session.multi_agent_version()
    }

    /// Refresh the thread's layer-backed user config state from a caller-supplied
    /// config snapshot. Thread-scoped layers and session-static settings remain
    /// unchanged.
    pub async fn refresh_runtime_config(&self, next_config: crate::config::Config) {
        self.session.refresh_runtime_config(next_config).await;
    }

    pub async fn environment_selections(&self) -> Vec<TurnEnvironmentSelection> {
        self.session.thread_environment_selections().await
    }

    /// Passively inspects the selected capability roots whose environments are ready now.
    pub fn inspect_selected_capability_roots(&self) -> SelectedCapabilityRootsStatus {
        self.session
            .services
            .turn_environments
            .environment_manager()
            .inspect_selected_capability_roots(&self.session.services.selected_capability_roots)
    }

    pub async fn read_mcp_resource(
        &self,
        server: &str,
        uri: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let result = self
            .current_mcp_runtime()
            .await
            .manager_arc()
            .read_resource(server, ReadResourceRequestParams::new(uri))
            .await?;

        Ok(serde_json::to_value(result)?)
    }

    pub async fn call_mcp_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: Option<serde_json::Value>,
        meta: Option<serde_json::Value>,
    ) -> anyhow::Result<CallToolResult> {
        self.current_mcp_runtime()
            .await
            .manager_arc()
            .call_tool(server, tool, arguments, meta)
            .await
    }

    pub fn enabled(&self, feature: Feature) -> bool {
        self.session.enabled(feature)
    }

    pub async fn increment_out_of_band_elicitation_count(&self) -> CodexResult<i64> {
        let mut elicitations = self.out_of_band_elicitations.lock().await;
        let incremented = elicitations.count.checked_add(1).ok_or_else(|| {
            CodexErr::Fatal("out-of-band elicitation count overflowed".to_string())
        })?;
        if elicitations.count == 0 {
            elicitations.registration = Some(self.session.services.elicitations.register());
        }
        elicitations.count = incremented;
        Ok(incremented)
    }

    pub async fn decrement_out_of_band_elicitation_count(&self) -> CodexResult<i64> {
        let mut elicitations = self.out_of_band_elicitations.lock().await;
        if elicitations.count == 0 {
            return Err(CodexErr::InvalidRequest(
                "out-of-band elicitation count is already zero".to_string(),
            ));
        }

        elicitations.count -= 1;
        if elicitations.count == 0 {
            elicitations.registration = None;
        }
        Ok(elicitations.count)
    }
}

async fn cccx_current_working_tree_digest(cwd: &std::path::Path) -> Option<String> {
    use sha2::Digest;
    let diff = tokio::process::Command::new("git")
        .arg("diff")
        .arg("--no-ext-diff")
        .arg("--binary")
        .current_dir(cwd)
        .output()
        .await
        .ok()?;
    if !diff.status.success() {
        return None;
    }
    let status = tokio::process::Command::new("git")
        .arg("status")
        .arg("--porcelain=v1")
        .arg("-z")
        .current_dir(cwd)
        .output()
        .await
        .ok()?;
    if !status.status.success() {
        return None;
    }
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"git-diff-binary\0");
    hasher.update(&diff.stdout);
    hasher.update(b"\0git-status-porcelain-z\0");
    hasher.update(&status.stdout);
    Some(format!("sha256:{:x}", hasher.finalize()))
}
