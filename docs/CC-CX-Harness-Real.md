Prompt: tôi nhận thấy claude code và codex có 2 lớp harness coding agent với những triết lí phát triển hoàn toàn khác nhau. hãy tìm hiểu và cho tôi biết lớp harness của claude code và codex có những gì?

Ở đây mình dùng **“harness”** theo nghĩa: lớp runtime bao quanh model để biến model thành coding agent — nó cấp context, tool, quyền, sandbox, workflow, hook, subagent, review và cơ chế áp dụng patch. Codex thậm chí mô tả rõ agent nhận “context provided by the harness” và phát tool call để chạy lệnh terminal / apply patch. 

## 1. Claude Code harness gồm những gì?

**Claude Code = agent terminal/IDE giàu lifecycle và policy.** Nó chạy trên terminal, IDE, desktop app và web; mục tiêu chính là đọc codebase, sửa file, chạy command và tích hợp với dev tools. 

| Lớp | Thành phần trong Claude Code |
|---|---|
| **Surface/runtime** | Terminal CLI, VS Code/JetBrains, desktop app, web/mobile approval UI. |
| **Tool layer** | `Read`, `Grep`, `Glob`, `Edit`, `Write`, `Bash`, `WebFetch`, `WebSearch`, `NotebookEdit`, `Agent` subagent, `Artifact`, `AskUserQuestion`, cron/scheduled tasks, v.v. Claude docs nói tool name dùng trực tiếp trong permission rules, subagent tool lists và hook matchers; custom tool đi qua MCP, workflow lặp lại đi qua Skill.  |
| **Context / memory** | `CLAUDE.md`, `.claude/rules/`, auto memory. Hai hệ thống `CLAUDE.md` và auto memory đều được load đầu mỗi conversation; `CLAUDE.md` là instruction do người viết, auto memory là learning do Claude tự ghi từ corrections/preferences.  |
| **Permission layer** | Fine-grained allow/ask/deny rules. Deny → ask → allow, và rule được **Claude Code enforce**, không phải model tự hứa làm theo.  |
| **Permission modes** | `default/manual`, `acceptEdits`, `plan`, `auto`, `dontAsk`, `bypassPermissions`. `acceptEdits` tự chấp nhận sửa file trong working dir; `plan` cho Claude explore/plan nhưng không edit source; `auto` dùng classifier model để review action; `bypassPermissions` bỏ prompt trừ một số circuit breaker.  |
| **Sandbox layer** | Permissions áp dụng cho toàn bộ tool; sandbox là OS-level enforcement cho `Bash` và child processes, giới hạn filesystem/network. Claude khuyến nghị dùng cả hai: deny rule ngăn agent thử, sandbox ngăn command vượt boundary nếu prompt injection xảy ra.  |
| **Hooks/lifecycle** | Hook có thể là shell command, HTTP endpoint hoặc LLM prompt, chạy tại lifecycle event như `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PermissionRequest`, `PostToolUse`, `SubagentStart/Stop`, `PreCompact`, `ConfigChange`, v.v.  |
| **Skills/custom commands** | `SKILL.md` thêm workflow prompt-based, chỉ load body khi dùng nên tiết kiệm context; bundled skills có `/debug`, `/code-review`, `/batch`, `/loop`, `/claude-api`, v.v.  |
| **Subagents** | `Agent` tool spawn subagent có context riêng; có thể restrict tools, deny writes, scope MCP server, preload skills, và set `permissionMode`. Subagents inherit tool/MCP từ parent trừ vài tool phụ thuộc UI/session state.  |
| **Config/managed policy** | `settings.json` theo user/project/local/managed; Claude Code watch settings và hot-reload permissions/hooks. Managed settings có precedence cao nhất và có thể khóa MCP, hooks, permission rules, sandbox domains, marketplace, models.  |

**Triết lý của Claude Code harness:** rất “programmable agent runtime”. Nó cho bạn cắm hook ở gần như mọi điểm của vòng đời agent, định nghĩa rule theo tool/call, mở rộng bằng Skill/MCP/subagent, và để memory sống trong repo/user/org. Nói ngắn: **Claude Code coi agent như một tiến trình cộng tác có lifecycle, policy, memory và event bus.**

## 2. Codex harness gồm những gì?

**Codex = bounded executor/workspace agent.** Codex CLI được mô tả là coding agent chạy local từ terminal, có thể inspect, edit, run code trong selected directory; nó cũng có IDE extension, desktop app, web/cloud. 

