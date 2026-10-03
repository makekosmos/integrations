#!/usr/bin/env node
// Validates every packages/<id>/manifest.json plus external-apps.json.
// Runs in the quality workflow and locally: `npm run validate`.
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { fail, validateExternalApps, validateManifest } from "./manifest-schema.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

export function validateRepo(repoRoot = root) {
  const external = JSON.parse(readFileSync(path.join(repoRoot, "external-apps.json"), "utf8"));
  const externalIds = validateExternalApps(external);
  const packagesDir = path.join(repoRoot, "packages");
  const dirs = readdirSync(packagesDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .sort();
  if (dirs.length === 0) fail("no packages found");
  const ids = new Set();
  for (const dir of dirs) {
    const packageDir = path.join(packagesDir, dir);
    const manifestPath = path.join(packageDir, "manifest.json");
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    validateManifest(manifest, { packageDir, externalIds });
    if (ids.has(manifest.id)) fail(`${dir}: duplicate package id ${manifest.id}`);
    ids.add(manifest.id);
  }
  return { packages: [...ids], externalApps: [...externalIds] };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const { packages, externalApps } = validateRepo();
    console.log(`Validated ${packages.length} packages, ${externalApps.length} external apps.`);
  } catch (error) {
    console.error(`[validate] ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
  }
}
