Kết luận: hiện tại cccx mới pass chắc chắn **3/25 gate** — Gate 12, 14 và 23. **22 gate còn lại vẫn PARTIAL** theo chính tiêu chuẩn runtime/E2E của [task.md](D:/Learning_Programing/cccx/docs/task.md:951). Chưa thể kết luận “cccx hoàn thiện đúng thiết kế”.

## Phát hiện audit

### Critical — Thiếu một E2E spine xuyên suốt

Các unit/seam test hiện chứng minh từng mảnh, nhưng chưa chứng minh toàn bộ chuỗi:

```text
binary startup
→ MainCc/CC identity
→ mode/permission
→ WorkSpec
→ spawned child projection
→ tool execution/sandbox
→ Artifact
→ VerificationRecord
→ TaskGraph
→ StopHook
→ TUI/headless output
```

Đây là lý do Gate 1–11 và nhiều gate liên quan lifecycle vẫn PARTIAL, dù implementation nền đã khá sâu. Chuỗi này phải đồng thời chứng minh các invariant tại [architecture.md](D:/Learning_Programing/cccx/docs/architecture.md:15).

### Critical — Gate 5 cần quyết định kiến trúc thật

Gate 5 yêu cầu Bypass chỉ hoạt động khi có sandbox attestation thật và circuit breaker luôn còn hiệu lực. Hiện app-server đã fail-closed đúng, nhưng chưa có backend chứng minh OS isolation thật.

Để Gate 5 PASS theo báo cáo hiện tại, cần cả hai:

- Negative: không có attestation → Bypass bị từ chối.
- Positive: môi trường thực sự isolated → backend phát hành attestation typed và Bypass hoạt động, circuit breaker vẫn chặn.

Mock, env var hoặc confirmation dialog không đủ. Nếu Windows/app-server không cung cấp primitive phù hợp, phải dừng hỏi owner hoặc sửa acceptance; không được hạ chuẩn.

### High — Ma trận progress chưa đồng bộ với các remediation bên dưới

Ví dụ:

- Gate 13 vẫn ghi thiếu inter-process lease, nhưng remediation sau đó nói lease đã được triển khai và test cross-process.
- Gate 6/15 đã có exact-ID và one-active-queue core proof, nhưng điều kiện còn thiếu thực sự là multi-worker TUI integration.
- Gate 17 đã có dispatch-time lease proof; phần còn thiếu là spawned-worker/lifecycle proof.

Trước khi implement tiếp, cần re-baseline ledger từ bằng chứng mới nhất, nhưng vẫn giữ các gate này PARTIAL cho tới khi E2E còn thiếu hoàn tất.

### High — Projection phải được chứng minh như một boundary thống nhất

Gate 8, 9, 10, 17 và 21 không nên đóng bằng các test tách rời. Một spawned CX thật phải đồng thời chứng minh:

- Parent capability lấy từ runtime thật.
- Tool/MCP/plugin catalog đã lọc.
- Executor áp cùng projection.
- OS sandbox có writable roots tương ứng.
- Shell, MCP write và `apply_patch` dùng cùng canonical scope/stale-read policy.
- Unknown tool fail-closed.
- Write worker serialize, read workers chạy song song.

Điều này trực tiếp kiểm chứng adapter deny-by-default tại [architecture.md](D:/Learning_Programing/cccx/docs/architecture.md:885).

### High — Gate 20 cần định nghĩa survivor chính xác

Architecture nói raw MCP/tool output cũ có thể bị loại khi compact; TaskGraph, ADR, WorkSpec/Artifact và Project Memory mới là durable truth.

Vì vậy “MCP-derived context survives” không nên được hiểu là giữ toàn bộ raw MCP output. Kế hoạch phải giữ:

- MCP registry/health và source references cần thiết.
- Stale-read state cần thiết để fail-closed.
- Durable ADR/reference IDs.

Không giữ raw output như pseudo-memory, tránh memory contamination.

## Kế hoạch pass 25 gate

### Phase 0 — Canonical final-gate harness

Tạo một validation fixture chung:

- Real cccx binary.
- Test `CCCX_HOME` cô lập.
- Temporary git project.
- Controllable mock 9router.
- Khả năng chạy nhiều process/worker.
- TUI input/output capture và headless JSONL capture.

Mỗi gate phải có ledger gồm requirement, current proof, missing proof, exact command, negative case và evidence artifact. Không đổi PASS dựa trên comment hoặc contract test.

### Phase 1 — Startup, identity, authority và mode

Cover Gate **1–4**.

- Startup E2E chứng minh actor đầu tiên là `MainCc`, harness `CC`.
- Spawn CC/CX rồi resume, chứng minh ActorIdentity và harness không đổi.
- Child spawn/reroute/ADR bị từ chối; MainCc được phép.
- Config precedence: default → user → project → CLI.
- Shift+Tab chỉ cycle Plan/Default/AcceptEdits/Auto.
- Sáu mode tạo tool-permission outcomes đúng, không chỉ parse đúng enum.

Exit: Gate 1–4 có binary/session runtime proof.

### Phase 2 — Attestation và permission queue

Cover Gate **5, 6, 15**.

