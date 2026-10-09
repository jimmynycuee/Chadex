# Computer use：Chromium／Electron 網頁輔助資訊支援設計

狀態：S1–S6 已實作（macOS），尚待 §9 實機測試與 §10 決策確認，實作差異見 §12。分支：`feature/computer-chromium-ax`，基準 `dd74d05`。
範圍：macOS 實作；Windows 維持現有行為，新能力回 `capability_unavailable`（見 §7）。
不在範圍：放寬 `input_text`（只接受空的 `AXTextField`／`AXTextArea`、2048 bytes）、`key`（只開放導覽鍵）、登入／授權畫面封鎖。這些限制原樣保留。

---

## 0. 現況與問題（讀程式碼得到的事實）

| 項目 | 位置 | 現況 |
|---|---|---|
| AX 樹 | `crates/chadex-runtime-computer/src/platform/macos/accessibility.rs::accessibility_tree` | 從 `exact_ax_window` 開始 BFS，`MAX_ACCESSIBILITY_DEPTH = 8`、`MAX_ACCESSIBILITY_NODES = 256`（`lib.rs`），預設 6／128。 |
| 元素 handle | `lib.rs::ElementRecord { surface_id, path: Vec<usize>, lineage: Vec<ElementFingerprint> }` | `path` 是從視窗根算起的子節點 index；`lineage` 是根到目標每一層的 fingerprint。`resolve_correlated_element` 每次從視窗根依 path 重走，並逐層比對 fingerprint，不符就 `stale_element`。 |
| Registry | `lib.rs::ElementRegistry` | 每次觀察 `replace_surface` 會讓同一 surface 的舊 id 全部失效、generation +1；總量上限 `MAX_ELEMENT_REGISTRY = 1024`（FIFO 淘汰）；`list_windows` 清空全部。 |
| Deadline | `lib.rs::AxObservationDeadline` | 每次原生操作 10 s 硬上限；每個 AX IPC 的 messaging timeout ≤ 2 s。逾時回 `accessibility_failed: ... deadline exceeded`。 |
| `find_elements` | `runtime-engine/src/tool_runtime/computer_tools.rs` | **在 server 端**：先向 runner 要一棵 8 層／256 節點的樹，再用 `filter_accessibility_tree` 過濾。只比對 role、subrole、label（title／description／placeholder 子字串）、focused、enabled；**刻意不搜 AXValue**。 |
| 敏感資訊 | `accessibility_tree` | `AXSecureTextField`、subrole 含 `Secure`、`AXProtectedContent` 的節點不讀 value；protected 時連 title／description／placeholder 都不讀。 |
| 敏感 surface | `lib.rs::ensure_surface_not_sensitive` | 只擋 control 類（activate、press／focus、scroll、key、input_text、pointer）。觀察類（tree、find、state）**目前不擋**。 |
| Chromium | — | Chadex 沒有設定 `AXManualAccessibility`／`AXEnhancedUserInterface`。Chromium 系 app 預設不建網頁內容的 AX 樹，`AXWebArea` 底下幾乎是空的。 |

三個疊在一起的問題：

1. Chromium 沒開輔助資訊時，`AXWebArea` 底下是空的。
2. 就算開了，網頁內容通常在視窗根往下 10–30 層，超過 8 層上限；而 256 節點會先被瀏覽器外框（toolbar、tab strip）用掉。
3. Chromium 把網頁文字放在 `AXStaticText` 的 **AXValue**，不是 AXTitle，所以現在不搜 value 的 `find_elements` 找不到網頁上的文字。

---

## 1. AXManualAccessibility

### 1.1 選 `AXManualAccessibility`，不用 `AXEnhancedUserInterface`

| | `AXManualAccessibility` | `AXEnhancedUserInterface` |
|---|---|---|
| 來源 | Electron 為第三方輔助工具加的屬性，Chromium 的 mac browser application 也接受（兩者都在 `NSApplication` 的 `accessibilitySetValue:forAttribute:` 處理）。 | VoiceOver 開啟時設定的 AppKit 屬性。 |
| 副作用 | 只讓 Chromium 打開 renderer 的 accessibility mode。 | AppKit 會把它當成「有螢幕閱讀器」：視窗移動／縮放會加動畫，視窗管理工具（Rectangle、Magnet、yabai 等）設定位置時會有延遲或位置錯誤，這是已知問題。且作用於整個 app 的 AppKit 行為。 |
| 結論 | **採用。** | **預設不用。** 只有在實機驗證某個 Chromium 衍生瀏覽器不接受 `AXManualAccessibility` 時，才考慮做成額外的 opt-in 設定（列為使用者決定 D2）。 |

「Chromium 也接受 `AXManualAccessibility`」必須在 Brave、Chrome、Edge、Arc 上實機確認（§9 L1）。設計上，不支援時要能優雅退化：回報 `web_accessibility = "unsupported"`，樹照常回傳。

### 1.2 對誰設：偵測，不是一律嘗試

建議用 **偵測**（決策 D1）。一律嘗試雖然簡單，但會對所有被觀察的 app 做一次寫入。偵測的成本只是讀一次 bundle 資訊，而且能把副作用限制在已知的 app 類型。

新增純函式 `classify_web_engine(bundle_id: &str, bundle_path: &Path) -> WebEngine`，`WebEngine = None | Chromium | Electron`：

1. `Contents/Frameworks/Electron Framework.framework` 存在 → `Electron`。
2. bundle id 在 allowlist → `Chromium`：
   `com.google.Chrome`、`com.google.Chrome.beta`、`com.google.Chrome.dev`、`com.google.Chrome.canary`、`org.chromium.Chromium`、`com.brave.Browser`（含 `.beta`、`.nightly`）、`com.microsoft.edgemac`（含 `.Beta`、`.Dev`、`.Canary`）、`company.thebrowser.Browser`（Arc）、`com.vivaldi.Vivaldi`、`com.operasoftware.Opera`。
3. 通用規則：`Contents/Frameworks/` 底下（只列一層，最多 64 個 entry）有名稱以 ` Helper (Renderer).app` 結尾的 bundle，或任一 `*.framework/Versions/Current/Helpers/` 裡有這種 bundle → `Chromium`。
4. 其他 → `None`，不設定任何屬性。

pid 對應 bundle：`NSRunningApplication::runningApplicationWithProcessIdentifier(pid)` → `bundleIdentifier()`、`bundleURL()`（`objc2-app-kit` 已啟用 `NSRunningApplication` feature）。偵測結果用 `bundle_path` 當 key 快取在 `ComputerRuntime`（上限 32 筆，超過就清空重建）。

### 1.3 什麼時機設定

