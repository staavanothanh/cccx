use super::*;
use crate::session::step_context::StepContext;
use pretty_assertions::assert_eq;

struct TestHandler {
    tool_name: codex_tools::ToolName,
}

impl ToolExecutor<ToolInvocation> for TestHandler {
    fn tool_name(&self) -> codex_tools::ToolName {
        self.tool_name.clone()
    }

    fn spec(&self) -> codex_tools::ToolSpec {
        test_spec(&self.tool_name)
    }

    fn handle(&self, _invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async {
            Ok(
                Box::new(crate::tools::context::FunctionToolOutput::from_text(
                    "ok".to_string(),
                    Some(true),
                )) as Box<dyn crate::tools::context::ToolOutput>,
            )
        })
    }
}

impl CoreToolRuntime for TestHandler {}

#[derive(Clone)]
enum LifecycleTestResult {
    Ok { success: bool },
    Err,
}

struct LifecycleTestHandler {
    tool_name: codex_tools::ToolName,
    result: LifecycleTestResult,
}

impl ToolExecutor<ToolInvocation> for LifecycleTestHandler {
    fn tool_name(&self) -> codex_tools::ToolName {
        self.tool_name.clone()
    }

    fn spec(&self) -> codex_tools::ToolSpec {
        test_spec(&self.tool_name)
    }

    fn handle(&self, _invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call())
    }
}

impl LifecycleTestHandler {
    async fn handle_call(
        &self,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        match self.result.clone() {
            LifecycleTestResult::Ok { success } => Ok(Box::new(
                crate::tools::context::FunctionToolOutput::from_text(
                    "ok".to_string(),
                    Some(success),
                ),
            )
                as Box<dyn crate::tools::context::ToolOutput>),
            LifecycleTestResult::Err => Err(FunctionCallError::RespondToModel(
                "handler failed".to_string(),
            )),
        }
    }
}

impl CoreToolRuntime for LifecycleTestHandler {}

struct CccxPostToolUseCounterTestHandler {
    tool_name: codex_tools::ToolName,
}

impl ToolExecutor<ToolInvocation> for CccxPostToolUseCounterTestHandler {
    fn tool_name(&self) -> codex_tools::ToolName {
        self.tool_name.clone()
    }

    fn spec(&self) -> codex_tools::ToolSpec {
        test_spec(&self.tool_name)
    }

    fn handle(&self, _invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async {
            Ok(
                Box::new(crate::tools::context::FunctionToolOutput::from_text(
                    "ok".to_string(),
                    Some(true),
                )) as Box<dyn crate::tools::context::ToolOutput>,
            )
        })
    }
}

impl CoreToolRuntime for CccxPostToolUseCounterTestHandler {
    fn post_tool_use_payload(
        &self,
        _invocation: &ToolInvocation,
        _result: &dyn crate::tools::context::ToolOutput,
    ) -> Option<PostToolUsePayload> {
        Some(PostToolUsePayload {
            tool_name: HookToolName::apply_patch(),
            tool_use_id: "stable-post-tool-use-id".to_string(),
            tool_input: serde_json::json!({
                "command": "*** Begin Patch\n*** Update File: src/main.rs\n-old\n+new\n+added\n*** End Patch",
            }),
            tool_response: serde_json::json!("Success. Updated files."),
        })
    }
}

#[test]
fn cccx_read_tracking_canonicalizes_relative_read_paths() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file = temp.path().join("src").join("lib.rs");
    std::fs::create_dir_all(file.parent().expect("parent")).expect("create parent");
    std::fs::write(&file, "fn main() {}\n").expect("write fixture");
    let payload = PostToolUsePayload {
        tool_name: HookToolName::new("mcp__filesystem__read_file"),
        tool_use_id: "read-1".to_string(),
        tool_input: serde_json::json!({"path": "src/lib.rs"}),
        tool_response: serde_json::json!("ok"),
    };

    assert_eq!(
        cccx_read_paths_from_post_tool_use_payload(&payload, temp.path()),
        vec![std::fs::canonicalize(file).expect("canonical file")]
    );
}

