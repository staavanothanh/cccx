# cccx Fork Plan — Codex CLI → CC/CX Harness

Baseline upstream:

- Source: `D:\Learning_Programing\codex-cli-src`
- Commit: `800715d201651a2a07c2706dca10400109dae3d3`
- Policy: fork from Codex Apache-2.0 source only. Claude Code leaked source is not an implementation input.

Goal: turn Codex CLI into `cccx`, a personal local coding CLI with one 9router model gateway, CC-led/CX-delegated harnesses, WorkSpec/Artifact contracts, strong permission gates, Task Graph, and terminal UX described in `architecture.md` and `design.md`.

---

## 1. Fork Strategy

Use a full source fork of Codex CLI, not selective copy-paste.

Reason:

- Codex has many cross-cutting crates: core, tui, exec, hooks, state, rollout, model-provider, app-server-protocol.
- Subagent, resume, hook, permission and JSONL paths cross crate boundaries.
- Selective copying would lose integration tests and make upstream drift harder to track.

Recommended repo setup:

```text
D:\Learning_Programing\cccx
├─ docs/                         # current design docs
├─ upstream/                     # optional read-only notes, not needed in final tree
├─ codex-cli source files         # copied from codex-cli-src root
└─ .cccx/                        # local dev metadata, ignored if runtime-generated
```

Git plan:

```text
1. Commit current docs as "docs: cccx architecture baseline".
2. Copy Codex source into this repo, excluding Codex .git and generated build output.
3. Commit as "chore: import codex cli baseline 800715d2".
4. Add upstream remote pointing to OpenAI Codex source.
5. All cccx changes happen in small feature commits after import.
```

Do not squash the import with cccx changes. Keeping the import clean makes future `git diff upstream` and rebases tractable.

---

## 2. Copy From Codex Unchanged

Copy these as-is first. Do not rewrite during the import commit.

| Area | Upstream path | Initial policy |
|---|---|---|
| Rust workspace/build | `Cargo.toml`, `Cargo.lock`, `BUILD.bazel`, `MODULE.bazel*`, `justfile`, `defs.bzl` | Copy unchanged |
| CLI launcher | `codex-cli/`, package root files | Copy, later rename/wrap |
| Core runtime | `codex-rs/core/` | Copy, then add cccx boundary modules |
| TUI | `codex-rs/tui/` | Copy, then modify commands/status/agent panels |
| Exec/headless | `codex-rs/exec/` | Copy, then extend JSONL |
| Hooks | `codex-rs/hooks/` | Copy; do not treat hooks as security boundary |
| State/rollout | `codex-rs/state/`, `codex-rs/rollout/`, `codex-rs/rollout-trace/` | Copy; extend only where needed |
| Tool/runtime layer | `codex-rs/apply-patch/`, `codex-rs/sandboxing/`, `codex-rs/linux-sandbox/`, `codex-rs/windows-sandbox-rs/`, `codex-rs/shell-*` | Copy; reuse heavily |
| MCP/plugin/skills base | `codex-rs/codex-mcp/`, `codex-rs/plugin/`, `codex-rs/core-plugins/`, `codex-rs/skills/`, `codex-rs/core-skills/` | Copy; extend cccx manifest later |
| Protocol/app server | `codex-rs/protocol/`, `codex-rs/app-server*` | Copy; extend only after CLI MVP |
| Tests/snapshots | all existing tests | Copy; keep upstream regression safety |

---

## 3. Do Not Copy / Exclude From Import

Exclude generated or local-only content:

```text
.git/
target/
node_modules/
dist/
build/
coverage/
.turbo/
.cache/
*.log
```

Do not copy or import Claude Code leaked source into the cccx repo. If notes are needed, keep them as clean-room behavior summaries in docs only.

---

## 4. Keep But Disable Or Hide For cccx MVP

These Codex features can remain in source to reduce fork churn, but should be disabled, hidden, or hard-wired for the personal cccx MVP.

