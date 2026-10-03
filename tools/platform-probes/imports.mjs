// Load-time imports of the Windows exe (patch W0004): the exe must still be loadable on Windows 10
// RTM (build 10240), so the minimum-version notice can run instead of a system loader error box.
//
//   node tools/platform-probes/imports.mjs [target/x86_64-pc-windows-msvc/release/KwikPaste.exe]
//
// Runs `dumpbin /imports` (found through vswhere) and fails when the exe imports
// - any DLL that Windows 10 RTM does not ship (icuuc.dll, icuin.dll: 1703+), or
// - a function the Windows SDK headers only declare for builds after 10240 (inside
//   `NTDDI_VERSION >= NTDDI_WIN10_TH2…` or `WINVER >= 0x0605…` blocks), e.g. GetDpiForWindow.
import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";

const exe = resolve(
  process.argv[2] ?? "target/x86_64-pc-windows-msvc/release/KwikPaste.exe",
);
const MISSING_ON_RTM = new Set(["icuuc.dll", "icuin.dll", "icu.dll"]);

/**
 * Path of dumpbin.exe from the newest Visual Studio installation.
 */
const findDumpbin = () => {
  const vswhere = join(
    process.env["ProgramFiles(x86)"] ?? "C:/Program Files (x86)",
    "Microsoft Visual Studio/Installer/vswhere.exe",
  );
  const install = execFileSync(
    vswhere,
    ["-latest", "-products", "*", "-property", "installationPath"],
    { encoding: "utf8" },
  ).trim();
  const tools = join(install, "VC/Tools/MSVC");
  const version = readdirSync(tools).sort().at(-1);

  return join(tools, version, "bin/Hostx64/x64/dumpbin.exe");
};

/**
 * Functions the SDK headers declare only for builds newer than Windows 10 RTM.
 */
const postRtmFunctions = () => {
  const kits = join(
    process.env["ProgramFiles(x86)"] ?? "C:/Program Files (x86)",
    "Windows Kits/10/Include",
  );
  if (!existsSync(kits)) {
    return new Map();
  }
  const sdk = join(kits, readdirSync(kits).sort().at(-1));
  const later =
    /WINVER\s*>=\s*0x060[5-9]|NTDDI_WIN10_(TH2|RS[1-5]|19H1|VB|MN|FE|CO|NI|CU|ZN|GA|GE|BR)\b|NTDDI_WIN11/;
  const found = new Map();
  for (const dir of ["um", "shared"]) {
    for (const file of readdirSync(join(sdk, dir))) {
      if (!file.toLowerCase().endsWith(".h")) {
        continue;
      }
      const lines = readFileSync(join(sdk, dir, file), "latin1").split(/\r?\n/);
      const stack = [];
      for (const line of lines) {
        const text = line.trim();
        if (/^#\s*if/.test(text)) {
          stack.push(later.test(text) && />=/.test(text));
          continue;
        }
        if (/^#\s*(else|elif)/.test(text)) {
          if (stack.length > 0) {
            stack[stack.length - 1] = false;
          }
          continue;
        }
        if (/^#\s*endif/.test(text)) {
          stack.pop();
          continue;
        }
        if (!stack.some(Boolean)) {
          continue;
        }
        const alone = /^([A-Za-z_]\w*)\s*\($/.exec(text);
        const inline =
          /(?:WINAPI|APIENTRY|STDAPI|NTAPI)\s+([A-Za-z_]\w*)\s*\(/.exec(text);
        const name = alone?.[1] ?? inline?.[1];
        if (name) {
          found.set(name, file);
        }
      }
    }
  }

  return found;
};

const dump = execFileSync(findDumpbin(), ["/nologo", "/imports", exe], {
  encoding: "utf8",
  maxBuffer: 64 * 1024 * 1024,
});
const postRtm = postRtmFunctions();
const dlls = [];
const problems = [];
let dll = "";
for (const line of dump.split(/\r?\n/)) {
  const library = /^\s+(\S+\.dll)$/i.exec(line);
  if (library) {
    dll = library[1].toLowerCase();
    dlls.push(dll);
    if (MISSING_ON_RTM.has(dll)) {
      problems.push(`${dll} (not shipped before Windows 10 1703)`);
    }
    continue;
  }
  const symbol = /^\s+[0-9A-F]+\s+(\S+)$/.exec(line);
  if (symbol && postRtm.has(symbol[1])) {
    problems.push(
      `${dll}!${symbol[1]} (declared for builds after 10240 in ${postRtm.get(symbol[1])})`,
    );
  }
}

process.stdout.write(`${exe}\nimported DLLs: ${dlls.join(", ")}\n`);
process.stdout.write(
  `checked against ${postRtm.size} post-10240 SDK declarations\n`,
);
if (problems.length > 0) {
  process.stdout.write(
    `FAILED: load-time imports newer than Windows 10 RTM:\n  ${problems.join("\n  ")}\n`,
  );
  process.exit(1);
}
process.stdout.write("PASSED: nothing newer than Windows 10 RTM is imported\n");
