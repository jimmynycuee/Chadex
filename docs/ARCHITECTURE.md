# Chadex Architecture

## 原則

Chadex 的 Swift 層不推測連線成功，而只消費 Rust helper 的 truth-based snapshot。Tunnel credential、OpenAI Secure MCP Tunnel lifecycle、readiness 與 ChatGPT verification 由 Chadex Core 管理；本機 MCP/file/Runner runtime 由 Chadex-owned `runtime-engine/` 提供。WebCodex 固定版 source 只保留作為 provenance、歷史比較與 Apache-2.0 attribution reference，不參與 production build。

## 元件

```text
SwiftUI / AppKit
  │
  ├─ ProjectStore (non-secret preferences)
  ├─ KeychainStore (API key)
  └─ HelperClient
       │ private NDJSON over stdin/stdout
       ▼
chadex-helper
  ├─ runtime_bridge (Chadex protocol only)
  ├─ Chadex Core
  │    ├─ CredentialStore (memory-only Zeroizing API key)
  │    ├─ TunnelManager
  │    │    └─ pinned OpenAI tunnel-client v0.0.12
  │    ├─ MCP ingress + VerificationTracker
  │    │    └─ epoch-fenced MCP proxy
  │    └─ ChadexRuntimeCore
  │         └─ RuntimeBackendAdapter (single migration boundary)
  │              └─ Chadex runtime entrypoints
  │                   ├─ chadex-runtime-cli
  │                   ├─ chadex-runtime-server
  │                   └─ chadex-runtime-runner
  │                        └─ Chadex runtime-engine crates
  │                             ├─ MCP/file operations
  │                             ├─ Runner execution
  │                             ├─ project activation
  │                             └─ process / tool / validation contracts
```

## 隔離

Chadex 不讀寫既有 WebCodex Desktop 的設定或憑證。預設 runtime 資料位於：

`~/Library/Application Support/Chadex/runtime`

Chadex preferences 位於：

`~/Library/Application Support/Chadex/preferences.json`

API key 不在上述檔案中，而在 macOS Keychain。

## 專案切換

UI 選取新專案時只呼叫 Chadex runtime contract。若目前 runtime 已就緒，`RuntimeBackendAdapter` 會呼叫 backend `activate_local_project`，並直接採用 backend 回傳的已驗證 project，避免 Chadex 再做一次重複 inspection；若 runtime 已停止，Chadex 仍先檢查 canonical path，再更新下一次要啟動的 target project。只有 Rust 回覆成功後 Swift 才更新 `selectedProjectID`。

Tunnel 若仍存在，切換前由 Chadex `TunnelManager` 停止它。每次 target project 改變，`VerificationTracker` 都建立新的 epoch；Tunnel ingress 只可更新它啟動時捕捉的 epoch，所以舊專案或舊連線的 MCP traffic 不能驗證新專案。

## 連線流程

「連接 ChatGPT」不是單一 optimistic flag：

1. `resumeService`：若尚無 runtime，建立本機 setup；若已儲存 runtime，恢復；若 runtime 已 ready，維持真實狀態。
2. `ChadexRuntimeCore` 確認 runtime project 與 UI target 一致；必要時由內部 `RuntimeBackendAdapter` 執行 backend project activation。
3. `startTunnel`：Chadex `TunnelManager` 取得本機 backend 的 loopback MCP endpoint，建立 Chadex MCP ingress，直接啟動固定版 OpenAI `tunnel-client`。
4. Chadex 先執行 `doctor` 與 control-plane probe，再以 `/readyz` 確認 Tunnel 真正 ready；此時 UI 只能進入「等待 ChatGPT 驗證」。
5. ingress 在 ready 後才 arm verification。`initialize` / `tools/list` 的有效相符回應只標示 connected；目前 epoch 的 `tools/call` 收到 ID 相符、非錯誤的完整工具結果後才標示 verified。HTTP 2xx、無 ID 的通知、RPC/tool error 與截斷回應本身都不能驗證專案。

## 程序生命週期

helper 從 stdin EOF 視為 parent-liveness lease 結束。它先呼叫 Chadex `TunnelManager.shutdown()`，只終止自己持有的 OpenAI `tunnel-client` child，再關閉 Chadex runtime engine。沒有以程序名稱廣泛 kill。

`⌘Q` 會經 `applicationShouldTerminate` 延後 App 終止，先送 `shutdown` request 給 helper；若 helper 在 bounded grace period 內沒有退出，Swift 才把該 helper child terminate。這不會碰既有 WebCodex instance。

## Polling

- 操作中：1 秒。
- 前景 idle：5 秒。
- 背景：15 秒。
- App 回到前景：立即 refresh。
- Swift 使用 `refreshInFlight` 避免重疊 status request。

helper 以 Tokio concurrent request tasks 處理 NDJSON。Tunnel start/stop/credential mutation 由 Chadex `lifecycle` mutex 序列化；stop flag 可中斷 `doctor`、control-plane probe 與 readiness wait。Runtime engine 內其餘副作用仍由既有 bounded operation/cancellation contracts 管理。

## Performance tracing

Phase 1 在不改變 MCP streaming 行為的前提下，於 Chadex-owned ingress 記錄最近 100 筆 request timing。量測會拆出 ingress 前處理、backend response headers RTT、response stream 與總時間；其中 backend headers RTT 暫時視為包含 temporary WebCodex backend、project/authority resolution、Runner round-trip 與 tool execution 的黑盒區段。Swift `HelperClient` 另以 monotonic clock 記錄最近 100 筆 Swift ↔ Rust helper round trip。

這些 timing 只在記憶體中 bounded 保存，並在使用者主動「匯出診斷資料」時讀取；正常 status polling 不會多打一個 performance query。trace 不記錄 request params、project path、檔案內容、Authorization header 或 API key。

## Phase 2 read hot path

