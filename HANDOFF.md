---
head_commit: 3256161b5f7a7e077523922107cd2956bb9f3244
branch: codex/v072-release-prep
phase: V072
updated: 2026-10-15
updated_by: codex
---

# Handoff · v0.7.2 release preparation

The working candidate is based on public `main` at `3256161b5f7a7e077523922107cd2956bb9f3244`, on `codex/v072-release-prep`. Implementation and release notes are local; this state is not yet published. The separate dirty primary checkout remains untouched.

User scope includes macOS and Windows v0.7.2 release, a Windows GitHub Release installer plus in-app updater, and Windows close-to-tray behavior. On Windows, the title-bar X and in-app close button hide to the tray and preserve the helper/Tunnel; only tray Quit stops the helper. User explicitly authorized push, merge, tag and publication on 2026-10-15 once required gates pass.

The Goal environment/state repair and duplicate-start fix are included. Required sandboxing and formal human gates remain intact. G1 Goal `wc_grun_2FeZZCpzxbwrKtSH` was already cancelled and is not resumed or modified by this work.

Updater signing is configured: the private key is stored as the GitHub Actions secret `TAURI_SIGNING_PRIVATE_KEY`; the matching public key is the repository variable `CHADEX_UPDATER_PUBLIC_KEY`. The local key files are outside the checkout with mode 600. Their contents are not recorded here.

Local validation: Node 22.12.0 Windows tests passed 84/84 and the production frontend build passed. Goal environment (13), checkpoint staging (3), Windows desktop (17), installer (22), release preparation (14), and updater manifest (10) tests passed. Goal reliability A–J and Docker Worker/D1 integration evidence are recorded in `docs/GOAL_RELIABILITY_FIX_REPORT.md`. The complete `scripts/release_check.sh` now passes with exit 0, including Swift 307 tests (1 skipped), Rust helper 364 + 3 integration tests (4 ignored), runtime-engine 2,840 passed / 3 ignored, release-critical crates, Computer Use acceptance, release tooling, a fresh optimized macOS app package, Hardened Runtime, and deep/strict code-signature verification. The first run failed after its test cache disappeared; the full rerun succeeded with 30 GiB free. Rustfmt is unavailable in the pinned local toolchain.

Remaining release gates: commit and push the candidate, pass full PR/main CI including Windows installed lifecycle and historical upgrade, then pass the tag-triggered public-history, clean source, Apple Silicon DMG, Windows signed updater and manifest jobs before publication. Graphify was regenerated and Obsidian sync/check passed (69 outputs, no stale generated notes). Live connected ChatGPT and Windows-host hardware/UI acceptance were not performed locally; hosted CI and the release workflow remain authoritative for Windows build/package gates. Do not densely poll GitHub workflows while waiting.

## Prior Goal-reliability handoff

`head_commit` identifies the latest validated implementation commit; a following documentation-only commit updates this pointer. Work remains local and unpushed.

## Active follow-up — Goal environment/state repair (2026-10-15)

The downloaded Goal issue report has been implemented and its automated A–J
coverage completed in this PG2 worktree. No push/merge/deploy/install and no change
to G1 financial policy, Sheets, schedules, website or formal Worker/D1 services.
Primary dirty release and separate Windows worktrees remain preserved.

Implemented: bound-Runner preflight before agent/checkpoint; bounded Python stdin
transport (including long native preflight paths); private Linux Docker acceptance
with exact identity/wait/owned cleanup and crash-recovery inventory; scoped inputs
and full dependency tree+lock pins; immutable acceptance revisions; trusted scoped
receipts and separate Ed25519 external authority; operator-only adoption/retry;
queued ChatGPT Resume reclaim; actual-activity idle policy; mutation barriers;
separate checkpoint/acceptance counters and native review/recovery controls.
Literal Git staging excludes derived/private inputs; Python acceptance does not
create/update checkpoint bytecode. Unknown outcomes still fail closed.

