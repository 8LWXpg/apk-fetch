# apk-fetch architecture

`apk-fetch` downloads APKs from third-party mirror sites. Each site is isolated
behind one trait so a single site breaking, changing its markup, or blocking
requests fails over to the next instead of taking the whole tool down.

## Crate layout

Leaf crate packages have bare names; outside code reaches them through the
`apk_fetch::` umbrella.

| Namespace (dir under `crates/`) | Responsibility |
|---|---|
| `apk_fetch` (`cli`, package `apk-fetch`, lib + bin) | Umbrella lib re-exporting the below, plus the clap binary. Thin dispatch: parse args → build registry → per-subcommand handler. |
| `apk_fetch::contract` (`contract`) | The `Provider` trait, domain types, `ProviderError`, `ProviderRegistry` fallback resolver, and the shared output macros. No I/O. (Not called `core` — a crate by that name shadows the `core` sysroot crate and breaks derive macros.) |
| `apk_fetch::fetch` (`fetch`) | HTTP via the system `curl`: per-provider throttle, transient-error retry, blocked-response detection, `GET` + multipart `POST`. |
| `apk_fetch::providers::apkmirror` (`providers/apkmirror`) | APKMirror — search-walk to the download; Cloudflare-challenge-prone (see below). |
| `apk_fetch::providers::apkpure` (`providers/apkpure`) | APKPure — package-id-addressable, `d.apkpure.com/b/APK/{pkg}` 302s straight to the APK. |
| `apk_fetch::providers::apkcombo` (`providers/apkcombo`) | APKCombo — no Cloudflare/captcha; slug via redirect → download page (`xid`) → `POST /dl` variant fragment → `POST /checkin` token → signed R2 URL. |
| `fixtures` (`providers/fixtures`, dev-only) | The `tests/<app>/` walk and file read its three sibling providers' parser tests share. A `[dev-dependencies]` crate; nothing here ships. (Not named `test` — that shadows the sysroot `test` the `#[test]` macro expands into.) |

Default priority: `apkcombo,apkpure,apkmirror`.

Dependency direction is strictly `apk-fetch → providers → fetch → contract`;
`contract` depends on nothing in the workspace. The umbrella lib
(`crates/cli/src/lib.rs`) lives beside `main.rs` in the `apk-fetch` package, so
the binary reaches the leaves as `apk_fetch::…` with no extra crate.

