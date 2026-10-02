// Minimal readers for the zip and tar.gz release archives, so the checks run the same on every runner
// (and on a developer machine) without unzip/tar quirks.
import { gunzipSync, inflateRawSync } from "node:zlib";

/**
 * Entries of a zip archive from its central directory: `{ name, method, size, compressedSize, localOffset }`.
 */
export const zipEntries = (buf) => {
  let end = -1;
  for (
    let at = buf.length - 22;
    at >= Math.max(0, buf.length - 22 - 0xffff);
    at -= 1
  ) {
    if (buf.readUInt32LE(at) === 0x06054b50) {
      end = at;
      break;
    }
  }
  if (end < 0) {
    throw new Error("not a zip archive (no end of central directory)");
  }

  const count = buf.readUInt16LE(end + 10);
  let at = buf.readUInt32LE(end + 16);
  const entries = [];
  for (let index = 0; index < count; index += 1) {
    if (buf.readUInt32LE(at) !== 0x02014b50) {
      throw new Error("corrupt zip central directory");
    }
    const nameLength = buf.readUInt16LE(at + 28);
    const extraLength = buf.readUInt16LE(at + 30);
    const commentLength = buf.readUInt16LE(at + 32);
    entries.push({
      compressedSize: buf.readUInt32LE(at + 20),
      localOffset: buf.readUInt32LE(at + 42),
      method: buf.readUInt16LE(at + 10),
      name: buf.toString("utf8", at + 46, at + 46 + nameLength),
      size: buf.readUInt32LE(at + 24),
    });
    at += 46 + nameLength + extraLength + commentLength;
  }

  return entries;
};

/**
 * The uncompressed bytes of one zip entry (stored or deflated).
 */
export const zipRead = (buf, entry) => {
  const at = entry.localOffset;
  if (buf.readUInt32LE(at) !== 0x04034b50) {
    throw new Error(`corrupt local header for ${entry.name}`);
  }
  const start = at + 30 + buf.readUInt16LE(at + 26) + buf.readUInt16LE(at + 28);
  const data = buf.subarray(start, start + entry.compressedSize);
  if (entry.method === 0) {
    return data;
  }
  if (entry.method === 8) {
    return inflateRawSync(data);
  }

  throw new Error(`unsupported zip method ${entry.method} for ${entry.name}`);
};

const field = (block, start, length) => {
  const raw = block.subarray(start, start + length);
  const end = raw.indexOf(0);
  return raw.toString("utf8", 0, end < 0 ? raw.length : end);
};

const octal = (block, start, length) => {
  const text = field(block, start, length).trim();
  return text === "" ? 0 : Number.parseInt(text, 8);
};

/**
 * Entries of a .tar.gz: `{ name, type, mode, size, data }`. Handles ustar prefixes, GNU long names and
 * PAX `path` records (what the tar crate and bsdtar write).
 */
export const tarEntries = (gz) => {
  const buf = gunzipSync(gz);
  const entries = [];
  let longName;
  let paxPath;
  for (let at = 0; at + 512 <= buf.length; ) {
    const header = buf.subarray(at, at + 512);
    if (header.every((byte) => byte === 0)) {
      break;
    }
    const size = octal(header, 124, 12);
    const type = String.fromCharCode(header[156] || 0x30);
    const data = buf.subarray(at + 512, at + 512 + size);
    at += 512 + Math.ceil(size / 512) * 512;

    if (type === "L") {
      longName = data.toString("utf8").replace(/\0+$/, "");
      continue;
    }
    if (type === "x") {
      const match = data.toString("utf8").match(/^\d+ path=(.*)$/m);
      paxPath = match?.[1];
      continue;
    }
    if (type === "g") {
      continue;
    }

    let name = field(header, 0, 100);
    if (field(header, 257, 5) === "ustar") {
      const prefix = field(header, 345, 155);
      if (prefix !== "") {
        name = `${prefix}/${name}`;
      }
    }
    name = paxPath ?? longName ?? name;
    longName = undefined;
    paxPath = undefined;
    entries.push({
      data,
      mode: octal(header, 100, 8) & 0o7777,
      name,
      size,
      type,
    });
  }

  return entries;
};
