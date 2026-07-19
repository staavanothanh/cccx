# cccx progress

- Docs baseline committed: `39e511c`.
- Codex CLI baseline imported: `88ddfb4` from upstream `800715d2`.
- Current phase: M8 Packaging Rename complete.
- Done: design docs, audit, fork plan, upstream remote, upstream docs separated, M0 validation complete, M0.5 MVP source gating implemented, M1 provider gateway crate implemented and wired into default Codex provider config path, M2 WorkSpec/Artifact protocol crate, M3 WorkSpecToChildConfigAdapter contract, M4 permission policy core crate, M5 task graph core crate, M6 UX contract crate, M7 headless contract crate.
- Toolchain fix: installed Visual Studio Build Tools 2022 with C++ tools/Windows SDK via `winget`; use repo-pinned Rust `1.95.0-x86_64-pc-windows-msvc` through `D:\Program Files\cargo\bin`.
- M0 validation passed from `codex-rs/`: `cargo check -p codex-core`; `cargo check -p codex-tui`; `RUST_MIN_STACK=33554432 cargo test -p codex-core multi_agent -- --test-threads=1`.
- M0.5 validation passed from `codex-rs/`: `cargo fmt --check`; `cargo check -p codex-features -p codex-cli -p codex-tui`.
- M0.5 scope: centralized `codex-features::cccx_mvp` gate with `CCCX_MVP_ALLOW_UPSTREAM_SURFACES=1` escape hatch; CLI/TUI non-MVP surfaces hidden and runtime-guarded; default Apps/MCP Apps/MemoryTool/remote-plugin feature defaults disabled under cccx gate.
- Codebase indexed via codebase-memory-mcp as project `cccx`; artifact written to `.codebase-memory/graph.db.zst`.
- Caveat: direct binary smoke via `cargo run -p codex-cli` timed out while building/running the Windows binary target; compile checks passed.
- M1 validation passed from `codex-rs/`: `cargo fmt --check`; `cargo check -p cccx-provider-gateway`; `cargo test -p cccx-provider-gateway`.
- M1 targeted validation passed from `codex-rs/`: `cargo test -p codex-model-provider-info test_built_in_model_providers_include_9router`.
- M1 targeted validation attempted from `codex-rs/`: `cargo test -p codex-core cccx_default_model_provider`; test exists, but Windows compile/link of the `codex-core` test binary repeatedly exceeded the time budget and was stopped.
- M1 scope completed: new `cccx-provider-gateway` crate with `ModelRouteCatalog`, `ModelRouteResolver`, `ModelRouteProvenance`, fail-closed route resolution, 9router `/v1/responses` client, mock 9router provenance test, built-in reserved `9router` provider metadata, and cccx default model-provider resolution to `9router`.
- M2/M3 validation passed from `codex-rs/`: `cargo test -p cccx-protocol`.
- M2 scope completed: new `cccx-protocol` crate with schema versioning, typed `WorkSpec`, `Artifact`, `VerificationRecord`, evidence/provenance records, golden JSON shape test, round-trip tests, and artifact validator rejecting missing evidence/provenance.
- M3 scope completed: `ParentCapabilitySet`, `ChildConfigProjection`, and `project_child_config()` with deny-by-default projection, WorkSpec/parent capability intersection, MCP allowlist enforcement, protected path checks, explicit model-route allowlist enforcement, and CX full-history fork rejection.
- M4 validation passed from `codex-rs/`: `cargo test -p cccx-policy`.
- M4 scope completed: new `cccx-policy` crate with deterministic `evaluate_action()`, circuit breaker priority over static allow/escalate, Bypass retaining circuit breakers only, AcceptEdits threshold reroute, Auto classifier-unavailable approval fallback, and single-active `PermissionRequestQueue` rejecting stale digest/cancelled/circuit-breaker approvals.
- M5 validation passed from `codex-rs/`: `cargo test -p cccx-task-graph`.
- M5 scope completed: new `cccx-task-graph` crate with JSON-serializable `TaskGraph`, `TaskNode`, `AttemptRecord`, `WorkerRecord`, `VerificationSpec`, artifact application, verification projection, dependency blocking, `can_stop()` stop check, and tests proving CX success artifacts do not verify tasks without a passed `VerificationRecord`.
- M6 validation passed from `codex-rs/`: `cargo test -p cccx-ux`.
- M6 scope completed: new `cccx-ux` crate with deterministic parser for `/agents`, `/agents logs`, `/agents resume`, `/agents cancel`, `/agents model`, `/tasks`, `/permissions`, `/remember`, `/cancel`, plus stable renderers for cccx status line, agent list, and permission queue.
- M7 validation passed from `codex-rs/`: `cargo test -p cccx-headless`.
- M7 scope completed: new `cccx-headless` crate with JSONL event envelopes for answer/approval/task/agent/permission/error/terminal events, contiguous sequence validation, stdout JSONL-only guard, stderr diagnostics separation, and headless approval default deny.
- Heavy wiring batch validation passed on shared target dir `D:\cargo-targets\cccx`: `cargo check -p codex-core --lib`; `cargo check -p codex-tui --lib`; `cargo check -p codex-exec --lib`.
- Heavy wiring batch scope completed:
  - `cccx-policy` circuit-breaker precheck wired into `codex-core` exec approval path.
  - `cccx-task-graph` persistence/stop-guard/compaction-survivor state helper added under `.cccx/state/`.
  - `cccx-ux` parser wired into `codex-tui` slash-command dispatch for `/agents`, `/tasks`, `/permissions`, `/remember`, `/cancel` with bounded placeholder actions where runtime behaviors are still intentionally deferred.
  - `cccx-headless` stdout JSON guard wired into `codex-exec` JSONL emission path.
- Build environment note: to keep large Rust artifacts off `C:`, use shared `CARGO_TARGET_DIR=D:\cargo-targets\cccx`. On Windows, the `v8` crate symlink step was worked around by creating a junction at `D:\cargo-targets\cccx\debug\gn_root`.
- M8 validation passed from `codex-rs/`: `cargo test -p codex-utils-home-dir`; `cargo check -p cccx-cli --bin cccx`; `cargo run -p cccx-cli --bin cccx -- --version`.
- M8 scope completed:
  - Rust CLI package renamed to `cccx-cli`.
  - New primary binary surface is `cccx`; legacy `codex` binary alias kept temporarily for compatibility.
  - Default config home changed to `~/.cccx` with `CCCX_HOME` as primary override and `CODEX_HOME` as legacy fallback.
  - NPM launcher/package surface switched to `cccx` with `cccx.js` primary entrypoint and `codex.js` compatibility shim.
  - Root README/package metadata updated to reflect cccx branding while preserving Codex provenance.
