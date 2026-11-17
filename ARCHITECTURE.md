# Architecture

`apk-fetch` downloads APKs from third-party mirror sites. Each site is isolated
behind one trait.

## Module Layout

One crate, one binary, layering is enforced by module visibility rather than by
separate packages. Items shared across modules are `pub(crate)`.

```
src/
├── main.rs            mod declarations, exit code, error print
├── cli.rs             mod args, commands, dispatch, exit
├── cli/args.rs, dispatch.rs, exit.rs, commands.rs
├── common.rs          mod + re-exports
├── common/contract.rs Provider trait, domain types, ProviderError, ProviderRegistry
├── common/ui.rs       output macros (info!/warning!/success!/error!)
├── common/fetch.rs    HTTP via the system curl
├── providers.rs       pub(crate) use Aptoide, ApkPure, ApkMirror
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

