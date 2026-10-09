---
schema_version: 1
project: chadex
canonical_branch: main        # 所有 agent 以此為準；階段分支完成後才合回
updated: 2026-10-09
updated_by: claude           # codex | webcodex | chadex | claude | human
current_phase: V060
# 狀態依據：git 分支是否已合入 main（done 者的 closed_commit 為分支最後一個 commit），
# 加上 2026-10-01～10-05 的工作紀錄。owner 留空，接手時由 agent 填入。
phases:
  - id: W1
    name: Windows Core Foundation
    branch: windows/w1-core-readiness
    status: done
    owner: null
    depends_on: []
    closed_commit: 0e1b317
  - id: W2
    name: Windows Runtime E2E
    branch: windows/w2-runtime-e2e
    status: done
    owner: null
    depends_on: [W1]
    closed_commit: c605691
  - id: W3
    name: Windows Desktop Product
    branch: windows/w3-desktop-product
    status: done
    owner: null
    depends_on: [W2]
    closed_commit: dd7d5eb
  - id: W4
    name: Windows Distribution/Beta (source converged)
    branch: codex/w4-source-converged
    status: done
    owner: null
    depends_on: [W3]
    closed_commit: c3eedfc
  - id: W5
    name: Windows Hardening/Stable (release readiness)
    branch: codex/w5-release-readiness
    status: done
    owner: null
    depends_on: [W4]
    closed_commit: c85ad22
  - id: AP1
    name: Project Instructions
    branch: feature/ap1-project-instructions
    status: done
    owner: null
    depends_on: []
    closed_commit: fa54870
  - id: AP2
    name: Skills Center
    branch: feature/ap2-skills-center
    status: done
    owner: null
    depends_on: [AP1]
    closed_commit: b36c0d2
  - id: AP3
    name: Project Memory
    branch: feature/ap3-project-memory
    status: done
    owner: null
    depends_on: [AP2]
    closed_commit: 3b6cbe3
  - id: AP35
    name: Workspace navigation
    branch: feature/ap35-workspace-navigation
    status: done
    owner: null
    depends_on: [AP3]
    closed_commit: 4632938
  - id: CM1
    name: Computer Use permission readiness
    branch: feature/cm1-computer-readiness
    status: done
    owner: null
    depends_on: []
    closed_commit: d6523cd
  - id: CM2
    name: Computer observe (read-only)
    branch: feature/cm2-computer-observe
    status: done
    owner: null
    depends_on: [CM1]
    closed_commit: 5e9cab8
  - id: CM3
    name: Computer control (Observe/Act/Verify)
    branch: feature/cm3-computer-control
    status: done
    owner: null
    depends_on: [CM2]
    closed_commit: 08c921f
  - id: CM4
    name: Computer control safety and stop
    branch: feature/cm4-computer-safety
    status: done
    owner: null
    depends_on: [CM3]
    closed_commit: 7a5d650
  - id: CM5
    name: Computer Use acceptance
    branch: feature/cm5-computer-acceptance
    status: done
    owner: null
    depends_on: [CM4]
    closed_commit: 957519e
  - id: V040
    name: v0.4.0 release (AP1-AP4, CM1-CM5)
    branch: main
    status: done
    owner: null
    depends_on: [AP35, CM5, W5]
    closed_commit: 5fc7c84
  - id: V041
    name: v0.4.1 (Global Instructions, Computer Use UI, W5 completion)
    branch: windows/w5-release-completion-v041
    status: done
    owner: null
    depends_on: [V040]
    gate:                     # 2026-10-05 final fresh rerun from clean V041 worktree at 0af8fcb
      - cmd: "swift test"
        expect: "all Swift tests pass"
        result: "passed: 84/84 @ 0af8fcb"
      - cmd: "helper tests"
        expect: "all non-ignored helper tests pass"
        result: "passed: 184 passed, 3 ignored @ 0af8fcb"
      - cmd: "Computer Use safety"
        expect: "10/10"
        result: "passed: 10/10 @ 0af8fcb"
      - cmd: "release gate"
        expect: "release_check.sh exits 0 and builds a fresh release-profile app"
        result: "passed: release_check.sh exit 0; fresh release-profile Chadex.app built and signed @ 0af8fcb"
    closed_commit: 0af8fcb
  - id: V042
    name: Baseline hygiene & documentation sync
    branch: chore/v042-baseline-hygiene
    status: done
    owner: claude-code
    depends_on: [V041]
    # baseline: main @ 16efcf1, CI run 37306101038 attempt 2 = success
    #（attempt 1 的 W5 installer candidate 為 runner 端 process_inventory_failed，rerun 通過）
    # 2026-10-05 經使用者授權 fast-forward 合入 main @ 924c75c；closed_commit 為最後一個實作 commit
    gate:
      - cmd: "scripts/test.sh (swift test + helper cargo test)"
        expect: "all pass"
        result: "passed: Swift 83/83; helper 179+3 passed, 3 ignored @ a220432（較 V041 少的 1+2 個為刪除的 getProjectInstructions 測試）"
      - cmd: "python3 scripts/test_ci_change_scope.py"
        expect: "5/5"
        result: "passed: 5/5 @ f0a7253"
      - cmd: "full CI on branch"
        expect: "all jobs success"
        result: "passed: run 37323159202 = 9/9 success @ f0a7253（含 source release gate、W5 installer + historical upgrade）"
      - cmd: "docs-only fast path"
        expect: "heavy jobs skipped; history + docs checks pass"
        result: "passed: run 37330360154 @ a2a8619, 38s, 6 heavy jobs skipped"
      - cmd: "graphify update . + coverage diff"
        expect: "vendor removed; active source coverage unchanged"
        result: "passed: 60,141→27,938 nodes / 177,154→90,703 edges; runtime-engine 19,232 / apps 367 unchanged; vendor 0"
      - cmd: "sync_graphify_obsidian.py --check"
        expect: "changed=0, stale_generated=0"
        result: "passed: 68 outputs, changed=0, stale_generated=0"
    closed_commit: a2a8619
  - id: AP5
    name: External skill sources
    branch: feature/ap5-external-skill-sources
    status: done
    owner: null
    depends_on: [V042]
    # 決策（2026-10-05）：外部來源 script 預設不可執行，閘門對所有 configured roots 一律預設關閉；
    # 連結範圍為 Runner 層級（所有專案共用）。
    # 2026-10-06：實作完成 @ 5103954；CI run 37355667286 = 9/9 success；release_check.sh 通過。
    # 待使用者手動驗收與合併（合併順序：AP5 → LS1 → 其餘）。
    # 一併經 integration/skill-parity 驗收並合入 main（見 V050 gate）。
    gate:
      - cmd: "release_check.sh @ integration/skill-parity"
        expect: "exit 0"
        result: "passed: Swift 167/167; Rust passed 3861 @ 4e7e2e4"
      - cmd: "full CI"
        expect: "all jobs success"
        result: "passed: run 37655626569 @ 4e7e2e4 (attempt 2; attempt 1 W5 historical upgrade default_uninstaller_self_copy process_inventory_failed, runner flake)"
    closed_commit: 5103954
  - id: LS1
    name: Runtime launch/connect speed (macOS prewarm)
    branch: feature/runtime-launch-speed
    status: done
    owner: null
    depends_on: [AP5]
    # review 修正在 feature/runtime-launch-speed-fixes（0a10a3a、b881a39），reviewer 複查無 blocking；
    # 「還原本機服務」改為預設開啟的背景預熱開關（使用者決定）。合併時一併帶入。
    # 一併經 integration/skill-parity 驗收並合入 main（見 V050 gate）。
    gate:
      - cmd: "release_check.sh @ integration/skill-parity"
        expect: "exit 0"
        result: "passed: Swift 167/167; Rust passed 3861 @ 4e7e2e4"
      - cmd: "full CI"
        expect: "all jobs success"
        result: "passed: run 37655626569 @ 4e7e2e4 (attempt 2; attempt 1 W5 historical upgrade default_uninstaller_self_copy process_inventory_failed, runner flake)"
    closed_commit: b881a39
  - id: AP6a
    name: Windows helper fixes for external skill sources
    branch: feature/ap6a-windows-helper
    status: done
    owner: null
    depends_on: [LS1]
    # e81694c + review 修正 a04f28a；等 reviewer 複查與 Windows CI。
    # 一併經 integration/skill-parity 驗收並合入 main（見 V050 gate）。
    gate:
      - cmd: "release_check.sh @ integration/skill-parity"
        expect: "exit 0"
        result: "passed: Swift 167/167; Rust passed 3861 @ 4e7e2e4"
      - cmd: "full CI"
        expect: "all jobs success"
        result: "passed: run 37655626569 @ 4e7e2e4 (attempt 2; attempt 1 W5 historical upgrade default_uninstaller_self_copy process_inventory_failed, runner flake)"
    closed_commit: a04f28a
  - id: AP6b
    name: Local admin token for skill/memory management
    branch: feature/ap6b-admin-token
    status: done
    owner: null
    depends_on: [LS1]
    # 決策：獨立本機 admin token（chadex-desktop-admin），Project Memory 也改走它；tunnel token 縮權另案。
    # 78ac710、2b676a8、d61caeb；reviewer 複查無 blocking。
    # 一併經 integration/skill-parity 驗收並合入 main（見 V050 gate）。
    gate:
      - cmd: "release_check.sh @ integration/skill-parity"
        expect: "exit 0"
        result: "passed: Swift 167/167; Rust passed 3861 @ 4e7e2e4"
      - cmd: "full CI"
        expect: "all jobs success"
        result: "passed: run 37655626569 @ 4e7e2e4 (attempt 2; attempt 1 W5 historical upgrade default_uninstaller_self_copy process_inventory_failed, runner flake)"
    closed_commit: 06916eb
  - id: AP6c
    name: Windows Skills UI parity
    branch: feature/ap6c-windows-skills-ui
    status: done
    owner: null
    depends_on: [LS1]
    # e304401；CI 失敗為無關 flaky，已 rerun。
    # 一併經 integration/skill-parity 驗收並合入 main（見 V050 gate）。
    gate:
      - cmd: "release_check.sh @ integration/skill-parity"
        expect: "exit 0"
        result: "passed: Swift 167/167; Rust passed 3861 @ 4e7e2e4"
      - cmd: "full CI"
        expect: "all jobs success"
        result: "passed: run 37655626569 @ 4e7e2e4 (attempt 2; attempt 1 W5 historical upgrade default_uninstaller_self_copy process_inventory_failed, runner flake)"
    closed_commit: 3d0dffe
  - id: LS2
    name: Windows runtime prewarm at launch
    branch: feature/w-runtime-prewarm
    status: done
    owner: null
    depends_on: [AP6c, LS1]
    # 7b2907f（建在 AP6c + speed fixes 之上）。
    # 一併經 integration/skill-parity 驗收並合入 main（見 V050 gate）。
    gate:
      - cmd: "release_check.sh @ integration/skill-parity"
        expect: "exit 0"
        result: "passed: Swift 167/167; Rust passed 3861 @ 4e7e2e4"
      - cmd: "full CI"
        expect: "all jobs success"
        result: "passed: run 37655626569 @ 4e7e2e4 (attempt 2; attempt 1 W5 historical upgrade default_uninstaller_self_copy process_inventory_failed, runner flake)"
    closed_commit: 7b2907f
  - id: V043
    name: Runtime-engine test cleanup
    branch: chore/v043-runtime-test-cleanup
    status: done
    owner: null
    depends_on: [LS1]
    # 一併經 integration/skill-parity 驗收並合入 main（見 V050 gate）。
    gate:
      - cmd: "release_check.sh @ integration/skill-parity"
        expect: "exit 0"
        result: "passed: Swift 167/167; Rust passed 3861 @ 4e7e2e4"
      - cmd: "full CI"
        expect: "all jobs success"
        result: "passed: run 37655626569 @ 4e7e2e4 (attempt 2; attempt 1 W5 historical upgrade default_uninstaller_self_copy process_inventory_failed, runner flake)"
    closed_commit: 9313f50
  - id: V050
    name: v0.5.0 release (Skills page, external sources, launch speed)
    branch: integration/skill-parity
    status: in_progress
    owner: claude
    depends_on: [AP5, LS1, AP6a, AP6b, AP6c, LS2, V043]
    # 驗收修正：ZIP 任意位置匯入＋自動放平、獨立 Skills 頁、啟用開關＋移除、
    # Project Memory limit、預熱後對齊選中專案、啟用重播、Skills 啟動載入。使用者驗收通過（2026-10-07）。
    gate:
      - cmd: "release_check.sh"
        expect: "exit 0"
        result: "passed: Swift 167/167; Rust passed 3861 @ 4e7e2e4"
      - cmd: "full CI"
        expect: "all jobs success"
        result: "passed: run 37655626569 @ 4e7e2e4 (attempt 2; attempt 1 W5 historical upgrade default_uninstaller_self_copy process_inventory_failed, runner flake)"
      - cmd: "manual acceptance (dist/Chadex.app)"
        expect: "Skills page, ZIP import, enable/remove, script gate, Project Memory, connect speed"
        result: "passed: user acceptance 2026-10-07 @ 9f708ea (Skills) and @ d5f2042 (project switch/remove, broad-folder warning); diagnostics Chadex-Diagnostics-20261007-025823"
    closed_commit: null
  - id: D1
    name: macOS design v2 (Liquid Glass sidebar, connection circuit, activity timeline, dark default)
    branch: feature/d1-design-v2
    status: pending_validation
    owner: claude
    depends_on: []
    # PR #2。只有 AI 評審，沒有外部設計驗證，也沒有使用者實機驗收；不可宣稱得獎水準。
    gate:
      - cmd: "release_check.sh"
        expect: "exit 0"
        result: null
      - cmd: "manual acceptance (macOS)"
        expect: "design v2 visual review by the user"
        result: null
    closed_commit: null
  - id: W-design
    name: Windows design v2 (Fluent type, connection circuit, activity timeline, Mica, dark default)
    branch: feature/w-design-v2
    status: in_review
    owner: claude
    depends_on: []
    # PR #1。只有 CI 驗證，沒有 Windows 實機測試。
    gate:
      - cmd: "full CI"
        expect: "all jobs success"
        result: null
    closed_commit: null
  - id: V060
    name: v0.6.0 release prep (version bump, changelog, release notes)
    branch: chore/v0.6.0-prep
    status: in_progress
    owner: claude
    depends_on: [D1, W-design]
    # 預計包含 PR #1–#7；#8（游標疊加層）與 #9（Chromium 網頁 AX）原規劃 v0.6.1，視合併情況再決定。
    gate:
      - cmd: "release_check.sh"
        expect: "exit 0"
        result: null
      - cmd: "full CI"
        expect: "all jobs success"
        result: null
      - cmd: "manual acceptance (dist/Chadex.app)"
        expect: "design v2, Computer Use permission card, #8 screenshot exclusion if included"
        result: null
    closed_commit: null
