// Proves, without pushing any tag, that the 1.x release channels can never receive a 2.x build
// (PLAN §2 rule 1):
//
//   1. release.yml (Tauri build, latest.json) is not triggered by v2+ tags, and its v2-guard refuses a
//      v2+ tag given by hand;
//   2. channel-pointer.yml (old Qiniu paths, channel-beta/nightly) skips v2+ tags;
//   3. a 2.x release never carries a latest.json asset (native-release.yml and channel-pointer-v2.yml
//      stop on one), and native-release.yml only runs for v2 tags.
//
// The tag filters are evaluated with GitHub's pattern rules; the guard snippets between
// `# >>> <name>` and `# <<< <name>` are cut out of the workflow files and run with bash, so the test runs
// the code the workflows run.
//
// Usage: node scripts/ci/check-release-guards.mjs
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const ROOT = resolve(import.meta.dirname, "..", "..");
const WORKFLOWS = join(ROOT, ".github", "workflows");
const failures = [];

const say = (line) => {
  process.stdout.write(`${line}\n`);
};

const check = (ok, label) => {
  if (ok) {
    say(`ok   ${label}`);
  } else {
    failures.push(label);
    say(`FAIL ${label}`);
  }
};

const workflow = (name) => {
  return readFileSync(join(WORKFLOWS, name), "utf8").replaceAll("\r\n", "\n");
};

/**
 * The `on.push.tags` patterns of a workflow (a plain list of quoted strings).
 */
const pushTags = (text) => {
  const match = text.match(/^ {2}push:\n {4}tags:\n((?: {6}- .*\n)+)/m);
  if (!match) {
    throw new Error("no on.push.tags list");
  }

  return match[1]
    .trim()
    .split("\n")
    .map((line) => {
      return line.trim().replace(/^- /, "").replace(/^"|"$/g, "");
    });
};

/**
 * GitHub filter pattern → RegExp: `*` any run without `/`, `**` anything, `?` / `+` quantify the
 * previous character, `[...]` a character class.
 */
const patternRegex = (pattern) => {
  let source = "";
  for (let index = 0; index < pattern.length; index += 1) {
    const char = pattern[index];
    if (char === "*") {
      if (pattern[index + 1] === "*") {
        source += ".*";
        index += 1;
      } else {
        source += "[^/]*";
      }
    } else if (char === "?" || char === "+") {
      source += char;
    } else if (char === "[") {
      const end = pattern.indexOf("]", index);
      source += pattern.slice(index, end + 1);
      index = end;
    } else {
      source += char.replace(/[.\\^$|(){}]/g, "\\$&");
    }
  }

  return new RegExp(`^${source}$`);
};

/**
 * Whether a tag push triggers a workflow with these patterns: the last matching pattern wins, a `!`
 * pattern excludes.
 */
const triggers = (patterns, tag) => {
  let matched = false;
  for (const pattern of patterns) {
    const negative = pattern.startsWith("!");
    if (patternRegex(negative ? pattern.slice(1) : pattern).test(tag)) {
      matched = !negative;
    }
  }

  return matched;
};

/**
 * The lines between `# >>> name` and `# <<< name`, dedented.
 */
const snippet = (text, name) => {
  const lines = text.split("\n");
  const start = lines.findIndex((line) => line.trim() === `# >>> ${name}`);
  const end = lines.findIndex((line) => line.trim() === `# <<< ${name}`);
  if (start < 0 || end < start) {
    throw new Error(`no ${name} snippet`);
  }
  const indent = lines[start].match(/^ */)[0].length;

  return lines
    .slice(start + 1, end)
    .map((line) => line.slice(indent))
    .join("\n");
};

const bash = () => {
  const git = "C:\\Program Files\\Git\\bin\\bash.exe";
  // On Windows a bare `bash` may be WSL's launcher; Git's bash is what GitHub's `shell: bash` uses.
  return process.platform === "win32" && existsSync(git) ? git : "bash";
};

/**
 * Runs a snippet with bash in a scratch directory; returns `{ status, output }` where output is what it
 * wrote to $GITHUB_OUTPUT.
 */
const runSnippet = (code, variables, files = {}) => {
  const dir = mkdtempSync(join(tmpdir(), "release-guard-"));
  try {
    for (const [name, content] of Object.entries(files)) {
      writeFileSync(join(dir, name), content);
    }
    const prelude = Object.entries(variables)
      .map(([name, value]) => `${name}=${JSON.stringify(value)}`)
      .join("\n");
    const script = `set -euo pipefail\nGITHUB_OUTPUT=github-output.txt\n: > "$GITHUB_OUTPUT"\n${prelude}\n${code}\n`;
    writeFileSync(join(dir, "snippet.sh"), script);
    const result = spawnSync(bash(), ["snippet.sh"], {
      cwd: dir,
      encoding: "utf8",
    });
    if (result.error) {
      throw result.error;
    }

    return {
      output: readFileSync(join(dir, "github-output.txt"), "utf8"),
      status: result.status,
    };
  } finally {
    rmSync(dir, { force: true, recursive: true });
  }
};

