#[cfg(test)]
mod renewal_contract {
    use crate::api::Api;
    use crate::cloud::jobs::renew;
    use serde_json::Value;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };
    fn exercise_single(failure: &str) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = requests.clone();
        let mode = failure.to_string();
        let task_id = if failure == "user-pause" {
            "cancelled-original"
        } else {
            "remote-original"
        };
        let server = std::thread::spawn(move || {
            let mut task = serde_json::json!({"id":task_id,"status":"error","name":"renamed.m2ts","progress":{"downloaded":31524812937u64},
                "meta":{"req":{"url":"https://old.example/video/00003.m2ts?old","extra":{"method":"GET","body":"","header":{"cookie":"old","Referer":"https://www.guangyapan.com/"}}},
                "opts":{"path":"/video","name":"renamed.m2ts"},"res":{"size":89524764672u64,"files":[{"name":"00003.m2ts","size":89524764672u64}]}}});
            let mut patched = false;
            while !stop.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(v) => v,
                    Err(_) => {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                        continue;
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0u8; 4096];
                let (end, len) = loop {
                    let n = stream.read(&mut buf).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&bytes[..end]);
                        let len = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        break (end + 4, len);
                    }
                };
                while bytes.len() < end + len {
                    let n = stream.read(&mut buf).unwrap();
                    bytes.extend_from_slice(&buf[..n]);
                }
                let head = String::from_utf8_lossy(&bytes[..end]);
                let request = head.lines().next().unwrap().to_string();
                log.lock().unwrap().push(request.clone());
                let body: Value = serde_json::from_slice(&bytes[end..]).unwrap_or(Value::Null);
                let data = if request.starts_with("GET /api/v1/tasks ") {
                    serde_json::json!([task.clone()])
                } else if request.contains("/stats ") {
                    serde_json::json!({"connections":[{"downloaded":if patched&&mode=="lost-progress"{0}else{30000000000u64}},{"downloaded":1524812937u64}]})
                } else if request.starts_with("POST /api/v1/resolve?probe=1 ") {
                    assert_eq!(body["req"]["extra"]["header"]["Cookie"], "new");
                    assert!(body["req"]["extra"]["header"].get("cookie").is_none());
                    assert_eq!(
                        body["req"]["extra"]["header"]["Referer"],
                        "https://www.guangyapan.com/"
                    );
                    if mode == "user-pause" {
                        drop(tauri::async_runtime::block_on(
                            crate::cloud::user_task_control(task_id),
                        ));
                        task["status"] = serde_json::json!("pause");
                    }
                    serde_json::json!({"res":{"size":if mode=="wrong-size"{1}else{89524764672u64},"range":mode!="no-range"}})
                } else if request.starts_with(&format!("PATCH /api/v1/tasks/{task_id} ")) {
                    assert!(body.get("opts").is_none());
                    if mode != "old-patch" {
                        task["meta"]["req"]["url"] = body["req"]["url"].clone();
                        task["meta"]["req"]["extra"] = body["req"]["extra"].clone();
                    }
                    if mode == "lost-header" {
                        task["meta"]["req"]["extra"]["header"] = serde_json::json!({});
                    }
                    if mode == "changed-dir" {
                        task["meta"]["opts"]["path"] = serde_json::json!("/wrong");
                    }
                    patched = true;
                    Value::Null
                } else {
                    assert!(request.starts_with(&format!("PUT /api/v1/tasks/{task_id}/continue ")));
                    Value::Null
                };
                let response = serde_json::json!({"code":0,"data":data}).to_string();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).unwrap();
            }
        });
        let api = Api::new(port, "test-token".into());
        let task = tauri::async_runtime::block_on(api.list_tasks())
            .unwrap()
            .remove(0);
        let result = tauri::async_runtime::block_on(renew(
            &api,
            &task,
            "https://new.example/changed-cdn-path?new",
            &[
                ("Cookie".into(), "new".into()),
                ("Referer".into(), "https://www.guangyapan.com/".into()),
            ],
            89524764672u64,
        ));
        stopped.store(true, Ordering::Relaxed);
        server.join().unwrap();
        let log = requests.lock().unwrap();
        assert_eq!(result.is_ok(), failure.is_empty(), "{failure}: {result:?}");
        assert_eq!(
            log.iter().filter(|r| r.contains("/continue ")).count(),
            usize::from(failure.is_empty())
        );
        assert!(!log
            .iter()
            .any(|r| r.starts_with("DELETE") || r.starts_with("POST /api/v1/tasks")));
        if ["wrong-size", "no-range"].contains(&failure) {
            assert!(!log.iter().any(|r| r.starts_with("PATCH")));
        }
    }
    #[test]
    fn sdk_bound_file_keeps_same_task_and_30gb_segments() {
        exercise_single("")
    }
    #[test]
    fn incompatible_link_or_kernel_never_resumes() {
        for mode in [
            "user-pause",
            "wrong-size",
            "no-range",
            "old-patch",
            "lost-header",
            "lost-progress",
            "changed-dir",
        ] {
            exercise_single(mode)
        }
    }
}
