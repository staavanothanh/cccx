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
    pub fn attest_mode(&mut self) -> Result<(), ModeTransitionError> {
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

    /// Shift+Tab cycle: Plan → Default → AcceptEdits → Auto → Plan...
    /// Excludes DontAsk and Bypass per design.md §8.
    pub fn next_in_cycle(&self) -> MainMode {
        match self.current.clone() {
            MainMode::Plan => MainMode::Default,
            MainMode::Default => MainMode::AcceptEdits,
            MainMode::AcceptEdits => MainMode::Auto,
            MainMode::Auto => MainMode::Plan,
            MainMode::DontAsk | MainMode::Bypass => MainMode::Plan,
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
    fn shift_tab_cycle_excludes_dont_ask_and_bypass() {
        let mut state = CccxModeState::new(MainMode::Plan);
        assert_eq!(state.next_in_cycle(), MainMode::Default);
        state.change_mode(MainMode::Default).unwrap();
        assert_eq!(state.next_in_cycle(), MainMode::AcceptEdits);
        state.change_mode(MainMode::AcceptEdits).unwrap();
        assert_eq!(state.next_in_cycle(), MainMode::Auto);
        state.change_mode(MainMode::Auto).unwrap();
        assert_eq!(state.next_in_cycle(), MainMode::Plan);

        assert_eq!(
            CccxModeState::new(MainMode::DontAsk).next_in_cycle(),
            MainMode::Plan
        );
        assert_eq!(
            CccxModeState::new(MainMode::Bypass).next_in_cycle(),
            MainMode::Plan
        );
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
}
