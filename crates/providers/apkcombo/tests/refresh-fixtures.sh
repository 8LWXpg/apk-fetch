#!/usr/bin/env bash

set -euo pipefail

UA='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36'
BASE='https://apkcombo.com'
TESTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

declare -A APPS=(
  [youtube]=com.google.android.youtube
  [youtube-music]=com.google.android.apps.youtube.music
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

  # Same trick the provider uses: /en/{pkg}/ 301s to the canonical /{slug}/{pkg}/,
  # so a HEAD names the slug without downloading a page.
  local slug
  slug=$(curl -fsSIL -A "$UA" -o /dev/null -w '%{url_effective}' "$BASE/en/$pkg/" \
         | sed -nE "s#^$BASE/([^/]+)/$pkg/?\$#\1#p")
  [[ -n "$slug" ]] || { echo "  !! apkcombo has no page for $pkg" >&2; return 1; }

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
