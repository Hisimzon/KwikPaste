// Vendors a GPUI crate into third_party/gpui/<crate>: downloads the published .crate, checks its sha256
// against third_party/gpui/upstream.json, unpacks it and applies the crate's patch queue in order.
//
// Usage:
//   node scripts/gpui/vendor.mjs <crate>           rebuild third_party/gpui/<crate>
//   node scripts/gpui/vendor.mjs <crate> --check   rebuild in a temp dir and require a byte-for-byte match
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  cpSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, resolve } from "node:path";

const ROOT = resolve(import.meta.dirname, "..", "..");
const GPUI_DIR = join(ROOT, "third_party", "gpui");

const say = (line) => {
  process.stdout.write(`${line}\n`);
};

const fail = (lines, code = 1) => {
  process.stderr.write(`${lines.join("\n")}\n`);
  process.exit(code);
};

/**
 * Reads the crate's entry from upstream.json.
 */
const loadEntry = (name) => {
  const upstream = JSON.parse(
    readFileSync(join(GPUI_DIR, "upstream.json"), "utf8"),
  );
  const entry = upstream.crates.find((crate) => {
    return crate.name === name;
  });

  if (!entry) {
    fail([`${name} is not listed in third_party/gpui/upstream.json`]);
  }

  return entry;
};

/**
 * Downloads the .crate, verifies it and returns the directory holding the patched sources.
 */
const build = async (entry, workDir) => {
  const url = `https://static.crates.io/crates/${entry.name}/${entry.name}-${entry.version}.crate`;
  const response = await fetch(url);

  if (!response.ok) {
    fail([`${url} returned HTTP ${response.status}`]);
  }

  const bytes = Buffer.from(await response.arrayBuffer());
  const digest = createHash("sha256").update(bytes).digest("hex");

  if (digest !== entry.sha256) {
    fail([
      `${entry.name}-${entry.version}.crate has sha256 ${digest}, expected ${entry.sha256}`,
    ]);
  }

  writeFileSync(join(workDir, "crate.tar.gz"), bytes);
  execFileSync("tar", ["-xzf", "crate.tar.gz"], {
    cwd: workDir,
    stdio: "inherit",
  });

  const crateDir = join(workDir, `${entry.name}-${entry.version}`);
  rmSync(join(crateDir, ".cargo-ok"), { force: true });

  // With core.autocrlf on, git apply writes the touched files with CRLF; keep crates.io's bytes.
  for (const patch of entry.patches) {
    const patchPath = join(GPUI_DIR, "patches", entry.name, patch);
    const args = [
      "-c",
      "core.autocrlf=false",
      "-c",
      "core.eol=lf",
      "apply",
      "--whitespace=nowarn",
      patchPath,
    ];
    execFileSync("git", args, {
      cwd: crateDir,
      stdio: "inherit",
    });
  }

  return crateDir;
};

/**
 * Lists every file under `dir` as a path relative to it, with forward slashes.
 */
const listFiles = (dir) => {
  return readdirSync(dir, { recursive: true, withFileTypes: true })
    .filter((item) => {
      return item.isFile();
    })
    .map((item) => {
      return relative(dir, join(item.parentPath, item.name)).replaceAll(
        "\\",
        "/",
      );
    })
    .sort();
};

/**
 * Returns the files that differ between two trees, including ones present on only one side.
 */
const compareTrees = (expected, actual) => {
  const expectedFiles = listFiles(expected);
  const actualFiles = listFiles(actual);
  const all = [...new Set([...expectedFiles, ...actualFiles])].sort();

  return all.filter((file) => {
    if (!expectedFiles.includes(file) || !actualFiles.includes(file)) {
      return true;
    }

    return !readFileSync(join(expected, file)).equals(
      readFileSync(join(actual, file)),
    );
  });
};

const [name, flag] = process.argv.slice(2);

if (!name) {
  fail(["usage: node scripts/gpui/vendor.mjs <crate> [--check]"], 2);
}

const entry = loadEntry(name);
const workDir = mkdtempSync(join(tmpdir(), "kwikpaste-vendor-"));
const target = join(GPUI_DIR, entry.name);

try {
  const built = await build(entry, workDir);

  if (flag === "--check") {
    const differences = compareTrees(built, target);

    if (differences.length > 0) {
      rmSync(workDir, { force: true, recursive: true });
      fail([
        `third_party/gpui/${entry.name} differs from ${entry.name} ${entry.version} + its patches:`,
        ...differences.map((file) => {
          return `  ${file}`;
        }),
      ]);
    }

    say(
      `third_party/gpui/${entry.name} matches ${entry.name} ${entry.version} + ${entry.patches.length} patches`,
    );
  } else {
    rmSync(target, { force: true, recursive: true });
    cpSync(built, target, { recursive: true });
    say(
      `vendored ${entry.name} ${entry.version} with ${entry.patches.length} patches into third_party/gpui/${entry.name}`,
    );
  }
} finally {
  rmSync(workDir, { force: true, recursive: true });
}
