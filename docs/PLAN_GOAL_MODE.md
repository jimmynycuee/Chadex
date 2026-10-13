# Plan Mode 與 Goal Mode

本文件及匯出的 `chadex-plan`／`chadex-goal` Skills 對應 Chadex v0.7.1。此版本的兩種入口省略 `project` 時都使用 Chadex App 目前開啟的專案；舊版若仍要求明確 `project`，此預設不適用，必須傳入專案。匯出依賴仍是 `chadex` MCP connector，並由使用者的 Tunnel ID 產生連線 URL。

兩個獨立的功能：

- **Plan Mode**：以**後端強制唯讀**的 Plan Mode 規劃 → 使用者在 App 核准**確切版本**（「核准，讓 ChatGPT 開工」）→ ChatGPT 在同一則回覆裡（`plan_wait_approval` 拿到 `approved`）直接在專案中實作並跑驗收。不建立 Goal、不使用 Codex。
- **Goal Mode**：在 ChatGPT 給目標與驗收指令 → Chadex 記下請求並通知 → 使用者在 App 選模式開始：
  - **Codex 模式**：Chadex 在背景用 Codex CLI 一輪輪修改直到驗收通過，可關閉 ChatGPT；**會消耗使用者的 Codex token／額度**。
  - **ChatGPT 模式**：ChatGPT 在對話中自己修改 Goal 的隔離 worktree，每輪呼叫 `goal_check`，Chadex commit 並跑核准的驗收，沒過就叫它繼續；不消耗 Codex 額度，但對話要開著。

模型永遠不能自己啟動 Goal：開始一定要在 App 按一下，模式也由使用者在 App 選。

## 使用方式（ChatGPT）

| 想做的事 | 在 ChatGPT 輸入 | 實際入口 |
| --- | --- | --- |
| 規劃、不動程式 | 「`@chadex-plan` 規劃 ○○，先不要改程式」 | `plan_start`（project 省略時用 App 目前開著的專案）→ 唯讀調查 → `plan_save` → `plan_wait_approval`（每次最多等 45 秒，同一則回覆內重複呼叫） |
| 核准計畫並開工 | **不在 ChatGPT**：App「Plan & Goal」（⌘6）→「核准第 N 版，讓 ChatGPT 開工」 | `plan_review` `approve`（只限 App）；ChatGPT 的 `plan_wait_approval` 回 `approved` 後直接實作。若 ChatGPT 的回覆已結束，回去說「開始」 |
| 直接給目標 | 「`@chadex-goal` 修好 ○○，驗收用 `python3 -m unittest`」 | `goal_run_start`（title、objective、acceptance；project 省略時用 App 目前開著的專案）→ 狀態 `awaiting_start` |
| 開始 Goal | **不在 ChatGPT**：App →「用 Codex 在背景執行」（消耗 Codex token）或「在 ChatGPT 對話中執行」 | `plan_review` `approve_and_start` + `mode`（`codex`／`chatgpt`，只限 App） |
| ChatGPT 模式的每一輪 | ChatGPT 自己改 `workspace_project`，再呼叫 `goal_check` | `goal_check`：commit + sandbox 驗收；`achieved` 為 true 才算完成。30 分鐘沒呼叫會暫停（`chatgpt_idle`），下一次 `goal_check` 恢復 |
| 查進度／暫停／取消 | 「查 Chadex Goal 進度」 | `goal_run_status`（`wait_secs` 最多等 45 秒）、`goal_run_list`、`goal_run_control` |
| 得知完成 | Goal 結束（成功、失敗、需要處理、取消）時 App 跳 macOS 通知。ChatGPT 不會被通知，要問它才會查 | App 每 10 秒讀 `goal_operator` overview |
| 緊急停止 | Chadex App「緊急停止所有 Goal」 | `goal_operator`（只限 App） |

