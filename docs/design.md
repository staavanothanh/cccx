# cccx — Thiết kế Giao diện & Tương tác (UX Layer)

> Layer này gồm 2 loại nội dung khác nhau về mức độ ảnh hưởng đến kiến trúc:

> **Interaction Contract** — phải đồng bộ với `architecture.md` khi sửa:
> command grammar, approval flow, mode transition, headless output schema,
> secure input protocol, resume semantics, permission queue UX.
>
> **Visual Design** — sửa tự do, không ảnh hưởng logic lõi:
> màu sắc, border, spacing, persona art, animation, layout thẩm mỹ.

---

## 1. Nguyên tắc thiết kế

```text
1. Terminal-first: mọi trạng thái quan trọng phải đọc được bằng mắt trong
   1-2 dòng, không cần mở file/log riêng để hiểu chuyện gì đang xảy ra.
2. Không che giấu quyết định harness: user luôn thấy đang ở mode nào,
   CC hay CX đang chạy, có bao nhiêu subagent đang hoạt động.
3. Nhân vật/mascot là lớp trang trí ngoài cùng — tắt được hoàn toàn
   (--no-persona hoặc config) mà không mất bất kỳ thông tin chức năng nào.
4. Mọi thứ hiển thị trong TUI phải có phiên bản plain-text tương đương
   cho headless mode (cccx exec, CI/script) — không phụ thuộc màu/icon
   để truyền thông tin.
5. Thiên hướng thẩm mỹ Claude Code, không phải Codex: semantic color
   token (không hardcode RGB), panel bo góc nhẹ nhàng thay vì terminal
   thô, nhưng KHÔNG đánh đổi độ nhẹ — không dùng framework TUI nặng,
   không animation phức tạp, chỉ 1 spinner + màu có ý nghĩa là đủ.
```

---

## 2. Kế thừa từ Codex CLI (Legacy Boundary)

> File này chỉ thiết kế phần **cccx-specific** — component kế thừa nguyên từ Codex CLI base không được thiết kế lại. Mục này ghi rõ ranh giới để không design lại những gì đã có, đồng thời tránh conflict khi implement.

### 2.1 Giữ nguyên từ Codex CLI

```text
Chat view
  - User/assistant message streaming
  - Code block rendering + syntax highlight theme
  - User input area (multi-line editing)
  - Chat history scroll

Built-in tools display
  - File tree picker
  - Search result display (grep output format)
  - Diff view (edit workflow)
  - Error/build output formatting

Basic command infrastructure
  - Command parser
  - Autocomplete (file paths, basic commands)
```

### 2.2 Modified từ Codex CLI (design.md cover)

```text
Status bar → status line với mode/harness/tasks/ctx/model (mục 5 UI)
Permission dialog → permission panel bo góc (mục 7 UI)
Agent display → subagent listing với logicalSessionId + displayName (mục 13 UI)
Theme → semantic token system thay vì RGB cứng (mục 4 UI)
```

### 2.3 Mới hoàn toàn (cccx-only, design.md cover)

```text
Mode state machine (6 mode) display & switching UX (mục 8 UI)
Task Graph DAG với /tasks command (mục 6 UI)
Subagent permission queue (mục 13 UI)
Goal evaluator UX (mục 9 UI)
Audit escalation panel (mục 11 UI)
Persona system (mục 10 UI)
Startup/Shutdown sequence (mục 10 UI)
Help system (mục 15 UI)
Network error / reconnect UX (mục 12 UI)
Secure input mode (mục 16 UI)
Subagent attach view (read-only) (mục 13 UI)
Font policy (mục 3 UI)
```

### 2.4 Nguyên tắc sửa đổi

```text
- Không thiết kế lại component đã ghi ở mục 2.1 — kế thừa nguyên
- Nếu cần sửa component ở mục 2.1, cập nhật mục này và ghi rõ lý do
- Claude Code aesthetic chỉ áp dụng ở layer cosmetic (màu, border, spacing),
  không thay đổi layout logic của Codex CLI
- Khi nâng cấp Codex CLI upstream, ưu tiên giữ modified/new layers
  không bị ghi đè bởi upstream changes
```

---

## 3. Terminal Compatibility & Font Policy

### 3.1 Approach

cccx theo approach của Claude Code và Codex CLI: **require modern terminal cho
interactive TUI, nhưng vẫn có plain/headless contract machine-readable.**

```text
- Không có --no-unicode flag (MVP)
- Không có ASCII fallback per-glyph trong interactive TUI
- Không auto-detect terminal capability để đổi glyph
- Yêu cầu terminal hỗ trợ Unicode BMP + true color
```

### 3.2 Ký tự Unicode được dùng

```text
Ký tự an toàn (tested trên Windows Terminal, iTerm2, Kitty, VSCode terminal):
  ╭╮╰╯    → box drawing rounded (Claude Code style)
  ┌┐└┘    → box drawing square (chỉ dùng nếu sau này thêm plain interactive mode)
  ●       → bullet
  ✓ ✗     → check / cross
  ⚠       → warning
  ▶       → expand
  →       → arrow
  ─       → horizontal line (U+2500)
  ~       → tilde
  └       → tree branch

Ký tự cần TRÁNH:
  ⎯       → U+23AF (Horizontal Line Extension) — nhiều font thiếu
            Dùng ─ (U+2500) thay thế
  ⏸       → U+23F8 (Pause) — không phổ biến
            Dùng [S] (suspended) thay thế

Fallback mode labels (khi không dùng màu):
  mode.plan        → [P]    mode.default     → [D]
  mode.acceptEdits → [A]    mode.auto        → [G]
  mode.dontAsk     → [N]    mode.bypass      → [R!]
```

