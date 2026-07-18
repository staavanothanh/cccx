# cccx — completion phase plan

Ngày lập kế hoạch: 2026-07-18

Mục tiêu: sau khi thực thi 100% plan này, cccx phải khớp `docs/architecture.md` và `docs/design.md` ở mức runtime, không chỉ parser/prompt/scaffold.

Nguyên tắc bắt buộc cho mọi phase:

- Không weaken test để xanh.
- Không dùng prompt để thay thế policy/state-machine/runtime gate.
- `cccx` binary phải fail-closed về 9router-only.
- Main agent chạy CC harness; mỗi subagent chạy đúng một harness trong suốt lifecycle.
- Main CC là scheduler duy nhất.
- Subagent chỉ nhận WorkSpec, trả Artifact/SchedulingRequest; không nhận raw context dump.
- Tool/MCP sent-to-model phải khớp tool/MCP executable-by-runtime.
- Test targeted trước, sau đó mới chạy broader check vừa đủ để tiết kiệm target disk.

Thiết lập PowerShell khuyến nghị:

```powershell
cd D:\Learning_Programing\cccx\codex-rs
$env:PATH = "D:\Program Files\cargo\bin;$env:PATH"
$env:CARGO_TARGET_DIR = "D:\cargo-targets\cccx"
$env:CCCX_HOME = "D:\Learning_Programing\cccx\.cccx-test-home"
```

## Phase 0 — đóng baseline và trạng thái worktree

Goal: chốt một baseline có thể review/test trước khi sửa tiếp.

Tasks:

1. Kiểm tra `git status --short` và phân loại thay đổi hiện tại:
   - thay đổi đã hoàn thiện từ implementer;
   - thay đổi cần giữ để phase sau;
   - file generated/target không được track.
2. Chạy targeted smoke hiện tại để biết baseline:
   - `cargo test -p codex-api chat_completions --lib`
   - `cargo test -p cccx-protocol -p cccx-policy -p cccx-provider-gateway -p cccx-task-graph -p cccx-mode -p cccx-ux -p cccx-headless -p cccx-harness-cc -p cccx-harness-cx --no-fail-fast`
   - `cargo test -p codex-core provider_gateway_bridge --lib`
   - `cargo test -p codex-core cccx --lib`
   - `cargo check -p cccx-cli --bin cccx`
3. Commit hoặc stash có chủ đích trước khi bắt đầu phase lớn.
4. Update `docs/progress.md` nếu kết quả khác baseline.

Acceptance:

- Có baseline commit hoặc trạng thái dirty được ghi rõ.
- Không có file target/build artifact bị track.
- Các test pass/fail hiện tại được ghi lại trước khi sửa.

Suggested chain nếu dùng ECC:

- `planner` → `reviewer`

## Phase 1 — harden 9router-only model gateway

Goal: mọi LLM call trong `cccx` đi qua 9router boundary duy nhất, có provenance và fail-closed.

Primary files:

- `codex-rs/model-provider-info/src/lib.rs`
- `codex-rs/core/src/client.rs`
- `codex-rs/core/src/provider_gateway_bridge.rs`
- `codex-rs/cccx-provider-gateway/src/lib.rs`
- `codex-rs/core/src/config/mod.rs`
- `codex-rs/tui/src/chatwidget/model_popups.rs`
- config tests liên quan model provider

Tasks:

1. Tách rõ behavior của binary `codex` và binary `cccx`:
   - `codex` giữ upstream provider behavior;
   - `cccx` default/fallback chỉ dùng 9router.
2. Loại mọi fallback GPT/OpenAI khỏi cccx runtime path, trừ test legacy explicit.
3. Đưa provider resolution của Main CC, subagent, classifier, goal evaluator, summarizer về cùng một cổng.
4. Ghi provenance mỗi attempt:
   - requested route;
   - effective route/model từ 9router nếu có;
   - fallback policy;
   - wire API;
   - stream termination status.
5. `/model` chỉ hiển thị route từ cccx config/9router catalog/cache, không hiển thị GPT upstream preset nếu user không cấu hình GPT qua 9router.
6. Nếu 9router catalog chưa có API local ổn định, triển khai local config catalog trước:
   - `default_model`;
   - `available_models`;
   - `fallback_policy_id`.

Tests:

```powershell
cargo test -p codex-core provider_gateway_bridge --lib
cargo test -p codex-core config --lib
cargo test -p codex-tui model --lib
cargo test -p cccx-provider-gateway --lib
cargo check -p cccx-cli --bin cccx
```

Acceptance:

