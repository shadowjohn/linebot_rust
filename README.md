# linebot_rust 🦀

基於 **Rust + Windows UI Automation** 打造的高效能、滑鼠釋放、高解析度螢幕縮放（DPI Scaling）免疫的 Windows LINE Desktop 自動化發送機器人。

這是為 **Focusit 系統** 專門量身設計的自動化控制核心，用於替代原先依賴影像比對的 Python + SikuliX 舊系統，提供企業級的強健度與常駐穩定性。

---

## 🚀 核心優勢與技術特性

1. **滑鼠指針完全釋放**
   * 拋棄傳統 SikuliX 的實體滑鼠軌跡模擬。本專案透過 Windows UI Accessibility API 直接獲取視窗控制項並以作業系統層級發送按鍵，**Bot 在背景執行時，您的實體滑鼠指標完全自由，絕不搶占滑鼠**，讓您可以同時在本機安心工作。
2. **100% 免疫高解析度螢幕縮放 (DPI Scaling)**
   * SikuliX 會因為 Windows 螢幕縮放（125% 或 150%）導致座標偏移而「點偏」失敗。本專案採用 **純鍵盤快捷鍵導航樹 (`{enter} -> {tab} -> {tab} -> {down} -> {enter}`)** 搭配**動態焦點追蹤技術**，不論螢幕解析度與縮放比為何，皆能 100% 精準開窗。
3. **無痛的自動鎖定與崩潰復原 (`run.lock`)**
   * 採用與 Python 版對齊的「先 `unlink` 再獨佔開啟」機制。即便伺服器斷電或程式非預期崩潰，下次開機時**會自動清理殘留的鎖檔案**，實現無人看守的自我修復。
4. **精準控制 LINE 圖片確認彈窗與上傳時差**
   * 發送圖片時，能自動聚焦 LINE 專屬的「傳送圖片確認彈窗」，發送第一個 Enter 完成確認（將圖片放入對話框），隨後發送第二個 Enter 正式發出。
   - 內建 3 秒的物理上傳與動畫緩衝，避免因上傳未完即關閉聊天室視窗（`Alt+F4`）導致傳送中斷。
5. **單一二進位檔，無額外執行期相依性**
   * 100% 純 Rust 編譯。不需要安裝 Java VM、Jython、Python 等龐大的 Runtime，內存佔用極低（僅數 MB），非常適合作為 Windows 系統服務常駐。

---

## 🛠️ 環境需求

* Windows 10 / 11 
* LINE 電腦版客戶端（請確保已登入）
* 已安裝 Rust 編譯工具鏈（若需自行編譯）

---

## ⚙️ 快速配置

專案預設載入同目錄下的 **`config.ini`**。請參考專案中的 [config.ini.example](config.ini.example) 進行設定：

```ini
# 環境設定區
[Settings]
# 服務派工網址
SERVICE_URL = https://map.gis.tw/SystemReport/linebotgis_api.aspx
# 機器人 Token
BOT_TOKEN = 請輸入您的BOT_TOKEN
# 輪詢秒數 (預設 5 秒偵測一次新工作)
POLL_SECONDS = 5
# 發送完畢後是否按 Alt+F4 自動關閉獨立聊天室視窗 (建議上線設為 true)
CLOSE_CHAT_AFTER_SEND = true
# 模擬瀏覽器 User-Agent
USER_AGENT = Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36
```

> ⚠️ **安全提示**：為防止 Token 洩漏，`config.ini` 已被寫入 `.gitignore`，**絕對不會**被簽入 Git 倉庫。

---

## 📦 編譯與安裝

在專案根目錄下使用 Cargo 進行發布版本編譯：

```powershell
# 編譯最佳化 release 版本
cargo build --release
```
編譯完成的執行檔將位於 `target\release\linebot_rust.exe`。

---

## 💻 命令行用法與命令手冊

`linebot_rust.exe` 提供了豐富的診斷與測試參數，方便在本機進行除錯與驗證：

### 1. 正式派工常駐模式 (Daemon Mode)
讀取 `config.ini` 開始輪詢派工 API：
```powershell
target\release\linebot_rust.exe
```

### 2. 測試發送文字訊息（不自動關閉視窗）
用於測試 Bot 是否能順利叫醒 LINE、進群並貼上文字：
```powershell
target\release\linebot_rust.exe --config config.ini --no-close --test-room "聊天室名稱" --test-message "哈囉！這是一條測試訊息。"
```

### 3. 測試發送圖片/附件（不自動關閉視窗）
用於測試 Bot 的「雙 Enter 預覽彈窗確認」及「貼圖上傳緩衝」是否工作正常：
```powershell
target\release\linebot_rust.exe --config config.ini --no-close --test-room "聊天室名稱" --test-file "C:\path\to\your_image.png"
```

### 4. 診斷與控制項樹傾印 (`--inspect-line`)
高速檢查當前 Windows 系統上 LINE 主視窗（`AllInOneWindow`）與所有已開啟聊天視窗（`ChatWindow`）的 UI Automation 控制項結構（例如搜尋框 `LcTextField` 與聊天輸入框 `AutoSuggestTextArea`）：
```powershell
target\release\linebot_rust.exe --inspect-line
```

### 5. 完整參數說明
* `--config <path>`：指定設定檔路徑（預設為 `config.ini`）。
* `--once`：主迴圈僅執行一次派工即退出（適合排程呼叫）。
* `--inspect-line`：執行 LINE 控制項結構診斷。
* `--no-close`：測試發送時，發送完畢後不送出 `Alt+F4` 關閉聊天室視窗。
* `--test-room <name>`：指定測試目標聊天室名稱。
* `--test-message <msg>`：指定測試發送文字。
* `--test-file <path>`：指定測試發送檔案的絕對路徑。

---

## 🧪 跑單元測試

專案內建完整的單元測試，用以確保路徑解析、設定檔 parser 與圖片特徵偵測邏輯正確：

```powershell
cargo test
```

---

## 📜 授權條款

本專案採用 MIT 授權條款開放原始碼。
