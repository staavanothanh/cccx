# cccx implementation plan — finish-to-design

Tài liệu này là plan thực thi cho implementer sau audit hiện tại. Nguồn sự
thật bắt buộc:

- `docs/architecture.md`
- `docs/design.md`
- `docs/implementer-issues.md`

Mục tiêu cuối: khi hoàn thành 100% plan này, `cccx` phải đúng thiết kế
CC-led/CX-delegated, 9router-only, WorkSpec/Artifact boundary enforced bằng
runtime state/gate, không chỉ bằng prompt hoặc contract test.

## Quy tắc thực thi chung

1. Làm theo thứ tự phase. Không nhảy phase trừ khi phase trước đã đạt exit
   criteria.
2. Mỗi phase bắt đầu bằng test đỏ. Nếu không viết được test đỏ, dừng và ghi rõ
   vì sao contract chưa đủ rõ.
3. Mọi phase có runtime wiring phải có ít nhất một test chạm call-site runtime.
   Không được chỉ green bằng crate contract độc lập.
4. Dùng codebase-memory-mcp trước khi sửa code:
   - `index_repository(repo_path="D:\\Learning_Programing\\cccx", name="cccx", mode="full")`
     nếu graph stale.
   - `search_graph`, `search_code`, `trace_path`, `get_code_snippet` trước
     khi fallback sang `rg`.
5. Không dùng leaked Claude Code source làm input implementation. Chỉ tham khảo từ đó hoặc
   dựa vào public/observable behavior và docs của repo.
6. Không thêm provider path trực tiếp. Mọi LLM call phải đi qua 9router.
7. Tiết kiệm disk:
   - Luôn dùng `CARGO_TARGET_DIR=D:\cargo-targets\cccx`.
   - Không chạy full workspace build sau từng file.
   - Dùng targeted tests trước, full gate chỉ ở cuối milestone.

Lệnh PowerShell chuẩn cho mọi validation:

```powershell
Set-Location D:\Learning_Programing\cccx\codex-rs
$env:PATH = "D:\Program Files\cargo\bin;$env:PATH"
$env:CARGO_TARGET_DIR = "D:\cargo-targets\cccx"
```

Stop condition chung:

- Test đỏ vì thiếu feature đúng như phase mong muốn → tiếp tục implement.
- Test đỏ vì spec mâu thuẫn hoặc API upstream không cho phép enforce đúng
  boundary → dừng, cập nhật `docs/implementer-issues.md`, hỏi owner.
- Build/test sinh thêm target quá lớn hoặc thiếu dung lượng → dừng, báo dung
  lượng hiện tại và đề xuất cleanup rõ target.

---

## Phase 0 — Baseline, inventory, và regression harness

### Skills implementer cần load

- `agent-harness-construction`: xác định đúng runtime call-site và boundary harness cần test.
- `ai-regression-testing`: thiết kế regression matrix chứng minh feature chạy thật, không chỉ green bằng contract rời.
- `code-review-testing`: siết test để phát hiện greenwash, fixture giả, và coverage hổng.

Mục tiêu: khóa lại trạng thái hiện tại, tạo baseline test map để các phase sau
không greenwash bằng test rời rạc.

### RED tests cần thêm trước

- `codex-rs/core/tests/cccx_runtime_contract_test.rs`
  - Test helper dựng `Config` + `WorkSpec` + `ParentCapabilitySet` thật.
  - Assert projection từ WorkSpec phải hiện diện trong child runtime config,
    không chỉ trong log.
- `codex-rs/tui/tests/cccx_tui_contract_test.rs` hoặc test module tương ứng
  trong `codex-rs/tui/src/...`
  - Assert mode command cập nhật cả TUI visible mode và core session mode event.
- `codex-rs/exec/tests/cccx_exec_contract_test.rs`
  - Assert `cccx exec --json` chỉ emit envelope `cccx.exec_event.v1`.

Nếu repo không có test harness phù hợp cho một đường, tạo test nhỏ nhất trong
crate chứa code đó. Không tạo crate test giả chỉ validate pure struct.

### Công việc

1. Chạy inventory bằng MCP + `git status --short`.
2. Liệt kê các call-site cccx hiện có:
   - mode: `cccx-mode`, TUI slash/keyboard, `Session.cccx_mode`,
     `exec_policy`.
   - spawn: `multi_agents_common`, `cccx_spawn`, `cccx_runtime`,
     `cccx-protocol`.
   - gateway: `client.rs`, `model-provider-info`, `provider_gateway_bridge`.
   - task graph/goal/stop: `cccx_task_graph_state`, `goal_evaluator`,
     `hook_runtime`.
   - TUI: `/agents`, `/permissions`, `/tasks`, `/goal`, `/model`.
   - headless: `exec`, `cccx-headless`.
3. Ghi test matrix vào comment đầu file test hoặc `docs/progress.md`.

### Validation

```powershell
cargo test -p cccx-protocol
cargo test -p cccx-mode
cargo test -p cccx-policy
cargo test -p cccx-provider-gateway
cargo test -p cccx-task-graph
cargo test -p cccx-headless
```

