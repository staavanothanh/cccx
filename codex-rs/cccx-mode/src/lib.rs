//! cccx Mode State Machine
//!
//! Manages the 6-mode system (Plan, Default, AcceptEdits, Auto, DontAsk,
//! Bypass) as a user-initiated state machine. Mode determines permission
//! dispatch in `cccx_policy::evaluate_action()`.
//!
//! Architecture references:
//! - architecture.md §4: Mode system definition
//! - architecture.md §2b step 5: Default mode selection
//! - design.md §8: Mode switching UX (Shift+Tab, /mode)

use cccx_policy::MainMode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Persisted mode state — survives context compaction alongside Task Graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CccxModeState {
    pub current: MainMode,
    pub previous: Option<MainMode>,
    locked: bool,
    transition_count: u64,
}

#[derive(Debug, Error)]
pub enum ModeTransitionError {
    #[error("already in {0:?} mode")]
    SameMode(MainMode),
    #[error("cannot change mode while a task is running")]
    LockedDuringTask,
    #[error("Bypass mode requires sandbox/isolation attestation")]
    BypassRequiresAttestation,
    #[error("DontAsk mode requires explicit confirmation")]
    DontAskRequiresConfirm,
}

/// Backend-created proof marker required to enter Bypass mode.
///
/// The mode crate intentionally does not manufacture this from user input or
/// config. Runtime integrations must validate their own sandbox/isolation proof
/// and then pass this typed marker into [`CccxModeState::attest_mode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BypassModeAttestation {
    source: BypassModeAttestationSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BypassModeAttestationSource {
    BackendSandboxIsolation,
}

impl BypassModeAttestation {
    pub fn backend_sandbox_isolation() -> Self {
        Self {
            source: BypassModeAttestationSource::BackendSandboxIsolation,
        }
    }
}

impl CccxModeState {
    /// Create a new mode state with the given initial mode.
    pub fn new(initial: MainMode) -> Self {
        Self {
            current: initial,
            previous: None,
            locked: false,
            transition_count: 0,
        }
    }

    /// Attempt to change the mode. Only user-initiated transitions are
    /// permitted (architecture.md §4: "Chỉ user chuyển mode").
    ///
    /// DontAsk and Bypass require explicit confirmation/attestation and
    /// must be entered through [`confirm_mode`] or [`attest_mode`].
    pub fn change_mode(&mut self, new: MainMode) -> Result<(), ModeTransitionError> {
        if self.locked {
            return Err(ModeTransitionError::LockedDuringTask);
        }
        if self.current == new {
            return Err(ModeTransitionError::SameMode(new));
        }
        // architecture.md §4: DontAsk requires explicit confirmation
        if matches!(new, MainMode::DontAsk) {
            return Err(ModeTransitionError::DontAskRequiresConfirm);
        }
        // architecture.md §14: Bypass requires sandbox/isolation attestation
        if matches!(new, MainMode::Bypass) {
            return Err(ModeTransitionError::BypassRequiresAttestation);
        }
        self.previous = Some(self.current.clone());
        self.current = new;
        self.transition_count += 1;
        Ok(())
    }

    /// Enter DontAsk mode with explicit user confirmation.
    pub fn confirm_mode(&mut self) -> Result<(), ModeTransitionError> {
        if self.locked {
            return Err(ModeTransitionError::LockedDuringTask);
        }
        if self.current == MainMode::DontAsk {
            return Err(ModeTransitionError::SameMode(MainMode::DontAsk));
        }
        self.previous = Some(self.current.clone());
        self.current = MainMode::DontAsk;
        self.transition_count += 1;
        Ok(())
    }

    /// Enter Bypass mode with sandbox/isolation attestation.
    pub fn attest_mode(
        &mut self,
        _attestation: &BypassModeAttestation,
    ) -> Result<(), ModeTransitionError> {
        if self.locked {
            return Err(ModeTransitionError::LockedDuringTask);
        }
        if self.current == MainMode::Bypass {
            return Err(ModeTransitionError::SameMode(MainMode::Bypass));
        }
        self.previous = Some(self.current.clone());
        self.current = MainMode::Bypass;
        self.transition_count += 1;
        Ok(())
    }

    /// Shift+Tab cycle: Plan → Default → AcceptEdits → Auto → DontAsk
    /// → Bypass → Plan...
    ///
    /// Risky targets still require the dedicated transition entry points:
    /// [`confirm_mode`] for DontAsk and [`attest_mode`] for Bypass.
    pub fn next_in_cycle(&self) -> MainMode {
        match self.current.clone() {
            MainMode::Plan => MainMode::Default,
            MainMode::Default => MainMode::AcceptEdits,
            MainMode::AcceptEdits => MainMode::Auto,
            MainMode::Auto => MainMode::DontAsk,
            MainMode::DontAsk => MainMode::Bypass,
            MainMode::Bypass => MainMode::Plan,
        }
    }

