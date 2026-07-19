# cccx remediation plan — runtime truth to design completion

Ngày lập lại plan: 2026-07-19.

Plan này thay toàn bộ plan cũ sau khi đối chiếu:

- `docs/architecture.md` — source of truth cho harness, mode, permission,
  WorkSpec/Artifact, Task Graph, memory, MCP, 9router và lifecycle.
- `docs/design.md` — source of truth cho TUI, command, status line, secure input
  và headless contract.
- `docs/progress.md` từ line 174 — những gì implementer đã claim/đã test.
- `docs/audit.md` — input từ ba audit agent; không được coi mọi verdict PASS
  trong file này là bằng chứng cuối nếu runtime call-site cho kết quả ngược lại.
- Code và test tại commit `25ecb5f2e0` — bằng chứng thực thi hiện tại.

Mục tiêu: khi hoàn thành 100% plan này, cccx phải thực sự là CLI
CC-led/CX-delegated, 9router-only, enforce bằng typed runtime state, tool
executor, sandbox, hook và structured artifacts; không chỉ đúng ở prompt,
parser hoặc unit contract rời.

## 0. Kết luận audit đã verify

### Những phần đã có nền tảng tốt và cần giữ

- Các crate `cccx-mode`, `cccx-policy`, `cccx-protocol`,
  `cccx-task-graph`, `cccx-provider-gateway`, `cccx-headless`, `cccx-ux` đã
  tạo được vocabulary/contract ban đầu.
- Provider mặc định 9router và fail-closed check đã được chèn vào Responses,
  Chat Completions, compaction và memory summarization paths.
- MCP defaults có merge logic; user config được ưu tiên.
- Circuit breaker chạy trước Bypass trong policy contract.
- Task Graph có persistence cơ bản; Stop hook đã đọc Task Graph.
- TUI branding/persona, slash-command registration và chat-completions stream
  cơ bản đã hoạt động trong smoke test của owner.
- `.gitignore` không track target/build artifacts đã biết.

### Các verdict trong `audit.md` cần hiệu chỉnh

| Hạng mục | Verdict sau verify | Bằng chứng runtime |
|---|---|---|
| Main CC chạy CC harness | FAIL | `cccx_harness_cc::system_prompt_fragment()` chỉ được gọi trong child adapter. |
| Typed actor/scheduler boundary | FAIL | `ActorIdentity::can_spawn()` không có production caller; spawn guard dò substring trong prompt. |
| Mode là source of truth | FAIL | TUI đổi collaboration mask; shell/edit/MCP vẫn truyền `MainMode::Default`. |
| Bypass attestation | FAIL | `attest_mode()` chỉ đổi enum, không nhận bằng chứng sandbox thật. |
| WorkSpec child projection | FAIL | parent tool/MCP set truyền `Default::default()`; projection chỉ ghi prompt/model và log count. |
| Tool/MCP/scope enforcement | PARTIAL | Có reactive gate cho shell/edit/MCP nhưng catalog model thấy chưa lọc; path chỉ so string, không canonicalize/symlink-resolve. |
| Permission queue | PARTIAL | Approval được enqueue nhưng native Y/N không resolve queue; các command approve/deny/batch còn báo `post-MVP`. |
| Provenance 9router | FAIL | effective route được suy ra từ local provider/model, record bị trace rồi drop; Artifact dùng `unknown`, không nối cùng attempt. |
| Verification/StopGuard | FAIL về end-to-end | `AgentStatus::Completed` được biến thành `exit_code=0` và hash status text, nên task có thể verified mà không chạy verification command. |
| Headless JSONL | PARTIAL | Answer/Error/Terminal đã map; AgentUpdate/TaskUpdate/PermissionRequest/ApprovalDenied chưa map đầy đủ. |
| Project Memory | FAIL | lưu project-local `.cccx/memory`, không dùng `CCCX_HOME/projects/<hash>`, `load_context()` không có production caller. |
| Checkpoint/resume | FAIL | save/load/validate chỉ có unit tests, không có production call-site. |
| Plugin isolation | FAIL | mới có parser/schema test rút gọn; không có loader/runtime integration. |
| UX command completion | PARTIAL | `/agents model`, resume/cancel, `/permissions ...` còn placeholder hoặc global interrupt. |
| Compaction survivor | PARTIAL | chỉ re-inject Task Graph; thiếu active WorkSpec/Artifact, ADR và Project Memory. |

### Lỗi correctness đặc biệt nghiêm trọng

1. `core/src/cccx_runtime.rs::complete_spawn()` truyền `Some(0)` cho mọi
   subagent có `AgentStatus::Completed`, dù không chạy command trong
   `VerificationSpec`. Đây là false verification, phải sửa trước khi tin
   StopGuard.
2. `CccxRuntimeRegistry::register_spawn()` và `complete_spawn()` đều
   load-modify-write `task-graph.json`; không có writer lease/CAS/atomic merge.
   Hai worker hoàn thành gần nhau có thể làm mất update.
3. Hai test trong `core/src/cccx_spawn.rs` vẫn dùng boolean `false`, còn model
   projection test dùng boolean `true`. Đây không phải runtime proof; targeted
   suite tương ứng chưa thể xanh thật.
4. `apply_cccx_work_spec_to_spawn_config()` truyền parent tool/MCP catalog rỗng,
   vì vậy deny-by-default projection thường rỗng rồi bị bỏ đi.

## 1. Quy tắc thực thi bắt buộc

1. Làm phase theo thứ tự. Không đóng phase sau nếu dependency phase trước chưa
   đạt exit criteria.
