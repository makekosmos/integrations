import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import zlib from "node:zlib";
import { readZip, writeZip } from "./zip-utils.mjs";

const LFH_SIG = Buffer.from([0x50, 0x4b, 0x03, 0x04]);
const CDFH_SIG = Buffer.from([0x50, 0x4b, 0x01, 0x02]);

// Minimal method-8 (deflate) archive — the local writer only emits stored
// entries, so inflate-path fixtures are assembled by hand.
function writeDeflateZip(file, name, payload, declaredUncomp) {
  const compressed = zlib.deflateRawSync(payload);
  const nameBuf = Buffer.from(name, "utf8");
  const crc = zlib.crc32(payload);
  const lfh = Buffer.alloc(30);
  lfh.writeUInt32LE(0x04034b50, 0);
  lfh.writeUInt16LE(8, 8);
  lfh.writeUInt32LE(crc, 14);
  lfh.writeUInt32LE(compressed.length, 18);
  lfh.writeUInt32LE(declaredUncomp, 22);
  lfh.writeUInt16LE(nameBuf.length, 26);
  const cdStart = lfh.length + nameBuf.length + compressed.length;
  const cdfh = Buffer.alloc(46);
  cdfh.writeUInt32LE(0x02014b50, 0);
  cdfh.writeUInt16LE(8, 10);
  cdfh.writeUInt32LE(crc, 16);
  cdfh.writeUInt32LE(compressed.length, 20);
  cdfh.writeUInt32LE(declaredUncomp, 24);
  cdfh.writeUInt16LE(nameBuf.length, 28);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(1, 8);
  eocd.writeUInt16LE(1, 10);
  eocd.writeUInt32LE(cdfh.length + nameBuf.length, 12);
  eocd.writeUInt32LE(cdStart, 16);
  return writeFile(file, Buffer.concat([lfh, nameBuf, compressed, cdfh, nameBuf, eocd]));
}

