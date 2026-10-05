# Swift ↔ Rust Bridge Protocol

## Transport

- `chadex-helper` 是 App 私有 child process。
- stdin / stdout 使用 UTF-8、每行一個 JSON object（NDJSON）。
- stdout 僅允許協定訊息。
- diagnostics 只寫 stderr，經 activity sanitizer 遮蔽 token pattern。
- API key 不放 argv，也不回傳給 Swift。

## Request

```json
{
  "protocol_version": 1,
  "request_id": "uuid",
  "method": "getStatus",
  "params": {}
}
```

## Response

成功：

```json
{
  "protocol_version": 1,
  "request_id": "uuid",
  "result": {}
}
```

失敗：

```json
{
  "protocol_version": 1,
  "request_id": "uuid",
  "error": {
    "code": "structured_code",
    "message": "plain-language reason",
    "recovery": "one suggested recovery action",
    "details": {}
  }
}
```

protocol version 不相容時 fail closed，不嘗試猜測欄位。

## v1 Methods

下表與 `rust-helper/src/runtime_bridge.rs` 的 `handle_request` dispatch 一一對應；未列出的 method 回 `method_not_found`。App 與 helper 必須同版本一起更新。macOS 由 Swift `HelperClient` 呼叫全部 method；Windows 的 Tauri whitelisted IPC（`apps/windows/src-tauri`）經 `apps/windows/bridge` 只轉送 Runtime / connection、`queryActivities` 與 Skills（`discoverExternalSkillSources`、`getExternalSkillRoots`、`setExternalSkillRoots`、`getSkillCatalog`、`getSkillInventory`、`installSkill`、`activateSkill`、`deactivateSkill`）子集，目前沒有 Project Memory 或 Computer safety 的 desktop RPC。

### Runtime / connection

| Method | 行為 |
| --- | --- |
| `getStatus` | 取得目前 bridge snapshot；可帶 `include_mascot_jobs`（見下方） |
| `refreshRuntime` | Chadex runtime health refresh |
| `observeChatGPTActivity` | 相容性方法名稱；刷新 Chadex Tunnel/verification snapshot，不再呼叫 WebCodex activity probe |
| `inspectProject` | canonicalize + inspect folder |
| `activateProject` | 設定 UI target，不假裝 runtime 已切換 |
| `switchLocalProject` | 依目前 runtime 狀態安全切換專案 |
| `configureLocalSetup` / `resumeService` | 建立或恢復本機 runtime（同一實作） |
| `connectChatGPT` | 完整 `ensure runtime → start tunnel` 連線流程；成功與否以本回應判斷 |
| `startTunnel` | 由 Chadex TunnelManager 直接啟動並驗證 OpenAI Secure MCP Tunnel |
| `stopTunnel` / `disconnectAI` | 只停止 Chadex 擁有的 Tunnel child 與 MCP ingress（同一實作） |
| `stopLocalService` | 停止 Chadex 管理的本機 runtime |
| `provideCredential` | Tunnel ID + API key 注入 Chadex memory-only CredentialStore；不 echo secret |
| `clearCredential` | 清除 helper 記憶體中的 Tunnel credential，並重設目前 target 的 verification |
| `updateProxySettings` | 更新上游 Tunnel proxy 設定（`auto` / `direct` / `custom`） |
| `cancelOperation` | 以 exact operation ID 取消 runtime operation |
| `cancelTask` | 以 `project` + `task_id` 要求 cooperative task cancellation |
| `shutdown` | 先 abort／drain 進行中的 request，回覆後進行 graceful shutdown |

### Diagnostics

| Method | 行為 |
| --- | --- |
| `queryActivities` | 最多 200 筆 bounded activity |
| `queryPerformanceTraces` | 最多 100 筆 bounded MCP ingress timing；只含 method/tool 名稱、bytes、status 與各階段耗時，不含 params、檔案路徑或內容 |
| `queryLifecyclePerformanceTraces` | 最多 100 筆 bounded lifecycle（connect／switch 等）階段耗時 |

### Skills

所有 method 都帶 `path`（目前 project），由 helper 以 exact runtime probe target 轉成 runtime tool call。

| Method | 參數 | 行為 |
| --- | --- | --- |
| `getSkillCatalog` | `path` | Skill descriptor catalog（不含 definition 內容）；descriptor 含 `scripts_allowed`（見下） |
| `getSkillInventory` | `path` | 已安裝／啟用狀態與 `state_revision` |
| `getSkillDefinition` | `path`, `skill_id`, `definition_revision`, `package_revision?` | Lazy 讀取指定 revision 的 `SKILL.md` |
| `createProjectSkill` | `path`, `skill_key`, `content` | 在 `.agents/skills/<key>/` 建立 project skill |
| `installSkill` | `path`, `skill_key`, `artifact_path` | 從 artifact 安裝 skill |
| `activateSkill` | `path`, `skill_key`, `package_revision`, `state_revision` | Revision-fenced 啟用 |
| `deactivateSkill` | `path`, `skill_key`, `state_revision` | Revision-fenced 停用 |