fn install_blocking_post_tool_use_hook_for_counter_test(
    config: &crate::config::Config,
) -> anyhow::Result<String> {
    std::fs::create_dir_all(&config.codex_home)?;
    let command = {
        #[cfg(windows)]
        {
            "echo blocked cccx counter regression 1>&2 & exit /b 2".to_string()
        }
        #[cfg(not(windows))]
        {
            "printf '%s\\n' 'blocked cccx counter regression' >&2; exit 2".to_string()
        }
    };
    let hooks = serde_json::json!({
        "hooks": {
            "PostToolUse": [{
                "hooks": [{
                    "type": "command",
                    "command": command,
                    "statusMessage": "blocking cccx counter regression"
                }]
            }]
        }
    });
    std::fs::write(config.codex_home.join("hooks.json"), hooks.to_string())?;
    Ok(command)
}

fn trusted_config_layer_stack_for_counter_test(
    config_layer_stack: &codex_config::ConfigLayerStack,
    codex_home: &codex_utils_absolute_path::AbsolutePathBuf,
    hooks: Vec<codex_hooks::HookListEntry>,
) -> codex_config::ConfigLayerStack {
    let mut user_config = config_layer_stack
        .get_active_user_layer()
        .map(|layer| layer.config.clone())
        .unwrap_or_else(|| codex_config::TomlValue::Table(Default::default()));
    let user_table = user_config
        .as_table_mut()
        .expect("user config should be a table");
    let hooks_table = user_table
        .entry("hooks")
        .or_insert_with(|| codex_config::TomlValue::Table(Default::default()))
        .as_table_mut()
        .expect("hooks config should be a table");
    let state_table = hooks_table
        .entry("state")
        .or_insert_with(|| codex_config::TomlValue::Table(Default::default()))
        .as_table_mut()
        .expect("hook state config should be a table");
    for hook in hooks {
        let mut hook_state = codex_config::TomlValue::Table(Default::default());
        let hook_state_table = hook_state
            .as_table_mut()
            .expect("hook state should be a table");
        hook_state_table.insert(
            "trusted_hash".to_string(),
            codex_config::TomlValue::String(hook.current_hash),
        );
        state_table.insert(hook.key, hook_state);
    }

    config_layer_stack.with_user_config(
        &codex_home.join(codex_config::CONFIG_TOML_FILE),
        user_config,
    )
}

fn test_spec(tool_name: &codex_tools::ToolName) -> codex_tools::ToolSpec {
    codex_tools::ToolSpec::Function(codex_tools::ResponsesApiTool {
        name: tool_name.name.clone(),
        description: "Test tool.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: codex_tools::JsonSchema::default(),
        output_schema: None,
    })
}

#[derive(Debug, PartialEq, Eq)]
enum RecordedToolLifecycle {
    Start {
        call_id: String,
        tool_name: codex_tools::ToolName,
    },
    Finish {
        call_id: String,
        tool_name: codex_tools::ToolName,
        outcome: codex_extension_api::ToolCallOutcome,
    },
}

struct ToolLifecycleRecorder {
    records: Arc<std::sync::Mutex<Vec<RecordedToolLifecycle>>>,
}

impl codex_extension_api::ToolLifecycleContributor for ToolLifecycleRecorder {
    fn on_tool_start<'a>(
        &'a self,
        input: codex_extension_api::ToolStartInput<'a>,
    ) -> codex_extension_api::ToolLifecycleFuture<'a> {
        let records = Arc::clone(&self.records);
        let record = RecordedToolLifecycle::Start {
            call_id: input.call_id.to_string(),
            tool_name: input.tool_name.clone(),
        };
        Box::pin(async move {
            records
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(record);
        })
    }

    fn on_tool_finish<'a>(
        &'a self,
        input: codex_extension_api::ToolFinishInput<'a>,
    ) -> codex_extension_api::ToolLifecycleFuture<'a> {
        let records = Arc::clone(&self.records);
        let record = RecordedToolLifecycle::Finish {
            call_id: input.call_id.to_string(),
            tool_name: input.tool_name.clone(),
            outcome: input.outcome,
        };
        Box::pin(async move {
            records
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(record);
        })
    }
}