### Exit criteria

- Baseline test mới tồn tại và đỏ đúng vì runtime chưa wire, không đỏ do compile
  lỗi tùy tiện.
- Có danh sách call-site runtime cần sửa cho từng phase.

---

## Phase 1 — 9router-only ProviderGateway là hard boundary

### Skills implementer cần load

- `agent-harness-construction`: giữ 9router là boundary bắt buộc cho mọi LLM call.
- `api-design`: chuẩn hóa contract provider gateway, route id, metadata, và lỗi fail-closed.
- `gateguard`: ép điều tra trust boundary trước khi sửa các call-site có thể bypass provider gateway.

Mục tiêu: không còn direct OpenAI/upstream provider route nào có thể chạy trong
`cccx`, kể cả main, subagent, classifier, compaction, summarize memory,
goal-evaluator, headless.

### RED tests cần thêm trước

- `codex-rs/core/src/client.rs` tests:
  - main request với provider khác 9router bị fail-closed trước network.
  - `compact_conversation_history()` với provider khác 9router bị fail-closed.
  - `summarize_memories()` với provider khác 9router bị fail-closed.
- `codex-rs/core/tests/cccx_gateway_runtime_test.rs`:
  - spawn subagent pin route không thuộc 9router → reject WorkSpec/attempt.
  - classifier/goal evaluator route không thuộc 9router → reject config.
- `codex-rs/features/src/cccx_mvp.rs` tests:
  - env escape hatch `CCCX_MVP_ALLOW_UPSTREAM_SURFACES` không được disable
    provider boundary. Nếu vẫn cần debug flag, nó chỉ được mở UI legacy, không
    mở LLM route.

### Công việc

1. Tạo một boundary function duy nhất, ví dụ `ensure_cccx_gateway_provider(...)`
   hoặc mở rộng `ProviderGatewayBridge`, dùng chung mọi call-site LLM.
2. Fail-closed theo provider id/wire_api/base_url:
   - chỉ provider logical `9router-local` hoặc config marked `cccx_gateway=true`
     được phép.
   - model id là opaque 9router route id; không infer provider từ model string.
3. Loại bỏ hoặc thu hẹp `CCCX_MVP_ALLOW_UPSTREAM_SURFACES`:
   - được phép expose legacy UI debug.
   - không được bypass gateway fail-closed.
4. Provider provenance:
   - attempt phải ghi `requestedRouteId`, `effectiveRouteId` nếu response/header
     có dữ liệu.
   - nếu 9router không trả effective route, ghi `unknown`, không giả mạo.
   - input/output/tool digest phải là digest thực từ payload/stream, không phải
     chuỗi placeholder.
5. Retry transport:
   - retry chỉ trước observable side effect.
   - sau tool call/permission/artifact thì không replay stream như cùng attempt.

### Validation

```powershell
cargo test -p codex-core cccx_gateway --lib
cargo test -p codex-core provider_gateway --lib
cargo test -p cccx-provider-gateway
cargo test -p cccx-headless
```

### Exit criteria

- Không có LLM call-site cccx nào bypass boundary.
- Search literal `CCCX_MVP_ALLOW_UPSTREAM_SURFACES` cho thấy flag không thể mở
  direct provider route.
- Tests có ít nhất một case đi qua runtime client/core path.

---

## Phase 2 — Mode single source of truth và permission state machine

### Skills implementer cần load

- `agent-harness-construction`: map mode UX sang core harness state thay vì prompt/UI-only.
- `autonomous-agent-harness`: thiết kế mode transition như state machine có enforcement.
- `gateguard`: ép điều tra permission semantics trước khi sửa bypass/dont-ask hoặc quyền file/shell.

Mục tiêu: 6 mode là security state machine thật của Main CC; TUI, session,
tool executor và permission gate đọc cùng một state.

### RED tests cần thêm trước

- `codex-rs/core/src/exec_policy.rs`:
  - Plan deny edit/shell mutate/MCP write.
  - Default yêu cầu approval cho write/shell non-read.
  - AcceptEdits allow edit trong threshold nhưng shell vẫn ask.
  - Auto gọi classifier gate hoặc deterministic classifier stub.
  - DontAsk chỉ allow explicit allowlist, deny silent phần còn lại.
  - Bypass vẫn chặn circuit breaker.
- `codex-rs/core/src/session/tests.rs`:
  - mode slash/setting cập nhật `Session.cccx_mode`.
  - restored session giữ mode đúng.
- `codex-rs/tui/src/chatwidget/tests/...`:
  - `/plan`, `/default`, `/accept-edits`, `/auto`, `/dont-ask`,
    `/bypass --confirm` update status line và gửi event core.
  - Shift+Tab chỉ cycle Plan → Default → AcceptEdits → Auto.
  - DontAsk/Bypass không nằm trong quick cycle.

### Công việc

1. Chọn source of truth: `CccxModeState` trong core session.
2. TUI mode command không chỉ đổi collaboration mask; phải gửi/ghi event làm
   core session mode đổi.
