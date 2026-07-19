//! cccx MVP surface gate.
//!
//! The fork keeps upstream Codex code in place for mergeability, but the cccx
//! MVP must not advertise or run non-MVP surfaces by default.
//!
//! Phase 1: `CCCX_MVP_ALLOW_UPSTREAM_SURFACES` env var controls ONLY UI surface
//! visibility (legacy commands, features). It does NOT disable the 9router
//! provider boundary — see `provider_boundary_active()` below.

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

/// Controls UI surface visibility (commands, features, login surfaces).
/// CAN be disabled via `CCCX_MVP_ALLOW_UPSTREAM_SURFACES=1` for debugging.
pub fn surface_gate_enabled() -> bool {
    !env_truthy(ALLOW_UPSTREAM_SURFACES_ENV)
}

/// Controls the 9router-only provider boundary — every LLM call MUST go
/// through 9router. This gate CANNOT be disabled by any env var or config
/// flag. It is a hard architectural invariant (architecture.md §2b step 2).
///
/// Phase 1 fix: separated from `surface_gate_enabled()` to prevent the
/// `CCCX_MVP_ALLOW_UPSTREAM_SURFACES=1` escape hatch from also disabling
/// the provider boundary.
pub fn provider_boundary_active() -> bool {
    // Always enforced. The env var only unlocks UI surfaces for debugging;
    // it does not open non-9router LLM routes.
    true
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase 1: verify provider_boundary_active() is truly invariant by
    /// inspecting its SOURCE at compile time.
    ///
    /// The old `surface_gate_enabled()` reads `CCCX_MVP_ALLOW_UPSTREAM_SURFACES`
    /// env var. When that var = 1, the function returns `false`. This value fed
    /// `ProviderGatewayBridge::new()`, making `bridge.enabled = false`, which
    /// made `check_fail_closed()` unconditionally return `true` — boundary
    /// bypassed.
    ///
    /// Phase 1 fix: `provider_boundary_active()` never reads any env var.
    /// This test proves it with compile-time source inspection.
    #[test]
    fn p1_boundary_fn_has_no_env_dependency() {
        let source = include_str!("cccx_mvp.rs");

        // Find the provider_boundary_active function body
        let fn_start = source
            .find("pub fn provider_boundary_active")
            .expect("boundary function must exist in this file");
        let body = &source[fn_start..];
        let close = body.find('}').expect("fn must have closing brace");
        let fn_body = &body[..close + 1];

        // Strip doc comments (/// ...) and line comments (// ...) before checking
        let code_only: String = fn_body
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                !trimmed.starts_with("///") && !trimmed.starts_with("//")
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            !code_only.contains("env"),
            "Phase 1 invariant: provider_boundary_active() code MUST NOT \
             read environment variables (comment references are okay)."
        );
        assert!(
            !code_only.contains("var("),
            "Phase 1 invariant: provider_boundary_active() MUST NOT call std::env::var()."
        );
        assert!(
            !code_only.contains("surface_gate"),
            "Phase 1 invariant: provider_boundary_active() MUST NOT delegate \
             to surface_gate_enabled()."
        );
        assert!(
            code_only.contains("true"),
            "Phase 1 invariant: provider_boundary_active() MUST return true."
        );
    }

    /// Phase 1: demonstrate the semantic split.
    ///
    /// `surface_gate_enabled()` CONTROLS UI surfaces — CAN be disabled.
    /// `provider_boundary_active()` ENFORCES 9router boundary — CANNOT be disabled.
    #[test]
    fn p1_surface_and_boundary_are_independent() {
        // provider_boundary_active() is unconditionally true
        assert!(provider_boundary_active());
        assert!(
            provider_boundary_active(),
            "Phase 1: provider boundary must always be active"
        );

        // surface_gate_enabled() may be true or false (depends on env var),
        // but it does NOT affect the boundary
        let ui_gate = surface_gate_enabled();
        let boundary = provider_boundary_active();

        // The boundary holds regardless of the UI gate state
        assert!(
            boundary,
            "Phase 1: boundary is invariant. UI gate state ({}) does not affect it.",
            if ui_gate { "enabled" } else { "disabled" }
        );
    }
}
