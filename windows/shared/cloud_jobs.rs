use super::*;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Pull {
    pub remote_id: String,
    pub path: String,
    pub name: String,
    pub size: u64,
    pub local_id: String,
    pub intent: bool,
    pub attempts: u32,
    pub next: u64,
    pub downloaded: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub provider: String,
    pub origin: String,
    pub url: String,
    pub name: String,
    pub dir: String,
    pub remote_path: String,
    pub remote_id: String,
    pub state: String,
    pub message: String,
    pub size: u64,
    pub progress: u64,
    pub created: u64,
    pub updated: u64,
    pub revision: u64,
    pub submitted: bool,
    pub pulls: Vec<Pull>,
    pub original: Option<Task>,
}

fn persist(c: &Cloud, job: &Job) -> Result<(), String> {
    let mut list = c.entries.lock().unwrap();
    if let Some(old) = list.iter_mut().find(|j| j.id == job.id) {
        *old = job.clone()
    } else {
        list.push(job.clone())
    }
    sdk::private_write(&c.root.join("jobs.json"), &*list)
}
fn current(c: &Cloud, job: &Job) -> bool {
    c.entries.lock().unwrap().iter().any(|j| {
        j.id == job.id && j.revision == job.revision && j.state != "pause" && j.state != "cancelled"
    })
}
fn commit(c: &Cloud, job: &Job) -> Result<bool, String> {
    let mut list = c.entries.lock().unwrap();
    let Some(old) = list.iter_mut().find(|j| {
        j.id == job.id && j.revision == job.revision && j.state != "pause" && j.state != "cancelled"
    }) else {
        return Ok(false);
    };
    *old = job.clone();
    sdk::private_write(&c.root.join("jobs.json"), &*list)?;
    Ok(true)
}
// Even if the user paused while the provider was accepting the intent, retain
// the returned identity. A later resume must never create a second cloud task.
fn record_submission(c: &Cloud, result: &Job) -> Result<bool, String> {
    let mut list = c.entries.lock().unwrap();
    let Some(job) = list.iter_mut().find(|j| j.id == result.id) else {
        return Ok(false);
    };
    job.submitted = true;
    job.remote_id = result.remote_id.clone();
    let active =
        job.revision == result.revision && !matches!(job.state.as_str(), "pause" | "cancelled");
    if active {
        job.state = result.state.clone();
        job.message = result.message.clone()
    } else if result.remote_id.is_empty() {
        job.message = "提交结果待确认，请到网盘核对；不会自动重复提交".into();
    }
    sdk::private_write(&c.root.join("jobs.json"), &*list)?;
    Ok(active)
}
fn request(p: &Provider, j: &Job) -> Value {
    json!({"tool":p.offline_tool,"path":j.remote_path,"remote_id":j.remote_id})
}

pub async fn submit(
    c: &'static Cloud,
    api: Api,
    task: Task,
    provider_id: &str,
    url: &str,
) -> Result<Value, String> {
    let _g = c.control.lock().await;
    if c.entries.lock().unwrap().iter().any(|j| {
        j.provider == provider_id && (j.submitted || !matches!(j.state.as_str(), "cancelled" | "error"))
            && (j.origin == task.id || j.url.trim() == url.trim()
                || (url.to_ascii_lowercase().starts_with("magnet:")
                    && crate::links::canonical_url(&j.url) == crate::links::canonical_url(url)))
    }) {
        return Ok(json!({"alreadySubmitted":true,"message":"这个资源已交给该网盘"}));
    }
    if let Some(j) = c
        .entries
        .lock()
        .unwrap()
        .iter()
        .find(|j| j.origin == task.id && !matches!(j.state.as_str(), "cancelled" | "done"))
    {
        return Ok(json!({"id":format!("cloud:{}",j.id),"message":"这条任务已经交给云盘"}));
    }
    let id = format!("{:032x}", rand::random::<u128>());
    let name = task
        .name
        .clone()
        .filter(|n| !n.is_empty())
        .or_else(|| {
            task.meta
                .as_ref()
                .and_then(|m| m.res.as_ref())
                .map(|r| r.name.clone())
        })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "云盘任务".into());
    let dir = task
        .saved_dir()
        .ok_or("原任务保存目录不可用，已保留任务")?
        .to_string_lossy()
        .into_owned();
    let job = Job {
        id: id.clone(),
        provider: provider_id.into(),
        origin: task.id.clone(),
        url: url.into(),
        name,
        dir,
        remote_path: format!("/cloud/{provider_id}/Shandian/{id}"),
        state: "queued".into(),
        size: task.total(),
        original: Some(task),
        created: now(),
        updated: now(),
        ..Default::default()
    };
    persist(c, &job)?;
    drop(_g);
    if !c.stepping.swap(true, Ordering::SeqCst) {
        tauri::async_runtime::spawn(async move {
            step(c, &api).await;
            c.stepping.store(false, Ordering::SeqCst);
        });
    }
    Ok(json!({"id":format!("cloud:{id}"),"message":"已加入云盘队列，原任务和已有文件已保留"}))
}

