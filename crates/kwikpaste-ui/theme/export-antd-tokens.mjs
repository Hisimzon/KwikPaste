// 导出 1.x 前端实际生效的 antd v6 design token：default 与 dark 两种算法，快贴没有自定义 seed，
// 所以这两套 token 就是常量。结果写到同目录的 antd_tokens_light_dark.json（冻结表），
// kwikpaste-ui 的单测拿它逐项比对 src/theme/antd.rs 里的 Rust 常量。
//
// 用法：仓库根目录装好前端依赖后运行 `node crates/kwikpaste-ui/theme/export-antd-tokens.mjs [输出路径]`。
// 只有 1.x 升级 antd、需要重新冻结时才跑；跑完用 git diff 检查变动的 token。
// 键按字母排序写出，与 biome 的 useSortedKeys 一致，提交钩子不会再改写这个文件。
import { writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(join(here, "..", "..", "..", "package.json"));
const theme = require("antd/lib/theme/index.js").default;
const output = process.argv[2] ?? join(here, "antd_tokens_light_dark.json");

/**
 * 按键名自然排序对象（只有一层：token 值都是字符串或数字），顺序与 biome 相同。
 */
const sortKeys = (tokens) => {
  return Object.fromEntries(
    Object.entries(tokens).sort(([left], [right]) => {
      return left.localeCompare(right, "en", { numeric: true });
    }),
  );
};

const frozen = {
  antdVersion: require("antd/package.json").version,
  dark: sortKeys(theme.getDesignToken({ algorithm: theme.darkAlgorithm })),
  light: sortKeys(theme.getDesignToken({ algorithm: theme.defaultAlgorithm })),
};

writeFileSync(output, `${JSON.stringify(frozen, null, 2)}\n`);
