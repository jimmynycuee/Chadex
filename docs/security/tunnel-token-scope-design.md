# Tunnel token 縮權設計

日期：2026-10-09。來源基準：`dd74d05`（v0.5.0）。狀態：設計草案，待使用者決定第 6 節的取捨後才實作。本文件只引用程式碼位置與 token 前綴，不含任何 token 值。

範圍：本機 topology（`<data>/runtime/local`）＋ OpenAI Secure MCP Tunnel（helper `tunnel.rs`）。不在範圍內：remote topology（tunnel 本來就要求 loopback runtime，`tunnel.rs:345` 的 `validate_loopback_server_url`）、runtime CLI 的 Cloudflare／regular tunnel、`webcodex share` 的 project-share 模式。

## 0. 摘要

- 現況：ChatGPT 的每個 MCP 請求由本機 tunnel-client 附上 `Authorization: Bearer <WEBCODEX_TOKEN>`，runtime 解析成 `AuthKind::Bootstrap`，`has_scope()` 對任何 scope 都回 true。ingress 擋掉五個管理工具，但這是 helper 端的名稱黑名單，不是權限檢查。
- 已經成立的部分：tunnel 只到得了 runtime 的 `/mcp`，到不了 `/api/tokens/*` 等管理 HTTP API（ingress 只掛 `/mcp`，backend URL 固定）。ChatGPT 端從頭到尾拿不到 token 本身，token 是本機注入的。
- 必須承認的事：`run_shell` 等 `job:run` 工具以同一個 OS 使用者執行、沒有沙盒，可以讀 `webcodex.env`、desktop admin token 檔、pairing user token 檔，再直接打 loopback runtime 的任何 API。**只要 ChatGPT 能用 shell，縮小 token scope 就擋不住已被提示注入、而且會用 shell 的 ChatGPT。**
- 縮權的實際價值：讓「不經 shell 的單一工具呼叫」拿不到管理面、讓 ChatGPT 的身分可以單獨撤銷和限時、讓外洩或殘留檔案的有效期有限、讓稽核能分辨 ChatGPT 和桌面端。真正把 shell 變成邊界的只有「shell 需要人工批准」（方案 D），代價是 UX。
- 建議：先做方案 A（helper 鑄造 tunnel 專用、無 admin、有期限的 PAT，tunnel 啟動時輪替，失敗就不連線），保留 ingress 黑名單當第二層。方案 D（執行類工具批准閘門）做成第二階段、由使用者決定預設值。方案 C 的短效刷新與方案 B 的專案綁定先延後。

## 1. 現況盤點

### 1.1 Token 種類

| Token | 前綴／名稱 | 產生 | 存放 | 生命週期 | 範圍 | 使用者 |
|---|---|---|---|---|---|---|
| Bootstrap `WEBCODEX_TOKEN` | `wc_boot_` | `chadex-runtime-cli/.../tokens.rs:6` `generate_bootstrap_token`；`server init` 已有值就沿用（`webcodex_cli/server.rs:111-118`） | `<data>/runtime/local/webcodex.env`（`runtime_compat/state/coordinator.rs:1713-1714`；`webcodex_cli/env.rs:38`），owner-only 檔 | 永久；沒有輪替機制，要換只能改 env 檔並重啟 runtime | `bootstrap_context()`（`auth/mod.rs:297-304`）：`is_bootstrap=true`，`has_scope()` 一律成立（`auth/context.rs:86-92`） | tunnel（`tunnel.rs:346-347`）、helper 鑄造／撤銷 admin token（`admin_credential.rs:225`） |
| Tunnel authorization 檔 | 內容為 `Bearer <bootstrap>` | `TunnelSession::create`（`tunnel.rs:93-113`） | `<root>/tunnel-sessions/<pid>-<nonce>/mcp-authorization`，0600 | tunnel session 期間；`Drop` 時刪整個目錄（`tunnel.rs:116-120`）。**沒有啟動時清掃**：只有建立點（`tunnel.rs:99`），helper 崩潰會留下含 bootstrap 的檔案 | 同 bootstrap | tunnel-client 以 `--mcp.extra-headers Authorization: file:<path>` 讀取（`tunnel.rs:697-720`） |
| Pairing code | `wc_pair_` | `create_local_pairing`，`--ttl-secs 600`（`runtime_compat/integration/bridge.rs:313-339`） | 只在記憶體／CLI 輸出 | 一次性，60–3600 秒（`pairing_http.rs:26-28`） | 兌換 user token 與 agent token | helper 本機配對 |
| Pairing user token | `wc_pat_`，name `chatgpt-action`，kind `user` | `pairing_enroll`（`pairing_http.rs:386-405`） | connections 目錄的 `user_token_file`（`bridge.rs:374` 的 `--dir`） | `expires_at: None`，永久直到撤銷 | `ENROLL_USER_SCOPES`：`runtime:read`、`runner:manage`、`session:collaborate`、`project:read`、`project:write`、`job:run`（`pairing_http.rs:30-37`） | 桌面端大多數呼叫（`admin_credential.rs:66-80`）。名稱雖然是 `chatgpt-action`，ChatGPT 並不用它 |
| Runner agent token | `wc_agent_` | 同上（`pairing_http.rs:387`） | Runner 設定 | 永久 | `agent:*`，綁 `allowed_client_id`；只能走 Runner transport（`auth/middleware.rs:206-211`） | 本機 Runner |
| Desktop admin token | `wc_pat_`，name `chadex-desktop-admin` | helper 產生，用 bootstrap 呼叫 `/api/tokens/register_hash` 只註冊 hash（`admin_credential.rs:221-276`） | `webcodex.env` 同目錄的 `chadex-desktop-admin-token`（`admin_credential.rs:34, 96-103`） | TTL 30 天（`admin_credential.rs:41`）；401 時重鑄，並撤銷同名舊 token（`admin_credential.rs:281-318`） | 只有 `admin` | 桌面端的 Skill 管理與 Memory 工具（`admin_credential.rs:51-64`） |
| OpenAI control-plane API key | `sk-` | 使用者輸入 | macOS Keychain；Windows credential store（`apps/windows/src-tauri/src/credentials.rs:3-4`） | 使用者管理 | 控制 tunnel 本身，不是 runtime token | tunnel-client 的 `CONTROL_PLANE_API_KEY`（`tunnel.rs:708`） |

