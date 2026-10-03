// Минимальный ZIP reader + writer для Package v1 archive format.
//
// Package archive = ZIP-архив с обязательным `manifest.json` в root. Используем только
// нужные нам части ZIP-spec'а (PKZIP APPNOTE 6.3.x):
//   - End of central directory record (EOCD, signature 0x06054b50);
//   - Central directory header (CDFH, 0x02014b50);
//   - Local file header (LFH, 0x04034b50);
//   - Compression: stored (0) или deflate (8) — этого хватает для bundles
//     сделанных стандартными утилитами (Windows Compressed Folders, 7-Zip,
//     `zip` cli, electron-builder и т.д.).
//
// Использует только node:fs + node:zlib — никаких runtime-зависимостей.
//
// Безопасность: path traversal protection — отвергаем entries с
// `..`/абсолютным path/drive letter.

import { readFileSync, writeFileSync, mkdirSync, statSync, readdirSync } from "node:fs";
import path from "node:path";
import zlib from "node:zlib";

const EOCD_SIG = 0x06054b50;
const CDFH_SIG = 0x02014b50;
const LFH_SIG = 0x04034b50;
const ZIP64_EOCD_LOCATOR_SIG = 0x07064b50;

/**
 * Возвращает массив entries из .zip файла.
 * Каждая entry: { name, isDir, data: Buffer } — data уже декомпрессирована.
 */
export function readZip(zipPath) {
  const buf = readFileSync(zipPath);
  const eocd = findEOCD(buf);
  if (!eocd) throw new Error(`not a valid zip (no EOCD): ${zipPath}`);
  const { cdOffset, cdEntries } = eocd;
  const entries = [];
  let offset = cdOffset;
  for (let i = 0; i < cdEntries; i++) {
    if (offset + 46 > buf.length || buf.readUInt32LE(offset) !== CDFH_SIG) {
      throw new Error(`zip: bad central dir header at ${offset}`);
    }
    const compMethod = buf.readUInt16LE(offset + 10);
    const compSize = buf.readUInt32LE(offset + 20);
    const uncompSize = buf.readUInt32LE(offset + 24);
    const nameLen = buf.readUInt16LE(offset + 28);
    const extraLen = buf.readUInt16LE(offset + 30);
    const commentLen = buf.readUInt16LE(offset + 32);
    const lfhOffset = buf.readUInt32LE(offset + 42);
    const name = buf.subarray(offset + 46, offset + 46 + nameLen).toString("utf8");

    // Прочитать LFH чтобы пропустить его name+extra и добраться до payload'а.
    if (lfhOffset + 30 > buf.length || buf.readUInt32LE(lfhOffset) !== LFH_SIG) {
      throw new Error(`zip: bad local header at ${lfhOffset} for ${name}`);
    }
    const lfhMethod = buf.readUInt16LE(lfhOffset + 8);
    const lfhNameLen = buf.readUInt16LE(lfhOffset + 26);
    const lfhExtraLen = buf.readUInt16LE(lfhOffset + 28);
    const lfhName = buf.subarray(lfhOffset + 30, lfhOffset + 30 + lfhNameLen).toString("utf8");
    // Local and central headers must agree on name and method; otherwise the
    // bytes behind this entry can be attributed to a different filename.
    if (lfhMethod !== compMethod || lfhName !== name) {
      throw new Error(`zip: local header does not match central directory for ${name}`);
    }
    const dataStart = lfhOffset + 30 + lfhNameLen + lfhExtraLen;
    const rawData = buf.subarray(dataStart, dataStart + compSize);

    let data;
    if (name.endsWith("/") || uncompSize === 0) {
      data = Buffer.alloc(0);
    } else if (compMethod === 0) {
      data = Buffer.from(rawData);
    } else if (compMethod === 8) {
      // The declared size bounds the decode itself, not only the comparison:
      // an entry whose stream inflates past its central-directory metadata is
      // rejected before it can exhaust memory.
      data = zlib.inflateRawSync(rawData, { maxOutputLength: uncompSize });
    } else {
      throw new Error(`zip: unsupported compression method ${compMethod} for ${name}`);
    }
    if (data.length !== uncompSize) {
      throw new Error(`zip: size mismatch for ${name}: declared ${uncompSize}, decoded ${data.length}`);
    }
    entries.push({ name, isDir: name.endsWith("/"), data });
    offset += 46 + nameLen + extraLen + commentLen;
  }
  return entries;
}