Catalog descriptor 的 `scripts_allowed`（bool）表示該 Skill 的腳本資源是否被預設政策允許：configured／external root 依 root 的 `script_roots` opt-in；installed skill 為 `true`；project skill 為 `false`。舊版 helper 可能不帶此欄位，client 必須視為「未回報」。

#### External Skill sources

下列三個 method 不帶 `path`（設定是 Runner 全域），與 `getSkillCatalog` 使用相同 request／response envelope。

| Method | 參數 | 行為 |
| --- | --- | --- |
| `discoverExternalSkillSources` | 無 | 掃描 `~/.agents/skills`、`$CLAUDE_CONFIG_DIR/skills`（預設 `~/.claude/skills`）、`$CODEX_HOME/skills`（預設 `~/.codex/skills`），唯讀、不寫入任何設定 |
| `getExternalSkillRoots` | 無 | 讀取目前 Runner `[skills]` 的 `roots`／`script_roots` 與 `revision` |
| `setExternalSkillRoots` | `roots`, `script_roots`, `expected_revision`, `verify_project_path?` | Replace 語意：送完整清單；`script_roots` 必須是 `roots` 的子集 |

`discoverExternalSkillSources` result：

```json
{ "format": "chadex.external_skill_sources.v1",
  "sources": [{
    "kind": "agents|claude|codex", "path": "...", "canonical_path": "...?",
    "status": "available|not_found|not_directory|unavailable|scan_limit_exceeded|duplicate_source",
    "root_is_link": false, "same_as": "agents|claude|codex?",
    "valid_count": 0, "symlink_count": 0, "invalid_count": 0, "script_count": 0,
    "truncated": false, "provided_by": ["canonical root"],
    "packages": [{ "package": "...", "state": "valid|symlink|invalid", "name": "?",
                   "description": "?", "has_scripts": false, "link_target_root": "?",
                   "invalid_reason": "?", "name_conflict": false }] }],
  "recommended_roots": ["canonical path"] }
```

`provided_by` 是該 source 內 symlink package 解析後落入的 canonical root；全部 package 都是 symlink（`valid_count == 0`）或 `duplicate_source` 的 source 不應直接連接，應連接 `provided_by`／`recommended_roots` 中的真實 root。

`getExternalSkillRoots` result：`{ "format": "chadex.external_skill_roots.v1", "roots": [...], "script_roots": [...], "revision": "<sha256 of runner.toml>" }`。Local runtime 尚未設定或非 idle 時回 `runtime_not_ready`。

`setExternalSkillRoots` result 同上並多 `"generation": Int`。若清單與目前設定相同，helper 不寫檔，只盡力 reload 讓 Runner 與檔案重新同步，並回 `"runner_resynced": Bool`；同步失敗時帶 `"resync_error"`，但不視為錯誤。路徑必須是 exact canonical path（取自 discovery 的 `canonical_path`／`recommended_roots`，或 `resolvingSymlinksInPath()` 加 standardize 後的目錄選擇結果）；有開啟 project 時帶 `verify_project_path`，供寫入後驗證 catalog。

`setExternalSkillRoots` 錯誤碼：

| Code | 意義 |
| --- | --- |
| `external_skill_roots_conflict` | `expected_revision` 已過期；`details.current_revision`。重新讀取後再套用 |
| `skill_root_invalid`／`skill_root_not_found`／`skill_root_is_link`／`skill_root_not_directory`／`skill_root_not_canonical`／`skill_root_sensitive` | 路徑被拒；`details.path` |
| `runner_config_rejected` | Runner 的 config check 拒絕新設定 |
| `runner_config_reload_failed`／`runner_config_restart_required`／`runner_config_reload_unsupported` | Runner 無法 hot reload；設定已回復（restart_required 需重啟 runtime）。若 reload 結果無法確認（`outcome_unknown`）且回復後無法重新同步，改回 `external_skill_roots_state_unknown` |
| `runtime_unreachable`／`runtime_not_ready` | Runtime 無法連線或尚未就緒 |
| `external_skill_roots_unverified` | 寫入後驗證失敗；`details.rolled_back` 表示是否已回復 |
| `external_skill_roots_state_unknown` | Runner 可能仍在使用新設定；client 應重新讀取並再套用一次 |

### Project Memory

| Method | 參數 | 行為 |
| --- | --- | --- |
| `getProjectMemoryCatalog` | `path` | Memory descriptor catalog |
| `getProjectMemory` | `path`, `memory_key`, `expected_revision?` | 讀取單筆 memory |
| `setProjectMemory` | `path`, `memory_key`, `summary`, `body`, `priority`, `bootstrap`, `tags`, `expected_revision?` | 建立或以 revision CAS 更新 |
| `deleteProjectMemory` | `path`, `memory_key`, `expected_revision` | 以 revision CAS 刪除 |

### Computer Use safety

