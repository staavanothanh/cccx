use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use cccx_headless::AgentUpdatePayload;
use cccx_headless::AnswerPayload;
use cccx_headless::ApprovalDeniedPayload;
use cccx_headless::ErrorPayload;
use cccx_headless::HeadlessEvent;
use cccx_headless::HeadlessEventEnvelope;
use cccx_headless::ModelRoutePayload;
use cccx_headless::PermissionRequestPayload;
use cccx_headless::TaskUpdatePayload;
use cccx_headless::TerminalPayload;
use cccx_headless::TerminalStatus;
use codex_app_server_protocol::CollabAgentTool;
use codex_app_server_protocol::CollabAgentToolCallStatus;
use codex_app_server_protocol::CommandExecutionStatus;
use codex_app_server_protocol::McpToolCallStatus;
use codex_app_server_protocol::PatchApplyStatus;
use codex_app_server_protocol::PatchChangeKind;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadTokenUsage;
use codex_app_server_protocol::TurnStatus;
use codex_core::config::Config;
use codex_protocol::models::WebSearchAction;
use codex_protocol::protocol::SessionConfiguredEvent;
use serde_json::json;

pub use crate::event_processor::CodexStatus;
use crate::event_processor::EventProcessor;
use crate::event_processor::handle_last_message;
use crate::exec_events::AgentMessageItem;
use crate::exec_events::AgentUpdateEvent;
use crate::exec_events::CollabAgentState;
use crate::exec_events::CollabAgentStatus;
use crate::exec_events::CollabTool;
use crate::exec_events::CollabToolCallItem;
use crate::exec_events::CollabToolCallStatus;
use crate::exec_events::CommandExecutionItem;
use crate::exec_events::CommandExecutionStatus as ExecCommandExecutionStatus;
use crate::exec_events::ErrorItem;
use crate::exec_events::FileChangeItem;
use crate::exec_events::FileUpdateChange;
use crate::exec_events::ItemCompletedEvent;
use crate::exec_events::ItemStartedEvent;
use crate::exec_events::ItemUpdatedEvent;
use crate::exec_events::McpToolCallItem;
use crate::exec_events::McpToolCallItemError;
use crate::exec_events::McpToolCallItemResult;
use crate::exec_events::McpToolCallStatus as ExecMcpToolCallStatus;
use crate::exec_events::PatchApplyStatus as ExecPatchApplyStatus;
use crate::exec_events::PatchChangeKind as ExecPatchChangeKind;
use crate::exec_events::ReasoningItem;
use crate::exec_events::ThreadErrorEvent;
use crate::exec_events::ThreadEvent;
use crate::exec_events::ThreadItem as ExecThreadItem;
use crate::exec_events::ThreadItemDetails;
use crate::exec_events::ThreadStartedEvent;
use crate::exec_events::TodoItem;
use crate::exec_events::TodoListItem;
use crate::exec_events::TurnCompletedEvent;
use crate::exec_events::TurnFailedEvent;
use crate::exec_events::TurnStartedEvent;
use crate::exec_events::Usage;
use crate::exec_events::WebSearchItem;

pub struct EventProcessorWithJsonOutput {
    last_message_path: Option<PathBuf>,
    next_item_id: AtomicU64,
    raw_to_exec_item_id: HashMap<String, String>,
    running_todo_list: Option<RunningTodoList>,
    last_total_token_usage: Option<ThreadTokenUsage>,
    last_critical_error: Option<ThreadErrorEvent>,
    final_message: Option<String>,
    emit_final_message_on_shutdown: bool,
    // cccx-headless fields (Phase 11)
    cccx_seq: AtomicU64,
    cccx_session_id: Option<String>,
    cccx_turn_id: Option<String>,
    cccx_exit_code: i32,
}

#[derive(Debug, Clone)]
struct RunningTodoList {
    item_id: String,
    items: Vec<TodoItem>,
}

