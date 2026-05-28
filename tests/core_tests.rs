use std::path::PathBuf;

use line_controller::config::Settings;
use line_controller::models::{WorkItem, is_image_data};
use line_controller::paths::{line_launcher_path, work_cache_path};

#[test]
fn parses_settings_ini_with_trimmed_values() {
    let text = r#"
# 環境設定區
[Settings]
SERVICE_URL = https://map.gis.tw/SystemReport/linebotgis_api.aspx
BOT_TOKEN =  abc123
USER_AGENT =  GIS Test Agent
POLL_SECONDS = 7
"#;

    let settings = Settings::parse_ini(text).expect("settings should parse");

    assert_eq!(
        settings.service_url,
        "https://map.gis.tw/SystemReport/linebotgis_api.aspx"
    );
    assert_eq!(settings.bot_token, "abc123");
    assert_eq!(settings.user_agent, "GIS Test Agent");
    assert_eq!(settings.poll_seconds, 7);
}

#[test]
fn parses_work_item_when_optional_fields_are_missing() {
    let raw = serde_json::json!({
        "id": "42",
        "room_name": "測試聊天室",
        "message": "hello"
    });

    let item: WorkItem = serde_json::from_value(raw).expect("work item should parse");

    assert_eq!(item.id, "42");
    assert_eq!(item.room_name, "測試聊天室");
    assert_eq!(item.message, "hello");
    assert_eq!(item.file_uuid, "");
    assert_eq!(item.file_subname, "");
}

#[test]
fn detects_supported_image_headers() {
    assert!(is_image_data(&[0xff, 0xd8, 0xff, 0x00]));
    assert!(is_image_data(b"\x89PNG\r\n\x1a\nabc"));
    assert!(is_image_data(b"GIF89aabc"));
    assert!(!is_image_data(b"<html>not image</html>"));
}

#[test]
fn builds_expected_windows_paths() {
    let home = PathBuf::from(r"C:\Users\stw_s");
    let base = PathBuf::from(r"D:\mytools\line_controller");

    assert_eq!(
        line_launcher_path(&home),
        PathBuf::from(r"C:\Users\stw_s\AppData\Local\LINE\bin\LineLauncher.exe")
    );
    assert_eq!(
        work_cache_path(&base, "20260529", "100", "png"),
        PathBuf::from(r"D:\mytools\line_controller\cache\20260529\100.png")
    );
}
