// Checks the NSIS script that `tauri bundle` rendered (target/<triple>/release/nsis/<arch>/) against the
// install identity 1.x shipped with. 2.x installs over 1.x and keeps its data only if these stay the same:
// product and binary name, identifier, publisher (the `Software\fastthree` key), the uninstall key, the
// install mode, the start menu shortcut and the .kwikpastebak association.
//
// It also checks the assumptions packaging/windows/installer-hooks.nsh makes about the template of the
// pinned CLI: the hooks file is included, its graceful `--quit` preinstall hook still precedes
// `CheckIfAppIsRunning`, `.onInit` still inserts SetContext (where the OS gate runs), and the original
// SetContext body has not changed.
//
// --identity test checks the opposite for a throwaway package: nothing of the real identity is left, so
// installing it on a developer machine cannot touch the installed 1.x.
//
// Usage: node scripts/ci/check-nsis-render.mjs <dir with installer.nsi and utils.nsh> --version <v> --arch <x64|arm64> [--identity production|test]
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const say = (line) => {
  process.stdout.write(`${line}\n`);
};

export const PRODUCTION = {
  BUNDLEID: "com.fastthree.kwikpaste",
  MAINBINARYNAME: "KwikPaste",
  MANUFACTURER: "fastthree",
  PRODUCTNAME: "KwikPaste",
};

const SHARED = {
  ALLOWDOWNGRADES: "true",
  DISPLAYLANGUAGESELECTOR: "true",
  INSTALLMODE: "both",
  INSTALLWEBVIEW2MODE: "",
  MANUKEY: "Software\\${MANUFACTURER}",
  MANUPRODUCTKEY: "${MANUKEY}\\${PRODUCTNAME}",
  STARTMENUFOLDER: "",
  UNINSTKEY:
    "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${PRODUCTNAME}",
};

// SetContext as tauri-bundler 2.9.4 ships it in utils.nsh; installer-hooks.nsh repeats this body.
const SET_CONTEXT = `!macro SetContext
  !if "\${INSTALLMODE}" == "currentUser"
    SetShellVarContext current
  !else if "\${INSTALLMODE}" == "perMachine"
    SetShellVarContext all
  !endif

  \${If} \${RunningX64}
    !if "\${ARCH}" == "x64"
      SetRegView 64
    !else if "\${ARCH}" == "arm64"
      SetRegView 64
    !else
      SetRegView 32
    !endif
  \${EndIf}
!macroend`;

const squash = (text) => {
  return text.replace(/\s+/g, " ").trim();
};

/**
 * The `!define NAME "value"` lines at the top level of the rendered script.
 */
export const defines = (script) => {
  const result = {};
  for (const match of script.matchAll(/^!define (\w+) "(.*)"\s*$/gm)) {
    result[match[1]] ??= match[2];
  }

  return result;
};

/**
 * The body of `Function <name>` … `FunctionEnd`.
 */
const functionBody = (script, name) => {
  const header = new RegExp(
    `^Function ${name.replace(/\./g, "\\.")}\\r?$`,
    "m",
  ).exec(script);
  if (!header) {
    return undefined;
  }

  return script.slice(
    header.index,
    script.indexOf("FunctionEnd", header.index),
  );
};

/**
 * Returns the list of problems with a rendered installer.nsi / utils.nsh pair.
 */