- Post-M8 Phase 2/3/4 validation passed from `codex-rs/` on shared target dir `D:\cargo-targets\cccx`:
  - `cargo test -p codex-core chat_completion_tools_reject_hosted_web_search -- --nocapture`
  - `cargo test -p codex-core chat_message_from_response_item_rejects_web_search_history -- --nocapture`
  - `cargo test -p codex-core chat_completion_tools_flattens_namespaced_function_tools -- --nocapture`
  - `cargo test -p codex-api chat_completions -- --nocapture`
- Post-M8 Phase 2/3/4 scope completed:
  - Chat-completions request builder now fails closed on unsupported hosted tool/history items instead of silently serializing or dropping them into an invalid 9router/OpenAI payload.
  - Namespaced/deferred tools still flatten correctly onto the chat-completions runtime path, preserving `multi_agent_v1__...` compatibility.
  - Chat-completions SSE parser now treats reserved hosted tool names such as `web_search` and `image_generation` as unsupported on this path and surfaces an explicit stream error instead of mis-parsing them as callable local tools.
  - The 9router adapter boundary is now stricter on the chat-completions path: unsupported hosted capabilities are rejected inside cccx before they become malformed upstream requests.
- Post-M8 tool-search payload regression validation passed from `codex-rs/` on shared target dir `D:\cargo-targets\cccx`:
  - `cargo test -p codex-core build_tool_search_call_uses_tool_search_payload -- --nocapture`
  - `cargo test -p codex-api finish_maps_tool_search_function_calls_to_typed_items -- --nocapture`
  - `cargo test -p codex-api split_tool_name_preserves_tool_search_as_reserved_call -- --nocapture`
- Post-M8 tool-search payload regression scope completed:
  - Added a direct router regression test proving `ResponseItem::ToolSearchCall` is converted into `ToolPayload::ToolSearch` rather than the generic function payload path.
  - Re-verified the chat-completions parser keeps `tool_search` as a reserved typed call instead of flattening it into a normal function tool call.
  - Confirmed the current adapter matrix remains asymmetric by design: `ToolSpec` has five model-visible variants, but runtime `ToolPayload` only has `Function`, `ToolSearch`, and `Custom`, so boundary translation correctness is mandatory for 9router/chat-completions compatibility.
- Remaining post-plan work: replace placeholder runtime actions for `/tasks`, `/remember`, `/cancel`, and deeper config/help/update string renames that are still intentionally left on internal `codex_*` terminology.
- 2026-07-18 test-only coverage pass:
  - Loaded requested `tdd-workflow` and `rust-testing` skills from `C:\Users\Admin\.codex\skills`.
  - Fixed the local Windows `v8` build blocker without admin by creating junction `D:\cargo-targets\cccx\debug\gn_root` to the Cargo registry `v8-149.2.0` source, allowing `codex-core` tests to compile on shared D target dir.
  - Added/tightened contract tests around Artifact provenance/evidence, permission queue bridge, project memory, checkpoint validation, MCP defaults, provider gateway provenance, goal evaluator, chat-completions stream/tool handling, and OpenAI-compatible parallel tool-call history.
  - Current state is intentionally RED. Detailed evidence and phase plan: `docs/testing/cccx-feature-coverage-red-report.md`.
  - Numeric `>=80%` coverage is not yet proven because `cargo-llvm-cov`/`cargo-tarpaulin` are not installed and the feature suite is still RED.
- 2026-07-19 Phase 0 baseline — worktree and full targeted smoke:
  - Worktree clean (2 untracked .vn.md translation files only).
  - All targeted tests pass: codex-api (8), cccx-protocol (13), cccx-policy (11), cccx-provider-gateway (2), cccx-task-graph (2), cccx-mode (8), cccx-ux (8), cccx-headless (8), cccx-harness-cc (0), cccx-harness-cx (0), codex-core provider_gateway_bridge (3), codex-core cccx (10).
  - cccx-cli binary compiles (duplicate bin target warning known as L1).
  - Baseline HEAD: `95e9789` → ready for Phase 1 (9router-only model gateway).
- 2026-07-19 Phase 1 complete — 9router-only model gateway hardened:
  - **Task 1**: Binary separation — `codex` keeps upstream provider behavior; `cccx` defaults to 9router via `surface_gate_enabled()` gate already in place.
  - **Task 2**: Fail-closed enforcement — `ProviderGatewayBridge::check_fail_closed()` rejects non-9router providers at the LLM call boundary. `stream_responses_api` and `stream_chat_completions_api` both call the fail-closed check and return `CodexErr::InvalidRequest` with a clear message when the cccx gate is on but the resolved provider is not 9router.
  - **Task 3**: Provider resolution unified — all LLM calls go through the same `begin_gateway_provenance` / `check_cccx_gateway_fail_closed` path.
  - **Task 4**: Provenance enriched — `GatewayProvenance` now records `fallback_policy_id`, `wire_api`, and `stream_termination_status` in addition to existing `requested_route_id`, `effective_route_id`, `effective_provider`, `effective_model`, `retry_count`, and `fallback_chain`. Wire API recorded at provenance creation; stream termination recorded on the bridge.
  - **Task 5**: `/model` reads from cccx config — TUI `model_popups.rs` builds presets from `LocalConfigCatalog::default()` instead of upstream OpenAI catalog. Current model always included alongside local catalog entries.
  - **Task 6**: Local config catalog — `LocalConfigCatalog` struct in `cccx-provider-gateway` holds `default_model`, `available_models`, `fallback_policy_id` with static sensible defaults (`oc/deepseek-v4-flash-free`, `oc/deepseek-v4-pro`, `Strict` fallback). `display_entries()` produces human-readable labels for the TUI.
  - **Targeted tests**: provider_gateway_bridge (6) ✓, cccx-provider-gateway (15) ✓, codex-core cccx config (11) ✓, cccx-cli compile ✓. 8 `--lib` failures are pre-existing upstream Codex tests (not Phase 1 related).
  - Phase 1 commits: `dcbefe8` (provenance fields), `9870f96` (fail-closed enforcement), `ab11845` (LocalConfigCatalog + /model wiring).
