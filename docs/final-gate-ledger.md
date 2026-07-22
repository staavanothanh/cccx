# Final-gate evidence ledger

This ledger is the execution companion to `task.md` final completion gate and
`plan_final_gate.md`. A gate is `PASS` only when the listed runtime/E2E proof
is green and its negative case is covered where applicable. Focused unit tests
remain supporting evidence, not a completion claim.

## Shared final-gate fixture

All final-gate runtime scenarios use an isolated `CCCX_HOME`, a temporary Git
workspace, a 9router-labelled local mock server, and the shared
`D:\\cargo-targets\\cccx` target directory. The fixture must expose both
interactive/TUI events and headless JSONL where a gate crosses either surface.

## Gate matrix

| Gate | Current | Runtime proof required to pass | Negative proof | Primary owner |
|---:|---|---|---|---|
| 1 | PASS | Top-level `cccx exec` binary startup emits `MainCc` + `CC` harness | Child-only harness prompts are not selected | core session/bootstrap |
| 2 | PASS | Spawn then resume preserves typed actor/harness | Child cannot mutate identity/harness | core spawn/runtime |
| 3 | PASS | Main schedules; CC/CX child authority is denied | Child spawn, reroute, ADR write deny | core agent control/ADR |
| 4 | PASS | Config precedence plus six-mode action outcomes are proven through runtime | Shift+Tab excludes DontAsk/Bypass; Bypass without attestation rejects | mode/policy/TUI |
| 5 | PASS (disabled) | Owner-selected Windows/app-server behavior rejects Bypass because no trusted OS-isolation backend exists; circuit breaker remains invariant | Missing/untrusted attestation denies and mode remains unchanged | sandbox/app-server/core |
| 6 | PASS | Multiple workers produce one native active approval prompt | Cancelled/stale request never renders | queue/session/TUI |
| 7 | PASS | Main WorkSpec → child Artifact → acceptance runtime flow | Bad schema/binding rejects | spawn/runtime/task graph |
| 8 | PASS | Child receives live parent capability intersection | Empty intersection is read-only | spawn/executor |
| 9 | PASS | Spawned child tool/MCP/executor/sandbox agree on projection | Unknown/forbidden capability denies | spawn/tools/sandbox |
| 10 | PASS | Shell, MCP and patch writes use canonical observed targets | Junction, multi-target, stale target deny | executor/policy |
| 11 | PASS | Default child input is structured WorkSpec/Artifact only | Raw parent context fork rejects | multi-agent spawn/runtime |
| 12 | PASS | Preserve real verification-command regression | Prose-only completion cannot verify | verification/task graph |
| 13 | PASS | Concurrent process state writes retain both valid updates | Live competing writer rejects | state/task graph |
| 14 | PASS | Preserve actual stop-path regression | Unverified task blocks terminal completion | stop hook/task graph |
| 15 | PASS | TUI commands inspect/approve/deny exact queue IDs | Invalid ID/digest and unsafe batch reject | queue/slash/TUI |
| 16 | PASS | TUI/session interrupt-resume preserves logical ID | Cancelled worker cannot resume | runtime/checkpoint/TUI |
| 17 | PASS | Spawned child dispatch uses project lease; writers serialize while readers overlap | Unknown writer class serializes | tool runtime |
| 18 | PASS | Complete live LLM-path inventory reaches parsed/trusted 9router HTTP/SSE boundary | Spoofed/non-9router route and unsupported WS/realtime fail before transport | client/gateway |
| 19 | PASS | Responses/Chat stream, retry, auth and terminal attempts persist factual provenance | Metadata omission remains `unknown`; post-tool disconnect never replays | gateway/provenance |
| 20 | PASS | Compact/restart restores typed memory, ADR, WorkSpec/Artifact/checkpoint, MCP-health and stale-read refs | Child/alias ADR writes deny or become proposals; forged TaskGraph and raw output cannot grant durable authority | compact/memory/ADR/MCP |
| 21 | PASS | Spawned CX catalog contains only trusted compatible plugin grants | CC-only/native grant does not leak | plugins/spawn |
| 22 | PARTIAL | TUI commands/status/network/secure input use live state | Secret absent from parser/history/log/telemetry | TUI/secure config |
| 23 | PASS | Preserve JSONL terminal-last/exit-code regression | Headless approval defaults deny | headless/exec |
| 24 | PARTIAL | cccx-owned aggregate line coverage is at least 80% | Runtime seams retain negative tests | release validation |
| 25 | PARTIAL | Tracked generated-source allowlist passes | target, logs, local state and secrets are untracked | repository hygiene |

## Evidence record format

When a gate changes state, append to `progress.md`:

1. gate number and requirement;
2. test or smoke command, including target directory and fixture identity;
3. exact result and generated evidence path/digest;
4. the negative case proved;
5. remaining dependency, if the gate remains `PARTIAL`.

## Non-negotiable stop conditions

- Gate 5 is accepted as `PASS (disabled)` for the current personal Windows
  build by owner decision dated 2026-07-21. Re-enabling Bypass requires a real
  OS-isolation attestation backend and new positive runtime proof.
- Missing 9router effective metadata is represented as `unknown`, never copied
  from the requested route.
- A plugin/CX catalog that cannot be proactively filtered fails closed.
- Coverage runs are deferred when disk cannot support the dedicated target;
  record the measured free space and target size first.
