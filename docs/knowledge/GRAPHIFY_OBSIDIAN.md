# Graphify -> Obsidian knowledge sync

Chadex keeps two different graphs on purpose:

- **Graphify** is the detailed code graph: symbols, files, calls, dependencies, and confidence.
- **Obsidian** is the human development graph: phases, decisions, architecture components, problems, results, and lessons.

The integration is curated instead of copying tens of thousands of Graphify nodes into the vault.

## One-command refresh

Set the local Obsidian Chadex project path once:

```sh
export CHADEX_OBSIDIAN_PROJECT="/path/to/Obsidian/YourVault/01 Projects/Chadex"
```

Then run:

```sh
./scripts/update_graphify_obsidian.sh
```

The wrapper performs:

1. records any pre-existing dated Graphify backup directories
2. runs `graphify update .`
3. parses `graphify-out/graph.json`
4. selects Chadex production nodes only
5. generates bounded component and high-degree source-file notes
6. creates Graphify-to-Phase wikilinks
7. refreshes `Architecture/Graphify Architecture.canvas`
8. after a successful sync, removes only dated Graphify backup directories created by that run; pre-existing archives are never touched

This keeps `graphify-out/graph.json` as the single canonical code graph instead of accumulating parallel dated copies during routine Obsidian refreshes.

The generated subtree is:

```text
Architecture/
  Graphify Architecture.canvas
  Generated/
    Graphify/
      Graphify Snapshot.md
      Graphify Knowledge Index.md
      Components/
      Files/
```

Only files carrying `generated_by: "chadex-graphify-obsidian-sync"` inside that generated subtree are eligible for stale-file cleanup. Human-authored Phase, Decision, Architecture, Bug, Session, and project notes are never replaced by the sync.

## Why the graph is bounded

Importing every Graphify symbol would produce tens of thousands of Obsidian nodes and a largely unusable graph. The sync instead emits:

- stable architecture components,
- up to 48 high-degree production source files by default,
- component-to-file and file-to-file links,
- component-to-Phase links,
- a generated snapshot/index,
- a radial Architecture Canvas.

This gives Obsidian Graph View enough structure to show the dense development network while keeping Graphify as the authoritative detailed code graph.

## Validation

Run the unit test:

```sh
python3 scripts/test_sync_graphify_obsidian.py
```

Preview without writing:

```sh
python3 scripts/sync_graphify_obsidian.py \
  --graph graphify-out/graph.json \
  --source-root . \
  --obsidian-root "$CHADEX_OBSIDIAN_PROJECT" \
  --dry-run
```

Check whether the generated layer is current:

```sh
python3 scripts/sync_graphify_obsidian.py \
  --graph graphify-out/graph.json \
  --source-root . \
  --obsidian-root "$CHADEX_OBSIDIAN_PROJECT" \
  --check
```

A non-zero `--check` result means the generated Obsidian layer differs from what the current graph would produce.

## Freshness caveat

Graphify may keep its existing report/commit metadata when an update has no topology delta. The generated snapshot records both the graph's `built_at_commit` and the current source HEAD and shows a mismatch warning rather than silently claiming they are identical.