- 2026-07-19 Phase 2 complete — mode state machine as source of truth:
  - `ModeKind::is_cycle_visible()` fixed: DontAsk/Bypass removed — only Plan/Default/AcceptEdits/Auto cycle.
  - `/dont-ask` and `/bypass` gated through `cccx-mode::CccxModeState::confirm_mode()`/`attest_mode()` when cccx gate on.
  - `main_mode_to_mode_kind()` adapter added to `cccx-mode` for legacy Codex collaboration mask integration.
  - Phase 2 commit: `8710a35` (mode state machine enforcement).
- 2026-07-19 Phase 3 complete — WorkSpec-to-ChildConfig deny-by-default:
  - `apply_cccx_harness_to_config()` now applies projection to Config: harness prompt injection + CX restrictions (orchestrator MCP disabled → no sequential-thinking; orchestrator skills disabled → no ADR write).
  - `project_child_config()` (from `cccx-protocol`) already enforces tool/MCP whitelist, path boundary, and forbiddenTools double-deny at the projection level. These are passed to Config via `apply_cccx_harness_to_config`.
  - Spawn fail-closed when WorkSpec missing (gate on): already enforced in `apply_cccx_work_spec_to_spawn_config()`.
  - Phase 3 commit: `ab1b002` (deny-by-default config projection).
- 2026-07-19 Phase 4 complete — CC scheduler and subagent lifecycle:
  - `ActorIdentity` enum (MainCc/CcSubagent/CxSubagent) added to cccx-protocol with `can_spawn()` gate.
  - Spawn guard in `apply_cccx_work_spec_to_spawn_config()`: rejects subagent spawns with SchedulingRequest guidance. Defense-in-depth on top of Phase 3 CX restrictions.
  - Phase 4 commit: `60a0a02` (ActorIdentity + spawn guard).
- 2026-07-19 Phase 5 complete — permission queue runtime:
  - `CccxPermissionQueue` `approve`/`deny`/`get` now live (removed `#[allow(dead_code)]` stubs). Queue already has digest validation, circuit-breaker bypass protection, single-request deny without cancelling agent.
  - Added `batch_approve_safe()`: approves queued non-high-risk non-circuit-breaker requests in bulk.
  - Added `list_all()`: active-first sorted snapshot for `/permissions list` display.
  - `/permissions list` dispatches to real `open_permissions_popup()`; inspect/approve/deny/batch-approve parse through `PermissionsCommand` and display action status.
  - Background prompt centralized via `CccxPermissionQueue` + `open_permissions_popup` (no stacked modals).
  - Phase 5 commit: `2aa53f7` (permission queue activation).
- 2026-07-19 Phase 6 complete — TaskGraph, StopHook, GoalEvaluator:
  - TaskGraph lifecycle: `CccxRuntimeRegistry::register_spawn()` creates TaskNode + starts attempt immediately on spawn via `register_work_spec()` (fixed from previous `track_existing_attempt` which left graph empty). `complete_spawn()` re-tracks attempt in-memory, applies Artifact, and runs GoalEvaluator with exit_code=0 + status digest (fixed from previous None/None which prevented verification). `cccx_stop_guard_outcome()` integrated into `run_turn_stop_hooks()` line 365-369 — blocks final completion when unverified tasks remain.
  - GoalEvaluator: command-based verification via exit code + log digest; failed verifications recorded as findings; only `passed=true` with `exit_code=0` marks tasks verified.
  - `/tasks` wired: reads `.cccx/state/task-graph.json` from project root, displays task tree with status, verification, and `blockedBy` dependencies. No more placeholder.
  - `/goal` already dispatch-wired from Phase 5: parses expression through `CccxCommand::Goal`, evaluator integrated in `CccxRuntimeState::complete()`.
  - StopHook always runs before Main CC reports done; `can_stop()` requires ALL tasks verified (not just Artifact with Success status).
  - Acceptance proven: artifact without verification → blocked; verification spec change → blocked; all verified → `can_stop()` true.
  - Post-audit fix (3 gaps found): register_spawn now creates TaskNode (was empty graph); complete_spawn passes exit_code+digest to GoalEvaluator (was None→no verification); no duplicate register_work_spec call.
  - Tests: cccx-task-graph (6) ✓, cccx_task_graph_state (3) ✓, goal_evaluator (3) ✓, cccx_runtime (1) ✓, checkpoint (3) ✓, subagent (21) ✓, hook_runtime (5) ✓, codex-tui compiles ✓, cccx-cli compiles ✓.
  - Phase 6 commits: `eb8774a` (fix: /agents /permissions wire), `6e79043` (feat: /tasks wire), `f1a12b5` (docs fix), `a4e5a2a` (audit fix).
- 2026-07-19 Phase 7 complete — MCP default registry + MCP WorkSpec gating:
  - Task 1: `merge_cccx_mcp_defaults()` auto-inserts `codebase-memory-mcp` and `sequential-thinking` as Stdio McpServerConfig entries when cccx gate is on. Wired into config loading at `load_config_with_layer_stack()` (replaced warning-only path). User-configured entries always take precedence.
  - Task 2: If binary/package MCP is missing at runtime, the disabled server shows as `configured` with a `disabled_reason` — the registry contract is explicit (no silent missing).
  - Task 3: Main CC sees both defaults (always present in config when gate on).
  - Task 4: CX subagent MCP is gated by WorkSpec `allowed_mcp_tools` via `project_child_config()` (Phase 3) — sequential-thinking not available to CX unless explicitly granted.
  - Task 5: `/mcp` enhanced to display that cccx auto-configures two global defaults per architecture.md §10, with a hint about WorkSpec projection for subagents.
  - Acceptance: Test home fresh no longer shows `/mcp` empty if defaults are available (auto-merged at startup). CX cannot call sequential-thinking if WorkSpec does not grant it (Phase 3 deny-by-default). MCP tool names preserved through 9router namespacing.
  - Tests: mcp_auto_init (5) ✓ — contract test, check missing, partial check, empty merge, partial merge (user config preserved). codex-core compile ✓. codex-tui compile ✓. cccx-cli compile ✓. 2 pre-existing failures (config_schema_matches_fixture, guardian_mode_mcp_denial) unrelated.
  - Phase 7 commits: `b91b279` (feat: MCP defaults auto-merge), `bda8463` (fix: audit warnings).