2. Mỗi feature dùng TDD tại call-site thật:
   - viết test đỏ;
   - lưu output đỏ trong progress;
   - implement tối thiểu;
   - chạy lại xanh;
   - review negative path.
3. Không được dùng các kiểu test sau để claim done:
   - `assert!(true)`/`assert!(false)`;
   - biến boolean tự gán để mô phỏng field runtime;
   - đọc source bằng `include_str!` rồi tìm substring thay cho gọi API;
   - dựng provenance/Artifact fixture không đi qua request/stream/runtime path;
   - parser/renderer unit test để claim TUI command đã chạy thật.
4. Một phase chỉ commit khi targeted tests của phase xanh. Không giữ
   intentional RED test trong HEAD.
5. Re-index trước mỗi phase cross-module và dùng MCP-first:

```text
index_repository(repo_path="D:\Learning_Programing\cccx", name="cccx", mode="moderate")
get_architecture(project="cccx")
search_graph(project="cccx", query="...")
trace_path(project="cccx", function_name="...")
get_code_snippet(project="cccx", qualified_name="...")
```

Chỉ fallback `rg` cho docs/config/log/literal hoặc khi graph thiếu.

6. Không dùng leaked Claude Code source. Chỉ dùng public/observable behavior,
   Codex public source và hai design docs.
7. Không mở provider trực tiếp. Tất cả LLM calls phải qua 9router.
8. Không sửa rộng upstream chỉ để “đẹp code”. Tách cccx-owned runtime seams rõ,
   giữ khả năng rebase Codex.
9. Mỗi phase cập nhật `docs/progress.md` với:
   - requirement → symbol/call-site;
   - test đỏ và test xanh;
   - limitation còn lại;
   - commit hash.

### PowerShell và disk discipline

```powershell
Set-Location D:\Learning_Programing\cccx\codex-rs
$env:PATH = "D:\Program Files\cargo\bin;$env:PATH"
$env:CARGO_TARGET_DIR = "D:\cargo-targets\cccx"
```

- Dùng đúng một shared target dir.
- Targeted test trước; không full build sau từng file.
- Không tạo target trong repo.
- Trước full gate, kiểm tra dung lượng `Get-PSDrive D`.
- Chỉ cleanup đúng absolute path đã verify; không xóa Cargo registry source.

### Windows V8 junction prerequisite

`Cargo.lock` hiện pin `v8 = 149.2.0`. Trước test đụng `codex-core`,
`codex-tui`, `code-mode` hoặc build rộng:

```powershell
$v8Version = "149.2.0"
$v8Source = Get-ChildItem -Directory -Recurse `
  -Path C:\Users\Admin\.cargo\registry\src `
  -Filter "v8-$v8Version" `
  -ErrorAction SilentlyContinue |
  Select-Object -First 1 -ExpandProperty FullName

if (-not $v8Source) {
  throw "Missing Cargo registry source for v8-$v8Version"
}

$gnRoot = "D:\cargo-targets\cccx\debug\gn_root"
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $gnRoot) | Out-Null

if (Test-Path -LiteralPath $gnRoot) {
  $item = Get-Item -LiteralPath $gnRoot -Force
  if ($item.LinkType -in @("Junction", "SymbolicLink")) {
    Remove-Item -LiteralPath $gnRoot -Force
  } else {
    throw "Refusing to replace non-link path: $gnRoot"
  }
}

