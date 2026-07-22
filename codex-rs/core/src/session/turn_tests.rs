use super::*;
use codex_extension_api::ExtensionData;
use codex_extension_api::TurnItemContributor;
use codex_protocol::ResponseItemId;
use codex_protocol::items::AgentMessageContent;
use pretty_assertions::assert_eq;
use std::sync::Arc;

struct RewriteAgentMessageContributor;

impl TurnItemContributor for RewriteAgentMessageContributor {
    fn contribute<'a>(
        &'a self,
        _thread_store: &'a ExtensionData,
        _turn_store: &'a ExtensionData,
        item: &'a mut TurnItem,
    ) -> codex_extension_api::ExtensionFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if let TurnItem::AgentMessage(agent_message) = item {
                agent_message.content = vec![AgentMessageContent::Text {
                    text: "plan contributed assistant text".to_string(),
                }];
            }
            Ok(())
        })
    }
}

fn assistant_output_text(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: Some(ResponseItemId::with_suffix("msg", "1")),
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

#[tokio::test]
async fn plan_mode_uses_contributed_turn_item_for_last_agent_message() {
    let (mut session, turn_context) = crate::session::tests::make_session_and_context().await;
    let mut builder = codex_extension_api::ExtensionRegistryBuilder::new();
    builder.turn_item_contributor(Arc::new(RewriteAgentMessageContributor));
    session.services.extensions = Arc::new(builder.build());
    let turn_store = ExtensionData::new(turn_context.sub_id.clone());
    let mut state = PlanModeStreamState::new(&turn_context.sub_id);
    let mut last_agent_message = None;
    let item = assistant_output_text("original assistant text");

    let handled = handle_assistant_item_done_in_plan_mode(
        &session,
        &turn_context,
        &turn_store,
        &item,
        &mut state,
        /*previously_active_item*/ None,
        &mut last_agent_message,
    )
    .await;

    assert!(handled);
    assert_eq!(
        last_agent_message.as_deref(),
        Some("plan contributed assistant text")
    );
}

#[test]
fn cccx_blocks_retryable_stream_error_after_tool_execution() {
    let outcome: CodexResult<()> = Err(CodexErr::Stream("disconnect".to_string(), None));

    let err = block_retry_after_tool_execution(outcome, true, true)
        .expect_err("retryable stream error after tool execution must be blocked");

    assert!(
        err.to_string()
            .contains("automatic retry is blocked to avoid repeating side effects"),
        "unexpected error: {err}"
    );
    assert!(
        !err.is_retryable(),
        "blocked side-effect retry must not remain retryable"
    );
}

#[test]
fn cccx_blocks_retryable_stream_error_after_user_visible_output() {
    let outcome = Err(CodexErr::Stream(
        "disconnect after text delta".to_string(),
        None,
    ));
    let err = block_retry_after_tool_execution::<()>(outcome, true, true)
        .expect_err("observable model output must disable transparent retry");

    assert!(
        err.to_string().contains("observable output or tool intent"),
        "unexpected guarded error: {err}"
    );
}

#[test]
fn non_cccx_or_no_tool_execution_keeps_retryable_stream_error() {
    let no_tool = block_retry_after_tool_execution::<()>(
        Err(CodexErr::Stream("disconnect".to_string(), None)),
        false,
        true,
    )
    .expect_err("original error should pass through");
    assert!(no_tool.is_retryable());

    let non_cccx = block_retry_after_tool_execution::<()>(
        Err(CodexErr::Stream("disconnect".to_string(), None)),
        true,
        false,
    )
    .expect_err("original error should pass through");
    assert!(non_cccx.is_retryable());
}