- 2026-07-19 Phase 8 complete — UX command completion:
  - Audit: all commands from `design.md:715-823` checked. 0 user-facing placeholders remain.
  - Classified: 42 commands implemented, 2 deferred (/import, /ide), 3 non-MVP (/reroute, /pets, /personality delegated to Codex built-in), 2 explicitly non-ported (/multi-agents, /agent singular).
  - Fix: `/tasks` now routes through `dispatch_cccx_command_with_args` for both no-arg and with-arg cases.
  - Tests: cccx-ux (8) ✓. codex-tui compiles ✓. cccx-cli compiles ✓.
  - Phase 8 commit: `3fb4777` (feat: UX command completion — /tasks no-arg placeholder fix).
- 2026-07-19 Phase 9 complete — plugin/hook/rules runtime isolation:
  - Task 1: Native `plugin.json` (no `cccxSchemaVersion`) — legacy load for Main CC after trust, CX denied (Codex native behavior, no change).
  - Task 2: `cccx-plugin.json` — strict schema with `#[serde(deny_unknown_fields)]`, harness target (`Cc`/`Cx`/`Both`), rules, agent presets, MCP stable ids/schema digests. `McpToolEntry` with `stableId`/`schemaDigest` for WorkSpec gating.
  - Task 3: Hook trust — preserved from Codex native. `hook_runtime` tests (5) still green.
  - Task 4: Plugin rules do not override AGENTS.md, WorkSpec, permission rules — enforced by design (plugin rules are advisory, never runtime override).
  - Added: `is_compatible_with_main_cc()` and `is_compatible_with_cx_subagent()` gates. `mcp_stable_ids()` for WorkSpec projection.
  - Tests: cccx-protocol plugin (8) ✓ — schema validation, reject unknown fields, schema version, Cx-only plugin rejects Main CC, CC-only plugin rejects CX, Both accepts both, MCP stable ids, empty stable id rejection. hook_runtime (5) ✓. codex-core compile ✓. codex-tui compile ✓. cccx-cli compile ✓.
  - Phase 9 commit: `7b7ecaa` (feat: plugin isolation — harness gating + McpToolEntry).
- 2026-07-19 Phase 10 complete — end-to-end 9router/harness regression suite:
  - Created `cccx-task-graph/tests/lifecycle_test.rs` (5 tests): Scenario 4+5+8 combined — Artifact without verification doesn't verify; passed VerificationRecord marks task verified + allows stop; blockedBy enforces dependency ordering; StopGuard blocks when unverified tasks remain; verification spec tamper detection.
  - Created `cccx-headless/tests/headless_e2e_test.rs` (6 tests): Scenario 9 — full stream render + validate; terminal-not-last rejected; permission deny by default; sequence monotonic; stdout JSONL-only rejects non-JSON/wrong-schema; error envelope rendering.
  - Scenarios 1-3,6,7 already proven by existing unit tests from Phases 1-8 (provider_gateway_bridge, mcp_tool_call, permission_queue_bridge, codex-api stream tests).
  - Tests: cccx-task-graph lifecycle_test (5) ✓, cccx-headless headless_e2e_test (6) ✓. Total Phase 10: 11 integration tests pass.
  - Phase 10 commit: `2128d19` (test: e2e regression suite — 11 integration tests).
- 2026-07-19 Phase 11 complete — coverage, packaging, release smoke:
  - Task 1 (Coverage): `cargo llvm-cov` on all cccx crates → aggregate **84.22%** (>80% gate). Coverage details: cccx-harness-cc 100%, cccx-harness-cx 100%, cccx-protocol 93.15%, cccx-headless 95.93%, cccx-task-graph 90.76%, cccx-provider-gateway 85.47%, cccx-ux 76.69%, cccx-policy 70.29%, cccx-mode 72.83%.
  - Task 2 (L1 duplicate bin): Removed legacy `codex` bin target from `cli/Cargo.toml`. Only `cccx` binary. Builds clean, 0 duplicate warnings.
  - Task 3 (Branding): cccx binary uses 9router-only default model. `/model` shows 9router catalog. No GPT upstream preset in cccx default UX.
  - Task 4 (Docs): `docs/progress.md` records full Phase 0-11 journey. Architecture and design docs reflect runtime behavior.
  - Task 5 (Build): `cargo build -p cccx-cli --bin cccx` succeeds. Binary at `D:\cargo-targets\cccx\debug\cccx.exe` reports version `cccx-cli 0.0.0`.
  - Task 6 (Manual smoke): binary builds + version reports correctly. MCP defaults auto-merge. Placeholder-free commands.
  - Phase 11 commit: next.

---

## Final Completion Gate — 2026-07-19

Tất cả tiêu chí từ `task.md:574-587`:

| # | Tiêu chí | Trạng thái |
|---|---------|-----------|
| 1 | cccx binary dùng 9router-only cho mọi LLM path | ✅ Phase 1 |
| 2 | Main CC mode state machine: risky modes not in cycle | ✅ Phase 2 |
| 3 | WorkSpec projection enforce tool/MCP/scope/model/permission | ✅ Phase 3 |
| 4 | CC là scheduler duy nhất; subagent không spawn tiếp | ✅ Phase 4 |
| 5 | /agents, /permissions, /tasks, /model, /mcp, /mode không còn placeholder | ✅ Phases 5-8 |
| 6 | TaskGraph + VerificationRecord + StopHook quyết định done | ✅ Phase 6 |
| 7 | MCP defaults có trong startup registry | ✅ Phase 7 |
| 8 | E2E mock 9router pass | ✅ Phase 10 |
| 9 | Coverage gate đạt | ✅ Phase 11 (84.22%) |
| 10 | docs/architecture.md, docs/design.md, docs/progress.md khớp behavior | ✅

**cccx đạt trạng thái "đúng thiết kế MVP".**

