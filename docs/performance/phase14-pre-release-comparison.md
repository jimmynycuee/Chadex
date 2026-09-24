# Phase 14 Pre-Release Performance Comparison

Date: 2026-09-22
Environment: macOS 26.6.2, arm64, Mac14,2

## Scope

This benchmark checks whether Phase 14A/14B lifecycle, recovery, durability, and runtime-resilience work caused a material executor-path regression before Phase 14C.

The directly comparable benchmark is the preserved Rust integration harness used for the Phase 11 -> Phase 12 comparison: one warmup, single-threaded exact test execution, process startup included, build time excluded. Phase 13, Phase 14, and the WebCodex-derived Phase 12 compatibility runtime were freshly rebuilt with the `dogfood` profile and measured in a balanced rotating order. Phase 11 uses the original 11-sample evidence because its exact executable is no longer retained.

The Phase 13, Phase 14, and WebCodex-derived compatibility-runtime test bodies are identical for both measured cases after whitespace normalization:

- `small_read_cancel`: `5be12ab2904b3bf1`
- `small_edit_validate_review`: `3faa30322406d371`

Source identities:

- Phase 11 historical binary: `e06a31fd7e6d1b43517e4e0972c1a5130ae72fc33fa812de488f02a001f6e119`
- Phase 13: commit `b717b1c`, measured binary `58fdac2958c6bb7f4f2f7665e74c3460ea06b73de24e0fd72b9a26a5aed5d256`
- Phase 14: commit `c0b8297`, measured binary `046da14e2b84aa209997e0a9f42ff13e5a5316299ea5447fe4d9bd4e0523c407`
- WebCodex-derived compatibility runtime: based on upstream `1bdc05e488ee56bca358bc6ba455cebd5831917d` / v0.4.1 and the retained Phase 12 executor compatibility surface, measured binary `8d3950d2f0522add2b0c7c052b26065e454b8a3146819d0d38c128e3f0f58934`. A pristine checkout of upstream `1bdc05e4` does not contain the Chadex `execute_task` benchmark cases and therefore cannot participate directly in this executor micro-benchmark.

## Results

| Case | Phase 11 median | Phase 13 median | Phase 14 median | WebCodex-compat median | Phase 14 vs P13 | Phase 14 vs compat | Phase 14 vs P11 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Small read + cancel | 23.157 ms | 38.896 ms | 38.086 ms | 37.721 ms | -2.08% | +0.97% | +64.47% |
| Edit + validate + review | 1050.466 ms | 1047.554 ms | 1070.951 ms | 1062.501 ms | +2.23% | +0.80% | +1.95% |

For the 1-second edit/validate/review case, a second 7-sample confirmation run produced:

| Variant | Confirmation median |
| --- | ---: |
| Phase 13 | 1047.399 ms |
| Phase 14 | 1066.249 ms |
| WebCodex-derived compat | 1056.137 ms |

That confirmation puts Phase 14 at +1.80% vs Phase 13 and +0.96% vs the WebCodex-derived compatibility runtime, reproducing the primary result.

## Interpretation

The tiny read/cancel case exposes fixed lifecycle overhead that did not exist in the Phase 11 baseline: Phase 14 is about 15 ms slower in absolute terms than Phase 11. This overhead predates Phase 14 itself: Phase 13 and the compatibility control cluster with Phase 14 around 38 ms, and Phase 14 is slightly faster than Phase 13 in this case.

For the more representative edit -> validate -> review path, Phase 14 remains within about 2% of Phase 11, Phase 13, and the WebCodex-derived compatibility runtime. The interquartile ranges overlap, so the small ordering between these controls should not be treated as a meaningful performance ranking.

The compatibility-control primary run had one 6.04-second outlier in the edit case; median statistics are therefore used for the main comparison. The 7-sample confirmation run had no comparable outlier and reproduced the same ~1% Phase 14 vs compatibility-control difference.

## Release-gate conclusion

No material Phase 14A/14B executor-path performance regression was detected. Phase 14 adds substantially stronger execution continuity, durable recovery, bounded helper memory/RPC behavior, and cleanup semantics while keeping the realistic measured path effectively at Phase 11/13/WebCodex-derived compatibility-runtime performance levels.

This is not a ChatGPT Web/network E2E benchmark. Historical Phase 11 connected-runtime measurements (8s/12s package workloads) remain useful context, but they are not mixed into the four-way table because the four versions were not all measured through the same connected runtime in this run.

Raw samples and confirmation data are in `benchmarks/phase14-pre-release-comparison.json`. The rerunnable harness is `benchmarks/phase14_pre_release_comparison.py`.
