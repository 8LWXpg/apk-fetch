#!/usr/bin/env bash
#
# Re-fetch the HTML fixtures the provider parser tests run against.
#
#   bash scripts/refresh-fixtures.sh              # all providers
#   bash scripts/refresh-fixtures.sh apkmirror    # just one
#
# When a mirror site changes its markup, run this, then look at the diff and
# adjust the selectors in the matching `src/parse.rs` (and any version strings
# the tests still assert). Fixtures are trimmed of <script>/<style>/<svg> to keep
# the repo small; that never removes structure the parsers use.
#
# Each provider is a self-contained function below — add a new one the same way
# when you add a provider crate.

set -euo pipefail

UA='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36'
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

get()  { curl -fsSL -A "$UA" "$1"; }
TRIM_PY='import sys,re
h=sys.stdin.read()
for t in ("script","style","svg","noscript"):
    h=re.sub(r"<"+t+r"\b[^>]*>.*?</"+t+r">","",h,flags=re.S|re.I)
sys.stdout.write(h)'
save() {
  local path="$1"
  python -c "$TRIM_PY" > "$path"
  echo "  $(basename "$path")  ($(wc -c < "$path") bytes)"
}

refresh_apkmirror() {
  local base="https://www.apkmirror.com"
  local dir="$ROOT/crates/providers/apkmirror/tests/fixtures"
  echo "apkmirror -> $dir"

  get "$base/?post_type=app_release&searchtype=apk&s=org.mozilla.firefox" | save "$dir/search-firefox.html"
  get "$base/?post_type=app_release&searchtype=apk&s=com.example.does.not.exist.xyz" | save "$dir/search-no-results.html"
  get "$base/apk/mozilla/firefox/" | save "$dir/app-firefox.html"

  # latest release page, discovered from the app page
  local rel
  rel=$(grep -oE '/apk/mozilla/firefox/firefox[a-z0-9-]+-release/' "$dir/app-firefox.html" | head -1)
  get "$base$rel" | save "$dir/version-firefox.html"

  # variant -> download page -> "starting" page (each URL is scraped from the previous)
  local dlp btn
  dlp=$(grep -oE '/apk/mozilla/firefox/[^"]+-android-apk-download/' "$dir/version-firefox.html" | head -1)
  get "$base$dlp" | save "$dir/download-page-firefox.html"
  btn=$(grep -oE '/apk/mozilla/firefox/[^"]+/download/\?key=[a-f0-9]+' "$dir/download-page-firefox.html" | head -1)
  get "$base$btn" | save "$dir/download-starting-firefox.html"
}

refresh_apkpure() {
  local base="https://apkpure.com"
  local dir="$ROOT/crates/providers/apkpure/tests/fixtures"
  echo "apkpure -> $dir"

  get "$base/search?q=firefox"                         | save "$dir/search-firefox.html"
  get "$base/x/org.mozilla.firefox"                    | save "$dir/app-firefox.html"
  get "$base/x/org.mozilla.firefox/versions"           | save "$dir/versions-firefox.html"
}

targets=("${@:-apkmirror apkpure}")
for t in ${targets[@]}; do
  case "$t" in
    apkmirror) refresh_apkmirror ;;
    apkpure)   refresh_apkpure ;;
    *) echo "unknown provider: $t" >&2; exit 2 ;;
  esac
done

echo
echo "done — now: cargo test --workspace"