## The `Provider` trait boundary

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> ProviderId;
    async fn search(&self, query: &str) -> Result<Vec<AppResult>, ProviderError>;
    async fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>, ProviderError>;
    async fn download_url(&self, pkg: &str, version: Option<&str>, arch: &str) -> Result<DownloadTarget, ProviderError>;
    async fn check(&self) -> Result<(), ProviderError>; // default: a canned search
}
```

`ProviderId` is one enum (`Apkmirror | Apkpure | Apkcombo`) used end to
end: the CLI parses `--provider` into it (clap `ValueEnum`), the registry is keyed
by it, `AppResult` / `VersionInfo` / `DownloadTarget` carry it, and its
`DEFAULT_PRIORITY` slice is the single source for `get` fallback order and the
default single provider. Providers
return a bare `ProviderError`; every call goes through `ProviderRegistry`
(`search` / `versions` / `download_url` / `check`), which tags the failure with
the id into a **`ProviderFailure`** (`Display` = `"{provider}: {source}"`). A bare
`ProviderError` never reaches the CLI, so error messages always name the provider
and the `NotFound` string never repeats it (`no app page for …`, not
`apkpure: apkpure has no …`).

`arch` is an ABI *preference* (`--arch`, default `arm64-v8a`), not a demand: a
provider that has no exact match falls back to a universal build, and one that
serves a single build per app (APKPure) ignores it. The resolved variant's arch
comes back in `DownloadTarget.arch` and in the filename,
`{pkg}-{version}-{arch}.{apk|xapk}` (`contract::download_filename`; `arch` is dropped
from the name when unknown, `xapk` when the variant is a bundle).

The **package id** (`org.mozilla.firefox`) is the only cross-provider identifier.
Each provider is responsible, end to end, for mapping that id to a download on its
own site — there is deliberately **no shared "package id → site slug" registry**.
Mirror sites don't share an addressing scheme (APKMirror uses `{org}/{repo}` URL
slugs and has no package-id index; APKPure puts the package id straight in the
URL), and a shared table would be the one file every contributor edits and every
provider breakage routes through — the opposite of the isolation this design buys.

Adding a site = one new crate implementing `Provider`, a `ProviderId` arm, and a
line in `cli/src/main.rs::build_registry`.

`search` is the escape hatch: it returns `AppResult { package, .. }`, so when a
site's own search-by-id is imperfect a user can `apk-fetch search <name>` to
discover the identifier, then `get` it.

### APKMirror resolution flow

APKMirror's own search ranks on a blind substring match (`s=line` floats
`Lineage2M` and `Korean Air` above `LINE`) and returns one row per *release*, so
`search` re-ranks the hits by query relevance (`parse::relevance`: whole-word hit
beats prefix beats substring; stable, ties keep site order) and drops repeat
apps. The CLI renders results as aligned `{title} {version} {package}` columns
under a per-provider header. Unlike `get`, `search` is discovery, not failover:
it queries every provider named in `--provider` (or all of them with `--all`, or
just the top-priority one by default) and shows each one's hits.

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
GETs one page to resolve the version: the app page for the latest string when
unpinned, or `/x/{pkg}/versions` to confirm a `--version` pin exists (a missing
build → `NotFound` → the resolver fails over, rather than the endpoint quietly
serving "latest"). Either GET also 404s → `NotFound` for an unknown package.

### APKCombo resolution flow

No Cloudflare, no captcha on the download path (the page's reCAPTCHA is unrelated).
`slug_for` reads the `{slug}` URL segment off the bare app page `{BASE}/{pkg}/`
(it self-links canonically); then: download page → scrape `var xid` → `POST
/{slug}/{pkg}/{xid}/dl` (form: package_name, version) returns the variant fragment
→ `POST /checkin` returns an `fp=…&ip=…` token → `{r2_href}&{token}&package_name=…`
302s to a signed R2 URL. Variant fragment parsing reads both the recommended
(`#best-variant-tab`) and full (`#variants-tab`) lists.

### Fixtures

Pure parsers live in each provider's `src/parse.rs`, unit-tested against saved
fixtures in `tests/<app>/` (one dir per app; `fixtures::app_dirs` reads the
directory, so adding a dir extends coverage with no code change). The walk and
the file read are shared by all three providers, so they live in the dev-only
`fixtures` crate rather than three times over. Each provider ships a
`tests/refresh-fixtures.sh`: no args re-fetches every app dir in its `APPS` map,
`refresh-fixtures.sh <app> <package-id>` adds or refreshes one. Run it after a
site changes, check the diff, adjust selectors. Tests assert on structure, not
version numbers, so a refresh rarely breaks them. Selectors are `const &str`.

Every provider carries the same two fixture apps: `youtube/`
(`com.google.android.youtube`) and `youtube-music/`
(`com.google.android.apps.youtube.music`). Both are proprietary Play-gated
Google apps — the tool's real use case, and the hardest case each provider has
to handle. YouTube earns its place twice over: its APKCombo `/{pkg}/` page
soft-404s, so it pins the `/en/` hop that a Spotify-only corpus sailed past.

apkmirror keeps a third dir, `nonexistent/`, whose no-results search page must
parse as `NotFound` rather than junk hits off the "Popular uploads" widget.

The **apkcombo dir name doubles as its search query** (APKCombo search is
name-based, not package-id-addressable), so renaming a dir there changes what
gets fetched. The other two search by package id and treat the dir name as a
label.

Each app dir's fixtures are cross-checked against *each other* — the package the
version/download pages are for must be one the search page actually found. A
per-file "did this parse" assert cannot catch a fixture captured from the wrong
page; this can.

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
| 130 | cancelled with Ctrl+C (`128 + SIGINT`) |