### 3.3 Terminal requirements

```text
Minimum:
  - Terminal hỗ trợ Unicode BMP
  - Windows: Windows Terminal (không support legacy cmd.exe/conhost)
  - macOS: Terminal.app hoặc iTerm2
  - Linux: hầu hết terminal hiện đại (GNOME Terminal, Konsole, Kitty, Alacritty)
  - VS Code terminal: OK

Khuyến nghị:
  - Nerd Font hoặc font có Unicode coverage đầy đủ
    (FiraCode, JetBrains Mono, Cascadia Code)
  - True color support (24-bit)
```

### 3.4 Windows-specific notes

```text
- Phát hiện Windows Terminal qua biến môi trường WT_SESSION
- Legacy conhost (cmd.exe, PowerShell ISE) không support rounded box drawing
  → Nếu chạy trên legacy, UI có thể hiển thị sai — known limitation
  → Không implement fallback, user nên nâng cấp lên Windows Terminal
- Cần SetConsoleMode với ENABLE_VIRTUAL_TERMINAL_PROCESSING flag
  để ANSI escape sequence hoạt động trên Windows
```

---

## 4. Theme System — semantic token, không hardcode màu

> Port từ cách Claude Code làm: màu không gắn cứng vào UI element, mà
> map qua **semantic key** (ý nghĩa) → theme quyết định key đó ra màu gì.
> Nhẹ hơn Codex ở chỗ Codex dùng RGB cứng trong core, khó theme lại.

```text
Semantic key (cố định, không đổi theo theme):
  mode.plan, mode.default, mode.acceptEdits, mode.auto, mode.dontAsk, mode.bypass
  permission.approve, permission.deny, permission.pending
  status.success, status.error, status.warning
  diff.added, diff.removed
  subagent.1 .. subagent.8   // 8 slot màu xoay vòng cho subagent chạy song song;
                              // màu KHÔNG phải định danh duy nhất, chỉ là
                              // visual aid. Định danh ổn định là displayName/id.

Theme có sẵn:
  auto   -> tự dò background sáng/tối của terminal, chọn dark/light tương ứng
  dark   -> RGB đầy đủ, cho terminal hỗ trợ true color
  light  -> RGB đầy đủ, nền sáng
  ansi   -> semantic color 16 màu cho terminal vẫn render Unicode được
  high-contrast -> cho người khó phân biệt màu, tăng độ tương phản, có
                    thể kèm ký hiệu chữ thay vì chỉ dựa màu (mục 3 UI)

Lệnh: /theme  -> mở picker chọn theme, hoặc tạo custom theme (override
                  từng semantic key riêng, không cần đổi cả bộ)
Cấu hình: ~/.cccx/theme.json — mỗi key map tới 1 giá trị màu (hex hoặc
          tên ANSI), người dùng override cái nào cũng được, không cần
          định nghĩa lại toàn bộ theme.
```

**Vẫn giữ "nhẹ"**: theme chỉ là 1 bảng tra cứu (lookup table) áp lên text đã render sẵn, không cần dựng lại toàn bộ UI như 1 framework component-based nặng — khác biệt so với Claude Code (dùng Ink/React fork) nhưng vẫn giữ được lợi ích thẩm mỹ chính: **màu có ý nghĩa nhất quán, không phải trang trí ngẫu nhiên**.

---

## 5. Status Line — luôn hiển thị, 1 dòng cố định ở đáy terminal

```text
[cccx] mode:AcceptEdits  harness:CC  model:claude-sonnet-4  tasks:3/7  ctx:62%  ───  (?) thinking...
        │                 │           │                      │           │              │
        mode hiện tại      ai đang     model name (CC/CX)    Task Graph  context        trạng thái 
        (dùng token        chạy        — lấy từ 9router      progress    budget           hiện tại
         mode.* ở mục 4 UI)               config                         (mục 8 kiến trúc)
```

**Giải thích các field:**

```text
model       → model đang dùng cho main CC harness
              Nếu CC dùng model khác classifier (mục 5 architecture),
              hiển thị cả hai: model:sonnet|classifier:haiku
              Nếu CX subagent spawn với model khác, không hiện ở status line
              (xem /agents logs để biết model của từng CX)
```

**Màu mode dùng đúng semantic key đã định nghĩa ở mục 4** (không hardcode lại ở đây — theme quyết định giá trị màu thật, bảng dưới chỉ là default gợi ý cho theme `dark`):
```text
mode.plan        -> xanh dương nhạt   (ký hiệu fallback ANSI: [P])
mode.default     -> trắng/mặc định    (ký hiệu: [D])
mode.acceptEdits -> vàng              (ký hiệu: [A])
mode.auto        -> cam               (ký hiệu: [G])   // G = Go, tránh trùng A
mode.dontAsk     -> tím               (ký hiệu: [N])   // N = No-ask
mode.bypass      -> đỏ, viền panel dày hơn hẳn các mode khác (không dùng
                     nhấp nháy — nhấp nháy khó chịu và không nhẹ; thay vào
                     đó dùng độ tương phản/độ dày viền để nhắc nhở liên tục)
```