3. `exec_policy` không hardcode `ModeKind::Default`; đọc mode từ session/context.
4. `collaboration_modes` chỉ là UX preset/compat layer, không là security state.
5. `/bypass --confirm`:
   - cần sandbox capability attestation thật.
   - nếu chưa detect được isolation backend, mode switch phải bị deny rõ lý do.
   - env var/user confirm không đủ.
6. Counter/threshold:
   - projected counter chỉ tính trước quyết định.
   - commit counter đúng một lần ở PostToolUse khi action thành công.

### Validation

```powershell
cargo test -p cccx-mode
cargo test -p codex-core cccx_mode --lib
cargo test -p codex-core exec_policy --lib
cargo test -p codex-tui cccx_mode
cargo check -p codex-tui
```

### Exit criteria

- Một mode switch trong TUI làm thay đổi state mà tool executor đọc được.
- Plan mode không thể edit/shell mutate kể cả model yêu cầu tool trực tiếp.
- Bypass không bật nếu không có isolation attestation.

---

## Phase 3 — WorkSpec-to-ChildConfig deny-by-default projection

### Skills implementer cần load

- `agent-harness-construction`: enforce WorkSpec/ChildConfig là structured boundary thật.
- `path-types`: xử lý canonical path, symlink, workspace root, và path allow/deny chính xác.
- `gateguard`: ép điều tra đầy đủ tool/MCP/network/filesystem surface trước khi mở quyền.

Mục tiêu: WorkSpec là boundary thực cho mọi subagent. Child chỉ thấy và chỉ chạy
được tool/MCP/model/scope mà Main CC cấp.

### RED tests cần thêm trước

- `cccx-protocol`:
  - `project_child_config()` intersection với parent capability thật.
  - forbiddenTools thắng allowedTools.
  - model route explicit ngoài parent allowlist bị reject.
  - permission ceiling parent thấp hơn WorkSpec thì child bị hạ quyền.
- `codex-core` runtime:
  - `apply_cccx_harness_to_config()` thật sự mutate `Config`:
    tool allowlist, MCP allowlist, writable roots, model, permission policy.
  - child tool catalog gửi cho model không có tool ngoài WorkSpec.
  - child executor từ chối function tool không nằm trong allowedTools.
- `cccx-policy`:
  - edit nhiều path: tất cả path phải nằm trong scope, không dùng `any()`.
  - path canonical/symlink ngoài scope bị deny.
  - shell/MCP args extract multi-path và check tất cả path.

### Công việc

1. `ParentCapabilitySet` phải lấy từ runtime parent config/capability, không lấy
   lại từ chính WorkSpec.
2. `project_child_config()` trả `ChildRuntimeProjection` có đủ:
   - `allowed_tools`
   - `allowed_mcp_tools`
   - `writable_scope`
   - `protected_paths`
   - `permission_policy`
   - `model_route`
   - `fallback_policy_id`
3. `apply_cccx_harness_to_config()` phải apply projection vào `Config`.
4. Wire tool catalog builder:
   - tool sent-to-model = intersection runtime executable tool ∩ WorkSpec.
   - MCP tools phải filter bằng stable id/server/schema digest.
5. Wire executor gate cho mọi function tool:
   - shell
   - edit/patch
   - MCP
   - spawn/agent tools
   - plugin/hook/custom tools
6. Scope normalization:
   - canonical absolute paths.
   - symlink resolution.
   - reject unresolved path khi tool có side effect.
   - multi-target uses `all()` semantics.

### Validation

```powershell
cargo test -p cccx-protocol
cargo test -p cccx-policy
cargo test -p codex-core cccx_spawn --lib
cargo test -p codex-core subagent --lib
cargo test -p codex-core tool_catalog --lib
```

### Exit criteria

- Subagent không thể thấy tool ngoài WorkSpec trong prompt/tool schema.
- Nếu model cố gọi tool ngoài WorkSpec, executor deny trước side effect.
- WorkSpec explicit model route được áp vào child config thật.

---

## Phase 4 — Harness identity, scheduler boundary, và subagent lifecycle

### Skills implementer cần load

- `agent-harness-construction`: đảm bảo một agent instance chỉ chạy một harness suốt lifecycle.
- `autonomous-agent-harness`: thiết kế scheduler/main-agent và bounded worker lifecycle.
- `agent-architecture-audit`: rà module boundary CC-led/CX-delegated trước khi wire sâu.

Mục tiêu: Main CC là scheduler duy nhất; CC-harness subagent không có quyền
Main CC; CX không spawn/ADR/scope-expand. Không nhận diện actor bằng prompt text.

### RED tests cần thêm trước

- `cccx-protocol`/`codex-core`:
  - `ActorIdentity::MainCc.can_spawn() == true`.
  - `ActorIdentity::CcSubagent.can_spawn() == false`.
  - `ActorIdentity::CxSubagent.can_spawn() == false`.
  - Subagent attempt gọi spawn tool → denied with `scheduler_only`.
- `codex-core` spawn path:
  - main startup inject CC harness fragment.
  - CC subagent prompt không tuyên bố là final scheduler.
  - CX prompt không chứa planner/ADR/scheduler quyền.