#[derive(Debug, PartialEq)]
pub struct CollectedThreadEvents {
    pub events: Vec<ThreadEvent>,
    pub status: CodexStatus,
}

impl EventProcessorWithJsonOutput {
    pub fn new(last_message_path: Option<PathBuf>) -> Self {
        Self {
            last_message_path,
            next_item_id: AtomicU64::new(0),
            raw_to_exec_item_id: HashMap::new(),
            running_todo_list: None,
            last_total_token_usage: None,
            last_critical_error: None,
            final_message: None,
            emit_final_message_on_shutdown: false,
            cccx_seq: AtomicU64::new(0),
            cccx_session_id: None,
            cccx_turn_id: None,
            cccx_exit_code: 0,
        }
    }

    pub fn final_message(&self) -> Option<&str> {
        self.final_message.as_deref()
    }

    fn next_item_id(&self) -> String {
        format!("item_{}", self.next_item_id.fetch_add(1, Ordering::SeqCst))
    }

    #[allow(clippy::print_stdout)]
    fn emit(&self, thread_event: ThreadEvent) {
        for envelope in self.map_thread_event_to_envelopes(thread_event) {
            let line = serde_json::to_string(&envelope).unwrap_or_else(|err| {
                json!({
                    "schemaVersion": cccx_headless::CCCX_HEADLESS_EVENT_SCHEMA_VERSION,
                    "type": "error",
                    "payload": {
                        "message": format!("failed to serialize headless event: {err}"),
                        "recoverable": false,
                    },
                })
                .to_string()
            });
            debug_assert!(cccx_headless::assert_stdout_lines_are_json(&line).is_ok());
            println!("{line}");
        }
    }

    fn map_thread_event_to_envelopes(&self, event: ThreadEvent) -> Vec<HeadlessEventEnvelope> {
        let headless_events: Vec<HeadlessEvent> = match event {
            ThreadEvent::ItemStarted(ref item) => {
                self.map_started_or_updated_item_to_headless_events(&item.item, "running")
            }
            ThreadEvent::ItemUpdated(ref item) => {
                self.map_started_or_updated_item_to_headless_events(&item.item, "updated")
            }
            ThreadEvent::ItemCompleted(ref item) => match &item.item.details {
                ThreadItemDetails::AgentMessage(msg) => {
                    vec![HeadlessEvent::Answer(AnswerPayload {
                        text: msg.text.clone(),
                    })]
                }
                ThreadItemDetails::Error(err) => vec![HeadlessEvent::Error(ErrorPayload {
                    message: err.message.clone(),
                    recoverable: true,
                })],
                ThreadItemDetails::TodoList(_) => {
                    vec![HeadlessEvent::TaskUpdate(TaskUpdatePayload {
                        task_id: item.item.id.clone(),
                        status: "completed".to_string(),
                        verified: true,
                    })]
                }
                ThreadItemDetails::CollabToolCall(_) => Vec::new(),
                _ => Vec::new(),
            },
            ThreadEvent::Error(ref err) => vec![HeadlessEvent::Error(ErrorPayload {
                message: err.message.clone(),
                recoverable: false,
            })],
            ThreadEvent::AgentUpdate(ref update) => {
                vec![HeadlessEvent::AgentUpdate(Self::agent_update_payload(
                    update,
                ))]
            }
            // ThreadStarted, TurnStarted, TurnCompleted, TurnFailed, and
            // non-mapped items are execution details; terminal is emitted
            // once during shutdown.
            _ => Vec::new(),
        };

        headless_events
            .into_iter()
            .map(|event| self.envelope(event))
            .collect()
    }

    fn map_started_or_updated_item_to_headless_events(
        &self,
        item: &ExecThreadItem,
        status: &str,
    ) -> Vec<HeadlessEvent> {
        match &item.details {
            ThreadItemDetails::TodoList(_) => vec![HeadlessEvent::TaskUpdate(TaskUpdatePayload {
                task_id: item.id.clone(),
                status: status.to_string(),
                verified: false,
            })],
            ThreadItemDetails::CollabToolCall(_) => Vec::new(),
            _ => Vec::new(),
        }
    }