runtime 另外支援、但 Chadex 桌面端目前沒用的 credential：project credential（`auth/project_credential.rs`，只在 project-scoped server 由 env 設定）、OAuth2 `wc_oat_`（含 `project_share` subject，`auth/tokens.rs:149-346`）、shared key（`auth/shared_key.rs`）。它們是方案 B 可以參考的積木。

注意：桌面端的 `webcodex.env` 會寫入 `WEBCODEX_SHARED_KEY_ENABLED=true`（`webcodex_cli/env.rs:46`）。因此任何本機程序拿一個不是 `wc_` 開頭的任意字串當 bearer，都會被當成 shared-key principal（`auth/middleware.rs:418-430`），拿到包含 `job:run`、`computer:control` 的 model scopes（`auth/shared_key.rs:51-82`）。這和「本機使用者是可信任的」這個前提一致，但也說明 loopback 上的認證本來就不是邊界。

### 1.2 runtime auth 層怎麼判斷 scope

1. `AuthMiddleware::handle`（`auth/middleware.rs:265`）取出 bearer。project-scoped server 先比對 project credential（`:335-383`，桌面端不走這段），接著跑 verifier chain（`:400`）。
2. Verifier chain（`auth/tokens.rs:363-379`）：`PatVerifier` 先用 constant-time 比對 bootstrap（`auth/tokens.rs:68-70`，`config.rs:568`），比對成功就回 bootstrap context；不然用 SHA-256 查 `api_keys`，檢查使用者是否停用、是否過期，並更新 `last_used_at`（`auth/tokens.rs:79-115`）。之後查 account credential，再交給 `OAuth2Verifier`。
3. `enforce_token_surface`（`auth/middleware.rs:189-238`）依 token 種類限制路徑：agent token 只能走 Runner transport，account credential 只能走 account control，OAuth 不能走 Runner transport。
4. `enforce_route_scope`（`auth/scopes.rs:146-195`）依 `route_metadata` 判斷：`Require(scope)` 檢查 `has_scope`；`Unknown` 只放行 bootstrap。`POST /mcp` 是 `BodyAware(McpToolCall)`（`route_metadata/mcp.rs:20-29`），所以在路由層直接放行，交給工具層檢查。
5. 工具層 `check_runtime_tool_scope`（`tool_runtime/kernel.rs:137-235`）依 ToolDefinition 的 `Require`／`RequireAll`／`RequireAny` 檢查；沒宣告 policy 的工具（`Unknown`）只有 bootstrap 能呼叫（`:226-234`）。
6. `tools/list` 投影（`mcp/tools.rs:71-114`）：非 OAuth principal 只會濾掉 `RequireAny` 工具和 operator extension family；`SkillManagement` 只對有 `admin` 的 principal 顯示（`:103-105`）。所以 ChatGPT 目前在 tool list 裡看得到 `skill_install` 等工具，只是呼叫會被 ingress 擋下。

