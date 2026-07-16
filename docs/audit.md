Kết luận audit 2026-07-16: thiết kế `cccx` đã qua được các vấn đề critical/high ban đầu ở mức đặc tả, nhưng chưa nên coi là frozen spec để implement ngay. Trạng thái hiện tại tốt hơn bản audit cũ: repo đã `git init`, Codex upstream local có commit thật, WorkSpec/Artifact đã có route model/provenance, permission queue và headless contract đã rõ hơn. Các rủi ro còn lại chủ yếu nằm ở việc biến đặc tả thành enforcement trong Codex fork.

## Scope

- Audited docs: `docs/architecture.md`, `docs/design.md`.
- Codex upstream local baseline: `D:\Learning_Programing\codex-cli-src`, HEAD `800715d201651a2a07c2706dca10400109dae3d3`.
- Skills used: `agent-architecture-audit`, `agent-harness-construction`, `loop-design-check`, `autonomous-agent-harness`.
- Claude Code source leak remains non-authoritative. Use only public/observable behavior and clean-room design notes.

## Findings

1. **[High] Baseline drift must become a repeated gate, not a one-time note.**

   `architecture.md` now pins Codex baseline to `800715d201651a2a07c2706dca10400109dae3d3`, but the project depends heavily on upstream Codex internals: hooks, multi-agent spawn/resume, command registry, config layering and model provider behavior. Every `git pull` can invalidate "kế thừa nguyên" claims.

   Required fix: add an implementation checklist later: after upstream pull, run source checks for hook event names, `spawn_agent`, `resume_agent_from_rollout`, command registry, model override semantics, and config schema before editing cccx.

2. **[High] WorkSpec policy is strong, but implementation must not reuse Codex child config inheritance directly.**

   Current architecture correctly requires deny-by-default child config and `WorkSpec.allowedTools`. Codex upstream builds child config from parent runtime state and role overlays. That is useful baseline behavior, but unsafe as-is for cccx because CC/CX authority must be the intersection of WorkSpec, parent permission and sandbox scope.

   Required fix: implement `WorkSpecToChildConfigAdapter` as a hard boundary. Treat native Codex inheritance as input data, not as final authority.

3. **[High] Full-history fork is incompatible with normal CX execution.**

   Codex upstream full-history fork rejects child overrides for agent type/model/reasoning. `architecture.md` now documents `forkMode = "workspec_only" | "full_history_debug"`, with default `workspec_only`. This is the right direction.

   Required fix: implementation must enforce that CX execution never runs via full-history fork. Full-history fork can be debug/read-only only, inherits parent, and cannot override harness/model/permission.

4. **[Medium] Model route policy is now coherent; implementation must enforce catalog freshness.**

   The design now says `requestedRouteId` is an opaque 9router id and resolver validates against allowlist/capability profile. `architecture.md` also requires a local `ModelRouteCatalog` with timestamp/source/digest and fail-closed spawn behavior.

   Required implementation fix: make route validation a core resolver step, not a UI-only warning.

5. **[Medium] Goal/evaluator loop is safer; implementation must protect acceptance boundaries.**

   VerificationRecord and StopHook reduce "agent says done" risk. `architecture.md` now adds `verificationSpec` with protected paths/digests so Build/CX cannot silently weaken tests, edit acceptance commands, or rewrite verification policy to pass.

   Required implementation fix: enforce protected paths in permission gate and verify digests before marking any task verified.

6. **[Medium] Headless contract now has `agent-update`, but event payloads need one manifest.**

   `design.md` now includes `agent-update` with `modelRoute`, but command help, TUI status, JSONL and TaskGraph still risk diverging if each is hand-coded.

   Required fix: generate command metadata and JSONL event schemas from a single typed manifest where practical.

7. **[Low] Accessibility/plain fallback is intentionally strict for personal UX, but should be explicit.**

   The design intentionally requires Unicode/true-color for interactive TUI and relies on headless/plain modes for machine-readable fallback. That is acceptable for this personal project, but it should stay framed as a conscious product constraint, not an accidental accessibility omission.

   Required fix: keep `--json`, `cccx exec`, and stderr/stdout separation non-negotiable. Add `--plain` only if interactive fallback becomes a real need.

## Resolved Since Previous Audit

- Repo now has `.git`; old "no git" P0 is obsolete.
- Architecture baseline now points to current Codex local HEAD.
- WorkSpec now includes `modelRoute`, `forkMode`, route provenance and fallback policy.
- WorkSpec now includes `verificationSpec` with protected paths/digests.
- Provider Gateway now records `requestedRouteId/effectiveRouteId` instead of only model names.
- UX now has `/agents model <preset> <route|inherit-main-requested|inherit-main-effective>`.
- Headless JSONL now has an `agent-update` event with model route provenance.
- Plugin agent presets now include optional model route fields and allowed route list.

## Next Implementation Order

1. Implement baseline drift checks and upstream feature map.
2. Implement `WorkSpecToChildConfigAdapter` deny-by-default.
3. Implement `ModelRouteResolver` + `ModelRouteCatalog`.
4. Implement permission gate as core policy, with hooks as extensions only.
5. Implement JSONL/command manifest from typed schema.
6. Add verification boundary protection before enabling autonomous CX repair loops.
