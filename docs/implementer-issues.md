# cccx — vấn đề còn tồn đọng cho implementer

Ngày audit: 2026-07-18

Nguồn sự thật:

- `docs/architecture.md`
- `docs/design.md`

Phạm vi file này: chỉ ghi các vấn đề hiện còn lệch thiết kế hoặc còn thiếu runtime wiring. Các lỗi RED cũ về chat-completions tool-call grouping, MCP tool namespace split, EOF-before-DONE, checkpoint git head, path boundary, plugin manifest, headless envelope đã có commit sửa trước đó và không được lặp lại như blocker hiện tại.

## Kết luận kỹ thuật

cccx đã vượt qua giai đoạn scaffold ban đầu: có crate riêng cho protocol/policy/provider-gateway/task-graph/mode/UX/headless/harness, có nhiều test hợp đồng đã xanh, và binary đã đi được request 9router ở case đơn giản.

Nhưng cccx chưa thể coi là hoàn thiện theo thiết kế. Các phần quan trọng nhất vẫn còn ở trạng thái “có khung, chưa thành runtime boundary”:

- 9router chưa thật sự là cổng LLM duy nhất có fail-closed contract.
- Mode hiển thị/command còn dựa nhiều vào collaboration mode kế thừa từ Codex, chưa thống nhất quanh `cccx-mode`.
- WorkSpec được yêu cầu khi spawn nhưng projection chưa thật sự khóa child config theo deny-by-default.
- CC/CX scheduler boundary chưa được enforce đủ: subagent phải là worker bounded, không tự mở rộng scheduler/tool/MCP scope.
- `/agents`, `/permissions`, `/tasks` còn nhiều nhánh parser/placeholder chưa thành hành vi thật.
- MCP default mới ở mức warning/check, chưa bảo đảm startup registry đúng như thiết kế.

Nếu implement đúng toàn bộ `docs/task.md`, cccx mới đạt trạng thái “đúng kỳ vọng thiết kế”: CC-led, CX-delegated, 9router-only, WorkSpec-bound, UI command đủ chức năng, có test chứng minh.

## Baseline quan sát hiện tại

Worktree tại thời điểm audit đang dirty, có nhiều thay đổi runtime chưa commit. Đây không tự động là lỗi, nhưng là rủi ro audit/review vì trạng thái chạy test có thể không khớp commit gần nhất.

Ví dụ nhóm file đang thay đổi:

- `codex-rs/core/src/agent/control.rs`
- `codex-rs/core/src/tools/handlers/multi_agents*.rs`
- `codex-rs/core/src/cccx_runtime.rs`
- `codex-rs/core/src/permission_queue_bridge.rs`
- `codex-rs/tui/src/chatwidget/slash_dispatch.rs`
- `codex-rs/cccx-policy/src/lib.rs`

Validation đã từng chạy và đạt ở mức targeted:

- `cargo test -p codex-api chat_completions --lib`
- Các crate cccx nhỏ: `cccx-protocol`, `cccx-policy`, `cccx-provider-gateway`, `cccx-task-graph`, `cccx-mode`, `cccx-ux`, `cccx-headless`, `cccx-harness-cc`, `cccx-harness-cx`
- Nhóm targeted trong `codex-core`: `provider_gateway_bridge`, `mcp_auto_init`, `permission_queue_bridge`, `goal_evaluator`, `checkpoint`, `cccx`, `subagent`
- `cargo check -p codex-tui`
- `cargo check -p cccx-cli --bin cccx`
- Coverage aggregate từng pass `83.38%`, nhưng không thay thế e2e runtime validation; một số crate cccx nhỏ vẫn dưới 80% nếu xét riêng lẻ.

## Critical

### C1 — WorkSpec-to-ChildConfig Adapter mới append prompt, chưa enforce deny-by-default

Thiết kế yêu cầu adapter áp WorkSpec vào child config theo các lớp:

- harness;
- allowed/forbidden tools;
- MCP allowlist;
- filesystem scope;
- permission policy;
- model route;
- context projection;
- artifact sink.

