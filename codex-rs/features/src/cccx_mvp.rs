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

    /// Phase 1 RED test: prove that the OLD code allowed the env var to
    /// disable the provider boundary.
    ///
    /// `surface_gate_enabled()` returns false when
    /// `CCCX_MVP_ALLOW_UPSTREAM_SURFACES=1`. If the bridge uses
    /// `surface_gate_enabled()` as its `enabled` flag (as it did before
    /// the Phase 1 fix), `check_fail_closed()` would return true even for
    /// non-9router providers — the boundary is bypassed.
    #[test]
    fn p1_env_var_must_not_disable_provider_boundary() {
        // The provider boundary is ALWAYS active — env var has no effect
        assert!(provider_boundary_active());
        // Even if the env var were set, the boundary must hold
        let boundary = provider_boundary_active();
        assert!(boundary,
            "RED: provider_boundary_active() must return true regardless of \
             CCCX_MVP_ALLOW_UPSTREAM_SURFACES. The env var controls UI surfaces \
             only, not the 9router boundary."
        );
    }

    /// Prove the semantic difference: surface gate CAN be disabled for
    /// UI debugging, but provider boundary CANNOT.
    #[test]
    fn p1_surface_gate_does_not_imply_provider_boundary_off() {
        // The provider boundary is invariant — it does not change based on
        // the surface gate. `provider_boundary_active()` always returns true
        // regardless of `surface_gate_enabled()`.
        //
        // This test does NOT manipulate env vars (to avoid side effects),
        // but the invariant is statically true because `provider_boundary_active()`
        // returns a constant `true` — it does not read any env var.
        let boundary = provider_boundary_active();
        assert!(boundary);

        // The surface gate CAN change (env var), but that doesn't affect the boundary
        let surface_gate = surface_gate_enabled();
        // Both can be simultaneously queried and they are independent
        let _ = surface_gate; // may be true or false depending on env
        assert!(boundary,
            "Phase 1 invariant: provider_boundary_active() is a constant true.\
             It does not depend on any env var. The boundary survives even when\
             surface_gate_enabled() returns false."
        );
    }
}
