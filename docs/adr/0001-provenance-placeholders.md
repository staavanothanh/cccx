# ADR-0001: Provenance phải chứa digest thật, không chứa placeholder

**Date**: 2026-07-19
**Status**: accepted
**Deciders**: implementer

## Context

`provenance_for_runtime()` trong `codex-rs/core/src/cccx_runtime.rs` tạo
`ArtifactProvenance` với digest giả dạng `sha256:cccx-runtime-input-{ws_id}`
và `sha256:cccx-runtime-output-{ws_id}`. effective_route_id copy nguyên
requested_route_id, ngay cả khi gateway_request_id là None (không có
evidence từ 9router).

Kết quả: TaskGraph có thể xác nhận task đã verified dựa trên provenance giả,
không có evidence thật từ execution. `implementer-issues.md` §4 ghi rõ:
"Provenance dùng route giống nhau và digest giả dạng chuỗi sha256:cccx-runtime-*,
không lấy từ request 9router thật."

## Decision

1. `provenance_for_runtime()` giờ sinh SHA256 digest thật từ WorkSpec payload
   (objective + allowed_tools) và output digest từ nội dung thực.
2. `effective_route_id` ghi `"unknown"` khi không có gateway_request_id
   (không copy requested_route_id).
3. `ArtifactProvenance::verify_placeholders()` được gọi từ
   `Artifact::validate()` để từ chối mọi provenance chứa chuỗi `cccx-runtime`.
4. Khi có 9router gateway request thật trong tương lai, effective_route_id
   phải được populate từ header/body của 9router response.

## Alternatives Considered

### Alternative 1: Giữ placeholder + chỉ log warning
- **Pros**: Không break code hiện tại
- **Cons**: Tiếp tục vi phạm architecture.md §12; verified task dựa trên evidence giả
- **Why not**: Đây chính là vấn đề implementer-issues §4 đã chỉ ra

### Alternative 2: Bỏ provenance hoàn toàn khi không có 9router evidence
- **Pros**: Không tạo data giả
- **Cons**: Break mọi Artifact validate (provenance là required field)
- **Why not**: Provenance vẫn cần thiết cho audit trail; "unknown" cho
  effective_route_id là đủ trung thực

## Consequences

### Positive
- Task verification giờ dựa trên digest SHA256 thật từ input/output payload
- Provenance placeholder không còn có thể bypass validation
- Artifact::validate() giờ là gate cứng chống placeholder

### Negative
- `effective_route_id` từ runtime subagent completion giờ là "unknown",
  không còn tự động inherit từ requested → code nào parse field này
  cần handle trường hợp "unknown"

### Risks
- Khi wire 9router gateway request id vào complete_spawn, cần populate
  effective_route_id thật — hiện deferred vì chưa có gateway response
  metadata tại time of completion
