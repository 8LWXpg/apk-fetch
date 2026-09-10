#!/usr/bin/env bash

set -euo pipefail

UA='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36'
BASE='https://www.apkmirror.com'
TESTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Apps refreshed by a no-argument run: <dir> -> <package-id>. `nonexistent` is a
# bogus id kept for the "no results" parser test. Add a line to make an app part
# of the default set.
declare -A APPS=(
  [youtube]=com.google.android.youtube
  [youtube-music]=com.google.android.apps.youtube.music
  [nonexistent]=com.example.does.not.exist.xyz
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
  get "$BASE/?post_type=app_release&searchtype=apk&s=$pkg" | save "$app" search.html

  # A bogus package only needs the (empty) search page.
  grep -q 'No results found matching your query' "$TESTS/$app/search.html" && return 0

  # Derive the app page from the first non-beta search result, then walk the
  # download chain (each URL is scraped from the previous page). The beta/alpha
  # filter mirrors ApkMirror::top_release_url.
  local rel org repo
  rel=$(grep -oE '/apk/[a-z0-9-]+/[a-z0-9-]+/[a-z0-9-]+-release/' "$TESTS/$app/search.html" \
        | grep -vE '/apk/[^/]+/[a-z0-9-]*-(beta|alpha|dev|canary)/' | head -1)
  org=$(cut -d/ -f3 <<<"$rel"); repo=$(cut -d/ -f4 <<<"$rel")
  get "$BASE/apk/$org/$repo/" | save "$app" app.html

  local relpage dlp btn
  relpage=$(grep -oE "/apk/$org/$repo/[a-z0-9-]+-release/" "$TESTS/$app/app.html" | head -1)
  get "$BASE$relpage" | save "$app" version.html
  dlp=$(grep -oE "/apk/$org/$repo/[^\"]+-android-apk-download/" "$TESTS/$app/version.html" | head -1)
  get "$BASE$dlp" | save "$app" download-page.html
  btn=$(grep -oE "/apk/$org/$repo/[^\"]+/download/\?key=[a-f0-9]+" "$TESTS/$app/download-page.html" | head -1)
  get "$BASE$btn" | save "$app" download-starting.html
}

if [[ $# -eq 2 ]]; then
  refresh_app "$1" "$2"
elif [[ $# -eq 0 ]]; then
  for app in "${!APPS[@]}"; do refresh_app "$app" "${APPS[$app]}"; done
else
  echo "usage: $0 [<app> <package-id>]" >&2; exit 2
fi

echo "done — now: cargo test -p apkmirror"