- **只在 AX 觀察時**才設定：`computer_accessibility_tree`（舊 kind）、`computer_accessibility_subtree`、`computer_accessibility_find`（新 kind），而且要在 `exact_ax_window` 解析成功之後、走訪之前。`list_windows`、snapshot、control 類**不**觸發。
- 觀察 sensitive surface（`ensure_surface_not_sensitive` 會失敗的 surface）時**不設定**。
- 每次觀察都重設一次（`AXUIElementSetAttributeValue(app, "AXManualAccessibility", kCFBooleanTrue)`）。這是冪等操作，只花一次 IPC。原因是 Chromium 有「一段時間沒有輔助工具 API 呼叫就自動關閉 accessibility」的機制（auto-disable），只記 pid 不重設，可能在之後的觀察拿到空樹。
- 設定失敗的處理：`AttributeUnsupported`、`IllegalArgument`、`ActionUnsupported`、`NotImplemented` → 記成 `unsupported`，繼續觀察。`APIDisabled` → 照現有慣例回 `permission_denied`。其他錯誤 → 記成 `unsupported`，繼續觀察（觀察不能因為「加分功能」失敗而失敗）。

設定開關（決策 D3）：`ComputerConfig` 新增 `web_accessibility: WebAccessibilityPolicy { Auto, Off }`，預設 `Auto`。runner 啟動時讀環境變數 `CHADEX_COMPUTER_WEB_ACCESSIBILITY=off` 改成 `Off`（其他值或未設定都是 `Auto`）。`Off` 時完全不寫屬性，輸出回 `disabled`。

### 1.4 設定後怎麼等（有上限的重試）

Chromium 在屬性設定後才非同步建 renderer 的 AX 樹。等待邏輯：

```
memo = runtime.web_ax_memo.get((pid, launch_date_secs))
set AXManualAccessibility = true
if memo 存在（這個 process 已經等過）: state = already_enabled，不等待
else:
    for delay in [0ms, 150ms, 300ms, 600ms, 900ms]  // 累計 ≤ 1.95 s
        sleep(delay)，但不得超過 deadline 剩餘時間 − 3 s
        probe：從 exact window BFS，depth ≤ 12、visited ≤ 200，找 role == "AXWebArea" 且 child_count > 0
        找到 → state = enabled；寫入 memo；break
        找不到 AXWebArea（例如 Electron 的原生設定頁）→ 也算 enabled，但只等第一輪
    迴圈結束仍沒有內容 → state = pending（不寫 memo，下次觀察會再等一次）
```

- memo：`Mutex<HashMap<(u32 pid, Option<i64> launch_date_secs), Instant>>`，上限 64 筆，超過就移除最舊的。`launch_date` 來自 `NSRunningApplication.launchDate`，用來避免 pid 重複使用時誤判。memo **只**決定要不要等，不決定要不要設定（每次都設定）。
- 等待在 platform 函式裡進行，這時不持有任何 registry lock（`ComputerRuntime::accessibility_tree` 已經是先複製 `SurfaceRecord` 再呼叫 platform，維持這個做法）。
- 整段等待算在同一個 `AxObservationDeadline`（10 s）裡。
- 等待邏輯寫成可注入 clock 和 probe 的純函式 `wait_for_web_content(probe, clock, schedule, deadline) -> WebAxState`，方便單元測試。

### 1.5 對使用者的影響、要不要復原

- **效能**：Chromium 打開 accessibility 後，所有分頁的 renderer 都會維護 AX 樹，大頁面（Google Sheets、長文件）的 CPU 和記憶體會上升，通常在個位數到十幾個百分點，要實機量測（L6）。效果持續到瀏覽器重啟，或被 Chromium 的 auto-disable 關掉為止。
- **其他輔助工具**：開啟不會干擾 VoiceOver 或其他工具（它們本來就會打開同一個 mode）。
- **不復原（建議）**：設回 `false` 可能把其他輔助工具需要的 accessibility 一起關掉，而且 Chromium 重建樹也有成本。runner 結束時也沒有可靠的時機可以做復原。決策 D4 讓使用者確認。
- **輸出揭露**：新 kind 的輸出帶 `web_accessibility`（§4），讓模型和稽核紀錄知道 Chadex 對目標 app 做過什麼。
- **「唯讀工具」語意**：`computer_observe` 的 `readOnlyHint = true`。打開 Chromium accessibility 會改變目標 app 的內部狀態，但不會改到使用者資料，也看不到畫面變化，和 VoiceOver 的行為一樣。這是設計上的取捨（決策 D5），另一個做法是改成需要批准的 `computer_control(action=enable_web_accessibility)`。

---

## 2. 子樹查詢（`root_element_id`）

### 2.1 模型看到的介面

在現有的 `computer_observe(action=accessibility_tree)` 加一個選用參數：

```json
"root_element_id": {"type": "string", "minLength": 9, "maxLength": 128,
  "description": "Optional fresh element_id from the same surface; observe its descendants instead of the window root. Re-issues ids for the whole surface."}
```

`max_depth`（仍上限 8）和 `max_nodes`（仍上限 256）改成**相對於 root**。不新增 action，舊呼叫方式不變。

### 2.2 語意

1. 驗證：`root_element_id` 以 `element_` 開頭、長度 ≤ `MAX_ELEMENT_ID_BYTES`；不合格回 `invalid_request`。
2. `ComputerRuntime::accessibility_subtree(surface_id, root_element_id, max_depth, max_nodes)`：
   - 取 surface（`stale_surface`）。
   - **新規則**：`ensure_surface_not_sensitive(&record)?`（見 §6）。
   - 從 registry 取 `ElementRecord`。找不到、已淘汰 → `stale_element`；屬於別的 surface → `stale_element`。
   - `root.contains_protected_content()` 或 lineage 中有 secure fingerprint → `permission_denied: protected or secure Accessibility content cannot be a query root`。
3. platform：`resolve_correlated_element(surface, root_record, deadline)` 驗證 lineage，再從這個節點 BFS。
   - 每個子節點的 `path = root.path ++ 相對 path`，`lineage = root.lineage ++ 相對 lineage`。所以新 id 跟一般 id 一樣，由視窗根解析，`element_state`／`control`／`scroll_to_element`／`input_text` 完全不用改。
   - 輸出的 `depth` 從 root 的 0 開始算（root 本身是 depth 0、`parent_element_id = null`），現有 server 驗證器 `validate_accessibility_node` 的「depth ≤ max_depth、parent 在 depth−1」規則不用改。
   - `inherited_protected` 的初始值 = `root.contains_protected_content()`（已在第 2 步擋掉，所以實際上是 `false`）。
4. **絕對深度上限** `MAX_ACCESSIBILITY_ABSOLUTE_DEPTH = 64`：`root.path.len() + relative_depth` 到 64 就不再往下展開，並設 `truncated = true`。這個限制同時約束 `resolve_correlated_element` 的成本（每層大約 9 次 AX IPC；64 層約 600 次，在 10 s deadline 內）。
5. Registry：結果透過 `replace_surface` 寫入，**取代**這個 surface 的所有舊 id（包含傳入的 `root_element_id`）、generation +1。root 會以新 id 出現在 `nodes[0]`，模型可以接著用。這樣維持現有「一次觀察 = 一個 generation」的不變式，不會混用不同時間點的 handle。

### 2.3 lineage 記憶體

子樹讓 lineage 從最多 9 層變成最多 64 層。256 個節點各自複製 64 個 fingerprint（每個最多約 1.3 KB 字串），最壞情況每次觀察約 21 MB，registry 1024 筆約 85 MB。所以要改成共用：

