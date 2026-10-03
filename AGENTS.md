# AGENTS.md — integrations

Single repo for first-party integration packages and the published package
catalog. Replaces `makekosmos/package-index` + `makekosmos/store` (both
archived after the switch): no Ed25519 signing, no envelopes, no second
listing file. Trust model: HTTPS + GitHub Releases + pinned `sha256`/`size`
in `catalog.json`, same as the Engine updater and native apps.

## Layout

- `packages/<name>/` — one package per directory: `manifest.json`, `icon.png`,
  Rust worker (`Cargo.toml`, `src/`, `tests/`). The manifest also carries the
  `store` block (`description`, `categories`, `connects_to`,
  `data_compatibility`) — one source of truth per package.
- `external-apps.json` — storefront entries for third-party apps integrations
  connect to (validated by `scripts/manifest-schema.mjs`).
- `scripts/` — the pipeline (`manifest-schema.mjs`, `validate-manifests.mjs`,
  `build-packages.mjs`, `build-catalog.mjs`, `zip-utils.mjs` + tests).
- `research/` — RE notes and captures, gitignored, never committed.

## Rules

- Fresh import: packages were copied without git history; do not try to
  reconstruct cortex history here.
- Do not commit secrets, tokens, cookies, `.env`, private keys or captured
  session data. Audit new material before adding it under `packages/`.
- `.kspkg` archives contain exactly `manifest.json`, the worker exe and the
  manifest icon — nothing else may be added.
- `ark-markdown-bridge` depends on `kosmos-package-protocol` pinned by git rev
  to `makekosmos/cortex`; for local `cargo test` before that rev is on the
  remote, use a temporary `[patch]` override and remove it before committing.
- Workflows: `quality.yml` runs on PRs (validate + script tests + cargo build
  and test for every package on windows + macos + catalog dry run);
  `publish.yml` is manual dispatch only and publishes `catalog-N` releases
  with `GITHUB_TOKEN` alone.
- Release invariant: `releases/latest` must always resolve to a catalog
  release — nothing else may create a GitHub release in this repo. The
  Engine's catalog URL depends on it (README, "Releases").
- `.kspkg` artifacts are platform-specific compiled workers, named
  `<id>-<version>-<os>-<arch>.kspkg`. A platform ships only when every
  manifest's `targets` declare it; `build-catalog` fails on a declared
  platform without an artifact.

## Checks

```powershell
npm test
node scripts/validate-manifests.mjs
node scripts/build-packages.mjs --out out --sequence 0
node scripts/build-catalog.mjs --packages out/packages.json --external-apps external-apps.json --sequence 1 --out out/catalog
```