- Chạy `cccx` không còn default sang `gpt-*` nếu `CCCX_HOME` hợp lệ và cccx gate bật.
- Tắt/misconfigure 9router trong cccx cho lỗi rõ, không âm thầm route OpenAI.
- Request có tool/MCP/subagent vẫn đi qua OpenAI-compatible chat-completions payload mà 9router nhận được.
- Stream disconnect/truncated EOF được phân biệt với completed.

Suggested chain:

- `architect` → `tdd_guide` → `security_reviewer` → `reviewer`

## Phase 2 — mode state machine là source of truth

Goal: mọi mode UX/policy/runtime của Main CC dùng `cccx-mode`, không dùng Codex collaboration mode như nguồn sự thật.

Primary files:

- `codex-rs/cccx-mode/src/lib.rs`
- `codex-rs/protocol/src/config_types.rs`
- `codex-rs/tui/src/app.rs`
- `codex-rs/tui/src/chatwidget/slash_dispatch.rs`
- `codex-rs/cccx-policy/src/lib.rs`

Tasks:

1. Sửa Shift+Tab cycle chỉ gồm:
   - Plan;
   - Default;
   - AcceptEdits;
   - Auto.
2. Để `DontAsk` và `Bypass` chỉ vào bằng command explicit:
   - `/dont-ask` kèm confirmation;
   - `/bypass --confirm` kèm sandbox/isolation attestation hợp lệ.
3. Tạo adapter `CccxModeState -> Codex approval/sandbox config` ở boundary thấp nhất cần thiết.
4. Status line, policy evaluator và slash command đều đọc cùng `CccxModeState`.
5. Subagent không có mode; subagent permission chỉ từ WorkSpec projection.
6. Xóa/suppress thông báo “Model changed ... for Plan/Default mode” nếu status line đã thể hiện state.

Tests:

```powershell
cargo test -p cccx-mode --lib
cargo test -p cccx-policy --lib
cargo test -p codex-tui mode --lib
cargo check -p codex-tui
```

Acceptance:

- Shift+Tab không bao giờ cycle vào DontAsk/Bypass.
- Mode risky không thể bật nếu thiếu confirmation/attestation.
- TUI/status/policy không lệch mode.

Suggested chain:

- `tdd_guide` → `reviewer`

## Phase 3 — WorkSpec-to-ChildConfig deny-by-default

Goal: WorkSpec thật sự khóa child agent capability, không chỉ thêm prompt.

Primary files:

- `codex-rs/core/src/cccx_spawn.rs`
- `codex-rs/core/src/tools/handlers/multi_agents_common.rs`
- `codex-rs/core/src/tools/handlers/multi_agents/spawn.rs`
- `codex-rs/core/src/tools/handlers/multi_agents_v2/spawn.rs`
- `codex-rs/core/src/exec_policy.rs`
- `codex-rs/core/src/mcp_tool_call.rs`
- `codex-rs/core/src/apply_patch.rs`
- `codex-rs/core/src/unified_exec/process_manager.rs`
- `codex-rs/cccx-protocol/src/lib.rs`

Tasks:

1. Implement `ChildConfigProjectionApplier`:
   - tool catalog filter;
   - MCP tool filter;
   - filesystem writable/readable scope;
   - permission ceiling;
   - model route;
   - harness prompt fragment;
   - artifact/checkpoint sink ids.
2. Spawn fail-closed nếu thiếu WorkSpec khi cccx gate bật.
3. Runtime fail-closed nếu model gọi tool không nằm trong projection.
4. Tool schema sent-to-model phải bằng đúng projection toolset.
5. Kiểm tra `forbiddenTools` double-deny sau `allowedTools`.
6. Đảm bảo CX không có spawn, ADR write, sequential-thinking mặc định.

Tests:

```powershell
cargo test -p codex-core cccx_spawn --lib
cargo test -p codex-core subagent --lib
cargo test -p codex-core exec_policy --lib
cargo test -p codex-core mcp_tool_call --lib
cargo test -p cccx-protocol --lib
```

Acceptance:

- Một WorkSpec chỉ allow `shell_command` thì child model chỉ thấy `shell_command` và runtime chỉ chạy `shell_command`.
- MCP ngoài WorkSpec bị ẩn khỏi schema và bị deny nếu gọi lách.
- Path ngoài scope bị deny bằng real path boundary.
- Subagent không thể tự mở rộng scope/tool/MCP bằng prompt.

Suggested chain:

- `architect` → `tdd_guide` → `security_reviewer` → `reviewer`

## Phase 4 — CC scheduler và subagent lifecycle

Goal: Main CC là scheduler duy nhất; subagent là worker có lifecycle rõ, cancellable/resumable, giao tiếp qua Artifact.

Primary files:

- `codex-rs/core/src/cccx_runtime.rs`
- `codex-rs/core/src/agent/control.rs`
- `codex-rs/core/src/agent/control/spawn.rs`
- `codex-rs/core/src/tools/handlers/multi_agents*.rs`
- checkpoint/resume modules
- TUI `/agents` dispatch/view modules

Tasks:

1. Thêm/siết `ActorIdentity`:
   - `MainCc`;
   - `CcSubagent`;
   - `CxSubagent`.
2. Spawn handler reject nếu caller không phải Main CC.
3. Khi subagent cần thêm worker/scope, nó chỉ được trả `SchedulingRequest` Artifact.
4. `CccxRuntimeRegistry` quản lý:
   - logicalSessionId;
   - displayName;
   - attemptId;
   - harness;
   - WorkSpec;
   - status;
   - checkpoint.
5. `/agents list/view/logs/resume/cancel/model` gọi state thật, không placeholder.
6. Resume phải validate checkpoint/git head trước khi tiếp tục.
7. Cancel phải hủy đúng agent/request, không ảnh hưởng worker khác.

Tests:

```powershell
cargo test -p codex-core subagent --lib
cargo test -p codex-core cccx_runtime --lib
cargo test -p codex-core checkpoint --lib
cargo test -p codex-tui agents --lib
```

Acceptance:

- Main CC spawn được CC/CX subagent với WorkSpec hợp lệ.
- CX/CC subagent spawn tiếp bị deny và có guidance trả SchedulingRequest.
- `/agents cancel <id>` cancel đúng worker.
- `/agents resume <id>` không resume mù khi codebase drift.

Suggested chain:

- `architect` → `tdd_guide` → `reviewer`

## Phase 5 — permission queue runtime

Goal: permission prompt từ background subagent được gom về một queue có command điều khiển thật.

Primary files:

- `codex-rs/core/src/permission_queue_bridge.rs`
- `codex-rs/cccx-policy/src/lib.rs`
- `codex-rs/tui/src/chatwidget/slash_dispatch.rs`
- TUI permission rendering modules

Tasks:

1. `/permissions` hiển thị active + queued request thật.
2. `/permissions inspect <id>` hiển thị action, diff/checkpoint digest, agent id, risk.
3. `/permissions approve <id>` approve đúng request nếu digest vẫn khớp.
4. `/permissions deny <id>` deny đúng request, không cancel cả agent.
5. `/permissions batch-approve <ids...>` bỏ qua high-risk/destructive request.
6. Background prompt không được stack modal; chỉ queue + status indicator.

Tests:

```powershell
cargo test -p codex-core permission_queue_bridge --lib
cargo test -p cccx-policy --lib
cargo test -p codex-tui permissions --lib
```

Acceptance:

- Nhiều request từ nhiều subagent hiển thị đúng thứ tự.
- Approve/deny một request không ảnh hưởng request không liên quan.
- Batch approve không bypass circuit breaker/high-risk action.

Suggested chain:

- `tdd_guide` → `security_reviewer` → `reviewer`

## Phase 6 — TaskGraph, StopHook, GoalEvaluator

Goal: completion không dựa vào model tự nói “xong”; phải dựa vào TaskGraph + VerificationRecord + StopHook.

Primary files:

- `codex-rs/core/src/cccx_task_graph_state.rs`
- `codex-rs/core/src/goal_evaluator.rs`
- `codex-rs/core/src/hook_runtime.rs`
- `codex-rs/core/src/session/session.rs`
- `codex-rs/core/src/session/mod.rs`
- `codex-rs/tui/src/chatwidget/slash_dispatch.rs`

Tasks:

1. Khi user giao task lớn/WorkSpec spawn, tạo TaskGraph node và persist state.
2. Khi subagent hoàn tất, Artifact cập nhật task status nhưng không set `verifiedAt` trực tiếp.
3. Tạo `VerificationRecord` từ command/log/evidence.
4. `verifiedAt` chỉ là projection từ `VerificationRecord.passed=true`.
5. StopHook luôn chạy trước khi Main CC báo done.
6. Nếu còn task unverified, StopHook chặn final và trả lý do cụ thể.
7. `/tasks` render tree thật theo `blockedBy`, trạng thái, agent, verifiedAt.
8. `/goal` chỉ bật evaluator cho task được user khai báo, không tự lan toàn session.

Tests:

```powershell
cargo test -p codex-core cccx_task_graph --lib
cargo test -p codex-core goal_evaluator --lib
cargo test -p codex-core hook_runtime --lib
cargo test -p codex-tui tasks --lib
```

Acceptance:

- Một task có Artifact prose nhưng thiếu VerificationRecord không được marked verified.
- StopHook chặn “done” khi còn task chưa verified.
- `/tasks` phản ánh đúng graph đang lưu trong session.

Suggested chain:

- `architect` → `tdd_guide` → `reviewer`

## Phase 7 — MCP default registry và MCP WorkSpec gating

Goal: Main CC có global MCP defaults theo thiết kế; subagent chỉ thấy MCP trong WorkSpec.

Primary files:

- `codex-rs/core/src/mcp_auto_init.rs`
- `codex-rs/core/src/config/mod.rs`
- `codex-rs/core/src/mcp_tool_call.rs`
- `codex-rs/tui/src/chatwidget/slash_dispatch.rs`
- test home/config fixtures

Tasks:

1. Startup cccx merge default MCP server entries:
   - `codebase-memory-mcp`;
   - `sequential-thinking`.
2. Nếu binary/package MCP thiếu, report warning actionable nhưng `/mcp` vẫn thể hiện trạng thái configured/missing rõ.
3. Main CC có thể dùng MCP default.
4. CC/CX subagent chỉ thấy MCP nếu WorkSpec cấp stable tool id/schema digest.
5. `/mcp` hiển thị global defaults + project/user additions.

Tests:

```powershell
cargo test -p codex-core mcp_auto_init --lib
cargo test -p codex-core mcp_tool_call --lib
cargo test -p codex-tui mcp --lib
```

Acceptance:

- Test home mới không còn `/mcp` trống nếu defaults available.
- CX không thể gọi `sequential-thinking` nếu WorkSpec không cấp.
- MCP tool name qua 9router vẫn preserve namespace.

Suggested chain:

- `tdd_guide` → `security_reviewer` → `reviewer`

## Phase 8 — UX command completion

Goal: tất cả slash command trong `docs/design.md` có typed parser và runtime behavior hoặc explicit documented non-MVP rejection.

Primary files:

- `codex-rs/cccx-ux/src/lib.rs`
- `codex-rs/tui/src/chatwidget/slash_dispatch.rs`
- `codex-rs/tui/src/app.rs`
- command help/autocomplete modules

Tasks:

1. Audit command list từ `docs/design.md:719-822`.
2. Mỗi command phải rơi vào một trong ba trạng thái:
   - implemented;
   - implemented as safe read-only status;
   - non-MVP explicit rejection có lý do.
3. Không để message kiểu “runtime action wiring is pending”.
4. `/help` và autocomplete không hiển thị capability chưa có hành vi.
5. `/remember` chỉ admission memory khi user explicit.
6. `/review`, `/theme`, `/skills`, `/plugins`, `/status` khớp thiết kế MVP.

Tests:

```powershell
cargo test -p cccx-ux --lib
cargo test -p codex-tui slash --lib
cargo check -p codex-tui
```

Acceptance:

- Không còn placeholder command user-facing.
- Command list trong help/autocomplete khớp behavior thật.

Suggested chain:

- `tdd_guide` → `reviewer`

## Phase 9 — plugin/hook/rules runtime isolation

Goal: cccx extended plugin không phá harness boundary; Codex native plugin vẫn được kế thừa an toàn.

Primary files:

- `codex-rs/cccx-protocol/src/plugin.rs`
- plugin loader/trust flow modules
- MCP/tool registry modules
- hook runtime modules

Tasks:

1. Native `plugin.json` không có `cccxSchemaVersion`:
   - load legacy cho Main CC sau trust;
   - không tự cấp capability cho CX.
2. `cccx-plugin.json` có `cccxSchemaVersion`:
   - strict schema;
   - harness target;
   - rules;
   - agent presets;
   - MCP stable ids/schema digests.
3. Hook trust giữ behavior Codex native: hook mới/thay đổi không chạy nếu chưa trust.
4. Plugin rules không override AGENTS.md, WorkSpec, permission rules.

Tests:

```powershell
cargo test -p cccx-protocol plugin --lib
cargo test -p codex-core plugin --lib
cargo test -p codex-core hook_runtime --lib
```

Acceptance:

- CX chỉ nhận plugin capability qua WorkSpec.
- Unknown cccx schema fields fail closed.
- Untrusted hook không chạy.

Suggested chain:

- `architect` → `security_reviewer` → `tdd_guide` → `reviewer`

## Phase 10 — end-to-end 9router/harness regression suite

Goal: có test chứng minh cccx hoạt động thực sự theo thiết kế, không chỉ unit contract.

Test scenarios với mock 9router:

1. Main CC hello:
   - model route từ 9router config;
   - response stream completed đúng.