**Khi có subagent đang chạy** — mỗi subagent có 2 định danh:

```text
1. logicalSessionId -> định danh nội bộ, không đổi, dùng cho resume/cancel/log.
2. displayName  -> tên ngắn do harness sinh tất định để user đọc nhanh,
                   vd a1-auth-fix, a2-review-auth, a9-test-refresh.
```

Màu chỉ là lớp phụ trợ. Mỗi CX được gán 1 trong **8 màu xoay vòng**
(`subagent.1`..`subagent.8`), gán theo thứ tự spawn. Nếu có hơn 8 subagent chạy
song song, màu được tái sử dụng nhưng displayName/id vẫn khác nhau, nên UI
không phụ thuộc màu để phân biệt subagent:

```text
[cccx] mode:Auto  harness:CC  model:claude-sonnet-4  tasks:3/7  ctx:58%
  └─ ● a1-auth-fix     CX implementer  "fix login timeout"      [running, 45s]  (subagent.1)
  └─ ● b2-research     CC researcher   "explore auth API patterns" [running, 22s]  (subagent.2)
  └─ ● a2-auth-audit   CX auditor      "review auth.ts patch"     [interrupted, resumable] (subagent.3)
```
Ký hiệu `●` tô theo màu `subagent.N` riêng của từng subagent — cùng 1 subagent luôn giữ
nguyên màu suốt vòng đời (kể cả sau resume ở mục 12 UI). Khi màu bị tái sử dụng
do >8 subagent, UI luôn in displayName trước role để tránh nhầm.
Format hiển thị chung: `<displayName>  <harness role>  "<objective>"` — áp dụng cho
cả CC-harness (`b2-research` `CC researcher`) lẫn CX-harness (`a1-auth-fix` `CX implementer`).

### Subagent displayName policy

```text
- Harness sinh displayName, không để LLM tự đặt tên tự do.
- Format mặc định: <shortId>-<role/topic>, ví dụ a1-auth-fix.
- shortId tăng theo session hiện tại, không tái dùng trong cùng Task Graph.
- role/topic lấy từ WorkSpec.role + objective, slug hóa ASCII, giới hạn độ dài.
- Custom Codex-style agent có thể khai báo nickname_candidates; cccx chỉ dùng
  chúng làm nickname trình bày nếu không làm mất tính duy nhất. Source of truth
  vẫn là logicalSessionId + WorkSpec.agent name.
```

Ghi chú xác thực: Codex docs hiện có `nickname_candidates` trong custom agent
TOML. Đây là pool nickname hiển thị cho spawned agents; Codex vẫn định danh
custom agent bằng field `name`. Vì vậy cccx nên tận dụng ý tưởng nickname
trình bày, nhưng không dựa vào màu hoặc nickname làm identity ổn định.

Dòng phụ chỉ hiện khi có ≥1 CX đang active — không chiếm chỗ khi làm việc đơn giản (main agent tự làm, không spawn ai).

---

## 6. Hiển thị Task Graph (mục 7 kiến trúc)

Không bắt buộc phải mở riêng — gõ `/tasks` để xem dạng cây theo `blockedBy`:

```text
$ /tasks

Task Graph — "Refactor auth module"
├─ [✓] T1  Explore current auth flow              (cc:self, verified)         ← Main CC tự làm
├─ [✓] T2  Design new token refresh strategy       (cc:researcher, verified)   ← CC-harness subagent
├─ [~] T3  Implement refresh logic                 (cx:implementer, in_progress)
│     └─ blocks: T4, T5
├─ [ ] T4  Write tests for refresh logic            (cx:tester, blocked by T3)
└─ [ ] T5  Audit security of new flow                (cx:auditor, blocked by T3)

2/5 verified · 1 in progress · 2 blocked
```
Ký hiệu: `[✓]` done+verified, `[~]` in_progress, `[ ]` pending, `[!]` blocked (lỗi/cần correction), `[S]` interrupted (chờ resume, fallback từ ⏸).

Notation phân biệt actor:
```text
cc:self           → Main CC tự làm việc trực tiếp
cc:<role>         → CC-harness subagent (researcher/architect/planner)
cx:<role>         → CX-harness subagent (implementer/tester/auditor/resolver)
```

Headless mode: `cccx tasks --format=json` trả đúng cấu trúc Task Graph thô, không có ASCII tree.

---

## 7. Xác nhận quyền (permission prompt) — theo đúng permission-gate mục 5 kiến trúc

Khi `evaluate()` trả `require-approval`, hiển thị đủ thông tin để quyết định nhanh, không phải đọc lại toàn bộ context:

```text
┌─ Cần xác nhận ──────────────────────────────────────────────────────┐
│ Hành động : edit src/auth/session.ts (47 dòng thay đổi)             │
│ Lý do gate: vượt ngưỡng AcceptEdits (diff > 40 dòng)                │
│ Ai yêu cầu: CC (main agent)                                         │
│ Đề xuất   : spawn CX-implementer thay vì CC tự sửa trực tiếp        │
├─────────────────────────────────────────────────────────────────────┤
│ [Y] Cho phép CC tự sửa   [S] Spawn CX   [D] Xem diff   [N] Từ chối  │
└─────────────────────────────────────────────────────────────────────┘
```
Circuit Breaker (mục 5 kiến trúc) hiển thị khác — không có lựa chọn approve.
Panel chỉ giải thích hành động bị hard-deny, rule nào khớp, và cách sửa task
an toàn hơn. Nếu sau này thêm break-glass flow, nó phải là cơ chế riêng ngoài
Circuit Breaker mặc định.

---

## 8. Chuyển mode — cả phím tắt lẫn slash command (đã chốt ở kiến trúc mục 4)

```text
Phím tắt: Shift+Tab  -> cycle qua Plan / Default / AcceptEdits / Auto
          (DontAsk, Bypass KHÔNG nằm trong cycle nhanh — phải gõ lệnh
           tường minh, tránh lỡ tay bấm nhầm vào mode rủi ro cao)

Slash command — direct mode switch:
  /plan              -> chuyển Plan
  /default           -> chuyển Default
  /accept-edits      -> chuyển AcceptEdits
  /auto              -> chuyển Auto
  /dont-ask          -> chuyển DontAsk (hỏi xác nhận 1 lần)
  /bypass --confirm  -> chuyển Bypass (bắt buộc flag --confirm + isolation attestation, không cycle được)
```

Mỗi lần chuyển mode, status line cập nhật ngay + in 1 dòng log ngắn:
```text
→ mode: AcceptEdits → Auto  (classifier động bật từ đây)
```

---

## 9. Goal & StopHook UX (mục 12 kiến trúc)

```text
$ cccx "fix all failing tests in tests/auth" --goal "tests/auth pass, npm test exits 0"

[cccx] goal set: "tests/auth pass, npm test exits 0"
[cccx] mode:AcceptEdits  harness:CC  goal:▶ active

... (làm việc) ...

[cccx] goal-evaluator: checking... ✓ CONDITION MET (verified via test log)
[cccx] Task T3 verified. StopHook: 5/5 tasks verified → cho phép dừng.

Hoàn tất. 1 goal đã đạt, 5 task verified, 0 vòng audit cần thiết.
```
Nếu goal-evaluator từ chối, in rõ lý do (từ `evidence.findings`), không chỉ "chưa đạt":
```text
[cccx] goal-evaluator: ✗ NOT MET — 2/15 test vẫn fail (xem test log dòng 340)
[cccx] → tạo WorkSpec correction cho CX#a1, tiếp tục...
```

---

## 10. Startup & Persona

### 10.1 Startup Sequence

Khi cccx khởi động (persona enabled):

```text
 /\_/\    cccx v0.1.0 — CC/CX harness
( •.• )   mode:Plan — ready
 > ^ <    D:\code\cccx
```

3 dòng. Persona art bên trái, startup info bên phải.
Horizontal layout — khác Claude Code (art centered, info below) và Codex (banner full-width).
Persona expression `( •.• )` là default, thay đổi theo state (deferred — xem mục 10.2).

Khi persona disabled — bỏ 3 dòng art, chỉ hiện 3 dòng info (version, mode, folder).

Không prefix [cccx], không progress lines.
Các bước init (config load, 9router check, MCP init) chạy ngầm.
Nếu 9router offline → startup banner không được ghi "ready" mơ hồ; hiển thị
`model:offline` hoặc dòng lỗi ngắn trên stderr. Interactive vẫn mở được để user
nhập lại key/retry, nhưng mọi model call bị block cho tới khi ProviderGateway OK.

Persona config: ~/.cccx/persona.json (enabled, name, states — xem mục 10.2).
Init logic chi tiết: xem architecture.md mục 2b.

### 10.2 Persona System (tùy chọn, thuần trang trí)

```text
~/.cccx/persona.json
{
  "name": "Shin",
  "enabled": true,
  "states": {
    "thinking":  "Shin · thinking…",
    "waiting":   "Shin · awaiting approval",
    "done":      "Shin · done",
    "error":     "Shin · needs attention"
  }
}
```

Text ngắn, cố định (không random array). Hiển thị ở cuối status line, thay thế spinner mặc định.

**Expression mapping** (deferred — implement sau):

```text
State     Expression   Art
──────────────────────────────────────
default   ( •.• )      /\_/|  ( •.• )  > ^ <
thinking  ( -.- )      /\_/|  ( -.- )  > ^ <
waiting   ( o.o )      /\_/|  ( o.o )  > ^ <
done      ( ^.^ )      /\_/|  ( ^.^ )  > ^ <
error     ( x.x )      /\_/|  ( x.x )  > ^ <
```

MVP: chỉ hiện expression `( •.• )` mặc định cho mọi state.
Expression switching theo state → implement sau khi core stable.

