# apk-fetch architecture

`apk-fetch` downloads APKs from third-party mirror sites. Each site is isolated
behind one trait so a single site breaking, changing its markup, or blocking
requests fails over to the next instead of taking the whole tool down.

## Crate layout

| Crate | Responsibility |
|---|---|
| `apk-fetch-core` | The `Provider` trait, domain types, `ProviderError`, `ProviderRegistry` fallback resolver, and the shared output macros. No I/O. |
| `apk-fetch-fetch` | HTTP via the system `curl`: per-provider throttle, transient-error retry, blocked-response detection. |
| `apk-fetch-apkmirror` | APKMirror provider — search-walk to the download. |
| `apk-fetch-apkpure` | APKPure provider — package-id-addressable, no HTML walk needed for downloads. |
| `apk-fetch` (cli) | clap binary. Thin dispatch: parse args → build registry → per-subcommand handler. |

Dependency direction is strictly `cli → providers → fetch → core`. `core` depends
on nothing in the workspace.

## The `Provider` trait boundary

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn search(&self, query: &str) -> Result<Vec<AppResult>, ProviderError>;
    async fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>, ProviderError>;
    async fn download_url(&self, pkg: &str, version: Option<&str>, arch: &str) -> Result<DownloadTarget, ProviderError>;
    async fn check(&self) -> Result<(), ProviderError>; // default: a canned search
}
```

`arch` is an ABI *preference* (`--arch`, default `arm64-v8a`), not a demand: a
provider that has no exact match falls back to a universal build, and one that
serves a single build per app (APKPure) ignores it. The resolved variant's arch
comes back in `DownloadTarget.arch` and in the filename,
`{pkg}-{version}-{arch}.{apk|xapk}` (`core::download_filename`; `arch` is dropped
from the name when unknown, `xapk` when the variant is a bundle).

The **package id** (`org.mozilla.firefox`) is the only cross-provider identifier.
Each provider is responsible, end to end, for mapping that id to a download on its
own site — there is deliberately **no shared "package id → site slug" registry**.
Mirror sites don't share an addressing scheme (APKMirror uses `{org}/{repo}` URL
slugs and has no package-id index; APKPure puts the package id straight in the
URL), and a shared table would be the one file every contributor edits and every
provider breakage routes through — the opposite of the isolation this design buys.

Adding a site = one new crate implementing `Provider`, registered in
`cli/src/main.rs::build_registry`. Zero edits to shared code.

`search` is the escape hatch: it returns `AppResult { package, .. }`, so when a
site's own search-by-id is imperfect a user can `apk-fetch search <name>` to
discover the identifier, then `get` it.

### APKMirror resolution flow

APKMirror has no package-id lookup, so every entrypoint starts from the site's own
search (query = the package id), takes the top hit, then walks:

```
search results  ─▶ app page (version list)  ─▶ variants table
                                                    │
        download page ◀── choose_variant(arch): exact-arch APK,
             │                else universal APK, else any APK, else bundle
     "download starting" page ─▶ APK URL (download.php, needs Referer)
```

### APKPure resolution flow

APKPure is package-id-addressable: `/x/{pkg}` resolves to the app page and
`d.apkpure.com/b/APK/{pkg}?version={v}` 302s straight to the APK. `download_url`
only touches the app page to read the latest version string (and to 404 →
`NotFound`); with `--version` pinned it skips even that.

### Fixtures

Pure parsers live in each provider's `src/parse.rs`, unit-tested against saved
HTML in `tests/<app>/*.html` (one dir per app) — no network in tests. Each
provider ships a `tests/refresh-fixtures.sh`: no args re-fetches every app dir,
`refresh-fixtures.sh <app> <package-id>` adds or refreshes one. Run it after a
site changes, check the diff, adjust selectors. Tests assert on structure, not
version numbers, so a refresh rarely breaks them. CSS selectors are `const &str`
in `parse.rs`; they move to config only if a real breakage proves it.

## Why "blocked" is a typed error

`ProviderError::Blocked { retry_after }` is **distinct from `ParseError`**. A
Cloudflare challenge page, a `403`, a `429`, or a `cf-mitigated` response header is
not our markup assumptions breaking — it means "this site won't serve *this
client* right now; ask someone else."

Consequences of the distinction:

- **The resolver fails over on it.** `ProviderRegistry::resolve_with_fallback`
  tries providers in the caller-supplied order and moves to the next on *any*
  `ProviderError`, but `Blocked` is the case the whole fallback mechanism exists
  for. The aggregated `ResolveError` can report `all_blocked()` vs
  `all_not_found()` vs `any_network()` so the CLI picks a matching exit code.
- **`fetch` never retries it.** Retry-with-backoff applies only to transient
  network/5xx errors. Retrying a challenge just burns your rate budget and makes
  the block worse — it surfaces immediately instead.
- **It's not a bug report.** A `ParseError` means "go fix the selectors". A
  `Blocked` means the site is working fine and doesn't like us — a different fix
  (better client fingerprint, slower cadence, or just relying on another
  provider).

### Why HTTP goes through `curl`, not a Rust client

APKMirror's Cloudflare config issues a managed challenge (`cf-mitigated:
challenge`) to `reqwest` on `/apk/*` paths based on the TLS ClientHello
fingerprint — with **both** `rustls` and `native-tls` backends, regardless of
headers, HTTP version, or cookies. A stock browser UA on `curl` (schannel /
system OpenSSL) passes; the Rust client does not.

Rather than pull in a BoringSSL-based fingerprint-impersonating client, `fetch`
shells out to the system `curl` (present on Windows 10+, macOS, and effectively
every Linux). `HttpFetcher` still owns throttle, retry, and block detection —
`curl` is just the transport. If `curl` is missing, every fetch fails with a
`Network` error naming it.

We send **only** the User-Agent. Adding a lone `Accept-Language` on top of curl's
fingerprint is enough to get APKPure's WAF to 403 — real browsers send a dozen
correlated headers or none of this matters, and a half-set reads as a bot. Genuine
blocks (a challenge `curl` also can't pass, a 403/429, a body signature) still map
to `ProviderError::Blocked` and drive failover — see
`fetch::{looks_blocked, map_http_status}`.

`download_to_file` also rejects a non-ZIP payload: some mirror download endpoints
answer `200` with an HTML landing page, and an APK/XAPK must start with `PK`.

## Exit codes (CLI)

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | unexpected error (parse failure, I/O) |
| 2 | invalid arguments (clap) |
| 3 | not found on any provider |
| 4 | all providers blocked / rate-limited |
| 5 | network failure |
