use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;

pub fn log_file_path(base_dir: &Path, ymd: &str) -> PathBuf {
    base_dir.join("log").join(format!("{ymd}.txt"))
}

pub fn append_error(base_dir: &Path, message: &str) -> Result<()> {
    let ymd = Local::now().format("%Y%m%d").to_string();
    append_error_with_ymd(base_dir, &ymd, message)
}

pub fn append_error_with_ymd(base_dir: &Path, ymd: &str, message: &str) -> Result<()> {
    let path = log_file_path(base_dir, ymd);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("建立 log 目錄失敗: {}", parent.display()))?;
    }

    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("開啟 log 檔案失敗: {}", path.display()))?;

    writeln!(file, "[{timestamp}] {message}")
        .with_context(|| format!("寫入 log 檔案失敗: {}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_daily_log_path_under_log_directory() {
        let base = PathBuf::from(r"D:\mytools\linebot_rust");

        assert_eq!(
            log_file_path(&base, "20260531"),
            PathBuf::from(r"D:\mytools\linebot_rust\log\20260531.txt")
        );
    }

    #[test]
    fn append_error_creates_log_directory_and_file() {
        let base = std::env::temp_dir().join(format!(
            "linebot_rust_log_test_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);

        append_error_with_ymd(&base, "20260531", "send_message failed: 401").unwrap();

        let text = fs::read_to_string(log_file_path(&base, "20260531")).unwrap();
        assert!(text.contains("send_message failed: 401"));

        let _ = fs::remove_dir_all(&base);
    }
}