- `ElementRecord.lineage: Vec<Arc<ElementFingerprint>>`。BFS 時 clone 的是 `Arc`。
- 用到的地方：macOS `resolve_correlated_element`、`element_state`、`control`、`validate_*_target`、Windows `resolve_uia_element`（`current_root != element.lineage[0]` 改成 `current_root != *element.lineage[0]`）、`ElementRecord::target_fingerprint`（回傳 `Option<&ElementFingerprint>`，改成 `.map(Arc::as_ref)`）。
- `ElementRecord` 的 `PartialEq` 仍然比較值（`Arc<T: PartialEq>` 本來就是比值）。

### 2.4 失效處理

| 狀況 | 錯誤 | 修復建議（`computer_error_recovery_message` 已有） |
|---|---|---|
| root id 不存在、已淘汰、被新觀察取代 | `stale_element` | 改用 `find_elements` 或不帶 root 的 `accessibility_tree` 重新取得 |
| root 所屬 surface 不同 | `stale_element` | 同上 |
| lineage 比對失敗（頁面重排、標題改變） | `stale_element` | 同上 |
| surface 失效 | `stale_surface` | `computer_observe(action=windows)` |
| runner 不支援 | `capability_unavailable` | 不帶 `root_element_id` 重試（Windows、舊 runner） |

Chromium 的網頁樹常會變動（Sheets、動態頁面），`stale_element` 會比原生 app 常見。這是正確行為：不能拿過期的 handle 去操作。

---

## 3. 深層 `find_elements`

### 3.1 介面（在現有 action 加參數）

```json
"root_element_id": {同 §2.1},
"value": {"type": "string", "minLength": 1, "maxLength": 256,
  "description": "Optional case-sensitive literal substring matched against AXValue of non-secure, non-protected elements only; values are never returned."},
"max_depth": {"type": "integer", "minimum": 1,
  "description": "Search depth below the root; defaults to 32, values above 48 are clamped to 48."}
```

`label` 的說明要從 “AXValue is never searched” 改成 “matched only against title, description, or placeholder; use value for AXValue”。

「至少要有一個條件」的規則不變；`value` 算一個語意條件。

### 3.2 runner 端搜尋（新 wire kind `computer_accessibility_find`）

把搜尋搬到 runner，因為 (a) 只有 runner 能走 8 層以下，(b) 只把符合的節點送回 server，回應大小跟走訪量無關。

payload（runner 用 `ensure_exact_payload_fields` 嚴格檢查，所有 key 都必須出現，可為 `null`）：

```json
{"surface_id": "...", "root_element_id": null, "role": null, "subrole": null,
 "label": null, "value": null, "focused": null, "enabled": null,
 "limit": 8, "max_depth": 32}
```

上限（`lib.rs` 新增常數）：

| 常數 | 值 | 說明 |
|---|---|---|
| `DEFAULT_FIND_DEPTH` / `MAX_FIND_DEPTH` | 32 / 48 | 相對 root 的深度 |
| `MAX_ACCESSIBILITY_ABSOLUTE_DEPTH` | 64 | 與 §2 共用 |
| `MAX_FIND_VISITED` | 4000 | 每次搜尋最多讀幾個節點（不對外開放參數） |
| `MAX_FIND_CHILDREN_PER_NODE` | 512 | 單一節點最多展開幾個子節點（大型清單、表格） |
| `FIND_SOFT_BUDGET` | 6 s | 從 deadline 建立起算；到了就停止走訪，回傳目前結果 |
| `DEFAULT_FIND_ELEMENTS_LIMIT` / `MAX_FIND_ELEMENTS_LIMIT` | 8 / 32 | 沿用 server 現有值 |

時間預算：沿用 `AxObservationDeadline`（10 s 硬上限、IPC ≤ 2 s）。搜尋另外加一個**軟預算**：每處理一個節點都檢查 `elapsed >= FIND_SOFT_BUDGET`，到了就停，`stop_reason = "time_budget"`，**照常回傳成功**。硬 deadline 仍然回錯誤，用來處理單一 IPC 卡住的情況。如果同一次呼叫先花時間在 §1.4 的等待上，軟預算不會延長（共用同一個起點），保留至少約 3 s 給解析和 registry。

走訪：BFS，用 arena 存已讀節點，避免每個 queue entry 都複製 path 和 lineage：

```rust
struct Visited { parent: Option<u32>, child_index: u32, depth: u16,
                 fingerprint: Arc<ElementFingerprint>, sensitive: bool }
```

只有符合條件的節點才回溯 arena 組出 `path`／`lineage`，寫成 `ElementRecord`。

每個節點讀取的順序（敏感資訊規則，跟現有 tree 一樣，再加嚴）：

1. `AXRole`、`AXSubrole`、`AXProtectedContent`（以及 `AXIdentifier`）。
2. `protected = inherited_protected || AXProtectedContent`；`secure = role == "AXSecureTextField" || subrole 含 "Secure"`。Chromium 的 `<input type=password>` 是 role `AXTextField` + subrole `AXSecureTextField`，會被第二條抓到。
3. 只有 `!protected` 時才讀 `AXTitle`、`AXDescription`、`AXPlaceholderValue`（同 `element_fingerprint`）。
4. 只有「有 `value` 條件」且 `!protected && !secure` 時才讀 `AXValue`。**secure 或 protected 的節點絕不呼叫 AXValue**，`value` 條件對它們一律不符合。
5. **加嚴**：往下傳的 `inherited_protected` 改成 `protected || secure`（現有 tree 只傳 `protected`），secure 欄位的子孫一樣不讀 title 和 value。這條只套用在新的 find／subtree 路徑。要不要也改舊的 tree 由決策 D7 決定。
6. `AXEnabled`、`AXFocused`：只有在有 focused／enabled 條件或節點符合時才讀。
7. `AXChildren` 的數量和元素：深度、絕對深度、visited 預算都沒到上限時才讀，最多讀 `MAX_FIND_CHILDREN_PER_NODE` 個。

比對：沿用 server 現在的 `node_matches_find_query` 語意（role、subrole 完全相等；label、value 是 case-sensitive 子字串；focused、enabled 完全相等，`null` 不符合）。把這個函式搬進 `chadex-runtime-computer`（純函式），server 的 fallback 路徑和 runner 共用同一份行為（server 可以保留自己那份，但要有一致性測試）。

符合的節點達到 `limit` 後，繼續走訪只為了算 `total_matches`，最多再走到 `MAX_FIND_VISITED`。不走的話 `truncated` 就沒辦法準確。

### 3.3 輸出

```json
{
  "platform": "macos",
  "surface_id": "surface_…",
  "observation_generation": 7,
  "search_mode": "deep",
  "root_element_id": "element_…",      // 新 id；沒有指定 root 時為 null
  "web_accessibility": "enabled",
  "elements": [
    {"element_id": "element_…", "role": "AXButton", "subrole": null,
     "title": "Share", "description": null, "placeholder": null,
     "enabled": true, "focused": false,
     "depth": 17,
     "ancestors": "… › AXWebArea “Budget - Google Sheets” › AXGroup › AXToolbar “Main”"}
  ],
  "count": 1,
  "scanned_nodes": 1834,
  "truncated": false,
  "stop_reason": "complete"            // complete | limit | visit_budget | time_budget | depth_bound
}
```

