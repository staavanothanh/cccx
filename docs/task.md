# cccx implementation task plan

Date: 2026-07-18

Source documents:

- `docs/architecture.md`
- `docs/design.md`
- `docs/implementer-issues.md`
- `docs/testing/cccx-feature-coverage-red-report.md`

Current baseline commit:

- `7d3ee89b08 test: add cccx feature coverage contracts`

Rule for implementers:

- Work phase by phase.
- Do not remove or weaken RED tests.
- Make the smallest production change that turns the phase tests green.
- Run only targeted tests inside each phase first.
- Run broader cccx feature tests after each phase.
- Keep `CARGO_TARGET_DIR=D:\cargo-targets\cccx`.

## Phase 0 — Environment and baseline verification

Goal: make sure the implementer can reproduce the current RED state.

Steps:

1. Set PowerShell environment:

   ```powershell
   cd D:\Learning_Programing\cccx\codex-rs
   $env:PATH = "D:\Program Files\cargo\bin;$env:PATH"
   $env:CARGO_TARGET_DIR = "D:\cargo-targets\cccx"
   ```

2. Verify `v8` junction exists:

   ```powershell
   Get-Item "D:\cargo-targets\cccx\debug\gn_root" -Force | Format-List FullName,LinkType,Target
   ```

3. If missing, recreate:

   ```powershell
   cmd /c mklink /J "D:\cargo-targets\cccx\debug\gn_root" "C:\Users\Admin\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\v8-149.2.0"
   ```

4. Reproduce current RED:

   ```powershell
   cargo test -p codex-api chat_completions --lib
   cargo test -p codex-core parallel_function_call_history_must_serialize_as_one_assistant_message --lib
   cargo test -p cccx-policy --lib
   cargo test -p cccx-mode --lib
   cargo test -p cccx-headless --lib
   cargo test -p cccx-ux --lib
   cargo test -p cccx-protocol --lib
   ```

Acceptance:

- The same failures listed in `docs/implementer-issues.md` reproduce.
- No new compile failures appear.

## Phase 1 — 9router/OpenAI-compatible chat-completions correctness

Goal: make cccx reliably compatible with 9router when tools/MCP/subagents are present.

Primary files:

- `codex-rs/codex-api/src/sse/chat_completions.rs`
- `codex-rs/core/src/client.rs`
- `codex-rs/core/src/client_tests.rs`

Tasks:

1. Fix `split_tool_name()` to preserve MCP server namespace.
2. Fix stream parser so EOF before `[DONE]` emits a stream error and never emits completed.
3. Fix chat history serialization so one assistant turn with multiple function calls becomes one assistant message with multiple `tool_calls`.
4. Verify no regression in `tool_search` reserved typed call handling.

Target tests:

```powershell
cargo test -p codex-api chat_completions --lib
cargo test -p codex-core parallel_function_call_history_must_serialize_as_one_assistant_message --lib
```

Broader phase check:

```powershell
cargo test -p codex-core chat_completion --lib
```

Acceptance:

- Phase target tests pass.
- 9router request payload remains OpenAI-compatible.
- No fallback to Responses API semantics is introduced for 9router.

Suggested ECC chain:

- `tdd_guide` for RED→GREEN discipline.
- `architect` or `reviewer` after green because this is a boundary translation layer.

## Phase 2 — Permission, scope, and resume safety

Goal: enforce cccx safety by state machines and path boundaries.

Primary files:

- `codex-rs/cccx-policy/src/lib.rs`
- `codex-rs/core/src/permission_queue_bridge.rs`
- `codex-rs/core/src/checkpoint.rs`

Tasks:

1. Add single-request deny operation to `PermissionRequestQueue`.
2. Update `CccxPermissionQueue::deny()` to deny only one prompt and promote next.
3. Replace subagent scope prefix matching with normalized path-boundary checks.
4. Add checkpoint base commit metadata and compare against current git head.
5. Define migration behavior for checkpoints without base commit.

Target tests:

```powershell
cargo test -p cccx-policy --lib
cargo test -p codex-core permission_queue_bridge::tests --lib
cargo test -p codex-core checkpoint::tests --lib
```

Acceptance:

- Denying a prompt does not cancel unrelated queued prompts.
- `D:/repo/src/authentication` is not inside `D:/repo/src/auth`.
- Checkpoint from a different git head returns `NeedsRevalidation`.

Suggested ECC chain:

- `tdd_guide`
- `security_reviewer`

## Phase 3 — Mode state machine and UX command surface

Goal: make the 6-mode system usable and safe from TUI commands.

Primary files:

- `codex-rs/cccx-mode/src/lib.rs`
- `codex-rs/cccx-ux/src/lib.rs`
- `codex-rs/tui/src/slash_command.rs`
- `codex-rs/tui/src/chatwidget/slash_dispatch.rs`
- `codex-rs/tui/src/app.rs`

Tasks:

1. Split safe mode transition from risky transition APIs.
2. Require explicit confirmation for `DontAsk`.
3. Require sandbox/isolation attestation for `Bypass`.
4. Keep Shift+Tab cycle on safe modes only unless design says otherwise.
5. Add all missing design slash commands to `cccx-ux`.
6. Wire parser outputs into TUI dispatch with non-placeholder behavior where MVP requires it.

Target tests:

```powershell
cargo test -p cccx-mode --lib
cargo test -p cccx-ux --lib
```

Broader phase check:

```powershell
cargo test -p codex-tui slash --lib
```

Acceptance:

- Mode tests pass.
- UX parser tests pass.
- No mode can bypass permission gates by prompt text alone.

Suggested ECC chain:

- `tdd_guide`
- `reviewer`

## Phase 4 — Headless JSONL contract

Goal: make headless output stable for automation and downstream tooling.

Primary files:

- `codex-rs/cccx-headless/src/lib.rs`
- `codex-rs/exec/src/event_processor_with_jsonl_output.rs`
- `codex-rs/exec/src/lib.rs`

Tasks:

1. Align envelope fields with design:
   - `schemaVersion`
   - `sessionId`
   - `turnId`
   - sequence number
   - stable event type and payload
2. Ensure terminal event is emitted last.
3. Keep stdout JSONL-only and diagnostics on stderr.

Target tests:

```powershell
cargo test -p cccx-headless --lib
```

Broader phase check:

```powershell
cargo test -p codex-exec jsonl --lib
```

Acceptance:

- Headless JSONL tests pass.
- Existing `codex exec` JSONL behavior is not broken except where intentionally renamed/extended for cccx.

Suggested ECC chain:

- `tdd_guide`
- `reviewer`

## Phase 5 — Plugin manifest and config schema closure

Goal: make plugin/config schema match cccx design and fail closed.

Primary files:

- `codex-rs/cccx-protocol/src/plugin.rs`
- `codex-rs/cccx-protocol/src/lib.rs`
- Config schema generation code if affected.

Tasks:

1. Decide final manifest field shape.
2. Prefer design schema names as canonical.
3. If backward compatibility is needed, accept legacy field names explicitly with tests.
4. Keep unknown fields rejected.

Target tests:

```powershell
cargo test -p cccx-protocol --lib
```

Acceptance:

- Plugin manifest tests pass.
- Schema behavior is documented in code tests and design docs if changed.

Suggested ECC chain:

- `architect`
- `tdd_guide`
- `reviewer`

## Phase 6 — Runtime wiring completeness

Goal: move from scaffold contracts to actual runtime behavior.

Primary files:

- `codex-rs/core/src/session/session.rs`
- `codex-rs/core/src/cccx_spawn.rs`
- `codex-rs/core/src/artifact_collector.rs`
- `codex-rs/core/src/project_memory.rs`
- `codex-rs/core/src/goal_evaluator.rs`
- `codex-rs/core/src/provider_gateway_bridge.rs`
- `codex-rs/core/src/cccx_task_graph_state.rs`
- TUI surfaces that display task/agent/permission status.

Tasks:

1. Wire WorkSpec projection into actual subagent spawn path.
2. Wire ArtifactCollector into subagent completion.
3. Wire GoalEvaluator into task graph verification.
4. Wire ProjectMemory only through explicit `/remember` admission.
5. Wire ProviderGatewayBridge provenance into request lifecycle.
6. Wire MCP default check into startup/config diagnostics without forcing global state.

Target tests:

```powershell
cargo test -p codex-core cccx --lib
cargo test -p codex-core subagent --lib
cargo test -p codex-core mcp_auto_init::tests --lib
cargo test -p codex-core provider_gateway_bridge::tests --lib
cargo test -p codex-core goal_evaluator::tests --lib
```

Acceptance:

- No cccx runtime module is only dead scaffold unless explicitly documented as deferred.
- Subagent spawn uses WorkSpec/Artifact rather than raw context dump.
- Main CC remains the only scheduler.

Suggested ECC chain:

- `architect`
- `tdd_guide`
- `security_reviewer`
- `reviewer`

## Phase 7 — Full cccx feature validation

Goal: verify all feature contracts together before coverage.

Commands:

```powershell
cargo test -p cccx-protocol -p cccx-policy -p cccx-provider-gateway -p cccx-task-graph -p cccx-mode -p cccx-ux -p cccx-headless -p cccx-harness-cc -p cccx-harness-cx --no-fail-fast
cargo test -p codex-api chat_completions --lib
cargo test -p codex-core cccx --lib
```

Acceptance:

- All current RED tests are green.
- No new regression tests are red.
- cccx binary still starts with 9router config.

## Phase 8 — Coverage gate

Goal: prove `>=80%` coverage truthfully.

Tasks:

1. Install or vendor `cargo-llvm-cov`.
2. Decide whether coverage gate is workspace-wide or scoped to cccx-owned crates plus touched runtime modules.
3. Run coverage.
4. Document exclusions.

Preferred command:

```powershell
cargo llvm-cov --workspace --fail-under-lines 80
```

Fallback scoped command if workspace-wide is too slow:

```powershell
cargo llvm-cov -p cccx-protocol -p cccx-policy -p cccx-provider-gateway -p cccx-task-graph -p cccx-mode -p cccx-ux -p cccx-headless -p cccx-harness-cc -p cccx-harness-cx --fail-under-lines 80
```

Acceptance:

- Coverage report exists.
- `>=80%` line coverage is proven by command output.
- `docs/testing/cccx-feature-coverage-red-report.md` is superseded by a GREEN evidence report.

## Stop conditions

Stop and report instead of continuing if:

- A phase introduces a new architectural conflict with `docs/architecture.md`.
- A test must be weakened to pass.
- 9router requires a payload shape that conflicts with OpenAI-compatible chat-completions.
- Target dir exceeds acceptable disk usage and cleanup is required before continuing.

## Recommended commit sequence

1. `fix: make chat completions compatible with 9router tools`
2. `fix: enforce cccx permission and scope safety`
3. `fix: complete cccx mode and slash command contracts`
4. `fix: align cccx headless jsonl envelope`
5. `fix: align cccx plugin manifest schema`
6. `feat: wire cccx runtime state machines`
7. `test: add cccx coverage gate`