bootstrap 在 scope 檢查之外，還有以下特殊待遇。改成 PAT 時，這些都會一起改變：

| 位置 | bootstrap | 一般 user-kind PAT |
|---|---|---|
| Runner 可見性 `runner_http/auth.rs:3-31` | `owner_bypass`＋global visibility | 只看得到同 username 擁有的 Runner |
| Observation principal `tool_runtime/session_context.rs:780-816` | `("bootstrap", user_id or "bootstrap")` | `(principal_kind, api_key_id)`，token 換了 principal 就跟著換 |
| Activity scope `tool_runtime/activity.rs:19` | `HostGlobal` | `Unscoped` |
| 未知 MCP method `mcp.rs:1124` | 放行 | 拒絕 |
| Window activity 可見性 `tool_runtime/window_activity.rs:677` | `is_admin_caller()` 為真 | 為假 |
| Host file import trust `mcp/tools.rs:1196-1229` | `NotOAuthToken`，不信任 | kind=`user`、loopback，且 `WEBCODEX_MCP_TRUST_LOOPBACK_API_TOKEN_FILE_IMPORT=true`（桌面端預設會寫入，`runtime_compat/state/policy.rs:88-89`）→ **信任** |

工具與必要 scope（由 `runtime-engine/crates/chadex-runtime-tool-contracts/src/tool_definition/*.rs` 歸納）：

| 類別 | 代表工具 | scope |
|---|---|---|
| 讀取專案 | `read_files`、`search_project_texts`、`git_*` 讀取、LSP、`skill_list` | `project:read` |
| 寫入專案 | `write_project_file`、`apply_patch`、`git_commit_paths`、`register_project`、`create_project` | `project:write` |
| 執行 | `run_shell`、`run_process`、`run_script`、`session_shell_exec`、`cargo_test`、`go_test`、`run_skill_resource`、`code_mode_exec_effectful` | `job:run`；`run_detached_process` 另需 `job:detach` |
| Project Memory | `memory_search`／`memory_read`；`memory_set`／`memory_delete` | `project:read`＋`memory:read`；`project:write`＋`memory:manage` |
| 溝通／Goal | `create_goal`、`*_agent_task*`、`*conversation*` | `communication:read`／`communication:manage` |
| Computer | `computer_observe`；`computer_control`；`computer_save_snapshot` | `computer:read`；`computer:control` 或 `computer:launch`（各 action 另有細項 scope）；`project:write`＋`computer:read` |
| Coding agent | `coding_agent_start` 等 | `coding_agent:run`（`start` 另需 `project:write`） |
| 其他 | `runner_config_reload`；`ssh_resource`；`plugin_tool` | `runner:manage`；`ssh:local`；`plugin:*`（RequireAny） |
| 管理 | `skill_inventory`、`skill_versions`、`skill_install`、`skill_activate`、`skill_deactivate`、`skill_remove_revision`、`memory_scope_list`、`memory_scope_purge`、`read_tool_trace` | `admin` |

### 1.3 tunnel 請求會經過的檢查

```text
ChatGPT ─▶ OpenAI control plane ─▶ 本機 tunnel-client
           （tunnel-client 從 mcp-authorization 檔加上 Authorization header）
        ─▶ helper McpIngress 127.0.0.1:<ephemeral>/mcp
        ─▶ runtime <server_url>/mcp ─▶ AuthMiddleware ─▶ 工具層 scope
```

- tunnel-client 的參數與環境：`CONTROL_PLANE_API_KEY`，並移除 `OPENAI_*` 和 bootstrap 環境變數；Authorization 用 `file:` 傳，不放在 argv（`tunnel.rs:697-720`）。
- Ingress（`verification.rs:163-221`）只掛 `/mcp` 和 `/mcp/`（`:202-205`），backend URL 固定為 `<server_url>/mcp`（`tunnel.rs:348`）。因此 tunnel 到不了任何 `/api/*` 路由。
- `proxy_inner` 的檢查依序如下（`verification.rs:280-370`）：
  1. 暫停中回 503。
  2. 同時處理中的請求上限 32，超過回 429。
  3. body 上限 16 MiB、讀取期限 10 秒，超過回 413／408。
  4. POST body 解析失敗就拒絕（`:331-336`）。
  5. Computer control 安全閘門：`ComputerControlMode` 有 read-only／ask／allow-session／always 四種，batch 一律拒絕（`:337-361`；`computer_safety.rs:311`）。
  6. 管理工具黑名單（`:363-369`、`:715-757`）：直接呼叫，或經 `call_runtime_tool` 間接呼叫，都會被擋；batch 只要含其中一個就整批拒絕。
  7. 原樣轉送 header，包含 Authorization（`:643-660`）。
