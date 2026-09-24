# WebCodex Upstream Attribution and Modifications

Chadex contains production code derived from WebCodex under the Apache License 2.0, and retains the fixed upstream source snapshot for provenance and historical comparison.

The Chadex repository itself is distributed under the Apache License 2.0; see the root `LICENSE`. The upstream copies below are retained separately so derivative provenance remains explicit.

- Repository: `https://github.com/yyjeqhc/webcodex`
- Fixed source revision: `1bdc05e488ee56bca358bc6ba455cebd5831917d`
- Upstream package version at that revision: `0.4.1`
- Upstream source timestamp used for reproducible build metadata: `1789620198`
- License: `vendor/webcodex/LICENSE`, `runtime-engine/LICENSE`, and packaged copy `attribution/WebCodex-LICENSE.txt`

`vendor/webcodex/.git` is intentionally not included. As of Phase 6I, production build scripts do not compile or link from `vendor/webcodex` and no longer pin `WEBCODEX_GIT_*` as the runtime build identity. Chadex production runtime metadata uses `CHADEX_RUNTIME_GIT_*` from the Chadex repository. The fixed upstream revision above remains the provenance anchor for the derivative source and is documented independently of the current Chadex build identity.

## Chadex-specific modifications

Phase 6I 後，production MCP/file/Runner implementation 位於 Chadex-owned `runtime-engine/`。這份 source 是由上述固定 WebCodex baseline 演進而來的 derivative，但 production Cargo package identity、build path、runtime entrypoints、process ownership 與 app bundle 都由 Chadex 擁有；`vendor/webcodex` 不再是 library/build dependency。Secure MCP Tunnel 的 Tunnel ID、API key、OpenAI `tunnel-client` process、readiness 與 ChatGPT verification 仍全部由 `rust-helper/src/chadex_core/` 擁有。原始 vendor snapshot 保留作 provenance、歷史比較與必要 attribution；以下項目記錄 Phase 6A–6H 遷移期間曾直接存在於 upstream-derived tree 的 Chadex-specific modifications：

1. `apps/desktop/src-tauri/src/models.rs`
   - Keeps `TunnelConfigSource::Memory` for the inert backend-only placeholder used by Chadex's path-based integration.
2. `apps/desktop/src-tauri/src/tunnel_config.rs`
   - Keeps an in-memory/empty TunnelConfig mode so the temporary backend never loads an upstream `tunnel-config.json` or inherited Tunnel credential.
   - This config is not populated by Chadex and is not used to launch a Tunnel.
3. `apps/desktop/src-tauri/src/state.rs`
   - Adds `AppState::new_for_chadex_backend`, which constructs the temporary backend with an empty/inert TunnelConfig.
   - Adds `ChadexRuntimeTunnelTarget` / `chadex_runtime_tunnel_target`, exposing only the non-secret loopback server URL and private server-env **path** needed by Chadex Core.
   - Adds `ChadexRuntimeProbeTarget`, `chadex_runtime_probe_target`, and `chadex_apply_runtime_probe` for Phase 6B's exact saved-runtime observation. The boundary exposes a user-token **file path**, never token contents, and the apply step rechecks the full saved identity before publishing readiness.
   - Adds `ChadexProjectActivationTarget` / `ChadexProjectActivationObservation` plus `activate_local_project_with_chadex_fast_path` for Phase 6C. The callback remains inside the existing Desktop operation/cancellation/process-baseline fence; the apply step revalidates saved runtime identity and reuses the original project-activation commit/rollback path.
   - Adds `configure_local_setup_with_chadex_fast_path` for Phase 6D. The default WebCodex local-setup path supplies no fast observation; Chadex may supply only a bounded read-only Runner/project readiness observation after the owned Runner has been spawned, with the original CLI readiness/activation path retained as fallback.
   - Chadex reads backend authentication tokens inside Rust; token contents are never returned through the helper protocol.
   - The previous Chadex-specific WebCodex credential injection/clearing APIs have been removed from `AppState`.
4. `apps/desktop/src-tauri/src/tunnel_config/tests.rs`
   - Verifies the Chadex backend boundary keeps WebCodex Tunnel state empty/inert and creates no Tunnel credential file.

Phase 6A first introduced the Chadex-owned runtime contract and isolated upstream-derived state behind an adapter. Phase 6B–6D then moved readiness observation and project activation onto Chadex-owned typed paths while preserving the existing authorization, cancellation and process fences. Phase 6E–6G removed user-facing upstream branding leaks and narrowed compatibility boundaries. Phase 6H transferred production executable/process/bundle ownership to `chadex-runtime-*` entrypoints while still linking the temporary vendored libraries. Phase 6I completes the production dependency cut: the maintained implementation is promoted into `runtime-engine/`, package identities are `chadex-runtime-*`, helper/runtime manifests resolve only to that tree, and production build metadata is Chadex-owned. A hard acceptance physically removed `vendor/webcodex` before running locked/offline Cargo checks and a complete signed `./scripts/build_app.sh`; the build succeeded and the final bundle contained only Chadex runtime executable names while retaining `UPSTREAM.md` and the exact Apache-2.0 license copy. The original vendor snapshot was then restored unchanged for provenance/reference only.


## 2026-09-19 Chadex turn economy 與契約修補

- `webcodex-tool-contracts`：精簡高頻工具的使用指引，重用 exact Project identity、減少重複 status/discovery、使用既有 read batch；保留工具與權限能力。部分讀取的 output schema 明確要求 snapshot revision，完整讀取仍可省略。
- `webcodex-tool-runtime-contracts`：補上 `read_files.include_read_revision` 輸入白名單，使既有公開 schema 與 runtime 實作一致；新增 true/false、錯誤型別與拼字錯誤回歸案例。
- 未升級 upstream baseline；測試、量測與部署界線見 `docs/SECURITY_AND_PERFORMANCE_REVIEW.md`。
