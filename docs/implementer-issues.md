# cccx current implementation issues for implementers

Date: 2026-07-18

Audience: implementers fixing the current RED feature contracts.

## Executive verdict

cccx is not yet implementation-complete against `docs/architecture.md` and `docs/design.md`. The current code has enough scaffold to describe the intended architecture, but several runtime boundaries are still wrapper-level or placeholder-level. The most urgent failures are in the 9router/OpenAI-compatible chat-completions path, because they affect real conversations as soon as the agent uses tools, MCP, or subagents.

The immediate goal is not to add new features. The goal is to make the existing cccx contract tests green while preserving the design constraints:

- 9router is the only model gateway boundary.
- CC is the scheduler/control plane.
- CX is bounded execution.
- Cross-agent work must use WorkSpec/Artifact style structured data.
- Tool/MCP serialization must be valid for OpenAI-compatible chat-completions.
- Safety must be enforced by state machines and policy gates, not prompt wording.

## Evidence baseline

Checkpoint commit before this document:

- `7d3ee89b08 test: add cccx feature coverage contracts`

Test evidence:

- Detailed RED report: `docs/testing/cccx-feature-coverage-red-report.md`
- `v8` Windows build blocker has been worked around locally by creating junction `D:\cargo-targets\cccx\debug\gn_root`.

## Critical issues

### C1 — Chat-completions request history is invalid for parallel tool calls

Failure:

- `codex-core::client::tests::parallel_function_call_history_must_serialize_as_one_assistant_message`

Observed mechanism:

- A single assistant turn with multiple function/tool calls is serialized into multiple assistant messages.
- OpenAI-compatible chat-completions expects one assistant message with a `tool_calls` array.
- This can explain 9router upstream `400` failures after tool/subagent/MCP calls.

Evidence:

- Test: `codex-rs/core/src/client_tests.rs:962`
- Current per-call serialization sites: `codex-rs/core/src/client.rs:296`, `codex-rs/core/src/client.rs:313`, `codex-rs/core/src/client.rs:329`

Expected fix:

- Fix the chat history conversion layer, not the model prompt.
- Preserve assistant turn grouping when converting `ResponseItem::FunctionCall` history to chat-completions messages.
- Emit one assistant message with `tool_calls: [ ... ]`, then corresponding `tool` result messages.

Acceptance:

- `cargo test -p codex-core parallel_function_call_history_must_serialize_as_one_assistant_message --lib` passes.
- Existing chat-completions request tests still pass.

### C2 — MCP tool names are split incorrectly

Failure:

- `codex-api::sse::chat_completions::tests::split_tool_name_preserves_mcp_server_namespace`

Observed mechanism:

- `mcp__codebase_memory_mcp__search_graph` loses its namespace boundary.
- This can route MCP calls to the wrong runtime tool payload or crash tool dispatch.

Evidence:

- Split function: `codex-rs/codex-api/src/sse/chat_completions.rs:483`
- RED test: `codex-rs/codex-api/src/sse/chat_completions.rs:631`

Expected fix:

- Treat `mcp__<server>__<tool>` as a namespaced runtime tool.
- Preserve namespace as `mcp__codebase_memory_mcp`, name as `search_graph`.
- Keep reserved typed calls such as `tool_search` separate from generic function tools.

Acceptance:

- `cargo test -p codex-api chat_completions --lib` no longer fails on namespace split.
- Existing `tool_search` reserved-call tests still pass.

### C3 — Stream EOF before `[DONE]` is treated as successful completion

Failure:

- `codex-api::sse::chat_completions::tests::eof_before_done_is_reported_as_stream_error_not_completed`

Observed mechanism:

- The parser sends `ResponseEvent::Completed` from accumulator finish even if the stream ends without `[DONE]`.
- This hides truncated responses and can make cccx think a long task completed cleanly after a disconnect.

Evidence:

- `[DONE]` branch: `codex-rs/codex-api/src/sse/chat_completions.rs:436`
- Current completed emission: `codex-rs/codex-api/src/sse/chat_completions.rs:377`
- RED test: `codex-rs/codex-api/src/sse/chat_completions.rs:704`

