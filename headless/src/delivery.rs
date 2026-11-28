use anyhow::{Context, Result};
use linebot_rust::models::{WorkItem, is_image_data};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::Path,
};

#[derive(Debug, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Image(Vec<u8>, String),
}

pub fn plan(job: &WorkItem, data: Option<Vec<u8>>) -> Result<Vec<Part>, &'static str> {
    let mut out = Vec::with_capacity(2);
    if job.has_file() {
        let bytes = data.ok_or("402")?;
        let ext = job.file_subname.to_ascii_lowercase();
        let matches_header = match ext.as_str() {
            "jpg" | "jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
            "png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
            _ => false,
        };
        if bytes.len() > 10 * 1024 * 1024 || !is_image_data(&bytes) || !matches_header {
            return Err("402");
        }
        out.push(Part::Image(bytes, format!("image.{ext}")));
    }
    if job.has_message() {
        out.push(Part::Text(job.message.clone()));
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Pending,
    Sent,
}

pub struct Journal {
    file: File,
    jobs: HashMap<String, Outcome>,
}

impl Journal {
    pub fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(path)?;
        fs2::FileExt::try_lock_exclusive(&file).context("同一個試驗狀態目錄已有程序在使用")?;
        let mut jobs = HashMap::new();
        for line in BufReader::new(&file).lines() {
            let (id, state): (String, Outcome) = serde_json::from_str(&line?)
                .map_err(|_| anyhow::anyhow!("發送紀錄不完整，請人工核對；未發送"))?;
            jobs.insert(id, state);
        }
        Ok(Self { file, jobs })
    }

    pub fn status(&self, id: &str) -> Option<Outcome> {
        self.jobs.get(id).copied()
    }

    pub fn record(&mut self, id: &str, outcome: Outcome) -> Result<()> {
        let mut entry = serde_json::to_vec(&(id, outcome))?;
        entry.push(b'\n');
        self.file.write_all(&entry)?;
        self.file.sync_data()?;
        self.jobs.insert(id.to_owned(), outcome);
        Ok(())
    }
}
