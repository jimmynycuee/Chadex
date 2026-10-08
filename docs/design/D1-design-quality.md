# D1 · Design Quality（macOS）

> 狀態：**Pending Validation**。本文件的評分來自實作者與一個獨立 AI reviewer，
> 不是外部設計評審；依 Goal 規則，未經外部（人類）評估前不宣稱達到得獎水準。

## 1. 驗收標準（開工前制定）

每個維度 1–5 分；通過門檻是每一維度 ≥ 4，且沒有未解決的 Critical / High。

| 維度 | 5 分的定義 | 可量測條件 |
| --- | --- | --- |
| A. 無障礙 | 任何介面大小都可讀、可用鍵盤與 VoiceOver 完成核心流程 | 文字 ≥ 10 pt（HIG macOS 最小值）；資訊性文字不使用 `tertiaryLabel`；狀態不只靠顏色；每個 icon-only 控制項有 label；步驟／狀態有 accessibility value；區段標題有 header trait |
| B. 平台慣例 | 看起來、用起來就是 Mac App | 每個主要指令都在選單列且有快捷鍵；Settings 在 App 選單、記住上次分頁；側欄 ≤ 2 層、可收合；系統色與系統元件 |
| C. 視覺一致性 | 一套字級、表面、間距與標題系統 | 字級走 `ChadexFontStyle`；編輯區用同一個表面 token；控制項欄位左緣對齊；同層標題同一種大小寫 |
| D. 互動與回饋 | 每個狀態都有立即且正確的回饋 | loading／empty／filtered-empty／error 各自有文案；主要動作在所有入口同名 |
| E. 文案 | 說清楚會發生什麼、中英混排一致 | 同一層標題同樣大小寫；錯誤附修正方式 |

## 2. 量測基準

`NSColor` 語意色疊在 `windowBackgroundColor` 上的 WCAG 對比（`scratchpad/contrast.swift` 實算）：

| 顏色 | Light | Dark | 判定 |
| --- | --- | --- | --- |
| `labelColor` | 14.94:1 | 12.23:1 | 通過 |
| `secondaryLabelColor` | 3.95:1 | 5.89:1 | 系統標準；light 低於 4.5:1，由 macOS「增加對比」接手 |
| `tertiaryLabelColor` | **1.88:1** | **2.26:1** | 不可用於資訊文字 |
| `systemGreen`（文字） | **2.22:1** | 8.25:1 | light 不可當文字色 |
| `systemOrange`（文字） | **2.31:1** | 7.47:1 | light 不可當文字色 |
| `systemRed`（文字） | 3.57:1 | 4.86:1 | 小字需搭配粗體或圖示 |

## 3. 稽核發現與處理