export const checkRender = (script, utils, { version, arch, identity }) => {
  const problems = [];
  const values = defines(script);
  const expect = (name, value) => {
    if (values[name] !== value) {
      problems.push(
        `${name} is ${JSON.stringify(values[name])}, expected ${JSON.stringify(value)}`,
      );
    }
  };

  for (const [name, value] of Object.entries(SHARED)) {
    expect(name, value);
  }
  expect("VERSION", version);
  expect("ARCH", arch);

  if (identity === "production") {
    for (const [name, value] of Object.entries(PRODUCTION)) {
      expect(name, value);
    }
    if (
      !script.includes(
        '!insertmacro APP_ASSOCIATE "kwikpastebak" "快贴备份" "快贴历史备份"',
      )
    ) {
      problems.push("the .kwikpastebak association is missing or changed");
    }
  } else {
    for (const [name, value] of Object.entries(PRODUCTION)) {
      if (values[name] === value) {
        problems.push(
          `${name} is the real ${JSON.stringify(value)} in a test-identity package`,
        );
      }
    }
    if (/APP_ASSOCIATE "kwikpastebak"/.test(script)) {
      problems.push("a test-identity package must not register .kwikpastebak");
    }
  }

  for (const language of ["English", "SimpChinese"]) {
    if (!script.includes(`!insertmacro MUI_LANGUAGE "${language}"`)) {
      problems.push(`language ${language} is missing`);
    }
  }

  const hooks = script.match(
    /^!include "(.+installer-hooks\.nsh|.+os-gate-hooks\.nsh)"\s*$/m,
  );
  if (!hooks) {
    problems.push("the installer hooks are not included");
  }

  const onInit = functionBody(script, ".onInit");
  if (!onInit) {
    problems.push(".onInit is missing");
  } else {
    const context = onInit.indexOf("!insertmacro SetContext");
    const multiUser = onInit.indexOf("!insertmacro MULTIUSER_INIT");
    if (context < 0) {
      problems.push(
        ".onInit no longer inserts SetContext, so the OS gate would not run",
      );
    } else if (multiUser >= 0 && multiUser < context) {
      problems.push(".onInit inserts MULTIUSER_INIT before SetContext");
    }
    if (!onInit.includes('${GetOptions} $CMDLINE "/UPDATE" $UpdateMode')) {
      problems.push(".onInit no longer reads /UPDATE");
    }
  }
  if (!functionBody(script, "un.onInit")?.includes("!insertmacro SetContext")) {
    problems.push("un.onInit no longer inserts SetContext");
  }
  if (
    !script.includes(
      '!insertmacro CheckIfAppIsRunning "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"',
    )
  ) {
    problems.push("the installer no longer closes a running KwikPaste.exe");
  }
  const preinstall = script.match(
    /^!macro NSIS_HOOK_PREINSTALL\r?\n([\s\S]*?)^!macroend\s*$/m,
  );
  if (!preinstall) {
    problems.push("the preinstall hook no longer requests graceful --quit");
  } else {
    const body = preinstall[1];
    if (
      !body.includes("ExecWait '\"$INSTDIR\\${MAINBINARYNAME}.exe\" --quit'")
    ) {
      problems.push("the preinstall hook does not launch KwikPaste.exe --quit");
    }
    if (
      !body.includes('nsis_tauri_utils::FindProcess "${MAINBINARYNAME}.exe"') ||
      !body.includes("Sleep 100") ||
      !body.includes("StrCpy $1 30")
    ) {
      problems.push(
        "the preinstall hook no longer waits up to three seconds for exit",
      );
    }
    const hookOffset = script.indexOf("!macro NSIS_HOOK_PREINSTALL");
    const checkOffset = script.indexOf(
      '!insertmacro CheckIfAppIsRunning "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"',
    );
    if (hookOffset < 0 || checkOffset < 0 || hookOffset > checkOffset) {
      problems.push(
        "the graceful quit hook must run before CheckIfAppIsRunning",
      );
    }
  }
  if (
    !functionBody(script, ".onInstSuccess")?.includes(
      'nsis_tauri_utils::RunAsUser "$INSTDIR\\${MAINBINARYNAME}.exe" "$R0"',
    )
  ) {
    problems.push(".onInstSuccess no longer restarts the app with RunAsUser");
  }

  const original = utils.match(/!macro SetContext[\s\S]*?!macroend/);
  if (!original || squash(original[0]) !== squash(SET_CONTEXT)) {
    problems.push(
      "SetContext in utils.nsh changed; update packaging/windows/installer-hooks.nsh to match",
    );
  }

  return problems;
};

const main = () => {
  const args = process.argv.slice(2);
  const option = (name) => {
    const index = args.indexOf(name);
    return index >= 0 ? args[index + 1] : undefined;
  };
  const dir = args[0];
  const version = option("--version");
  const arch = option("--arch");
  const identity = option("--identity") ?? "production";
  if (!dir || !version || !arch) {
    say(
      "Usage: node scripts/ci/check-nsis-render.mjs <dir> --version <v> --arch <x64|arm64> [--identity production|test]",
    );
    process.exit(2);
  }

  const script = readFileSync(join(dir, "installer.nsi"), "utf8").replace(
    /^﻿/,
    "",
  );
  const utils = readFileSync(join(dir, "utils.nsh"), "utf8").replace(/^﻿/, "");
  const problems = checkRender(script, utils, { arch, identity, version });
  if (problems.length > 0) {
    for (const problem of problems) {
      say(`::error::${dir}: ${problem}`);
    }
    process.exit(1);
  }
  say(
    `ok  ${dir}: ${identity} identity, graceful quit hook before running-app check, OS gate in .onInit, template unchanged`,
  );
};

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main();
}