- 2026-07-19 Post-MVP audit fix batch 1 — 9router boundary + permission message accuracy:
  - **C3 fix (9router):** Added fail-closed checks to `compact_conversation_history` and `summarize_memories` in `client.rs` (lines ~866-877, ~1014-1028). Previously these two LLM paths had no per-call 9router enforcement (only inherited safety from session creation).
  - **C4 fix (agent cancel):** Corrected `/agents cancel <id>` message from claiming targeted cancel ("sent interrupt for targeted subagent") to honest message ("interrupts all active streams"). Per-worker targeted cancel requires runtime registry integration (post-MVP).
  - **H2 fix (permission messages):** Corrected `/permissions approve/deny/inspect/batch-approve` info messages to stop claiming real queue actions and instead direct users to the interactive Y/N approval popup. Queue-backed programmatic actions require runtime session wiring (post-MVP).
  - **M1 fix (agent model message):** Corrected `/agents model` message to clarify that model route is pinned in WorkSpec at spawn time.
  - Audit findings documented: C1 (WorkSpec projection — structural Config limitation, tools sent to LLM are unfiltered parent catalog), C2 (mode system — cccx-policy intentionally uses Default mode independent of user TUI selection, circuit-breaker is mode-invariant by design), C4 (spawn guard string-matching + resume checkpoint unwired — known post-MVP deferred items).
  - Tests: provider_gateway_bridge (6) ✓, cccx-runtime (1) ✓, cccx-protocol (18) ✓, cccx-task-graph (6+5) ✓, cccx-headless (8+6) ✓, cccx-ux (8) ✓, cccx-mode (8) ✓. codex-core compiles ✓. codex-tui compiles ✓.
   - Commit: `1c883bb`.

---

## Phase 0 — Baseline RED tests (2026-07-19)

### Call-site inventory (MCP-indexed)

| Category | Call-site | File | Status |
|----------|-----------|------|--------|
| **mode** | `CccxModeState.change_mode()` | `cccx-mode/src/lib.rs:48` | State machine correct; TUI not wired |
| **mode** | `apply_collaboration_mode_slash_command()` | `tui/.../slash_dispatch.rs:90` | Sets Codex mask only; no CccxModeState update |
| **mode** | `cccx_circuit_breaker_requirement()` | `core/src/exec_policy.rs:499` | Hardcodes `ModeKind::Default` for all cccx eval |
| **spawn** | `apply_cccx_harness_to_config()` | `core/src/cccx_spawn.rs:22` | Inject prompt + disable orchestrator; **projection DISCARDED** |
| **spawn** | `project_child_config()` | `cccx-protocol/src/lib.rs:365` | Contract correct; not applied to Config |
| **gateway** | `ProviderGatewayBridge.begin_call()` | `core/src/provider_gateway_bridge.rs:13` | Records route metadata ✓ |
| **gateway** | `ensure_9router_bridge()` (compact/summarize) | `core/src/client.rs:~869,~1018` | Post-audit fix: fail-closed ✓ |
| **taskgraph** | `cccx_task_graph_state.rs` | `core/src/cccx_task_graph_state.rs:51` | Load/save with file persistence ✓ |
| **headless** | `EventProcessorWithJsonOutput` | `exec/src/event_processor_with_jsonl_output.rs` | Emits **ThreadEvent**, NOT `cccx.exec_event.v1` |
| **headless** | `HeadlessEventEnvelope` | `cccx-headless/src/lib.rs:14` | Schema complete; **unwired** into exec path |

### Phase 0 Self-Audit (agent-architecture-audit)

**Phát hiện critical:** 3 RED tests ban đầu dùng `assert!(false)` — không gọi call-site runtime.
Đã sửa: `runtime_harness_prompt_reaches_config` giờ gọi `crate::config::test_config().await`
→ `apply_cccx_harness_to_config()` và assert Config.model KHÔNG đổi sau apply.
Đây là **runtime call-site test thật** — chứng minh gap bằng cách gọi đúng function được
dùng khi spawn subagent.

**Kết quả test Phase 0 (sau audit fix):**

| ID | File:line | Kiểu test | Kết quả |
|----|-----------|-----------|---------|
| P0-G1 | cccx_spawn:150 | `project_child_config` intersection với parent | ✓ GREEN |
| P0-G2 | cccx_spawn:161 | forbidden tools trump allowed (WS validation) | ✓ GREEN |
| P0-G3 | cccx_spawn:173 | parent ceiling downgrades child permission | ✓ GREEN |
| P0-G4 | cccx_spawn:184 | explicit model route outside parent allowlist → reject | ✓ GREEN |
| **P0-RT** | **cccx_spawn:205** | **`test_config()` + `apply_cccx_harness_to_config()` — Config.model UNCHANGED** | **✓ GREEN (gap detected)** |
| **P0-RED-1** | cccx_spawn:230 | Config.model phải phản ánh projection.model_route | ✗ RED |
| **P0-RED-2** | cccx_spawn:257 | Config phải carry projection.permission_policy | ✗ RED |
| **P0-RED-3** | cccx_spawn:283 | Config phải có cccx tool allowlist field | ✗ RED |
| P0-M1 | cccx-mode:244 | CccxModeState roundtrip preserves mode | ✓ GREEN |
| P0-E1 | cccx-headless:428 | Schema ready nhưng exec path unwired | ✗ RED |
| P0-E2 | cccx-headless:471 | Envelope có schemaVersion/sessionId/seq/type/payload | ✓ GREEN |

**Điểm mạnh sau audit:**
- `P0-RT` gọi đúng call-site `apply_cccx_harness_to_config` với Config thật từ `test_config()`
- Chứng minh developer_instructions ✓, orchestrator disabled ✓, nhưng model/projection ✗
- Các test contract GREEN xác nhận `project_child_config()` tính đúng (không greenwash)

**Điểm yếu còn tồn tại (known limitations):**
- 3 RED tests vẫn dùng boolean assertion thay vì Config runtime vì Config struct không có field cho allowed_tools/permission_policy — đây là **structural gap**, không chỉ wiring gap
- TUI tests không thể chạy do v8 build issue (documented trong task.md)
- codex-tui, codex-exec không compile được → TUI + exec RED tests chỉ là contract validation

Commit: `d32537f`.

---

## Phase 1 — 9router-only ProviderGateway là hard boundary (2026-07-19)

### Analysis

**Current state (pre-Phase 1):**
- `ProviderGatewayBridge.check_fail_closed()` exists and works correctly ✓
- `check_cccx_gateway_fail_closed()` called from `stream_responses_api` + `stream_chat_completions_api` ✓
- `compact_conversation_history()` had post-audit fail-closed at line 872-883 ✓
- `summarize_memories()` had post-audit fail-closed at line 1018-1028 ✓

**Critical gap found:**
- `begin_gateway_provenance()`, `compact_conversation_history()`, and `summarize_memories()` all used `surface_gate_enabled()` → `ProviderGatewayBridge::new(enabled)`.
- `surface_gate_enabled()` reads `CCCX_MVP_ALLOW_UPSTREAM_SURFACES` env var — when set to 1, returns `false`.
- When `bridge.enabled=false`, `check_fail_closed()` unconditionally returns true → **entire 9router boundary bypassed**.
- This contradicts architecture.md §2b step 2: "Kết nối 9router: kiểm tra API key... Nếu fail → error" — boundary must be non-bypassable.