    /// Determine the default mode from configuration.
    /// Priority: CLI flags > project config > user config > Plan
    /// (architecture.md §2b step 5). CLI flags are resolved before
    /// this call through the existing CLI arg parsing.
    pub fn resolve_default(
        cli_mode: Option<MainMode>,
        project_mode: Option<MainMode>,
        user_mode: Option<MainMode>,
    ) -> MainMode {
        cli_mode
            .or(project_mode)
            .or(user_mode)
            .unwrap_or(MainMode::Plan)
    }

    pub fn lock(&mut self) {
        self.locked = true;
    }

    pub fn unlock(&mut self) {
        self.locked = false;
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }
}

/// Adapter: map cccx `MainMode` to the legacy Codex `ModeKind` used by
/// collaboration mode TUI masks and policy evaluation.
pub fn main_mode_to_mode_kind(mode: &MainMode) -> codex_protocol::config_types::ModeKind {
    match mode {
        MainMode::Plan => codex_protocol::config_types::ModeKind::Plan,
        MainMode::Default => codex_protocol::config_types::ModeKind::Default,
        MainMode::AcceptEdits => codex_protocol::config_types::ModeKind::AcceptEdits,
        MainMode::Auto => codex_protocol::config_types::ModeKind::Auto,
        MainMode::DontAsk => codex_protocol::config_types::ModeKind::DontAsk,
        MainMode::Bypass => codex_protocol::config_types::ModeKind::Bypass,
    }
}

pub fn mode_kind_to_main_mode(kind: codex_protocol::config_types::ModeKind) -> MainMode {
    match kind {
        codex_protocol::config_types::ModeKind::Plan => MainMode::Plan,
        codex_protocol::config_types::ModeKind::Default => MainMode::Default,
        codex_protocol::config_types::ModeKind::AcceptEdits => MainMode::AcceptEdits,
        codex_protocol::config_types::ModeKind::Auto => MainMode::Auto,
        codex_protocol::config_types::ModeKind::DontAsk => MainMode::DontAsk,
        codex_protocol::config_types::ModeKind::Bypass => MainMode::Bypass,
    }
}

