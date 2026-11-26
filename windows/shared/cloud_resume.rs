// Local and NAS kernels share the same HTTP and multi-file protocols. Keep validation and
// progress checks identical; callers choose and retain the target Api instance.
use crate::api::{Api, MultiFile, Task};
use serde_json::Value;

pub fn kind(task: &Task) -> Option<String> {
    let mut task = task.clone();
    task.tag_kind();
    let kind = task.kind?;
    ["阿里云盘", "夸克网盘", "UC 网盘", "百度网盘", "123 云盘", "天翼云盘", "移动云盘", "迅雷云盘", "光鸭云盘"]
        .contains(&kind.as_str()).then_some(kind)
}

fn check_task(task: &Task) -> Result<(), String> {
    let url = task.url();
    if !url.starts_with("shandian-multi:") && !url.starts_with("http://") && !url.starts_with("https://") { return Err("此任务不支持网盘换链续传".into()); }
    if task.status != "pause" && task.status != "error" { return Err("请先暂停原网盘任务再更新地址".into()); }
    Ok(())
}

fn single_name(task: &Task) -> Result<String, String> {
    let res = task.meta.as_ref().and_then(|m|m.res.as_ref()).ok_or("缺少原文件信息")?;
    if res.files.len() != 1 || res.files[0].name.is_empty() || res.size == 0 { return Err("原单文件信息不完整，进度已保留".into()); }
    Ok(res.files[0].name.clone())
}

fn counts(task: &Task, stats: &Value) -> Result<Vec<u64>, String> {
    if !task.url().starts_with("shandian-multi:") {
        let connections = stats["connections"].as_array().filter(|c|!c.is_empty()).ok_or("原任务分段记录不可用，进度已保留")?;
        let counts: Option<Vec<u64>> = connections.iter().map(|c|c["downloaded"].as_u64()).collect();
        let counts = counts.ok_or("原任务分段进度不可用，进度已保留")?;
        if task.downloaded() > counts.iter().sum::<u64>() { return Err("分段记录与原进度不一致，已停止续传并保留文件".into()); }
        return Ok(counts);
    }
    let files = stats["files"].as_array().ok_or("分段记录不可用，请更新内核；原任务和文件已保留")?;
    let expected = task.meta.as_ref().and_then(|m|m.req.as_ref()).and_then(|r|r.extra["files"].as_array()).ok_or("缺少原文件信息")?;
    if files.len() != expected.len() { return Err("分段记录与原文件数量不一致，原进度已保留".into()); }
    let counts: Option<Vec<u64>> = files.iter().map(|f|f["downloaded"].as_u64()).collect();
    let counts = counts.ok_or("分段进度不可用，原进度已保留")?;
    if task.downloaded() > counts.iter().sum::<u64>() { return Err("分段记录与原进度不一致，已停止续传并保留文件".into()); }
    Ok(counts)
}

pub async fn prepare(api: &Api, id: &str) -> Result<(String, Vec<Value>), String> {
    let list = api.list_tasks().await.map_err(|e|e.to_string())?;
    let task = list.iter().find(|t|t.id==id).ok_or("原任务不存在")?;
    check_task(task)?;
    let kind = kind(task).ok_or("此网盘暂不支持自动取链")?;
    let stats = api.stats(id).await.map_err(|e|e.to_string())?;
    counts(task, &stats)?;
    if !task.url().starts_with("shandian-multi:") {
        return Ok((kind, vec![serde_json::json!({"name":single_name(task)?,"size":task.total()})]));
    }
    let old = task.meta.as_ref().and_then(|m|m.req.as_ref()).and_then(|r|r.extra["files"].as_array()).ok_or("缺少原文件信息")?;
    let selected = task.meta.as_ref().and_then(|m|m.opts.as_ref()).map(|o|o.select_files.as_slice()).unwrap_or_default();
    let files: Vec<Value> = old.iter().enumerate()
        .filter(|(i,_)|selected.is_empty()||selected.contains(i))
        .filter(|(i,_)|stats["files"][*i]["status"].as_str()!=Some("done"))
        .map(|(i,f)|serde_json::json!({"name":f["name"],"size":stats["files"][i]["size"]})).collect();
    if files.is_empty() { return Err("没有需要更新地址的文件".into()); }
    Ok((kind,files))
}

