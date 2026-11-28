mod delivery;
mod line;
#[cfg(test)]
mod tests;

use anyhow::{Context, Result, bail, ensure};
use delivery::{Journal, Outcome, Part, plan};
use line::Session;
use linebot_rust::{config::Settings, work_api::WorkApi};
use std::{env, fs, path::PathBuf, thread, time::Duration};

fn main() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] | ["--help"] | ["-h"] => {
            println!(
                "linebot_headless login <private-dir>\nlinebot_headless check <private-dir> <group-id> <room-name>\nlinebot_headless read <private-dir> <group-id> <room-name> <1-20>\nlinebot_headless once|run <private-dir> <group-id> <room-name> <test-queue.ini>"
            );
            Ok(())
        }
        ["login", dir] => {
            let _journal = Journal::open(&state_dir(dir)?.join("jobs.jsonl"))?;
            Session::qr(&state_dir(dir)?.join("session.json"))
        }
        ["check", dir, id, name] => {
            let _journal = Journal::open(&state_dir(dir)?.join("jobs.jsonl"))?;
            Session::open(&state_dir(dir)?.join("session.json"), id, name)?;
            println!("LINE 授權與指定群組已核對；未讀聊天、未發送。");
            Ok(())
        }
        ["read", dir, id, name, count] => {
            let n: i32 = count
                .parse()
                .ok()
                .filter(|n| (1..=20).contains(n))
                .context("筆數須為 1–20")?;
            let _journal = Journal::open(&state_dir(dir)?.join("jobs.jsonl"))?;
            let session = Session::open(&state_dir(dir)?.join("session.json"), id, name)?;
            let rows = session.read(n)?;
            println!("{}", serde_json::to_string(&rows)?);
            Ok(())
        }
        [mode @ ("once" | "run"), dir, id, name, ini] => {
            let config =
                fs::read_to_string(ini).map_err(|_| anyhow::anyhow!("無法讀取指定測試佇列設定"))?;
            let settings = Settings::parse_ini(&config)?;
            ensure!(
                settings.service_url.starts_with("https://"),
                "派工服務須使用 HTTPS"
            );
            let api = WorkApi::new(&settings).map_err(|_| anyhow::anyhow!("無法建立派工連線"))?;
            let dir = state_dir(dir)?;
            let mut journal = Journal::open(&dir.join("jobs.jsonl"))?;
            let session = Session::open(&dir.join("session.json"), id, name)?;
            loop {
                if dir.join("stop").is_file() {
                    break;
                }
                poll(&api, |part| session.send(part), name, &mut journal)?;
                if *mode == "once" {
                    break;
                }
                thread::sleep(Duration::from_secs(settings.poll_seconds));
            }
            Ok(())
        }
        _ => bail!("參數不符，請執行 --help"),
    }
}

fn state_dir(dir: &str) -> Result<PathBuf> {
    let path = PathBuf::from(dir);
    ensure!(
        path.is_absolute() && path.is_dir(),
        "請指定已建立的絕對路徑私人目錄，不要用共用或 Git 目錄"
    );
    Ok(path)
}

fn poll(
    api: &WorkApi,
    mut send: impl FnMut(Part) -> Result<()>,
    room: &str,
    journal: &mut Journal,
) -> Result<()> {
    let work = api
        .get_work()
        .map_err(|_| anyhow::anyhow!("取派工失敗（詳細回應未記錄）"))?;
    ensure!(work.status == "OK", "派工未回傳 OK");
    for job in work.data {
        ensure!(
            !job.id.is_empty()
                && job.id.len() <= 128
                && job
                    .id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')),
            "派工 ID 格式不符；未發送"
        );
        ensure!(
            job.room_name == room,
            "派工不是本機指定的測試群；未發送、未回報"
        );
        if journal.status(&job.id) == Some(Outcome::Pending) {
            bail!("工單 {} 發送結果不明；未重送，請先在群內核對", job.id);
        }
        if journal.status(&job.id) == Some(Outcome::Sent) {
            ack(api, &job.id, true, "200")?;
            continue;
        }
        let data = if job.has_file() {
            match api.download_file(&job.file_uuid) {
                Ok(bytes) => Some(bytes),
                Err(_) => {
                    ack(api, &job.id, false, "402")?;
                    continue;
                }
            }
        } else {
            None
        };
        let parts = match plan(&job, data) {
            Ok(parts) => parts,
            Err(code) => {
                ack(api, &job.id, false, code)?;
                continue;
            }
        };
        journal.record(&job.id, Outcome::Pending)?;
        for part in parts {
            if send(part).is_err() {
                bail!(
                    "工單 {} 可能未送、部分送出或已送；未回報成功、未重送，請核對後處理",
                    job.id
                );
            }
        }
        journal.record(&job.id, Outcome::Sent)?;
        ack(api, &job.id, true, "200")?;
    }
    Ok(())
}

fn ack(api: &WorkApi, id: &str, ok: bool, code: &str) -> Result<()> {
    api.update_work_status(id, ok, code)
        .map_err(|_| anyhow::anyhow!("工單 {id} 回報失敗；未自行重送訊息"))
}
