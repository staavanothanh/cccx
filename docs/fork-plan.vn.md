# Kế hoạch Fork cccx — Codex CLI → CC/CX Harness

Upstream gốc:

- Nguồn: `D:\Learning_Programing\codex-cli-src`
- Commit: `800715d201651a2a07c2706dca10400109dae3d3`
- Chính sách: fork từ source Codex Apache-2.0 thôi. Mã nguồn rò rỉ của Claude Code không phải đầu vào triển khai.

Mục tiêu: biến Codex CLI thành `cccx`, một CLI coding cá nhân chạy local với một cổng model 9router, harness CC-led/CX-delegated, hợp đồng WorkSpec/Artifact, cổng phân quyền mạnh, Task Graph, và terminal UX mô tả trong `architecture.md` và `design.md`.

---

## 1. Chiến Lược Fork

Dùng fork toàn bộ source của Codex CLI, không copy-paste chọn lọc.

Lý do:

- Codex có nhiều crate xuyên suốt: core, tui, exec, hooks, state, rollout, model-provider, app-server-protocol.
- Đường dẫn subagent, resume, hook, permission và JSONL cắt qua ranh giới crate.
- Copy chọn lọc sẽ mất integration tests và gây khó theo dõi drift từ upstream.

Cấu trúc repo đề xuất:

```text
D:\Learning_Programing\cccx
├─ docs/                         # tài liệu thiết kế hiện tại
├─ upstream/                     # ghi chú read-only tùy chọn, không cần trong cây cuối
├─ codex-cli source files         # copy từ codex-cli-src root
└─ .cccx/                        # metadata dev local, bỏ qua nếu runtime-generated
```

Kế hoạch Git:

```text
1. Commit tài liệu hiện tại với "docs: cccx architecture baseline".
2. Copy source Codex vào repo này, loại trừ .git và build output của Codex.
3. Commit với "chore: import codex cli baseline 800715d2".
4. Thêm remote upstream trỏ đến source OpenAI Codex.
5. Mọi thay đổi cccx xảy ra trong các feature commit nhỏ sau import.
```

Đừng squash import với thay đổi cccx. Giữ import sạch để future `git diff upstream` và rebase khả thi.

---

## 2. Copy Nguyên Trạng Từ Codex

Copy những thứ này nguyên trạng trước. Đừng viết lại trong lúc import commit.

| Khu vực | Đường dẫn upstream | Chính sách ban đầu |
|---|---|---|
| Rust workspace/build | `Cargo.toml`, `Cargo.lock`, `BUILD.bazel`, `MODULE.bazel*`, `justfile`, `defs.bzl` | Copy không đổi |
| CLI launcher | `codex-cli/`, file gốc package | Copy, sau đổi tên/bọc |
| Core runtime | `codex-rs/core/` | Copy, sau thêm module biên cccx |
| TUI | `codex-rs/tui/` | Copy, sau sửa commands/status/agent panels |
| Exec/headless | `codex-rs/exec/` | Copy, sau mở rộng JSONL |
| Hooks | `codex-rs/hooks/` | Copy; không coi hooks là ranh giới bảo mật |
| State/rollout | `codex-rs/state/`, `codex-rs/rollout/`, `codex-rs/rollout-trace/` | Copy; mở rộng chỉ khi cần |
| Tầng tool/runtime | `codex-rs/apply-patch/`, `codex-rs/sandboxing/`, `codex-rs/linux-sandbox/`, `codex-rs/windows-sandbox-rs/`, `codex-rs/shell-*` | Copy; tái sử dụng nhiều |
| Nền tảng MCP/plugin/skills | `codex-rs/codex-mcp/`, `codex-rs/plugin/`, `codex-rs/core-plugins/`, `codex-rs/skills/`, `codex-rs/core-skills/` | Copy; mở rộng manifest cccx sau |
| Protocol/app server | `codex-rs/protocol/`, `codex-rs/app-server*` | Copy; mở rộng chỉ sau CLI MVP |
| Tests/snapshots | tất cả test hiện có | Copy; giữ an toàn hồi quy upstream |

---

## 3. Không Copy / Loại Trừ Khỏi Import

Loại trừ nội dung generated hoặc local-only:

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

Không copy hoặc import mã nguồn rò rỉ của Claude Code vào repo cccx. Nếu cần ghi chú, giữ chúng dưới dạng tóm tắt hành vi clean-room trong docs thôi.