pub async fn refresh(api: &Api, id: &str, files: &[MultiFile]) -> Result<(), String> {
    if files.is_empty() { return Err("没有收到新下载地址".into()); }
    let list = api.list_tasks().await.map_err(|e|e.to_string())?;
    let task = list.iter().find(|t|t.id==id).ok_or("原任务不存在")?;
    check_task(task)?;
    let before = api.stats(id).await.map_err(|e|e.to_string())?;
    let saved = counts(task,&before)?;
    if !task.url().starts_with("shandian-multi:") {
        return refresh_single(api, task, files, &saved).await;
    }
    api.refresh_multi_links(id,files).await.map_err(|e|e.to_string())?;
    // Old kernels may accept PATCH without implementing it. Read back URLs and
    // authentication headers before allowing the original task to resume.
    let list = api.list_tasks().await.map_err(|e|e.to_string())?;
    let task = list.iter().find(|t|t.id==id).ok_or("更新后原任务不存在")?;
    let updated = task.meta.as_ref().and_then(|m|m.req.as_ref()).and_then(|r|r.extra["files"].as_array());
    if !files.iter().all(|f|updated.is_some_and(|arr|arr.iter().any(|v|
        v["name"].as_str()==Some(f.name.as_str()) && v["url"].as_str()==Some(f.url.as_str()) &&
        f.headers.iter().all(|(key,value)|v["header"].as_object().is_some_and(|headers|headers.iter().any(|(k,v)|k.eq_ignore_ascii_case(key)&&v.as_str()==Some(value.as_str()))))))) {
        return Err("当前内核未完整更新地址或鉴权头，请更新内核；原任务和进度已保留".into());
    }
    let after = api.stats(id).await.map_err(|e|e.to_string())?;
    let current = counts(task,&after)?;
    if saved.iter().zip(&current).any(|(before,after)|after<before) { return Err("更新后分段进度发生变化，已暂停以保护原文件".into()); }
    api.resume(id).await.map_err(|e|e.to_string())
}

