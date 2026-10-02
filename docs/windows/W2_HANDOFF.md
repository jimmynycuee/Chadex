# W2 進度與交接

狀態：**W2 已完成並通過 native Windows core + runtime E2E 驗收**。

W2 完成後依使用者要求停止；不得自動開始 W3。

## 分支與驗證對象

- 開發分支：`windows/w2-runtime-e2e`。
- 基底：W1 `0e1b3172a08b81cc7b2b3deb7ffc25f47862db01`，包含 macOS `v0.3.2` ancestry。
- Runtime 實際驗證 source：`cbe258cefedaa952cb720bf2594c8b5e8ec6e2d4`。
- 最終驗收：[GitHub Actions run 36987012302](https://github.com/jimmynycuee/Chadex/actions/runs/36987012302)，Windows job `110774230581`。
- 未 merge `main`，未建立 W3 分支，未發布 Windows Desktop 或 stable release。

## 已完成的修正

1. PowerShell gate 明確檢查每個 native command 的 exit code，修正舊 CI 假綠。
2. Runner Windows process-tree marker 的 cfg 與呼叫者一致，恢復完整 library 編譯。
3. Git fixture 明確使用 LF，另新增 CRLF snapshot 保留測試；production 保持原 repo 設定。
4. Timeout 訊息保留既有前綴及完整 adaptive 診斷；沒有更動 timeout 行為或預算。
5. Helper 預設 Windows data path 使用 `%LOCALAPPDATA%\Chadex\runtime`，缺失時 fail closed；保留顯式 override 與 macOS 預設。
6. Tunnel cache 加強 private DACL、reparse ancestor、file identity、唯一暫存檔與精確 version 檢查；新增官方 asset download/hash/reuse/replacement 及 supervisor cleanup native tests。
7. 建立使用實際 helper/server/runner 與 MCP tools 的 Windows E2E harness，產出 bounded JSON 證據。

## 驗證結果

最終 Windows 驗收已全綠：Source release、ARM64 package、public-history / secret scan、Windows core 與 Windows runtime E2E 全部通過。Windows core 的 runner library 為 784 passed / 0 failed / 2 ignored；helper 為 150 passed / 0 failed / 6 ignored；Windows tunnel supervisor 為 3 passed / 0 failed；官方 pinned Windows tunnel asset 的 download/hash/install/reuse/replacement/version 測試亦通過。

最終 Windows host 為 Microsoft Windows Server 2025 Datacenter `10.0.26100`，GitHub runner `2.337.0`，image `windows-2025-vs2026` version `20260925.250.1`。

Runtime E2E 共 13 stages 全 passed：55.093 秒 durable job 僅啟動一次且只有一個 Job，確實經歷 observation timeout，runtime identity 未變；cancellation 觀察到 2 個 payload processes 且最後 0 殘留；helper shutdown 觀察 7 個 owned processes、`forced_cleanup_count=0`、最後 0 殘留。GitHub run `36987012302` 保留 canonical raw artifact；repo 內保存 `docs/windows/evidence/W2_windows_runtime_e2e_36987012302.sanitized.json` 與同目錄摘要，僅移除 31 個會觸發 Gitleaks 的 `token_sha256` digest 欄位。

本機 macOS 已執行：

| 驗證 | 實際結果 |
| --- | --- |
| Swift | 70 passed / 0 failed |
| Helper | 159 unit + 3 integration passed；3 原有 opt-in |
| Runner library | 863 passed / 0 failed / 5 原有 opt-in，`--test-threads=4` |
| Process lifecycle opt-in | 15 passed，含 20 次 stress cycles |
| Runner config | 24 passed |
| Harness deterministic tests | 16 passed；不是 native E2E |
| Persistent-shell defaults | 7 passed / 4 failed / 6 原有 opt-in；與 W1 相同的 `/tmp`、`/var` canonical-path 比對限制 |

Runner 初次全併行回歸為 862 passed / 1 CPU-progress timing failure；同一測試在 narrow filter 與完整四執行緒回歸通過。未刪除、跳過或弱化有效測試。Persistent-shell 另以 `TMPDIR=/private/tmp` 重跑為 8 passed / 3 failed，三項明寫 `/tmp` 的 assertion 仍失敗；詳見 [W2 驗證紀錄](W2_RUNTIME_E2E.md)。

## 實際修改檔案（相對 W1）

```text
.github/workflows/ci.yml
docs/windows/W2_RUNTIME_E2E.md
docs/windows/W2_HANDOFF.md
runtime-engine/crates/chadex-runtime-runner/src/main_tests.rs
runtime-engine/crates/chadex-runtime-runner/src/webcodex_runner/job_manager_tests.rs
runtime-engine/crates/chadex-runtime-runner/src/webcodex_runner/shell.rs
rust-helper/src/chadex_core/tunnel.rs
rust-helper/src/chadex_core/tunnel/install_tests.rs
rust-helper/src/chadex_core/tunnel/windows_support.rs
rust-helper/src/chadex_core/tunnel/windows_tests.rs
rust-helper/src/runtime_bridge.rs
rust-helper/tests/windows_tunnel_supervisor.rs
scripts/test_windows_runtime_e2e.py
scripts/windows_core_check.ps1
scripts/windows_runtime_e2e.py
```

## 子代理紀錄

所有呼叫已接受；工具未提供可核對的實際 model / reasoning 中繼資料。以下是 spawn 指定值，不代表實際執行值已確認。代理均已停止寫入並關閉。

| 工作／代理 ID | 指定 model / reasoning | 已確認執行值 | 結果 |
| --- | --- | --- | --- |
| Harness / Erdos `01a0fac3-42e8-7093-b0da-fb7f70c6d5de` | `gpt-6.1-sol / xhigh` | 未確認 | 完成 harness 與 12 項測試，主代理整合與 native 驗收 |
| Tunnel / Galileo `01a0fac3-43f3-7291-87dc-c0407f4cab88` | `gpt-6.1-sol / high` | 未確認 | 完成安全檢查與測試，主代理整合 |
| CI / Nash `01a0fac3-44e8-7973-ab0f-62f8898ac7c5` | `gpt-6-luna / max` | 未確認 | 部分 patch 交接後關閉；主代理完成整合與驗證 |
| Timeout / James `01a0fae4-f229-7760-a9b6-0b1af85742c9` | `gpt-6.1-sol / high` | 未確認 | 重現根因、完成 production 修正；原測試 1 passed、adaptive filter 5 passed |

## W2 cleanup 與停止點

- 已保存 final Windows JSON 與摘要到 `docs/windows/evidence/`。
- W2 自建且未追蹤的 root `.build`、`runtime-engine/target`、`rust-helper/target` 已刪除。
- `scripts/__pycache__` 僅刪除 W2 的 `test_windows_runtime_e2e...pyc`；其他 cache 保留。
- 保留既有 `.toolchain`、`dist`、local backups、其他 worktrees 與使用者資料；沒有執行 `git clean`。

以下項目仍然 **不屬於 W2 驗證範圍**：

- Windows 11 實機與 Windows ARM64 實機。
- Windows Desktop UI、installer、簽章、更新流程與標準使用者完整 smoke。
- Credentialed OpenAI relay / ChatGPT workflow；官方 tunnel binary 安裝與 `--version` 不等同連外流程。
- E2E harness 使用隔離 data/resource/bin overrides，因此不證明未封裝桌面版本的預設 resource discovery。
- Graphify 與 Obsidian 的 W4 source-converged 同步。

W2 到此結束。之後若使用者明確要求 W3，從 `windows/w2-runtime-e2e` 的 final W2 closeout commit 繼續；不要重做 W1/W2。**目前不得自動進 W3、不 merge main、不發布 release。**
