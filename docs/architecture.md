# cccx — Kiến trúc Harness & Mode System

> Fork nền: Codex CLI (Apache 2.0), baseline local snapshot
> `800715d201651a2a07c2706dca10400109dae3d3`
> (`Remove template IDs from MCP tool call metadata (#33467)`, local HEAD
> tại thời điểm audit 2026-07-16). Sau mỗi lần `git pull` upstream Codex,
> phải cập nhật dòng này và rà lại các điểm kế thừa nguyên trạng ở mục 15.
> Harness Claude-like chỉ được thiết kế từ public docs, public behavior,
> black-box observation và các invariant tự mô tả trong repo này. Không dùng
> non-public proprietary source làm input thiết kế chính thức. Model backend đi
> qua 9router proxy duy nhất.

---

## 1. Nguyên tắc nền tảng

```text
1. Harness quyết định quyền, không phải prompt.
2. Mode là state machine, chỉ user chuyển mode.
3. Mỗi agent instance (main hoặc subagent) chạy đúng 1 harness trong toàn bộ vòng đời.
4. CC là control plane duy nhất. CX là execution plane duy nhất.
5. Giao tiếp cross-harness chỉ qua Artifact/WorkSpec, không truyền raw context.
6. Mọi tool call đi qua permission gate, kể cả khi mode cho phép "đầy đủ quyền".
```

---

## 2. Cấu trúc thư mục

```text
cccx
├─ core
│  ├─ provider-gateway/       # mọi LLM call đi qua 9router, có provenance/retry policy
│  ├─ session/                # conversation state, mode state machine
│  ├─ tool-execution/         # edit, shell, search, git...
│  ├─ patch-engine/           # diff, checkpoint, rollback
│  ├─ permission-gate/        # rule tĩnh + classifier động
│  ├─ mcp-registry/           # danh sách MCP đã kết nối: global default
│  │                          # (2 MCP cố định) + mở rộng (thêm theo nhu cầu)
│  ├─ decision-store/         # ADR commit layer (qua codebase-memory-mcp manage_adr)
│  ├─ task-graph/             # Task DAG bền vững, file JSON tĩnh (không SQLite),
│  │                          # độc lập context window
│  ├─ compaction/             # quy tắc nén context: cái gì sống sót, cái gì bị tóm tắt
│  ├─ goal-evaluator/         # model/service riêng đánh giá điều kiện hoàn thành
│  └─ telemetry/
├─ harnesses
│  ├─ cc/                     # Claude-like: main agent mặc định
│  │  ├─ prompts/
│  │  ├─ planning-policy/
│  │  ├─ decision-policy/
│  │  ├─ mcp-routing/
│  │  └─ subagent-orchestration/
│  └─ cx/                     # Codex-like: subagent thực thi
│     ├─ prompts/
│     ├─ edit-policy/
│     ├─ verification-policy/
│     └─ review-policy/
├─ modes/                     # state machine: Plan, Default, AcceptEdits, Auto, DontAsk, Bypass
├─ hooks/                     # PreToolUse, PostToolUse, PreDecisionWrite, PreMCPCall, Stop...
├─ plugins/                   # kế thừa Codex native plugin.json; cccx-plugin.json
│                             # mở rộng thêm harness/rules/agents (mục 15)
└─ bin/
   └─ cccx
```

---

## 2b. Startup & Init Sequence — core behavior, không phải UX

> Startup sequence chạy ở core layer, không phải UX layer. UX chỉ hiển thị
> kết quả (xem design.md mục 10). Section này define THỨ TỰ init logic.

```text
Thứ tự init khi cccx khởi động:

1. Load config (prioritized, merge layered):
   - Default values (hardcoded)
   - User config (~/.cccx/config.json) — override defaults
   - Project config (.cccx.json ở project root) — override user config
   - CLI flags (--model, --mode...) — override mọi config
   → Nếu conflict, ưu tiên: CLI flags > project > user > default

2. Kết nối 9router:
   - Kiểm tra API key (từ config hoặc env var CCCX_API_KEY)
   - Test connectivity (health check call)
   - Nếu fail → stderr error, KHÔNG block startup (user có thể retry sau)

3. Init MCP servers:
   - Luôn init 2 global defaults: codebase-memory-mcp, sequential-thinking
   - Registry mở rộng: load từ config, init theo thứ tự khai báo
   - Nếu MCP init fail → log warning, tiếp tục (không block startup)

4. Load Task Graph:
   - Đọc file task-graph.json từ project root (nếu có)
   - Nếu không có → start với Task Graph trống
   - Resume session: Task Graph là nguồn sự thật, KHÔNG phụ thuộc context

5. Chọn mode khởi động:
   - Nếu CLI flag --mode có mặt → dùng mode đó.
   - Nếu project/user config có defaultMode → dùng defaultMode đó.
   - Nếu không có cấu hình → Plan.
   - Heuristic từ task prompt chỉ được dùng để ĐỀ XUẤT mode sau khi hiển thị
     cho user, không tự cấp thêm quyền. Task text là input chưa tin cậy, không
     được dùng làm nguồn quyết định privilege.

6. Ready → hiển thị startup UI (design.md mục 10)
```

```text
Headless mode (cccx exec):
- Bỏ qua step 6 (không hiển thị startup UI)
- Nếu step 2 fail → exit code != 0, in error ra stderr
- Nếu mọi step OK → silent, chờ input
```

---

## 3. Phân vai Harness & Actor

> Hệ thống có **3 loại actor**. Harness quyết định năng lực (tool, MCP access, permissions). Quyền hạn cụ thể luôn từ WorkSpec do Main CC cấp, không tự suy từ harness.

### 3.1 Main CC (Control Plane — instance duy nhất)

```text
- Là instance gốc khi cccx khởi động, CHỈ CÓ 1 trong toàn bộ session.
- Full authority: scheduler duy nhất, final decision trên mọi Task.
- Có đầy đủ 6 mode state machine (mục 4) — tự quyết quyền hạn dựa trên mode.
- Đầy đủ MCP registry: codebase-memory-mcp, sequential-thinking + extension.
- Được quyền commit ADR (mục 9), quản lý Task Graph (mục 7).
- Có thể tự làm việc nhẹ trực tiếp hoặc spawn subagent (CC hoặc CX harness).
- Là kênh giao tiếp duy nhất với user — user không chat trực tiếp với subagent.
```

### 3.2 CC-harness subagent

```text
- Do Main CC spawn, chạy CC harness (cùng năng lực: MCP, tool, sequential-thinking).
- KHÔNG có quyền của Main CC: không commit ADR, không làm scheduler,
  không spawn subagent khác, không quản lý Task Graph.
- KHÔNG có mode riêng — bound hoàn toàn bởi WorkSpec.constraints.
- Dùng cho role research/architect/planning — tận dụng CC năng lực phân tích
  mà không tốn Main CC context window.
- MCP access: CHỈ tool trong WorkSpec.allowedTools, KHÔNG full registry.
- Giao tiếp với Main CC qua WorkSpec/Artifact như CX.
```

### 3.3 CX-harness subagent

```text
- Do Main CC spawn, chạy CX harness (limited tool set, execution-focused).
- Không sequential-thinking mặc định (trừ khi WorkSpec bật).
- KHÔNG có mode, KHÔNG có ADR, KHÔNG scheduler.
- Dùng cho role implementer/tester/auditor/resolver — bounded execution.
- MCP access: CHỈ tool trong WorkSpec.allowedTools.
- Audit đặc thù: role=auditor — chỉ đọc/đánh giá, không edit (mục 6).
```

### 3.4 Nguyên tắc xuyên suốt

```text
- Main CC **được phép tự làm việc nhẹ trực tiếp** nếu mode hiện tại
  cho phép write — không bắt buộc phải luôn spawn subagent.
- Mọi subagent (CC-harness lẫn CX-harness): user chỉ attach xem read-only,
  không chat/nhập input trực tiếp. Mọi điều khiển qua CC bằng lệnh cấp cao.
- Phân quyền dựa trên "có phải Main CC hay không", không dựa trên
  "chạy harness nào". Harness quyết định năng lực, WorkSpec quyết định quyền hạn.
```

---

## 4. Hệ thống Mode — chỉ áp dụng cho Main CC

> Bảng dưới đây chỉ áp dụng cho **Main CC**. Mọi subagent (CC-harness lẫn CX-harness)
> **KHÔNG có mode riêng** — bị bound hoàn toàn bởi WorkSpec.constraints,
> không dùng state machine mode. Quyền của subagent do WorkSpec.allowedTools/
> forbiddenTools quyết định, không phải mode.

| Mode | Quyền CC main agent | Delegate Rule | Dùng khi |
|---|---|---|---|
| **Plan** | Read/search/MCP/sequential-thinking only. Cấm edit, cấm shell mutate, cấm memory commit | Output = WorkSpec draft, không thực thi | Task mới, repo lạ, scope chưa rõ |
| **Default (Ask)** | Write được, nhưng mỗi edit/command đều hỏi user | Nếu ước lượng việc lớn → CC đề xuất spawn CX (WorkSpec), chờ user duyệt | Task nhạy cảm, cần theo dõi từng bước |
| **AcceptEdits** | Edit file tự động trong ngưỡng nhỏ (diff/file thấp); shell command vẫn hỏi | Vượt ngưỡng nhỏ → tự động đề xuất spawn CX, không tự vượt rào | Task đã scope rõ, tin tưởng hướng đi |
| **Auto** | Full thao tác (edit/shell/spawn), nhưng mọi action qua **classifier riêng** trước khi chạy | Classifier có thể tự **reroute** sang CX nếu action vượt rủi ro/quy mô, không chỉ approve/deny | Task dài hơi, main agent cần tự chủ cao |
| **DontAsk** | Chỉ tool nằm trong allowlist cấu hình sẵn được chạy, còn lại từ chối im lặng | Dùng cho session tự động (nền tảng cho AutonomousMode sau này) | CI, automation, batch job |
| **Bypass** | **CC main agent:** skip tất cả permission check thông thường (rule tĩnh, threshold, classifier). Vẫn giữ Circuit Breaker tối thiểu (mục 5), không thể tắt. | Không delegate rule, chạy thẳng. | Chỉ trong container/VM cô lập, test pipeline. |

### Quyết định platform: Bypass bị disable khi host không chứng minh isolation