高頻 Project-bound tool call 現在優先處理完整 runtime Project ID（`agent:<client_id>:<project_id>`），不再先列舉並複製所有可見 Runner/Project 後才找目標。成功解析的 exact ID 會進入最多 128 筆的 process-local cache；cache 依 Runner authority projection 分區，並由 Runner registration、Project inventory 原子切換、Project upsert/remove 與過期 Runner 清理共同推進的 routing epoch 立即失效。

Runner access 與 capability 檢查仍然保留，但同一 call 原本多次取得 registry lock 的 visibility / access / capability observation 已合併成一次 authoritative observation。沒有 Workflow recording Session 的一般 call 也不再建立最後必然被丟棄的 Session request/result audit projection。

模糊名稱、短 Project ID 與需要候選清單的錯誤仍走原有完整 resolver；permission evaluation、protected-path policy、`read_revision` / continuation consistency 與 mutation safety 沒有被 fast path 略過。

## Phase 3 read / mutation hot path

`read_files` 的 snapshot handle 改為 lazy：完整、inspection-only read 預設不建立也不輸出 `read_revision`；若後續 guarded edit 需要 snapshot handle，可用 `include_read_revision=true` 明確要求。只要 read 是 partial、需要 continuation，Runtime 仍會自動建立 revision，並把下一次 `read_files` 綁到同一 snapshot；呼叫者已帶 `expected_read_revision` 時也會維持原本的 stale-snapshot 驗證。Runner v1 read envelope 目前仍產生 SHA-256 作為內部完整性／snapshot 證據，因此這一階段移除的是不必要的 revision registry allocation/lookup 與 model-facing handle，不宣稱已消除 Runner 的檔案 digest 成本。

多檔 read 不新增大型 `file_read_batch` response。Runtime 先在本機完成 path / revision preflight，再用一次 RunnerRegistry lock 原子 admission 最多 8 個獨立 read request；整批 queue capacity、Runner instance、capability 與 Project placement 必須同時成立才會入列，任何 admission 失敗會 rollback 已插入的 request。Runner 仍回傳彼此獨立、bounded 的 response，Runtime concurrent await 後按原始 item index 恢復 deterministic ordering，沿用既有 shared deadline、cancellation 與 output budget。

`write_project_file` 與 `apply_text_edits` 則直接重用 outer dispatcher 已完成的 authoritative Project resolution，不再在 mutation implementation 內重跑一次 resolver。若呼叫路徑沒有預先解析結果，仍保留舊 resolver fallback；protected-path policy、permission / Runner authority、`expected_read_revision`、SHA stale-write guard、transactional edit、rollback 與 outcome-unknown semantics 均未被繞過。

## Phase 4 short-command Runner latency

Phase 1 重測顯示 `read_files` / mutation 已降到約 1–5 ms，但 `search_project_texts` 與 `git_status` 的 `backend_headers` 仍約 60 ms / 56 ms。對照同機器原生命令後，主要固定成本定位到 Runner shell lifecycle：短命令第一次 `try_wait()` 尚未觀察到退出時，舊實作固定 sleep 50 ms 才再次檢查，因此幾毫秒即可完成的 `git status`、`rg` / `grep` 仍會支付一個完整 50 ms completion tick。

Phase 4 將 active-command completion observation 改為自適應 polling：spawn 後前 25 ms 以 1 ms 間隔觀察短命令，之後退回 5 ms，避免長命令持續維持 1 kHz polling。這個 poll 只存在於已啟動 command 的 bounded wait loop，不改 Runner admission、Project / permission authority、timeout 上限、stop flag、process-tree cleanup 或 output bounds；更短的初始 poll 同時改善 cancellation / timeout observation latency。`git_status` 另優先使用 structured argv (`git`, `status`, `--porcelain`)，若 Runner 不支援該 capability 則保留原 raw-shell fallback。

在相同 isolated-helper / real temporary WebCodex backend / real Runner benchmark 下，`search_project_texts` 的 `backend_headers` median 由約 59.9 ms 降至 25.7 ms（約 -57%），`git_status` 由約 56.3 ms 降至 12.0 ms（約 -79%）；後者已接近同機器原生 `git status --porcelain` 約 10.8 ms 的 floor。`read_files` 維持約 1.1 ms，未出現回歸。剩餘 search 成本主要來自 bounded POSIX search wrapper（backend selection、status side-channel、`head` output caps、實際 `rg` / `grep`）與 Runner / backend 固定協調成本；不再由 Runner digest 主導。

## Phase 4B typed Runner-native search

新版 Runner 會明確註冊 additive `structured_search_text` capability。`search_project_texts` 在 Server 已完成 Project resolution、權限與 request normalization 後，優先送出 typed `search_text` Runner operation，而不是先產生 POSIX shell wrapper；只有舊 Runner 未宣告 capability 時才保留既有 `run_shell` + generated POSIX search fallback。這個 capability 不屬於 generation-2 baseline，因此 mixed-version rolling upgrade 不會把舊 Runner 誤判成支援新 request kind。

Typed request 只攜帶已正規化的 pattern / literal-or-regex mode、project-relative path、limit、context、include/exclude globs 與 result mode。Runner 重新驗證 payload bound 與 project-relative path，canonicalize 目標並再次確認沒有透過 symlink 逃出 Project root；搜尋仍使用 prepared Runner environment 找 `rg`，功能需要 ripgrep 而 `rg` 不存在時維持原 `feature_unavailable` 語意，普通 matches 則可退回 `grep`。Claude Code external-provider route 也保留：typed literal pattern 在送進舊 provider regex contract 前由 Runner escape，regex pattern 則原樣傳遞。

Runner-native executor 直接用 argv 啟動 `rg` / `grep`，不經 shell parser，並保留既有 protected excludes、deterministic parser marker、timeout / stop flag、managed process-tree cleanup 與 bounded stderr。stdout 只保留 32 KiB 加一個 byte probe，並依原本 `limit + 1`（context 模式含對應 line budget）提前停止整個 process tree；因此既有 Runtime parser 仍能判斷 `result_limit` / `output_bytes` truncation，而不需要先掃完整個 repository 再裁切。

