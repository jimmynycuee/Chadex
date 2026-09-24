# Chadex 體感速度與安全／功能檢查

日期：2026-09-19。來源基準：本機 `6ba9787` 加本輪 diff。未部署、未推送、未替換正在使用的 App。

## 已確認與修補

| 優先級 | 位置／觸發條件 | 問題與影響 | 修改與證據 |
|---|---|---|---|
| P1 | `rust-helper/src/chadex_core/activity.rs`：訊息含 `Authorization: Bearer <token>` | 舊遮罩只取代 header/scheme，仍留下 token；若此類訊息進入活動或診斷輸出，就會洩漏。未發現或讀取真實洩漏 token。 | 移除完整 token，支援大小寫、Tab、JSON，並遮罩 `sk-`。測試直接斷言 token 不存在；不再只檢查 `Bearer token` 子字串。 |
| P2 | `verification.rs`：未認證請求或 HTTP 200 的 RPC/tool error | 請求送出即標記 connected，HTTP 成功碼即 verified，會造成錯誤狀態；不等同已證明資料存取繞過。 | 以 bounded JSON/SSE observer 比對 request ID、成功內容及當前 epoch；401、RPC error、isError、structured failure、錯誤 ID、截斷與舊 epoch 不提供驗證。 |
| P2 | `verification.rs`：慢速／大量同時 body 或長時間回應 | 原有單次 body 上限不足以限制同時累積的工作。 | 32 個 admission permits，讀 body 10 秒 deadline；429／408／413。permit 包含 response lifetime，取消與 drop 可釋放；不截斷正常 SSE。 |
| P2 | `verification.rs`：backend redirect 或 Connection 指名 header | Proxy 原本允許自動改變請求目的地，且未移除 Connection 指定的 hop headers。 | 禁止 redirect；保留正常 auth 轉送，移除 hop headers。mock backend 回歸確認不跟隨 redirect。未宣稱已發現實際惡意 backend。 |
| P2 | `webcodex-tool-runtime-contracts/src/tool_call.rs`：使用 `include_read_revision` | 公開 schema 與執行層支援此欄位，白名單卻拒絕；照說明要求 guarded edit 會得到 `-32602 unknown field(s)`。 | 補上白名單，保留型別與未知欄位拒絕；實際隔離 MCP 工作流已取得 revision 並完成 guarded edit。 |
| P2 | `webcodex-tool-contracts/src/registry/output_schemas/files.rs`：partial read 缺 revision | schema 未要求 partial read 的 snapshot fence，與既有測試／runtime 契約不符。未證明 runtime 真正繞過 stale guard。 | has_more／budget_truncated 時要求 read_revision；完整讀取保留 sparse 輸出。原有 failing contract test 現已通過。 |
| P2 | `tunnel.rs`：過大下載回應 | 原本 `.bytes()` 完整分配後才檢查 64 MiB；限制無法阻止讀取期間的記憶體膨脹。 | 正式下載路徑改為 bounded chunks，涵蓋 Content-Length／chunked；解壓也使用 bounded reader。固定 archive/binary SHA-256 保留。 |

## 速度證據

### 既有真實 connector 的呼叫方式比較

在 Codex 已連接的 Chadex connector 上，交替執行 sequential／batch 各 5 次，使用相同檔案與範圍。這是 connector 工具等待時間，不是 ChatGPT 網頁完整任務時間，也不是部署新版本後的測試。

| 情境 | 工具次數 | sequential median | batch median |
|---|---:|---:|---:|
| 兩個檔案的範圍讀取 | 2 → 1 | 2,012 ms | 1,033 ms |
| 兩個預先確定的搜尋 | 2 → 1 | 1,936 ms | 1,049 ms |

原始資料：`performance/connector-batching.json`。此資料證明批次策略的收益，不證明模型一定會遵循新描述；也不包含模型思考或解釋結果的時間。沒有在連接中的使用者專案測試 mutation。

### 隔離、真實 backend／Runner 的本機比較

