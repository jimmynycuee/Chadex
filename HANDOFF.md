---
head_commit: 4266c21
branch: main
phase: V050
updated: 2026-10-08T03:10+08:00
updated_by: claude
---

# Handoff · v0.5.0

## 完成項目

`integration/skill-parity` 一次合入 `main`（fast-forward），包含 AP5、LS1（含 speed fixes）、AP6a、AP6b、AP6c、LS2、V043，以及驗收期間的修正：

- **Skills 頁（macOS）**：獨立側欄頁；來源卡片（Agents／Claude Code／Codex 的「連接」「允許腳本」）在上，Skill 列表一行精簡、搜尋＋篩選（全部／已啟用／外部來源／已安裝）、無效套件收合；內容欄 960pt 置中（Skills、Agent 設定、專案頁共用 `ChadexPageColumn`）。
- **Skills 頁（Windows）**：同樣的精簡列表、搜尋篩選、來源在上、SKILL.md 預覽、遠端 runtime 說明、topbar 對齊。
- **Skill ZIP 匯入**：可選任何位置的 ZIP，複製到 `<project>/.chadex/skill-imports/`（只在 git checkout 中加 `.gitignore`），安裝後刪除；helper 自動放平「單一頂層資料夾」的 ZIP（忽略 `__MACOSX`、`.DS_Store`），以 runtime 的上限驗證；錯誤顯示具體原因。
- **已安裝 Skill**：列尾「啟用」開關＋「移除…」（確認框說明無法復原、同一本機 runtime 的所有專案一起失去）；helper `removeSkill` 先停用再逐版本刪除。
- **修正**：
  - Project Memory catalog 超過 runtime 上限（64 → `MAX_MEMORY_SEARCH_LIMIT` 50），之前每次都失敗。
  - 啟用重播：idempotency key 改為每次呼叫加 nonce（原本確定性 key 讓「啟用→停用→啟用」重播舊結果、實際沒啟用）。
  - 啟用／停用錯誤被 refresh 清掉。
  - 啟動預熱只用 `desktop-state.json` 的舊專案 resume，沒對齊選中專案 → 預熱後 activate；App 遇 `project_runtime_mismatch` 用 `realignLocalProject`（不碰 tunnel）自動對齊一次。
  - Skills 啟動載入：載入中的 refresh 被丟掉、頁面取消連帶取消載入、預熱後不重載、catalog 的 mismatch 不觸發 realign。
- **專案切換／移除**（`b9a1bce`、`34ceda0`、`d5f2042`）：
  - 側欄選取跟隨目前專案（原本加入專案後一直顯示「正在切換到…」）。
  - 切換結果一出就結束 spinner；helper 等切換鎖上限 10 s、啟用上限 25 s（逾時取消並留在原專案）；App 等 60 s，逾時後採用 helper 的實際專案；realign 中的切換改為等待而非丟棄。
  - 移除專案的確認原本是 RootView 上第二個 `.alert(item:)`，永遠不會跳出 → 改為 NSAlert。
  - 移除使用中專案、切到替代專案失敗：維持不刪除並顯示原因（使用者決定）。
  - 加入家目錄或更上層（`/`、`/Users`、volume 根；Windows 磁碟／share 根、`C:\Users\<name>`）時先警告，取消為預設（使用者決定）。
- **測試覆寫真實 preferences**（`4189b1e`）：`VisualReviewTests` 的 `ReviewFileManager` 只重導 Application Support URL，但 `ProjectStore` 用家目錄組路徑，所以每次 `swift test`（含 `release_check.sh`）都會把 `~/Library/Application Support/Chadex/preferences.json` 換成測試資料。已改用 `CHADEX_PREFERENCES_DIR` 並加斷言。使用者的設定已靠執行中 App 存檔還原，備份在 `handoffs/v050/preferences.restored-backup.json`。
- **效能基準**：`docs/performance/v050-launch-connect-baseline.md`（視窗可用約 186 ms；runtime 背景就緒約 3.6 s；connect 約 0.68 s）。
- **測試隔離修正**（`4e7e2e4`）：`model_surface` 的 `adaptive_tools_list_exposes_ranked_direct_tools_and_gateway` 與 `phase16b_ui_surface_stays_bounded_and_preserves_presentation_tools` 每次 request 讀 `WEBCODEX_MCP_COMPACT_SCHEMAS` 卻沒拿 `TEST_ENV_LOCK`；其他持鎖測試暫時設成 `false` 時會讀到 full projection（757,057 bytes）。改為持 `TestEnvGuard` 並移除該 env。只影響測試，產品只在啟動時寫這個 env。
- 版本 0.5.0、CHANGELOG、`docs/releases/0.5.0.md`；W5 合成升級改為 0.4.1 → 0.5.0，CI 歷史升級改從 `v0.4.1` tag。

## Gate（實際執行）