在與 Phase 4 相同類型的 isolated-helper benchmark（real temporary WebCodex backend + real Runner；只有 external tunnel-client 使用 local fake stub）下，`search_project_texts` 共 30 次：`backend_headers` median 由 25.686 ms 降至 9.585 ms（約 -62.7%），p95 由 27.200 ms 降至 20.709 ms（約 -23.9%），`total` median 為 9.613 ms。作為未修改路徑的 control，`git_status` median 為 12.873 ms（Phase 4 為 11.985 ms，+0.888 ms），`read_files` 為 1.117 ms（Phase 4 為 1.072 ms，+0.045 ms）；這兩個差異視為本輪量測漂移，不宣稱 Phase 4B 對它們有性能改善。原始 Phase 4B 結果保留於 `/tmp/chadex_phase4b_perf_results.json`。

在與 Phase 4A 相同的 isolated-helper / real temporary WebCodex backend / real Runner benchmark 中，Phase 4B 將 `search_project_texts` 的 `backend_headers` median 從約 25.7 ms 降至 **9.6 ms**（相對 Phase 4A 約 -63%，相對最初約 59.9 ms baseline 累計約 -84%）；p95 約 20.7 ms。同期 `read_files` 維持約 1.1 ms，`git_status` 約 12.9 ms。相同搜尋條件的同機器 native grep floor 約 4.8 ms，因此目前剩餘約 4–5 ms median 差額主要是 MCP / RunnerRegistry / process supervision / result projection 的固定協調成本，而不是 POSIX wrapper 或 digest。這個量級已不支持僅為搜尋 hot path 立即進行整個 temporary WebCodex backend 重寫。

## Phase 6A Chadex Runtime Core boundary

Phase 6A 將產品 runtime contract 從 WebCodex Desktop state model 中抽離。`runtime_bridge` 現在只依賴 Chadex-owned `RuntimeSnapshot`、`RuntimeProject`、`RuntimeOperation`、`RuntimeProxyMode` 與 `ChadexRuntimeCore`；compatibility types 被限制在 `chadex_core/runtime_compat/` 與 `chadex_core/adapters/runtime_backend.rs` 邊界。因此後續可以替換 backend implementation，而不需要改 Swift protocol、Tunnel、verification 或大部分 helper orchestration。

Adapter 仍保留原有安全語意：`runtime_configured` 明確承接原本 `topology.is_some()` 判斷，避免抽象化後把「已有 project 但 runtime topology 尚未配置」誤當成可 resume；Project authorization、backend inspection、process ownership、cancellation、deadlines 與 protected-path policy 均沒有被繞過。

同一階段也移除第一批固定成本：helper response 改成 untagged typed payload，避免先把 typed response 轉成 `serde_json::Value` 再序列化一次；snapshot revision 比較不再 clone 舊 snapshot 只為清零 revision；readiness labels 與 operation kind 在 Chadex runtime boundary 使用 static labels，避免 healthy status snapshot 的重複字串配置。Swift/Rust wire shape 由專門測試固定。

`scripts/benchmark_workflow.py` 定義 Phase 6 後續固定 workflow：`runtime_status → read_files → search_project_texts → git_status`，使用真實 local MCP/Runner，預設 warmup 後量 p50/p95，另量 helper `getStatus` round trip。2026-09-19 保存於 `docs/performance/` 的同機 A/B control 中，Phase 6A 30-run workflow median 為 **43.054 ms**、p95 **46.194 ms**；Phase 5D snapshot 為 **45.802 ms**、p95 **48.237 ms**，觀察到約 6% 較低 median。量測時系統已有一套 Chadex runtime，因此兩版都重用同一個 MCP Server/Runner；6A 尚未修改該 hot path，這組數字主要用來確認沒有可辨識 regression，不將差距宣稱為 runtime-core 重構的因果加速。helper `getStatus` median 為 **0.099 ms**（Phase 5D **0.108 ms**），兩者都已遠低於使用者體感門檻；Phase 6B 之後才會以相同 workflow 量真正的 backend/orchestration 改善。

## Phase 6B typed runtime readiness observation

Phase 6B 把「既有 local runtime 是否仍可用」從 WebCodex Desktop 的多個 CLI 子程序探測移到 Chadex-owned typed observation。舊流程在 fresh helper 的 `resumeService` 會重新進入 `configure_local_setup`，依序執行 Server status、Runner status、Project readiness 等 CLI；`refreshRuntime` 也會重複走相同類型的 CLI status path。這些探測沒有改變 runtime，只是在確認已知 identity 是否仍成立，因此對已經健康的 runtime 形成接近 1 秒的固定恢復成本。

新的 fast path 只在已儲存的 local Full Runtime、autostart 開啟、且 Server URL / user-token file / Runner client ID / runtime Project ID / project path 全部完整時啟用。`WebCodexBackendAdapter` 以 `.no_proxy()` 的 bounded `reqwest::Client` 直接連到明確 port 的 loopback HTTP origin，從 user-token file 在 Rust 內取得 token，經正式 `call_runtime_tool → list_projects` admission 查詢 exact `client_id + runtime_project_id`。只有回覆同時證明 exact ID、exact client、exact path、`connected=true`、`agent_status=online`、單一且未截斷結果時，才視為 Server / Runner / Project 都仍 ready。

這個 fast path 是 fail-closed additive optimization，不是新的 authority source。URL 若不是 loopback HTTP、token file 是 symlink/空檔/過大、HTTP/MCP contract 失敗、Project identity 有任何一欄不一致、Runner offline，或 probe 與 publish 之間 saved identity 發生變動，Chadex 都不推測成功，而是回到原本完整 WebCodex Desktop orchestration。`chadex_apply_runtime_probe` 在 publish 前再次比較 Server URL、token-file path、Runner client ID、runtime Project ID 與 project path，避免 observation race。Project activation mutation、Runner tool execution、permission/protected-path policy、cancellation/deadline 與 process ownership 都沒有被 fast path 取代。