| # | 嚴重度 | 發現 | 處理 |
| --- | --- | --- | --- |
| 1 | Critical | `caption2` 為 9 pt，低於 macOS 10 pt 最小字級 | 改為 10 pt，並加回歸測試 |
| 2 | Critical | 35 處資訊性文字（專案路徑、Tunnel ID 標籤、說明、時間戳）用 `tertiaryLabel`，1.88:1 | 改為 `secondaryLabel`；只保留純裝飾 icon 的 tertiary |
| 3 | High | 連線狀態「就緒」用綠色小字、Computer Use 狀態用橘色小字（light 約 2.2–2.3:1） | 只為 symbol 上色，文字回到 primary／secondary |
| 4 | High | 連線步驟、目前專案狀態點只靠圖示／顏色，VoiceOver 讀不到狀態 | 加 accessibility value（已完成／就緒／進行中／尚未開始／錯誤）；狀態點對應的 row 有 value |
| 5 | High | 連線／中斷、側欄各目的地、使用指南沒有選單列入口與快捷鍵 | 檔案選單 ⌘K 連線／中斷；顯示選單 ⌘1–⌘4；說明選單 ⌘? 開啟使用指南 |
| 6 | High | 使用指南子步驟編號固定 18 pt 寬，大介面尺寸被截成「…」 | 編號隨字級縮放並不截斷 |
| 7 | Medium | 主要動作在總覽、選單列 extra 用兩套命名邏輯 | `AppModel.primaryActionTitle` 統一 |
| 8 | Medium | Settings 選單控制項置中，左緣不對齊；每次都回到「一般」 | 左對齊；記住上次分頁（HIG › Settings） |
| 9 | Medium | 三個編輯區用 16% quaternary 底色，邊界約 1.05:1，圓角 8／9 不一 | 新增 `chadexEditorSurface()` token：text background + hairline |
| 10 | Medium | 活動紀錄搜尋／篩選無結果時仍顯示「目前沒有活動紀錄」 | 分別顯示「找不到符合…」與「沒有警告或錯誤」 |
| 11 | Medium | 區段標題 `COMPUTER USE`、`SKILLS` 寫死全大寫，與中文標題並列不一致 | 兩種語言統一 title-style |
| 12 | Low | 側欄 icon 固定 13 pt，不隨介面大小縮放 | 改用縮放字級 |
| 13 | Medium | ⌘K／選單列 extra 在 bootstrap 期間可觸發總覽頁刻意擋下的連線（reviewer 發現） | `AppModel.primaryActionEnabled` 三處共用；改 ⇧⌘K；選單標題帶受詞（「取消連線準備」「重試連線」） |
| 14 | High | 10 pt caption 承載必要說明；compact 80% 會降到 8 pt | 所有尺寸夾住 10 pt；caption 11 pt；`headline` 預設 semibold；頂層區段改用比說明更大的 `SectionTitle` |
| 15 | Medium | 總覽「最近活動」被活動頁搜尋／篩選影響 | 改用未篩選的 `recentActivities`；活動頁篩選無結果時提供「清除搜尋與篩選」 |
| 16 | Medium | helper 錯誤訊息寫死英文、無修正方式；說明文案術語過多；`bytes` 寫死 | 錯誤在地化並附修正步驟；改寫優先序與 Computer Use 文案；位元組數在地化；中文不再混用「Computer control」 |
| 17 | Medium | 選單列 extra「設定連線」與總覽行為不同；沒有主視窗時說明選單無法使用 | 兩者都開同一個連線表單；說明選單會開新視窗到指南 |
| 18 | Low | 列表容器圓角 6／8／9／10 混用、邊界不可見；任務階段與時間不在地化 | `chadexGroupSurface()` token；任務階段有 VoiceOver 狀態；時間用 `Duration.formatted` |
| 19 | Low | Settings 視窗隱藏標題；連線步驟在寬視窗拉成長線 | 顯示目前分頁標題；步驟寬度上限 640 pt |
| 20 | Medium | `openWindow(id: "main")` 在已有主視窗時再開一個，連線表單同時出現在兩個視窗（reviewer 第二輪） | `MainWindowPresenter` 先把既有（含縮到 Dock 的）主視窗帶到前面，沒有才開新視窗 |
| 21 | Medium | 中文術語不一致（Computer Use／電腦控制／電腦操作、helper、Keychain、metadata）與多餘空白 | 統一為 Computer Use、本機輔助程式、「鑰匙圈」；改寫 Skills／Agent 說明 |

## 3a. 獨立評分紀錄（AI reviewer，非外部驗證）

| 維度 | 第 1 輪 @386e68b | 第 2 輪 @6677dc8 | 第 3 輪 @0f61177 |
| --- | --- | --- | --- |
| A. 無障礙 | 3 | 4 | 4 |
| B. 平台慣例 | 3 | 3 | 4 |
| C. 視覺一致性 | 3 | 4 | 4 |
| D. 互動與回饋 | 3 | 4 | 4 |
| E. 文案 | 2 | 3 | 4 |

第 3 輪：無 Critical／High，剩餘皆為 Low（secondaryLabel 淺色 3.95:1 由系統「增加對比」處理；依賴 SwiftUI `main-AppWindow-N` 視窗命名，失效時退回開新視窗；總覽頁 eyebrow 與其他頁 SectionTitle 兩種模式並存）。視窗 identifier 格式已由 reviewer 以 SwiftUI 探針在本機實測。

證據頁：https://claude.ai/artifact/4JFYhwQoAubu5qjHufkHAh（私人，需由擁有者分享）

## 4. 刻意保留

- **App 內外觀設定（跟隨系統／淺色／深色）**：HIG › Dark Mode 建議避免，但這是既有功能且預設跟隨系統；保留。
- **側欄底部「設定」**：與 ⌘, 重複，但非關鍵動作；保留以維持既有使用習慣。
- **Code Ferret**：產品識別的唯一招牌元素，已支援「減少動態效果」與關閉顯示。