- 黑名單沒擋、但 bootstrap 能叫的工具：`skill_inventory`、`skill_versions`、`memory_scope_list`（可列出所有專案的 memory scope）、`read_tool_trace`，以及所有 `Unknown` policy 工具、所有執行類工具。

### 1.4 log 與殘留

- helper activity sanitizer（`rust-helper/src/chadex_core/activity.rs:15-27`）會遮罩 `Bearer <x>`，以及 `wc_pair_`、`wc_pat_`、`wc_agent_`、`webcodex_temporary_`、`sk-` 前綴。**`wc_boot_`、`wc_oat_` 不在清單裡**：沒有前置 `Bearer` 的 bootstrap 字串不會被遮罩。
- tunnel-client 的 log 寫在 session 目錄（`--log.file`，JSON，info），session 結束時刪除。tunnel-client 是第三方 binary，它會不會記錄 extra headers 尚未驗證。
- Runner shell 會移除 `WEBCODEX_TOKEN`、`WEBCODEX_PAT`、`WEBCODEX_USER_TOKEN`、`AUTHORIZATION` 等環境變數（`chadex-runtime-runner/src/webcodex_runner/shell.rs:378-384`），但磁碟上的 env 檔和 token 檔仍可被同一個使用者讀取。
- 文件 `docs/ARCHITECTURE.md:292` 把 authorization 檔描述為「短生命週期」；但有崩潰殘留的例外（見 1.1）。

## 2. 威脅模型

前提：本機 OS 使用者可信；ChatGPT 的模型輸出不可信（可能被提示注入）。要保護的資產有：使用者檔案與程式碼、桌面控制、runtime 管理面（Skill 安裝、跨專案 Memory、token 管理），以及 bootstrap token 與 OpenAI API key。

| 威脅 | 情境 | 現況的損害 | 改用限縮 token 後 |
|---|---|---|---|
| T1 tunnel 外洩 | OpenAI Secure MCP Tunnel 沒有公開 URL，對應的情境是 control-plane API key 或 tunnel id 外洩，或 ChatGPT workspace 裡的其他人取得 connector | 攻擊者的請求和 ChatGPT 走同一條路，拿到 tunnel-client 注入的 bootstrap，也就是完整工具面，含 shell，等於使用者權限 | **差別不大**：有 shell 時一樣能拿到使用者權限。差別在於可以撤銷 tunnel token 當作緊急開關，不必換 bootstrap、不必重建桌面 admin token 或 Runner；沒有 shell 的攻擊路徑碰不到管理面。主要控制仍在 OpenAI 端（換 key、管 connector 分享）和 Chadex 的「停止 tunnel」 |
| T2 提示注入 | repo 內容、網頁或檔案誘導 ChatGPT 呼叫工具 | 不需要 shell 就能做的事：讀全部專案的 Memory scope 清單、讀 tool trace、在 tool list 看到管理工具；寫入類 admin 工具目前只靠 ingress 黑名單擋。透過 shell：讀 `webcodex.env` 拿 bootstrap，再打 loopback 的 `/api/tokens/register_hash` 自己鑄 admin token，或直接以使用者身分做任何事 | 不經 shell 的管理面由 runtime 權限擋下，不再只靠 helper 的名稱黑名單。**有 shell 時一樣能升級**，只是需要多步驟，而且每一步都會留在 job 紀錄。要讓 shell 本身成為邊界，只能靠方案 D |
| T3 token 進了 log 或殘留在磁碟 | 崩潰後留下的 `mcp-authorization`、未遮罩的 `wc_boot_` 字串、使用者匯出的診斷資料 | bootstrap 永久有效，是完整 admin（含 account／token 管理）。只能從 loopback 使用，所以攻擊者還需要本機存取 | tunnel token 有期限、沒有 admin、可以單獨撤銷；殘留檔案的有效期有限。bootstrap 不再寫進 tunnel session 目錄 |

縮權實際做得到的事：