Evidence thiết kế:

- `docs/architecture.md:885` — `WorkSpec-to-ChildConfig Adapter — deny-by-default`
- `docs/architecture.md:899` — chỉ enable tool được liệt kê tường minh
- `docs/architecture.md:903` — áp WorkSpec scope vào filesystem access
- `docs/architecture.md:909` — CX không có sequential-thinking, ADR write, spawn

Evidence code:

- `codex-rs/core/src/cccx_spawn.rs:21` gọi `project_child_config`.
- Nhưng hàm hiện chỉ append `cccx_harness_cc`/`cccx_harness_cx` vào `developer_instructions` và log `allowed_tools`.
- Không thấy projection được áp trực tiếp vào config runtime để filter tool catalog, MCP registry, sandbox roots, permission ceiling hoặc model route.

Impact:

- Đây là lỗi harness boundary, không phải lỗi prompt.
- Model có thể được “dặn” hành xử như CX nhưng runtime vẫn để tool/scope rộng hơn WorkSpec.
- Tool count gửi lên LLM có thể không phản ánh quyền thực thi thật.
- Subagent có thể lệch khỏi bounded-worker contract khi task dài hoặc có MCP/tool phức tạp.

Fix direction:

- Biến `project_child_config()` thành input bắt buộc của một `ChildConfigProjectionApplier`.
- Áp projection vào `Config`/session/tool router thật, không chỉ prompt.
- Test phải chứng minh tool/MCP/scope/model bị chặn trước khi prompt/model can thiệp.

### C2 — Mode system chưa có một nguồn sự thật duy nhất

Thiết kế yêu cầu 6 mode của cccx là state machine của Main CC, chỉ user chuyển mode; subagent không có mode riêng.

Evidence thiết kế:

- `docs/architecture.md:174` — subagent không dùng mode state machine, bị bound bởi WorkSpec.
- `docs/design.md:334` — Shift+Tab chỉ cycle Plan / Default / AcceptEdits / Auto.
- `docs/design.md:335` — DontAsk, Bypass không nằm trong cycle nhanh.
- `docs/design.md:343` — `/dont-ask` cần xác nhận.
- `docs/design.md:344` — `/bypass --confirm` cần flag và attestation.

Evidence code:

- `codex-rs/protocol/src/config_types.rs:659` định nghĩa `ModeKind::is_cycle_visible`.
- `codex-rs/protocol/src/config_types.rs:666-667` vẫn đưa `DontAsk` và `Bypass` vào cycle.
- `codex-rs/tui/src/chatwidget/slash_dispatch.rs:334-369` các slash `/plan`, `/default`, `/accept-edits`, `/auto`, `/dont-ask`, `/bypass` vẫn đi qua `apply_collaboration_mode_slash_command`.

Impact:

- UX có thể hiển thị đủ command nhưng state source vẫn là Codex collaboration mode, không phải `cccx-mode::CccxModeState`.
- Policy gate có nguy cơ đọc một mode, TUI/status line hiển thị mode khác.
- DontAsk/Bypass là mode rủi ro, không được phép cycle bằng phím tắt.

Fix direction:

- Chọn `cccx-mode` làm source of truth.
- Tạo adapter rõ ràng từ cccx mode sang legacy Codex approval/sandbox mode chỉ ở boundary cần thiết.
- Shift+Tab phải chỉ cycle safe modes.
- `/dont-ask` và `/bypass` phải đi qua API xác nhận/attestation, không gọi chung path với mode thường.

### C3 — 9router chưa là runtime boundary duy nhất

Thiết kế yêu cầu mọi LLM call của cccx đi qua 9router:

- Main CC;
- CC/CX subagent;
- classifier;
- goal evaluator;
- summarizer;
- tool repair/retry nếu có.

Evidence thiết kế:

- `docs/architecture.md:33` — `provider-gateway`: mọi LLM call đi qua 9router.
- `docs/architecture.md:1193` — `core/provider-gateway` là đường LLM duy nhất.
- `docs/architecture.md:1194` — mọi model call của Main CC, subagent, classifier, goal evaluator, summarizer đi qua 9router.

