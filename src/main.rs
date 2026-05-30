use std::env;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::Local;
use linebot_rust::config::{DEFAULT_CONFIG_FILE, Settings};
use linebot_rust::line_window::{LineController, LineError, LineOptions};
use linebot_rust::models::{WorkItem, is_image_data};
use linebot_rust::paths::{line_launcher_path, work_cache_path};
use linebot_rust::work_api::WorkApi;

fn main() -> Result<()> {
    let args = Args::parse(env::args().skip(1).collect())?;
    let base_dir = env::current_dir().context("取得目前工作目錄失敗")?;
    let settings = Settings::load_or_create(&args.config_path)?;
    let launcher_path = line_launcher_path(&home_dir()?);
    let line = LineController::new(LineOptions {
        launcher_path,
        close_chat_after_send: settings.close_chat_after_send && !args.no_close,
    })?;

    if args.inspect_line {
        println!("{}", line.inspect()?);
        return Ok(());
    }

    if let Some(room) = args.test_room.as_deref() {
        run_test_mode(
            &line,
            room,
            args.test_message.as_deref(),
            args.test_file.as_deref(),
        )?;
        return Ok(());
    }

    let _lock = LockFile::acquire(&base_dir.join("run.lock"))?;
    let api = WorkApi::new(&settings)?;

    loop {
        if consume_shutdown_file(&base_dir.join("restart.txt"))? {
            println!("restart.txt detected, shutdown.");
            break;
        }

        if let Err(err) = run_once(&api, &line, &base_dir) {
            eprintln!("run_once error: {err:#}");
        }

        if args.once {
            break;
        }

        sleep(Duration::from_secs(settings.poll_seconds));
    }

    Ok(())
}

fn run_test_mode(
    line: &LineController,
    room: &str,
    message: Option<&str>,
    file: Option<&Path>,
) -> Result<()> {
    if let Some(message) = message {
        line.send_message(room, message)
            .map_err(anyhow::Error::new)
            .context("測試文字傳送失敗")?;
        println!("test message sent.");
    }

    if let Some(file) = file {
        line.send_file(room, file)
            .map_err(anyhow::Error::new)
            .context("測試附件傳送失敗")?;
        println!("test file sent.");
    }

    if message.is_none() && file.is_none() {
        return Err(anyhow!(
            "--test-room 需要搭配 --test-message 或 --test-file"
        ));
    }

    Ok(())
}

fn run_once(api: &WorkApi, line: &LineController, base_dir: &Path) -> Result<()> {
    let response = api.get_work()?;
    if response.status != "OK" || response.data.is_empty() {
        println!("No works...");
        return Ok(());
    }

    for item in response.data {
        println!("Do work id={}, room={}", item.id, item.room_name);
        let (is_ok, error_code) = process_item(api, line, base_dir, &item);

        if let Err(err) = api.update_work_status(&item.id, is_ok, error_code) {
            eprintln!("updateWorkStatus failed id={}: {err:#}", item.id);
        }
    }

    Ok(())
}

fn process_item(
    api: &WorkApi,
    line: &LineController,
    base_dir: &Path,
    item: &WorkItem,
) -> (bool, &'static str) {
    let mut is_success = true;
    let mut error_code = "200";

    if item.has_message() {
        println!("Do send_message...");
        if let Err(err) = line.send_message(&item.room_name, &item.message) {
            eprintln!("send_message failed: {}", err);
            is_success = false;
            error_code = err.error_code;
        }
    }

    if item.has_file() {
        println!("Do send_file...");
        if let Err(err) = download_and_send_file(api, line, base_dir, item) {
            eprintln!("send_file failed: {}", err);
            is_success = false;
            error_code = err.error_code;
        }
    }

    (is_success, error_code)
}

