use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use reqwest::blocking::Client;

use crate::config::Settings;
use crate::models::WorkResponse;

#[derive(Debug, Clone)]
pub struct WorkApi {
    client: Client,
    service_url: String,
    bot_token: String,
}

impl WorkApi {
    pub fn new(settings: &Settings) -> Result<Self> {
        let client = Client::builder()
            .user_agent(settings.user_agent.clone())
            .timeout(Duration::from_secs(20))
            .build()
            .context("建立 HTTP client 失敗")?;

        Ok(Self {
            client,
            service_url: settings.service_url.clone(),
            bot_token: settings.bot_token.clone(),
        })
    }

    pub fn get_work(&self) -> Result<WorkResponse> {
        let text = self
            .client
            .get(&self.service_url)
            .query(&[
                ("mode", "getWork".to_string()),
                ("bot_token", self.bot_token.clone()),
                ("_t", unix_seconds().to_string()),
            ])
            .send()
            .context("getWork 連線失敗")?
            .error_for_status()
            .context("getWork HTTP 狀態異常")?
            .text()
            .context("getWork 讀取回應失敗")?;

        serde_json::from_str(&text).with_context(|| format!("getWork JSON 解析失敗: {}", text))
    }

    pub fn update_work_status(&self, id: &str, is_ok: bool, error_code: &str) -> Result<()> {
        self.client
            .get(&self.service_url)
            .query(&[
                ("mode", "updateWorkStatus".to_string()),
                ("bot_token", self.bot_token.clone()),
                ("is_ok", if is_ok { "1" } else { "0" }.to_string()),
                ("error_code", error_code.to_string()),
                ("id", id.to_string()),
            ])
            .send()
            .context("updateWorkStatus 連線失敗")?
            .error_for_status()
            .context("updateWorkStatus HTTP 狀態異常")?;

        Ok(())
    }

    pub fn download_file(&self, file_uuid: &str) -> Result<Vec<u8>> {
        let bytes = self
            .client
            .get(&self.service_url)
            .query(&[
                ("mode", "getFile".to_string()),
                ("file_uuid", file_uuid.to_string()),
            ])
            .send()
            .context("getFile 連線失敗")?
            .error_for_status()
            .context("getFile HTTP 狀態異常")?
            .bytes()
            .context("getFile 讀取檔案失敗")?;

        Ok(bytes.to_vec())
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
