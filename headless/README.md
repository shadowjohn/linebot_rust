# LINE headless 試驗版（Windows）

Cargo 從 GitHub 取得固定的 line-rs 版本（`884203cb747193d83b476460a7a3a5e611f197bb`），不必另外 clone 到相鄰目錄。既有 GUI、派工服務、正式設定與排程均不變。
這是非官方 LINE 客戶端。只用隔離的通報帳號與測試群；上游警告桌面型 session
可能使登入同一帳號的 LINE 桌面客戶端登出。Windows 登出和 LINE 撤銷授權不同；
撤銷後不會自動重登或改走 GUI。

## 本機操作

在 `D:\mytools\linebot_rust` 的 PowerShell：

```powershell
cargo build --manifest-path headless/Cargo.toml --locked
$exe = Join-Path $PWD 'headless\target\debug\linebot_headless.exe'
$state = Join-Path $env:LOCALAPPDATA '3wa\linebot-headless-test'
New-Item -ItemType Directory -Path $state -Force | Out-Null
$group = 'c00000000000000000000000000000000'   # 換成專用測試群的 LINE 群組 ID
$room = '測試群名稱'                          # 須與 LINE 完全一致
& $exe login $state                            # 只在自己的終端機用手機掃 QR
& $exe check $state $group $room                 # 無發送，無訊息讀取
& $exe read $state $group $room 5                # 無發送，最多 20 筆近況，只回傳文字
& $exe once $state $group $room 'D:\path\test-queue.ini'
```

試驗設定沿用原 `[Settings]` 格式、HTTPS `SERVICE_URL`、測試專用
`BOT_TOKEN`，只給指定測試群排單。不要使用正在由 GUI 領取的正式佇列或正式 token。
`run` 持續輪詢，同目錄建一個空的 `stop` 檔後會在下一輪前正常停止；要再次開始，
先移除該檔。狀態目錄只給執行帳號存取，不放在 Git、共用磁碟或同步資料夾。
同一狀態目錄請固定配同一個帳號、群組和派工佇列；測另一個請用獨立目錄。

## 相容範圍

- 上游 `getWork`、`getFile`、`updateWorkStatus` 及資料欄位不變。沿用文字、
  JPG/PNG/GIF；通用檔案與 OpenChat 暫不支援。圖文同單會發圖片再發文字，LINE 內為兩則訊息，
  不是一筆原子交易。附件驗證不通回報 402。
- 發前記待核對，兩段都確定完成才記成功並回報 200。若送出結果不確定，該工單停止且不重送。
  若已記成功而回報失敗，重啟後遇同 ID 只補回報，不補發。不要刪紀錄重跑不確定的工單。
- `read` 不輪詢派工、不標記已讀、不下載附件、不保存對話；回傳時間和文字。
  帳號無權或缺少正常授權取得的加密金鑰時，不能讀取該內容。
- `session.json` 內含可用 session 與加密金鑰，等同憑證。QR、PIN、session
  不要截圖傳出。部署後遇授權失效，於手機 LINE 確認該登入裝置；不可複製 GUI 的 session。
- 尚未在實際 LINE 或登出 Windows 桌面的情況驗收。建議順序：專用帳號掃碼 →
  指定測試群驗證文字與圖片 → 由同一 Windows 執行帳號啟動「不論使用者登入與否都執行」
  的測試工作，真正登出 Windows 後用手機驗收一張測試工單，再考慮切正式佇列。