function findEOCD(buf) {
  // EOCD расположен в конце файла, comment может быть до 65535 байт.
  // Ищем signature с конца. Авторитетна только последняя запись, чей comment
  // дотягивает ровно до конца файла: EOCD-сигнатура, зашитая в comment, или
  // хвостовые байты после записи делают набор entries зависимым от того, на
  // какой сигнатуре остановится reader, — такой архив отвергаем.
  const minOffset = Math.max(0, buf.length - 65557);
  let eocd = -1;
  for (let i = buf.length - 22; i >= minOffset; i--) {
    if (buf.readUInt32LE(i) !== EOCD_SIG) continue;
    if (i + 22 + buf.readUInt16LE(i + 20) !== buf.length) {
      // Последняя сигнатура, не закрывающая файл, — trailing junk, а не EOCD.
      // Более ранняя — просто байты внутри comment'а или данных.
      if (eocd < 0) return null;
      continue;
    }
    // Две self-consistent EOCD записи — неоднозначный архив.
    if (eocd >= 0) throw new Error("zip: ambiguous end-of-central-directory records");
    eocd = i;
  }
  if (eocd < 0) {
    // Заглушить unused warning.
    void ZIP64_EOCD_LOCATOR_SIG;
    return null;
  }
  const cdEntries = buf.readUInt16LE(eocd + 10);
  const cdSize = buf.readUInt32LE(eocd + 12);
  const cdOffset = buf.readUInt32LE(eocd + 16);
  // ZIP64 not supported — Package v1 archives remain intentionally small.
  if (cdEntries === 0xffff || cdOffset === 0xffffffff || cdSize === 0xffffffff) {
    throw new Error("zip: ZIP64 archives not supported");
  }
  return { cdOffset, cdEntries, cdSize };
}

/**
 * Path traversal protection. Возвращает relative POSIX path (с `/`), отвергая
 * `..`, абсолютные пути, drive letter'ы. Bad → throw.
 */
export function safeEntryName(name) {
  if (Object.prototype.toString.call(name) !== "[object String]" || name.length === 0) {
    throw new Error("zip: empty entry name");
  }
  // Normalize separators.
  const normalized = name.replace(/\\/g, "/");
  if (normalized.startsWith("/")) {
    throw new Error(`zip: absolute path in entry: ${name}`);
  }
  if (/^[a-zA-Z]:/.test(normalized)) {
    throw new Error(`zip: drive letter in entry: ${name}`);
  }
  const parts = normalized.split("/");
  for (const p of parts) {
    if (p === "..") {
      throw new Error(`zip: parent traversal in entry: ${name}`);
    }
  }
  return normalized;
}

/**
 * Извлекает все entries в target dir. Создаёт промежуточные директории.
 * Применяет safeEntryName к каждому имени.
 */
export function extractZip(zipPath, targetDir) {
  const entries = readZip(zipPath);
  mkdirSync(targetDir, { recursive: true });
  for (const e of entries) {
    const safe = safeEntryName(e.name);
    const out = path.join(targetDir, safe);
    if (e.isDir) {
      mkdirSync(out, { recursive: true });
      continue;
    }
    mkdirSync(path.dirname(out), { recursive: true });
    writeFileSync(out, e.data);
  }
}

// ---------------------------------------------------------------------------
// Writer — минимальный, stored (без сжатия) для удобства dev/test'ов.
// Используется в тестах для генерации Package v1 fixtures.
// ---------------------------------------------------------------------------

import crypto from "node:crypto";

function crc32(buf) {
  // CRC32 IEEE — таблица генерируется лениво.
  if (!crc32._table) {
    const table = new Uint32Array(256);
    for (let i = 0; i < 256; i++) {
      let c = i;
      for (let k = 0; k < 8; k++) {
        c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      }
      table[i] = c >>> 0;
    }
    crc32._table = table;
  }
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    c = (crc32._table[(c ^ buf[i]) & 0xff] ^ (c >>> 8)) >>> 0;
  }
  return (c ^ 0xffffffff) >>> 0;
}