fn download_and_send_file(
    api: &WorkApi,
    line: &LineController,
    base_dir: &Path,
    item: &WorkItem,
) -> Result<(), LineError> {
    let data = api
        .download_file(&item.file_uuid)
        .map_err(|e| LineError::new("402", format!("附件下載失敗: {e:#}")))?;

    if data.is_empty() {
        return Err(LineError::new("402", "附件下載為空檔"));
    }

    let ext = if item.file_subname.trim().is_empty() {
        "bin"
    } else {
        item.file_subname.trim()
    };

    if matches!(
        ext.to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "gif"
    ) && !is_image_data(&data)
    {
        return Err(LineError::new(
            "402",
            "附件副檔名為圖片，但檔案內容不是圖片",
        ));
    }

    let ymd = Local::now().format("%Y%m%d").to_string();
    let path = work_cache_path(base_dir, &ymd, &item.id, ext);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| LineError::new("402", format!("建立 cache 目錄失敗: {e}")))?;
    }
    fs::write(&path, data).map_err(|e| LineError::new("402", format!("寫入附件失敗: {e}")))?;

    line.send_file(&item.room_name, &path)
}

fn consume_shutdown_file(path: &Path) -> Result<bool> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("刪除 {} 失敗", path.display()))?;
        return Ok(true);
    }
    Ok(false)
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("找不到 USERPROFILE"))
}

#[derive(Debug, Default)]
struct Args {
    config_path: PathBuf,
    once: bool,
    inspect_line: bool,
    no_close: bool,
    test_room: Option<String>,
    test_message: Option<String>,
    test_file: Option<PathBuf>,
}

impl Args {
    fn parse(raw: Vec<String>) -> Result<Self> {
        let mut args = Self {
            config_path: PathBuf::from(DEFAULT_CONFIG_FILE),
            ..Self::default()
        };
        let mut index = 0;

        while index < raw.len() {
            match raw[index].as_str() {
                "--config" => {
                    index += 1;
                    args.config_path = raw
                        .get(index)
                        .map(PathBuf::from)
                        .ok_or_else(|| anyhow!("--config 缺少路徑"))?;
                }
                "--once" => args.once = true,
                "--inspect-line" => args.inspect_line = true,
                "--no-close" => args.no_close = true,
                "--test-room" => {
                    index += 1;
                    args.test_room = Some(
                        raw.get(index)
                            .cloned()
                            .ok_or_else(|| anyhow!("--test-room 缺少聊天室名稱"))?,
                    );
                }
                "--test-message" => {
                    index += 1;
                    args.test_message = Some(
                        raw.get(index)
                            .cloned()
                            .ok_or_else(|| anyhow!("--test-message 缺少訊息"))?,
                    );
                }
                "--test-file" => {
                    index += 1;
                    args.test_file = Some(
                        raw.get(index)
                            .map(PathBuf::from)
                            .ok_or_else(|| anyhow!("--test-file 缺少檔案路徑"))?,
                    );
                }
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => return Err(anyhow!("未知參數: {}", other)),
            }

            index += 1;
        }

        Ok(args)
    }
}

fn print_usage() {
    println!(
        r#"linebot_rust

Usage:
  linebot_rust.exe
  linebot_rust.exe --once
  linebot_rust.exe --inspect-line
  linebot_rust.exe --test-room "聊天室" --test-message "測試訊息"
  linebot_rust.exe --test-room "聊天室" --test-file "D:\path\file.png"

Options:
  --config <path>   指定 config.ini 路徑
  --no-close        測試時不送 Alt+F4 關閉聊天室視窗
"#
    );
}

struct LockFile {
    path: PathBuf,
    file: Option<fs::File>,
}

impl LockFile {
    fn acquire(path: &Path) -> Result<Self> {
        if path.exists() {
            if let Err(e) = fs::remove_file(path) {
                return Err(anyhow!(
                    "已有程序執行中或無法建立 lock: {} (錯誤: {})",
                    path.display(),
                    e
                ));
            }
        }

        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .with_context(|| format!("無法建立 lock 檔案: {}", path.display()))?;

        use std::io::Write;
        let mut file_clone = file.try_clone()?;
        let _ = file_clone.write_all(b"GG");

        Ok(Self {
            path: path.to_path_buf(),
            file: Some(file),
        })
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}