Expected fix:

- Track whether `[DONE]` was received.
- If the stream ends before `[DONE]`, emit a stream error and do not emit `Completed`.
- Keep retry/reconnect policy explicit at the caller/gateway layer.

Acceptance:

- `cargo test -p codex-api chat_completions --lib` passes.
- No regression in normal `[DONE]` stream completion.

## High issues

### H1 — Permission queue bridge denies one prompt by cancelling all prompts from the same agent

Failure:

- `codex-core::permission_queue_bridge::tests::denying_one_prompt_must_not_cancel_other_prompts_from_same_agent`

Observed mechanism:

- `CccxPermissionQueue::deny()` resolves the agent id and calls `cancel_agent_requests(agent_id)`.
- That violates the central queue design: deny should apply to one request, then promote the next request.

Evidence:

- `codex-rs/core/src/permission_queue_bridge.rs:46`
- Bulk cancel call: `codex-rs/core/src/permission_queue_bridge.rs:52`

Expected fix:

- Add a single-request deny operation in `cccx-policy::PermissionRequestQueue`.
- `CccxPermissionQueue::deny(request_id)` should mark only that request as `Denied`.
- Then promote the next queued request.

Acceptance:

- `cargo test -p codex-core permission_queue_bridge::tests --lib` passes.

### H2 — Subagent path scope uses unsafe prefix matching

Failure:

- `cccx-policy::tests::subagent_scope_check_requires_real_path_boundary`

Observed mechanism:

- `D:/repo/src/authentication/session.rs` matches scope `D:/repo/src/auth` because policy uses `starts_with`.
- This breaks bounded CX execution guarantees.

Evidence:

- Prefix check: `codex-rs/cccx-policy/src/lib.rs:516`
- RED test: `codex-rs/cccx-policy/src/lib.rs:1060`

Expected fix:

- Normalize path separators.
- Treat a path as in scope only when it equals the scope root or starts with `scope_root + "/"`.
- Apply the same boundary logic to protected paths if not already done.

Acceptance:

- `cargo test -p cccx-policy --lib` passes.

### H3 — Checkpoint validation ignores current git head

Failure:

- `codex-core::checkpoint::tests::checkpoint_from_different_git_head_requires_revalidation`

Observed mechanism:

- `validate_checkpoint()` accepts `_current_git_head` but does not use it.
- A resumed subagent may apply stale partial work against a different tree state.

Evidence:

- `codex-rs/core/src/checkpoint.rs:80`
- `_current_git_head`: `codex-rs/core/src/checkpoint.rs:82`
- RED test: `codex-rs/core/src/checkpoint.rs:181`

Expected fix:

- Add `base_commit` or equivalent to `SubagentCheckpoint`.
- Return `NeedsRevalidation { base_commit, current_head }` when current head differs.
- Preserve backward-compatible handling for old checkpoints by marking them `NeedsRevalidation` or `Stale`, not `Valid`.

Acceptance:

- `cargo test -p codex-core checkpoint::tests --lib` passes.

### H4 — Risky mode transitions are not enforced

Failure:

- `cccx-mode::tests::risky_modes_require_explicit_confirmation_or_attestation`

Observed mechanism:

- `DontAsk` and `Bypass` error variants exist, but `change_mode()` currently changes mode directly.

Evidence:

- Error variants: `codex-rs/cccx-mode/src/lib.rs:31`
- Commented design constraint: `codex-rs/cccx-mode/src/lib.rs:57`
- RED test: `codex-rs/cccx-mode/src/lib.rs:168`

Expected fix:

- Split transition APIs:
  - normal mode change for Plan/Default/AcceptEdits/Auto;
  - explicit confirmation API for `DontAsk`;
  - explicit sandbox/isolation attestation API for `Bypass`.
- Wire TUI slash commands and Shift+Tab behavior to these APIs.

Acceptance:

- `cargo test -p cccx-mode --lib` passes.
- TUI mode switching still supports the safe Shift+Tab cycle.