| Feature | Why | Action |
|---|---|---|
| ChatGPT/OAuth login flows | cccx uses 9router only | Keep source, hide commands/UI, block runtime path unless explicitly re-enabled |
| Direct provider selection outside 9router | Violates 9router-only model backend | Replace config resolution with ProviderGateway route |
| Bedrock/LM Studio/Ollama provider UX | Not needed for MVP | Keep crates if build requires them, but do not expose as first-class cccx UX |
| Remote control/cloud task surfaces | Personal local CLI, not SaaS | Keep source, feature-gate or hide |
| Marketplace/plugin install UX | Not core to MVP | Keep native plugin loader, defer marketplace UX |
| Codex memories | cccx uses explicit Project Memory policy | Disable default memory admission; keep state migrations if needed |
| Full-history subagent fork for execution | Conflicts with WorkSpec/Artifact boundary | Allow only `full_history_debug`, never CX worker execution |

---

## 5. New cccx Modules To Add

Prefer adding cccx-specific crates/modules instead of scattering logic through upstream files. Existing Codex code should call these modules at narrow integration points.

### 5.1 Core Policy Crates

```text
codex-rs/cccx-protocol/
  WorkSpec, Artifact, VerificationRecord, ModelRoute, PermissionRequestQueue,
  TaskGraph event types, JSON schemas.

codex-rs/cccx-provider-gateway/
  9router client, ModelRouteResolver, ModelRouteCatalog, provenance,
  retry/fallback policy.

codex-rs/cccx-policy/
  mode state machine, permission gate, circuit breaker, verification boundary,
  WorkSpecToChildConfigAdapter.

codex-rs/cccx-task-graph/
  task DAG persistence, attempts, worker lifecycle, verification records.

codex-rs/cccx-harness/
  CC/CX harness prompts, role policy, WorkSpec planner, Artifact validator.
```

If creating separate crates is too heavy for the first patch, start under:

```text
codex-rs/core/src/cccx/
```

Then split into crates after the integration points stabilize.

### 5.2 TUI/Exec Additions

```text
codex-rs/tui/src/cccx/
  status_line.rs
  agents_panel.rs
  permission_queue_panel.rs
  task_graph_view.rs
  model_route_view.rs

codex-rs/exec/src/cccx/
  jsonl_events.rs
  headless_permission_policy.rs
```

### 5.3 Config Additions

```text
codex-rs/core/src/config/cccx.rs
codex-rs/core/src/config/cccx_schema.rs
```

Config keys:

```toml
[cccx]
default_mode = "plan"
provider_gateway = "9router"

[cccx.models]
default_subagent_model = "inherit-main-requested"
fallback_policy = "compatible"

[cccx.agents.cx_implementer]
model = "inherit-main-requested"

[cccx.permission]
circuit_breaker = true
```

---

## 6. Upstream Files To Modify

This is the first concrete edit map. Keep modifications narrow and route cccx behavior through new modules.

### 6.1 Model Provider / 9router Boundary

| File/area | Action |
|---|---|
| `codex-rs/model-provider/` | Add or wrap provider implementation for 9router-only route execution |
| `codex-rs/model-provider-info/` | Add cccx route metadata/capability mapping if existing model presets cannot represent 9router routes cleanly |
| `codex-rs/models-manager/` | Feed `ModelRouteCatalog` and hide non-9router direct providers in cccx mode |
| `codex-rs/core/src/client.rs`, `client_common.rs` | Ensure all model calls pass through ProviderGateway provenance path |
| `codex-rs/core/src/config/mod.rs`, `schema.rs` | Add cccx config fields and deprecate direct provider UX for cccx mode |

### 6.2 Harness / Subagent / WorkSpec

| File/area | Action |
|---|---|
| `codex-rs/core/src/tools/handlers/multi_agents_common.rs` | Replace final child config authority with `WorkSpecToChildConfigAdapter` |
| `codex-rs/core/src/tools/handlers/multi_agents.rs` and `multi_agents_v2.rs` | Add cccx spawn path that requires WorkSpec |
| `codex-rs/core/src/tools/handlers/multi_agents/spawn.rs` | Route model override through `ModelRouteResolver`, record `modelRoute` |
| `codex-rs/core/src/tools/handlers/multi_agents/resume_agent.rs` | Add cccx resume validation: checkpoint, base revision drift, immutable model route |
| `codex-rs/core/src/agent/control/spawn.rs` | Preserve Codex resume mechanics, add cccx lifecycle metadata where needed |
| `codex-rs/core/src/agent/role.rs`, `config/agent_roles.rs` | Map role presets to cccx CC/CX roles without granting authority directly |
| `codex-rs/protocol/src/protocol.rs`, `items.rs` | Add cccx WorkSpec/Artifact/agent provenance event fields if needed by TUI/exec |

