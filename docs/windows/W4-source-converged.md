# Windows Track W4 — Source-converged Knowledge

W4 updates Graphify and the existing Obsidian Chadex project from the W3 source. It starts on `codex/w4-source-converged` from final W3 commit `dd7d5ebbc4bf06c91ebe3a86a8e52c2d38fc52c3`. [W3 final CI 37029362354](https://github.com/jimmynycuee/Chadex/actions/runs/37029362354), attempt 2, was rechecked as successful on 2026-10-03 (Asia/Taipei), including all four jobs and the Windows W2/W3 gates.

## Ownership and scope

Git owns exact source/history and CI artifacts own executed results. Graphify supplies code/symbol/relation navigation. Obsidian records architecture, decisions, phase evidence and limitations. Graph coverage and generated links are not execution or product-acceptance evidence.

W4 preserves the macOS stable release history and adds Windows development knowledge. W2 runtime acceptance is complete. W3 automated implementation is verified; full W3 acceptance remains pending. The six native/private categories remain `not validated`: picker, Explorer, tray, launch-at-login, notifications and credentialed tunnel/ChatGPT workflow. Installed resource discovery, physical Windows 11/ARM64, signing, installer and updater are also uncertified. W4 does not merge `main`, publish a release or start W5.

## Source mapping

The existing sync script now includes the Windows `.ts`/`.tsx` and Rust production paths alongside Swift/helper/runtime sources. Three components connect the existing helper/runtime components to W3: Windows Desktop UI, Windows Native Bridge, and Windows Credentials and Preferences. Test fixtures, smoke-only sources, declarations, vendor provenance and dependency/build directories are excluded from curated production notes. Disconnected source nodes still receive notes when they are component anchors; inferred graph relations remain visibly identified as navigation evidence.

Graphify ignore rules explicitly exclude Windows dependency, build, resource and projected-asset directories. Existing source, vendor provenance and historical graph backups are preserved. No semantic LLM extraction or community-labeling API call is required.

The generated layer stays bounded at 17 components and at most 48 production file notes. Stable sorting removes dependence on Python hash seed and graph node/link order. Generated wikilinks and Canvas file references use Vault paths to disambiguate archived notes with identical names. Human Phase/Decision notes remain outside the generated sync boundary.

## Reproduce and validate

Use the installed `graphify` on PATH and supply the existing Obsidian Chadex project directory:

```sh
graphify update .
graphify check-update .
python3 scripts/test_sync_graphify_obsidian.py
python3 scripts/sync_graphify_obsidian.py --source-root . \
  --graph graphify-out/graph.json --obsidian-root "$CHADEX_OBSIDIAN_PROJECT" --dry-run
python3 scripts/sync_graphify_obsidian.py --source-root . \
  --graph graphify-out/graph.json --obsidian-root "$CHADEX_OBSIDIAN_PROJECT"
python3 scripts/sync_graphify_obsidian.py --source-root . \
  --graph graphify-out/graph.json --obsidian-root "$CHADEX_OBSIDIAN_PROJECT" --check
```

The sync detects the Vault root by its existing `.obsidian` directory; detached fixture directories retain project-relative paths. Tests cover Windows source inclusion, smoke/test exclusion, disconnected anchors, output determinism across input ordering and three hash seeds, Vault-relative Canvas/links, idempotence and drift detection. The existing source release gate already runs this test suite.

Acceptance requires a successful normal graph refresh, explicit Windows anchor coverage, valid graph/source provenance, zero generated-output drift, valid YAML/source/commit references and resolved touched-note/Canvas links. A docs-only final commit can differ from `graph_built_at_commit`; record both SHAs and inspect their diff instead of rewriting graph metadata. `check-update` is a pending semantic-update signal, not a substitute for source coverage. Zero-node files and changed community labels remain known navigation limitations.

The W4 handoff records actual graph/sync counts, validation and final branch/CI status. W5 release readiness/distribution requires a separate instruction and real native/private acceptance evidence.
