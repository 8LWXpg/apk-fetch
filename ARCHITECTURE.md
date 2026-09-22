# apk-fetch architecture

`apk-fetch` downloads APKs from third-party mirror sites. Each site is isolated
behind one trait.

## Module layout

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

Modules use the `foo.rs` + `foo/` form rather than `foo/mod.rs`. Everything about
one site lives in that provider's folder.

Dependency direction is strictly `main → cli → providers → common`; `common`
reaches for nothing above it. Default provider priority:
`apkcombo,apkpure,apkmirror`.

## Where the details live

The per-site resolution flows, the `Provider` trait and error contract, the
fixture strategy, the choice of `curl`, and the exit-code table are all
documented next to the code that implements them:

- `src/common/contract.rs` — trait + error types, the block/parse distinction.
- `src/common/fetch.rs` — system `curl` fetcher.
- `src/providers/fixtures.rs` — fixture placement and capture strategy.
- `src/cli/exit.rs` — exit-code conversion.
- Each `<provider>.rs` / `parse.rs` — that site's resolution flow.