候選 helper、當前來源 dogfood backend；每個變體 30 次，1 次 warmup，交替順序。每輪使用相同小檔案，修改與驗證只在暫存專案。沒有 fake Tunnel，也沒有網路延遲模擬；直接呼叫 local backend，因此不含 ingress／Tunnel／模型。

| 情境 | 工具次數 | sequential median／p95 | batch median／p95 | 回應 bytes median |
|---|---:|---:|---:|---:|
| 搜尋後閱讀供解釋 | 4 → 2 | 23.799／68.481 ms | 12.946／26.025 ms | 1390 → 992 |
| 多檔閱讀 | 2 → 1 | 4.741／13.773 ms | 2.569／6.764 ms | 618 → 419 |
| 讀取、修改、驗證 | 6 → 3 | 28.588／70.729 ms | 19.616／51.734 ms | 2744 → 1986 |

原始資料：`performance/local-turn-economy.json`，含逐次耗時、request/response bytes、headers／stream 時間及 binary SHA-256。這是**相同候選版本的策略 A/B**，不是兩個版本的 backend 效能比較。測試期間有其他編譯／驗證負載，p95 僅供本輪比較；沒有宣稱所有專案都會得到相同改善。

冷啟動 setup 單次樣本約 3159 ms，與上述 warm workflow 分開，不作 cold median／p95 推論。既有 ingress 記憶體 trace 仍保留 ingress、backend headers、stream 分段時間；本輪沒有取得實際 ChatGPT 網頁的前後各五次完整任務或部署後 ingress 效能比較。

## 驗證範圍

- helper：115 passed、3 個既有 ignored；新增下載路徑接線後另重跑 Tunnel 7 passed，新增遮罩後另跑 activity 3 passed。ignored 為依賴特定 dogfood/Windows 環境的原有測試，未改標記。
- tool contracts：139 passed；runtime contracts：91 passed。
- Swift：20 passed。測試產出的追蹤圖片已核對生成來源並還原，不包含在本輪 diff。
- Runner canonical write-boundary：2 passed，涵蓋 symlink／protected path 與內部合法 alias。
- Server 跨專案 session：2 passed，確認 read/write 都在 execution/mutation 前拒絕。
- Server auth gate：7 passed，含 token class、錯誤 token surface、未認證與 query-token 邊界。
- 真實 isolated backend：stale-write、`../` traversal、symlink escape 均拒絕；寫入目標未被錯誤修改、外部 marker 未回傳。
- 程式檢查：loopback 目標、固定 SHA、private credential file 的 create_new／0600、exact-root activation、typed argv 與 process ownership 路徑。沒有對整個上游或所有第三方依賴作完整安全認證，合法授權 shell 也不被描述成 filesystem sandbox。

## 重跑與驗收界線

使用既有 Rust 環境設定後：

```sh
cargo test --locked --manifest-path rust-helper/Cargo.toml
cargo test --locked --manifest-path vendor/webcodex/Cargo.toml -p webcodex-tool-contracts -p webcodex-tool-runtime-contracts
cargo test --locked --manifest-path vendor/webcodex/Cargo.toml -p webcodex-runner structured_write_paths
cargo test --locked --manifest-path vendor/webcodex/Cargo.toml -p webcodex --lib cross_project_session
cargo test --locked --manifest-path vendor/webcodex/Cargo.toml -p webcodex --lib auth::tests::gate_
swift test
python3 scripts/benchmark_turn_economy.py --helper rust-helper/target/debug/chadex-helper --resources .build/chadex-benchmark-resources --iterations 30 --output /tmp/chadex-turn-economy.json
```

benchmark 的 `--resources` 須包含當前建置的 `webcodex-runtime/{webcodex,webcodex-server,webcodex-runner}`；正式重跑前先建置候選 binaries。工具不輸出 token、專案 ID 或檔案內容。Swift 的現有 visual test 會重建 `ui-review`，應在乾淨工作目錄執行或單獨選擇其他 tests。

真正 ChatGPT 網頁驗收仍待部署後於相同模型、固定三個情境前後各 5 次執行，記錄送出到最終答案時間與工具次數；mutation 情境用獨立測試專案。這輪不更動目前 App／Tunnel，因此不能宣稱這部分已完成。
