use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result};
use clipboard_win::{Clipboard, Setter, formats};
use uiautomation::controls::WindowControl;
use uiautomation::core::{UIAutomation, UIElement};
use uiautomation::patterns::UIValuePattern;
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
        let window = self.ensure_main_window().map_err(anyhow::Error::new)?;
        let edit = self.find_search_edit(&window)?;
        let items = self.find_result_items(&window, 0).unwrap_or_default();

        Ok(format!(
            "LINE window OK, class={}, search_edit={}, list_items={}",
            window.get_classname().unwrap_or_default(),
            edit.get_classname().unwrap_or_default(),
            items.len()
        ))
    }

    pub fn send_message(&self, room_name: &str, message: &str) -> Result<(), LineError> {
        let main_window = self.open_room(room_name)?;
        let target = self.focused_or(main_window.clone());

        target
            .send_text_by_clipboard(message)
            .map_err(|e| LineError::new("405", format!("貼上訊息失敗: {}", e)))?;
        sleep(Duration::from_millis(150));
        target
            .send_keys("{enter}", 30)
            .map_err(|e| LineError::new("405", format!("送出訊息失敗: {}", e)))?;

        self.finish_chat(&main_window, &target);
        Ok(())
    }

    pub fn send_file(&self, room_name: &str, file_path: &Path) -> Result<(), LineError> {
        let main_window = self.open_room(room_name)?;
        let target = self.focused_or(main_window.clone());
        let path_text = file_path.to_string_lossy().to_string();
        let files = vec![path_text.as_str()];

        let _clipboard = Clipboard::new_attempts(10)
            .map_err(|e| LineError::new("405", format!("開啟剪貼簿失敗: {}", e)))?;
        formats::FileList
            .write_clipboard(&files)
            .map_err(|e| LineError::new("405", format!("設定檔案剪貼簿失敗: {}", e)))?;

        target
            .send_keys("{ctrl}v", 30)
            .map_err(|e| LineError::new("405", format!("貼上附件失敗: {}", e)))?;
        sleep(Duration::from_secs(2));
        target
            .send_keys("{enter}", 30)
            .map_err(|e| LineError::new("405", format!("送出附件失敗: {}", e)))?;

        self.finish_chat(&main_window, &target);
        Ok(())
    }

    fn open_room(&self, room_name: &str) -> Result<UIElement, LineError> {
        let main_window = self.ensure_main_window()?;
        self.activate_window(&main_window)?;

        let search_edit = self
            .find_search_edit(&main_window)
            .map_err(|e| LineError::new("401", format!("找不到 LINE 搜尋框: {}", e)))?;
        self.set_edit_value(&search_edit, "")?;
        self.set_edit_value(&search_edit, room_name)?;
        sleep(Duration::from_millis(900));

        let items = match self.find_result_items(&main_window, 1000) {
            Ok(items) => items,
            Err(_) => {
                let _ = self.set_edit_value(&search_edit, "");
                return Err(LineError::new("401", format!("查無聊天室: {}", room_name)));
            }
        };
        let first = match items.first() {
            Some(first) => first,
            None => {
                let _ = self.set_edit_value(&search_edit, "");
                return Err(LineError::new("401", format!("查無聊天室: {}", room_name)));
            }
        };

        if first.double_click().is_err() {
            main_window
                .send_keys("{enter}", 30)
                .map_err(|e| LineError::new("401", format!("開啟聊天室失敗: {}", e)))?;
        }

        sleep(Duration::from_millis(1200));
        Ok(main_window)
    }

    fn ensure_main_window(&self) -> Result<UIElement, LineError> {
        if let Ok(window) = self.find_main_window(500) {
            return Ok(window);
        }

        if !self.options.launcher_path.exists() {
            return Err(LineError::new(
                "404",
                format!(
                    "LineLauncher.exe 不存在: {}",
                    self.options.launcher_path.display()
                ),
            ));
        }

        Command::new(&self.options.launcher_path)
            .spawn()
            .map_err(|e| LineError::new("404", format!("啟動 LINE 失敗: {}", e)))?;

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

        if let Ok(pattern) = edit.get_pattern::<UIValuePattern>() {
            pattern
                .set_value(value)
                .map_err(|e| LineError::new("405", format!("設定搜尋框內容失敗: {}", e)))?;
            return Ok(());
        }

        edit.send_keys("{ctrl}a{delete}", 10)
            .map_err(|e| LineError::new("405", format!("清空搜尋框失敗: {}", e)))?;
        edit.send_text_by_clipboard(value)
            .map_err(|e| LineError::new("405", format!("貼上搜尋文字失敗: {}", e)))?;
        Ok(())
    }

    fn focused_or(&self, fallback: UIElement) -> UIElement {
        self.automation.get_focused_element().unwrap_or(fallback)
    }

    fn finish_chat(&self, main_window: &UIElement, target: &UIElement) {
        if self.options.close_chat_after_send {
            // LINE 搜尋結果雙擊通常會開啟獨立聊天室；Alt+F4 用來關閉該聊天室。
            let _ = target.send_keys("{alt}{f4}", 30);
            sleep(Duration::from_millis(500));
        }

        if let Ok(edit) = self.find_search_edit(main_window) {
            let _ = self.set_edit_value(&edit, "");
        }
    }
}