- 初始 `tools/list` 只直接列出 Plan/Goal 的入口 `plan_start`、`goal_run_start`；其他工具依 `tool_manifest` 回報的 route 呼叫。`gateway` 走 `call_runtime_tool`，`direct` 工具若未載入也可用 gateway fallback。找不到工具時先查 manifest，不推定 connector 快取過期。因此**即使沒有 Skill，一般文字指令也能完整使用**。
- `plan_save` 會回傳帶有確切 `plan_id` 和 `wait_secs` 的 `plan_wait_approval` `suggested_call`；`waiting` 會再提供同一 Plan 的觀察呼叫。`approved` 才開始實作 Plan；`closed`、`approval_withdrawn` 不得實作；Goal-origin 請求改用 `goal_run_list`／`goal_run_status`，不呼叫 `plan_wait_approval`。
- 觀察呼叫只有 server 同時明確回報 `execution_state: not_started`、`recovery_kind: retry`、`retry_after_secs` 才可重試；以 5 秒為間隔，遵守更長的 server delay，總計最多 60 秒，保持同一 identity 與參數。Auth/security 拒絕及 `user_action` 不重試。寫入或 `outcome_unknown` 先用 Plan／Goal 的讀取工具對帳，不盲目重送。
- Skills：`skills/chadex-plan/SKILL.md`、`skills/chadex-goal/SKILL.md`。OpenAI 官方文件說明 ChatGPT 可用 `@` 呼叫 Skill，所以上傳後可用 `@chadex-plan`／`@chadex-goal`。它們也能以 ZIP 上傳到 Chadex Skills Center（managed skill），由 ChatGPT 透過 `skill_load` 讀取。**`@plan`／`@goal` 這種短名稱未驗證**：需把 Skill 改名為 `plan`／`goal`，且可能與其他 Skill 衝突；ChatGPT 個人方案的 Skill 上傳位置本次也未實測。
- **匯出 Skill（必做）**：Skill 要在 `agents/openai.yaml` 宣告 Chadex MCP 連線，ChatGPT 才拿得到 Chadex 工具，而網址含你個人的 Tunnel ID。因此請用 App「Plan & Goal」頁底的「匯出 ChatGPT Skills…」選資料夾，產生 `chadex-plan.zip`、`chadex-goal.zip`，再到 ChatGPT「設定 → Skills」上傳並取代舊的。zip 含個人通道位址，請勿分享。需先在設定填好有效的 Tunnel ID。
- Skill 只是程序說明，不是權限；所有限制都在後端。

## 架構

```text
ChatGPT ──MCP(tunnel, bootstrap token)──► runtime kernel ──► Runner
   │ plan_start/plan_save/goal_run_start          │
   │                                     PlanGuard（每次派送前）
Chadex App ──bridge planGoalOperator──► helper ──REST /api/tools/call(user token)──► plan_review / goal_operator
                                                   │
                                     GoalSupervisor（runtime server 內的背景迴圈）
                                       └─ kernel run_process（受限 principal）→ Runner → managed worktree
```

重用既有系統，不另建資料庫或執行器：同一個 SQLite（`webcodex.db`）、同一個 tool kernel 與 Runner 授權、既有 `prepare_managed_worktree`、既有 Job 觀測（`run_process` / `observe_jobs` / `stop_job`）。

### 資料（`chadex-runtime-store`：`plan.rs`、`goal_run.rs`）

- `wc_plans`：owner、project、state（drafting/approved/closed）、`latest_revision`、Plan Mode lock（每個 owner 最多一把，6 小時逾時）。
- `wc_plan_revisions`：不可變；SQLite trigger 拒絕 UPDATE/DELETE；讀取時重算 `content_sha256` 檢查完整性。hash 涵蓋 Markdown 內容、驗收指令、預算。
- `wc_plan_decisions`：approve/reject/revoke/supersede/expire/consume。approve 綁定 revision、hash、project、Runner client、project root、Git HEAD、到期時間；同一 plan 只能有一筆 active（unique index）。
- `wc_goal_runs`、`wc_goal_run_ops`、`wc_goal_run_events`、`wc_goal_kill_switch`：run、操作日誌、稽核事件、持久緊急停止。

### 決策紀錄

