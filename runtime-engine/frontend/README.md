# Runtime Console build assets

`dist/` is intentionally versioned because `runtime-engine/src/console_web.rs` embeds these files at Rust compile time with `include_str!`. A fresh clone must therefore contain this directory; it is not a machine-local build cache.

The current assets are generated from the fixed upstream-derived frontend sources retained at `vendor/webcodex/frontend/`. That vendor tree is **not** part of the production Cargo dependency graph or normal app build path. Regeneration is a maintainer operation only:

```sh
./scripts/update_runtime_frontend.sh
```

After regeneration, review the `runtime-engine/frontend/dist/` diff together with any source/provenance change. Do not replace this directory with an ignored local cache: doing so makes a fresh-clone Rust build fail before compilation.