---

## 4. Giữ Nhưng Tắt Hoặc Ẩn Cho MVP cccx

Những tính năng Codex này có thể ở lại source để giảm churn fork, nhưng nên bị tắt, ẩn, hoặc hard-wired cho MVP cccx cá nhân.

| Tính năng | Tại sao | Hành động |
|---|---|---|
| Luồng đăng nhập ChatGPT/OAuth | cccx chỉ dùng 9router | Giữ source, ẩn commands/UI, chặn đường dẫn runtime trừ khi bật lại |
| Chọn provider trực tiếp ngoài 9router | Vi phạm model backend chỉ-9router | Thay thế resolution config bằng route ProviderGateway |
| UX provider Bedrock/LM Studio/Ollama | Không cần cho MVP | Giữ crate nếu build cần, nhưng không expose thành UX hạng nhất cccx |
| Mặt điều khiển từ xa/tác vụ đám mây | CLI local cá nhân, không SaaS | Giữ source, feature-gate hoặc ẩn |
| UX cài đặt marketplace/plugin | Không cốt lõi cho MVP | Giữ native plugin loader, hoãn UX marketplace |
| Codex memories | cccx dùng chính sách Project Memory tường minh | Tắt admission memory mặc định; giữ state migrations nếu cần |
| Fork subagent full-history để thực thi | Xung đột với ranh giới WorkSpec/Artifact | Chỉ cho phép `full_history_debug`, không bao giờ cho CX worker execution |

---

## 5. Module cccx Mới Cần Thêm

Ưu tiên thêm crate/module dành riêng cho cccx thay vì rải logic khắp file upstream. Code Codex hiện tại nên gọi các module này tại các điểm tích phân hẹp.

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

Nếu tạo crate riêng quá nặng cho bản vá đầu, bắt đầu dưới:

```text
codex-rs/core/src/cccx/
```

Sau tách thành crate khi các điểm tích phân ổn định.

### 5.2 Bổ Sung TUI/Exec

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

### 5.3 Bổ Sung Config

```text
codex-rs/core/src/config/cccx.rs
codex-rs/core/src/config/cccx_schema.rs
```

Khóa config:

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

## 6. File Upstream Cần Sửa

Đây là bản đồ edit cụ thể đầu tiên. Giữ sửa đổi hẹp và route hành vi cccx qua module mới.

### 6.1 Ranh Giới Model Provider / 9router

| File/khu vực | Hành động |
|---|---|
| `codex-rs/model-provider/` | Thêm hoặc bọc implementation provider cho route execution chỉ-9router |
| `codex-rs/model-provider-info/` | Thêm metadata/capability mapping route cccx nếu preset model hiện tại không biểu diễn được route 9router |
| `codex-rs/models-manager/` | Feed `ModelRouteCatalog` và ẩn provider trực tiếp không phải 9router trong chế độ cccx |
| `codex-rs/core/src/client.rs`, `client_common.rs` | Đảm bảo mọi gọi model đều qua đường dẫn provenance ProviderGateway |
| `codex-rs/core/src/config/mod.rs`, `schema.rs` | Thêm trường config cccx và deprecate UX provider trực tiếp cho chế độ cccx |

### 6.2 Harness / Subagent / WorkSpec

| File/khu vực | Hành động |
|---|---|
| `codex-rs/core/src/tools/handlers/multi_agents_common.rs` | Thay thế thẩm quyền child config cuối bằng `WorkSpecToChildConfigAdapter` |
| `codex-rs/core/src/tools/handlers/multi_agents.rs` và `multi_agents_v2.rs` | Thêm đường dẫn spawn cccx yêu cầu WorkSpec |
| `codex-rs/core/src/tools/handlers/multi_agents/spawn.rs` | Route model override qua `ModelRouteResolver`, ghi lại `modelRoute` |
| `codex-rs/core/src/tools/handlers/multi_agents/resume_agent.rs` | Thêm validation resume cccx: checkpoint, base revision drift, immutable model route |
| `codex-rs/core/src/agent/control/spawn.rs` | Giữ cơ chế resume Codex, thêm metadata lifecycle cccx nếu cần |
| `codex-rs/core/src/agent/role.rs`, `config/agent_roles.rs` | Ánh xạ role preset sang role CC/CX cccx mà không cấp quyền trực tiếp |
| `codex-rs/protocol/src/protocol.rs`, `items.rs` | Thêm trường sự kiện WorkSpec/Artifact/agent provenance cccx nếu TUI/exec cần |

