# Chadex

**ChatGPT Web + your local repo. No copy-paste loop.**

## ⬇️ Download Chadex

### **[Download the latest release →](https://github.com/jimmynycuee/Chadex/releases)**

`macOS 14+ · Apple Silicon` · `Windows x64` · `Free & Open Source`

**Latest stable release: v0.7.2** — more reliable Goal runs (execution-environment preflight, isolated acceptance lanes, checkpoint-aware retries and resumable recovery), no duplicate Plan or Goal runs, and the first Windows x64 installer: closing the window hides Chadex to the system tray while it stays connected, and later versions install from inside the app. v0.7.1: Plan and Goal reliability fixes: manifest-first Skill tool discovery, approval persistence across reconnects, safe single-flight recovery without replaying uncertain writes, and project-scoped isolated Goal execution. v0.7.0 introduced Plan Mode and Goal Mode, plus a notice while ChatGPT operates the Mac. 0.6.5: Computer Use no longer leaves Chromium browsers slow: web accessibility is switched back off after 120 s idle, and ChatGPT is told when to use Computer Use. 0.6.4 made macOS buttons follow the interface size in a Liquid Glass style. 0.6.3 moved releases to the macOS 26 SDK so the design v2 Liquid Glass appears in the released app. Patch releases 0.6.1–0.6.3 fix off-centre macOS button labels at larger interface sizes and give all buttons the capsule shape of the status pills. v0.6.0 brought design v2 to macOS and Windows, a Computer Use permission card, an agent cursor overlay and Chromium web accessibility on macOS, Tunnel ID validation before saving, and a stricter local runtime (no shared-key login, loopback-only CORS and Host checks).

> On the Releases page, expand **Assets** and download `Chadex-...-macos-arm64.dmg` for macOS or `Chadex-...-windows-x64-unsigned-setup.exe` for Windows.

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

### Windows