1. 管理面改由 runtime 權限擋，ingress 黑名單降為第二層。黑名單漏掉的 admin 唯讀工具（`memory_scope_list`、`skill_inventory`、`skill_versions`、`read_tool_trace`）會一起收掉。
2. `tools/list` 不再把管理工具給 ChatGPT 看，減少誘因與誤用。
3. ChatGPT 的身分可以單獨撤銷或輪替，不影響 bootstrap、桌面 admin token 和 Runner。
4. 有期限：外洩或殘留的有效期有限。
5. 可稽核：`principal_kind`／`api_key_id` 能把 ChatGPT 的動作和桌面端、bootstrap 分開。
6. 是後續控制的基礎：shell 批准、專案綁定都需要一個可辨識的 tunnel principal。

做不到的事（不能宣稱）：防止使用 shell 的 ChatGPT 取得 OS 使用者權限；防止它讀取 `webcodex.env` 或其他 token 檔；把 ChatGPT 限制在單一專案目錄內（shell 可以 `cd` 到任何地方，`git_commit_paths` 會觸發 git hooks，`cargo_test` 會跑 build script，寫入檔案之後也可能被執行）。

## 3. 方案比較

### 方案 A：tunnel 專用的限縮 PAT（helper 鑄造）

做法：仿照 `admin_credential.rs`，新增 helper 的 tunnel credential 模組。tunnel 啟動時用 bootstrap 呼叫既有的 `/api/tokens/register_hash`，註冊一個名稱為 `chadex-tunnel` 的 PAT，`expires_at` 設為 TTL，scope 用明確白名單。之後把 `mcp-authorization` 的內容改成這個 PAT。輪替時撤銷同名舊 token。bootstrap 只留在 helper，不再交給 tunnel-client。

- scope 白名單放在 `chadex-runtime-core/src/authority.rs`（helper 已經依賴這個 crate，見 `rust-helper/Cargo.toml:22`），讓 runtime 測試和 helper 共用同一份定義。
- 改動範圍
  - runtime：不需要改功能，`register_hash`、`expires_at`、各 scope 都已存在。只新增 `CHADEX_TUNNEL_SCOPES` 常數和覆蓋率測試。
  - helper：新的 tunnel credential 模組；`tunnel.rs` 的 `start` 改寫 authorization 檔；啟動時清掃殘留的 `tunnel-sessions`；sanitizer 加上 `wc_boot_` 和 `wc_oat_`。
  - App（macOS）：MVP 不需要改；之後加狀態列與撤銷按鈕。
  - Windows：同一個 helper，程式碼不用改；Windows 上 `write_private_secret_file_atomic` 已經處理 ACL。之後補 UI。
- 相容性
  - ChatGPT connector 的設定不會失效：token 是本機注入的，connector 只認 tunnel 身分。
  - `tools/list` 會少掉管理工具，ChatGPT 端可能需要重新整理 connector。
  - Observation principal 從 `bootstrap` 改為 `api_key_id`：升級前用 bootstrap 建立的 session 或 job，升級後可能無法 observe（需驗證）。之後每次輪替 token 也會遇到同樣問題，所以 MVP **只在 tunnel 啟動時輪替**，不在 session 進行中換。
  - Runner 可見性：PAT 的 username 必須是 pairing 使用者，也就是 Runner 的擁有者；bootstrap 原本有 `owner_bypass`。
  - Host file import 會從不信任變成信任（第 1.2 節表格），需要決定（第 6 節）。
  - 舊版升級：第一次啟動 tunnel 時自動鑄造，使用者不需要做任何事。降級回 v0.5.0 時回到 bootstrap，殘留的 `chadex-tunnel` token 只是一筆沒人用的紀錄。
- 使用者體驗：正常情況下沒有感覺。鑄造失敗時 tunnel 啟動失敗，回傳穩定的錯誤碼；**不退回 bootstrap**（是否 fail closed 需要決定）。
- 風險：白名單太窄會讓現有功能壞掉（Memory、Computer、Goal、coding agent），太寬則效益有限；`Unknown` policy 工具在非 bootstrap 下會失效；tunnel-client 會不會重讀 `file:` 來源尚未驗證，所以 MVP 不依賴執行中輪替。

### 方案 B：依專案綁定

做法：讓 tunnel token 帶 `project_grant_id`，Runner、專案、Memory、session 的可見性都限制在目前選取的專案。runtime 已有 `RunnerAccessGroup::ProjectGrant`（`runner_http/auth.rs:11-14`），但 `ProjectAuthState` 只從 env 讀取、只用在 project-scoped server（`auth/project_credential.rs:41-79`），共用的本機 runtime 沒有「PAT 綁專案」的資料模型。

