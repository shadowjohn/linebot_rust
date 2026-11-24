use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clipboard_win::{Clipboard, Setter, formats, get_clipboard, set_clipboard};
use uiautomation::actions::{Invoke, Value};
use uiautomation::controls::{ButtonControl, EditControl, WindowControl};
use uiautomation::core::{UIAutomation, UIElement};
use uiautomation::inputs::{Keyboard, Mouse};
use uiautomation::types::{ControlType, Rect};

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
    search_edit_cache: Mutex<Option<UIElement>>,
}

struct OpenedRoom {
    main_window: UIElement,
    chat_surface: UIElement,
    chat_input: Option<UIElement>,
    close_chat_window: Option<UIElement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainWindowStartupAction {
    UseExisting,
    Launch,
}

fn main_window_startup_action(
    main_window_exists: bool,
    launcher_exists: bool,
) -> Result<MainWindowStartupAction, LineError> {
    if main_window_exists {
        return Ok(MainWindowStartupAction::UseExisting);
    }

    if launcher_exists {
        Ok(MainWindowStartupAction::Launch)
    } else {
        Err(LineError::new("404", "LineLauncher.exe 不存在"))
    }
}

fn is_search_result_row_rect(
    search_left: i32,
    search_right: i32,
    search_bottom: i32,
    row_left: i32,
    row_right: i32,
    row_top: i32,
    row_height: i32,
    row_width: i32,
) -> bool {
    let overlaps_search_column = row_left < search_right && row_right > search_left;
    overlaps_search_column && row_top > search_bottom && row_height >= 20 && row_width >= 80
}

// LINE exposes the section headings and result rows as buttons in newer desktop versions.
fn search_result_section(label: &str) -> Option<bool> {
    let (section, details) = label.split_once(", ")?;
    let (count, _) = details.split_once(", ")?;
    count.parse::<u32>().ok()?;
    match section {
        "聊天" | "好友" | "群組" | "Chats" | "Friends" | "Groups" => Some(true),
        "訊息" | "Messages" => Some(false),
        _ => None,
    }
}

fn find_room_button_index<'a>(
    labels: impl IntoIterator<Item = &'a str>,
    room_name: &str,
) -> Result<Option<usize>, LineError> {
    let mut saw_result_section = false;
    let mut in_room_section = false;
    let mut matched_index = None;
    for (index, label) in labels.into_iter().enumerate() {
        if let Some(room_section) = search_result_section(label) {
            saw_result_section = true;
            in_room_section = room_section;
            continue;
        }

        // Row names can append a timestamp, unread count and preview to the room title.
        let matches_room = label == room_name
            || label.strip_prefix(room_name).is_some_and(|suffix| suffix.starts_with(", "));
        if in_room_section && matches_room && matched_index.replace(index).is_some() {
            return Err(LineError::new("401", "找到多筆同名聊天室搜尋結果"));
        }
    }
    if saw_result_section && matched_index.is_none() {
        return Err(LineError::new("401", "聊天搜尋結果中找不到指定聊天室"));
    }
    Ok(matched_index)
}

fn chat_title_matches(title: &str, room_name: &str) -> bool {
    title == room_name
        || title.strip_prefix(room_name).is_some_and(|suffix| {
            let member_count = suffix.strip_prefix(" (")
                .and_then(|count| count.strip_suffix(')'))
                .or_else(|| suffix.strip_prefix(", ").and_then(|count| count.strip_suffix('人')));
            member_count.is_some_and(|count| count.parse::<u32>().is_ok())
        })
}

fn is_main_panel_input_rect(
    main_left: i32,
    main_right: i32,
    main_top: i32,
    main_bottom: i32,
    edit_left: i32,
    edit_right: i32,
    edit_top: i32,
    edit_height: i32,
    edit_width: i32,
) -> bool {
    let main_width = main_right - main_left;
    let right_panel_left = main_left + (main_width / 2);
    let bottom_area_top = main_top + ((main_bottom - main_top) * 2 / 3);

    edit_left >= right_panel_left
        && edit_right <= main_right
        && edit_top >= bottom_area_top
        && edit_height >= 20
        && edit_width >= 100
}