Evidence code:

- `codex-rs/model-provider-info/src/lib.rs:407` có `create_9router_provider`.
- `codex-rs/model-provider-info/src/lib.rs:421` đặt 9router dùng `WireApi::ChatCompletions`.
- `codex-rs/core/src/provider_gateway_bridge.rs` hiện là bridge/provenance nhỏ.
- `codex-rs/core/src/client.rs` vẫn là nơi stream model call chính theo provider wire API kế thừa.
- `codex-rs/core/src/config/config_tests.rs` còn test `cccx_default_model_provider_uses_openai_when_gate_is_disabled`.

Impact:

- cccx đã cấu hình được 9router cho happy path, nhưng chưa fail-closed ở mọi LLM call path.
- Nếu gate/config/env lệch, OpenAI upstream model vẫn có thể xuất hiện hoặc được dùng ở path kế thừa.
- `/model` hiện chưa có catalog 9router thực, chủ yếu dựa vào current model/config fallback.
- Fallback/provenance của 9router chưa đủ để làm căn cứ WorkSpec model route và subagent model policy.

Fix direction:

- Định nghĩa rõ `CccxModelGateway`/`ProviderGatewayRuntime` là boundary duy nhất cho binary `cccx`.
- Direct OpenAI/default provider chỉ được tồn tại cho `codex` legacy hoặc test explicitly named.
- Mọi path tạo `ModelClient` trong cccx phải đi qua 9router provider resolution.
- `/model` và `/agents model` đọc từ 9router config/catalog, không từ upstream GPT preset list.

### C4 — CC/CX scheduler boundary chưa được enforce đủ

Thiết kế yêu cầu Main CC là scheduler duy nhất. CC-harness subagent và CX-harness subagent đều không được spawn tiếp; nếu cần thêm scope/worker thì trả SchedulingRequest/Artifact về Main CC.

Evidence thiết kế:

- `docs/architecture.md:1346` — CX không tự spawn CC hoặc mở rộng scope/tool/MCP.
- `docs/architecture.md:1372` — CC-harness subagent cũng không tự spawn subagent khác.
- `docs/architecture.md:576` — cross-harness communication phải qua WorkSpec/Artifact.

Evidence code:

- `codex-rs/core/src/tools/handlers/multi_agents_common.rs:249` có `apply_cccx_work_spec_to_spawn_config`.
- `codex-rs/core/src/tools/handlers/multi_agents_common.rs:261` yêu cầu `work_spec` khi cccx gate bật.
- `codex-rs/core/src/tools/handlers/multi_agents_common.rs:270` gọi `apply_cccx_harness_to_config`.
- Chưa thấy guard rõ ràng kiểu “nếu caller hiện là subagent / CX / non-main CC thì reject spawn and return SchedulingRequest guidance”.

Impact:

- Với model có tool `spawn_agent`, child agent có thể tự mở rộng agent tree nếu tool catalog không bị WorkSpec filter chặt.
- Điều này phá triết lý CC-led, CX-delegated.

Fix direction:

- Runtime phải có `ActorIdentity { main|subagent, harness, logicalSessionId, workSpecId }`.
- Spawn handler phải fail-closed nếu caller không phải Main CC.
- Khi subagent cần thêm worker/scope, chỉ được emit Artifact/SchedulingRequest.

## High

### H1 — `/agents` parser có nhưng runtime actions còn placeholder

Evidence thiết kế:

- `docs/design.md:515-541` định nghĩa `/agents view/logs/resume/cancel/model`.
- `docs/design.md:593-601` yêu cầu list subagent có id, harness, model, state, commands.

Evidence code:

- `codex-rs/tui/src/chatwidget/slash_dispatch.rs:747` ghi `/agents ... parsed; runtime action wiring is pending`.

Impact:

- User nhìn thấy command nhưng không điều khiển được lifecycle thật.
- Cancellable/resumable worker, read-only attach và model route preset chưa đạt thiết kế.

Fix direction:

