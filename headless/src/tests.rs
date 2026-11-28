#[test]
fn plans_existing_text_and_picture_contract() {
    let job: linebot_rust::models::WorkItem = serde_json::from_str(
        r#"{"id":"1003","room_name":"test","message":"Hello","file_uuid":"pic","file_subname":"png"}"#,
    ).unwrap();
    let image = b"\x89PNG\r\n\x1a\n".to_vec();
    let parts = super::plan(&job, Some(image.clone())).unwrap();
    assert_eq!(
        parts,
        vec![
            crate::delivery::Part::Image(image, "image.png".into()),
            crate::delivery::Part::Text("Hello".into())
        ]
    );
}

#[test]
fn bad_image_is_rejected_before_any_text_is_sent() {
    let job: linebot_rust::models::WorkItem = serde_json::from_str(
        r#"{"id":"1003","room_name":"test","message":"Hello","file_uuid":"pic","file_subname":"png"}"#,
    ).unwrap();
    assert_eq!(
        super::plan(&job, Some(b"not a PNG".to_vec())).unwrap_err(),
        "402"
    );
}

#[test]
fn journal_never_resends_a_pending_or_delivered_job() {
    let root = std::env::temp_dir().join(format!("linebot-headless-test-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let p = root.join("journal");
    let mut journal = super::Journal::open(&p).unwrap();
    journal.record("1003", super::Outcome::Pending).unwrap();
    journal.record("1004", super::Outcome::Sent).unwrap();
    drop(journal);
    let journal = super::Journal::open(&p).unwrap();
    assert_eq!(journal.status("1003"), Some(super::Outcome::Pending));
    assert_eq!(journal.status("1004"), Some(super::Outcome::Sent));
    drop(journal);
    std::fs::remove_file(&p).unwrap();
    std::fs::remove_dir(&root).unwrap();
}

#[test]
fn legacy_dispatcher_contract_with_local_http_and_no_line() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
        time::Duration,
    };

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        let expected = [
            "getWork",
            "getFile",
            "updateWorkStatus",
            "getWork",
            "updateWorkStatus",
        ];
        for (i, mode) in expected.iter().enumerate() {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut temp = [0; 512];
            while !request.ends_with(b"\r\n\r\n") {
                let count = stream.read(&mut temp).unwrap();
                assert!(count > 0 && request.len() < 8192);
                request.extend(&temp[..count]);
            }
            let request = std::str::from_utf8(&request).unwrap();
            let first = request.lines().next().unwrap();
            assert!(first.contains(&format!("mode={mode}")));
            let (status, body): (_, &[u8]) = match i {
                0 | 3 => ("200 OK", br#"{"status":"OK","data":[{"id":"42","room_name":"test","message":"Hello","file_uuid":"pic","file_subname":"png"}]}"#),
                1 => {
                    assert!(first.contains("file_uuid=pic"));
                    ("200 OK", b"\x89PNG\r\n\x1a\n")
                },
                2 | 4 => {
                    assert!(first.contains("id=42") && first.contains("is_ok=1") && first.contains("error_code=200"));
                    (if i == 2 { "503 Service Unavailable" } else { "200 OK" }, b"{}")
                },
                _ => unreachable!(),
            };
            write!(
                stream,
                "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        }
    });

    let settings = linebot_rust::config::Settings::parse_ini(&format!(
        "[Settings]\nSERVICE_URL={origin}\nBOT_TOKEN=synthetic-only\n"
    ))
    .unwrap();
    let api = linebot_rust::work_api::WorkApi::new(&settings).unwrap();
    let root =
        std::env::temp_dir().join(format!("linebot-headless-contract-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let p = root.join("jobs.jsonl");
    let mut journal = super::Journal::open(&p).unwrap();
    let mut sent = Vec::new();
    let result = super::poll(
        &api,
        |part| {
            sent.push(part);
            Ok(())
        },
        "test",
        &mut journal,
    );
    assert!(result.is_err());
    assert_eq!(sent.len(), 2);
    assert_eq!(journal.status("42"), Some(super::Outcome::Sent));
    drop(journal);
    let mut journal = super::Journal::open(&p).unwrap();
    super::poll(&api, |_| panic!("已送成功不得重送"), "test", &mut journal).unwrap();
    worker.join().unwrap();
    drop(journal);
    std::fs::remove_file(&p).unwrap();
    std::fs::remove_dir(&root).unwrap();
}
