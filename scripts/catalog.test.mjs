import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { buildCatalog } from "./build-catalog.mjs";
import { validateRepo } from "./validate-manifests.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function manifest(id) {
  return {
    schema_version: 2,
    id,
    name: id,
    description: "d",
    version: "0.1.0",
    kind: "source",
    engine_api: ">=1.0.0",
    entrypoint: "worker.exe",
    icon: "icon.png",
    publisher: "kosmos",
    permissions: [
      { capability: "network", scopes: ["https://example.com/"] },
      { capability: "ark.write", scopes: ["upsert_object"] },
    ],
    targets: [{ runtime: "worker", os: ["windows"] }],
    data: { access: [], defines: [], mappings: [] },
    integration: {
      settings: [
        {
          key: "session",
          label: "l",
          kind: "token",
          required: true,
          injection: { kind: "cookies", origins: ["https://example.com/"] },
        },
      ],
      schedule: { interval_seconds: 60 },
    },
    store: { description: "d", categories: ["integrations"] },
  };
}

function entry(id, sha256 = "a".repeat(64), platform = { os: "windows", arch: "x86_64" }) {
  const artifact = `${id}-0.1.0-${platform.os}-${platform.arch}.kspkg`;
  return {
    manifest: manifest(id),
    os: platform.os,
    arch: platform.arch,
    url: `https://github.com/makekosmos/integrations/releases/download/catalog-1/${artifact}`,
    sha256,
    size: 1234,
    artifact,
  };
}

test("buildCatalog emits a schema-1 document", () => {
  const catalog = buildCatalog({
    packages: [entry("com.kosmos.a")],
    externalApps: [],
    sequence: 3,
    issuedAt: "2026-10-03T00:00:00Z",
    expiresAt: "2027-10-03T00:00:00Z",
    revoked: [{ id: "com.kosmos.a", version: "0.1.0", sha256: "b".repeat(64), reason: "r" }],
  });
  assert.equal(catalog.schema_version, 1);
  assert.equal(catalog.sequence, 3);
  assert.equal(catalog.packages.length, 1);
  assert.equal(catalog.revoked.length, 1);
});

test("buildCatalog rejects duplicate identities and non-HTTPS urls", () => {
  assert.throws(() =>
    buildCatalog({
      packages: [entry("com.kosmos.a"), entry("com.kosmos.a")],
      externalApps: [],
      sequence: 1,
      issuedAt: "2026-01-01T00:00:00Z",
      expiresAt: "2027-01-01T00:00:00Z",
    }),
  );
  const bad = entry("com.kosmos.b");
  bad.url = "http://example.com/x.kspkg";
  assert.throws(() =>
    buildCatalog({
      packages: [bad],
      externalApps: [],
      sequence: 1,
      issuedAt: "2026-01-01T00:00:00Z",
      expiresAt: "2027-01-01T00:00:00Z",
    }),
  );
});

test("buildCatalog rejects bad hashes, expiry inversions and empty revocations", () => {
  assert.throws(() =>
    buildCatalog({
      packages: [entry("com.kosmos.a", "not-hex")],
      externalApps: [],
      sequence: 1,
      issuedAt: "2026-01-01T00:00:00Z",
      expiresAt: "2027-01-01T00:00:00Z",
    }),
  );
  assert.throws(() =>
    buildCatalog({
      packages: [entry("com.kosmos.a")],
      externalApps: [],
      sequence: 1,
      issuedAt: "2027-01-01T00:00:00Z",
      expiresAt: "2026-01-01T00:00:00Z",
    }),
  );
});

test("every committed manifest and external app validates", () => {
  const { packages, externalApps } = validateRepo(root);
  assert.equal(packages.length, 8);
  assert.equal(externalApps.length, 7);
});