- TUI/headless:
  - `/agents view` attach read-only, không có input path vào subagent.

### Công việc

1. Thêm explicit `ActorIdentity` vào session/turn/subagent config.
2. Bỏ substring prompt check để nhận diện child.
3. Main startup:
   - inject CC harness vào Main CC.
   - set actor identity = `MainCc`.
4. Subagent spawn:
   - WorkSpec.harness = CC → actor identity = `CcSubagent`.
   - WorkSpec.harness = CX → actor identity = `CxSubagent`.
5. Tool catalog:
   - spawn/reroute/task scheduling tools chỉ available cho `MainCc`.
   - CC subagent có MCP phân tích nếu WorkSpec cấp, nhưng không có scheduler/ADR.
   - CX không có sequential-thinking mặc định; chỉ có nếu WorkSpec explicit.
6. Artifact nếu subagent cần thêm scope/worker:
   - trả `SchedulingRequest` trong Artifact.
   - Main CC quyết định tạo WorkSpec mới.

### Validation

```powershell
cargo test -p cccx-harness-cc
cargo test -p cccx-harness-cx
cargo test -p cccx-protocol actor
cargo test -p codex-core cccx_actor --lib
cargo test -p codex-core subagent --lib
cargo test -p codex-tui agents
```

### Exit criteria

- Không còn actor detection bằng prompt substring.
- Chỉ Main CC có thể spawn/cancel/reroute/schedule.
- CC subagent và CX subagent đều là bounded workers.

---

## Phase 5 — Artifact, VerificationRecord, provenance thật

### Skills implementer cần load

- `agent-harness-construction`: chuẩn hóa Artifact/VerificationRecord làm output boundary.
- `ai-regression-testing`: tạo test chứng minh verification dựa trên evidence thực thi.
- `architecture-decision-records`: ghi provenance/decision khi contract verification thay đổi.

Mục tiêu: task không thể được đánh dấu success bằng self-report/prose. Verified
chỉ đến từ `VerificationRecord.passed=true` có evidence digest thật.

### RED tests cần thêm trước

- `cccx-task-graph`:
  - `completed` Artifact không tự set `verifiedAt`.
  - chỉ `record_verification(passed=true)` set verified projection.
- `codex-core`:
  - native subagent completed mà không Artifact chuẩn → status `unverified`.
  - exit code 0 giả lập không đủ nếu thiếu evidence/log digest.
  - provenance digest phải thay đổi khi payload thay đổi.
  - requested/effective route không được giả mạo giống nhau nếu gateway không có
    evidence.

### Công việc

1. Chuẩn hóa Artifact schema runtime:
   - `status`: success/failed/blocked/interrupted/cancelled.
   - `changes`: file list/diff digest.
   - `verification`: command, exit code, log digest, findings.
   - `provenance`: gateway attempts thật.
   - `schedulingRequests`: optional.
2. `complete_spawn()` không tự dựng success Artifact khi chỉ thấy native
   completed. Nó phải:
   - parse Artifact từ child output nếu có.
   - nếu không có Artifact hợp lệ → `completed_unverified`.
3. Digest thật:
   - input payload digest.
   - output digest.
   - tool call sequence digest.
   - log digest cho verification command.
4. Artifact validation:
   - mismatch WorkSpec id/attempt id → reject.
   - Artifact ngoài scope → reject.
   - verification command không khớp spec hoặc thiếu digest → unverified.

### Validation

```powershell
cargo test -p cccx-task-graph
cargo test -p cccx-protocol artifact
cargo test -p codex-core cccx_runtime --lib
cargo test -p codex-core goal_evaluator --lib
```

### Exit criteria

- “Agent nói xong” không đủ để Task verified.
- Mọi verified task có VerificationRecord và evidence digest thật.
- Provenance không còn placeholder `sha256:cccx-runtime-*`.

---

## Phase 6 — TaskGraph, GoalEvaluator, StopHook runtime loop

### Skills implementer cần load

- `autonomous-loops`: thiết kế loop task/goal/stop có điều kiện dừng rõ và chống chạy vô hạn.
- `agent-harness-construction`: wire TaskGraph/GoalEvaluator/StopHook vào runtime harness.
- `agent-introspection-debugging`: thêm state/event giúp debug khi loop không tiến triển.

Mục tiêu: Main CC tạo/quản lý TaskGraph bền vững; StopHook chặn “done” nếu còn
task chưa verified; `/goal` lưu condition và chạy verification đúng flow.

### RED tests cần thêm trước

- `codex-core`:
  - user task mới tạo TaskGraph node/task root.
  - Plan mode tạo WorkSpec draft + TaskGraph nhưng không thực thi.
  - StopHook block final answer khi còn unverified task.
  - StopHook allow final answer khi mọi task verified.
  - `/goal "..."` lưu goal condition vào task/session state.
  - GoalEvaluator chạy verification command hoặc nhận command evidence thật,
    không tự set verified từ prose.
- `codex-tui`:
  - `/tasks` render graph thật từ session, không placeholder.
  - `/goal` status line hiển thị active goal.