### 6.3 Cổng Phân Quyền / Sandbox

| File/khu vực | Hành động |
|---|---|
| `codex-rs/core/src/tools/approvals.rs` | Chèn cổng phân quyền core cccx trước approval user/plugin |
| `codex-rs/core/src/tools/sandboxing.rs` | Thi hành phạm vi WorkSpec và đường dẫn được bảo vệ verification |
| `codex-rs/core/src/tools/runtimes/apply_patch.rs` | Chặn ghi patch ngoài phạm vi WorkSpec/đường dẫn bảo vệ |
| `codex-rs/core/src/tools/runtimes/shell.rs`, `unified_exec.rs` | Phân loại hành động shell mutate/network/config-write cho cổng cccx |
| `codex-rs/core/src/mcp_tool_call.rs` | Áp dụng allowlist MCP từ WorkSpec; không CX self-expansion |
| `codex-rs/core/src/hook_runtime.rs` | Giữ hooks là phần mở rộng sau quyết định core, không phải circuit breaker |

### 6.4 Task Graph / Verification / Memory

| File/khu vực | Hành động |
|---|---|
| `codex-rs/state/` | Thêm lưu trữ task graph cccx hoặc giữ file JSON dưới `.cccx/state/` trước |
| `codex-rs/core/src/tasks/` | Thêm vòng đời TaskGraph và tích hợp VerificationRecord |
| `codex-rs/core/src/session/turn.rs`, `turn_context.rs` | Tải trạng thái TaskGraph/WorkSpec tại ranh giới turn |
| `codex-rs/core/src/compact*.rs` | Giữ TaskGraph/WorkSpec/Artifact qua compaction |
| `codex-rs/state/src/runtime/memories.rs` | Tắt auto memory admission cho cccx, chỉ giữ Project Memory tường minh |

### 6.5 TUI / Commands

| File/khu vực | Hành động |
|---|---|
| `codex-rs/tui/src/slash_command.rs` | Thêm `/agents`, `/tasks`, `/permissions`, `/remember`, `/autoreview`; tránh `/agent` số ít |
| `codex-rs/tui/src/chatwidget/command_lifecycle.rs` | Route command mới đến hành động core cccx |
| `codex-rs/tui/src/bottom_pane/status_line_*` | Thêm trạng thái mode/harness/tasks/model route/permission queue cccx |
| `codex-rs/tui/src/app/agent_status_feed.rs` | Mở rộng với logicalSessionId/displayName/modelRoute |
| `codex-rs/tui/src/chatwidget/permission_popups.rs`, `permissions_menu.rs` | PermissionRequestQueue tập trung |
| `codex-rs/tui/src/chatwidget/model_popups.rs` | Giới hạn UI model xuống catalog route 9router |

### 6.6 Headless / JSONL

| File/khu vực | Hành động |
|---|---|
| `codex-rs/exec/src/exec_events.rs` | Thêm payload `agent-update`, `task-update`, `permission-request`, `verification` cccx |
| `codex-rs/exec/src/event_processor_with_jsonl_output.rs` | Phát sự kiện route provenance và TaskGraph cccx |
| `codex-rs/exec/src/lib.rs` | Giữ hợp đồng stdout/stderr, chặn interactive approvals trong headless |
| `codex-rs/exec/tests/` | Thêm test schema JSONL cho sự kiện cccx |

### 6.7 Đóng Gói / Thương Hiệu

| File/khu vực | Hành động |
|---|---|
| `package.json`, `codex-cli/package.json` nếu có | Đổi tên binary/package surface thành cccx chỉ sau khi core build |
| `codex-cli/bin/codex.js` | Thêm launcher `cccx` hoặc bọc đổi tên |
| `codex-rs/cli/src/main.rs` | Thêm thương hiệu command cccx và config mặc định |
| `README.md`, `NOTICE`, `LICENSE` | Giữ thông báo Apache-2.0; thêm README cccx sau import |

---

## 7. File/Tính Năng Sẽ Xóa Chỉ Sau Khi MVP Hoạt Động

Tránh xóa sớm. Đầu tiên ẩn/tắt, sau xóa khi test xanh.

Ứng viên xóa sau MVP:

```text
Đường dẫn UI tác vụ đám mây
Màn hình thiết lập chỉ OAuth
UX đăng nhập Bedrock
UX đăng ký điều khiển từ xa
UX cài đặt marketplace
UX auto-admission memory Codex
Bề mặt desktop-only app-server không dùng nếu cccx CLI-only giữ nguyên
```

Đừng xóa crate cấp thấp chỉ vì MVP ẩn chúng. Một số được tham chiếu bởi app-server, tests, hoặc config schemas.

---

## 8. Các Mốc Triển Khai

### M0 — Import Sạch

Kết quả:

- Tài liệu hiện tại đã commit.
- Baseline Codex được copy thành commit import sạch.
- Lệnh build/test cho baseline upstream được ghi lại.

Kiểm tra:

```text
cargo check -p codex-core
cargo check -p codex-tui
cargo test -p codex-core multi_agent
```

Trạng thái ngày 2026-07-16: hoàn tất. Validation đã pass trên Windows với Visual Studio Build Tools 2022 và Rust toolchain `1.95.0-x86_64-pc-windows-msvc` được pin bởi repo. Với môi trường local này, chạy Cargo từ VS dev shell và prepend `D:\Program Files\cargo\bin` vào `PATH`; multi-agent test cần `RUST_MIN_STACK=33554432` kèm `-- --test-threads=1` để tránh stack overflow của test thread trên Windows.

### M0.5 — Ẩn/Tắt Surface MVP cccx

Kết quả:

- Giữ source Codex upstream để dễ merge, nhưng ẩn hoặc tắt các surface không thuộc MVP cccx.
- Ẩn hoặc chặn UX ChatGPT/OAuth login/logout trong đường chạy cccx.
- Ẩn lựa chọn provider trực tiếp ngoài 9router gateway.
- Ẩn UX Bedrock, LM Studio, Ollama và provider-specific khác khỏi surface MVP cccx.
- Ẩn remote/cloud task surfaces cho MVP cá nhân chạy local.
- Ẩn marketplace install UX nhưng giữ native plugin loader nếu build còn cần.
- Tắt default Codex memory admission cho tới khi Project Memory của cccx được implement rõ ràng.
- Xem full-history subagent fork là debug-only, không bao giờ là CX execution worker.

Kiểm tra:

- CLI help và TUI command surfaces không quảng bá feature bị tắt trong MVP.
- Runtime path của feature bị tắt phải fail-closed hoặc điều hướng sang lựa chọn được cccx hỗ trợ.
- Source upstream vẫn được giữ lại trừ khi đã chứng minh việc xóa là an toàn sau khi MVP chạy được.
- Feature gating đủ tập trung để M1/M2 có thể enforce 9router và WorkSpec policy mà không cần rải ad hoc checks.

Trạng thái ngày 2026-07-16: đã triển khai lượt gate MVP đầu tiên. Gate tập trung `codex-features::cccx_mvp` hiện ẩn hoặc chặn ChatGPT/OAuth login/logout, remote-control/root remote flags, desktop app launch, cloud tasks, plugin marketplace/add UX, default Apps/MCP Apps/MemoryTool/remote-plugin feature defaults, và các slash command TUI ngoài MVP như `/apps`, `/plugins`, `/memories`, `/logout`, `/app`, `/fork`, `/usage`, `/agent`, `/subagents`. Escape hatch: đặt `CCCX_MVP_ALLOW_UPSTREAM_SURFACES=1` để tạm mở lại surface upstream khi debug/merge. Validation đã pass: `cargo fmt --check`; `cargo check -p codex-features -p codex-cli -p codex-tui`. Source inspection xác nhận các attribute ẩn help/popup và runtime guard đã được wire. Smoke trực tiếp qua binary `cargo run -p codex-cli` đã thử nhưng timeout khi build/run Windows binary target, không có compile failure.

### M1 — Provider Gateway 9router

Kết quả:

- `ProviderGateway` với route execution chỉ-9router.
- `ModelRouteCatalog`, `ModelRouteResolver`, trường provenance.
- CC chính có thể chạy một prompt qua 9router.

Kiểm tra:

- Unit test cho route resolution: explicit, preset, inherit-main-requested, route không hợp lệ fail-closed.
- Test mock 9router cho provenance requested/effective route.

### M2 — Giao Thức WorkSpec / Artifact

Kết quả:

- `WorkSpec`, `Artifact`, `VerificationRecord` đã định kiểu.
- Test serialization.
- Artifact validator từ chối thiếu evidence/provenance.

Kiểm tra:

- Golden JSON test cho schema stability.
- Round-trip test.

### M3 — WorkSpecToChildConfigAdapter

Kết quả:

- Child config bắt đầu deny-by-default.
- Tool được phép, tool cấm, phạm vi, permission và model route chỉ đến từ WorkSpec cộng giao với parent.
- Fork full-history không thể tạo CX execution worker.

Kiểm tra:

- Test chứng minh parent broad permission không rò rỉ sang CX.
- Test chứng minh CX không thể dùng MCP ngoài WorkSpec.
- Test chứng minh đường dẫn bảo vệ không thể bị sửa.

### M4 — Cổng Phân Quyền / Circuit Breaker

Kết quả:

- Cổng phân quyền core trước hooks/user prompt.
- Circuit breaker fail-closed.
- PermissionRequestQueue chỉ một prompt hoạt động.

Kiểm tra:

- Static deny thắng static allow.
- Classifier failure leo thang hoặc từ chối như thiết kế.
- Permission digest bị hủy/hết hạn không thể được chấp thuận.

### M5 — Task Graph / Verification Loop

Kết quả:

- TaskGraph bền vững.
- Vòng đời Attempt/Worker/VerificationRecord.
- StopHook chặn "done" khi task chưa được xác minh.

Kiểm tra:

- Compact/clear không mất TaskGraph.
- CX "done" không có VerificationRecord không đánh dấu task đã verify.
- Thay đổi verification command/path được bảo vệ chặn verification.

### M6 — TUI Commands / UX

Kết quả:

- `/agents`, `/agents logs`, `/agents resume`, `/agents cancel`, `/agents model`.
- `/tasks`, `/permissions`, `/remember`.
- Thanh trạng thái cccx với mode/harness/tasks/model/permission queue.

Kiểm tra:

- Snapshot test cho status line, agent list, permission queue.
- Command parser test cho command có namespace.

### M7 — Headless Exec

Kết quả:

- Sự kiện JSONL cho agent/task/permission/terminal.
- Không prompt/spinner/banner trong stdout.
- Approval bị từ chối mặc định cho headless.

Kiểm tra:

- JSONL schema test.
- stdout/stderr separation test.

### M8 — Đổi Tên Gói

Kết quả:

- Binary `cccx`.
- Đường dẫn config mặc định và tài liệu đã cập nhật.
- Thông báo Codex provenance/Apache được giữ lại.

Kiểm tra:

- `cccx --version`
- `cccx exec --json "..."` smoke test.
- Test baseline Codex hiện tại vẫn áp dụng và xanh.

---

## 9. Danh Sách Kiểm Tra Drift Upstream

Chạy sau mỗi `git pull` từ Codex upstream:

```text
1. git rev-parse HEAD trong source upstream Codex và cập nhật architecture.md.
2. Kiểm tra tên sự kiện hook:
   - SubagentStart
   - SubagentStop
   - PermissionRequest
   - PreCompact/PostCompact
   - UserPromptSubmit
3. Kiểm tra đường dẫn spawn/resume:
   - core/src/tools/handlers/multi_agents*
   - core/src/agent/control/spawn.rs
   - resume_agent_from_rollout
4. Kiểm tra ngữ nghĩa model override:
   - multi_agents_common.rs
   - models-manager/model-provider-info
5. Kiểm tra registry command:
   - tui/src/slash_command.rs
   - chatwidget/command_lifecycle.rs
6. Kiểm tra hình dạng event JSONL exec:
   - exec/src/exec_events.rs
   - exec/src/event_processor_with_jsonl_output.rs
7. Chạy lại test dành riêng cho cccx trước khi merge upstream.
```

---

## 10. Không Phải Mục Tiêu Cho MVP

```text
- Hệ thống tài khoản public SaaS/team/nhiều người dùng
- UX đăng nhập provider trực tiếp Anthropic/OpenAI/Bedrock/Ollama
- Plugin marketplace công khai cho cccx
- Daemon nền tự động theo lịch
- Trình ghi đồng thời multi-worktree
- Chat trực tiếp user với subagent
- Import hoặc copy implementation rò rỉ của Claude Code
```

Những thứ này có thể xem xét lại sau, nhưng thêm vào lúc này sẽ làm mờ ranh giới harness core.
