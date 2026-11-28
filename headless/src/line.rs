use crate::delivery::Part;
use anyhow::{Context, Result, anyhow, ensure};
use line_rs::{
    Device,
    client::{
        Chat, Client, InitOptions, login_with_auth_token, login_with_qr, message::TalkMessage,
    },
    storage::{FileStorage, Storage},
};
use qrcode::{QrCode, render::unicode};
use serde::Serialize;
use std::{
    io::{self, IsTerminal},
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::runtime::Runtime;

const TOKEN_KEY: &str = "3wa:headless:authorized_access";
const QUIET: Duration = Duration::from_secs(30);

pub struct Session {
    rt: Runtime,
    client: Client,
    chat: Chat,
}

#[derive(Serialize)]
pub struct PreviewRow {
    created_ms: Option<i64>,
    text: String,
}

impl Session {
    pub fn qr(path: &Path) -> Result<()> {
        ensure!(
            io::stderr().is_terminal(),
            "QR 授權只能在本人可操作的終端機執行"
        );
        let store = storage(path)?;
        let rt = Runtime::new()?;
        rt.block_on(async {
            ensure!(
                store
                    .get_string(TOKEN_KEY)
                    .await
                    .map_err(|_| anyhow!("無法讀取私有 session"))?
                    .is_none(),
                "已有本機 session；先用 check，若授權已撤銷請另建私人目錄重新授權"
            );
            let client = tokio::time::timeout(
                Duration::from_secs(180),
                login_with_qr(
                    init(store),
                    |url| {
                        if let Ok(code) = QrCode::new(url.as_bytes()) {
                            eprintln!("{}", code.render::<unicode::Dense1x2>().build());
                        } else {
                            eprintln!("無法顯示 QR code，請停止重試。");
                        }
                    },
                    |pin| {
                        if pin.chars().all(|c| c.is_ascii_digit()) {
                            eprintln!("請在手機 LINE 輸入：{pin}");
                        }
                    },
                ),
            )
            .await
            .map_err(|_| anyhow!("QR 授權逾時"))?
            .map_err(|_| anyhow!("LINE QR 授權失敗；未送出訊息"))?;
            persist_access(&client).await?;
            println!("授權完成，未讀取聊天或發送訊息。");
            Ok(())
        })
    }

    pub fn open(path: &Path, id: &str, name: &str) -> Result<Self> {
        ensure!(
            id.len() == 33
                && id.starts_with('c')
                && id[1..].bytes().all(|c| c.is_ascii_hexdigit())
                && !name.is_empty(),
            "請指定一般 LINE 群組的 ID 與確切名稱（不支援 OpenChat）"
        );
        ensure!(path.is_file(), "未有獨立 LINE 授權，請先本人以 login 掃 QR");
        let store = storage(path)?;
        let rt = Runtime::new()?;
        let (client, chat) = rt.block_on(async {
            let token = store
                .get_string(TOKEN_KEY)
                .await
                .map_err(|_| anyhow!("無法讀取私有 session"))?
                .filter(|t| !t.is_empty())
                .context("找不到本程式自行授權的 session")?;
            let client = tokio::time::timeout(QUIET, login_with_auth_token(&token, init(store)))
                .await
                .map_err(|_| anyhow!("LINE 連線逾時"))?
                .map_err(|_| anyhow!("LINE 無法恢復本機 session；若已撤銷需本人重新掃 QR"))?;
            let chat = tokio::time::timeout(QUIET, client.get_chat(id))
                .await
                .map_err(|_| anyhow!("查詢指定群組逾時"))?
                .map_err(|_| anyhow!("找不到指定群組或帳號無權存取"))?;
            ensure!(
                chat.mid() == id && chat.name() == name,
                "群組 ID 或名稱未能吻合"
            );
            persist_access(&client).await?;
            Ok::<_, anyhow::Error>((client, chat))
        })?;
        Ok(Self { rt, client, chat })
    }

    pub fn send(&self, part: Part) -> Result<()> {
        self.rt.block_on(async {
            tokio::time::timeout(QUIET, async {
                match part {
                    Part::Text(text) => {
                        self.chat.send_message(text).await?;
                    }
                    Part::Image(bytes, filename) => {
                        self.chat.send_image(bytes, Some(&filename)).await?;
                    }
                }
                Ok::<_, line_rs::Error>(())
            })
            .await
            .map_err(|_| anyhow!("LINE 發送逾時"))?
            .map_err(|_| anyhow!("LINE 未確認這段發送"))?;
            persist_access(&self.client).await
        })
    }

    pub fn read(&self, n: i32) -> Result<Vec<PreviewRow>> {
        self.rt.block_on(async {
            let rows = tokio::time::timeout(QUIET, async {
                let values = self
                    .client
                    .base
                    .talk()
                    .get_recent_messages_v2(self.chat.mid(), n)
                    .await
                    .map_err(|_| anyhow!("無法取回指定群的近況"))?;
                let values = values.as_list().context("LINE 回傳格式不符")?;
                let mut result = Vec::new();
                for row in values.iter().take(n as usize) {
                    if row["to"].as_str() != Some(self.chat.mid()) {
                        continue;
                    }
                    let Ok(msg) = TalkMessage::from_raw(self.client.clone(), row.clone()).await
                    else {
                        continue;
                    };
                    if msg.content_type() != "NONE" || msg.text().is_empty() {
                        continue;
                    }
                    result.push(PreviewRow {
                        created_ms: msg.raw["createdTime"].as_i64(),
                        text: msg.text().to_owned(),
                    });
                }
                Ok::<_, anyhow::Error>(result)
            })
            .await
            .map_err(|_| anyhow!("讀取指定群逾時"))??;
            persist_access(&self.client).await?;
            Ok(rows)
        })
    }
}

fn storage(path: &Path) -> Result<Arc<dyn Storage>> {
    Ok(Arc::new(
        FileStorage::new(path).map_err(|_| anyhow!("無法開啟私有 session"))?,
    ))
}

fn init(store: Arc<dyn Storage>) -> InitOptions {
    InitOptions::new(Device::DesktopWin)
        .device_name("3WA headless test")
        .storage_arc(store)
}

async fn persist_access(client: &Client) -> Result<()> {
    let token = client
        .auth_token()
        .filter(|s| !s.is_empty())
        .context("LINE 授權已失效")?;
    let storage = &client.base.storage;
    let previous = storage
        .get_string(TOKEN_KEY)
        .await
        .map_err(|_| anyhow!("無法讀取私有 session"))?;
    if previous.as_deref() != Some(token.as_str()) {
        storage
            .set(TOKEN_KEY, token.into())
            .await
            .map_err(|_| anyhow!("無法保存私有 session"))?;
    }
    Ok(())
}
