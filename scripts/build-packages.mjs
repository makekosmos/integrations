#!/usr/bin/env node
// Builds every packages/<id> into a .kspkg (manifest.json + worker exe +
// icon.png), verifies the archive contents, and writes packages.json — the
// resolved catalog entries the catalog build consumes.
import { createHash } from "node:crypto";
import { mkdir, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { fail, validateManifest, validateExternalApps } from "./manifest-schema.mjs";
import { readZip, writeZip } from "./zip-utils.mjs";

const RELEASE_BASE = "https://github.com/makekosmos/integrations/releases/download";

// Rust target triple -> catalog platform. The catalog row carries the
// platform explicitly because a .kspkg wraps a compiled worker binary — one
// archive can never serve two operating systems.
const TARGET_PLATFORMS = new Map([
  ["x86_64-pc-windows-msvc", { os: "windows", arch: "x86_64" }],
  ["aarch64-pc-windows-msvc", { os: "windows", arch: "arm64" }],
  ["x86_64-apple-darwin", { os: "macos", arch: "x86_64" }],
  ["aarch64-apple-darwin", { os: "macos", arch: "arm64" }],
]);

function defaultTarget() {
  if (process.platform === "win32" && process.arch === "x64") return "x86_64-pc-windows-msvc";
  if (process.platform === "darwin" && process.arch === "arm64") return "aarch64-apple-darwin";
  fail("no default target on this host — pass --target <rust-triple>");
}

function parseArgs(argv) {
  const args = { validateOnly: false };
  for (let i = 2; i < argv.length; i += 1) {
    const flag = argv[i];
    if (flag === "--validate-only") {
      args.validateOnly = true;
      continue;
    }
    if (!flag.startsWith("--") || i + 1 >= argv.length || argv[i + 1].startsWith("--")) {
      fail(`missing value for ${flag}`);
    }
    args[flag.slice(2)] = argv[++i];
  }
  if (!args.out) fail("required argument --out");
  return args;
}

function runCargo(cargoToml, binary, targetDir, target, packageDir) {
  const result = spawnSync(
    process.env.CARGO ?? "cargo",
    [
      "build",
      "--manifest-path",
      cargoToml,
      "--bin",
      binary,
      "--release",
      "--locked",
      "--target",
      target,
      "--target-dir",
      targetDir,
    ],
    {
      env: {
        ...process.env,
        RUSTFLAGS: [
          process.env.RUSTFLAGS,
          target.endsWith("-windows-msvc") ? "-C link-arg=/Brepro" : "",
          target.endsWith("-windows-msvc")
            ? `--remap-path-prefix=${path.resolve(packageDir)}=/integrations`
            : "",
        ]
          .filter(Boolean)
          .join(" "),
      },
      stdio: "inherit",
      windowsHide: true,
    },
  );
  if (result.status !== 0) fail(`cargo build failed for ${binary}`);
}

function declaresPlatform(manifest, platform) {
  // The manifest is the source of truth for what a package release supports;
  // building for an undeclared platform would ship bytes the manifest does
  // not stand behind.
  return manifest.targets.some(
    (target) =>
      target.runtime === "worker" &&
      Array.isArray(target.os) &&
      target.os.includes(platform.os) &&
      (!Array.isArray(target.arch) || target.arch.includes(platform.arch)),
  );
}

async function buildPackage(
  repoRoot,
  dir,
  externalIds,
  out,
  sequence,
  target,
  platform,
  validateOnly,
) {
  const packageDir = path.join(repoRoot, "packages", dir);
  const manifestPath = path.join(packageDir, "manifest.json");
  const manifestBytes = await readFile(manifestPath).catch(() => fail(`${dir}: manifest.json is missing`));
  const manifest = JSON.parse(manifestBytes);
  validateManifest(manifest, { packageDir, externalIds });
  const archiveName = `${manifest.id}-${manifest.version}-${platform.os}-${platform.arch}.kspkg`;
  if (!declaresPlatform(manifest, platform)) {
    fail(`${dir}: manifest targets do not declare ${platform.os}/${platform.arch}`);
  }
  if (validateOnly) return { manifest, artifact: archiveName };
  const stage = path.join(out, "stage", dir);
  const targetDir = path.join(stage, "build");
  await rm(stage, { recursive: true, force: true });
  await mkdir(stage, { recursive: true });
  const binary = manifest.entrypoint.slice(0, -4);
  runCargo(path.join(packageDir, "Cargo.toml"), binary, targetDir, target, repoRoot);
  const executable = path.join(targetDir, target, "release", manifest.entrypoint);
  const info = await stat(executable).catch(() => null);
  if (!info?.isFile() || info.size === 0) fail(`${dir}: built worker is missing: ${executable}`);
  const archive = path.join(out, archiveName);
  writeZip(archive, [
    { name: "manifest.json", data: manifestBytes },
    { name: manifest.entrypoint, data: await readFile(executable) },
    { name: manifest.icon, data: await readFile(path.join(packageDir, manifest.icon)) },
  ]);
  const entries = readZip(archive)
    .filter((entry) => !entry.isDir)
    .map((entry) => entry.name)
    .sort();
  if (JSON.stringify(entries) !== JSON.stringify([manifest.icon, "manifest.json", manifest.entrypoint].sort())) {
    fail(`${dir}: archive must contain only manifest, worker and icon`);
  }
  const bytes = await readFile(archive);
  return {
    manifest,
    os: platform.os,
    arch: platform.arch,
    url: `${RELEASE_BASE}/catalog-${sequence}/${archiveName}`,
    sha256: createHash("sha256").update(bytes).digest("hex"),
    size: bytes.length,
    artifact: archiveName,
  };
}

async function main() {
  const args = parseArgs(process.argv);
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  const out = path.resolve(args.out);
  const sequence = args.sequence ?? "0";
  const target = args.target ?? defaultTarget();
  const platform = TARGET_PLATFORMS.get(target);
  if (!platform) fail(`unknown --target ${target} (no catalog platform mapping)`);
  await mkdir(out, { recursive: true });
  const external = JSON.parse(await readFile(path.join(repoRoot, "external-apps.json"), "utf8"));
  const externalIds = validateExternalApps(external);
  const dirs = (await readdir(path.join(repoRoot, "packages"), { withFileTypes: true }))
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .sort();
  const packages = [];
  for (const dir of dirs) {
    const built = await buildPackage(
      repoRoot,
      dir,
      externalIds,
      out,
      sequence,
      target,
      platform,
      args.validateOnly,
    );
    packages.push(built);
    if (!args.validateOnly) console.log(`${dir}: ${built.sha256} ${built.size}`);
  }
  if (!args.validateOnly) {
    await writeFile(
      path.join(out, "packages.json"),
      `${JSON.stringify({ schema_version: 1, packages }, null, 2)}\n`,
    );
  }
  console.log(
    `${args.validateOnly ? "Validated" : "Built"} ${packages.length} packages: ${dirs.join(", ")}`,
  );
}

main().catch((error) => {
  console.error(`[build-packages] ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
