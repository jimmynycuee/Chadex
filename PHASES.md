---
schema_version: 1
project: chadex
canonical_branch: main        # 所有 agent 以此為準；階段分支完成後才合回
updated: 2026-10-05T23:30+08:00
updated_by: claude           # codex | webcodex | chadex | claude | human
current_phase: null          # null 代表目前沒有 active phase；下一階段建立後再填入
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
目前沒有 active phase。待決定：`AP5 — External skill sources`、Windows UI parity / external acceptance。

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
