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
- `packages/ark-markdown-bridge` owns its worker code (`src/main.rs`,
  `tests/worker_protocol.rs` — ported verbatim from cortex `runtime/`) and
  depends only on the `kosmos-package-protocol` crate, pinned by git rev to
  `makekosmos/cortex`. The rev pin activates once the cortex branch lands on
  the shared cortex remote; until then a local `[patch]` or `path` override
  pointing at a cortex checkout is needed for `cargo test`. Do not vendor
  cortex code.
- No GitHub Actions; local checks are the gate.

## Checks (as run at import)

```powershell
# Validate every manifest parses as JSON
Get-ChildItem packages/*/manifest.json | ForEach-Object { node -e "JSON.parse(require('fs').readFileSync('$($_.FullName)','utf8'))" }

# Confirm research/ is ignored
git check-ignore -v research/huawei-health/README.md
```

Per-package Rust crates are standalone (`cargo test` inside a package dir).
For `ark-markdown-bridge`, `cargo test` resolves `kosmos-package-protocol`
from `makekosmos/cortex` at the pinned rev; if that rev is not yet on the
remote, temporarily add a `[patch."https://github.com/makekosmos/cortex"]`
entry pointing at a local cortex checkout and remove it before committing.