#[test]
fn handler_looks_up_namespaced_aliases_explicitly() {
    let namespace = "mcp__codex_apps__gmail";
    let tool_name = "gmail_get_recent_emails";
    let plain_name = codex_tools::ToolName::plain(tool_name);
    let namespaced_name = codex_tools::ToolName::namespaced(namespace, tool_name);
    let plain_handler = Arc::new(TestHandler {
        tool_name: plain_name.clone(),
    }) as Arc<dyn CoreToolRuntime>;
    let namespaced_handler = Arc::new(TestHandler {
        tool_name: namespaced_name.clone(),
    }) as Arc<dyn CoreToolRuntime>;
    let registry = ToolRegistry::new(HashMap::from([
        (plain_name.clone(), Arc::clone(&plain_handler)),
        (namespaced_name.clone(), Arc::clone(&namespaced_handler)),
    ]));

    let plain = registry.tool(&plain_name);
    let namespaced = registry.tool(&namespaced_name);
    let missing_namespaced = registry.tool(&codex_tools::ToolName::namespaced(
        "mcp__codex_apps__calendar",
        tool_name,
    ));

    assert_eq!(plain.is_some(), true);
    assert_eq!(namespaced.is_some(), true);
    assert_eq!(missing_namespaced.is_none(), true);
    assert!(
        plain
            .as_ref()
            .is_some_and(|handler| Arc::ptr_eq(handler, &plain_handler))
    );
    assert!(
        namespaced
            .as_ref()
            .is_some_and(|handler| Arc::ptr_eq(handler, &namespaced_handler))
    );
}

#[tokio::test]
async fn function_tools_expose_default_hook_payloads_and_rewrites() -> anyhow::Result<()> {
    let (session, turn) = crate::session::tests::make_session_and_context().await;
    let tool_name = codex_tools::ToolName::namespaced("functions.", "echo");
    let handler = TestHandler {
        tool_name: tool_name.clone(),
    };
    let invocation = ToolInvocation {
        payload: ToolPayload::Function {
            arguments: serde_json::json!({ "message": "hello" }).to_string(),
        },
        ..test_invocation(Arc::new(session), Arc::new(turn), "call-1", tool_name)
    };
    let output =
        crate::tools::context::FunctionToolOutput::from_text("echoed".to_string(), Some(true));

    assert_eq!(
        handler.pre_tool_use_payload(&invocation),
        Some(PreToolUsePayload {
            tool_name: HookToolName::new("functions.echo"),
            tool_input: serde_json::json!({ "message": "hello" }),
        })
    );
    assert_eq!(
        handler.post_tool_use_payload(&invocation, &output),
        Some(PostToolUsePayload {
            tool_name: HookToolName::new("functions.echo"),
            tool_use_id: "call-1".to_string(),
            tool_input: serde_json::json!({ "message": "hello" }),
            tool_response: serde_json::json!("echoed"),
        })
    );

    let invocation = handler
        .with_updated_hook_input(invocation, serde_json::json!({ "message": "rewritten" }))?;
    let ToolPayload::Function { arguments } = invocation.payload else {
        panic!("generic rewritten function payload should remain function-shaped");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&arguments)?,
        serde_json::json!({ "message": "rewritten" })
    );

    Ok(())
}

#[tokio::test]
async fn function_hook_input_defaults_empty_arguments_to_object() {
    let (session, turn) = crate::session::tests::make_session_and_context().await;
    let tool_name = codex_tools::ToolName::plain("echo");
    let handler = TestHandler {
        tool_name: tool_name.clone(),
    };
    let invocation = ToolInvocation {
        payload: ToolPayload::Function {
            arguments: "  ".to_string(),
        },
        ..test_invocation(Arc::new(session), Arc::new(turn), "call-1", tool_name)
    };

    assert_eq!(
        handler.pre_tool_use_payload(&invocation),
        Some(PreToolUsePayload {
            tool_name: HookToolName::new("echo"),
            tool_input: serde_json::json!({}),
        })
    );
}