- `release_check.sh` @ `4e7e2e4`：通過（exit 0；Swift 167/167；Rust 3861 passed、0 failed；2026-10-08 01:55–02:15，跑前跑後 `preferences.json` mtime／hash 不變）。
  - 前一次 @ `d5f2042` 失敗：`model_surface` 兩個測試平行執行時偶發失敗，根因已修（見下）。
- CI：run `37655626569` @ `4e7e2e4` attempt 2 全部 success（attempt 1 的 W5 historical upgrade `default_uninstaller_self_copy` `process_inventory_failed`）。`d5f2042` 的 run `37563112100` 同樣 attempt 1 失敗於同一 stage、attempt 2 全綠。
- 使用者手動驗收 `dist/Chadex.app` @ `9f708ea`：通過（Skills 頁、ZIP 匯入、啟用／移除、腳本閘門、Project Memory、連接速度）。
- 使用者手動驗收 `dist/Chadex.app` @ `d5f2042`：通過（移除專案、切換專案不卡住、加入家目錄警告）。

## 使用者決定（不要重新討論）

- 外部來源 script 預設不可執行、逐一開啟；連結範圍是 Runner 層級。
- 桌面端用獨立本機 admin token（`chadex-desktop-admin`），Project Memory 也走它；pairing token scope 不變。
- Helper 的 MCP ingress 擋 tunnel 來的 `skill_install`、`skill_activate`、`skill_deactivate`、`skill_remove_revision`、`memory_scope_purge`，解析失敗一律拒絕。
- Remote topology 不支援匯入，UI 顯示說明。
- 「啟動時在背景準備本機服務」預設開啟。
- ZIP 匯入用做法 A（複製進專案）；helper 自動放平單層包裝資料夾。
- 已安裝 Skill 列尾：啟用開關＋移除（有確認）。
- 版本號用 v0.5.0（不是 v0.5.1）。

## 另開任務

- **tunnel token 縮權**：ChatGPT 透過 bootstrap `WEBCODEX_TOKEN` 仍有全部權限。
- **啟動 `credential_push` 錯誤**：診斷 `bootstrap phase=credential_push ... error`（`pushStoredCredentialIfAvailable`，自 0.1.0 RC1 未改），驗收時連線狀態 `unconfigured`。未查原因。
- **W5 historical upgrade 的 `default_uninstaller_self_copy` `process_inventory_failed`**（優先）：最近 3 次 CI 有 2 次失敗，每次都要整個 workflow 重跑（約 +1 小時）。查原因，或讓該 stage 在 job 內重試。
- **測試入口不讀 env**：`#[cfg(test)] handle_mcp_request`（`runtime-engine/src/mcp.rs`）改用固定的 compact 設定，需要 full 的測試走顯式參數（約 4–6 個測試），根本消除這類 env 競爭。
- **加速發版驗證**：CI 與本機 `release_check.sh` 可同時跑（同 commit、互不依賴）；本次 gate 實測約 20 分鐘。
- `docs/windows/W5-release-readiness.md` 仍描述 v0.4.1／v0.4.0 的 fixture 與歷史升級，待更新。

## 已知限制

- ChatGPT 經 bootstrap token 仍有全部權限；ingress 擋管理工具不是權限邊界（shell 工具沒有沙盒）。
- 隱藏的 `$` share（例如 `\\fs\home$`）不能當 Skill 來源；少數本機 UNC 別名還沒擋。
- Windows 只有 CI 驗證，還沒有實機測試。
- Skill 移除是整個本機 runtime 共用（所有專案一起失去）；多版本移除不是原子操作，中途失敗會留下停用＋部分版本，可再移除一次。
- 每次 Skill 變更都佔一筆 runtime replay 紀錄（上限 1024、保留 7 天），短時間大量變更會遇到 `skill_store_replay_capacity_exceeded`。
- `realignLocalProject` 在 tunnel Ready 時不暫停 ingress（與修正前相同）。

## 新增的已知 flaky

- ~~runtime-engine `model_surface` 的 `phase16b_ui_surface_stays_bounded…` 與 `adaptive_tools_list_exposes_ranked_direct_tools_and_gateway`~~：根因為 env 競爭，已在 `4e7e2e4` 修正（原本可用 3 個測試 `--test-threads=4` 40/40 重現，修正後 40/40 通過）。
- W5 installer candidate／historical upgrade 的 `default_uninstaller_self_copy` `process_inventory_failed`（既有）：要 rerun 整個 workflow；本次兩個 commit 的 historical upgrade 都在 attempt 1 失敗。

## 環境須知

- Rust：`export PATH="$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH"`；`cargo test` 加 `< /dev/null`。
- 不要用 `git stash` / `stash pop` / `reset` / `checkout -- .`（stash 在所有 worktree 間共用）。
- Node 25 跑 Windows vitest 要 `NODE_OPTIONS=--no-experimental-webstorage`。
- `build_app.sh` 偵測到目標 app 在執行時改輸出 `dist/Chadex-next.app`；`release_check.sh` 把 app 建在暫存目錄，驗收 app 要另外 `build_app.sh`。
