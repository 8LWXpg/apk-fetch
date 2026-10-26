#!/usr/bin/env bash
# scripts/probe.sh page <url>            -> blocked? status, redirects, CF headers
# scripts/probe.sh dl <url> [referer]    -> does it serve a ZIP/APK? (reads 4 bytes)
# Set APK_FETCH_CURL to curl-impersonate to compare against stock curl.
set -u
CURL=${APK_FETCH_CURL:-curl}
UA='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36'
SIGS='Just a moment\.\.\.|cf-browser-verification|cf_chl_opt|Attention Required! \| Cloudflare|Checking if the site connection is secure|_cf_chl_|Enable JavaScript and cookies to continue'
mode=${1:?page|dl} url=${2:?url} ref=${3:-}
hdr=$(mktemp) body=$(mktemp); trap 'rm -f "$hdr" "$body"' EXIT
args=(-sSL --max-redirs 10 --compressed --max-time 30 -A "$UA" -D "$hdr")
[ -n "$ref" ] && args+=(-H "Referer: $ref")

# helper: one line per hop
chain() { grep -iE '^(HTTP/|location:)' "$hdr" | tr -d '\r' | sed 's/^/  /'; }

case $mode in
page)
  "$CURL" "${args[@]}" -o "$body" -w 'status=%{http_code} final=%{url_effective} redirects=%{num_redirects} size=%{size_download}\n' "$url"
  chain
  grep -iE '^(server|cf-ray|cf-mitigated|set-cookie: (__cf|cf_))' "$hdr" | sort -u
  if grep -qE "$SIGS" "$body"; then echo "BLOCKED: challenge signature in body"; else echo "no challenge signature"; fi
  ;;
dl)
  magic=$("$CURL" "${args[@]}" -o - "$url" 2>/dev/null | head -c4 | xxd -p)
  chain
  grep -iE '^(content-type|content-disposition|content-length)' "$hdr" | tail -n 3
  [ "$magic" = 504b0304 ] && echo "OK: ZIP magic (PK\\x03\\x04)" || echo "NOT an APK: first bytes=$magic"
  ;;
esac