---

# Phases

> 這份檔案是「現在做到哪」的唯一來源。任何 agent 開工前先讀，收工前更新。
> `phase-handoff` mod 只解析上方 front matter；其餘文字隨意。

## 規則

1. 同一時間每個 phase 只能有一個 `owner`。要接手，先在 `HANDOFF.md` 交接，再改 `owner`。
2. `status: done` 必須同時填 `closed_commit`，且 gate 每一條都執行過。
3. 目前分支不是 `current_phase` 對應的 `branch` 時，先停下來確認。
4. 改 `current_phase` 或任何 `status` 時，同步更新 `updated` 與 `updated_by`。

## 目前位置

`V041` 已完成並發布。正式 `v0.4.1` tag 指向產品 commit `3ef3dd4`；其後 `main` 只有非產品 commit：
`0af8fcb`（release 後 Graphify report refresh）與 `16efcf1`（新增本檔）。`16efcf1` 的 CI run `37306101038`
第一次在 W5 installer candidate 的 `default_uninstaller_self_copy` stage 出現 runner 端 `process_inventory_failed`，
rerun（attempt 2）7/7 jobs success，視為 baseline 綠燈。

`V042` 已完成，2026-10-05 fast-forward 合入 `main` @ `924c75c`：文件與 v0.4.1 架構對齊、移除未使用的
`getProjectInstructions` bridge path、Graphify 排除 provenance/generated material、CI docs-only fast path 與 Rust/npm cache。
Active phase：`AP5 — External skill sources`（branch `feature/ap5-external-skill-sources`）。之後：Windows UI parity / external acceptance。