- `depth` 是**絕對**深度（從視窗根算），用來判斷元素在多深的位置。
- `ancestors`：由近到遠取祖先，組成「遠 › … › 近」的字串，每段是 `role` 加上（非 protected 時）最多 40 bytes 的 title（截斷時切在 UTF-8 字元邊界）。整串 ≤ 256 bytes，太長就從遠端截掉並加 `… › ` 前綴。protected 的祖先只顯示 role。**不放 value。**
- 回傳的元素**不含 `value`**（維持現有 find 的契約與稽核規則）。要看內容時，用 `root_element_id` 指向該元素做 `accessibility_tree`，就會套用一般 tree 的 value 規則。
- Registry：root（有指定時）＋所有回傳的 match 用一次 `replace_surface` 寫入，跟 §2.2 一樣取代該 surface 的舊 id。沒有回傳的 match 不登記。
- `truncated = stop_reason != "complete" || total_matches > count`。

### 3.4 server 端與相容性

`dispatch_computer_tool` 的 `FindElements` 分支：

1. 先送 `computer_accessibility_find`。
2. 如果回 `capability_unavailable`（舊 runner、Windows runner），而且請求**沒有** `root_element_id`：退回現有流程（8 層／256 節點的 tree + server 端 `filter_accessibility_tree`），輸出加 `search_mode: "tree_filter"`、`root_element_id: null`、`stop_reason: null`、`web_accessibility: null`，match 的 `depth`／`ancestors` 為 `null`。`value` 條件在 fallback 也支援（server 對 node 的 `value` 欄位做子字串比對；runner 對 secure／protected 本來就回 `null`）。`max_depth` 在 fallback 一律用 8。
3. 有 `root_element_id` 而 runner 不支援 → 直接回 `capability_unavailable`。

能力檢查在 runner registry 的 `enqueue`（`requests.rs`），在派送之前就失敗，所以「先試再退回」沒有副作用，也不用另外查 capability。

---

## 4. Wire 協定與能力圍欄（rolling upgrade）

現有 `computer_accessibility_tree` 的 runner 處理**不檢查多餘欄位**，而 server 的 `validate_accessibility_tree` **嚴格拒絕多餘欄位**。所以：

- 不能把 `root_element_id` 塞進舊 kind：舊 runner 會默默忽略，回傳整個視窗的樹，看起來成功但其實錯了。
- 不能在舊 kind 的輸出加欄位：新 runner 配舊 server 會被判成 `invalid_runner_response`。

做法：參考 `computer_element_state` 的 rolling-upgrade 模式，新增一個能力、兩個新 kind。

| 項目 | 內容 |
|---|---|
| 能力 | `RUNNER_CAPABILITY_COMPUTER_ACCESSIBILITY_QUERY = "computer_accessibility_query"`；`RunnerCapabilities.computer_accessibility_query: bool`（`#[serde(default)]`，缺少時是 false）；`RunnerFeature::ComputerAccessibilityQuery` |
| 宣告 | `chadex-runtime-runner/src/lib.rs`：`cfg!(target_os = "macos")`（**Windows 不宣告**） |
| 新 kind | `computer_accessibility_subtree`（payload：`surface_id`、`root_element_id`（可為 null）、`max_depth`、`max_nodes`）；`computer_accessibility_find`（payload 見 §3.2） |
| 需要的 feature | 兩者都需要 `[ComputerAccessibilityObserve, ComputerAccessibilityQuery]` |
| payload 上限 | 沿用 `SHELL_COMPUTER_REQUEST_PAYLOAD_MAX_BYTES` |
| 舊 kind | `computer_accessibility_tree` 的輸入輸出完全不變。新 runner 收到舊 kind 時**仍會**執行 §1 的 Chromium 開啟（所以舊 server 也受惠），但不回報 `web_accessibility` |

server 的 `accessibility_tree` action：

- 有 `root_element_id` → `computer_accessibility_subtree`；`capability_unavailable` 直接回傳。
- 沒有 → 先送 `computer_accessibility_subtree`（root 為 null，可以拿到 `web_accessibility`）；`capability_unavailable` 時退回 `computer_accessibility_tree`。

subtree 輸出 = 現有 tree 欄位，加上：

```json
"root": {"element_id": "element_…", "absolute_depth": 14} | null,
"web_accessibility": "not_applicable" | "enabled" | "already_enabled" | "pending" | "unsupported" | "disabled" | "skipped_sensitive"
```

（`root.element_id` 就是 `nodes[0].element_id`；`skipped_sensitive` 在新 kind 實際上不會出現，因為 sensitive surface 會被擋。保留這個值給舊 kind 的內部記錄使用。）

server 新增 `validate_accessibility_subtree`（重用 `validate_accessibility_node`，另外驗證 `root`、`web_accessibility` 的列舉值）和 `validate_accessibility_find`（欄位封閉、`count == elements.len() ≤ limit`、`scanned_nodes ≤ MAX_FIND_VISITED`、element_id 不重複且格式正確、`depth ≤ 64`、`ancestors ≤ 256 bytes`、`stop_reason` 列舉）。model-facing 的輸出 schema（`output_schemas/computer.rs`）中，`computer_accessibility_tree` 新增 optional 的 `root`、`web_accessibility`；`computer_find_elements` 新增 `search_mode`、`root_element_id`、`web_accessibility`、`stop_reason`，match 新增 `depth`、`ancestors`（全部可為 null），`scanned_nodes` 上限從 256 放寬到 4000。

---

## 5. 對外 schema 與 model surface 的影響

- **不新增工具或 action。** `computer_observe` 的 action 數量維持 12（`computer_schemas.rs::computer_observe_schema_is_closed_read_only_action_union` 的 expected list 不變）。
- 新增屬性：`accessibility_tree` +`root_element_id`；`find_elements` +`root_element_id`、`value`、`max_depth`。gateway 的頂層 `properties` 聯集會多 `root_element_id`、`value` 兩個 `{}`（`max_depth` 已經有了）。
- 估計：full schema 約增加 600–800 bytes；compact 模式只改寫 common 描述，對 computer 屬性沒有額外縮減，所以增量差不多。`mcp_tools_list_compact_is_smaller_than_full_serialized` 不受影響。
- `computer_observe` 的 tool description 目前約 795 字元，硬上限是 900（`MODEL_TOOL_DESCRIPTION_MAX_CHARS`）。**不改 tool description**，說明都放在屬性 description。
- `ToolCall`（`chadex-runtime-tool-runtime-contracts/src/tool_call.rs`）：`AccessibilityTree` 加 `root_element_id: Option<String>`；`FindElements` 加 `root_element_id`、`value`、`max_depth`，全部 `#[serde(default)]`。enum 有 `deny_unknown_fields`，所以舊呼叫方式仍然有效，新欄位也能正確解析。
- 稽核：`tool_audit.rs::computer_observe_audit_projection` 的遮蔽清單要從 `["role", "subrole", "label"]` 改成 `["role", "subrole", "label", "value"]`，`value` 只記 `value_present: true`。`root_element_id` 是不透明 id，可以記錄。
- 新增 schema 大小護欄測試（仿照 `edit_schemas.rs::apply_text_edits_model_schema_size_is_bounded`）：S5 開始前先量 `computer_observe` input schema 的 serialized bytes 當基準，測試斷言 ≤ 基準 + 1024。