#[tokio::test]
async fn spawn_agent_function_tools_use_agent_matcher_alias() {
    let (session, turn) = crate::session::tests::make_session_and_context().await;
    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let hook_payloads = [
        codex_tools::ToolName::plain("spawn_agent"),
        codex_tools::ToolName::namespaced(MULTI_AGENT_V1_NAMESPACE, "spawn_agent"),
    ]
    .into_iter()
    .map(|tool_name| {
        let handler = TestHandler {
            tool_name: tool_name.clone(),
        };
        let invocation = ToolInvocation {
            payload: ToolPayload::Function {
                arguments: serde_json::json!({ "message": "inspect this repo" }).to_string(),
            },
            ..test_invocation(Arc::clone(&session), Arc::clone(&turn), "call-1", tool_name)
        };
        handler.pre_tool_use_payload(&invocation)
    })
    .collect::<Vec<_>>();

    assert_eq!(
        hook_payloads,
        vec![
            Some(PreToolUsePayload {
                tool_name: HookToolName::spawn_agent(),
                tool_input: serde_json::json!({ "message": "inspect this repo" }),
            }),
            Some(PreToolUsePayload {
                tool_name: HookToolName::spawn_agent(),
                tool_input: serde_json::json!({ "message": "inspect this repo" }),
            }),
        ]
    );
}

#[tokio::test]
async fn code_mode_wait_does_not_expose_default_hook_payloads() {
    let (session, turn) = crate::session::tests::make_session_and_context().await;
    let output = crate::tools::context::FunctionToolOutput::from_text("ok".to_string(), Some(true));

    let wait = crate::tools::handlers::CodeModeWaitHandler;
    let wait_invocation = test_invocation(
        Arc::new(session),
        Arc::new(turn),
        "wait-call",
        wait.tool_name(),
    );
    assert_eq!(wait.pre_tool_use_payload(&wait_invocation), None);
    assert_eq!(wait.post_tool_use_payload(&wait_invocation, &output), None);
}

#[tokio::test]
async fn write_stdin_does_not_expose_default_pre_tool_use_payload() {
    let (session, turn) = crate::session::tests::make_session_and_context().await;

    let write_stdin = crate::tools::handlers::WriteStdinHandler;
    let invocation = test_invocation(
        Arc::new(session),
        Arc::new(turn),
        "write-stdin-call",
        write_stdin.tool_name(),
    );

    assert_eq!(write_stdin.pre_tool_use_payload(&invocation), None);
}

#[test]
fn post_tool_use_feedback_output_keeps_code_mode_result_typed() {
    let result = AnyToolResult {
        call_id: "call-1".to_string(),
        payload: ToolPayload::Function {
            arguments: "{}".to_string(),
        },
        result: Box::new(PostToolUseFeedbackOutput {
            original: Box::new(codex_tools::JsonToolOutput::new(
                serde_json::json!({ "typed": true }),
            )),
            model_visible: crate::tools::context::FunctionToolOutput::from_text(
                "hook feedback".to_string(),
                /*success*/ None,
            ),
        }),
        post_tool_use_payload: None,
    };

    assert_eq!(
        result.into_response(),
        ResponseInputItem::FunctionCallOutput {
            call_id: "call-1".to_string(),
            output: codex_protocol::models::FunctionCallOutputPayload::from_text(
                "hook feedback".to_string()
            ),
        }
    );

    let result = AnyToolResult {
        call_id: "call-1".to_string(),
        payload: ToolPayload::Function {
            arguments: "{}".to_string(),
        },
        result: Box::new(PostToolUseFeedbackOutput {
            original: Box::new(codex_tools::JsonToolOutput::new(
                serde_json::json!({ "typed": true }),
            )),
            model_visible: crate::tools::context::FunctionToolOutput::from_text(
                "hook feedback".to_string(),
                /*success*/ None,
            ),
        }),
        post_tool_use_payload: None,
    };

    assert_eq!(
        result.code_mode_result(),
        serde_json::json!({ "typed": true })
    );
}

