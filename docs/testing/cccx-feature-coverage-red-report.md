# cccx feature coverage RED report

Date: 2026-07-18

Scope: test-only pass to lock cccx design contracts before implementation fixes.

## Skill workflow

- Requested skills: `tdd-workflow`, `rust-testing`.
- Both skill files were loaded from `C:\Users\Admin\.codex\skills\`.
- This pass followed RED-first behavior: tests were added/expanded, implementation was not changed except for the local Windows build workaround needed to run tests.

## Toolchain blocker resolved

`codex-core` tests were previously blocked by `v8 v149.2.0` on Windows because the crate attempted to create a directory symlink across drives:

- target: `D:\cargo-targets\cccx\debug\gn_root`
- source: `C:\Users\Admin\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\v8-149.2.0`
- error: `symlink_dir failed: A required privilege is not held by the client.`

Non-admin workaround applied:

```powershell
cmd /c mklink /J "D:\cargo-targets\cccx\debug\gn_root" "C:\Users\Admin\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\v8-149.2.0"
```

Evidence:

- `cargo test -p codex-core checkpoint_with_empty_completed_step_is_not_resumable --lib` passed after the junction was created.

## Test guarantees added or tightened

| Area | Test target | Guarantee | Current result |
|---|---|---|---|
| Artifact protocol | `codex-core artifact_collector::tests` | CX artifacts must carry provenance and runtime evidence for success; changed paths are deduplicated. | PASS |
| Permission queue bridge | `codex-core permission_queue_bridge::tests` | Denying one prompt must not bulk-cancel unrelated prompts from the same agent. | RED |
| Project memory | `codex-core project_memory::tests` | Memory loads empty when absent, appends explicit notes under `.cccx/memory`, and bounds loaded context. | PASS |
| Checkpoint/resume | `codex-core checkpoint::tests` | Empty progress is not resumable; checkpoint from a different git head must require revalidation. | RED |
| MCP defaults | `codex-core mcp_auto_init::tests` | Exactly two global MCP defaults are required; partial config reports only missing servers. | PASS |
| Provider gateway bridge | `codex-core provider_gateway_bridge::tests` | Gateway provenance records requested/effective 9router route; route prefixing only applies to canonical 9router provider. | PASS |
| Goal evaluator | `codex-core goal_evaluator::tests` | Goal verification requires command exit code and evidence digest; prose alone cannot verify a task. | PASS |
| Chat-completions MCP tool names | `codex-api chat_completions` | MCP namespaced tools such as `mcp__codebase_memory_mcp__search_graph` must preserve server namespace. | RED |
| Chat-completions stream integrity | `codex-api chat_completions` | EOF before `[DONE]` must surface as stream error, not completed response. | RED |
| Chat-completions parallel tools | `codex-core parallel_function_call_history_must_serialize_as_one_assistant_message` | Parallel tool calls from one assistant turn must serialize as one assistant message with multiple `tool_calls`. | RED |

## Validation commands run

```powershell
$env:PATH = "D:\Program Files\cargo\bin;$env:PATH"
$env:CARGO_TARGET_DIR = "D:\cargo-targets\cccx"
cargo fmt --manifest-path "D:\Learning_Programing\cccx\codex-rs\Cargo.toml" --all
cargo test -p codex-core checkpoint_with_empty_completed_step_is_not_resumable --lib
cargo test -p cccx-protocol -p cccx-policy -p cccx-provider-gateway -p cccx-task-graph -p cccx-mode -p cccx-ux -p cccx-headless -p cccx-harness-cc -p cccx-harness-cx --no-fail-fast
cargo test -p codex-core artifact_collector::tests --lib
cargo test -p codex-core permission_queue_bridge::tests --lib
cargo test -p codex-core project_memory::tests --lib
cargo test -p codex-core checkpoint::tests --lib
cargo test -p codex-core mcp_auto_init::tests --lib
cargo test -p codex-core provider_gateway_bridge::tests --lib
cargo test -p codex-core goal_evaluator::tests --lib
cargo test -p codex-api chat_completions --lib
cargo test -p codex-core parallel_function_call_history_must_serialize_as_one_assistant_message --lib
```

## Confirmed RED list

1. `cccx-headless::jsonl_event_matches_design_exec_envelope_contract`
   - Missing `schemaVersion = "cccx.exec_event.v1"` and related stable envelope fields.
2. `cccx-headless::terminal_event_must_be_last_jsonl_event`
   - Terminal JSONL event is not guaranteed to be last.
3. `cccx-mode::risky_modes_require_explicit_confirmation_or_attestation`
   - `DontAsk` can be entered without the design-required explicit confirmation.
4. `cccx-policy::subagent_scope_check_requires_real_path_boundary`
   - Prefix matching allows `D:/repo/src/authentication/...` to pass a scope of `D:/repo/src/auth`.
5. `cccx-protocol::plugin::cccx_plugin_manifest_uses_design_schema_field_names`
   - Plugin manifest still expects `allowed_tools`, not the design field shape.
6. `cccx-ux::parses_all_design_mode_slash_commands`
   - Mode commands such as `/plan` are not registered in the UX parser contract.
7. `cccx-ux::parses_design_task_goal_and_utility_commands`
   - Commands such as `/goal tests pass` are not registered in the UX parser contract.
8. `codex-core::permission_queue_bridge::denying_one_prompt_must_not_cancel_other_prompts_from_same_agent`
   - Bridge deny currently cancels all prompts for the agent.
9. `codex-core::checkpoint::checkpoint_from_different_git_head_requires_revalidation`
   - Checkpoint validation ignores `current_git_head`.
10. `codex-api::split_tool_name_preserves_mcp_server_namespace`
    - Tool name splitting loses MCP server namespace boundaries.
11. `codex-api::eof_before_done_is_reported_as_stream_error_not_completed`
    - Stream EOF before `[DONE]` is treated as normal completion.
12. `codex-core::parallel_function_call_history_must_serialize_as_one_assistant_message`
    - Parallel tool history becomes multiple assistant messages, invalid for OpenAI-compatible chat-completions payloads.

## Coverage status

Numeric 80% coverage was not verified because no Rust coverage tool is currently installed:

- `cargo llvm-cov --version` failed: no such cargo command.
- `cargo tarpaulin --version` failed: no such cargo command.

Coverage cannot be truthfully claimed until RED tests are made green and `cargo-llvm-cov` or equivalent is installed. Recommended command after green:

```powershell
cargo llvm-cov --workspace --fail-under-lines 80
```

## Phase plan to green

### Phase A — 9router/chat-completions correctness

Fix first because it directly affects real prompt/tool/subagent/MCP usage.

- Preserve MCP namespace when converting `mcp__server__tool` between model-visible names and runtime tool calls.
- Treat premature EOF as stream error and keep/retry behavior explicit.
- Group parallel assistant tool calls into one OpenAI-compatible assistant message with multiple `tool_calls`.

### Phase B — safety state machines

- Change permission queue deny semantics from “cancel all agent requests” to “deny this request, then promote next”.
- Add git base/head metadata to checkpoints or validation input so cross-head resume requires revalidation.
- Replace subagent scope `starts_with` checks with normalized path-boundary checks.

### Phase C — user-facing contracts

- Align headless JSONL envelope with `cccx.exec_event.v1`, stable `sessionId`/`turnId`, and terminal-last ordering.
- Enforce explicit confirmation/attestation for `DontAsk` and `Bypass`.
- Register/dispatch all design slash commands and remove placeholder-only command gaps.

### Phase D — plugin/config schema closure

- Align plugin manifest schema field names with design docs and update backward-compatibility policy.

### Phase E — coverage gate

- Install or vendor `cargo-llvm-cov`.
- Run focused cccx feature tests, then workspace coverage.
- Set CI/local gate to fail under 80% once the RED list is green.
