# W4 進度與交接

W4 的 Graphify／Obsidian source-converged 工作已完成並通過本機同步與結構驗證。W3 完整產品驗收仍 pending，W5 尚未開始。

## 來源

- 分支：`codex/w4-source-converged`。
- W3 final baseline：`dd7d5ebbc4bf06c91ebe3a86a8e52c2d38fc52c3`；[CI 37029362354](https://github.com/jimmynycuee/Chadex/actions/runs/37029362354) attempt 2 已查核，四個 jobs 與 W2／W3 gates 全成功。
- W4 implementation／graph source：`f9ba10bebf1ec1ceb4cb17e90e50c3a21b9c830e`。
- 首次 closeout commit 保存此交接、sanitized validation 與 Graphify report；後續 CI correction 修正 shared process-tracking harness，最終 SHA／CI 與 refreshed graph provenance 由交接回覆及 Vault closeout record 提供，不將 W3 CI 當成 W4 CI。
- [W4 範圍與重現指令](W4-source-converged.md)、[本機驗證投影](evidence/W4_knowledge_validation.json)。

## 已完成

1. 同步器加入 Windows TS／TSX、Tauri、bridge、Credential Manager／preferences；與既有 Swift/helper/runtime 使用同一套 production mapping。
2. Generated output 使用 deterministic sorting 與 Vault-qualified paths；修正歷史同名筆記歧義與 Canvas file references。Tests／smoke-only／declarations／build/dependency outputs 不納入 curated production notes。
3. 正常 `graphify update .` 完成：**59,454 nodes／175,118 links／1,834 communities**。13 個 Windows component anchors 皆有 graph nodes 與 generated file notes；graph traversal 可連到 existing helper／tunnel／verification，vendor 同名節點須由 source_file 區分。
4. Obsidian 同步 **17 components／48 curated source files／68 generated outputs**，新增或更新 **9 份人工筆記／Canvas**。人工入口為 Windows Track、W2 Runtime E2E、W3 Desktop Product、W4 Source-converged Knowledge；主索引、架構、Phase map／timeline 與人工 Canvas 補入 Windows 邊界。
5. 寫入前備份本次修改與 retired generated files；寫入前檢查 hash，避免覆盖並行更動。15 份不再選入的 managed generated notes 退出 current layer，177 份其他既有檔案逐檔 hash 保持一致；歷史 archive 未刪除。備份保留在 ignored `local-backups/w4-knowledge-20261003/`，Graphify safety snapshot 也保留為 ignored local data。
6. 修正舊 W2 文件將 W4 誤寫為 distribution 的描述；W4 是知識來源同步，release/distribution 屬 W5。補上 W3 final-HEAD CI provenance。

## 驗證

- Sync deterministic tests：**6 passed**，包含 source inclusion/exclusion、disconnected anchors、graph input ordering、三種 hash seeds、Vault paths、idempotence／drift detection。
- Staged 與 live Vault：**77 files**、**65 source references**、**11 commit references**、**539 wikilinks**、**2 Canvas** 檢查皆無錯誤。
- Generated `--check`：`changed=0`、`stale_generated=0`。
- `graphify check-update .`：exit 0；無 `needs_update` flag。此項是 pending-update signal，不單獨證明來源完整。
- 初始 graph metadata 與 implementation SHA 相符；CI correction 後須正常 refresh graph 並同步 Vault，保留實際 source SHA，不能手改 graph metadata 代替 refresh。Final closeout record 保存新的 provenance；上方數字與本機驗證投影記錄初次同步。
- `git diff --check` 通過；source release gate 沿用原 CI 的 sync test entry。CI correction 改動 shared Windows harness，必須重新執行 Windows W2／W3 acceptance；本機 macOS harness tests 不取代 native Windows acceptance。

## 限制與下一步

### CI completion follow-up

Initial W4 HEAD `0f4f734b4fd1cff478b1a7de1d87e85b4ac61987`, run `37039349458` attempt 1, passed source, ARM64 package and secret scan. Windows core and W2 runtime E2E also passed, but desktop smoke failed at `workflow.shutdown` (`desktop_shutdown_not_clean`): 149 aggregate observed identities, 122 reported remaining. Cleanup then failed and PowerShell reported a Win32 console pipe error; this is not accepted as an infrastructure-only failure or a successful desktop result. Attempt 2 was cancelled after a deterministic tracking defect was reproduced, to validate the corrected source instead.

Windows retains a creator PID after that creator exits. The harness previously followed bare `ParentProcessId` relations, so an older unrelated process could be attributed to a later owner of the same PID. Traversal now requires child creation time to be at least the current parent's creation time, using the inventory's fixed-width UTC timestamps. Missing creation identity fails closed. Checkpoint helper PIDs must already belong to the observed desktop tree. Both deliberate termination and fallback cleanup acquire a process handle, compare its creation identity at the observed CIM microsecond precision and terminate that same handle; no bare-PID `taskkill` or unverified `/T` expansion remains. Shutdown polling tracks newly observed descendants of live owned identities; an unobserved child first seen after its creator exits fails closed as ambiguous rather than producing a false zero or being adopted for cleanup. The existing zero-residual/zero-fallback acceptance criteria are unchanged; production Job Object ownership is unchanged.

Two initial regression cases failed before the correction. Independent review exposed additional pre-existing identity-adoption, termination-race and late-child gaps; targeted regressions now cover those paths, including preservation of a tracked child and grandchild after parent PID reuse. The corrected local suites pass: runtime harness 27, desktop harness/resource preparation 15, sync 6. This establishes the harness defects and corrections; the sanitized first-run report alone does not prove which unrelated process identities were captured. CIM timestamp precision and same-handle termination follow [CIM_DATETIME](https://learn.microsoft.com/en-us/windows/win32/wmisdk/cim-datetime) and [GetProcessTimes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes). Native CI and actual final E2E artifacts remain required before W4 closeout. The final handoff reply and Vault closeout record identify the exact corrected SHA/run, avoiding a self-referential evidence commit.

Graphify 警告 85 個 zero-node files，主要為資料／診斷 JSON；13 個 Windows production anchors 無缺漏。Community label set 隨 clustering 改變，部分 names 使用 hub fallback；未呼叫付費 semantic extraction／labeling，不能宣稱完整文件語意圖譜。Obsidian reading-view UI 本次 **not validated**；已驗證檔案、YAML、連結與 Canvas 結構／路徑。

W3 的 `native_picker`、`explorer_open`、`tray`、`launch_at_login`、`notifications`、`credentialed_tunnel` 仍全部 **not validated**。使用者沒有 Windows 主機；CI debug artifact、isolated Credential Manager roundtrip 與 local MCP 不取代實機或 credentialed ChatGPT workflow。Installed default resource discovery、Windows 11／ARM64 hardware、signing、installer、updater 亦未 certified。

W4 不 merge `main`、不發布 release。下一階段另行啟動 W5，先定義 release readiness／distribution scope，並補齊上述人工／私密驗收。