- 改動範圍：runtime 改動大（DB 欄位、PAT verifier、專案／Runner／Memory／session 的可見性與 `register_project` 的限制）；helper 要在切換專案時重鑄 token，並和 `realignLocalProject` 協調（已知限制：tunnel Ready 時不會暫停 ingress）；App 要處理切換專案的 UX；Windows 相同。
- 相容性：切換專案後，原本對話中引用其他專案的 session 會失效（這是設計目的）；tunnel 必須重新載入授權。
- 使用者體驗：ChatGPT 一次只能看到一個專案；切換專案時連線會短暫中斷。
- 價值與風險：可以擋住不經 shell 的跨專案讀取（尤其 Memory），但 shell 仍可直接讀其他目錄。可見性語意牽動範圍廣，回歸風險高。

### 方案 C：短效 token 加上刷新

做法：TTL 縮到 1–24 小時，由 helper 在到期前重鑄並改寫 authorization 檔。也可以改走 OAuth 的 access／refresh token，但 Secure MCP Tunnel 的認證是在本機注入的，OAuth 在這裡沒有實際效益。

- 改動範圍：helper 加排程與 401 偵測（ingress 看得到 backend 的狀態碼）；runtime 需要為 tunnel token 提供穩定的 principal（例如以 token family 或 name 推導，而不是 `api_key_id`），否則每次輪替都會切斷 session 與 job 的 observe 能力。
- 前提：要先確認 tunnel-client 每次請求都會重讀 `file:` 來源；如果不會，刷新就得重啟 tunnel，等於讓 ChatGPT 斷線。
- 相容性：connector 設定不變。
- 使用者體驗：刷新失敗時，ChatGPT 會在任務中途遇到 401。
- 價值與風險：外洩窗口從 30 天或永久縮到幾小時。實務上比較適合當作方案 A 的參數，不必單獨成為一個方案。

### 方案 D：依工具類別設定 scope，shell 需要額外批准

- D1：tunnel token 預設不含 `job:run`，由使用者開啟「允許 ChatGPT 執行指令」才加入。但 Chadex 的核心用途就是寫程式，關掉 shell 等於拿掉主要功能，只適合當作可選的嚴格模式。
- D2：在 ingress 加執行類工具的批准閘門，比照既有的 `ComputerSafetyController`（ask／allow-session／always，並有逾時與 batch 拒絕）。
- 改動範圍：helper 把 computer safety 擴展成 execution safety。分類應來自 runtime 的 ToolDefinition（`ToolEffect::Execute`、`shell_like`、`permission_risk`），不要在 helper 另外維護名稱清單，以免和現行黑名單一樣與 runtime 脫節。必須涵蓋 `call_runtime_tool` 間接呼叫、`code_mode_exec_effectful`、`run_skill_resource`、`coding_agent_start`、`cargo_*`、`go_test`。App 需要 macOS 與 Windows 的批准 UI。runtime 可選擇在 manifest 中輸出分類。
- 相容性：ChatGPT 被拒時看到的是 tool error；批准逾時（computer 目前是 45 秒）要短於 ChatGPT 端的工具逾時。
- 使用者體驗：coding 任務會發出大量指令，ask 模式會造成批准疲勞，實際上大多數人會選 allow-session。
- 價值與風險：這是唯一能讓「shell 等於完整權限」受到人工把關的方案。但間接執行無法完全列舉，例如寫入 git hook、`build.rs`、`package.json` scripts 之後再由其他工具觸發。所以 D2 算是降低風險，不算完整的邊界。

### 比較

| | A 限縮 PAT | B 專案綁定 | C 短效刷新 | D2 執行批准 |
|---|---|---|---|---|
| 擋不經 shell 的管理面 | 是 | 是（需搭配 A） | 否（需搭配 A） | 否 |
| 擋已注入且會用 shell 的 ChatGPT | 否 | 否 | 否 | 部分（依模式） |
| 可撤銷 | 是 | 是 | 是 | 不適用 |
| 縮短外洩窗口 | 中（天） | 中 | 高（小時） | 不適用 |
| runtime 改動 | 極小 | 大 | 中 | 小（可選） |
| helper 改動 | 中 | 中 | 中 | 中 |
| App／Windows UI | 可選 | 需要 | 不需要 | 需要 |
| connector 設定失效 | 否 | 否 | 否 | 否 |
| 主要回歸風險 | scope 白名單、principal 改變 | 可見性語意 | 刷新斷線、principal 穩定性 | 批准疲勞、漏網工具 |

## 4. 建議方案

採用 A，D2 作為可選的第二階段；C 與 B 延後。理由：A 用現成的 `register_hash` 和 admin token 的模式，runtime 幾乎不用改，就能把管理面從「helper 黑名單」改成「runtime 權限」，同時得到撤銷、期限與稽核。真正處理 shell 風險的是 D2，但它是產品取捨，不應該和 A 綁在一起。