### Công việc

1. TaskGraph storage:
   - project-local file theo architecture.
   - load startup, save mutation.
   - không phụ thuộc context window.
2. Main CC runtime:
   - tạo root task khi nhận user task.
   - cho main direct work tạo node `cc:self`.
   - spawn subagent gắn attempt vào task node.
3. Goal:
   - `/goal` parse và attach condition vào current/root task.
   - CLI/headless `--goal` nếu đã có parser thì map cùng code path.
4. Verification:
   - nếu task có command verification, chạy command hoặc validate log digest
     từ CX.
   - `GoalEvaluator` chỉ đánh giá evidence; không tự tạo evidence.
5. StopHook:
   - deterministic read TaskGraph projection.
   - block final answer khi còn task unverified.
   - emit actionable reason.

### Validation

```powershell
cargo test -p cccx-task-graph
cargo test -p codex-core cccx_task_graph --lib
cargo test -p codex-core goal_evaluator --lib
cargo test -p codex-core stop_hook --lib
cargo test -p codex-tui tasks
```

### Exit criteria

- `/tasks` hiển thị state thật.
- `/goal` có runtime state thật.
- StopHook không thể bị pass nếu task graph còn unverified.

---

## Phase 7 — Permission queue và approval UX thật

### Skills implementer cần load

- `agent-harness-construction`: nối permission queue với runtime tool execution.
- `gateguard`: ép điều tra request flow trước khi sửa centralized approval/escalation.
- `accessibility`: đảm bảo prompt/queue không phụ thuộc màu và có stable identifiers.

Mục tiêu: background subagent permission request không chồng modal; `/permissions`
điều khiển queue thật bằng requestId/digest.

### RED tests cần thêm trước

- `codex-core`:
  - nhiều subagent gửi permission request → một active, còn lại queued.
  - approve với digest mismatch → reject.
  - deny request không cancel subagent; child nhận denial.
  - batch approve không nhận high-risk/circuit breaker/mixed action class.
- `codex-tui`:
  - `/permissions` list queue thật.
  - `/permissions approve pr-x`, `deny`, `inspect`, `batch-approve` mutate queue
    hoặc trả lỗi rõ.
  - Y/N chỉ áp dụng active request.

### Công việc

1. Tạo permission queue state trong core/session hoặc reuse bridge hiện có nhưng
   phải có runtime call-site.
2. Request shape:
   - `requestId`
   - `agent logicalSessionId/displayName`
   - action type
   - scope/risk
   - digest of action payload
   - status active/queued/approved/denied/stale.
3. TUI:
   - `/permissions` không mở upstream approval-mode popup nữa trong cccx mode.
   - command grammar theo design.
4. Tool executor:
   - `requireApproval()` tạo queue request.
   - approval resume đúng pending action nếu digest khớp.
5. Headless:
   - không block chờ approval.
   - emit `approval-denied` JSONL event và deny mặc định.

### Validation

```powershell
cargo test -p codex-core permission_queue --lib
cargo test -p codex-tui permissions
cargo test -p cccx-headless approval
```

### Exit criteria

- Chỉ một prompt active tại một thời điểm.
- `/permissions approve|deny|inspect|batch-approve` điều khiển queue thật.
- High-risk không thể batch approve.

---

## Phase 8 — Subagent control, checkpoint, resume/cancel/reroute UX

### Skills implementer cần load

- `autonomous-agent-harness`: thiết kế lifecycle cancel/resume/reroute cho worker bounded.
- `agent-introspection-debugging`: expose task/session/checkpoint state đủ để debug.
- `test-tui`: kiểm tra slash command, keyboard path, và TUI state transition.

Mục tiêu: `/agents` là control surface thật cho bounded workers; checkpoint và
resume không chỉ là API/test rời rạc.

### RED tests cần thêm trước

- `codex-core`:
  - SubagentStop ghi checkpoint với logicalSessionId/attemptId/baseCommit.
  - network interrupted → Artifact interrupted + checkpoint persisted.
  - resume loads checkpoint, validates git HEAD drift, injects checkpoint context.
  - cancel marks attempt cancelled and prevents further tool calls.
  - reroute creates new WorkSpec/attempt; không mutate subagent đang chạy.
- `codex-tui`:
  - `/agents list` shows active/resumable/completed.
  - `/agents view <id>` read-only attach.
  - `/agents logs <id> --tail N` sanitized logs.
  - `/agents resume <id> [--revalidate]` calls resume path.
  - `/agents cancel <id>` cancels exact agent, không interrupt toàn session.
  - `/agents model <preset> <route>` writes next-spawn config, validates route.

### Công việc

1. `logicalSessionId` và `attemptId` là identity ổn định.
2. Checkpoint storage:
   - base commit/hash.
   - last completed step.
   - partial diff digest.
   - WorkSpec id.
3. Resume path:
   - không fire SubagentStart như new spawn.
   - load checkpoint.
   - validate drift.
   - inject checkpoint context.
4. Cancel:
   - cancel exact subagent.
   - preserve checkpoint/diff for inspection.