- **鎖綁定 principal，而非 ChatGPT 對話**：ChatGPT 不提供可驗證的對話身分。principal key = sha256(credential kind, token kind, subject)，不含 token id（OAuth 換發不會讓鎖失效），並區分 ChatGPT 連線與 App 操作者。
- **預設拒絕的白名單**：只放行審查過的唯讀工具（檔案讀取／搜尋、Git 讀取、Memory、Skills、Job 觀測）。LSP 不放行（language server 可能在專案內建置）。新加的工具自動被拒。
- **核准只走 App**：ChatGPT 經 tunnel 以 runtime **bootstrap** token 連線；操作者工具要求 REST transport **且** `ApiToken`（token kind `user`），bootstrap 與任何 MCP 呼叫都會被拒，模型文字無法冒充。
- **Goal 用 `codex exec`，不用 ACP**：本機沒有安裝 ACP adapter（現有 ACP 路徑需 `npx` 臨時下載 `codex-acp`），而 `codex exec` 已安裝且沿用使用者的 Codex 登入；以既有 Runner Job 執行，不需新的執行器。可用 `CHADEX_GOAL_AGENT_ARGV`（JSON 陣列，prompt 走 stdin）換成其他 CLI。
- **隔離 worktree + 結果分支**：從核准時的 HEAD 建 managed worktree，每輪 checkpoint commit，成功時建立 `chadex/goal/<run_id>`。使用者主工作樹不動，也不自動合併／推送。

## 安全與正確性

### Plan Mode

- 檢查點：kernel `call_tool_with_context_inner` 一開始（在 Computer/Plugin/SSH specialized gateway 之前，以工具名稱檢查）、參數解析後（限定 plan 的 project），以及 MCP `handle_call` 在 `mcp_tool`／`plugin_tool`／`ssh_resource` 直接派送之前。Code Mode、task executor 的巢狀呼叫都會重新進入 kernel。
- 鎖只能由 App 核准、App「要求修改」以外的關閉（close），或逾時解除；模型無法解除。鎖定期間 `goal_run_start` 與 resume 也被拒。
- Plan 狀態讀取失敗時 guard 進入 degraded，所有非唯讀工具 fail closed。
- admin／bootstrap 權限不能繞過 Plan Mode（測試以 admin scope 窮舉所有工具）。

### 版本與核准

- 只有最新 revision 可審核；送出時須附顯示中的 revision 與 hash（`plan_revision_stale`、`plan_content_hash_mismatch`）。
- 直接 Goal 是只有一版、`origin = goal` 的 plan：建立時**不上 Plan Mode 鎖**、不能修訂（`goal_request_immutable`，要改就重新呼叫 `goal_run_start`，舊的未開始請求自動關閉）、不能「要求修改」（避免把擁有者鎖成唯讀，改用捨棄）。建立時記下受限的 Goal dispatch principal；Plan Mode 中不能建立。
- `approve_and_start`：核准前先檢查所有可能拒絕的條件，包括：擁有者是否正在 Plan Mode 規劃別的 plan（`goal_owner_in_plan_mode`）、有沒有為**最新版**記錄的 dispatch principal 且不超過 7 天（`goal_dispatch_unknown`）、緊急停止、以擁有者記錄的 principal 重新解析專案時，Runner／root 是否與操作者看到的相同。之後才核准（綁定 HEAD）、以 plan 擁有者身分 consume 核准並建立 run。若啟動仍失敗，剛建立的核准會被撤回（`approval_withdrawn`），不會留下可被啟動的核准。`plan_save` 每次都會為該版重新記錄（或在算不出來時清除）dispatch principal，所以一般計畫也能一步核准並開始，且不會沿用舊版或過期的權限。
- 儲存新 revision 會 supersede 舊核准並重新上鎖；要求修改會撤銷 active 核准並重新上鎖；核准單次使用（App 的 `approve_and_start` 在同一交易內 consume；模型的 `goal_run_start(plan_id)` 只會找已開始的 run，永遠不會 consume），重複 start 回放同一個 run。
- Goal 啟動時 Runner／root 必須與核准時相同（`plan_approval_environment_changed`）；核准需至少一個驗收指令。

### Goal 執行