- Wire `/agents list/view/logs/resume/cancel/model` vào `CccxRuntimeRegistry`, checkpoint/resume, log store và model route config.

### H2 — `/permissions` approve/deny/inspect/batch-approve còn placeholder

Evidence thiết kế:

- `docs/design.md:630-659` định nghĩa permission queue tập trung và command approve/deny/inspect/batch-approve.

Evidence code:

- `codex-rs/core/src/permission_queue_bridge.rs` có queue bridge và enqueue state.
- `codex-rs/tui/src/chatwidget/slash_dispatch.rs:758` ghi approval queue action wiring pending.

Impact:

- Background subagent permission queue chưa có UX điều khiển thật.
- Nếu permission prompt nhiều hoặc đến từ agent nền, user vẫn chưa có queue command đúng thiết kế.

Fix direction:

- `/permissions` phải đọc queue thật.
- `approve/deny/inspect/batch-approve` phải gọi action thật, có digest/checkpoint validation.
- Batch approve không được include high-risk request.

### H3 — `/tasks`, Task Graph và StopHook chưa chứng minh được lifecycle end-to-end

Evidence thiết kế:

- `docs/design.md:282-285` yêu cầu `/tasks` hiển thị tree theo `blockedBy`.
- `docs/architecture.md:1257-1258` yêu cầu StopHook luôn bật, kiểm tra mọi Task đã verified.
- `docs/architecture.md:1319-1322` yêu cầu `verifiedAt` chỉ là projection từ `VerificationRecord.passed=true`.

Evidence code:

- `codex-rs/tui/src/chatwidget/slash_dispatch.rs:816` ghi `/tasks ... TUI tree renderer is pending session wiring`.
- Có `cccx_task_graph_state` và `goal_evaluator`, nhưng cần e2e chứng minh graph được tạo/cập nhật/chặn done trong session thật.

Impact:

- Main CC có thể báo xong dựa vào conversation/model answer thay vì TaskGraph verified state.
- `/tasks` không phải nguồn quan sát đáng tin cậy nếu chưa wired.

Fix direction:

- Session runtime phải tạo/cập nhật TaskGraph khi có WorkSpec/subagent/Artifact.
- StopHook phải đọc TaskGraph thật và chặn final done khi còn task unverified.
- `/tasks` phải render từ state thật.

### H4 — MCP default registry mới warning-only

Evidence thiết kế:

- `docs/architecture.md:89` — startup luôn init 2 global defaults: `codebase-memory-mcp`, `sequential-thinking`.
- `docs/architecture.md:1106-1119` — MCP registry gồm global defaults và registry mở rộng.

Evidence code:

- `codex-rs/core/src/mcp_auto_init.rs:5` khai báo `CCCX_GLOBAL_MCP_SERVERS`.
- `codex-rs/core/src/mcp_auto_init.rs:10` có `check_cccx_mcp_defaults`.
- `codex-rs/core/src/mcp_auto_init.rs:21` có `warn_missing_cccx_mcps`.
- `codex-rs/core/src/config/mod.rs:3824-3826` đang warning khi thiếu MCP, chưa auto-init thật.

Impact:

- `/mcp` có thể vẫn báo “No MCP servers configured” nếu user/test home chưa cấu hình.
- Main CC không có MCP-first discovery mặc định như thiết kế.

Fix direction:

- Startup cccx phải merge default MCP config vào runtime registry khi chưa có user override.
- Nếu executable/package MCP thiếu thì warning rõ dependency thiếu, nhưng registry contract vẫn có trạng thái explicit.
- WorkSpec projection vẫn phải giới hạn MCP cho subagent.

### H5 — Runtime tool list chưa được kiểm soát như một contract

Hiện tượng user từng thấy: request gửi 12–14 tools. Số lượng này có thể đúng hoặc sai tùy mode/harness/WorkSpec, nhưng hiện chưa có contract test chứng minh:

- Main CC thấy full toolset dự kiến.
- CC subagent thấy only WorkSpec tools.
- CX subagent không thấy spawn/ADR/sequential-thinking trừ khi được cấp.
- MCP tool schema được preserve qua 9router/OpenAI-compatible chat-completions.