- `cccx config set persona.enabled false` hoặc flag `--no-persona` tắt hoàn toàn, quay về spinner ASCII trung tính (`⠋⠙⠹...`).
- Headless mode (`cccx exec`) tự động tắt persona, không cần flag — vì output ở đó cần machine-parseable, không nên lẫn câu thoại.
- Ở startup, persona text không thay spinner (startup dùng horizontal layout — xem mục 10.1).

---

## 11. Confirm thoát khỏi audit loop / escalate lên user (mục 6 kiến trúc)

Khi CC không tự tin quyết định sau khi CX-auditor bất đồng với implementer (đã đạt cap 1 vòng audit, mục 6/14 kiến trúc):

```text
┌─ CC cần bạn quyết định ─────────────────────────────────────────┐
│ CX-implementer báo: "done"                                      │
│ CX-auditor báo: "conflict — hàm processPayment thiếu rollback   │
│                  khi transaction fail (severity: critical)"     │
│                                                                 │
│ CC không tự tin tự quyết vì đây là path nhạy cảm (payment).     │
│ Đã đạt giới hạn 1 vòng audit/Task — không audit thêm.           │
├─────────────────────────────────────────────────────────────────┤
│ [A] Chấp nhận theo implementer   [B] Chấp nhận theo auditor     │
│ [C] Xem cả 2 evidence.findings chi tiết   [R] Yêu cầu sửa lại   │
└─────────────────────────────────────────────────────────────────┘
```
Đây là điểm UX quan trọng nhất của toàn hệ thống — nơi duy nhất mà thiết kế "final authority của CC" thực sự chuyển thành "final authority của user" khi CC tự nhận không đủ tự tin. Giao diện phải làm nổi bật, không chìm trong log thông thường (border riêng, không auto-scroll qua).

---

## 12. Network Error & Resume UX (mục 6, 11 kiến trúc)

### 12.1 Network Error (9router connection loss)

Khi 9router timeout giữa session:

```text
[cccx] ⚠ 9router: connection lost (timeout in 30s)
[cccx] ⚠ Auto-retry in 5s... (attempt 2/3)
```

Sau 3 lần thất bại:

```text
[cccx] ✗ 9router: offline after 3 retries
       → [R]etry / [S]witch model / [Q]uit & save session
```

Cơ chế:
```text
- Exponential backoff: 5s → 15s → 45s
- Auto-retry tối đa 3 lần, sau đó chờ user action
- Không mất conversation history (lưu trong session state,
  không phụ thuộc network)
- Nếu CX subagent đang chạy khi mất kết nối:
  checkpoint tự động → resume sau khi network恢复
```

### 12.2 Reconnect / Resume (checkpoint)

```text
[cccx] ⚠ CX#a1 mất kết nối (9router timeout). Đang tự động thử lại... (2/3)
[cccx] ⚠ CX#a1 mất kết nối. Đã lưu checkpoint tại bước "applied patch to auth.ts".
        → gõ /agents resume a1 để tiếp tục, hoặc /agents resume a1 --revalidate nếu nghi codebase đã đổi
```

`/agents resume <displayName>` gọi đúng flow RESUME đã thiết kế (mục 6 kiến trúc) — không tạo WorkSpec mới. Nếu checkpoint quá cũ, `cccx` tự đề xuất thêm flag `--revalidate` thay vì âm thầm resume mù.

---

## 13. Subagent control UX — TUI nhẹ, không chat trực tiếp với subagent

MVP cố ý KHÔNG có transcript chat trực tiếp user ↔ subagent và KHÔNG có interrupt
từng tool call của subagent (dù CC-harness hay CX-harness).
Lý do: **giữ CC là kênh điều khiển duy nhất**, tránh 2 kênh giao tiếp song song
gây mất đồng bộ Task Graph và bypass permission pipeline.

User chỉ **attach xem read-only** transcript subagent qua `/agents view <id>` (xem dưới),
không nhập input trực tiếp. Mọi điều khiển subagent qua CC bằng command cấp cao:

```text
/agents
  Liệt kê mọi subagent đang chạy, interrupted/resumable, completed gần đây.

/agents view <displayName|logicalSessionId>
  Attach xem read-only transcript của subagent. Không nhận input.
  Esc để detach, quay về main conversation.

/agents logs <displayName|logicalSessionId> [--tail 80]
  Xem log tóm tắt/tail đã sanitize, không mở full transcript vào main context.

/agents resume <displayName|logicalSessionId> [--revalidate]
  Resume đúng subagent attempt nếu trạng thái resumable và chưa cancelled.

/agents cancel <displayName|logicalSessionId>
  Hủy subagent. Nếu có diff/checkpoint chưa merge, UI báo rõ và giữ lại để xem.

/cancel <displayName|logicalSessionId>
  Alias ngắn cho `/agents cancel <id>`.

/cancel --all
  Hủy mọi subagent đang chạy sau xác nhận.

/reroute <displayName|logicalSessionId> --role tester
  Không đổi harness/model của subagent đang chạy; thao tác này cancel worker cũ
  hoặc đợi nó dừng, rồi spawn worker mới với WorkSpec mới.

/agents model <agent-preset> <route|inherit-main-requested|inherit-main-effective>
  Đổi model route cho lần spawn tiếp theo của preset/role đó, không mutate
  subagent đang chạy giữa chừng. <route> là opaque 9router route id.
```

