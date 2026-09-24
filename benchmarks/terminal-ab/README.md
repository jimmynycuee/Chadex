# Terminal WebCodex vs codex-chatgpt-web A/B benchmark

This benchmark deliberately avoids Codex task spawning. A parent Python process creates two fresh
Git fixtures at the same pinned commit, launches each provider from Terminal, captures provider/runtime
logs, stops timing, and only then runs the hidden evaluator.

## Providers

- **A — WebCodex**: starts `webcodex share` with a unique profile and isolated state directory. A
  separate Terminal driver must submit the public prompt to ChatGPT Web with that live WebCodex
  connector and wait for the final response.
- **B — codex-chatgpt-web Full mode**: launches standalone `codex exec --json` with the configured
  `chatgpt-web/extra-high` route, captures Codex JSONL, and copies only browser-turn diagnostics that
  changed during that run.

The harness never substitutes a direct WebCodex tool replay for A. `webcodex share` exposes MCP tools
but does not itself send a prompt to ChatGPT Web, so a real browser/ChatGPT Web driver is required for
a valid A run.

## Fairness rules

Both arms receive only the public prompt and their own clean fixture. Hidden tests remain outside the
fixture and run after the provider process exits. Fixture setup, grading, and report generation are
outside the agent task. `suite` alternates provider order between the short and long task to reduce a
fixed warm-cache/order bias.

Each pair also writes `run-metadata.json` with the exact baseline SHA, public-prompt SHA-256,
provider order, fixture/hidden-grading policies, and the declared/inferred model-effort labels. The
same metadata is embedded into `comparison.json` so a result cannot be interpreted without its
fairness inputs.

The short task has an exact canonical diff. The long Task History task intentionally allows multiple
valid implementations, so its diff correctness is behavioral: all hidden tests must pass and
`git diff --check` must stay clean.

## Metrics

The JSON result records:

1. parent-observed wall time;
2. tool calls;
3. distinct files explicitly evidenced in tool/command logs;
4. shell calls and build/test calls;
5. retries;
6. token/context usage;
7. hidden-test pass rate;
8. diff correctness;
9. one-shot completion;
10. context/continuation failure markers.

`files_read` is marked as a lower bound unless exact evidence exists. WebCodex runtime logs do not own
ChatGPT Web token accounting or model context state, so A's token/context, model-level retry, and
continuation fields remain `null` unless the external driver reports them. Missing evidence is never
filled with an estimate.

## Driver contract for A

Set `WEBCODEX_BENCH_DRIVER` or pass `--webcodex-driver-command`. The harness exports:

- `BENCH_TASK`
- `BENCH_FIXTURE_ROOT`
- `BENCH_PROMPT_FILE`
- `BENCH_WEBCODEX_STATE_DIR`
- `BENCH_WEBCODEX_SHARE_JSON`
- `BENCH_WEBCODEX_MCP_URL`
- `BENCH_WEBCODEX_AUTH`
- `BENCH_WEBCODEX_SHARE_PID`
- `BENCH_DRIVER_RESULT`
- `BENCH_MODEL`
- `BENCH_EFFORT`

The driver must use ChatGPT Web with the supplied live WebCodex connector, submit exactly the prompt
from `BENCH_PROMPT_FILE`, wait until the final response is complete, then exit 0. It may write
`BENCH_DRIVER_RESULT` as JSON with any model-level evidence it can prove, for example:

The repository includes `scripts/webcodex_chatgpt_driver.mjs` for the browser side. It attaches to
the existing logged-in launcher browser, uses a clean Temporary Chat, explicitly selects Extra High,
selects the connector named by `BENCH_WEBCODEX_CONNECTOR_NAME` (default `WebCodex Benchmark`),
submits the prompt once, and exits only after a stable completed assistant turn is observed.

For `--webcodex-auth query-token`, WebCodex intentionally redacts the sensitive query-token MCP URL
from its JSON machine output. In that mode `BENCH_WEBCODEX_MCP_URL` is therefore empty rather than
misreporting the local server URL. The ChatGPT custom App still needs to be bound to the exact
temporary endpoint for that run before the timed browser turn begins.

For benchmark validity, this A-side driver must be independent browser automation. It must not invoke
`codex`, codex-chatgpt-web, Codex Native2, or another Codex task/thread as the model transport; doing
so would make A share B's transport/runtime and contaminate the comparison.

```json
{
  "completed": true,
  "retries": 0,
  "token_context_usage": {"input_tokens": 1234, "output_tokens": 567},
  "context_continuation_failure": false
}
```

If ChatGPT Web does not expose token usage, omit it instead of estimating it.

## Commands

Preflight without running an agent:

```bash
python3 scripts/benchmark_terminal_ab.py preflight
```

Run one short A/B pair:

```bash
python3 scripts/benchmark_terminal_ab.py pair --task short \
  --webcodex-driver-command '/usr/local/bin/node /absolute/path/to/scripts/webcodex_chatgpt_driver.mjs' \
  --webcodex-tunnel cloudflare \
  --webcodex-auth query-token
```

Run the short + long suite:

```bash
python3 scripts/benchmark_terminal_ab.py suite \
  --webcodex-driver-command '/usr/local/bin/node /absolute/path/to/scripts/webcodex_chatgpt_driver.mjs' \
  --webcodex-tunnel cloudflare \
  --webcodex-auth query-token
```

Generated run artifacts live under `benchmarks/terminal-ab/results/` and are git-ignored.
