cccx Final Completion Gate — Audit Report
Date: 2026-07-19 | Method: Read-only, 3 parallel subagents cross-referencing code, docs, tests
1. Final Gate: 16 Criteria Verdict
#	Criterion	Status	Key Evidence
1	Main CC chạy CC harness từ startup	FAIL	apply_cccx_harness_to_config() only runs for subagents. No call-site applies CC harness to Main agent.
2	Mọi subagent có actor identity rõ	PARTIAL	ActorIdentity exists + prompt fragments injected for children. But runtime uses substring match (multi_agents_common.rs:268), not structured check.
3	Chỉ Main CC có scheduler/spawn/ADR authority	PARTIAL	CX orchestrator disabled + forbidden tools list in prompt. But no actor.can_spawn() runtime guard before spawn.
4	Mode state machine là source of truth cho permission	FAIL	cccx-mode has complete 6-mode state machine. Permission pipeline hardcodes CccxMainMode::Default everywhere (exec_policy.rs:318,374,432). TUI never calls CccxModeState::change_mode().
5	Bypass cần sandbox attestation thật	FAIL	Circuit breaker retention ✓. attest_mode() does no sandbox verification — only sets mode flag.
6	WorkSpec projection áp vào child config	FAIL	project_child_config() computes full projection. apply_cccx_harness_to_config() applies only: harness prompt + orchestrator flags + explicit model route. allowed_tools, allowed_mcp_tools, writable_scope, permission_policy are COMPUTED BUT DISCARDED (logged only).
7	Subagent không thể dùng tool ngoài WorkSpec	PARTIAL	Gates exist for shell/edit/MCP. But LLM tool catalog sent unfiltered (consequence of #6). Reactive enforcement, not proactive.
8	Mọi LLM call đi qua 9router fail-closed	PASS	provider_boundary_active() hardcoded true. check_fail_closed() rejects non-9router. All calls gated.
9	Provider provenance/digest evidence thật	PASS	SHA256 from WorkSpec payload. effective_route_id="unknown" without evidence. verify_placeholders() gate.
10	Task verified chỉ từ VerificationRecord.passed=true	PASS	record_verification() sets verified_at only on passed=true. StopGuard blocks without it.
11	StopHook block final answer khi TaskGraph unverified	PASS	stop_guard() checks can_stop(). Integrated into run_turn_stop_hooks(). Phase 6 + Phase 13 tests prove.
12	/agents, /permissions, /tasks, /goal, /model, /remember, /mcp không placeholder	PASS	All 7 have real implementations (Phase 5-8). Some have post-MVP limitation messages but are honest, not stubs.
13	cccx exec --json emit cccx.exec_event.v1	PASS	HeadlessEventEnvelope with schema version. Terminal event always last. Phase 11 wired.
14	MCP defaults init đúng	PASS	merge_cccx_mcp_defaults() auto-inserts both. If missing → disabled_reason. User config takes precedence.
15	Project Memory/ADR/plugin isolation	PARTIAL	Plugin isolation ✓. ADR boundary ✓ (CX forbidden). Storage path: .cccx/memory/ vs designed ~/.cccx/projects/<hash>/memory/. Not loaded at startup.
16	Target/build artifacts không git tracked	PASS	.gitignore covers all known artifact paths comprehensively.
Score: 8 PASS / 4 PARTIAL / 4 FAIL
2. The 4 FAILs — Shared Root Cause
All 4 FAILs converge on a single architectural gap: the control-plane/execution-plane boundary exists in design docs and contract crates but is not fully wired into the runtime execution path:
FAIL#	What's Broken	Root Cause
#1	Main CC harness not installed	apply_cccx_harness_to_config() only wired for subagent spawns
#4	Mode state machine doesn't drive permissions	CccxModeState not persisted from TUI to session; exec_policy hardcodes Default
#5	Bypass no sandbox attestation	attest_mode() is a no-op transition — no isolation verification
#6	WorkSpec projection not applied to Config	allowed_tools/allowed_mcp_tools/writable_scope computed but not written into Config struct (structural Config limitation)
3. Implementer-Issues Status
Group	FIXED	PARTIAL	OPEN
Critical	0	3 (C2 scope+capacity, C3 prompt+guard, C4 provenance+digest)	1 (C1 mode split-brain)
High	3 (H5 route, H11 exec_event, H15 surface boundary)	1 (H14 /model catalog)	7 (H6-H10, H12-H13)
7 remaining High issues are in "runtime wiring" — APIs exist but not connected to execution paths: /goal no persistence, TaskGraph only on spawn, /permissions stub, /agents cancel all-streams, checkpoint no runtime call-site, plugin no runtime loader, Project Memory wrong path + not loaded at startup.
4. Architecture vs Runtime Gap Map
Architecture Claim	Actual Runtime
"Mode là state machine"	State machine exists but TUI never writes to it — split-brain
"Main CC chạy CC harness"	Only subagents get harness injection — Main CC bare
"Bypass cần sandbox attestation"	attest_mode() no-ops — bypass gate is open
"WorkSpec projection áp toàn bộ"	Only harness prompt + model route applied — tool/MCP/scope discarded
"MCP defaults luôn đăng ký"	✓ Implemented
"9router fail-closed"	✓ Implemented (hard boundary)
"TaskGraph file-based"	✓ Implemented
"StopHook block unverified"	✓ Implemented
"Headless JSONL v1"	✓ Implemented
"Artsifact/Verification evidence thật"	✓ Implemented
5. Verdict
Claim "cccx khớp thiết kế MVP" is NOT fully supported by code evidence. The system has 4 FAIL criteria that form a connected cluster around three critical areas:
1. Main CC has no harness identity at runtime (#1) — the architectural anchor of CC-led/CX-delegated is missing
2. Mode system is split-brain (#4) — the 6-mode state machine is correct but disconnected from the permission pipeline
3. WorkSpec projection is incomplete (#6) — tool/MCP/scope projection is computed but the result is thrown away, leaving subagent boundaries at prompt-level enforcement only
Conversely, 8 criteria PASS with strong evidence — notably 9router fail-closed, provenance integrity, verification pipeline, UX commands, headless output, MCP defaults, and build hygiene. These confirm substantial progress on the execution-side boundaries.
The project has landed correct contract designs (cccx-protocol, cccx-policy, cccx-mode, cccx-task-graph, cccx-headless, cccx-ux) but the runtime wiring from those contracts into the Codex execution loop is still partial. The system architecture is sound on paper, viable at the contract-test level, but not yet enforced at every runtime boundary as required by AGENTS.md: "Harness behavior must be enforced by state machines, permission gates, hooks, and structured artifacts, not by prompt wording alone."

## 2026-07-22 — Superseding final-gate audit

The 2026-07-19 report above is retained as historical evidence and is no
longer the current verdict. Final-gate Phases 1–8 subsequently closed its
runtime wiring findings and added binary/session proof for the expanded
25-gate matrix in `docs/final-gate-ledger.md`.

Current verdict: **25/25 PASS, 0 OPEN/PARTIAL** for the personal Windows cccx
scope defined by `docs/architecture.md` and `docs/design.md`. Gate 5 is a
deliberate fail-closed `PASS (disabled)`: Bypass cannot activate without a
trusted OS-isolation attestation backend, and the circuit breaker remains
invariant.

Fresh release evidence includes 88.19% aggregate line coverage for the nine
cccx-owned crates; 28 cross-crate core E2E tests; 187 focused core runtime
tests; plugin, TUI, secure-input, stream-error and headless/exec lanes; a final
binary build/smoke; and a deterministic tracked-artifact/generated-source
hygiene gate. Exact commands and counts are recorded at the end of
`docs/progress.md`.
