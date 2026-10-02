// Checks the packaged native release assets in one directory against the 1.x asset contract.
//
//   names       KwikPaste_<v>_{x64,arm64}-setup.exe, _{x64,arm64}_portable.zip, _{aarch64,x64}.dmg,
//               _{aarch64,x64}.app.tar.gz; .sig next to setup, zip and tar.gz (the dmg is not signed).
//   .sig        base64 minisign over the raw asset bytes, made with the release key (or --pubkey).
//   portable    exactly KwikPaste/KwikPaste.exe and KwikPaste/portable.txt: the 1.x portable updater
//               takes the only .exe; the marker is the one 1.x ships.
//   .app.tar.gz the only top-level entry is KwikPaste.app/, no ./ prefix, no AppleDouble (._*), no
//               absolute or .. paths, the executable is 0755; Info.plist has the 19 keys of 1.4.0 with the
//               same identifier, executable and document types, and LSMinimumSystemVersion 10.15.
//   e2e         no binary contains the e2e-overrides sentinel.
//   size        the Windows installers stay within the budget: warning above 5.5 MiB, error above 6 MiB.
//   latest.json must not exist: old clients read GitHub's latest/download/latest.json.
//
// Usage: node scripts/ci/check-native-artifacts.mjs <dir> --version <v> [--identity production|test]
//          [--pubkey <file>] [--unsigned] [--exe <built exe or binary>]...
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

import { tarEntries, zipEntries, zipRead } from "./lib/archive.mjs";
import { parsePublicKey, verifySignature } from "./lib/minisign.mjs";
import { parsePlist } from "./lib/plist.mjs";

const ROOT = resolve(import.meta.dirname, "..", "..");
export const SENTINEL = "KWIKPASTE_E2E_OVERRIDES_ENABLED";
const MIB = 1024 * 1024;
export const SETUP_WARN_BYTES = 5.5 * MIB;
export const SETUP_MAX_BYTES = 6 * MIB;

const say = (line) => {
  process.stdout.write(`${line}\n`);
};

/**
 * JSON with object keys sorted, so equal values compare equal whatever order they were written in.
 */
const canonical = (value) => {
  return JSON.stringify(value, (_key, item) => {
    if (item && typeof item === "object" && !Array.isArray(item)) {
      return Object.fromEntries(
        Object.entries(item).sort(([a], [b]) => a.localeCompare(b)),
      );
    }
    return item;
  });
};

const isDeepEqual = (left, right) => {
  return canonical(left) === canonical(right);
};

/**
 * Checks one `.app.tar.gz`; returns problems.
 */
export const checkAppTarball = (buf, { product, version, identity }) => {
  const problems = [];
  const entries = tarEntries(buf);
  const tops = [...new Set(entries.map((entry) => entry.name.split("/")[0]))];
  if (tops.length !== 1 || tops[0] !== `${product}.app`) {
    problems.push(
      `top-level entries are ${JSON.stringify(tops)}, expected only ${product}.app`,
    );
  }
  for (const { name } of entries) {
    if (
      /(^|\/)\._/.test(name) ||
      name.startsWith("./") ||
      name.startsWith("/") ||
      /(^|\/)\.\.(\/|$)/.test(name)
    ) {
      problems.push(`unsafe entry ${name}`);
    }
  }

  const executable = entries.find(
    (entry) => entry.name === `${product}.app/Contents/MacOS/${product}`,
  );
  if (!executable || executable.type !== "0") {
    problems.push(`${product}.app/Contents/MacOS/${product} is missing`);
  } else {
    if (executable.mode !== 0o755) {
      problems.push(
        `the executable has mode ${executable.mode.toString(8)}, expected 755`,
      );
    }
    if (executable.data.includes(SENTINEL)) {
      problems.push("the executable contains the e2e-overrides sentinel");
    }
  }

  const plistEntry = entries.find(
    (entry) => entry.name === `${product}.app/Contents/Info.plist`,
  );
  if (!plistEntry) {
    problems.push("Info.plist is missing");
    return problems;
  }
  const plist = parsePlist(plistEntry.data.toString("utf8"));
  const contract = JSON.parse(
    readFileSync(join(ROOT, "packaging", "macos", "info-plist.json"), "utf8"),
  );
  const keys = Object.keys(plist).sort();
  if (!isDeepEqual(keys, contract.keys)) {
    const missing = contract.keys.filter((key) => !keys.includes(key));
    const extra = keys.filter((key) => !contract.keys.includes(key));
    problems.push(
      `Info.plist keys differ from 1.4.0: missing ${JSON.stringify(missing)}, extra ${JSON.stringify(extra)}`,
    );
  }
  for (const key of contract.versionKeys) {
    if (plist[key] !== version) {
      problems.push(
        `Info.plist ${key} is ${JSON.stringify(plist[key])}, expected ${JSON.stringify(version)}`,
      );
    }
  }
  if (identity === "production") {
    for (const [key, value] of Object.entries(contract.values)) {
      if (!isDeepEqual(plist[key], value)) {
        problems.push(
          `Info.plist ${key} is ${JSON.stringify(plist[key])}, expected ${JSON.stringify(value)}`,
        );
      }
    }
  } else if (plist.CFBundleIdentifier === contract.values.CFBundleIdentifier) {
    problems.push("a test-identity bundle uses the real CFBundleIdentifier");
  }

  return problems;
};

