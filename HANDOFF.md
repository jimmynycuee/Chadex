---
head_commit: 88912d6
branch: chore/v0.6.0-prep
phase: V060
updated: 2026-10-09
updated_by: claude
---

# Handoff · v0.6.0 prep (draft)

`head_commit` 是版本號 commit；其上還有一個文件 commit（CHANGELOG、`docs/releases/0.6.0.md`、`PHASES.md`、本檔）。這個分支已 push，draft PR 是 #10。

## 完成項目

- 版本號全部改為 0.6.0：Rust crates 與 `Cargo.lock`、`apps/windows` 的 `package.json`／`package-lock.json`／`tauri.conf.json`、`scripts/build_app.sh`、`docs/RELEASE.md`、相關測試 fixture。
- W5 合成升級改為 0.5.0 → 0.6.0，CI 歷史升級改從 `v0.5.0` tag（`.github/workflows/ci.yml`、`scripts/build_windows_release.ps1`）。
- `CHANGELOG.md` 0.6.0 段落與 `docs/releases/0.6.0.md` 草稿（寫成「預計包含」）。
- `PHASES.md` 新增 D1、W-design、V060，`current_phase` 為 V060。

## PR 狀態（repo: jimmynycuee/Chadex，皆未合併）

使用者已決定 #8、#9 納入 0.6.0，0.6.0 在這一批做完後就發佈。

| PR | 內容 | 版本 | 備註 |
|----|------|------|------|
| #1 | Windows 設計 v2 | 0.6.0 | 只有 CI 驗證，無實機測試 |
| #2 | macOS 設計 v2（D1）、修正 macOS 15 SDK 編譯 | 0.6.0 | Pending Validation：只有 AI 評審，無外部驗證 |
| #3 | W5 smoke 印出非預期例外；`process_inventory` 15→60 s | 0.6.0 | 先合 |
| #4 | Computer Use 權限卡片 | 0.6.0 | base 目前是 `feature/d1-design-v2`，#2 合併後改到 main |
| #5 | 工具說明只在 key／pointer 前 `activate_window` | 0.6.0 | |
| #6 | computer use 每個動作耗時紀錄 | 0.6.0 | |
| #7 | runtime 測試 env 競爭根治 | 0.6.0 | |
| #8 | 游標疊加層（macOS） | 0.6.0 | 發佈前必須實測截圖排除（macOS 14/15/26） |
| #9 | Chromium 網頁 AX | 0.6.0 | L1 實測 Chrome／Brave 是否接受 `AXManualAccessibility`；rootless `find_elements` 對 sensitive 畫面改為拒絕 |
| #10 | Release prep（本分支，draft） | 0.6.0 | 最後合併；合併前對齊實際內容，必要時 rebase 解衝突 |
| #11 | Windows core gate：`job_manager` 測試競爭；本機 Windows shell open 期限 90 s | 0.6.0 | |
| #12 | W5 文件更新到 v0.5.0 baseline | 0.6.0 | 純文件 |
| #13 | CI push 只在 main 觸發 | 0.6.0 | |
| #14 | Tunnel ID 存檔前用 helper 規則驗證 | 0.6.0 | 修 `credential_push` 啟動失敗；需實機啟動確認 |
| #15 | 登入頁、密碼管理程式視為 sensitive | 0.6.0 | 與 #9 搭配，網頁登入頁會被擋 |
| #16 | tunnel token 縮權設計文件 | 不列入 | 只是設計，不列入使用者可見變更；討論用 |
| #17 | 停用本機 runtime 的 shared-key 登入 | 0.6.0 | 升級影響：手動建立的 shared-key `webcodex connect` profile 會失效 |
| 待開 | runtime CORS／Host 白名單、anonymous 強制關閉（分支 `fix/runtime-cors-host`） | 0.6.0（預計） | PR 尚未開；沒趕上就從文件移除 |

## 建議合併順序

1. #3
2. #11
3. #13
4. #2
5. #4（改 base 到 main，重跑 CI）
6. #1
7. #5、#6、#7
8. #14、#15、#17、CORS PR（PR 開出後）
9. #9
10. #8
11. #12
12. 最後 #10：合併前把 `docs/releases/0.6.0.md` 和 CHANGELOG 對齊實際合併內容（拿掉 draft／planned、填 Validation，CORS 項目沒合就刪），更新 README 的 latest release 與 docs 連結，必要時 rebase 解衝突。

## 發佈前必須完成的 gate

- `release_check.sh` exit 0（在乾淨的 release checkout 上）。
- 完整 CI 在 release commit 上全部 success（W5 historical upgrade 需要 `v0.5.0` tag 可用）。
- 使用者在 `dist/Chadex.app` 實機驗收（設計 v2、權限卡片）。
- #8 截圖排除實測（⌘⇧3/4/5、QuickTime、螢幕分享、`computer_observe`）。
- #9 L1：Chrome／Brave 是否接受 `AXManualAccessibility`。
- #14：啟動後診斷匯出顯示 `credential_push ok`。
- #17：升級後本機連線、runner online、專案啟用正常。
- Skills 頁和 Project Memory 正常。
- macOS 設計 v2 仍是 Pending Validation，release notes 不可宣稱外部驗證或得獎水準；Windows 只有 CI，不可宣稱實機測試。

## 另開任務（沿用 0.5.0）

- tunnel token 縮權（設計在 #16，討論中，不屬於 0.6.0）；`credential_push` 啟動錯誤已由 #14 處理，仍待實機確認；`docs/windows/W5-release-readiness.md` 與 `docs/windows/W5_HANDOFF.md`、`docs/ARCHITECTURE.md` 仍描述 v0.4.1／v0.4.0 的 fixture 與歷史升級（由 #12 處理）。
- `PHASES.md` 的 V050 仍是 `in_progress`、`closed_commit: null`（v0.5.0 已發布），需另行收尾。

## 環境須知

- Rust：`export PATH="$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH"`；`cargo test` 加 `< /dev/null`。
- 不要用 `git stash` / `reset` / `checkout -- .`。
- Node 25 跑 Windows vitest 要 `NODE_OPTIONS=--no-experimental-webstorage`。
