# AGENTS.md — integrations

Standalone integration packages (source `.kspkg` workers) split out of
`cortex/packages`. Each package carries its own `manifest.json` and version and
is released independently through Package Index (makekosmos/package-index).

## Layout

- `packages/<name>/` — one package per directory: `manifest.json`, `icon.png`,
  Rust worker (`Cargo.toml`, `src/`, `tests/`) or TS sources. `raycast-api` is
  a TypeScript helper package (`package.json`, no manifest).
- `research/` — reverse-engineering notes, capture tools and investigation
  dumps (currently `research/huawei-health/`). **This directory is gitignored
  and must never be committed.** Anything not publishable (RE notes, session
  captures, credentials, research dumps) goes here, not into `packages/`.

## Rules

- Fresh import: packages were copied without git history; do not try to
  reconstruct cortex history here.
- Do not commit secrets, tokens, cookies, `.env`, private keys or captured
  session data. Audit new material before adding it under `packages/`.
- `packages/ark-markdown-bridge` still references `../../runtime` (cortex) in
  `Cargo.toml`, `src/main.rs` and `tests/worker_protocol.rs`; decoupling is a
  follow-up task. Do not "fix" by vendoring cortex code.
- No GitHub Actions; local checks are the gate.

## Checks (as run at import)

```powershell
# Validate every manifest parses as JSON
Get-ChildItem packages/*/manifest.json | ForEach-Object { node -e "JSON.parse(require('fs').readFileSync('$($_.FullName)','utf8'))" }

# Confirm research/ is ignored
git check-ignore -v research/huawei-health/README.md
```

Per-package Rust crates are standalone (`cargo test` inside a package dir),
except `ark-markdown-bridge` which will not build until the `../../runtime`
dependency is decoupled.