    fn agent_update_payload(update: &AgentUpdateEvent) -> AgentUpdatePayload {
        AgentUpdatePayload {
            logical_session_id: update.logical_session_id.clone(),
            display_name: update.display_name.clone(),
            role: update.role.clone(),
            status: update.status.clone(),
            model_route: ModelRoutePayload {
                selection_source: update.model_route.selection_source.clone(),
                requested_route_id: update.model_route.requested_route_id.clone(),
                effective_route_id: update.model_route.effective_route_id.clone(),
                fallback_chain: update.model_route.fallback_chain.clone(),
            },
        }
    }

    fn envelope(&self, event: HeadlessEvent) -> HeadlessEventEnvelope {
        HeadlessEventEnvelope {
            schema_version: cccx_headless::CCCX_HEADLESS_EVENT_SCHEMA_VERSION.to_string(),
            seq: self.cccx_seq.fetch_add(1, Ordering::SeqCst) + 1,
            session_id: self
                .cccx_session_id
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            turn_id: self
                .cccx_turn_id
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            event,
        }
    }

    fn approval_denied_envelope(&self, request_id: String) -> HeadlessEventEnvelope {
        self.envelope(HeadlessEvent::ApprovalDenied(ApprovalDeniedPayload {
            request_id,
            reason: "headless approval default is deny".to_string(),
        }))
    }

    fn permission_request_envelope(
        &self,
        request_id: String,
        agent_display_name: String,
        action: String,
        scope: String,
        risk: String,
    ) -> HeadlessEventEnvelope {
        self.envelope(HeadlessEvent::PermissionRequest(PermissionRequestPayload {
            request_id,
            agent_display_name,
            action,
            scope,
            risk,
        }))
    }

    fn terminal_event_envelope(&self) -> HeadlessEventEnvelope {
        let status = match self.cccx_exit_code {
            0 => TerminalStatus::Success,
            3 => TerminalStatus::Cancelled,
            _ => TerminalStatus::Error,
        };
        let exit_code = Some(self.cccx_exit_code);

        self.envelope(HeadlessEvent::Terminal(TerminalPayload {
            status,
            exit_code,
        }))
    }

    /// Emit a cccx-headless Terminal event on shutdown.
    /// Called from print_final_output after the exec loop completes.
    #[allow(clippy::print_stdout)]
    fn emit_terminal_event(&self) {
        let envelope = self.terminal_event_envelope();
        let line = serde_json::to_string(&envelope).unwrap_or_else(|err| {
            json!({
                "schemaVersion": cccx_headless::CCCX_HEADLESS_EVENT_SCHEMA_VERSION,
                "type": "error",
                "payload": {
                    "message": format!("failed to serialize terminal event: {err}"),
                    "recoverable": false,
                },
            })
            .to_string()
        });
        debug_assert!(cccx_headless::assert_stdout_lines_are_json(&line).is_ok());
        println!("{line}");
    }

    fn usage_from_last_total(&self) -> Usage {
        let Some(usage) = self.last_total_token_usage.as_ref() else {
            return Usage::default();
        };
        Usage {
            input_tokens: usage.total.input_tokens,
            cached_input_tokens: usage.total.cached_input_tokens,
            cache_write_input_tokens: usage.total.cache_write_input_tokens,
            output_tokens: usage.total.output_tokens,
            reasoning_output_tokens: usage.total.reasoning_output_tokens,
        }
    }

    pub fn map_todo_items(plan: &[codex_app_server_protocol::TurnPlanStep]) -> Vec<TodoItem> {
        plan.iter()
            .map(|step| TodoItem {
                text: step.step.clone(),
                completed: matches!(
                    step.status,
                    codex_app_server_protocol::TurnPlanStepStatus::Completed
                ),
            })
            .collect()
    }

