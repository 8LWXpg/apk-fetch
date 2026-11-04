# Architecture

`apk-fetch` downloads APKs from third-party mirror sites. Each site is isolated
behind one trait.

## Module Layout

One crate, one binary, layering is enforced by module visibility rather than by
separate packages. Items shared across modules are `pub(crate)`.

```
src/
├── main.rs            mod declarations, the tokio runtime, exit code, error print
├── cli.rs             facade: mod args, commands, dispatch, exit
├── cli/args.rs, dispatch.rs, exit.rs, commands.rs
├── common.rs          facade + re-exports
├── common/contract.rs Provider trait, domain types, ProviderError, ProviderRegistry
├── common/ui.rs       output macros (info!/warning!/success!/error!)
├── common/fetch.rs    HTTP via the system curl
├── providers.rs       facade: pub(crate) use ApkCombo, ApkMirror, ApkPure
└── providers/
    ├── fixtures.rs    #[cfg(test)] fixture plumbing
    ├── scrape.rs      shared selectors, choose_variant, query/version helpers
    ├── apkcombo.rs,   apkcombo/{parse.rs, tests/}
    ├── apkmirror.rs,  apkmirror/{parse.rs, tests/}
    └── apkpure.rs,    apkpure/{parse.rs, tests/}
```

Modules use the `foo.rs` + `foo/` form. Everything about one site lives in that
provider's folder.

Dependency direction is strictly `main → cli → providers → common`; `common`
reaches for nothing above it.

## Where the Details Live

- `src/common/contract.rs` — trait + error types, the block/parse distinction.
- `src/common/fetch.rs` — system `curl` fetcher.
- `src/providers/fixtures.rs` — fixture placement and capture strategy.
- `src/cli/exit.rs` — exit-code conversion.
- Each `<provider>.rs` / `parse.rs` — that site's resolution flow.

## Adding a provider

1. **`ProviderId`** (`common/contract.rs`): add the variant. Also add it to `DEFAULT_PRIORITY`.
2. **Files**: `providers/<name>.rs` + `providers/<name>/parse.rs`. Add `mod <name>;` and `pub(crate) use` in `providers.rs`, and the arm in `build_registry` (`cli/dispatch.rs`).
3. **Boilerplate**: a struct holding an `HttpFetcher` with `#[derive(Default)]`; `ProviderConst` (`ID`, `BASE_URL`); `pub type Url = contract::Url<super::<Name>>` in `parse.rs`; the `#[cfg(test)] with_fetcher`. Copy from `aptoide.rs` (API) or `apkpure.rs` (scraping).
4. **Contract** (see `contract.rs`):
   - `search` is user-facing; `download_url` must not depend on it.
   - `versions` must be sorted newest first, with no duplicates.
   - `download_url(pkg, None, _)` must return the same version as `versions()[0]`.
   - `DownloadTarget.arch` is what the build actually has, universal if the site says nothing. Use `scrape::choose_variant` when picking among variants.
   - Anything the CDN checks (e.g. `Referer`) goes in `headers`.
5. **Errors**: `NoMatch` = site answered, nothing there (no hit, version, variant). `ParseError` = markup/JSON shape changed. Don't produce `Blocked`/`NotFound`/`Network` yourself; `HttpFetcher` maps status codes and challenge pages. Exit codes follow from the variant (`cli/exit.rs`).
6. **Fixtures**:
   - Write `fixture_name(url)`: maps each fetched URL to a file stem. Playback panics on an unmapped URL; `None` means "don't record". Order of checks matters when URLs overlap.
   - Add the `refresh` and `tests` modules (copy from any provider).
   - Run `cargo test refresh_fixtures -- --ignored --nocapture` (needs network), read the printed diff, commit `tests/<app>/` including `record.json`.
   - The flow runs `search(<app dir name>)`, `versions(pkg)`, `download_url(pkg)` for each entry in `fixtures::APPS`, so the provider must resolve those.
7. **Docs**: help text of `get --priority` in `args.rs`, README (intro and footer), the module tree here.