---

## 6. 安全

1. **批准／治理流程**：新參數掛在現有的 `ComputerObserveToolCall::AccessibilityTree`／`FindElements` variant 上，所以 `computer_observe_policy`（`SCOPE_COMPUTER_READ`）和 `govern_specialized_invocation` 自動適用，不需要另外的路徑。新的 wire kind 也走 registry 的 capability 圍欄。
2. **Sensitive surface**：新 kind（subtree、find）在 `ComputerRuntime` 裡，任何原生呼叫之前就先執行 `ensure_surface_not_sensitive(&record)?`，比現有 tree 更嚴。理由是深層搜尋讀到的內容比 8 層多很多。舊的 `computer_accessibility_tree` 行為不變（決策 D7）。
3. **Chromium 開啟**：sensitive surface 不設定；`Off` 時完全不設定。
4. **敏感欄位**：§3.2 第 2–5 點。另外，`root_element_id` 指向 protected 或 secure 節點時拒絕（§2.2）。
5. **Control 類完全不變**：deep find 拿到的 id 和一般 id 一樣，之後的 `press`／`focus`／`input_text`／`scroll_to_element` 仍然要經過 `ensure_surface_not_sensitive`、`has_positive_evidence`、`validate_text_input_target`（必須是空欄位、≤ 2048 bytes），以及 key 白名單。
6. **開啟是 app 層級，sensitive 是視窗層級**（實作後補充）：`AXManualAccessibility` 設在 application 元素上，所以同一個 Chromium／Electron app 的另一個一般視窗被觀察過之後，這個 app 的 sensitive 視窗（例如標題含 authentication）也會開始提供網頁內容。為了讓放行的範圍和開啟前一樣，對 sensitive surface 做的不帶 root 的 tree（新舊 kind 都一樣），在 `AXWebArea` 節點停止展開：保留 `AXWebArea` 節點本身（`child_count` 照實回報），子樹省略並標示 `truncated = true`。只在 Chadex 會開啟的情況下才這樣做（`Auto`、sensitive、判定為 Chromium／Electron），原生 app 與純 WebKit app 不受影響。帶 root 的 subtree 和 find 本來就擋 sensitive surface。
7. **已知缺口（不在這次修）**：瀏覽器的登入頁標題（例如 “Sign in – Google Accounts”）和密碼管理頁（`chrome://password-manager`）不在 `sensitive_auth_title` 的 marker 裡。深層搜尋會讓這些頁面能被讀得更深（密碼本身仍是 secure 欄位，不會讀出；但「顯示密碼」後的明文是 `AXStaticText`）。要不要加 marker（例如 `password manager`、`sign in`、`log in`）列為決策 D6。

---

## 7. Windows

- 這次 **Windows 不實作**新能力。Windows runner 不宣告 `computer_accessibility_query`，所以：
  - `accessibility_tree` 不帶 root → 退回舊 kind，行為不變。
  - `find_elements` 不帶 root → 退回 tree_filter（`search_mode: "tree_filter"`），`value` 條件可以用。
  - 帶 `root_element_id` → `capability_unavailable`。
- 背景說明（之後要做時參考）：Windows 上的 Chromium 在偵測到 UIA client 查詢時（`WM_GETOBJECT`／UIA provider 請求）會自動打開 accessibility，**不需要**類似 `AXManualAccessibility` 的開關，但第一次查詢後一樣要等樹建好。之後實作時，要用 §8 S1 的 traversal engine 做一個 UIA adapter：用 `ControlViewWalker`，並用 `IUIAutomationCacheRequest` 一次抓 `ControlType`、`IsPassword`、`Name`、`HelpText`、`AutomationId`、`RuntimeId`，減少跨 process 呼叫。密碼欄位用 `CurrentIsPassword`（已在 `uia_fingerprint`）。另外 `resolve_uia_element` 的 `index >= MAX_ACCESSIBILITY_NODES` 限制要配合 `MAX_FIND_CHILDREN_PER_NODE` 一起調整。

---

## 8. 實作步驟與驗收條件

依賴：S1 → S2 → S3；S4 的 core／registry 部分可以和 S1 並行，runner handler 要等 S2；S5 依賴 S4 的 wire 定義；S6 最後。

### S1. 平台無關的 traversal engine（`chadex-runtime-computer`，純 Rust）

- 新模組 `src/ax_traversal.rs`（`#[cfg(any(test, target_os = "macos", windows))]`）：
  ```rust
  pub(crate) trait AxSource {
      type Node: Clone;
      fn fingerprint(&self, node: &Self::Node, inherited_protected: bool) -> Result<ElementFingerprint, String>;
      fn value(&self, node: &Self::Node) -> Result<Option<String>, String>; // 只在安全時呼叫
      fn enabled_focused(&self, node: &Self::Node) -> Result<(Option<bool>, Option<bool>), String>;
      fn child_count(&self, node: &Self::Node) -> Result<usize, String>;
      fn children(&self, node: &Self::Node, take: usize) -> Result<Vec<Self::Node>, String>;
      fn child_at(&self, node: &Self::Node, index: usize) -> Result<Self::Node, String>;
      fn check_deadline(&self) -> Result<(), String>;
  }
  ```
  - `observe_tree(source, root, root_prefix: Option<&ElementRecord>, bounds) -> AccessibilityTreeResult`（現有 BFS 行為原樣移過來，另外支援 prefix）。
  - `find(source, root, prefix, query, bounds, clock) -> FindResult`（§3.2）。
  - `resolve(source, window_root, record) -> Node`（從現有 `resolve_correlated_element` 抽出來）。
  - `ancestors_summary(...)`、`node_matches_find_query(...)`。
- `ElementRecord.lineage` 改成 `Vec<Arc<ElementFingerprint>>`（§2.3），macOS、Windows 呼叫端跟著改。
- **驗收**：用 `FakeAxTree`（記錄每個 method 呼叫）的單元測試全部通過，涵蓋：
  - 不帶 prefix 的 `observe_tree` 輸出和舊實作逐欄位一致（用同一棵假樹比對 golden）。
  - prefix：path、lineage 正確串接；depth 相對；絕對深度 64 截斷。
  - find：各種條件的組合、limit、`total_matches`、`stop_reason` 的五種值（時間用注入的 clock）、children-per-node 上限。
  - **secure／protected 節點和它們的子孫從來沒有呼叫 `value()`**；protected 節點沒有讀 title；`value` 條件不會符合它們。
  - `ancestors`：截斷、UTF-8 邊界、protected 祖先只顯示 role。
  - `resolve`：find／subtree 產生的 record 可以解析回同一個節點；改 title、重排子節點後回 `stale_element`。
  - 現有 `chadex-runtime-computer` 的測試全部通過（Linux 上的 unsupported 平台測試也是）。

### S2. macOS adapter、subtree／find runtime