    fn map_item_with_id(
        item: ThreadItem,
        make_id: impl FnOnce() -> String,
    ) -> Option<ExecThreadItem> {
        match item {
            ThreadItem::AgentMessage { text, .. } => Some(ExecThreadItem {
                id: make_id(),
                details: ThreadItemDetails::AgentMessage(AgentMessageItem { text }),
            }),
            ThreadItem::Reasoning { summary, .. } => {
                let text = summary.join("\n");
                if text.trim().is_empty() {
                    return None;
                }
                Some(ExecThreadItem {
                    id: make_id(),
                    details: ThreadItemDetails::Reasoning(ReasoningItem { text }),
                })
            }
            ThreadItem::CommandExecution {
                command,
                aggregated_output,
                exit_code,
                status,
                ..
            } => Some(ExecThreadItem {
                id: make_id(),
                details: ThreadItemDetails::CommandExecution(CommandExecutionItem {
                    command,
                    aggregated_output: aggregated_output.unwrap_or_default(),
                    exit_code,
                    status: match status {
                        CommandExecutionStatus::InProgress => ExecCommandExecutionStatus::InProgress,
                        CommandExecutionStatus::Completed => ExecCommandExecutionStatus::Completed,
                        CommandExecutionStatus::Failed => ExecCommandExecutionStatus::Failed,
                        CommandExecutionStatus::Declined => ExecCommandExecutionStatus::Declined,
                    },
                }),
            }),
            ThreadItem::FileChange {
                changes, status, ..
            } => Some(ExecThreadItem {
                id: make_id(),
                details: ThreadItemDetails::FileChange(FileChangeItem {
                    changes: changes
                        .into_iter()
                        .map(|change| FileUpdateChange {
                            path: change.path,
                            kind: match change.kind {
                                PatchChangeKind::Add => ExecPatchChangeKind::Add,
                                PatchChangeKind::Delete => ExecPatchChangeKind::Delete,
                                PatchChangeKind::Update { .. } => ExecPatchChangeKind::Update,
                            },
                        })
                        .collect(),
                    status: match status {
                        PatchApplyStatus::InProgress => ExecPatchApplyStatus::InProgress,
                        PatchApplyStatus::Completed => ExecPatchApplyStatus::Completed,
                        PatchApplyStatus::Failed | PatchApplyStatus::Declined => {
                            ExecPatchApplyStatus::Failed
                        }
                    },
                }),
            }),
            ThreadItem::McpToolCall {
                server,
                tool,
                status,
                arguments,
                result,
                error,
                ..
            } => Some(ExecThreadItem {
                id: make_id(),
                details: ThreadItemDetails::McpToolCall(McpToolCallItem {
                    server,
                    tool,
                    status: match status {
                        McpToolCallStatus::InProgress => ExecMcpToolCallStatus::InProgress,
                        McpToolCallStatus::Completed => ExecMcpToolCallStatus::Completed,
                        McpToolCallStatus::Failed => ExecMcpToolCallStatus::Failed,
                    },
                    arguments,
                    result: result.map(|result| McpToolCallItemResult {
                        content: result.content,
                        meta: result.meta,
                        structured_content: result.structured_content,
                    }),
                    error: error.map(|error| McpToolCallItemError {
                        message: error.message,
                    }),
                }),
            }),
            ThreadItem::CollabAgentToolCall {
                tool,
                sender_thread_id,
                receiver_thread_ids,
                prompt,
                agents_states,
                status,
                ..
            } => Some(ExecThreadItem {
                id: make_id(),
                details: ThreadItemDetails::CollabToolCall(CollabToolCallItem {
                    tool: match tool {
                        CollabAgentTool::SpawnAgent => CollabTool::SpawnAgent,
                        CollabAgentTool::SendInput => CollabTool::SendInput,
                        CollabAgentTool::ResumeAgent => CollabTool::Wait,
                        CollabAgentTool::Wait => CollabTool::Wait,
                        CollabAgentTool::CloseAgent => CollabTool::CloseAgent,
                    },
                    sender_thread_id,
                    receiver_thread_ids,
                    prompt,
                    agents_states: agents_states
                        .into_iter()
                        .map(|(thread_id, state)| {
                            (
                                thread_id,
                                CollabAgentState {
                                    status: match state.status {
                                        codex_app_server_protocol::CollabAgentStatus::PendingInit => {
                                            CollabAgentStatus::PendingInit
                                        }
                                        codex_app_server_protocol::CollabAgentStatus::Running => {
                                            CollabAgentStatus::Running
                                        }
                                        codex_app_server_protocol::CollabAgentStatus::Interrupted => {
                                            CollabAgentStatus::Interrupted
                                        }
                                        codex_app_server_protocol::CollabAgentStatus::Completed => {
                                            CollabAgentStatus::Completed
                                        }
                                        codex_app_server_protocol::CollabAgentStatus::Errored => {
                                            CollabAgentStatus::Errored
                                        }
                                        codex_app_server_protocol::CollabAgentStatus::Shutdown => {
                                            CollabAgentStatus::Shutdown
                                        }
                                        codex_app_server_protocol::CollabAgentStatus::NotFound => {
                                            CollabAgentStatus::NotFound
                                        }
                                    },
                                    message: state.message,
                                },
                            )
                        })
                        .collect(),
                    status: match status {
                        CollabAgentToolCallStatus::InProgress => CollabToolCallStatus::InProgress,
                        CollabAgentToolCallStatus::Completed => CollabToolCallStatus::Completed,
                        CollabAgentToolCallStatus::Failed => CollabToolCallStatus::Failed,
                    },
                }),
            }),
            ThreadItem::WebSearch(item) => Some(ExecThreadItem {
                id: make_id(),
                details: ThreadItemDetails::WebSearch(WebSearchItem {
                    id: item.id,
                    query: item.query,
                    action: match item.action {
                        Some(action) => serde_json::from_value(
                            serde_json::to_value(action).unwrap_or_else(|_| json!("other")),
                        )
                        .unwrap_or(WebSearchAction::Other),
                        None => WebSearchAction::Other,
                    },
                }),
            }),
            _ => None,
        }
    }

