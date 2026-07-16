//! cccx MVP surface gate.
//!
//! The fork keeps upstream Codex code in place for mergeability, but the cccx
//! MVP must not advertise or run non-MVP surfaces by default.

pub const ALLOW_UPSTREAM_SURFACES_ENV: &str = "CCCX_MVP_ALLOW_UPSTREAM_SURFACES";

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.as_str(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(false)
}

pub fn surface_gate_enabled() -> bool {
    !env_truthy(ALLOW_UPSTREAM_SURFACES_ENV)
}

pub fn disabled_surface_message(surface: &str) -> String {
    format!(
        "cccx MVP disables {surface}. Use the cccx/9router local flow, or set {ALLOW_UPSTREAM_SURFACES_ENV}=1 to temporarily expose upstream Codex surfaces for debugging."
    )
}

pub fn is_cli_surface_disabled(surface: &str) -> bool {
    surface_gate_enabled()
        && matches!(
            surface,
            "login"
                | "logout"
                | "app"
                | "remote-control"
                | "cloud"
                | "cloud-tasks"
                | "plugin add"
                | "plugin marketplace"
        )
}

pub fn is_tui_slash_command_disabled(command: &str) -> bool {
    surface_gate_enabled()
        && matches!(
            command,
            "apps"
                | "plugins"
                | "memories"
                | "debug-m-drop"
                | "debug-m-update"
                | "logout"
                | "app"
                | "fork"
                | "usage"
                | "debug-config"
                | "statusline"
                | "rollout"
                | "test-approval"
                | "ps"
                | "agent"
                | "subagents"
        )
}