## 5. 驗證

- `swift test`：89/89 通過（含新增 `DesignSystemTests`：每個介面尺寸的字級下限、Settings 開啟分頁邏輯、雙語 key 一致、中文字間無多餘空白）。
- Release 建置：`swift build -c release` 成功，`Chadex --resource-preflight` → `Chadex resource preflight OK`（exit 0）。
- `VisualReviewTests`：15 張狀態截圖（light／dark、100–160%，含 Agent 設定、空活動、連線表單、執行中／驗證失敗任務），前 10 張有改版前對照。
- 嘗試以測試讀取執行時的 AppKit 無障礙樹：SwiftUI 只在有輔助技術連線時才產生完整節點，而測試程序未獲輔助使用權限（`AXIsProcessTrusted() == false`），無法自動驗證，已移除該測試，改列為人工走查項目。
- 限制：截圖為離屏渲染。macOS 未授予螢幕錄製權限，側欄 vibrancy／選取列與 Settings 分頁列無法正確繪製（黑色選取列、`.…` 標題是渲染器限制，不是 App 問題）。真實視窗截圖與 VoiceOver 實機走查需人工補做。

## 6. 尚待外部驗證

- 由人類設計師或目標使用者，對照第 1 節標準獨立評分。
- 實機 VoiceOver、鍵盤全流程、增加對比／減少透明度走查。
- 真實視窗（含側欄 vibrancy）的前後對照截圖。

## 7. 自動化無法完成的驗證（2026-10-08）

- 電腦控制：兩次申請 Chadex 視窗控制都回傳 `user_denied`（第二次是在使用者選擇授權之後，回應是立即的）。
- 螢幕錄製：`CGPreflightScreenCaptureAccess() == false`。
- 輔助使用：`AXIsProcessTrusted() == false`。

因此真實視窗截圖、VoiceOver 與鍵盤走查、人類評審仍待完成，D1 維持 **Pending Validation**。

## 8. 第二版視覺方向（參考 Apple Design Award 2026：Tide Guide、Structured）

使用者回饋「沒感覺到更有設計感」後，改做看得見的設計語言，而不是只修規格：

- **Structured 式時間軸**（`ActivityTimeline`）：左側時間欄、一條連續的線串起彩色事件節點、依「今天／昨天／日期」分組；總覽「最近活動」與活動紀錄頁共用。顏色代表事件類型，同時有符號形狀與 VoiceOver 等級（警告／錯誤）＋時間，不只靠顏色。
- **Tide Guide 式狀態色**（`ConnectionAmbience`）：總覽背景隨連線狀態換色（未連線灰、準備中／等待青、已驗證綠、錯誤紅），「減少動態效果」時不做轉場動畫。卡片維持不透明，文字對比不受影響。
- **Liquid Glass**：依 HIG `liquid-glass.md`「Don't use Liquid Glass in the content layer」，內容卡片不做玻璃。專案以 SDK 27 建置，側欄、toolbar、Settings 已自動採用系統玻璃；macOS 26+ 以 `backgroundExtensionEffect()` 讓狀態色延伸到浮動側欄底下，由系統玻璃折射。刻意不加自訂玻璃按鈕（主要動作在內容卡片內，屬內容層）。
- **Computer Use 模式卡片**：radio 列表改為可選卡片（符號＋名稱＋這個模式下 ChatGPT 能做什麼），顏色隨交出的控制權增加（灰→青→靛→橘）；選取狀態有 2 pt 外框、勾選符號與 VoiceOver `isSelected`。安全說明與「停止 Computer Use」收進同一張卡片。
- **Agent 設定／Skills**：Agent 設定有頁首（標題、說明、目前專案膠囊），Global Instructions 與 Skills 入口各為一張卡片；`chadexGroupSurface()` 改成與 `chadexCard` 相同的表面，Skills 來源、列表與 Project Memory 自動統一。
- **漏網的對比問題**：Skills 外部來源錯誤原為橘色 callout 文字（約 2.3:1）、兩個 Skill 表單錯誤為紅色 caption（3.57:1），改為 primary 文字＋彩色符號。`SKILL 列表`、`PROJECT MEMORY`、`REPOSITORY INSTRUCTIONS` 寫死全大寫，改為 title-style。
- 連線錯誤時只標記失敗的那一段：Tunnel 失敗時 ChatGPT 節點顯示「尚未連線」而非「錯誤」。

