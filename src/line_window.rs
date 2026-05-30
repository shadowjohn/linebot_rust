use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result};
use clipboard_win::{Clipboard, Setter, formats};
use uiautomation::controls::WindowControl;
use uiautomation::core::{UIAutomation, UIElement};
use uiautomation::inputs::Keyboard;
use uiautomation::types::ControlType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineError {
    pub error_code: &'static str,
    pub message: String,
}

impl LineError {
    pub fn new(error_code: &'static str, message: impl Into<String>) -> Self {
        Self {
            error_code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for LineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.error_code, self.message)
    }
}

impl std::error::Error for LineError {}

#[derive(Debug, Clone)]
pub struct LineOptions {
    pub launcher_path: PathBuf,
    pub close_chat_after_send: bool,
}

pub struct LineController {
    automation: UIAutomation,
    options: LineOptions,
}

impl LineController {
    pub fn new(options: LineOptions) -> Result<Self> {
        Ok(Self {
            automation: UIAutomation::new().context("初始化 Windows UI Automation 失敗")?,
            options,
        })
    }

    pub fn inspect(&self) -> Result<String> {
        let mut output = String::new();
        output.push_str("=== LINE WINDOW INSPECT ===\n");
        
        // Find main window
        if let Ok(main_win) = self.automation.create_matcher().control_type(ControlType::Window).classname("AllInOneWindow").find_first() {
            let name = main_win.get_name().unwrap_or_default();
            output.push_str(&format!("Found Main Window (AllInOneWindow): \"{}\"\n", name));
            
            output.push_str("  --- Edit Controls (depth 10) ---\n");
            let edits = self.automation.create_matcher().from_ref(&main_win).control_type(ControlType::Edit).depth(10).find_all().unwrap_or_default();
            for edit in edits {
                let e_name = edit.get_name().unwrap_or_default();
                let e_class = edit.get_classname().unwrap_or_default();
                let e_id = edit.get_automation_id().unwrap_or_default();
                output.push_str(&format!("    Edit: class=\"{}\", name=\"{}\", id=\"{}\"\n", e_class, e_name, e_id));
            }
        } else {
            output.push_str("Main Window (AllInOneWindow) NOT found!\n");
        }
        
        // Find all chat windows
        output.push_str("\n=== CHAT WINDOWS ===\n");
        let chat_matcher = self.automation.create_matcher().control_type(ControlType::Window).classname("ChatWindow");
        if let Ok(chat_wins) = chat_matcher.find_all() {
            output.push_str(&format!("Found {} ChatWindow(s)\n", chat_wins.len()));
            for (idx, chat_win) in chat_wins.into_iter().enumerate() {
                let name = chat_win.get_name().unwrap_or_default();
                output.push_str(&format!("{}. ChatWindow: \"{}\"\n", idx + 1, name));
                
                output.push_str("  --- Edit Controls (depth 10) ---\n");
                let edits = self.automation.create_matcher().from_ref(&chat_win).control_type(ControlType::Edit).depth(10).find_all().unwrap_or_default();
                for edit in edits {
                    let e_name = edit.get_name().unwrap_or_default();
                    let e_class = edit.get_classname().unwrap_or_default();
                    let e_id = edit.get_automation_id().unwrap_or_default();
                    output.push_str(&format!("    Edit: class=\"{}\", name=\"{}\", id=\"{}\"\n", e_class, e_name, e_id));
                }
            }
        } else {
            output.push_str("No ChatWindow found or query failed.\n");
        }

        Ok(output)
    }

    pub fn send_message(&self, room_name: &str, message: &str) -> Result<(), LineError> {
        let (main_window, chat_win) = self.open_room(room_name)?;
        let target = self
            .find_chat_input(&chat_win)
            .map_err(|e| LineError::new("405", format!("找不到聊天輸入框: {}", e)))?;

        target
            .set_focus()
            .map_err(|e| LineError::new("405", format!("聚焦聊天輸入框失敗: {}", e)))?;
        sleep(Duration::from_millis(150));

        // 貼上訊息
        self.paste_text(&target, message)?;
        
        // 再等1秒
        sleep(Duration::from_secs(1));

        // 再按 enter
        target
            .send_keys("{enter}", 30)
            .map_err(|e| LineError::new("405", format!("送出訊息失敗: {}", e)))?;

        self.finish_chat(&main_window, &chat_win);
        Ok(())
    }