回應皆為 `ComputerSafetySnapshot`（`mode`、`stopped`、`generation`、`pending_approvals`、`audit`）。語意見 [Architecture · Computer Use safety](ARCHITECTURE.md#computer-use-safety)。

| Method | 參數 | 行為 |
| --- | --- | --- |
| `getComputerSafety` | — | 目前 safety snapshot |
| `setComputerControlMode` | `mode` | `read_only` / `ask_before_control` / `allow_session` / `always_allow`；同時清除 Stop |
| `approveComputerControl` | `approval_id` | 批准單一 pending action；已失效回 `computer_approval_stale` |
| `approveComputerControlAlways` | `approval_id` | 批准並原子切換為 `always_allow`；已失效回 `computer_approval_expired` |
| `denyComputerControl` | `approval_id` | 拒絕單一 pending action |
| `stopComputerControl` | — | 持久 Stop：deny 所有 pending，直到明確 resume |
| `resumeComputerControl` | — | 解除 Stop，保留原模式 |

### Removed

- `getProjectInstructions`（V042 移除）：只服務 v0.4.1 已移除的 repository instructions 面板。Repository `AGENTS.md` 仍由 runtime `project.instructions` context material 提供給模型；Chadex Global Instructions 由 app 直接讀寫 `global-instructions.md`，不經 bridge RPC。

## Backend Snapshot

對 UI 暴露的 phase 僅有：

- `unconfigured`
- `preparing`
- `waiting_for_chatgpt_verification`
- `verified`
- `stopped`
- `error`

`tunnel_ready` 與 `chat_gpt_verified_for_selected_project` 是不同欄位。前者只代表 Chadex 已確認 OpenAI `tunnel-client` `/readyz`；後者還要求 target project 精確等於 runtime project、verification epoch 與目前專案一致，而且 Chadex MCP ingress 已看到該 epoch 的 `tools/call` 被 backend 接受。

## Secret Contract

`provideCredential` 的 `api_key` 是協定中唯一會攜帶 OpenAI API key 的 request 欄位。helper 收到後立即從 request object 移除並放入 `Zeroizing<String>`，再交給 Chadex `CredentialStore`。API key 不會交給 temporary WebCodex backend、snapshot 不序列化 API key；只有 Chadex 啟動的 `tunnel-client` exact child environment 會收到它。

`observeChatGPTActivity` 保留原名稱是為了不破壞 Swift protocol v1。其實作已改為 Chadex-owned verification refresh；未來 protocol major version 可再重新命名。

## Performance diagnostics

Phase 1 tracing 在 Chadex MCP ingress 量測四個區段：

- `ingress_pre_backend_us`：ingress 收到 request 到開始送往本機 backend。
- `backend_headers_us`：送出 backend request 到收到 response headers；目前包含 WebCodex backend / project authority / Runner / tool execution 到 headers ready 的總時間。
- `response_stream_us`：收到 headers 到 response body stream 完成或被 client 中止。
- `total_us`：整個 ingress request 的總時間。

trace 只保留最近 100 筆，記錄 MCP method、`tools/call` 的 tool name、request/response bytes、HTTP status、completion state 與 timing。request params、project path、檔案內容、Authorization 與 API key 不會寫入 performance trace。

新增的相容欄位：`finished_at_ms`、`request_id_hashes`、`server_trace_id`。前兩個時間戳是 wall-clock epoch 毫秒，用於對照外部紀錄；四個 duration 仍由 monotonic clock 計算，不依賴時鐘同步。JSON-RPC string/number ID 以其 canonical JSON UTF-8 的完整 SHA-256 表示（保留型別差異），每個 request 最多 16 筆，不保存原始字串 ID。notification 沒有 ID，留空。Hash 是關聯用途，不是授權或匿名化保證。

MCP POST handler 自行產生 `x-chadex-trace-id` response header，不採信 caller 傳入的同名 header。它沿用既有 Server→Runner trace identity；ingress 僅保留 UUID 格式的值，舊 backend 沒有 header 時為 null。這不開啟 full payload tracing，也不改變 JSON-RPC body、streaming、admission 或 verification。Swift diagnostics 匯出毫秒時間與這些欄位；舊 helper 回覆仍可解碼。

這些欄位能關聯可取得的 client／ingress／Server→Runner 紀錄，但不會憑空提供 relay、宿主 dispatch 或 approval timestamps。`response_stream_us` 完成也不代表遠端宿主已收到結果；外部未觀測區間仍須標示為未定位等待。

## Optional mascot observation (v0.3.1)

`getStatus` accepts `params.include_mascot_jobs` (default `false`). When requested, the helper may add `mascot_jobs`, a bounded current-project array containing only Job ID, status, optional millisecond timestamps, and optional exit code. Missing/null means observation unavailable; an empty array means the bounded scoped query observed no Jobs. The projection does not expose command text, stdout, stderr, or credentials and does not change Job execution or retry policy.

MCP performance records may also include optional `tool_failed`. A live request and its completed response retain the same sequence. Older snapshots/records remain decodable.
