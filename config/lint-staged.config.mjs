/** @type {import("lint-staged").Configuration} */
const config = {
  "**/*.{ts,tsx,js,jsx,json,jsonc,css}": [
    "biome check --write --no-errors-on-unmatched",
  ],
  "src-tauri/**/*.{rs,toml,lock}": () => {
    return [
      "cargo fmt --manifest-path src-tauri/Cargo.toml --all",
      "cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings",
    ];
  },
  // 原生 workspace：根目录的 cargo fmt 只格式化成员，不碰 third_party。
  "{crates/**/*.{rs,toml},Cargo.toml,Cargo.lock}": () => {
    return [
      "cargo fmt",
      "cargo clippy --locked --workspace --all-targets -- -D warnings",
    ];
  },
};

export default config;