- Xây sandbox attestation backend thật hoặc giữ Bypass disabled và kích hoạt stop condition.
- Chứng minh circuit breaker còn hiệu lực trong Bypass.
- Mọi native approval đi qua CCCX central queue.
- Hai hoặc nhiều worker cùng request vẫn chỉ có một active prompt.
- Approve/deny/inspect/batch dùng exact `requestId`.
- Digest mismatch, stale request và cancelled worker đều không thể được approve.

Exit: multi-worker session-to-TUI proof xanh; Gate 5 chỉ PASS khi có positive attestation proof.

### Phase 3 — Child capability boundary

Cover Gate **8–10, 17, 21**.

Chạy một spawned-CX scenario kiểm tra toàn bộ:

- Parent runtime capability intersection.
- Empty writable intersection → read-only.
- Tool/MCP/plugin catalog projection.
- Executor và OS sandbox projection.
- Canonical path, junction/symlink, move destination và multi-target.
- Shell/MCP/apply-patch stale-read enforcement.
- Unknown future tool bị deny.
- Hai writer serialize; nhiều reader parallel.
- CC-only/native plugin capability không rò sang CX.

Exit: spawned-child positive và negative E2E cùng xanh.

### Phase 4 — WorkSpec, Artifact, lifecycle và TaskGraph

Cover Gate **7, 11–14, 16**.

Golden runtime flow:

```text
MainCc creates Task
→ WorkSpec
→ child runs
→ Artifact returned
→ binding validated
→ real verification command
→ VerificationRecord
→ Task verified
→ StopHook allows completion
```

Negative cases:

- Sai `taskId`, `workSpecId` hoặc `attemptId` bị reject.
- Prose “tests passed” không verify task.
- Default child không nhận raw parent history.
- Unverified task khiến StopHook block.
- Concurrent process không gây lost update.
- Interrupt → checkpoint → resume giữ `logicalSessionId`, đổi incarnation hợp lệ.
- Cancel thu hồi pending permission nhưng giữ metadata/checkpoint cần xem.

Exit: Gate 12/14 tiếp tục PASS; Gate 7/11/13/16 được nâng bằng E2E.

### Phase 5 — 9router transport và provenance

Cover Gate **18–19**.

Inventory toàn bộ LLM paths:

- Main chat.
- CC/CX subagents.
- Classifier.
- Goal evaluator.
- Compaction/summarization.
- Review/audit.
- Realtime và Chat Completions/Responses compatibility paths.

Mock 9router phải cover:

- Successful streaming.
- Disconnect trước side effect.
- Disconnect sau tool event.
- 429 + `Retry-After`.
- Requested route khác effective route.
- Metadata bị thiếu.
- Auth failure/retry attempts.

Provenance phải lưu attempt, requested/effective route, fallback chain và digests thật. Metadata thiếu phải là `unknown`, không copy requested thành effective.

Exit: không còn provider bypass; mọi retry/stream outcome có provenance đúng.

### Phase 6 — Durable context, ADR và MCP

Cover Gate **20**, đồng thời regression Gate 3/10/21.

- `/remember` lưu đúng project scope.
- Memory sống qua compaction dù không có TaskGraph.
- Active WorkSpec/Artifact/checkpoint refs sống qua compaction/restart.
- ADR refs được persist; chỉ MainCc commit sau verification.
- CX chỉ gửi ADR proposal.
- MCP registry/health/stale-read state được khôi phục đúng.
- Raw MCP/tool output cũ không bị biến thành trusted durable memory.

Exit: compact và restart đều phục hồi đúng authority/state.

### Phase 7 — TUI, secure input và headless

Cover Gate **22–23**, hoàn tất user-facing proof cho Gate 6/15/16.

- Status line lấy live core state.
- `/agents`, `/tasks`, `/permissions`, `/mcp`, `/model`, `/remember` phản ánh runtime thật.
- Network retry UX đúng attempt/backoff/status.
- Secure input không lọt vào parser buffer, history, transcript, config plaintext, log hoặc telemetry.
- Permission/lifecycle flows chạy qua TUI thật.
- Headless giữ JSONL versioned, terminal-last và exit code 0/1/2/3.

Exit: Gate 22 PASS; Gate 23 regression vẫn xanh.

### Phase 8 — Coverage và repository hygiene

Cover Gate **24–25** và final acceptance.

- Định nghĩa rõ “cccx-owned code” và generated-source allowlist.
- Coverage cccx-owned ≥80% sau tất cả thay đổi.
- Runtime trust boundaries có negative tests.
- Không track `target/`, test home, logs, tmp, secrets hoặc local state.
- Chạy lại golden E2E spine.
- Chỉ build binary cuối một lần sau các targeted tests.
- Ghi exact command/result vào progress cho từng gate.

Exit cuối cùng:

```text
25/25 PASS
0 OPEN/PARTIAL
coverage ≥80%
golden binary E2E green
git artifact hygiene green
```

Thứ tự phụ thuộc bắt buộc là:

```text
Phase 0
→ Phase 1
→ Phase 2 + Phase 3
→ Phase 4
→ Phase 5 + Phase 6
→ Phase 7
→ Phase 8
```
