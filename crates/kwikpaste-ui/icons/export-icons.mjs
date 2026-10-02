// 从 1.x 前端用的 iconify 图标包导出原生版需要、而 gpui-kit-assets 默认图标集里没有的图标，
// 写成同目录的 `<集合>-<名字>.svg`，由 src/assets.rs 用 include_bytes! 内嵌。
//
// 用法：仓库根目录装好前端依赖后运行 `node crates/kwikpaste-ui/icons/export-icons.mjs`。
// 新增图标时把名字加进 ICONS，再在 src/icon.rs 的 IconName 里加一项；跑完用 git diff 检查。
import { writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(join(here, "..", "..", "..", "package.json"));

/** `集合:名字`，与 1.x UnoCSS 的 `i-集合:名字` 类名一致。 */
const ICONS = [
  "lucide:image-off",
  "lucide:key-round",
  "lucide:laptop",
  "lucide:monitor",
  "lucide:notebook-pen",
  "ph:push-pin-bold",
];

for (const icon of ICONS) {
  const [prefix, name] = icon.split(":");
  const set = require(`@iconify-json/${prefix}/icons.json`);
  const data = set.icons[name];

  if (!data) {
    throw new Error(`${icon} is missing from @iconify-json/${prefix}`);
  }

  const width = data.width ?? set.width ?? 16;
  const height = data.height ?? set.height ?? 16;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}">${data.body}</svg>\n`;

  writeFileSync(join(here, `${prefix}-${name}.svg`), svg);
  process.stdout.write(`${prefix}-${name}.svg\n`);
}