Impact:

- Tool format có thể không crash, nhưng quyền model thấy và quyền runtime execute có thể lệch nhau.
- Đây là dạng wrapper regression nghiêm trọng cho agent harness.

Fix direction:

- Thêm `ToolCatalogSnapshot` test cho Main CC / CC-subagent / CX-subagent theo từng mode.
- Fail nếu tool sent-to-model không khớp tool executable-by-runtime.

## Medium

### M1 — Subagent model route policy chưa khép kín

Evidence thiết kế:

- `docs/architecture.md:532-568` định nghĩa subagent model route policy: CC resolve, WorkSpec pin, 9router ghi effective route/fallback.

Current gap:

- Có discussion/design về inherit-main-requested, inherit-main-effective, preset route.
- Chưa đủ bằng chứng runtime cho `/agents model <preset> <route>`, validation với 9router catalog, và provenance per attempt.

Fix direction:

- Route resolution phải xảy ra trước spawn và ghi vào WorkSpec.
- 9router effective route/fallback phải ghi vào Artifact/provenance, không tự mutate policy của attempt đang chạy.

### M2 — Plugin extended manifest đã có schema nhưng runtime isolation chưa đủ

Evidence thiết kế:

- `docs/architecture.md:1419-1466` định nghĩa `cccx-plugin.json`, harness isolation, rules, agents preset, MCP stableId/schemaDigest.

Current gap:

- Schema tests đã từng xanh, nhưng runtime load/trust/isolation mapping cần e2e.
- Codex native plugin và cccx extended plugin cần phân biệt rõ.

Fix direction:

- Thêm loader tests cho native plugin vs cccx-plugin.
- Verify CX chỉ thấy plugin capability nếu WorkSpec cấp stableId/schemaDigest.

### M3 — Headless đã có envelope tests nhưng thiếu e2e với runtime cccx

Current gap:

- `cccx-headless` contract unit tests không đủ chứng minh `cccx exec` thật emit đúng event khi có tool call, subagent event, permission queue, TaskGraph, stream error.

Fix direction:

- Mock 9router e2e cho `cccx exec`.
- Assert JSONL terminal event last, sequence monotonic, no non-JSON stdout.

### M4 — Coverage aggregate chưa đủ đại diện cho feature completeness

Current state:

- Aggregate coverage từng pass trên scoped set, nhưng có crate riêng dưới 80%.
- E2E coverage cho các path cccx quan trọng còn thiếu.

Fix direction:

- Dùng hai gate:
  - aggregate scoped cccx + touched core >= 80%;
  - critical crate/path minimum riêng cho mode/policy/ux/gateway/spawn.

## Low

### L1 — Duplicate binary target warning

User từng thấy warning:

- `cli/src/main.rs` có mặt trong cả bin target `cccx` và `codex`.

Impact thấp nhưng nên dọn trước release vì gây nhiễu smoke test.

### L2 — Upstream Codex fallback/branding còn rải rác

Một số UI/help/model default path vẫn có thể rơi về upstream wording hoặc GPT preset nếu `CCCX_HOME`/gate/config không đúng.

Fix direction:

- Centralize cccx branding/model fallback.
- `cccx` binary không được dùng GPT preset mặc định trừ khi user cấu hình explicit qua 9router route.

## Done / không còn là blocker chính

Các mục sau không nên đưa lại vào phase mới nếu test vẫn xanh:

- Chat-completions gom nhiều tool calls vào một assistant message.
- MCP tool namespace split.
- EOF trước `[DONE]` không còn được coi là completed.
- Scope path boundary dùng real boundary thay vì prefix thô.
- Checkpoint yêu cầu revalidation khi git head đổi.
- Permission deny một request không cancel toàn agent.
- Plugin manifest field names khớp design schema.
- Headless envelope contract cơ bản.

Nếu một mục trong danh sách này đỏ trở lại, xử lý như regression, không coi là feature mới.