| Lớp | Thành phần trong Codex |
|---|---|
| **Surface/runtime** | CLI, IDE extension, ChatGPT desktop app, ChatGPT web, Codex cloud. CLI có `/init`, `/status`, `/permissions`, `/model`, `/review`.  |
| **Core tool loop** | Model nhận prompt/context từ harness, stream response, update plan, gọi terminal commands và apply patch. OpenAI’s open-source Codex prompt còn chỉ rõ dùng `apply_patch` để edit file, ưu tiên minimal/focused changes, không commit trừ khi user yêu cầu.  |
| **Repo instruction layer** | `AGENTS.md`. Codex đọc `AGENTS.md` trước khi làm việc; có global scope trong `~/.codex`, project scope từ git root tới cwd, file gần cwd override file ở root vì được concat sau. Default cap cho combined project docs là 32 KiB.  |
| **Sandbox + approval** | Hai lớp chính: sandbox mode quyết định agent được làm gì về mặt kỹ thuật, approval policy quyết định khi nào phải hỏi trước khi vượt boundary.  |
| **Local sandbox** | CLI/IDE dùng OS-level sandbox; mặc định không có network và chỉ write trong active workspace. Auto preset cho phép read/edit/run trong working dir, nhưng hỏi approval khi edit ngoài workspace hoặc cần network.  |
| **Cloud sandbox** | Codex cloud chạy trong isolated OpenAI-managed containers; setup phase có thể có network để cài dependency, agent phase offline mặc định trừ khi bật internet; secrets chỉ có trong setup và bị remove trước agent phase.  |
| **Permission profiles** | Beta permission profiles gom filesystem rules và network rules thành named profile. Built-in có `:read-only`, `:workspace`, `:danger-full-access`; enterprise admin có thể restrict profile qua managed requirements.  |
| **Network controls** | Network mặc định tắt trong `workspace-write`; có thể bật `sandbox_workspace_write.network_access`, và có network proxy/domain allow/deny để constrain traffic của spawned commands/subprocesses.  |
| **Auto-review** | Thay human approval ở sandbox boundary bằng reviewer agent riêng. Main agent vẫn ở cùng sandbox/approval policy/network/filesystem limits; reviewer chỉ quyết định escalation request có được chạy không.  |
| **Skills/plugins/MCP** | Codex CLI docs nhấn mạnh đóng gói repeatable instructions thành skills, thêm plugins để nối team tools/data, có MCP để connect external tools.  |
| **Subagents** | Có built-in agents như `default`, `worker`, `explorer`; custom agents nằm dưới `~/.codex/agents/` hoặc `.codex/agents/`. Subagents inherit sandbox policy và permission mode từ parent; approval từ inactive thread vẫn có thể surface trong CLI.  |
| **Review workflow** | `/review` review uncommitted changes, commit hoặc base branch; Codex báo prioritized findings mà không modify working tree.  |

**Triết lý của Codex harness:** “workspace-bound autonomous executor”. Codex đặt **sandbox/approval/workspace boundary** làm primitive trung tâm, rồi chuẩn hóa cách agent edit bằng patch, chạy test/tool trong repo, review diff, và handoff local ↔ cloud. Nói ngắn: **Codex coi agent như một worker chạy trong môi trường bị giới hạn rõ ràng, có audit trail qua commands/diffs/approvals.**

## 3. Khác biệt cốt lõi

| Trục | Claude Code | Codex |
|---|---|---|
| **Đơn vị kiểm soát chính** | Tool permission + hook lifecycle + settings/memory. | Sandbox boundary + approval policy + workspace/cloud environment. |
| **Cách mở rộng** | Hooks rất sâu, Skills, MCP, subagents, auto memory, project/user/org settings. | AGENTS.md, config.toml, skills/plugins/MCP, custom agents, auto-review, cloud env. |
| **Cách an toàn** | Permission rules enforce bởi Claude Code; sandbox là defense-in-depth cho Bash. | Sandbox là boundary chính; approvals quyết định khi nào vượt boundary. |
| **Cách làm việc** | Tương tác nhiều, có plan mode, hỏi user, hook tự động hóa lifecycle. | Patch/command/review-centric, hợp với local repo, CI, cloud delegation. |
| **Subagent philosophy** | Subagent như extension của tool/lifecycle, có tool allow/deny, MCP scope, skill preload. | Subagent như worker thread/config layer, inherit sandbox/permission từ parent. |
| **Memory/instructions** | `CLAUDE.md` + auto memory + rules scoped theo path. | `AGENTS.md` instruction chain; global/project/nested override. |