/**
 * Checks one portable zip; returns problems.
 */
export const checkPortableZip = (buf) => {
  const problems = [];
  const entries = zipEntries(buf);
  const names = entries.map((entry) => entry.name).sort();
  if (
    !isDeepEqual(names, ["KwikPaste/KwikPaste.exe", "KwikPaste/portable.txt"])
  ) {
    problems.push(
      `entries are ${JSON.stringify(names)}, expected KwikPaste/KwikPaste.exe and KwikPaste/portable.txt`,
    );
    return problems;
  }

  const exe = zipRead(
    buf,
    entries.find((entry) => entry.name === "KwikPaste/KwikPaste.exe"),
  );
  if (exe.toString("latin1", 0, 2) !== "MZ") {
    problems.push("KwikPaste.exe is not an executable");
  }
  if (exe.includes(SENTINEL)) {
    problems.push("KwikPaste.exe contains the e2e-overrides sentinel");
  }
  const marker = zipRead(
    buf,
    entries.find((entry) => entry.name === "KwikPaste/portable.txt"),
  );
  if (
    !marker.equals(
      readFileSync(join(ROOT, "src-tauri", "assets", "portable.txt")),
    )
  ) {
    problems.push("portable.txt differs from the marker 1.x ships");
  }

  return problems;
};

const parseArgs = (argv) => {
  const options = {
    exes: [],
    identity: "production",
    pubkey: join(ROOT, "crates", "kwikpaste-updater", "pubkey.txt"),
    signed: true,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--version") {
      options.version = argv[++index];
    } else if (arg === "--identity") {
      options.identity = argv[++index];
    } else if (arg === "--pubkey") {
      options.pubkey = argv[++index];
    } else if (arg === "--unsigned") {
      options.signed = false;
    } else if (arg === "--exe") {
      options.exes.push(argv[++index]);
    } else if (!options.dir) {
      options.dir = arg;
    } else {
      throw new Error(`unknown argument ${arg}`);
    }
  }
  if (!options.dir || !options.version) {
    throw new Error(
      "Usage: check-native-artifacts.mjs <dir> --version <v> [--identity production|test] [--pubkey <file>] [--unsigned] [--exe <path>]...",
    );
  }

  return options;
};

/**
 * Checks every asset in `dir`; returns `{ problems, warnings, notes }`.
 */