- `impl AxSource for MacAxSource<'_>`（包住 `AXUIElement` 和 `AxObservationDeadline`，重用 `optional_ax_*`、`ax_array_count`、`ax_elements`、`ax_element_at`）。
- `platform::accessibility_tree` 改用 engine（輸出不變）；新增 `platform::accessibility_subtree`、`platform::find_elements`。
- `ComputerRuntime::accessibility_subtree`、`ComputerRuntime::find_elements`：參數驗證、`ensure_surface_not_sensitive`、root 驗證（§2.2）、`replace_surface`。非原生平台的 `mod platform` stub 回 `unsupported_platform`。Windows 平台模組新增同名函式，回 `unsupported_platform`。
- 可選優化（不是驗收條件）：如果 `objc2-application-services` 有 `AXUIElementCopyMultipleAttributeValues` 的 binding，就把第 1 步和第 3 步的屬性各合併成一次 IPC。要加測試證明合併後的 fingerprint 和逐一讀取的結果相同。
- **驗收**：registry 測試（subtree／find 取代舊 id、generation +1、root 用新 id 重新發、別的 surface 的 root → `stale_element`、已淘汰的 → `stale_element`、protected root → `permission_denied`）；sensitive surface 在原生呼叫前就被擋（加一個 `#[cfg(test)]` 的 surface 注入 helper）；新增 `#[ignore]` live 測試 `computer_macos_find_deep_chromium_live_smoke`、`computer_macos_subtree_chromium_live_smoke`。

### S3. Chromium 開啟（macOS）

- `classify_web_engine`（純函式）、`WebAccessibilityPolicy`（`ComputerConfig`）、memo、`wait_for_web_content`（可注入 probe／clock）、`enable_web_accessibility(pid, surface) -> WebAxState`。在三種觀察 kind 中，於 `exact_ax_window` 之後呼叫。
- **驗收**：用 tempdir 建假的 bundle（仿照 `macos_live_tests.rs::create_test_application`）測試 Electron、allowlist、Renderer helper 通用規則和 None；等待邏輯測試（第一次等到內容 → `enabled`；有 memo → `already_enabled` 而且 probe 次數為 0；一直空 → `pending` 且總等待 ≤ 1.95 s；deadline 剩不多時縮短等待）；`Off` 時 setter 的呼叫次數為 0；sensitive surface 時 setter 的呼叫次數為 0（setter 透過 trait 注入）；`AttributeUnsupported` → `unsupported`，觀察照樣成功。

### S4. 協定、registry、runner

- `chadex-runtime-core`：能力常數、`RunnerCapabilities` 欄位（serde default false）、`RunnerComputerOperationKind::{AccessibilitySubtree, AccessibilityFind}` 的 `wire_kind`／`from_wire`。
- `chadex-runtime-runner-registry`：`RunnerFeature::ComputerAccessibilityQuery`（`capabilities.rs` 的各個 match）、`requests.rs` 的 kind → feature 對照。
- `chadex-runtime-runner-config`、`runtime-engine/src/runner_ws.rs`、`runner_quic.rs`：預設值補上 `false`。
- `chadex-runtime-runner`：`lib.rs` 只在 macOS 宣告；`webcodex_runner/computer.rs` 處理兩個新 kind，用 `ensure_exact_payload_fields` 嚴格解析；runner 從 env 讀 `CHADEX_COMPUTER_WEB_ACCESSIBILITY` 傳進 `ComputerConfig`。
- **驗收**：能力的 serde 測試（缺少 → false、有值 → true）；registry 對沒有能力的 runner enqueue 新 kind 會拒絕（仿照 `tests/computer_accessibility.rs`）；runner payload 多欄位或少欄位 → `invalid_request`；舊 kind 的 payload 解析不變；registration 測試確認 macOS 宣告、非 macOS 不宣告。

### S5. Contracts 與 server

- `input_schemas/computer.rs`、`tool_call.rs`、`tool_audit.rs`（遮蔽 `value`）、`output_schemas/computer.rs`、`computer_tools.rs`（§3.4、§4 的派送與退回、`validate_accessibility_subtree`、`validate_accessibility_find`、server 端 `value` 過濾）。
- **驗收**：
  - `computer_schemas.rs`：`find_elements` 的 present 清單加 `root_element_id`、`value`、`max_depth`；`accessibility_tree` 有 `root_element_id`；schema instance 驗證接受新欄位，拒絕錯誤型別。
  - `tool_audit.rs`：`value` 的明文不會出現在 request summary。
  - `computer_tools_tests.rs`：新 runner 走 deep；舊 runner（`capability_unavailable`）且沒有 root → tree_filter，並帶 `search_mode`；有 root 而且是舊 runner → `capability_unavailable`；validator 拒絕多餘欄位、超量 `scanned_nodes`、重複 id、錯誤的 `stop_reason`。
  - schema 大小護欄測試（§5）。
  - `model_surface`、`mcp_tests::tools`（compact／full）、`tool_runtime/tests/schema/*` 全部通過。

### S6. 文件與交接

- 在本文件後面補上實作和設計的差異，以及 §9 實機測試的結果欄位；更新 `HANDOFF.md`／`PHASES.md`（如果這件事要變成一個 phase）。

### 每個步驟的驗證指令（Rust 建置名額允許時才跑）

```
cargo test -p chadex-runtime-computer
cargo test -p chadex-runtime-core
cargo test -p chadex-runtime-runner-registry
cargo test -p chadex-runtime-runner
cargo test -p chadex-runtime-tool-contracts
cargo test -p chadex-runtime-tool-runtime-contracts
cargo test -p chadex-runtime-engine computer_   # 再加 model_surface、mcp_tests
cargo clippy --workspace --all-targets
```

另外需要在 Windows CI 確認 `ElementRecord.lineage` 改成 `Arc` 之後，`windows_uia_tests.rs` 仍然可以編譯並通過。

---

## 9. 只能讓使用者實機測的項目

| # | 測試 | 要記錄的結果 |
|---|---|---|
| L1 | Brave、Chrome、Edge、Arc 各開一個一般網頁：第一次 `accessibility_tree` 的 `web_accessibility` 值、`AXWebArea` 底下的節點數（開啟前後比較）、第一次觀察花的時間 | 每個瀏覽器是否接受 `AXManualAccessibility` |
| L2 | Electron app（VS Code、Slack、Notion 或 Discord 擇一）同 L1 | 同上 |
| L3 | Google Sheets：用 `find_elements` 找名稱方塊、選單列、工具列按鈕、工作表分頁、儲存格內容（`value`）；分別在 Sheets 的「螢幕閱讀器支援」開啟和關閉時測試 | 哪些元素找得到。預期儲存格格線是 canvas，大多找不到 |
| L4 | 長網頁（例如 Wikipedia 長條目）：無條件的深層搜尋，記錄 `scanned_nodes`、`stop_reason`、花費時間 | 4000／6 s 的預算是否合適 |
| L5 | 從 deep find 拿到的元素做 `element_state`、`press`（連結）、`scroll_to_element`；用 `root_element_id` 指向 `AXWebArea` 做子樹查詢 | 是否成功，以及 `stale_element` 出現的頻率 |
| L6 | 開啟前後瀏覽器的 CPU 和記憶體（活動監視器，重頁面如 Sheets） | 增加量 |
| L7 | 有安裝視窗管理工具（Rectangle、Magnet）或使用 Stage Manager 時：觀察後移動、縮放視窗 | 確認沒有動畫或位置異常（驗證沒有誤用 `AXEnhancedUserInterface`） |
| L8 | 一邊開 VoiceOver 一邊觀察，然後關掉 VoiceOver | VoiceOver 正常；Chadex 結束後瀏覽器正常 |
| L9 | 閒置 5 分鐘以上再觀察 | Chromium auto-disable 後，重設是否讓樹恢復 |
| L10 | 登入頁的密碼欄：tree／find 都不回 value；`value` 條件不會符合；`input_text` 被拒絕。另外記錄登入頁標題是否會觸發 sensitive 封鎖（預期不會，見 D6） | 安全行為 |
| L11 | `CHADEX_COMPUTER_WEB_ACCESSIBILITY=off` 啟動 runner | `web_accessibility = "disabled"`，網頁樹是空的 |
| L12 | Windows 上的 Chrome／Edge：`find_elements` 走 tree_filter；帶 root → `capability_unavailable` | 相容性 |