/**
 * Создаёт ZIP (stored, без compression). entries: [{ name, data: Buffer | string }].
 */
export function writeZip(zipPath, entries) {
  const chunks = [];
  const cdEntries = [];
  let offset = 0;
  for (const e of entries) {
    const name = e.name;
    const data = Buffer.isBuffer(e.data) ? e.data : Buffer.from(String(e.data), "utf8");
    const crc = crc32(data);
    const nameBuf = Buffer.from(name, "utf8");
    const lfh = Buffer.alloc(30);
    lfh.writeUInt32LE(LFH_SIG, 0);
    lfh.writeUInt16LE(20, 4); // version needed
    lfh.writeUInt16LE(0, 6); // flags
    lfh.writeUInt16LE(0, 8); // method = stored
    lfh.writeUInt16LE(0, 10); // mod time
    lfh.writeUInt16LE(0, 12); // mod date
    lfh.writeUInt32LE(crc, 14);
    lfh.writeUInt32LE(data.length, 18);
    lfh.writeUInt32LE(data.length, 22);
    lfh.writeUInt16LE(nameBuf.length, 26);
    lfh.writeUInt16LE(0, 28); // extra
    chunks.push(lfh, nameBuf, data);
    cdEntries.push({ name: nameBuf, crc, size: data.length, offset });
    offset += lfh.length + nameBuf.length + data.length;
  }
  const cdStart = offset;
  for (const c of cdEntries) {
    const cdfh = Buffer.alloc(46);
    cdfh.writeUInt32LE(CDFH_SIG, 0);
    cdfh.writeUInt16LE(20, 4); // version made by
    cdfh.writeUInt16LE(20, 6); // version needed
    cdfh.writeUInt16LE(0, 8);
    cdfh.writeUInt16LE(0, 10);
    cdfh.writeUInt16LE(0, 12);
    cdfh.writeUInt16LE(0, 14);
    cdfh.writeUInt32LE(c.crc, 16);
    cdfh.writeUInt32LE(c.size, 20);
    cdfh.writeUInt32LE(c.size, 24);
    cdfh.writeUInt16LE(c.name.length, 28);
    cdfh.writeUInt16LE(0, 30); // extra
    cdfh.writeUInt16LE(0, 32); // comment
    cdfh.writeUInt16LE(0, 34); // disk
    cdfh.writeUInt16LE(0, 36); // internal attrs
    cdfh.writeUInt32LE(0, 38); // external attrs
    cdfh.writeUInt32LE(c.offset, 42);
    chunks.push(cdfh, c.name);
    offset += cdfh.length + c.name.length;
  }
  const cdSize = offset - cdStart;
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(EOCD_SIG, 0);
  eocd.writeUInt16LE(0, 4);
  eocd.writeUInt16LE(0, 6);
  eocd.writeUInt16LE(cdEntries.length, 8);
  eocd.writeUInt16LE(cdEntries.length, 10);
  eocd.writeUInt32LE(cdSize, 12);
  eocd.writeUInt32LE(cdStart, 16);
  eocd.writeUInt16LE(0, 20);
  chunks.push(eocd);
  writeFileSync(zipPath, Buffer.concat(chunks));
}

/**
 * Рекурсивно собирает entries из директории. Возвращает [{ name, data }],
 * пригодный для writeZip. `name` — POSIX path relative от dir'а.
 */
export function entriesFromDir(dir) {
  const out = [];
  function walk(rel) {
    const abs = path.join(dir, rel);
    const stat = statSync(abs);
    if (stat.isDirectory()) {
      for (const child of readdirSync(abs)) {
        walk(rel ? `${rel}/${child}` : child);
      }
    } else {
      out.push({ name: rel.replace(/\\/g, "/"), data: readFileSync(abs) });
    }
  }
  walk("");
  return out;
}

// Suppress unused warning in some bundlers.
void crypto;