正式數據保存於 `docs/performance/phase6a-orchestration-control.json` 與 `phase6b-orchestration.json`。相同已存在 local runtime 條件下，fresh-helper cold `resumeService` median 從 **903.160 ms** 降至 **2.165 ms**（約 **-99.76%**），p95 從 **948.892 ms** 降至 **3.222 ms**；`refreshRuntime` median 從 **135.494 ms** 降至 **0.648 ms**（約 **-99.52%**），p95 從 **157.528 ms** 降至 **1.499 ms**。這是 Phase 6B 直接改到的 orchestration path，因此可視為因果改善。作為未修改 tool-execution control，Phase 6A 的 4-call workflow median **43.054 ms / p95 46.194 ms**，Phase 6B 為 **43.278 ms / p95 46.162 ms**；+0.224 ms median 視為量測漂移，沒有可辨識 regression。

## Phase 6C typed project activation

Phase 6C 將「已健康 local runtime 上切換到已經持久授權的 project」從 WebCodex Desktop 的多段 CLI orchestration 移到 Chadex-owned typed activation fast path。Phase 6B 的舊流程即使切回同一個 project，也會依序做 Runner observation、啟動 `webcodex project activate` 子程序，再用 `ops projects` 等待 project readiness；實測 same-project switch median 約 597 ms，p95 約 1.01 s。

新的 fast path 只在 **target canonical path 同時存在於 persisted config 與目前在線 Runner active policy 的 exact `allowed_root`** 時啟用。第一層只讀既有 private `runner.toml` 的最小 projection（`server_url`、`client_id`、`policy.allowed_roots`），拒絕 symlink / 非 regular file / 過大 config；第二層透過正式 MCP `call_runtime_tool → list_runners(client_id, include_projects=true)` 取得 Server 維護的 Runner semantic view，要求 exact client、`connected=true`、`status=online`，且 active `policy.allowed_roots` 也包含 exact target。兩層都使用既有 `webcodex_runner_config::paths` equality；父目錄 authority 不算 exact project authority。這避免了「磁碟 config 已變更但 Runner 尚未 reload」的 race，同時保留 explicit project root 必須持久化的既有語意。任何一層無法證明時，都回到原 CLI config-write → typed config-check → generation-CAS reload → reconcile 流程。

若 active Runner semantic view 已包含同一路徑的 enabled project registration，Chadex 直接從該 authoritative inventory 建立 runtime identity，不再發送任何 Runner mutation。只有 exact persisted + active root 均成立、但 project 尚未 registered 時，才呼叫既有 hidden `/api/projects/resolve-or-register`；該 endpoint 仍由 WebCodex ToolRuntime 執行 Runner capability、owner、project-write、active-job 與 routing-projection fences。成功回覆必須包含 exact runtime-project prefix、exact agent project ID、exact client ID 與 exact path。若 transport 或 mutation outcome 可能不確定，Chadex 不重送 mutation，而是先以 project inventory reconcile exact project；無法確認時回傳 reconcile-required。404/405、mixed-version contract、policy/authority gate 不符合等非不確定失敗則退回原 Desktop/CLI activation。

fast path 仍執行在原本 `DesktopOperationKind::LocalProjectActivate` admission/cancellation/process-baseline 範圍內；publish 前再次比對 Server URL、token-file path、runner-config path 與 Runner client identity，再共用原本 `commit_local_project_activation` 的 config rollback、activity、snapshot/readiness 更新。Tunnel switch 邏輯、verification epoch reset、Runner tool execution、protected-path policy、process ownership 與 Swift wire format均未改動。

正式結果保存於 `docs/performance/phase6b-project-switch-control.json` 與 `phase6c-project-switch.json`。相同已存在且已 exact-authorized 的 selected project 條件下，最終 hardened 100-run same-project switch median 從 **597.116 ms** 降至 **17.382 ms**（約 **-97.1%**），p95 從 **1010.004 ms** 降至 **24.191 ms**（約 **-97.6%**）。作為未修改 tool-execution control，Phase 6B 4-call workflow 為 **43.278 ms median / 46.162 ms p95**；Phase 6C 保存的 control 為 **42.489 ms / 46.426 ms**，沒有可辨識 regression。

## Phase 6D fresh local runtime readiness

Phase 6D 先以獨立 `CHADEX_DATA_DIR`、temporary projects、real packaged helper / Server / Runner 做 measurement-first cold-runtime benchmark，完全不碰正在使用中的 Chadex runtime。Phase 6C baseline 的首次 local runtime 建立 median 為 **550.998 ms**；activity timestamps 將其中穩定拆為 `operation → setup` **66.5 ms**、`setup → Server spawn` **71.0 ms**、`Server spawn → Runner spawn` **183.0 ms**、`Runner spawn → ready` **214.5 ms**。因此本階段不先優化約 160 ms 的首次新-project authority extension，也不碰已經約 20 ms 的 exact-authorized switch，而是只處理最大的 Runner-start readiness 固定成本。

Chadex backend 現在可透過 `configure_local_setup_with_chadex_fast_path` 在原本 `DesktopOperationKind::LocalSetup` admission / cancellation / process-baseline 範圍內提供一個 **read-only fresh-runtime observer**。正常 WebCodex `configure_local_setup` 仍使用 no-op callback，因此 upstream/default behavior 不變。當 local Runner 已由原流程安全 spawn 後，Chadex 在 **150 ms total budget** 內、以最多每 **10 ms** 一次的 cadence，透過既有 bounded/no-proxy HTTP client 重試 authenticated `call_runtime_tool → list_runners(client_id, include_projects=true)` semantic observation；每次 HTTP call 另以剩餘總 budget 作 outer timeout。只有 exact client online、exact root、exact enabled project 與 `agent:{client_id}:{project_id}` identity 全部吻合才接受 observation。Runner 尚未進入 semantic view、暫時性 non-ready response 或短暫 transport miss 只在這個 bounded window 內重試；budget 用完就不猜測成功，而是落回原本完整 readiness flow。

