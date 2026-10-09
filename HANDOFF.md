---
head_commit: 88912d6
branch: chore/v0.6.0-prep
phase: V060
updated: 2026-10-09
updated_by: claude
---

# Handoff · v0.6.0 prep (draft)

`head_commit` 是版本號 commit；其上還有一個文件 commit（CHANGELOG、`docs/releases/0.6.0.md`、`PHASES.md`、本檔）。這個分支尚未 push、尚未開 PR。

## 完成項目

- 版本號全部改為 0.6.0：Rust crates 與 `Cargo.lock`、`apps/windows` 的 `package.json`／`package-lock.json`／`tauri.conf.json`、`scripts/build_app.sh`、`docs/RELEASE.md`、相關測試 fixture。
- W5 合成升級改為 0.5.0 → 0.6.0，CI 歷史升級改從 `v0.5.0` tag（`.github/workflows/ci.yml`、`scripts/build_windows_release.ps1`）。
- `CHANGELOG.md` 0.6.0 段落與 `docs/releases/0.6.0.md` 草稿（寫成「預計包含」）。
- `PHASES.md` 新增 D1、W-design、V060，`current_phase` 為 V060。

## PR 狀態（repo: jimmynycuee/Chadex，皆未合併）

| PR | 內容 | 版本 | 備註 |
|----|------|------|------|
| #1 | Windows 設計 v2 | 0.6.0 | 只有 CI 驗證，無實機測試 |
| #2 | macOS 設計 v2（D1）、修正 macOS 15 SDK 編譯 | 0.6.0 | Pending Validation：只有 AI 評審，無外部驗證 |
| #3 | W5 smoke 印出非預期例外；`process_inventory` 15→60 s | 0.6.0 | 先合 |
| #4 | Computer Use 權限卡片 | 0.6.0 | #2 合併後要改 base |
| #5 | 工具說明只在 key／pointer 前 `activate_window` | 0.6.0 | |
| #6 | computer use 每個動作耗時紀錄 | 0.6.0 | |
| #7 | runtime 測試 env 競爭根治 | 0.6.0 | |
| #8 | 游標疊加層（macOS） | 候選（原 0.6.1） | 需實測截圖排除 |
| #9 | Chromium 網頁 AX | 候選（原 0.6.1） | |

## 建議合併順序

1. #3 先合（解決 W5 CI 的 `process_inventory_failed` flake，後面的 CI 才穩）。
2. #2（macOS 設計 v2）合併後，#4 改 base 到 main 並重跑 CI。
3. #1（Windows 設計 v2）。
4. #5、#6、#7 依序（互相沒有已知依賴）。
5. #8、#9 視驗證結果決定是否納入 0.6.0，否則留 0.6.1。
6. 全部合併後，更新 `docs/releases/0.6.0.md`（拿掉 draft／planned、填 Validation）、README 的 latest release 與 docs 連結，再 rebase 本分支並開 PR。

## 發佈前必須完成的 gate

- `release_check.sh` exit 0（在乾淨的 release checkout 上）。
- 完整 CI 在 release commit 上全部 success（W5 historical upgrade 需要 `v0.5.0` tag 可用）。
- 使用者在 `dist/Chadex.app` 實機驗收（設計 v2、權限卡片）。
- 若納入 #8：實測游標疊加層不會出現在截圖中。
- macOS 設計 v2 仍是 Pending Validation，release notes 不可宣稱外部驗證或得獎水準；Windows 只有 CI，不可宣稱實機測試。

## 另開任務（沿用 0.5.0）

- tunnel token 縮權；啟動 `credential_push` 錯誤；`docs/windows/W5-release-readiness.md` 與 `docs/windows/W5_HANDOFF.md`、`docs/ARCHITECTURE.md` 仍描述 v0.4.1／v0.4.0 的 fixture 與歷史升級。
- `PHASES.md` 的 V050 仍是 `in_progress`、`closed_commit: null`（v0.5.0 已發布），需另行收尾。

## 環境須知

- Rust：`export PATH="$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH"`；`cargo test` 加 `< /dev/null`。
- 不要用 `git stash` / `reset` / `checkout -- .`。
- Node 25 跑 Windows vitest 要 `NODE_OPTIONS=--no-experimental-webstorage`。