const ONE_X = [
  "v1.4.0",
  "v1.4.1",
  "v1.4.1-beta.1",
  "v1.10.0",
  "v1.4.2-nightly.20270101.1",
];
const TWO_PLUS = [
  "v2.0.0",
  "v2.0.0-beta.1",
  "v2.1.3",
  "v2.0.0-rc.2",
  "v3.0.0",
  "v9.9.9",
  "v10.0.0",
  "v12.0.0-rc.1",
];

// 1. release.yml
const release = workflow("release.yml");
const releaseTags = pushTags(release);
for (const tag of ONE_X) {
  check(triggers(releaseTags, tag), `release.yml builds ${tag}`);
}
for (const tag of [...TWO_PLUS, "channel-v2-stable"]) {
  check(!triggers(releaseTags, tag), `release.yml is not triggered by ${tag}`);
}
const releaseGuard = snippet(release, "v2-guard");
for (const tag of ONE_X) {
  check(
    runSnippet(releaseGuard, { tag_name: tag }).status === 0,
    `release.yml v2-guard lets ${tag} through`,
  );
}
for (const tag of [...TWO_PLUS, "vnext"]) {
  check(
    runSnippet(releaseGuard, { tag_name: tag }).status !== 0,
    `release.yml v2-guard refuses ${tag} given by hand`,
  );
}

// 2. channel-pointer.yml
const pointerGuard = snippet(workflow("channel-pointer.yml"), "v2-guard");
for (const tag of [...ONE_X, "channel-beta", "channel-v2-stable"]) {
  const result = runSnippet(pointerGuard, { tag_name: tag });
  check(
    result.status === 0 && result.output === "",
    `channel-pointer.yml handles ${tag} as before`,
  );
}
for (const tag of TWO_PLUS) {
  const result = runSnippet(pointerGuard, { tag_name: tag });
  check(
    result.status === 0 && result.output.includes("proceed=false"),
    `channel-pointer.yml skips ${tag}`,
  );
}

// 3. native-release.yml and channel-pointer-v2.yml
const native = workflow("native-release.yml");
const nativeTags = pushTags(native);
for (const tag of ["v2.0.0", "v2.0.0-beta.1", "v2.3.4"]) {
  check(triggers(nativeTags, tag), `native-release.yml builds ${tag}`);
}
for (const tag of [...ONE_X, "channel-v2-stable"]) {
  check(
    !triggers(nativeTags, tag),
    `native-release.yml is not triggered by ${tag}`,
  );
}

const pointerV2 = workflow("channel-pointer-v2.yml");
const names =
  "KwikPaste_2.0.0_x64-setup.exe\nKwikPaste_2.0.0_x64-setup.exe.sig\nlatest-v2.json\n";
for (const [file, text] of [
  ["native-release.yml", native],
  ["channel-pointer-v2.yml", pointerV2],
]) {
  const guard = snippet(text, "no-latest-json");
  check(
    runSnippet(
      guard,
      { TAG_NAME: "v2.0.0", tag_name: "v2.0.0" },
      { "asset-names.txt": names },
    ).status === 0,
    `${file} accepts a 2.x release with latest-v2.json`,
  );
  check(
    runSnippet(
      guard,
      { TAG_NAME: "v2.0.0", tag_name: "v2.0.0" },
      { "asset-names.txt": `${names}latest.json\n` },
    ).status !== 0,
    `${file} stops on a latest.json asset`,
  );
}

const v1Skip = snippet(pointerV2, "v1-skip");
for (const tag of [...ONE_X, "channel-v2-stable", "channel-beta"]) {
  check(
    runSnippet(v1Skip, { tag_name: tag }).output.includes("proceed=false"),
    `channel-pointer-v2.yml skips ${tag}`,
  );
}
for (const tag of TWO_PLUS) {
  const result = runSnippet(v1Skip, { tag_name: tag });
  check(
    result.status === 0 && result.output === "",
    `channel-pointer-v2.yml publishes ${tag}`,
  );
}

if (failures.length > 0) {
  say(`${failures.length} check(s) failed.`);
  process.exit(1);
}
say("The 1.x channels never see 2.x, and 2.x never publishes latest.json.");