成功 observation 只替代舊的三段 **readiness/confirmation** 開銷：`runner status` CLI → `project activate` CLI → `ops projects` readiness CLI。它不寫 Runner config、不新增 allowed root、不自行註冊 project，也不改 project mutation authority。若 route/contract 不支援、token/transport 無法安全讀取、Runner 尚未 ready 或 identity 不一致，Chadex 只在上述 bounded observer 內嘗試確認，無法證明 ready 就落回原本完整 CLI readiness/activation flow；cancellation 則立即中止。Project authority extension、generation-CAS reload、protected paths、process ownership、Tunnel verification epoch 與 Swift wire behavior均未被此 fast path 取代。

正式 control 保存於 `docs/performance/phase6c-cold-runtime-control.json`，Phase 6D 結果保存於 `phase6d-cold-runtime.json`。Phase 6C control 為 6-run isolated sample；正式 Phase 6D 為 12-run isolated sample。first-runtime median 從 **550.998 ms** 降至 **408.308 ms**（約 **-25.9%**），Phase 6D p95 為 **425.383 ms**；最直接受本階段影響的 `Runner spawn → ready` median 從 **214.5 ms** 降至 **86.0 ms**（約 **-59.9%**），而 `operation → setup`、`setup → Server spawn`、`Server → Runner spawn` 三段 median 約 **67.5 / 72.0 / 183.5 ms**，支持改善來自 typed readiness 而非 Server/Runner spawn 變快。未修改的 4-call tool workflow control 為 **40.806 ms median / 46.887 ms p95**；same-project switch 為 **23.242 / 28.060 ms**，均未顯示可辨識 regression。Phase 6D 量到的 new-project extension median 變化不歸因於本階段，因為該 mutation path 沒有被 6D 修改。

## Phase 6H Chadex-owned production runtime entrypoints

Phase 6H 先切斷 production process/bundle 對 upstream executable identity 的直接依賴，而不是一次重寫整個已被 Phase 9–12 強化過的 MCP/Runner tool runtime。新的 `chadex-runtime/` package 產生 `chadex-runtime-cli`、`chadex-runtime-server`、`chadex-runtime-runner` 三個 Chadex-owned entrypoints；`RuntimeToolchain`、production bundle 與 local process spawn 都只使用這三個名稱。`Chadex.app` 不再包含或直接啟動 `webcodex`、`webcodex-server`、`webcodex-runner` executables。

為保持 Phase 11 worktree isolation、Phase 12 execution semantics、authorization 與 tool contracts 不變，這三個 entrypoint 在本階段仍連結固定 revision 的 WebCodex libraries。Runner 原本的大型 executable core 被抽成可重用 library entrypoint，原 upstream binary 保留成 thin compatibility wrapper；Chadex wrapper 以同一固定 build metadata 輸出可驗證的 version/commit identity。開發環境新增 `CHADEX_RUNTIME_BIN_DIR`，舊 `WEBCODEX_DESKTOP_BIN_DIR` 僅保留 legacy fallback；production bundle 固定走 `Contents/Resources/chadex-runtime/`，因此 legacy environment 不會覆蓋正式 app runtime。

這一層的目的不是宣稱 backend 已完全 Chadex-native，而是建立下一步 replacement seam：production app、build/package layout、runtime resolver 與 process ownership 已經屬於 Chadex；WebCodex 現在退到 entrypoint 後方的 temporary library implementation。`UPSTREAM.md` 與 Apache-2.0 license 繼續隨 app bundle 保留。驗證包含 Chadex runtime 三 binary build/version smoke、`chadex-helper` **123 passed / 0 failed / 3 ignored**、Swift **25 passed / 0 failed**、Runner library **855 passed / 0 failed / 4 ignored**，以及 production `.app` bundle audit：只存在三個 `chadex-runtime-*` executables，舊三個 upstream executable 名稱為 0 筆。

## Phase 6I production dependency independence

Phase 6I 將 Phase 9–12 已驗證過的 MCP/file/Runner implementation 從 `vendor/webcodex` 的 build dependency 升格為 Chadex-owned `runtime-engine/`。Production crate/package identity 改為 `chadex-runtime-*`，`chadex-runtime/Cargo.toml`、`rust-helper/Cargo.toml`、runtime resolver、build target 與 package script 都只指向 `runtime-engine/`；build metadata 也由 `CHADEX_RUNTIME_GIT_*` 反映 Chadex repository identity。`vendor/webcodex` 不再出現在 production Cargo dependency graph、active build path 或 app executable path。

這不是把 upstream provenance 隱藏或宣稱原始碼從零重寫。`runtime-engine/` 明確是由固定 WebCodex Apache-2.0 baseline 演進而來的 production derivative；`runtime-engine/LICENSE`、`attribution/WebCodex-LICENSE.txt`、`UPSTREAM.md` 與原始 `vendor/webcodex` snapshot 都保留。差別在於原始 vendor tree 現在只負責 provenance、歷史比較與 attribution，不再決定 Chadex 能否 compile、test 或 package。

Phase 6I 的 hard acceptance 不是單純搜尋字串：實際將 `vendor/webcodex` 暫時移出 repository 後，`chadex-runtime` 與 `chadex-helper` 的 `--locked --offline` check 成功，production `./scripts/build_app.sh` 亦完整成功，包含 runtime binaries、Rust helper、Swift release build、plist validation、Apple Development signing 與 final codesign verification。Bundle audit 只包含 `chadex-runtime-cli`、`chadex-runtime-server`、`chadex-runtime-runner` 三個 runtime executable，沒有 `webcodex`、`webcodex-server`、`webcodex-runner`；bundled `WebCodex-LICENSE.txt` 與 `attribution/WebCodex-LICENSE.txt` SHA-1 完全一致。驗證後原始 vendor snapshot 已原樣放回。

行為回歸則直接在 promoted source 上驗證：Phase 12 execution semantics **65 passed / 0 failed**；tool contracts **139 passed / 0 failed**；tool runtime contracts **94 passed / 0 failed**；Runner **855 passed / 0 failed / 4 ignored**；`chadex-helper` **123 passed / 0 failed / 3 ignored**；Swift `ProtocolModelTests` **13 passed / 0 failed**。Runner 初次搬移時的 76 個失敗全部追溯到 test fixture 的舊相對路徑（LSP fake server、process argv/tree helper），修正為 `runtime-engine` 路徑後全數通過，沒有放寬或刪除測試。