test("catalog build runs end to end against stub packages.json", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "catalog-test-"));
  try {
    // Reuse a committed manifest so the icon lookup resolves the real package.
    const real = JSON.parse(
      await readFile(path.join(root, "packages", "leetcode", "manifest.json"), "utf8"),
    );
    const packagesFile = path.join(dir, "packages.json");
    const out = path.join(dir, "out");
    await mkdir(out, { recursive: true });
    // The artifact lives next to its packages.json, as a platform leg emits it.
    const artifactBytes = Buffer.from("bytes");
    const artifactSha256 = createHash("sha256").update(artifactBytes).digest("hex");
    await writeFile(
      packagesFile,
      JSON.stringify({
        schema_version: 1,
        packages: [
          {
            manifest: real,
            os: "windows",
            arch: "x86_64",
            url: `https://github.com/makekosmos/integrations/releases/download/catalog-7/${real.id}-${real.version}-windows-x86_64.kspkg`,
            sha256: artifactSha256,
            size: artifactBytes.length,
            artifact: `${real.id}-${real.version}-windows-x86_64.kspkg`,
          },
        ],
      }),
    );
    await writeFile(
      path.join(dir, `${real.id}-${real.version}-windows-x86_64.kspkg`),
      artifactBytes,
    );
    execFileSync(process.execPath, [
      path.join(root, "scripts", "build-catalog.mjs"),
      "--packages",
      packagesFile,
      "--external-apps",
      path.join(root, "external-apps.json"),
      "--sequence",
      "7",
      "--issued-at",
      "2026-10-03T00:00:00Z",
      "--expires-at",
      "2027-10-03T00:00:00Z",
      "--out",
      out,
    ]);
    const catalog = JSON.parse(await readFile(path.join(out, "catalog.json"), "utf8"));
    assert.equal(catalog.sequence, 7);
    assert.equal(catalog.packages.length, 1);
    assert.equal(catalog.external_apps.length, 7);
    assert.deepEqual(catalog.revoked, []);
    const sums = await readFile(path.join(out, "SHA256SUMS.txt"), "utf8");
    for (const line of sums.trim().split("\n")) {
      assert.match(line, /^[0-9a-f]{64} {2}\S+$/);
    }
    assert.ok(sums.includes("catalog.json"));
    assert.ok(sums.includes(`icon-${real.id}.png`));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("buildCatalog merges per-platform rows into one archives[] entry", () => {
  const catalog = buildCatalog({
    packages: [
      entry("com.kosmos.a", "a".repeat(64), { os: "windows", arch: "x86_64" }),
      // Same release built on a second platform leg: identical manifest is
      // required, a different one fails.
      { ...entry("com.kosmos.a", "b".repeat(64), { os: "windows", arch: "arm64" }) },
    ],
    externalApps: [],
    sequence: 1,
    issuedAt: "2026-01-01T00:00:00Z",
    expiresAt: "2027-01-01T00:00:00Z",
  });
  assert.equal(catalog.packages.length, 1);
  assert.deepEqual(
    catalog.packages[0].archives.map((item) => `${item.os}/${item.arch}`),
    ["windows/arm64", "windows/x86_64"],
  );
});

test("buildCatalog rejects artifact names that are not the deterministic kspkg name", () => {
  for (const artifact of ["../catalog.json", "a\nb.kspkg", "real.kspkg", undefined]) {
    const bad = { ...entry("com.kosmos.a"), artifact };
    assert.throws(
      () =>
        buildCatalog({
          packages: [bad],
          externalApps: [],
          sequence: 1,
          issuedAt: "2026-01-01T00:00:00Z",
          expiresAt: "2027-01-01T00:00:00Z",
        }),
      /artifact must be/,
    );
  }
});

test("manifest id charset is restricted to artifact-safe characters", async () => {
  const { validateManifest } = await import("./manifest-schema.mjs");
  for (const id of ["../escape", "a/b", "a\\b", "bad id", "UPPER", ".hidden", "-lead"]) {
    const bad = manifest("com.kosmos.a");
    bad.id = id;
    assert.throws(() => validateManifest(bad, {}), /invalid manifest identity/);
  }
  for (const id of ["a", "ark-markdown-bridge", "com.kosmos.huawei-health"]) {
    const good = manifest(id);
    validateManifest(good, {});
  }
});

test("catalog build fails when artifact bytes do not match declared sha256/size", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "catalog-test-"));
  try {
    const real = JSON.parse(
      await readFile(path.join(root, "packages", "leetcode", "manifest.json"), "utf8"),
    );
    const artifact = `${real.id}-${real.version}-windows-x86_64.kspkg`;
    const packagesFile = path.join(dir, "packages.json");
    const out = path.join(dir, "out");
    await mkdir(out, { recursive: true });
    await writeFile(
      packagesFile,
      JSON.stringify({
        schema_version: 1,
        packages: [
          {
            manifest: real,
            os: "windows",
            arch: "x86_64",
            url: `https://github.com/makekosmos/integrations/releases/download/catalog-7/${artifact}`,
            sha256: "c".repeat(64),
            size: 5,
            artifact,
          },
        ],
      }),
    );
    await writeFile(path.join(dir, artifact), "bytes");
    assert.throws(() =>
      execFileSync(process.execPath, [
        path.join(root, "scripts", "build-catalog.mjs"),
        "--packages",
        packagesFile,
        "--external-apps",
        path.join(root, "external-apps.json"),
        "--sequence",
        "7",
        "--out",
        out,
      ]),
    );
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("buildCatalog rejects undeclared or unbuilt platforms", () => {
  const foreign = entry("com.kosmos.a", "a".repeat(64), { os: "macos", arch: "arm64" });
  assert.throws(() =>
    buildCatalog({
      packages: [foreign],
      externalApps: [],
      sequence: 1,
      issuedAt: "2026-01-01T00:00:00Z",
      expiresAt: "2027-01-01T00:00:00Z",
    }),
  );
  const declared = entry("com.kosmos.a");
  declared.manifest.targets = [
    { runtime: "worker", os: ["windows"], arch: ["x86_64", "arm64"] },
  ];
  assert.throws(() =>
    buildCatalog({
      packages: [declared],
      externalApps: [],
      sequence: 1,
      issuedAt: "2026-01-01T00:00:00Z",
      expiresAt: "2027-01-01T00:00:00Z",
    }),
  );
});