建議的 tunnel scope 白名單（第一階段目標是「與現況相同的功能，只拿掉管理面」，之後再依 performance trace 的工具使用紀錄收緊）：

- 包含：`runtime:read`、`session:collaborate`、`project:read`、`project:write`、`job:run`、`memory:read`、`memory:manage`、`communication:read`、`communication:manage`、`computer:read`、`computer:control`、`computer:launch`、`computer:display_read`、`computer:pointer_control`、`computer:clipboard_read`、`computer:clipboard_write`
- 排除：`admin`、`account:manage`、`agent:*`、`plugin:manage`
- 待決定（第 6 節）：`job:detach`、`coding_agent:run`、`runner:manage`、`ssh:local`、`mcp:local`、`plugin:inspect`、`plugin:invoke`

## 5. 分階段實作

測試指令僅供實作者參考；本文件撰寫時沒有執行任何建置或測試。

### 階段 0：低風險前置修補（不改變授權）

- 範圍：
  - helper sanitizer 加上 `wc_boot_`、`wc_oat_` 前綴（`activity.rs`）。
  - helper 啟動時清掃殘留的 `tunnel-sessions/*`（`tunnel.rs`）。
  - ingress 黑名單補上 `skill_inventory`、`skill_versions`、`memory_scope_list`、`read_tool_trace`（`verification.rs:715`）。桌面端是直接呼叫 runtime，不經 ingress，所以不受影響。
- 驗收：
  - 含 `wc_boot_` 的訊息經 sanitizer 後不含原字串。
  - 模擬崩潰後，下次啟動沒有任何殘留的 `mcp-authorization`。
  - 上述四個工具經 tunnel 呼叫時回傳 `management_tool_not_available_over_tunnel`，包含直接呼叫、`call_runtime_tool` 間接呼叫和 batch。
- 測試：`activity.rs` 的 sanitizer 單元測試（比照 `sanitizer_redacts_runtime_credentials`）；`tunnel.rs` 的 lifecycle 測試加上「預先放一個舊 session 目錄」；`verification/ingress_tests.rs` 擴充黑名單案例。`cargo test -p chadex-helper`（helper crate）。

### 階段 1：tunnel 專用 PAT（方案 A MVP）

- 範圍：
  - `chadex-runtime-core/src/authority.rs` 新增 `CHADEX_TUNNEL_SCOPES`。
  - helper 新增 `adapters/runtime_backend/tunnel_credential.rs`（或放在 `tunnel/` 下），沿用 `admin_credential.rs` 的鑄造、寫檔、撤銷舊 token 與 backoff 流程：name 為 `chadex-tunnel`，TTL 依決定值，username 用 pairing 使用者。
  - `tunnel.rs` 的 `start` 改用此 token 建立 `TunnelSession`，bootstrap 不再進入 tunnel session。
  - 鑄造失敗時 tunnel 不啟動，回傳穩定錯誤碼（例如 `tunnel_credential_unavailable`）。
  - 輪替時機只有 tunnel 啟動：還有效且剩餘時間大於 TTL 的一半就沿用，否則重鑄。
  - `tunnel stop` 是否撤銷 token 依決定值。
- 驗收：
  1. `mcp-authorization` 的內容不含 bootstrap，也不含 admin token。改寫現有測試 `tunnel_authorization_carries_only_the_bootstrap_token_never_the_admin_token`（`tunnel.rs:1303`），斷言改為「只含 tunnel token」。
  2. 用 tunnel token 呼叫 `tools/list` 時，不出現 `SkillManagement` 和 `memory_scope_*` 工具；呼叫 `skill_install` 時，即使繞過 ingress、直接打 runtime，也會回傳 insufficient scope。
  3. 覆蓋率測試：以 `iter_tool_metadata()` 逐一檢查每個工具，除了明確列出的排除清單，tunnel scopes 都要滿足其 authority；沒有 `Unknown` policy 工具被意外排除。
  4. Runner 可見：tunnel token 能 `list_runners` 和 `read_files` 目前選取的專案。
  5. 撤銷之後，下一個 MCP 請求回 401；重新啟動 tunnel 後恢復。
  6. 過期之後，下一次啟動 tunnel 會鑄造新 token，並撤銷舊 token。