---

## 10. 需要使用者決定的事項

| # | 決策 | 建議 |
|---|---|---|
| D1 | 只對偵測到的 Chromium／Electron 設定，或對所有被觀察的 app 都嘗試 | 偵測 |
| D2 | `AXManualAccessibility` 不被接受時，是否提供 `AXEnhancedUserInterface` 作為 opt-in 退路（會有視窗動畫問題） | 先不做，看 L1 結果再決定 |
| D3 | 預設自動開啟（`Auto`，可以用 env 關掉），或預設關閉、由使用者開啟 | 預設 `Auto` |
| D4 | 是否在某個時機把 `AXManualAccessibility` 設回 false | 不復原 |
| D5 | 把開啟動作視為 `computer_observe`（唯讀工具）的一部分，或改成需要批准的 `computer_control` action | 放在 observe 裡，輸出揭露 |
| D6 | 是否擴充 `sensitive_auth_title`（password manager、sign in、log in…）。會影響現有 control 的封鎖範圍 | 建議加，但要另外開一個變更 |
| D7 | 新的「secure 子孫也視為 protected」以及「sensitive surface 擋觀察」，是否也套用到舊的 `accessibility_tree` | 這次只套用新路徑，之後再評估 |
| D8 | Windows 這次只回 `capability_unavailable`，之後再做 UIA adapter | 同意分階段 |

---

## 11. 風險

- Chromium 接受 `AXManualAccessibility` 是依據 Electron 和 Chromium 的實作，**還沒有在這台機器實測**（L1）。如果不接受，§1 的成果只剩 Electron。
- Google Sheets 的儲存格大多是 canvas 繪製，開了 AX 也只會改善外框和工具列。使用者「在 Sheets 裡定位元素」的需求只會部分改善；而名稱方塊不是空的，所以 `input_text` 規則仍然不能用它跳到儲存格（這是範圍外的限制）。
- Chromium 的網頁樹變動頻繁，path + 完整 lineage 比對會讓 `stale_element` 變多。這是安全上正確的行為，但會增加模型的重試次數。之後可以考慮把 `AXDOMIdentifier` 加進 fingerprint 的 positive evidence，但這會改到 `ElementFingerprint` 的跨平台結構，不在這次範圍。
- 深層搜尋每次最多約 4000 × 5 次 IPC。在很慢的頁面上會常常碰到軟預算，回傳部分結果；模型要靠 `root_element_id` 縮小範圍。

---

## 12. 實作狀態與設計差異

### 12.1 §10 決策（目前實作採用的預設，全部可調整）

| # | 實作方式 | 調整位置 |
|---|---|---|
| D1 | 只對偵測到的 Chromium／Electron 設定 | `web_accessibility.rs::classify_web_engine`、`enable_web_accessibility` |
| D2 | 沒有 `AXEnhancedUserInterface` 備用方案 | 未實作 |
| D3 | 預設 `Auto`；`CHADEX_COMPUTER_WEB_ACCESSIBILITY=off`（不分大小寫）關閉 | `WebAccessibilityPolicy`、runner `computer_config` |
| D4 | 不復原 | 未實作 |
| D5 | 放在 `computer_observe` 裡，輸出帶 `web_accessibility`，稽核紀錄也記錄這個值 | `tool_audit.rs` |
| D6 | 沒有擴充 `sensitive_auth_title` | 未改 |
| D7 | 加嚴只套用帶 root 的 subtree 和 find；不帶 root 的 `accessibility_tree` 與舊 kind 行為相同。見 12.3 第 1 點 | `TreeMode::{Legacy, Query}` |
| D8 | Windows 不宣告 `computer_accessibility_query`，帶 root 回 `capability_unavailable`，不帶 root 退回舊流程 | runner `lib.rs`、server fallback |

### 12.2 實作位置

- 遍歷引擎：`crates/chadex-runtime-computer/src/ax_traversal.rs`（`AxSource`、`observe_tree`、`find`、`resolve`、`ancestors_summary`、`probe_web_content`），假 AX 樹測試在 `src/ax_traversal/tests.rs`。
- Chromium 開啟：`src/web_accessibility.rs`（分類、等待、memo、policy），測試在 `src/web_accessibility/tests.rs`。
- macOS adapter：`platform/macos/accessibility.rs`（`MacAxSource`、`observe_accessibility_tree`、`accessibility_subtree`、`find_elements`）。
- 協定：`computer_accessibility_query` 能力；`computer_accessibility_subtree`／`computer_accessibility_find` 兩個 wire kind。
- Server：`src/tool_runtime/computer_tools.rs`（派送、fallback、`validate_accessibility_subtree`／`validate_accessibility_find`）。

### 12.3 與設計不同的地方

