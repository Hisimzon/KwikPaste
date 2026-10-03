// Checks Windows installers against the size budget in lib/size.mjs (warning above 5.5 MiB, error above
// 6 MiB) and prints every size in MiB; --exe also prints the size of the packaged exe. Native CI runs it on
// every build, so the installer cannot grow past the budget unnoticed between releases.
//
// Usage: node scripts/ci/check-installer-size.mjs <setup.exe>... [--exe <KwikPaste.exe>]...
import { appendFileSync, statSync } from "node:fs";
import { basename } from "node:path";
import { pathToFileURL } from "node:url";

import { checkSetupSize, formatMiB } from "./lib/size.mjs";

const say = (line) => {
  process.stdout.write(`${line}\n`);
};

const parseArgs = (argv) => {
  const options = { exes: [], setups: [] };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--exe") {
      options.exes.push(argv[++index]);
    } else if (arg.startsWith("--")) {
      throw new Error(`unknown argument ${arg}`);
    } else {
      options.setups.push(arg);
    }
  }
  if (options.setups.length === 0) {
    throw new Error(
      "Usage: check-installer-size.mjs <setup.exe>... [--exe <KwikPaste.exe>]...",
    );
  }

  return options;
};

const main = () => {
  const options = parseArgs(process.argv.slice(2));
  const summary = ["| File | Size |", "| --- | ---: |"];
  let failed = false;

  for (const exe of options.exes) {
    const size = statSync(exe).size;
    say(`ok  ${basename(exe)} is ${formatMiB(size)} (${size} bytes)`);
    summary.push(`| ${basename(exe)} | ${formatMiB(size)} |`);
  }
  for (const setup of options.setups) {
    const size = statSync(setup).size;
    const { note, problem, warning } = checkSetupSize(basename(setup), size);
    if (problem) {
      say(`::error::${problem}`);
      failed = true;
    } else if (warning) {
      say(`::warning::${warning}`);
    } else {
      say(`ok  ${note}`);
    }
    summary.push(`| ${basename(setup)} | ${formatMiB(size)} |`);
  }

  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${summary.join("\n")}\n`);
  }
  if (failed) {
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
