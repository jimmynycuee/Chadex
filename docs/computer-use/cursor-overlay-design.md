# Computer Use 代理游標疊加層（Cursor Overlay）設計

- 狀態：已依 [§14 實作紀錄](#14-實作紀錄) 的預設決定實作（macOS）；§12 仍待使用者確認，預計 v0.6.1
- 範圍：macOS 實作；Windows 只寫設計（[§9](#9-windows-設計備註本版不實作)）
- 基準：`origin/main` @ `dd74d05`
- 讀者：實作 runtime／helper／macOS App 的 agent。每一段都標出要改的檔案與驗收條件。

## 0. 摘要

Computer Use 執行 click、press、focus、scroll、input、key 等動作時，在目標位置顯示一個「代理游標」或高亮框，讓使用者看得出 AI 正在操作哪裡。疊加層是**純視覺、best effort**：

1. Runner 在**每個有效果的動作之前**（所有驗證與敏感畫面檢查都通過後、第一個 native effect 之前）送出 `will_act` 事件，動作結束後送出 `finished`。送出動作只有「編碼＋非阻塞 `try_send`」，**不等任何回覆**。
2. 通道：Runner 目前沒用到的 **stdout** → helper 既有的 process supervisor 機器事件 drain（`MachineEventEmitter`，QuickShare／RegularTunnel 已在用）→ helper 既有的 Swift NDJSON stdout，新增一種**不帶 `request_id` 的 event frame**。
3. App 用一個 `NSPanel`（borderless、nonactivating、`ignoresMouseEvents = true`、`sharingType = .none`、不會成為 key window、跨 Spaces 與全螢幕）在目標處畫游標或外框，自動淡出。開啟「減少動態效果」時不做移動與漣漪動畫。
4. 被封鎖的敏感畫面**不顯示** overlay：事件只在 sensitive／protected 檢查通過後才送出，被拒的動作不會產生任何事件。
5. 使用者可在 Computer Use 頁關閉（預設開啟，待確認）。

---

## 1. 現況資料流（讀碼結論）

```text
ChatGPT ──tools/call──▶ OpenAI tunnel-client ──▶ helper MCP ingress
                                                   │  ComputerSafetyController.authorize()
                                                   │  （Ask 模式在這裡等使用者批准）
                                                   ▼
                                     chadex-runtime-server（loopback）
                                                   │ Runner protocol（網路 transport）
                                                   ▼
                     chadex-runtime-runner（helper 的 child process）
                       └ webcodex_runner/computer.rs::handle_computer_operation
                           └ chadex-runtime-computer::ComputerRuntime（真正送 CGEvent／AX）

Swift App ◀──NDJSON stdout（只有 request/response）── chadex-helper ◀── stdin
```

證據與要點：

| 事實 | 位置 |
| --- | --- |
| 有效果的動作只有：`pointer_move`／`pointer_click`、`control`（`press`／`focus`）、`scroll_to_element`、`input_text`、`key_input`、`activate_window`、`launch_application`、`write_clipboard`。目前**沒有** drag 與滾輪捲動 | `runtime-engine/crates/chadex-runtime-core/src/runner_operation.rs`（`RunnerComputerOperationKind`）、`runtime-engine/crates/chadex-runtime-runner/src/webcodex_runner/computer.rs` |
| Pointer 目標在 runtime 內已換算成 **CG 全域座標（points，主螢幕左上為原點，y 向下）**：`target = CGDisplayBounds.origin + (x / source_width) * bounds.size`，snapshot 像素 → points 的 Retina 縮放已在這裡完成 | `chadex-runtime-computer/src/platform/macos/input.rs::map_macos_pointer_coordinate`、`PointerPlan.target_x/target_y` |
| AX 動作（press／focus／scroll／input）在 `resolve_correlated_element` 之後、`perform_action` 之前沒有讀元素 frame；但已有 `optional_ax_point(…, "AXPosition")`／`optional_ax_size(…, "AXSize")` 可用，座標同樣是 CG 全域 points | `platform/macos/accessibility.rs`（`control`、`scroll_to_element`、`ax_window_geometry_matches`） |
| 敏感畫面檢查：`ensure_surface_not_sensitive`、`ensure_application_not_sensitive`、pointer 的 `ensure_pointer_target_not_sensitive`（枚舉所有視窗）、AX 的 `contains_protected_content`／secure text | `chadex-runtime-computer/src/lib.rs`、`platform/mod.rs` |
| Chadex 自己的截圖：整個螢幕用 `CGDisplayCreateImage`，單一視窗用 xcap（`CGWindowListCreateImage` 只含該視窗） | `platform/macos/capture.rs`、`platform/mod.rs` |
| Runner 由 helper 直接 spawn（`--stop-on-stdin-eof`），stdin/stdout/stderr 都是 pipe；runner **完全不寫 stdout**（全部 `eprintln!`） | `rust-helper/src/chadex_core/runtime_compat/integration/bridge.rs::local_runner_command`、`runtime_compat/process/registry.rs::spawn_owned` |
| Helper 已有「child stdout 是機器事件 NDJSON」的機制：`spawn_owned(kind, command, machine_stdout: true)` 回傳 `MachineEventInbox`（容量 64、保留 critical、溢位時回 `machine_event_overflow`）。LocalRunner 目前傳 `false` | `runtime_compat/process/events.rs`、`runtime_compat/state/coordinator.rs`（LocalRunner 四個 spawn 點） |
| Helper → App 只有 request/response，沒有推播。App 端 `consumeStdout` 把**任何解不成 `HelperResponse` 的行都當成協定錯誤並 `failAllPending`** | `rust-helper/src/runtime_bridge.rs::run_async／write_response`、`Sources/ChadexApp/HelperClient.swift::consumeStdout`、`Models.swift::HelperResponse` |
| Activity timeline（`ActivityLog`，200 筆 ring）只能用 `queryActivities` 輪詢；App 的輪詢間隔是 1 s／5 s／15 s | `runtime_compat/activity.rs`、`docs/ARCHITECTURE.md#polling` |
| Computer 偏好存在 `ChadexPreferences`；控制 UI 在 `ProjectDetailView` 的 `.computerUse` 頁 | `Sources/ChadexApp/ProjectStore.swift`、`ProjectDetailView.swift::computerControlSection` |

---

## 2. 通道選擇

### 2.1 決定

| 段 | 通道 | 沿用或新增 |
| --- | --- | --- |
| Runner → helper | Runner 的 **stdout fd**，每行一個 JSON；helper 用既有 `spawn_owned(..., machine_stdout: true)` ＋ `drain_stream` ＋ `MachineEventEmitter` 接收 | **沿用** QuickShare／RegularTunnel 的機器事件管線。Runner stdout 目前是空的，不會跟 log 混在一起 |
| Helper → App | 既有 NDJSON stdout，新增 **event frame**（有 `event`、沒有 `request_id`） | **沿用**傳輸；協定新增一種 frame。`protocol_version` 維持 `1`（App 與 helper 同版發布，見 `BRIDGE_PROTOCOL.md`），App 必須先認得 event frame |
| App 開關 | 新 method `setComputerOverlayEvents { enabled }` | 新增 RPC；helper 預設**不轉送**，App 啟用後才送，避免 App 沒準備好時收到 frame |

### 2.2 為什麼不沿用 activity timeline

- 只能輪詢，最快 1 s，遠超過延遲預算（[§4](#4-時序與延遲預算)）。
- `ActivityEntry.message` 是給人看的、會經過 sanitizer 的字串；座標不該變成 timeline 上的雜訊，也不該進入「匯出診斷資料」。
- 200 筆 ring buffer 會被高頻動作洗掉真正重要的生命週期事件。

診斷只保留計數器（[§6.4](#64-診斷計數)），不寫 ActivityLog。

### 2.3 其他被否決的方案

| 方案 | 否決原因 |
| --- | --- |
| Helper ingress 看 `tools/call` 參數自己算位置 | ingress 只有 snapshot 像素座標或 `element_id`，沒有元素 frame，也不知道 runner 的驗證會不會在之後擋下動作；可能在錯的位置畫游標 |
| 透過 server 回傳的 tool result 帶座標 | 只有「之後」，沒有「之前」；也會改到模型看得到的輸出 |
| Unix datagram socket（runner → helper） | 非阻塞語意很好，但要多管一個 socket 路徑與權限，Windows 沒有 AF_UNIX datagram。stdout pipe 加上 runner 端的非阻塞 writer thread 一樣不會卡住動作 |
| 一律推送、不用 `setComputerOverlayEvents` | App 關掉功能時 helper 仍會寫 stdout；而且舊的 `consumeStdout` 對未知行會 `failAllPending`，必須由 App 明確打開 |

### 2.4 已知限制

- 只有 **helper 自己 spawn 的 local runner** 有 overlay。遠端 runner、使用者自行啟動的 runner、QuickShare 內嵌 runtime 都沒有（`setComputerOverlayEvents` 會回 `runner_channel: "detached"`）。
- Runner 重啟（換專案、reload 失敗等）期間沒有 overlay；新 runner 起來後自動接上。

---

## 3. 事件格式（schema v1）

### 3.1 座標系統

只用一種座標空間：

- `space = "macos_cg_global_pt"`：Quartz 全域顯示座標，**單位是 points**，原點在**主螢幕（含選單列的螢幕，`CGMainDisplayID()`）左上角**，x 向右、y **向下**。其他螢幕可能是負座標。`CGDisplayBounds`、`CGEvent` location、AX `AXPosition`／`AXSize`、xcap 視窗 x/y 都用這個空間。
- Retina：points 與 AppKit 的 points 相同，App **不需要乘 backingScaleFactor**。runtime 已經在 `map_macos_pointer_coordinate` 把 snapshot 像素換成 points。
- 事件附帶 `display`（`CGDirectDisplayID` 與 runtime 當下看到的 `CGDisplayBounds`），App 用它檢查螢幕配置是否在這段時間內改變（[§7.3](#73-座標換算多螢幕)）。
- Windows 將來用 `space = "windows_virtual_screen_px"`（per-monitor-v2 物理像素）。App 收到不認識的 `space` 一律丟棄。

### 3.2 Runner → helper（runner stdout，每行一個 JSON，≤ 1 KiB）

```json
{
  "event": "computer_overlay",
  "schema": "chadex.computer_overlay.v1",
  "token": "3f6c…（helper 每次 spawn 產生的 128-bit hex）",
  "seq": 42,
  "phase": "will_act",
  "action_id": 17,
  "action": "click",
  "target": { "kind": "point", "x": 812.5, "y": 433.0 },
  "space": "macos_cg_global_pt",
  "display": { "id": 69733378, "bounds": { "x": 0, "y": 0, "width": 1512, "height": 982 } },
  "ttl_ms": 2000,
  "emitted_at_ms": 1791500000123
}
```

```json
{
  "event": "computer_overlay",
  "schema": "chadex.computer_overlay.v1",
  "token": "3f6c…",
  "seq": 43,
  "phase": "finished",
  "action_id": 17,
  "outcome": "succeeded",
  "emitted_at_ms": 1791500000171
}
```

欄位：

| 欄位 | 型別 | 說明 |
| --- | --- | --- |
| `event` | `"computer_overlay"` | 固定值；helper 據此分流 |
| `schema` | `"chadex.computer_overlay.v1"` | 版本。欄位不相容時換 `v2`；helper 對不認識的 schema 丟棄，不猜欄位 |
| `token` | hex string | helper 以環境變數 `CHADEX_COMPUTER_OVERLAY_TOKEN` 傳入，runner 讀取後立即從自身環境移除。不符就丟棄（縱深防禦，見 [§8.5](#85-偽造與注入)） |
| `seq` | u64 | runner 行程內單調遞增；helper 用它丟棄亂序 |
| `phase` | `"will_act"` \| `"finished"` | |
| `action_id` | u64 | runner 行程內單調遞增；`will_act` 與 `finished` 配對 |
| `action` | enum | `move`、`click`、`press`、`focus`、`scroll`、`input`、`key`、`activate`；保留 `drag`、`wheel`（目前 runtime 沒有這些動作，App 必須能處理） |
| `target` | object | `{"kind":"point","x","y"}`、`{"kind":"rect","x","y","width","height"}`（左上角＋尺寸）、`{"kind":"path","from":{x,y},"to":{x,y}}`（保留給 drag）、`{"kind":"none"}` |
| `space` | enum | 見 §3.1 |
| `display` | object \| null | point／rect 所在螢幕。AX 元素取不到時可為 null，App 改用包含目標中心點的螢幕 |
| `key` | object \| 省略 | 只在 `action = "key"` 時出現：`{"name":"enter","modifiers":["command"]}`。`name` 與 `modifiers` 都是 runtime 既有的封閉詞彙（`key_code`、`validate_key_modifiers`），不含任何使用者文字 |
| `ttl_ms` | u32 | 沒收到 `finished` 時 App 最多顯示多久；runtime 預設 2000，App clamp 到 [500, 5000] |
| `outcome` | enum | 只在 `finished`：`succeeded`、`failed`、`not_started`、`unknown` |
| `emitted_at_ms` | u64 | wall-clock epoch ms；App 用來丟棄積壓過久的事件（> 1000 ms） |

**刻意不帶的資訊**：輸入文字或長度、剪貼簿、元素 title／role／description、應用程式名稱、視窗標題、`surface_id`／`element_id`、錯誤訊息全文。overlay 只需要幾何與動作種類。

動作與目標對應：

| Runtime 動作 | `action` | `target` | 取得方式 |
| --- | --- | --- | --- |
| `computer_pointer_move` | `move` | point | `PointerPlan.target_x/target_y` |
| `computer_pointer_click` | `click` | point | 同上 |
| `computer_control` `press` | `press` | rect（取不到時 none） | `AXPosition`＋`AXSize` |
| `computer_control` `focus` | `focus` | rect／none | 同上 |
| `computer_scroll_to_element` | `scroll` | rect／none | 同上（捲動前的位置） |
| `computer_input_text` | `input` | rect／none | 同上 |
| `computer_key_input` | `key` | rect（焦點視窗）／none | `exact_ax_window` 的 `AXPosition`＋`AXSize` |
| `computer_activate_window` | `activate` | rect／none | 同上 |
| `computer_launch_application`、`computer_write_clipboard`、所有觀察類動作 | 不送事件 | — | 沒有可指的位置 |

`outcome` 對應：`Ok` 且結果 `success == true` → `succeeded`；`Ok` 但 `success == false` → `failed`；`Err` 以 `not_started:` 開頭 → `not_started`；`Err` 含 `outcome_unknown` → `unknown`；其他 `Err` → `failed`。

### 3.3 Helper → App（event frame）

```json
{"protocol_version":1,"event":"computer_overlay","data":{"v":1,"seq":42,"phase":"will_act","action_id":17,"action":"click","target":{"kind":"point","x":812.5,"y":433.0},"space":"macos_cg_global_pt","display":{"id":69733378,"bounds":{"x":0,"y":0,"width":1512,"height":982}},"ttl_ms":2000,"emitted_at_ms":1791500000123}}
```

- 與 runner 事件相同，但**移除 `token`、`event`、`schema`**，改成 `data.v = 1`。
- Helper 另外會送 `{"phase":"clear","reason":"stopped|runner_exited|overflow|disabled|session_ended"}`，App 收到後立刻隱藏。
- Frame 判別：**有 `event` 且沒有 `request_id`** 的行是 event frame。App 不認識的 `event` 名稱直接忽略，不能影響 pending request。

### 3.4 新 RPC：`setComputerOverlayEvents`

```json
{"protocol_version":1,"request_id":"…","method":"setComputerOverlayEvents","params":{"enabled":true}}
```

回應 `result`：`{"enabled":true,"runner_channel":"attached"|"detached"|"unsupported"}`。

- Helper 預設 `enabled = false`。App 在 helper（重新）啟動後、以及使用者切換開關時呼叫。
- `enabled = false` 時 helper 立刻送一次 `clear`，之後丟棄所有 runner 事件（仍持續 drain，避免 runner 的 pipe 塞滿）。
- `unsupported`：非 macOS helper。

### 3.5 驗證規則（helper 端，全部 fail closed → 丟棄該行）

- 整行 ≤ 1 KiB（`drain_stream` 的機器行上限是 16 KiB；overlay 驗證再收緊）。
- `serde(deny_unknown_fields)`；`schema`、`token`、`event` 完全相符。
- 數值：有限值，`|x|, |y| ≤ 100_000`，`0 < width, height ≤ 32_768`；`ttl_ms ≤ 10_000`。
- `action`、`target.kind`、`space`、`outcome`、`key.name`、`key.modifiers` 都是封閉列舉。
- `seq` 必須大於上一個已接受的 `seq`（同一個 runner 生命週期內）。
- `finished` 的 `action_id` 必須等於最近一個 `will_act` 的 `action_id`；不符就丟棄。

---

## 4. 時序與延遲預算

### 4.1 時序（以 `pointer_click` 為例）

```text
helper ingress   authorize（Ask 模式在這裡等使用者，overlay 不參與）
      │
runner           validate → prepare_pointer → ensure_pointer_target_not_sensitive
      │          ── emit will_act (seq n) ──▶ [overlay channel，try_send，不等待]
      │          spend snapshot generation → move → settle(≤50 ms) → down/up
      │          ── emit finished (seq n+1) ─▶ [try_send]
      ▼          回傳 tool result（耗時與沒有 overlay 時相同）

runner writer thread ─write─▶ pipe ─▶ helper drain thread ─▶ MachineEventInbox
                                          ─▶ overlay forwarder task（驗證、開關）
                                          ─▶ stdout writer ─▶ App readabilityHandler
                                          ─▶ main queue ─▶ panel 定位、orderFrontRegardless
```

### 4.2 延遲預算（`will_act` 寫出 → overlay 第一個可見 frame）

| 區段 | 目標 p95 | 備註 |
| --- | --- | --- |
| Runner 編碼＋`try_send`（**動作執行緒上唯一的新成本**） | ≤ 50 µs | 不配置大型 buffer；字串 ≤ 1 KiB |
| Writer thread → pipe → helper drain → 驗證 → 轉送 | ≤ 2 ms | 全是記憶體與本機 pipe |
| App 讀 stdout → main queue → 設 frame → 合成 | ≤ 1 個 display frame（≈ 16.7 ms） | `DispatchQueue.main.async` 保持 FIFO |
| **總計** | **≤ 33 ms**；硬性上限 1000 ms，超過丟棄 | |

AX 動作多一筆讀 frame 的成本（`AXPosition`＋`AXSize`，在動作執行緒上，用既有 `AxObservationDeadline` 限時）：目標**中位數 ≤ 2 ms、p95 ≤ 5 ms**，只在 runner 有 overlay sink 時才讀。這是「讀資料」，不是等待 App。超過預算的處理見 [§12](#12-待使用者決定) 第 7 項。

### 4.3 「之前」的實際意義

Runtime 不能等待，所以 `will_act` 與真正的 effect 之間只差幾十微秒。AX press 幾乎同時發生；pointer click 在 move 與 down 之間有最多 50 ms 的 settle，overlay 通常與點擊同時出現。實際體驗是「**同時出現＋殘留約 1 秒**」，使用者能看出 AI 剛剛點了哪裡、正在操作哪個元素，但看不到「提前預告」。真正的提前預告需要 runtime 刻意延遲或新增 preview 能力，列為待決（[§12](#12-待使用者決定) 第 4、6 項），v0.6.1 不做。

### 4.4 不阻塞保證（每一層）

| 層 | 機制 | 失敗時 |
| --- | --- | --- |
| Runner 動作執行緒 | 只做 `SyncSender::try_send`（容量 32）；`Full`／`Disconnected` 立即返回並遞增 `dropped` | 動作照常執行 |
| Runner writer thread | 專屬 thread 阻塞在 `write_all`；helper 不讀時只有這條 thread 卡住 | `EPIPE`（Rust binary 預設忽略 SIGPIPE）→ thread 結束，之後 `try_send` 皆回 `Disconnected` |
| Helper drain | 既有 `spawn_blocking` drain，解析後放進有界 inbox，不會反壓 runner | inbox 溢位 → `machine_event_overflow` → 轉成 `clear` |
| Helper forwarder | 獨立 task，用容量 32 的 `tokio::mpsc`；`try_send`，滿了就丟最舊的 `will_act` | App 卡住只影響 forwarder，不影響 ingress 與 runtime |
| Helper stdout | 與 response 共用 `Arc<Mutex<Stdout>>`，每次只持有寫一行的時間 | App 不讀 stdout 時 response 本來就會卡住；overlay 不會讓情況更糟 |
| App | `consumeStdout` 在背景 thread 分流，event 用 `DispatchQueue.main.async` | App 沒在跑 → helper 隨 stdin EOF 結束，runner 隨之結束（既有生命週期） |

---

## 5. Runtime 設計（`runtime-engine/`）

### 5.1 `chadex-runtime-computer`：事件型別與 sink

新增 `src/overlay.rs`（純 Rust，不碰 IO）：

```rust
pub trait ComputerOverlaySink: Send + Sync {
    /// 必須立即返回；實作不得阻塞或做 IO。
    fn emit(&self, event: ComputerOverlayEvent);
}

pub enum OverlayAction { Move, Click, Press, Focus, Scroll, Input, Key, Activate }
pub enum OverlayTarget { Point { x: f64, y: f64 }, Rect { x: f64, y: f64, width: f64, height: f64 }, None }
pub enum OverlayOutcome { Succeeded, Failed, NotStarted, Unknown }
pub struct OverlayDisplay { pub id: u32, pub bounds: (f64, f64, f64, f64) }
pub enum ComputerOverlayEvent {
    WillAct { action_id: u64, action: OverlayAction, target: OverlayTarget,
              display: Option<OverlayDisplay>, key: Option<OverlayKey>, ttl_ms: u32 },
    Finished { action_id: u64, outcome: OverlayOutcome },
}
```

- `ComputerRuntime` 增加欄位 `overlay: Option<Arc<dyn ComputerOverlaySink>>` 與 builder `with_overlay_sink(self, sink)`。**不要**把 sink 放進 `ComputerConfig`（它是 `Copy + PartialEq`）。
- RAII guard `OverlayActionGuard`：`begin(action, target, …)` 送 `WillAct`；`finish(&result)` 依 §3.2 對應送 `Finished`；若在 `finish` 前 drop（提前 `?` 返回或 panic），送 `Finished { outcome: Unknown }`。sink 為 `None` 時 guard 不做事，也不讀 AX frame。
- `outcome` 對應寫成純函式 `overlay_outcome(&Result<Value, String>) -> OverlayOutcome`。
- `ttl_ms` 預設 2000，常數放在 `overlay.rs`。

### 5.2 送出點（都在最後一道驗證之後、第一個 native effect 之前）

| 函式 | 送出點 |
| --- | --- |
| `ComputerRuntime::pointer_effect`（`lib.rs`） | `platform::ensure_pointer_target_not_sensitive` 成功之後、`dispatch_after_spending_pointer_generation` 之前。目標取自新的 `PointerPlan::overlay_target()`（macOS 用 `target_x/target_y`、`native_display_id`、bounds） |
| `platform::control`、`scroll_to_element`、`input_text`（macOS `accessibility.rs`／`input.rs`） | `resolve_correlated_element` 與 capability 檢查通過之後、`prepare_ax_call`／`perform_action` 之前。這幾個 platform 函式新增參數 `before_effect: &mut dyn FnMut(OverlayTarget, Option<OverlayDisplay>)`；由 `lib.rs` 傳入 closure 建立 guard |
| `platform::key_input` | `validate_key_input_target` 與 `deadline.ensure_remaining()` 之後、`CGEvent::post_to_pid` 之前 |
| `platform::activate_window` | `AXRaise` 之前 |

AX frame 讀取：`optional_ax_point(deadline, element, "AXPosition")` ＋ `optional_ax_size(deadline, element, "AXSize")`；任何錯誤、`None`、非有限值、零尺寸 → `OverlayTarget::None`，**絕不讓 frame 讀取失敗變成動作失敗**。`display` 用包含 rect 中心點的 `CGGetDisplaysWithPoint` 結果；取不到時為 `None`。

Windows platform 函式要同步改簽名（CI 會編 Windows），本版一律呼叫 `before_effect(OverlayTarget::None, None)`，而 runner 在 Windows 不安裝 sink（[§9](#9-windows-設計備註本版不實作)）。

### 5.3 `list_windows` 過濾 overlay 視窗

Overlay 顯示時是一個真的螢幕上視窗，`xcap::Window::all()` 會列出它。`platform/mod.rs::list_windows` 新增純函式 `is_chadex_overlay_window(application, title) -> bool`：`title == "Chadex Agent Cursor"`（常數，App 端設成同一字串）且 `application` 以 `Chadex` 開頭時略過。`ensure_pointer_target_not_sensitive` 不需改（overlay 不是敏感視窗）。

### 5.4 `chadex-runtime-runner`：stdout sink

新增 `src/webcodex_runner/computer_overlay.rs`：

1. **啟動時**（`run_cli` 最前面，任何 thread 建立之前）：只在 macOS，且 `CHADEX_COMPUTER_OVERLAY=stdout-v1` 與 `CHADEX_COMPUTER_OVERLAY_TOKEN` 都存在時啟用：
   - `fcntl(1, F_DUPFD_CLOEXEC, 3)` 取得私有 fd；開 `/dev/null` 後 `dup2` 到 fd 1。之後任何 child process 或誤用的 `println!` 都只會寫到 `/dev/null`，不會混入通道。
   - 讀出 token 後 `std::env::remove_var` 兩個變數（edition 2021，單執行緒時呼叫），子程序不會繼承。
2. `StdoutOverlaySink { tx: SyncSender<Vec<u8>>, seq: AtomicU64, dropped: AtomicU64, token }`：`emit` 編碼成一行 JSON（`serde_json::to_vec`，附 `\n`），`try_send`；失敗只遞增 `dropped`。
3. Writer thread `chadex-computer-overlay`：`for line in rx { file.write_all(&line)? }`，錯誤即結束。
4. `computer.rs::computer_runtime()` 的 `OnceLock` 初始化時，若 sink 存在就 `ComputerRuntime::new(cfg).with_overlay_sink(sink)`。
5. 未啟用時行為與現在完全相同（stdout 仍未使用）。

---

## 6. Helper 設計（`rust-helper/`）

### 6.1 Runner spawn

- `integration/bridge.rs::local_runner_command`：在 macOS 加上 `CHADEX_COMPUTER_OVERLAY=stdout-v1` 與每次新產生的 `CHADEX_COMPUTER_OVERLAY_TOKEN`（`getrandom` 16 bytes → hex）。Token 要能讓 coordinator 取回（例如 `local_runner_command` 回傳 `(Command, OverlayChannelToken)`，或由 coordinator 產生後傳入）。
- `state/coordinator.rs` 的 LocalRunner 四個 `spawn_owned` 呼叫改成 `machine_stdout: true`，把回傳的 `MachineEventReceiver` 與 token 交給 `ComputerOverlayHub::attach_runner`。Runner 的 stdout 除了 `tracing` 的輸出之外沒有別的內容：通道啟用後 fd 1 指向 `/dev/null`，`tracing` 改寫 stderr（見 [§14](#14-實作紀錄)），所以 log 不會遺失，stderr 照舊進 log。

### 6.2 `chadex_core/computer_overlay.rs`（新）

```text
ComputerOverlayHub
  enabled: AtomicBool                      // 預設 false
  out: tokio::mpsc::Sender<Value>          // 容量 32，接到 run_async 的 stdout writer
  current: Mutex<Option<AttachedRunner>>   // token、last_seq、last_will_act_id、forwarder JoinHandle
  counters: forwarded / dropped_invalid / dropped_disabled / dropped_backpressure / overflows

attach_runner(inbox, token)  // 取消舊 forwarder，送 clear(runner_exited)，啟動新 forwarder
forwarder: while let Some(v) = inbox.recv().await {
    "machine_event_overflow" → clear(overflow)
    validate(v, token, last_seq) → Ok(frame) / Err(_) 計數丟棄
    enabled == false → 計數丟棄
    out.try_send(frame)；Full → 計數丟棄
  }
  inbox 關閉（runner 結束） → clear(runner_exited)
set_enabled(bool)            // false 時送 clear(disabled)
clear(reason)                // 送 {"phase":"clear","reason":…}
```

- `validate` 是純函式：輸入 `serde_json::Value`，輸出 helper → App 的 `data` 或錯誤碼，方便單元測試。
- Kill switch 連動：`TunnelManager::stop_computer_control`、`set_computer_control_mode(ReadOnly)`、`begin_session` 時呼叫 `hub.clear(...)`。在 Stop 狀態下，ingress 不會再放行任何 control，所以不需要額外擋事件。

### 6.3 `runtime_bridge.rs`

- `run_async`：建立 `(overlay_tx, overlay_rx)`，hub 放進 `Bridge`；另起一個 task `while let Some(frame) = overlay_rx.recv().await { write_event(stdout.clone(), &frame).await }`。`write_event` 與 `write_response` 共用同一把 `Mutex<Stdout>`，格式 `{"protocol_version":1,"event":"computer_overlay","data":…}\n`。Shutdown 時先 abort 這個 task 再寫 shutdown response。
- `handle_request` 加 `setComputerOverlayEvents`；`params.enabled` 必須是 bool，否則回 `invalid_params`。
- `docs/BRIDGE_PROTOCOL.md` 新增「Events」一節與 method 表列（本設計落地時一起改）。

### 6.4 診斷計數

計數器只在記憶體中，納入既有「匯出診斷資料」（`getStatus` 不加欄位，避免輪詢負擔；可加在 `queryLifecyclePerformanceTraces` 回應或新的 diagnostics 欄位，實作時擇一）。不記錄座標。

---

## 7. macOS App 設計（`Sources/ChadexApp/`）

新檔案：`ComputerOverlayPanel.swift`（視窗與繪製）、`ComputerOverlayController.swift`（事件 → 顯示邏輯）、`ComputerOverlayGeometry.swift`（純函式換算）、`ComputerOverlayModels.swift`（Decodable）。

### 7.1 HelperClient 接收 event frame

- `consumeStdout`：每一行先解一個輕量 envelope `struct HelperFrameEnvelope: Decodable { var event: String?; var requestId: String? }`。`event != nil && requestId == nil` → 走 event 路徑；否則照舊解 `HelperResponse`。
- **Event frame 解析失敗只丟棄並計數，絕不 `failAllPending`。** 不認識的 `event` 名稱忽略。
- 新增 `var onEvent: (@Sendable (HelperEventFrame) -> Void)?`（以 `lock` 保護讀寫）。在背景 thread 呼叫；`AppModel` 設定時用 `DispatchQueue.main.async` 轉回主執行緒，保持事件順序（不要用多個 `Task { @MainActor }`，順序不保證）。

### 7.2 Panel 設定

```swift
final class ComputerOverlayPanel: NSPanel {
    static let windowTitle = "Chadex Agent Cursor"   // runtime 以此過濾 list_windows

    init() {
        super.init(contentRect: .zero,
                   styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered, defer: true)
        title = Self.windowTitle
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        ignoresMouseEvents = true            // 點擊穿透到底下的 app
        sharingType = .none                  // 不出現在截圖／錄影（需驗證，見 §8.1）
        isFloatingPanel = true
        hidesOnDeactivate = false            // Chadex 通常不是前景 app
        becomesKeyOnlyIfNeeded = true
        isReleasedWhenClosed = false
        isExcludedFromWindowsMenu = true
        animationBehavior = .none
        level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.assistiveTechHighWindow)))
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary,
                              .stationary, .ignoresCycle]
        setAccessibilityElement(false)       // 不出現在 VoiceOver
    }
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}
```

- **顯示一律用 `orderFrontRegardless()`**，絕不呼叫 `makeKeyAndOrderFront`、`NSApp.activate`。隱藏用 `orderOut(nil)`（不是 alpha 0），這樣閒置時不在視窗列表中。
- Level 選 `assistiveTechHighWindow`（輔助技術游標用的層級，高於選單與 pop-up），AI 透過 AX press 選單項目時游標才不會被選單蓋住。因為 `ignoresMouseEvents` 加上很短的顯示時間，不會擋到任何系統對話框的操作。若實機發現蓋住系統 UI 造成困擾，退回 `.statusBar`。
- `.canJoinAllSpaces` 與 `.moveToActiveSpace` 互斥，只用前者；`.fullScreenAuxiliary` 讓它出現在其他 app 的全螢幕 Space；`.stationary` 讓 Mission Control 不移動它。
- 只用一個小 panel，依目標移動與縮放（point：64×64 pt；rect：元素 frame 外擴 6 pt）。不用每個螢幕一個全螢幕透明視窗：記憶體與合成成本較低，而且萬一某版 macOS 截到它，影響範圍也小。
- 內容用 layer-backed `NSView` ＋ `CAShapeLayer`（不用 SwiftUI），動畫與 reduce motion 分支可以精確控制：
  - point：箭頭游標形狀（accent color 填色、白色 1.5 pt 描邊）＋外圈。
  - rect：圓角 6 pt 外框（accent color 2 pt，「增加對比」開啟時 3 pt）。
  - `key`：在 rect 底部中央顯示按鍵 badge（例如 `⌘ ↩`），由 `key.name`／`key.modifiers` 對照表產生。
  - `input`：在 rect 左側顯示文字游標圖示，**絕不顯示文字**。
  - rect 面積超過螢幕的 60%：不畫外框，改在中心畫 point 樣式。
  - 不認識的 `action`（例如未來的 `drag`、`wheel`）：用 point 樣式畫在 target 中心或 `to`。
- 顏色用 `NSColor.controlAccentColor`，深淺色模式下都可辨識；`failed`／`not_started` 只把外圈改成 `systemOrange` 後淡出，不顯示文字。

> macOS 26 API：本設計不需要任何 macOS 26 才有的 API。若實作者想用新 API（例如 `NSGlassEffectView` 做 badge 底），必須包在 `#if compiler(>=6.2)` 內，再加 `if #available(macOS 26.0, *)`，否則提供舊路徑；CI 用 macos-15 的舊 SDK。巢狀型別不要取名 `State`（例：`OverlayVisibility`、`OverlayPhase`）。

### 7.3 座標換算（多螢幕）

純函式，全部可單元測試：

```swift
enum ComputerOverlayGeometry {
    /// CG 全域（左上原點、y 向下）→ AppKit 全域（主螢幕左下原點、y 向上）。
    /// primaryHeight = 主螢幕（frame.origin == .zero 的 NSScreen）的 frame.height。
    static func appKitRect(fromCG r: CGRect, primaryHeight: CGFloat) -> CGRect {
        CGRect(x: r.minX, y: primaryHeight - r.maxY, width: r.width, height: r.height)
    }
    static func appKitPoint(fromCG p: CGPoint, primaryHeight: CGFloat) -> CGPoint {
        CGPoint(x: p.x, y: primaryHeight - p.y)
    }
}
```

- `primaryHeight` 取 `NSScreen.screens.first { $0.frame.origin == .zero }`，再與 `CGDisplayBounds(CGMainDisplayID()).height` 交叉確認。**不要用 `NSScreen.main`**（那是 key window 所在的螢幕）。
- 用 `NSScreen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? CGDirectDisplayID` 找出 `display.id` 對應的螢幕；再比對事件中的 `display.bounds` 與 App 當下的 `CGDisplayBounds(id)`（容差 0.5 pt）。對不上，或找不到螢幕 → **丟棄事件**（螢幕配置在途中改變，畫在錯的位置比不畫更糟）。
- `display == nil` 時，用 `NSScreen.screens.first { $0.frame.contains(center) }`；找不到就丟棄。
- 換算後的 panel frame clamp 在該螢幕 `frame` 內（不是 `visibleFrame`，選單列區域也可能是目標）。
- 不同螢幕的 backing scale 不需處理：CG 與 AppKit 都是 points，CALayer 的 `contentsScale` 跟隨 panel 所在螢幕。
- 收到 `NSApplication.didChangeScreenParametersNotification` 時立刻隱藏 overlay。

### 7.4 動畫、減少動態效果、自動隱藏

`ComputerOverlayController`（`@MainActor`）的時間常數：

| 常數 | 值 |
| --- | --- |
| `finished` 後保留（succeeded） | 800 ms |
| `finished` 後保留（failed／not_started／unknown） | 1200 ms |
| 沒收到 `finished` 的上限 | `ttl_ms`，clamp [500, 5000] |
| 淡出 | 200 ms（減少動態效果時 150 ms） |
| 事件過期門檻 | `now - emitted_at_ms > 1000 ms` 丟棄 |
| 滑行（glide） | 160 ms ease-out；條件：上一個標記仍可見、在同一螢幕、距上次事件 < 1.5 s |
| 點擊漣漪 | 350 ms，scale 0.6 → 1.4、opacity 0.8 → 0 |

動畫決策寫成純函式，方便測試：

```swift
struct OverlayAnimationPlan: Equatable {
    var glideDuration: TimeInterval?   // nil = 直接定位
    var appearDuration: TimeInterval   // 0 = 立即出現
    var ripple: Bool
    var fadeOutDuration: TimeInterval
}
static func plan(action: OverlayActionKind, reduceMotion: Bool,
                 previous: OverlayVisibility, sameScreen: Bool, sinceLast: TimeInterval) -> OverlayAnimationPlan
```

- 減少動態效果開啟：`glideDuration = nil`、`ripple = false`、不做縮放，直接出現（`appearDuration = 0`），只保留淡出（交叉淡化屬於可接受的效果）。
- 跨螢幕移動永遠不滑行：先隱藏再在新位置出現。開啟「顯示器有個別空間」時，跨顯示器的視窗只會顯示在其中一個顯示器上。
- 讀取：`NSWorkspace.shared.accessibilityDisplayShouldReduceMotion`，並監聽 `NSWorkspace.shared.notificationCenter` 的 `NSWorkspace.accessibilityDisplayOptionsDidChangeNotification`。Controller 以注入的 `reduceMotion: () -> Bool` 與 `now: () -> Date`（或 `ContinuousClock`）建構，測試可替換。
- 新的 `will_act` 取消待執行的隱藏計時；`finished` 的 `action_id` 不等於目前顯示中的 `action_id` 就忽略。
- 其他立即隱藏的時機：`clear` frame、helper 斷線（`HelperClient` termination）、使用者關掉開關、App 按下 Stop、`NSWorkspace.willSleepNotification`、`NSWorkspace.sessionDidResignActiveNotification`、App 結束。

### 7.5 設定與生命週期

- `ChadexPreferences` 新增 `var computerCursorOverlay: Bool?`，`nil` 視為開啟（沿用 `prepareServiceOnLaunch` 的 optional 慣例）；計算屬性 `computerCursorOverlayEnabled`。
- UI：`ProjectDetailView.computerControlSection` 的控制模式下方加一個 `Toggle`。L10n key：`computer.cursorOverlay.title`（zh-Hant「顯示代理游標」／en "Show agent cursor"）、`computer.cursorOverlay.help`（說明：只在本機顯示，不會出現在截圖與螢幕錄影中）。兩個 `Localizable.strings` 都要加。
- `AppModel`：
  - helper 啟動成功後（`startIfNeededAsync` 之後的既有流程）與 toggle 變更時呼叫 `setComputerOverlayEvents`；失敗只記錄，不顯示錯誤（overlay 是 best effort）。
  - 擁有 `ComputerOverlayController`，第一次收到事件才建立 panel（lazy）。
  - 關閉開關：先本地隱藏，再送 RPC。

---

## 8. 隱私與安全

### 8.1 Overlay 不能出現在任何截圖裡

主要機制是 `sharingType = .none`。各種截圖路徑的預期：

| 截圖路徑 | 預期 | 依據／需驗證 |
| --- | --- | --- |
| Chadex `computer_observe` 單一視窗截圖（xcap → `CGWindowListCreateImage` 只含目標視窗） | 不會包含，結構上就排除 | overlay 是另一個視窗 |
| Chadex 整個螢幕截圖（`CGDisplayCreateImage`） | 預期不包含 | **必須實機驗證**（macOS 14／15／26） |
| 系統截圖 ⌘⇧3／⌘⇧4／⌘⇧5 與螢幕錄影 | 預期不包含 | **必須實機驗證**；macOS 15 起系統截圖改走 ScreenCaptureKit，Apple 不保證 ScreenCaptureKit 會遵守 `NSWindowSharingNone` |
| 第三方 ScreenCaptureKit 錄影、會議軟體分享畫面 | 依 OS 版本 | 實機驗證；結果記入 release notes |

驗證方式：

1. **自動化 live test**（`#if` 不需要；以 `XCTSkipUnless(ProcessInfo.processInfo.environment["CHADEX_LIVE_CAPTURE_TESTS"] == "1")` 守門，需要 test runner 有螢幕錄製權限，所以不在 CI 跑）：建立 `ComputerOverlayPanel`，在已知位置畫一塊純洋紅色，`orderFrontRegardless()`，等兩個 runloop，然後用 `CGDisplayCreateImage(CGMainDisplayID())`，以及 macOS 14+ 的 `SCScreenshotManager.captureImage`，檢查該區域中心像素**不是洋紅色**。同一個測試也斷言 `CGWindowListCopyWindowInfo` 中這個視窗的 `kCGWindowSharingState == 0`。
2. **手動步驟**列在 [§10.2](#102-需要使用者實機測試)。

若某版 macOS 驗證失敗（overlay 出現在截圖中），處理方式待使用者決定（[§12](#12-待使用者決定) 第 5 項）。設計上的退路：App 加 `OverlayCapturePolicy`，依 OS 主版本決定是否允許顯示；或後續讓 runtime 的整個螢幕截圖改用 ScreenCaptureKit 的 `SCContentFilter(display:excludingWindows:)` 主動排除 overlay 視窗。

### 8.2 敏感畫面：**不顯示**（決定）

- 事件只在 `ensure_surface_not_sensitive`、`ensure_application_not_sensitive`、`ensure_pointer_target_not_sensitive`、protected／secure text 檢查全部通過後才送出。動作被這些規則擋下時**不會有任何 `will_act`**。
- 理由：
  1. 這些畫面上 AI 本來就不能操作，顯示游標會讓使用者誤以為 AI 正在密碼框或授權對話框上動作。
  2. 不顯示也代表不把「密碼對話框在哪裡」的幾何資訊送過 IPC。
  3. overlay 是高 level 視窗，不在認證對話框上方畫東西，可以避免視覺上遮住或混淆系統安全 UI。
- 殘留情況：overlay 顯示期間，若有認證對話框剛好跳出，標記最多再停留 `ttl_ms`。因為 `ignoresMouseEvents`，它不會擋到任何輸入。可接受。

### 8.3 不帶內容

事件與 frame 不含輸入文字、剪貼簿、元素或視窗名稱、應用程式名稱、錯誤全文（見 §3.2）。`key` 只帶封閉詞彙的按鍵名稱。Overlay 不寫入 ActivityLog、performance trace 或 audit；診斷只有計數。

### 8.4 不干擾 Computer Use 本身

- `ignoresMouseEvents = true`：CGEvent 點擊會穿透 overlay，落在底下的 app。
- 不成為 key／main window、不 activate：不會搶焦點，因此不會讓 `key_input` 的「焦點視窗必須是目標」檢查失敗。
- `list_windows` 過濾（§5.3）：模型看不到 overlay 視窗。
- AX：overlay 屬於 Chadex 行程，不會出現在其他 app 的 AX tree；而且設了 `setAccessibilityElement(false)`。

### 8.5 偽造與注入

- 只接受 helper 自己 spawn 的 runner 的 stdout；runner 啟動時把通道 fd 設成 `CLOEXEC`，並把 fd 1 改指 `/dev/null`，子程序無法寫入這條通道。
- 每次 spawn 都用新 token，而且 runner 會從自身環境移除 token。**Token 不是祕密**：同一 uid 的程序可以用 `sysctl KERN_PROCARGS2` 讀到行程啟動時的初始環境（含 token），移除環境變數只是不讓 runner 的子程序繼承。真正的防偽造是 CLOEXEC 的私有 fd：只有 runner 行程持有通道，其他程序（包含 runner 的子程序）寫不進去；token 只是縱深防禦與丟棄誤送資料用。
- App 只信任 helper stdout。即使被偽造，影響也只限於畫一個游標，不會觸發任何動作。

---

## 9. Windows 設計備註（本版不實作）

- **視窗**：在 `apps/windows/src-tauri` 用 Rust 建立原生 Win32 視窗（不用 WebView）：`WS_POPUP`，擴充樣式 `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`，用 `UpdateLayeredWindow` 畫 premultiplied ARGB。`ShowWindow(SW_SHOWNOACTIVATE)`，永遠不 activate。
- **截圖排除**：`SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`（Windows 10 2004+）。呼叫失敗或系統較舊時**不顯示 overlay**；不要退回 `WDA_MONITOR`，它會在截圖中留下黑框。
- **座標**：Windows runtime 的 `PointerPlan.global_x/global_y` 已是 per-monitor-v2 物理像素虛擬螢幕座標，`space = "windows_virtual_screen_px"`。Overlay 執行緒必須是 `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`，直接用物理像素定位，不需換算。
- **減少動態效果**：`SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION)` 為 `FALSE` 時視同 reduce motion。
- **通道**：runner 端用 `DuplicateHandle` 取得不可繼承的 stdout handle，再把 `STD_OUTPUT_HANDLE` 指到 `NUL`；helper 端沿用同一個 forwarder；`apps/windows/bridge` 把 event frame 轉成 Tauri event 給 overlay 模組（不經前端 JS）。
- **`list_windows` 過濾**：Windows 的視窗枚舉也要排除同一個 title sentinel。

---

## 10. 測試計畫

### 10.1 自動化（單元與整合）

**Runtime（`chadex-runtime-computer`、`chadex-runtime-runner`）**

| 測試 | 內容 |
| --- | --- |
| 事件編碼 golden | `WillAct`／`Finished` 各種 target 序列化成 §3.2 的 JSON（欄位名稱、列舉字串、無多餘欄位、≤ 1 KiB） |
| outcome 對應 | `overlay_outcome` 對 `Ok(success true/false)`、`not_started:`、`outcome_unknown`、其他錯誤 |
| Guard | 提前 `?` 返回與 `panic`（`catch_unwind`）時都送出 `Finished{Unknown}`；`finish` 後 drop 不重送 |
| sink 為 None | 用計數 sink 驗證：沒有 sink 時不呼叫 frame reader（以注入的 frame reader closure 計數） |
| 送出順序 | 用 fake platform（現有 `*_with` 注入模式，例如 `prepare_macos_pointer_plan_with`）驗證：驗證失敗或 sensitive 被擋 → 0 個事件；成功 → `will_act` 先於 dispatch closure、`finished` 在後 |
| 非阻塞 | `StdoutOverlaySink` 寫到沒人讀的 pipe：灌 10 000 筆，每次 `emit` < 1 ms，`dropped > 0`，沒有 deadlock；讀端關閉後 writer thread 結束、`emit` 仍立即返回 |
| fd 隔離 | 啟用後 spawn `sh -c 'echo leak'`，通道讀端讀不到 `leak` |
| `list_windows` 過濾 | `is_chadex_overlay_window` 純函式表格測試 |

**Helper（`rust-helper`）**

| 測試 | 內容 |
| --- | --- |
| `validate` | 合法事件通過；token 錯、schema 錯、多餘欄位、超出範圍數值、未知 action、`seq` 倒退、`finished` 配對錯 → 丟棄並遞增對應計數 |
| 開關 | `enabled=false` 時不轉送；切成 false 會送一次 `clear(disabled)` |
| Runner 結束 | inbox 關閉 → `clear(runner_exited)`；`attach_runner` 換新 runner 時舊 forwarder 被取消 |
| 溢位 | `machine_event_overflow` → `clear(overflow)` |
| Kill switch | `stop_computer_control` → `clear(stopped)` |
| 不阻塞 ingress | forwarder 的 `out` 接收端永不讀取（channel 塞滿）時，`handle_request` 的其他 method 仍在時限內回應；overlay 計數 `dropped_backpressure` 增加 |
| 協定 | `setComputerOverlayEvents` 參數錯誤回 `invalid_params`；event frame 序列化格式 golden |

**Swift（`Tests/ChadexAppTests`）**

| 測試 | 內容 |
| --- | --- |
| HelperClient event frame | 仿 `HelperClientResilienceTests` 的 shell 假 helper：先輸出一行 event frame、一行壞掉的 event frame、一行未知 event，再輸出 response → request 正常完成，`onEvent` 收到一次 |
| 座標換算 | 單螢幕；副螢幕在右；副螢幕在左（負 x）；副螢幕在上（CG 負 y）；副螢幕在下；不同高度的主副螢幕；rect 跨越邊界時 clamp |
| 螢幕比對 | `display.id` 找不到、bounds 不符 → 丟棄；`display == nil` 時用中心點找螢幕 |
| 動畫計畫 | `OverlayAnimationPlan.plan` 在 reduce motion 開／關、同螢幕／跨螢幕、間隔長／短時的輸出 |
| 自動隱藏 | 注入時鐘：`finished` 後 800／1200 ms 隱藏；沒有 `finished` 時依 clamp 後的 `ttl_ms`；新 `will_act` 取消隱藏；過期事件丟棄；`clear` 立即隱藏 |
| Panel 設定 | 建立 `ComputerOverlayPanel` 後斷言 `sharingType == .none`、`ignoresMouseEvents`、`!canBecomeKey`、`!canBecomeMain`、`collectionBehavior` 含 `.canJoinAllSpaces`／`.fullScreenAuxiliary`、`hidesOnDeactivate == false`、`title == windowTitle` |
| 偏好 | 仿 `PreferencesTests`：沒有 key 時解碼為開啟；寫回後保留 |
| 截圖排除 live test | 見 §8.1，`CHADEX_LIVE_CAPTURE_TESTS=1` 才跑 |

### 10.2 需要使用者實機測試

在 macOS 14、15、26（手邊有的版本）各做一次：

1. **基本**：Ask 模式批准一次 `pointer_click` → 目標點出現游標與漣漪，約 1 秒後淡出；AX `press` → 元素外框；`input_text` → 外框加文字游標圖示，沒有任何文字；`key_input`（例如 ⌘↩）→ 視窗底部 badge。
2. **截圖排除**：overlay 顯示中按 ⌘⇧3、⌘⇧4 框選、⌘⇧5 錄影、QuickTime 螢幕錄影、任一會議軟體分享整個螢幕，以及讓 ChatGPT 立刻呼叫 `computer_observe` 整個螢幕截圖 → 都看不到 overlay（建議暫時用 debug build 把顏色改成洋紅色，比較好判斷）。
3. **多螢幕**：外接螢幕放在左邊、上方，與主螢幕不同解析度與縮放（Retina＋非 Retina）→ 在兩個螢幕上操作時位置都準確；動作途中拔掉或重排螢幕 → overlay 消失，不會出現在錯的位置。
4. **Spaces 與全螢幕**：目標 app 在全螢幕 Space、在另一個 Space、切換 Space 時 → overlay 出現在目標所在的畫面，不會把使用者帶回 Chadex。
5. **焦點**：overlay 顯示中，`key_input` 送到目標 app 仍然成功；Chadex 不會跳到前景；點擊穿透到底下的元素。
6. **減少動態效果**：系統設定 → 輔助使用 → 顯示 → 減少動態效果，開啟後連續操作 → 沒有滑行、沒有漣漪，直接出現再淡出；不重啟 App 也能即時切換。
7. **開關**：在 Computer Use 頁關閉 → 立刻消失、之後不再出現；重開 App 後設定保留。
8. **斷線與 Stop**：overlay 顯示中按 Stop → 立即消失；結束 helper（Activity Monitor）→ overlay 消失、App 重新連線後恢復。
9. **敏感畫面**：讓 AI 嘗試點擊「鑰匙圈存取」或系統認證對話框 → 動作被拒，且**沒有** overlay。
10. **耗時**：同一組動作（例如 20 次 AX press、20 次 pointer click）在開關開／關下比較 tool 耗時，差異要在 §4.2 的預算內（用既有 performance trace 匯出比較）。
11. **深淺色與增加對比**：兩種外觀下都清楚可見。

---

## 11. 實作步驟拆分與驗收條件

三段可依序交給不同 worker；runtime 與 App 的純邏輯可以平行做，整合以 helper 為準。每段都**不改模型看得到的工具輸出**。

### 11.1 Runtime（`runtime-engine/`）

範圍：
- `crates/chadex-runtime-computer/src/overlay.rs`（新）：事件型別、`ComputerOverlaySink`、`OverlayActionGuard`、`overlay_outcome`、JSON 編碼。
- `crates/chadex-runtime-computer/src/lib.rs`：`with_overlay_sink`；在 `pointer_effect`、`control`、`scroll_to_element`、`input_text`、`key_input`、`activate_window` 接上 guard。
- `crates/chadex-runtime-computer/src/platform/macos/{input.rs,accessibility.rs}`：`PointerPlan::overlay_target()`、`before_effect` 參數、AX frame 讀取。
- `crates/chadex-runtime-computer/src/platform/windows/*`：同步簽名，回 `OverlayTarget::None`。
- `crates/chadex-runtime-computer/src/platform/mod.rs`：`is_chadex_overlay_window` 與 `list_windows` 過濾。
- `crates/chadex-runtime-runner/src/webcodex_runner/computer_overlay.rs`（新）、`computer.rs`、`lib.rs::run_cli` 開頭的 fd 與環境設定。

驗收：
- §10.1 Runtime 測試全部通過；既有 computer、runner 測試不變（不刪、不放寬）。
- 未設定環境變數時，runner stdout 仍然完全沒有輸出（加一個測試）。
- 被 sensitive、stale、permission 檢查擋下的動作不產生事件（測試）。
- Windows target 可以編譯（CI）。

### 11.2 Helper（`rust-helper/`）

範圍：
- `src/chadex_core/computer_overlay.rs`（新）＋ `mod.rs` 註冊。
- `src/chadex_core/runtime_compat/integration/bridge.rs::local_runner_command`：環境變數與 token。
- `src/chadex_core/runtime_compat/state/coordinator.rs`：LocalRunner 改 `machine_stdout: true`，inbox 交給 hub。
- `src/chadex_core/tunnel.rs`：Stop、改成 read_only、新 session 時呼叫 `hub.clear`。
- `src/runtime_bridge.rs`：event writer task、`setComputerOverlayEvents`。
- `docs/BRIDGE_PROTOCOL.md`：Events 一節與 method 表。

驗收：
- §10.1 Helper 測試全部通過。
- 沒有 App 呼叫 `setComputerOverlayEvents` 時，helper stdout 只有 response（測試）。
- Runner 結束、溢位、Stop 都會送 `clear`。
- 既有 helper 測試數量不減少。

### 11.3 macOS App（`Sources/ChadexApp/`）

範圍：
- `HelperClient.swift`：envelope 分流、`onEvent`，event frame 永不 `failAllPending`。
- `Models.swift` 或 `ComputerOverlayModels.swift`：`HelperEventFrame`、`ComputerOverlayEventData`、`SetComputerOverlayEventsParams`。
- `ComputerOverlayPanel.swift`、`ComputerOverlayController.swift`、`ComputerOverlayGeometry.swift`（新）。
- `ProjectStore.swift`：`computerCursorOverlay`。
- `AppModel.swift`：開關、啟動時同步、Stop 與斷線時隱藏。
- `ProjectDetailView.swift`：Toggle；`Resources/{en,zh-Hant}.lproj/Localizable.strings`。

驗收：
- §10.1 Swift 測試全部通過（包含 panel 設定斷言）。
- 不使用 macOS 26 專屬 API；如果使用，必須有 `#if compiler(>=6.2)` 加 `#available` 保護，且 macos-15 SDK 的 CI 能建置。
- 巢狀型別沒有命名為 `State`。
- §10.2 實機清單由使用者完成，結果記入 PR 或 release notes。

### 11.4 整合 gate

- 依 repo 慣例跑 `scripts/test.sh`（Swift＋helper）與 runtime 的 computer／runner crate 測試，再跑完整 CI。本設計階段沒有執行任何建置或測試。
- 實機 §10.2 第 2 項（截圖排除）是**放行條件**：任何受支援的 macOS 版本若驗證失敗，依 §12 第 5 項的決定處理後才能發布。

---

## 12. 待使用者決定

1. **預設開或關**：建議預設開啟（`nil` → 開）。
2. **開關位置**：建議放在 Computer Use 頁的控制模式下方；是否也要放進設定視窗？
3. **敏感畫面不顯示**：本設計已選「不顯示」（§8.2），請確認。
4. **「同時＋殘留」是否足夠**：runtime 不等待，overlay 實際上與動作同時出現。若要真正提前預告，可在 overlay 開啟時讓 runtime 於 effect 前固定延遲（例如 150 ms），但這會增加每個動作的耗時，違反本次「不影響耗時」的要求。建議 v0.6.1 不做。
5. **某版 macOS 截圖仍截到 overlay 時**：(a) 該 OS 版本自動停用 overlay（建議作為發布時的預設處理）；(b) 照常顯示，在開關說明中警告；(c) 投入後續工作，把 runtime 的整個螢幕截圖改成 ScreenCaptureKit 並排除 overlay 視窗。
6. **Ask 模式等待批准時預覽目標**：最有「提前預告」價值，但需要 runtime 新增「解析目標但不執行」的能力，並讓 helper 在 approval 期間顯示。建議列為 v0.6.x 之後的獨立項目。
7. **AX 讀 frame 的額外耗時**：若實機量測超過 p95 5 ms，選擇 (a) 接受；(b) AX 動作改成只畫視窗外框（不讀元素 frame）；(c) 改用 observation 時快取的 frame（可能過時）。
8. **`key` badge 是否顯示按鍵名稱**：詞彙是封閉的，不含使用者內容；建議顯示。
9. **Phase 紀錄**：`PHASES.md` 的 `current_phase` 仍是 `V050`（`integration/skill-parity`），沒有 v0.6.1／cursor overlay 的 phase。是否新增 phase 由使用者或 orchestrator 決定；本設計 commit 沒有修改 `PHASES.md`。

## 13. 風險

| 風險 | 影響 | 緩解 |
| --- | --- | --- |
| ScreenCaptureKit 不遵守 `sharingType = .none`（macOS 15+ 系統截圖、會議軟體） | 使用者的截圖或錄影出現代理游標 | §8.1 實機 gate；§12 第 5 項的政策；overlay 只用小視窗、短時間顯示，降低外洩範圍 |
| `CGDisplayCreateImage` 截到 overlay | 模型看到自己的游標，可能誤判畫面 | live test；失敗時依 §12 第 5 項處理（a 或 c） |
| AX frame 讀取在目標 app 忙碌時變慢 | AX 動作耗時增加 | 用既有 deadline 限時，失敗就退回 `None`；§12 第 7 項 |
| `assistiveTechHighWindow` 蓋到系統 UI | 視覺干擾 | 很短的顯示時間加上 `ignoresMouseEvents`；必要時退回 `.statusBar` |
| Helper stdout 多一種 frame | 舊版 App 會 `failAllPending` | Helper 預設不送，只有 App 明確啟用後才送；App 與 helper 同版發布 |
| Runner 不是 helper spawn 的 | 沒有 overlay | `runner_channel: "detached"`，屬已知限制（§2.4） |

## 14. 實作紀錄

macOS 已實作（`feature/computer-cursor-overlay`）。使用者確認 §12 之前，以下預設決定生效；每一項只改一處就能調整。

| §12 | 目前預設 | 要改的地方 |
| --- | --- | --- |
| 1 預設開關 | 開（`computerCursorOverlay` 為 `nil` 視為開） | `ChadexPreferences.cursorOverlayEnabled(for:)`（`ProjectStore.swift`） |
| 2 開關位置 | 只放在 Computer Use 頁（控制模式區塊之後，Stop 狀態下也看得到） | `ProjectDetailView.computerCursorOverlayToggle` |
| 3 敏感畫面 | 不顯示：事件只在敏感檢查之後送出 | runtime 送出點，見 §5.2；測試在 `overlay_runtime_tests` |
| 4 提前預告 | 不延遲：同時出現＋殘留 | 無，刻意不做 |
| 5 截圖排除失敗 | 尚未做自動停用；**發布前 gate**，需實機驗證 §10.2 第 2 項 | `ComputerOverlayPanel.swift` 的 `TODO(release gate)` |
| 6 Ask 批准前預覽 | 不做 | 無 |
| 7 AX frame | 照設計：讀取失敗就不畫框（`OverlayTarget::None`），讀取有獨立 150 ms 預算，不佔動作的 AX 預算 | `overlay_frame_for_element`（`platform/macos/accessibility.rs`） |
| 8 按鍵 badge | 只顯示動作類型（鍵盤圖示），不顯示按鍵名稱；runtime 仍送封閉詞彙的 key | `ComputerOverlayStyle.showsKeyNames`（`ComputerOverlayGeometry.swift`） |

與前文不同之處：

- **平台函式簽名**：`control`／`scroll_to_element`／`input_text`／`key_input`／`activate_window` 在 macOS 多一個 `&mut OverlayActionGuard` 參數（不是 closure）。Windows 與不支援的平台維持原簽名，由 `lib.rs` 的 `overlay_platform` 轉接層略過 guard；Windows 因此沒有送出點，與「Windows 不安裝 sink」一致。
- **Runner 的 tracing**：`tracing_subscriber` 預設寫 stdout，不是設計假設的「完全不寫 stdout」。通道啟用後 fd 1 指向 `/dev/null`，所以 tracing 在此時改寫 stderr，診斷仍進 helper 的 log。
- **Token**：由 `ComputerOverlayHub::prepare_runner_command` 產生並寫入 spawn 的 `Command`（不是 `local_runner_command` 回傳 token）。Hub 是整個 helper 行程唯一的實例（`install_shared_hub`），由 `run_async` 安裝；沒有安裝時（單元測試）Runner 用舊命令列啟動。
- **Helper 轉送佇列滿了**：丟棄新的 frame 並計數（`tokio::mpsc` 無法丟最舊的）；App 端的 `ttl_ms` 自動隱藏兜底。
- **診斷計數**：目前只放在 `setComputerOverlayEvents` 的回應（`counters`），尚未接進「匯出診斷資料」。
- **Space／螢幕**：`NSWorkspace.activeSpaceDidChangeNotification` 沒有接（設計的隱藏時機清單沒有它）；`.canJoinAllSpaces` 讓標記跟著使用者。
- **Live 截圖測試**：`CGDisplayCreateImage` 在 macOS 15 SDK 已被標為 obsoleted，Swift 無法呼叫；`ComputerOverlayLiveCaptureTests` 只涵蓋 `kCGWindowSharingState` 與 ScreenCaptureKit。Runtime 整個螢幕截圖（`CGDisplayCreateImage`）是否含 overlay 仍要照 §10.2 手動確認。
- **Frame 讀取的位置（TOCTOU）**：AX frame 讀取是慢速 IPC（上限 150 ms），所以一律在**最後一道安全驗證之前**讀好（`OverlayActionGuard::prepare_frame`），驗證通過後只做 `begin_prepared`（非阻塞 `try_send`）就進入 effect。`key_input`／`input_text`／`control`／`scroll_to_element`／`activate_window` 都照此順序；一步完成的 `begin(read)` 只留在測試中，避免寫回舊模式。frame 讀取的時間仍計入動作自己的 wall-clock deadline（它是獨立的 deadline 物件，但時間照樣流逝）。
- **使用者關閉 overlay 時 runner 仍會讀 frame**：helper 在 macOS 一律替 Runner 開通道，runner 目前不知道 App 是否開啟 overlay，所以 `OverlayActionGuard` 只依「有沒有 sink」決定讀不讀。代價是在驗證之前多一次 AX 讀取（目標 p95 ≤ 5 ms，上限 150 ms），已不在 TOCTOU 空窗內。若要在關閉時完全省掉，需要 helper 把 enabled 狀態傳給 runner（例如受控通道或信號），本版沒有做。
- **`setComputerOverlayEvents` 串行**：App 端同一時間只送一個，送完若偏好又變了就再送一次最新值；helper 沒在跑時不送（也不會為此啟動 helper），等 `.started` 時再同步。
- **Runner 安裝結果有三態**：未啟用、啟用、fd 1 已重導但通道沒建起來；只要 fd 1 被重導，`tracing` 就寫 stderr。