    fn started_item_id(&mut self, raw_id: &str) -> String {
        if let Some(existing) = self.raw_to_exec_item_id.get(raw_id) {
            return existing.clone();
        }
        let exec_id = self.next_item_id();
        self.raw_to_exec_item_id
            .insert(raw_id.to_string(), exec_id.clone());
        exec_id
    }

    fn completed_item_id(&mut self, raw_id: &str) -> String {
        self.raw_to_exec_item_id
            .remove(raw_id)
            .unwrap_or_else(|| self.next_item_id())
    }

    fn map_started_item(&mut self, item: ThreadItem) -> Option<ExecThreadItem> {
        match item {
            ThreadItem::AgentMessage { .. } | ThreadItem::Reasoning { .. } => None,
            other => {
                let raw_id = other.id().to_string();
                Self::map_item_with_id(other, || self.started_item_id(&raw_id))
            }
        }
    }

    fn map_completed_item_mut(&mut self, item: ThreadItem) -> Option<ExecThreadItem> {
        if let ThreadItem::Reasoning { summary, .. } = &item
            && summary.join("\n").trim().is_empty()
        {
            return None;
        }
        match &item {
            ThreadItem::AgentMessage { .. } | ThreadItem::Reasoning { .. } => {
                Self::map_item_with_id(item, || self.next_item_id())
            }
            other => {
                let raw_id = other.id().to_string();
                Self::map_item_with_id(item, || self.completed_item_id(&raw_id))
            }
        }
    }