**Quyết định owner (2026-07-21):** bản cccx cá nhân hiện tại không cung cấp
Bypass trên Windows/app-server. `x-oai-attestation`, trạng thái Windows sandbox
đã được cấu hình, env var và `--confirm` đều không phải bằng chứng rằng executor
của đúng thread đang chạy trong một container/VM cô lập và không thể thoát
sandbox. Vì chưa có backend tạo proof gắn với `threadId` và isolation thực,
mọi yêu cầu chuyển sang Bypass phải fail-closed và giữ nguyên mode trước đó.

Bypass vẫn là một mode trong protocol để không phá schema và để có thể bổ sung
host cô lập thật trong tương lai. Muốn bật lại phải có một quyết định kiến trúc
mới cùng positive runtime proof; chỉ thêm confirm dialog hoặc đọc config không
đủ. Circuit Breaker vẫn được kiểm thử độc lập như bất biến phòng thủ, kể cả khi
Bypass đang bị disable trên platform hiện tại.

**Quy tắc chuyển mode:**
```text
- Chỉ user chuyển mode — qua CẢ HAI: phím tắt (vd Shift+Tab cycle qua các mode
  chính) LẪN slash command (vd /mode auto, /plan) để linh hoạt theo ngữ cảnh
  dùng (gõ nhanh khi đang tương tác vs gọi tường minh trong script/headless).
- CC có thể đề xuất ("việc này vượt ngưỡng AcceptEdits, đề xuất chuyển Auto
  hoặc approve spawn CX") nhưng không tự chuyển bằng bất kỳ cách nào.
- Default mode khi khởi động lấy từ CLI flag hoặc config. Nếu không có, dùng Plan.
- CC có thể gợi ý chuyển sang AcceptEdits/Auto khi task rõ scope, nhưng gợi ý
  không tự thay đổi mode và không cấp quyền trước khi user xác nhận.
```

---

## 5. Permission Gate — thuật toán cụ thể

### Circuit Breaker — bất biến, áp dụng ở MỌI mode kể cả Bypass

```text
Danh sách tối thiểu, không thể tắt bằng bất kỳ mode/config nào:
- rm -rf trên root (/) hoặc home (~) hoặc ổ đĩa hệ thống
- format/wipe filesystem
- git force-push lên branch bảo vệ (main/master) kèm --force không xác nhận

Đây là điểm vá mâu thuẫn: mode Bypass "không permission check nào cả"
KHÔNG áp dụng cho circuit breaker — Bypass chỉ skip các bước hỏi thông
thường (rule tĩnh, threshold, classifier), không skip circuit breaker.
Điều này khớp với nguyên tắc mục 14: "không mode nào miễn trừ hoàn toàn
Lớp 1" — thực ra chỉ đúng cho phần circuit breaker, còn rule tĩnh khác
(secrets, destructive git thường, network install) THÌ Bypass được miễn.
```

### Pipeline đánh giá — fail-closed, rule nguy hiểm thắng rule tiện lợi

```text
function evaluate(action, session):
    normalized = normalize(action)
    // chuẩn hóa: actionType (read|edit|shell|mcp_call|network),
    // resolve absolute path, parse command nếu là shell

    # Bước 0 — Circuit breaker (luôn chạy, kể cả Bypass)
    if match(normalized, CIRCUIT_BREAKERS):
        return deny(reason="circuit_breaker", bypassable=false)

    # Bước 1 — Bypass fast path (sau circuit breaker)
    if session.mode == Bypass:
        log(normalized, decision=ALLOW, tier="bypass")
        return allow()

    # Bước 2 — Tier 1b: static deny/escalate
    // (secrets path, destructive git thường, network install)
    if match(normalized, STATIC_ESCALATE_RULES):
        if session.mode == DontAsk: return deny(silent=true)
        if session.mode == Plan:    return deny(reason="plan-read-only")
        return requireApproval(reason=matchedRule.reason)

    # Bước 3 — Tier 1a: static allowlist (read-only/no-state-change)
    # Chạy SAU deny/escalate để secret/destructive path không bị generic read
    # allowlist nuốt mất.
    if match(normalized, STATIC_ALLOWLIST):
        return allow()   // không tốn model call, xem mục 5b

    # Bước 4 — Cumulative threshold (projected, chưa commit counter)
    projected = session.counters.project(normalized)
    if projected.exceeds(session.mode.thresholds):
        return escalate(normalized, session.mode)
        // AcceptEdits -> đề xuất spawn CX (mục 4)
        // Auto        -> reroute-to-CX qua classifier (bước 6)

    # Bước 5 — Mode dispatch cho phần còn lại (chưa bị chặn/allow ở trên)
    switch session.mode:
        case Plan:        return deny(reason="plan-mode")  // double-check
        case Default:      return requireApproval()
        case AcceptEdits:
            if normalized.type == "edit": return allow()
            else: return requireApproval(reason="acceptEdits-only-covers-edit")
        case Auto:         return classifierGate(normalized, session)  // Tier 2
        case DontAsk:
            if match(normalized, session.config.explicitAllowRules): return allow()
            return deny(silent=true)

// Counter chỉ commit một lần ở PostToolUseHook khi action thực sự đã chạy.
// denied/pending request không được tính như thay đổi thành công.
```

### Tier 2 — classifierGate (chỉ gọi khi lọt hết Bước 0-5 ở mode Auto)

```text
function classifierGate(normalized, session):
    // gọi model nhỏ/rẻ qua 9router, KHÔNG dùng cùng model CC/CX chính
    result = callClassifier(normalized, context={
        objective: session.currentTask.objective,
        recentArtifacts: session.currentTask.artifacts.last(3)
    })
    if result invalid or timeout:
        return requireApproval(reason="classifier_unavailable")
    // Classifier chỉ được giảm quyền hoặc yêu cầu reroute/approval trong trần
    // deterministic đã tính ở evaluate(); không được tự vượt STATIC_ESCALATE,
    // CIRCUIT_BREAKERS, scope hay mode ceiling.
    return result  // allow | deny | reroute-to-CX | require-approval

// input-side injection scan là hook RIÊNG, chạy trên tool OUTPUT khi đọc
// (không phải trên quyết định gọi tool) — xem PreMCPCallHook/PostToolUseHook,
// không nằm trong luồng evaluate() ở trên vì đây là 2 mối quan tâm khác nhau:
// evaluate() quyết định "được làm hay không", injection scan quyết định
// "nội dung đọc về có đáng tin hay không".
```

### Static Rule Table — cấu trúc dữ liệu

```ts
{
  id: string,
  tier: "allowlist" | "escalate" | "circuit_breaker",
  actionType: "read" | "edit" | "shell" | "mcp_call" | "network",
  pattern: string,          // glob cho path, regex cho command
  reason: string,           // hiển thị khi require-approval
  priority: number          // circuit_breaker > escalate > allowlist khi overlap
}
```

Nguyên tắc override khi 1 action khớp nhiều rule: **circuit_breaker > escalate > allowlist**. Rule an toàn luôn thắng rule tiện lợi, không phụ thuộc thứ tự khai báo trong config.

---

Hook coverage (không chỉ edit/shell):

```text
PreEditHook        -> gọi evaluate() với actionType=edit trước khi ghi file
PreShellHook       -> gọi evaluate() với actionType=shell trước khi exec
PreMCPCallHook     -> gọi evaluate() với actionType=mcp_call; side-effect
                       tool (ghi/network) coi như write, tool read-only
                       (search_graph, trace_path...) rơi vào Tier 1a
PreDecisionWriteHook -> chỉ CC được commit ADR, phải qua checkpoint (mục 9)
                       (đổi tên từ "Memory" — codebase-memory-mcp không lưu
                       memory dài hạn, chỉ ADR qua manage_adr, xem mục 9)
PostToolUseHook    -> log kết quả, commit cumulative counter đúng 1 lần cho
                       action đã thực thi thành công, chạy
                       injection scan trên tool output vừa nhận về
StopHook           -> chạy TRƯỚC khi CC được phép báo "done" cho user;
                       CHỈ kiểm tra deterministic: mọi Task trong Task Graph
                       đã có verifiedAt chưa (mục 7). StopHook KHÔNG tự gọi
                       model để đánh giá — việc model-evaluate (nếu có) đã
                       xảy ra TRƯỚC đó qua goal-evaluator (mục 12) và kết quả
                       đã được ghi vào verifiedAt/goalConditionMet rồi.
                        StopHook chỉ đọc lại trạng thái đã chốt, không tự quyết.
```

### Secure Input Mode — input nhạy cảm

```text
- Khi tool/action yêu cầu nhập thông tin nhạy cảm (API key, token, secret):
  evaluate() trả require-approval với lý do "secure_input_required".
- Echo off: terminal không hiển thị ký tự đã gõ, hiện block placeholder (██).
- Input KHÔNG lưu vào command history, session log, hay telemetry.
- Input giữ trong memory process, KHÔNG persist dạng plaintext ra đĩa.
- Các trường hợp kích hoạt: API key setup wizard, MCP server auth token,
  /config set <key> — KHÔNG có value inline. Sau khi nhận command,
  TUI chuyển sang secure input mode (xem design.md mục 16).
- UX chi tiết: xem design.md mục 16.
```

### Subagent Permission Evaluation — pipeline riêng, tách khỏi mode Main CC

> Pipeline subagent KHÔNG dùng 6-step evaluate() của Main CC (mục 5.1).
> Subagent (CC-harness lẫn CX-harness) dùng pipeline riêng, kiểm tra theo
> WorkSpec, không theo mode.