async fn files(client: &sdk::Sdk, p: &Provider, j: &Job) -> Result<Vec<Pull>, String> {
    let mut stack = vec![j.remote_path.clone()];
    let mut out = Vec::new();
    let mut visited = std::collections::HashSet::new();
    while let Some(path) = stack.pop() {
        if !visited.insert(path.clone()) {
            continue;
        }
        if visited.len() > 10000 {
            return Err("云端目录过多，请在网盘检查此任务".into());
        }
        let data = client
            .call(
                reqwest::Method::POST,
                "/api/fs/native_offline/files",
                Some(&json!({"tool":p.offline_tool,"path":path})),
            )
            .await?;
        let entries = data
            .as_array()
            .or_else(|| data["files"].as_array())
            .ok_or("云端文件列表不完整")?;
        for f in entries {
            let name = f["name"].as_str().unwrap_or("");
            if !safe_component(name) {
                return Err("云端返回了无效文件名，原任务已保留".into());
            }
            let full = format!("{}/{}", path.trim_end_matches('/'), name);
            if f["is_dir"].as_bool() == Some(true) {
                stack.push(full)
            } else {
                let id = f["id"]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .ok_or("云端没有返回稳定文件标识，已保留原任务")?;
                out.push(Pull {
                    remote_id: id.into(),
                    path: full,
                    name: name.into(),
                    size: f["size"].as_u64().ok_or("云端文件大小未知")?,
                    ..Default::default()
                })
            }
        }
    }
    if out.is_empty() {
        return Err("云端已完成但未找到可拉回文件，原任务已保留".into());
    }
    if let Some(task) = j.original.as_ref() {
        if let (Some(res), Some(opts)) = (
            task.meta.as_ref().and_then(|m| m.res.as_ref()),
            task.meta.as_ref().and_then(|m| m.opts.as_ref()),
        ) {
            if !opts.select_files.is_empty() {
                let mut selected = Vec::new();
                for i in &opts.select_files {
                    let wanted = res
                        .files
                        .get(*i)
                        .ok_or("原任务文件选择记录不完整，已有进度已保留")?;
                    let relative = if wanted.path.is_empty() {
                        wanted.name.clone()
                    } else {
                        format!("{}/{}", wanted.path.trim_matches('/'), wanted.name)
                    };
                    let matches: Vec<_> = out
                        .iter()
                        .filter(|f| {
                            f.size == wanted.size
                                && (f.path.ends_with(&format!("/{relative}"))
                                    || f.name == wanted.name)
                        })
                        .collect();
                    if matches.len() != 1 {
                        return Err("云端文件不能与原任务所选文件唯一对应，原任务已保留".into());
                    }
                    selected.push(matches[0].clone());
                }
                out = selected;
            }
        }
    }
    Ok(out)
}
fn safe_component(n: &str) -> bool {
    !n.is_empty() && n != "." && n != ".." && !n.contains(['/', '\\', ':', '\0'])
}
async fn link(
    client: &sdk::Sdk,
    p: &Provider,
    f: &Pull,
) -> Result<(String, Vec<(String, String)>), String> {
    let v=client.call(reqwest::Method::POST,"/api/fs/native_offline/link",Some(&json!({"tool":p.offline_tool,"path":f.path,"remote_file_id":f.remote_id,"expected_size":f.size}))).await?;
    if v["id"].as_str() != Some(f.remote_id.as_str()) || v["size"].as_u64() != Some(f.size) {
        return Err("云端文件身份或大小发生变化，已有进度已保留".into());
    }
    let url = v["url"]
        .as_str()
        .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
        .ok_or("网盘未返回有效下载地址")?
        .to_string();
    let headers = v["headers"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.into())))
                .collect()
        })
        .unwrap_or_default();
    Ok((url, headers))
}