    fn reconcile_unfinished_started_items(
        &mut self,
        turn_items: &[ThreadItem],
    ) -> Vec<ThreadEvent> {
        turn_items
            .iter()
            .filter_map(|item| {
                let raw_id = item.id().to_string();
                if !self.raw_to_exec_item_id.contains_key(&raw_id) {
                    return None;
                }
                self.map_completed_item_mut(item.clone())
                    .map(|item| ThreadEvent::ItemCompleted(ItemCompletedEvent { item }))
            })
            .collect()
    }

    fn final_message_from_turn_items(items: &[ThreadItem]) -> Option<String> {
        items
            .iter()
            .rev()
            .find_map(|item| match item {
                ThreadItem::AgentMessage { text, .. } => Some(text.clone()),
                _ => None,
            })
            .or_else(|| {
                items.iter().rev().find_map(|item| match item {
                    ThreadItem::Plan { text, .. } => Some(text.clone()),
                    _ => None,
                })
            })
    }

    pub fn thread_started_event(session_configured: &SessionConfiguredEvent) -> ThreadEvent {
        ThreadEvent::ThreadStarted(ThreadStartedEvent {
            thread_id: session_configured.thread_id.to_string(),
        })
    }

    pub fn collect_warning(&mut self, message: String) -> CollectedThreadEvents {
        CollectedThreadEvents {
            events: vec![ThreadEvent::ItemCompleted(ItemCompletedEvent {
                item: ExecThreadItem {
                    id: self.next_item_id(),
                    details: ThreadItemDetails::Error(ErrorItem { message }),
                },
            })],
            status: CodexStatus::Running,
        }
    }