## Medium issues

### M1 — Headless JSONL envelope does not match cccx design

Failures:

- `cccx-headless::tests::jsonl_event_matches_design_exec_envelope_contract`
- `cccx-headless::tests::terminal_event_must_be_last_jsonl_event`

Observed mechanism:

- Test expects `schemaVersion = "cccx.exec_event.v1"` but serialized event lacks it.
- Terminal event ordering is not guaranteed.

Evidence:

- Schema validation area: `codex-rs/cccx-headless/src/lib.rs:162`
- RED schema assertion: `codex-rs/cccx-headless/src/lib.rs:353`
- RED terminal-last test: `codex-rs/cccx-headless/src/lib.rs:376`

Expected fix:

- Align serialized JSON field names with design: `schemaVersion`, `sessionId`, `turnId`, monotonic sequence, stable terminal event.
- Make terminal event final in emission helpers, not only in tests.

Acceptance:

- `cargo test -p cccx-headless --lib` passes.

### M2 — UX parser/dispatcher does not yet register all design slash commands

Failures:

- `cccx-ux::tests::parses_all_design_mode_slash_commands`
- `cccx-ux::tests::parses_design_task_goal_and_utility_commands`

Observed mechanism:

- Parser knows some cccx commands, but `/plan`, `/goal tests pass`, `/review`, `/theme`, etc. are not all represented by stable typed commands.

Evidence:

- RED mode command table: `codex-rs/cccx-ux/src/lib.rs:336`
- RED utility command table: `codex-rs/cccx-ux/src/lib.rs:355`

Expected fix:

- Extend `cccx-ux` command enum and parser to cover all commands in `docs/design.md`.
- Then wire TUI slash dispatch without placeholder-only behavior.

Acceptance:

- `cargo test -p cccx-ux --lib` passes.
- Add focused TUI dispatcher tests for at least mode, task, permission, memory commands.

### M3 — Plugin manifest schema still uses implementation-shaped field names

Failure:

- `cccx-protocol::plugin::tests::cccx_plugin_manifest_uses_design_schema_field_names`

Observed mechanism:

- Manifest struct expects `allowed_tools`; design test supplies design-shaped camelCase fields.

Evidence:

- Current field: `codex-rs/cccx-protocol/src/plugin.rs:42`
- Validation use: `codex-rs/cccx-protocol/src/plugin.rs:63`
- RED test fixture starts at `codex-rs/cccx-protocol/src/plugin.rs:77`

Expected fix:

- Decide final manifest schema:
  - either design camelCase only;
  - or accept both design camelCase and legacy snake_case with explicit compatibility tests.
- Do not silently accept unknown Codex/plugin fields.

Acceptance:

- `cargo test -p cccx-protocol --lib` passes.

## Lower-priority but required closure

### L1 — Coverage cannot be claimed yet

Current state:

- `cargo llvm-cov` is not installed.
- `cargo tarpaulin` is not installed.
- Feature suite is RED.

Expected fix:

- After RED list is green, install or vendor `cargo-llvm-cov`.
- Run `cargo llvm-cov --workspace --fail-under-lines 80` or a scoped equivalent justified in docs.

### L2 — Several cccx modules are still scaffolded but not fully wired

Observed from warnings and current code shape:

- `GoalEvaluator`, `CccxPermissionQueue`, task graph state helpers, project memory, checkpoints, artifact collector, provider gateway bridge exist but are not fully consumed by runtime paths.
- This is acceptable during scaffolding, but not for feature-complete cccx.

Expected fix:

- Wire each state machine into the runtime one phase at a time.
- Keep tests red/green per module and avoid prompt-only enforcement.

## Recommended implementation order

1. Fix 9router/chat-completions correctness first.
2. Fix policy and state-machine safety.
3. Fix headless/UX/schema user-facing contracts.
4. Wire scaffolded core modules into runtime.
5. Add coverage tooling and enforce the 80% gate.

Do not start with cosmetic branding or docs-only cleanup. Those do not address the current failure mode.