Kết luận gọn: **Claude Code harness giống một “programmable agent OS” cho developer workflow**, nhiều điểm móc để can thiệp hành vi. **Codex harness giống một “sandboxed execution platform” cho agent**, ưu tiên boundary, patch, diff, review và chạy local/cloud nhất quán.

---

Prompt: tôi thấy cùng 1 model nhưng đặt ở claude code và codex thì cho ra kết quả khác nhau. điều đó rõ ràng đến từ lớp harness của 2 nền tảng. và trên cộng động gần đây, các pro dev thường sử dụng song song cả codex và claude code. vậy họ sử dụng chúng như thế nào?

Đúng: **cùng model nhưng khác harness thì khác hành vi**. Model chỉ là “bộ não”; harness quyết định agent thấy gì, được gọi tool nào, sửa file bằng cơ chế nào, bị sandbox ra sao, có hỏi approval không, có hook/subagent/AGENTS.md/CLAUDE.md không, và có chạy trong terminal hay cloud task không.

Cách nhiều dev giỏi dùng song song thường không phải “tool nào tốt hơn”, mà là **chia vai**.

## Pattern phổ biến nhất: Claude Code làm kiến trúc sư, Codex làm executor/reviewer

### 1. Claude Code để hiểu bài toán, thiết kế, refactor phức tạp

Dev thường dùng Claude Code khi cần nhiều đối thoại, nhiều nuance, hoặc phải “ngửi” codebase:

* phân tích kiến trúc hiện tại;
* tìm root cause của bug khó;
* thiết kế API / data model / migration;
* refactor nhiều file nhưng cần giữ convention;
* frontend/UI nơi cần lặp nhiều vòng với taste;
* viết plan/spec chi tiết cho task.

Lý do: Claude Code có harness thiên về **interactive pair-programming**: memory `CLAUDE.md`, hooks, skills, subagents, permission modes, lifecycle events. Vì vậy nó phù hợp với giai đoạn “nghĩ cùng tôi, soi codebase, đề xuất chiến lược, rồi sửa cẩn thận”. Các bài so sánh cộng đồng gần đây cũng hay mô tả Claude Code mạnh ở architecture, complex features, frontend và reasoning depth, còn Codex mạnh ở speed/automation/cost-sensitive tasks. ([nxcode.io][1])

### 2. Codex để chạy việc độc lập, test, sửa nhỏ, PR review

Codex thường được dùng cho phần “đã biết cần làm gì, giờ triển khai đi”:

* task rõ ràng, scope đóng;
* fix CI/lint/test;
* viết test còn thiếu;
* cập nhật docs;
* rename/migration cơ học;
* tạo PR từ issue;
* review PR và bắt regression;
* chạy nhiều phương án song song.

OpenAI docs cũng định vị Codex cloud đúng kiểu này: offload larger jobs, track progress, review summary/diff, run tasks in parallel, delegate từ GitHub/Linear/Slack, và quay lại review khi có kết quả. ([OpenAI Developers][2]) Codex GitHub review có thể chạy bằng `@codex review`, tự review PR, chỉ flag P0/P1 để tránh noise, rồi có thể được nhờ fix issue ngay trong PR. ([OpenAI Developers][3])

## Workflow thực tế: “Claude viết spec, Codex thi công”

Một pattern rất hay là:

1. Mở Claude Code trong repo.
2. Bảo Claude đọc codebase, hỏi/không hỏi tùy case, rồi tạo `task.md`.
3. `task.md` ghi rõ mục tiêu, files liên quan, constraints, tests cần chạy, edge cases, non-goals.
4. Mở Codex, bảo: “Implement theo `task.md`, keep scope minimal, run tests.”
5. Codex sửa code, chạy test, tạo diff.
6. Quay lại Claude Code hoặc chính bạn review diff.
7. Nếu có PR, gọi Codex review hoặc Claude review lần nữa từ góc nhìn khác.

Có bài cộng đồng mô tả đúng pattern này: Claude hiểu feature và viết `task.md` để Codex đọc triển khai, gồm context cần thiết, file cần touch và convention phải theo. ([DEV Community][4])

Tại sao pattern này hiệu quả? Vì nó ép harness của hai bên làm đúng sở trường:

* **Claude Code** dùng context/hook/memory để biến yêu cầu mơ hồ thành **operating contract**.
* **Codex** dùng sandbox/patch/test/review để biến contract thành **diff có thể kiểm tra**.

## Workflow thứ hai: “parallel agents, separate worktrees”

Pro dev không để hai agent cùng sửa một working tree bừa bãi. Họ thường dùng:

* git branch riêng;
* git worktree riêng;
* cloud task riêng;
* hoặc patch/diff riêng rồi merge thủ công.

Codex docs nhấn mạnh cloud tasks có thể chạy song song trong môi trường riêng, review diff rồi mở PR. ([OpenAI Developers][2]) Codex CLI cũng có use case “hand work to the cloud” và “local code review” từ terminal. ([OpenAI Developers][5])

Ví dụ thực tế:

```text
main repo:
  Claude Code đang pair với dev để sửa feature A

worktree/codex-test-gap:
  Codex viết missing unit tests

worktree/codex-docs:
  Codex cập nhật docs/API examples

cloud task:
  Codex thử alternative implementation cho performance bug
```

Sau đó dev chỉ review diff tốt nhất. Đây là cách biến agent thành “junior contractors song song”, nhưng vẫn giữ human/dev chính làm integrator.

## Workflow thứ ba: “cross-review”

Một cách rất đáng giá: **không cho cùng agent tự chấm bài của nó**.

Ví dụ:

* Claude Code implement → Codex review PR.
* Codex implement → Claude Code review architecture/design.
* Codex fix CI → Claude kiểm tra xem fix có làm sai intent không.
* Claude refactor lớn → Codex chạy review kiểu regression/security/test coverage.

Codex có review workflow chính thức qua GitHub và có thể follow `AGENTS.md` review guidelines, ví dụ “đừng log PII”, “check auth middleware”. ([OpenAI Developers][3]) Một số bài cộng đồng cũng mô tả hybrid pattern: Claude Code cho interactive pair-programming/frontend, Codex Cloud cho long-running background tasks và PR review. ([Firecrawl][6])

## Workflow thứ tư: “Codex cho boring work, Claude cho judgment work”

Cách chia vai rất thực dụng:

| Loại việc                           | Tool thường hợp hơn       |
| ----------------------------------- | ------------------------- |
| Hiểu yêu cầu mơ hồ                  | Claude Code               |
| Design/refactor nhiều tradeoff      | Claude Code               |
| Frontend polish / UX nuance         | Claude Code               |
| Tạo task spec cho agent khác        | Claude Code               |
| Fix lint/typing/test fail           | Codex                     |
| Viết test từ behavior đã rõ         | Codex                     |
| Docs/changelog/examples             | Codex                     |
| PR review tự động                   | Codex                     |
| Chạy nhiều implementation song song | Codex cloud               |
| Migration cơ học                    | Codex                     |
| Security/regression pass trên diff  | Codex hoặc Codex Security |

Nói hơi cực đoan nhưng dễ nhớ:

> **Claude Code để quyết định “nên làm gì và làm thế nào cho đúng”. Codex để “làm nhiều việc rõ scope, tạo diff, chạy test, review”.**

## Workflow thứ năm: dùng cả hai như “hai tư duy khác nhau”

Vì harness khác nhau, hai agent có bias khác nhau:

* Claude Code dễ “đàm thoại”, diễn giải intent, tự tổ chức workflow bằng hook/skill.
* Codex thường literal hơn với spec, command/diff/audit rõ hơn.

Có một paper arXiv 2026 so sánh Claude Code và Codex trên pipeline khoa học, nhận thấy hai agent tạo hành vi khác nhau dù cùng task/resources: Claude hoàn thành nhanh hơn nhưng có silent deviations, còn Codex mất lâu hơn, có restart/tự sửa rõ hơn và follow spec literal hơn trong một tình huống dẫn tới khác biệt khoa học. ([arXiv][7]) Dù đây không phải benchmark coding app thông thường, nó minh họa đúng điều bạn nói: **harness + agent loop tạo “tính cách vận hành” khác nhau.**

## Setup repo để dùng song song cho sạch

Một repo dùng cả hai nên có cả:

```text
CLAUDE.md
AGENTS.md
/tasks/
  task-template.md
/scripts/
  test.sh
  lint.sh
  typecheck.sh
```

Trong `CLAUDE.md`, bạn ghi cho Claude:

```md
- Khi task mơ hồ, trước tiên đọc codebase và tạo implementation brief.
- Với task giao cho Codex, viết /tasks/<name>.md gồm:
  - goal
  - files likely touched
  - constraints
  - tests to run
  - non-goals
  - review checklist
```

Trong `AGENTS.md`, bạn ghi cho Codex:

```md
- Keep changes minimal and scoped to the task.
- Run ./scripts/test.sh when touching backend code.
- Run ./scripts/lint.sh before final response.
- Do not introduce new dependencies without explicit instruction.
- In reviews, prioritize correctness, security, data loss, auth, privacy, and test gaps.
```

Codex docs xác nhận `AGENTS.md` là instruction layer cho repo, đọc theo scope từ global/project/nested, file gần cwd override file root. ([Blake Crosley][8])

## Anti-pattern dev giỏi thường tránh

Không nên:

* cho cả hai agent sửa cùng branch cùng lúc;
* prompt quá mơ hồ rồi giao Codex làm feature lớn;
* để Claude tự review refactor lớn của chính Claude mà không có reviewer khác;
* không chạy test mà tin summary;
* để instructions mâu thuẫn giữa `CLAUDE.md` và `AGENTS.md`;
* để agent tự thêm dependency, thay architecture, hoặc “cleanup toàn repo” không scope.

## Công thức phối hợp rất mạnh

Dùng prompt kiểu này với Claude Code:

```text
Read the codebase and create /tasks/checkout-retry.md for Codex.
Include exact goal, relevant files, constraints, edge cases, tests to run, and non-goals.
Do not implement yet.
```

Rồi với Codex:

```text
Implement /tasks/checkout-retry.md.
Keep the diff minimal.
Run the specified tests.
If a test cannot run, explain why and what you inspected instead.
```

Sau khi Codex xong, quay lại Claude:

```text
Review this diff for architecture regressions, hidden coupling, missed edge cases, and whether it followed /tasks/checkout-retry.md.
Do not edit yet; produce findings only.
```

Đây là kiểu “agent pipeline” mà nhiều pro dev đang tiến tới: **một agent lập brief, một agent thi công, một agent khác review, human merge.**

[1]: https://www.nxcode.io/resources/news/claude-code-vs-codex-cli-terminal-coding-comparison-2026?utm_source=chatgpt.com "Claude Code vs Codex CLI 2026: Which Terminal AI ..."
[2]: https://developers.openai.com/codex/ide/cloud-tasks/?utm_source=chatgpt.com "Features – Codex IDE"
[3]: https://developers.openai.com/codex/third-party/github?utm_source=chatgpt.com "Codex code review in GitHub | ChatGPT Learn"
[4]: https://dev.to/bigboybamo/how-i-use-claude-code-and-codex-together-to-build-net-features-faster-1n40?utm_source=chatgpt.com "How I Use Claude Code and Codex Together to Build .NET ..."
[5]: https://developers.openai.com/codex/cli?utm_source=chatgpt.com "Codex CLI | ChatGPT Learn - OpenAI Developers"
[6]: https://www.firecrawl.dev/blog/claude-code-vs-codex?utm_source=chatgpt.com "Claude Code vs Codex: Which AI Coding Agent Should ..."
[7]: https://arxiv.org/abs/2605.28916?utm_source=chatgpt.com "First head-to-head comparison of agentic AI applied to the analysis of simulated data of the Einstein Telescope"
[8]: https://blakecrosley.com/blog/claude-code-to-codex-migration?utm_source=chatgpt.com "Claude Code to Codex Migration Guide 2026 - Blake Crosley"

---

Prompt: vậy tại sao chưa có ai từng thử build 1 agent coding cli mang lớp harness hợp nhất của cả claude code và codex?

Không đúng là “chưa ai thử”. Đúng hơn là:

> **Đã có nhiều dự án thử xây coding agent CLI/harness lai, nhưng chưa ai tái tạo được một “Claude Code + Codex unified harness” thật sự đầy đủ, ổn định, phổ biến, và tốt hơn cả hai trong mọi tình huống.**

Các dự án gần với hướng này gồm **OpenHands**, **Aider**, **OpenCode**, **Gemini CLI**, **Qwen Code**, các agent IDE như Cline/Roo/Cursor, và cả các prototype học thuật. OpenHands có sandbox Docker/process/remote để chạy command và sửa file an toàn; Aider có workflow terminal pair-programming kèm auto lint/test; OpenCode là open-source agent chạy terminal/desktop/IDE, có LSP và multi-session; Gemini CLI/Qwen Code cũng là terminal coding agents model-specific. ([OpenHands Docs][1])