New-Item -ItemType Junction -Path $gnRoot -Target $v8Source | Out-Null
Get-Item -LiteralPath $gnRoot -Force | Select-Object FullName,LinkType,Target
Test-Path "$gnRoot\BUILD.gn"
Test-Path "$gnRoot\src"
```

Nếu `Cargo.lock` đổi version, đọc lại version và cập nhật `$v8Version`. Không
hardcode junction vào repo và không commit nó.

---

## Phase 0 — Re-baseline test truth và dependency map

### Skills/roles

- Skills: `agent-architecture-audit`, `code-review-context`,
  `ai-regression-testing`, `code-review-testing`.
- ECC roles: `explorer` để trace call path; `tdd_guide` để kiểm tra test proof.

### Mục tiêu

Khóa một baseline trung thực trước khi sửa runtime; chỉ rõ test nào hiện đỏ,
test nào greenwash và call-site nào phải được chạm ở từng phase.

### Công việc

1. Re-index và tạo requirement matrix trong `docs/progress.md`:
   `design requirement → runtime entry → enforcement point → persisted state →
   user surface → test`.
2. Chạy riêng ba test `cccx_spawn` hiện tại để xác nhận:
   - permission projection RED;
   - tool allowlist projection RED;
   - model projection test hiện là greenwash.
3. Inventory production caller cho:
   - `ActorIdentity::can_spawn`;
   - `CccxModeState::change_mode/attest_mode`;
   - `evaluate_action/evaluate_subagent`;
   - `save_checkpoint/load_checkpoint`;
   - `CccxPluginManifest`;
   - `ProjectMemory::load_context`;
   - `GatewayProvenance::record_retry/record_stream_termination`;
   - mọi `HeadlessEvent` variant.
4. Không sửa production code trong phase này. Không commit test đỏ mới.

### Validation/exit criteria

- Baseline ghi đúng expected RED thay vì claim toàn suite xanh.
- Không còn hạng mục “PASS” chỉ dựa trên schema/parser/fixture.
- Có danh sách exact test target cho Phase 1–12.

---

## Phase 1 — Canonical protocol và typed runtime identity

### Skills/roles

- Skills: `api-design`, `agent-harness-construction`, `path-types`,
  `coding-standards`.
- ECC roles: `architect` trước, `tdd_guide` implement, `reviewer` kiểm tra
  compatibility.

### Mục tiêu

Biến WorkSpec/Artifact và actor identity thành contract đầy đủ đúng
`architecture.md`, đủ dữ liệu để các phase sau enforce mà không dò prompt.

### Scope chính

- `cccx-protocol/src/lib.rs`
- `cccx-protocol/src/plugin.rs` chỉ sửa type dùng chung, chưa wire loader
- `core/src/config/mod.rs`
- `core/src/session/session.rs`, `session/turn_context.rs`
- serialization/persistence fixtures liên quan

### RED tests trước

1. WorkSpec JSON snapshot phải dùng `cccx.work_spec.v1` và có:
   `id/taskId/attemptId/createdAt/expiresAt/baseRevision`, typed scope,
   typed enforced/advisory constraints, typed tool grants
   `stableId/serverId/schemaDigest`, rich immutable model route,
   verificationSpec, expectedArtifact, checkpoint/baseCommit.
2. Reject role↔harness mismatch.
3. Reject expired WorkSpec, empty/duplicate stable IDs, schemaDigest mismatch,
   unknown fallback policy và invalid parent attempt chain.
4. Artifact snapshot phải có lifecycle status, changedFiles/testsRun,
   residualRisks/followUp, evidence và full provenance.
5. Config→Session→TurnContext round-trip phải giữ typed
   `ActorIdentity`, `HarnessKind`, attempt/logical session IDs; không thể đổi
   harness giữa lifecycle.

### Implement

1. Tách schema constants cho WorkSpec, Artifact và TaskGraph; không dùng chung
   `cccx.protocol.v1` cho mọi object.
2. Dùng discriminated union hoặc validation tương đương cho role↔harness.
3. Thay string tool set bằng stable tool grant; giữ conversion adapter tại
   boundary Codex tool registry.
4. ModelRoute phải mang selection source, requested route, model/capability
   profile, fallback policy và parent provenance; route id luôn opaque.
5. Artifact provenance phải có requested/effective model/provider nếu có,
   gateway/trace ID, retry/fallback chain, input/output/tool-call-sequence digest.
6. Thêm typed actor/harness runtime context vào Config/Session/TurnContext.
7. Xử lý persisted legacy schema hiện tại bằng migration rõ hoặc fail với
   backup path + actionable message; không silently deserialize sai.

### Validation

```powershell
cargo test -p cccx-protocol
cargo test -p cccx-task-graph
cargo test -p codex-core cccx_protocol --lib
```

### Exit criteria

- Không còn authority/model/tool/scope quyết định bằng string heuristic.
- WorkSpec/Artifact round-trip đúng hai design docs.
- Runtime context biết chính xác actor/harness/attempt của caller.

---

## Phase 2 — Main CC bootstrap và scheduler authority

### Skills/roles

- Skills: `agent-harness-construction`, `autonomous-agent-harness`,
  `gateguard`, `agent-introspection-debugging`.
- ECC roles: `architect`, `implementer`, `security_reviewer`, `reviewer`.

### Mục tiêu

Main agent thực sự chạy CC harness từ startup; chỉ Main CC có scheduler,
reroute và ADR commit authority.

### RED tests trước

1. TUI và exec bootstrap với cccx enabled phải tạo Session actor `MainCc`,
   harness `Cc`, và model-visible instructions chứa Main CC control-plane
   contract.
2. CC/CX child Session phải có actor tương ứng và đúng một harness suốt resume.
3. Gọi spawn từ `CcSubagent` hoặc `CxSubagent` bị deny bằng typed guard dù
   developer instructions bị xóa/sửa.
4. MainCc spawn vẫn đi qua WorkSpec validation; không có raw-context default.
5. ADR write từ child bị deny tại executor/gate, không chỉ trong prompt.

### Implement

1. Tách prompt fragment cho Main CC khỏi prompt CC subagent.
2. Inject Main CC harness trong shared startup path dùng chung TUI/headless,
   trước model call đầu tiên.
3. Thay prompt substring guard trong `multi_agents_common.rs` bằng
   `actor_identity.can_spawn()` và typed authority checks.
4. Chỉ MainCc được tạo/sửa Task scheduling state, approve reroute, commit ADR.
5. Child config bắt buộc carry actor/harness immutable; resume giữ identity cũ.
6. Prompt vẫn dùng để hướng dẫn model nhưng không còn là enforcement source.

### Validation

```powershell
cargo test -p cccx-harness-cc
cargo test -p cccx-harness-cx
cargo test -p codex-core cccx_actor --lib
cargo test -p codex-core cccx_spawn_authority --lib
```

### Exit criteria

- Main startup proof chạm Session thật.
- Xóa harness text không làm child có quyền spawn/ADR.
- Một instance không thể đồng thời là CC và CX.

---

## Phase 3 — Mode single source of truth và Main permission pipeline

### Skills/roles

- Skills: `gateguard`, `agent-harness-construction`, `error-handling`,
  `ai-regression-testing`.
- ECC roles: `tdd_guide`, `security_reviewer`, `reviewer`.

### Mục tiêu

Sáu mode thực sự điều khiển permission từ một state machine; Bypass chỉ mở khi
sandbox backend attestation thật; Auto classifier và cumulative counter hoạt
động đúng.

### RED tests trước

1. TUI `/plan`, `/default`, `/accept-edits`, `/auto`, `/dont-ask`, `/bypass`
   phát typed mode-change op đến core; Session state, persisted state, status
   line và TurnContext cùng giá trị.
2. Shift+Tab chỉ cycle Plan→Default→AcceptEdits→Auto→Plan theo design; không
   tự vào DontAsk/Bypass.
3. Shell/edit/MCP/network cùng một action cho decision khác nhau theo live mode.
4. Restart/compaction giữ mode; CLI > project config > user config > Plan.
5. Bypass bị reject khi chỉ có user confirm nhưng không có verified sandbox
   attestation; circuit breaker vẫn deny sau attestation.
6. Cumulative threshold chỉ tăng sau successful PostToolUse; deny/pending/fail
   không tăng; duplicate hook không double-count.
7. Auto gọi classifier qua 9router, invalid/timeout → require approval.

### Implement

1. Tạo core-owned mode service/state; TUI chỉ gửi user intent, không dựng
   `CccxModeState::default()` tạm.
2. TurnContext snapshot mode từ Session; mọi main tool executor gọi policy với
   mode thật. Xóa các `MainMode::Default` hardcode tại apply_patch, MCP và exec.
3. Implement sandbox attestation type chỉ do backend tạo. Trên backend không
   chứng minh isolation, Bypass disabled với message rõ.
4. Implement projected cumulative counters và commit-once PostToolUse record.
5. Implement classifier client bằng 9router route/profile riêng; classifier chỉ
   giảm quyền trong deterministic ceiling.
6. Bảo toàn priority circuit-breaker > escalate > allowlist và full audit log.

### Validation

```powershell
cargo test -p cccx-mode
cargo test -p cccx-policy
cargo test -p codex-core cccx_mode --lib
cargo test -p codex-core cccx_permission --lib
cargo test -p codex-tui cccx_mode
```

### Exit criteria

- Không production caller nào hardcode Default thay cho active mode.
- Auto classifier, counter và attestation có positive/negative runtime tests.
- Circuit breaker không thể bypass bằng mode/config/env.

---

## Phase 4 — WorkSpec projection, proactive catalog filtering và executor gates

### Skills/roles

- Skills: `agent-harness-construction`, `path-types`, `mcp-server-patterns`,
  `gateguard`, `code-breaking-changes`.
- ECC roles: `architect`, `tdd_guide`, `security_reviewer`, `reviewer`.

### Mục tiêu

Projection phải thực sự hạ quyền child ở ba lớp: model-visible tool catalog,
runtime executor và OS sandbox.

### RED tests trước

1. Spawn bằng Config thật: projected permission/tool/MCP/scope/model route phải
   tồn tại trong child runtime config. Thay ba boolean test hiện tại bằng test
   gọi API thật.
2. Parent capability lấy từ actual ToolRegistry, MCP manager, sandbox roots và
   model catalog; không lấy từ WorkSpec và không truyền empty default.
3. Tool không có trong WorkSpec không xuất hiện trong request gửi model.
4. `tool_search`/dynamic tool discovery không thể re-enable tool bị cấm.
5. MCP stableId/serverId/schemaDigest mismatch bị deny trước call.
6. Multi-file edit có một path ngoài scope bị deny; Windows case/path URI,
   `..`, junction/symlink escape bị deny.
7. Shell/MCP args có nhiều target phải extract và kiểm tra tất cả.
8. OS sandbox writable roots không rộng hơn projection.

### Implement

1. Thêm child capability projection vào Config/TurnContext bằng typed fields.
2. Lấy parent ceiling tại spawn từ actual registries, không từ WorkSpec.
3. Lọc ToolSpec trước model request; áp double-deny cho forbidden tools.
4. Lọc MCP registry per child; verify schema digest tại spawn/call.
5. Reactive executor gate cho read/edit/shell/MCP/network/decision write.
6. Dùng canonical absolute path + symlink/junction resolution và path-boundary
   APIs; không so string prefix thô.
7. Implement deterministic direct-read ngay trước write để chống stale MCP
   graph; mismatch → blocked Artifact, không ghi đè.
8. WorkSpec permission độc lập Main mode; Main Bypass không propagate child.

### Validation

```powershell
cargo test -p cccx-protocol
cargo test -p cccx-policy
cargo test -p codex-core cccx_spawn --lib
cargo test -p codex-core cccx_tool_projection --lib
cargo test -p codex-core cccx_subagent_scope --lib
```

### Exit criteria

- Hai intentional RED tests cũ được thay bằng runtime tests xanh.
- Model không thấy capability ngoài WorkSpec.
- Nếu model đoán tên tool hoặc dùng path escape, executor và sandbox vẫn deny.

---

## Phase 5 — Task Graph, verification evidence, Goal và single-writer state

### Skills/roles

- Skills: `autonomous-loops`, `eval-harness`, `gateguard`,
  `content-hash-cache-pattern`, `error-handling`.
- ECC roles: `architect`, `tdd_guide`, `reviewer`.

### Mục tiêu

Loại bỏ false verification và lost update; Task chỉ verified từ bằng chứng độc
lập đúng VerificationSpec.

### RED tests trước

1. `AgentStatus::Completed` không có verification output phải để task
   unverified và StopGuard block.
2. Verification command thực sự chạy với argv/cwd/timeout đã pin; exit code và
   log digest lấy từ output thật.
3. Artifact summary text “tests pass” không thể tạo `passed=true`.
4. VerificationSpec/protected digest drift block verification.
5. Hai concurrent spawn/complete không làm mất task/artifact/attempt.
6. Crash giữa temp write và rename giữ được file TaskGraph hợp lệ trước đó.
7. `/goal` tạo task-scoped goal condition và VerificationRecord; không chỉ dùng
   upstream thread-goal UI state.
8. StopGuard chỉ cho root final khi mọi task verified; failed evaluator tạo
   correction path, không infinite loop.

### Implement

1. Xóa `Some(0)` giả trong `complete_spawn()`.
2. Tạo verification runner thuộc harness; record command, cwd, timeout,
   exit code, captured-log digest và protected input digest.
3. Artifact Success chỉ chuyển task sang awaiting-verification; chỉ
   `VerificationRecord.passed=true` hợp lệ mới set verifiedAt.
4. Tạo core-owned `CccxStateStore`/writer service:
   - một writer lease;
   - atomic temp-write + fsync/rename thích hợp;
   - generation/CAS hoặc merge dưới lock;
   - recovery từ corrupt partial write.
5. Dùng cùng store cho TaskGraph/mode/active attempt metadata.
6. Wire `/goal` vào cccx task goal; evaluator model tùy chọn vẫn qua 9router và
   không được tự set verifiedAt.
7. Bounded audit correction theo `auditDepth`/changed-region rules.

### Validation

```powershell
cargo test -p cccx-task-graph
cargo test -p codex-core cccx_runtime --lib
cargo test -p codex-core cccx_state_store --lib
cargo test -p codex-core cccx_goal --lib
```

### Exit criteria

- Không còn đường nào coi terminal status/prose là test exit code.
- Concurrency/crash tests chứng minh persistence không lost update.
- StopGuard trở thành safety proof thật, không phải proof dựa trên record giả.

---

## Phase 6 — Permission queue và subagent lifecycle/resume/cancel

### Skills/roles

- Skills: `autonomous-agent-harness`, `continuous-agent-loop`,
  `error-handling`, `gateguard`, `agent-introspection-debugging`.
- ECC roles: `architect`, `tdd_guide`, `security_reviewer`, `reviewer`.

### Mục tiêu

Biến subagent thành worker cancellable/resumable có logical identity ổn định;
mọi permission prompt đi qua một queue thật.

### RED tests trước

1. Hai agent hỏi quyền gần nhau: chỉ một active modal, còn lại queued.
2. Native Y/N approve/deny cập nhật đúng queue request bằng digest; queue không
   giữ request stale sau khi native approval kết thúc.
3. `/permissions approve|deny|inspect|batch-approve` thao tác queue thật;
   high-risk/circuit-breaker không batch approve.
4. Cancel một agent chỉ cancel worker/request của agent đó; không interrupt
   sibling/main stream.
5. Interrupt lưu checkpoint và cho resume; cancel final không auto-resume.
6. Checkpoint được ghi sau successful bounded step/PostToolUse và SubagentStop,
   không chỉ trong unit helper.
7. Resume đúng `logicalSessionId`, load checkpoint, validate age/baseCommit/
   working-tree drift và inject partial state.
8. Completed attempt không resume; work mới tạo attempt mới có parentAttemptId.
9. Chỉ một write worker giữ working-tree lease; read-only CC/CX có thể song song.

### Implement

1. Kết nối CccxPermissionQueue với native approval request/response lifecycle;
   queue là source of truth cho active/queued status.
2. Queue record đủ requestId, agent identity/displayName, task/attempt, action,
   digest, scope, risk và reason.
3. Thêm targeted agent-control ops thay cho global `interrupt()`.
4. Wire checkpoint save/load/validate vào PostToolUse, SubagentStop và
   `resume_agent_from_rollout()`.
5. Persist logicalSessionId/displayName/attempt/worker status trong state store.
6. Implement disconnect→interrupted Artifact từ latest checkpoint.
7. Wire `/agents view/logs/resume/cancel/model` và `/cancel <id>` vào runtime
   registry; model/reroute chỉ tác động next attempt.
8. Thu hồi queued permissions và write lease khi cancel/fail.

### Validation

```powershell
cargo test -p cccx-policy permission_queue
cargo test -p codex-core cccx_permission_queue --lib
cargo test -p codex-core cccx_checkpoint --lib
cargo test -p codex-core cccx_agent_lifecycle --lib
cargo test -p codex-tui cccx_agents
```

### Exit criteria

- Không còn message “requires runtime wiring (post-MVP)”.
- Targeted cancel không ảnh hưởng agent khác.
- Checkpoint/resume có production call-site và drift negative tests.

---

## Phase 7 — Canonical 9router transport, model catalog và end-to-end provenance

### Skills/roles

- Skills: `api-connector-builder`, `connections-optimizer`,
  `cost-aware-llm-pipeline`, `error-handling`, `agent-introspection-debugging`.
- ECC roles: `docs_researcher` để verify public 9router/OpenAI-compatible
  contract, `implementer`, `security_reviewer`, `reviewer`.

### Mục tiêu

Giữ cccx mỏng như OpenCode client: gửi OpenAI Chat Completions-compatible
payload cho 9router, để 9router dịch provider; nhưng quản lý stream, retry,
catalog và provenance đúng design.

### RED tests trước

1. Local mock 9router nhận prompt/context/tools/tool results đúng schema và
   trả SSE tool-call→tool-result→final answer qua nhiều lượt.
2. `[DONE]`/EOF hợp lệ không bị coi là truncated; premature EOF/tool-call
   fragment tạo error/interrupted rõ.
3. Retry 5/15/45 và max 3 bằng paused time; 429 dùng Retry-After.
4. Sau observable side effect, transparent retry bị cấm và phải new
   attempt/resume.
5. Response metadata/header thực tế được nối vào cùng attempt Artifact;
   thiếu metadata → `unknown`, không suy ra requested=effective.
6. Retry count, fallback chain, stream termination và tool-call-sequence digest
   được persist, không chỉ trace log.
7. Mọi LLM surface: main/child/classifier/goal/compaction/review/audit đều
   fail-closed nếu provider không phải 9router.
8. `/model` chỉ hiện models từ cccx config/catalog + current configured model;
   không hardcode model chưa được owner cấu hình.
9. Resolver áp đúng priority user > role preset > agent suggestion > session
   default > inherit main requested và validate capability profile.

### Implement

1. Chọn một canonical live transport trong `codex-core`; loại duplicate legacy
   gateway khỏi runtime authority.
2. Để 9router sở hữu downstream provider translation; cccx không parse provider
   từ route string và không tự dịch theo từng provider.
3. Tạo per-call/attempt gateway context sống đến stream terminal; feed metadata
   vào Artifact/TaskGraph/headless events.
4. Track state trước/sau side effect để quyết định retry an toàn.
5. Record completed/truncated/timeout/error tại stream consumer, không tại lúc
   chỉ mới nhận HTTP stream object.
6. Thay static `LocalConfigCatalog::default()` bằng catalog đọc từ CCCX_HOME/
   config và optional refresh; cache có fetchedAt/source/digest.
7. Xóa misleading env bypass text; provider boundary không có debug escape.
8. Redact secret/token khỏi digest/log/telemetry.

### Validation

```powershell
cargo test -p cccx-provider-gateway
cargo test -p codex-core cccx_9router --lib
cargo test -p codex-core cccx_gateway_provenance --lib
cargo test -p codex-tui cccx_model_popup
```

### Exit criteria

- Tool loop dài không mất context hoặc panic vì tool payload.
- Provenance trong Artifact là cùng request/attempt thật.
- Không có direct-provider LLM path hoặc catalog GPT upstream trong cccx mode.

---

## Phase 8 — Durable context: compaction, Project Memory, ADR và MCP stale gate

### Skills/roles

- Skills: `context-budget`, `architecture-decision-records`,
  `mcp-server-patterns`, `content-hash-cache-pattern`, `knowledge-ops`.
- ECC roles: `architect`, `tdd_guide`, `security_reviewer`, `reviewer`.

### Mục tiêu

Những state được thiết kế là durable phải sống qua restart/compaction và đúng
authority/storage boundary.

### RED tests trước

1. Project hash ổn định theo canonical project root; memory nằm dưới
   `CCCX_HOME/projects/<hash>/memory/MEMORY.md`.
2. Main startup nạp tối đa 200 dòng memory; child chỉ nhận memory được WorkSpec
   cho phép, không raw dump.
3. `/remember` explicit write sống qua restart, không tự biến Artifact thành
   memory.
4. Compaction re-inject AGENTS, TaskGraph, ADR index, Project Memory, active
   WorkSpec/Artifact/checkpoint refs; không chỉ TaskGraph.
5. Chỉ MainCc commit ADR qua PreDecisionWrite gate; child trả ADRProposal.
6. Hai default MCP được merge, health-check và warning đúng; user config thắng.
7. MCP-first discovery không thay thế direct-read trước edit; stale mismatch
   block write.

### Implement

1. Tạo ProjectMemory store từ CCCX_HOME + canonical project hash; migrate hoặc
   cảnh báo file project-local hiện tại.
2. Nạp memory vào Main initial context và compaction replacement context.
3. Tạo structured compaction survivor bundle thay vì một TaskGraph message.
4. Persist active WorkSpec/Artifact/checkpoint refs trong state store.
5. Wire ADR commit authority và proposal ingestion.
6. Hoàn thiện MCP registry routing Main full / child WorkSpec-bounded và startup
   degraded state.
7. Đảm bảo MCP output injection scan/PostToolUse hook chạy đúng.

### Validation

```powershell
cargo test -p codex-core cccx_project_memory --lib
cargo test -p codex-core cccx_compaction --lib
cargo test -p codex-core cccx_adr --lib
cargo test -p codex-core cccx_mcp --lib
```

### Exit criteria

- Restart/compaction integration test giữ đủ durable state.
- Memory/ADR không vượt Main authority.
- MCP discovery và stale-write safety đều có runtime proof.

---

## Phase 9 — Extended plugin loader và harness isolation

### Skills/roles

- Skills: `plugin-creator`, `mcp-server-patterns`, `gateguard`,
  `config-gc`, `api-design`.
- ECC roles: `architect`, `implementer`, `security_reviewer`, `reviewer`.

### Mục tiêu

`cccx-plugin.json` không chỉ parse được mà phải load qua native Codex trust flow
và không tự mở quyền cho child.

### RED tests trước

1. Extended manifest round-trip đủ `rules`, `agents`, `skills`, `mcp_servers`,
   `hooks`, `apps`; unknown fields/version fail closed.
2. Missing harness default CC kèm warning; không default both.
3. Native `plugin.json` load Main sau trust nhưng không tự cấp cho child.
4. Untrusted/changed hook hash không chạy.
5. CX chỉ nhận plugin tool khi WorkSpec grant stableId/server/schemaDigest đúng.
6. Plugin preset model route vẫn qua catalog/capability validation.

### Implement

1. Mở rộng schema đúng architecture §15.
2. Wire discovery/loader vào native plugin pipeline; không tạo marketplace song
   song nếu Codex đã có surface tương đương.
3. Map native vs extended manifest rõ và emit actionable diagnostics.
4. Merge rules/agents/MCP/tools vào Main capability set sau trust.
5. Child visibility luôn đi qua Phase 4 projection.

### Validation

```powershell
cargo test -p cccx-protocol plugin
cargo test -p codex-core cccx_plugin --lib
cargo test -p codex-tui cccx_plugin
```

### Exit criteria

- `CccxPluginManifest` có production loader caller.
- Plugin install/trust/isolation có positive và adversarial tests.

---

## Phase 10 — TUI/command/secure-input completion đúng design

### Skills/roles

- Skills: `test-tui`, `accessibility`, `make-interfaces-feel-better`,
  `browser-qa` chỉ khi cần terminal capture automation.
- ECC roles: `tdd_guide`, `reviewer`.

### Mục tiêu

Mọi surface đã claim phải điều khiển runtime thật, hiển thị state thật và không
phụ thuộc màu.

### RED tests trước

1. Startup banner/persona, art alignment/color và `/model to change` snapshot ở
   narrow/wide terminal.
2. Status line dùng live core state: mode, Main harness, model route,
   TaskGraph progress, context, permission queue, run status.
3. Active agent sublines dùng logicalSessionId/displayName/role/objective/status;
   >8 agent vẫn phân biệt bằng text.
4. `/agents`, `/permissions`, `/tasks`, `/goal`, `/model`, `/remember`, `/mcp`
   và `/cancel` dispatch action thật; không có `post-MVP`/placeholder text.
5. Shift+Tab và risky-mode confirmation đúng Phase 3.
6. Network retry/disconnect/resumable UX đúng design §12.
7. Secure input: no echo, không history/transcript/log/telemetry, clear line,
   persist qua OS keyring; inline secret value bị reject.
8. Theme semantic token/accessibility: state không chỉ thể hiện bằng màu.

### Implement

1. Thay cccx-ux renderer-only data bằng subscription/read model từ core state.
2. Bổ sung task progress và queue data vào status line; không dùng legacy
   collaboration mask làm cccx mode authority.
3. Wire command events đến Phase 5/6/7/8 services.
4. Implement deterministic displayName allocation/persistence.
5. Tận dụng native secret input/keyring nhưng thêm cccx setup/config flow và
   regression guards theo design.
6. Đồng bộ `/help`, autocomplete và command availability.

### Validation

```powershell
cargo test -p cccx-ux
cargo test -p codex-tui cccx
cargo test -p codex-tui secure_input
```

### Exit criteria

- Không còn command giả hành động bằng info message.
- Snapshot và event tests chạm runtime state thật.
- Secret không xuất hiện trong history/transcript/log fixture.

---

## Phase 11 — Headless `cccx.exec_event.v1` completion

### Skills/roles

- Skills: `api-design`, `e2e-testing`, `error-handling`,
  `ai-regression-testing`.
- ECC roles: `tdd_guide`, `reviewer`.

### Mục tiêu

`cccx exec --json` emit đầy đủ machine contract, không chỉ Answer/Error/Terminal.

### RED tests trước

1. Scenario spawn worker emit AgentUpdate với logicalSessionId/displayName/
   role/status/full model-route provenance.
2. Task state transition emit TaskUpdate.
3. Approval request emit PermissionRequest; headless deny mặc định emit
   ApprovalDenied và exit code 2, không block.
4. Cancel emit terminal cancelled/exit 3.
5. Error sau writer init chỉ đi stdout JSONL; stderr chỉ bootstrap fatal trước
   writer.
6. Sequence tăng đơn điệu, session/turn IDs ổn định, terminal đúng một lần và
   luôn cuối.

### Implement

1. Expose typed cccx runtime events từ core thay vì cố suy lại từ thiếu field
   trong upstream `ThreadEvent`.
2. Map mọi variant vào `HeadlessEventEnvelope`.
3. Propagate Phase 7 provenance và Phase 6 permission/lifecycle state.
4. Enforce noninteractive default-deny và stdout/stderr contract.

### Validation

```powershell
cargo test -p cccx-headless
cargo test -p codex-exec cccx_jsonl
```

### Exit criteria

- Không còn test ghi “AgentUpdate deferred”.
- Parser có thể đọc toàn bộ stdout JSONL và terminal là line cuối.

---

## Phase 12 — Cross-crate E2E, coverage và release gate

### Skills/roles

- Skills: `e2e-testing`, `agent-eval`, `production-audit`,
  `delivery-gate`, `code-review-testing`, `code-review-change-size`,
  `update-v8-version` khi Cargo.lock đổi V8.
- ECC roles: `reviewer`, `security_reviewer`, `build_error_resolver` nếu test
  harness/build hỏng.

### Mục tiêu

Chứng minh các feature chạy cùng nhau trong binary thật; loại toàn bộ
greenwashing và đóng kế hoạch bằng evidence tái lập được.

### E2E scenarios bắt buộc

1. Startup với test CCCX_HOME:
   - MainCc/CC harness;
   - 9router provider/model config;
   - MCP defaults health/degraded message;
   - Project Memory load.
2. User đổi từng mode; permission decision và status line đồng bộ; Bypass fail
   nếu không có attestation.
3. Main tạo WorkSpec → typed projection → CX sees filtered tools → bounded edit
   → Artifact real provenance → independent verification → Task verified →
   StopGuard allow.
4. Negative child tries spawn/tool/MCP/path escape/ADR → deny.
5. Hai agents + permission queue + targeted cancel + sibling unaffected.
6. Simulated disconnect trước side effect retries; sau side effect tạo
   interrupted checkpoint; resume đúng logical worker.
7. Concurrent TaskGraph writes không mất update.
8. Compaction/restart giữ mode/TaskGraph/active WorkSpec/Artifact/checkpoint/
   ADR/memory.
9. Headless scenario emit đủ agent/task/permission/approval/terminal events.
10. Plugin trust/isolation scenario.

### Coverage/test quality

- ≥80% line coverage aggregate cho cccx-owned crates.
- Với upstream `codex-core`/`codex-tui` không yêu cầu 80% toàn crate, nhưng mọi
  cccx runtime seam trong matrix phải có positive + negative test.
- Không test source text thay runtime.
- Không ignored cccx correctness test trừ platform-specific test có CI lane
  tương ứng và lý do rõ.

### Validation cuối

```powershell
cargo test -p cccx-protocol
cargo test -p cccx-harness-cc
cargo test -p cccx-harness-cx
cargo test -p cccx-mode
cargo test -p cccx-policy
cargo test -p cccx-provider-gateway
cargo test -p cccx-task-graph
cargo test -p cccx-ux
cargo test -p cccx-headless
cargo test -p codex-core cccx --lib
cargo test -p codex-tui cccx
cargo test -p codex-exec cccx
cargo check -p cccx-cli --bin cccx
```

Chỉ build binary một lần sau targeted tests xanh:

```powershell
cargo build -p cccx-cli --bin cccx
& "D:\cargo-targets\cccx\debug\cccx.exe" --version
& "D:\cargo-targets\cccx\debug\cccx.exe" exec --help
```

Coverage khi đủ dung lượng:

```powershell
$env:CARGO_TARGET_DIR = "D:\cargo-targets\cccx-llvmcov"
cargo llvm-cov `
  -p cccx-protocol -p cccx-harness-cc -p cccx-harness-cx `
  -p cccx-mode -p cccx-policy -p cccx-provider-gateway `
  -p cccx-task-graph -p cccx-ux -p cccx-headless `
  --lcov --output-path D:\cargo-targets\cccx-coverage.lcov
