#!/usr/bin/env bash
#
# Re-fetch the APKMirror HTML fixtures that src/parse.rs tests run against.
#
#   bash crates/providers/apkmirror/tests/refresh-fixtures.sh
#
# Run this when APKMirror changes its markup: check the diff, then adjust the
# selectors in src/parse.rs. The tests assert on structure, not specific version
# numbers, so a refresh rarely breaks them. Fixtures are trimmed of
# <script>/<style>/<svg> to keep the repo small; that removes no structure the
# parsers use. APKPure has its own script at ../../apkpure/tests/.

set -euo pipefail

UA='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36'
BASE='https://www.apkmirror.com'
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/fixtures" && pwd)"

TRIM='import sys,re
h=sys.stdin.read()
for t in ("script","style","svg","noscript"):
    h=re.sub(r"<"+t+r"\b[^>]*>.*?</"+t+r">","",h,flags=re.S|re.I)
sys.stdout.write(h)'

get()  { curl -fsSL -A "$UA" "$1"; }
save() { python -c "$TRIM" > "$DIR/$1"; echo "  $1  ($(wc -c < "$DIR/$1") bytes)"; }

get "$BASE/?post_type=app_release&searchtype=apk&s=org.mozilla.firefox"          | save search-firefox.html
get "$BASE/?post_type=app_release&searchtype=apk&s=com.example.does.not.exist.xyz" | save search-no-results.html
get "$BASE/apk/mozilla/firefox/"                                                 | save app-firefox.html

# latest release page, discovered from the app page
REL=$(grep -oE '/apk/mozilla/firefox/firefox[a-z0-9-]+-release/' "$DIR/app-firefox.html" | head -1)
get "$BASE$REL" | save version-firefox.html

# variant -> download page -> "starting" page (each URL scraped from the previous)
DLP=$(grep -oE '/apk/mozilla/firefox/[^"]+-android-apk-download/' "$DIR/version-firefox.html" | head -1)
get "$BASE$DLP" | save download-page-firefox.html
BTN=$(grep -oE '/apk/mozilla/firefox/[^"]+/download/\?key=[a-f0-9]+' "$DIR/download-page-firefox.html" | head -1)
get "$BASE$BTN" | save download-starting-firefox.html

echo "done — now: cargo test -p apk-fetch-apkmirror"