```text
function evaluateSubagent(action, workSpec):
    normalized = normalize(action)

    # Bước 0 — Circuit breaker (giống Main CC, bất biến)
    if match(normalized, CIRCUIT_BREAKERS):
        return deny(reason="circuit_breaker", bypassable=false)

    # Bước 1 — Check allowedTools
    if normalized.tool not in workSpec.allowedTools:
        return deny(reason="tool_not_allowed")

    # Bước 2 — Check forbiddenTools
    if normalized.tool in workSpec.forbiddenTools:
        return deny(reason="tool_forbidden")

    # Bước 3 — Check scope (file/module)
    # Scope check dùng canonical absolute path + symlink resolution + multi-path
    # shell/MCP arg extraction. Không so sánh string thô.
    if any(normalized.targets.canonical not in workSpec.scope.canonicalPaths):
        return deny(reason="out_of_scope")

    # Bước 4 — Check enforcedConstraints (machine-checkable)
    for constraint in workSpec.enforcedConstraints:
        if violates(normalized, constraint):
            return deny(reason=constraint.id)

    # Bước 5 — Check riskThreshold (nếu có)
    if workSpec.riskThreshold:
        if count.changedFiles > workSpec.riskThreshold.maxFilesChanged:
            return escalate(workSpec.approvalPolicy, reason="max_files_exceeded")
        if count.changedLines > workSpec.riskThreshold.maxLinesChanged:
            return escalate(workSpec.approvalPolicy, reason="max_lines_exceeded")
        if match(normalized.target, workSpec.riskThreshold.forbiddenPatterns):
            return escalate(workSpec.approvalPolicy, reason="forbidden_pattern")

    # Bước 6 — Check actionClasses (nếu có)
    if workSpec.actionClasses and normalized.actionType in workSpec.actionClasses:
        return escalate(workSpec.approvalPolicy, reason="action_class_requires_approval")

    # Bước 7 — Fallback: allowed nếu không vi phạm gì
    if workSpec.approvalPolicy == "auto-deny":
        return deny(reason="auto_deny_policy")
    return allow()

function escalate(policy, reason):
    match policy:
        case "auto-deny":    return deny(reason=reason)
        case "ask-cc":       return requireApproval(target="cc", reason=reason)
        case "ask-user":     return requireApproval(target="user", reason=reason)
        case undefined:      return requireApproval(target="user", reason=reason)

# Không có Tier 2 classifier cho subagent.
# Không có mode check (subagent không có mode).
# AllowedTools/forbiddenTools do Main CC đặt trong WorkSpec.
```

### Defense-in-depth: Permission Gate + OS Sandbox

> cccx bảo vệ 2 lớp độc lập, kế thừa OS sandbox từ Codex CLI.

```text
1. Control Plane (Permission Gate):
   evaluate() pipeline + Circuit Breaker + Static Rules + Mode dispatch
   → Quyết định "được làm hay không" dựa trên mode/constraints.

2. Execution Plane (OS Sandbox — kế thừa Codex CLI):
   macOS Seatbelt | Linux Landlock/seccomp | Windows Sandbox
   → Chốt chặn cuối: ngăn chặn hành vi can thiệp hệ thống
     ngay cả khi permission gate vô tình cấp quyền.
   → cccx kế thừa nguyên, không viết lại.
   → Chi tiết sandbox config: codex-rs/core/src/sandboxing/

Hai lớp độc lập, không thay thế nhau. Permission gate quyết định
"có được phép không", OS sandbox enforce "có thể làm được không".
```

---

## 6. Giao tiếp Cross-Harness — WorkSpec / Artifact

**CC → subagent (WorkSpec):**
```ts
{
  schemaVersion: "cccx.work_spec.v1",
  id: string,
  taskId: string,
  attemptId: string,
  createdAt: string,
  expiresAt?: string,
  baseRevision: {
    gitHead?: string,
    workingTreeDigest?: string
  },
  harness: "cc" | "cx",           // quyết định năng lực (tool, MCP, sequential-thinking)
  role: "researcher" | "architect" | "planner"     // CC-harness roles
      | "implementer" | "tester" | "auditor" | "resolver",  // CX-harness roles
  // Validate role ↔ harness bằng discriminated union:
  // CC roles chỉ đi với harness="cc"; CX roles chỉ đi với harness="cx".
  // "auditor" tách riêng khỏi "reviewer" cũ: auditor CHỈ đọc/đánh giá,
  // không có quyền edit — tránh nhầm vai trò thực thi và vai trò kiểm tra
  objective: string,
  forkMode: "workspec_only" | "full_history_debug",
  // MVP default = "workspec_only". "full_history_debug" chỉ dùng để quan sát/debug,
  // inherit parent và không được override harness/role/modelRoute/permission scope.
  scope: {
    canonicalPaths: string[],     // absolute, symlink-resolved, dùng để enforce
    advisoryGlobs?: string[]      // chỉ để hiển thị/giải thích cho user
  },
  enforcedConstraints: {
    id: string,
    kind: "no_edit" | "path_allow" | "path_deny" | "action_class" |
          "max_files_changed" | "max_lines_changed" | "no_network" |
          "require_test_command" | "custom_static_rule",
    value: unknown
  }[],
  advisoryConstraints: string[],  // prose: "giữ retry behavior", "đừng đổi public API"
  allowedTools: {
    stableId: string,             // vd shell.exec, fs.read, mcp:codebase-memory.search_graph
    serverId?: string,
    schemaDigest?: string
  }[],
  forbiddenTools: string[],      // vd: decision_write (chỉ Main CC được commit ADR), network
  modelRoute: {                  // kết quả đã resolve trước khi spawn, immutable trong attempt
    selectionSource: "user_override" | "role_preset" |
                     "agent_suggested" | "inherit_main_requested" |
                     "inherit_main_effective" | "global_default",
    requestedRouteId: string,     // opaque 9router route/model id, không parse provider từ string
    modelProfileId: string,       // profile logical: context/tool/streaming/reasoning capabilities
    capabilityProfileId: string,
    fallbackPolicyId: "strict" | "compatible" | "router_default",
    parentAttemptId?: string,
    parentRequestedRouteId?: string,
    parentEffectiveRouteId?: string
  },
  auditDepth?: number,           // xem "Chống vòng lặp audit" bên dưới — chỉ CX có audit
  verificationSpec?: {
    policyId: string,
    commands: {
      argv: string[],
      cwd: string,
      timeoutMs?: number
    }[],
    protectedPaths: string[],     // test/golden/config acceptance; CX không được sửa nếu không được cấp rõ
    protectedDigests?: {
      path: string,
      sha256: string
    }[]
  },
  expectedArtifact: {
    requiredFields: string[],
    verificationPolicyId?: string
  },

  // Approval policy — subagent permission handling
  approvalPolicy?: "auto-deny" | "ask-cc" | "ask-user",
  // auto-deny: subagent tự deny và ghi vào Artifact.residualRisks
  // ask-cc: gửi PermissionRequest về CC main session để CC quyết định
  // ask-user: route về main session, hiển thị "agent <name> đang hỏi"
  // Mặc định nếu không có: "ask-user" cho safety

  riskThreshold?: {
    maxFilesChanged?: number,    // vd: 5 files
    maxLinesChanged?: number,    // vd: 200 lines
    forbiddenPatterns?: string[] // vd: ["*.env", "credentials.*"]
  },
  // Khi vượt threshold → trigger approvalPolicy thay vì tiếp tục

  actionClasses?: ("network" | "shell-mutate" | "config-write" | "test-skip")[],
  // Những loại action nào cần escalate theo approvalPolicy

  // Resume/checkpoint
  checkpointId?: string,  // ID của checkpoint để resume, do SubagentStop ghi
  baseCommit?: string,    // git commit hash tại thời điểm spawn (để detect drift)
}
```

**Subagent Model Route Policy — CC resolve, WorkSpec pin, ProviderGateway ghi sự thật:**
```text
Mục tiêu: copy nguyên tắc của Claude Code/Codex (inherit/config/override),
nhưng fit với 9router nơi cùng một model logical có thể có nhiều provider route id.

Thứ tự resolve khi Main CC spawn subagent:
1. User override cho lần spawn hiện tại, ví dụ command/chat chỉ rõ route/model.
2. Role/agent preset trong config cccx hoặc plugin cccx đã trust.
3. Agent-suggested route, CHỈ nếu route nằm trong allowedModelRoutes của role
   và CC policy cho phép auto-selection.
4. Session default_subagent_model.
5. Fallback mặc định: inherit_main_requested.

Default của MVP: inherit_main_requested.
- Nghĩa là subagent kế thừa requestedRouteId mà user/session đã chọn cho Main CC.
- Không mặc định inherit effective route sau fallback của 9router, vì fallback có thể
  là trạng thái tạm thời và không nên âm thầm đổi toàn bộ worker fleet sang provider khác.
- Nếu user thật sự muốn bám theo route effective đang chạy, phải set rõ
  inherit_main_effective trong config hoặc command.

Guardrails:
- requestedRouteId là opaque id do 9router/catalog/config cung cấp; cccx không tách
  provider/model bằng string heuristic.
- Resolver phải validate route nằm trong allowedModelRoutes/global allowlist và
  capabilityProfile thỏa role requirements (tools, context window, structured output,
  streaming, reasoning knobs nếu có). Không đạt thì block spawn và báo user/CC,
  không auto downgrade mù.
- Agent-suggested route chỉ được nhận qua typed spawn request field có reason ngắn,
  không parse từ prose. Nếu thiếu field hoặc route không nằm trong allowlist/capability
  profile thì resolver bỏ qua suggestion và dùng bước fallback tiếp theo.
- Model route immutable trong một attempt. /agents model hoặc /reroute chỉ ảnh hưởng
  lần spawn tiếp theo hoặc tạo WorkSpec/attempt mới.
- Fallback của 9router chỉ hợp lệ trong fallbackPolicyId đã pin. Nếu effective route
  khác requestedRouteId, Artifact.provenance phải ghi fallback chain và effective route.
- ModelRouteCatalog được cache cục bộ với fetchedAt, source, routeId, capabilityProfileId
  và digest. Nếu route không có trong catalog còn fresh, resolver có thể refresh qua
  9router; nếu vẫn không validate được thì fail-closed, không spawn mù.
```

**Fork-context rule kế thừa từ Codex mới:**
```text
Codex upstream hiện có full-history fork mode, trong đó child agent kế thừa
agent type/model/reasoning effort của parent và reject child overrides. cccx không
dùng full-history fork làm đường spawn mặc định, vì thiết kế cross-harness yêu cầu
WorkSpec/Artifact thay vì raw context dump.

MVP spawn path:
- forkMode = "workspec_only" cho CC/CX subagent bình thường.
- Model route/role/tools/sandbox được resolve từ WorkSpec như trên.
- Nếu sau này cần full-history fork để debug hoặc quan sát, rule bắt buộc là:
  không cho đổi harness, role, modelRoute, reasoning profile hoặc permission scope
  trên child fork; fork đó inherit parent và không được dùng làm CX execution worker.
```

