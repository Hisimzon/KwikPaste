#!/usr/bin/env bash
set -euo pipefail

# Dispatch-only CI test. A new minisign key is created for every run and every artifact is temporary.
[[ "$(uname -s)" == "Darwin" ]] || { echo 'macOS only' >&2; exit 2; }
root="$(cd "$(dirname "$0")/../.." && pwd)"
run="$(mktemp -d "${TMPDIR:-/tmp}/kwikpaste-native-update-e2e.XXXXXX")"
key_dir="$run/key"; old_out="$run/old"; new_out="$run/new"; server_dir="$run/server"
mkdir -p "$key_dir" "$old_out" "$new_out" "$server_dir"
manifest="$server_dir/latest.json"
port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
old_version=2.0.0-e2e.1
new_version=2.0.0-e2e.2
target=aarch64-apple-darwin
arch=aarch64
original_manifest="$(cat "$root/Cargo.toml")"
original_lock="$(cat "$root/Cargo.lock")"
server_pid=''

cleanup() {
  [[ -z "$server_pid" ]] || kill "$server_pid" 2>/dev/null || true
  printf '%s' "$original_manifest" > "$root/Cargo.toml"
  printf '%s' "$original_lock" > "$root/Cargo.lock"
  rm -rf "$run"
}
trap cleanup EXIT

password="e2e-$(uuidgen | tr -d '-')"
pnpm tauri signer generate --ci -p "$password" -w "$key_dir/e2e.key" -f >/dev/null

build() {
  local version="$1" out="$2"
  python3 - "$root/Cargo.toml" "$version" <<'PY'
import re
import sys

path, version = sys.argv[1:]
text = open(path, encoding='utf-8').read()
text = re.sub(r'(\[workspace\.package\].*?^version\s*=\s*")[^"]+(")',
              rf'\g<1>{version}\g<2>', text, count=1, flags=re.S | re.M)
open(path, 'w', encoding='utf-8', newline='').write(text)
PY
  cargo build --locked --release -p kwikpaste-app --features e2e-overrides --target "$target"
  TAURI_SIGNING_PRIVATE_KEY="$key_dir/e2e.key" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$password" \
    node scripts/release/package-native.mjs --target "$target" --out "$out" --identity test --pubkey "$key_dir/e2e.key.pub" --features e2e-overrides
}

build "$old_version" "$old_out"
build "$new_version" "$new_out"

new_archive="$new_out/KwikPasteBundleTest_${new_version}_${arch}.app.tar.gz"
cp "$new_archive" "$server_dir/"
sig="$(cat "$new_archive.sig")"
python3 - "$manifest" "$new_version" "$sig" "$(basename "$new_archive")" "$port" <<'PY'
import json
import sys

path, version, signature, name, port = sys.argv[1:]
url = 'http://127.0.0.1:' + port + '/' + name
platforms = {key: {'url': url, 'signature': signature}
             for key in ('darwin-aarch64-app', 'darwin-aarch64')}
with open(path, 'w', encoding='utf-8') as output:
    json.dump({'kwikpaste': {'schema': 1, 'requires': {'macos': '10.15'}},
               'notes': 'local e2e', 'pub_date': '2026-10-06T00:00:00Z',
               'version': version, 'platforms': platforms}, output, indent=2)
    output.write('\n')
PY
(cd "$server_dir" && python3 -m http.server "$port" --bind 127.0.0.1 >/dev/null 2>&1) & server_pid=$!
sleep 1

old_archive="$old_out/KwikPasteBundleTest_${old_version}_${arch}.app.tar.gz"
mkdir -p "$run/App"
tar -xzf "$old_archive" -C "$run/App"
app="$run/App/KwikPasteBundleTest.app"
exe="$app/Contents/MacOS/KwikPasteBundleTest"
[[ -x "$exe" ]] || { echo "missing $exe" >&2; exit 1; }
export KWIKPASTE_SELFTEST=1
export KWIKPASTE_UPDATE_ENDPOINT="http://127.0.0.1:$port/latest.json"
export KWIKPASTE_UPDATE_PUBLIC_KEY="$(cat "$key_dir/e2e.key.pub")"
old_hash="$(shasum -a 256 "$exe" | awk '{print $1}')"
"$exe" --selftest-update-e2e 2>&1 | tee "$run/update.log"
new_hash="$(shasum -a 256 "$exe" | awk '{print $1}')"
[[ "$old_hash" != "$new_hash" ]] || { echo 'macOS app was not replaced' >&2; exit 1; }
grep -Eq 'downloaded|installing' "$run/update.log"
echo "ok macOS app swap: $old_hash -> $new_hash"