#[tokio::test]
async fn dispatch_notifies_tool_lifecycle_contributors() -> anyhow::Result<()> {
    let (mut session, turn) = crate::session::tests::make_session_and_context().await;
    let records = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut builder = codex_extension_api::ExtensionRegistryBuilder::<crate::config::Config>::new();
    builder.tool_lifecycle_contributor(Arc::new(ToolLifecycleRecorder {
        records: Arc::clone(&records),
    }));
    session.services.extensions = Arc::new(builder.build());

    let ok_tool = codex_tools::ToolName::plain("ok_tool");
    let failing_tool = codex_tools::ToolName::plain("failing_tool");
    let ok_handler = Arc::new(LifecycleTestHandler {
        tool_name: ok_tool.clone(),
        result: LifecycleTestResult::Ok { success: false },
    }) as Arc<dyn CoreToolRuntime>;
    let failing_handler = Arc::new(LifecycleTestHandler {
        tool_name: failing_tool.clone(),
        result: LifecycleTestResult::Err,
    }) as Arc<dyn CoreToolRuntime>;
    let registry = ToolRegistry::new(HashMap::from([
        (ok_tool.clone(), ok_handler),
        (failing_tool.clone(), failing_handler),
    ]));
    let session = Arc::new(session);
    let turn = Arc::new(turn);

    registry
        .dispatch_any(test_invocation(
            Arc::clone(&session),
            Arc::clone(&turn),
            "ok-call",
            ok_tool.clone(),
        ))
        .await?;
    let err = match registry
        .dispatch_any(test_invocation(
            Arc::clone(&session),
            Arc::clone(&turn),
            "failing-call",
            failing_tool.clone(),
        ))
        .await
    {
        Ok(_) => panic!("failing handler should return an error"),
        Err(err) => err,
    };
    assert_eq!(err.to_string(), "handler failed");

    let expected = vec![
        RecordedToolLifecycle::Start {
            call_id: "ok-call".to_string(),
            tool_name: ok_tool.clone(),
        },
        RecordedToolLifecycle::Finish {
            call_id: "ok-call".to_string(),
            tool_name: ok_tool,
            outcome: codex_extension_api::ToolCallOutcome::Completed { success: false },
        },
        RecordedToolLifecycle::Start {
            call_id: "failing-call".to_string(),
            tool_name: failing_tool.clone(),
        },
        RecordedToolLifecycle::Finish {
            call_id: "failing-call".to_string(),
            tool_name: failing_tool,
            outcome: codex_extension_api::ToolCallOutcome::Failed {
                handler_executed: true,
            },
        },
    ];
    let actual = records
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .drain(..)
        .collect::<Vec<_>>();
    assert_eq!(expected, actual);

    Ok(())
}

#[tokio::test]
async fn cccx_post_tool_use_counter_does_not_commit_for_unsuccessful_tool() -> anyhow::Result<()> {
    let (session, turn) = crate::session::tests::make_session_and_context().await;
    let session = Arc::new(session);
    let turn = Arc::new(turn);
    let tool_name = codex_tools::ToolName::plain("cccx_unsuccessful_counter_test");
    let handler = Arc::new(LifecycleTestHandler {
        tool_name: tool_name.clone(),
        result: LifecycleTestResult::Ok { success: false },
    });
    let registry = ToolRegistry::with_handler_for_test(handler);

    registry
        .dispatch_any(test_invocation(
            session,
            Arc::clone(&turn),
            "call-1",
            tool_name,
        ))
        .await?;

    assert_eq!(turn.cccx_post_tool_use_changed_lines_for_test().await, 0);

    Ok(())
}

#[tokio::test]
async fn cccx_post_tool_use_counter_commits_after_success_without_double_counting()
-> anyhow::Result<()> {
    let (session, turn) = crate::session::tests::make_session_and_context().await;
    let session = Arc::new(session);
    let turn = Arc::new(turn);
    let tool_name = codex_tools::ToolName::plain("cccx_apply_patch_counter_test");
    let handler = Arc::new(CccxPostToolUseCounterTestHandler {
        tool_name: tool_name.clone(),
    });
    let registry = ToolRegistry::with_handler_for_test(handler);

    registry
        .dispatch_any(test_invocation(
            Arc::clone(&session),
            Arc::clone(&turn),
            "call-1",
            tool_name.clone(),
        ))
        .await?;
    assert_eq!(turn.cccx_post_tool_use_changed_lines_for_test().await, 3);

    registry
        .dispatch_any(test_invocation(
            session,
            Arc::clone(&turn),
            "call-2",
            tool_name,
        ))
        .await?;
    assert_eq!(turn.cccx_post_tool_use_changed_lines_for_test().await, 3);

    Ok(())
}