2. Main CC tool call:
   - tool schema đúng;
   - tool result serialized đúng;
   - không upstream GPT fallback.
3. MCP call:
   - MCP namespace preserve;
   - WorkSpec gating hoạt động.
4. Spawn CX:
   - WorkSpec required;
   - child toolset bounded;
   - Artifact returned;
   - TaskGraph updated.
5. CX cố spawn tiếp:
   - bị deny;
   - trả SchedulingRequest guidance.
6. Permission queue:
   - background request vào queue;
   - approve/deny/inspect hoạt động.
7. Stream disconnect:
   - EOF trước `[DONE]` là stream error;
   - completed chỉ khi có terminal marker hợp lệ.
8. StopHook:
   - chặn done nếu task unverified;
   - cho done khi VerificationRecord passed.
9. Headless:
   - stdout JSONL-only;
   - terminal event last;
   - event có schemaVersion/sessionId/turnId/sequence.

Candidate files:

- `codex-rs/core/tests/cccx_e2e.rs`
- `codex-rs/tui/tests/cccx_tui.rs`
- `codex-rs/exec/tests/cccx_exec.rs`
- shared mock 9router fixture

Commands:

```powershell
cargo test -p codex-core cccx_e2e --test cccx_e2e
cargo test -p codex-tui cccx --test cccx_tui
cargo test -p codex-exec cccx --test cccx_exec
```

Acceptance:

- Các scenario trên pass bằng mock server deterministic.
- Không cần gọi 9router thật trong CI/local test mặc định.

Suggested chain:

- `tdd_guide` → `e2e-runner` → `reviewer`

## Phase 11 — coverage, packaging, release smoke

Goal: đóng chất lượng cuối trước khi gọi cccx MVP complete.

Tasks:

1. Coverage:
   - aggregate scoped cccx + touched core >= 80%;
   - critical path coverage riêng cho mode/policy/gateway/spawn/permissions/tasks.
2. Dọn warning duplicate bin target `cccx`/`codex` nếu còn.
3. Dọn upstream Codex branding/model wording trong cccx UI path.
4. Update docs:
   - `docs/architecture.md`;
   - `docs/design.md`;
   - `docs/progress.md`;
   - testing evidence report.
5. Build final binary:
   - `cargo build -p cccx-cli --bin cccx`
6. Manual smoke với `CCCX_HOME` test:
   - startup banner;
   - `/model`;
   - `/mcp`;
   - `/mode`;
   - `/agents`;
   - hello request tới 9router local;
   - one tool-call request.

Commands:

```powershell
cargo llvm-cov -p cccx-protocol -p cccx-policy -p cccx-provider-gateway -p cccx-task-graph -p cccx-mode -p cccx-ux -p cccx-headless -p cccx-harness-cc -p cccx-harness-cx --fail-under-lines 80 --summary-only
cargo build -p cccx-cli --bin cccx
& "D:\cargo-targets\cccx\debug\cccx.exe" --version
```

Acceptance:

- Coverage evidence được lưu.
- Binary chạy với 9router config thật.
- Không còn GPT upstream model xuất hiện trong cccx default UX.
- Không còn user-facing placeholder command trong MVP scope.

Suggested chain:

- `build_error_resolver` → `reviewer`

## Final completion gate

Chỉ gọi cccx “đúng thiết kế MVP” khi tất cả điều kiện sau đều đúng:

- `cccx` binary dùng 9router-only cho mọi LLM path.
- Main CC mode state machine hoạt động đúng và risky modes không vào cycle.
- WorkSpec projection enforce tool/MCP/scope/model/permission ở runtime.
- CC là scheduler duy nhất; subagent không spawn tiếp.
- `/agents`, `/permissions`, `/tasks`, `/model`, `/mcp`, `/mode` không còn placeholder trong MVP behavior.
- TaskGraph + VerificationRecord + StopHook quyết định trạng thái done.
- MCP defaults có trong startup registry khi available.
- E2E mock 9router pass cho tool/MCP/subagent/permission/headless/stream cases.
- Coverage gate đạt và có evidence.
- `docs/architecture.md`, `docs/design.md`, `docs/progress.md` khớp behavior thực tế.

## Stop conditions

Dừng và báo cáo nếu gặp một trong các điều kiện:

- Muốn làm xanh test bằng cách nới policy/harness boundary.
- Cần thay đổi thiết kế nguồn sự thật trong `architecture.md`/`design.md`.
- 9router behavior thực tế mâu thuẫn với giả định OpenAI-compatible chat-completions.
- Target disk gần đầy và cần cleanup trước khi tiếp tục build/test.
- Worktree có thay đổi chồng lấn không rõ thuộc phase nào.
