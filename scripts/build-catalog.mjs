#!/usr/bin/env node
// Assembles the single catalog.json consumed by the Engine:
//   packages[]     — manifest + archive_url + sha256 + size per .kspkg
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
  const seen = new Set();
  const entries = packages.map((entry) => {
    const id = entry?.manifest?.id;
    const version = entry?.manifest?.version;
    if (!id || !version || seen.has(`${id}@${version}`)) fail(`duplicate or missing package identity: ${id}`);
    seen.add(`${id}@${version}`);
    if (!validSha256(entry.sha256) || !Number.isSafeInteger(entry.size) || entry.size <= 0) {
      fail(`${id}: invalid sha256/size`);
    }
    if (typeof entry.archive_url !== "string" || new URL(entry.archive_url).protocol !== "https:") {
      fail(`${id}: archive_url must be HTTPS`);
    }
    return {
      manifest: entry.manifest,
      archive_url: entry.archive_url,
      sha256: entry.sha256,
      size: entry.size,
    };
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
  const builtPath = path.resolve(args.packages);
  const built = JSON.parse(await readFile(builtPath, "utf8"));
  if (built?.schema_version !== 1 || !Array.isArray(built.packages)) {
    fail("packages.json: schema_version 1 document with packages is required");
  }
  const external = JSON.parse(await readFile(path.resolve(args["external-apps"]), "utf8"));
  const externalIds = validateExternalApps(external);
  const artifactsDir = path.dirname(builtPath);
  const iconAssets = [];
  for (const entry of built.packages) {
    validateManifest(entry.manifest, { externalIds });
    const iconName = `icon-${entry.manifest.id}.png`;
    // Package directories are named by provider, not manifest id, so the icon
    // is located by matching manifests.
    await copyFile(await findIcon(repoRoot, entry.manifest.id), path.join(out, iconName));
    iconAssets.push(iconName);
  }
  const catalog = buildCatalog({
    packages: built.packages,
    externalApps: external.external_apps,
    sequence,
    issuedAt: args["issued-at"] ?? new Date().toISOString(),
    expiresAt: args["expires-at"] ?? new Date(Date.now() + 366 * 24 * 3600 * 1000).toISOString(),
    revoked: args.revoked ? JSON.parse(await readFile(path.resolve(args.revoked), "utf8")).revoked : [],
  });
  const catalogBytes = Buffer.from(`${JSON.stringify(catalog, null, 2)}\n`, "utf8");
  await writeFile(path.join(out, "catalog.json"), catalogBytes);
  const sums = [];
  for (const name of [...built.packages.map((entry) => entry.artifact), ...iconAssets].sort()) {
    const bytes = await readFile(path.join(out, name)).catch(() => readFile(path.join(artifactsDir, name)));
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
    if (manifest.id === id) return path.join(repoRoot, "packages", dir.name, manifest.icon);
  }
  fail(`${id}: no package directory provides this manifest id`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(`[build-catalog] ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
  });
}
