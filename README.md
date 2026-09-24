# Chadex

Chadex 是一個 macOS 14+ 原生工具，讓使用者用較簡單的 Finder／系統設定式介面把 ChatGPT Web 安全連到明確選定的本機專案。

## 安裝

Chadex 的免費 macOS 發佈採用 GitHub Release + ad-hoc signed DMG，模式與 WebCodex Desktop 相同，不需要付費 Apple Developer Program。下載 Apple Silicon (`arm64`) DMG 後拖入 Applications 即可。因為免費版本未經 Apple notarization，第一次開啟若被 macOS 阻擋，請到 **系統設定 → 隱私權與安全性 → 仍要打開（Open Anyway）**，再確認開啟；不要全域關閉 Gatekeeper。

第一版的核心流程是：**選擇專案 → 連接 ChatGPT → 查看活動與結果**。Chadex 不提供聊天介面、模型 API、程式編輯器、遠端 Server 或 Quick Share UI；工作指令仍在 ChatGPT 內下達。

## 架構

- **SwiftUI / AppKit**：原生視窗、資料夾選擇器、Menu Bar、Keychain、Login Item。
- **Swift 狀態層**：只消費 Rust helper 的真實 snapshot，不以按鈕結果推測已連線。
- **Rust helper / Chadex Core**：私有 NDJSON stdin/stdout bridge；Chadex 自己持有 Tunnel credential、OpenAI `tunnel-client` lifecycle、readiness 與 ChatGPT verification。
- **Chadex MCP ingress**：位於 Secure MCP Tunnel 與本機 MCP backend 之間，透明轉送 MCP traffic，並以 connection epoch 隔離不同專案／不同連線的驗證證據。
- **Chadex runtime engine**：production app 只建置與啟動 `chadex-runtime-cli/server/runner`，其 MCP/file operations、Runner、tool contracts 與 process supervision 由 `runtime-engine/` 內的 Chadex-owned crates 提供。這份 production source 由固定 WebCodex Apache-2.0 baseline 演進而來，但 Cargo graph、build script 與 app bundle 都不再依賴 `vendor/webcodex`。Tunnel ID/API key、OpenAI Secure MCP Tunnel 與 ChatGPT verification 仍完全由 Chadex 擁有。

完整說明見 `docs/ARCHITECTURE.md` 與 `docs/BRIDGE_PROTOCOL.md`。

## 上游基準

WebCodex 固定於：

`1bdc05e488ee56bca358bc6ba455cebd5831917d`

原始固定版本完整來源保留於 `vendor/webcodex`，只作 provenance、歷史比較與 attribution reference，不參與 production build。Chadex production runtime source 位於 `runtime-engine/`；來源、授權與遷移關係列於 `UPSTREAM.md`。Apache-2.0 license 同時保留於 `vendor/webcodex/LICENSE`、`runtime-engine/LICENSE` 與 `attribution/WebCodex-LICENSE.txt`。

## 建置需求

- macOS 14+
- 目前 RC1 打包與實機 release validation 以 Apple silicon (`arm64`) 為目標；Intel / Universal runtime 尚未完成 release-level 驗證，因此不宣稱正式支援。
- Xcode 及 Swift 5.10+ toolchain
- Rust 1.98.1（由 `rust-toolchain.toml` 固定；`bootstrap_rust.sh` 會使用相同版本）
- Git、curl

如果機器沒有 Rust，可將 toolchain 安裝在 Chadex 專案內，不修改 shell 設定：

```bash
./scripts/bootstrap_rust.sh
```

## 測試

```bash
./scripts/test.sh
```

這會執行 Swift tests 與 Rust helper tests。Chadex runtime engine 的 Phase 12 execution semantics、tool contracts 與 Runner regression suites 可直接由 `runtime-engine/Cargo.toml` 執行；需要外部工具或平台條件的 integration tests 仍維持 `ignored` 標記。

日常 `test.sh` 保持快速；準備 RC / Release 時使用完整 release gate：

```bash
./scripts/release_check.sh
```