fn counts(v: &Value) -> Option<Vec<u64>> {
    let c = v["connections"].as_array()?;
    if c.is_empty() {
        return None;
    }
    c.iter().map(|v| v["downloaded"].as_u64()).collect()
}
pub(super) async fn renew(
    api: &Api,
    t: &Task,
    url: &str,
    headers: &[(String, String)],
    size: u64,
) -> Result<(), String> {
    let control = task_control(&t.id);
    let epoch = control.epoch.load(Ordering::SeqCst);
    if t.status != "error" {
        return Err("任务状态已经变化，已保留进度".into());
    }
    let before = api
        .stats(&t.id)
        .await
        .map_err(|e| sdk::safe_error(&e.to_string()))?;
    let saved = counts(&before).ok_or("原任务续传分段不可用，已有进度已保留")?;
    if saved.iter().sum::<u64>() < t.downloaded() {
        return Err("原任务分段与进度不一致，已保留文件".into());
    }
    let mut extra = t
        .meta
        .as_ref()
        .and_then(|m| m.req.as_ref())
        .map(|r| r.extra.clone())
        .unwrap_or_else(|| json!({}));
    if !extra.is_object() {
        extra = json!({})
    }
    let map: serde_json::Map<String, Value> =
        headers.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    extra["header"] = json!(map);
    let probe = api
        .probe_http(url, &extra)
        .await
        .map_err(|e| sdk::safe_error(&e.to_string()))?;
    if probe["res"]["size"].as_u64() != Some(size) || probe["res"]["range"].as_bool() != Some(true)
    {
        return Err("新链接大小或断点续传校验失败，已有进度已保留".into());
    }
    let _commit = control.gate.lock().await;
    if control.epoch.load(Ordering::SeqCst) != epoch {
        return Err("用户已操作此任务，自动续传已停止，已有进度保留".into());
    }
    let actual = api
        .list_tasks()
        .await
        .map_err(|e| sdk::safe_error(&e.to_string()))?
        .into_iter()
        .find(|v| v.id == t.id)
        .ok_or("原任务已移除，自动续传已停止")?;
    if actual.status != "error" || actual.url() != t.url() {
        return Err("任务状态已变化，自动续传已停止".into());
    }
    api.refresh_http_link(&t.id, url, &extra)
        .await
        .map_err(|e| sdk::safe_error(&e.to_string()))?;
    let fresh = api
        .list_tasks()
        .await
        .map_err(|e| sdk::safe_error(&e.to_string()))?
        .into_iter()
        .find(|v| v.id == t.id)
        .ok_or("原任务暂时不可用")?;
    let after = api
        .stats(&t.id)
        .await
        .map_err(|e| sdk::safe_error(&e.to_string()))?;
    let retained = counts(&after).ok_or("更新后续传分段不可用，已保持暂停")?;
    if fresh.url() != url
        || fresh.total() != size
        || fresh.downloaded() < t.downloaded()
        || fresh
            .meta
            .as_ref()
            .and_then(|m| m.opts.as_ref())
            .map(|o| (&o.path, &o.name))
            != t.meta
                .as_ref()
                .and_then(|m| m.opts.as_ref())
                .map(|o| (&o.path, &o.name))
        || retained.len() != saved.len()
        || saved.iter().zip(retained).any(|(a, b)| b < *a)
        || !headers.iter().all(|(k, v)| {
            fresh
                .meta
                .as_ref()
                .and_then(|m| m.req.as_ref())
                .is_some_and(|r| {
                    r.extra["header"].as_object().is_some_and(|m| {
                        m.iter().any(|(key, value)| {
                            key.eq_ignore_ascii_case(k) && value.as_str() == Some(v.as_str())
                        })
                    })
                })
        })
    {
        return Err("内核更新校验失败，原任务和进度已保留".into());
    }
    api.resume(&t.id)
        .await
        .map_err(|e| sdk::safe_error(&e.to_string()))
}
fn expired(t: &Task) -> bool {
    t.status == "error"
        && t.shandian_error.as_deref().is_some_and(|e| {
            ["401", "403", "410"]
                .iter()
                .any(|n| e.contains(&format!("status={n}")) || e.contains(&format!("HTTP {n}")))
        })
}

