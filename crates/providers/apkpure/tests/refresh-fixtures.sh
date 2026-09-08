#!/usr/bin/env bash
#
# Re-fetch the APKPure HTML fixtures that src/parse.rs tests run against.
# Fixtures live in tests/<app>/*.html — one directory per app.
#
#   bash crates/providers/apkpure/tests/refresh-fixtures.sh
#       refresh every app directory that already exists
#
#   bash crates/providers/apkpure/tests/refresh-fixtures.sh <app> <package-id>
#       add (or refresh just) one app, e.g.  focus  org.mozilla.focus
#
# Run this when APKPure changes its markup: check the diff, then adjust the
# selectors in src/parse.rs. Tests assert on structure, not version numbers, so a
# refresh rarely breaks them. Fixtures are trimmed of <script>/<style>/<svg>.
# APKMirror has its own script at ../../apkmirror/tests/.

set -euo pipefail

UA='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36'
BASE='https://apkpure.com'
TESTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Apps refreshed by a no-argument run: <dir> -> <package-id>. Add a line to make
# an app part of the default set.
declare -A APPS=(
  [newpipe]=org.schabi.newpipe
)

TRIM='import sys,re
h=sys.stdin.read()
for t in ("script","style","svg","noscript"):
    h=re.sub(r"<"+t+r"\b[^>]*>.*?</"+t+r">","",h,flags=re.S|re.I)
sys.stdout.write(h)'

get()  { curl -fsSL -A "$UA" "$1"; }
save() { local d="$1" f="$2"; mkdir -p "$TESTS/$d"; python -c "$TRIM" > "$TESTS/$d/$f"
         echo "  $d/$f  ($(wc -c < "$TESTS/$d/$f") bytes)"; }

refresh_app() {
  local app="$1" pkg="$2"
  echo "$app  ($pkg)"
  get "$BASE/search?q=$pkg"          | save "$app" search.html
  get "$BASE/x/$pkg"                 | save "$app" app.html
  get "$BASE/x/$pkg/versions"        | save "$app" versions.html
}

if [[ $# -eq 2 ]]; then
  refresh_app "$1" "$2"
elif [[ $# -eq 0 ]]; then
  for app in "${!APPS[@]}"; do refresh_app "$app" "${APPS[$app]}"; done
else
  echo "usage: $0 [<app> <package-id>]" >&2; exit 2
fi

echo "done — now: cargo test -p apk-fetch-apkpure"