Final executed evidence: Store 213; Supervisor 23; Plan 32; contracts 142 + 96;
environment/transport/control 5; mutation gate 1; Swift 307 tests/1 skipped/0 failures;
scoped input tests 13; actual Git regressions 3; native sandbox 33/33 controls
and 23 unit tests. Native combined script exit 2 denotes Worker/D1 not validated
on that native backend; supported Docker standalone passed 30/30 controls,
14/14 child controls and true Worker/D1. Fresh reliability helper E2E passed 32/32;
standard Plan/Goal helper E2E passed all 10 assertions; one bounded actual Codex
Goal succeeded with exactly one agent dispatch and only its intended fixture file.
The full local release gate passed: 307 Swift tests/1 skipped, 364 helper unit
tests + 3 integrations/4 ignored, 2,840 runtime-engine tests/3 ignored, and the
release packaging/signature checks. No fixture cleanup blockers were reported.
Logs: ignored dist/goal-reliability-evidence.

Original G1 Goal is cancelled, not needs_attention; this repair did not cancel it.
Fresh read-only Git verification retained checkpoint
6d6e81125265145338f30d2058497b8ef9b47738 with clean tracked files. Do not revive the
cancelled ID. A new explicitly approved source/base/budget is needed; do not reset
or switch the formal original project. No new G1 Goal or owner evidence was created.
Other nonterminal parked runs may use verified adoption/revision/checkpoint retry
within their original budget. Real owner/finance/restore gates still block SUCCESS.

Remaining: human native UI/connected ChatGPT acceptance and hosted Windows/release
workflow validation. The optimized candidate `.app` was built and locally signed
by the release gate, but installed build35 is unchanged. PG2 remains
in_progress with no closed_commit. See docs/GOAL_RELIABILITY_FIX_REPORT.md for
A–J scopes, source anchors and limits. The separate macOS 28/Intel Node warning
migration is not part of this repair.

Subagents: Store and UI used Luna/max (execution metadata confirmed), completed;
sandbox used Sol/xhigh (confirmed), interrupted by workspace credits. Main completed
integration and verification without replacement delegation. Exact IDs and the
assigned/confirmed table are in the repair report and prior work records.

## Prior committed duplicate-start milestone — 2026-10-14

The user reported that clicking either Goal execution option can create a second effective run. Work starts from the published v0.7.1 tag in an isolated worktree. The primary dirty release checkout and the separate Windows loopback-fix worktree remain untouched.

Scope: immediately lock start options during dispatch, show the existing run after success, and prevent repeated or concurrent approval/start requests for the exact same revision/hash from creating another run. Preserve human approval, owner/Runner binding, failed-start recovery, and genuine new-revision starts. Do not cancel/delete the user's existing live runs or publish a new release as part of this fix.

Implemented: immediate UI reservation of both execution modes, persistent existing-run presentation, unknown-outcome reconciliation, and transactional exact-revision run deduplication. Consumed approval replay preserves run identity/mode; direct Goal active approvals are reused only while valid. Ordinary Plan reapproval remains superseding.

Validation: Swift full suite 279 executed / 1 skipped / 0 failed, focused start-state 12 passed; Store 194/194, Plan operator 31/31 and Goal 18/18 passed. Final isolated helper/operator E2E exit 0: the published build35 baseline created 3 runs, patched runtime retains 1 with created=false for same/cross-mode retries and unchanged approval IDs. Details: docs/GOAL_START_DEDUPLICATION_REPORT.md; ignored local logs/JSON: dist/goal-start-evidence.

Remaining: review, candidate packaging, native button-click smoke and connected human ChatGPT acceptance, then merge/release when authorized. PG2 is in_progress, not closed. Installed /Applications/Chadex.app remains v0.7.1 build35; existing live runs are untouched. The separate Intel Node/dependency migration for the macOS 28 warning is not applied.

## Historical handoff (v0.6.0 prep; preserved below)

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
| #18 | runtime CORS／Host 白名單、anonymous 強制關閉 | 0.6.0 | reviewer 審查兩輪後合併 |

## 建議合併順序

1. #3
2. #11
3. #13
4. #2
5. #4（改 base 到 main，重跑 CI）
6. #1
7. #5、#6、#7
8. #14、#15、#17、#18
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