證據（離屏渲染）：`ui-review/d1-v2/`。離屏渲染畫不出 vibrancy／玻璃，側欄折射效果需實機確認，仍屬 **Pending Validation**。
- 2026-10-08：以不同 bundle ID（`app.chadex.ChadexDesignPreview`）組裝設計預覽版並開啟給使用者實機檢視；對預覽版的電腦控制申請同樣回傳 `user_denied`，真實視窗截圖仍待使用者提供。

## 9. 第一份真實視窗證據（使用者提供，2026-10-08）

使用者在預覽版截了總覽、Agent 設定、Skills、Computer Use、使用指南與 Settings 三個分頁。據此發現並處理：

- **App 一直以舊外觀相容模式執行**：SwiftPM 預設的 swiftbuild 後端把 `LC_BUILD_VERSION` 的 SDK 標成部署目標（`sdk 14.0`；已安裝的 v0.5.0 則是 `sdk 15.5`），macOS 依此不套用 Liquid Glass，側欄、toolbar、Settings 全是舊樣式。`--build-system native` 會正確標 `sdk 27.0`。`scripts/build_app.sh` 改為以 `vtool -set-build-version` 重新標記實際 SDK。
- **Settings 下拉選單被拉成整列寬**（離屏渲染看不出）：選單改為 `.fixedSize()` 保持原生寬度。
- **使用指南頂端標題被 toolbar 區壓住、與視窗標題重疊**：主視窗為了舊外觀的分隔線接縫設了 `titlebarAppearsTransparent = true`，捲動內容因此直接穿到標題下。改為只在 macOS 26 以前設定，新外觀交給系統的 scroll edge effect；待實機確認。

同時處理獨立 AI reviewer 第 4 輪（A3／B4／C3／D4／E4，不算外部驗證）的 High／Medium：
- H1：彩色節點上的白色符號低於 3:1 → `ChadexBrand.glyph(on:)` 依底色選白或近黑，`testStatusGlyphsKeepNonTextContrast` 斷言兩種外觀都 ≥ 3:1。
- M1：深色卡片原為白色 α 0.055（半透明，次要文字會隨洗色降到 3.2:1）→ 改為不透明 `white 0.155`。
- M2：「減少透明度」與「增加對比」時不畫狀態洗色。
- M5：已驗證時連線圖改用與膠囊、洗色相同的綠色。
- M3：模式卡片以 `accessibilityRepresentation` 提供原生 radio group（數量、選取、方向鍵），目前模式的說明放在 accessibility value；卡片由低風險排到高風險（僅讀取→詢問→本次→永遠）；切換時不再整排變淡。
- M4：新增 `chadexAttentionCard(tint:)`（卡片＋狀態色左側色條），用於連線錯誤（紅）與 Computer Use 待批准請求（橘）；錯誤標題改為 primary 文字＋紅色符號。
- L2 日期分組改用 index 當 id；L3 時間依 App 語言格式化；L7 Settings 標題與卡片內容對齊；L8 Agent 設定（全域）移除「目前專案」膠囊與按鈕上的「→」。
- 尚未處理：鍵盤焦點環樣式（需實機）、L1（側欄延伸背景只在總覽）、L9 文案。

## 10. AI reviewer 第 5 輪（@cfe907e，非外部驗證）

A4／B4／C4／D4／E4，無 Critical／High：依第 1 節門檻屬 AI reviewer 層級通過，**不等於外部驗證**。本輪後續處理：
- M-new-1：`chadexAttentionCard` 的色條原本凸出 12 pt 圓角 → 色條放在 stroke 之下並以卡片形狀裁切。
- L-new-1：Computer Use 模式在請求進行中改為直接忽略（含 VoiceOver 的 radio Picker），不再出現「已存檔但 helper 未切換」。
- L-new-2：被點選的模式卡片在請求進行中顯示 `ProgressView`。
- 注意（L-new-3）：`swift test`／`swift run` 的產物仍標 `sdk 14.0`，`ui-review/` 的離屏截圖因此是相容外觀，不代表出貨（`build_app.sh` 重標為 SDK 27）的樣子。
- 仍待處理：鍵盤焦點環樣式、未選取模式卡片的淡色 icon（L-new-4）、選單寬度隨選項變動（L-new-5）、L1、L9；待批准請求移到模式卡片上方。
