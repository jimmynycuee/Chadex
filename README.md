# Chadex

Chadex 是一個原生 macOS 工具，讓 ChatGPT 能安全地連接到你明確選定的本機專案，直接協助閱讀檔案、執行工具與完成開發工作。

**核心流程：選擇專案 → 連接 ChatGPT → 在 ChatGPT 中開始工作。**

## 下載與安裝

一般使用者不需要 clone repository 或自行編譯。

1. 前往 [GitHub Releases](https://github.com/jimmynycuee/Chadex/releases)。
2. 下載最新的 Apple Silicon (`arm64`) `.dmg`。
3. 打開 DMG，將 `Chadex.app` 拖入 **Applications**。
4. 從 Applications 或 Spotlight 啟動 Chadex。

目前免費發佈版本採 Hardened Runtime + ad-hoc signing，尚未經 Apple notarization。第一次開啟若被 macOS 阻擋，請前往 **系統設定 → 隱私權與安全性 → 仍要打開（Open Anyway）**，再確認開啟。請勿為此全域關閉 Gatekeeper。

### 系統需求

- macOS 14+
- Apple Silicon (`arm64`)
- ChatGPT 中可使用自訂 MCP / connector 的環境

Intel / Universal build 目前尚未完成 release-level 驗證，因此暫不列為正式支援。

## Chadex 能做什麼

- 以原生 macOS 介面選擇並管理本機專案。
- 將 ChatGPT 連接到指定專案，而不是暴露整台電腦的任意路徑。
- 提供檔案操作、shell、Git、長時間任務與本機開發工具的執行能力。
- 支援專案切換、工作階段、隔離式 worktree 與長時間 execution lifecycle。
- 以 macOS Keychain 保存敏感憑證，並由本機 runtime 管理連線與執行狀態。
- 在 App 中顯示實際連線、活動與任務狀態，不以按鈕操作結果假設連線成功。

Chadex **不是另一個聊天介面或模型 API client**；工作指令仍然在 ChatGPT 中下達。

## 快速開始

1. 開啟 Chadex。
2. 選擇要讓 ChatGPT 存取的本機專案。
3. 完成 Chadex 顯示的 ChatGPT 連接設定。
4. 等待目前專案顯示為已連線／已驗證。
5. 回到 ChatGPT，直接要求它檢查、修改、測試或執行該專案。

切換到另一個專案時，Chadex 會重新建立該專案自己的連線與驗證狀態，不沿用上一個專案的驗證結果。

## 安全設計

Chadex 將專案選擇、憑證、Secure MCP Tunnel、ChatGPT verification 與本機 runtime lifecycle 分開管理。API key 的持久化來源是 macOS Keychain；App UI 不會直接持有或顯示 runtime secret。

更完整的 trust boundary、connection epoch、credential lifecycle 與 runtime 架構請見：

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
- [`docs/BRIDGE_PROTOCOL.md`](docs/BRIDGE_PROTOCOL.md)
- [`docs/SECURITY_AND_PERFORMANCE_REVIEW.md`](docs/SECURITY_AND_PERFORMANCE_REVIEW.md)

## 開發

需要從 source 建置 Chadex 時：

### 需求

- macOS 14+
- Xcode / Swift 5.10+
- Rust 1.98.1
- Git
- curl

若本機沒有對應 Rust toolchain：

```bash
./scripts/bootstrap_rust.sh
```

### 測試

```bash
./scripts/test.sh
```

Release / RC 前的完整驗證：

```bash
./scripts/release_check.sh
```

### 建立本機 App

```bash
./scripts/build_app.sh
open dist/Chadex.app
```

Release profile、簽署、DMG、GitHub Actions 與 distribution boundary 的完整流程請見 [`docs/RELEASE.md`](docs/RELEASE.md)。

## 文件

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — runtime、project isolation、execution lifecycle 與安全邊界
- [`docs/BRIDGE_PROTOCOL.md`](docs/BRIDGE_PROTOCOL.md) — Swift ↔ Rust bridge protocol
- [`docs/RELEASE.md`](docs/RELEASE.md) — release gate、簽署、DMG 與 GitHub 發佈流程
- [`docs/SECURITY_AND_PERFORMANCE_REVIEW.md`](docs/SECURITY_AND_PERFORMANCE_REVIEW.md) — security / performance 驗證與 benchmark
- [`UPSTREAM.md`](UPSTREAM.md) — upstream provenance、修改歷史與 attribution

## License & Attribution

Chadex repository 目前以 **Apache License 2.0** 發布，完整條款見 [`LICENSE`](LICENSE)。

Chadex 的部分 runtime source 由 Apache-2.0 licensed WebCodex baseline 演進而來；固定 upstream revision、衍生關係、修改紀錄與保留的 license notices 統一記錄於 [`UPSTREAM.md`](UPSTREAM.md)。
