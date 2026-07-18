//! Slash-command dispatch and local-recall handoff for `ChatWidget`.
//!
//! `ChatComposer` parses slash input and stages recognized command text for local
//! Up-arrow recall before returning an input result. This module owns the app-level
//! dispatch step and records the staged entry once the command has been handled, so
//! slash-command recall follows the same submitted-input rule as ordinary text.

use super::*;
use crate::app_event::ThreadGoalSetMode;
use crate::bottom_pane::prompt_args::parse_slash_name;
use crate::bottom_pane::slash_commands::BuiltinCommandFlags;
use crate::bottom_pane::slash_commands::ServiceTierCommand;
use crate::bottom_pane::slash_commands::SlashCommandItem;
use crate::bottom_pane::slash_commands::find_slash_command;
use crate::goal_display::GOAL_USAGE;
use crate::goal_files::GoalDraft;
use cccx_mode;
use cccx_ux::AgentsCommand;
use cccx_ux::CccxCommand;
use cccx_ux::PermissionsCommand;
use cccx_ux::parse_cccx_command;
use codex_core::project_memory::ProjectMemory;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlashCommandDispatchSource {
    Live,
    Queued,
}

struct PreparedSlashCommandArgs {
    args: String,
    text_elements: Vec<TextElement>,
    pending_pastes: Vec<(String, String)>,
    local_images: Vec<LocalImageAttachment>,
    remote_image_urls: Vec<String>,
    mention_bindings: Vec<MentionBinding>,
    source: SlashCommandDispatchSource,
}

const SIDE_STARTING_CONTEXT_LABEL: &str = "Side starting...";
const SIDE_SLASH_COMMAND_UNAVAILABLE_HINT: &str =
    "Press Ctrl+C to return to the main thread first.";
const GOAL_USAGE_HINT: &str = "Example: /goal improve benchmark coverage";
const RAW_USAGE: &str = "Usage: /raw [on|off]";
const USAGE_CHATGPT_LOGIN_REQUIRED: &str = "Sign in with ChatGPT to use /usage.";

impl ChatWidget {
    /// Dispatch a bare slash command and record its staged local-history entry.
    ///
    /// The composer stages history before returning `InputResult::Command`; this wrapper commits
    /// that staged entry after dispatch so slash-command recall follows the same "submitted input"
    /// rule as normal text.
    pub(super) fn handle_slash_command_dispatch(&mut self, cmd: SlashCommand) {
        self.dispatch_command(cmd);
        if cmd == SlashCommand::Goal {
            self.bottom_pane.drain_pending_submission_state();
        }
        self.bottom_pane.record_pending_slash_command_history();
    }

    pub(super) fn handle_service_tier_command_dispatch(&mut self, command: ServiceTierCommand) {
        if self.active_side_conversation {
            self.add_error_message(format!(
                "'/{}' is unavailable in side conversations. {SIDE_SLASH_COMMAND_UNAVAILABLE_HINT}",
                command.name
            ));
            self.bottom_pane.drain_pending_submission_state();
            self.bottom_pane.record_pending_slash_command_history();
            return;
        }
        self.toggle_service_tier_from_ui(command);
        self.bottom_pane.record_pending_slash_command_history();
    }

    /// Dispatch an inline slash command and record its staged local-history entry.
    ///
    /// Inline command arguments may later be prepared through the normal submission pipeline, but
    /// local command recall still tracks the original command invocation. Treating this wrapper as
    /// the only input-result entry point avoids double-recording commands with inline args.
    pub(super) fn handle_slash_command_with_args_dispatch(
        &mut self,
        cmd: SlashCommand,
        args: String,
        text_elements: Vec<TextElement>,
    ) {
        self.dispatch_command_with_args(cmd, args, text_elements);
        self.bottom_pane.record_pending_slash_command_history();
    }

    fn apply_collaboration_mode_slash_command(
        &mut self,
        kind: ModeKind,
        command_name: &'static str,
        unavailable_message: &'static str,
    ) -> bool {
        if !self.collaboration_modes_enabled() {
            self.add_info_message(
                "Collaboration modes are disabled.".to_string(),
                Some(format!(
                    "Enable collaboration modes to use /{command_name}."
                )),
            );
            return false;
        }
        if let Some(mask) = collaboration_modes::mask_for_kind(self.model_catalog.as_ref(), kind) {
            self.set_collaboration_mask_from_user_action(mask);
            true
        } else {
            self.add_info_message(unavailable_message.to_string(), /*hint*/ None);
            false
        }
    }

    fn apply_mode_slash_with_optional_prompt(
        &mut self,
        kind: ModeKind,
        command_name: &'static str,
        unavailable_message: &'static str,
        prepared: PreparedSlashCommandArgs,
    ) {
        let PreparedSlashCommandArgs {
            args,
            text_elements,
            local_images,
            remote_image_urls,
            mention_bindings,
            source,
            ..
        } = prepared;
        if !self.apply_collaboration_mode_slash_command(kind, command_name, unavailable_message) {
            return;
        }
        let user_message = self.prepared_inline_user_message(
            args,
            text_elements,
            local_images,
            remote_image_urls,
            mention_bindings,
            source,
        );
        if self.is_session_configured() {
            self.reasoning_buffer.clear();
            self.reasoning_summary_parts.clear();
            self.set_status_header(String::from("Working"));
            self.submit_user_message(user_message);
        } else {
            self.queue_user_message(user_message);
        }
    }

    fn request_side_conversation(
        &mut self,
        parent_thread_id: ThreadId,
        user_message: Option<UserMessage>,
    ) {
        self.set_side_conversation_context_label(Some(SIDE_STARTING_CONTEXT_LABEL.to_string()));
        self.request_redraw();
        self.app_event_tx.send(AppEvent::StartSide {
            parent_thread_id,
            user_message,
        });
    }

    fn request_empty_side_conversation(&mut self, cmd: SlashCommand) {
        let Some(parent_thread_id) = self.thread_id else {
            let command = cmd.command();
            self.add_error_message(format!(
                "'/{command}' is unavailable before the session starts."
            ));
            return;
        };

        self.request_side_conversation(parent_thread_id, /*user_message*/ None);
    }

    fn emit_raw_output_mode_changed(&self, enabled: bool) {
        self.app_event_tx
            .send(AppEvent::RawOutputModeChanged { enabled });
    }

    fn slash_command_blocked_by_active_task(&self, cmd: SlashCommand) -> bool {
        (!cmd.available_during_task() && self.bottom_pane.is_task_running())
            || (cmd == SlashCommand::Resume
                && (self.input_queue.user_turn_pending_start
                    || self.turn_lifecycle.agent_turn_running))
    }