5. Reroute:
   - stop/cancel old or wait stop.
   - spawn new attempt with new WorkSpec/role/model.

### Validation

```powershell
cargo test -p codex-core checkpoint --lib
cargo test -p codex-core subagent_resume --lib
cargo test -p codex-tui agents
cargo check -p codex-tui
```

### Exit criteria

- `/agents cancel <id>` không cancel toàn bộ stream nếu có id cụ thể.
- Resume dùng checkpoint thật và drift check.
- User không thể chat trực tiếp với subagent trong attach view.

---

## Phase 9 — MCP defaults, Project Memory, ADR, plugin isolation

### Skills implementer cần load

- `mcp-server-patterns`: cấu hình default MCP, tool allowlist, transport, và failure behavior.
- `architecture-decision-records`: nối ADR lifecycle với decision/provenance của cccx.
- `gateguard`: ép điều tra plugin/MCP/config boundary trước khi sửa hoặc mở scope.

Mục tiêu: startup/init và extension surfaces đúng thiết kế: MCP default luôn
đăng ký, Project Memory lưu đúng nơi, ADR chỉ Main CC, plugin không tự mở quyền
cho CX.

### RED tests cần thêm trước

- MCP:
  - startup config thiếu MCP vẫn auto-merge `codebase-memory-mcp` và
    `sequential-thinking`.
  - nếu binary MCP thiếu, warning rõ và `/mcp` hiển thị failed/default status.
  - subagent chỉ thấy MCP trong WorkSpec.
- Project Memory:
  - `/remember` lưu dưới `CCCX_HOME/projects/<project-hash>/memory` hoặc home
    cccx tương đương, không ghi vào project `.cccx/memory` nếu trái design.
  - startup load memory summary vào Main CC context.
  - CX không tự ghi memory.
- ADR:
  - chỉ Main CC có PreDecisionWriteHook/ADR commit.
  - CC/CX subagent ADR write bị deny.
- Plugin:
  - `cccx-plugin.json` strict validates schema v1.
  - native `plugin.json` load legacy vào Main CC sau trust.
  - CX không thấy plugin tool trừ khi WorkSpec cấp stable id.

### Công việc

1. MCP auto-init:
   - config merge ở startup core, không chỉ warning helper.
   - registry state hiển thị được trong `/mcp`.
2. Project Memory:
   - xác định CCCX_HOME path resolver.
   - migrate hoặc fallback nếu file cũ tồn tại.
   - explicit-first `/remember`.
3. ADR:
   - implement PreDecisionWriteHook call-site.
   - route qua codebase-memory-mcp ADR API nếu có; nếu không, fail rõ.
4. Plugin loader:
   - strict cccx schema.
   - harness isolation.
   - hook trust preserved.

### Validation

```powershell
cargo test -p codex-core mcp_auto_init --lib
cargo test -p codex-core project_memory --lib
cargo test -p codex-core adr --lib
cargo test -p codex-core plugin --lib
cargo test -p codex-tui mcp
cargo test -p codex-tui remember
```

### Exit criteria

- `/mcp` không nói “No MCP servers configured” khi default init khả dụng.
- `/remember` survives restart and loads into Main CC context.
- Plugin install không tự mở tool/MCP cho CX.

---

## Phase 10 — Model catalog và subagent model route policy

### Skills implementer cần load

- `agent-harness-construction`: giữ main/subagent model routing theo CC/CX harness contract.
- `cost-aware-llm-pipeline`: thiết kế fallback/default model policy có kiểm soát chi phí và năng lực.
- `api-design`: chuẩn hóa model catalog là 9router route id opaque, không provider-specific.

Mục tiêu: `/model` và subagent model selection dùng 9router catalog/config,
không hardcode GPT/upstream hoặc danh sách tạm.

### RED tests cần thêm trước

- `codex-tui`:
  - `/model` under cccx reads configured 9router catalog/current config.
  - no GPT upstream fallback appears unless user configured route through 9router.
  - offline catalog shows current route + config routes, không fetch direct.
- `codex-core`:
  - WorkSpec model route resolve order:
    explicit spawn override
    → role/preset config
    → agent-suggested route if allowlisted
    → session default_subagent_model
    → inherit-main-requested.
  - inherit-main-requested vs inherit-main-effective được phân biệt.
  - fallback policy pinned per attempt.

### Công việc

1. Define 9router catalog provider:
   - local config list.
   - optional 9router catalog endpoint nếu có.
   - offline cache.
2. `/model` consumes catalog provider, not hardcoded presets.
3. Subagent route resolver consumes same source + WorkSpec policy.
4. Logs/headless events show requested/effective route and selection source.

### Validation

```powershell
cargo test -p codex-core model_route --lib
cargo test -p codex-tui model
cargo test -p cccx-headless model_route
```

### Exit criteria

- Chỉ model route 9router configured/catalog xuất hiện.
- Subagent attempt ghi route selection source rõ.
- Gateway không tự chọn model thay scheduler.

---

## Phase 11 — Headless exec contract và JSONL event stream