    pub fn send_file(&self, room_name: &str, file_path: &Path) -> Result<(), LineError> {
        let (main_window, chat_win) = self.open_room(room_name)?;
        let target = self
            .find_chat_input(&chat_win)
            .map_err(|e| LineError::new("405", format!("找不到聊天輸入框: {}", e)))?;

        target
            .set_focus()
            .map_err(|e| LineError::new("405", format!("聚焦聊天輸入框失敗: {}", e)))?;
        sleep(Duration::from_millis(150));

        let path_text = file_path.to_string_lossy().to_string();
        let files = vec![path_text.as_str()];

        {
            let _clipboard = Clipboard::new_attempts(10)
                .map_err(|e| LineError::new("405", format!("開啟剪貼簿失敗: {}", e)))?;
            formats::FileList
                .write_clipboard(&files)
                .map_err(|e| LineError::new("405", format!("設定檔案剪貼簿失敗: {}", e)))?;
        } // 關鍵：在此處釋放 (Drop) 剪貼簿鎖！否則 LINE 會因為被鎖定而無法讀取剪貼簿

        // 貼上附件
        target
            .send_keys("{ctrl}v", 30)
            .map_err(|e| LineError::new("405", format!("貼上附件失敗: {}", e)))?;
        
        // 1. 等待 1.5 秒讓「傳送圖片確認彈窗」完全加載並自動取得焦點
        sleep(Duration::from_millis(1500));

        // 2. 第一個 enter：直接對 chat_win 送出 enter！
        // 因為彈窗是 chat_win 的子視窗/模態彈窗，所以對 chat_win 送出 enter 會由系統自動路由給當前處於最上層的彈窗確認按鈕！
        // 這完美避免了使用 get_focused_element() 時，若控制台在最上層會導致 enter 送錯給命令提示字元的問題！
        chat_win.send_keys("{enter}", 30)
            .map_err(|e| LineError::new("405", format!("送出附件確認失敗: {}", e)))?;
            
        // 等待 1 秒讓確認彈窗關閉並把圖放進對話框
        sleep(Duration::from_secs(1));

        // 3. 第二個 enter：再次聚焦對話框並發送 enter，將圖片真正發送出去！
        target
            .set_focus()
            .map_err(|e| LineError::new("405", format!("重新聚焦輸入框失敗: {}", e)))?;
        target
            .send_keys("{enter}", 30)
            .map_err(|e| LineError::new("405", format!("發送圖片失敗: {}", e)))?;
            
        // 4. 再等待 3 秒物理緩衝，確保 LINE 有充足時間將圖片檔案上傳並完成傳送動畫！
        sleep(Duration::from_secs(3));

        self.finish_chat(&main_window, &chat_win);
        Ok(())
    }

    fn open_room(&self, room_name: &str) -> Result<(UIElement, UIElement), LineError> {
        let main_window = self.ensure_main_window()?;
        self.activate_window(&main_window)?;

        let search_edit = self
            .find_search_edit(&main_window)
            .map_err(|e| LineError::new("401", format!("找不到 LINE 搜尋框: {}", e)))?;
        self.set_edit_value(&search_edit, "")?;
        self.set_edit_value(&search_edit, room_name)?;
        // 1. 等待 1.5 秒讓搜尋結果清單在畫面上完全渲染出來
        sleep(Duration::from_millis(1500));

        // 2. DPI 免疫鍵盤導航 —— 使用全域 Keyboard 直接發送按鍵
        //    Keyboard::new().send_keys() 不像 UIElement::send_keys() 會先呼叫 set_focus() 重置焦點，
        //    因此能讓 {enter} → {tab} → {tab} → {down} → {enter} 的焦點自然流動不被打斷。
        //    ┌─────────────────────────────────────────────────────┐
        //    │  {enter}       → 確認搜尋（焦點從搜尋框到結果列表）  │
        //    │  {tab}{tab}    → 焦點跳到「聊天室」分頁             │
        //    │  {down}        → 選中第一個搜尋結果                 │
        //    │  {enter}       → 開啟該聊天室視窗                   │
        //    └─────────────────────────────────────────────────────┘
        Keyboard::new()
            .interval(350)
            .send_keys("{enter}")
            .map_err(|e| LineError::new("401", format!("鍵盤導航 Enter 失敗: {}", e)))?;
        sleep(Duration::from_millis(800));

        Keyboard::new()
            .interval(350)
            .send_keys("{tab}{tab}{down}{enter}")
            .map_err(|e| LineError::new("401", format!("鍵盤導航 Tab/Down/Enter 失敗: {}", e)))?;

        // 3. 等待 3 秒物理緩衝讓獨立聊天視窗完全彈出
        sleep(Duration::from_secs(3));
        
        // 4. 確認視窗有彈出 (透過 find_active_chat_window 驗證，傳入 room_name 精準匹配)
        let chat_win = self
            .find_active_chat_window(room_name)
            .map_err(|e| LineError::new("401", format!("開啟聊天室後找不到聊天視窗: {}", e)))?;

        Ok((main_window, chat_win))
    }