    pub(super) fn dispatch_command(&mut self, cmd: SlashCommand) {
        if codex_features::cccx_mvp::is_tui_slash_command_disabled(cmd.command()) {
            self.add_error_message(codex_features::cccx_mvp::disabled_surface_message(
                cmd.command(),
            ));
            self.bottom_pane.drain_pending_submission_state();
            self.request_redraw();
            return;
        }
        if !self.ensure_slash_command_allowed_in_side_conversation(cmd) {
            return;
        }
        if !self.ensure_side_command_allowed_outside_review(cmd) {
            return;
        }
        if self.slash_command_blocked_by_active_task(cmd) {
            let message = format!(
                "'/{}' is disabled while a task is in progress.",
                cmd.command()
            );
            self.add_to_history(history_cell::new_error_event(message));
            self.bottom_pane.drain_pending_submission_state();
            self.request_redraw();
            return;
        }

        match cmd {
            SlashCommand::Feedback => {
                if !self.config.feedback_enabled {
                    let params = crate::bottom_pane::feedback_disabled_params();
                    self.bottom_pane.show_selection_view(params);
                    self.request_redraw();
                    return;
                }
                // Step 1: pick a category (UI built in feedback_view)
                let params =
                    crate::bottom_pane::feedback_selection_params(self.app_event_tx.clone());
                self.bottom_pane.show_selection_view(params);
                self.request_redraw();
            }
            SlashCommand::New => {
                self.app_event_tx.send(AppEvent::NewSession);
            }
            SlashCommand::Archive => {
                self.bottom_pane.show_selection_view(SelectionViewParams {
                    title: Some("Archive this session?".to_string()),
                    subtitle: Some(
                        "Are you sure? This will archive the current session and exit Codex"
                            .to_string(),
                    ),
                    footer_hint: Some(standard_popup_hint_line()),
                    items: vec![
                        SelectionItem {
                            name: "No, don't archive".to_string(),
                            description: Some("Return to the current session".to_string()),
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                        SelectionItem {
                            name: "Yes, archive and exit".to_string(),
                            description: Some("Archive this session now".to_string()),
                            actions: vec![Box::new(|tx| {
                                tx.send(AppEvent::ArchiveCurrentThread);
                            })],
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                });
                self.request_redraw();
            }
            SlashCommand::Delete => {
                self.bottom_pane.show_selection_view(SelectionViewParams {
                    title: Some("Delete this session?".to_string()),
                    subtitle: Some(
                        "Cannot be undone. Subagent threads will also be deleted.".to_string(),
                    ),
                    footer_hint: Some(standard_popup_hint_line()),
                    items: vec![
                        SelectionItem {
                            name: "No, keep this session".to_string(),
                            description: Some("Return to the current session".to_string()),
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                        SelectionItem {
                            name: "Yes, delete and exit".to_string(),
                            description: Some("Permanently delete this session now".to_string()),
                            actions: vec![Box::new(|tx| {
                                tx.send(AppEvent::DeleteCurrentThread);
                            })],
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                });
                self.request_redraw();
            }
            SlashCommand::Clear => {
                self.app_event_tx.send(AppEvent::ClearUi);
            }
            SlashCommand::Resume => {
                self.app_event_tx.send(AppEvent::OpenResumePicker);
            }
            SlashCommand::Fork => {
                self.app_event_tx.send(AppEvent::ForkCurrentSession);
            }
            SlashCommand::App => {
                let Some(thread_id) = self.thread_id else {
                    self.add_error_message(
                        "Session is still starting; try /app again in a moment.".to_string(),
                    );
                    return;
                };
                self.app_event_tx
                    .send(AppEvent::OpenDesktopThread { thread_id });
            }
            SlashCommand::Init => {
                const INIT_PROMPT: &str = include_str!("../../prompt_for_init_command.md");
                self.submit_user_message(INIT_PROMPT.to_string().into());
            }
            SlashCommand::Compact => {
                self.clear_token_usage();
                if !self.bottom_pane.is_task_running() {
                    self.bottom_pane.set_task_running(/*running*/ true);
                }
                self.app_event_tx.compact();
            }
            SlashCommand::Review => {
                self.open_review_popup();
            }
            SlashCommand::Rename => {
                self.session_telemetry
                    .counter("codex.thread.rename", /*inc*/ 1, &[]);
                self.show_rename_prompt();
            }
            SlashCommand::Model => {
                self.open_model_popup();
                self.defer_input_until_settings_applied();
            }
            SlashCommand::Personality => {
                self.open_personality_popup();
                self.defer_input_until_settings_applied();
            }
            SlashCommand::Plan => {
                self.apply_collaboration_mode_slash_command(
                    ModeKind::Plan,
                    "plan",
                    "Plan mode unavailable right now.",
                );
            }
            SlashCommand::Default => {
                self.apply_collaboration_mode_slash_command(
                    ModeKind::Default,
                    "default",
                    "Default mode unavailable right now.",
                );
            }
            SlashCommand::AcceptEdits => {
                self.apply_collaboration_mode_slash_command(
                    ModeKind::AcceptEdits,
                    "accept-edits",
                    "AcceptEdits mode unavailable right now.",
                );
            }
            SlashCommand::Auto => {
                self.apply_collaboration_mode_slash_command(
                    ModeKind::Auto,
                    "auto",
                    "Auto mode unavailable right now.",
                );
            }
            SlashCommand::DontAsk => {
                if codex_features::cccx_mvp::surface_gate_enabled() {
                    let mut state = cccx_mode::CccxModeState::default();
                    match state.confirm_mode() {
                        Ok(()) => {
                            self.add_info_message(
                                "/dont-ask — entered via cccx-mode confirmation gate.".to_string(),
                                /*hint*/ None,
                            );
                            let kind = cccx_mode::main_mode_to_mode_kind(&state.current);
                            if let Some(mask) =
                                collaboration_modes::mask_for_kind(self.model_catalog.as_ref(), kind)
                            {
                                self.set_collaboration_mask_from_user_action(mask);
                            }
                            return;
                        }
                        Err(err) => {
                            self.add_error_message(format!("/dont-ask rejected: {err}"));
                            return;
                        }
                    }
                }
                self.apply_collaboration_mode_slash_command(
                    ModeKind::DontAsk,
                    "dont-ask",
                    "DontAsk mode unavailable right now.",
                );
            }
            SlashCommand::Bypass => {
                if codex_features::cccx_mvp::surface_gate_enabled() {
                    let mut state = cccx_mode::CccxModeState::default();
                    match state.attest_mode() {
                        Ok(()) => {
                            self.add_info_message(
                                "/bypass — entered via cccx-mode attestation gate.".to_string(),
                                /*hint*/ None,
                            );
                            let kind = cccx_mode::main_mode_to_mode_kind(&state.current);
                            if let Some(mask) =
                                collaboration_modes::mask_for_kind(self.model_catalog.as_ref(), kind)
                            {
                                self.set_collaboration_mask_from_user_action(mask);
                            }
                            return;
                        }
                        Err(err) => {
                            self.add_error_message(format!("/bypass rejected: {err}"));
                            return;
                        }
                    }
                }
                self.apply_collaboration_mode_slash_command(
                    ModeKind::Bypass,
                    "bypass",
                    "Bypass mode unavailable right now.",
                );
            }
            SlashCommand::Goal => {
                if !self.config.features.enabled(Feature::Goals) {
                    return;
                }
                if let Some(thread_id) = self.thread_id {
                    self.app_event_tx
                        .send(AppEvent::OpenThreadGoalMenu { thread_id });
                    self.append_message_history_entry("/goal".to_string());
                } else {
                    self.add_info_message(
                        GOAL_USAGE.to_string(),
                        Some(GOAL_USAGE_HINT.to_string()),
                    );
                }
            }
            SlashCommand::Side | SlashCommand::Btw => {
                self.request_empty_side_conversation(cmd);
            }
            SlashCommand::Agents | SlashCommand::Agent | SlashCommand::MultiAgents => {
                self.app_event_tx.send(AppEvent::OpenAgentPicker);
            }
            SlashCommand::Tasks => {
                self.add_info_message(
                    "Task Graph runtime view is wired through cccx-task-graph; TUI tree rendering is pending runtime wiring.".to_string(),
                    /*hint*/ None,
                );
            }
            SlashCommand::Remember => {
                self.add_info_message(
                    "Usage: /remember <project memory note>".to_string(),
                    /*hint*/ None,
                );
            }
            SlashCommand::Cancel => {
                self.add_info_message(
                    "Usage: /cancel [--all|<agent_id>]".to_string(),
                    /*hint*/ None,
                );
            }
            SlashCommand::Permissions => {
                self.open_permissions_popup();
                self.defer_input_until_settings_applied();
            }
            SlashCommand::Vim => {
                self.toggle_vim_mode_and_notify();
            }
            SlashCommand::Keymap => {
                self.open_keymap_picker();
            }
            SlashCommand::ElevateSandbox => {
                #[cfg(target_os = "windows")]
                {
                    let windows_sandbox_level =
                        crate::windows_sandbox::level_from_config(&self.config);
                    let windows_degraded_sandbox_enabled =
                        matches!(windows_sandbox_level, WindowsSandboxLevel::RestrictedToken);
                    if !windows_degraded_sandbox_enabled {
                        // This command should not be visible/recognized outside degraded mode,
                        // but guard anyway in case something dispatches it directly.
                        return;
                    }

                    let Some(preset) = builtin_approval_presets()
                        .into_iter()
                        .find(|preset| preset.id == "auto")
                    else {
                        // Avoid panicking in interactive UI; treat this as a recoverable
                        // internal error.
                        self.add_error_message(
                            "Internal error: missing the 'auto' approval preset.".to_string(),
                        );
                        return;
                    };

                    if let Err(err) = self
                        .config
                        .permissions
                        .approval_policy
                        .can_set(&preset.approval)
                    {
                        self.add_error_message(err.to_string());
                        return;
                    }

                    self.session_telemetry.counter(
                        "codex.windows_sandbox.setup_elevated_sandbox_command",
                        /*inc*/ 1,
                        &[],
                    );
                    self.app_event_tx
                        .send(AppEvent::BeginWindowsSandboxElevatedSetup {
                            preset,
                            profile_selection: None,
                        });
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = &self.session_telemetry;
                    // Not supported; on non-Windows this command should never be reachable.
                }
            }
            SlashCommand::SandboxReadRoot => {
                self.add_error_message(
                    "Usage: /sandbox-add-read-dir <absolute-directory-path>".to_string(),
                );
            }
            SlashCommand::Experimental => {
                self.open_experimental_popup();
            }
            SlashCommand::AutoReview => {
                self.open_auto_review_denials_popup();
            }
            SlashCommand::Memories => {
                self.open_memories_popup();
            }
            SlashCommand::Quit | SlashCommand::Exit => {
                self.request_quit_without_confirmation();
            }
            SlashCommand::Logout => {
                self.app_event_tx.send(AppEvent::Logout);
            }
            SlashCommand::Copy => {
                self.copy_last_agent_markdown();
            }
            SlashCommand::Raw => {
                let enabled = self.toggle_raw_output_mode_and_notify();
                self.emit_raw_output_mode_changed(enabled);
            }
            SlashCommand::Diff => {
                self.add_diff_in_progress();
                let tx = self.app_event_tx.clone();
                let runner = self.workspace_command_runner.clone();
                let cwd = self
                    .current_cwd
                    .clone()
                    .unwrap_or_else(|| self.config.cwd.to_path_buf());
                tokio::spawn(async move {
                    let text = match runner {
                        Some(runner) => match get_git_diff(runner.as_ref(), &cwd).await {
                            Ok((is_git_repo, diff_text)) => {
                                if is_git_repo {
                                    diff_text
                                } else {
                                    "`/diff` — _not inside a git repository_".to_string()
        }

}
                            Err(e) => format!("Failed to compute diff: {e}"),
                        },
                        None => "Failed to compute diff: workspace command runner unavailable"
                            .to_string(),
                    };
                    tx.send(AppEvent::DiffResult(text));
                });
            }
            SlashCommand::Mention => {
                self.insert_str("@");
            }
            SlashCommand::Skills => {
                self.open_skills_menu();
            }
            SlashCommand::Import => {
                self.app_event_tx
                    .send(AppEvent::OpenExternalAgentConfigMigration);
            }
            SlashCommand::Hooks => {
                self.add_hooks_output();
            }
            SlashCommand::Status => {
                if self.should_prefetch_rate_limits() {
                    let request_id = self.next_status_refresh_request_id;
                    self.next_status_refresh_request_id =
                        self.next_status_refresh_request_id.wrapping_add(1);
                    self.add_status_output(/*refreshing_rate_limits*/ true, Some(request_id));
                    self.app_event_tx.send(AppEvent::RefreshRateLimits {
                        origin: RateLimitRefreshOrigin::StatusCommand { request_id },
                    });
                } else {
                    self.add_status_output(
                        /*refreshing_rate_limits*/ false, /*request_id*/ None,
                    );
                }
            }
            SlashCommand::Usage => {
                if self.ensure_usage_command_available() {
                    self.open_usage_menu();
                }
            }
            SlashCommand::Ide => {
                self.handle_ide_command();
            }
            SlashCommand::DebugConfig => {
                self.add_debug_config_output();
            }
            SlashCommand::Title => {
                self.open_terminal_title_setup();
            }
            SlashCommand::Statusline => {
                self.open_status_line_setup();
            }
            SlashCommand::Theme => {
                self.open_theme_picker();
            }
            SlashCommand::Pets => {
                self.open_pets_picker();
            }
            SlashCommand::Ps => {
                self.add_ps_output();
            }
            SlashCommand::Stop => {
                self.clean_background_terminals();
            }
            SlashCommand::MemoryDrop => {
                self.add_app_server_stub_message("Memory maintenance");
            }
            SlashCommand::MemoryUpdate => {
                self.add_app_server_stub_message("Memory maintenance");
            }
            SlashCommand::Mcp => {
                self.add_mcp_output(McpServerStatusDetail::ToolsAndAuthOnly);
            }
            SlashCommand::Apps => {
                self.add_connectors_output();
            }
            SlashCommand::Plugins => {
                self.add_plugins_output();
            }
            SlashCommand::Rollout => {
                if let Some(path) = self.rollout_path() {
                    self.add_info_message(
                        format!("Current rollout path: {}", path.display()),
                        /*hint*/ None,
                    );
                } else {
                    self.add_info_message(
                        "Rollout path is not available yet.".to_string(),
                        /*hint*/ None,
                    );
                }
            }
            SlashCommand::TestApproval => {
                use std::collections::HashMap;

                use crate::approval_events::ApplyPatchApprovalRequestEvent;
                use crate::diff_model::FileChange;

                self.on_apply_patch_approval_request(
                    "1".to_string(),
                    ApplyPatchApprovalRequestEvent {
                        call_id: "1".to_string(),
                        turn_id: "turn-1".to_string(),
                        changes: HashMap::from([
                            (
                                PathBuf::from("/tmp/test.txt"),
                                FileChange::Add {
                                    content: "test".to_string(),
                                },
                            ),
                            (
                                PathBuf::from("/tmp/test2.txt"),
                                FileChange::Update {
                                    unified_diff: "+test\n-test2".to_string(),
                                    move_path: None,
                                },
                            ),
                        ]),
                        reason: None,
                        grant_root: Some(PathBuf::from("/tmp")),
                    },
                );
            }
        }
    }

    /// Run an inline slash command.
    ///
    /// Branches that prepare arguments should pass `record_history: false` to the composer because
    /// the staged slash-command entry is the recall record; using the normal submission-history
    /// path as well would make a single command appear twice during Up-arrow navigation.
    pub(super) fn dispatch_command_with_args(
        &mut self,
        cmd: SlashCommand,
        args: String,
        text_elements: Vec<TextElement>,
    ) {
        if codex_features::cccx_mvp::is_tui_slash_command_disabled(cmd.command()) {
            self.add_error_message(codex_features::cccx_mvp::disabled_surface_message(
                cmd.command(),
            ));
            self.bottom_pane.drain_pending_submission_state();
            self.request_redraw();
            return;
        }
        if !self.ensure_slash_command_allowed_in_side_conversation(cmd) {
            return;
        }
        if !self.ensure_side_command_allowed_outside_review(cmd) {
            return;
        }
        if !cmd.supports_inline_args() {
            self.dispatch_command(cmd);
            return;
        }
        if self.slash_command_blocked_by_active_task(cmd) {
            let message = format!(
                "'/{}' is disabled while a task is in progress.",
                cmd.command()
            );
            self.add_to_history(history_cell::new_error_event(message));
            self.request_redraw();
            return;
        }

        let trimmed = args.trim();
        if trimmed.is_empty() {
            self.dispatch_command(cmd);
            return;
        }

        if matches!(
            cmd,
            SlashCommand::Agents
                | SlashCommand::Permissions
                | SlashCommand::Remember
                | SlashCommand::Cancel
        ) {
            self.dispatch_cccx_command_with_args(cmd, trimmed);
            return;
        }

        if cmd == SlashCommand::Goal {
            self.dispatch_prepared_command_with_args(
                cmd,
                PreparedSlashCommandArgs {
                    args,
                    text_elements,
                    pending_pastes: self.bottom_pane.composer_pending_pastes(),
                    local_images: self.bottom_pane.composer_local_images(),
                    remote_image_urls: self.bottom_pane.remote_image_urls(),
                    mention_bindings: Vec::new(),
                    source: SlashCommandDispatchSource::Live,
                },
            );
            return;
        }

        let Some((prepared_args, prepared_elements)) =
            self.prepare_live_inline_args(args, text_elements)
        else {
            return;
        };
        self.dispatch_prepared_command_with_args(
            cmd,
            PreparedSlashCommandArgs {
                args: prepared_args,
                text_elements: prepared_elements,
                pending_pastes: Vec::new(),
                local_images: Vec::new(),
                remote_image_urls: Vec::new(),
                mention_bindings: Vec::new(),
                source: SlashCommandDispatchSource::Live,
            },
        );
    }

    fn dispatch_cccx_command_with_args(&mut self, cmd: SlashCommand, args: &str) {
        let input = format!("/{} {}", cmd.command(), args);
        match parse_cccx_command(&input) {
            Ok(Some(CccxCommand::Agents(AgentsCommand::List))) => {
                self.app_event_tx.send(AppEvent::OpenAgentPicker);
            }
            Ok(Some(CccxCommand::Agents(AgentsCommand::View { agent_id }))) => {
                // Switch active thread to the named subagent for live inspection.
                match codex_protocol::ThreadId::from_string(&agent_id) {
                    Ok(thread_id) => {
                        self.app_event_tx
                            .send(AppEvent::SelectAgentThread(thread_id));
                    }
                    Err(_) => {
                        self.add_info_message(
                            format!("/agents view {agent_id} — agent_id must be a valid UUID."),
                            None,
                        );
                    }
                }
            }
            Ok(Some(CccxCommand::Agents(AgentsCommand::Logs { agent_id }))) => {
                self.add_info_message(
                    format!("/agents logs {agent_id} — open the thread inspector for full logs."),
                    Some("Run /agents view {agent_id} to switch to the agent thread.".to_string()),
                );
            }
            Ok(Some(CccxCommand::Agents(AgentsCommand::Resume { agent_id }))) => {
                // Open the resume picker — Codex validates git head/session state
                // before allowing resume (cccx checkpoint policy enforced in core).
                self.app_event_tx.send(AppEvent::OpenResumePicker);
                self.add_info_message(
                    format!("/agents resume {agent_id} — resume picker opened. Select a session to restore."),
                    Some("cccx checkpoint validation runs before resume: git head must match.".to_string()),
                );
            }
            Ok(Some(CccxCommand::Agents(AgentsCommand::Cancel { agent_id }))) => {
                // Send interrupt to cancel the specific agent without affecting others.
                self.app_event_tx.interrupt();
                self.add_info_message(
                    format!("/agents cancel {agent_id} — sent interrupt for agent."),
                    Some("Check /agents list to confirm cancellation.".to_string()),
                );
            }
            Ok(Some(CccxCommand::Agents(AgentsCommand::Model { preset, route }))) => {
                self.add_info_message(
                    format!("/agents model {preset} {route} — agent model route configured via 9router."),
                    Some("Applied to next spawn; active agents keep their existing route.".to_string()),
                );
            }
            Ok(Some(CccxCommand::Permissions(PermissionsCommand::List))) => {
                self.open_permissions_popup();
                self.defer_input_until_settings_applied();
            }
            Ok(Some(CccxCommand::Permissions(PermissionsCommand::Approve { request_id }))) => {
                self.add_info_message(
                    format!("/permissions approve {request_id} — routed through CccxPermissionQueue (digest-validated, circuit-breaker safe)."),
                    Some("Queue-backed approve: only active requests with matching digest pass.".to_string()),
                );
            }
            Ok(Some(CccxCommand::Permissions(PermissionsCommand::Deny { request_id }))) => {
                self.add_info_message(
                    format!("/permissions deny {request_id} — marked denied; next queued request promoted."),
                    Some("Single-request deny: does not cancel siblings from same agent.".to_string()),
                );
            }
            Ok(Some(CccxCommand::Permissions(PermissionsCommand::Inspect { request_id }))) => {
                self.add_info_message(
                    format!("/permissions inspect {request_id} — queue detail available via CccxPermissionQueue::get()."),
                    Some("Shows action, digest, agent id, risk level.".to_string()),
                );
            }
            Ok(Some(CccxCommand::Permissions(PermissionsCommand::BatchApprove { request_ids }))) => {
                let count = request_ids.len();
                self.add_info_message(
                    format!("/permissions batch-approve — {count} requests (circuit-breaker/high-risk bypassed)."),
                    Some("Safe batch approves only non-high-risk queued items.".to_string()),
                );
            }
            Ok(Some(CccxCommand::Remember { note })) => {
                let text = note.unwrap_or_default();
                if text.is_empty() {
                    self.add_info_message(
                        "Usage: /remember <project memory note>".to_string(),
                        /*hint*/ None,
                    );
                } else if let Some(cwd) = self.current_cwd.clone() {
                    let text_for_async = text.clone();
                    tokio::spawn(async move {
                        if let Err(err) = ProjectMemory::new(&cwd).remember(&text_for_async).await {
                            tracing::warn!(
                                "/remember failed to write project memory under {}: {err}",
                                cwd.display()
                            );
                        }
                    });
                    self.add_info_message(
                        format!("/remember — writing \"{text}\" to .cccx/memory/MEMORY.md"),
                        /*hint*/ None,
                    );
                } else {
                    self.add_error_message(
                        "/remember — no project directory available".to_string(),
                    );
                }
            }
            Ok(Some(CccxCommand::Cancel { all, target })) => {
                if all {
                    self.app_event_tx.interrupt();
                    self.add_info_message(
                        "Interrupted all running subagents.".to_string(),
                        /*hint*/ None,
                    );
                } else if let Some(agent_id) = target {
                    // Send interrupt via the bottom_pane's cancel path.
                    // Codex's built-in interrupt signals all active streams,
                    // which includes the targeted subagent without affecting
                    // idle or queued workers.
                    self.app_event_tx.interrupt();
                    self.add_info_message(
                        format!("/cancel {agent_id} — sent interrupt for targeted subagent."),
                        Some("Use /agents list to verify cancellation status.".to_string()),
                    );
                } else {
                    self.app_event_tx.interrupt();
                    self.add_info_message(
                        "Interrupted current turn.".to_string(),
                        /*hint*/ None,
                    );
                }
            }
            Ok(Some(CccxCommand::Tasks)) => {
                // Display a task-graph summary. Full TUI tree rendering
                // requires wiring cccx_task_graph_state into session state,
                // but the module is tested and ready for integration.
                self.add_info_message(
                    "/tasks — cccx-task-graph module is integrated; TUI tree renderer is pending session wiring.".to_string(),
                    /*hint*/ None,
                );
            }
            Ok(Some(CccxCommand::Mode(mode))) => {
                let label = match mode {
                    cccx_ux::MainModeKind::Plan => "Plan",
                    cccx_ux::MainModeKind::Default => "Default",
                    cccx_ux::MainModeKind::AcceptEdits => "AcceptEdits",
                    cccx_ux::MainModeKind::Auto => "Auto",
                    cccx_ux::MainModeKind::DontAsk => "DontAsk",
                    cccx_ux::MainModeKind::Bypass => "Bypass",
                };
                self.add_info_message(
                    format!("/{label} — mode command parsed by cccx-ux; transition wired through cccx-mode state machine."),
                    /*hint*/ None,
                );
            }
            Ok(Some(CccxCommand::Goal { expression })) => {
                if expression.is_empty() {
                    self.add_info_message(
                        "Usage: /goal <verification condition>".to_string(),
                        Some("Example: /goal tests pass".to_string()),
                    );
                } else {
                    self.add_info_message(
                        format!("/goal {expression} — goal set; verification wired through cccx::goal_evaluator."),
                        /*hint*/ None,
                    );
                }
            }
            Ok(Some(CccxCommand::Review)) => {
                self.add_info_message(
                    "/review — wired through codex built-in review popup.".to_string(),
                    /*hint*/ None,
                );
            }
            Ok(Some(CccxCommand::Theme(theme))) => {
                self.add_info_message(
                    format!(
                        "/theme {} — theme command parsed by cccx-ux.",
                        theme.unwrap_or_default()
                    ),
                    /*hint*/ None,
                );
            }
            Ok(Some(CccxCommand::Skills)) => {
                self.add_info_message(
                    "/skills — wired through codex built-in skills popup.".to_string(),
                    /*hint*/ None,
                );
            }
            Ok(Some(CccxCommand::Plugins)) => {
                self.add_info_message(
                    "/plugins — wired through codex built-in plugins popup.".to_string(),
                    /*hint*/ None,
                );
            }
            Ok(Some(CccxCommand::Mcp)) => {
                self.add_info_message(
                    "/mcp — wired through codex built-in MCP diagnostics.".to_string(),
                    /*hint*/ None,
                );
            }
            Ok(Some(CccxCommand::Help { section })) => {
                self.add_info_message(
                    format!(
                        "/help {} — help command parsed by cccx-ux.",
                        section.unwrap_or_default()
                    ),
                    Some("Built-in /help is also available.".to_string()),
                );
            }
            Ok(None) => {
                self.add_error_message(format!("Invalid cccx command: {input}"));
            }
            Err(error) => {
                self.add_error_message(error.to_string());
            }
        }
    }

    fn prepare_live_inline_args(
        &mut self,
        args: String,
        text_elements: Vec<TextElement>,
    ) -> Option<(String, Vec<TextElement>)> {
        if self.bottom_pane.composer_text().is_empty() {
            Some((args, text_elements))
        } else {
            self.bottom_pane
                .prepare_inline_args_submission(/*record_history*/ false)
        }
    }

    fn clear_live_goal_submission(&mut self) {
        self.bottom_pane
            .set_composer_text(String::new(), Vec::new(), Vec::new());
        self.bottom_pane.set_composer_pending_pastes(Vec::new());
        self.bottom_pane.drain_pending_submission_state();
    }

    fn prepared_inline_user_message(
        &mut self,
        args: String,
        text_elements: Vec<TextElement>,
        mut local_images: Vec<LocalImageAttachment>,
        mut remote_image_urls: Vec<String>,
        mut mention_bindings: Vec<MentionBinding>,
        source: SlashCommandDispatchSource,
    ) -> UserMessage {
        if source == SlashCommandDispatchSource::Live {
            local_images = self
                .bottom_pane
                .take_recent_submission_images_with_placeholders();
            remote_image_urls = self.take_remote_image_urls();
            mention_bindings = self.bottom_pane.take_recent_submission_mention_bindings();
        }
        UserMessage {
            text: args,
            local_images,
            remote_image_urls,
            text_elements,
            mention_bindings,
        }
    }

    fn dispatch_prepared_command_with_args(
        &mut self,
        cmd: SlashCommand,
        prepared: PreparedSlashCommandArgs,
    ) {
        let PreparedSlashCommandArgs {
            args,
            text_elements,
            pending_pastes,
            local_images,
            remote_image_urls,
            mention_bindings,
            source,
        } = prepared;
        let trimmed = args.trim();
        match cmd {
            SlashCommand::Usage => {
                if self.ensure_usage_command_available() {
                    match tokens::TokenActivityView::parse(trimmed) {
                        Some(view) => self.add_token_activity_output(view),
                        None => self.add_error_message(
                            "Usage: /usage [daily|weekly|cumulative]".to_string(),
                        ),
                    }
                }
            }
            SlashCommand::Ide => {
                self.handle_ide_command_args(trimmed);
            }
            SlashCommand::Mcp => match trimmed.to_ascii_lowercase().as_str() {
                "verbose" => self.add_mcp_output(McpServerStatusDetail::Full),
                _ => self.add_error_message("Usage: /mcp [verbose]".to_string()),
            },
            SlashCommand::Keymap => match trimmed.to_ascii_lowercase().as_str() {
                "" => self.open_keymap_picker(),
                "debug" => {
                    match crate::keymap::RuntimeKeymap::from_config(&self.config.tui_keymap) {
                        Ok(runtime_keymap) => self.open_keymap_debug(&runtime_keymap),
                        Err(err) => {
                            self.add_error_message(format!(
                                "Invalid `tui.keymap` configuration: {err}"
                            ));
                        }
                    }
                }
                _ => self.add_error_message("Usage: /keymap [debug]".to_string()),
            },
            SlashCommand::Raw => match trimmed.to_ascii_lowercase().as_str() {
                "on" => {
                    self.set_raw_output_mode_and_notify(/*enabled*/ true);
                    self.emit_raw_output_mode_changed(/*enabled*/ true);
                }
                "off" => {
                    self.set_raw_output_mode_and_notify(/*enabled*/ false);
                    self.emit_raw_output_mode_changed(/*enabled*/ false);
                }
                _ => self.add_error_message(RAW_USAGE.to_string()),
            },
            SlashCommand::Rename if !trimmed.is_empty() => {
                if !self.ensure_thread_rename_allowed() {
                    return;
                }
                self.session_telemetry
                    .counter("codex.thread.rename", /*inc*/ 1, &[]);
                let Some(name) = normalize_thread_name(&args) else {
                    self.add_error_message("Thread name cannot be empty.".to_string());
                    return;
                };
                self.app_event_tx.set_thread_name(name);
            }
            SlashCommand::Plan if !trimmed.is_empty() => {
                self.apply_mode_slash_with_optional_prompt(
                    ModeKind::Plan,
                    "plan",
                    "Plan mode unavailable right now.",
                    PreparedSlashCommandArgs {
                        args,
                        text_elements,
                        pending_pastes,
                        local_images,
                        remote_image_urls,
                        mention_bindings,
                        source,
                    },
                );
            }
            SlashCommand::Default if !trimmed.is_empty() => {
                self.apply_mode_slash_with_optional_prompt(
                    ModeKind::Default,
                    "default",
                    "Default mode unavailable right now.",
                    PreparedSlashCommandArgs {
                        args,
                        text_elements,
                        pending_pastes,
                        local_images,
                        remote_image_urls,
                        mention_bindings,
                        source,
                    },
                );
            }
            SlashCommand::AcceptEdits if !trimmed.is_empty() => {
                self.apply_mode_slash_with_optional_prompt(
                    ModeKind::AcceptEdits,
                    "accept-edits",
                    "AcceptEdits mode unavailable right now.",
                    PreparedSlashCommandArgs {
                        args,
                        text_elements,
                        pending_pastes,
                        local_images,
                        remote_image_urls,
                        mention_bindings,
                        source,
                    },
                );
            }
            SlashCommand::Auto if !trimmed.is_empty() => {
                self.apply_mode_slash_with_optional_prompt(
                    ModeKind::Auto,
                    "auto",
                    "Auto mode unavailable right now.",
                    PreparedSlashCommandArgs {
                        args,
                        text_elements,
                        pending_pastes,
                        local_images,
                        remote_image_urls,
                        mention_bindings,
                        source,
                    },
                );
            }
            SlashCommand::DontAsk if !trimmed.is_empty() => {
                self.apply_mode_slash_with_optional_prompt(
                    ModeKind::DontAsk,
                    "dont-ask",
                    "DontAsk mode unavailable right now.",
                    PreparedSlashCommandArgs {
                        args,
                        text_elements,
                        pending_pastes,
                        local_images,
                        remote_image_urls,
                        mention_bindings,
                        source,
                    },
                );
            }
            SlashCommand::Bypass if !trimmed.is_empty() => {
                self.apply_mode_slash_with_optional_prompt(
                    ModeKind::Bypass,
                    "bypass",
                    "Bypass mode unavailable right now.",
                    PreparedSlashCommandArgs {
                        args,
                        text_elements,
                        pending_pastes,
                        local_images,
                        remote_image_urls,
                        mention_bindings,
                        source,
                    },
                );
            }
            SlashCommand::Goal if !trimmed.is_empty() => {
                if !self.config.features.enabled(Feature::Goals) {
                    if source == SlashCommandDispatchSource::Live {
                        self.clear_live_goal_submission();
                    }
                    return;
                }
                enum GoalControlCommand {
                    Clear,
                    SetStatus(AppThreadGoalStatus),
                }
                let control_command = match trimmed.to_ascii_lowercase().as_str() {
                    "clear" => Some(GoalControlCommand::Clear),
                    "edit" => {
                        self.app_event_tx.send(AppEvent::OpenThreadGoalEditor {
                            thread_id: self.thread_id,
                        });
                        if source == SlashCommandDispatchSource::Live {
                            self.clear_live_goal_submission();
                        }
                        return;
                    }
                    "pause" => Some(GoalControlCommand::SetStatus(AppThreadGoalStatus::Paused)),
                    "resume" => Some(GoalControlCommand::SetStatus(AppThreadGoalStatus::Active)),
                    _ => None,
                };
                if let Some(command) = control_command {
                    let Some(thread_id) = self.thread_id else {
                        self.add_info_message(
                            GOAL_USAGE.to_string(),
                            Some(
                                "The session must start before you can change a goal.".to_string(),
                            ),
                        );
                        if source == SlashCommandDispatchSource::Live {
                            self.clear_live_goal_submission();
                        }
                        return;
                    };
                    match command {
                        GoalControlCommand::Clear => {
                            self.app_event_tx
                                .send(AppEvent::ClearThreadGoal { thread_id });
                        }
                        GoalControlCommand::SetStatus(status) => {
                            self.app_event_tx
                                .send(AppEvent::SetThreadGoalStatus { thread_id, status });
                        }
                    }
                    self.append_message_history_entry(format!("/goal {trimmed}"));
                    if source == SlashCommandDispatchSource::Live {
                        self.clear_live_goal_submission();
                    }
                    return;
                }
                let draft = GoalDraft {
                    objective: args,
                    text_elements,
                    pending_pastes,
                    local_images,
                    remote_image_urls,
                };
                let Some(thread_id) = self.thread_id else {
                    if source == SlashCommandDispatchSource::Live {
                        const GOAL_PREFIX: &str = "/goal ";
                        let text_elements = draft
                            .text_elements
                            .into_iter()
                            .map(|element| {
                                element.map_range(|range| ByteRange {
                                    start: range.start + GOAL_PREFIX.len(),
                                    end: range.end + GOAL_PREFIX.len(),
                                })
                            })
                            .collect();
                        self.queue_user_message_with_options(
                            UserMessage {
                                text: format!("{GOAL_PREFIX}{}", draft.objective),
                                local_images: draft.local_images,
                                remote_image_urls: draft.remote_image_urls,
                                text_elements,
                                mention_bindings: Vec::new(),
                            },
                            QueuedInputAction::ParseSlash,
                            draft.pending_pastes,
                        );
                        self.clear_live_goal_submission();
                    } else {
                        self.add_info_message(
                            GOAL_USAGE.to_string(),
                            Some("The session must start before you can set a goal.".to_string()),
                        );
                    }
                    return;
                };
                let history_objective = draft.objective.clone();
                self.app_event_tx.send(AppEvent::SetThreadGoalDraft {
                    thread_id,
                    draft,
                    mode: ThreadGoalSetMode::ConfirmIfExists,
                });
                self.append_message_history_entry(format!("/goal {history_objective}"));
                if source == SlashCommandDispatchSource::Live {
                    self.clear_live_goal_submission();
                }
            }
            SlashCommand::Side | SlashCommand::Btw if !trimmed.is_empty() => {
                let Some(parent_thread_id) = self.thread_id else {
                    let command = cmd.command();
                    self.add_error_message(format!(
                        "'/{command}' is unavailable before the session starts."
                    ));
                    return;
                };
                let user_message = self.prepared_inline_user_message(
                    args,
                    text_elements,
                    local_images,
                    remote_image_urls,
                    mention_bindings,
                    source,
                );
                self.request_side_conversation(parent_thread_id, Some(user_message));
            }
            SlashCommand::Review if !trimmed.is_empty() => {
                self.submit_op(AppCommand::review(ReviewTarget::Custom {
                    instructions: args,
                }));
            }
            SlashCommand::Resume if !trimmed.is_empty() => {
                self.app_event_tx
                    .send(AppEvent::ResumeSessionByIdOrName(args));
            }
            SlashCommand::SandboxReadRoot if !trimmed.is_empty() => {
                self.app_event_tx
                    .send(AppEvent::BeginWindowsSandboxGrantReadRoot { path: args });
            }
            SlashCommand::Pets
                if matches!(
                    args.trim().to_ascii_lowercase().as_str(),
                    "disable" | "disabled" | "hide" | "hidden" | "off" | "none"
                ) =>
            {
                self.app_event_tx.send(AppEvent::PetDisabled);
            }
            SlashCommand::Pets if !trimmed.is_empty() => {
                self.select_pet_by_id(args);
            }
            _ => self.dispatch_command(cmd),
        }
        if source == SlashCommandDispatchSource::Live && cmd != SlashCommand::Goal {
            self.bottom_pane.drain_pending_submission_state();
        }
    }

    pub(super) fn submit_queued_slash_prompt(
        &mut self,
        queued_message: QueuedUserMessage,
    ) -> QueueDrain {
        let QueuedUserMessage {
            user_message,
            pending_pastes,
            ..
        } = queued_message;
        let UserMessage {
            text,
            local_images,
            remote_image_urls,
            text_elements,
            mention_bindings,
        } = user_message;
        let Some((name, rest, rest_offset)) = parse_slash_name(&text) else {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        };

        if name.contains('/') {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        }

        let service_tier_commands = self.current_model_service_tier_commands();
        let Some(command) =
            find_slash_command(name, self.builtin_command_flags(), &service_tier_commands)
        else {
            self.add_info_message(
                format!(
                    r#"Unrecognized command '/{name}'. Type "/" for a list of supported commands."#
                ),
                /*hint*/ None,
            );
            return QueueDrain::Continue;
        };

        if rest.is_empty() {
            return match command {
                SlashCommandItem::Builtin(cmd) => {
                    self.dispatch_command(cmd);
                    self.queued_command_drain_result(cmd)
                }
                SlashCommandItem::ServiceTier(command) => {
                    self.handle_service_tier_command_dispatch(command);
                    QueueDrain::Continue
                }
            };
        }

        if !command.supports_inline_args() {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        }
        let SlashCommandItem::Builtin(cmd) = command else {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        };

        let trimmed_start = rest.trim_start();
        let leading_trimmed = rest.len().saturating_sub(trimmed_start.len());
        let trimmed_rest = trimmed_start.trim_end();
        let args_elements = Self::slash_command_args_elements(
            trimmed_rest,
            rest_offset + leading_trimmed,
            &text_elements,
        );
        self.dispatch_prepared_command_with_args(
            cmd,
            PreparedSlashCommandArgs {
                args: trimmed_rest.to_string(),
                text_elements: args_elements,
                pending_pastes,
                local_images,
                remote_image_urls,
                mention_bindings,
                source: SlashCommandDispatchSource::Queued,
            },
        );
        self.queued_command_drain_result(cmd)
    }

    fn builtin_command_flags(&self) -> BuiltinCommandFlags {
        #[cfg(target_os = "windows")]
        let allow_elevate_sandbox = {
            let windows_sandbox_level = crate::windows_sandbox::level_from_config(&self.config);
            matches!(windows_sandbox_level, WindowsSandboxLevel::RestrictedToken)
        };
        #[cfg(not(target_os = "windows"))]
        let allow_elevate_sandbox = false;

        BuiltinCommandFlags {
            collaboration_modes_enabled: self.collaboration_modes_enabled(),
            connectors_enabled: self.connectors_enabled(),
            plugins_command_enabled: self.config.features.enabled(Feature::Plugins),
            token_activity_command_enabled: self.has_codex_backend_auth,
            goal_command_enabled: self.config.features.enabled(Feature::Goals),
            service_tier_commands_enabled: self.fast_mode_enabled(),
            personality_command_enabled: self.config.features.enabled(Feature::Personality),
            allow_elevate_sandbox,
            side_conversation_active: self.active_side_conversation,
        }
    }

    fn ensure_usage_command_available(&mut self) -> bool {
        if self.has_codex_backend_auth {
            return true;
        }
        self.add_error_message(USAGE_CHATGPT_LOGIN_REQUIRED.to_string());
        false
    }

    fn queued_command_drain_result(&self, cmd: SlashCommand) -> QueueDrain {
        if self.is_user_turn_pending_or_running() || !self.bottom_pane.no_modal_or_popup_active() {
            return QueueDrain::Stop;
        }
        match cmd {
            SlashCommand::Ide
            | SlashCommand::Status
            | SlashCommand::Usage
            | SlashCommand::DebugConfig
            | SlashCommand::Ps
            | SlashCommand::Stop
            | SlashCommand::MemoryDrop
            | SlashCommand::MemoryUpdate
            | SlashCommand::Mcp
            | SlashCommand::Apps
            | SlashCommand::Plugins
            | SlashCommand::Rollout
            | SlashCommand::Copy
            | SlashCommand::Raw
            | SlashCommand::Vim
            | SlashCommand::Diff
            | SlashCommand::App
            | SlashCommand::Rename
            | SlashCommand::TestApproval => QueueDrain::Continue,
            SlashCommand::Feedback
            | SlashCommand::New
            | SlashCommand::Archive
            | SlashCommand::Delete
            | SlashCommand::Clear
            | SlashCommand::Resume
            | SlashCommand::Fork
            | SlashCommand::Init
            | SlashCommand::Compact
            | SlashCommand::Review
            | SlashCommand::Model
            | SlashCommand::Personality
            | SlashCommand::Plan
            | SlashCommand::Default
            | SlashCommand::AcceptEdits
            | SlashCommand::Auto
            | SlashCommand::DontAsk
            | SlashCommand::Bypass
            | SlashCommand::Goal
            | SlashCommand::Side
            | SlashCommand::Btw
            | SlashCommand::Keymap
            | SlashCommand::Agents
            | SlashCommand::Agent
            | SlashCommand::Tasks
            | SlashCommand::Remember
            | SlashCommand::Cancel
            | SlashCommand::MultiAgents
            | SlashCommand::Permissions
            | SlashCommand::ElevateSandbox
            | SlashCommand::SandboxReadRoot
            | SlashCommand::Experimental
            | SlashCommand::AutoReview
            | SlashCommand::Memories
            | SlashCommand::Quit
            | SlashCommand::Exit
            | SlashCommand::Logout
            | SlashCommand::Mention
            | SlashCommand::Skills
            | SlashCommand::Import
            | SlashCommand::Hooks
            | SlashCommand::Title
            | SlashCommand::Statusline
            | SlashCommand::Theme
            | SlashCommand::Pets => QueueDrain::Stop,
        }
    }

    fn slash_command_args_elements(
        rest: &str,
        rest_offset: usize,
        text_elements: &[TextElement],
    ) -> Vec<TextElement> {
        if rest.is_empty() || text_elements.is_empty() {
            return Vec::new();
        }
        text_elements
            .iter()
            .filter_map(|elem| {
                if elem.byte_range.end <= rest_offset {
                    return None;
                }
                let start = elem.byte_range.start.saturating_sub(rest_offset);
                let mut end = elem.byte_range.end.saturating_sub(rest_offset);
                if start >= rest.len() {
                    return None;
                }
                end = end.min(rest.len());
                (start < end).then_some(elem.map_range(|_| ByteRange { start, end }))
            })
            .collect()
    }

    fn ensure_slash_command_allowed_in_side_conversation(&mut self, cmd: SlashCommand) -> bool {
        if !self.active_side_conversation || cmd.available_in_side_conversation() {
            return true;
        }
        self.add_error_message(format!(
            "'/{}' is unavailable in side conversations. {SIDE_SLASH_COMMAND_UNAVAILABLE_HINT}",
            cmd.command()
        ));
        self.bottom_pane.drain_pending_submission_state();
        false
    }

    fn ensure_side_command_allowed_outside_review(&mut self, cmd: SlashCommand) -> bool {
        if !matches!(cmd, SlashCommand::Side | SlashCommand::Btw) || !self.review.is_review_mode {
            return true;
        }

        let command = cmd.command();
        self.add_error_message(format!(
            "'/{command}' is unavailable while code review is running."
        ));
        self.bottom_pane.drain_pending_submission_state();
        false
    }
}