- 測試：
  - helper：比照 `admin_credential.rs` 的 mock server 測試（首次鑄造只註冊 hash、檔案權限、symlink 不跟隨、撤銷同名舊 token、backoff）。
  - runtime：在 `auth/tests.rs` 新增覆蓋率測試，並新增 PAT 的 `tools/list` 投影測試。
  - 整合：以真實 runtime 的 ignored 測試（比照 `admin_credential.rs:747` `real_runtime_accepts_the_minted_admin_token_and_denies_the_user_token`）驗證 tunnel token 能用一般工具、不能用 admin 工具。
  - 手動：macOS 實機連 ChatGPT，各跑一次讀檔、`run_shell`、Memory 讀寫、Computer observe／control，以及嘗試 `skill_install`。Windows 只有 CI，需要列為已知限制。
- 必須先驗證：既有的 bootstrap session 或 job 在升級後能不能被新的 principal observe。不能的話，要在 release note 說明，或在 runtime 為 `chadex-tunnel` 提供穩定的 principal。

### 階段 2：可見性與撤銷 UX

- 範圍：
  - helper protocol 新增 tunnel credential 狀態（只回傳 prefix、到期時間、scope 清單，絕不回傳 token 值）以及 `revokeTunnelCredential`。
  - macOS 設定頁顯示「ChatGPT 存取：限縮，到期日 …」，並提供「撤銷並中斷」。
  - Windows UI 對齊。
  - 更新 `docs/BRIDGE_PROTOCOL.md`、`docs/ARCHITECTURE.md:292`。
- 驗收：protocol 回應與 diagnostics 匯出不含 token 值；撤銷後 tunnel 狀態變為中斷，且 runtime 端的該 token 為 revoked。
- 測試：helper protocol 測試（斷言輸出不含 `wc_pat_` 後面的字元）；Swift `HelperClient` 解碼測試；Windows bridge 的 fake helper 測試。

### 階段 3（可選）：執行類工具批准閘門（方案 D2）

- 範圍：
  - ingress 新增 execution safety，分類來自 runtime 的 ToolDefinition metadata。
  - 模式沿用 `ComputerControlMode` 的四種值，預設值依決定。
  - 涵蓋 `call_runtime_tool`、code mode、batch（含執行類就整批拒絕或逐一批准）。
  - macOS 與 Windows 的批准 UI。
- 驗收：
  - ask 模式下，未批准的 `run_shell` 不會到達 runtime。
  - allow-session 模式只在同一個 tunnel session 內有效。
  - 分類測試：每個 `ToolEffect::Execute` 或 `shell_like` 的工具都被歸為執行類。
- 測試：比照 `computer_safety.rs` 的單元測試；ingress 測試（直接呼叫、gateway、batch）；runtime metadata 覆蓋率測試。

### 階段 4（延後）

- 方案 C：先驗證 tunnel-client 會重讀 `file:` header 來源，並讓 runtime 提供穩定的 tunnel principal，之後才縮短 TTL、加入執行中刷新。
- 方案 B：只有在「同時對 ChatGPT 開放多個專案」成為真實需求時才做。

## 6. 需要使用者決定的取捨

1. **Scope 白名單的嚴格度**：第一階段是要「與現況相同、只拿掉 admin」（建議），還是直接採最小集合？待決定的 scope：`job:detach`、`coding_agent:run`、`runner:manage`、`ssh:local`、`mcp:local`、`plugin:inspect`、`plugin:invoke`。
2. **TTL**：7 天（建議，配合「只在啟動 tunnel 時輪替」）、24 小時，或比照 admin token 的 30 天？越短，長時間掛著的 tunnel 越可能在中途過期。
3. **Fail closed**：鑄造失敗時，是讓 tunnel 不啟動（建議），還是退回 bootstrap 並顯示警告？
4. **停止 tunnel 時是否撤銷 token**：撤銷比較乾淨，但每次連線都要重鑄、寫入資料庫；不撤銷就靠 TTL 讓它失效。
5. **Host file import trust**：改用 user-kind PAT 後，ChatGPT 的 host file import 會從「不信任」變成「信任」。要接受（功能變多），還是讓 tunnel token 用不同的 kind 或旗標維持不信任（runtime 需要小改）？
6. **方案 D2 要不要做、預設模式**：ask、allow-session 或 always？這直接決定 shell 是否受人工把關，也決定 UX 成本。
7. **升級時的 session 連續性**：如果驗證結果是 bootstrap 建立的 session 或 job 升級後無法 observe，要接受（寫進 release note），還是先在 runtime 加入穩定的 principal？
8. **本機 shared key**：`WEBCODEX_SHARED_KEY_ENABLED=true` 讓任何本機程序用任意字串就能取得 model scopes。這不屬於 tunnel 縮權的範圍，但會削弱「loopback 認證」的意義。要另開任務檢討嗎？
