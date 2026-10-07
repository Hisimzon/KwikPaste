#!/usr/bin/env bash
set -euo pipefail

# Dispatch-only CI test. A new minisign key is created for every run and every artifact is temporary.
[[ "$(uname -s)" == "Darwin" ]] || { echo 'macOS only' >&2; exit 2; }
root="$(cd "$(dirname "$0")/../.." && pwd)"
run="$(mktemp -d "${TMPDIR:-/tmp}/kwikpaste-native-update-e2e.XXXXXX")"
key_dir="$run/key"; old_out="$run/old"; new_out="$run/new"; server_dir="$run/server"
mkdir -p "$key_dir" "$old_out" "$new_out" "$server_dir"
manifest="$server_dir/latest.json"
port_file="$run/http.port"
http_log="$run/http.log"
old_version=2.0.0-e2e.1
new_version=2.0.0-e2e.2
target=aarch64-apple-darwin
arch=aarch64
original_manifest="$(cat "$root/Cargo.toml")"
original_lock="$(cat "$root/Cargo.lock")"
server_pid=''
port=''

cleanup() {
  [[ -z "$server_pid" ]] || kill "$server_pid" 2>/dev/null || true
  printf '%s' "$original_manifest" > "$root/Cargo.toml"
  printf '%s' "$original_lock" > "$root/Cargo.lock"
  rm -rf "$run"
}
trap cleanup EXIT

# Everything that decides whether a loopback request reaches the local server.
diagnose() {
  echo '--- loopback diagnostics'
  sw_vers || true
  env | grep -i '_proxy=' || echo 'no proxy environment variables'
  scutil --proxy || true
  for flag in --getglobalstate --getblockall --getstealthmode --getallowsigned; do
    /usr/libexec/ApplicationFirewall/socketfilterfw "$flag" || true
  done
  python3 -c 'import sys; print("python", sys.executable, sys.version.split()[0])' || true
  [[ -z "$port" ]] || lsof -nP -iTCP:"$port" -sTCP:LISTEN || true
  echo '--- local server log'
  cat "$http_log" 2>/dev/null || true
}

fail() {
  diagnose
  echo "$1" >&2
  exit 1
}

# The server binds port 0 itself, so nothing can take the port between choosing and listening.
start_server() {
  cat > "$run/serve.py" <<'PY'
import functools
import http.server
import os
import sys

directory, port_file = sys.argv[1:]
handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=directory)
with http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler) as server:
    with open(port_file + '.tmp', 'w', encoding='utf-8') as output:
        output.write(str(server.server_address[1]))
    os.replace(port_file + '.tmp', port_file)
    server.serve_forever()
PY
  python3 -u "$run/serve.py" "$server_dir" "$port_file" > "$http_log" 2>&1 &
  server_pid=$!
  for _ in $(seq 50); do
    [[ -s "$port_file" ]] && break
    sleep 0.2
  done
  port="$(cat "$port_file" 2>/dev/null || true)"
  [[ -n "$port" ]] || fail 'local HTTP server did not start'
}

probe() {
  curl -v --noproxy '*' --fail --max-time 5 "http://127.0.0.1:$port/$1" || fail "loopback probe of /$1 failed"
}

# Serve and probe before the 15-minute builds, so a broken loopback fails in seconds.
start_server
echo ready > "$server_dir/ready"
probe ready

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
  cargo build --release -p kwikpaste-app --features e2e-overrides --target "$target"
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
probe latest.json

old_archive="$old_out/KwikPasteBundleTest_${old_version}_${arch}.app.tar.gz"
mkdir -p "$run/App"
tar -xzf "$old_archive" -C "$run/App"
app="$run/App/KwikPasteBundleTest.app"
exe="$app/Contents/MacOS/KwikPasteBundleTest"
[[ -x "$exe" ]] || { echo "missing $exe" >&2; exit 1; }
export KWIKPASTE_SELFTEST=1
export KWIKPASTE_UPDATE_ENDPOINT="http://127.0.0.1:$port/latest.json"
# Both channels read the local manifest: a published beta must not decide this test.
export KWIKPASTE_UPDATE_BETA_ENDPOINT="$KWIKPASTE_UPDATE_ENDPOINT"
export KWIKPASTE_UPDATE_PUBLIC_KEY="$(cat "$key_dir/e2e.key.pub")"
old_hash="$(shasum -a 256 "$exe" | awk '{print $1}')"
app_status=0
"$exe" --selftest-update-e2e 2>&1 | tee "$run/update.log" || app_status=$?
new_hash="$(shasum -a 256 "$exe" | awk '{print $1}')"
[[ "$old_hash" != "$new_hash" ]] || fail "macOS app was not replaced (exit $app_status)"
grep -Eq 'downloaded|installing' "$run/update.log" || fail 'update log has no download or install step'
echo "ok macOS app swap: $old_hash -> $new_hash"
