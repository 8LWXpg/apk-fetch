# apk-fetch

A fast, multi-source command-line APK downloader with automatic fallback. It
resolves Android package names across multiple third-party mirrors (APKCombo,
APKPure, APKMirror), selects the correct build for your target ABI, and gracefully
retries down the chain if a source fails.

## Prerequisite

- **`curl` 7.12.3+**: Required on your `PATH` for all network requests.
  > **Note:** `curl` must be compiled with `zlib` support.
- **`curl-impersonate` (Optional / Recommended)**: Some mirrors detect and block stock `curl` via TLS fingerprinting. If you encounter a `Blocked` error that does not happen in a browser, place [curl-impersonate](https://github.com/lexiforest/curl-impersonate) ahead of stock `curl` on your `PATH`.

## Quickstart

```sh
# Search an app by name
apk-fetch search youtube

# List published versions of a package
apk-fetch versions com.google.android.youtube

# Download the latest build into ./downloads
apk-fetch get com.google.android.youtube --out-dir downloads

# Pin a version, or ask a specific mirror first
apk-fetch get com.google.android.youtube --version 21.37.47 --provider apkpure
```

## Install

### Download

Download binary from [releases](https://github.com/8LWXpg/apk-fetch/releases) page.

### Using `cargo-binstall`

```sh
cargo binstall --git https://github.com/8LWXpg/apk-fetch apk-fetch
```

### Build from Source

```sh
cargo install --git https://github.com/8LWXpg/apk-fetch
```

## Usage

```
apk-fetch <COMMAND>

Commands:
  search     Search for an app by name
  versions   List published versions of a package
  get        Resolve and download an APK
  providers  Provider management

Options:
  -h, --help  Print help
  -V, --version  Print version
```

### `search <query>`

```sh
apk-fetch search youtube --all        # every provider
apk-fetch search youtube --provider apkpure,apkcombo
apk-fetch search youtube --json       # machine-readable output
```

### `versions <package-id>`

```sh
apk-fetch versions com.google.android.youtube --all
```

### `get <package-id>`

```sh
apk-fetch get com.google.android.youtube # latest, default ABI
apk-fetch get com.google.android.youtube --version 21.37.47
apk-fetch get com.google.android.youtube --arch universal
apk-fetch get com.google.android.youtube --provider apkmirror
apk-fetch get com.google.android.youtube --priority apkcombo,apkmirror
apk-fetch get com.google.android.youtube --out-dir ~/Downloads
```

- `--version <V>` — exact version to fetch.
- `--provider <name>` — try only this provider.
- `--priority <a,b>` — ordered list of providers to try until one resolves.
- `--arch <abi>` — `arm64-v8a` (default), `armeabi-v7a`, `x86`, `x86_64`, or `universal`; falls back to a universal build when the exact ABI is unavailable.
- `--out-dir <dir>` — save directory (default: `.`).

### `providers`

```sh
apk-fetch providers list    # configured mirrors and their priority
apk-fetch providers check   # reachability of every mirror
```

When no provider is given, `get` tries in the built-in priority order:
APKCombo, APKPure, APKMirror.
