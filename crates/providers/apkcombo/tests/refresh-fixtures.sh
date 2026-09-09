#!/usr/bin/env bash
#
# Re-fetch the APKCombo HTML fixtures that src/parse.rs tests run against.
# Fixtures live in tests/<app>/*.html — one directory per app.
#
#   bash crates/providers/apkcombo/tests/refresh-fixtures.sh
#   bash crates/providers/apkcombo/tests/refresh-fixtures.sh <app> <package-id>
#
# APKCombo has no Cloudflare/captcha on the download path. The variants fixture is
# the POST /{slug}/{pkg}/{xid}/dl fragment. Tests assert on structure, not version
# numbers. Fixtures are trimmed of <script>/<style>/<svg>.

set -euo pipefail

UA='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36'
BASE='https://apkcombo.com'
TESTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

declare -A APPS=(
  [spotify]=com.spotify.music
)

TRIM='import sys,re
h=sys.stdin.read()
for t in ("script","style","svg","noscript"):
    h=re.sub(r"<"+t+r"\b[^>]*>.*?</"+t+r">","",h,flags=re.S|re.I)
sys.stdout.write(h)'

get()  { curl -fsSL -A "$UA" "$@"; }
save() { local d="$1" f="$2"; mkdir -p "$TESTS/$d"; python -c "$TRIM" > "$TESTS/$d/$f"
         echo "  $d/$f  ($(wc -c < "$TESTS/$d/$f") bytes)"; }

refresh_app() {
  local app="$1" pkg="$2"
  echo "$app  ($pkg)"
  # APKCombo search is name-based; the dir name is the app name.
  get "$BASE/search?q=$app" | save "$app" search.html
  # The bare app page self-links with the canonical {slug} segment.
  get "$BASE/$pkg/" | save "$app" app.html

  local slug
  slug=$(grep -oE "href=\"/[a-z0-9-]+/$pkg/\"" "$TESTS/$app/app.html" | head -1 | cut -d/ -f2)
  [[ -n "$slug" ]] || { echo "  !! could not resolve slug for $pkg" >&2; return 1; }

  get "$BASE/$slug/$pkg/old-versions"                     | save "$app" old-versions.html
  get "$BASE/$slug/$pkg/download/phone-latest-apk"        | save "$app" download-page.html

  # xid lives in a <script> (stripped from the fixture); re-fetch the raw page.
  local xid
  xid=$(get "$BASE/$slug/$pkg/download/phone-latest-apk" \
        | grep -oE 'var xid = "[a-z0-9]+"' | grep -oE '[a-z0-9]{6,}' | tail -1 || true)
  xid=${xid:-01a1200x20240308}
  get -F "package_name=$pkg" -F "version=" -e "$BASE/$slug/$pkg/download/phone-latest-apk" \
      "$BASE/$slug/$pkg/$xid/dl" | save "$app" variants.html
}

if [[ $# -eq 2 ]]; then refresh_app "$1" "$2"
elif [[ $# -eq 0 ]]; then for a in "${!APPS[@]}"; do refresh_app "$a" "${APPS[$a]}"; done
else echo "usage: $0 [<app> <package-id>]" >&2; exit 2
fi

echo "done — now: cargo test -p apkcombo"