### Skills implementer cần load

- `api-design`: khóa schema `cccx.exec_event.v1` và compatibility contract.
- `agent-harness-construction`: bảo đảm headless đi cùng runtime harness/gateway/mode path.
- `ai-regression-testing`: test JSONL event stream end-to-end, ordering, và failure cases.

Mục tiêu: `cccx exec` đáp ứng `design.md` mục 17; stdout/stderr sạch, JSONL
versioned, terminal event cuối cùng bắt buộc.

### RED tests cần thêm trước

- `codex-rs/exec/tests/cccx_exec_json_test.rs`:
  - `cccx exec --json` emits only JSON objects on stdout.
  - every event has `schemaVersion=sessionId=turnId=seq=type=payload`.
  - final event type `terminal`.
  - errors after writer ready are JSONL, không stderr.
  - approval denied maps exit code 2.
  - cancelled maps exit code 3.
  - agent-update includes model route requested/effective/fallback chain.
- `cccx-headless`:
  - pure envelope serializer/deserializer snapshot.
  - reject non-json stdout contamination.

### Công việc

1. Wire `cccx-headless` event envelope into real `exec` JSON path.
2. Map upstream `ThreadEvent` → `CccxExecEvent`.
3. Split stdout/stderr:
   - plain: stdout final answer only.
   - json: stdout JSONL only.
   - bootstrap fatal before writer ready may go stderr.
4. Add terminal event on all exits.

### Validation

```powershell
cargo test -p cccx-headless
cargo test -p codex-exec cccx
cargo check -p cccx-cli --bin cccx
```

### Exit criteria

- Machine parser can consume entire stdout as JSONL.
- No banner/persona/progress leaks into headless stdout.

---

## Phase 12 — UX completion pass

### Skills implementer cần load

- `test-tui`: kiểm tra command palette, Shift+Tab, status line, model list, permissions UI.
- `accessibility`: bảo đảm stable id, không phụ thuộc màu, text/keyboard path rõ.
- `make-interfaces-feel-better`: hoàn thiện terminal UX theo `docs/design.md` mà không thêm scope ngoài thiết kế.

Mục tiêu: các command và UI cccx-specific trong `design.md` hoạt động hoặc được
ẩn rõ nếu deferred; không để placeholder nói như đã implement.

### RED tests cần thêm trước

- `codex-tui`:
  - startup banner đúng cccx persona art, mode, model, folder.
  - status line fields: mode, harness, model, task progress, ctx, permission.
  - `/help` lists cccx command set and hides removed/upstream commands.
  - `/theme`, `/pets`/persona config at least no-op safe or implemented as
    documented MVP.
  - network error renders retry UX text.
  - secure input path does not write secret to history/transcript.

### Công việc

1. Replace remaining placeholder command outputs:
   - `/agents model`
   - `/permissions ...`
   - `/tasks`
   - `/goal`
   - `/remember`
2. Removed/hidden upstream commands:
   - keep code if needed but `is_visible=false`.
   - blocked surfaces return cccx-specific message.
3. Secure input:
   - API key/config token flow must avoid transcript/history.
4. Persona:
   - art matches `design.md`.
   - disabled in headless.

### Validation

```powershell
cargo test -p codex-tui cccx
cargo test -p codex-tui help
cargo test -p codex-tui secure_input
cargo check -p codex-tui
```

### Exit criteria

- No visible placeholder text for cccx-specific commands.
- Help/status/banner match design.
- Sensitive input is not persisted in test transcript/history.

---

## Phase 13 — End-to-end proving suite

### Skills implementer cần load

- `e2e-testing`: dựng luồng end-to-end chứng minh main/subagent/MCP/permission/9router hoạt động cùng nhau.
- `ai-regression-testing`: chống regression trên các contract cccx trọng yếu.
- `agent-eval`: thiết kế scenario đánh giá agent behavior thay vì chỉ unit test struct.

Mục tiêu: chứng minh các feature chính chạy cùng nhau trong runtime, không chỉ
unit test từng crate.

### E2E scenarios cần có

1. Startup:
   - CCCX_HOME test dir mới.
   - 9router config present.
   - MCP defaults merged.
   - Main CC starts in configured/default mode.
2. Mode gate:
   - Plan denies edit.
   - AcceptEdits allows small edit, asks shell.
   - Bypass denied without sandbox attestation.
3. Spawn CX:
   - Main CC creates WorkSpec.
   - CX receives limited tool catalog.
   - CX attempts disallowed tool → denied.
   - CX produces Artifact.
   - Task verified only after VerificationRecord.
4. Permission queue:
   - two background permission requests.
   - active + queued UI state.
   - approve digest match, reject mismatch.
5. 9router stream:
   - mocked local 9router endpoint receives Chat Completions-compatible request.
   - cccx consumes DONE without premature ResponseAborted regression.
   - tool call stream maps correctly.
6. Headless:
   - `cccx exec --json` scenario with agent-update, permission-denied, terminal.

### Validation

```powershell
cargo test -p codex-core cccx_e2e --test '*'
cargo test -p codex-tui cccx_e2e
cargo test -p codex-exec cccx_e2e
```