#[tokio::test]
async fn cccx_post_tool_use_counter_does_not_commit_when_post_hook_blocks() -> anyhow::Result<()> {
    let (session, turn) = crate::session::tests::make_session_and_context().await;
    let config = session.get_config().await;
    let blocking_command = install_blocking_post_tool_use_hook_for_counter_test(config.as_ref())?;
    let discovery_config = codex_hooks::HooksConfig {
        feature_enabled: true,
        config_layer_stack: Some(config.config_layer_stack.clone()),
        ..codex_hooks::HooksConfig::default()
    };
    let listed_hooks = codex_hooks::list_hooks(discovery_config.clone());
    assert!(
        listed_hooks.hooks.iter().any(|hook| hook.event_name
            == codex_protocol::protocol::HookEventName::PostToolUse
            && hook.command.as_deref() == Some(blocking_command.as_str())),
        "blocking PostToolUse hook should be discoverable. hooks={:?} warnings={:?}",
        listed_hooks.hooks,
        listed_hooks.warnings
    );
    let trusted_config_layer_stack = trusted_config_layer_stack_for_counter_test(
        &config.config_layer_stack,
        &config.codex_home,
        listed_hooks.hooks.clone(),
    );
    let hook_config = codex_hooks::HooksConfig {
        config_layer_stack: Some(trusted_config_layer_stack),
        ..discovery_config
    };
    let trusted_hooks = codex_hooks::list_hooks(hook_config.clone());
    assert!(
        trusted_hooks.hooks.iter().any(|hook| hook.event_name
            == codex_protocol::protocol::HookEventName::PostToolUse
            && hook.command.as_deref() == Some(blocking_command.as_str())
            && hook.enabled),
        "trusted PostToolUse hook should be enabled. hooks={:?} warnings={:?}",
        trusted_hooks.hooks,
        trusted_hooks.warnings
    );
    session
        .services
        .hooks
        .store(Arc::new(codex_hooks::Hooks::new(hook_config)));
    let session = Arc::new(session);
    let turn = Arc::new(turn);
    let tool_name = codex_tools::ToolName::plain("cccx_apply_patch_blocked_counter_test");
    let handler = Arc::new(CccxPostToolUseCounterTestHandler {
        tool_name: tool_name.clone(),
    });
    let registry = ToolRegistry::with_handler_for_test(handler);

    let result = registry
        .dispatch_any(test_invocation(
            session,
            Arc::clone(&turn),
            "call-1",
            tool_name,
        ))
        .await;
    let err = match result {
        Ok(_) => {
            let changed_lines = turn.cccx_post_tool_use_changed_lines_for_test().await;
            anyhow::bail!(
                "blocking PostToolUse hook should reject the tool result. changed_lines={} hooks={:?}",
                changed_lines,
                trusted_hooks.hooks
            )
        }
        Err(err) => err,
    };

    assert!(
        matches!(err, FunctionCallError::RespondToModel(ref message)
            if message == "blocked cccx counter regression"),
        "blocked PostToolUse hook reason should be returned to the model. Got: {err:?}"
    );
    assert_eq!(
        turn.cccx_post_tool_use_changed_lines_for_test().await,
        0,
        "blocked PostToolUse result must not commit changed-line counter"
    );

    Ok(())
}

fn test_invocation(
    session: Arc<crate::session::session::Session>,
    turn: Arc<crate::session::turn_context::TurnContext>,
    call_id: &str,
    tool_name: codex_tools::ToolName,
) -> ToolInvocation {
    let step_context = StepContext::for_test(Arc::clone(&turn));
    ToolInvocation {
        session,
        step_context,
        turn,
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        tracker: Arc::new(tokio::sync::Mutex::new(
            crate::turn_diff_tracker::TurnDiffTracker::new(),
        )),
        call_id: call_id.to_string(),
        tool_name,
        source: crate::tools::context::ToolCallSource::Direct,
        payload: ToolPayload::Function {
            arguments: "{}".to_string(),
        },
    }
}