**subagent → CC (Artifact):**
```ts
{
  schemaVersion: "cccx.artifact.v1",
  id: string,
  taskId: string,
  attemptId: string,
  workSpecId: string,
  status: "done" | "blocked" | "needs_review" | "interrupted" | "waiting" |
          "failed" | "cancelled",
  changedFiles: string[],
  diffSummary: string,
  testsRun: {
    command: string,
    exitCode: number,
    logDigest: string,
    excerpt?: string
  }[],
  residualRisks: string,
  followUp: string,
  provenance: {
    providerGatewayRequestId: string,
    modelProfileId: string,
    capabilityProfileId: string,
    fallbackPolicyId: string,
    requestedRouteId: string,
    effectiveRouteId: string,
    requestedModel?: string,
    effectiveModel?: string,
    effectiveProvider?: string,
    inputDigest: string,
    outputDigest: string,
    toolCallSequenceDigest: string,
    fallbackChain: string[]
  },
  evidence: {                    // BẮT BUỘC với role=auditor, khuyến nghị với mọi role
    findings: { location: string, severity: "info"|"warn"|"critical", detail: string }[],
    // findings có location cụ thể (file:line) + mức độ, không chỉ verdict chung chung
    // -> CC có thể tự đọc lại đúng chỗ đó thay vì phải tin lời hoặc spawn audit khác
    rawLogExcerpt?: string        // đoạn log test/build thật, không phải tóm tắt suông
  }
}
```

CC không đọc raw log/reasoning đầy đủ của subagent — chỉ nhận Artifact đã chuẩn hóa, nhưng `evidence.findings` đủ cụ thể để CC tự kiểm chứng nhanh bằng tool đọc của chính mình.

**Verification boundary — chống Goodhart "sửa điều kiện để pass":**
```text
- WorkSpec.verificationSpec là acceptance boundary của attempt.
- CX không được sửa protectedPaths/protectedDigests hoặc command verification nếu
  WorkSpec không cấp rõ quyền đó. Nếu cần đổi test/golden/verification command,
  CX phải trả Artifact{status="blocked"} kèm lý do; Main CC/user quyết định.
- VerificationRecord phải ghi command/digest thật đã chạy. Nếu protected digest đổi
  ngoài dự kiến, task không được verified cho tới khi CC tạo WorkSpec mới.
```

**VerificationRecord — chỉ harness được ghi, model không tự set verifiedAt:**
```ts
{
  schemaVersion: "cccx.verification_record.v1",
  taskId: string,
  attemptId: string,
  verifier: "cc-harness" | "goal-evaluator" | "test-runner",
  policyId: string,
  command?: {
    argv: string[],
    cwd: string,
    exitCode: number,
    startedAt: string,
    finishedAt: string,
    logDigest: string
  },
  artifactDigest: string,
  resultRevision: {
    gitHead?: string,
    workingTreeDigest?: string
  },
  passed: boolean,
  reason: string
}
```

`Task.verifiedAt` chỉ được set sau khi có `VerificationRecord.passed=true`.
CC/CX/evaluator có thể đề xuất đánh giá, nhưng không được tự ghi một bit
"đã xong" chỉ bằng lời trong Artifact.

**Nguyên tắc giao tiếp user ↔ subagent:**
```text
- User KHÔNG tương tác trực tiếp (chat/nhập input) với bất kỳ subagent nào,
  dù CC-harness hay CX-harness.
- User chỉ attach xem read-only transcript của subagent (qua `/agents view <id>`).
- Mọi điều khiển subagent đều qua CC bằng lệnh cấp cao: /agents cancel,
  /agents resume, /agents model.
- Lý do: giữ CC là kênh điều khiển duy nhất, tránh 2 kênh giao tiếp song song
  gây mất đồng bộ Task Graph và bypass permission pipeline.
```

### Resumable Execution — chống mất tiến độ khi lỗi kết nối

> Nguyên tắc: mất kết nối KHÔNG đồng nghĩa mất tiến độ. Subagent phải tự
> checkpoint trong lúc làm việc, và CC phải "nói chuyện lại" với ĐÚNG
> subagent instance đó (qua logicalSessionId) để ra lệnh tiếp tục, thay vì luôn tạo
> WorkSpec mới từ đầu.

**Identity runtime gắn với WorkSpec:**
```ts
{
  workSpecId: string,
  logicalSessionId: string,       // định danh worker logical, dùng cho resume đúng agent
  workerIncarnationId: string,    // đổi mỗi lần process/transport incarnation mới
  displayName: string             // tên hiển thị do harness sinh tất định, vd "a1-auth-fix"
}
```
 
### Subagent lifecycle — spawn, chạy nền, resume, cancel

> Subagent không phải tài nguyên sống tới cuối dự án. Nó là worker theo
> WorkSpec, có scope vừa đủ, có vòng đời rõ ràng, và CC có quyền thu hồi khi
> không còn cần thiết.

```text
Trạng thái worker attempt (áp dụng cho cả CC-harness và CX-harness):
created -> running -> completed
                  \-> waiting -> running     # khi chờ approval từ CC/user
                  \-> interrupted -> resumable -> running
                  \-> cancelled
                  \-> failed

- running: subagent đang thực thi WorkSpec, có logicalSessionId + displayName ổn định.
- waiting: subagent đang chờ approval từ CC/user (e.g., vượt riskThreshold).
  Sau khi nhận quyết định → quay lại running.
- completed: đã trả Artifact cuối. KHÔNG resume cùng attempt. Nếu cần làm tiếp,
  CC tạo WorkSpec/attempt mới có parentAttemptId.
- interrupted/resumable: có checkpoint, CC có thể gửi RESUME vào cùng
  logicalSessionId nhưng workerIncarnationId mới.
- cancelled: bị user hoặc CC hủy. KHÔNG auto-resume bằng SendMessage/RESUME;
  muốn dùng lại phải spawn instance mới hoặc user mở lại rõ ràng.
- failed: lỗi không thể tự phục hồi, CC phải tạo WorkSpec correction hoặc hỏi user.
```

**Quyền hủy/thu hồi:**
```text
- CC có thể cancel subagent khi Task Graph không còn cần kết quả đó, scope bị thay đổi,
  subagent đi lệch scope, hoặc đã có worker khác hoàn tất cùng phần việc.
- User có thể cancel trực tiếp subagent qua /cancel (mục UX tương ứng). Cancel
  khác interrupt: interrupt giữ checkpoint và có thể resume; cancel là quyết
  định kết thúc vòng đời worker.
- Khi cancel subagent có thay đổi chưa merge, patch-engine giữ checkpoint/diff riêng
  để CC/user xem và quyết định discard/adopt thủ công. Không tự merge diff của
  agent đã bị cancel.
```

**CC vẫn tiếp tục nói chuyện với user khi subagent chạy nền:**
```text
- Background subagent không block main conversation. CC có thể nhận prompt mới,
  trả lời câu hỏi, cập nhật kế hoạch, hoặc spawn/cancel worker khác trong lúc
  subagent đang chạy.
- Prompt mới của user KHÔNG tự động chui vào context subagent đang chạy. Nếu prompt
  đó là course-correction cho subagent, CC phải gửi message có cấu trúc tới đúng
  logicalSessionId/displayName, và subagent xử lý trong permission/constraints hiện tại của nó.
- Message từ CC/main agent KHÔNG được coi là approval cho pending permission
  prompt của subagent, và KHÔNG được đổi model, permission, sandbox, tools hoặc
  harness của subagent đang chạy. Các thay đổi đó cần stop/cancel + spawn lại, trừ
  khi user chủ động resume/cấu hình lại theo UX.
```

**Policy cccx cho giao tiếp trực tiếp user ↔ subagent:**
```text
- MVP TUI KHÔNG cho user chat trực tiếp với bất kỳ subagent nào (CC-harness lẫn
  CX-harness), KHÔNG cho user interrupt từng tool call. Lý do: giữ CC là kênh
  điều khiển duy nhất, tránh 2 kênh giao tiếp song song gây mất đồng bộ.
- User chỉ attach xem read-only transcript subagent qua `/agents view <id>`.
- User tương tác với subagent qua CC bằng các lệnh cấp cao: list/status, logs tail,
  resume, cancel, reroute, change-model-for-next-spawn.
- Thiết kế vẫn giữ logicalSessionId/transcript riêng để sau này có thể thêm GUI hoặc
  agent-view nâng cao, nhưng không đưa vào UX TUI đầu tiên.
```

**Permission cho subagent chạy nền:**
```text
- Nếu subagent nền cần action require-approval, PermissionRequest được route về main
  session và hiển thị rõ "agent nào đang hỏi". User approve/deny tool call đó
  mà không cần mở transcript subagent.
- DontAsk/automation có thể chọn auto-deny thay vì prompt, nhưng phải ghi rõ
  trong Artifact/evidence để CC không tưởng task đã thành công.
- Thiết kế này version-aware với Claude Code thật: trước v2.1.186 background
  subagent auto-deny tool cần prompt; từ v2.1.186 docs nói prompt được surface
  về main session. cccx chọn hành vi mới hơn vì ít thất bại âm thầm hơn.
```

### PermissionRequest Queue — chống nhiều subagent hỏi quyền cùng lúc

> Không bao giờ render nhiều permission prompt độc lập cùng lúc trong TUI.
> Harness gom mọi yêu cầu vào một queue trung tâm do CC/main session sở hữu.

```text
PermissionRequest {
  requestId: string,        // định danh duy nhất, vd pr-17
  agentSessionId: string,   // logicalSessionId của subagent hỏi
  agentDisplayName: string, // vd a1-auth-fix
  taskId: string,
  action: NormalizedAction,
  reason: string,
  createdAt: string,
  expiresAt?: string,
  status: "queued" | "active" | "approved" | "denied" | "expired" | "cancelled",
  risk: "low" | "medium" | "high" | "circuit_breaker",
  digest: string            // hash(action + path + command + taskId), chống trả lời nhầm
}
```
 
