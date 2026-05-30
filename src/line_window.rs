use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result};
use clipboard_win::{Clipboard, Setter, formats};
use uiautomation::controls::WindowControl;
use uiautomation::core::{UIAutomation, UIElement};
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
        target
            .send_text_by_clipboard(message)
            .map_err(|e| LineError::new("405", format!("貼上訊息失敗: {}", e)))?;
        
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

        // 2. 第一個 enter：動態發送給確認彈窗，將圖片確認放入對話框中
        let focused = self.automation.get_focused_element().unwrap_or_else(|_| chat_win.clone());
        focused.send_keys("{enter}", 30)
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
        sleep(Duration::from_millis(900));

        // 1. 輸入完後，先按 enter 等1秒
        search_edit
            .send_keys("{enter}", 30)
            .map_err(|e| LineError::new("401", format!("搜尋後按 enter 失敗: {}", e)))?;
        sleep(Duration::from_secs(1));

        // 2. 按 tab 等500ms (動態發送給當前焦點)
        let focused = self.automation.get_focused_element().unwrap_or_else(|_| main_window.clone());
        focused.send_keys("{tab}", 30)
            .map_err(|e| LineError::new("401", format!("按第一個 tab 失敗: {}", e)))?;
        sleep(Duration::from_millis(500));

        // 3. 再按 tab 等500ms
        let focused = self.automation.get_focused_element().unwrap_or_else(|_| main_window.clone());
        focused.send_keys("{tab}", 30)
            .map_err(|e| LineError::new("401", format!("按第二個 tab 失敗: {}", e)))?;
        sleep(Duration::from_millis(500));

        // 4. 按 down 等1秒
        let focused = self.automation.get_focused_element().unwrap_or_else(|_| main_window.clone());
        focused.send_keys("{down}", 30)
            .map_err(|e| LineError::new("401", format!("按 down 失敗: {}", e)))?;
        sleep(Duration::from_secs(1));

        // 5. 再按 enter 等3秒 (開啟聊天室)
        let focused = self.automation.get_focused_element().unwrap_or_else(|_| main_window.clone());
        focused.send_keys("{enter}", 30)
            .map_err(|e| LineError::new("401", format!("開啟聊天室按 enter 失敗: {}", e)))?;
        sleep(Duration::from_secs(3));
        
        // 4. 確認視窗有彈出 (透過 find_active_chat_window 驗證)
        let chat_win = self
            .find_active_chat_window()
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

    fn find_search_edit(&self, window: &UIElement) -> Result<UIElement> {
        self.automation
            .create_matcher()
            .from_ref(window)
            .control_type(ControlType::Edit)
            .classname("LcTextField")
            .depth(12)
            .timeout(5000)
            .find_first()
            .context("找不到 LcTextField")
    }

    fn find_result_items(&self, window: &UIElement, timeout_ms: u64) -> Result<Vec<UIElement>> {
        self.automation
            .create_matcher()
            .from_ref(window)
            .control_type(ControlType::ListItem)
            .depth(12)
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
            edit.send_text_by_clipboard(value)
                .map_err(|e| LineError::new("405", format!("貼上搜尋文字失敗: {}", e)))?;
            // 貼上後等待 300ms 讓 LINE 的搜尋清單結果有足夠時間渲染出來！
            sleep(Duration::from_millis(300));
        }
        Ok(())
    }

    fn find_active_chat_window(&self) -> Result<UIElement> {
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
