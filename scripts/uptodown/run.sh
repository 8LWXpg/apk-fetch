#!/usr/bin/env bash
KEY='MDGMXUMdvHJBG/vjdFgmqX6LUdy7ecfwvYNd0gyfOCs='
B=https://www.uptodown.app/eapi
PKG=com.google.android.youtube
CHROME='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36'
DALVIK='Dalvik/2.1.0 (Linux; U; Android 16; Pixel 8 Pro Build/BP4A.260205.001)'
H=(-H 'Identificador: Uptodown_Android' -H 'Identificador-Version: 745')

# --- auth (v739 HMAC -> JWT)
TS=$(date +%s); ID=$(openssl rand -hex 8)
SIG=$(printf %s "$TS" | openssl dgst -sha256 -hmac "$KEY" -hex | awk '{print $NF}')
TOK=$(curl -sS -A "$CHROME" "${H[@]}" -d "identifier=$ID" -d id_plataforma=13 -d lang=en \
  -d "unixtime=$TS" -d "hmac=$SIG" "$B/auth/token?identifier=$ID" | jq -r .token)
echo "token: ${TOK:0:20}..."

A=(-g -A "$CHROME" "${H[@]}" -H "Authorization: Bearer $TOK" -H 'Accept: application/json')
get() { curl -sS "${A[@]}" "$B$1"; }

APPID=$(get "/apps/byPackagename/$PKG" | jq -r .data.appID)
echo "appID: $APPID"

echo; echo "=== 1. search (first hit + keys)"
get "/v2/apps/search/youtube?page[limit]=5&page[offset]=0&lang=en" \
  | jq '{keys: (.data.results[0]|keys), first: .data.results[0], n: (.data.results|length)}'
