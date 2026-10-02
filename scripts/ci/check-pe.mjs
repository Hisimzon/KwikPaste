// Checks the resources and imports of the native Windows exe (KwikPaste.exe).
//
// - RT_GROUP_ICON has ID 1. GPUI loads the window class icon with `LoadImageW(module, 1, IMAGE_ICON)` and
//   silently shows none when that fails; uninstall entries, shortcuts and the .kwikpastebak association
//   use the first icon group, which is the same one as long as there is only one.
// - VERSIONINFO carries the product name, company, file name and version.
// - Exactly one RT_MANIFEST: GPUI embeds it, the app must not add a second one.
// - No VC++ runtime import: the exe is linked with +crt-static so the portable zip is one file.
//
// Usage: node scripts/ci/check-pe.mjs <KwikPaste.exe> --version <semver> [--production]
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const RT_ICON = 3;
const RT_GROUP_ICON = 14;
const RT_VERSION = 16;
const RT_MANIFEST = 24;
const DIR_IMPORT = 1;
const DIR_RESOURCE = 2;
const DIR_DELAY_IMPORT = 13;

const say = (line) => {
  process.stdout.write(`${line}\n`);
};

/**
 * Parses the headers of a PE image and returns a helper to map RVAs to file offsets.
 */
export const parseHeaders = (buf) => {
  if (buf.readUInt16LE(0) !== 0x5a4d) {
    throw new Error("not an MZ executable");
  }
  const pe = buf.readUInt32LE(0x3c);
  if (buf.readUInt32LE(pe) !== 0x00004550) {
    throw new Error("missing PE signature");
  }

  const coff = pe + 4;
  const machine = buf.readUInt16LE(coff);
  const sectionCount = buf.readUInt16LE(coff + 2);
  const optionalSize = buf.readUInt16LE(coff + 16);
  const optional = coff + 20;
  const magic = buf.readUInt16LE(optional);
  const dirsOffset = magic === 0x20b ? optional + 112 : optional + 96;
  const dirCount = buf.readUInt32LE(dirsOffset - 4);

  const directory = (index) => {
    if (index >= dirCount) {
      return { rva: 0, size: 0 };
    }

    return {
      rva: buf.readUInt32LE(dirsOffset + index * 8),
      size: buf.readUInt32LE(dirsOffset + index * 8 + 4),
    };
  };

  const sections = [];
  for (let index = 0; index < sectionCount; index += 1) {
    const at = optional + optionalSize + index * 40;
    sections.push({
      rawPointer: buf.readUInt32LE(at + 20),
      rawSize: buf.readUInt32LE(at + 16),
      virtualAddress: buf.readUInt32LE(at + 12),
      virtualSize: buf.readUInt32LE(at + 8),
    });
  }

  const offsetOf = (rva) => {
    const section = sections.find((candidate) => {
      const span = Math.max(candidate.virtualSize, candidate.rawSize);
      return (
        rva >= candidate.virtualAddress && rva < candidate.virtualAddress + span
      );
    });
    if (!section) {
      throw new Error(`RVA 0x${rva.toString(16)} is outside every section`);
    }

    return rva - section.virtualAddress + section.rawPointer;
  };

  return { directory, machine, offsetOf };
};

const readAscii = (buf, offset) => {
  const end = buf.indexOf(0, offset);
  return buf.toString("latin1", offset, end);
};

/**
 * Lists the DLLs the image imports, normal and delay-loaded.
 */
export const importedDlls = (buf, headers) => {
  const dlls = [];
  const normal = headers.directory(DIR_IMPORT);
  if (normal.rva !== 0) {
    for (let at = headers.offsetOf(normal.rva); ; at += 20) {
      const nameRva = buf.readUInt32LE(at + 12);
      if (nameRva === 0 && buf.readUInt32LE(at + 16) === 0) {
        break;
      }
      dlls.push(readAscii(buf, headers.offsetOf(nameRva)));
    }
  }

  const delayed = headers.directory(DIR_DELAY_IMPORT);
  if (delayed.rva !== 0) {
    for (let at = headers.offsetOf(delayed.rva); ; at += 32) {
      const nameRva = buf.readUInt32LE(at + 4);
      if (nameRva === 0) {
        break;
      }
      dlls.push(readAscii(buf, headers.offsetOf(nameRva)));
    }
  }

  return dlls;
};

/**
 * Walks the resource tree: returns `{ [type]: [{ id, size, offset }] }` with numeric types and IDs
 * (named entries get their name as `id`).
 */
export const resources = (buf, headers) => {
  const dir = headers.directory(DIR_RESOURCE);
  const result = {};
  if (dir.rva === 0) {
    return result;
  }
  const base = headers.offsetOf(dir.rva);

  const entries = (offset) => {
    const named = buf.readUInt16LE(offset + 12);
    const ids = buf.readUInt16LE(offset + 14);
    const list = [];
    for (let index = 0; index < named + ids; index += 1) {
      const at = offset + 16 + index * 8;
      const rawName = buf.readUInt32LE(at);
      const rawData = buf.readUInt32LE(at + 4);
      let name = rawName;
      if (rawName & 0x80000000) {
        const nameAt = base + (rawName & 0x7fffffff);
        const length = buf.readUInt16LE(nameAt);
        name = buf.toString("utf16le", nameAt + 2, nameAt + 2 + length * 2);
      }
      list.push({
        directory: (rawData & 0x80000000) !== 0,
        name,
        offset: base + (rawData & 0x7fffffff),
      });
    }

    return list;
  };

  for (const type of entries(base)) {
    result[type.name] = [];
    for (const item of entries(type.offset)) {
      for (const language of entries(item.offset)) {
        const rva = buf.readUInt32LE(language.offset);
        const size = buf.readUInt32LE(language.offset + 4);
        result[type.name].push({
          id: item.name,
          offset: headers.offsetOf(rva),
          size,
        });
      }
    }
  }

  return result;
};

