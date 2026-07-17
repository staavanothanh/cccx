<p align="center"><strong>cccx</strong> is a personal CC/CX coding CLI fork built on Codex CLI.
<p align="center">
  <img src="https://github.com/openai/codex/blob/main/.github/codex-cli-splash.png" alt="Codex CLI splash" width="80%" />
</p>
</br>
This repository preserves Codex provenance and Apache-2.0 licensing, but the active fork target is <code>cccx</code>.</p>

---

## Quickstart

### Running cccx locally

This fork is currently optimized for local source builds and personal use.

From the repo root:

```shell
cd codex-rs
cargo run -p cccx-cli --bin cccx -- --help
```

Default config home is `~/.cccx`.

- Primary override env: `CCCX_HOME`
- Legacy compatibility fallback: `CODEX_HOME`

The historical `codex` binary name is still kept as a compatibility alias during the rename phase.

## Docs

- [**Codex Documentation**](https://developers.openai.com/codex)
- [**Contributing**](./upstream-docs/codex/contributing.md)
- [**Installing & building**](./upstream-docs/codex/install.md)
- [**Open source fund**](./upstream-docs/codex/open-source-fund.md)

This repository is licensed under the [Apache-2.0 License](LICENSE).
