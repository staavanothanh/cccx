use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::rate_limits::parse_all_rate_limits;
use crate::telemetry::SseTelemetry;
use codex_client::ByteStream;
use codex_client::StreamResponse;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::{debug, warn};

const OPENAI_MODEL_HEADER: &str = "openai-model";
const REQUEST_ID_HEADER: &str = "x-request-id";

pub fn spawn_chat_completions_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
) -> ResponseStream {
    let rate_limit_snapshots = parse_all_rate_limits(&stream_response.headers);
    let upstream_request_id = stream_response
        .headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let server_model = stream_response
        .headers
        .get(OPENAI_MODEL_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string);

    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    let tx_event_backup = tx_event.clone();
    let handle = tokio::spawn(async move {
        if let Some(model) = server_model {
            let _ = tx_event.send(Ok(ResponseEvent::ServerModel(model))).await;
        }
        for snapshot in rate_limit_snapshots {
            let _ = tx_event.send(Ok(ResponseEvent::RateLimits(snapshot))).await;
        }
        process_chat_completions_sse(stream_response.bytes, tx_event, idle_timeout, telemetry)
            .await;
    });
    tokio::spawn(async move {
        if let Err(e) = handle.await {
            if e.is_panic() {
                let panic_payload = e.into_panic();
                let msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                let _ = tx_event_backup
                    .send(Err(ApiError::Stream(format!(
                        "SSE processing panicked: {msg}"
                    ))))
                    .await;
            }
        }
    });

    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionChunk {
    id: Option<String>,
    model: Option<String>,
    choices: Vec<ChatCompletionChoice>,
    usage: Option<ChatCompletionUsage>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionChoice {
    #[serde(rename = "index", default)]
    _index: usize,
    delta: ChatCompletionDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionDelta {
    role: Option<String>,
    content: Option<String>,
    #[serde(default, alias = "reasoning")]
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<ChatCompletionToolCallDelta>>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<ChatCompletionFunctionDelta>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionUsage {
    prompt_tokens: i64,
    #[serde(default)]
    completion_tokens: i64,
    #[serde(default)]
    total_tokens: i64,
    #[serde(default)]
    prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(default)]
    completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Debug, Default, Deserialize)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: i64,
}

#[derive(Debug, Default, Deserialize)]
struct CompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: i64,
}

impl From<ChatCompletionUsage> for TokenUsage {
    fn from(value: ChatCompletionUsage) -> Self {
        Self {
            input_tokens: value.prompt_tokens,
            cached_input_tokens: value
                .prompt_tokens_details
                .map(|details| details.cached_tokens)
                .unwrap_or(0),
            cache_write_input_tokens: 0,
            output_tokens: value.completion_tokens,
            reasoning_output_tokens: value
                .completion_tokens_details
                .map(|details| details.reasoning_tokens)
                .unwrap_or(0),
            total_tokens: value.total_tokens,
        }
    }
}

#[derive(Debug, Default)]
struct PendingToolCall {
    id: Option<String>,
    name: String,
    arguments: String,
}

#[derive(Debug, Default)]
struct ChatCompletionAccumulator {
    response_id: Option<String>,
    finish_reason: Option<String>,
    assistant_started: bool,
    assistant_text: String,
    tool_calls: BTreeMap<usize, PendingToolCall>,
    token_usage: Option<TokenUsage>,
}

impl ChatCompletionAccumulator {
    async fn ingest_chunk(
        &mut self,
        chunk: ChatCompletionChunk,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        if self.response_id.is_none() {
            self.response_id = chunk.id;
        }
        if let Some(model) = chunk.model {
            tx_event
                .send(Ok(ResponseEvent::ServerModel(model)))
                .await
                .map_err(|_| ApiError::Stream("chat completions stream receiver dropped".into()))?;
        }
        if let Some(usage) = chunk.usage {
            self.token_usage = Some(usage.into());
        }

        for choice in chunk.choices {
            self.finish_reason = choice.finish_reason.or(self.finish_reason.take());
            if matches!(choice.delta.role.as_deref(), Some("assistant")) {
                self.ensure_assistant_started(tx_event).await?;
            }
            if let Some(content) = choice.delta.content {
                self.ensure_assistant_started(tx_event).await?;
                self.assistant_text.push_str(&content);
                tx_event
                    .send(Ok(ResponseEvent::OutputTextDelta(content)))
                    .await
                    .map_err(|_| {
                        ApiError::Stream("chat completions stream receiver dropped".into())
                    })?;
            }
            if let Some(reasoning_content) = choice.delta.reasoning_content {
                self.ensure_assistant_started(tx_event).await?;
                tx_event
                    .send(Ok(ResponseEvent::ReasoningContentDelta {
                        delta: reasoning_content,
                        content_index: 0,
                    }))
                    .await
                    .map_err(|_| {
                        ApiError::Stream("chat completions stream receiver dropped".into())
                    })?;
            }
            if let Some(tool_calls) = choice.delta.tool_calls {
                for tool_call in tool_calls {
                    let pending = self.tool_calls.entry(tool_call.index).or_default();
                    if pending.id.is_none() {
                        pending.id = tool_call.id;
                    }
                    if let Some(function) = tool_call.function {
                        if let Some(name) = function.name {
                            pending.name.push_str(&name);
                        }
                        if let Some(arguments) = function.arguments {
                            pending.arguments.push_str(&arguments);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn ensure_assistant_started(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        if self.assistant_started {
            return Ok(());
        }

        self.assistant_started = true;
        tx_event
            .send(Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: String::new(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            })))
            .await
            .map_err(|_| ApiError::Stream("chat completions stream receiver dropped".into()))
    }

    async fn finish(
        self,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        if self.assistant_started {
            tx_event
                .send(Ok(ResponseEvent::OutputItemDone(ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: self.assistant_text,
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                })))
                .await
                .map_err(|_| ApiError::Stream("chat completions stream receiver dropped".into()))?;
        }

        // Emit OutputItemAdded for each tool call before OutputItemDone
        for (_, tool_call) in &self.tool_calls {
            let add_item = match split_tool_name(&tool_call.name) {
                ToolNameMapping::ToolSearch => ResponseItem::ToolSearchCall {
                    id: None,
                    call_id: None,
                    status: None,
                    execution: String::new(),
                    arguments: serde_json::Value::Null,
                    internal_chat_message_metadata_passthrough: None,
                },
                ToolNameMapping::Function { namespace, name } => ResponseItem::FunctionCall {
                    id: None,
                    name: name.clone(),
                    namespace: namespace.clone(),
                    arguments: String::new(),
                    call_id: String::new(),
                    internal_chat_message_metadata_passthrough: None,
                },
                ToolNameMapping::Custom { name } => ResponseItem::CustomToolCall {
                    id: None,
                    status: None,
                    call_id: String::new(),
                    name: name.clone(),
                    namespace: None,
                    input: String::new(),
                    internal_chat_message_metadata_passthrough: None,
                },
                ToolNameMapping::UnsupportedHosted { .. } => continue,
            };
            if tx_event
                .send(Ok(ResponseEvent::OutputItemAdded(add_item)))
                .await
                .is_err()
            {
                warn!("finish: OutputItemAdded send failed, receiver closed");
            }
        }

        let end_turn = !matches!(self.finish_reason.as_deref(), Some("tool_calls"));
        for (index, tool_call) in self.tool_calls {
            let mapped_item = match split_tool_name(&tool_call.name) {
                ToolNameMapping::ToolSearch => ResponseItem::ToolSearchCall {
                    id: None,
                    call_id: Some(
                        tool_call
                            .id
                            .unwrap_or_else(|| format!("chatcmpl-call-{index}")),
                    ),
                    status: Some("completed".to_string()),
                    execution: "client".to_string(),
                    arguments: parse_tool_search_arguments(&tool_call.arguments),
                    internal_chat_message_metadata_passthrough: None,
                },
                ToolNameMapping::UnsupportedHosted { name } => {
                    return Err(ApiError::Stream(format!(
                        "chat completions parser received unsupported hosted tool `{name}`"
                    )));
                }
                ToolNameMapping::Function { namespace, name } => ResponseItem::FunctionCall {
                    id: None,
                    name,
                    namespace,
                    arguments: tool_call.arguments,
                    call_id: tool_call
                        .id
                        .unwrap_or_else(|| format!("chatcmpl-call-{index}")),
                    internal_chat_message_metadata_passthrough: None,
                },
                ToolNameMapping::Custom { name } => ResponseItem::CustomToolCall {
                    id: None,
                    status: Some("completed".to_string()),
                    call_id: tool_call
                        .id
                        .unwrap_or_else(|| format!("chatcmpl-call-{index}")),
                    name,
                    namespace: None,
                    input: parse_custom_tool_input(&tool_call.arguments),
                    internal_chat_message_metadata_passthrough: None,
                },
            };
            tx_event
                .send(Ok(ResponseEvent::OutputItemDone(mapped_item)))
                .await
                .map_err(|_| ApiError::Stream("chat completions stream receiver dropped".into()))?;
        }

        tx_event
            .send(Ok(ResponseEvent::Completed {
                response_id: self
                    .response_id
                    .unwrap_or_else(|| "chatcmpl-local".to_string()),
                token_usage: self.token_usage,
                end_turn: Some(end_turn),
            }))
            .await
            .map_err(|_| ApiError::Stream("chat completions stream receiver dropped".into()))
    }
}

async fn process_chat_completions_sse(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
) {
    let mut stream = stream.eventsource();
    let mut acc = ChatCompletionAccumulator::default();
    let mut done_received = false;

    loop {
        let start = Instant::now();
        let response = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                debug!("Chat completions SSE error: {e:#}");
                let _ = tx_event.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => {
                if !done_received {
                    if let Err(err) = std::mem::take(&mut acc).finish(&tx_event).await {
                        let _ = tx_event.send(Err(err)).await;
                    }
                }
                return;
            }
            Err(_) => {
                if !done_received {
                    // Emit whatever we accumulated before timeout
                    if let Err(finish_err) = std::mem::take(&mut acc).finish(&tx_event).await {
                        let _ = tx_event.send(Err(finish_err)).await;
                    }
                    let _ = tx_event
                        .send(Err(ApiError::Stream(
                            "idle timeout waiting for chat completion SSE".into(),
                        )))
                        .await;
                }
                return;
            }
        };

        if sse.data.trim() == "[DONE]" {
            if !done_received {
                if let Err(err) = std::mem::take(&mut acc).finish(&tx_event).await {
                    let _ = tx_event.send(Err(err)).await;
                    return;
                }
                done_received = true;
            }
            continue;
        }

        if done_received {
            continue;
        }

        let chunk: ChatCompletionChunk = match serde_json::from_str(&sse.data) {
            Ok(chunk) => chunk,
            Err(err) => {
                warn!(
                    "Failed to parse chat completions chunk: {err}, data: {}",
                    sse.data
                );
                continue;
            }
        };

        if let Err(err) = acc.ingest_chunk(chunk, &tx_event).await {
            let _ = tx_event.send(Err(err)).await;
            return;
        }
    }
}

enum ToolNameMapping {
    ToolSearch,
    UnsupportedHosted {
        name: String,
    },
    Function {
        namespace: Option<String>,
        name: String,
    },
    Custom {
        name: String,
    },
}

fn split_tool_name(value: &str) -> ToolNameMapping {
    if value == "tool_search" {
        return ToolNameMapping::ToolSearch;
    }
    if value == "web_search" || value == "image_generation" {
        return ToolNameMapping::UnsupportedHosted {
            name: value.to_string(),
        };
    }
    if let Some(name) = value.strip_prefix("custom__")
        && !name.is_empty()
    {
        return ToolNameMapping::Custom {
            name: name.to_string(),
        };
    }
    // Use rsplit_once so that MCP namespaced names like
    // mcp__codebase_memory_mcp__search_graph resolve to
    // namespace=mcp__codebase_memory_mcp, name=search_graph.
    if let Some((namespace, name)) = value.rsplit_once("__")
        && !namespace.is_empty()
        && !name.is_empty()
    {
        return ToolNameMapping::Function {
            namespace: Some(namespace.to_string()),
            name: name.to_string(),
        };
    }
    ToolNameMapping::Function {
        namespace: None,
        name: value.to_string(),
    }
}

fn parse_tool_search_arguments(arguments: &str) -> JsonValue {
    serde_json::from_str(arguments).unwrap_or_else(|_| JsonValue::String(arguments.to_string()))
}

fn parse_custom_tool_input(arguments: &str) -> String {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|value| {
            value
                .get("input")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| arguments.to_string())
}

#[cfg(test)]
mod tests {
    use super::ChatCompletionAccumulator;
    use super::ChatCompletionChunk;
    use super::PendingToolCall;
    use super::ToolNameMapping;
    use super::process_chat_completions_sse;
    use super::split_tool_name;
    use crate::common::ResponseEvent;
    use crate::error::ApiError;
    use bytes::Bytes;
    use codex_client::ByteStream;
    use codex_protocol::models::ResponseItem;
    use futures::stream;
    use serde_json::json;
    use std::time::Duration;
    use tokio::sync::mpsc;

    async fn collect_events(
        acc: &mut ChatCompletionAccumulator,
        chunk: ChatCompletionChunk,
    ) -> Vec<ResponseEvent> {
        let (tx, mut rx) = mpsc::channel(32);
        acc.ingest_chunk(chunk, &tx).await.expect("ingest chunk");
        drop(tx);

        let mut out = Vec::new();
        while let Some(Ok(event)) = rx.recv().await {
            out.push(event);
        }
        out
    }

    #[tokio::test]
    async fn emits_reasoning_content_deltas_for_openai_compatible_streams() {
        let mut acc = ChatCompletionAccumulator::default();
        let chunk: ChatCompletionChunk = serde_json::from_value(serde_json::json!({
            "choices": [{
                "index": 0,
                "finish_reason": null,
                "delta": {
                    "role": "assistant",
                    "content": null,
                    "reasoning_content": "thinking"
                }
            }]
        }))
        .expect("chunk");

        let events = collect_events(&mut acc, chunk).await;
        assert!(matches!(events[0], ResponseEvent::OutputItemAdded(_)));
        assert!(matches!(
            &events[1],
            ResponseEvent::ReasoningContentDelta { delta, content_index }
                if delta == "thinking" && *content_index == 0
        ));
    }

    #[tokio::test]
    async fn accepts_reasoning_alias_for_openai_compatible_streams() {
        let mut acc = ChatCompletionAccumulator::default();
        let chunk: ChatCompletionChunk = serde_json::from_value(serde_json::json!({
            "choices": [{
                "index": 0,
                "finish_reason": null,
                "delta": {
                    "role": "assistant",
                    "reasoning": "alias text"
                }
            }]
        }))
        .expect("chunk");

        let events = collect_events(&mut acc, chunk).await;
        assert!(matches!(
            &events[1],
            ResponseEvent::ReasoningContentDelta { delta, .. } if delta == "alias text"
        ));
    }

    #[test]
    fn split_tool_name_preserves_tool_search_as_reserved_call() {
        assert!(matches!(
            split_tool_name("tool_search"),
            ToolNameMapping::ToolSearch
        ));
    }

    #[test]
    fn split_tool_name_rejects_hosted_reserved_tools() {
        assert!(matches!(
            split_tool_name("web_search"),
            ToolNameMapping::UnsupportedHosted { name } if name == "web_search"
        ));
        assert!(matches!(
            split_tool_name("image_generation"),
            ToolNameMapping::UnsupportedHosted { name } if name == "image_generation"
        ));
    }

    #[test]
    fn split_tool_name_preserves_mcp_server_namespace() {
        assert!(matches!(
            split_tool_name("mcp__codebase_memory_mcp__search_graph"),
            ToolNameMapping::Function { namespace: Some(namespace), name }
                if namespace == "mcp__codebase_memory_mcp" && name == "search_graph"
        ));
    }

    #[tokio::test]
    async fn finish_maps_tool_search_function_calls_to_typed_items() {
        let (tx, mut rx) = mpsc::channel(32);
        let mut acc = ChatCompletionAccumulator::default();
        acc.tool_calls.insert(
            0,
            PendingToolCall {
                id: Some("call-1".to_string()),
                name: "tool_search".to_string(),
                arguments: r#"{"query":"spawn agent","limit":2}"#.to_string(),
            },
        );

        acc.finish(&tx).await.expect("finish");
        drop(tx);

        let mut events = Vec::new();
        while let Some(Ok(event)) = rx.recv().await {
            events.push(event);
        }

        assert!(
            matches!(
                &events[0],
                ResponseEvent::OutputItemAdded(ResponseItem::ToolSearchCall { .. })
            ),
            "expected first event to be OutputItemAdded(ToolSearchCall)"
        );
        assert!(matches!(
            &events[1],
            ResponseEvent::OutputItemDone(ResponseItem::ToolSearchCall {
                call_id,
                execution,
                arguments,
                ..
            }) if call_id.as_deref() == Some("call-1")
                && execution == "client"
                && arguments == &json!({"query":"spawn agent","limit":2})
        ));
    }

    #[tokio::test]
    async fn finish_rejects_unsupported_hosted_tool_calls() {
        let (tx, _rx) = mpsc::channel(32);
        let mut acc = ChatCompletionAccumulator::default();
        acc.tool_calls.insert(
            0,
            PendingToolCall {
                id: Some("call-1".to_string()),
                name: "web_search".to_string(),
                arguments: r#"{"query":"weather"}"#.to_string(),
            },
        );

        let err = acc
            .finish(&tx)
            .await
            .expect_err("hosted tool should be rejected");
        assert!(
            err.to_string()
                .contains("unsupported hosted tool `web_search`")
        );
    }

    #[tokio::test]
    async fn eof_before_done_is_reported_as_stream_error_not_completed() {
        let chunk = json!({
            "id": "chatcmpl-1",
            "choices": [{
                "index": 0,
                "finish_reason": null,
                "delta": { "role": "assistant", "content": "partial" }
            }]
        });
        let sse = format!("data: {chunk}\n\n");
        let stream: ByteStream = Box::pin(stream::iter(vec![Ok(Bytes::from(sse))]));
        let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(32);

        process_chat_completions_sse(stream, tx, Duration::from_secs(5), None).await;

        let mut saw_error = false;
        let mut saw_completed = false;
        while let Some(event) = rx.recv().await {
            match event {
                Ok(ResponseEvent::Completed { .. }) => saw_completed = true,
                Err(err) if err.to_string().contains("ended before [DONE]") => saw_error = true,
                _ => {}
            }
        }

        assert!(
            saw_error,
            "EOF before [DONE] must be surfaced as a stream error"
        );
        assert!(
            !saw_completed,
            "truncated streams must not be marked completed"
        );
    }
}