const align4 = (value) => {
  return (value + 3) & ~3;
};

/**
 * Parses one VS_VERSIONINFO-style block (`wLength`, `wValueLength`, `wType`, key, value, children).
 */
const versionBlock = (data, offset) => {
  const length = data.readUInt16LE(offset);
  const valueLength = data.readUInt16LE(offset + 2);
  const type = data.readUInt16LE(offset + 4);
  let at = offset + 6;
  let key = "";
  for (;;) {
    const unit = data.readUInt16LE(at);
    at += 2;
    if (unit === 0) {
      break;
    }
    key += String.fromCharCode(unit);
  }
  at = align4(at);
  const valueBytes = type === 1 ? valueLength * 2 : valueLength;
  const value = data.subarray(at, at + valueBytes);
  at = align4(at + valueBytes);

  const children = [];
  const end = offset + length;
  while (at < end) {
    const child = versionBlock(data, at);
    if (child.length === 0) {
      break;
    }
    children.push(child);
    at = align4(at + child.length);
  }

  return { children, key, length, type, value };
};

/**
 * Reads the numeric versions and the first string table of a VERSIONINFO resource.
 */
export const versionInfo = (data) => {
  const root = versionBlock(data, 0);
  if (
    root.key !== "VS_VERSION_INFO" ||
    root.value.readUInt32LE(0) !== 0xfeef04bd
  ) {
    throw new Error("VERSIONINFO is malformed");
  }
  const word = (offset) => {
    const ms = root.value.readUInt32LE(offset);
    const ls = root.value.readUInt32LE(offset + 4);
    return [ms >>> 16, ms & 0xffff, ls >>> 16, ls & 0xffff].join(".");
  };

  const strings = {};
  const fileInfo = root.children.find((child) => {
    return child.key === "StringFileInfo";
  });
  for (const table of fileInfo?.children ?? []) {
    for (const entry of table.children) {
      strings[entry.key] = entry.value
        .toString("utf16le")
        .replace(/\0.*$/s, "");
    }
  }

  return { fileVersion: word(8), productVersion: word(16), strings };
};

/**
 * Returns the list of problems with the exe at `path`.
 */
export const checkExe = (path, { version, production }) => {
  const buf = readFileSync(path);
  const headers = parseHeaders(buf);
  const problems = [];
  const table = resources(buf, headers);

  const groups = table[RT_GROUP_ICON] ?? [];
  if (!groups.some((group) => group.id === 1)) {
    problems.push(
      `RT_GROUP_ICON has no ID 1 (found ${JSON.stringify(groups.map((group) => group.id))})`,
    );
  }
  if ((table[RT_ICON] ?? []).length === 0) {
    problems.push("no RT_ICON images");
  }

  const manifests = table[RT_MANIFEST] ?? [];
  if (manifests.length !== 1) {
    problems.push(
      `expected exactly one RT_MANIFEST, found ${manifests.length}`,
    );
  }

  const versions = table[RT_VERSION] ?? [];
  if (versions.length !== 1) {
    problems.push(`expected exactly one VERSIONINFO, found ${versions.length}`);
  } else {
    const { offset, size } = versions[0];
    const info = versionInfo(buf.subarray(offset, offset + size));
    const numeric = `${version.split(/[-+]/)[0]}.0`;
    const expected = {
      CompanyName: "fastthree",
      FileDescription: production ? "KwikPaste" : "KwikPaste (native dev)",
      FileVersion: numeric,
      OriginalFilename: "KwikPaste.exe",
      ProductName: "KwikPaste",
      ProductVersion: version,
    };
    for (const [key, value] of Object.entries(expected)) {
      if (info.strings[key] !== value) {
        problems.push(
          `VERSIONINFO ${key} is ${JSON.stringify(info.strings[key])}, expected ${JSON.stringify(value)}`,
        );
      }
    }
    if (info.fileVersion !== numeric || info.productVersion !== numeric) {
      problems.push(
        `VERSIONINFO numeric versions are ${info.fileVersion} / ${info.productVersion}, expected ${numeric}`,
      );
    }
  }

  const dlls = importedDlls(buf, headers);
  const runtime = dlls.filter((dll) => {
    return /^(vcruntime\d+|msvcp\d+|ucrtbase|api-ms-win-crt-)/i.test(dll);
  });
  if (runtime.length > 0) {
    problems.push(
      `imports the C runtime (${runtime.join(", ")}): +crt-static is missing`,
    );
  }

  return { dlls, problems };
};

const main = () => {
  const args = process.argv.slice(2);
  const path = args.find((arg) => {
    return !arg.startsWith("--");
  });
  const versionIndex = args.indexOf("--version");
  const version = versionIndex >= 0 ? args[versionIndex + 1] : undefined;
  if (!path || !version) {
    say(
      "Usage: node scripts/ci/check-pe.mjs <KwikPaste.exe> --version <semver> [--production]",
    );
    process.exit(2);
  }

  const { problems, dlls } = checkExe(path, {
    production: args.includes("--production"),
    version,
  });
  say(`imports: ${dlls.join(", ")}`);
  if (problems.length > 0) {
    for (const problem of problems) {
      say(`::error::${path}: ${problem}`);
    }
    process.exit(1);
  }
  say(
    `ok  ${path}: icon group 1, VERSIONINFO ${version}, one manifest, no C runtime import`,
  );
};

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