```

### Cleanup/docs

1. Scan và loại mọi `post-MVP`, `deferred`, intentional RED, boolean proof và
   placeholder provenance khỏi feature đã claim.
2. `docs/progress.md` ghi exact commands/results/coverage/commit.
3. Cập nhật `docs/audit.md` hoặc tạo final audit section với verdict mới dựa
   trên E2E evidence.
4. Đồng bộ architecture/design nếu owner chủ động đổi requirement; không tự
   sửa spec để làm code hiện tại “pass”.
5. `git status --short` không chứa target/generated artifacts.

### Exit criteria

- Toàn bộ commands trên xanh.
- Binary smoke xanh với test CCCX_HOME và local mock 9router.
- Coverage đạt gate.
- Không còn known critical/high gap trong requirement matrix.

---

## Final completion gate

Chỉ được báo “cccx hoàn thiện đúng thiết kế” khi tất cả đều có runtime/E2E
evidence:

1. Main CC chạy CC harness ngay từ startup.
2. Mọi instance có typed ActorIdentity và đúng một harness suốt lifecycle.
3. Chỉ MainCc có spawn/scheduler/reroute/ADR authority.
4. Sáu mode dùng một state machine; Shift+Tab chỉ cycle bốn mode an toàn.
5. Bypass cần sandbox attestation thật và không tắt circuit breaker.
6. Main permission pipeline có static priority, cumulative counter và Auto
   classifier qua 9router.
7. WorkSpec/Artifact schema đúng design và validation fail-closed.
8. Parent capability intersection lấy từ runtime thật.
9. Child tool catalog, MCP catalog, executor và OS sandbox đều bị projection.
10. Canonical path/symlink/multi-target/stale-read gates đều hoạt động.
11. Cross-harness chỉ truyền WorkSpec/Artifact, không raw context default.
12. Verification command/evidence là thật; terminal prose không verify task.
13. TaskGraph/state writes atomic, single-writer và không lost update.
14. StopHook block mọi unverified task.
15. Permission queue central, digest-safe và command/TUI điều khiển thật.
16. Checkpoint/resume/cancel/interrupt giữ đúng logicalSessionId semantics.
17. Write-worker lease enforce; read workers có thể parallel.
18. Mọi LLM call qua 9router fail-closed.
19. Retry/stream/provenance/model catalog đúng attempt và response thực tế.
20. Project Memory/ADR/MCP/compaction durability đúng storage/authority.
21. Native/extended plugin trust và harness isolation hoạt động.
22. TUI status/agents/commands/network/secure-input khớp design.
23. Headless emit đủ `cccx.exec_event.v1`, terminal cuối, exit codes đúng.
24. cccx-owned coverage ≥80% và runtime seams có negative tests.
25. Target/generated artifacts không được git track.

Nếu một mục chưa có runtime proof, ghi OPEN/PARTIAL trong progress; không dùng
contract test hoặc comment để claim complete.

## Stop conditions cần hỏi owner

1. Windows backend không thể cấp sandbox isolation attestation đáng tin:
   mặc định disable Bypass; không hạ tiêu chuẩn bằng confirm dialog.
2. 9router không trả effective route/provider metadata:
   ghi `unknown`; không copy requested route. Chỉ đổi nếu owner sửa spec.
3. Codex upstream API không cho proactive tool catalog filtering:
   dừng ở Phase 4, trình bày boundary/refactor options; không chỉ giữ reactive
   gate rồi claim complete.
4. Native plugin trust API không thể mở rộng an toàn:
   giữ cccx extended plugin disabled/fail-closed và hỏi owner, không tự load.
5. Thiếu disk cho full/coverage lane:
   báo exact free space và target sizes trước khi đề xuất cleanup.
