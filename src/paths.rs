use std::path::{Path, PathBuf};

pub fn line_launcher_path(home_dir: &Path) -> PathBuf {
    home_dir
        .join("AppData")
        .join("Local")
        .join("LINE")
        .join("bin")
        .join("LineLauncher.exe")
}

pub fn line_current_exe_path(home_dir: &Path) -> PathBuf {
    home_dir
        .join("AppData")
        .join("Local")
        .join("LINE")
        .join("bin")
        .join("current")
        .join("LINE.exe")
}

pub fn work_cache_path(base_dir: &Path, ymd: &str, id: &str, file_subname: &str) -> PathBuf {
    base_dir
        .join("cache")
        .join(ymd)
        .join(format!("{}.{}", id, sanitize_extension(file_subname)))
}

fn sanitize_extension(file_subname: &str) -> String {
    let trimmed = file_subname.trim().trim_start_matches('.');
    if trimmed.is_empty() {
        return "bin".to_string();
    }

    trimmed
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}