test("readZip round-trips a valid archive", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "zip-utils-"));
  try {
    const file = path.join(dir, "ok.zip");
    writeZip(file, [
      { name: "manifest.json", data: Buffer.from("{}") },
      { name: "a.txt", data: Buffer.from("abcd") },
    ]);
    const entries = readZip(file);
    assert.deepEqual(entries.map((entry) => entry.name), ["manifest.json", "a.txt"]);
    assert.equal(entries[1].data.toString("utf8"), "abcd");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("readZip rejects a local-header name that differs from the central entry", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "zip-utils-"));
  try {
    const file = path.join(dir, "name-mismatch.zip");
    writeZip(file, [{ name: "manifest.json", data: Buffer.from("{}") }]);
    const bytes = Buffer.from(await readFile(file));
    assert.equal(bytes.indexOf(LFH_SIG), 0);
    bytes.subarray(30, 30 + "manifest.json".length).set(Buffer.from("nanifest.jso"));
    await writeFile(file, bytes);
    assert.throws(() => readZip(file), /local header does not match central directory/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("readZip rejects a local-header method that differs from the central entry", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "zip-utils-"));
  try {
    const file = path.join(dir, "method-mismatch.zip");
    writeZip(file, [{ name: "a.txt", data: Buffer.from("abcd") }]);
    const bytes = Buffer.from(await readFile(file));
    assert.equal(bytes.indexOf(LFH_SIG), 0);
    bytes.writeUInt16LE(8, 8); // local method stored -> deflate, payload untouched
    await writeFile(file, bytes);
    assert.throws(() => readZip(file), /local header does not match central directory/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("readZip rejects a central uncompressed size that disagrees with the data", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "zip-utils-"));
  try {
    const file = path.join(dir, "size-lie.zip");
    writeZip(file, [{ name: "a.txt", data: Buffer.from("abcd") }]);
    const bytes = Buffer.from(await readFile(file));
    const cd = bytes.indexOf(CDFH_SIG);
    assert.notEqual(cd, -1);
    bytes.writeUInt32LE(100, cd + 24); // central uncompressedSize 4 -> 100
    await writeFile(file, bytes);
    assert.throws(() => readZip(file), /size mismatch/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("readZip rejects out-of-bounds offsets with clean errors", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "zip-utils-"));
  try {
    const file = path.join(dir, "oob.zip");
    writeZip(file, [{ name: "a.txt", data: Buffer.from("abcd") }]);
    const bytes = Buffer.from(await readFile(file));
    // Point the EOCD's central-directory offset far past EOF — must fail with
    // a zip error, not a raw Buffer bounds exception.
    const oob = Buffer.from(bytes);
    oob.writeUInt32LE(0x00ff_ff00, oob.length - 6);
    await writeFile(file, oob);
    assert.throws(() => readZip(file), /bad central dir header/);
    // Point the central entry's local-header offset past EOF.
    const oobLocal = Buffer.from(bytes);
    const cd = oobLocal.indexOf(CDFH_SIG);
    oobLocal.writeUInt32LE(0x00ff_ff00, cd + 42);
    await writeFile(file, oobLocal);
    assert.throws(() => readZip(file), /bad local header/);
    // A buffer smaller than an EOCD record cannot contain one.
    await writeFile(file, Buffer.from("PK"));
    assert.throws(() => readZip(file), /not a valid zip/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

// Append an archive comment (declared by the real EOCD) that embeds a second
// EOCD-shaped record — a "shadow" directory declaration a signature-only
// backward scan would stop at instead of the authoritative record.
async function embedShadowEocd(file, { count, commentLen }) {
  const bytes = Buffer.from(await readFile(file));
  const eocd = bytes.length - 22;
  const fake = Buffer.alloc(22);
  fake.writeUInt32LE(0x06054b50, 0);
  fake.writeUInt16LE(count, 10);
  fake.writeUInt32LE(bytes.readUInt32LE(eocd + 12), 12);
  fake.writeUInt32LE(bytes.readUInt32LE(eocd + 16), 16);
  fake.writeUInt16LE(commentLen, 20);
  const tail = Buffer.concat([fake, Buffer.alloc(commentLen)]);
  bytes.writeUInt16LE(tail.length, eocd + 20);
  await writeFile(file, Buffer.concat([bytes, tail]));
}

test("readZip rejects a shadow EOCD record inside the archive comment", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "zip-utils-"));
  try {
    const file = path.join(dir, "shadow.zip");
    writeZip(file, [{ name: "a.txt", data: "abcd" }, { name: "b.txt", data: "ef" }]);
    // The embedded record claims a one-entry directory and does not reach
    // EOF itself — but a scan that stops at the last signature sees it.
    await embedShadowEocd(file, { count: 1, commentLen: 0 });
    assert.throws(() => readZip(file), /not a valid zip|end-of-central-directory/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("readZip rejects a second self-consistent EOCD record as ambiguous", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "zip-utils-"));
  try {
    const file = path.join(dir, "ambiguous.zip");
    writeZip(file, [{ name: "a.txt", data: "abcd" }]);
    // The trailing record's comment reaches EOF too: two records both satisfy
    // the EOCD shape, so the entry set is ambiguous and must be rejected.
    await embedShadowEocd(file, { count: 1, commentLen: 8 });
    assert.throws(() => readZip(file), /ambiguous/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("readZip ignores an EOCD-shaped byte run stored inside an entry", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "zip-utils-"));
  try {
    const file = path.join(dir, "nested.zip");
    const inner = Buffer.alloc(22);
    inner.writeUInt32LE(0x06054b50, 0);
    inner.writeUInt16LE(1, 10);
    // The signature bytes live in entry data, not at a record position — and
    // the record it would describe does not reach EOF — so the real EOCD
    // still governs the archive.
    writeZip(file, [{ name: "a.txt", data: "abc" }, { name: "nested.bin", data: inner }]);
    assert.deepEqual(readZip(file).map((entry) => entry.name), ["a.txt", "nested.bin"]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("readZip bounds inflate output by the declared uncompressed size", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "zip-utils-"));
  try {
    // A correctly-declared deflate entry round-trips.
    const ok = path.join(dir, "ok-deflate.zip");
    const payload = Buffer.from("deflate me ".repeat(64));
    await writeDeflateZip(ok, "a.txt", payload, payload.length);
    const entries = readZip(ok);
    assert.equal(entries[0].data.toString("utf8"), payload.toString("utf8"));

    // The central directory under-declares uncompressedSize; the stream
    // inflates far beyond it. The decode must abort at the declared bound
    // instead of allocating the real expansion before the length check.
    const bomb = path.join(dir, "bomb.zip");
    await writeDeflateZip(bomb, "a.txt", Buffer.alloc(8 * 1024 * 1024, 0x41), 64);
    assert.throws(() => readZip(bomb));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

// Determinism is why this module exists at all: the catalog pins each
// .kspkg by sha256, so two builds of the same package must produce the same
// bytes (stored method, zeroed DOS timestamps, caller-ordered entries).
test("writeZip is byte-identical across runs", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "zip-utils-"));
  try {
    const entries = [
      { name: "manifest.json", data: Buffer.from("{}") },
      { name: "worker.exe", data: Buffer.alloc(4096, 7) },
      { name: "icon.png", data: Buffer.from([0x89, 0x50, 0x4e, 0x47]) },
    ];
    const first = path.join(dir, "first.zip");
    const second = path.join(dir, "second.zip");
    writeZip(first, entries);
    await new Promise((resolve) => setTimeout(resolve, 1100));
    writeZip(second, entries);
    assert.deepEqual(await readFile(first), await readFile(second));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("readZip rejects a payload whose bytes no longer match the central CRC", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "zip-utils-"));
  try {
    const file = path.join(dir, "crc.zip");
    writeZip(file, [{ name: "a.txt", data: Buffer.from("abcd") }]);
    const bytes = Buffer.from(await readFile(file));
    // Flip a payload byte; every size and offset field stays consistent, so
    // only the CRC32 can catch the corruption.
    bytes[30 + "a.txt".length] = 0x58;
    await writeFile(file, bytes);
    assert.throws(() => readZip(file), /crc mismatch/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("readZip rejects a deflate payload that decodes to different bytes than the CRC declares", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "zip-utils-"));
  try {
    const file = path.join(dir, "crc-deflate.zip");
    // Declared sizes are honest, but the central CRC belongs to another
    // payload entirely.
    const payload = Buffer.from("deflate me ".repeat(8));
    await writeDeflateZip(file, "a.txt", payload, payload.length);
    const bytes = Buffer.from(await readFile(file));
    const cd = bytes.indexOf(CDFH_SIG);
    assert.notEqual(cd, -1);
    bytes.writeUInt32LE(0xdeadbeef, cd + 16);
    await writeFile(file, bytes);
    assert.throws(() => readZip(file), /crc mismatch/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
