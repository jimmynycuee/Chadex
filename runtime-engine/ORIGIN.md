# Chadex Runtime Engine — Source Origin

`runtime-engine/` is the production runtime source used by Chadex.

It was derived from the Apache-2.0 licensed WebCodex project at fixed revision `1bdc05e488ee56bca358bc6ba455cebd5831917d`, then progressively modified through the Chadex runtime, performance, task-execution, worktree-isolation, and execution-semantics phases.

Phase 6I promotes that maintained derivative into Chadex-owned production package/build identity:

- production packages use `chadex-runtime-*` identities;
- production Cargo manifests and build scripts resolve only through `runtime-engine/`;
- `vendor/webcodex` is not required to compile, test, or package Chadex;
- the original fixed upstream snapshot remains in `vendor/webcodex` only for provenance, historical comparison, and attribution;
- the Apache-2.0 license remains in this directory and is bundled through `attribution/WebCodex-LICENSE.txt`.

This ownership boundary does not erase upstream authorship. See `../UPSTREAM.md` for the upstream repository, fixed revision, migration history, attribution, and validation evidence.
