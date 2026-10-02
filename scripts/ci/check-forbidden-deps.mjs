// Checks that no release target of the native workspace pulls in rustls or ring.
//
// TLS goes through the system (SChannel on Windows, Security.framework on macOS): reqwest is used with
// default features off plus native-tls. rustls with ring (or aws-lc-rs) would add about 1 MB to the
// installer, and it sneaks in easily through a dependency's default features. `cargo tree` only
// resolves the graph, so all four targets are checked from one machine without building anything.
//
// Usage: node scripts/ci/check-forbidden-deps.mjs
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";

const ROOT = resolve(import.meta.dirname, "..", "..");
const TARGETS = [
  "x86_64-pc-windows-msvc",
  "aarch64-pc-windows-msvc",
  "x86_64-apple-darwin",
  "aarch64-apple-darwin",
];
const FORBIDDEN = ["ring", "rustls"];

const say = (line) => {
  process.stdout.write(`${line}\n`);
};

/**
 * Returns the inverted dependency tree of `krate` on `target`, or an empty string when nothing uses it.
 */
const dependents = (target, krate) => {
  const result = spawnSync(
    "cargo",
    [
      "tree",
      "--locked",
      "--workspace",
      "--target",
      target,
      "--invert",
      krate,
      "--edges",
      "normal,build,dev",
    ],
    { cwd: ROOT, encoding: "utf8" },
  );

  if (result.status !== 0) {
    // Not in the lock file at all: nothing can depend on it.
    if (result.stderr.includes("did not match any packages")) {
      return "";
    }

    throw new Error(
      `cargo tree failed for ${krate} on ${target}:\n${result.stderr}`,
    );
  }

  return result.stdout.trim();
};

let failed = false;

for (const target of TARGETS) {
  for (const krate of FORBIDDEN) {
    const tree = dependents(target, krate);

    if (tree === "") {
      say(`ok  ${target}  no ${krate}`);
      continue;
    }

    failed = true;
    say(`::error::${krate} is in the dependency graph of ${target}:`);
    say(tree);
  }
}

if (failed) {
  process.exit(1);
}

say(
  `${FORBIDDEN.join(" and ")} stay out of all ${TARGETS.length} release targets.`,
);