#[allow(dead_code)]
fn push_element_signature(output: &mut String, element: &UIElement) {
    if let Ok(value) = element.get_name() {
        output.push_str(&value);
        output.push('\n');
    }
    if let Ok(value) = element.get_classname() {
        output.push_str(&value);
        output.push('\n');
    }
    if let Ok(value) = element.get_automation_id() {
        output.push_str(&value);
        output.push('\n');
    }
}

fn trace_step(start: Instant, message: &str) {
    println!("[line +{:>5}ms] {}", start.elapsed().as_millis(), message);
}

fn should_restore_message_after_file_confirm(input_value: Option<&str>, message: &str) -> bool {
    !input_value.unwrap_or_default().contains(message)
}

#[cfg(test)]
fn poll_attempts(timeout: Duration, interval: Duration) -> u32 {
    let timeout_ms = timeout.as_millis();
    let interval_ms = interval.as_millis().max(1);

    timeout_ms.div_ceil(interval_ms) as u32
}

impl LineController {
    pub fn new(options: LineOptions) -> Result<Self> {
        Ok(Self {
            automation: UIAutomation::new().context("初始化 Windows UI Automation 失敗")?,
            options,
            search_edit_cache: Mutex::new(None),
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
        let trace_start = Instant::now();
        trace_step(trace_start, "send_message start");
        let room = self.open_room(room_name)?;
        trace_step(trace_start, "open_room done");
        let target = self.room_chat_input(&room)?;
        trace_step(trace_start, "chat input found");

        target
            .set_focus()
            .map_err(|e| LineError::new("405", format!("聚焦聊天輸入框失敗: {}", e)))?;
        sleep(Duration::from_millis(150));
        trace_step(trace_start, "chat input focused");

        // 貼上訊息
        self.paste_text(&target, message)?;
        trace_step(trace_start, "message pasted");
        
        // 再等1秒
        sleep(Duration::from_secs(1));

        // 再按 enter
        self.send_enter_to_input(&target)
            .map_err(|e| LineError::new("405", format!("送出訊息失敗: {}", e)))?;
        trace_step(trace_start, "message enter sent");

        // 給 LINE 一點時間完成送出狀態更新，再關閉獨立聊天室視窗。
        sleep(Duration::from_millis(300));

        self.finish_chat(&room.main_window, room.close_chat_window.as_ref());
        trace_step(trace_start, "finish_chat done");
        Ok(())
    }

    pub fn send_file(&self, room_name: &str, file_path: &Path) -> Result<(), LineError> {
        let trace_start = Instant::now();
        trace_step(trace_start, "send_file start");
        let room = self.open_room(room_name)?;
        trace_step(trace_start, "open_room done");
        let target = self.room_chat_input(&room)?;
        trace_step(trace_start, "chat input found");

        target
            .set_focus()
            .map_err(|e| LineError::new("405", format!("聚焦聊天輸入框失敗: {}", e)))?;
        sleep(Duration::from_millis(150));
        trace_step(trace_start, "chat input focused");

        self.paste_file(&target, file_path)?;
        trace_step(trace_start, "file pasted");
        
        // 1. 等待 1.5 秒讓「傳送圖片確認彈窗」完全加載並自動取得焦點
        sleep(Duration::from_millis(1500));

        // 2. 第一個 enter：直接對 chat_win 送出 enter！
        // 因為彈窗是 chat_win 的子視窗/模態彈窗，所以對 chat_win 送出 enter 會由系統自動路由給當前處於最上層的彈窗確認按鈕！
        // 這完美避免了使用 get_focused_element() 時，若控制台在最上層會導致 enter 送錯給命令提示字元的問題！
        room.chat_surface.send_keys("{enter}", 30)
            .map_err(|e| LineError::new("405", format!("送出附件確認失敗: {}", e)))?;
        trace_step(trace_start, "file confirm enter sent");
            
        // 等待 1 秒讓確認彈窗關閉並把圖放進對話框
        sleep(Duration::from_secs(1));

        // 3. 第二個 enter：再次聚焦對話框並發送 enter，將圖片真正發送出去！
        target
            .set_focus()
            .map_err(|e| LineError::new("405", format!("重新聚焦輸入框失敗: {}", e)))?;
        self.send_enter_to_input(&target)
            .map_err(|e| LineError::new("405", format!("發送圖片失敗: {}", e)))?;
        trace_step(trace_start, "file send enter sent");
            
        // 4. 等待 1 秒讓 LINE 完成送出狀態更新，再關閉獨立聊天室視窗。
        sleep(Duration::from_millis(1000));

        self.finish_chat(&room.main_window, room.close_chat_window.as_ref());
        trace_step(trace_start, "finish_chat done");
        Ok(())
    }

    pub fn send_message_with_file(
        &self,
        room_name: &str,
        message: &str,
        file_path: &Path,
    ) -> Result<(), LineError> {
        let trace_start = Instant::now();
        trace_step(trace_start, "send_message_with_file start");
        let room = self.open_room(room_name)?;
        trace_step(trace_start, "open_room done");
        let target = self.room_chat_input(&room)?;
        trace_step(trace_start, "chat input found");

        target
            .set_focus()
            .map_err(|e| LineError::new("405", format!("聚焦聊天輸入框失敗: {}", e)))?;
        sleep(Duration::from_millis(150));
        trace_step(trace_start, "chat input focused");

        // 先貼附件再貼文字，避免附件確認 Enter 被 LINE 當成送出文字，造成同內容重複發送。
        self.paste_file(&target, file_path)?;
        trace_step(trace_start, "file pasted");

        // 等待附件確認彈窗出現並取得焦點。
        sleep(Duration::from_millis(1500));
        room.chat_surface.send_keys("{enter}", 30)
            .map_err(|e| LineError::new("405", format!("送出附件確認失敗: {}", e)))?;
        trace_step(trace_start, "file confirm enter sent");

        // 讓確認彈窗關閉並將圖片掛到同一則待送訊息上。
        sleep(Duration::from_secs(1));
        target
            .set_focus()
            .map_err(|e| LineError::new("405", format!("重新聚焦輸入框失敗: {}", e)))?;

        let input_value = self.input_text_value(&target).ok();
        if should_restore_message_after_file_confirm(input_value.as_deref(), message) {
            self.paste_text(&target, message)?;
            trace_step(trace_start, "message pasted after file confirm");
            sleep(Duration::from_millis(300));
        } else {
            trace_step(trace_start, "message already in input after file confirm");
        }

        self.send_enter_to_input(&target)
            .map_err(|e| LineError::new("405", format!("送出文字與附件失敗: {}", e)))?;
        trace_step(trace_start, "message and file enter sent");

        sleep(Duration::from_millis(1000));
        self.finish_chat(&room.main_window, room.close_chat_window.as_ref());
        trace_step(trace_start, "finish_chat done");
        Ok(())
    }

    fn open_room(&self, room_name: &str) -> Result<OpenedRoom, LineError> {
        let trace_start = Instant::now();
        trace_step(trace_start, "open_room start");
        let main_window = self.ensure_main_window()?;
        trace_step(trace_start, "main window ready");
        self.activate_window(&main_window)?;
        trace_step(trace_start, "main window activated");

        let search_edit = self
            .find_search_edit(&main_window)
            .map_err(|e| LineError::new("401", format!("找不到 LINE 搜尋框: {}", e)))?;
        trace_step(trace_start, "search edit found");
        self.set_edit_value(&search_edit, room_name)?;
        trace_step(trace_start, "room name pasted");

        // 1. 輪詢搜尋框下方結果；找到可點擊結果就立刻單擊，優先使用主視窗右側聊天面板。
        let search_result = self.wait_first_search_result(
            &main_window,
            &search_edit,
            room_name,
            Duration::from_secs(5),
            Duration::from_millis(100),
        )?;
        trace_step(trace_start, "search result found");

        let search_rect = search_edit.get_bounding_rectangle()
            .map_err(|e| LineError::new("401", format!("取得搜尋框位置失敗: {e}")))?;
        let process_id = main_window.get_process_id()
            .map_err(|e| LineError::new("401", format!("取得 LINE 程序 ID 失敗: {e}")))?;
        if ButtonControl::try_from(&search_result).and_then(|button| button.invoke()).is_err() {
            self.click_search_result(&search_result, false)?;
        }
        trace_step(trace_start, "search result selected");

        if let Ok(chat_input) = self.wait_main_panel_chat_input(
            &main_window, &search_rect, room_name, Duration::from_millis(2500), Duration::from_millis(100),
        ) {
            trace_step(trace_start, "main panel chat input ready");
            return Ok(OpenedRoom {
                main_window: main_window.clone(),
                chat_surface: main_window,
                chat_input: Some(chat_input),
                close_chat_window: None,
            });
        }

        // 2. 單擊可能已開獨立窗，只有尚未找到指定聊天室時才雙擊。
        let chat_win = match self.find_active_chat_window(process_id, room_name) {
            Ok(chat_win) => chat_win,
            Err(_) => {
                self.click_search_result(&search_result, true)?;
                trace_step(trace_start, "fallback double clicked");
                self.wait_active_chat_window(
                    process_id, room_name, Duration::from_secs(10), Duration::from_millis(100),
                )?
            }
        };
        trace_step(trace_start, "chat window found");

        Ok(OpenedRoom {
            main_window,
            chat_surface: chat_win.clone(),
            chat_input: None,
            close_chat_window: Some(chat_win),
        })
    }

    fn click_search_result(&self, result: &UIElement, double: bool) -> Result<(), LineError> {
        let click = || -> Result<()> {
            let point = result.get_click_point()?;
            let walker = self.automation.get_control_view_walker()?;
            let mut hit = self.automation.element_from_point(point)?;
            // UIA 可回傳列的矩形中心，即使該位置已被另一個視窗遮擋。
            while !self.automation.compare_elements(&hit, result)? {
                hit = walker.get_parent(&hit).context("結果列被遮擋或已變動")?;
            }
            let mouse = Mouse::default();
            if double {
                mouse.double_click(&point)?;
            } else {
                mouse.click(&point)?;
            }
            Ok(())
        };
        click().map_err(|e| LineError::new("401", format!("無法安全點選聊天室搜尋結果: {e:#}")))
    }

    fn ensure_main_window(&self) -> Result<UIElement, LineError> {
        if let Ok(main_window) = self.find_main_window(0) {
            return Ok(main_window);
        }

        match main_window_startup_action(false, self.options.launcher_path.exists()) {
            Ok(MainWindowStartupAction::UseExisting) => unreachable!("前面已直接回傳既有主視窗"),
            Ok(MainWindowStartupAction::Launch) => {
                Command::new(&self.options.launcher_path)
                    .spawn()
                    .map_err(|e| LineError::new("404", format!("啟動 LINE 失敗: {}", e)))?;
                // 等待 LINE 完整啟動與視窗加載
                sleep(Duration::from_secs(3));
            }
            Err(_) => {
                return Err(LineError::new(
                    "404",
                    format!(
                        "LineLauncher.exe 不存在: {}",
                        self.options.launcher_path.display()
                    ),
                ));
            }
        }

        self.find_main_window(30000)
            .map_err(|e| LineError::new("404", format!("等待 LINE 視窗逾時: {}", e)))
    }

    fn find_main_window(&self, timeout_ms: u64) -> Result<UIElement> {
        self.automation
            .create_matcher()
            .control_type(ControlType::Window)
            .classname("AllInOneWindow")
            .depth(2)
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
        set_clipboard(formats::Unicode, text)
            .map_err(|e| LineError::new("405", format!("寫入 Unicode 文字到剪貼簿失敗: {}", e)))?;

        // 先讀回確認，避免 VM 端剪貼簿轉碼成 ??? 後又繼續貼進 LINE 搜尋框。
        let clipboard_text: String = get_clipboard(formats::Unicode)
            .map_err(|e| LineError::new("405", format!("讀回 Unicode 剪貼簿失敗: {}", e)))?;
        if clipboard_text != text {
            return Err(LineError::new(
                "405",
                format!(
                    "剪貼簿文字驗證失敗，expected=\"{}\", actual=\"{}\"",
                    text, clipboard_text
                ),
            ));
        }

        self.send_keys_to_input(element, "{ctrl}v")
            .map_err(|e| LineError::new("405", format!("貼上文字失敗: {}", e)))?;
            
        Ok(())
    }

    fn paste_file(&self, element: &UIElement, file_path: &Path) -> Result<(), LineError> {
        let path_text = file_path.to_string_lossy().to_string();
        let files = vec![path_text.as_str()];

        {
            let _clipboard = Clipboard::new_attempts(10)
                .map_err(|e| LineError::new("405", format!("開啟剪貼簿失敗: {}", e)))?;
            formats::FileList
                .write_clipboard(&files)
                .map_err(|e| LineError::new("405", format!("設定檔案剪貼簿失敗: {}", e)))?;
        } // 關鍵：在此處釋放 (Drop) 剪貼簿鎖！否則 LINE 會因為被鎖定而無法讀取剪貼簿

        self.send_keys_to_input(element, "{ctrl}v")
            .map_err(|e| LineError::new("405", format!("貼上附件失敗: {}", e)))?;

        Ok(())
    }

    fn send_enter_to_input(&self, element: &UIElement) -> Result<()> {
        self.send_keys_to_input(element, "{enter}")
    }

    fn send_keys_to_input(&self, element: &UIElement, keys: &str) -> Result<()> {
        element.set_focus()?;
        if !element.has_keyboard_focus()? {
            sleep(Duration::from_millis(100));
            anyhow::ensure!(element.has_keyboard_focus()?, "LINE 輸入框未取得鍵盤焦點");
        }
        Keyboard::new().interval(30).send_keys(keys)?;
        Ok(())
    }

    fn input_text_value(&self, element: &UIElement) -> Result<String> {
        let edit = EditControl::try_from(element)?;
        Ok(edit.get_value()?)
    }

    #[allow(dead_code)]
    fn item_contains_text(&self, item: &UIElement, text: &str) -> bool {
        self.item_text_signature(item).contains(text)
    }

    #[allow(dead_code)]
    fn item_text_signature(&self, item: &UIElement) -> String {
        let mut text = String::new();
        push_element_signature(&mut text, item);

        // LINE 搜尋結果的 ListItem 有時 name 是空的，文字可能藏在子層 Text/Label 類元素。
        if let Ok(found) = self.automation
            .create_matcher()
            .from_ref(item)
            .depth(4)
            .timeout(0)
            .find_all()
        {
            for child in found {
                push_element_signature(&mut text, &child);
            }
        }

        text
    }

    fn find_search_edit(&self, window: &UIElement) -> Result<UIElement> {
        if let Ok(cache) = self.search_edit_cache.lock() {
            if let Some(edit) = cache.as_ref() {
                if edit.get_classname().unwrap_or_default() == "LcTextField" {
                    return Ok(edit.clone());
                }
            }
        }

        let edit = self.automation
            .create_matcher()
            .from_ref(window)
            .control_type(ControlType::Edit)
            .classname("LcTextField")
            .depth(12)
            .timeout(1000)
            .find_first()
            .context("找不到 LcTextField")?;

        if let Ok(mut cache) = self.search_edit_cache.lock() {
            *cache = Some(edit.clone());
        }

        Ok(edit)
    }

    fn find_result_items(&self, window: &UIElement, timeout_ms: u64) -> Result<Vec<UIElement>> {
        self.automation
            .create_matcher()
            .from_ref(window)
            .control_type(ControlType::ListItem)
            .timeout(timeout_ms)
            .find_all()
            .context("找不到聊天室搜尋結果")
    }

    fn find_room_result_button(
        &self,
        main_window: &UIElement,
        search_rect: &Rect,
        room_name: &str,
    ) -> Result<Option<UIElement>, LineError> {
        let left = search_rect.get_left();
        let right = search_rect.get_right();
        let top = search_rect.get_top();
        let bottom = search_rect.get_bottom();
        let Ok(list) = self.automation
            .create_matcher()
            .from_ref(main_window)
            .control_type(ControlType::List)
            .classname("LcListView")
            .depth(12)
            .filter_fn(Box::new(move |element: &UIElement| {
                let rect = element.get_bounding_rectangle()?;
                Ok(rect.get_left() < right && rect.get_right() > left
                    && rect.get_top() >= top && rect.get_bottom() > bottom
                    && !element.is_offscreen()?)
            }))
            .timeout(0)
            .find_first()
        else {
            return Ok(None);
        };

        let buttons = self.automation
            .create_matcher()
            .from_ref(&list)
            .control_type(ControlType::Button)
            .depth(2)
            .timeout(0)
            .find_all()
            .unwrap_or_default();
        let rows: Vec<_> = buttons.into_iter()
            .filter_map(|button| {
                let label = button.get_name().ok()?;
                let rect = button.get_bounding_rectangle().ok()?;
                // The heading can begin slightly above the bottom edge of the search box.
                if button.is_offscreen().ok()? || rect.get_height() <= 0
                    || rect.get_width() <= 0 || rect.get_bottom() <= bottom
                {
                    return None;
                }
                Some((button, label))
            })
            .collect();
        let index = find_room_button_index(rows.iter().map(|(_, name)| name.as_str()), room_name)?;
        Ok(index.map(|index| rows[index].0.clone()))
    }

    fn wait_first_search_result(
        &self,
        main_window: &UIElement,
        search_edit: &UIElement,
        room_name: &str,
        timeout: Duration,
        interval: Duration,
    ) -> Result<UIElement, LineError> {
        let deadline = Instant::now() + timeout;

        loop {
            match self.find_first_search_result(main_window, search_edit, room_name) {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if Instant::now() >= deadline {
                        return Err(e);
                    }
                }
            }

            sleep(interval);
        }
    }

    fn find_first_search_result(
        &self,
        main_window: &UIElement,
        search_edit: &UIElement,
        room_name: &str,
    ) -> Result<UIElement, LineError> {
        let search_rect = search_edit
            .get_bounding_rectangle()
            .map_err(|e| LineError::new("401", format!("取得搜尋框位置失敗: {}", e)))?;
        if let Some(result) = self.find_room_result_button(main_window, &search_rect, room_name)? {
            return Ok(result);
        }

        let search_left = search_rect.get_left();
        let search_right = search_rect.get_right();
        let search_bottom = search_rect.get_bottom();

        let mut candidates = self
            .find_result_items(main_window, 0)
            .map_err(|e| LineError::new("401", format!("找不到聊天室搜尋結果: {}", e)))?
            .into_iter()
            .filter_map(|item| {
                let rect = item.get_bounding_rectangle().ok()?;
                if !is_search_result_row_rect(
                    search_left,
                    search_right,
                    search_bottom,
                    rect.get_left(),
                    rect.get_right(),
                    rect.get_top(),
                    rect.get_height(),
                    rect.get_width(),
                ) {
                    return None;
                }
                Some((item, rect.get_top()))
            })
            .collect::<Vec<_>>();

        candidates.sort_by_key(|(_, top)| *top);

        let result = candidates
            .first()
            .map(|(item, _)| item)
            .ok_or_else(|| LineError::new("401", "搜尋框下方找不到可點擊的聊天室結果"))?;

        Ok(result.clone())
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

    fn find_active_chat_window(&self, process_id: u32, room_name: &str) -> Result<UIElement> {
        let windows = self.automation
            .create_matcher()
            .control_type(ControlType::Window)
            .classname("ChatWindow")
            .name(room_name)
            .filter_fn(Box::new(move |window: &UIElement| Ok(window.get_process_id()? == process_id)))
            .depth(2)
            .timeout(0)
            .find_all()
            .context("找不到指定聊天室的獨立視窗")?;
        match windows.as_slice() {
            [window] => Ok(window.clone()),
            _ => anyhow::bail!("有多個同名聊天室視窗"),
        }
    }

    fn wait_active_chat_window(
        &self,
        process_id: u32,
        room_name: &str,
        timeout: Duration,
        interval: Duration,
    ) -> Result<UIElement, LineError> {
        let deadline = Instant::now() + timeout;

        loop {
            match self.find_active_chat_window(process_id, room_name) {
                Ok(chat_win) => return Ok(chat_win),
                Err(e) => {
                    if Instant::now() >= deadline {
                        return Err(LineError::new(
                            "401",
                            format!("開啟聊天室後找不到聊天視窗: {}", e),
                        ));
                    }
                }
            }

            sleep(interval);
        }
    }

    fn wait_main_panel_chat_input(
        &self,
        main_window: &UIElement,
        search_rect: &Rect,
        room_name: &str,
        timeout: Duration,
        interval: Duration,
    ) -> Result<UIElement, LineError> {
        let deadline = Instant::now() + timeout;

        loop {
            match self.find_main_panel_chat_input(main_window) {
                Ok(input) if self.main_panel_title_matches(main_window, search_rect, room_name) => {
                    return Ok(input);
                }
                _ if Instant::now() >= deadline => {
                    return Err(LineError::new("401", "主視窗右側未確認開啟指定聊天室"));
                }
                _ => {}
            }

            sleep(interval);
        }
    }

    fn main_panel_title_matches(&self, main_window: &UIElement, search_rect: &Rect, room_name: &str) -> bool {
        let right = search_rect.get_right();
        let bottom = search_rect.get_bottom();
        let room_name = room_name.to_owned();
        self.automation
            .create_matcher()
            .from_ref(main_window)
            .depth(14)
            .timeout(0)
            .filter_fn(Box::new(move |element: &UIElement| {
                if !chat_title_matches(&element.get_name().unwrap_or_default(), &room_name) {
                    return Ok(false);
                }
                let rect = element.get_bounding_rectangle()?;
                Ok(rect.get_left() >= right && rect.get_top() < bottom
                    && rect.get_width() > 0 && rect.get_height() > 0
                    && !element.is_offscreen()?)
            }))
            .find_first()
            .is_ok()
    }

    fn find_main_panel_chat_input(&self, main_window: &UIElement) -> Result<UIElement, LineError> {
        if let Ok(input) = self.find_chat_input_with_timeout(main_window, 0) {
            return Ok(input);
        }

        let main_rect = main_window
            .get_bounding_rectangle()
            .map_err(|e| LineError::new("405", format!("取得 LINE 主視窗位置失敗: {}", e)))?;

        let mut candidates = self
            .automation
            .create_matcher()
            .from_ref(main_window)
            .control_type(ControlType::Edit)
            .depth(14)
            .timeout(0)
            .find_all()
            .map_err(|e| LineError::new("405", format!("掃描主視窗輸入框失敗: {}", e)))?
            .into_iter()
            .filter_map(|edit| {
                let rect = edit.get_bounding_rectangle().ok()?;
                if !is_main_panel_input_rect(
                    main_rect.get_left(),
                    main_rect.get_right(),
                    main_rect.get_top(),
                    main_rect.get_bottom(),
                    rect.get_left(),
                    rect.get_right(),
                    rect.get_top(),
                    rect.get_height(),
                    rect.get_width(),
                ) {
                    return None;
                }
                Some((edit, rect.get_top()))
            })
            .collect::<Vec<_>>();

        candidates.sort_by_key(|(_, top)| *top);

        candidates
            .last()
            .map(|(edit, _)| edit.clone())
            .ok_or_else(|| LineError::new("405", "主視窗右側面板找不到可用輸入框"))
    }

    fn find_chat_input(&self, chat_surface: &UIElement) -> Result<UIElement> {
        self.find_chat_input_with_timeout(chat_surface, 1000)
    }

    fn room_chat_input(&self, room: &OpenedRoom) -> Result<UIElement, LineError> {
        self.activate_window(&room.chat_surface)?;
        if let Some(input) = room.chat_input.as_ref() {
            return Ok(input.clone());
        }

        self.find_chat_input(&room.chat_surface)
            .map_err(|e| LineError::new("405", format!("找不到聊天輸入框: {}", e)))
    }

    fn find_chat_input_with_timeout(&self, chat_surface: &UIElement, timeout_ms: u64) -> Result<UIElement> {
        self.automation
            .create_matcher()
            .from_ref(chat_surface)
            .control_type(ControlType::Edit)
            .classname("AutoSuggestTextArea")
            .depth(10)
            .timeout(timeout_ms)
            .find_first()
            .context("找不到聊天室輸入框 AutoSuggestTextArea")
    }

    fn finish_chat(&self, main_window: &UIElement, chat_win: Option<&UIElement>) {
        if self.options.close_chat_after_send {
            if let Some(chat_win) = chat_win {
                let _ = chat_win.send_keys("{alt}{f4}", 30);
                sleep(Duration::from_millis(500));
            }
        }

        let _ = main_window;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_row_must_be_visible_and_below_search_box() {
        assert!(is_search_result_row_rect(100, 300, 120, 90, 320, 150, 80, 260));

        assert!(!is_search_result_row_rect(100, 300, 120, 90, 320, 90, 80, 260));
        assert!(!is_search_result_row_rect(100, 300, 120, 90, 320, 150, 0, 260));
        assert!(!is_search_result_row_rect(100, 300, 120, 90, 320, 150, 80, 0));
        assert!(!is_search_result_row_rect(100, 300, 120, 350, 600, 150, 80, 260));
    }

    #[test]
    fn selects_room_button_in_chat_section_before_same_named_message() {
        let labels = [
            "聊天, 1, 關閉清單",
            "測試聊天室, 15:20, 1, 收到",
            "訊息, 1, 關閉清單",
            "測試聊天室, 15:20, 1, 收到",
        ];
        assert_eq!(find_room_button_index(labels, "測試聊天室").unwrap(), Some(1));
        assert!(find_room_button_index(labels, "測試").is_err());
        assert!(find_room_button_index(labels[2..].iter().copied(), "測試聊天室").is_err());
        assert_eq!(find_room_button_index(["既有聊天列"], "測試聊天室").unwrap(), None);
    }

    #[test]
    fn rejects_ambiguous_room_buttons() {
        let labels = ["聊天, 2, 關閉清單", "測試聊天室", "測試聊天室, 15:20"];
        assert!(find_room_button_index(labels, "測試聊天室").is_err());
    }

    #[test]
    fn chat_title_requires_the_room_name_with_optional_member_count() {
        assert!(chat_title_matches("測試聊天室", "測試聊天室"));
        assert!(chat_title_matches("測試聊天室 (23)", "測試聊天室"));
        assert!(chat_title_matches("測試聊天室, 23人", "測試聊天室"));
        assert!(!chat_title_matches("測試聊天室備用", "測試聊天室"));
        assert!(!chat_title_matches("測試聊天室 (副本)", "測試聊天室"));
    }

    #[test]
    fn main_panel_input_must_be_on_right_bottom_area() {
        assert!(is_main_panel_input_rect(700, 1400, 20, 700, 1050, 1380, 600, 40, 330));

        assert!(!is_main_panel_input_rect(700, 1400, 20, 700, 780, 1010, 600, 40, 230));
        assert!(!is_main_panel_input_rect(700, 1400, 20, 700, 1050, 1380, 300, 40, 330));
        assert!(!is_main_panel_input_rect(700, 1400, 20, 700, 1050, 1380, 600, 10, 330));
    }

    #[test]
    fn existing_main_window_does_not_need_launcher() {
        assert_eq!(
            main_window_startup_action(true, false).unwrap(),
            MainWindowStartupAction::UseExisting
        );
    }

    #[test]
    fn chat_window_polling_has_bounded_attempts() {
        assert_eq!(
            poll_attempts(Duration::from_secs(10), Duration::from_millis(100)),
            100
        );
        assert_eq!(
            poll_attempts(Duration::from_millis(10001), Duration::from_millis(100)),
            101
        );
    }

    #[test]
    fn search_result_polling_has_bounded_attempts() {
        assert_eq!(
            poll_attempts(Duration::from_secs(5), Duration::from_millis(100)),
            50
        );
        assert_eq!(
            poll_attempts(Duration::from_millis(5001), Duration::from_millis(100)),
            51
        );
    }

    #[test]
    fn message_restore_skips_when_input_already_has_message() {
        assert!(!should_restore_message_after_file_confirm(Some("測試文字"), "測試文字"));
    }
}