async fn refresh_single(api: &Api, original: &Task, files: &[MultiFile], saved: &[u64]) -> Result<(), String> {
    if files.len()!=1 || files[0].name!=single_name(original)? { return Err("新地址的文件名与原任务不匹配，进度已保留".into()); }
    let file=&files[0];
    let old=url::Url::parse(&original.url()).map_err(|_|"原下载地址无效")?;
    let fresh=url::Url::parse(&file.url).map_err(|_|"新下载地址无效")?;
    if !matches!(fresh.scheme(),"http"|"https") || fresh.host_str().is_none() || old.path()!=fresh.path() {
        return Err("新旧地址不是同一资源，已保留原进度".into());
    }
    let req=original.meta.as_ref().and_then(|m|m.req.as_ref()).ok_or("缺少原请求信息")?;
    let mut extra=req.extra.clone();
    if !extra.is_object() { extra=serde_json::json!({}); }
    let mut headers=extra["header"].as_object().cloned().unwrap_or_default();
    for (key,value) in &file.headers {
        headers.retain(|k,_|!k.eq_ignore_ascii_case(key));
        headers.insert(key.clone(),serde_json::json!(value));
    }
    extra["header"]=Value::Object(headers);
    // Probe on the target kernel (NAS links may be IP-bound), before changing
    // anything. A size mismatch or a server ignoring Range must never restart
    // a partially downloaded file from zero.
    let probe=api.probe_http(&file.url,&extra).await.map_err(|_|"新地址验证失败，原任务和分段进度已保留")?;
    let res=&probe["res"];
    if res["size"].as_u64()!=Some(original.total()) || res["range"].as_bool()!=Some(true) {
        return Err("新地址大小不一致或不支持断点续传，原进度已保留".into());
    }
    let opts=original.meta.as_ref().and_then(|m|m.opts.as_ref());
    let path=opts.map(|o|(&o.path,&o.name));
    api.refresh_http_link(&original.id,&file.url,&extra).await.map_err(|e|e.to_string())?;
    let list=api.list_tasks().await.map_err(|e|e.to_string())?;
    let task=list.iter().find(|t|t.id==original.id).ok_or("更新后原任务不存在")?;
    check_task(task)?;
    let updated=task.meta.as_ref().and_then(|m|m.req.as_ref()).ok_or("更新后请求信息不存在")?;
    let updated_opts=task.meta.as_ref().and_then(|m|m.opts.as_ref());
    let same_headers=extra["header"].as_object().is_some_and(|expected|expected.iter().all(|(key,value)|
        updated.extra["header"].as_object().is_some_and(|actual|actual.iter().any(|(k,v)|k.eq_ignore_ascii_case(key)&&v==value))));
    if updated.url!=file.url || !same_headers || ["method","body"].iter().any(|key|updated.extra[*key].as_str().unwrap_or("")!=extra[*key].as_str().unwrap_or("")) || task.total()!=original.total() || updated_opts.map(|o|(&o.path,&o.name))!=path {
        return Err("内核未完整更新地址和鉴权信息，原任务已保留，请更新内核".into());
    }
    let after=api.stats(&original.id).await.map_err(|e|e.to_string())?;
    if counts(task,&after)?!=saved || task.downloaded()<original.downloaded() {
        return Err("更新后分段进度发生变化，已停止续传以保护原文件".into());
    }
    api.resume(&original.id).await.map_err(|e|e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};

    fn exercise(failure: &str) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::<(String,Value)>::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let (log, stop, server_failure) = (requests.clone(),stopped.clone(),failure.to_string());
        let server = std::thread::spawn(move || {
            let mut task = serde_json::json!({"id":"remote-original","status":"pause","progress":{"downloaded":12},
                "meta":{"req":{"url":"shandian-multi:lessons","labels":{"shandian.source":"123 云盘"},"extra":{"files":[
                    {"name":"part.zip","url":"https://cdn.example/old","header":{"Cookie":"old-session"}},
                    {"name":"done.zip","url":"https://cdn.example/done","header":{}}]}},
                    "opts":{"path":"/downloads/lessons","selectFiles":[0,1]}}});
            let mut patched = false;
            while !stop.load(Ordering::Relaxed) {
                let (mut stream,_) = match listener.accept() { Ok(v)=>v, Err(_)=>{std::thread::sleep(std::time::Duration::from_millis(2));continue} };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
                let mut bytes = Vec::new(); let mut chunk = [0u8;4096];
                let (header_end,length) = loop {
                    let n=stream.read(&mut chunk).unwrap(); if n==0 {panic!("incomplete HTTP request")}; bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end)=bytes.windows(4).position(|v|v==b"\r\n\r\n") {
                        let head=String::from_utf8_lossy(&bytes[..end]);
                        let length=head.lines().find_map(|l|l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v|v.trim().parse::<usize>().ok())).unwrap_or(0);
                        break (end+4,length)
                    }
                };
                while bytes.len()<header_end+length {let n=stream.read(&mut chunk).unwrap();bytes.extend_from_slice(&chunk[..n]);}
                let header=String::from_utf8_lossy(&bytes[..header_end]);
                assert!(header.to_ascii_lowercase().contains("x-api-token: remote-test-token"));
                let request=header.lines().next().unwrap().to_string();
                let body:Value=serde_json::from_slice(&bytes[header_end..]).unwrap_or(Value::Null);
                log.lock().unwrap().push((request.clone(),body.clone()));
                let data = if request.starts_with("GET /api/v1/tasks ") {serde_json::json!([task.clone()])}
                    else if request.contains("/stats ") {
                        if server_failure=="old-stats" {Value::Null} else {serde_json::json!({"files":[
                            {"name":"part.zip","size":16,"downloaded":if patched&&server_failure=="lost-progress"{2}else{4},"status":"error"},
                            {"name":"done.zip","size":8,"downloaded":8,"status":"done"}]})}
                    } else if request.starts_with("PATCH ") {
                        assert_eq!(body["req"]["extra"]["files"].as_array().unwrap().len(),1);
                        if server_failure!="old-patch" {
                            task["meta"]["req"]["extra"]["files"][0] = body["req"]["extra"]["files"][0].clone();
                            if server_failure=="lost-header" {task["meta"]["req"]["extra"]["files"][0]["header"]=serde_json::json!({});}
                        }
                        patched=true;Value::Null
                    } else {assert!(request.starts_with("PUT /api/v1/tasks/remote-original/continue "));Value::Null};
                let response=serde_json::json!({"code":0,"data":data}).to_string();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).unwrap();
            }
        });
        let api=Api::new(port,"remote-test-token".into());
        let prepared=tauri::async_runtime::block_on(prepare(&api,"remote-original"));
        let result = if failure=="old-stats" {prepared.map(|_|())} else {
            let (kind,wanted)=prepared.unwrap();assert_eq!(kind,"123 云盘");assert_eq!(wanted.len(),1);assert_eq!(wanted[0]["name"],"part.zip");
            tauri::async_runtime::block_on(refresh(&api,"remote-original",&[MultiFile{name:"part.zip".into(),url:"https://cdn.example/new".into(),headers:vec![("Cookie".into(),"new-session".into())]}]))
        };
        stopped.store(true,Ordering::Relaxed);server.join().unwrap();
        let log=requests.lock().unwrap();
        assert_eq!(result.is_ok(),failure.is_empty(),"{failure}: {result:?}");
        assert_eq!(log.iter().filter(|(r,_)|r.contains("/continue ")).count(),if failure.is_empty(){1}else{0});
        assert!(!log.iter().any(|(r,_)|r.starts_with("DELETE")||r.starts_with("POST")),"refresh must not recreate or delete tasks");
    }
    #[test] fn remote_refresh_keeps_id_completed_files_and_authentication() {exercise("")}
    #[test] fn unsupported_remote_kernel_does_not_resume() {exercise("old-patch");exercise("old-stats")}
    #[test] fn lost_authentication_or_progress_does_not_resume() {exercise("lost-header");exercise("lost-progress")}

    fn exercise_single(failure: &str) {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();
        let port=listener.local_addr().unwrap().port();listener.set_nonblocking(true).unwrap();
        let stopped=Arc::new(AtomicBool::new(false));let stop=stopped.clone();
        let requests=Arc::new(Mutex::new(Vec::<String>::new()));let log=requests.clone();
        let mode=failure.to_string();
        let server=std::thread::spawn(move || {
            let mut task=serde_json::json!({"id":"remote-original","status":"error","name":"renamed.m2ts","progress":{"downloaded":31524812937u64},
                "meta":{"req":{"url":"https://old.example/video/00003.m2ts?old","extra":{"method":"GET","body":"","header":{"cookie":"old","Referer":"https://www.guangyapan.com/"}}},
                "opts":{"path":"/video","name":"renamed.m2ts"},"res":{"size":89524764672u64,"files":[{"name":"00003.m2ts","size":89524764672u64}]}}});
            let mut patched=false;
            while !stop.load(Ordering::Relaxed) {
                let (mut stream,_)=match listener.accept(){Ok(v)=>v,Err(_)=>{std::thread::sleep(std::time::Duration::from_millis(2));continue}};
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
                let mut bytes=Vec::new();let mut buf=[0u8;4096];
                let (end,len)=loop {
                    let n=stream.read(&mut buf).unwrap();assert!(n>0);bytes.extend_from_slice(&buf[..n]);
                    if let Some(end)=bytes.windows(4).position(|b|b==b"\r\n\r\n") {
                        let head=String::from_utf8_lossy(&bytes[..end]);
                        let len=head.lines().find_map(|l|l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v|v.trim().parse::<usize>().ok())).unwrap_or(0);
                        break (end+4,len)
                    }
                };
                while bytes.len()<end+len {let n=stream.read(&mut buf).unwrap();bytes.extend_from_slice(&buf[..n]);}
                let head=String::from_utf8_lossy(&bytes[..end]);let request=head.lines().next().unwrap().to_string();
                log.lock().unwrap().push(request.clone());
                let body:Value=serde_json::from_slice(&bytes[end..]).unwrap_or(Value::Null);
                let data=if request.starts_with("GET /api/v1/tasks "){serde_json::json!([task.clone()])}
                else if request.contains("/stats ") {
                    serde_json::json!({"connections":[{"downloaded":if patched&&mode=="lost-progress"{0}else{30000000000u64}},{"downloaded":1524812937u64}]})
                } else if request.starts_with("POST /api/v1/resolve?probe=1 ") {
                    assert_eq!(body["req"]["extra"]["header"]["Cookie"],"new");
                    assert!(body["req"]["extra"]["header"].get("cookie").is_none());
                    assert_eq!(body["req"]["extra"]["header"]["Referer"],"https://www.guangyapan.com/");
                    serde_json::json!({"res":{"size":if mode=="wrong-size"{1}else{89524764672u64},"range":mode!="no-range"}})
                } else if request.starts_with("PATCH /api/v1/tasks/remote-original ") {
                    assert!(body.get("opts").is_none());
                    if mode!="old-patch" {task["meta"]["req"]["url"]=body["req"]["url"].clone();task["meta"]["req"]["extra"]=body["req"]["extra"].clone();}
                    if mode=="lost-header" {task["meta"]["req"]["extra"]["header"]=serde_json::json!({});}
                    if mode=="changed-dir" {task["meta"]["opts"]["path"]=serde_json::json!("/wrong");}
                    patched=true;Value::Null
                } else {assert!(request.starts_with("PUT /api/v1/tasks/remote-original/continue "));Value::Null};
                let response=serde_json::json!({"code":0,"data":data}).to_string();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).unwrap();
            }
        });
        let api=Api::new(port,"test-token".into());
        let (kind,files)=tauri::async_runtime::block_on(prepare(&api,"remote-original")).unwrap();
        assert_eq!(kind,"光鸭云盘");assert_eq!(files[0]["name"],"00003.m2ts");assert_eq!(files[0]["size"],89524764672u64);
        let result=tauri::async_runtime::block_on(refresh(&api,"remote-original",&[MultiFile{
            name:if failure=="wrong-name"{"other.m2ts"}else{"00003.m2ts"}.into(),
            url:if failure=="wrong-resource"{"https://new.example/other/00003.m2ts"}else{"https://new.example/video/00003.m2ts?new"}.into(),headers:vec![("Cookie".into(),"new".into())]}]));
        stopped.store(true,Ordering::Relaxed);server.join().unwrap();
        let log=requests.lock().unwrap();
        assert_eq!(result.is_ok(),failure.is_empty(),"{failure}: {result:?}");
        assert_eq!(log.iter().filter(|r|r.contains("/continue ")).count(),usize::from(failure.is_empty()));
        assert!(!log.iter().any(|r|r.starts_with("DELETE")||r.starts_with("POST /api/v1/tasks")));
        if ["wrong-name","wrong-resource","wrong-size","no-range"].contains(&failure) {assert!(!log.iter().any(|r|r.starts_with("PATCH")));}
    }
    #[test] fn single_http_refresh_preserves_30gb_segments_and_headers() {exercise_single("")}
    #[test] fn single_http_rejects_mismatch_before_patch() {for mode in ["wrong-name","wrong-resource","wrong-size","no-range"]{exercise_single(mode)}}
    #[test] fn single_http_requires_preserved_progress_and_readback() {for mode in ["old-patch","lost-header","lost-progress","changed-dir"]{exercise_single(mode)}}
}
