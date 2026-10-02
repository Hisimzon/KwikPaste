// Checks that kwikpaste-core embeds exactly the migrations that 1.x shipped.
//
// Released databases store the sha384 of each migration as checked out on the user's platform (CRLF on
// Windows, LF on macOS). The native app opens those same databases, so every file under
// src-tauri/migrations must exist in crates/kwikpaste-core/migrations with the same git blob, the same
// checked-out bytes and the same git attributes (no eol / -text / binary rule may ever touch *.sql).
// Migrations that exist only in core must come after the last one 1.x shipped.
//
// Usage: node scripts/ci/check-migration-parity.mjs
import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";

const ROOT = resolve(import.meta.dirname, "..", "..");
const TAURI_DIR = "src-tauri/migrations";
const CORE_DIR = "crates/kwikpaste-core/migrations";

const say = (line) => {
  process.stdout.write(`${line}\n`);
};

const git = (args) => {
  return execFileSync("git", args, { cwd: ROOT, encoding: "utf8" });
};

/**
 * Returns the tracked .sql files of a directory as a map from file name to git blob id.
 */
const trackedBlobs = (dir) => {
  const blobs = new Map();

  for (const line of git(["ls-files", "--stage", "--", dir]).split("\n")) {
    const match = line.match(/^\d+ ([0-9a-f]+) \d+\t(.+)$/);

    if (match?.[2].endsWith(".sql")) {
      blobs.set(match[2].slice(dir.length + 1), match[1]);
    }
  }

  return blobs;
};

/**
 * Returns `text` and `eol` of a file as git resolves them, e.g. `{ text: "auto", eol: "unspecified" }`.
 */
const attributes = (path) => {
  const attrs = {};

  for (const line of git(["check-attr", "text", "eol", "--", path]).split(
    "\n",
  )) {
    const match = line.match(/: (text|eol): (.+)$/);

    if (match) {
      attrs[match[1]] = match[2];
    }
  }

  return attrs;
};

const versionOf = (name) => {
  return Number.parseInt(name, 10);
};

const problems = [];
const tauri = trackedBlobs(TAURI_DIR);
const core = trackedBlobs(CORE_DIR);

if (tauri.size === 0) {
  problems.push(`no migrations are tracked under ${TAURI_DIR}`);
}

for (const dir of [TAURI_DIR, CORE_DIR]) {
  const tracked = dir === TAURI_DIR ? tauri : core;

  for (const name of readdirSync(join(ROOT, dir))) {
    if (name.endsWith(".sql") && !tracked.has(name)) {
      problems.push(`${dir}/${name} is not tracked by git`);
    }
  }
}

for (const [name, blob] of tauri) {
  const tauriPath = `${TAURI_DIR}/${name}`;
  const corePath = `${CORE_DIR}/${name}`;
  const coreBlob = core.get(name);

  if (!coreBlob) {
    problems.push(`${corePath} is missing`);
    continue;
  }

  if (coreBlob !== blob) {
    problems.push(
      `${corePath} is blob ${coreBlob}, ${tauriPath} is blob ${blob}`,
    );
    continue;
  }

  if (
    !readFileSync(join(ROOT, tauriPath)).equals(
      readFileSync(join(ROOT, corePath)),
    )
  ) {
    problems.push(`${corePath} and ${tauriPath} differ in the working tree`);
    continue;
  }

  const tauriAttrs = attributes(tauriPath);
  const coreAttrs = attributes(corePath);

  if (tauriAttrs.text !== "auto" || tauriAttrs.eol !== "unspecified") {
    problems.push(
      `${tauriPath} has text=${tauriAttrs.text} eol=${tauriAttrs.eol}, expected text=auto and no eol`,
    );
    continue;
  }

  if (coreAttrs.text !== tauriAttrs.text || coreAttrs.eol !== tauriAttrs.eol) {
    problems.push(
      `${corePath} has text=${coreAttrs.text} eol=${coreAttrs.eol}, ${tauriPath} has text=${tauriAttrs.text} eol=${tauriAttrs.eol}`,
    );
    continue;
  }

  say(`ok  ${name}  ${blob}`);
}

const lastShipped = Math.max(...[...tauri.keys()].map(versionOf));

for (const name of core.keys()) {
  if (tauri.has(name)) {
    continue;
  }

  if (versionOf(name) <= lastShipped) {
    problems.push(
      `${CORE_DIR}/${name} is not a released migration but sorts before ${String(lastShipped).padStart(4, "0")}`,
    );
    continue;
  }

  say(`new ${name}  (core only)`);
}

if (problems.length > 0) {
  process.stderr.write(
    `${problems
      .map((problem) => {
        return `error: ${problem}`;
      })
      .join("\n")}\n`,
  );
  process.exit(1);
}

say(`${tauri.size} released migrations match ${CORE_DIR}.`);