Nhưng câu hỏi cốt lõi của bạn vẫn đúng: **tại sao chưa có một harness “best of both worlds” thật sự thắng?**

## 1. Vì “hợp nhất” hai harness này không phải cộng tính năng

Claude Code và Codex không chỉ khác feature. Chúng khác **triết lý điều khiển agent**.

Claude Code nghiêng về:

* lifecycle hooks;
* memory/`CLAUDE.md`;
* subagents có context riêng;
* skills;
* tool permissions chi tiết;
* workflow tương tác;
* agent như một “programmable dev coworker”.

Claude docs mô tả hooks là shell command, HTTP endpoint hoặc LLM prompt chạy tại các lifecycle events; memory gồm `CLAUDE.md` và auto memory; subagents có context window, system prompt, tool access và permissions riêng. ([Claude Platform Docs][2])

Codex nghiêng về:

* sandbox;
* approval policy;
* workspace boundary;
* patch/diff;
* cloud/local task execution;
* review loop;
* agent như một “bounded executor”.

Codex docs nói local Codex dùng OS-enforced sandbox, network mặc định tắt, và approval policy quyết định khi nào agent phải dừng để hỏi trước khi hành động. ([learn.chatgpt.com][3])

Nếu ghép máy móc, bạn sẽ có agent vừa muốn **tự động hóa sâu** như Claude Code, vừa muốn **boundary cứng** như Codex. Hai mục tiêu này xung đột:

```text
Claude-style hooks/subagents/memory  -> tăng autonomy, tăng surface area
Codex-style sandbox/approval         -> giảm surface area, tăng kiểm soát
```

Một harness tốt không thể chỉ bật tất cả. Nó phải quyết định **cái gì là deterministic policy, cái gì là model judgment, cái gì là user approval, cái gì là sandbox hard stop**.

## 2. Vì harness thật sự là sản phẩm, không chỉ là wrapper API

Một coding CLI đơn giản có thể làm trong vài tuần:

```text
prompt -> model -> tool call -> shell/edit -> result -> model
```

Nhưng một harness ngang Claude Code/Codex cần rất nhiều lớp khó:

```text
context indexer
repo map
edit engine
patch engine
tool scheduler
terminal/session manager
sandbox
network policy
approval UX
diff viewer
rollback/checkpoint
test runner
hook system
subagent orchestration
memory
instruction hierarchy
MCP/plugin system
IDE integration
cloud task runtime
secrets handling
audit logging
enterprise policy
telemetry/eval harness
```

Các dự án open-source đã chạm vào từng phần. OpenHands tập trung mạnh vào sandbox/runtime; OpenCode nhấn mạnh terminal/IDE/desktop, LSP và multi-session; Aider nổi tiếng ở CLI pair-programming + lint/test loop. Nhưng “full hybrid” nghĩa là phải làm **tất cả các lớp trên cùng lúc**, và mỗi lớp lại có edge case bảo mật, UX, cross-platform và enterprise. ([OpenHands Docs][1])

## 3. Vì phần quan trọng nhất của Claude Code/Codex không hoàn toàn công khai

Bạn có thể đọc docs, quan sát hành vi, xem một phần code hoặc prompt, nhưng không có đầy đủ:

* internal prompts;
* routing logic;
* compaction heuristics;
* safety classifiers;
* tool scoring;
* context selection heuristics;
* cloud runtime implementation;
* telemetry feedback loop;
* eval suite nội bộ;
* model-specific tuning.

Claude Code có public docs về tools, hooks, skills, memory, settings; Codex có docs về sandbox/approval/AGENTS.md/cloud. Nhưng cái làm chúng “có cảm giác riêng” phần lớn nằm ở **heuristic + eval + tuning vòng kín**. Anthropic và OpenAI đều có lợi thế vì họ kiểm soát cả model, product telemetry, safety policy, inference stack và distribution. ([Claude][4])

Open-source harness có thể copy interface, nhưng rất khó copy **behavioral prior**.

## 4. Vì safety của coding agent khó hơn nhiều so với chat app

Coding agent có quyền đọc file, sửa file, chạy command, mở network, xử lý secrets, invoke build tools, chạy package manager, gọi MCP server. Một harness “mạnh” cũng là một attack surface lớn.