    fn ensure_main_window(&self) -> Result<UIElement, LineError> {
        let exists = self.find_main_window(500).is_ok();

        if !self.options.launcher_path.exists() {
            return Err(LineError::new(
                "404",
                format!(
                    "LineLauncher.exe 不存在: {}",
                    self.options.launcher_path.display()
                ),
            ));
        }

        if !exists {
            Command::new(&self.options.launcher_path)
                .spawn()
                .map_err(|e| LineError::new("404", format!("啟動 LINE 失敗: {}", e)))?;
            // 等待 LINE 完整啟動與視窗加載
            sleep(Duration::from_secs(3));
        } else {
            // 已經啟動但可能縮小在系統列 (System Tray)，再跑一次 Launcher 可強制呼叫回前景
            let _ = Command::new(&self.options.launcher_path).spawn();
            // 關鍵！給 Launcher 1.5 秒的時間讓 LINE 官方程序將主視窗從系統列完全還原並顯示到螢幕上！
            sleep(Duration::from_millis(1500));
        }

        self.find_main_window(30000)
            .map_err(|e| LineError::new("404", format!("等待 LINE 視窗逾時: {}", e)))
    }

    fn find_main_window(&self, timeout_ms: u64) -> Result<UIElement> {
        self.automation
            .create_matcher()
            .control_type(ControlType::Window)
            .classname("AllInOneWindow")
            .timeout(timeout_ms)
            .find_first()
            .context("找不到 LINE 主視窗 AllInOneWindow")
    }

    fn activate_window(&self, window: &UIElement) -> Result<(), LineError> {
        if let Ok(control) = WindowControl::try_from(window.clone()) {
            let _ = control.set_foregrand();
        }

        window
            .set_focus()
            .map_err(|e| LineError::new("405", format!("聚焦 LINE 視窗失敗: {}", e)))?;
        sleep(Duration::from_millis(300));
        Ok(())
    }

    fn paste_text(&self, element: &UIElement, text: &str) -> Result<(), LineError> {
        // 使用 uiautomation 內建的 send_text_by_clipboard
        // 它底層會呼叫 Clipboard::open().set_text() 寫入 UTF-16 中文文字，
        // 接著 Ctrl+V 貼上，最後自動還原剪貼簿內容。
        //
        // 注意：在某些 Windows 環境下，clipboard.restore() 可能會回報一個
        // Windows 錯誤碼 0（「操作順利完成」），這其實代表操作成功，
        // 但 uiautomation 會把它當作 Err 拋出。所以我們需要忽略此類假錯誤。
        match element.send_text_by_clipboard(text) {
            Ok(()) => Ok(()),
            Err(e) => {
                let msg = format!("{}", e);
                // 「操作順利完成」= Windows ERROR_SUCCESS (0)，是假錯誤
                if msg.contains("操作順利完成") || msg.contains("completed successfully") {
                    Ok(())
                } else {
                    Err(LineError::new("405", format!("貼上文字失敗: {}", e)))
                }
            }
        }
    }

