Kết luận: `cccx` chưa khớp hoàn toàn với `architecture.md` và `design.md`. Claim “đúng thiết kế MVP” trong [progress.md](D:/Learning_Programing/cccx/docs/progress.md:157) hiện là overclaim. Các crate contract khá đầy đủ, nhưng nhiều boundary quan trọng chưa trở thành nguồn sự thật của runtime.

## Critical

1. Mode đang split-brain, chưa phải security state machine thật.

- TUI chuyển collaboration mask, nhưng permission pipeline vẫn hardcode `Default` cho shell/MCP/edit tại [exec_policy.rs](D:/Learning_Programing/cccx/codex-rs/core/src/exec_policy.rs:317).
- `Session.cccx_mode` được load/save nhưng không được cập nhật từ các mode command.
- `/bypass` gọi `attest_mode()` mà không có sandbox capability attestation thật tại [slash_dispatch.rs](D:/Learning_Programing/cccx/codex-rs/tui/src/chatwidget/slash_dispatch.rs:391).

Hệ quả: Plan/Auto/DontAsk/Bypass chưa thực sự chi phối permission như thiết kế.

2. WorkSpec deny-by-default chưa enforce đầy đủ.

- [cccx_spawn.rs](D:/Learning_Programing/cccx/codex-rs/core/src/cccx_spawn.rs:27) tính projection nhưng chỉ inject prompt, tắt hai orchestrator flag và log số lượng; không áp allowlist tool/MCP, writable scope, permission ceiling hay model route vào child `Config`.
- Tool catalog gửi cho model chưa được lọc theo WorkSpec.
- Gate runtime chỉ phủ shell/edit/MCP, chưa phủ mọi function tool.
- `parent_capability_set_for_work_spec()` lấy capability của parent trực tiếp từ chính WorkSpec và đặt permission ceiling thành `Allow`, khiến phép intersection không phản ánh quyền parent thật tại [cccx_runtime.rs](D:/Learning_Programing/cccx/codex-rs/core/src/cccx_runtime.rs:253).
- Scope check dùng `any()`: edit nhiều path có thể lọt nếu chỉ một path nằm trong scope; đồng thời chỉ thay dấu slash, không canonicalize/symlink-resolve thật tại [cccx-policy/src/lib.rs](D:/Learning_Programing/cccx/codex-rs/cccx-policy/src/lib.rs:509).

3. Hai harness chưa thực sự được thiết lập đúng vai trò.

- Main agent không thấy call-site nào cài CC harness; fragment CC/CX chỉ được inject cho child tại [cccx_spawn.rs](D:/Learning_Programing/cccx/codex-rs/core/src/cccx_spawn.rs:29).
- Ngược lại, prompt của CC subagent lại tuyên bố nó là control plane có final authority và được tạo WorkSpec cho CX tại [cccx-harness-cc/src/lib.rs](D:/Learning_Programing/cccx/codex-rs/cccx-harness-cc/src/lib.rs:24), trái với thiết kế CC subagent không phải scheduler.
- `ActorIdentity::can_spawn()` tồn tại nhưng runtime dùng substring trong prompt để nhận diện child tại [multi_agents_common.rs](D:/Learning_Programing/cccx/codex-rs/core/src/tools/handlers/multi_agents_common.rs:263).

4. Artifact/Verification có thể xác nhận sai task.

- Khi native agent chuyển sang `Completed`, runtime tự dựng `ArtifactStatus::Success`, tự gán exit code `0` và dùng chuỗi status làm “log digest” tại [cccx_runtime.rs](D:/Learning_Programing/cccx/codex-rs/core/src/cccx_runtime.rs:215).
- Provenance dùng route giống nhau và digest giả dạng chuỗi `sha256:cccx-runtime-*`, không lấy từ request 9router thật tại [cccx_runtime.rs](D:/Learning_Programing/cccx/codex-rs/core/src/cccx_runtime.rs:272).
- GoalEvaluator chỉ kiểm tra exit code được truyền vào, không chạy verification command tại [goal_evaluator.rs](D:/Learning_Programing/cccx/codex-rs/core/src/goal_evaluator.rs:13).

Vì vậy TaskGraph/StopHook có wiring thật nhưng có thể nhận verification không dựa trên test thật.

## High

- WorkSpec model route chưa được áp vào `config.model`; resolver hiện chủ yếu là contract/test, còn runtime chỉ tạo chuỗi hiển thị tại [cccx_runtime.rs](D:/Learning_Programing/cccx/codex-rs/core/src/cccx_runtime.rs:244).
- `/goal` chỉ in “goal set”, không lưu goal condition hay chạy evaluator tại [slash_dispatch.rs](D:/Learning_Programing/cccx/codex-rs/tui/src/chatwidget/slash_dispatch.rs:1009).
- TaskGraph chỉ được tạo khi spawn subagent; Main CC không có runtime tool để tạo/quản lý DAG cho task tự thực hiện.
- `/permissions approve|deny|inspect|batch-approve` chưa điều khiển queue; `/permissions` hiện mở popup đổi permission mode upstream tại [slash_dispatch.rs](D:/Learning_Programing/cccx/codex-rs/tui/src/chatwidget/slash_dispatch.rs:835).
- `/agents cancel` interrupt toàn bộ stream; resume mở generic picker; model chỉ in message; view yêu cầu UUID và chuyển hẳn sang thread subagent thay vì attach read-only tại [slash_dispatch.rs](D:/Learning_Programing/cccx/codex-rs/tui/src/chatwidget/slash_dispatch.rs:791).
- Checkpoint có API và unit test nhưng không có runtime call-site lưu/load/resume tại [checkpoint.rs](D:/Learning_Programing/cccx/codex-rs/core/src/checkpoint.rs:44).
- `cccx.exec_event.v1` chưa wire vào `cccx exec --json`; exec vẫn emit upstream `ThreadEvent`.
- `cccx-plugin.json` mới dừng ở protocol/tests, chưa có runtime loader và harness isolation.
- Project Memory ghi vào project `.cccx/memory`, trái storage design `~/.cccx/projects/<hash>/memory`, và chưa được load vào startup/context tại [project_memory.rs](D:/Learning_Programing/cccx/codex-rs/core/src/project_memory.rs:13).
- `/model` dùng catalog hardcode hai model thay vì config/catalog 9router thật tại [model_popups.rs](D:/Learning_Programing/cccx/codex-rs/tui/src/chatwidget/model_popups.rs:43).
- Biến `CCCX_MVP_ALLOW_UPSTREAM_SURFACES` có thể tắt toàn bộ 9router fail-closed boundary tại [cccx_mvp.rs](D:/Learning_Programing/cccx/codex-rs/features/src/cccx_mvp.rs:19), trái ranh giới “9router-only”.

## Những phần đã đạt hoặc gần đạt

- Main Responses/Chat Completions, compact và memory summarize đã có 9router fail-closed check.
- 9router dùng Chat Completions để router tự translation là đúng hướng.
- WorkSpec bắt buộc khi spawn.
- Shell/edit/MCP có subagent runtime gate, dù coverage và scope check chưa đủ.
- Shift+Tab chỉ cycle Plan/Default/AcceptEdits/Auto.
- Hai MCP default được auto-merge vào config; tuy nhiên command stdio mặc định vẫn phụ thuộc binary có trong PATH.
- StopHook có call-site đọc TaskGraph và block task chưa verified.
- Các contract test đã chạy đều xanh: protocol, mode, policy, provider-gateway, task-graph, UX và headless. `codex-core` runtime test chưa hoàn tất do cache được dọn.