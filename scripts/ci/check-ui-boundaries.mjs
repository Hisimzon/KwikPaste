// Guards the native UI layering (appendix D §2.1 and §2.4 of the rewrite plan):
//
// 1. Only crates/kwikpaste-ui may use gpui-component / gpui-base. crates/kwikpaste-app must not depend on
//    them in its Cargo.toml nor name `gpui_component` / `gpui_base` in its sources; it goes through the
//    wrappers that kwikpaste-ui exports.
// 2. Colors come from the frozen antd tokens (`KpTokens`). Outside crates/kwikpaste-ui/src/theme/ no UI
//    source may build a color from literals: `rgb(..)`, `rgba(..)`, `hsla(..)`, `hsl(..)`, `Rgba { .. }`,
//    `Hsla { .. }` or gpui's named colors (`black()`, `white()`, `red()`, ...).
//
// Comment lines are skipped, so prose may mention the forbidden names.
//
// Usage: node scripts/ci/check-ui-boundaries.mjs
import { readdirSync, readFileSync } from "node:fs";
import { join, resolve, sep } from "node:path";

const ROOT = resolve(import.meta.dirname, "..", "..");
const APP = "crates/kwikpaste-app";
const UI = "crates/kwikpaste-ui";
const THEME_DIR = `${UI}/src/theme/`;

const COMPONENT_DEPENDENCY =
  /^\s*(gpui-component|gpui-base|gpui_component|gpui_base)\b/;
const COMPONENT_PATH = /\b(gpui_component|gpui_base)\b/;
const COLOR_LITERALS = [
  /\b(rgb|rgba|hsl|hsla)\s*\(/,
  /\b(Rgba|Hsla)\s*\{/,
  /\b(black|white|red|green|blue|yellow|transparent_black|transparent_white|opaque_grey)\s*\(\s*\)/,
];

const say = (line) => {
  process.stdout.write(`${line}\n`);
};

/**
 * Returns every .rs file under a directory (paths relative to the repository root, with `/`).
 */
const rustFiles = (dir) => {
  const files = [];
  const walk = (current) => {
    for (const entry of readdirSync(join(ROOT, current), {
      withFileTypes: true,
    })) {
      const path = `${current}/${entry.name}`;
      if (entry.isDirectory()) {
        walk(path);
      } else if (entry.name.endsWith(".rs")) {
        files.push(path);
      }
    }
  };
  walk(dir);

  return files;
};

/**
 * Yields `[lineNumber, text]` for the lines of a file that are not comments, with the contents of
 * string literals blanked (clipboard data such as `"rgb(255 136 0)"` is not a UI color).
 */
function* codeLines(path) {
  const lines = readFileSync(join(ROOT, path), "utf8").split(/\r?\n/);
  for (const [index, line] of lines.entries()) {
    const trimmed = line.trimStart();
    if (
      trimmed.startsWith("//") ||
      trimmed.startsWith("*") ||
      trimmed.startsWith("/*")
    ) {
      continue;
    }
    yield [index + 1, line.replace(/"(?:\\.|[^"\\])*"/g, '""')];
  }
}

const problems = [];

for (const [number, line] of codeLines(`${APP}/Cargo.toml`)) {
  if (!line.trimStart().startsWith("#") && COMPONENT_DEPENDENCY.test(line)) {
    problems.push(
      `${APP}/Cargo.toml:${number}: kwikpaste-app must not depend on gpui-component or gpui-base`,
    );
  }
}

for (const path of rustFiles(`${APP}/src`)) {
  for (const [number, line] of codeLines(path)) {
    if (COMPONENT_PATH.test(line)) {
      problems.push(
        `${path}:${number}: use the kwikpaste-ui wrappers instead of gpui-component / gpui-base`,
      );
    }
  }
}

for (const path of [...rustFiles(`${APP}/src`), ...rustFiles(`${UI}/src`)]) {
  if (path.startsWith(THEME_DIR)) {
    continue;
  }
  for (const [number, line] of codeLines(path)) {
    if (COLOR_LITERALS.some((pattern) => pattern.test(line))) {
      problems.push(
        `${path}:${number}: color literal outside ${THEME_DIR} (take it from KpTokens)`,
      );
    }
  }
}

if (problems.length > 0) {
  for (const problem of problems) {
    say(`::error::${problem.split(sep).join("/")}`);
  }
  say(`${problems.length} UI boundary problem(s).`);
  process.exit(1);
}

say(`UI boundaries hold (${APP}, ${UI}).`);