Có nghiên cứu gần đây về “overeager coding agents” cho thấy agent có thể làm quá phạm vi yêu cầu, ví dụ xóa/sửa file ngoài ý định, và hiện tượng này thay đổi theo framework/harness chứ không chỉ theo model. Nghiên cứu đó test Claude Code, OpenHands, Codex CLI, Gemini CLI và nhiều base model, kết luận model-layer alignment không tự động truyền xuống permission gating của framework. ([arXiv][5])

Vì vậy một unified harness phải giải quyết các câu hỏi khó:

* Hook có được block command không?
* Subagent có được network không?
* MCP tool có được đọc secrets không?
* Memory có thể bị prompt injection ghi bẩn không?
* Auto mode có chạy migration DB không?
* Approval prompt có làm user mỏi và bấm bừa không?
* Sandbox chặn OS-level hay chỉ “model promised not to”?

Claude-style extensibility càng mạnh thì security model càng khó. Codex-style sandbox càng chặt thì agent càng khó tự trị. Đây là tradeoff thật, không phải thiếu ý tưởng.

## 5. Vì UX của “một harness thống nhất” rất dễ thành quá tải

Pro dev thích dùng Claude Code + Codex song song vì mỗi tool có **mental model rõ**:

```text
Claude Code = thảo luận, hiểu repo, design, refactor tinh tế
Codex       = task rõ scope, sandbox, patch, test, review, cloud execution
```

Một tool hợp nhất nếu phơi hết tính năng sẽ có quá nhiều mode:

```text
plan mode
auto mode
accept edits
read-only
workspace-write
danger-full-access
hook mode
subagent mode
cloud mode
review mode
skill mode
MCP mode
memory mode
worktree mode
```

Vấn đề không phải kỹ thuật đơn thuần; là **interaction design**. Một agent càng nhiều quyền, càng cần UI giúp dev hiểu “nó đang ở chế độ nào, thấy gì, sắp làm gì, có thể phá gì, rollback ra sao”.

Nếu UX không cực kỳ tốt, unified harness sẽ thua hai tool chuyên biệt vì dev không tin nó.

## 6. Vì kinh tế sản phẩm không ủng hộ “neutral meta-harness”

Một harness hợp nhất tốt nhất phải model-agnostic: chạy Claude, GPT/Codex, Gemini, Qwen, local models, v.v.

Nhưng các công ty có model mạnh lại muốn harness của họ tối ưu cho model của họ:

* Anthropic muốn Claude Code làm Claude tốt nhất.
* OpenAI muốn Codex làm GPT/Codex tốt nhất.
* Google/Alibaba tối ưu Gemini CLI/Qwen Code cho model của họ.
* Tool độc lập thì thiếu quyền truy cập sâu vào model/runtime/telemetry.

Qwen Code thậm chí được mô tả là terminal coding agent tối ưu cho Qwen series; Gemini CLI là agent terminal mang Gemini vào command line. ([GitHub][6])

Một neutral harness có nguy cơ bị kẹt ở giữa: không có model-tuning sâu như vendor, cũng không có UX/distribution như vendor.

## 7. Vì harness tốt phải được eval liên tục trên hành vi, không chỉ benchmark pass rate

Coding agent không chỉ cần “giải được task”. Nó cần:

* không phá scope;
* không che giấu failure;
* biết khi nào hỏi;
* chạy đúng test;
* không sửa ngoài ý định;
* không leak secrets;
* không hallucinate command output;
* không làm diff quá lớn;
* biết rollback;
* biết chia subtask.

Các paper gần đây bắt đầu coi harness/scaffolding/context engineering là đối tượng nghiên cứu riêng. Một paper về terminal coding agents mô tả các vấn đề như safety controls, context bloat, adaptive compaction, model routing, dual-agent planning/execution và event-driven reminders. ([arXiv][7])

Nói cách khác: muốn build unified harness nghiêm túc, bạn cần **evaluation infrastructure** gần bằng product infrastructure.

## 8. Vì “best of both” thực ra nên là “router”, không phải một agent duy nhất

Tôi nghĩ hướng đúng không phải build một CLI duy nhất bắt chước toàn bộ Claude Code + Codex. Hướng đúng là một **agent orchestrator**:

```text
Human
  |
  v
Unified CLI / Control Plane
  |
  +-- Planner/Architect agent      -> Claude-style
  +-- Executor agent               -> Codex-style sandbox
  +-- Reviewer agent               -> independent diff review
  +-- Test/CI agent                 -> deterministic scripts
  +-- Security/policy layer         -> hard enforcement
  +-- Memory/instructions layer     -> repo/user/team knowledge
```

Tức là không cố làm một “super agent”, mà làm **harness phân vai**:

| Vai                                     | Harness tốt hơn              |
| --------------------------------------- | ---------------------------- |
| hiểu repo, tạo plan, phân tích tradeoff | Claude-style                 |
| sửa code trong boundary rõ              | Codex-style                  |
| chạy test/lint/typecheck                | deterministic runner         |
| review diff                             | reviewer agent độc lập       |
| policy/security                         | non-LLM enforcement          |
| memory/project convention               | instruction + indexed memory |

Đây là kiến trúc có khả năng thắng hơn “một agent làm tất cả”.

## 9. Nếu build thật, sản phẩm nên trông như thế nào?

Một unified coding CLI đáng xây sẽ có các primitive sau:

```text
1. Workspace sandbox như Codex
   - read-only / workspace-write / full-access
   - network off by default
   - approval policy rõ

2. Hook/event bus như Claude Code
   - pre-tool
   - post-edit
   - post-test
   - pre-commit
   - pre-compact
   - subagent start/stop

3. Instruction hierarchy hợp nhất
   - AGENTS.md
   - CLAUDE.md import/compat
   - .agent/rules/
   - task.md
   - local private overrides

4. Subagent orchestration
   - planner
   - implementer
   - reviewer
   - test-fixer
   - security-auditor

5. Patch-first edit engine
   - mọi thay đổi thành diff
   - checkpoint/rollback
   - minimal patch preference

6. Deterministic test runner
   - không để model “nhớ” chạy test
   - hook bắt buộc chạy theo file touched

7. Memory nhưng bị sandbox hóa
   - memory read/write có audit
   - không cho prompt injection ghi arbitrary policy

8. Worktree/cloud parallelism
   - mỗi task một branch/worktree/container
   - merge bằng PR/diff review
```

Điểm quyết định: **LLM không được là enforcement layer**. Model có thể đề xuất, nhưng sandbox, hook, policy, test gate, approval gate phải nằm ngoài model.

## Kết luận

Có người đã thử các mảnh của ý tưởng này. Chưa có “Claude Code + Codex unified harness” thật sự vì:

1. Hai harness có triết lý điều khiển khác nhau, không cộng feature được.
2. Full harness là cả một operating system cho agent, không phải CLI wrapper.
3. Nhiều phần hành vi cốt lõi của Claude Code/Codex là proprietary và model-tuned.
4. Safety/security cực khó khi kết hợp hooks, subagents, MCP, shell, network, memory.
5. UX dễ quá tải.
6. Vendor incentives không khuyến khích một neutral harness thắng tất cả.
7. Cần eval hành vi/harness rất sâu, không chỉ SWE-bench pass rate.

Câu trả lời ngắn nhất: **chưa có vì bài toán đúng không phải “gộp Claude Code và Codex”, mà là thiết kế một control plane biết lúc nào dùng Claude-style reasoning, lúc nào dùng Codex-style execution, và lúc nào dùng deterministic enforcement.**

[1]: https://docs.openhands.dev/openhands/usage/architecture/runtime?utm_source=chatgpt.com "Runtime Architecture - OpenHands Docs"
[2]: https://docs.anthropic.com/en/docs/claude-code/hooks?utm_source=chatgpt.com "Hooks reference - Claude Code Docs"
[3]: https://learn.chatgpt.com/docs/agent-approvals-security?utm_source=chatgpt.com "Agent approvals & security | ChatGPT Learn"
[4]: https://code.claude.com/docs/en/tools-reference?utm_source=chatgpt.com "Tools reference - Claude Code Docs"
[5]: https://arxiv.org/abs/2605.18583?utm_source=chatgpt.com "Overeager Coding Agents: Measuring Out-of-Scope Actions on Benign Tasks"
[6]: https://github.com/google-gemini/gemini-cli?utm_source=chatgpt.com "google-gemini/gemini-cli: An open-source AI ..."
[7]: https://arxiv.org/abs/2603.05344?utm_source=chatgpt.com "Building AI Coding Agents for the Terminal: Scaffolding, Harness, Context Engineering, and Lessons Learned"