Cold dogfood optimized build 曾因 shell execution 的 300 秒 budget 被截斷，但使用長任務 process execution 後同一個 `build_app.sh` 正常完成；這是 build orchestration/time-budget 問題，不是 runtime dependency failure，也沒有為了通過驗收降低最佳化設定。

## Phase 11 concurrent isolated execution

`ExecutionWorkspace` 是 task executor 的唯一執行根抽象，明確區分 `none`、`task`、`package` 與 `integration` 四種 isolation mode。Search/read-only task 留在既有 Project；需要 mutation 或可能產生副作用的 task 先由 Runner 建立 detached managed worktree，再把 nested read/search/edit/process/validation/review tool call 綁到該 worktree 的 runtime Project。Runner 的 managed-worktree lifecycle 集中處理 Git worktree、registry、lineage、base commit 與 recovery metadata，不為一般 task 建立永久 branch。

Task admission 以 process-local bounded scheduler 控制，預設最多 2 個 task、最多 3 個 nested executor；限制可由 `CHADEX_MAX_CONCURRENT_TASKS` 與 `CHADEX_GLOBAL_EXECUTOR_CONCURRENCY` 調整，但上限維持 3。Pre-plan complexity、safe validation boundaries、package file/symbol overlap 與可用的 fresh Graphify dependency evidence 先判斷 package 是否真的獨立；之後 adaptive cost gate 再比較 estimated sequential cost 與 parallel critical path + isolation overhead，只有預估 wall-clock gain 至少 15% 才啟用 package worktrees。Cold-start overhead 使用保守 prior，成功 parallel task 會把實際 worktree/commit/integration/final-validation/apply/cleanup overhead 寫入 private `performance/adaptive-execution.json`，以 EWMA（25% 新樣本權重）逐步校正每台 Mac；telemetry 不保存檔名或程式內容。Graphify stale/missing 或收益不足時都保守退回 task-level sequential isolation。

平行 package 各自從同一個 snapshot 建立 worktree；clean repo 先用 cheap Git clean check，tree/index 都等於 HEAD 時直接以 HEAD 建 worktree，不再先重建 anonymous effective-tree；只有 staged/unstaged/non-ignored untracked 狀態存在時才走 temporary-index dirty snapshot。第一個 package workspace 建立後，其餘 derived package worktrees 以同一 base snapshot 併發準備；成功 apply 後，各 package worktree 也併發 cleanup，integration worktree 最後單獨清理以保留 recovery ordering。Package 結果先 commit，再在獨立 integration worktree cherry-pick。Package-local validation（明確只引用該 package path）不會在正常 integration 無條件重跑；global build/test/release/review 仍在整合後執行，reconciliation 則保留完整 safety checks。Apply-back 以 result commit 的 Git diff 建立 authoritative write-set；只有 source 變更與 task write-set 重疊才需要 reconciliation。完全不相關的 working-tree/index/HEAD 變更（包含 Graphify derived metadata refresh）不再造成 false block。真正重疊時才在另一個 reconciliation worktree執行 three-way apply 與 validation；若 reconciliation 成功，先 commit reconciled tree，再以 reconciliation snapshot 做 strict whole-source CAS 後自動 apply-back，只有衝突、validation 失敗或 reconciliation 後 source 再次變動才 preserved + blocked。Phase timing 分開記錄 package execution、workspace prepare wall、package validation/commit、integration prepare/merge、final validation、apply-back、package cleanup wall、integration cleanup，不再把平行 component sum 或 whole-task elapsed 誤標為 wall-clock execution/overhead。

Recovery manifest 使用 atomic rename、`0600` 權限與每個 execution Project 的穩定 identity；成功 workspace 才清理，失敗、衝突或 cleanup 不確定時保留 Runner registry 與 detached worktree。所有 nested mutation path 先經 lexical relative-path guard，再由 Runner 對 canonical root 做 authoritative boundary check；不依賴 process cwd。

Task-level concurrency 另提供 `execute_tasks` batch admission：單一 ChatGPT/connector request 可提交 2–4 個彼此獨立的 deterministic task plan，Runtime 只 resolve 一次 source Project，再讓各 task 經同一個中央 scheduler、各自 worktree、validation、write-set apply-back 與 recovery lifecycle。這避免外層 connector 將多個 `execute_task` tool call 序列化而吃掉 backend concurrency；`execute_tasks` 不取代單一大型 task 內的 package dependency planning，也不能繞過每個 nested mutation 的既有 permission/path/stale-write guard。

Phase 11 recovery hardening 另外持久化 deterministic task recovery plan，Server 啟動時掃描 private task state，把未達 authoritative terminal state 的 task 恢復為 `interrupted`、把 active workspace 標成 preserved，並把最新 recovered task 投影回 `latest.json` 供 macOS UI 顯示「已中斷 · 可恢復」。正式 `task_recovery` surface 提供 `list`、`retry`、`discard`：`retry` 以新 task id 重播原 deterministic plan，不沿用舊 session identity；`discard` 只依 recovery manifest 與 Git `worktree list --porcelain -z` 做 unregister/remove/prune，不使用 blind `rm -rf`。Recovery list 只觀察、不 prune；unknown/orphan managed worktree 只回報，不自動刪除。Terminal task state/log 採 TTL 與 total-size GC，cleaned workspace manifest 另有 TTL；active/preserved/interrupted task 的 recovery state 與 logs 不進自動 GC。Task state/recovery plan 改為 private atomic sibling-write + rename，避免 crash 留下半寫 JSON。Local state 另保存 private `source_path`（不進 model-facing tool projection），desktop helper 只在 recovery metadata 與目前選定 project path 一致時保留重啟後進度，避免把其他專案的 stale recovery 顯示到目前專案。`cfg(test)` 下 production `CHADEX_TASK_STATE_DIR` 完全停用；persistence 測試只對 explicit TempDir 執行，避免 test suite 污染真實 Application Support registry。