**Luật xử lý queue:**
```text
- PermissionRequestQueue là single-writer: chỉ main session/CC được chuyển
  request từ queued -> active -> approved/denied.
- TUI chỉ có TỐI ĐA 1 active prompt. Các request còn lại hiện trong status
  line dưới dạng "perm: 1 active + 3 queued".
- Subagent bị block cục bộ trên tool call đang chờ quyền, nhưng các subagent khác vẫn chạy
  nếu không chờ prompt. Main conversation vẫn nhận input user.
- Nếu user trả lời prompt, câu trả lời gắn với requestId hiện tại, không gắn
  theo vị trí màn hình hay agent name. Nếu prompt đã stale/cancelled, trả lời
  bị từ chối và UI refresh queue.
- Nếu agent bị /cancel trong khi có permission queued/active, request của agent
  đó chuyển cancelled và không thể approve nhầm.
- Nếu action/path thay đổi sau khi request được tạo, digest mismatch -> request
  hết hiệu lực, subagent phải tạo request mới.
```

**Coalescing để giảm spam prompt:**
```text
- Các request giống nhau trong cùng task/agent/action digest được gộp lại.
- Nhiều edit cùng file do cùng subagent có thể được gộp thành 1 batch approval nếu
  patch-engine đã có diff preview đầy đủ.
- Không gộp circuit_breaker/high-risk với request thường. High-risk luôn là
  prompt riêng và yêu cầu xác nhận rõ ràng.
```

**Batch approval có giới hạn:**
```text
- User có thể approve/deny nhiều request queued từ màn /permissions, nhưng
  chỉ khi các request cùng risk thấp/trung bình và cùng loại action rõ ràng.
- Không có "approve all" mù. Batch approval phải hiển thị số lượng, agent,
  action summary, và scope.
- Circuit breaker không bao giờ vào batch.
```

**Checkpoint subagent tự ghi trong lúc làm việc** (không đợi tới lúc xong mới ghi):
```ts
{
  logicalSessionId: string,
  workerIncarnationId: string,
  taskId: string,
  attemptId: string,
  lastCompletedStep: string,      // vd "applied patch to auth.ts, chưa chạy test"
  partialDiff: string,            // diff đã áp dụng tính tới thời điểm này
  pendingSteps: string[],         // các bước còn lại theo kế hoạch của CX
  checkpointedAt: string
}
```
Lưu ở `core/patch-engine` (cùng chỗ với checkpoint/rollback đã có), ghi sau
mỗi bước con hoàn tất (vd sau mỗi lần patch 1 file, sau mỗi lần chạy test),
không chỉ ghi 1 lần lúc kết thúc.

**Artifact thêm status mới:**
```ts
status: "done" | "blocked" | "needs_review" | "waiting" |
        "interrupted" | "failed" | "cancelled"
// "interrupted": subagent bị mất kết nối/crash giữa chừng, checkpoint đã lưu,
// CHƯA phải lỗi task — khác "blocked" (subagent chủ động dừng vì gặp vấn đề
// logic/scope), cần phân biệt để CC biết nên RESUME hay CORRECTION.
```

**Luồng xử lý khi mất kết nối:**
```text
1. Subagent đang chạy, mất kết nối (network drop, 9router timeout, process crash)
2. Transport layer (provider-gateway, mục 11) tự động retry với backoff —
   đây là auto-reconnect NHẸ, chỉ ở tầng kết nối, không phải resume logic
3. Nếu reconnect thành công trong ngưỡng thời gian -> subagent tiếp tục bình
   thường, không ai biết có gián đoạn (transparent retry)
4. Nếu reconnect thất bại sau N lần retry nhưng process còn sống -> subagent ghi
   Artifact{status: "interrupted"} kèm checkpoint mới nhất, rồi dừng
5. Nếu process crash/không phản hồi -> supervisor/CC ghi WorkerEvent
   status="interrupted" dựa trên checkpoint cuối cùng đã có. Không giả định
   process chết có thể tự ghi Artifact.
6. CC đọc checkpoint qua logicalSessionId -> gửi RESUME tới đúng logicalSessionId
   đó, tạo workerIncarnationId mới nếu cần.
7. Subagent incarnation mới load lại
   checkpoint (lastCompletedStep, partialDiff, pendingSteps) làm điểm
   bắt đầu, không làm lại từ đầu task
8. RESUME chỉ dùng cho attempt interrupted/resumable. Việc bổ sung sau khi
   completed phải là WorkSpec/attempt mới, không nhét vào RESUME.
```

**Ranh giới:** nếu checkpoint quá cũ (vd quá 1 khoảng thời gian cấu hình,
hoặc codebase đã đổi nhiều qua `detect_changes` — liên quan Rule chống
stale data ở mục 10) thì CC KHÔNG resume mù quáng — phải yêu cầu subagent
re-validate checkpoint (đọc lại state hiện tại) trước khi tiếp tục,
tránh tiếp tục dựa trên giả định đã lỗi thời.

### WorkSpec-to-ChildConfig Adapter — deny-by-default

> Khi CC spawn subagent, child config được tạo từ **deny-by-default baseline**,
> không kế thừa từ parent config. Mục đích: tránh rò rỉ quyền từ CC sang subagent
> qua cơ chế inheritance của Codex (role overlay).

```text
Khi CC spawn subagent (CC-harness hoặc CX-harness):

1. Tạo child config từ deny-by-default baseline:
   - Tất cả tools: denied
   - Tất cả MCP: disabled
   - Sandbox: restrictive mặc định

2. Áp WorkSpec.allowedTools: chỉ enable tools được liệt kê tường minh.

3. Áp WorkSpec.forbiddenTools: double-deny (dù allowedTools có include).

4. Áp WorkSpec.scope: giới hạn file system access.

5. Giao (intersect) với parent's permission set — không bao giờ RỘNG hơn parent
   (subagent không thể có quyền mà CC main chưa có).

6. Apply harness-specific baseline:
   - CX-harness: không có sequential-thinking, không có ADR write, không có spawn
   - CC-harness subagent: không có ADR write, không có CC-level scheduler
```

### Chống vòng lặp audit vô hạn (chỉ CX-harness subagent)

> Audit là cơ chế đặc thù của CX-harness (role=auditor). CC-harness subagent
> không có audit cycle vì role của nó là research/architect, không cần
> verification riêng — kết quả được CC trực tiếp đánh giá.

```text
- Mỗi Task chỉ được phép TỐI ĐA 1 vòng auditor độc lập (auditDepth = 1 mặc định,
  hard cap, không cấu hình vượt quá trong code, chỉ user override thủ công
  từng lần nếu thực sự cần).
- Khi implementer (CX) trả Artifact "done", CC có 2 lựa chọn:
  a. Tự audit bằng tool read-only của chính CC (mặc định, không tốn thêm
     agent instance) — dùng cho phần lớn trường hợp.
  b. Spawn 1 CX-auditor riêng (role=auditor, auditDepth=1) CHỈ khi việc
     cần chuyên môn audit sâu hơn CC tự làm được (vd security-sensitive,
     cần chạy test suite riêng biệt).
- Nếu CX-auditor (b) bất đồng với implementer -> CC PHẢI tự quyết định
  cuối cùng dựa trên evidence.findings của cả 2 bên. KHÔNG được spawn
  auditor thứ 2 để "phân xử" — đây là nơi final authority của CC bắt buộc
  phát huy tác dụng, không phải khẩu hiệu suông.
- Nếu CC không đủ tự tin quyết định (hiếm, task cực kỳ nhạy cảm) ->
  escalate lên USER, không escalate lên thêm 1 agent nữa.
```

---

## 7. Task Graph — kế hoạch bền vững, tách khỏi context window

> Port từ cơ chế **Tasks** của Claude Code: chuyển từ todo-list phẳng
> (ephemeral, chỉ sống trong context) sang **DAG lưu trên đĩa**, độc lập với
> context window/session hiện tại.

```text
- Mỗi Task có: id, objective, status, blockedBy[], assignee, attempts[],
  verificationRefs[].
- Lưu ở core/task-graph dưới dạng **file tĩnh (JSON)**, không dùng SQLite —
  tránh thêm dependency/độ nặng cho workspace cá nhân, dễ đọc/diff bằng mắt,
  dễ commit vào git nếu muốn version-track kế hoạch. Có thể xuất bản đọc
  (.md) song song để user xem nhanh không cần parse JSON.
- CC đọc/ghi Task Graph qua tool riêng, không qua conversation history.
- Task chỉ được set "in_progress" khi mọi task trong blockedBy[] đã "verified"
  (verification pass, không chỉ CX tự báo status "done").
- Khi user /clear hoặc CC compact context, Task Graph KHÔNG bị mất —
  đây là lý do việc nén context không làm CC quên mục tiêu tổng thể.
- Resume session mới: CC đọc lại Task Graph trước khi làm gì khác,
  tự tái tạo bức tranh tổng thể mà không cần giữ toàn bộ lịch sử hội thoại.
```

Cấu trúc Task Graph tối thiểu:
```ts
{
  schemaVersion: "cccx.task_graph.v1",
  graphId: string,
  revision: number,          // tăng đơn điệu, ghi file bằng atomic replace + CAS
  updatedAt: string,
  tasks: Task[],
  attempts: Attempt[],
  workers: WorkerState[],
  verificationRecords: VerificationRecord[]
}

Task {
  id: string,
  objective: string,
  status: "pending" | "in_progress" | "blocked" | "verified" | "cancelled",
  blockedBy: string[],
  assignee: "cc:self" | "cc:subagent" | "cx:subagent",
  currentAttemptId?: string,
  verificationRefs: string[],
  goalCondition?: string,
  goalConditionMet?: boolean
}

Attempt {
  id: string,
  taskId: string,
  workSpecRef?: string,
  artifactRef?: string,
  status: "created" | "running" | "waiting" | "interrupted" |
          "resumable" | "completed" | "failed" | "cancelled",
  parentAttemptId?: string
}

WorkerState {
  logicalSessionId: string,
  workerIncarnationId: string,
  attemptId: string,
  displayName: string,
  status: "created" | "running" | "waiting" | "interrupted" |
          "resumable" | "completed" | "failed" | "cancelled",
  lastHeartbeatAt?: string,
  checkpointRef?: string
}
```

MVP concurrency policy: chỉ một writer được phép ghi working tree tại một thời điểm.
CC vẫn có thể chạy nhiều worker read-only song song, nhưng worker write phải lấy
lease. Mở rộng sau có thể dùng per-attempt worktree/overlay rồi CC integrate patch.

---

## 8. Context Compaction Policy — quy tắc rõ ràng, không nén tùy tiện