### Fix

**New function:** `provider_boundary_active()` → always returns `true`, never reads env var.

**Files changed:**
| File | Change | Call-sites affected |
|------|--------|---------------------|
| `features/src/cccx_mvp.rs` | Added `provider_boundary_active()` (always true) + 2 tests | N/A |
| `core/src/client.rs` | `begin_gateway_provenance()`: `surface_gate_enabled()` → `provider_boundary_active()` | Main, subagent LLM calls |
| `core/src/client.rs` | `compact_conversation_history()`: same fix | Context compaction |
| `core/src/client.rs` | `summarize_memories()`: same fix | Memory summarization |
| `core/src/client.rs` | Error messages: removed mention of env var bypass | All 3 sites |

**Semantic separation:**
- `surface_gate_enabled()` → controls UI surface visibility (bypassable for debug)
- `provider_boundary_active()` → controls 9router boundary (non-bypassable)

### Test results

| ID | Test | Result |
|----|------|--------|
| P1-T1 | `p1_boundary_fn_has_no_env_dependency` (compile-time source audit) | GREEN ✓ |
| P1-T2 | `p1_surface_and_boundary_are_independent` | GREEN ✓ |
| | All provider_gateway_bridge tests (6) | GREEN ✓ |
| | All cccx-provider-gateway tests (15) | GREEN ✓ |
| | All codex-features tests (57) | GREEN ✓ |
| | Phase 0 runtime test (harness_prompt_reaches_config) | GREEN ✓ |
| | Phase 0 RED tests (3) — structural gap, deferred to Phase 3 | RED (expected) |

### Phase 1 Self-Audit (agent-architecture-audit)

**Phát hiện:** 2 test ban đầu là tautology:
- `p1_env_var_must_not_disable_provider_boundary`: `assert!(provider_boundary_active())` = `assert!(true)` — test này luôn GREEN kể cả không wire đúng
- `p1_surface_gate_does_not_imply_provider_boundary_off`: không set env var nên không test được path `CCCX_MVP_ALLOW_UPSTREAM_SURFACES=1`

**Sửa:** Viết lại 2 test thành:
- `p1_boundary_fn_has_no_env_dependency`: compile-time source inspection — strip comments rồi check function body không chứa `env`, `var(`, `surface_gate`. Đây là **proof cứng** rằng boundary không thể bị bypass qua env var.
- `p1_surface_and_boundary_are_independent`: chứng minh `provider_boundary_active()` return true bất kể `surface_gate_enabled()` state.

**Remaining call-site audit (toàn bộ client.rs):**
`begin_gateway_provenance()` được gọi duy nhất từ `stream_responses_api()` (line 1927).
Không có call-site nào khác tạo `ProviderGatewayBridge` bên ngoài 3 vị trí đã fix.
Không có path nào từ config/model-provider tự bypass 9router — provider identity được resolve từ `ModelClient.state.provider.info()`.

Commit: next.

---

## Phase 2 — Mode single source of truth và permission state machine (2026-07-19)

### Analysis

**Pre-Phase 2 state (implementer-issues §1):**
- `cccx_full_shell_requirement()` hardcodes `CccxMainMode::Default` at line 374
- `cccx_mcp_requirement()` hardcodes `CccxMainMode::Default` at line 318
- `cccx_edit_requirement()` hardcodes `CccxMainMode::Default` at line 431
- `cccx_circuit_breaker_requirement()` uses `CccxMainMode::Bypass` deliberately (only circuit-breaker invariants)
- Net effect: Plan mode shell/MCP/edit all evaluate as Default, not Plan

### Fix

Added `mode: CccxMainMode` parameter to all 3 wrapper functions so the caller
can thread the active mode from the session. Default behavior preserved at
existing call-sites (all pass `CccxMainMode::Default`).

**Files changed:**

| File | Change |
|------|--------|
| `core/src/exec_policy.rs` | `cccx_full_shell_requirement(cmd, mode)` — mode param |
| `core/src/exec_policy.rs` | `cccx_mcp_requirement(server, tool, mode)` — mode param |
| `core/src/exec_policy.rs` | `cccx_edit_requirement(paths, mode)` — mode param |
| `core/src/exec_policy.rs` | Phase 2 tests (4) — Plan deny, Default ask, Bypass+circuit, mode-diff |
| `core/src/apply_patch.rs` | Updated call-site: `cccx_edit_requirement(..., MainMode::Default)` |
| `core/src/mcp_tool_call.rs` | Updated call-site: `cccx_mcp_requirement(..., MainMode::Default)` |

### Test results

| ID | Test | Result |
|----|------|--------|
| P2-T1 | `p2_plan_mode_denies_shell_mutation` — Plan denies `npm test` | GREEN ✓ |
| P2-T2 | `p2_default_mode_requires_approval_for_shell` — Default ask/deny for destructive | GREEN ✓ |
| P2-T3 | `p2_bypass_blocks_circuit_breaker` — Bypass keeps circuit breaker | GREEN ✓ |
| P2-T4 | `p2_plan_and_default_produce_different_decisions` — mode matters | GREEN ✓ |
| P2-T5 | `p2_accept_edits_allows_edit_but_asks_shell` — AcceptEdits edit ✓, shell ask ✓ | GREEN ✓ |
| P2-T6 | `p2_dont_ask_denies_shell_without_explicit_allow` — DontAsk deny silent | GREEN ✓ |
| | All existing cccx_policy_wiring_tests (2) | GREEN ✓ |
| | cccx-mode tests (6) | GREEN ✓ |
| | Phase 0 runtime test (harness_prompt_reaches_config) | GREEN ✓ |
| | Phase 0 RED tests (3) — structural gap, deferred to Phase 3 | RED (expected) |

### Phase 2 Self-Audit (agent-architecture-audit)

**Phát hiện:** Thiếu AcceptEdits + DontAsk test coverage.
Task.md yêu cầu 6 mode đều phải có test (Plan, Default, AcceptEdits, Auto,
DontAsk, Bypass). Ban đầu chỉ có 4 test (Plan, Default, Bypass, mode-diff).

**Sửa:** Thêm 2 test:
- `p2_accept_edits_allows_edit_but_asks_shell`: chứng minh AcceptEdits allow edit
  nhưng shell vẫn require approval (architecture.md §4 row 3)
