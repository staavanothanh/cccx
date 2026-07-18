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
}
