use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clipboard_win::{Clipboard, Setter, formats, get_clipboard, set_clipboard};
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
        target
            .send_keys("{enter}", 30)
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

        self.paste_text(&target, message)?;
        trace_step(trace_start, "message pasted");
        sleep(Duration::from_millis(300));

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

        // LINE 貼圖確認後可能會清掉原本輸入框文字，這裡補貼一次文字到待送訊息區。
        self.paste_text(&target, message)?;
        trace_step(trace_start, "message restored after file confirm");
        sleep(Duration::from_millis(300));

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

        search_result
            .click()
            .map_err(|e| LineError::new("401", format!("單擊聊天室搜尋結果失敗: {}", e)))?;
        trace_step(trace_start, "search result clicked");

        if let Ok(chat_input) = self.wait_main_panel_chat_input(&main_window, Duration::from_millis(2500), Duration::from_millis(100)) {
            trace_step(trace_start, "main panel chat input ready");
            return Ok(OpenedRoom {
                main_window: main_window.clone(),
                chat_surface: main_window,
                chat_input: Some(chat_input),
                close_chat_window: None,
            });
        }

        // 2. 主視窗面板不可用時，fallback 雙擊開獨立聊天視窗。
        search_result
            .double_click()
            .map_err(|e| LineError::new("401", format!("雙擊聊天室搜尋結果失敗: {}", e)))?;
        trace_step(trace_start, "fallback double clicked");

        let chat_win = self.wait_active_chat_window(
            room_name,
            Duration::from_secs(10),
            Duration::from_millis(100),
        )?;
        trace_step(trace_start, "chat window found");

        Ok(OpenedRoom {
            main_window,
            chat_surface: chat_win.clone(),
            chat_input: None,
            close_chat_window: Some(chat_win),
        })
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

        element
            .send_keys("{ctrl}v", 30)
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

        element
            .send_keys("{ctrl}v", 30)
            .map_err(|e| LineError::new("405", format!("貼上附件失敗: {}", e)))?;

        Ok(())
    }

    fn send_enter_to_input(&self, element: &UIElement) -> Result<()> {
        if element.send_keys("{enter}", 30).is_ok() {
            return Ok(());
        }

        element.set_focus()?;
        sleep(Duration::from_millis(100));
        Keyboard::new().send_keys("{enter}")?;
        Ok(())
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
        _room_name: &str,
    ) -> Result<UIElement, LineError> {
        let search_rect = search_edit
            .get_bounding_rectangle()
            .map_err(|e| LineError::new("401", format!("取得搜尋框位置失敗: {}", e)))?;
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

    fn find_active_chat_window(&self, room_name: &str) -> Result<UIElement> {
        // 第一優先級：精準尋找 classname 為 "ChatWindow" 且視窗名稱 (Title) 與目標 room_name 相同的視窗
        // 這能確保即便開了多個 LINE 獨立聊天視窗，也能百分之百取得對的聊天室！
        if let Ok(matched) = self.automation
            .create_matcher()
            .control_type(ControlType::Window)
            .classname("ChatWindow")
            .name(room_name)
            .depth(2)
            .timeout(0)
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
            .depth(2)
            .timeout(0)
            .find_first()
            .context("找不到任何 ChatWindow")
    }

    fn wait_active_chat_window(
        &self,
        room_name: &str,
        timeout: Duration,
        interval: Duration,
    ) -> Result<UIElement, LineError> {
        let deadline = Instant::now() + timeout;

        loop {
            match self.find_active_chat_window(room_name) {
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
        timeout: Duration,
        interval: Duration,
    ) -> Result<UIElement, LineError> {
        let deadline = Instant::now() + timeout;

        loop {
            match self.find_main_panel_chat_input(main_window) {
                Ok(input) => return Ok(input),
                Err(e) => {
                    if Instant::now() >= deadline {
                        return Err(e);
                    }
                }
            }

            sleep(interval);
        }
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
}