### 13.1 Model route UX/config cho subagent

Subagent model selection là quyết định của CC scheduler, không phải subagent tự đổi
model trong lúc chạy. UI chỉ cho đổi policy cho lần spawn tiếp theo hoặc tạo
WorkSpec/attempt mới.

Resolve order hiển thị cho user:

```text
explicit spawn override
→ cccx config role/preset
→ agent-suggested route nếu allowlist cho phép
→ session default_subagent_model
→ inherit-main-requested
```

Config tối thiểu (`~/.cccx/config.toml` hoặc project override `.cccx/config.toml`):

```toml
[models]
default_subagent_model = "inherit-main-requested"
fallback_policy = "compatible"

[agents.cx_implementer]
model = "inherit-main-requested"

[agents.cx_auditor]
model = "9router/anthropic/claude-sonnet-4.5"

[agents.cc_researcher]
model = "9router/openai/gpt-5-mini"
```

UX rules:

```text
- Status line chỉ hiện model route của Main CC. Nếu subagent dùng route khác,
  xem /agents hoặc /agents logs.
- /agents list hiển thị route rút gọn và selectionSource khi subagent khác main.
- /agents model <preset> inherit-main-requested quay preset về default kế thừa main.
- /agents model <preset> <route> validate route với 9router catalog/allowlist trước
  khi lưu config; nếu offline thì ghi pending và validate ở spawn, không spawn mù.
- Nếu 9router fallback, logs hiển thị requested → effective để debug cost/chất lượng.
- Headless JSONL cũng phải emit requested/effective route cho mọi subagent event,
  không chỉ interactive `/agents logs`.
```

Ví dụ `/agents`:

```text
Agents — 3 active · 1 resumable
● a1-auth-fix      CX implementer  "fix login timeout"             running      45s  scope: src/auth/*  model:inherit-main
● b2-research      CC researcher   "explore auth API patterns"     running      22s  scope: src/auth/*  model:gpt-5-mini
● a2-auth-audit    CX auditor      "review auth.ts patch"          resumable    checkpoint: reviewed session.ts  model:sonnet-4.5

Commands: /agents view a1-auth-fix · /agents logs b2-research · /agents cancel a1-auth-fix · /agents resume a2-auth-audit
```

Khi subagent nền cần permission:

```text
╭─ Cần xác nhận cho subagent nền ────────────────────────────────────╮
│ Request  : pr-17                                                   │
│ Agent    : a1-auth-fix (CX implementer)                            │
│ Hành động: edit src/auth/session.ts (28 dòng thay đổi)             │
│ Lý do    : AcceptEdits threshold exceeded for background worker    │
│ Queue    : 3 request khác đang chờ                                 │
├────────────────────────────────────────────────────────────────────╯
│ [Y] Cho phép tool call này   [D] Xem diff/checkpoint   [N] Từ chối │
╰────────────────────────────────────────────────────────────────────╯
```

Từ chối permission không hủy subagent; subagent nhận denial và có thể recover hoặc trả
Artifact `blocked`. Muốn kết thúc worker phải dùng `/cancel`.

### Permission Queue UX — không chồng modal khi nhiều subagent hỏi quyền

TUI chỉ render **1 permission prompt active**. Các request khác vào queue và
được báo ở status line:

```text
[cccx] mode:Auto  harness:CC  model:claude-sonnet-4  tasks:3/7  ctx:58%  perm: pr-17 active + 3 queued
```

Gõ `/permissions` để mở danh sách:

```text
Permission Queue — 1 active · 3 queued

ACTIVE
> pr-17  a1-auth-fix      edit   src/auth/session.ts       medium  28 lines

QUEUED
  pr-18  a9-refresh-test  shell  npm test -- tests/auth    medium  read output
  pr-19  a4-doc-update    edit   docs/auth.md              low     12 lines
  pr-21  a8-session-test  shell  npm test -- tests/session medium  read output
  pr-20  a6-cleanup       shell  git clean -fd             high    destructive

Actions:
  /permissions approve pr-17
  /permissions deny pr-17
  /permissions inspect pr-17
  /permissions batch-approve pr-18 pr-21
```

Rules trình bày:

```text
- Không có nhiều box permission cùng lúc, không render prompt xen kẽ với log subagent.
- Prompt active luôn hiển thị requestId, agent displayName, action, scope,
  risk và số request đang queued.
- User trả lời bằng phím Y/N chỉ áp dụng cho prompt active hiện tại.
- Với nhiều request, khuyến khích dùng command có requestId:
  /permissions approve pr-17, /permissions deny pr-18, /permissions inspect pr-20.
- Nếu request stale/cancelled/digest mismatch, UI từ chối approval và refresh:
  "pr-17 no longer valid; action changed, inspect new request pr-21".
- High-risk/circuit breaker dùng panel riêng, không được batch approve.
```

Batch approval chỉ dùng cho nhóm request an toàn, cùng ngữ cảnh và cùng action class:

```text
╭─ Batch approval ───────────────────────────────────────────────────╮
│ 2 requests selected                                                │
│ pr-18  a9-refresh-test  shell  npm test -- tests/auth    medium    │
│ pr-21  a8-session-test  shell  npm test -- tests/session medium    │
│                                                                    │
│ Không trộn shell/edit/network khác loại; không bao gồm high-risk.   │
├────────────────────────────────────────────────────────────────────╯
│ [A] Approve selected   [I] Inspect individually   [N] Cancel batch │
╰────────────────────────────────────────────────────────────────────╯
```

---

## 14. Output convention chung

```text
- Diff luôn hiện dạng unified diff chuẩn (+/-), có syntax highlight nếu
  terminal hỗ trợ, fallback plain nếu không.
- Mọi quyết định harness quan trọng (spawn CX, chuyển mode, escalate)
  đều in 1 dòng log ngắn bắt đầu bằng "→", không lẫn vào output nội dung.
- Cảnh báo (StopHook chặn, Circuit Breaker, goal-evaluator từ chối) luôn
  có prefix riêng biệt: "⚠" cho cảnh báo, "✗" cho từ chối/thất bại,
  "✓" cho xác nhận thành công — nhất quán xuyên suốt mọi màn hình.
- Không dùng emoji ngoài bộ ký hiệu tối giản trên — tránh nhiễu và đảm
  bảo render đúng trên mọi terminal/font.
- Panel/box luôn dùng ký tự bo góc (`╭╮╰╯`), không dùng góc vuông
  (`┌┐└┘`) — đây là điểm khác biệt thẩm mỹ chính so với Codex, giữ
  nhất quán trên mọi màn hình có khung (permission prompt, escalate
  confirm, /theme picker...).
```

---

## 15. Help System

### 15.1 Cách dùng

```text
/help                  → List tất cả commands
/help <command>        → Chi tiết command + flags + examples
/help --shortcuts      → Keyboard shortcuts cheat sheet
```

### 15.2 Command list

```text
Mode & Session:
  /plan                  → Chuyển Plan mode
  /default               → Chuyển Default mode
  /accept-edits          → Chuyển AcceptEdits mode
  /auto                  → Chuyển Auto mode
  /dont-ask              → Chuyển DontAsk mode (có xác nhận)
  /bypass --confirm      → Chuyển Bypass mode (cần --confirm + isolation attestation)
  /status                → Session config + token usage
  /model                 → Chọn model (nguồn: 9router config)
  /theme [name]          → Chọn theme / mở picker
  /clear                 → Clear terminal
  /keymap                → Remap phím tắt
  /vim                   → Toggle Vim mode
  /new                   → Tạo session mới
  /rename                → Rename session
  /archive               → Archive session
  /delete                → Xóa session
  /exit, /quit           → Thoát (confirm nếu subagent đang chạy)

Task & Goal:
  /tasks                 → Task Graph tree view
  /goal "..."            → Set goal evaluator condition
  /review                → Review changes
  /compact               → Compact context window
  /stop                  → Dừng background terminal process

Agent Control:
  /agents list           → List subagents đang chạy/resumable
  /agents view <id>      → Attach xem read-only transcript subagent (Esc detach)
  /agents resume <id>    → Resume subagent từ checkpoint (CC validate safety)
  /agents cancel <id>    → Cancel subagent
  /cancel [--all]        → Cancel subagent (alias cho agents cancel)
  /agents logs <id>      → Xem log tóm tắt của agent
  /reroute <id> --role X → Đổi role của agent
  /agents model <X> <Y>  → Đổi model route cho lần spawn tiếp theo

Memory:
  /remember <nội dung>   → Ghi vào Project Memory explicit-first
  ( /import — phát triển sau )   → Import config/state đa nguồn
  ( /ide — phát triển sau )      → IDE integration

Permissions:
  /permissions           → Permission queue (active + queued)
  /permissions approve <requestId>        → Approve permission request (digest check)
  /permissions deny <requestId>           → Deny permission request
  /permissions inspect <requestId>        → Inspect action/diff/checkpoint
  /permissions batch-approve <ids...>     → Batch approve (không gồm high-risk)

Utilities:
  /diff                  → Show git diff
  /copy                  → Copy response ra clipboard
  /raw                   → Toggle raw output mode
  /side /btw             → Side conversation trên main thread
  /init                  → Create AGENTS.md
  /pets                  → Chọn/hide persona (cat)
  /personality           → Chọn giọng điệu (trỏ vào persona system)
  /autoreview            → CC direct verify (gắn với verifiedAt)
  /hooks                 → Inspect/toggle lifecycle hooks
  /mention               → Attach file vào context (@mention)
  /title                 → Set terminal title
  /mcp                   → List MCP servers + tools
  /skills                → List và invoke skills
  /plugins               → Plugin picker
  /resume                → Load session cũ đã save (không dùng cho subagent)
  /help                  → This page

Nhóm 4 — tạm ẩn (is_visible = false, giữ code):
  /elevate-sandbox, /sandbox-add-read-dir, /fork, /app
  /apps, /usage, /debug-config, /statusline, /rollout
  /test-approval, /ps

Phát triển sau:
  /import                → Import config/state
  /ide                   → IDE integration

Loại bỏ (không port):
  /multi-agents          → Thay bằng Task Graph luôn bật
  /agent (singular)      → Không port. Dùng /agents logs/model/... hoặc /agents view.
  /memories, MemoryDrop, MemoryUpdate → Dùng Project Memory thay thế
  /experimental          → Không cần feature flags runtime
```

