# 本機測試資料與備份

2026-09-30 將原本位於 Chadex 同層的六個資料夾集中到以下位置，保留原始內容、Git 紀錄與未提交修改。

路徑皆以 Chadex repository 根目錄為準：

```text
benchmarks/local-fixtures/
├── Benchmark-Chadex/
├── Benchmark-WebCodex/
├── Chadex-E2E-Benchmark/
└── Chadex-Phase14-Connected-Benchmark/

local-backups/
├── Chadex_Backups/     # Phase 5D 舊備份
└── Chadex-backups/     # Phase 19 回復資料及 runtime/App 備份
```

兩個目錄均已加入 `.gitignore`，不納入主 repository 的版本控制。備份仍在同一顆磁碟上；需要獨立磁碟備份時，應另行複製到外部儲存裝置。

Benchmark 腳本的預設來源已改為 `benchmarks/local-fixtures/`；自訂 `--source`、`--root` 或 `CHADEX_BENCHMARK_SOURCE` 仍可使用。若外部設定或 App 的專案捷徑仍指向舊位置，請改選此處的新位置。

## Git 工作環境（2026-10-02）

- 專案根目錄為日常使用的 `main` 工作樹，已納入寵物活動停留修正 `d3b56a3`、`407006d`，並以本機正常 fast-forward 更新；正式 `v0.3.1` tag 仍為 `6f462e1`。
- 整理前的 80 個未提交檔案完整保留在 `wip/pre-main-cleanup-20261002` 與 `local-backups/Chadex-worktrees/wip-pre-main-20261002/`。這是舊基準上的 working tree，尚未整合或重新驗證的 runtime 修改不能當成新版 main 的功能。
- `local-backups/git-preservation/20261002-pre-main/manifest.json` 記錄原始 SHA、檔案 SHA-256/大小/權限/mtime、WIP 工作樹及安全 stash。`files/`、`tracked.patch`、`index.patch` 與 `branches-before.bundle` 保留可回復資料；WIP 工作樹加鎖以避免誤清理。
- `local-backups/Chadex-worktrees/v0.3.1/` 保留已安裝的 build 13 程式來源（detached `407006d`）、Swift 建置與 App 候選／舊版；它不是最新 main 的日常開發路徑。
- Windows 專用工作樹仍在專案同層的 `.webcodex-managed-worktrees/` 中，其 `windows/w1-core-readiness` 分支與未提交內容獨立保留。
- 完全合併的 `public/*`、`ci/release-pipeline-opt` 與已整合的寵物修正分支移除本機重複參照。舊開發／安全參照統一放在 `archive/*`；歷史 release tags、`release/v0.3.0`、`release/v0.3.1` 保留。
- 本機整理不等於 GitHub 發佈：沒有移動 release tags、改寫遠端歷史或重發 DMG。`/Applications/Chadex.app` 為 v0.3.1 本機 build 13，Swift UI 修正來源 `407006d`；封裝沿用的 runtime/helper binary identity 仍為 `bd841bb`。

不要直接刪除保留工作樹。需要繼續舊的未提交工作時，使用 WIP 路徑並先檢視 `git status`；需要把其中修改移入 main 時，以有界 diff 整合並另行驗證。Graphify 排除 `local-backups/` 與 `benchmarks/local-fixtures/`，以免把歷史資料當成目前程式碼。