    pub fn collect_thread_events(
        &mut self,
        notification: ServerNotification,
    ) -> CollectedThreadEvents {
        let mut events = Vec::new();
        let status = match notification {
            ServerNotification::ConfigWarning(notification) => {
                let message = match notification.details {
                    Some(details) if !details.is_empty() => {
                        format!("{} ({details})", notification.summary)
                    }
                    _ => notification.summary,
                };
                events.push(ThreadEvent::ItemCompleted(ItemCompletedEvent {
                    item: ExecThreadItem {
                        id: self.next_item_id(),
                        details: ThreadItemDetails::Error(ErrorItem { message }),
                    },
                }));
                CodexStatus::Running
            }
            ServerNotification::Warning(notification) => {
                let warning = self.collect_warning(notification.message);
                events.extend(warning.events);
                warning.status
            }
            ServerNotification::Error(notification) => {
                self.cccx_exit_code = 1;
                let message = match notification.error.additional_details {
                    Some(details) if !details.is_empty() => {
                        format!("{} ({details})", notification.error.message)
                    }
                    _ => notification.error.message,
                };
                let error = ThreadErrorEvent { message };
                self.last_critical_error = Some(error.clone());
                events.push(ThreadEvent::Error(error));
                CodexStatus::Running
            }
            ServerNotification::DeprecationNotice(notification) => {
                let message = match notification.details {
                    Some(details) if !details.is_empty() => {
                        format!("{} ({details})", notification.summary)
                    }
                    _ => notification.summary,
                };
                events.push(ThreadEvent::ItemCompleted(ItemCompletedEvent {
                    item: ExecThreadItem {
                        id: self.next_item_id(),
                        details: ThreadItemDetails::Error(ErrorItem { message }),
                    },
                }));
                CodexStatus::Running
            }
            ServerNotification::HookStarted(_) | ServerNotification::HookCompleted(_) => {
                CodexStatus::Running
            }
            ServerNotification::ItemStarted(notification) => {
                if let Some(item) = self.map_started_item(notification.item) {
                    events.push(ThreadEvent::ItemStarted(ItemStartedEvent { item }));
                }
                CodexStatus::Running
            }
            ServerNotification::ItemCompleted(notification) => {
                if let Some(item) = self.map_completed_item_mut(notification.item) {
                    if let ThreadItemDetails::AgentMessage(AgentMessageItem { text }) =
                        &item.details
                    {
                        self.final_message = Some(text.clone());
                    }
                    events.push(ThreadEvent::ItemCompleted(ItemCompletedEvent { item }));
                }
                CodexStatus::Running
            }
            ServerNotification::ModelRerouted(notification) => {
                events.push(ThreadEvent::ItemCompleted(ItemCompletedEvent {
                    item: ExecThreadItem {
                        id: self.next_item_id(),
                        details: ThreadItemDetails::Error(ErrorItem {
                            message: format!(
                                "model rerouted: {} -> {} ({:?})",
                                notification.from_model, notification.to_model, notification.reason
                            ),
                        }),
                    },
                }));
                CodexStatus::Running
            }
            ServerNotification::ModelVerification(_) => CodexStatus::Running,
            ServerNotification::CccxRuntimeAgentUpdate(notification) => {
                events.push(ThreadEvent::AgentUpdate(AgentUpdateEvent {
                    logical_session_id: notification.logical_session_id,
                    display_name: notification.display_name,
                    role: notification.role,
                    status: notification.status,
                    model_route: crate::exec_events::AgentUpdateModelRoute {
                        selection_source: notification.model_route.selection_source,
                        requested_route_id: notification.model_route.requested_route_id,
                        effective_route_id: notification.model_route.effective_route_id,
                        fallback_chain: notification.model_route.fallback_chain,
                    },
                    permission_request_id: notification.permission_request_id,
                }));
                CodexStatus::Running
            }
            ServerNotification::ThreadTokenUsageUpdated(notification) => {
                self.last_total_token_usage = Some(notification.token_usage);
                CodexStatus::Running
            }
            ServerNotification::TurnCompleted(notification) => {
                if let Some(running) = self.running_todo_list.take() {
                    events.push(ThreadEvent::ItemCompleted(ItemCompletedEvent {
                        item: ExecThreadItem {
                            id: running.item_id,
                            details: ThreadItemDetails::TodoList(TodoListItem {
                                items: running.items,
                            }),
                        },
                    }));
                }
                events.extend(self.reconcile_unfinished_started_items(&notification.turn.items));
                match notification.turn.status {
                    TurnStatus::Completed => {
                        self.cccx_exit_code = 0;
                        if let Some(final_message) =
                            Self::final_message_from_turn_items(notification.turn.items.as_slice())
                        {
                            self.final_message = Some(final_message);
                        }
                        self.emit_final_message_on_shutdown = true;
                        events.push(ThreadEvent::TurnCompleted(TurnCompletedEvent {
                            usage: self.usage_from_last_total(),
                        }));
                        CodexStatus::InitiateShutdown
                    }
                    TurnStatus::Failed => {
                        self.cccx_exit_code = 1;
                        self.final_message = None;
                        self.emit_final_message_on_shutdown = false;
                        let error = notification
                            .turn
                            .error
                            .map(|error| ThreadErrorEvent {
                                message: match error.additional_details {
                                    Some(details) if !details.is_empty() => {
                                        format!("{} ({details})", error.message)
                                    }
                                    _ => error.message,
                                },
                            })
                            .or_else(|| self.last_critical_error.clone())
                            .unwrap_or_else(|| ThreadErrorEvent {
                                message: "turn failed".to_string(),
                            });
                        events.push(ThreadEvent::TurnFailed(TurnFailedEvent { error }));
                        CodexStatus::InitiateShutdown
                    }
                    TurnStatus::Interrupted => {
                        self.cccx_exit_code = 3;
                        self.final_message = None;
                        self.emit_final_message_on_shutdown = false;
                        CodexStatus::InitiateShutdown
                    }
                    TurnStatus::InProgress => CodexStatus::Running,
                }
            }
            ServerNotification::TurnDiffUpdated(_) => CodexStatus::Running,
            ServerNotification::TurnPlanUpdated(notification) => {
                let items = Self::map_todo_items(&notification.plan);
                if let Some(running) = self.running_todo_list.as_mut() {
                    running.items = items.clone();
                    let item_id = running.item_id.clone();
                    events.push(ThreadEvent::ItemUpdated(ItemUpdatedEvent {
                        item: ExecThreadItem {
                            id: item_id,
                            details: ThreadItemDetails::TodoList(TodoListItem { items }),
                        },
                    }));
                } else {
                    let item_id = self.next_item_id();
                    self.running_todo_list = Some(RunningTodoList {
                        item_id: item_id.clone(),
                        items: items.clone(),
                    });
                    events.push(ThreadEvent::ItemStarted(ItemStartedEvent {
                        item: ExecThreadItem {
                            id: item_id,
                            details: ThreadItemDetails::TodoList(TodoListItem { items }),
                        },
                    }));
                }
                CodexStatus::Running
            }
            ServerNotification::TurnStarted(_) => {
                events.push(ThreadEvent::TurnStarted(TurnStartedEvent {}));
                CodexStatus::Running
            }
            _ => CodexStatus::Running,
        };

        CollectedThreadEvents { events, status }
    }
}