1. 前往 [GitHub Releases](https://github.com/jimmynycuee/Chadex/releases)，下載 `Chadex-<版本>-windows-x64-unsigned-setup.exe`。
2. 執行安裝檔。安裝檔尚未做 Authenticode 簽署，若 SmartScreen 顯示「Windows 已保護您的電腦」，請按「其他資訊 → 仍要執行」。
3. 從開始功能表啟動 Chadex，依 App 內「使用說明」完成設定。

關閉視窗只會把 Chadex 收到系統匣，連線仍會保持；要完全結束，請在系統匣圖示按右鍵選 **Quit**。v0.7.2 起可在 App 內更新；v0.7.1 及更早版本需先手動安裝一次 v0.7.2。

## Chadex 能做什麼

- 以原生 macOS 介面選擇並管理本機專案。
- 將 ChatGPT 連接到指定專案，而不是暴露整台電腦的任意路徑。
- 提供檔案操作、shell、Git、長時間任務與本機開發工具的執行能力。
- 支援專案切換、工作階段、隔離式 worktree 與長時間 execution lifecycle。
- 長時間命令以 durable job 持續執行；觀察 timeout 或輸出截斷不會自動重新啟動同一份工作，terminal/failure 仍可提早喚醒後續流程。
- 以 macOS Keychain 保存敏感憑證，並由本機 runtime 管理連線與執行狀態。
- 在 App 中顯示實際連線、活動與任務狀態，不以按鈕操作結果假設連線成功。
- **Agent 設定**提供 Chadex 全域共用的 Global Instructions 與目前專案的 Skills；Skills 可用「上傳 Skill…」複製一份到 Chadex，或在「外部 Skill 來源」連接你已用於 Codex／Claude Code／共用 agent 的資料夾（腳本預設關閉，可逐資料夾允許）；repository `AGENTS.md` 維持原生檔案與 nested hierarchy，Chadex 只讀取已註冊 project root 內適用的規則，不把外層 ambient `AGENTS.md` 當成全域設定。
- Project Memory 會以受限 bootstrap summary 在任務開始時自動提供長期架構／決策／工作流程脈絡，詳細內容維持 lazy load，並可從進階 Memory Inspector 檢查。
- **Computer Use** 支援 read-only observation、需批准／本次工作階段允許／持久 **Always allow** 的控制、敏感內容保護與 stale/outcome-unknown recovery；**Stop** 會持續生效（重新連線也不解除），直到明確 Resume。Read-only / Ask / Always allow 預設政策可離線設定。
- **Plan Mode／Goal Mode**（開發中，見 [`docs/PLAN_GOAL_MODE.md`](docs/PLAN_GOAL_MODE.md)）：在 ChatGPT 先以後端強制唯讀的 Plan Mode 規劃並儲存不可變版本，於 Chadex App「計畫與目標」核准確切版本後，由 Chadex 在隔離 worktree 中自主執行、以核准的驗收指令客觀判定完成；關閉 ChatGPT 不影響執行，並提供暫停／取消／緊急停止。
- 新增 Code Ferret 吉祥物，依已觀測到的工具與任務狀態顯示動畫；可關閉顯示或動態效果，並支援 macOS「減少動態效果」。

Chadex **不是另一個聊天介面或模型 API client**；工作指令仍然在 ChatGPT 中下達。

## 快速開始

1. 開啟 Chadex。
2. 選擇要讓 ChatGPT 存取的本機專案。
   - 可在尚未連線 project 前先編輯 **Chadex Global Instructions**；選取專案後可在 **Agent 設定**管理 Skills，並在 **Computer Use** 設定離線預設政策。Repository 自己的 `AGENTS.md` 不需要在 Chadex 中設定，runtime 會直接讀取。
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
- [`docs/PLAN_GOAL_MODE.md`](docs/PLAN_GOAL_MODE.md) — Plan Mode／Goal Mode 使用方式、安全邊界、恢復語意與實測證據
- [`docs/RELEASE.md`](docs/RELEASE.md) — release gate、簽署、DMG 與 GitHub 發佈流程
- [`docs/releases/0.7.2.md`](docs/releases/0.7.2.md) — Goal 復原、Windows 安裝檔、系統匣生命週期與 App 內更新
- [`docs/releases/0.7.1.md`](docs/releases/0.7.1.md) — Plan／Goal 路由、核准持久性、安全復原與驗收修正
- [`docs/releases/0.7.0.md`](docs/releases/0.7.0.md) — Plan 模式、Goal 模式（Codex／ChatGPT 兩種模式）、Computer Use 操作中提示
- [`docs/releases/0.6.5.md`](docs/releases/0.6.5.md) — Computer Use 閒置後關回 Chromium 網頁輔助功能、提示 ChatGPT 何時使用
- [`docs/releases/0.6.4.md`](docs/releases/0.6.4.md) — 按鈕跟著介面大小縮放的 Liquid Glass 樣式
- [`docs/releases/0.6.3.md`](docs/releases/0.6.3.md) — 改用 macOS 26 SDK 建置正式版
- [`docs/releases/0.6.2.md`](docs/releases/0.6.2.md) — macOS 按鈕膠囊形修正
- [`docs/releases/0.6.1.md`](docs/releases/0.6.1.md) — macOS 按鈕標籤在大介面尺寸置中修正、膠囊按鈕形狀
- [`docs/releases/0.6.0.md`](docs/releases/0.6.0.md) — Design v2、Computer Use 權限卡／游標疊加層／網頁無障礙、Tunnel ID 驗證、本機 runtime 加固與已知限制
- [`docs/releases/0.5.0.md`](docs/releases/0.5.0.md) — External Skill sources、Skills 頁、本機 admin token、啟動預熱與已知限制
- [`docs/releases/0.4.1.md`](docs/releases/0.4.1.md) — Windows W5 自動化 release-readiness 收斂、歷史版本升級與卸載驗證邊界
- [`docs/releases/0.4.0.md`](docs/releases/0.4.0.md) — Agent Settings、Skills、Project Memory、Computer Use 與離線設定
- [`docs/releases/0.3.3.md`](docs/releases/0.3.3.md) — ChatGPT 檔案匯入橋接、自動更新偵測與 Windows W5 source 狀態
- [`docs/releases/0.3.2.md`](docs/releases/0.3.2.md) — 寵物狀態判斷、活動停留與實際驗證
- [`docs/releases/0.3.1.md`](docs/releases/0.3.1.md) — Code Ferret 初次實裝
- [`docs/SECURITY_AND_PERFORMANCE_REVIEW.md`](docs/SECURITY_AND_PERFORMANCE_REVIEW.md) — security / performance 驗證與 benchmark
- [`UPSTREAM.md`](UPSTREAM.md) — upstream provenance、修改歷史與 attribution

## License & Attribution

Chadex repository 目前以 **Apache License 2.0** 發布，完整條款見 [`LICENSE`](LICENSE)。

Chadex 的部分 runtime source 由 Apache-2.0 licensed WebCodex baseline 演進而來；固定 upstream revision、衍生關係、修改紀錄與保留的 license notices 統一記錄於 [`UPSTREAM.md`](UPSTREAM.md)。