release gate 預設要求 clean Git worktree，並驗證 license / attribution、Swift 與 Rust helper tests、所有 runtime targets 的編譯、release-critical runtime/tooling regression suites、production runtime wrapper，以及以 **release profile** 重新打包到暫存目錄的 `.app`。開發中的 dirty tree 若只要預先驗證，可顯式設定 `CHADEX_RELEASE_ALLOW_DIRTY=1`；這不代表 artifact 可發布。完整 workspace 中需真實外部條件或可能長時間等待的 integration tests 不以無上限方式塞進這個 gate。

## 建立本機 `.app`

```bash
./scripts/build_app.sh
open dist/Chadex.app
```

腳本會：

1. 從 `runtime-engine/` 建置 Chadex-owned runtime crates 與 `chadex-runtime-cli/server/runner` entrypoints；正式打包預設使用 Cargo `release` profile，不讀取 `vendor/webcodex`。開發 dogfood build 必須顯式設定 `CHADEX_RUNTIME_PROFILE=dogfood`。
2. 建置 Rust helper。
3. 建置 Swift executable 與 localization resources。
4. 組成標準 `Chadex.app` bundle；production bundle 不再包含 `webcodex`、`webcodex-server`、`webcodex-runner` executable。
5. 保留 Chadex `LICENSE`、`UPSTREAM.md` 與 WebCodex Apache-2.0 attribution，並完成 app 簽署。

此流程不是 Developer ID 發佈、公證、App Store 或自動更新流程。

RC / Release 的完整 gate、compile-time frontend asset 規則、版本注入與 distribution boundary 見 `docs/RELEASE.md`。

## 授權

Chadex repository 以 **Apache License 2.0** 發布，完整條款見根目錄 `LICENSE`。`runtime-engine/` 包含由固定 WebCodex Apache-2.0 baseline 演進而來的 derivative source；上游版本、修改歷史與保留的 license copy 詳見 `UPSTREAM.md` 與 `attribution/WebCodex-LICENSE.txt`。

## 憑證

- Tunnel ID：非機密，存於 Chadex Application Support preferences。
- API key：只存 macOS Keychain。
- App 啟動時 API key 經 helper 私有 stdin 傳入 Chadex `CredentialStore`，並以 `Zeroizing<String>` 只存在 Rust 記憶體。
- Chadex `TunnelManager` 直接啟動固定版 OpenAI `tunnel-client`；API key 只出現在該精確 child process 所需的 `CONTROL_PLANE_API_KEY` environment，不出現在 argv、snapshot、stdout 或 activity log。
- Chadex runtime engine 的 legacy-compatible Tunnel config 保持 empty/inert，不會收到 Chadex Tunnel ID 或 API key，也不建立獨立 runtime Tunnel credential file。

## 狀態語意

Chadex 明確區分：未設定、準備中、等待 ChatGPT 驗證、目前專案已驗證、已停止、錯誤。

**Tunnel ready 不等於 ChatGPT verified。** Chadex MCP ingress 在 Tunnel `/readyz` 成功後才開始觀察外部 MCP traffic；`initialize` / `tools/list` 只代表 ChatGPT 已連到 Chadex，只有目前 connection epoch 的 `tools/call` 收到請求 ID 相符、非錯誤的完整工具結果後才標示「此專案已驗證」。切換專案、換憑證、重新啟 Tunnel 或 disconnect 都會重設 epoch，因此舊專案證據不能沿用。

## 目前限制

- 真實 OpenAI Tunnel 的端到端驗收需要有效 Tunnel ID / Restricted API key 與 ChatGPT 端設定；沒有憑證時會維持未設定或未驗證，不使用模擬結果。
- 若系統 PATH 與 Chadex managed cache 都沒有固定版 `tunnel-client`，第一次連線會從 OpenAI 官方 GitHub release 下載 v0.0.12，並在執行前驗證固定 SHA-256。
- `runtime-engine/` 包含由固定 WebCodex baseline 演進而來的 Chadex-owned production runtime source；`vendor/webcodex` 只保留 attribution/provenance reference，已不在 production Cargo dependency graph、build path 或 app bundle executable path。
- Login Item 最終行為需以實際 `.app` 放置位置與 macOS 系統權限驗證；CLI build 成功不等於該系統整合已實機驗證。
- Developer ID 簽署、公證、自動更新與 App Store 上架不在本階段範圍。
