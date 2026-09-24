# Chadex performance audit — 2026-09-22

目標尚未達成：已實作並驗證本機優化與 Tunnel 存活期修正，但目前 connected workflow 並未穩定快過 WebCodex，也未低於歷史 7.867 s median。不能把本機改善當作端到端達標。

## 版本與量測邊界

- 原版 Chadex：`72e7fb0decd4e380fd1ecddbcb93b1312bb1031d`，原始 `dist/Chadex.app` 保留。
- 候選版：相同 base commit 加本次未提交修改；`dist/Chadex-Performance.app`。Runtime 正確回報 `git_dirty=true`，不宣稱 clean source alignment。四個 helper/runtime binary SHA256 記錄於 `benchmarks/performance-runtime-path.json`。
- 對照：WebCodex Desktop 0.4.1 / `f080c8f3ea70`。
- Fixture：使用者指定的 Benchmark-Chadex / Benchmark-WebCodex，HEAD `49f793fd66b054a38923c3f6e3ccd9c1aca6579f`。
- 固定五步：search → read → apply_text_edits → unittest → show_changes。順序相依，不平行、不以 batch 取代正式 workflow。
- 本次 connected 資料經 **Codex 的 connector tools** 取得，計時包括 tool-await 宿主、transport 與本機處理；排除模型思考、evaluator、reset。使用者原始數據來自 **ChatGPT**，宿主不同，不能宣稱重現原始環境。Before 與 after 也不同時段，因此每組均交替 AB/BA 搭配即時 WebCodex control。
- 每組五輪保留全部樣本；tail 報 sample maximum，五筆不足以估計穩定 p95。隔離本機測試為 warmup 後 20 輪 AB/BA，另報 p95。

## Connected 結果

| 資料組 | Chadex median / max | WebCodex median / max |
|---|---:|---:|
| 使用者原始 ChatGPT | 12.924 / 24.036 s | 7.867 / 8.388 s |
| 本次 Codex before | 12.835 / 14.927 s | 12.329 / 14.601 s |
| 本次 Codex after | 14.904 / 31.535 s | 12.963 / 17.311 s |

目前 Chadex after median 比同組 WebCodex 慢約 15%。整體結果沒有改善，不用刪除 outlier 或局部工具改善掩蓋。WebCodex control 也比歷史數字慢，不能將跨時段差異全部歸因於本次程式改動。

| 工具 median（ms） | Chadex before | Chadex after | WebCodex after |
|---|---:|---:|---:|
| search | 1175 | 1098 | 1139 |
| read | 1091 | 1011 | 991 |
| edit | 5076 | 4868 | 4743 |
| unittest | 4053 | 5570 | 4577 |
| review | 1297 | 1313 | 1344 |
| runtime_status，獨立五輪 control | 1097 | 1008 | 1056 |

原始 samples、各呼叫 epoch timestamp、正確性結果見 `benchmarks/performance-connected-{before,after}.json`；歷史資料另存 `performance-historical-connected.json`。每步 median 的總和不等於 workflow median。

## 實際 runtime path 與 bottleneck

路徑為 connector 宿主 → hosted relay → tunnel-client → Chadex 驗證 ingress → MCP server（auth、admission、tool routing）→ Runner → 檔案／Git／process → 回傳。Ingress 保留 project epoch fencing、request/body 限制與 connection evidence 驗證。Runner mutation 的 authorization、stale-write、durable receipt、execution continuity 與 worktree isolation 都未關閉。

### 1. Connected 的主要時間位於本機 ingress 區間之外

從本次實際 native diagnostics 匯出 bounded ingress traces，以固定順序與工具名稱對應 25 次 sequential workflow 呼叫：

| Chadex run | tool-await 總和 | 本機 ingress 總和 |
|---|---:|---:|
| 1 | 15011 ms | 865.05 ms |
| 2 | 12435 ms | 444.48 ms |
| 3 | 31535 ms | 401.06 ms |
| 4 | 12907 ms | 458.57 ms |
| 5 | 14904 ms | 459.92 ms |

本機 ingress 包含 request body、backend headers 等待與 response stream；各輪約 94–99% 時間在此區間之外。新的 `apply_text_edits` 14,669 ms spike 對應本機 **42.10 ms**。這足以否定「此次 spike 主要由本機 executor 執行耗時造成」，但不能定位為特定 approval、relay、上行或下行網路問題，也不能回推歷史 13,184 ms spike 的原因。

Evidence：`performance-connected-ingress-after.txt` 與 `performance-connected-attribution.json`。Native export timestamp 只有秒精度，對應依序列／名稱，不是假裝有跨層 request-ID tracing。Tunnel 的 `dispatcher forwarded command` log 發出時點尚未驗證，所以 `performance-connected-forwarding.json` 僅保留時間相關性，不拿它當 execution-start 邊界。

另以 30 次本機 control 比較 direct endpoint 與 ingress：median 2.449 / 3.166 ms；驗證 ingress 不是秒級額外延遲來源。

### 2. 本機重複建 schema

CPU sampling 顯示 `runtime_status` 為了 names/count 呼叫 `registered_tool_specs()`，每次產生 129 組完整 tool schemas。改從 canonical model-visible definitions 讀名稱，測試核對 names/count 與完整 registry 一致。

MCP stateless extension 的 membership lookup 同樣反覆產生 schema；改以 `OnceLock<HashSet<String>>` 保存 immutable compiled names。沒有 cache scopes、authorization、Runner state、tool results 或 admission 決策。

### 3. Git review 的重複程序

`show_changes` 原本重複啟動 Git 查 HEAD。現在一次取得 full/short identity，numstat 重用已取得 commit，正常路徑少三個 Git 程序。保留 subject 的 bounded producer、錯誤檢查與 unborn HEAD fallback；沒有降低 diff 完整性或開啟 external diff/textconv。