    #[allow(dead_code)]
    fn item_contains_text(&self, item: &UIElement, text: &str) -> bool {
        // 1. 先看 ListItem 本身有沒有包含 name
        if item.get_name().unwrap_or_default().contains(text) {
            return true;
        }

        // 2. 遞迴搜尋其下所有子元素，看有沒有任何子元素的 name 包含 text
        if let Ok(found) = self.automation
            .create_matcher()
            .from_ref(item)
            .find_all()
        {
            for child in found {
                let name = child.get_name().unwrap_or_default();
                if name.contains(text) {
                    return true;
                }
            }
        }
        false
    }

    fn find_search_edit(&self, window: &UIElement) -> Result<UIElement> {
        self.automation
            .create_matcher()
            .from_ref(window)
            .control_type(ControlType::Edit)
            .classname("LcTextField")
            .timeout(5000)
            .find_first()
            .context("找不到 LcTextField")
    }

    #[allow(dead_code)]
    fn find_result_items(&self, window: &UIElement, timeout_ms: u64) -> Result<Vec<UIElement>> {
        self.automation
            .create_matcher()
            .from_ref(window)
            .control_type(ControlType::ListItem)
            .timeout(timeout_ms)
            .find_all()
            .context("找不到聊天室搜尋結果")
    }

    fn set_edit_value(&self, edit: &UIElement, value: &str) -> Result<(), LineError> {
        edit.set_focus()
            .map_err(|e| LineError::new("405", format!("聚焦搜尋框失敗: {}", e)))?;
        // 聚焦後稍微等待 300ms 確保焦點已完全鎖定
        sleep(Duration::from_millis(300));

        edit.send_keys("{ctrl}a{delete}", 10)
            .map_err(|e| LineError::new("405", format!("清空搜尋框失敗: {}", e)))?;
        sleep(Duration::from_millis(100));
            
        if !value.is_empty() {
            self.paste_text(edit, value)?;
            // 貼上後等待 300ms 讓 LINE 的搜尋清單結果有足夠時間渲染出來！
            sleep(Duration::from_millis(300));
        }
        Ok(())
    }

    fn find_active_chat_window(&self, room_name: &str) -> Result<UIElement> {
        // 第一優先級：精準尋找 classname 為 "ChatWindow" 且視窗名稱 (Title) 與目標 room_name 相同的視窗
        // 這能確保即便開了多個 LINE 獨立聊天視窗，也能百分之百取得對的聊天室！
        if let Ok(matched) = self.automation
            .create_matcher()
            .control_type(ControlType::Window)
            .classname("ChatWindow")
            .name(room_name)
            .timeout(1000)
            .find_first()
        {
            return Ok(matched);
        }

        // 第二優先級：如果目前的焦點就在該 ChatWindow 的某個子控制項上，從焦點往上追溯
        if let Ok(focused) = self.automation.get_focused_element() {
            if let Ok(walker) = self.automation.get_control_view_walker() {
                let mut current = focused;
                for _ in 0..10 {
                    let classname = current.get_classname().unwrap_or_default();
                    if classname == "ChatWindow" {
                        return Ok(current);
                    }
                    if let Ok(parent) = walker.get_parent(&current) {
                        current = parent;
                    } else {
                        break;
                    }
                }
            }
        }
        
        // 最終備用：獲取任意一個 ChatWindow
        self.automation
            .create_matcher()
            .control_type(ControlType::Window)
            .classname("ChatWindow")
            .find_first()
            .context("找不到任何 ChatWindow")
    }

    fn find_chat_input(&self, chat_win: &UIElement) -> Result<UIElement> {
        self.automation
            .create_matcher()
            .from_ref(chat_win)
            .control_type(ControlType::Edit)
            .classname("AutoSuggestTextArea")
            .depth(10)
            .find_first()
            .context("找不到聊天室輸入框 AutoSuggestTextArea")
    }

    fn finish_chat(&self, main_window: &UIElement, chat_win: &UIElement) {
        if self.options.close_chat_after_send {
            let _ = chat_win.send_keys("{alt}{f4}", 30);
            sleep(Duration::from_millis(500));
        }

        if let Ok(edit) = self.find_search_edit(main_window) {
            let _ = self.set_edit_value(&edit, "");
        }
    }
}