1. model-facing `accessibility_tree` 不帶 root 時，新 runner 也送 subtree wire kind（為了拿到 `web_accessibility`），但 runner 對「不帶 root 的 subtree」套用舊 tree 的語意（`TreeMode::LegacyRootless`）：不擋 sensitive surface，secure 子孫的處理（只傳遞 `AXProtectedContent`）、節點、記錄和原生讀取順序都和舊 kind 相同，輸出只多 `root: null` 與 `web_accessibility`。sensitive surface 上仍不寫 `AXManualAccessibility`，狀態回 `skipped_sensitive`。只有帶 root 的 subtree 和所有 find 才加嚴（擋 sensitive surface、secure 子孫視為 protected、root 不得是 protected／secure）。這符合 D7。測試：`rootless_subtree_is_the_legacy_walk_with_only_the_subtree_envelope_added`、`rootless_subtree_keeps_the_legacy_sensitive_surface_behavior`、`rooted_queries_refuse_sensitive_surfaces_before_any_native_call`。
2. `find` 找到 `limit + 1` 個符合者就停止（`stop_reason = "limit"`），不為了數 `total_matches` 繼續走到預算用完。`truncated` 一樣準確，而且不需要多走最多 4000 個節點。
3. 單一節點的子節點數超過 `MAX_FIND_CHILDREN_PER_NODE`（512），或剩餘 visit 預算不足以展開所有子節點時，`stop_reason` 是 `visit_budget`（列舉值不新增）。
4. 有指定 `root_element_id` 時，root 本身不會出現在 find 的結果裡（只有它的後代會），root 另外用新 id 回傳在 `root_element_id`。
5. `AxSource` 的 `enabled_focused` 拆成 `enabled`／`focused` 兩個方法（只在需要時才讀），另外新增 `role`（探測用，一次 IPC）和 `value(node, max_bytes)`。`value` 條件比對最多讀 4096 bytes（tree 輸出仍維持 256 bytes），否則網頁上的長文字只能比對前 256 bytes。
6. `resolve` 的 inherited 規則：secure 父節點底下的子節點，只有在記錄的子 fingerprint 已經是 `protected` 時才視為繼承 protected。這讓舊 tree 與新路徑產生的 record 都能解析回自己，也不改舊 tree 的行為。
7. `find` 的 payload 上限為 8 KiB（`SHELL_COMPUTER_ACCESSIBILITY_FIND_PAYLOAD_MAX_BYTES`），不沿用 4096：四個 256 bytes 的篩選字串，最壞情況 JSON 跳脫後會超過 4096。其他新 kind 沿用基準值。
8. Gateway 的 `computer_action_schema` 會把每個分支屬性的 `description` 全部移除，所以 §5 說的「說明放在屬性 description」不成立。實際做法是在 `computer_observe` 的 tool description 加一句（887／900 字元）。input schema 從 4175 bytes 增加到約 4440 bytes。
9. 探測（`probe_web_content`）：預算用完或深度不夠而沒有找到 `AXWebArea` 時，視為「還是空的」繼續等，只有整棵樹走完仍沒有 `AXWebArea` 才視為 `NoWebArea`（算 enabled）。
10. Chromium 開啟在 platform 函式裡、`exact_ax_window` 成功之後做，不是在 `ComputerRuntime`。舊的 `computer_accessibility_tree` kind 在 macOS 上也會做（不回報狀態）。sensitive surface 與 `Off` 不會寫屬性。
11. 遍歷引擎只在 `any(test, macos)` 編譯，沒有編進 Windows 正式版（沒有 Windows adapter，編進去只會有 dead code）。Windows 只做 `ElementRecord.lineage` 改 `Arc`，以及 `observe_accessibility_tree`／`accessibility_subtree`／`find_elements` 三個函式（後兩個回 `unsupported_platform`）。
12. `objc2-app-kit` 加了 `libc` feature、`objc2-foundation` 加了 `NSDate` feature（`runningApplicationWithProcessIdentifier`、`launchDate` 需要）。`Cargo.lock` 沒有變動。
13. 沒有實作 `AXUIElementCopyMultipleAttributeValues` 合併（設計標示為可選）。
14. 新增兩個 `#[ignore]` live 測試：`computer_macos_find_deep_chromium_live_smoke`、`computer_macos_subtree_chromium_live_smoke`。

### 12.3.1 審查後的修正

15. 等待時間（`web_accessibility.rs`）：整段等待（sleep 加 probe）有 3 秒的硬上限（`WAIT_TOTAL_BUDGET`），而且每一輪都用 `remaining()` 重新檢查，不會用到保留給走訪的 3 秒（`WAIT_RESERVE`）。probe 自己也收到剩餘預算，並在超時後以「無法確定」結束（`probe_web_content` 接收 clock 與 budget）。沒有剩餘預算時不 probe、不 sleep。
16. deep find 的軟預算：至少處理第一個節點（root）之後才檢查（`ax_traversal.rs`），所以 `scanned_nodes` 一定 ≥ 1，server 的驗證（1..=4000）不用改。選這個而不是放寬 server，是因為「回報 0 個節點的成功結果」本身就沒有意義。
17. 錯誤處理：probe 與設定屬性的錯誤，除了 `permission_denied` 與 deadline 逾時以外，一律視為暫時性錯誤（Chromium 重建樹時會回 `InvalidUIElement`、`CannotComplete`），繼續等待，最後回 `pending`／`unsupported`，觀察照常成功（符合 §1.3）。
18. memo 分兩種狀態（`WebMemo`）：`Confirmed`（probe 看到內容，或確定沒有 web area）和 `Waited`（等過一次但沒有確認，包含整輪排程跑完和預算用完兩種情況）。
    - 沒有 memo：照 §1.4 等待。看到內容寫 `Confirmed` 並回 `enabled`；否則寫 `Waited` 並回 `pending`。
    - 命中任何一種 memo：不 sleep，只做一次有預算的 probe（預算為剩餘時間扣掉 3 秒保留，最多 3 秒）。看到內容就升級成 `Confirmed` 並回 `already_enabled`；沒看到就降成 `Waited` 並回 `pending`；暫時性錯誤也當作沒有確認。沒有 probe 預算時，兩種 memo 都回 `pending`，memo 不變（沒有新證據）。
    - 所以 `already_enabled` 現在一定是「這次 probe 確認過內容」的結果（沒有預算可以 probe 時回 `pending`），`pending` 的網頁每次觀察最多多花一次 probe，不會再花整段等待。這同時處理了 Chromium 閒置後 auto-disable 的情況（L9），仍建議實機確認。
19. 見 §6 第 6 點（sensitive 視窗不展開 `AXWebArea`）。這對舊 kind 是刻意加嚴：sensitive 視窗加上 Chromium／Electron 時，即使使用者自己開了 VoiceOver（其他工具先打開 accessibility），該視窗的網頁內容也不會出現在 Chadex 的 tree 裡；原生 app 與純 WebKit app 不受影響。
20. `ComputerObserveToolCall::FindElements.value` 改用 `RedactedString`（序列化與一般字串相同，`Debug` 只印位元組數）。`RunnerComputerOperation` 的 `Debug` 對 `InputText`、`WriteClipboard`、`AccessibilityFind` 不印 payload。`RunnerRequest.stdin` 的 `Debug` 是既有行為，所有 kind 都會印，這次沒有改。
21. `web_context_for(record)` 把「sensitive surface 就不開啟」的決定集中到一處並有測試；tree_filter 的 `value` 可以命中 secure 欄位的子孫（舊 tree 本來就公開這些資料），註解已說明。

### 12.3.2 需要使用者確認或實機驗證的行為

- 新 macOS runner 上，不帶 root 的 `find_elements` 對 sensitive surface 會回 `permission_denied`（find 一律加嚴）。以前走 server 的 tree_filter 時可以搜，這是既有 action 的行為改變。
- memo 命中後 Chromium 可能已經 auto-disable：現在每次命中都會再 probe 一次（見 §12.3.1 第 18 點），`already_enabled` 代表剛剛確認過有內容；實機行為仍留給 L9。

### 12.4 實機測試結果（§9）

尚未執行。請依 §9 的表格記錄：

| # | 結果 |
|---|---|
| L1 | |
| L2 | |
| L3 | |
| L4 | |
| L5 | |
| L6 | |
| L7 | |
| L8 | |
| L9 | |
| L10 | |
| L11 | |
| L12 | |