### 15.3 Keyboard Shortcuts

```text
Shift+Tab              → Cycle mode: Plan → Default → AcceptEdits → Auto
                         (DontAsk, Bypass không nằm trong cycle)
Y / N                  → Approve / Deny permission prompt (active request)
Up / Down              → Command history (input area)
Tab                    → Auto-complete commands, file paths
Ctrl+C                 → Cancel current output / interrupt
Ctrl+D                 → Exit (nếu input trống)
Esc                    → Detach khỏi subagent attach view (khi đang `/agents view <id>`)
```

### 15.4 Hiển thị help output

```text
$ /help

Commands:
  Mode:  /plan /default /accept-edits /auto /dont-ask /bypass
         /status /model /theme /clear /keymap /vim
         /new /rename /archive /delete /quit /exit
  Tasks: /tasks /goal /review /compact /stop
  Agents: /agents list /agents view /agents resume /agents cancel
          /cancel /agents logs /reroute /agents model
  Memory: /remember
  Perms: /permissions approve|deny|inspect|batch-approve
  Utils: /diff /copy /raw /side /btw /init /pets
         /personality /hooks /autoreview /mention /title
         /mcp /skills /plugins /resume /help

Gõ /help <command> cho chi tiết.
Gõ /help --shortcuts cho phím tắt.
```

`/help --shortcuts` hiển thị bảng phím tắt dạng ngắn.

---

## 16. Secure Input Mode

Khi cccx cần nhập thông tin nhạy cảm (API key, token, secret):

```text
Enter 9router API key:
████████░░░░░░░░░░  (echo off, ký tự ẩn)
```

Quy tắc:

```text
1. Echo off: không hiển thị ký tự đã gõ, hiện ██ block thay thế
2. Không lưu input vào command history (Up arrow không recall)
3. Không ghi input vào session log / telemetry
4. Chỉ dùng cho: API key setup, token nhập, config secret
5. Input được giữ trong memory process, không persist dạng plaintext
6. Khi nhập xong, clear dòng input ngay (không để visible echo)
```

Các trường hợp dùng:
```text
- Thiết lập API key 9router lần đầu (setup wizard)
- /config set <key> — 2-step flow: (1) user gõ key, (2) TUI chuyển secure input mode
- Nhập token cho MCP server yêu cầu auth
```

**2-step secure input flow:**
```text
1. User gõ: /config set <key>         // chỉ key, KHÔNG có value inline
2. TUI nhận command, switch sang secure input mode:
   - Prompt: "Enter value for <key>: "
   - Raw TTY no-echo (không hiển thị ký tự, không buffer vào history)
   - Input KHÔNG đi qua command parser buffer
3. Value xử lý trong memory, lưu vào OS keyring (codex-rs/secrets)
4. Confirm: "✓ <key> saved securely"

Giá trị KHÔNG bao giờ xuất hiện trong:
- Command history (Up arrow không recall)
- TUI parser buffer
- Session transcript
- Log/telemetry
```

---

## 17. Headless / exec Contract

> Kế thừa từ Codex `exec/lib.rs`: stdout/stderr separation pattern.
> stdout = final answer, `--json` = JSONL, rest → stderr.

```text
cccx exec headless contract:

stdout:
  - Chỉ final answer (text) — hoặc JSONL events khi --json
  - KHÔNG có banner, spinner, prompt, progress lines

stderr:
  - Interactive: spinner, progress, prompt tương tác
  - Headless plain: diagnostics/error ngắn, không có spinner/progress
  - Headless --json: chỉ fatal bootstrap error trước khi JSONL writer sẵn sàng
    mới đi stderr; sau đó error phải là JSONL event trên stdout

--json flag:
  - Mọi output là JSONL (Newline-Delimited JSON)
  - Mỗi dòng là JSON object có envelope versioned:
    {
      "schemaVersion": "cccx.exec_event.v1",
      "sessionId": "s_...",
      "turnId": "t_...",
      "seq": 12,
      "type": "answer|approval-denied|task-update|agent-update|permission-request|error|terminal",
      "payload": {}
    }
  - Event terminal luôn là dòng cuối:
    { "type": "terminal", "payload": { "status": "success|error|cancelled", "exitCode": 0 } }

agent-update payload tối thiểu:
  {
    "logicalSessionId": "a1...",
    "displayName": "a1-auth-fix",
    "role": "cx:implementer",
    "status": "running|waiting|interrupted|resumable|completed|failed|cancelled",
    "modelRoute": {
      "selectionSource": "inherit_main_requested|role_preset|user_override|agent_suggested|global_default",
      "requestedRouteId": "9router/...",
      "effectiveRouteId": "9router/...",
      "fallbackChain": []
    }
  }

Exit codes:
  0 — success
  1 — internal error
  2 — approval denied (user denied a critical permission)
  3 — cancelled (user cancelled the operation)

Approval-denied trong headless:
  - Không interactive prompt → emit JSONL event { type: "approval-denied" }
  - Không block chờ user response — deny mặc định
```
