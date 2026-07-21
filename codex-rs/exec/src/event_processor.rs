use std::path::Path;

use codex_app_server_protocol::ServerNotification;
use codex_core::config::Config;
use codex_protocol::protocol::SessionConfiguredEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexStatus {
    Running,
    InitiateShutdown,
}

pub(crate) trait EventProcessor {
    /// Print summary of effective configuration and user prompt.
    fn print_config_summary(
        &mut self,
        config: &Config,
        prompt: &str,
        session_configured: &SessionConfiguredEvent,
    );

    /// Handle a single typed app-server notification emitted by the agent.
    fn process_server_notification(&mut self, notification: ServerNotification) -> CodexStatus;

    /// Handle a local exec warning that is not represented as an app-server notification.
    fn process_warning(&mut self, message: String) -> CodexStatus;

    fn print_final_output(&mut self) {}

    /// cccx: headless mode auto-denies all approval requests.
    /// Called from handle_server_request when exec rejects an approval.
    /// EventProcessorWithJsonOutput overrides to track exit_code=2.
    fn on_approval_denied(&mut self, _request_id: String) {}

    /// cccx: headless output reports an approval request before the mandatory
    /// denial, so JSONL consumers can distinguish the requested action from
    /// the terminal decision. Human output deliberately keeps its existing
    /// behavior.
    fn on_permission_request(
        &mut self,
        _request_id: String,
        _agent_display_name: String,
        _action: String,
        _scope: String,
        _risk: String,
    ) {
    }
}

pub(crate) fn handle_last_message(last_agent_message: Option<&str>, output_file: &Path) {
    let message = last_agent_message.unwrap_or_default();
    write_last_message_file(message, Some(output_file));
    if last_agent_message.is_none() {
        eprintln!(
            "Warning: no last agent message; wrote empty content to {}",
            output_file.display()
        );
    }
}

fn write_last_message_file(contents: &str, last_message_path: Option<&Path>) {
    if let Some(path) = last_message_path
        && let Err(e) = std::fs::write(path, contents)
    {
        eprintln!("Failed to write last message file {path:?}: {e}");
    }
}