- 每輪：agent → `git add -A` → `git commit` → 驗收。成功**只**看核准的驗收指令 exit code；agent 自稱完成不算數。
- Chadex 自己的 git 指令：在 agent 第一次執行前先記錄並驗證 worktree 的 git dir（必須位於 `<project>/.git/worktrees/`），之後一律帶 `--git-dir=<已記錄路徑> --work-tree=. -c core.hooksPath=/dev/null -c core.fsmonitor=false`。被改寫的 `.git` 檔無法重新導向，也不會執行任何 repository hook（E2E：測試 repo 的 `post-commit` hook 沒有被觸發）。
- Agent：`codex exec --ignore-user-config --ignore-rules --ephemeral -s workspace-write -c approval_policy="never" -c sandbox_workspace_write.network_access=false`。不載入使用者 config 中的 plugins、MCP、connectors（例如 Gmail、GitHub），無網路，只能寫 worktree。
- 驗收指令執行的是 agent 寫的程式碼，所以也在 `codex sandbox`（workspace-write、無網路）內執行。實測：sandbox 外連得到 runtime，sandbox 內連線被擋，因此驗收程式碼碰不到 Chadex 的 loopback 操作者 API。可用 `CHADEX_GOAL_CHECK_SANDBOX_ARGV` 覆寫；`[]` 代表操作者明確停用。自訂 provider（`CHADEX_GOAL_AGENT_ARGV`）若沒有設定 sandbox，驗收**不會執行**，run 會進入 `needs_attention`（`acceptance_sandbox_unconfigured`）。每個 run 的事件都會記錄 sandbox 狀態。`codex sandbox` 仍會讀取使用者的 Codex config；Chadex 已覆寫 `sandbox_mode`、`network_access`、`writable_roots`，其他 profile 設定是否會放寬限制尚未驗證。
- 派送身分：一般使用者沿用原身分，scope 限縮為 `runtime:read/project:read/project:write/job:run`。bootstrap（本機 ChatGPT）改用以該 Runner 擁有者為名的合成 `goal_run` principal，不沿用 bootstrap 全權。supervisor 的派送以 `tokio::task_local!` 標記，只能呼叫 `run_process`／`observe_jobs`／`stop_job`／`list_jobs`／`job_tail`。
- 防重複：lease + fence generation，每次寫入都檢查；操作日誌先寫 `intent` 再派送。重啟後若 agent 操作只有 intent 或 job 已遺失，標成 `unknown`，run 進入 `needs_attention`，**不重派**；驗收與 commit 可安全重做。lease 遺失時 driver 立即中止；同一程序不會重複 claim 自己仍在跑的 run。
- 停止與限額：迭代上限、總時間（自 start 起算）、單輪逾時、暫時錯誤有上限重試、同一失敗連續 3 次判定無進展。權限拒絕不重試。pause／cancel／緊急停止在每次派送前與等待中都會檢查，並停止執行中的 job，記錄能否確認已停止。
- `needs_attention` 只能由 App resume（ChatGPT 端拒絕，也不能用 pause→resume 繞過）。待處理的 cancel 優先於任何 park。
- 用量：解析 codex 輸出的 `tokens used`。取不到時顯示 `unknown`，不宣稱零成本。
- 高風險行為（付款、對外發送、部署、推送）沒有任何通道：agent 與驗收都無網路、不載入 connectors，也不能 commit 或 push；Chadex 不會合併或發佈。

### 信任邊界（請務必理解）

- **核准與操作者限定，是防止模型誤操作的流程控制，不是對模型的安全邊界。** 在 Plan Mode 以外，ChatGPT 連線擁有一般 Chadex 權限（bootstrap：shell、檔案），所以能讀取 App 的 user token（`webcodex-user-token`）或直接寫 SQLite，進而偽造核准。要防止這件事需要 OS 層隔離，例如 Keychain ACL，或由 App 持有、並經驗證 peer credential 的 Unix socket 通道。
- **Plan Mode 鎖定期間，這條路徑被後端擋住**：沒有 shell、沒有任何寫入工具，因此規劃階段無法偽造核准。
- Goal 執行期間，agent 與驗收都在無網路 sandbox 內，碰不到 loopback 操作者 API（E2E 實測）；Chadex 自己的 git 指令不執行 repository hook。
- 多使用者／遠端部署：任何 owner user token 都視為操作者，可看到並控制所有 plan 與 run（Chadex 目前是單人本機產品）。
- 派送身分在 run 期間不會重新驗證（撤銷 token 不會停止既有 run）；請用 cancel 或緊急停止。
- `plan_start` 之前已在跑的 job 不會被停止；之後它們發出的新工具呼叫會被拒。
- status 會把驗收與 agent 輸出的尾段（各 4 KB）回傳給 ChatGPT；測試若印出機密，機密也會一起回傳。

## 生命週期（實測）

