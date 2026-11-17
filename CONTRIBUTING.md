# Contributing

Read [Architecture](./ARCHITECTURE.md) first.

## Adding a New Provider

To add a new app source provider to the CLI, follow these steps to ensure it integrates cleanly with the registry, contract rules, test fixtures, and documentation.

### 1. Register the Provider ID
* **File:** `common/contract.rs`
* Add your new variant to the **`ProviderId`** enum.
* Add the new variant to **`DEFAULT_PRIORITY`** so it is included in standard resolution flows.

### 2. Create and Wire Up the Files
* **Files to create:** `providers/<name>.rs` and `providers/<name>/parse.rs`.
* **Register in `providers.rs`:** Add `mod <name>;` and expose it via `pub(crate) use`.
* **Register in CLI:** Add the matching arm to `build_registry` in `cli/dispatch.rs`.

### 3. Implement Boilerplate Code
Set up the core structure for your provider. To keep things consistent, look at existing implementations:
* Use **`aptoide.rs`** as a reference for API-based providers.
* Use **`apkpure.rs`** as a reference for scraping-based providers.

Your implementation should include:
* A struct holding an `HttpFetcher` with `#[derive(Default)]`.
* A **`ProviderConst`** implementation defining `ID` and `BASE_URL`.
* The type alias in `parse.rs`: `pub type Url = contract::Url<super::<Name>>`.
* The `#[cfg(test)] with_fetcher` helper.

### 4. Fulfill the Provider Contract
Ensure your implementation adheres to the core contracts outlined in `contract.rs`:
* **Search isolation:** `search` is strictly user-facing; `download_url` must never depend on search results.
* **Version ordering:** `versions` must return results sorted newest-first with zero duplicates.
* **Default download consistency:** `download_url(pkg, None, _)` must return the exact same version as `versions()[0]`.
* **Architecture targeting:** `DownloadTarget.arch` must reflect what the build actually requires (default to universal if the site specifies nothing). Use `scrape::choose_variant` when selecting among multiple variants.
* **Headers:** Any headers checked by the CDN (e.g., `Referer`) must be explicitly included in `headers`.

### 5. Handle Errors Correctly
Map errors strictly to standard semantics:
* **`NoMatch`**: Use when the remote site answered successfully, but found nothing (e.g., no search hit, no version, or no variant).
* **`ParseError`**: Use when the remote HTML markup or JSON shape has changed and cannot be parsed.
* **Do not manually produce** `Blocked`, `NotFound`, or `Network` errors; let `HttpFetcher` automatically map status codes and challenge pages.
* **Exit codes:** Ensure your provider's error states flow correctly into `cli/exit.rs`.

### 6. Write and Record Test Fixtures
Tests rely on recorded network fixtures to ensure reliability:
1. **Define mapping:** Write `fixture_name(url)` to map each fetched URL to a file stem. Playback will panic on any unmapped URL (returning `None` skips recording). Note that the order of checks matters if URLs overlap.
2. **Add modules:** Add the `refresh` and `tests` modules (copy the pattern from any existing provider).
3. **Record fixtures:** Run the following command (requires network access):
   ```bash
   cargo test refresh_fixtures -- --ignored --nocapture
   ```
4. **Commit artifacts:** Read the printed diff and commit the resulting `tests/<app>/` directory, including `record.json`.
5. *Note on test flow:* The test runner automatically executes `search(<app dir name>)`, `versions(pkg)`, and `download_url(pkg)` for each entry in `fixtures::APPS`, so your provider must successfully resolve all of them.

### 7. Update Documentation
Don't forget to update references across the codebase:
* Update the help text for `--priority` in `args.rs`.
* Update the `README.md` (intro and footer references).
* Add your module to the provider module tree.
