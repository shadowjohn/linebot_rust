use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow};

pub const DEFAULT_CONFIG_FILE: &str = "setting.ini";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub service_url: String,
    pub bot_token: String,
    pub user_agent: String,
    pub poll_seconds: u64,
    pub close_chat_after_send: bool,
}

impl Settings {
    pub fn parse_ini(text: &str) -> Result<Self> {
        let mut section = String::new();
        let mut values = HashMap::<String, String>::new();

        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].trim().to_ascii_lowercase();
                continue;
            }

            if section != "settings" {
                continue;
            }

            if let Some((key, value)) = line.split_once('=') {
                values.insert(key.trim().to_ascii_uppercase(), value.trim().to_string());
            }
        }

        let service_url = take_required(&values, "SERVICE_URL")?;
        let bot_token = take_required(&values, "BOT_TOKEN")?;
        let user_agent = values
            .get("USER_AGENT")
            .cloned()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(default_user_agent);
        let poll_seconds = values
            .get("POLL_SECONDS")
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(5);
        let close_chat_after_send = values
            .get("CLOSE_CHAT_AFTER_SEND")
            .map(|v| parse_bool(v))
            .unwrap_or(true);

        Ok(Self {
            service_url,
            bot_token,
            user_agent,
            poll_seconds,
            close_chat_after_send,
        })
    }

    pub fn load_or_create(path: &Path) -> Result<Self> {
        if !path.exists() {
            fs::write(path, default_ini())
                .with_context(|| format!("無法建立預設設定檔: {}", path.display()))?;
        }

        let text = fs::read_to_string(path)
            .with_context(|| format!("無法讀取設定檔: {}", path.display()))?;
        Self::parse_ini(&text)
    }
}

pub fn default_ini() -> String {
    format!(
        r#"# 環境設定區
[Settings]
# 服務網址
SERVICE_URL = https://map.gis.tw/SystemReport/linebotgis_api.aspx
# 機器人 TOKEN
BOT_TOKEN = 請輸入BOT_TOKEN
# 機器人 USER_AGENT
USER_AGENT = {}
# 輪詢秒數
POLL_SECONDS = 5
# 送出後是否 Alt+F4 關閉目前聊天室視窗
CLOSE_CHAT_AFTER_SEND = true
"#,
        default_user_agent()
    )
}

fn take_required(values: &HashMap<String, String>, key: &str) -> Result<String> {
    values
        .get(key)
        .cloned()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow!("setting.ini 缺少 [Settings] {}", key))
}

fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "y" | "on"
    )
}

fn default_user_agent() -> String {
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 GIS FCU Focusit (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string()
}