| 情境 | 行為 |
| --- | --- |
| 關閉 ChatGPT 網頁／不再發訊息 | Goal 照常完成（E2E：`goal_run_start` 之後沒有任何 MCP 呼叫） |
| 關閉 Chadex App | helper、server、Runner 一起停止；run 持久保存。重開後約 60 秒（lease 到期）接手；被中斷的 agent 判定 `unknown` → `needs_attention` → App 審查後 resume 繼續（E2E 實測，fake 與真 codex 皆通過） |
| Mac 睡眠 | 程序暫停；醒來後同一程序繼續持有 run（不重複 claim）。**未實機測試** |
| Mac 關機 | 不支援執行（設計範圍外）；開機並開啟 Chadex 後依上一列恢復 |

### 清單只留最新的（App「Plan & Goal」）

- 同一個身分在同一專案開新的 Plan 或直接 Goal 請求時，舊的未結案計畫會自動結案（撤回尚未使用的核准、解除 Plan Mode）。
- 開始新的 Goal 時，同專案已結束的 Goal 會從清單移除；Goal 成功時，對應計畫自動結案（期間若存了新版本則不結案）。
- **清空**：App 每次啟動（runtime 就緒後一次），以及 ChatGPT 從「已連線」變成斷線（同一專案；切換專案不算）時，呼叫 `goal_operator clear_board`：所有未結案的 Plan 與 Goal 請求結案，已結束的 Goal 從清單移除。**執行中的 Goal 繼續跑完**，完成後照常通知。
- 手動：Plan 可「刪除計畫」，已結束的 Goal 可「刪除」（執行中的要先取消）。
- 以上都只是結案或從清單移除，資料庫紀錄與結果分支 `chadex/goal/<run_id>` 都保留，不會真的刪除。

## 測試證據

- Store（`plan_goal_tests.rs`，12 項）：revision 不可變（trigger）、CAS、diff、核准綁定、v1→v2 舊核准失效、競爭核准、單次使用與回放、過期、撤銷、環境變更、鎖、lease fencing、操作日誌、控制、緊急停止，以及 review 回歸（H1、M1、L1、M5）。
- Runtime（`tests/plan_mode.rs`、`tests/goal_mode.rs`，20 項）：對**每一個**非白名單工具窮舉拒絕（含 admin）、gateway 工具、principal 隔離、操作者限制（MCP、bootstrap、OAuth）、kernel 端到端核准與 Goal 入場、scope 限縮；supervisor 的自我修復、自稱完成不算數、無進展、預算、期限、重啟後不重派、已記錄 job 的對帳、3 個並行 supervisor 只派送一次、權限拒絕不重試、暫時錯誤上限、取消／暫停／緊急停止。
- 真實堆疊 E2E（`scripts/e2e_plan_goal.py`，隔離的 helper + server + Runner，ChatGPT 以 MCP + bootstrap 模擬）：Plan Mode 15 種寫入／執行／gateway 嘗試全部被拒，專案（含 `.git`）前後 hash 相同；ChatGPT 自我核准被拒；舊版核准被拒；以真實 `codex exec` 修好 bug 並通過 sandbox 驗收；結果分支在外部獨立重驗通過；關閉後重開可恢復且不重派。
- 未驗證：ChatGPT 網頁實機操作（tunnel、真實對話、Skill 上傳與 `@` 呼叫）；App 新面板的實際畫面（只有編譯與模型解碼測試）；Mac 睡眠；Codex 以外的 provider；大型或耗時多輪的真實任務。

## 遷移與回復

- 只新增資料表（`CREATE TABLE IF NOT EXISTS`），不修改 Plan/Goal 以外的既有表。`wc_plans` 之後以 `ALTER TABLE ADD COLUMN` 加了 `origin`（預設 `plan`）與 `dispatch_json`，舊版本會忽略這兩欄。舊版本會忽略這些表，所以回退舊版本是安全的；回退後沒有 Plan Mode 強制。
- ChatGPT 模式：worktree 建好後才會出現 `workspace_project`，但 `.git` 路徑在第一次 commit 前才釘選，中間有很短的時間窗；模型本來就能寫原專案，所以不構成提權。
- 回退到不認得 `origin` 的舊版本時，直接 Goal 請求會被當成一般 plan 顯示；在舊版上對它「要求修改」或由模型 `plan_save` 會重新上 Plan Mode 鎖。回退前建議先捨棄尚未開始的 Goal 請求。
- `CHADEX_GOAL_SUPERVISOR=0` 停用 supervisor；run 保持持久狀態，之後再啟用即可繼續。
- 緊急停止持久保存在 DB，重啟後仍有效，只有 App 能解除。