本階段 synthetic benchmark 位於 `benchmarks/phase11_concurrency_benchmark.py`，結果保存於 `benchmarks/phase11-concurrency.json`；真正 ChatGPT → Chadex connector → `execute_task` → Runner-managed worktree 的 post-build acceptance 另保存於 `benchmarks/phase11-task3-connected-runtime.json`。Synthetic scheduler/Git plumbing 只作結構性證據：兩個獨立 task 約 **1.98x**，刻意強制短 package parallel 只有 **0.47x**，因此 adaptive gate 不把「可平行」直接等同「值得平行」。Final connected-runtime acceptance 在同一台 Mac、同一 disposable repo 上完成兩輪 post-lifecycle 8s/12s 三-package workload，以下採兩輪中位數：8s sequential 為 **29.91s runtime / 31.82s connector wall**，adaptive parallel 為 **24.39s / 26.75s**，改善約 **18.5% / 15.9%**；12s sequential 為 **42.55s / 44.89s**，adaptive parallel 為 **28.05s / 30.03s**，改善約 **34.1% / 33.1%**。Package prepare/cleanup 併發化後，8s measured parallel overhead 中位數約 **15.81s**，相較先前約 **21.80s** 下降約 **27.5%**；12s cleanup 中位數約 **5.83s**，相較先前約 **7.89s** 下降約 **26.1%**，相同 12s parallel 相較 lifecycle 優化前 **30.70s / 34.35s** 再改善約 **8.6% runtime / 12.6% connector wall**。EWMA 最新 base overhead 約 **12.63s**，而 8s 與 12s case 都是在 `measured_ewma` + fresh Graphify dependency proof 下做決策；1200 tracked files 的 clean snapshot microbenchmark 仍顯示 legacy effective-tree median **393.70 ms** → **66.71 ms**（約 **83.1% latency reduction / 5.90x**）。Dirty-tree snapshot、source HEAD/status preservation 與 mid-task source-change detection 仍通過。Phase 11 最終啟動 acceptance 也完成：舊 ad-hoc Keychain item 會一次性遷移到穩定 Apple Development code requirement；migration 首次啟動觀察到 2 個系統 Keychain 授權視窗，之後同一 build 重啟為 **0 次 Keychain / 0 次 Documents**。AppModel 啟動期間只有一個真正的 `SecItemCopyMatching` 路徑並以 session cache 重用。Startup recovery 同時修正 terminal-state closure，`completed + workspace active` 不再被視為 recoverable，也不再保留 stale `latest.json` 或顯示 task progress card。Unified startup trace 顯示後續啟動的 helper start 約 2–5 ms、credential push 約 45–56 ms、project activation 約 38–52 ms，而 local service restore 約 4.06–4.28 s；直接 `server status` probe 僅約 21–62 ms，因此剩餘可見「準備中」主要是實際 local server/runner readiness，而非 Keychain、recovery UI 或固定 probe delay。

## Secret boundary

- Tunnel ID 是非機密，Swift 存於 Chadex preferences。
- API key 的持久化唯一來源是 macOS Keychain。Swift `AppModel` 不在 init 時預讀 Keychain；bootstrap 首次需要 credential 時只允許一次 `SecItemCopyMatching`，之後同一 app session 只使用 memory cache，settings 與 diagnostics 不會重複觸發 Keychain authentication UI。
- `scripts/build_app.sh` 優先使用可用的 Apple Development code-signing identity（也可由 `CHADEX_CODESIGN_IDENTITY` 指定），只有完全沒有有效 identity 才 fallback ad-hoc 並明確警告；避免每次 rebuild 因 ad-hoc CDHash 改變而被 Keychain 視為全新 app。Files & Folders 的 Documents 授權是獨立的 macOS TCC 權限，當使用者選定的 project 位於 `~/Documents` 時可能在首次正式簽章執行時出現一次。
- Swift 只在需要時透過 helper private stdin 將 API key 傳入 Rust；helper 立即將 JSON 欄位取出為 `Zeroizing<String>`。
- Chadex runtime engine 永遠拿不到 Chadex Tunnel ID / API key。
- OpenAI API key 只被放入 Chadex 所擁有之 `tunnel-client` child 的 exact environment。
- runtime bootstrap token 只在 Rust 內讀取，用來建立 mode `0600` 的短生命週期 MCP Authorization file；不經 Swift/JSON protocol。

## Accessibility / UI

UI 使用系統字體、SF Symbols、原生 Split View / Form / Settings / Menu Bar controls，沒有自訂高風險動態效果。狀態均有文字與圖示，不只依靠顏色；一般文字輸入快捷鍵不被覆寫。


## ChatGPT turn economy 與 ingress 防護（2026-09-19）

工具指引優先重用確定的 Project identity、批次讀取既知相關 ranges，並把 runtime_status 留給診斷。`read_files.include_read_revision` 的 schema、欄位白名單及執行層保持一致；完整 inspection read 可省略 revision，partial/budget-truncated read 必須提供 revision。沒有新增通用 batch executor、檔案內容快取或 mutation 自動重試。

Ingress 每個 instance 最多保留 32 個 in-flight requests（包含回應 stream）；滿額立即 429。單次 body 上限 16 MiB、讀取期限 10 秒，分別回覆 413／408。permit 在 EOF、錯誤或 consumer drop 時釋放；不為長時間 SSE 設定整段固定 timeout。Ingress 不追蹤 backend redirects，並移除 Connection 指定的 hop-by-hop headers，正常 Authorization 仍透明轉交 backend 認證。

驗證 observer 不修改或等待完整回應再轉送。JSON 在 EOF 檢查；SSE 在完整 data event 檢查，支援 LF/CRLF 與分段。每個 JSON buffer、SSE line/event buffer 上限各 256 KiB，超限停止觀察但繼續轉送；因此超大的單一回應可以成功但不提供驗證證據。最多保留 16 個 request IDs，每個 ID 最多 1 KiB；舊 epoch 的結果不能改變新專案狀態。這是 backend 接受呼叫的證據，不是對遠端客戶端產品身分的密碼學證明。

