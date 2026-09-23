# apk-fetch

Multi-source command line APK downloader with fallback. Resolves a package from 
a chain of third-party mirrors (APKCombo, APKPure, APKMirror), picks a APK build
for your ABI, and downloads it, retrying the next source if one fails.

## Quickstart

```sh
# Search an app by name
apk-fetch search youtube

# List published versions of a package
apk-fetch versions com.google.android.youtube

# Download the latest build for your ABI into ./downloads
apk-fetch get com.google.android.youtube --output downloads

# Pin a version, or ask a specific mirror first
apk-fetch get com.google.android.youtube --version 21.37.47 --provider apkpure
```

Requires `curl` on your `PATH` — all fetching goes through it.

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
apk-fetch get com.google.android.youtube --output ~/Downloads
```

- `--version <V>` — exact version to fetch.
- `--provider <name>` — try only this provider.
- `--priority <a,b>` — ordered list of providers to try until one resolves.
- `--arch <abi>` — `arm64-v8a` (default), `armeabi-v7a`, `x86`, `x86_64`, or `universal`; falls back to a universal build when the exact ABI is unavailable.
- `--output <dir>` — save directory (default: `.`).

### `providers`

```sh
apk-fetch providers list    # configured mirrors and their priority
apk-fetch providers check   # reachability of every mirror
```

When no provider is given, `get` tries in the built-in priority order:
APKCombo, APKPure, APKMirror.
