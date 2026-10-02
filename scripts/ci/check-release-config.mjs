// Checks that the native app's bundle configuration keeps the install identity of 1.x, without building
// anything: crates/kwikpaste-app/tauri.conf.json against src-tauri/tauri*.conf.json, the updater key, the
// macOS Info.plist additions, and the exact Tauri CLI pin (2.12 changed how the NSIS installer closes a
// running app).
//
// Usage: node scripts/ci/check-release-config.mjs
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";

const ROOT = resolve(import.meta.dirname, "..", "..");
const CLI_VERSION = "2.11.4";

const say = (line) => {
  process.stdout.write(`${line}\n`);
};

const readJson = (...parts) => {
  return JSON.parse(readFileSync(join(ROOT, ...parts), "utf8"));
};

const same = (left, right) => {
  return JSON.stringify(left) === JSON.stringify(right);
};

const problems = [];
const expect = (label, actual, expected) => {
  if (!same(actual, expected)) {
    problems.push(
      `${label} is ${JSON.stringify(actual)}, expected ${JSON.stringify(expected)}`,
    );
  }
};

const native = readJson("crates", "kwikpaste-app", "tauri.conf.json");
const legacy = readJson("src-tauri", "tauri.conf.json");
const legacyWindows = readJson("src-tauri", "tauri.windows.conf.json");

for (const key of ["identifier", "productName", "mainBinaryName"]) {
  expect(key, native[key], legacy[key]);
}
for (const key of [
  "fileAssociations",
  "copyright",
  "license",
  "shortDescription",
  "publisher",
]) {
  expect(`bundle.${key}`, native.bundle[key], legacy.bundle[key]);
}
expect(
  "bundle.createUpdaterArtifacts",
  native.bundle.createUpdaterArtifacts,
  true,
);
expect(
  "bundle.licenseFile",
  resolve(ROOT, "crates", "kwikpaste-app", native.bundle.licenseFile),
  resolve(ROOT, "src-tauri", legacy.bundle.licenseFile),
);
for (const key of ["installMode", "languages", "displayLanguageSelector"]) {
  expect(
    `bundle.windows.nsis.${key}`,
    native.bundle.windows.nsis[key],
    legacyWindows.bundle.windows.nsis[key],
  );
}
expect(
  "bundle.windows.webviewInstallMode",
  native.bundle.windows.webviewInstallMode,
  { type: "skip" },
);
expect(
  "bundle.macOS.minimumSystemVersion",
  native.bundle.macOS?.minimumSystemVersion,
  "10.15",
);

const pubkey = readFileSync(
  join(ROOT, "crates", "kwikpaste-updater", "pubkey.txt"),
  "utf8",
).trim();
expect(
  "plugins.updater.pubkey",
  native.plugins.updater.pubkey,
  legacy.plugins.updater.pubkey,
);
expect(
  "crates/kwikpaste-updater/pubkey.txt",
  pubkey,
  legacy.plugins.updater.pubkey,
);

const plist = (...parts) => {
  return readFileSync(join(ROOT, ...parts), "utf8")
    .replace(/\s+/g, " ")
    .trim();
};
if (
  plist("crates", "kwikpaste-app", "Info.plist") !==
  plist("src-tauri", "Info.plist")
) {
  problems.push(
    "crates/kwikpaste-app/Info.plist differs from src-tauri/Info.plist",
  );
}

const manifest = readJson("package.json");
expect(
  "@tauri-apps/cli in package.json",
  manifest.devDependencies["@tauri-apps/cli"],
  CLI_VERSION,
);
const lock = readFileSync(join(ROOT, "pnpm-lock.yaml"), "utf8");
const locked = lock.match(
  /'@tauri-apps\/cli':\s*\n\s*specifier: (\S+)\s*\n\s*version: (\S+)/,
);
expect("@tauri-apps/cli in pnpm-lock.yaml", locked && [locked[1], locked[2]], [
  CLI_VERSION,
  CLI_VERSION,
]);

if (problems.length > 0) {
  for (const problem of problems) {
    say(`::error::${problem}`);
  }
  process.exit(1);
}
say(
  `ok  native bundle config keeps the 1.x identity, updater key and Info.plist; Tauri CLI pinned to ${CLI_VERSION}`,
);