Nếu wildcard không phù hợp với crate hiện tại, implementer phải ghi lệnh chính
xác vào `docs/progress.md` sau khi tạo test file.

### Exit criteria

- E2E fail nếu bỏ bất kỳ runtime wire quan trọng nào: mode, gateway, WorkSpec,
  permission, TaskGraph, headless.
- Không phụ thuộc network thật; 9router được mock/stub local.

---

## Phase 14 — Coverage, cleanup, release smoke

### Skills implementer cần load

- `code-review-testing`: rà coverage, test gaps, và flaky tests trước khi đóng plan.
- `production-audit`: kiểm tra release smoke, packaging, target cleanup, và cấu hình mặc định.
- `code-review-change-size`: rà diff cuối để phát hiện scope creep hoặc thay đổi ngoài plan.

Mục tiêu: đóng loop chất lượng; đảm bảo không regress compile, không giữ
artifact rác, không track target.

### Công việc

1. Coverage:
   - target ≥80% aggregate cho cccx-owned crates.
   - Với `codex-core`/`codex-tui`, không cần 80% toàn upstream; cần coverage
     meaningful cho cccx runtime seams.
2. Cleanup:
   - `target/`, `D:\cargo-targets\cccx*` không track git.
   - generated temp files không nằm trong repo.
3. Smoke binary:
   - build `cccx-cli --bin cccx`.
   - `cccx --version`.
   - `cccx exec --help`.
   - TUI startup with test CCCX_HOME.
4. Docs sync:
   - update `docs/progress.md`.
   - nếu behavior khác design, sửa design/architecture hoặc issue, không để
     mismatch.

### Validation

```powershell
cargo test -p cccx-protocol
cargo test -p cccx-mode
cargo test -p cccx-policy
cargo test -p cccx-provider-gateway
cargo test -p cccx-task-graph
cargo test -p cccx-ux
cargo test -p cccx-headless
cargo test -p codex-core cccx --lib
cargo test -p codex-tui cccx
cargo check -p cccx-cli --bin cccx
```

Optional coverage gate khi đủ dung lượng:

```powershell
$env:CARGO_TARGET_DIR = "D:\cargo-targets\cccx-llvmcov"
cargo llvm-cov --workspace --lcov --output-path D:\cargo-targets\cccx-coverage.lcov
```

### Exit criteria

- All cccx targeted tests pass.
- Binary builds.
- `git status --short` không có target/generated artifact.
- `docs/progress.md` phản ánh đúng feature done/deferred.

---

## Final completion gate

Chỉ được báo “cccx khớp thiết kế MVP” khi tất cả điều kiện sau đúng:

1. Main CC chạy CC harness từ startup.
2. Mọi subagent có actor identity rõ và đúng một harness suốt lifecycle.
3. Chỉ Main CC có scheduler/spawn/reroute/ADR authority.
4. Mode state machine là source of truth cho Main CC permission.
5. Bypass cần sandbox attestation thật và vẫn giữ circuit breaker.
6. WorkSpec projection áp vào child config, tool catalog và executor gate.
7. Subagent không thể dùng tool/MCP/model/scope ngoài WorkSpec.
8. Mọi LLM call đi qua 9router fail-closed.
9. Provider provenance/digest là evidence thật, không placeholder.
10. Task verified chỉ từ `VerificationRecord.passed=true`.
11. StopHook block final answer khi TaskGraph còn unverified.
12. `/agents`, `/permissions`, `/tasks`, `/goal`, `/model`, `/remember`,
    `/mcp` không còn placeholder cho behavior đã claim.
13. `cccx exec --json` emit đúng `cccx.exec_event.v1`.
14. MCP defaults init đúng hoặc warning rõ khi binary thiếu.
15. Project Memory/ADR/plugin isolation đúng storage và authority boundary.
16. Target/build artifacts không được git track.

Nếu một mục chưa làm, ghi vào `docs/progress.md` là deferred/known gap. Không
claim MVP complete.

## Spec decisions cần owner xác nhận nếu gặp trong lúc implement

1. Bypass attestation backend trên Windows: nếu không detect được VM/container
   isolation đáng tin, mặc định nên disable Bypass. Nếu owner muốn dùng Bypass
   trên máy dev thường, phải sửa architecture vì hiện tại design yêu cầu
   attestation thật.
2. 9router effective route metadata: nếu 9router không trả header/body cho
   effective route, provenance phải ghi `unknown`. Không được giả mạo bằng
   requested route. Nếu owner muốn treat requested=effective khi thiếu metadata,
   cần ghi rõ trong architecture.
3. Project Memory storage: architecture/design ưu tiên CCCX_HOME/project-hash.
   Nếu owner muốn project-local `.cccx/memory` vì tiện backup git, cần sửa
   design và privacy assumption.
4. GoalEvaluator model riêng: design cho phép deferred prose evaluator, nhưng
   verification command/evidence là bắt buộc trước. Nếu muốn model-only
   evaluator, cần sửa hard constraint “verified only via VerificationRecord”.