impl EventProcessor for EventProcessorWithJsonOutput {
    fn print_config_summary(
        &mut self,
        _: &Config,
        _: &str,
        session_configured: &SessionConfiguredEvent,
    ) {
        let thread_id = session_configured.thread_id.to_string();
        self.cccx_session_id = Some(thread_id.clone());
        self.cccx_turn_id = Some(thread_id);
        self.emit(Self::thread_started_event(session_configured));
    }

    fn process_server_notification(&mut self, notification: ServerNotification) -> CodexStatus {
        let collected = self.collect_thread_events(notification);
        for event in collected.events {
            self.emit(event);
        }
        collected.status
    }

    fn process_warning(&mut self, message: String) -> CodexStatus {
        let collected = self.collect_warning(message);
        for event in collected.events {
            self.emit(event);
        }
        collected.status
    }

    /// Override: headless mode auto-denies approvals → exit_code=2.
    #[allow(clippy::print_stdout)]
    fn on_approval_denied(&mut self, request_id: String) {
        self.cccx_exit_code = 2;
        let line = serde_json::to_string(&self.approval_denied_envelope(request_id))
            .unwrap_or_else(|err| {
                json!({
                    "schemaVersion": cccx_headless::CCCX_HEADLESS_EVENT_SCHEMA_VERSION,
                    "type": "error",
                    "payload": {
                        "message": format!("failed to serialize approval denial: {err}"),
                        "recoverable": false,
                    },
                })
                .to_string()
            });
        debug_assert!(cccx_headless::assert_stdout_lines_are_json(&line).is_ok());
        println!("{line}");
    }

    #[allow(clippy::print_stdout)]
    fn on_permission_request(
        &mut self,
        request_id: String,
        agent_display_name: String,
        action: String,
        scope: String,
        risk: String,
    ) {
        let line = serde_json::to_string(&self.permission_request_envelope(
            request_id,
            agent_display_name,
            action,
            scope,
            risk,
        ))
        .unwrap_or_else(|err| {
            json!({
                "schemaVersion": cccx_headless::CCCX_HEADLESS_EVENT_SCHEMA_VERSION,
                "type": "error",
                "payload": {
                    "message": format!("failed to serialize permission request: {err}"),
                    "recoverable": false,
                },
            })
            .to_string()
        });
        debug_assert!(cccx_headless::assert_stdout_lines_are_json(&line).is_ok());
        println!("{line}");
    }

    fn print_final_output(&mut self) {
        // cccx-headless: Terminal event is always emitted as the final JSONL line.
        self.emit_terminal_event();
        if self.emit_final_message_on_shutdown
            && let Some(path) = self.last_message_path.as_deref()
        {
            handle_last_message(self.final_message.as_deref(), path);
        }
    }
}

#[cfg(test)]
#[path = "event_processor_with_jsonl_output_tests.rs"]
mod tests;
