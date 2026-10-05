# W5 進度與交接

狀態：**W5 automatable Windows release-engineering track 已完成。** Candidate `23bc145dd383a3c065f68f85fca200f06eced005` 在同一輪 GitHub Actions run `37254301045`（run #65）通過 source / history / macOS package、Windows W2/W3 regression、W5 source integrity、current installed lifecycle、default NSIS self-copy uninstall，以及真實 public `v0.4.0` source → v0.4.1 historical upgrade。

這個結論只代表 **automated W5 release-engineering complete**。Windows 仍是 unsigned x64 CI validation candidate；不代表 signed/public Windows release，也不代表 physical Windows 11 / private credential workflow 已完成產品驗收。

## 候選版本與 CI

- 分支：`windows/w5-release-completion-v041`
- Candidate source：`23bc145dd383a3c065f68f85fca200f06eced005` (`feat: add global instructions and refine computer use`)
- GitHub Actions：run `37254301045` / run #65
- Final conclusion：`success`
- 同一 candidate HEAD 的 7 個 jobs 全成功：
  1. Source release gate
  2. Windows W5 source integrity
  3. Public history / secret scan
  4. ARM64 package gate
  5. Windows core gate
  6. Windows W5 installer candidate
  7. Windows W5 historical upgrade

## v0.4.1 current-candidate 安裝證據

Tested artifact：`windows-w5-unsigned-candidate-37254301045-1`

- artifact id：`11323170277`
- GitHub artifact digest：`sha256:9464d365941d6cf6b8298f091603e05aa0f359c6cd2d03cbbf16b07fecbad69a`
- version：`0.4.1`
- source SHA：`23bc145dd383a3c065f68f85fca200f06eced005`
- architecture：`x86_64`; profile：`release`
- production feature：`custom-protocol`; `desktop_smoke=false`
- Authenticode：`unsigned`; updater：`disabled`; publication：`not authorized`
- synthetic upgrade baseline：`0.4.0`
- installer：`Chadex-0.4.1-windows-x64-unsigned-setup.exe`
- installer SHA-256：`b5c5fe086424fc5220fc9e173a4f75b1bfa5d2a52b1ff8894e6e9b0b85f82c3e`；已直接重算並與 sidecar / candidate manifest 一致。

Sanitized installer report：`windows-w5-installer-report-37254301045-1`

- artifact id：`11322099526`
- GitHub artifact digest：`sha256:a0994e910be4994e48cdd856d0c9f0f5105b6e30777466df4843fbdac4167728`
- lifecycle：**18 / 18 stages passed**
- cleanup：`owned_count=67`、`remaining_count=0`、`forced_count=0`

Current lifecycle 覆蓋：candidate/hash validation、synthetic v0.4.0 baseline install、production WebView/runtime/default path、upgrade、preferences/project-data preservation、relaunch、same-version reinstall、controlled uninstall、registry removal、**default NSIS self-copy uninstall** 與 fixture cleanup。Default self-copy stage 同樣要求 user data/project marker 保留、registry entry 移除，且沒有 fallback forced cleanup。

## Historical v0.4.0 source → v0.4.1 upgrade

Historical report：`windows-w5-historical-upgrade-report-37254301045-1`

- artifact id：`11322819591`
- GitHub artifact digest：`sha256:b2baee7d64e973883e1f2c757321da2c6bdcd0fe1f80d2060e6a1d157eefde90`
- current source SHA：`23bc145dd383a3c065f68f85fca200f06eced005`
- baseline source SHA：`5fc7c84728a5de4ef9c39c20cf2e22f654cbb24c`（public tag `v0.4.0`）
- upgrade type：`historical_source_upgrade`
- lifecycle：**18 / 18 stages passed**
- cleanup：`owned_count=63`、`remaining_count=0`、`forced_count=0`

CI 從 exact public `v0.4.0` tag 建 isolated source worktree，建出真實舊版 installer/resource set，先安裝與啟動舊 binary，再升級到 already-tested v0.4.1 candidate。升級後 resource hashes、preferences、project data、rendered UI、runtime readiness、relaunch、reinstall、controlled/default uninstall、registry cleanup 與 process ownership 全部重新驗證。這份證據與 synthetic metadata fixture 分開，因此不再把 same-binary fixture 誤當 historical compatibility。

## v0.4.1 Global Instructions / Computer Use native Windows regression

同一個 `Windows core gate` 已在 native Windows runner 執行新增 contract：

- Windows Tauri Global Instructions storage tests：8 passed / 0 failed；包含 Unicode/blank round-trip 與 oversize 不取代現有檔案。
- Helper Computer safety：`Always allow` 跨 tunnel session 保留、approval 原子升級為 Always Allow、Stop 跨新 tunnel session 持續到 explicit Resume，皆通過。
- Windows runtime / desktop W2-W3 regression 同 job 全成功。
- interactive UIA live fixtures 仍是 ignored/manual 類別；沒有把 hosted CI 誤報成一般使用者實機互動 acceptance。

## Graphify / Obsidian closeout

Graphify 0.9.45 已正常 `update` 到 candidate source `23bc145d`：60,121 nodes / 177,143 edges / 1,839 communities；`graphify check-update .` 通過。Obsidian generated layer 同步 17 components / 48 curated production files / 68 outputs，寫入後 `--check` 為 `changed=0`、`stale_generated=0`。AP / Computer Use 人工筆記也已更新 v0.4.1 instruction ownership、Always Allow 與 persistent Stop 語意。

## External acceptance that remains outside W5 automation

以下仍需要 real user environment、private credentials/hardware 或明確 distribution decision：

- physical Windows 11 interaction 與 Windows ARM64 hardware/build
- native picker、Explorer open、tray、launch-at-login、notifications
- credentialed ChatGPT / Secure Tunnel workflow
- trusted Authenticode signing、SmartScreen / reputation
- genuinely missing WebView2 host 與 interactive bootstrapper / installer choices
- Credential Manager uninstall deletion policy
- production Windows updater / signing / delivery policy

`.github/workflows/release.yml` 仍只發布 macOS Apple Silicon DMG。Windows candidate 仍是 unsigned CI validation evidence；若未來要公開 Windows asset，需要另開 distribution phase，不應用 W5 automation success 取代簽章、實機與 private workflow acceptance。