### 4. 已重現的 orphan tunnel 問題

初期 connector 持續 `-32603`，本機 Server/Runner 正常。發現舊 orphan tunnel（PPID 1）仍連向已不存在的 ingress；log 的 upstream failures 對應失敗請求。只終止該已確認 orphan 後 connector 立即恢復。這是可重現連線故障，與歷史 latency spike 的關係未證實。

新增獨立 helper tunnel supervisor，以 stdin pipe 作為 owner lease。helper 正常關閉、crash 或 SIGKILL 後，kernel 關閉 pipe，supervisor 利用既有 `ManagedChild` 終止並回收自己擁有的 tunnel tree。Supervisor 不是每次 request 的額外 hop。停止超時保留 Child/session，回報 `tunnel_stop_incomplete` 供重試，不假裝清理完成。

## 本機 before / after

隔離 Server/Runner + loopback MCP，同 fixture、交替 20 輪；非 connected benchmark。

| 指標（ms） | Before median / p95 / max | After median / p95 / max |
|---|---:|---:|
| 五步 workflow | 244.077 / 293.596 / 377.260 | 199.252 / 296.728 / 306.781 |
| runtime_status | 17.729 / 38.447 / 115.534 | 1.197 / 2.387 / 11.607 |
| show_changes | 158.990 / 214.508 / 294.553 | 115.184 / 157.402 / 223.665 |
| apply_text_edits | 13.925 / 16.561 / 16.572 | 13.910 / 21.384 / 23.408 |

Workflow median -18.4%、status -93.2%、review -27.6%。Workflow p95 未改善；edit tail 亦未改善，不宣稱全面 tail 優化。沒有證據支持為這次 connected 差距重寫 executor。

## Correctness、resilience 與 package 驗證

- 每次 connected run：4 個 unittest 通過、43 個獨立 discount business-rule checks 通過、兩邊 diff SHA256 完全相同：`69d104a260d4803da81676ce24ac8067533e622ab71625131b49c8d86682fe63`。此 evaluator 為本次可重現測試，不冒稱持有使用者原先 hidden evaluator。
- Reset 只限 fixture 的兩個已知檔案，必須符合 baseline HEAD、無 staged changes、精確 diff hash；不 reset 未知修改，不 clean 未追蹤檔案。最後兩個 fixture 均回到 clean baseline。
- Helper：130 tests passed，3 existing ignored；supervisor integration：3 passed，涵蓋 owner abrupt death、EOF descendant cleanup、child exit propagation。
- Runtime status：29 passed；MCP：237 passed，1 existing ignored；executor：72 passed；final review：81 passed。分組可能重疊，不把它們相加冒稱 unique tests。沒有新增 skip 或削弱既有 tests。
- 額外 turn-economy harness 的 stale-write、path traversal、symlink escape guard 全部通過；不是正式 connected workflow 的替代數字。
- 完整 `scripts/build_app.sh` 打包成功，plist 與 deep/strict codesign 驗證通過。候選 app 已實際啟動，Server/Runner + connector 成功；dirty alignment 仍如實回報。
- 未執行多日 soak，不能宣稱已證明所有 long-running resilience。既有 durable mutation/recovery 語意未修改，相關 executor/helper regression 已執行。

## 重跑方式

```sh
PYTHONDONTWRITEBYTECODE=1 python3 scripts/benchmark_runtime_path.py \
  --before dist/Chadex.app --after dist/Chadex-Performance.app \
  --iterations 20 --output benchmarks/performance-runtime-path.json

PYTHONDONTWRITEBYTECODE=1 python3 scripts/benchmark_turn_economy.py \
  --helper dist/Chadex-Performance.app/Contents/Helpers/chadex-helper \
  --resources dist/Chadex-Performance.app/Contents/Resources \
  --iterations 20 --output benchmarks/performance-local-turn-after.json
```

Connected harness 為 `scripts/benchmark_connected_workflow.js`，需在提供兩個 connector 的 tool host 執行 `connectedRun(provider)`；每輪外部執行 `scripts/benchmark_connected_fixture.py PROVIDER cycle` 驗證及還原。不得把 loopback 替代為 connected 結果。

## 下一步與架構取捨

1. 優先在原始 ChatGPT 宿主跑同一個五步 AB/BA harness，並取得跨 connector/relay/local 的 request-ID timestamps。此處只有 Codex connector 可量測，尚未取得原宿主同條件 after；7.867 s 目標仍未驗證。
2. 以 spike 的 native ingress timing 對照宿主 dispatch、relay 接收／回傳、client receipt，定位區間之外的秒級等待。現有 tunnel forwarding log 不足以單独定位，不能為了速度關掉 authorization 或 auto-review。
3. 生產使用可保留既有 batch read/search 與減少不必要 model round trips；但正式五步基準不能改成 batch 冒稱勝出。任何新的 workflow orchestration 都要保留依賴、授權、失敗與 recovery 邊界。
4. 本機 review 仍占約 115 ms，可再評估 Git 子程序啟動與重複掃描；需維持 bounded output、Git config、特殊 filename、unborn repository、並行修改一致性。預期數十毫秒空間不足以解釋 connected 多秒缺口，目前不進行高風險 libgit2／persistent executor 改寫。
5. Supervisor 後續需較長 reconnect/crash soak；本次 deterministic crash tests 已驗證 lease 機制，但沒有以縮短耐久性保證換速度。

本次交付是已測量的本機優化、orphan recovery 修正與可重現證據；「穩定快過 WebCodex」仍是未完成的 acceptance criterion。沒有 commit、push 或發布；並行出現的 `terminal-ab` 工作未納入本次修改。
