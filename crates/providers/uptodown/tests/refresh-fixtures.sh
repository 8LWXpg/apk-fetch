#!/usr/bin/env bash
#
# Re-fetch the Uptodown fixtures that src/parse.rs tests run against.
# Fixtures live in tests/<app>/ — search.html, app.html, versions.json.
#
#   bash crates/providers/uptodown/tests/refresh-fixtures.sh
#   bash crates/providers/uptodown/tests/refresh-fixtures.sh <app> <package-id>
#
# Uptodown's download endpoint is Cloudflare-Turnstile-gated, so the provider only
# does search + versions; those need no captcha. Tests assert on structure.
# HTML fixtures are trimmed of <script>/<style>/<svg>; versions.json is raw.

set -euo pipefail

UA='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36'
TESTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

declare -A APPS=(
  [spotify]=com.spotify.music
)

TRIM='import sys,re
h=sys.stdin.read()
for t in ("script","style","svg","noscript"):
    h=re.sub(r"<"+t+r"\b[^>]*>.*?</"+t+r">","",h,flags=re.S|re.I)
sys.stdout.write(h)'

get()      { curl -fsSL -A "$UA" "$1"; }
save()     { local d="$1" f="$2"; mkdir -p "$TESTS/$d"; python -c "$TRIM" > "$TESTS/$d/$f"
             echo "  $d/$f  ($(wc -c < "$TESTS/$d/$f") bytes)"; }
save_raw() { local d="$1" f="$2"; mkdir -p "$TESTS/$d"; cat > "$TESTS/$d/$f"
             echo "  $d/$f  ($(wc -c < "$TESTS/$d/$f") bytes)"; }

refresh_app() {
  local app="$1" pkg="$2"
  echo "$app  ($pkg)"
  # search by the most specific name fragment of the package id
  local term
  term=$(echo "$pkg" | tr '.' '\n' | grep -vE '^(com|org|net|io|app|android)$' | awk '{ print length, $0 }' | sort -rn | head -1 | cut -d' ' -f2-)
  get "https://en.uptodown.com/android/search?query=$term" | save "$app" search.html

  # subdomain whose app page carries $pkg (checked against the "Package Name" row)
  local host="" page
  while read -r h; do
    [[ -z "$h" ]] && continue
    b=${h%/android}
    page=$(get "$b/android" || true)
    if [[ "$page" == *"<td>$pkg</td>"* ]]; then host=$b; break; fi
  done < <(grep -oE "https://[a-z0-9-]+\.en\.uptodown\.com/android" "$TESTS/$app/search.html" | sort -u)
  [[ -n "$host" ]] || { echo "  !! $pkg not found in search for '$term'" >&2; return 1; }
  local code
  get "$host/android" | save "$app" app.html
  code=$(grep -oE "data-code=[\"'][0-9]+[\"']" "$TESTS/$app/app.html" | grep -oE '[0-9]+' | head -1)
  get "$host/android/apps/$code/versions/1" | save_raw "$app" versions.json
}

if [[ $# -eq 2 ]]; then refresh_app "$1" "$2"
elif [[ $# -eq 0 ]]; then for a in "${!APPS[@]}"; do refresh_app "$a" "${APPS[$a]}"; done
else echo "usage: $0 [<app> <package-id>]" >&2; exit 2
fi

echo "done — now: cargo test -p apk-fetch-uptodown"