## V041 · v0.4.1

已實作（依 10/5 紀錄）：

- [x] Global Instructions（Chadex UI 的 AGENTS.md 改為 Chadex Global Instructions，不依賴 ambient ancestors）
- [x] Instructions precedence：Chadex Global → project/repo AGENTS → nested
- [x] Computer Use UI 第二輪（Stop 後只留 Resume、首次連線狀態）
- [x] 重跑上方 gate 並確認結果（最終基線 `0af8fcb`）
- [x] 發版收尾（下方 Release Checklist）

## Release Checklist（每個發版階段共用）

發版收尾順序固定，每項要有執行證據才能勾：

- [x] tests（Swift 84/84；helper 184 passed / 3 ignored；Computer Use safety 10/10；完整 `release_check.sh` exit 0）
- [x] 本機 app 驗證（`/Applications/Chadex.app` = 0.4.1 Build 1，已可啟動並運行；V041 安裝／升級驗收已完成）
- [x] Graphify sync（v0.4.1 source `3ef3dd4`；60,141 nodes / 177,154 edges / 1,840 communities；`0af8fcb` 僅刷新 report）
- [x] Obsidian docs sync（68 outputs；最終 `--check` = `changed=0`, `stale_generated=0`）
- [x] GitHub CI 通過（post-release `main` run `37290160357` = success）
- [x] release（public tag `v0.4.1` / `3ef3dd4`；Free Release run `37287481893` = success；macOS ARM64 DMG + SHA-256 已發布）

## 備註

- `release/v0.3.0` 至 `v0.3.2` 與 tag `v0.2.1`～`v0.4.0` 是版本發布，不另列為階段。
- `archive/*`、`wip/*` 分支不屬於階段。
