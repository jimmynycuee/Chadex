# Chadex v0.7.1 Plan／Goal 流程完整修正計畫

## 摘要

保留現有 v0.7.1 改動，修正三項問題：模型誤判後續工具不存在、連線狀態變化導致核准被撤銷，以及 502 缺少可診斷資訊與安全復原。

完成標準是：透過真實 ChatGPT＋tunnel 建立 Plan、在 Chadex 核准確切 revision、由 ChatGPT 接續實作並執行驗收；重連與 App 重啟不會無故撤銷核准，傳輸失敗不會造成工具重複執行。

## 1. 工具路由與版本相容性

- 保留目前 direct／gateway 分工，不把所有 Plan／Goal 工具加入初始工具列表。
- 統一兩份 Skill 的規則：未知契約先查 `tool_manifest`；依回傳路由使用直接工具或 `call_runtime_tool`。刪除「未直接列出就是快取過期」的說法。
- 在 `plan_save` 與等待結果加入既有格式的 `suggested_call`，攜帶確切 `plan_id` 與 gateway 呼叫參數；`waiting` 引導再次等待，終止狀態不得提供繼續實作指示。
- 明確區分 Plan 與 Goal：Plan 核准後由 ChatGPT 實作；`origin: goal` 使用 Goal 狀態／驗收工具，不呼叫 `plan_wait_approval`。
- 保留 v0.7.1 的專案預設與權限保護。Skill 匯出內容必須與隨 App 發布的 runtime schema 一致，包含 connector dependency；文件註明這份 Skill 配合 v0.7.1，避免套用到仍要求明確 `project` 的舊版本。
- 不新增舊 `codex-plan` 相容別名，因其內容與契約尚未確認。

## 2. 核准持久性與清理範圍

- 移除 App 啟動及 `chatGPTConnected` 變為 false 時自動執行 `clear_board` 的行為。這些事件只更新連線狀態並重新載入既有資料。
- Plan、revision、核准與唯讀鎖沿用 SQLite 持久化；重啟時重建 guard，不因 UI 或 tunnel 狀態清除資料。
- 保留確切 revision／hash、owner、期限、明確 Close／Revoke 與版本 supersession 檢查。過期或撤銷後不得回傳 `approved`。
- App 的清理操作限定目前專案。operator `clear_board` 增加可選的 `project`；App 必須傳入 helper 已解析的完整 project ID。省略僅保留為明確的全域 operator 操作，禁止生命週期自動呼叫。
- 不復活已撤銷的歷史核准，包括先前 G1。要重新執行，必須重新審核核准。
- 不更改 approval TTL、不放寬工具權限，也不把「讀到 approved」當作新的執行權限；實際工具仍通過既有授權檢查。

## 3. 傳輸錯誤與安全復原

- 使用現有未完成的 `BackendRecovery` 作為起點，完整接入 ingress、runtime bridge 與 tunnel 生命週期；未接入的程式不得算作修復成果。
- 區分連線被拒絕、連線建立逾時、headers 前中斷與 stream 中斷。保留原始 JSON-RPC request ID，以及既有的 request hash／trace correlation。
- helper 自身可辨識的失敗以 HTTP 200 的 MCP 工具錯誤回傳，沿用既有錯誤 envelope，附上 `error_kind`、`execution_state`、`dispatch_certainty`、`recovery_kind`，可重試時增加 `retry_after_secs`。upstream 已產生的業務錯誤維持原結果。
- 只有確認連線建立前發生 `ConnectionRefused` 才標記 `not_started` 並啟動復原；逾時或斷線若無法確認派送狀態，標記 `outcome_unknown`，不自動重播工具。
- 復原採 single-flight，走既有 runtime operation admission，只恢復原本已設定、允許 autostart 的同一專案。使用者 Stop、Disconnect、專案切換或 generation 改變時取消／拒絕舊復原，不能重新啟動使用者已停止的服務。
- 原請求立即回傳復原中資訊；背景復原每次最多 45 秒，沿用 3 秒 cooldown。同一失敗期間最多三次自動復原，成功的 backend 呼叫或使用者明確重新連線才重設計數。
- 復原後重新取得 backend target；URL／credential 改變時，透過既有連線流程在原請求釋放後重新綁定 tunnel，不沿用舊 target，也不重送原請求。
- Skill 僅對明確 `not_started`、可重試的觀察呼叫每 5 秒重試，總計最多 60 秒，保持同一 `plan_id`。寫入與結果不明的呼叫先對帳。
- 擴充現有診斷紀錄，保存錯誤分類、OS error code、耗時、request correlation、generation 與復原結果。僅保存安全 metadata；不保存 token、headers、工具 arguments 或程式內容。使用有界輪替檔，重啟後仍可查最近失敗。

## 4. 測試與驗收

先執行新增的最小相關測試，再跑受影響的 helper、runtime engine、store、tool contracts 與 Swift suites，使用 repository 的 Rust toolchain 設定。

必要案例：

- 初始工具列表沒有 `plan_wait_approval`，仍能經 manifest／gateway 完成完整 Plan 流程；匯出的 Skill ZIP 與 dependency 正確。
- Plan 等待中核准，回傳確切 revision；核准後 App 重啟、短暫斷線及重連仍有效。
- 新 revision、期限到期、明確撤銷及 owner 不符仍拒絕執行；專案 A 的清理不影響專案 B。
- 模擬 ConnectionRefused、headers 前斷線、stream 中斷與正常 45 秒等待，確認分類及執行狀態正確。
- 多個並行請求只觸發一次復原；Stop／Disconnect／切換專案可阻止復原；復原失敗有上限。
- 用執行計數器證明結果不明的寫入不被重播；驗證 backend URL／credential 更新與請求 drain 不會死鎖。

最後使用獨立候選 App、資料目錄與 Git fixture 做真實 ChatGPT＋tunnel 驗收：

1. 建立 Plan，在 Chadex App 人工核准 Revision 1。
2. ChatGPT 收到 `approved` 後實作小型修正，實際執行 `python3 -m unittest -v`。
3. 在等待及核准後分別重連／重啟，確認核准與唯讀狀態正確。
4. 中斷候選 runtime，驗證復原成功且實作只執行一次。
5. 另測 Goal 建立、App 啟動及狀態／驗收 gateway，確認 Plan 修正未破壞 Goal。

記錄 App／Server／Runner 版本、SHA、Skill 包 hash、plan ID、revision、核准與測試結果。缺少真實 tunnel 證據時，標記驗收未完成，不以本機模擬取代。

## 5. 工作保護與交付

- 以 `release/0.7.1` 現況為起點，實作前保存可恢復快照；保留既有未提交工作，不整批 reset／revert。
- 不擅自恢復已刪除的 `PHASES.md`／`HANDOFF.md`，不把無關變更納入提交。
- 預設不需要資料庫 migration；不改寫既有歷史核准。
- 原始 502 的確切原因仍未證實；交付時分開列出已修缺陷、注入測試結果與真實現場結果。
- 完成後交付 task-only diff、驗證紀錄、獨立 v0.7.1 候選 App 與新版 Skill 包。公開發版、部署及取代正式 App 另行處理。