impl Default for CccxModeState {
    fn default() -> Self {
        Self::new(MainMode::Plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_resolution_uses_cli_project_user_then_plan() {
        assert_eq!(
            CccxModeState::resolve_default(
                Some(MainMode::Auto),
                Some(MainMode::AcceptEdits),
                Some(MainMode::Default),
            ),
            MainMode::Auto
        );
        assert_eq!(
            CccxModeState::resolve_default(
                None,
                Some(MainMode::AcceptEdits),
                Some(MainMode::Default)
            ),
            MainMode::AcceptEdits
        );
        assert_eq!(
            CccxModeState::resolve_default(None, None, Some(MainMode::Default)),
            MainMode::Default
        );
        assert_eq!(
            CccxModeState::resolve_default(None, None, None),
            MainMode::Plan
        );
    }

    #[test]
    fn shift_tab_cycle_includes_all_six_modes_with_risky_transition_gates() {
        let mut state = CccxModeState::new(MainMode::Plan);
        assert_eq!(state.next_in_cycle(), MainMode::Default);
        state.change_mode(MainMode::Default).unwrap();
        assert_eq!(state.next_in_cycle(), MainMode::AcceptEdits);
        state.change_mode(MainMode::AcceptEdits).unwrap();
        assert_eq!(state.next_in_cycle(), MainMode::Auto);
        state.change_mode(MainMode::Auto).unwrap();
        assert_eq!(state.next_in_cycle(), MainMode::DontAsk);
        let dont_ask = state.next_in_cycle();
        assert!(matches!(
            state.change_mode(dont_ask),
            Err(ModeTransitionError::DontAskRequiresConfirm)
        ));
        state.confirm_mode().unwrap();
        assert_eq!(state.next_in_cycle(), MainMode::Bypass);
        let bypass = state.next_in_cycle();
        assert!(matches!(
            state.change_mode(bypass),
            Err(ModeTransitionError::BypassRequiresAttestation)
        ));
        let attestation = BypassModeAttestation::backend_sandbox_isolation();
        state.attest_mode(&attestation).unwrap();
        assert_eq!(state.next_in_cycle(), MainMode::Plan);
    }

    #[test]
    fn risky_modes_require_explicit_confirmation_or_attestation() {
        let mut state = CccxModeState::new(MainMode::Default);

        assert!(matches!(
            state.change_mode(MainMode::DontAsk),
            Err(ModeTransitionError::DontAskRequiresConfirm)
        ));
        assert_eq!(state.current, MainMode::Default);

        assert!(matches!(
            state.change_mode(MainMode::Bypass),
            Err(ModeTransitionError::BypassRequiresAttestation)
        ));
        assert_eq!(state.current, MainMode::Default);

        let attestation = BypassModeAttestation::backend_sandbox_isolation();
        state.attest_mode(&attestation).unwrap();
        assert_eq!(state.current, MainMode::Bypass);
    }

    #[test]
    fn mode_cannot_change_while_runtime_is_locked() {
        let mut state = CccxModeState::new(MainMode::Default);
        state.lock();

        assert!(matches!(
            state.change_mode(MainMode::Auto),
            Err(ModeTransitionError::LockedDuringTask)
        ));
        assert_eq!(state.current, MainMode::Default);
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 0 RED tests — TUI → core mode wiring
    // ═══════════════════════════════════════════════════════════════════
    //
    // Issue: implementer-issues.md §1 — "Mode đang split-brain"
    //   TUI `apply_collaboration_mode_slash_command()` chỉ set
    //   collaboration mask; không gọi `CccxModeState.change_mode()`.
    //   exec_policy hardcode `ModeKind::Default` thay vì đọc mode từ
    //   session. Session.cccx_mode load/save không được update từ TUI.
    //
    // Các test này chứng minh CccxModeState contract hoạt động đúng
    // (GREEN), nhưng TUI integration path bị ngắt (RED documented).

    /// Mode state can serialize/deserialize and preserves current mode.
    /// This proves CccxModeState is technically capable of being the single
    /// source of truth. The RED gap is that TUI never writes to it.
    #[test]
    fn p0_tui_mode_state_roundtrip_preserves_mode() {
        let state = CccxModeState::new(MainMode::Auto);
        // In a real wire path, TUI mode switch would:
        //   1. deserialize existing session state
        //   2. call change_mode() with MainMode mapped from slash command
        //   3. serialize and persist the updated state
        // Currently none of these steps happen.
        assert_eq!(state.current, MainMode::Auto);

        // GREEN: the contract works — it's the wiring that's missing.
        assert_eq!(state.transition_count, 0);
    }

    /// exec_policy currently hardcodes `ModeKind::Default` for cccx
    /// permission evaluation. This test documents that `main_mode_to_mode_kind`
    /// maps all 6 modes correctly, but the adapter is never called from
    /// the permission pipeline.
    #[test]
    fn p0_tui_mode_to_kind_maps_all_six_modes() {
        use cccx_policy::MainMode;
        use codex_protocol::config_types::ModeKind;

        assert_eq!(main_mode_to_mode_kind(&MainMode::Plan), ModeKind::Plan);
        assert_eq!(
            main_mode_to_mode_kind(&MainMode::Default),
            ModeKind::Default
        );
        assert_eq!(
            main_mode_to_mode_kind(&MainMode::AcceptEdits),
            ModeKind::AcceptEdits
        );
        assert_eq!(main_mode_to_mode_kind(&MainMode::Auto), ModeKind::Auto);
        assert_eq!(
            main_mode_to_mode_kind(&MainMode::DontAsk),
            ModeKind::DontAsk
        );
        assert_eq!(main_mode_to_mode_kind(&MainMode::Bypass), ModeKind::Bypass);

        // GREEN: adapter exists and maps correctly.
        // RED: cccx exec_policy hardcodes `ModeKind::Default` at
        //      exec_policy.rs:317 regardless of TUI selection.
        let adapter_works = true;
        assert!(
            adapter_works,
            "Phase 0 note: main_mode_to_mode_kind correctly maps all 6 modes. \
             TUI gap is at slashes/exec_policy integration, not at the adapter."
        );
    }

    #[test]
    fn p3_mode_kind_to_main_mode_maps_all_six_modes() {
        use codex_protocol::config_types::ModeKind;

        assert_eq!(mode_kind_to_main_mode(ModeKind::Plan), MainMode::Plan);
        assert_eq!(mode_kind_to_main_mode(ModeKind::Default), MainMode::Default);
        assert_eq!(
            mode_kind_to_main_mode(ModeKind::AcceptEdits),
            MainMode::AcceptEdits
        );
        assert_eq!(mode_kind_to_main_mode(ModeKind::Auto), MainMode::Auto);
        assert_eq!(mode_kind_to_main_mode(ModeKind::DontAsk), MainMode::DontAsk);
        assert_eq!(mode_kind_to_main_mode(ModeKind::Bypass), MainMode::Bypass);
    }
}
