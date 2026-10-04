#!/usr/bin/env node
// Assembles the single catalog.json consumed by the Engine:
//   packages[]     — manifest + per-platform archives[] {os, arch, url,
//                    sha256, size} — one artifact per built platform
//   external_apps[]— storefront-only third-party app entries
//   revoked[]      — plain catalog-trusted revocations {id, version, sha256, reason}
// plus SHA256SUMS.txt (sha256sum format, like the native-apps releases) and the
// per-package icon assets the Store downloads as icon-<id>.png.
import { createHash } from "node:crypto";
import { copyFile, mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { fail, validateExternalApps, validateManifest } from "./manifest-schema.mjs";

const MAX_PACKAGES = 2000;
const MAX_EXTERNAL_APPS = 256;
const MAX_REVOCATIONS = 10000;

function parseArgs(argv) {
  const args = {};
  for (let i = 2; i < argv.length; i += 1) {
    const flag = argv[i];
    if (!flag.startsWith("--") || i + 1 >= argv.length || argv[i + 1].startsWith("--")) {
      fail(`missing value for ${flag}`);
    }
    args[flag.slice(2)] = argv[++i];
  }
  for (const name of ["packages", "external-apps", "sequence", "out"]) {
    if (!args[name]) fail(`required argument --${name}`);
  }
  return args;
}

function hash(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function validSha256(value) {
  return typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
}

function declaredPlatforms(manifest) {
  // Every (os, arch) pair a worker target stands behind. A target without an
  // arch list means every arch, which no finite archive set can satisfy —
  // so the strict coverage check below only applies to explicit pairs.
  const declared = [];
  for (const target of manifest.targets ?? []) {
    if (target?.runtime !== "worker" || !Array.isArray(target.os)) continue;
    for (const os of target.os) {
      if (Array.isArray(target.arch)) {
        for (const arch of target.arch) declared.push(`${os}/${arch}`);
      } else {
        declared.push(`${os}/*`);
      }
    }
  }
  return declared;
}

function archiveDeclared(declared, os, arch) {
  return declared.includes(`${os}/${arch}`) || declared.includes(`${os}/*`);
}

export function buildCatalog({ packages, externalApps, sequence, issuedAt, expiresAt, revoked }) {
  if (!Number.isSafeInteger(sequence) || sequence <= 0) fail("sequence must be a positive integer");
  const issued = Date.parse(issuedAt);
  const expires = Date.parse(expiresAt);
  if (!Number.isFinite(issued) || !Number.isFinite(expires) || expires <= issued) {
    fail("issued_at/expires_at must be valid timestamps with expiry after issue");
  }
  if (!Array.isArray(packages) || packages.length === 0 || packages.length > MAX_PACKAGES) {
    fail("packages must be a non-empty array");
  }
  const byRelease = new Map();
  for (const row of packages) {
    const id = row?.manifest?.id;
    const version = row?.manifest?.version;
    if (!id || !version) fail(`missing package identity: ${id}`);
    const key = `${id}@${version}`;
    const existing = byRelease.get(key);
    // Rows for one release come from different platform legs of the same
    // build — identical manifest bytes are the proof they agree.
    if (existing && JSON.stringify(existing.manifest) !== JSON.stringify(row.manifest)) {
      fail(`${key}: manifests differ between platform builds`);
    }
    if (!existing) byRelease.set(key, { manifest: row.manifest, archives: [] });
    const declared = declaredPlatforms(row.manifest);
    if (
      typeof row.os !== "string" ||
      typeof row.arch !== "string" ||
      !archiveDeclared(declared, row.os, row.arch)
    ) {
      fail(`${key}: archive ${row.os}/${row.arch} is not declared in manifest targets`);
    }
    if (!validSha256(row.sha256) || !Number.isSafeInteger(row.size) || row.size <= 0) {
      fail(`${key}: invalid sha256/size`);
    }
    // The artifact name is deterministic — build-packages emits exactly
    // <id>-<version>-<os>-<arch>.kspkg. Anything else would smuggle a path
    // into the SHA256SUMS lookup (or a newline into the sums file itself).
    const expectedArtifact = `${id}-${version}-${row.os}-${row.arch}.kspkg`;
    if (row.artifact !== expectedArtifact) {
      fail(`${key}: artifact must be ${expectedArtifact}`);
    }
    if (typeof row.url !== "string" || new URL(row.url).protocol !== "https:") {
      fail(`${key}: archive url must be HTTPS`);
    }
    const archives = byRelease.get(key).archives;
    if (archives.some((a) => a.os === row.os && a.arch === row.arch)) {
      fail(`${key}: duplicate ${row.os}/${row.arch} archive`);
    }
    archives.push({ os: row.os, arch: row.arch, url: row.url, sha256: row.sha256, size: row.size });
  }
  const entries = [...byRelease.values()].map((entry) => {
    entry.archives.sort((a, b) => `${a.os}/${a.arch}`.localeCompare(`${b.os}/${b.arch}`));
    const declared = declaredPlatforms(entry.manifest).filter((item) => !item.endsWith("/*"));
    const built = new Set(entry.archives.map((a) => `${a.os}/${a.arch}`));
    for (const platform of declared) {
      if (!built.has(platform)) {
        fail(`${entry.manifest.id}@${entry.manifest.version}: declared ${platform} has no archive`);
      }
    }
    return entry;
  });
  if (!Array.isArray(externalApps) || externalApps.length > MAX_EXTERNAL_APPS) {
    fail("external_apps must be an array");
  }
  const revocations = revoked ?? [];
  if (!Array.isArray(revocations) || revocations.length > MAX_REVOCATIONS) {
    fail("revoked must be an array");
  }
  for (const item of revocations) {
    if (
      !item ||
      typeof item.id !== "string" ||
      !item.id ||
      typeof item.version !== "string" ||
      !/^\d+\.\d+\.\d+$/.test(item.version) ||
      !validSha256(item.sha256) ||
      typeof item.reason !== "string" ||
      !item.reason
    ) {
      fail("invalid revoked entry");
    }
  }
  return {
    schema_version: 1,
    sequence,
    issued_at: new Date(issued).toISOString(),
    expires_at: new Date(expires).toISOString(),
    packages: entries,
    external_apps: externalApps,
    revoked: revocations,
  };
}

async function main() {
  const args = parseArgs(process.argv);
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  const out = path.resolve(args.out);
  await mkdir(out, { recursive: true });
  const sequence = Number(args.sequence);
  // --packages is a comma-separated list: one packages.json per platform leg.
  const rows = [];
  const artifactDirs = [];
  for (const builtPath of args.packages.split(",").map((item) => path.resolve(item))) {
    const built = JSON.parse(await readFile(builtPath, "utf8"));
    if (built?.schema_version !== 1 || !Array.isArray(built.packages)) {
      fail(`${builtPath}: schema_version 1 document with packages is required`);
    }
    artifactDirs.push(path.dirname(builtPath));
    rows.push(...built.packages);
  }
  const external = JSON.parse(await readFile(path.resolve(args["external-apps"]), "utf8"));
  const externalIds = validateExternalApps(external);
  const seenManifests = new Set();
  const iconAssets = [];
  for (const row of rows) {
    validateManifest(row.manifest, { externalIds });
    if (seenManifests.has(row.manifest.id)) continue;
    seenManifests.add(row.manifest.id);
    const iconName = `icon-${row.manifest.id}.png`;
    // Package directories are named by provider, not manifest id, so the icon
    // is located by matching manifests.
    await copyFile(await findIcon(repoRoot, row.manifest.id), path.join(out, iconName));
    iconAssets.push(iconName);
  }
  const catalog = buildCatalog({
    packages: rows,
    externalApps: external.external_apps,
    sequence,
    issuedAt: args["issued-at"] ?? new Date().toISOString(),
    expiresAt: args["expires-at"] ?? new Date(Date.now() + 366 * 24 * 3600 * 1000).toISOString(),
    revoked: args.revoked ? JSON.parse(await readFile(path.resolve(args.revoked), "utf8")).revoked : [],
  });
  const catalogBytes = Buffer.from(`${JSON.stringify(catalog, null, 2)}\n`, "utf8");
  await writeFile(path.join(out, "catalog.json"), catalogBytes);
  const sums = [];
  for (const row of rows) {
    const name = row.artifact;
    let bytes;
    for (const dir of artifactDirs) {
      bytes = await readFile(path.join(dir, name)).catch(() => undefined);
      if (bytes) break;
    }
    if (!bytes) fail(`${name}: built artifact not found next to any packages.json`);
    // The catalog pins the declared hash/size; if the artifact on disk does
    // not match them, the release would ship a catalog no Engine can verify.
    if (hash(bytes) !== row.sha256 || bytes.length !== row.size) {
      fail(`${name}: artifact bytes do not match declared sha256/size`);
    }
    sums.push(`${row.sha256}  ${name}`);
  }
  for (const name of iconAssets.sort()) {
    const bytes = await readFile(path.join(out, name));
    sums.push(`${hash(bytes)}  ${name}`);
  }
  sums.push(`${hash(catalogBytes)}  catalog.json`);
  await writeFile(path.join(out, "SHA256SUMS.txt"), `${sums.sort().join("\n")}\n`);
  console.log(`catalog.json sequence ${catalog.sequence}: ${catalog.packages.length} packages, ${catalog.external_apps.length} external apps.`);
}

async function findIcon(repoRoot, id) {
  const dirs = await readdir(path.join(repoRoot, "packages"), { withFileTypes: true });
  for (const dir of dirs.filter((entry) => entry.isDirectory())) {
    const manifestPath = path.join(repoRoot, "packages", dir.name, "manifest.json");
    const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
    // The schema fixes the icon at "icon.png"; this on-disk manifest is read
    // only to map id -> directory, so its icon field is not trusted.
    if (manifest.id === id) return path.join(repoRoot, "packages", dir.name, "icon.png");
  }
  fail(`${id}: no package directory provides this manifest id`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(`[build-catalog] ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
  });
}