下載 tunnel-client 時在接收 chunk 前檢查累積 64 MiB 上限；解壓讀取同樣 bounded，原有固定 SHA-256 驗證保持。活動遮罩移除整個 Bearer token（包含混合大小寫與 Tab）及已知 credential prefixes，不只取代 header label。

本輪結果與重跑方式見 `SECURITY_AND_PERFORMANCE_REVIEW.md`。Local MCP、Codex connector、ChatGPT 網頁端到端時間分開呈現。

## Phase 12 — Execution Semantics Foundation

Phase 12 沿用 Phase 9–11 deterministic task executor、bounded scheduler、Runner
mutation guards、managed worktree 與既有 `task_recovery`；不新增 transaction manager、
checkpoint/resume、conversation persistence、subagents 或 recovery framework。
Graphify 仍只參與原本的可選規劃判斷，execution correctness 不依賴 graph。

權威狀態是 `execution_state`：

```text
queued → running → succeeded | failed | cancelled | interrupted | unknown
```

`execution_id` 使用既有 `task_id`，代表一次不可重新開啟的 execution。
現有 Swift UI 使用的 `status` 保留為相容進度投影：`completed` 對應 `succeeded`，
`blocked`／`failed_validation` 對應 `failed`；`cancelling` 只是 cancellation request，
canonical state 仍為 queued 或 running。原因與診斷放在 `failure`，不擴充 execution enum。
所有 terminal state 的後續 callback、cancel 與 guard drop 都不能覆寫該 execution。

- **Admission / replay fence**：在任何 worktree 或 nested mutation 前，於既有
  `CHADEX_TASK_STATE_DIR/executions/<execution_id>/` exclusive-create claim，並持久化
  queued receipt。相同 ID 的 concurrent／replayed request 一律拒絕，不重做 filesystem
  mutation；bounded UI history eviction、log GC 或 recovery discard 不刪除 claim。
  呼叫端需要重播防護時必須提供穩定 `task_id`；省略代表新的 submission，不能依
  goal、plan、transport request ID 或 session 猜測是否同一次執行。
- **Durability / publication**：`execution.rs` 的 receipt 與 task snapshot 共用單一
  transition lock。Running 與保守的 `effects_possible` fence 在副作用前寫入；成功時
  先將完整 bounded result 寫入 private sibling file、file sync、atomic rename 與
  parent-directory sync，才讓 observer 看到 `succeeded`。Raw JSONL 是選用診斷，
  `latest.json` 是 UI cache，兩者都不是成功證據。持久化失敗不公開成功，而保留
  replay fence 並回報 `unknown`／durability reason。未配置可用 storage 時 admission
  fail closed，不能降級成不可靠的成功回報。
- **Cancellation / timeout**：`cancel_task` 只記錄 cooperative request；已送出的工作
  仍須取得結果。Scheduler admission 後、worktree mutation 前再次檢查 cancellation
  與原始 deadline。已知尚未發生 mutation 的 timeout 可正常失敗；Runner timeout、
  disconnect、lost result 或仍須 Job observation 的 mutation/process outcome 分類為
  `unknown`。Sequential、parallel package、integration 與 reconciliation 共用同一
  nested-result classifier。任一 package 不確定時，最終 execution 不能成功或宣稱取消。
- **Interruption / restart**：future drop／panic 不代表 Runner 停止；可能已發生副作用
  時為 `unknown`，read-only execution 則為 `interrupted`。既有 startup recovery
  從 receipt 重建 UI projection，不 resume 工作、不改寫 terminal receipt。
  每個 claim 持有 OS ownership lock，另一 runtime 不會把仍有 live owner 的 execution
  判成 crashed。這是 execution receipt recovery，並非 Phase 13 continuity。
- **Retry**：沿用最多一次的既有 bounded read-only step retry，且 explicit unknown
  marker 永不自動重試。這些是同一 deterministic plan 內的 read attempts；整次 task
  的 `task_recovery retry` 永遠建立新 `execution_id`，在執行前持久化
  `previous_execution_id`。舊 terminal state 不變，retry 失敗也保留 lineage。
  `unknown` execution 不提供 retry action，明確呼叫 retry 亦拒絕。
- **Cleanup ownership**：只允許具 registration revision、不同 source/execution
  identity、absolute root 且與目前授權 Runner project root 相符的 workspace cleanup。
  仍由既有 unregister CAS 與 Git worktree removal 完成，不對無關 worktrees 做全域 prune。取消、失敗與不確定結果保留
  workspace；unregister/CAS 失敗後不再用 path-only fallback 刪除。Discard 的獨立 marker
  僅隱藏 recovery projection，保留不可重播的 claim 與 terminal receipt。
- **Fast path / concurrency**：單純 read/search 仍走原 Project、單 package、無 worktree
  或 Graphify probe。既有 2-task／3-executor bounded admission、package independence
  與 adaptive cost gate 不變；增加的是每 execution 的 durability 成本。

Focused coverage 在 `tool_runtime/tests/execution_semantics.rs` 與既有
`tests/chadex_task_executor.rs`；涵蓋 transition table、late terminal callbacks、cancel、
pre-mutation timeout、ambiguous mutation、concurrent/restarted replay、durability fault、
retry lineage、cleanup ownership、live-owner recovery fencing 與 small read fast path。
Phase 11 parallel/dirty-tree/reconciliation tests 仍是 regression gate。

Phase 13/14 延後事項：完整 checkpoint/resume、對話保存、unknown outcome 的人工確認／
reconciliation、rollback/conflict recovery，以及 receipt archive/retention policy。
Phase 12 保守地把無法確認的 workspace lifecycle failure 視為 unknown；不推測 mutation
沒有發生。Claim/receipt 目前保留，因此磁碟用量隨 execution 數成長；private state storage
需要由 host 保持可靠。效能比較另記錄於 `docs/performance/phase12-execution-semantics.md`，
僅代表本機測試 harness，不宣稱 ChatGPT Web／connector 端到端改善。