pub async fn step(c: &Cloud, api: &Api) {
    let list = c.entries.lock().unwrap().clone();
    if !list.iter().any(|j| {
        !matches!(
            j.state.as_str(),
            "pause" | "cancelled" | "done" | "uncertain" | "error"
        )
    }) {
        return;
    }
    let client = match c.sdk().await {
        Ok(s) => s,
        Err(_) => return,
    };
    let tasks = match api.list_tasks().await {
        Ok(t) => t,
        Err(_) => return,
    };
    for mut j in list {
        if !current(c, &j) || matches!(j.state.as_str(), "uncertain" | "error" | "done") {
            continue;
        }
        let Ok(p) = provider(&j.provider) else {
            continue;
        };
        let result:Result<(),String>=async {
            match j.state.as_str(){
                "queued"=>{j.state="submitting".into();j.submitted=true;j.message="正在提交云端…".into();if !commit(c,&j)?{return Ok(())}let mut body=request(&p,&j);body["url"]=json!(j.url);body["request_id"]=json!(j.id);
                    let submitted=client.call(reqwest::Method::POST,"/api/fs/native_offline/submit",Some(&body)).await;
                    match submitted {Ok(v) if v["remote_id"].as_str().is_some_and(|s|!s.is_empty())=>{j.remote_id=v["remote_id"].as_str().unwrap().into();j.state="running".into();j.message="云端下载中".into();let _guard=c.control.lock().await;let active=record_submission(c,&j)?;if active&&(j.origin.starts_with("xl:")||tasks.iter().any(|t|t.id==j.origin&&matches!(t.status.as_str(),"running"|"ready"|"wait"))){pause_origin(api,&j.origin).await.map_err(|_|"云端已接收，原任务暂停失败，请检查原任务")?;}},_=>{j.state="uncertain".into();j.message="提交结果待确认，请到网盘核对；不会自动重复提交，原任务与文件已保留".into();record_submission(c,&j)?;}}
                },
                "submitting"=>{j.state="uncertain".into();j.message="上次提交被中断，请到网盘核对；不会重复提交".into();commit(c,&j)?;},
                "running"=>{let v=client.call(reqwest::Method::POST,"/api/fs/native_offline/status",Some(&request(&p,&j))).await?;if !v["known"].as_bool().unwrap_or(false){j.message="暂未查到云端状态，保留关联并等待后续查询".into();commit(c,&j)?;return Ok(())}j.progress=v["progress"].as_u64().unwrap_or(0).min(100);if let Some(size)=v["total_bytes"].as_u64().filter(|n|*n>0){j.size=size}if v["partial"].as_bool()==Some(true){return Err("云端仅部分完成，请在网盘检查资源；原任务已保留".into())}if let Some(e)=v["error"].as_str().filter(|e|!e.is_empty()){return Err(sdk::safe_error(e))}if v["completed"].as_bool()==Some(true){j.pulls=files(&client,&p,&j).await?;j.state="pulling".into();j.message="正在拉回文件…".into();}commit(c,&j)?;},
                "pulling"=>{let mut done=true;for i in 0..j.pulls.len(){if !current(c,&j){return Ok(())}let f=j.pulls[i].clone();if f.local_id.is_empty(){done=false;if f.intent {return Err("拉回任务创建结果待确认，已停止自动创建以保护已有文件".into())}
                            let (url,headers)=link(&client,&p,&f).await?;let relative=f.path.strip_prefix(&format!("{}/",j.remote_path)).ok_or("云端文件路径不匹配")?;let path=PathBuf::from(&j.dir).join(format!("{}-云盘-{}",crate::wechat::safe_name(&j.name),&j.id[..8])).join(relative.replace('/',"\\"));let parent=path.parent().ok_or("保存目录不可用")?;
                            if path.exists(){return Err("拉回目录已有同名文件，已停止以保护已有内容".into())}std::fs::create_dir_all(parent).map_err(|_|"无法创建拉回目录")?;
                            let _guard=c.control.lock().await;if !current(c,&j){return Ok(())}j.pulls[i].intent=true;commit(c,&j)?;let id=api.add_task(&url,&parent.to_string_lossy(),&f.name,&headers,&[]).await.map_err(|_|"拉回任务创建结果待确认，请检查任务列表")?;j.pulls[i].local_id=id;commit(c,&j)?;
                        }else{let Some(t)=tasks.iter().find(|t|t.id==f.local_id)else{return Err("拉回任务已不在列表中，原文件保持不动".into())};if t.status!="done"{done=false}if expired(t)&&now()>=f.next {let (url,headers)=link(&client,&p,&f).await?;let _guard=c.control.lock().await;if !current(c,&j){return Ok(())}if f.attempts>=5&&t.downloaded()<=f.downloaded{return Err("连续换链仍未恢复，请重新关联账号；已有进度已保留".into())}let outcome=renew(api,t,&url,&headers,f.size).await;j.pulls[i].attempts=if t.downloaded()>f.downloaded{1}else{f.attempts+1};j.pulls[i].downloaded=t.downloaded();j.pulls[i].next=now()+30*(1u64<<j.pulls[i].attempts.min(4));commit(c,&j)?;outcome?;}
                        }}if done{j.state="done".into();j.message="文件已拉回，云端文件与原任务均保留".into()}commit(c,&j)?;},_=>{}
            }Ok(())
        }.await;
        if let Err(e) = result {
            j.message = sdk::safe_error(&e);
            if j.state != "pulling" {
                j.state = "error".into()
            }
            let _ = commit(c, &j);
        }
    }
}