### 6.3 Permission Gate / Sandbox

| File/area | Action |
|---|---|
| `codex-rs/core/src/tools/approvals.rs` | Insert cccx core permission gate before user/plugin approval |
| `codex-rs/core/src/tools/sandboxing.rs` | Enforce WorkSpec scope and verification protected paths |
| `codex-rs/core/src/tools/runtimes/apply_patch.rs` | Block patch writes outside WorkSpec scope/protected paths |
| `codex-rs/core/src/tools/runtimes/shell.rs`, `unified_exec.rs` | Classify shell mutate/network/config-write actions for cccx gate |
| `codex-rs/core/src/mcp_tool_call.rs` | Apply MCP allowlist from WorkSpec; no CX self-expansion |
| `codex-rs/core/src/hook_runtime.rs` | Keep hooks as extension after core decision, not as circuit breaker |

### 6.4 Task Graph / Verification / Memory

| File/area | Action |
|---|---|
| `codex-rs/state/` | Add cccx task graph storage or keep JSON files under `.cccx/state/` first |
| `codex-rs/core/src/tasks/` | Add TaskGraph lifecycle and VerificationRecord integration |
| `codex-rs/core/src/session/turn.rs`, `turn_context.rs` | Load TaskGraph/WorkSpec state at turn boundaries |
| `codex-rs/core/src/compact*.rs` | Preserve TaskGraph/WorkSpec/Artifact across compaction |
| `codex-rs/state/src/runtime/memories.rs` | Disable auto memory admission for cccx, keep explicit Project Memory only |

### 6.5 TUI / Commands

| File/area | Action |
|---|---|
| `codex-rs/tui/src/slash_command.rs` | Add `/agents`, `/tasks`, `/permissions`, `/remember`, `/autoreview`; avoid `/agent` singular |
| `codex-rs/tui/src/chatwidget/command_lifecycle.rs` | Route new commands to cccx core actions |
| `codex-rs/tui/src/bottom_pane/status_line_*` | Add cccx mode/harness/tasks/model route/permission queue status |
| `codex-rs/tui/src/app/agent_status_feed.rs` | Extend with logicalSessionId/displayName/modelRoute |
| `codex-rs/tui/src/chatwidget/permission_popups.rs`, `permissions_menu.rs` | Central PermissionRequestQueue |
| `codex-rs/tui/src/chatwidget/model_popups.rs` | Restrict model UI to 9router route catalog |

### 6.6 Headless / JSONL

| File/area | Action |
|---|---|
| `codex-rs/exec/src/exec_events.rs` | Add cccx `agent-update`, `task-update`, `permission-request`, `verification` payloads |
| `codex-rs/exec/src/event_processor_with_jsonl_output.rs` | Emit cccx route provenance and TaskGraph events |
| `codex-rs/exec/src/lib.rs` | Preserve stdout/stderr contract, block interactive approvals in headless |
| `codex-rs/exec/tests/` | Add JSONL schema tests for cccx events |

### 6.7 Packaging / Branding

| File/area | Action |
|---|---|
| `package.json`, `codex-cli/package.json` if present | Rename binary/package surface to cccx only after core builds |
| `codex-cli/bin/codex.js` | Add `cccx` launcher or rename wrapper |
| `codex-rs/cli/src/main.rs` | Add cccx command branding and config defaults |
| `README.md`, `NOTICE`, `LICENSE` | Preserve Apache-2.0 notices; add cccx README after import |

---

## 7. Files/Features To Remove Only After MVP Works

Avoid early deletion. First hide/disable, then remove once tests are green.

Candidate removals after MVP:

```text
Cloud task UI paths
OAuth-only setup prompts
Bedrock login UX
Remote control enrollment UX
Marketplace install UX
Codex memory auto-admission UX
Unused app-server desktop-only surfaces if CLI-only cccx stays true
```

Do not remove low-level crates just because MVP hides them. Some are referenced by app-server, tests, or config schemas.

---

## 8. Implementation Milestones

### M0 — Import Cleanly

Deliverable:

- Current docs committed.
- Codex baseline copied as a clean import commit.
- Build/test command for upstream baseline documented.