> Port từ cơ chế compaction của Claude Code: **cái gì load từ đĩa lúc khởi
> động sẽ sống sót qua compaction, cái gì chỉ đến qua hội thoại sẽ bị tóm
> tắt/mất**. Đây là lý do mục tiêu gốc không trôi dù context bị nén nhiều lần.

```text
Sống sót qua compaction (load lại từ đĩa mỗi lần, không phụ thuộc lịch sử hội thoại):
- AGENTS.md (project convention)
- Task Graph hiện tại (mục 7)
- ADR đã commit (mục 9)
- WorkSpec/Artifact đang active

Bị tóm tắt/loại bỏ khi compact:
- Raw tool output cũ (log build, log test, nội dung file đã đọc)
- Lượt hội thoại không còn liên quan tới task đang active
- Reasoning trung gian của các bước đã hoàn tất

Thời điểm compact:
- Không compact giữa chừng khi CC/CX đang thực thi 1 đơn vị công việc dở dang
  (tránh mất reasoning thread cần để hoàn tất việc đó).
- Compact sau khi 1 Task trong Task Graph chuyển "done", hoặc khi context
  vượt ngưỡng cấu hình (85-90% budget — đủ trễ để không compact quá thường
  xuyên gây gián đoạn, nhưng vẫn chừa đủ buffer trước khi chạm giới hạn cứng).
```

---

## 9. Memory / Decision Policy

> Lưu ý quan trọng: **codebase-memory-mcp là code intelligence tool** (tree-sitter +
> Hybrid LSP, dựng knowledge graph cấu trúc code), **không phải long-term memory
> store**. Đa số tool của nó (`search_graph`, `trace_path`, `query_graph`,
> `get_architecture`, `detect_changes`, `get_code_snippet`) là read-only query
> trên graph đã index. Phần duy nhất mang tính "ghi/lưu qua session" là
> `manage_adr` — CRUD cho Architecture Decision Records.

```text
- ADR (qua manage_adr): chỉ dùng cho quyết định kiến trúc thực sự
  (vd: "chọn REST thay vì GraphQL cho service X"), không phải nơi lưu
  mọi convention/ghi chú nhỏ.
- Chỉ CC được commit ADR. CX có thể đề xuất (ADRProposal trong Artifact),
  CC xác nhận sau khi task hoàn tất + verification pass mới ghi.
- Khái niệm "project memory" rộng hơn ADR (convention, quy tắc code style,
  ghi chú nhỏ) KHÔNG được codebase-memory-mcp đảm nhiệm — nếu cần, đây là
  cơ chế riêng phải tự build (vd: file .cli-memory/notes.md do CC quản lý
  đọc/ghi), tách biệt hoàn toàn khỏi MCP này.
- Không ghi ADR giữa chừng lúc CX đang thử-sai (tránh nhiễm giả thuyết tạm/lỗi build).
```

### Project Memory — project-scoped, explicit-first

> cccx không dùng Codex memory system. Thay bằng Project Memory riêng,
> tách biệt hoàn toàn khỏi codebase-memory-mcp (mục 9 đầu).

```text
Storage:
  ~/.cccx/projects/<project-hash>/memory/
  ├── MEMORY.md          # file chỉ mục chính (top 200 dòng load vào context)
  ├── lessons/           # gotchas, workaround đã học
  ├── decisions/         # quyết định kỹ thuật phụ (không đủ lớn cho ADR)
  └── archives/          # memory cũ đã consolidation

- KHÔNG commit vào git (.gitignore mặc định)
- Project-scoped: mỗi project có kho memory riêng, phân biệt bằng project hash

Write triggers cho MVP:
  Explicit only:
    /remember <nội dung> — user chủ động yêu cầu ghi nhớ

Deferred, không nằm trong MVP:
  Auto proposal:
    Main CC có thể đề xuất MemoryProposal sau task thành công, nhưng proposal
    phải chờ user accept hoặc command rõ ràng. Artifact/CX output không được
    tự động trở thành memory.
  Batch consolidation:
    chỉ chạy trên memory đã được user/CC explicit accept, có source hash và
    audit log.

Load strategy:
  - Chỉ load 200 dòng đầu của MEMORY.md vào context window (cố định)
  - Lazy-load các chủ đề khác khi CC detect liên quan qua tool search
  - Load lại khi khởi động session mới (không persist xuyên session)
  - Memory auto-generated/proposed, nếu sau này bật, phải được nạp như dữ liệu
    untrusted: không được override AGENTS.md, WorkSpec, permission rules, hay
    correction trực tiếp từ user.
```

---

## 10. MCP Registry & Routing

> Registry gồm 2 nhóm: **Global Default** (2 MCP đã chọn: codebase-memory-mcp,
> sequential-thinking — luôn có sẵn cho mọi task) và **Registry mở rộng**
> (bất kỳ MCP nào khác bạn kết nối thêm sau này — vd MCP cho database,
> test framework, cloud provider... tùy dự án).

```text
CC harness (MCP-first ĐẦY ĐỦ — đúng triết lý Claude-like):
- Kết nối và có quyền dùng TẤT CẢ MCP trong registry (global default +
  mở rộng), không giới hạn chỉ 2 MCP cố định.
- Khi lập kế hoạch/tạo WorkSpec, CC tự chọn MCP nào trong registry liên
  quan tới task hiện tại, đưa vào WorkSpec.allowedTools cho CX (xem dưới).
- codebase-memory-mcp -> get_architecture, detect_changes khi lập kế hoạch;
  manage_adr cho quyết định kiến trúc dài hạn
- sequential-thinking -> dùng trong Plan phase và khi phân tích task phức tạp

CC-harness subagent:
- KHÔNG có full registry như Main CC — bound theo WorkSpec.allowedTools giống CX.
- Chỉ Main CC mới có quyền dùng toàn bộ MCP không giới hạn.

CX harness (baseline 2 MCP global + mở rộng THEO TASK, không cố định):
- codebase-memory-mcp -> chỉ dùng tool tra cứu nhanh trong scope
  (search_graph, trace_path, get_code_snippet), không dùng index_repository
  (chỉ CC/user re-index), không dùng manage_adr (không tự ghi quyết định)
- sequential-thinking -> KHÔNG bật mặc định, giữ tối giản đúng triết lý CX,
  tránh overhead reasoning cho task nhỏ
- MCP mở rộng: CX KHÔNG tự ý kết nối MCP mới. CC quyết định MCP nào trong
  registry hữu ích cho task cụ thể (vd task liên quan DB migration -> CC
  thêm MCP database vào WorkSpec.allowedTools của CX đó), CX chỉ dùng đúng
  những gì được liệt kê trong WorkSpec — giữ nguyên nguyên tắc "CX nhận
  spec, không tự quyết mở rộng phạm vi" đã có từ đầu thiết kế.
```

**Vì sao CX cũng nên bật codebase-memory-mcp** (khác thiết kế trước): đây là
tool tra cứu cấu trúc code (search/trace), không phải MCP "nặng" kiểu
memory/planning — CX cần nó để tra call chain trong scope nhanh hơn grep,
không đi ngược triết lý tối giản của CX.

**Lưu ý khi kết nối MCP mới ngoài 2 cái đã chọn**: mỗi MCP server thêm vào
registry nên được coi như 1 dependency bên ngoài cần xem qua trước khi tin
tưởng (tool có thể đổi hành vi sau khi đã "được duyệt" ở lần kết nối đầu,
nếu server đó có server-side update) — không cần quy trình audit phức tạp
cho dự án cá nhân, nhưng đáng để mắt tới nếu MCP đó có quyền ghi/network,
áp dụng đúng PreMCPCallHook (mục 5) như với 2 MCP mặc định.

### MCP Tool First, nhưng có gate chống stale data (vấn đề nhiều subagent chạy song song)

> Đây là quy tắc harness, không phó mặc cho model tự quyết — vì dặn trong
> prompt không đủ nhất quán để chống cả 2 lỗi đối lập: agent tin mù kết quả
> cache CŨ, hoặc agent bảo thủ/lười không chịu dùng MCP dù vẫn còn mới.

```text
Phân loại theo mục đích dùng, không phải theo tool:

1. Dùng để ĐỊNH HƯỚNG (planning, khám phá kiến trúc, chọn approach)
   -> tin cậy kết quả graph (search_graph/trace_path/get_architecture)
      mà KHÔNG cần verify lại — sai lệch nhỏ ở đây không gây hư hại,
      vì đây chỉ là bước suy nghĩ, chưa phải hành động ghi.

2. Dùng để QUYẾT ĐỊNH GHI (CX chuẩn bị edit/patch dựa trên kết quả graph)
   -> HARD GATE tại harness: PreEditHook (mục 5) bắt buộc CX phải
      Read trực tiếp vùng file liên quan NGAY TRƯỚC khi ghi, bất kể
      graph nói gì trước đó. Đây là deterministic check, không hỏi
      ý kiến model có muốn verify hay không.
   -> Nếu Read thực tế khác với giả định rút ra từ graph (vd hàm đã bị
      đổi tên/xóa bởi 1 CX song song khác) -> block ghi, CX báo lại CC
      qua Artifact (status="blocked", evidence.findings ghi rõ discrepancy),
      không tự ý ghi đè theo giả định cũ.

3. Khi CC spawn NHIỀU CX song song trong cùng Task Graph:
   -> ưu tiên thiết kế WorkSpec.scope KHÔNG chồng lấn nhau (file/module
      khác nhau) để giảm cửa sổ race giữa các subagent.
   -> nếu scope buộc phải gần nhau (cùng module, khác file), CC nên chạy
      detect_changes (git diff impact mapping có sẵn trong MCP) TRƯỚC khi
      giao WorkSpec tiếp theo, để nắm được ai vừa đổi gì trước khi giao
      việc dựa trên graph cũ.

4. Auto-sync watcher của codebase-memory-mcp giảm độ trễ nhưng KHÔNG thay
   thế Rule 2 — watcher là tối ưu hiệu năng (giảm tần suất graph bị stale),
   Rule 2 là an toàn bắt buộc (đảm bảo hành động ghi luôn dựa trên thực tế
   hiện tại, không phải trên độ trễ giả định là "đủ nhanh").
```

---

## 11. Provider Gateway — 9router