pub fn views(c: &Cloud, key: Option<&str>) -> Vec<Task> {
    c.entries.lock().unwrap().iter().filter(|j|!matches!(j.state.as_str(),"cancelled"|"done")&&!(j.state=="pulling"&&j.pulls.iter().all(|p|!p.local_id.is_empty()))).filter(|j|key.is_none_or(|k|crate::links::canonical_url(&j.url)==k)).filter_map(|j|{let failed=matches!(j.state.as_str(),"error"|"uncertain");let mut t:Task=serde_json::from_value(json!({"id":format!("cloud:{}",j.id),"name":j.name,"protocol":"cloud","status":if failed{"error"}else if j.state=="pause"{"pause"}else{"running"},"progress":{"downloaded":j.size.saturating_mul(j.progress)/100,"speed":0},"meta":{"req":{"url":j.url},"res":{"name":j.name,"size":j.size}},"shandianError":if failed{j.message.as_str()}else{""}})).ok()?;t.kind=Some(format!("{} · 云端离线",provider(&j.provider).map(|p|p.name).unwrap_or_default()));t.created=Some(j.created*1000);t.stage=Some(j.message.clone());Some(t)}).collect()
}
pub fn hides(c: &Cloud, id: &str) -> bool {
    c.entries
        .lock()
        .unwrap()
        .iter()
        .any(|j| j.origin == id && !matches!(j.state.as_str(), "cancelled" | "error" | "uncertain"))
}
pub async fn control(c: &Cloud, api: &Api, id: &str, action: &str) -> Result<(), String> {
    let id = id.trim_start_matches("cloud:");
    let _guard = c.control.lock().await;
    let mut job = {
        let mut list = c.entries.lock().unwrap();
        let job = list
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or("云盘任务不存在")?;
        match action {
            "pause" => job.state = "pause".into(),
            "resume" => {
                if job.state == "uncertain"
                    || job.state == "submitting"
                    || (job.submitted && job.remote_id.is_empty())
                {
                    return Err("提交结果待确认，请先到网盘核对，不会重复提交".into());
                }
                job.state = if !job.pulls.is_empty() {
                    "pulling"
                } else if !job.remote_id.is_empty() {
                    "running"
                } else {
                    "queued"
                }
                .into()
            }
            "delete" => job.state = "cancelled".into(),
            _ => return Err("不支持此操作".into()),
        }
        job.revision += 1;
        let cloned = job.clone();
        sdk::private_write(&c.root.join("jobs.json"), &*list)?;
        cloned
    };
    for f in &job.pulls {
        if f.local_id.is_empty() {
            continue;
        }
        if action == "resume" {
            let _ = api.resume(&f.local_id).await;
        } else {
            let _ = api.pause(&f.local_id).await;
        }
    }
    if action == "delete" {
        job.message = "已停止跟踪，原任务、拉回文件和云端文件已保留".into();
        persist(c, &job)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn traversal_rejected() {
        for n in ["..", "a/b", "a\\b", "x:y", ""] {
            assert!(!safe_component(n))
        }
        assert!(safe_component("电影.mkv"));
    }
    #[test]
    fn only_actual_expiry_triggers_renew() {
        let task = |status: &str, e: &str| {
            serde_json::from_value::<Task>(json!({"id":"x","status":status,"shandianError":e}))
                .unwrap()
        };
        assert!(expired(&task("error", "connection 0 failed: status=403")));
        assert!(!expired(&task(
            "running",
            "connection 0 failed: status=403"
        )));
        assert!(!expired(&task("error", "disk is full")));
    }
}

#[path = "cloud_jobs_tests.rs"]
mod contract_tests;

#[cfg(test)]
mod intent_tests {
    use super::*;
    fn cloud(root: PathBuf, job: Job) -> Cloud {
        Cloud {
            root,
            runtime: tokio::sync::Mutex::new(None),
            accounts: Mutex::new(HashMap::new()),
            account_signals: Mutex::new(HashMap::new()),
            account_gate: tokio::sync::Mutex::new(()),
            entries: Mutex::new(vec![job]),
            cache: tokio::sync::Mutex::new(Cache {
                at: 0,
                identity: String::new(),
                sources: vec![],
            }),
            preloaded_limits: Mutex::new(HashMap::new()),
            stepping: AtomicBool::new(false),
            next: AtomicU64::new(0),
            control: tokio::sync::Mutex::new(()),
        }
    }
    fn cleanup(root: &Path) {
        let path = root.canonicalize().unwrap();
        assert!(path.starts_with(std::env::temp_dir().canonicalize().unwrap()));
        std::fs::remove_dir_all(path).unwrap()
    }
    #[test]
    fn accepted_identity_survives_pause_during_submission() {
        let root = std::env::temp_dir().join(format!("sd-intent-{:016x}", rand::random::<u64>()));
        let c = cloud(
            root.clone(),
            Job {
                id: "one".into(),
                state: "pause".into(),
                revision: 1,
                submitted: true,
                ..Default::default()
            },
        );
        let response = Job {
            id: "one".into(),
            state: "running".into(),
            remote_id: "accepted-once".into(),
            submitted: true,
            ..Default::default()
        };
        assert!(!record_submission(&c, &response).unwrap());
        let saved: Vec<Job> = read_json(&root.join("jobs.json"));
        assert_eq!(saved[0].remote_id, "accepted-once");
        assert_eq!(saved[0].state, "pause");
        assert_eq!(saved[0].revision, 1);
        cleanup(&root);
    }
    #[test]
    fn uncertain_paused_intent_cannot_be_resubmitted() {
        let root = std::env::temp_dir().join(format!("sd-intent-{:016x}", rand::random::<u64>()));
        let c = cloud(
            root.clone(),
            Job {
                id: "one".into(),
                state: "pause".into(),
                submitted: true,
                ..Default::default()
            },
        );
        let result = tauri::async_runtime::block_on(control(
            &c,
            &Api::new(1, "test".into()),
            "cloud:one",
            "resume",
        ));
        assert!(result.is_err());
        assert_eq!(c.entries.lock().unwrap()[0].state, "pause");
        assert!(!root.exists());
    }
}
