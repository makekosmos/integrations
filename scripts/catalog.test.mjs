import assert from "node:assert/strict";
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

function entry(id, sha256 = "a".repeat(64)) {
  return {
    manifest: manifest(id),
    archive_url: `https://github.com/makekosmos/integrations/releases/download/catalog-1/${id}-0.1.0.kspkg`,
    sha256,
    size: 1234,
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
  bad.archive_url = "http://example.com/x.kspkg";
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
    await writeFile(
      packagesFile,
      JSON.stringify({
        schema_version: 1,
        packages: [
          {
            manifest: real,
            archive_url: `https://github.com/makekosmos/integrations/releases/download/catalog-7/${real.id}-${real.version}.kspkg`,
            sha256: "c".repeat(64),
            size: 5,
            artifact: `${real.id}-${real.version}.kspkg`,
          },
        ],
      }),
    );
    await writeFile(path.join(out, `${real.id}-${real.version}.kspkg`), "bytes");
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
