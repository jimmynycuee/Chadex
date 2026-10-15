---
schema_version: 1
project: chadex
canonical_branch: main        # 所有 agent 以此為準；階段分支完成後才合回
updated: 2026-10-15
updated_by: codex            # codex | webcodex | chadex | claude | human
tracks:
  W: Windows 版
  AP: Agents.md與 Skills 功能
  CM: Computer Use
  V: 版本發佈
  LS: 啟動速度
  D: 介面設計
  PG: Plan Mode 與 Goal Mode
current_phase: V072
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
    status: done
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
    closed_commit: dd74d05  # v0.5.0 tag, released 2026-10-08
  - id: D1
    name: macOS design v2 (Liquid Glass sidebar, connection circuit, activity timeline, dark default)
    branch: feature/d1-design-v2
    status: done  # PR #2 merged 2026-10-09, shipped in v0.6.0
    owner: claude
    depends_on: []
    # PR #2。AI 評審加使用者實機驗收，沒有外部設計驗證；不可宣稱得獎水準。
    gate:
      - cmd: "release_check.sh"
        expect: "exit 0"
        result: "exit 0 on eddc98d (2026-10-09, V060)"
      - cmd: "manual acceptance (macOS)"
        expect: "design v2 visual review by the user"
        result: "accepted 2026-10-09 on a physical Mac (docs/releases/0.6.0.md)"
    closed_commit: 7e5aa88
  - id: W-design
    name: Windows design v2 (Fluent type, connection circuit, activity timeline, Mica, dark default)
    branch: feature/w-design-v2
    status: done  # PR #1 merged 2026-10-09, shipped in v0.6.0
    owner: claude
    depends_on: []
    # PR #1。只有 CI 驗證，沒有 Windows 實機測試。
    gate:
      - cmd: "full CI"
        expect: "all jobs success"
        result: "success in the v0.6.0 release CI (V060); no physical Windows testing"
    closed_commit: b1b912e
  - id: V060
    name: v0.6.0 release prep (version bump, changelog, release notes)
    branch: chore/v0.6.0-prep
    status: done
    owner: claude
    depends_on: [D1, W-design]
    # 實際包含 #1–#15、#17、#18；#8、#9 也在 0.6.0。2026-10-09 發布，tag v0.6.0 -> 0864465。
    gate:
      - cmd: "release_check.sh"
        expect: "exit 0"
        result: "exit 0 on eddc98d (2026-10-09)"
      - cmd: "full CI"
        expect: "all jobs success"
        result: "success on eddc98d; Windows core gate W3 flake process_ownership_ambiguous passed on rerun"
      - cmd: "manual acceptance (dist/Chadex.app)"
        expect: "design v2, Computer Use permission card, #8 screenshot exclusion if included"
        result: "accepted 2026-10-09 (local build); credential_push ok, overlay not in recording, Brave AX ok"
    closed_commit: 0864465
  - id: V063
    name: v0.6.1–v0.6.3 patch releases (button labels, capsule shape, macOS 26 SDK)
    branch: ci/macos-26-sdk
    status: done
    owner: claude
    depends_on: [V060]
    # 0.6.1 (#20, #22)、0.6.2 (#23) 在 macos-15 建置、連結 SDK 15.5，正式版外觀未生效；
    # 0.6.3 (#24) 改用 macos-26 並在 release.yml 檢查 SDK >= 26。外觀驗收須用 CI 建置的 App。
    gate:
      - cmd: "release_check.sh"
        expect: "exit 0"
        result: "exit 0 on 6c2f058 (0.6.1); 0.6.2/0.6.3 relied on the release workflow source gate"
      - cmd: "Free Release workflow"
        expect: "all jobs success, linked SDK >= 26"
        result: "v0.6.3 run 38044443784 success, SDK 26.5"
      - cmd: "manual acceptance (CI-built app)"
        expect: "capsule buttons, centred labels"
        result: "accepted 2026-10-10 on v0.6.3"
    closed_commit: 2435154
  - id: V064
    name: v0.6.4 Chadex glass buttons that follow the interface size
    branch: fix/glass-button-style
    status: done
    owner: claude
    depends_on: [V063]
    # macOS 26 SDK 的原生 bordered 按鈕不理會 .font、幾乎不隨 controlSize 變大；
    # 改用自訂 ChadexButtonStyle（Liquid Glass 膠囊），80%→160% 量測 62x22→120x43。
    gate:
      - cmd: "native button measurement"
        expect: "label/bezel grow with interface size"
        result: "custom style 62x22 (80%) -> 120x43 (160%); native 76x24 -> 88x36 only"
      - cmd: "Free Release workflow"
        expect: "all jobs success, linked SDK >= 26"
        result: "v0.6.4 run 38056253998 success, SDK 26.5"
      - cmd: "manual acceptance (CI-built app)"
        expect: "buttons scale 80-160%, light/dark, no jump while connecting"
        result: "accepted 2026-10-10 on preview run 38052007198"
    closed_commit: 086c220
  - id: CM6
    name: Computer Use web accessibility lease and usage hints
    branch: fix/cm6-chatgpt-browser-and-hints
    status: done
    owner: claude
    depends_on: [CM5, V064]
    # 使用者回報：Brave 裡的 ChatGPT 在 computer use 後變卡；ChatGPT 不會主動用 computer use；
    # 使用者手動切前景造成 control 結果不穩。AXManualAccessibility 改為閒置 120 s 後復原，
    # 並補強 computer_observe／computer_control 工具描述。
    gate:
      - cmd: "cargo test -p chadex-runtime-computer / -p chadex-runtime-tool-contracts / -p chadex-runtime-engine --lib"
        result: "172 / 141 / 2754 passed"
      - cmd: "CI DMG v0.6.5-preview (Free Release run 38074983512) installed to /Applications"
        expect: "ChatGPT uses computer use on its own; Brave ChatGPT not sluggish afterwards"
        result: "accepted 2026-10-11: ChatGPT drove Brave via computer_control to completion (13m52s); user reports no lag afterwards. AXManualAccessibility=false not read back directly (shell lacks AX permission)"
    closed_commit: 405e72e
  - id: V065
    name: v0.6.5 Computer Use web accessibility lease and usage hints
    branch: fix/cm6-chatgpt-browser-and-hints
    status: done
    owner: claude
    depends_on: [CM6]
    # release commit 6d3a027 shipped in PR #28 together with CM6; tag v0.6.5 -> main bbcc867.
    gate:
      - cmd: "PR #28 CI (run 38079743666)"
        result: "all 10 jobs success after rerunning a Windows W5 process_inventory_failed flake"
      - cmd: "Free Release on tag v0.6.5 (run 38085501827)"
        result: "success; DMG + .sha256 published, SDK 26.5, installed to /Applications"
    closed_commit: bbcc867
  - id: PG1
    name: Plan Mode and Goal Mode (backend enforcement, approval, supervisor, app panel)
    branch: feature/plan-goal-mode
    status: in_progress
    owner: claude
    depends_on: [V065]
    # 設計與證據：docs/PLAN_GOAL_MODE.md。尚未完成：ChatGPT 網頁實機（tunnel、Skill 上傳與 @ 呼叫）、
    # App「計畫與目標」面板目視驗收（CI 建置）、Mac 睡眠實測、合併 main 與發版（需使用者授權）。
    gate:
      - cmd: "cargo test -p chadex-runtime-engine --lib / -p chadex-runtime-store / contracts / rust-helper"
        result: "2777 / 180 / 141+96 / 333+3 passed, 0 failed (2026-10-11)"
      - cmd: "swift test"
        result: "250 executed, 1 skipped, 0 failures (2026-10-11)"
      - cmd: "scripts/e2e_plan_goal.py (isolated helper+server+runner) fake / codex / --restart"
        result: "Plan Mode writes all refused, project hash unchanged; real codex goal succeeded on independently re-verified branch; restart -> outcome_unknown -> app resume -> succeeded"
    closed_commit: null
  - id: V070
    name: v0.7.0 Plan Mode, Goal Mode (Codex / ChatGPT) and Computer Use activity notice
    branch: feature/plan-goal-mode
    status: in_progress
    owner: claude
    depends_on: [PG1]
    # 使用者 2026-10-11 指示直接發佈（未先做 CI 預覽版目視驗收與 ChatGPT 網頁實機）。PR #31 合併後推 tag v0.7.0。
    gate:
      - cmd: "cargo test engine --lib / store / contracts / tool-runtime-contracts; swift test; scripts/e2e_plan_goal.py fake"
        result: "2790 / 187 / 141 / 96 passed; 261 executed 0 failures; all E2E steps pass incl. ChatGPT-mode goal (2026-10-11)"
    closed_commit: null
  - id: V071
    name: v0.7.1 Plan/Goal reliability fixes and release
    branch: codex/v071-release
    status: in_progress
    owner: codex
    depends_on: [V070]
    # User approved formal release on 2026-10-13. GitHub PR/CI and tag release remain pending.
    gate:
      - cmd: "./scripts/release_check.sh @ e6e62ab"
        result: "passed exit 0; Swift, Rust runtime/helper suites, and fresh production app build succeeded"
      - cmd: "scripts/test_windows_installer_smoke.py; scripts/test_prepare_windows_release.py; node --test scripts/windows_installed_probe.test.mjs"
        result: "passed: 22 + 14 + 10 tests"
      - cmd: "graphify update .; scripts/sync_graphify_obsidian.py --check"
        result: "passed: 31,330 nodes, 100,759 edges, 833 communities @ 70b5f72; Obsidian generated layer check: 68 outputs, changed=0, stale_generated=0"
      - cmd: "GitHub PR CI and Free Release workflow"
        result: "pending"
    closed_commit: null
  - id: V072
    name: v0.7.2 Goal reliability and macOS/Windows release preparation
    branch: codex/v072-release-prep
    status: in_progress
    owner: codex
    depends_on: [V071, PG2]
    # User authorized push, merge, tag and v0.7.2 publication on 2026-10-15 after required release gates pass.
    gate:
      - cmd: "Goal reliability, Windows tray lifecycle and updater implementation"
        result: "implemented; Goal reliability A–J and Docker Worker/D1 E2E passed; Windows local contracts passed"
      - cmd: "Windows release workflow and signed updater manifest"
        result: "workflow and signature contract implemented; matching updater secret/public variable configured; hosted acceptance pending"
      - cmd: "macOS and Windows release gates"
        result: "local scripts/release_check.sh passed exit 0: Swift 307/1 skipped; helper 364 + 3 integration/4 ignored; runtime-engine 2840/3 ignored; production wrapper, release tooling, fresh optimized macOS app, Hardened Runtime and deep/strict codesign passed; PR CI and tag release gates remain pending"
      - cmd: "graphify update .; scripts/sync_graphify_obsidian.py --check"
        result: "passed: 33,545 nodes, 106,283 edges, 915 communities; Obsidian 69 outputs, changed=0 and stale_generated=0 at current candidate source"
    closed_commit: null
  - id: PG2
    name: Goal start deduplication and execution environment/state reliability
    branch: codex/goal-start-singleflight
    status: in_progress
    owner: codex
    depends_on: [PG1]
    # User reported duplicate effective start clicks on 2026-10-14.
    gate:
      - cmd: "Swift Plan/Goal action regression tests"
        result: "passed: full Swift suite 279 tests, 1 skipped, 0 failures; focused start-state suite 12 passed"
      - cmd: "Rust store and operator repeated/concurrent start regression tests"
        result: "passed: Store 194/194, Plan operator 31/31, Goal 18/18; final exit 0"
      - cmd: "isolated helper/operator E2E with rebuilt debug runtime"
        result: "passed exit 0: baseline 3 runs -> patched 1; same/cross-mode retries created=false, approval IDs unchanged"
      - cmd: "2026-10-15 complete Goal environment/state repair automated validation"
        result: "passed: Store 213, Supervisor 23, Plan 32, contracts 142+96, Swift 307/1 skipped/0 failures, scoped inputs 13, staging 3; native controls 33/33 and Docker actual Worker/D1"
      - cmd: "2026-10-15 final helper/Runner and actual provider integration"
        result: "passed: reliability 32/32 checks; standard Plan/Goal 10/10 assertions; actual Codex one agent dispatch succeeded; no production Goal or source changed"
      - cmd: "candidate native UI and connected ChatGPT acceptance"
        result: "not validated; installed build35 unchanged, no candidate packaged or release published"
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
