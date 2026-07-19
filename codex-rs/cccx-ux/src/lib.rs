//! cccx interactive UX contracts.
//!
//! This crate intentionally contains deterministic command parsing and view
//! rendering only. Runtime TUI wiring stays in `codex-tui` and can be batched
//! later with other heavy integration work.

use thiserror::Error;

pub const CCCX_UX_SCHEMA_VERSION: &str = "cccx.ux.v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CccxCommand {
    Agents(AgentsCommand),
    Tasks,
    Permissions(PermissionsCommand),
    Remember { note: Option<String> },
    Cancel { all: bool, target: Option<String> },
    // Mode commands (design.md §8 + architecture.md §4)
    Mode(MainModeKind),
    // Goal verification
    Goal { expression: String },
    // Utility commands
    Review,
    Theme(Option<String>),
    Skills,
    Plugins,
    Mcp,
    Help { section: Option<String> },
}

/// User-visible mode names that match the 6-mode system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainModeKind {
    Plan,
    Default,
    AcceptEdits,
    Auto,
    DontAsk,
    Bypass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentsCommand {
    List,
    View { agent_id: String },
    Logs { agent_id: String },
    Resume { agent_id: String },
    Cancel { agent_id: String },
    Model { preset: String, route: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionsCommand {
    List,
    Approve { request_id: String },
    Deny { request_id: String },
    Inspect { request_id: String },
    BatchApprove { request_ids: Vec<String> },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CommandParseError {
    #[error("unknown cccx command `{0}`")]
    UnknownCommand(String),
    #[error("command `{command}` is missing required argument `{argument}`")]
    MissingArgument {
        command: &'static str,
        argument: &'static str,
    },
    #[error("command `{0}` has too many arguments")]
    TooManyArguments(&'static str),
}

pub fn parse_cccx_command(input: &str) -> Result<Option<CccxCommand>, CommandParseError> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return Ok(None);
    }

    let mut parts = trimmed.split_whitespace();
    let Some(command) = parts.next() else {
        return Ok(None);
    };

    match command {
        "/agents" => parse_agents(parts.collect())
            .map(CccxCommand::Agents)
            .map(Some),
        "/tasks" => no_args("/tasks", parts.collect()).map(|()| Some(CccxCommand::Tasks)),
        "/permissions" => parse_permissions(parts.collect())
            .map(CccxCommand::Permissions)
            .map(Some),
        "/remember" => Ok(Some(CccxCommand::Remember {
            note: trimmed
                .strip_prefix("/remember")
                .map(str::trim)
                .filter(|note| !note.is_empty())
                .map(str::to_string),
        })),
        "/cancel" => parse_cancel(parts.collect()).map(Some),
        // Mode commands
        "/plan" => Ok(Some(CccxCommand::Mode(MainModeKind::Plan))),
        "/default" => Ok(Some(CccxCommand::Mode(MainModeKind::Default))),
        "/accept-edits" => Ok(Some(CccxCommand::Mode(MainModeKind::AcceptEdits))),
        "/auto" => Ok(Some(CccxCommand::Mode(MainModeKind::Auto))),
        "/dont-ask" => {
            let rest: Vec<&str> = parts.collect();
            if rest.first() == Some(&"--confirm") {
                Ok(Some(CccxCommand::Mode(MainModeKind::DontAsk)))
            } else {
                Ok(Some(CccxCommand::Mode(MainModeKind::DontAsk)))
            }
        }
        "/bypass" => {
            let rest: Vec<&str> = parts.collect();
            if rest.first() == Some(&"--confirm") {
                Ok(Some(CccxCommand::Mode(MainModeKind::Bypass)))
            } else {
                Ok(Some(CccxCommand::Mode(MainModeKind::Bypass)))
            }
        }
        // Goal / review / theme / skills / plugins / mcp / help
        "/goal" => {
            let rest: Vec<&str> = parts.collect();
            Ok(Some(CccxCommand::Goal {
                expression: rest.join(" "),
            }))
        }
        "/review" => Ok(Some(CccxCommand::Review)),
        "/theme" => {
            let rest: Vec<&str> = parts.collect();
            Ok(Some(CccxCommand::Theme(
                rest.first().map(|s| s.to_string()),
            )))
        }
        "/skills" => Ok(Some(CccxCommand::Skills)),
        "/plugins" => Ok(Some(CccxCommand::Plugins)),
        "/mcp" => Ok(Some(CccxCommand::Mcp)),
        "/help" => {
            let rest: Vec<&str> = parts.collect();
            Ok(Some(CccxCommand::Help {
                section: rest.first().map(|s| s.to_string()),
            }))
        }
        other => Err(CommandParseError::UnknownCommand(other.to_string())),
    }
}

fn parse_agents(parts: Vec<&str>) -> Result<AgentsCommand, CommandParseError> {
    match parts.as_slice() {
        [] | ["list"] => Ok(AgentsCommand::List),
        ["view", agent_id] => Ok(AgentsCommand::View {
            agent_id: (*agent_id).to_string(),
        }),
        ["logs", agent_id] => Ok(AgentsCommand::Logs {
            agent_id: (*agent_id).to_string(),
        }),
        ["resume", agent_id] => Ok(AgentsCommand::Resume {
            agent_id: (*agent_id).to_string(),
        }),
        ["cancel", agent_id] => Ok(AgentsCommand::Cancel {
            agent_id: (*agent_id).to_string(),
        }),
        ["model", preset, route] => Ok(AgentsCommand::Model {
            preset: (*preset).to_string(),
            route: (*route).to_string(),
        }),
        ["view"] => missing("/agents view", "agent_id"),
        ["logs"] => missing("/agents logs", "agent_id"),
        ["resume"] => missing("/agents resume", "agent_id"),
        ["cancel"] => missing("/agents cancel", "agent_id"),
        ["model"] | ["model", _] => missing("/agents model", "preset route"),
        [unknown, ..] => Err(CommandParseError::UnknownCommand(format!(
            "/agents {unknown}"
        ))),
    }
}

fn parse_permissions(parts: Vec<&str>) -> Result<PermissionsCommand, CommandParseError> {
    match parts.as_slice() {
        [] => Ok(PermissionsCommand::List),
        ["approve", request_id] => Ok(PermissionsCommand::Approve {
            request_id: (*request_id).to_string(),
        }),
        ["deny", request_id] => Ok(PermissionsCommand::Deny {
            request_id: (*request_id).to_string(),
        }),
        ["inspect", request_id] => Ok(PermissionsCommand::Inspect {
            request_id: (*request_id).to_string(),
        }),
        ["batch-approve", request_ids @ ..] if !request_ids.is_empty() => {
            Ok(PermissionsCommand::BatchApprove {
                request_ids: request_ids.iter().map(|id| (*id).to_string()).collect(),
            })
        }
        ["approve"] => missing("/permissions approve", "request_id"),
        ["deny"] => missing("/permissions deny", "request_id"),
        ["inspect"] => missing("/permissions inspect", "request_id"),
        ["batch-approve"] => missing("/permissions batch-approve", "request_ids"),
        [unknown, ..] => Err(CommandParseError::UnknownCommand(format!(
            "/permissions {unknown}"
        ))),
    }
}

fn parse_cancel(parts: Vec<&str>) -> Result<CccxCommand, CommandParseError> {
    match parts.as_slice() {
        [] => Ok(CccxCommand::Cancel {
            all: false,
            target: None,
        }),
        ["--all"] => Ok(CccxCommand::Cancel {
            all: true,
            target: None,
        }),
        [agent_id] => Ok(CccxCommand::Cancel {
            all: false,
            target: Some((*agent_id).to_string()),
        }),
        _ => Err(CommandParseError::TooManyArguments("/cancel")),
    }
}

fn no_args(command: &'static str, parts: Vec<&str>) -> Result<(), CommandParseError> {
    if parts.is_empty() {
        Ok(())
    } else {
        Err(CommandParseError::TooManyArguments(command))
    }
}

fn missing<T>(command: &'static str, argument: &'static str) -> Result<T, CommandParseError> {
    Err(CommandParseError::MissingArgument { command, argument })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLineView {
    pub mode: String,
    pub harness: String,
    pub model_route: String,
    pub verified_tasks: usize,
    pub total_tasks: usize,
    pub context_percent: u8,
    pub active_permission_request_id: Option<String>,
    pub queued_permission_count: usize,
}

pub fn render_status_line(view: &StatusLineView) -> String {
    let permission = match (
        view.active_permission_request_id.as_deref(),
        view.queued_permission_count,
    ) {
        (Some(active), queued) => format!("{active} active + {queued} queued"),
        (None, 0) => "idle".to_string(),
        (None, queued) => format!("none active + {queued} queued"),
    };

    format!(
        "[cccx] mode:{}  harness:{}  model:{}  tasks:{}/{}  ctx:{}%  perm:{}",
        view.mode,
        view.harness,
        view.model_route,
        view.verified_tasks,
        view.total_tasks,
        view.context_percent,
        permission
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentListView {
    pub active_count: usize,
    pub resumable_count: usize,
    pub rows: Vec<AgentListRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentListRow {
    pub display_name: String,
    pub role: String,
    pub objective: String,
    pub status: String,
    pub elapsed: String,
    pub scope: String,
    pub model_route: String,
}

pub fn render_agent_list(view: &AgentListView) -> String {
    let mut lines = vec![format!(
        "Agents — {} active · {} resumable",
        view.active_count, view.resumable_count
    )];

    for row in &view.rows {
        lines.push(format!(
            "• {}  {}  \"{}\"  {}  {}  scope:{}  model:{}",
            row.display_name,
            row.role,
            row.objective,
            row.status,
            row.elapsed,
            row.scope,
            row.model_route
        ));
    }

    lines.push(
        "Commands: /agents view <id> · /agents logs <id> · /agents cancel <id> · /agents resume <id>"
            .to_string(),
    );
    lines.join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionQueueView {
    pub active: Option<PermissionRequestRow>,
    pub queued: Vec<PermissionRequestRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequestRow {
    pub request_id: String,
    pub agent_display_name: String,
    pub action: String,
    pub scope: String,
    pub risk: String,
}

pub fn render_permission_queue(view: &PermissionQueueView) -> String {
    let active_count = usize::from(view.active.is_some());
    let mut lines = vec![format!(
        "Permission Queue — {} active · {} queued",
        active_count,
        view.queued.len()
    )];

    lines.push(String::new());
    lines.push("ACTIVE".to_string());
    match &view.active {
        Some(row) => lines.push(render_permission_row(row)),
        None => lines.push("(none)".to_string()),
    }

    lines.push(String::new());
    lines.push("QUEUED".to_string());
    if view.queued.is_empty() {
        lines.push("(none)".to_string());
    } else {
        lines.extend(view.queued.iter().map(render_permission_row));
    }

    lines.push(String::new());
    lines.push(
        "Actions: /permissions approve <id> · /permissions deny <id> · /permissions inspect <id>"
            .to_string(),
    );
    lines.join("\n")
}

fn render_permission_row(row: &PermissionRequestRow) -> String {
    format!(
        "{}  {}  action:{}  scope:{}  risk:{}",
        row.request_id, row.agent_display_name, row.action, row.scope, row.risk
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_namespaced_agent_commands() {
        assert_eq!(
            parse_cccx_command("/agents logs b2-research"),
            Ok(Some(CccxCommand::Agents(AgentsCommand::Logs {
                agent_id: "b2-research".to_string()
            })))
        );
        assert_eq!(
            parse_cccx_command("/agents model cx_auditor 9router/anthropic/claude-sonnet-4.5"),
            Ok(Some(CccxCommand::Agents(AgentsCommand::Model {
                preset: "cx_auditor".to_string(),
                route: "9router/anthropic/claude-sonnet-4.5".to_string()
            })))
        );
    }

    #[test]
    fn parses_permission_commands_with_stable_request_ids() {
        assert_eq!(
            parse_cccx_command("/permissions batch-approve pr-18 pr-21"),
            Ok(Some(CccxCommand::Permissions(
                PermissionsCommand::BatchApprove {
                    request_ids: vec!["pr-18".to_string(), "pr-21".to_string()]
                }
            )))
        );
    }

    #[test]
    fn parses_all_design_mode_slash_commands() {
        for command in [
            "/plan",
            "/default",
            "/accept-edits",
            "/auto",
            "/dont-ask",
            "/bypass --confirm",
        ] {
            assert!(
                parse_cccx_command(command).is_ok(),
                "mode command should be registered: {command}"
            );
        }
    }

    #[test]
    fn parses_design_task_goal_and_utility_commands() {
        for command in [
            "/goal tests pass",
            "/review",
            "/theme ansi",
            "/skills",
            "/plugins",
            "/mcp",
            "/help --shortcuts",
        ] {
            assert!(
                parse_cccx_command(command).is_ok(),
                "design command should be registered: {command}"
            );
        }
    }

    #[test]
    fn rejects_legacy_agent_singular_command() {
        assert!(matches!(
            parse_cccx_command("/agent logs a1"),
            Err(CommandParseError::UnknownCommand(command)) if command == "/agent"
        ));
    }

    #[test]
    fn render_status_line_snapshot() {
        let rendered = render_status_line(&StatusLineView {
            mode: "Auto".to_string(),
            harness: "CC".to_string(),
            model_route: "claude-sonnet-4".to_string(),
            verified_tasks: 3,
            total_tasks: 7,
            context_percent: 58,
            active_permission_request_id: Some("pr-17".to_string()),
            queued_permission_count: 3,
        });

        assert_eq!(
            rendered,
            "[cccx] mode:Auto  harness:CC  model:claude-sonnet-4  tasks:3/7  ctx:58%  perm:pr-17 active + 3 queued"
        );
    }

    #[test]
    fn render_agent_list_snapshot() {
        let rendered = render_agent_list(&AgentListView {
            active_count: 1,
            resumable_count: 1,
            rows: vec![
                AgentListRow {
                    display_name: "a1-auth-fix".to_string(),
                    role: "CX implementer".to_string(),
                    objective: "fix login timeout".to_string(),
                    status: "running".to_string(),
                    elapsed: "45s".to_string(),
                    scope: "src/auth/*".to_string(),
                    model_route: "inherit-main".to_string(),
                },
                AgentListRow {
                    display_name: "a2-auth-audit".to_string(),
                    role: "CX auditor".to_string(),
                    objective: "review auth.ts patch".to_string(),
                    status: "resumable".to_string(),
                    elapsed: "checkpoint".to_string(),
                    scope: "src/auth/*".to_string(),
                    model_route: "sonnet-4.5".to_string(),
                },
            ],
        });

        assert_eq!(
            rendered,
            "Agents — 1 active · 1 resumable\n\
• a1-auth-fix  CX implementer  \"fix login timeout\"  running  45s  scope:src/auth/*  model:inherit-main\n\
• a2-auth-audit  CX auditor  \"review auth.ts patch\"  resumable  checkpoint  scope:src/auth/*  model:sonnet-4.5\n\
Commands: /agents view <id> · /agents logs <id> · /agents cancel <id> · /agents resume <id>"
        );
    }

    #[test]
    fn render_permission_queue_snapshot() {
        let rendered = render_permission_queue(&PermissionQueueView {
            active: Some(PermissionRequestRow {
                request_id: "pr-17".to_string(),
                agent_display_name: "a1-auth-fix".to_string(),
                action: "edit".to_string(),
                scope: "src/auth/session.ts".to_string(),
                risk: "medium".to_string(),
            }),
            queued: vec![PermissionRequestRow {
                request_id: "pr-18".to_string(),
                agent_display_name: "a2-auth-audit".to_string(),
                action: "shell".to_string(),
                scope: "cargo test".to_string(),
                risk: "low".to_string(),
            }],
        });

        assert_eq!(
            rendered,
            "Permission Queue — 1 active · 1 queued\n\
\n\
ACTIVE\n\
pr-17  a1-auth-fix  action:edit  scope:src/auth/session.ts  risk:medium\n\
\n\
QUEUED\n\
pr-18  a2-auth-audit  action:shell  scope:cargo test  risk:low\n\
\n\
Actions: /permissions approve <id> · /permissions deny <id> · /permissions inspect <id>"
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 12 — UX completion contract verification
    // ═══════════════════════════════════════════════════════════════════
    //
    // task.md Phase 12: "các command và UI cccx-specific trong design.md
    // hoạt động hoặc được ẩn rõ nếu deferred; không để placeholder."
    //
    // TUI integration tests are blocked on Windows (v8 build issue).
    // These contract tests prove the UX crate meets design.md §15 requirements
    // at the parser + renderer level.

    /// Phase 12: Every command from design.md §15.2 must parse without error.
    /// Covers all command groups: Mode, Tasks, Agents, Memory, Permissions, Utils.
    #[test]
    fn p12_all_design_commands_parse_without_error() {
        let commands = [
            // Mode & Session — cccx-ux parsers (design.md:714-731)
            "/plan",
            "/default",
            "/accept-edits",
            "/auto",
            "/dont-ask",
            "/bypass --confirm",
            "/theme monokai",
            // Task & Goal (design.md:733-738)
            "/tasks",
            "/goal all tests pass",
            "/review",
            // Agent Control (design.md:740-748)
            "/agents list",
            "/agents view a1",
            "/agents logs a1",
            "/agents resume a1",
            "/agents cancel a1",
            "/agents model cx_auditor 9router/deepseek-v4",
            "/cancel",
            "/cancel --all",
            "/cancel a1",
            // Memory (design.md:750-753)
            "/remember project uses 9router for all LLM calls",
            "/remember",
            // Permissions (design.md:755-760)
            "/permissions",
            "/permissions approve pr-1",
            "/permissions deny pr-1",
            "/permissions inspect pr-1",
            "/permissions batch-approve pr-1 pr-2",
            // Utilities — cccx-ux parsers (design.md:762-778)
            "/mcp",
            "/skills",
            "/plugins",
            "/goal fix the bug",
            "/help",
            "/help --shortcuts",
            "/help /agents",
            "/theme dark",
        ];

        for command in commands {
            let result = parse_cccx_command(command);
            assert!(
                result.is_ok(),
                "Phase 12: design.md command must parse without error: `{command}`"
            );
        }

        // Commands handled by Codex native TUI (not cccx-ux parser):
        // /model, /status, /clear, /keymap, /vim, /new, /rename, /archive,
        // /delete, /exit, /quit, /compact, /stop, /diff, /copy, /raw,
        // /side, /btw, /init, /pets, /personality, /hooks, /autoreview,
        // /mention, /title, /resume
        // These are handled upstream and either work or are gated by
        // is_visible=false via surface_gate_enabled().
    }

    /// Phase 12: StatusLineView renderer produces stable identifiers.
    /// Accessibility rule: no color-dependent information, stable text format.
    #[test]
    fn p12_status_line_contains_all_required_fields() {
        let view = StatusLineView {
            mode: "Plan".to_string(),
            harness: "CC".to_string(),
            model_route: "9router/deepseek-v4".to_string(),
            verified_tasks: 2,
            total_tasks: 5,
            context_percent: 72,
            active_permission_request_id: Some("pr-42".to_string()),
            queued_permission_count: 1,
        };

        let rendered = render_status_line(&view);

        // Every required field must appear in the status line
        assert!(rendered.contains("[cccx]"), "missing brand prefix");
        assert!(rendered.contains("mode:Plan"), "missing mode");
        assert!(rendered.contains("harness:CC"), "missing harness");
        assert!(
            rendered.contains("model:9router/deepseek-v4"),
            "missing model route"
        );
        assert!(rendered.contains("tasks:2/5"), "missing task progress");
        assert!(rendered.contains("ctx:72%"), "missing context percentage");
        assert!(
            rendered.contains("perm:pr-42"),
            "missing permission request id"
        );
        assert!(
            rendered.contains("1 queued"),
            "missing queued permission count"
        );

        // Accessibility: no ANSI color escape codes
        assert!(
            !rendered.contains("\x1b["),
            "Phase 12 accessibility: status line must not contain ANSI escape codes"
        );
    }

    /// Phase 12: All renderers use stable identifiers, not color alone.
    /// Proves that permission queue, agent list, and status line outputs
    /// are text-based with explicit labels — screen reader compatible.
    #[test]
    fn p12_renderers_produce_text_only_no_ansi_escape_codes() {
        // Status line
        let sl = render_status_line(&StatusLineView {
            mode: "Auto".to_string(),
            harness: "CC".to_string(),
            model_route: "oc/deepseek-v4".to_string(),
            verified_tasks: 0,
            total_tasks: 1,
            context_percent: 0,
            active_permission_request_id: None,
            queued_permission_count: 0,
        });
        assert!(
            !sl.contains("\x1b["),
            "status line must have no ANSI escapes"
        );

        // Agent list
        let al = render_agent_list(&AgentListView {
            active_count: 1,
            resumable_count: 0,
            rows: vec![AgentListRow {
                display_name: "a1".to_string(),
                role: "CX".to_string(),
                objective: "test".to_string(),
                status: "running".to_string(),
                elapsed: "30s".to_string(),
                scope: "src/".to_string(),
                model_route: "inherit-main".to_string(),
            }],
        });
        assert!(
            !al.contains("\x1b["),
            "agent list must have no ANSI escapes"
        );

        // Permission queue
        let pq = render_permission_queue(&PermissionQueueView {
            active: None,
            queued: vec![],
        });
        assert!(
            !pq.contains("\x1b["),
            "permission queue must have no ANSI escapes"
        );
    }

    /// Phase 12: Agent list rows carry stable identifiers (displayName, role,
    /// objective) that do not depend on color or position alone.
    #[test]
    fn p12_agent_list_rows_carry_stable_identifiers() {
        let view = AgentListView {
            active_count: 2,
            resumable_count: 1,
            rows: vec![
                AgentListRow {
                    display_name: "a1-auth-fix".to_string(),
                    role: "CX:implementer".to_string(),
                    objective: "fix auth bug".to_string(),
                    status: "running".to_string(),
                    elapsed: "45s".to_string(),
                    scope: "src/auth/*".to_string(),
                    model_route: "9router/sonnet".to_string(),
                },
                AgentListRow {
                    display_name: "a2-auth-audit".to_string(),
                    role: "CX:auditor".to_string(),
                    objective: "review auth.ts".to_string(),
                    status: "resumable".to_string(),
                    elapsed: "checkpoint".to_string(),
                    scope: "src/auth/*".to_string(),
                    model_route: "9router/claude".to_string(),
                },
            ],
        };

        let rendered = render_agent_list(&view);

        // Each agent must have its stable identifier (displayName) present
        assert!(
            rendered.contains("a1-auth-fix"),
            "missing agent 1 display name"
        );
        assert!(
            rendered.contains("a2-auth-audit"),
            "missing agent 2 display name"
        );

        // Roles must be present (not color-coded instead)
        assert!(rendered.contains("CX:implementer"), "missing agent 1 role");
        assert!(rendered.contains("CX:auditor"), "missing agent 2 role");

        // Summary line must show count
        assert!(rendered.contains("2 active"), "missing active agent count");
        assert!(
            rendered.contains("1 resumable"),
            "missing resumable agent count"
        );
    }

    /// Phase 12: Deferred and removed commands are documented.
    /// Proves that commands listed as deferred/non-MVP in design.md are
    /// either rejected with clear errors or not falsely claimed as implemented.
    #[test]
    fn p12_deferred_and_removed_commands_are_documented() {
        // Commands explicitly removed (design.md:789-794)
        assert!(matches!(
            parse_cccx_command("/agent logs a1"),
            Err(CommandParseError::UnknownCommand(command)) if command == "/agent"
        ));
        assert!(matches!(
            parse_cccx_command("/multi-agents"),
            Err(CommandParseError::UnknownCommand(_))
        ));

        // Commands deferred (design.md:780-787) — hidden via is_visible=false
        // but code kept. They must either not exist or fail gracefully.
        // cccx-ux parser does not include these, so they produce UnknownCommand.
        for cmd in [
            "/elevate-sandbox",
            "/sandbox-add-read-dir",
            "/fork",
            "/app",
            "/apps",
            "/usage",
            "/debug-config",
            "/statusline",
            "/rollout",
            "/test-approval",
            "/ps",
            "/pets",
            "/personality",
            "/reroute",
        ] {
            assert!(
                matches!(
                    parse_cccx_command(cmd),
                    Err(CommandParseError::UnknownCommand(_))
                ),
                "Phase 12: hidden/deferred command `{cmd}` must not be falsely registered as cccx command"
            );
        }

        // Future commands (design.md:786-787)
        for cmd in ["/import", "/ide"] {
            assert!(
                matches!(
                    parse_cccx_command(cmd),
                    Err(CommandParseError::UnknownCommand(_))
                ),
                "Phase 12: future command `{cmd}` must not be falsely registered as implemented"
            );
        }
    }

    // TUI integration tests (task.md Phase 12 items 1, 4, 5, 6):
    // - startup banner: persona art, mode, model, folder
    // - /theme, /pets: no-op safe implementation
    // - network error: retry UX text
    // - secure input: no history/transcript leak
    // These require TUI test execution which is blocked on Windows
    // by the v8 build dependency. Documented in task.md and implementer-issues.md.
}
