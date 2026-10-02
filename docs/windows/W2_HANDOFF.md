# W2 進度與交接

狀態：**驗收中**。依使用者指示，W2 完成後暫停；不得自動開始 W3。

## 分支與驗證對象

- 開發分支：`windows/w2-runtime-e2e`。
- 基底：W1 `0e1b3172a08b81cc7b2b3deb7ffc25f47862db01`，包含 macOS `v0.3.2` ancestry。
- 最近完成的 native checkpoint：`28529a540ec21ab42dd4927afda2babcf75925e6`（official asset version 檢查失敗）。
- [Windows native 與 macOS CI](https://github.com/jimmynycuee/Chadex/actions/runs/36966169327)。
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

Windows 第二輪 native：runner 784 passed、helper 150 passed、supervisor 3 passed、額外 cleanup 2 passed；official asset version 檢查失敗，runtime E2E 未執行。已依 pinned 官方 source 修復合法 `+GitSHA` metadata 的辨識，保留 pinned base version 與非法 metadata 拒絕檢查；durable-job fixture 同時明確寫入 UTF-8 bytes，避免 Windows 自動換行轉換。修後 native 結果：**pending**，不得視為 W2 完成。

本機 macOS 已執行：

| 驗證 | 實際結果 |
| --- | --- |
| Swift | 70 passed / 0 failed |
| Helper | 159 unit + 3 integration passed；3 原有 opt-in |
| Runner library | 863 passed / 0 failed / 5 原有 opt-in，`--test-threads=4` |
| Process lifecycle opt-in | 15 passed，含 20 次 stress cycles |
| Runner config | 24 passed |
| Harness deterministic tests | 12 passed；不是 native E2E |
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

## 停止點與後續限制

W2 exit criteria 尚待 Windows native 結果。以下項目 **not validated**，留到使用者明確恢復後的適當階段：

- Windows 11 實機、Windows ARM64。
- Windows Desktop UI、installer、簽章、更新流程與標準使用者完整 smoke。
- Credentialed OpenAI relay / ChatGPT workflow；官方 tunnel binary 安裝與 `--version` 不等同連外流程。
- E2E harness 使用隔離 data/bin overrides；不證明未封裝桌面版本的預設 resource discovery。
- Graphify 與 Obsidian 的 W4 source-converged 同步。

目前 **不得進 W3**。完成 native 驗收與本輪 artifact cleanup 後更新此檔；之後需使用者恢復指示，再從最終 W2 commit 啟動 W3。保留 W1 / macOS baseline 的 ancestry 與既有使用者資料。
