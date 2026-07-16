# cccx progress

- Docs baseline committed: `39e511c`.
- Codex CLI baseline imported: `88ddfb4` from upstream `800715d2`.
- Current phase: M1 implementation complete; next phase is M2 WorkSpec/Artifact protocol.
- Done: design docs, audit, fork plan, upstream remote, upstream docs separated, M0 validation complete, M0.5 MVP source gating implemented, M1 provider gateway crate implemented and wired into default Codex provider config path.
- Toolchain fix: installed Visual Studio Build Tools 2022 with C++ tools/Windows SDK via `winget`; use repo-pinned Rust `1.95.0-x86_64-pc-windows-msvc` through `D:\Program Files\cargo\bin`.
- M0 validation passed from `codex-rs/`: `cargo check -p codex-core`; `cargo check -p codex-tui`; `RUST_MIN_STACK=33554432 cargo test -p codex-core multi_agent -- --test-threads=1`.
- M0.5 validation passed from `codex-rs/`: `cargo fmt --check`; `cargo check -p codex-features -p codex-cli -p codex-tui`.
- M0.5 scope: centralized `codex-features::cccx_mvp` gate with `CCCX_MVP_ALLOW_UPSTREAM_SURFACES=1` escape hatch; CLI/TUI non-MVP surfaces hidden and runtime-guarded; default Apps/MCP Apps/MemoryTool/remote-plugin feature defaults disabled under cccx gate.
- Codebase indexed via codebase-memory-mcp as project `cccx`; artifact written to `.codebase-memory/graph.db.zst`.
- Caveat: direct binary smoke via `cargo run -p codex-cli` timed out while building/running the Windows binary target; compile checks passed.
- M1 validation passed from `codex-rs/`: `cargo fmt --check`; `cargo check -p cccx-provider-gateway`; `cargo test -p cccx-provider-gateway`.
- M1 targeted validation passed from `codex-rs/`: `cargo test -p codex-model-provider-info test_built_in_model_providers_include_9router`.
- M1 targeted validation attempted from `codex-rs/`: `cargo test -p codex-core cccx_default_model_provider`; test exists, but Windows compile/link of the `codex-core` test binary repeatedly exceeded the time budget and was stopped.
- M1 scope completed: new `cccx-provider-gateway` crate with `ModelRouteCatalog`, `ModelRouteResolver`, `ModelRouteProvenance`, fail-closed route resolution, 9router `/v1/responses` client, mock 9router provenance test, built-in reserved `9router` provider metadata, and cccx default model-provider resolution to `9router`.
- Next: start M2 WorkSpec/Artifact protocol.