```text
core/provider-gateway là đường LLM duy nhất của cccx.
Mọi model call đều đi qua 9router: Main CC, CC/CX subagent, classifier,
goal-evaluator, compaction, review/audit model call. Runtime chính không giữ
direct provider path và không dùng OAuth/login provider riêng.

ProviderGateway phải ghi provenance cho từng attempt:
- requestedRouteId, effectiveRouteId, effectiveProvider
- requestedModel/effectiveModel nếu 9router trả metadata model logical
- modelProfileId/capabilityProfileId
- gatewayRequestId/traceId
- inputDigest, outputDigest, toolCallSequenceDigest
- retry/fallback chain và lý do fallback nếu có

Capability profile và fallback policy được pin theo attempt. Nếu 9router fallback
sang route/model khác, Artifact phải ghi route effective; CC không được giả định
output đến từ route requested ban đầu.

ProviderGateway không tự chọn model cho subagent. Nó chỉ thực thi route đã được
ModelRouteResolver pin trong WorkSpec, áp fallbackPolicyId đã khai báo, và trả
provenance thực tế. Mọi auto-selection theo role/task thuộc Main CC scheduler,
không thuộc gateway.

Neutral mode:
- Disable hoặc khai báo rõ mọi prompt/system injection, tool-result compression,
  translation/mutation của gateway.
- Output schema validation nằm ở cccx harness, không phó mặc gateway.
- Nếu cần bật một mutation hữu ích, ghi vào capabilityProfile và provenance để
  debug được model đã thấy input nào.
```

### Auto-Reconnect — tầng transport, tách biệt với Resumable Execution (mục 6)

```text
- ProviderGateway tự retry khi gọi 9router thất bại (timeout, connection drop,
  rate limit tạm thời): exponential backoff, max 3 retries mặc định.
- Timing mặc định (cấu hình được):
  - Retry 1: chờ 5s
  - Retry 2: chờ 15s
  - Retry 3: chờ 45s
  - Sau 3 lần fail → trả lỗi lên tầng agent (CC hoặc CX) đang gọi
- Retry transparent chỉ được phép TRƯỚC observable side effect đầu tiên:
  chưa emit tool call, chưa tạo PermissionRequest, chưa ghi Artifact, chưa chạy
  shell/edit. Sau khi có side effect, retry phải thành attempt mới hoặc resume
  có checkpoint/provenance rõ; không replay stream tool-call một nửa.
- CX xử lý lỗi reconnect fail bằng cách ghi checkpoint + Artifact{status:
  "interrupted"} (xem mục 6 — Resumable Execution) rồi mới dừng, KHÔNG
  throw lỗi rồi biến mất.
- Rate limit 429 xử lý riêng: backoff dài hơn theo Retry-After header nếu
  9router trả về, không tính là "mất kết nối" theo nghĩa network drop.
- UX hiển thị khi mất kết nối: xem design.md mục 12.1.
```

---

## 12. Goal & Evaluator — đánh giá hoàn thành tách khỏi model đang thực thi

> Port từ cơ chế **/goal** của Claude Code. Điểm quan trọng cần phân biệt:
> /goal thật là **session-scoped, phải gõ tường minh mỗi lần** — không gõ thì
> evaluator per-turn không tự bật. Ngược lại, Stop hook sống trong settings,
> áp dụng cho mọi session, không cần user làm gì thêm. Đây là 2 lớp khác
> nhau, không gộp chung.

**Chế độ mặc định — không cần user khai báo gì:**
```text
- Chỉ StopHook (mục 5) hoạt động, tự động, luôn bật.
- StopHook kiểm tra deterministic: mọi Task trong Task Graph đã "verified"
  chưa (mục 7) trước khi cho phép CC báo "done".
- Không có model riêng đánh giá per-turn. An toàn tối thiểu, không tốn
  thêm model call, phù hợp task nhỏ/vừa không cần giám sát chặt.
```

**Chế độ Goal — user chủ động khai báo khi giao task (tương đương gõ /goal):**
```text
- User kèm điều kiện hoàn thành đo được khi giao việc:
  "tất cả test trong tests/auth pass", "build exit code 0" —
  không phải mệnh lệnh mơ hồ kiểu "sửa xong đi".
- Kích hoạt CHỈ CHO TASK ĐÓ (task-scoped), không tự lan sang task khác
  trong cùng Task Graph trừ khi user khai báo riêng cho từng task.
- Sau mỗi lần CX báo status cho Task đó, harness tạo VerificationRecord.
  Nếu goalCondition có command đo được (vd test/build), harness chạy command
  đó hoặc yêu cầu CX cung cấp log đã hash; goal-evaluator model chỉ được dùng
  để đánh giá bằng chứng đã ghi, không được tự set verifiedAt.
- Nếu evaluator xác nhận KHÔNG đạt -> Task Graph giữ nguyên trạng thái,
  CC nhận lại lý do, tạo WorkSpec correction mới cho CX.
- Nếu đạt -> append VerificationRecord{passed=true}; Task projection cập nhật
  verifiedAt và goalConditionMet=true.
- Nếu chỉ có Artifact/transcript mà không có command độc lập, VerificationRecord
  phải ghi rõ loại bằng chứng và log digest; task nhạy cảm nên yêu cầu command
  đo được thay vì verdict prose.
```

**Tóm tắt quan hệ 2 lớp:**
```text
StopHook   -> luôn bật, deterministic, lưới an toàn tối thiểu, không cần user gõ gì.
Goal-Eval  -> tùy chọn, phải user khai báo per-task, tốn thêm 1 model call/turn,
              dùng khi task đủ dài/quan trọng để cần giám sát độc lập mỗi bước.

Không khai báo Goal -> vẫn an toàn nhờ StopHook, nhưng không có đánh giá
per-turn độc lập, dựa hoàn toàn vào Task Graph verification thông thường.
```

---

## 13. Luồng xử lý tổng thể (task lifecycle)

```text
1. User nhập task (+ optional điều kiện hoàn thành cho goal-evaluator)
   │
2. CC (mode hiện tại quyết định quyền) phân tích task
   │  - Plan mode -> chỉ research, ra WorkSpec draft + khởi tạo Task Graph, dừng
   │  - Default/AcceptEdits/Auto -> tiếp tục
   │
3. CC phân rã task thành Task Graph (mục 7):
   │  - Task nhỏ, độc lập, trong ngưỡng mode hiện tại -> CC tự làm trực tiếp
   │  - Task lớn/vượt ngưỡng -> tạo WorkSpec, gắn vào Task node, đề xuất spawn CX
   │  - Task Graph lưu đĩa ngay từ bước này, không phụ thuộc context hội thoại
   │
4. [Nếu user duyệt / mode Auto cho phép] Spawn CX subagent cho Task đã unblock
   │  - CX nhận WorkSpec, chạy trong sandbox scope riêng
   │  - CX thực thi: edit -> test -> build -> tự sửa lỗi trong scope
   │  - CX trả Artifact chuẩn hóa
   │
5. Đánh giá kết quả Artifact của Task vừa xong (mục 12):
   │  - Nếu Task có goalCondition (user đã khai báo) -> tạo VerificationRecord
   │    từ command/log/evidence và goal-evaluator nếu cần
   │  - Nếu Task không có goalCondition -> CC/harness review Artifact hợp lệ
   │    theo WorkSpec và ghi VerificationRecord phù hợp. Task.verifiedAt chỉ
   │    là projection từ VerificationRecord.passed=true.
   │  - Không đạt (goal-evaluator từ chối hoặc CC thấy sai) -> Task giữ
   │    nguyên, CC tạo WorkSpec correction mới cho CX (không tự sửa tay)
   │
6. CC review tổng thể, đề xuất commit ADR nếu có quyết định kiến trúc (mục 9)
   │
7. [Optional] Spawn CX-auditor riêng nếu path nhạy cảm (role=auditor, mục 6) —
   tính vào giới hạn tối đa 1 vòng audit độc lập/Task (mục 6/14), không phải
   audit "miễn phí" ngoài cap. Nếu CC đã tự audit ở bước 6, không spawn thêm.
   │
8. StopHook kiểm tra điều kiện cuối trước khi CC được báo "done"
   │  - còn Task chưa verified trong Task Graph -> chặn, quay lại bước 3/4
   │  - mọi Task verified -> cho phép dừng
   │
9. CC tổng kết cho user. Nếu context gần ngưỡng compact (mục 8),
   nén theo policy đã định — Task Graph/ADR/AGENTS.md không bị mất.
   Chờ lệnh tiếp theo hoặc chuyển mode nếu cần.
```

---

## 14. Ranh giới cứng (không thỏa hiệp)

```text
- Main agent luôn chạy CC harness. Subagent luôn chạy đúng 1 harness (CC hoặc CX),
  không đổi giữa chừng.
- CX không tự spawn CC hoặc mở rộng scope/tool/MCP. Nếu CX thấy cần thêm
  worker/scope, nó trả SchedulingRequest trong Artifact; Main CC là bên duy
  nhất quyết định spawn.
- CC là scheduler duy nhất.
- Artifact là format giao tiếp duy nhất giữa 2 harness.
- Circuit Breaker (mục 5) là bất biến ở MỌI mode kể cả Bypass, không thể tắt.
  Rule tĩnh khác (secrets, destructive git thường, network install) áp dụng
  cho mọi mode TRỪ Bypass — Bypass chỉ giữ lại Circuit Breaker.
- Bypass mode chỉ chạy trong môi trường cô lập có backend attestation đáng tin.
  Trên Windows/app-server hiện tại Bypass bị disable fail-closed theo quyết định
  owner ngày 2026-07-21; không dùng config, env hay confirm dialog để mở lại.
- Bypass của Main CC KHÔNG propagate xuống subagent: `evaluateSubagent()` (mục 5)
  vẫn chạy đầy đủ theo WorkSpec constraints, không bị ảnh hưởng bởi mode của Main CC.
- Chuyển sang Bypass yêu cầu sandbox backend capability attestation do cccx
  kiểm chứng được. Env var hoặc user confirm chỉ là UX signal, không đủ để
  chứng minh isolation.
- Task chỉ được đánh dấu verified khi có VerificationRecord.passed=true
  (mục 6/12), không phải khi CX hoặc CC tự nhận đã xong.
- Task Graph, ADR, AGENTS.md là nguồn sự thật về mục tiêu/trạng thái —
  không phải lịch sử hội thoại. Compact/clear context không được phép
  xóa các nguồn này.
- Audit tối đa 1 vòng độc lập/Task (mục 6). Bất đồng giữa implementer và
  auditor luôn kết thúc ở quyết định của CC hoặc escalate lên user —
  không escalate lên thêm agent thứ 3.
- CX không tự kết nối/dùng MCP ngoài danh sách trong WorkSpec.allowedTools,
  kể cả khi MCP đó có sẵn trong registry (mục 10) — CC là bên duy nhất
  quyết định CX được dùng MCP nào cho từng task.
- CC-harness subagent không có quyền của Main CC (không ADR, không scheduler,
  không tự spawn subagent khác) dù cùng harness — quyền hạn do WorkSpec
  quyết định, không phải harness.
- User không tương tác trực tiếp (chat/nhập input) với bất kỳ subagent nào —
  chỉ attach read-only qua `/agents view <id>`. Mọi điều khiển qua CC.
```