export const checkDirectory = (options) => {
  const problems = [];
  const warnings = [];
  const notes = [];
  const product =
    options.identity === "test" ? "KwikPasteBundleTest" : "KwikPaste";
  const version = options.version;
  const files = readdirSync(options.dir).filter((name) =>
    statSync(join(options.dir, name)).isFile(),
  );
  const publicKey = options.signed
    ? parsePublicKey(readFileSync(options.pubkey, "utf8"))
    : undefined;

  const checkSig = (name) => {
    if (!options.signed) {
      return;
    }
    const sigPath = join(options.dir, `${name}.sig`);
    if (!existsSync(sigPath)) {
      problems.push(`${name}.sig is missing`);
      return;
    }
    try {
      verifySignature(
        readFileSync(join(options.dir, name)),
        readFileSync(sigPath, "utf8"),
        publicKey,
      );
      notes.push(`${name}.sig verifies with key ${publicKey.keyId}`);
    } catch (error) {
      problems.push(`${name}.sig: ${error.message}`);
    }
  };

  if (files.includes("latest.json")) {
    problems.push("latest.json must never be a 2.x asset");
  }

  const known = new Set();
  const escaped = version.replaceAll(".", "\\.");
  for (const name of files) {
    const setup = name.match(
      new RegExp(`^${product}_${escaped}_(x64|arm64)-setup\\.exe$`),
    );
    const portable = name.match(
      new RegExp(`^KwikPaste_${escaped}_(x64|arm64)_portable\\.zip$`),
    );
    const tarball = name.match(
      new RegExp(`^${product}_${escaped}_(aarch64|x64)\\.app\\.tar\\.gz$`),
    );
    const dmg = name.match(
      new RegExp(`^${product}_${escaped}_(aarch64|x64)\\.dmg$`),
    );
    const path = join(options.dir, name);
    const size = statSync(path).size;

    if (setup) {
      known.add(name);
      const buf = readFileSync(path);
      if (buf.toString("latin1", 0, 2) !== "MZ") {
        problems.push(`${name} is not an executable`);
      }
      const mib = (size / MIB).toFixed(2);
      if (size > SETUP_MAX_BYTES) {
        problems.push(`${name} is ${mib} MiB, over the 6.0 MiB limit`);
      } else if (size > SETUP_WARN_BYTES) {
        warnings.push(
          `${name} is ${mib} MiB, over the 5.5 MiB target (limit 6.0 MiB)`,
        );
      }
      notes.push(`${name}: ${size} bytes (${mib} MiB)`);
      checkSig(name);
    } else if (portable) {
      known.add(name);
      for (const problem of checkPortableZip(readFileSync(path))) {
        problems.push(`${name}: ${problem}`);
      }
      notes.push(`${name}: ${size} bytes`);
      checkSig(name);
    } else if (tarball) {
      known.add(name);
      for (const problem of checkAppTarball(readFileSync(path), {
        identity: options.identity,
        product,
        version,
      })) {
        problems.push(`${name}: ${problem}`);
      }
      notes.push(`${name}: ${size} bytes`);
      checkSig(name);
    } else if (dmg) {
      known.add(name);
      if (files.includes(`${name}.sig`)) {
        problems.push(`${name}.sig exists, but 1.x never signed the dmg`);
      }
      notes.push(`${name}: ${size} bytes`);
    }
  }

  for (const name of files) {
    if (
      !known.has(name) &&
      !(name.endsWith(".sig") && known.has(name.slice(0, -4)))
    ) {
      problems.push(`unexpected file ${name}`);
    }
  }
  if (known.size === 0) {
    problems.push("no release assets found");
  }

  for (const exe of options.exes) {
    if (readFileSync(exe).includes(SENTINEL)) {
      problems.push(`${exe} contains the e2e-overrides sentinel`);
    } else {
      notes.push(`${exe}: no e2e-overrides sentinel`);
    }
  }

  return { notes, problems, warnings };
};

const main = () => {
  const options = parseArgs(process.argv.slice(2));
  const { problems, warnings, notes } = checkDirectory(options);
  for (const note of notes) {
    say(`ok  ${note}`);
  }
  for (const warning of warnings) {
    say(`::warning::${warning}`);
  }
  for (const problem of problems) {
    say(`::error::${problem}`);
  }
  if (problems.length > 0) {
    process.exit(1);
  }
};

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  try {
    main();
  } catch (error) {
    say(`::error::${error.message}`);
    process.exit(1);
  }
}
