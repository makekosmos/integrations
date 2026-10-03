# integrations

First-party integration packages for Kosmos and the single catalog the
Engine installs them from. This repo replaced the two-repo pipeline
(`package-index` built and signed `.kspkg` files, `store` held a second signed
storefront catalog): packages, storefront metadata and the published
`catalog.json` now live in one place, unsigned — installs are pinned by the
`sha256` + `size` recorded in the catalog, the same trust model the Engine
updater and native apps already use (HTTPS + GitHub Releases + hash).

## Layout

- `packages/<id>/` — one package per directory: `manifest.json`, `icon.png`,
  a Rust worker (`Cargo.toml`, `src/`, `tests/`). The manifest is the single
  source of truth: package identity, permissions, data contract and the
  `store` block (storefront description, categories, `connects_to`,
  `data_compatibility`) the Store listing is derived from.
- `external-apps.json` — third-party apps the integrations connect to
  (BigFrontend, LeetCode, Obsidian, …). Storefront-only entries; nothing is
  installed from them.
- `scripts/` — the pipeline: `validate-manifests.mjs`,
  `build-packages.mjs` (`.kspkg` per package), `build-catalog.mjs`
  (`catalog.json` + `SHA256SUMS.txt` + `icon-<id>.png` assets),
  `zip-utils.mjs`, `manifest-schema.mjs` and their tests.
- `toolchain.json` — the Node/Rust pins CI installs.

## Catalog

`gh release` assets per publish (`catalog-N` tags):

- `catalog.json` — `{schema_version, sequence, issued_at, expires_at,
  packages[], external_apps[], revoked[]}`; `sequence` is monotonic.
- `<package-id>-<version>.kspkg` — `manifest.json` + worker exe + `icon.png`.
- `icon-<package-id>.png` — the Store icon for each package.
- `SHA256SUMS.txt` — `sha256sum` lines covering every asset.

The Engine reads
`https://github.com/makekosmos/integrations/releases/latest/download/catalog.json`,
downloads each `.kspkg`, and installs only when the pinned size and sha256
match. `revoked` entries disable installed packages; they carry the same
trust as the rest of the catalog.

## Publish

`publish.yml` is manual-dispatch only — a catalog release rewrites what every
Engine installs, so publishing is an explicit reviewed act. Sequence defaults
to latest-release + 1. Only the built-in `GITHUB_TOKEN` is used; there are no
signing keys and no other secrets.

## Checks

```powershell
npm test                              # script tests (zip, manifest schema, catalog)
node scripts/validate-manifests.mjs   # every manifest + external-apps.json
node scripts/build-packages.mjs --out out --sequence 0   # build every .kspkg
node scripts/build-catalog.mjs --packages out/packages.json --external-apps external-apps.json --sequence 1 --out out/catalog
```

Per-package Rust crates are standalone (`cargo test` inside a package dir).
For `ark-markdown-bridge`, `cargo test` resolves `kosmos-package-protocol`
from `makekosmos/cortex` at the pinned rev; if that rev is not yet on the
remote, temporarily add a `[patch."https://github.com/makekosmos/cortex"]`
entry pointing at a local cortex checkout and remove it before committing.

License: TBD.