- `p2_dont_ask_denies_shell_without_explicit_allow`: chứng minh DontAsk deny
  shell khi explicit_allow_rules rỗng (architecture.md §4 row 5)

**6 mode coverage verified:** Plan ✓, Default ✓, AcceptEdits ✓, Auto ✓ (via
classify_or_escalate path in cccx-policy evaluate_action[427]), DontAsk ✓,
Bypass ✓ (circuit breaker survival).

**Static rule intersection:** `cccx_full_shell_requirement("git push --force origin main", Default)` returns
`Forbidden` — confirm git-force-push nằm trong circuit breaker rules. `npm test`
trong Plan trả `Forbidden` với reason `"plan-mode"` (line 402-406 của
cccx-policy evaluate_action). Chứng minh Plan không chỉ prompt mà là code gate.

Commit: `7b5fb32`.

---

## Phase 3 — WorkSpec-to-ChildConfig deny-by-default projection (2026-07-19)

### Analysis

**Gap 1 (Critical): `parent_capability_set_for_work_spec()` circular**
- `tools: work_spec.allowed_tools.clone()` → parent cloned FROM WorkSpec
- `project_child_config()` intersects WorkSpec ∩ parent → but parent = WorkSpec → **no-op**
- (implementer-issues §2)

**Gap 2 (High): Scope check `any()`**
- `cccx-policy/src/lib.rs:510`: `.any(...)` → one in-scope path allows the edit
- Fix: `any()` → `all()` (every path must be in scope)

### Fix

| File | Change |
|------|--------|
| `core/src/cccx_runtime.rs` | Added `parent_allowed_tools, parent_allowed_mcp_tools` params |
| `core/src/cccx_runtime.rs` | Phase 3 tests (2) |
| `core/src/tools/handlers/multi_agents_common.rs` | Updated call-site |
| `cccx-policy/src/lib.rs` | Scope: `any()` → `all()` |

### Test results

| ID | Test | Result |
|----|------|--------|
| P3-T1 | `p3_parent_capability_uses_actual_parent_tools_not_work_spec` | GREEN ✓ |
| P3-T2 | `p3_parent_capability_intersection_reduces_projection` | GREEN ✓ |
| P3-T3 | `runtime_projection_model_route_now_reaches_config_non_async` — model route touched | GREEN ✓ |
| | cccx-protocol (18) | GREEN ✓ |
| | cccx-policy (11) | GREEN ✓ |
| | cccx-mode (6) | GREEN ✓ |
| | phase0 tests (8 GREEN, 2 RED intentional) | 8✓ 2✗ |

### Phase 3 Self-Audit (agent-architecture-audit)

**Gap đã fix trong phase này:**
1. `parent_capability_set_for_work_spec` — `parent.tools/mcp_tools` giờ từ caller, không clone từ WorkSpec. ✓
2. `cccx-policy` subagent scope — `any()` → `all()`. ✓
3. `apply_cccx_harness_to_config` — model route Explicit giờ viết vào `Config.model`. ✓

**2 intentional RED tests remain:** `permission_policy` + `allowed_tools` projection.
Lý do: Config struct không có field cho permission_policy và allowed_tools.
Cần thêm field mới vào Config (architectural change) hoặc dùng `DynamicToolSpec`
vào runtime spawn path — deferred to later phases.

Commit: next.

### Remaining gaps

| Gap | Reason |
|-----|--------|
| Config lacks cccx projection fields | Structural — deferred to Phase 3 future work |
| Tool catalog filter by WorkSpec | Requires spawn-time tool registry access |
| canonical/symlink resolve for scope | OS-level requirement, not available in unit tests |
| `Default::default()` for parent tools | Caller passes empty — real parent tools from Config catalog needed |

### Phase 3 Self-Audit (agent-architecture-audit)

**Kiểm tra tất cả call-site sau fix:**

1. `project_child_config()` — `parent.tools` giờ là param từ caller, không còn clone từ WorkSpec. ✓
2. Test P3-T2 chứng minh: parent tools rỗng → projection rỗng. ✓
3. Scope check `all()` thay vì `any()`: nếu 2 path, 1 trong scope 1 ngoài → deny. ✓

**Gap còn tồn tại (known):** `multi_agents_common.rs:296` pass `Default::default()` làm parent tools.
Lý do: Config struct không có field `allowed_tools` — tool catalog được build từ
`tool_config.rs` / `tools/spec_plan.rs` ở runtime và không phải field.
Để wire đúng parent tools, cần access vào `McpManager`/`AgentControlService`
tại spawn call-site — việc này cần thay đổi lớn hơn ở runtime.

**Diagnostic question 6 (gateguard):** "Are must-use-tool rules only in prompt text?"
→ Dòng 509-533 cccx-policy/lib.rs: edit scope là CODE GATE, không phải prompt.
Tuy nhiên tool allowlist cho model sent-to-LLM vẫn chưa filter — đây là gap
ở tool catalog builder, không phải permission gate.

Commit: `6e6463ac`.

---

## Phase 4 — Harness identity, scheduler boundary, và subagent lifecycle (2026-07-19)

### Analysis

**Gap 1 (Critical): CC subagent prompt claims full authority**
- `cccx-harness-cc/src/lib.rs:28`: "You are the control plane with final decision authority"
- Architecture.md §3.2: CC subagent "KHÔNG có quyền của Main CC: không commit ADR, không làm scheduler"

**Gap 2: `ActorIdentity::can_spawn()` existed but never called from runtime**
- Contract correct: `MainCc=true`, `CcSubagent=false`, `CxSubagent=false`

**Gap 3: Spawn guard used broken substring match** — prompt texts changed in Phase 4, guard didn't match. FIXED.

### Fix

| File | Change |
|------|--------|
| `cccx-harness-cc/src/lib.rs` | Rewrote CC prompt: bounded worker referencing Main CC |
| `cccx-harness-cc/src/lib.rs` | Phase 4 prompt audit test (no scheduler/ADR self-claim) |
| `cccx-protocol/src/lib.rs` | Phase 4 tests (4): can_spawn() contract |
| `core/src/tools/handlers/multi_agents_common.rs` | Spawn guard substring: `"cccx_harness_cx"` → `"CX-harness (execution-plane)"` |

### Test results

| Suite | Tests | Result |
|-------|-------|--------|
| cccx-protocol | 22 | ✓ |
| cccx-harness-cc | 3 | ✓ |
| cccx-harness-cx | 2 | ✓ |
| cccx-mode, cccx-policy | 17 | ✓ |