Validation:

```text
cargo check -p codex-core
cargo check -p codex-tui
cargo test -p codex-core multi_agents
```

Adjust exact package names after import if upstream workspace names differ.

### M1 — 9router Provider Gateway

Deliverable:

- `ProviderGateway` with 9router-only route execution.
- `ModelRouteCatalog`, `ModelRouteResolver`, provenance fields.
- Main CC can run one prompt through 9router.

Validation:

- Unit tests for route resolution: explicit, preset, inherit-main-requested, invalid route fail-closed.
- Mock 9router test for requested/effective route provenance.

### M2 — WorkSpec / Artifact Protocol

Deliverable:

- Typed `WorkSpec`, `Artifact`, `VerificationRecord`.
- Serialization tests.
- Artifact validator rejects missing evidence/provenance.

Validation:

- Golden JSON tests for schema stability.
- Round-trip tests.

### M3 — WorkSpecToChildConfigAdapter

Deliverable:

- Child config starts deny-by-default.
- Allowed tools, forbidden tools, scope, permission and model route come only from WorkSpec plus parent intersection.
- Full-history fork cannot create CX execution worker.

Validation:

- Tests proving parent broad permission does not leak to CX.
- Tests proving CX cannot use MCP outside WorkSpec.
- Tests proving protected paths cannot be edited.

### M4 — Permission Gate / Circuit Breaker

Deliverable:

- Core permission gate before hooks/user prompt.
- Circuit breaker fail-closed.
- PermissionRequestQueue single active prompt.

Validation:

- Static deny beats static allow.
- Classifier failure escalates or denies as designed.
- Cancelled/stale permission digest cannot be approved.

### M5 — Task Graph / Verification Loop

Deliverable:

- Persistent TaskGraph.
- Attempt/Worker/VerificationRecord lifecycle.
- StopHook blocks "done" when tasks unverified.

Validation:

- Compact/clear does not lose TaskGraph.
- CX "done" without VerificationRecord does not mark task verified.
- Protected verification command/path changes block verification.

### M6 — TUI Commands / UX

Deliverable:

- `/agents`, `/agents logs`, `/agents resume`, `/agents cancel`, `/agents model`.
- `/tasks`, `/permissions`, `/remember`.
- cccx status line with mode/harness/tasks/model/permission queue.

Validation:

- Snapshot tests for status line, agent list, permission queue.
- Command parser tests for namespaced commands.

### M7 — Headless Exec

Deliverable:

- JSONL events for agent/task/permission/terminal.
- No prompt/spinner/banner in stdout.
- Approval denied default for headless.

Validation:

- JSONL schema tests.
- stdout/stderr separation tests.

### M8 — Packaging Rename

Deliverable:

- `cccx` binary.
- Default config path and docs updated.
- Codex provenance/Apache notice preserved.

Validation:

- `cccx --version`
- `cccx exec --json "..."` smoke test.
- Existing Codex baseline tests that still apply remain green.

---

## 9. Upstream Drift Checklist

Run after every `git pull` from Codex upstream:

```text
1. git rev-parse HEAD in upstream Codex source and update architecture.md.
2. Check hook event names:
   - SubagentStart
   - SubagentStop
   - PermissionRequest
   - PreCompact/PostCompact
   - UserPromptSubmit
3. Check spawn/resume paths:
   - core/src/tools/handlers/multi_agents*
   - core/src/agent/control/spawn.rs
   - resume_agent_from_rollout
4. Check model override semantics:
   - multi_agents_common.rs
   - models-manager/model-provider-info
5. Check command registry:
   - tui/src/slash_command.rs
   - chatwidget/command_lifecycle.rs
6. Check exec JSONL event shape:
   - exec/src/exec_events.rs
   - exec/src/event_processor_with_jsonl_output.rs
7. Re-run cccx-specific tests before merging upstream.
```

---

## 10. Non-Goals For MVP

```text
- Public SaaS/team/multi-user account system
- Direct Anthropic/OpenAI/Bedrock/Ollama provider login UX
- Public plugin marketplace for cccx
- Autonomous scheduled background daemon
- Multi-worktree concurrent writer
- Direct user chat with subagents
- Importing or copying Claude Code leaked implementation
```

These can be revisited later, but adding them now will blur the core harness boundary.

