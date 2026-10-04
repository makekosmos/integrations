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
- `<package-id>-<version>-<os>-<arch>.kspkg` — `manifest.json` + worker exe +
  `icon.png`. One artifact per platform the manifest's `targets` declare; the
  catalog entry's `archives[]` points each platform at its own bytes.
- `icon-<package-id>.png` — the Store icon for each package.
- `SHA256SUMS.txt` — `sha256sum` lines covering every asset.

The Engine reads
`https://github.com/makekosmos/integrations/releases/latest/download/catalog.json`,
downloads the `.kspkg` for its platform, and installs only when the pinned
size and sha256 match. `revoked` entries disable installed packages; they
carry the same trust as the rest of the catalog.

## Releases

**Invariant: `releases/latest` must always resolve to a catalog release.**
The Engine's catalog URL is the `releases/latest/download/catalog.json`
redirect, so nothing else may ever create a GitHub release in this repo —
no tag-based releases, no prereleases, no ad-hoc `gh release create`. The
only release producer is `publish.yml` (manual dispatch). If this ever needs
to change, pin a dedicated tag and update the Engine URL together with it.

## macOS rollout (not yet published)

All eight manifests declare Windows x86_64 and macOS arm64. Build the Apple
Silicon artifacts with:

```sh
node scripts/build-packages.mjs --out out/macos-arm64 --sequence 2 --target aarch64-apple-darwin
```

These are native Mach-O workers, despite the shared `.exe` archive entry name.
Intel macOS is not declared until its artifacts have been built and tested.
The Cortex checkout now has a macOS supervisor path, but these targets are
still not published. Before publishing, bump the changed package versions and
build both Windows and macOS legs from identical manifests; a macOS-only
`packages.json` intentionally fails the catalog's missing-platform check.

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