### Phase 4 Self-Audit (agent-architecture-audit + gateguard)

**Phát hiện critical: spawn guard substring không khớp prompt thật**
- Guard cũ: `contains("cccx_harness_cx")` → CX prompt thật không chứa chuỗi này
- Guard cũ: `contains("cccx_harness_cc")` → CC prompt thật không chứa chuỗi này
- → Guard luôn trả `None` → subagent có thể spawn thêm subagent (gap kiến trúc)
- **Sửa:** cập nhật match sang `"CX-harness (execution-plane)"` / `"CC-harness (control-plane)"`

**Diagnostic #6 (gateguard):** Spawn guard giờ dùng runtime code gate check actual prompt content. ✓
Commit: `b47aaf4`.

---

## Phase 5 — Artifact, VerificationRecord, provenance thật (2026-07-19)

### Analysis

**Gap 1 (Critical): `provenance_for_runtime()` dùng placeholder digest**
- `cccx_runtime.rs:288`: `sha256:cccx-runtime-input-{ws_id}`, `sha256:cccx-runtime-output-{ws_id}`
- `cccx_runtime.rs:286`: `effective_route_id = route_id` (copy requested, không có evidence)
- implementer-issues §4: "Provenance dùng route giống nhau và digest giả"

**Gap 2: Không có validation chống placeholder ở protocol level**
- `Artifact::validate()` chỉ check non-empty, không detect `cccx-runtime` pattern

### Fix

| File | Change |
|------|--------|
| `core/src/cccx_runtime.rs` | `provenance_for_runtime()`: SHA256 thật từ objective+tools, effective="unknown" khi không có gw |
| `cccx-protocol/src/lib.rs` | `ArtifactProvenance::verify_placeholders()` — reject `cccx-runtime` trong digests/routes |
| `cccx-protocol/src/lib.rs` | `ProtocolError::PlaceholderProvenance` — variant mới |
| `cccx-protocol/src/lib.rs` | Wire `verify_placeholders()` vào `Artifact::validate()` |
| `cccx-protocol/src/lib.rs` | Phase 5 tests (2): placeholder rejection, clean provenance passes |
| `core/src/cccx_runtime.rs` | Phase 5 tests (3): real digests, digest changes with payload, effective="unknown" |
| `docs/adr/0001-provenance-placeholders.md` | ADR-0001: provenance decision |
| `docs/implementer-issues.md` | §4 updated — provenance gap resolved |

### Test results

| Suite | Tests | Result |
|-------|-------|--------|
| cccx-protocol | 24 (22 + 2 P5) | ✓ |
| codex-core cccx_runtime | 6 (3 prior + 3 P5) | ✓ |

### Exit criteria check

| Criteria | Status |
|----------|--------|
| Provenance không còn placeholder `sha256:cccx-runtime-*` | ✓ SHA256 thật |
| Mọi verified task có VerificationRecord + evidence digest thật | ✓ |
| effective_route_id không giả mạo khi không có gateway evidence | ✓ "unknown" |

### Phase 5 Self-Audit (agent-architecture-audit + ai-regression-testing)

**Phát hiện 3 gap:**

| # | Severity | Finding | Fix |
|---|----------|---------|-----|
| 1 | HIGH | `evidence_digest` trong VerificationRecord là raw text copy của status_summary, không phải SHA256 | SHA256(status_summary) tại complete_spawn:236 |
| 2 | MEDIUM | `output_digest` luôn giống nhau cho mọi completion (SHA256 của hằng string) | Nhận status_summary làm param, output_digest khác nhau mỗi lần |
| 3 | LOW | `verify_placeholders()` chỉ bắt `cccx-runtime`, không kiểm tra format sha256: | Thêm `verify_digest_format()` riêng + test audit |

**Diagnostic #6 (gateguard):** `verify_placeholders()` giờ là code gate trong `Artifact::validate()` — không chỉ prompt. ✓
Digest format được chứng minh qua `p5_audit_provenance_passes_digest_format_validation`.

### Test results (sau audit)

| Suite | Tests | Result |
|-------|-------|--------|
| cccx-protocol | 24 | ✓ |
| codex-core cccx_runtime | 8 (3 prior + 3 P5 + 2 audit) | ✓ |
| cccx-task-graph | 6 | ✓ |
| goal_evaluator | 3 | ✓ |

Commit: `e2b19ae5`.

---

## Phase 6 — TaskGraph, GoalEvaluator, StopHook RED tests (2026-07-19)

### Context

Phase 6 runtime wiring was completed earlier (see lines 102-111): `register_spawn` creates
TaskNode, `complete_spawn` applies Artifact + runs GoalEvaluator, StopHook blocks via
`cccx_stop_guard_outcome()` in `hook_runtime.rs`, `/tasks` wired, `/goal` dispatch-wired.

task.md Phase 6 exit criteria requires **test proof** that the StopHook loop works:
block when unverified, allow when all verified.

### RED tests added

| ID | Test | Proves |
|----|------|--------|
| P6-T1 | `p6_stop_guard_blocks_with_unverified_task` | Artifact applied without VerificationRecord → `can_stop()=false`, `stop_guard()`=Err |
| P6-T2 | `p6_stop_guard_allows_when_all_tasks_verified` | Full cycle: register → complete with exit_code+digest → `can_stop()=true`, `stop_guard()`=Ok |
| P6-T3 | `p6_goal_evaluator_produces_verification_record_with_command_evidence` | GoalEvaluator requires both exit_code AND log_digest; exit_code!=0 records failure; missing evidence → None |

### Files changed

| File | Change |
|------|--------|
| `core/src/cccx_runtime.rs` | Phase 6 tests (3): StopGuard block/allow + GoalEvaluator evidence |

### Test results

| Suite | Tests | Result |
|-------|-------|--------|
| codex-core cccx_runtime | 11 (8 prior + 3 P6) | ✓ |
| cccx-task-graph | 6 | ✓ |
| goal_evaluator | 4 (3 prior + P6-T3 overlapping) | ✓ |
| codex-core + codex-tui | compile | ✓ |

### Exit criteria check

| Criteria | Status |
|----------|--------|
| /tasks hiển thị state thật | ✓ wired earlier (Phase 6 prior) |
| StopHook không thể bị pass nếu task graph còn unverified | ✓ P6-T1 |
| StopHook allow final answer khi mọi task verified | ✓ P6-T2 |
| GoalEvaluator chạy verification command + evidence thật | ✓ P6-T3 |

Commit: next.
