# v0.7.1 Plan／Goal 修正與候選驗收報告

## 修正內容

- 保留 compact direct／gateway 分工；Plan 保存與等待回覆包含精確 plan ID 和限時建議呼叫，MCP 投影保留 gateway 參數。Goal-origin 使用 Goal 狀態工具；已核准或終止的結果不再產生實作或續等建議。
- 啟動及 ChatGPT 斷線時改為重讀資料，不自動清除 Plan。SQLite 核准與 guard 持續檢查期限、撤銷、新 revision、owner 和 binding；App 清理只作用於目前專案。
- ingress 只在可證實請求尚未派送時標記 not_started。連線逾時、headers 或 stream 中斷會標記 outcome_unknown；HTTP 200 MCP error 保留 RPC IDs，未確定派送結果的請求不重播。
- recovery 整合 admission、同專案 autostart、generation、drain 和 target／credential 更新；採限時 single-flight 與 cooldown。Stop、Disconnect 或切換專案會阻止舊 recovery。取消競態不會意外重新啟動 tunnel，也不會由無效取消消耗 generation。
- 新增有界、重啟後可讀的 transport metadata journal 與 helper diagnostics；不保存 token、headers、arguments、原始錯誤或程式內容。
- Skill 改為 manifest-first；觀察呼叫只有在明確 not_started、retry recovery kind 與 retry_after_secs 齊全時才可限時重試。候選 App 使用獨立 Keychain namespace。

## 候選驗證證據

| 範圍 | 結果 |
| --- | --- |
| runtime engine | 2,804 passed、3 ignored；binary 1 passed |
| runtime store | 189 passed |
| tool contracts | 142 passed |
| tool-runtime contracts | 96 passed |
| helper | 362 passed、4 ignored；integration 3 passed |
| Swift | 267 executed、1 skipped、0 failures |
| ingress 分類／不重播／等待 | 31 passed |
| 取消邊界回歸 | 15 passed |
| 真實 ChatGPT Plan | 真人核准確切版本；重連與 App 重啟後核准仍有效；隔離 fixture 修改後，記錄的 unittest exit code 為 0 |
| 真實 server-only recovery | 一次 recovery ready，約 8.3 秒；Runner process identity 保留 |
| 真人啟動的 ChatGPT Goal | 第 1 輪達成；隔離 worktree 的驗收命令通過，主 fixture 保持不變 |
| Skill 交付 | 原生 ZIP 結構與檔案內容核對完成；雲端回讀的 Skill 內容與來源一致 |

候選 App build 20261012.3 已在隔離資料與 Keychain namespace 中啟動。以上 ChatGPT／tunnel 證據涵蓋已記錄的隔離案例，不代表所有 provider、網路故障或長時間 Goal 都已驗證。本機合成流程與真人 ChatGPT 驗收是不同證據來源。

## 已知限制

原始 502 事故的現場根因仍未證實。此報告記錄的是候選驗收；正式版本另由乾淨來源 release gate 和 tag workflow 驗證。免費 macOS 產物採 ad-hoc signing、未 notarize；Windows 僅為 unsigned CI 驗證候選，不公開發行。