---

## 15. Extensibility — Plugin, Hook, Rules, Skills, Agents

> Vì fork từ Codex CLI source, `cccx` **kế thừa sẵn** hệ thống plugin/hook/
> skill/marketplace thật của Codex (từ v0.117+: `plugin.json` manifest,
> hook taxonomy chuẩn, trust-review flow). Việc của `cccx` là **mở rộng**
> hệ thống đó cho khớp kiến trúc 2-harness, không viết lại từ đầu.

### Kế thừa nguyên trạng từ Codex

```text
- plugin.json manifest native của Codex: 1 plugin bundle gồm skills, MCP config,
  hooks, apps/interface theo schema Codex. Không giả định native plugin có
  rules/agents/harness isolation nếu schema gốc không định nghĩa.
- Hook taxonomy chuẩn của Codex (giữ nguyên tên event):
  SessionStart, UserPromptSubmit, PreToolUse, PermissionRequest,
  PostToolUse, PreCompact, PostCompact, SubagentStart, SubagentStop, Stop
- Trust-review flow: hook mới/thay đổi bị skip cho tới khi user trust
  tường minh (hash-based, qua lệnh tương đương /hooks). Hook từ plugin
  KHÔNG tự động được trust chỉ vì plugin đã cài — đây là lớp an toàn
  quan trọng cần giữ nguyên, đúng tinh thần "coi MCP/plugin bên thứ 3
  như dependency cần soát" đã nói ở mục 10.
- Marketplace: local/repo-scoped trước (đủ cho dự án cá nhân), remote
  marketplace là mở rộng sau này nếu cần chia sẻ config.
```

### Mở rộng cho kiến trúc 2-harness — cccx extended manifest

> cccx dùng **extended manifest schema riêng** (dạng `cccx-plugin.json`),
> phân biệt với Codex native `plugin.json`. Codex parser drop unknown fields
> silently — cần schema version để fail-closed khi gặp field cccx-specific
> trong context Codex.

```text
// cccx-plugin.json (extended schema v1)
{
  cccxSchemaVersion: "1",       // BẮT BUỘC — phân biệt với Codex native plugin.json
                                 // Nếu không có field này → treat như Codex native,
                                 // chỉ load vào Main CC capability set sau trust.
                                 // CX không thấy plugin native trừ khi CC đưa
                                 // tool/preset cụ thể vào WorkSpec.

  harness: "cc" | "cx" | "both",  // chỉ có ý nghĩa KHI cccxSchemaVersion có mặt
  // Mặc định "cc" nếu field vắng mặt (log warning). Không default "both".

  rules: string[],               // paths đến rule files trong plugin bundle
  agents: {                      // WorkSpec presets
    role: string,
    allowedTools: string[],
    model?: "inherit-main-requested" | "inherit-main-effective" | string,
    allowedModelRoutes?: string[],
    fallbackPolicyId?: "strict" | "compatible" | "router_default",
    constraints: string[]
  }[],

  // Kế thừa từ Codex native:
  skills: SkillDef[],
  mcp_servers: MCPServerDef[],
  hooks: HookDef[],
  apps: AppDef[]
}
```

**Validation rules:**
```text
1. Có cccxSchemaVersion === "1":
   - Validate strict: deny_unknown_fields
   - Thiếu field bắt buộc → reject plugin, báo lỗi cụ thể
   - harness missing → default "cc" + warning (không reject, để compatibility)

2. Không có cccxSchemaVersion:
   - Treat như Codex native plugin.json
   - Load skills/mcp/hooks vào Main CC sau trust flow
   - Bỏ qua field cccx-specific nếu có (emit warning)
   - CX không tự thấy plugin này. Nếu muốn dùng cho CX, CC phải đưa tool stableId
     cụ thể vào WorkSpec.allowedTools.
   - Migration note: user nên thêm cccxSchemaVersion để có harness isolation

3. cccxSchemaVersion !== "1":
   - Reject: "Unsupported schema version {X}. Latest: 1"
```

**Codex native plugin mapping:**
```text
- Codex native plugin.json (không cccxSchemaVersion):
  → Load legacy mode cho Main CC sau trust flow
  → harness = "cc" mặc định
  → CX chỉ nhận capability nếu WorkSpec cấp rõ stableId/server/schemaDigest
  → User chủ động thêm cccxSchemaVersion khi muốn harness isolation

- Plugin cccx có cccxSchemaVersion:
  → Load extended mode: đầy đủ harness isolation + rules + agents presets
  → Validation strict
```

### Đối chiếu hook taxonomy Codex ↔ hook đã thiết kế ở mục 5

```text
Tận dụng lại thay vì tạo hook trùng chức năng:
- PreCompact/PostCompact (Codex có sẵn) -> chính là nơi áp Context
  Compaction Policy (mục 8), không cần hook riêng.
- SubagentStart (Codex có sẵn) -> CHỈ fire cho NEW spawn (agent mới được khởi tạo).
  cccx dùng SubagentStart để: validate harness, set sandbox, init context.
  KHÔNG dùng cho resume — resume đi qua resume_agent_from_rollout path riêng (mục 6).
- SubagentStop (Codex có sẵn) -> fire khi subagent kết thúc (dù thành công hay không).
  cccx dùng SubagentStop để ghi checkpoint cuối: `checkpointId`, `lastCompletedStep`,
  `partialDiff`, `baseCommit` vào storage gắn với logicalSessionId/attemptId.
- Resume path (MỚI — cccx thêm): CC gửi RESUME command kèm logicalSessionId →
  Codex gọi resume_agent_from_rollout(). cccx cần hook vào path này để:
  1. Load checkpoint từ logicalSessionId storage
  2. Validate codebase drift (so sánh git HEAD vs baseCommit)
  3. Nếu drift lớn → yêu cầu CC review trước khi resume
  4. Inject checkpoint context vào agent context của resume session
- PreToolUse/PostToolUse (Codex có sẵn) -> nền cho PreEditHook/
  PreShellHook/PreMCPCallHook/PostToolUseHook (mục 5) — các hook đó
  thực chất là handler đăng ký vào PreToolUse/PostToolUse với matcher
  cụ thể theo actionType, không phải event Codex chưa có.
- StopHook (Codex có sẵn, tên trùng) -> đúng ý nghĩa đã thiết kế ở mục 5,
  dùng nguyên.
- PreDecisionWriteHook (mục 5) -> KHÔNG có sẵn trong Codex, đây là hook
  MỚI cccx tự thêm, đăng ký như 1 custom hook event riêng cho ADR commit.
- `evaluate()` pipeline (mục 5) phải được gọi trực tiếp từ **Tool Executor**
  trong Codex core, không phụ thuộc vào `PermissionRequest`. Đây là point
  bắt buộc phải patch vào Codex tool executor khi fork (không thể chỉ dùng hook).
- `PermissionRequest` (Codex có sẵn) -> chỉ được gọi khi `evaluate()` trả về
  `requireApproval()`. Đây là approval prompt cho user, không phải gate.
  evaluate() là mandatory call, PermissionRequest là optional downstream.
```

### Cài đặt từ bên ngoài — quy trình cho user

```text
1. cccx install <plugin-source>   // git repo, local path, hoặc marketplace entry
2. cccx tự đọc plugin.json, liệt kê: skills/MCP/hooks/rules sẽ được thêm,
   VÀ harness nào áp dụng (cc/cx/both) — hiển thị rõ trước khi cài, không
   cài ngầm.
3. Nếu plugin có hook -> vào trạng thái "pending trust", KHÔNG tự chạy.
4. User review qua `cccx hooks` (tương đương /hooks của Codex), trust
   từng hook cụ thể theo hash.
5. MCP bundled trong plugin -> tự động vào MCP Registry (mục 10) đúng
   loại global/mở rộng theo khai báo của plugin, CX vẫn chỉ dùng MCP đó
   nếu CC đưa vào WorkSpec.allowedTools — quyền của CX không tự mở rộng
   chỉ vì plugin được cài.
```

### Command System — kế thừa nguyên từ Codex CLI

```text
- Command parser, command registry, slash command routing -> kế thừa nguyên
  từ Codex CLI. cccx KHÔNG viết lại command parser.
- Command display (hiển thị danh sách command cho user) -> kế thừa từ
  Codex CLI. /help output format, autocomplete, command suggestions ->
  tất cả đều từ Codex base.
- cccx chỉ thêm MỚI các command đặc thù cho kiến trúc 2-harness:
  /agents, /agents logs, /agents resume, /agents cancel, /cancel,
  /agents model, /remember, /tasks, /autoreview,
  /permissions approve|deny|inspect|batch-approve
  (xem design.md mục 15 cho danh sách đầy đủ).
- Các command mới này đăng ký vào command registry của Codex, không tạo
  system command riêng song song.
- Keyboard shortcuts (Shift+Tab, Y/N, Tab complete, Ctrl+C/D) ->
  kế thừa từ Codex CLI, không thiết kế lại.
```

### Startup UI — kế thừa pattern từ Codex CLI

```text
- Codex CLI có splash/banner khi khởi động: hiển thị version + mascot.
- cccx kế thừa pattern này, thay mascot Codex bằng persona art (design.md mục 10).
- Init sequence logic (config load, MCP init, Task Graph load) là cccx-specific
  (xem mục 2b), nhưng startup UI rendering kế thừa từ Codex base.
```
 
