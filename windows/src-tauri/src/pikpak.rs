// PikPak 云盘离线。移植自 Mac 版 PikPak.swift。
//
// 为什么要这个：BT 网络里没人做种的老片、国内热门片（都在迅雷用户之间传）用本机下几乎下不动，
// 交给 PikPak 的服务器代下（热门的它云端早有现成文件，几秒就好），再从云盘按普通直链拉回本地。
// 用的是用户自己的 PikPak 账号，只存 token，不存密码。
//
// 和 Mac 版的差别（都是有意为之）：
//   1. 界面只用浏览器登录（Google 账号没有密码，内嵌窗口又过不了 Google），由扩展把登录信息交过来。令牌绑定网页客户端，刷新和云盘请求都按网页端签名。
//      PikPak 改版换签名参数时，从网页脚本重新取（和 Mac 版 refreshWebSign 一样）。
//   2. token 存 %APPDATA%\com.shandian.windows\pikpak.json，字段比 Mac 多一个 device_id：
//      风控签名要把设备号拼进去，换设备号容易被判成异常登录。
//   3. PikPak 在境外，请求一律按 proxy.rs 的规则挂系统代理；交给内核的那一段仍是 no_proxy
//      （api.rs 已经处理），这两件事别混。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::State;

use crate::AppState;

/// 签发 token 的客户端。这套参数取自开源库 PikPakAPI / Mac 版，改了就登不上
const CLIENT_ID: &str = "YNxT9w7GMdWvEOKa";
const CLIENT_SECRET: &str = "dbw2OtmVEeuUvIptb1Coyg";
const CLIENT_VERSION: &str = "1.47.1";
const PACKAGE_NAME: &str = "com.pikcloud.pikpak";

/// 验证码签名的盐，顺序不能动：每一轮都是「上一轮结果 + 这个盐」再过一次 MD5
const SALTS: &[&str] = &[
    "Gez0T9ijiI9WCeTsKSg3SMlx",
    "zQdbalsolyb1R/",
    "ftOjr52zt51JD68C3s",
    "yeOBMH0JkbQdEFNNwQ0RI9T3wU/v",
    "BRJrQZiTQ65WtMvwO",
    "je8fqxKPdQVJiy1DM6Bc9Nb1",
    "niV",
    "9hFCW2R1",
    "sHKHpe2i96",
    "p7c5E6AcXQ/IJUuAEC9W6",
    "",
    "aRv9hjc9P+Pbn+u3krN6",
    "BzStcgE8qVdqjEH16l4",
    "SqgeZvL5j9zoHP95xWHt",
    "zVof5yaJkPe3VFpadPof",
];

/// 和 Mac 版一模一样的 UA。PikPak 的接口会看它，换成下载工具的名字会被风控
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// 网页客户端。常量和 Mac 版一样，来自 mypikpak.com/drive 的网页脚本
const WEB_CLIENT_ID: &str = "YUMx5nI8ZU8Ap8pm";
const WEB_VERSION: &str = "2.0.0";
const WEB_PACKAGE: &str = "mypikpak.com";
const WEB_TIMESTAMP: &str = "1793092691562";
const WEB_SALTS: [&str; 15] = [
    "C9qPpZLN8ucRTaTiUMWYS9cQvWOE",
    "+r6CQVxjzJV6LCV",
    "F",
    "pFJRC",
    "9WXYIDGrwTCz2OiVlgZa90qpECPD6olt",
    "/750aCr4lm/Sly/c",
    "RB+DT/gZCrbV",
    "",
    "CyLsf7hdkIRxRm215hl",
    "7xHvLi2tOYP0Y92b",
    "ZGTXXxu8E/MIWaEDB+Sm/",
    "1UI3",
    "E7fP5Pfijd+7K+t6Tg/NhuLq0eEUVChpJSkrKxpO",
    "ihtqpG6FMt65+Xk+tWUH2",
    "NhXXU9rg4XXdzo7u5o",
];
/// 网页改版后重新取到的（时间戳、盐）
static TOKEN_REFRESH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static WEB_SIGN: std::sync::Mutex<Option<(String, Vec<String>)>> = std::sync::Mutex::new(None);
const USER_HOST: &str = "https://user.mypikpak.com";
const API_HOST: &str = "https://api-drive.mypikpak.com";

/// 任务列表要哪些状态。PENDING 也算「在跑」，不然刚提交的任务会从列表里消失
const TASK_FILTERS: &str =
    r#"{"phase":{"in":"PHASE_TYPE_RUNNING,PHASE_TYPE_ERROR,PHASE_TYPE_PENDING,PHASE_TYPE_COMPLETE"}}"#;

// ---------------- 登录信息 ----------------

/// 落盘的东西。注意没有 password 字段——密码只在登录那一次用，换完 token 就丢
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Session {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    username: String,
    /// 过期时刻（毫秒）。提前一分钟刷新，别等接口回 error_code 16 再救
    #[serde(default)]
    expires_at: i64,
    /// PikPak 的风控设备号，captcha 签名要拼它。第一次登录生成，之后一直复用
    #[serde(default)]
    device_id: String,
    #[serde(default)]
    user_id: String,
    /// 签发令牌的客户端。空 = 旧版账号密码登录的安卓客户端
    #[serde(default)]
    client_id: String,
}

fn file_of(state: &AppState) -> PathBuf {
    // 登录令牌只留在这台机器（%LOCALAPPDATA%），不跟账户漫游
    crate::migrate::local_path(&state.local, "pikpak.json")
}

fn load(state: &AppState) -> Option<Session> {
    let text = std::fs::read_to_string(file_of(state)).ok()?;
    let s: Session = serde_json::from_str(&text).ok()?;
    // 只有半个文件（比如写到一半断电）也当没登录，免得后面一堆接口报 401
    if s.access_token.is_empty() || s.refresh_token.is_empty() {
        return None;
    }
    Some(s)
}

fn save(state: &AppState, s: &Session) -> Result<(), String> {
    let _ = std::fs::create_dir_all(&state.local);
    let text = serde_json::to_string_pretty(s).map_err(|e| format!("序列化登录信息失败：{e}"))?;
    std::fs::write(file_of(state), text).map_err(|e| format!("写登录信息失败：{e}"))
}

/// 界面上的账号名。不返回 token，前端只需要知道「登没登、是谁」
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub logged_in: bool,
    pub username: String,
}

// ---------------- 数据结构 ----------------

/// 云端任务，形状按前端要的来（camelCase，和别处的命令一致）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PikPakTask {
    pub id: String,
    pub name: String,
    pub size: u64,
    /// running / done / error
    pub state: String,
    /// 0-100
    pub progress: u8,
    pub message: String,
}

#[derive(Clone)]
struct RemoteFile {
    id: String,
    relative: String,
    name: String,
    size: u64,
    url: String,
}

// ---------------- 云端任务：显示进度、下好自动拉回 ----------------

/// 交给云盘的任务。关掉 App 云端照常下，下次打开接着查
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudJob {
    pub id: String,
    pub name: String,
    pub url: String,
    pub size: u64,
    pub progress: u8,
    /// running 云端下载中 / pulling 拉回中（已在任务列表里）/ error
    pub state: String,
    pub message: String,
    /// 拉回时建的本地任务
    pub local: Vec<String>,
    pub tries: u32,
    pub origin: String,
    pub created: i64,
    pub dir: String,
    pub pull_dir: String,
    pub root_file_id: String,
    pub pulls: Vec<PullBinding>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PullBinding {
    pub file_id: String,
    pub name: String,
    pub relative: String,
    pub size: u64,
    pub dir: String,
    pub save_name: String,
    pub local_id: String,
    pub intent: String,
    pub attempts: u32,
    pub next: i64,
    pub downloaded: u64,
}
static JOB_OP: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

static JOBS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn jobs_file(state: &AppState) -> PathBuf {
    crate::migrate::local_path(&state.local, "pikpak-jobs.json")
}

pub fn load_jobs(state: &AppState) -> Vec<CloudJob> {
    let _g = JOBS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::fs::read_to_string(jobs_file(state)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_jobs_result(state: &AppState, jobs: &[CloudJob]) -> Result<(), String> {
    use std::{io::Write, os::windows::ffi::OsStrExt};
    let _g = JOBS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let file=jobs_file(state);if let Some(parent)=file.parent(){std::fs::create_dir_all(parent).map_err(|_|"无法创建云盘任务目录")?;}
    let tmp=file.with_extension(format!("{}.tmp",rand::random::<u64>()));
    let result=(||{
        let mut output=std::fs::OpenOptions::new().create_new(true).write(true).open(&tmp).map_err(|_|"无法保存云盘任务")?;
        output.write_all(&serde_json::to_vec(jobs).map_err(|_|"无法序列化云盘任务")?).map_err(|_|"无法写入云盘任务")?;
        output.sync_all().map_err(|_|"无法确认云盘任务已保存")?;drop(output);
        let from:Vec<u16>=tmp.as_os_str().encode_wide().chain(Some(0)).collect();let to:Vec<u16>=file.as_os_str().encode_wide().chain(Some(0)).collect();
        let ok=unsafe{windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(),to.as_ptr(),windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING|windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH)};
        if ok==0{return Err("无法原子保存云盘任务".into())}Ok(())
    })();if result.is_err(){let _=std::fs::remove_file(&tmp);}result
}
fn save_jobs(state: &AppState, jobs: &[CloudJob]) {let _=save_jobs_result(state,jobs);}
fn persist_job(state:&AppState,job:&CloudJob)->Result<(),String>{let mut jobs=load_jobs(state);if let Some(index)=jobs.iter().position(|j|j.id==job.id){jobs[index]=job.clone()}else{jobs.insert(0,job.clone())}save_jobs_result(state,&jobs)}

fn job_api(state:&AppState,_origin:&str)->Result<crate::api::Api,String>{let e=state.engine.lock().unwrap();Ok(crate::api::Api::new(e.port,e.token.clone()))}
fn expired_pull(task:&crate::api::Task)->bool {
    task.status=="error"&&task.shandian_error.as_deref().is_some_and(|e|{
        let e=e.to_ascii_lowercase();["status=401","status=403","status=410","status 401","status 403","status 410","http 401","http 403","http 410"].iter().any(|status|e.contains(status))
    })
}
fn child_finished(job:&CloudJob,tasks:&[crate::api::Task])->bool {
    !job.local.is_empty()&&job.local.iter().all(|id|tasks.iter().any(|t|t.id==id.trim_start_matches("nas:")&&t.status=="done"))
}
fn owned_child<'a>(binding:&PullBinding,tasks:&'a[crate::api::Task])->Result<Option<&'a crate::api::Task>,String>{
    let matches:Vec<_>=tasks.iter().filter(|t|t.meta.as_ref().and_then(|m|m.opts.as_ref()).is_some_and(|o|o.path==binding.dir&&o.name==binding.save_name)&&t.total()==binding.size).collect();
    match matches.len(){0=>Ok(None),1=>Ok(matches.first().copied()),_=>Err("同一拉回位置存在多个任务，已保留进度等待核对".into())}
}
fn pull_child_dir(root:&str,relative:&str,remote:bool)->String{
    let parts:Vec<_>=relative.split(['/','\\']).filter(|p|!p.is_empty()&&*p!="."&&*p!="..").collect();let parents=&parts[..parts.len().saturating_sub(1)];
    if remote {parents.iter().fold(root.trim_end_matches('/').to_string(),|path,part|format!("{path}/{}",crate::wechat::safe_name(part)))}
    else {parents.iter().fold(PathBuf::from(root),|path,part|path.join(crate::wechat::safe_name(part))).to_string_lossy().to_string()}
}
async fn pull_job_files(state:&AppState,job:&mut CloudJob,file_id:&str,title:&str)->Result<Vec<String>,String>{
    let picked=pick_files(files_of(state,file_id).await?);if picked.is_empty(){return Err("云端没有可下载的原文件".into())}
    let api=job_api(state,&job.origin)?;let tasks=api.list_tasks().await.map_err(|_|"暂时无法读取目标下载任务")?;
    let original=if !job.origin.is_empty(){tasks.iter().find(|t|t.id==job.origin.trim_start_matches("nas:"))}else{let mut matching=tasks.iter().filter(|t|t.url()==job.url&&t.status!="done");let first=matching.next();if matching.next().is_none(){first}else{None}};
    if let Some(t)=original {if job.origin.is_empty(){job.origin=t.id.clone()}if job.dir.is_empty(){job.dir=t.saved_dir().map(|p|p.to_string_lossy().to_string()).unwrap_or_default()}if t.status!="done"{api.pause(&t.id).await.map_err(|_|"无法暂停原任务，已有文件保持不动")?;}}
    if job.dir.is_empty(){job.dir=state.settings.lock().unwrap().dir_for(crate::links::LinkKind::Http,&state.folders).to_string_lossy().to_string()}
    if job.pull_dir.is_empty(){let folder=format!("{} (PikPak-{:016x})",crate::wechat::safe_name(if title.is_empty(){"云盘拉回"}else{title}),rand::random::<u64>());job.pull_dir=if job.origin.starts_with("nas:"){format!("{}/{}",job.dir.trim_end_matches('/'),folder)}else{PathBuf::from(&job.dir).join(folder).to_string_lossy().to_string()}}
    job.root_file_id=file_id.into();
    if job.pulls.is_empty(){job.pulls=picked.iter().map(|f|PullBinding{file_id:f.id.clone(),name:f.name.clone(),relative:f.relative.clone(),size:f.size,dir:pull_child_dir(&job.pull_dir,&f.relative,job.origin.starts_with("nas:")),save_name:crate::wechat::safe_name(&f.name),..Default::default()}).collect();persist_job(state,job)?;}
    let headers=vec![("User-Agent".to_string(),UA.to_string())];
    for index in 0..job.pulls.len(){
        if !job.pulls[index].local_id.is_empty(){continue}
        if !job.pulls[index].intent.is_empty(){
            let current=api.list_tasks().await.map_err(|_|"无法核对已提交的拉回任务")?;
            if let Some(found)=owned_child(&job.pulls[index],&current)?{job.pulls[index].local_id=found.id.clone();job.pulls[index].intent.clear();}
            else {job.state="confirm".into();job.message="上次拉回提交结果不确定，原任务和文件已保留；请核对目标任务后处理".into();persist_job(state,job)?;return Err(job.message.clone())}
        }else{
            let f=remote_file(state,&job.pulls[index].file_id,&job.pulls[index].relative).await?;
            if f.name!=job.pulls[index].name||f.size!=job.pulls[index].size {return Err("云端原文件信息已变化，已有文件和任务保持不动".into())}
            if !job.origin.starts_with("nas:"){std::fs::create_dir_all(&job.pulls[index].dir).map_err(|_|"无法创建拉回目录")?;}
            job.pulls[index].intent="creating".into();persist_job(state,job)?;
            match api.add_task(&f.url,&job.pulls[index].dir,&job.pulls[index].save_name,&headers,&[]).await {
                Ok(id)=>{job.pulls[index].local_id=id;job.pulls[index].intent.clear();}
                Err(_)=>{job.pulls[index].intent="unknown".into();job.state="confirm".into();job.message="拉回提交结果不确定，已停止自动重提并保留原任务".into();persist_job(state,job)?;return Err(job.message.clone())}
            }
        }
        job.local=job.pulls.iter().filter(|f|!f.local_id.is_empty()).map(|f|if job.origin.starts_with("nas:"){format!("nas:{}",f.local_id)}else{f.local_id.clone()}).collect();persist_job(state,job)?;
    }
    job.state="pulling".into();job.message.clear();persist_job(state,job)?;Ok(job.pulls.iter().map(|f|f.name.clone()).collect())
}
/// Manual pulls also use durable intents and keep the original task/files.
async fn pull_files(state:&AppState,file_id:&str,title:&str,source:&str)->Result<(Vec<String>,Vec<String>),String>{
    let _op=JOB_OP.lock().await;
    let mut job=load_jobs(state).into_iter().find(|j|!source.is_empty()&&j.url==source).unwrap_or_else(||CloudJob{id:format!("manual-{:032x}",rand::random::<u128>()),name:title.into(),url:source.into(),created:now_ms(),..Default::default()});
    let names=pull_job_files(state,&mut job,file_id,title).await?;Ok((job.local,names))
}
async fn migrate_pulls(state:&AppState,job:&mut CloudJob,tasks:&[crate::api::Task],cloud:&[Value])->Result<(),String>{
    if !job.pulls.is_empty(){return Ok(())}
    if job.root_file_id.is_empty(){job.root_file_id=cloud.iter().find(|t|s_of(t,"id")==job.id).map(|t|s_of(t,"file_id")).filter(|id|!id.is_empty()).ok_or("旧拉回任务缺少云文件身份，已有进度已保留，请重新关联确认")?;}
    let files=files_of(state,&job.root_file_id).await?;
    let mut bindings=Vec::new();
    for id in &job.local {
        let task=tasks.iter().find(|t|t.id==id.trim_start_matches("nas:")).ok_or("拉回任务缺失，不能视为下载完成；云端文件已保留")?;
        let name=task.meta.as_ref().and_then(|m|m.res.as_ref()).and_then(|r|r.files.first()).map(|f|f.name.as_str()).unwrap_or("");
        let matching:Vec<_>=files.iter().filter(|f|f.size==task.total()&&(f.name==name||crate::wechat::safe_name(&f.name)==name)).collect();
        if matching.len()!=1{return Err("无法唯一核对旧拉回文件的身份，已有进度已保留".into())}let f=matching[0];
        bindings.push(PullBinding{file_id:f.id.clone(),name:f.name.clone(),relative:f.relative.clone(),size:f.size,local_id:task.id.clone(),downloaded:task.downloaded(),..Default::default()});
    }
    if bindings.is_empty(){return Err("旧拉回任务为空，云端文件已保留".into())}job.pulls=bindings;persist_job(state,job)
}
async fn refresh_pulls(state:&AppState,job:&mut CloudJob,tasks:&[crate::api::Task],api:&crate::api::Api)->Result<(),String>{
    for binding in &mut job.pulls {
        let task=tasks.iter().find(|t|t.id==binding.local_id).ok_or("拉回任务缺失，不能视为下载完成；云端文件已保留")?;
        if task.downloaded()>binding.downloaded {binding.attempts=0;binding.next=0;binding.downloaded=task.downloaded()}
        if !expired_pull(task)||binding.next>now_ms(){continue}
        if binding.attempts>=5{return Err("自动换链连续失败，已保留拉回任务及原有进度，请核对 PikPak 登录".into())}
        binding.attempts+=1;binding.next=now_ms()+(30_000_i64*(1_i64<<binding.attempts.min(5)));
        let result=async{
            let fresh=remote_file(state,&binding.file_id,&binding.relative).await?;
            if fresh.name!=binding.name||fresh.size!=binding.size{return Err("云端原文件已变化，已有进度保持不动".into())}
            crate::cloud::renew_pull(api,task,&fresh.url,&[("User-Agent".into(),UA.into())],binding.size).await
        }.await;
        if let Err(e)=result{job.message=e}else{job.message.clear()}
    }
    persist_job(state,job)
}

/// 已开始拉回的云端任务 -> 接替它的本地任务，界面用来保持列表位置
#[tauri::command]
pub fn pikpak_pulled(state: State<'_, AppState>) -> std::collections::HashMap<String, Vec<String>> {
    load_jobs(&state).into_iter().filter(|j| matches!(j.state.as_str(),"pulling"|"done")).map(|j| (j.id, j.local)).collect()
}

/// 交给云盘的原任务在云端阶段不显示，只留云端这一条。失败了才露出来，可以接着在本地下
pub fn hides(jobs: &[CloudJob], url: &str, status: &str) -> bool {
    status != "done" && jobs.iter().any(|j| matches!(j.state.as_str(), "running"|"pulling"|"done") && j.url == url)
}

pub(crate) fn annotate_pulling_tasks(jobs: &[CloudJob], tasks: &mut [crate::api::Task]) {
    let pulling: std::collections::HashSet<&str> = jobs.iter()
        .filter(|job| job.state != "done" && !job.id.starts_with("manual-"))
        .flat_map(|job| job.local.iter().map(String::as_str))
        .collect();
    for task in tasks {
        if task.status != "done" && pulling.contains(task.id.as_str()) {
            let label = if task.id.starts_with("nas:") {
                "NAS · PikPak 中转 · 取回 NAS"
            } else {
                "PikPak 中转 · 取回本机"
            };
            task.origin_label = Some(label.into());
        }
    }
}

/// 云端阶段的任务拼进任务列表。拉回后本地任务自己会显示，这里不再列
pub fn jobs_as_tasks(state: &AppState) -> Vec<crate::api::Task> {
    jobs_as_tasks_matching(&load_jobs(state), None)
}

pub(crate) fn jobs_as_tasks_matching(jobs: &[CloudJob], key: Option<&str>) -> Vec<crate::api::Task> {
    jobs.iter()
          .filter(|j| key.map_or(true, |key| crate::links::canonical_url(&j.url) == key))
        .filter(|j| !matches!(j.state.as_str(),"pulling"|"done"))
        .filter_map(|j| {
            let failed = matches!(j.state.as_str(), "error"|"confirm");
            let mut t: crate::api::Task = serde_json::from_value(json!({
                "id": format!("pp:{}", j.id),
                "name": j.name,
                "protocol": "cloud",
                "status": if failed { "error" } else { "running" },
                "progress": { "downloaded": j.size * j.progress as u64 / 100, "speed": 0 },
                "meta": { "req": { "url": j.url }, "res": { "name": j.name, "size": j.size, "files": [] } },
                "shandianError": if failed { j.message.as_str() } else { "" },
            }))
            .ok()?;
            t.kind = Some("云盘离线".into());
            if !j.id.starts_with("manual-") {
                t.origin_label = Some(if j.state == "running" {
                    "PikPak 中转 · 云端下载"
                } else {
                    "PikPak 中转"
                }.into());
            }
            t.created = Some(j.created.max(0) as u64);
            if !failed {
                t.stage = Some(if j.state=="submitting" {"正在提交云端…".into()}else{format!("云端下载中 {}%", j.progress)});
            }
            Some(t)
        })
        .collect()
}

/// 用户在任务列表里移除：云端任务和文件一起删
pub async fn cancel_job(state: &AppState, id: &str) -> Result<(), String> {
    let _op=JOB_OP.lock().await;
    // Removing the download record does not delete the user's cloud files.
    let _=api(state,reqwest::Method::DELETE,&format!("{API_HOST}/drive/v1/tasks?task_ids={id}&delete_files=false"),None).await;
    let mut jobs = load_jobs(state);
    jobs.retain(|j| j.id != id);
    save_jobs(state, &jobs);
    Ok(())
}

/// 失败的任务重新提交一次
pub async fn retry_job(state:&AppState,id:&str)->Result<(),String>{
    let _op=JOB_OP.lock().await;
    let job=load_jobs(state).into_iter().find(|j|j.id==id).ok_or("任务不存在")?;
    if matches!(job.state.as_str(),"confirm"|"submitting"){return Err("提交结果不确定，请先在 PikPak 核对已有任务，避免重复下载".into())}
    if !job.pulls.is_empty(){let mut job=job;for pull in &mut job.pulls{pull.attempts=0;pull.next=0;}job.state="pulling".into();job.message.clear();return persist_job(state,&job)}
    let mut origin=if job.origin.is_empty(){None}else{job_api(state,&job.origin)?.list_tasks().await.map_err(|_|"无法读取原任务")?.into_iter().find(|t|t.id==job.origin.trim_start_matches("nas:"))};
    if let Some(task)=origin.as_mut(){task.id=job.origin.clone();}
    let mut jobs=load_jobs(state);jobs.retain(|j|j.id!=id);save_jobs_result(state,&jobs)?;
    submit_inner(state,&job.url,origin.as_ref()).await.map(|_|())
}

async fn check_jobs(app: &tauri::AppHandle) {
    use crate::shell::NotifyExt;
    use tauri::Manager;
    let _op=JOB_OP.lock().await;
    let st=app.state::<AppState>();let state:&AppState=&st;let mut jobs=load_jobs(state);
    if jobs.is_empty()||load(state).is_none(){return}
    let needs_cloud=jobs.iter().any(|j|!matches!(j.state.as_str(),"done"|"error"|"confirm"|"submitting")&&(j.state!="pulling"||j.pulls.is_empty()));
    let cloud=if needs_cloud {match offline_tasks(state).await{Ok(tasks)=>tasks,Err(_)=>return}}else{Vec::new()};
    for j in &mut jobs {
        match j.state.as_str(){
            "error"|"done"|"confirm"=>{},
            "submitting"=>{j.state="confirm".into();j.message="上次云端提交结果不确定，请先核对 PikPak 任务；原下载已保留".into();},
            "pulling"=>{
                let Ok(api)=job_api(state,&j.origin)else{continue};let Ok(tasks)=api.list_tasks().await else{continue};
                if let Err(e)=migrate_pulls(state,j,&tasks,&cloud).await{j.state="error".into();j.message=e;continue}
                if let Err(e)=refresh_pulls(state,j,&tasks,&api).await{j.state="error".into();j.message=e;continue}
                if child_finished(j,&tasks){j.state="done".into();j.message.clear();let _=app.notify_rust("云盘任务已拉回",&j.name);}
            },
            _=>{
                let Some(task)=cloud.iter().find(|t|s_of(t,"id")==j.id)else{j.message="云端任务暂未查询到，原下载及云端文件已保留".into();continue};
                let view=to_task(task);j.name=view.name.clone();j.size=view.size;j.progress=view.progress;
                match view.state.as_str(){
                    "error"=>{j.state="error".into();j.message=format!("云端下载失败：{}",nonempty(view.message,"PikPak 没给出原因"));let _=app.notify_rust("云盘任务失败",&j.name);},
                    "done"=>{
                        let fid=s_of(task,"file_id");if fid.is_empty(){j.state="error".into();j.message="PikPak 未返回原文件身份，原下载已保留".into();continue}
                        let title=j.name.clone();match pull_job_files(state,j,&fid,&title).await{
                            Ok(_)=>{let _=app.notify_rust("云端已下好，开始拉回",&j.name);},
                            Err(e)=>{if j.state!="confirm"{j.tries+=1;if j.tries>=5{j.state="error".into();}}j.message=e;}
                        }
                    },_=>{}
                }
            }
        }
    }
    save_jobs(state,&jobs);
}

/// App 开着时每 20 秒查一次
pub fn start_monitor(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            check_jobs(&app).await;
        }
    });
}

// ---------------- 命令 ----------------

/// 登录。成功与否只看能不能拿到 access_token；密码用完就扔
#[tauri::command]
pub async fn pikpak_login(
    username: String,
    password: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let username = username.trim().to_string();
    if username.is_empty() || password.is_empty() {
        return Err("账号和密码都要填".into());
    }
    let device = new_device_id();
    // 服务端按账号长什么样决定校验哪种登录名：邮箱、手机号、还是普通用户名
    let meta = if username.contains('@') {
        json!({ "email": username })
    } else if username.chars().all(|c| c.is_ascii_digit()) && username.len() >= 11 {
        json!({ "phone_number": username })
    } else {
        json!({ "username": username })
    };
    let signin = format!("{USER_HOST}/v1/auth/signin");
    // 登录这个动作本身要过人机验证：action 是「方法:完整 URL」，和 Mac 版一致，
    // 只写路径会被服务端判成签名不匹配
    let captcha = captcha_init(&device, &format!("POST:{signin}"), Some(meta)).await?;
    let body = json!({
        "client_id": CLIENT_ID,
        "client_secret": CLIENT_SECRET,
        "username": username,
        "password": password,
        "captcha_token": captcha,
    });
    let r = send(reqwest::Method::POST, &signin, Some(body), &device, None, None).await?;
    let (Some(at), Some(rt)) = (
        r.get("access_token").and_then(Value::as_str),
        r.get("refresh_token").and_then(Value::as_str),
    ) else {
        return Err(format!(
            "登录失败：{}",
            message(&r).unwrap_or_else(|| "账号或密码不对，或者 PikPak 要求人机验证".into())
        ));
    };
    let s = Session {
        access_token: at.to_string(),
        refresh_token: rt.to_string(),
        username: username.clone(),
        expires_at: now_ms() + expires_in(&r) * 1000,
        device_id: device,
        user_id: r
            .get("sub")
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default(),
        client_id: String::new(),
    };
    save(&state, &s)?;
    Ok(format!("已登录 PikPak：{username}"))
}

/// 浏览器扩展交来的网页登录信息（{key, c, device}）。令牌只存本机
pub fn adopt_web_login(state: &AppState, v: &Value) -> Result<(), String> {
    let c = &v["c"];
    if c["access_token"].as_str().unwrap_or_default().is_empty() || c["refresh_token"].as_str().unwrap_or_default().is_empty() {
        return Err("缺少登录信息".into());
    }
    // 网页存的 deviceid 形如 wdi10.<…>，签名和请求头用最后一段的前 32 位
    let raw = v["device"].as_str().unwrap_or_default().trim_matches('"');
    let device: String = raw.rsplit('.').next().unwrap_or(raw).chars().take(32).collect();
    let text = |k: &str| c[k].as_str().unwrap_or_default().to_string();
    let name = [text("username"), text("email")].into_iter().find(|s| !s.is_empty()).unwrap_or_else(|| "PikPak 账号".into());
    let sub = [text("sub"), text("user_id")].into_iter().find(|s| !s.is_empty()).unwrap_or_default();
    save(
        state,
        &Session {
            access_token: text("access_token"),
            refresh_token: text("refresh_token"),
            username: name,
            // 网页没给可靠的过期时刻，先当半小时，到点刷新；提前过期时接口回 16 也会刷新
            expires_at: now_ms() + 1_800_000,
            device_id: device,
            user_id: sub,
            client_id: WEB_CLIENT_ID.to_string(),
        },
    )
}

/// 只报「登没登、是谁」。token 不出 Rust 这一层
#[tauri::command]
pub fn pikpak_status(state: State<'_, AppState>) -> Status {
    match load(&state) {
        Some(s) => Status {
            logged_in: true,
            username: s.username,
        },
        None => Status {
            logged_in: false,
            username: String::new(),
        },
    }
}

/// Computer association validates the imported account before acknowledging
/// the browser transfer. A failed attempt keeps the previous local login.
static LINK_QUOTA:std::sync::Mutex<Option<(u128,std::time::Instant,Option<(u64,u64)>)>>=std::sync::Mutex::new(None);
static ACCOUNT_EDIT:tokio::sync::Mutex<()>=tokio::sync::Mutex::const_new(());
pub async fn associate_web_login(state:&AppState,value:&Value)->Result<(),String>{
    let _link=ACCOUNT_EDIT.lock().await;
    let previous=load(state);
    adopt_web_login(state,value)?;
    let checked=api(state,reqwest::Method::GET,&format!("{API_HOST}/drive/v1/about"),None).await;
    match checked {Err(error)=>{*LINK_QUOTA.lock().unwrap()=None;if let Some(old)=previous {save(state,&old)?}else{let _=std::fs::remove_file(file_of(state));}Err(error)},Ok(value)=>{*LINK_QUOTA.lock().unwrap()=Some((login_revision(state),std::time::Instant::now(),quota_value(&value)));Ok(())}}

}
pub fn login_revision(state:&AppState)->u128 {std::fs::metadata(file_of(state)).ok().and_then(|m|m.modified().ok()).and_then(|t|t.duration_since(std::time::UNIX_EPOCH).ok()).map(|t|t.as_nanos()).unwrap_or(0)}

/// 登出：把 token 文件删掉就够了（服务端没有「注销这个设备」的必要）
#[tauri::command]
pub async fn pikpak_logout(state: State<'_, AppState>) -> Result<(), String> {
    let _edit=ACCOUNT_EDIT.lock().await;
    match std::fs::remove_file(file_of(&state)) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删登录信息失败：{e}")),
    }
}

/// 提交离线下载。磁力、种子网址、http 直链都原样塞进 url.url，这一点和 Mac 版一样：
/// PikPak 自己按内容判断，不需要我们分类
#[tauri::command]
pub async fn pikpak_offline(url: String, state: State<'_, AppState>) -> Result<String, String> {
    submit(&state, &url).await
}

/// 登录过（接力判断用，不校验令牌是否还有效，提交失败时会走下一棒）
pub fn logged_in(state: &AppState) -> bool {
    load(state).is_some()
}

/// 云盘容量（字节）：(总量, 已用)。接口 /drive/v1/about；取不到返回 None。
/// 界面在拿到余额之前不允许交给云盘离线，避免没测出空间就直接提交
#[tauri::command]
pub async fn pikpak_quota(state: State<'_, AppState>) -> Result<Option<(u64, u64)>, String> {
    if let Some((revision,at,value))=LINK_QUOTA.lock().unwrap().take(){if revision==login_revision(&state)&&at.elapsed()<std::time::Duration::from_secs(15){return Ok(value)}}
    let v = api(&state, reqwest::Method::GET, &format!("{API_HOST}/drive/v1/about"), None).await?;
    Ok(quota_value(&v))
}

fn quota_value(v:&Value)->Option<(u64,u64)>{let number=|key:&str|v["quota"][key].as_u64().or_else(||v["quota"][key].as_str().and_then(|s|s.parse().ok()));match(number("limit"),number("usage")){(Some(total),Some(used))if total>0=>Some((total,used)),_=>None}}

pub async fn submit(state:&AppState,url:&str)->Result<String,String>{
    let _op=JOB_OP.lock().await;submit_inner(state,url,None).await
}
pub async fn submit_task(state:&AppState,url:&str,task:&crate::api::Task)->Result<String,String>{
    let _op=JOB_OP.lock().await;submit_inner(state,url,Some(task)).await
}
async fn submit_inner(state:&AppState,url:&str,task:Option<&crate::api::Task>)->Result<String,String>{
    let link=url.trim();if link.is_empty(){return Err("链接是空的，没东西可交".into())}
    if !matches!(url::Url::parse(link).ok().map(|u|u.scheme().to_owned()).as_deref(),Some("http"|"https"|"magnet")){return Err("PikPak 当前通道支持 HTTP、HTTPS 和磁力链接".into())}
    if load_jobs(state).iter().any(|j|j.url==link&&!matches!(j.state.as_str(),"done"|"error")){return Err("这条链接已有云盘任务，请等待当前任务或先核对未确认的提交".into())}
    let intent=format!("intent-{:032x}",rand::random::<u128>());
    let mut job=CloudJob{id:intent.clone(),url:link.into(),state:"submitting".into(),name:task.and_then(|t|t.name.clone()).unwrap_or_else(||"云端任务".into()),created:now_ms(),origin:task.map(|t|t.id.clone()).unwrap_or_default(),dir:task.and_then(|t|t.saved_dir()).map(|p|p.to_string_lossy().to_string()).unwrap_or_default(),..Default::default()};
    persist_job(state,&job)?;
    let body=json!({"kind":"drive#file","upload_type":"UPLOAD_TYPE_URL","url":{"url":link},"folder_type":"DOWNLOAD"});
    let response=match api(state,reqwest::Method::POST,&format!("{API_HOST}/drive/v1/files"),Some(body)).await{
        Ok(value)=>value,
        Err(_)=>{job.state="confirm".into();job.message="云端提交未确认，请先核对 PikPak 任务，避免重复提交；原下载已保留".into();persist_job(state,&job)?;return Err(job.message)}
    };
    let Some(remote)=response.get("task")else{job.state="confirm".into();job.message="PikPak 未返回任务身份，请先核对云端；原下载已保留".into();persist_job(state,&job)?;return Err(job.message)};
    let remote_id=s_of(remote,"id");if remote_id.is_empty(){job.state="confirm".into();job.message="PikPak 提交结果缺少任务身份，已停止自动重提".into();persist_job(state,&job)?;return Err(job.message)}
    job.id=remote_id;job.name=nonempty(nonempty(s_of(remote,"name"),&s_of(remote,"file_name")),&job.name);job.state=if s_of(remote,"phase")=="PHASE_TYPE_ERROR"{"error"}else{"running"}.into();job.message=if job.state=="error"{zh_message(&s_of(remote,"message"))}else{String::new()};
    let mut jobs=load_jobs(state);jobs.retain(|j|j.id!=intent);jobs.insert(0,job.clone());save_jobs_result(state,&jobs)?;
    if job.state=="error"{return Err(format!("云端拒绝了这个任务：{}",nonempty(job.message,"链接可能失效或受到服务商限制")))}
    Ok("已交给 PikPak，下好后自动拉回；原下载和已有文件已保留".into())
}

/// 把云端下好的文件拉回本地：取直链 + 文件名，交给内核当普通 HTTP 任务下。
/// 没下完就返回一句人话错误，前端 toast 出来即可
#[tauri::command]
pub async fn pikpak_pull(id: String, state: State<'_, AppState>) -> Result<String, String> {
    let list = offline_tasks(&state).await?;
    let mut file_id = id.clone();
    let mut title = String::new();
    if let Some(t) = list.iter().find(|t| s_of(t, "id") == id) {
        let phase = s_of(t, "phase");
        if phase == "PHASE_TYPE_ERROR" {
            return Err(format!(
                "云端下载失败了：{}",
                nonempty(s_of(t, "message"), "PikPak 没给出原因")
            ));
        }
        if phase != "PHASE_TYPE_COMPLETE" {
            return Err(format!(
                "云端还在下（{}%），等会儿再点一次",
                progress_of(t)
            ));
        }
        title = to_task(t).name;
        // 任务里带 file_id 就用它；不带就退回任务 id（老任务、网页端建的可能没有）
        let fid = s_of(t, "file_id");
        if !fid.is_empty() {
            file_id = fid;
        }
    }

    let (_, names) = pull_files(&state, &file_id, &title, &load_jobs(&state).into_iter().find(|j| j.id == id).map(|j| j.url).unwrap_or_default()).await?;
    // Durable mappings remain so an expired link resumes the same child task.
    Ok(format!("已从云盘拉回，开始下载：{}", names.join("、")))
}

/// 云端任务列表（设置页和右键菜单都靠它显示进度）
#[tauri::command]
pub async fn pikpak_tasks(state: State<'_, AppState>) -> Result<Vec<PikPakTask>, String> {
    let list = offline_tasks(&state).await?;
    Ok(list.iter().map(to_task).collect())
}

/// 删掉一个云端任务，连同它下好的文件。id 可能是任务 id（右键菜单给的就是这个），
/// 也可能是文件 id，两种都认——先按任务查一遍，查不到就当文件 id 用
#[tauri::command]
pub async fn pikpak_remove(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let _op=JOB_OP.lock().await;
    remove_cloud(&state, &id).await?;
    let mut jobs = load_jobs(&state);
    jobs.retain(|j| j.id != id);
    save_jobs(&state, &jobs);
    Ok(())
}

async fn remove_cloud(state: &AppState, id: &str) -> Result<(), String> {
    let list = offline_tasks(state).await.unwrap_or_default();
    let task = list.iter().find(|t| s_of(t, "id") == id);
    let file_id = task.map(|t| s_of(t, "file_id")).unwrap_or_else(|| id.to_string());

    // Mac 的 deleteForever 语义：先进回收站再彻底删除。免费账号只有 6 GB，
    // 拉回本地后不删干净，下几部片子就满了。失败的任务可能没有文件，这一步失败不算错
    if !file_id.is_empty() {
        let body = json!({ "ids": [file_id] });
        let _ = api(state, reqwest::Method::POST, &format!("{API_HOST}/drive/v1/files:batchTrash"), Some(body.clone())).await;
        let _ = api(state, reqwest::Method::POST, &format!("{API_HOST}/drive/v1/files:batchDelete"), Some(body)).await;
    }
    // 任务记录要单独删，只删文件的话它还留在云端列表里
    if task.is_some() {
        api(state, reqwest::Method::DELETE, &format!("{API_HOST}/drive/v1/tasks?task_ids={id}&delete_files=true"), None).await?;
    }
    Ok(())
}

// ---------------- 任务 / 文件 ----------------

/// 拉一次离线任务列表。过滤条件照 Mac 版抄：四个阶段都要，否则刚提交的 PENDING 任务看不到
async fn offline_tasks(state: &AppState) -> Result<Vec<Value>, String> {
    let url = format!(
        "{API_HOST}/drive/v1/tasks?type=offline&limit=200&filters={}",
        percent(TASK_FILTERS)
    );
    let r = api(state, reqwest::Method::GET, &url, None).await?;
    Ok(r.get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn to_task(t: &Value) -> PikPakTask {
    let name = nonempty(
        nonempty(s_of(t, "name"), &s_of(t, "file_name")),
        &t.get("params")
            .and_then(|p| p.get("url"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    );
    PikPakTask {
        id: s_of(t, "id"),
        name: nonempty(name, "云端任务"),
        size: n_of(t, "size").max(0) as u64,
        state: match s_of(t, "phase").as_str() {
            "PHASE_TYPE_COMPLETE" => "done",
            "PHASE_TYPE_ERROR" => "error",
            // RUNNING / PENDING 都算在跑，界面上都显示进度条
            _ => "running",
        }
        .into(),
        progress: progress_of(t),
        message: {
            let m = s_of(t, "message");
            if m.is_empty() || m.to_lowercase() == "saved" { m } else { zh_message(&m) }
        },
    }
}

fn progress_of(t: &Value) -> u8 {
    n_of(t, "progress").clamp(0, 100) as u8
}

fn parse_remote_file(info:&Value,file_id:&str,relative:&str)->Result<RemoteFile,String>{
    if s_of(info,"id")!=file_id||s_of(info,"kind")=="drive#folder"{return Err("PikPak 原文件身份无法核实，已有进度已保留".into())}
    let name=s_of(info,"name");let size=n_of(info,"size").max(0) as u64;
    let link=info["web_content_link"].as_str().filter(|s|!s.is_empty()).ok_or("PikPak 原文件下载地址尚未就绪，暂不使用转码文件替代")?;
    let uri=url::Url::parse(link).map_err(|_|"PikPak 原文件地址无效")?;if !matches!(uri.scheme(),"http"|"https")||uri.host_str().is_none()||name.is_empty(){return Err("PikPak 原文件信息不完整".into())}
    Ok(RemoteFile{id:file_id.into(),relative:if relative.is_empty(){name.clone()}else{relative.into()},name,size,url:link.into()})
}
async fn remote_file(state:&AppState,file_id:&str,relative:&str)->Result<RemoteFile,String>{
    let info=api(state,reqwest::Method::GET,&format!("{API_HOST}/drive/v1/files/{}",percent(file_id)),None).await?;parse_remote_file(&info,file_id,relative)
}
/// Resolve only original bytes. Transcoded media is a different representation
/// and must never be spliced into an existing original-file download.
async fn files_of(state:&AppState,file_id:&str)->Result<Vec<RemoteFile>,String>{
    let info=api(state,reqwest::Method::GET,&format!("{API_HOST}/drive/v1/files/{}",percent(file_id)),None).await?;
    if s_of(&info,"kind")!="drive#folder"{return Ok(vec![parse_remote_file(&info,file_id,"")?])}
    let mut out=Vec::new();for(id,relative,size)in list_folder(state,file_id).await?{let name=relative.rsplit('/').next().unwrap_or(&relative).to_string();out.push(RemoteFile{id,relative,name,size,url:String::new()})}Ok(out)
}
/// Preserve relative directories and every page when expanding a cloud folder.
async fn list_folder(state:&AppState,root:&str)->Result<Vec<(String,String,u64)>,String>{
    let mut out=Vec::new();let mut stack=vec![(root.to_string(),String::new())];let mut visited=std::collections::HashSet::new();
    while let Some((parent,prefix))=stack.pop(){if !visited.insert(parent.clone()){continue}let mut page=String::new();let mut pages=std::collections::HashSet::new();loop{
        let url=format!("{API_HOST}/drive/v1/files?parent_id={}&limit=500&filters={}&page_token={}",percent(&parent),percent(r#"{"trashed":{"eq":false}}"#),percent(&page));
        let result=api(state,reqwest::Method::GET,&url,None).await?;
        for file in result["files"].as_array().into_iter().flatten(){let id=s_of(file,"id");let name=s_of(file,"name");if id.is_empty()||name.is_empty(){continue}let relative=if prefix.is_empty(){name}else{format!("{prefix}/{name}")};if s_of(file,"kind")=="drive#folder"{stack.push((id,relative))}else{out.push((id,relative,n_of(file,"size").max(0) as u64));}}
        page=s_of(&result,"next_page_token");if page.is_empty(){break}if !pages.insert(page.clone()){return Err("PikPak 文件列表分页重复，已停止以免遗漏文件".into())}
    }}Ok(out)
}

fn pick_files(files: Vec<RemoteFile>) -> Vec<RemoteFile> {
    // 「只下正片 + 字幕」的规则别在这里重写一遍：links::pick_files 就是 Mac 版
    // FileSelection 的移植（样片、低清副本、海报都剔掉），两边保持一致
    let metas: Vec<crate::links::FileMeta> = files
        .iter()
        .map(|f| crate::links::FileMeta {
            name: f.name.clone(),
            size: f.size,
        })
        .collect();
    if !metas.iter().any(|m| crate::thumbs::is_video(&m.name)) {
        // 一个视频都没有（压缩包、文档之类）：只拿最大的那个，
        // 别把文件夹里几百个小文件全丢给内核
        return files.into_iter().max_by_key(|f| f.size).into_iter().collect();
    }
    match crate::links::pick_files(&metas) {
        Some(keep) => keep
            .into_iter()
            .filter_map(|i| files.get(i).cloned())
            .collect(),
        // None = 全都要（比如剧集每集都够大），照 Mac 版全下
        None => files,
    }
}

// ---------------- 请求 ----------------

/// 带登录态调 api-drive 的接口。token 过期（error_code 16）自动刷新重试一次，
/// 这是 Mac 版的同款兜底——expires_at 算不准的时候靠它
async fn api(
    state: &AppState,
    method: reqwest::Method,
    url: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let Some(s) = load(state) else {
        return Err("还没有登录 PikPak，请先在设置里登录".into());
    };
    let s = if s.expires_at > 0 && now_ms() + 60_000 >= s.expires_at {
        refresh(state, &s).await?
    } else {
        s
    };

    let r = authed(&s, method.clone(), url, body.clone()).await?;
    if err_code(&r) == 16 {
        let s2 = refresh(state, &s).await?;
        let r2 = authed(&s2, method, url, body).await?;
        check(&r2)?;
        return Ok(r2);
    }
    check(&r)?;
    Ok(r)
}

/// 网页端每个云盘请求都要带验证码令牌和客户端号
async fn authed(s: &Session, method: reqwest::Method, url: &str, body: Option<Value>) -> Result<Value, String> {
    if s.client_id != WEB_CLIENT_ID {
        return send(method, url, body, &s.device_id, Some(&s.access_token), None).await;
    }
    let path = url.strip_prefix(API_HOST).unwrap_or(url).split('?').next().unwrap_or_default();
    let captcha = captcha_web(s, &format!("{method}:{path}")).await?;
    send_ex(method, url, body, &s.device_id, Some(&s.access_token), Some(&captcha), &[("X-Client-Id", WEB_CLIENT_ID)]).await
}

async fn captcha_web(s: &Session, action: &str) -> Result<String, String> {
    let mut refreshed = false;
    loop {
        let (ts, salts) = WEB_SIGN.lock().unwrap().clone().unwrap_or_else(|| {
            (WEB_TIMESTAMP.to_string(), WEB_SALTS.iter().map(|x| x.to_string()).collect())
        });
        let mut sign = format!("{WEB_CLIENT_ID}{WEB_VERSION}{WEB_PACKAGE}{}{ts}", s.device_id);
        for salt in &salts {
            sign = md5_hex(&format!("{sign}{salt}"));
        }
        let body = json!({
            "client_id": WEB_CLIENT_ID,
            "action": action,
            "device_id": s.device_id,
            "meta": {
                "captcha_sign": format!("1.{sign}"),
                "client_version": WEB_VERSION,
                "package_name": WEB_PACKAGE,
                "user_id": s.user_id,
                "timestamp": ts,
            },
        });
        let r = send(reqwest::Method::POST, &format!("{USER_HOST}/v1/shield/captcha/init"), Some(body), &s.device_id, None, None).await?;
        if let Some(t) = r.get("captcha_token").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            return Ok(t.to_string());
        }
        let msg = message(&r).unwrap_or_else(|| "PikPak 要求人机验证，请重新网页登录".into());
        // 签名被拒多半是网页改版换了参数：重取一次再试
        if refreshed || !msg.contains("captcha_sign") || !refresh_web_sign().await {
            return Err(msg);
        }
        refreshed = true;
    }
}

/// 脚本里有一段 clientId:"…",clientVersion:"2.0.0",packageName:"mypikpak.com",timestamp:"…",algorithms:[{alg:"md5",salt:"…"},…]
async fn refresh_web_sign() -> bool {
    async fn fetch(url: &str) -> Option<String> {
        let mut b = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).user_agent(UA);
        if let Some(px) = crate::proxy::url_for(url).and_then(|p| reqwest::Proxy::all(&p).ok()) {
            b = b.proxy(px);
        }
        b.build().ok()?.get(url).send().await.ok()?.text().await.ok()
    }
    let Some(page) = fetch("https://mypikpak.com/drive/").await else { return false };
    let needle = format!("clientId:\"{WEB_CLIENT_ID}\",clientVersion");
    for part in page.split("src=\"").skip(1) {
        let Some(path) = part.split('"').next().filter(|p| p.starts_with("/drive/") && p.ends_with(".js")) else { continue };
        let Some(js) = fetch(&format!("https://mypikpak.com{path}")).await else { continue };
        let Some(at) = js.find(&needle) else { continue };
        let chunk: String = js[at..].chars().take(3000).collect();
        let Some(ts) = chunk.split("timestamp:\"").nth(1).and_then(|x| x.split('"').next()) else { continue };
        let salts: Vec<String> = chunk.split("salt:\"").skip(1).filter_map(|x| x.split('"').next().map(String::from)).collect();
        if salts.len() > 5 {
            *WEB_SIGN.lock().unwrap() = Some((ts.to_string(), salts));
            return true;
        }
    }
    false
}

/// 刷新 token。失败基本只有一个原因：refresh_token 也过期了（PikPak 会定期踢设备）
async fn refresh(state: &AppState, s: &Session) -> Result<Session, String> {
    let _refresh=TOKEN_REFRESH.lock().await;
    match load(state){Some(current)if current.refresh_token!=s.refresh_token=>return Ok(current),None=>return Err("PikPak 已退出登录，请重新关联账号".into()),_=>{}}
    // 令牌绑定签发它的客户端，刷新要用同一个
    let web = s.client_id == WEB_CLIENT_ID;
    let body = json!({
        "client_id": if web { WEB_CLIENT_ID } else { CLIENT_ID },
        "refresh_token": s.refresh_token,
        "grant_type": "refresh_token",
    });
    let web_headers = [("X-Client-Id", WEB_CLIENT_ID), ("X-Sdk-Version", "8.1.4"), ("X-Protocol-Version", "301")];
    let r = send_ex(
        reqwest::Method::POST,
        &format!("{USER_HOST}/v1/auth/token"),
        Some(body),
        &s.device_id,
        None,
        None,
        if web { &web_headers } else { &[] },
    )
    .await?;
    let (Some(at), Some(rt)) = (
        r.get("access_token").and_then(Value::as_str),
        r.get("refresh_token").and_then(Value::as_str),
    ) else {
        match load(state) {
            Some(current) if current.refresh_token != s.refresh_token => return Ok(current),
            None => return Err("PikPak 已退出，请重新关联".into()),
            _ => {}
        }
        if let Some(app) = crate::engine::APP.get() {
            crate::float::update_cloud_account_notice(app, "pikpak", "PikPak", false, "expired");
        }
        return Err("PikPak 登录已过期，请重新关联".into());
    };
    let next = Session {
        access_token: at.to_string(),
        refresh_token: rt.to_string(),
        expires_at: now_ms() + expires_in(&r) * 1000,
        ..s.clone()
    };
    // A user may have logged in while the request was in flight. Never let an
    // old rotation replace a newly associated account.
    match load(state){Some(current)if current.refresh_token!=s.refresh_token=>return Ok(current),None=>return Err("PikPak 已退出登录，请重新关联账号".into()),_=>{}}
    save(state, &next)?;
    Ok(next)
}

/// 人机验证令牌。安卓客户端这套签名 = client_id+版本+包名+设备号+时间戳，
/// 再过 15 轮「拼一个盐取 MD5」
async fn captcha_init(device: &str, action: &str, meta: Option<Value>) -> Result<String, String> {
    let meta = meta.unwrap_or_else(|| {
        let ts = now_ms().to_string();
        let mut sign = format!("{CLIENT_ID}{CLIENT_VERSION}{PACKAGE_NAME}{device}{ts}");
        for salt in SALTS {
            sign = md5_hex(&format!("{sign}{salt}"));
        }
        json!({
            "captcha_sign": format!("1.{sign}"),
            "client_version": CLIENT_VERSION,
            "package_name": PACKAGE_NAME,
            "user_id": "",
            "timestamp": ts,
        })
    });
    let body = json!({
        "client_id": CLIENT_ID,
        "action": action,
        "device_id": device,
        "meta": meta,
    });
    let r = send(
        reqwest::Method::POST,
        &format!("{USER_HOST}/v1/shield/captcha/init"),
        Some(body),
        device,
        None,
        None,
    )
    .await?;
    match r.get("captcha_token").and_then(Value::as_str) {
        Some(t) if !t.is_empty() => Ok(t.to_string()),
        _ => Err(message(&r).unwrap_or_else(|| {
            "PikPak 要求人机验证，先在 PikPak 官网或 App 里登录一次再试".into()
        })),
    }
}

/// 真正发请求。PikPak 全在境外，按 proxy.rs 的规则挂系统代理
async fn send(
    method: reqwest::Method,
    url: &str,
    body: Option<Value>,
    device: &str,
    bearer: Option<&str>,
    captcha: Option<&str>,
) -> Result<Value, String> {
    send_ex(method, url, body, device, bearer, captcha, &[]).await
}

async fn send_ex(
    method: reqwest::Method,
    url: &str,
    body: Option<Value>,
    device: &str,
    bearer: Option<&str>,
    captcha: Option<&str>,
    extra: &[(&str, &str)],
) -> Result<Value, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(UA);
    let proxy = crate::proxy::url_for(url).and_then(|p| reqwest::Proxy::all(p).ok());
    let via_proxy = proxy.is_some();
    if let Some(px) = proxy {
        builder = builder.proxy(px);
    } else {
        builder = builder.no_proxy();
    }
    let client = builder.build().map_err(|e| format!("建网络连接失败：{e}"))?;

    let mut req = client
        .request(method, url)
        .header("Content-Type", "application/json; charset=utf-8")
        .header("X-Device-Id", device);
    if let Some(t) = bearer {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    if let Some(c) = captcha {
        req = req.header("X-Captcha-Token", c);
    }
    for (k, v) in extra {
        req = req.header(*k, *v);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| network_error(&e, via_proxy))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| network_error(&e, via_proxy))?;
    // PikPak 错误也回 JSON，偶尔（网关/维护页）是 HTML。解析不了就按 HTTP 状态报
    match serde_json::from_str::<Value>(&text) {
        Ok(v) if v.is_object() => Ok(v),
        _ if status.is_success() => Ok(json!({})),
        _ => Err(format!("PikPak 返回了看不懂的内容（HTTP {status}）")),
    }
}

/// 展示已知的失败阶段和实际连接方式，不把带查询参数的请求地址送到界面。
fn network_error(error: &reqwest::Error, via_proxy: bool) -> String {
    use std::error::Error;
    let mut causes = String::new();
    let mut cause = error.source();
    while let Some(e) = cause {
        causes.push_str(&e.to_string().to_lowercase());
        cause = e.source();
    }
    let reason = if error.is_timeout() {
        "请求 PikPak 超时（30 秒内未完成）"
    } else if causes.contains("dns") || causes.contains("getaddrinfo") || causes.contains("no such host") {
        "无法解析 PikPak 服务器域名"
    } else if causes.contains("certificate") || causes.contains("tls") || causes.contains("ssl") {
        "无法与 PikPak 建立安全连接（TLS / 证书错误）"
    } else if error.is_connect() {
        "无法建立到 PikPak 的网络连接"
    } else {
        "与 PikPak 的网络连接中断"
    };
    let route = if via_proxy {
        "当前通过系统代理连接，请检查代理服务后重试。"
    } else {
        "当前为直连，请检查网络能否访问 PikPak 后重试。"
    };
    format!("{reason}。{route}")
}

/// 业务错误检查。PikPak 习惯 HTTP 200 + body 里带 error / error_code
fn check(v: &Value) -> Result<(), String> {
    let has_error = v
        .get("error")
        .and_then(Value::as_str)
        .map(|e| !e.is_empty())
        .unwrap_or(false);
    if has_error || (err_code(v) != 0 && err_code(v) != 16) {
        return Err(human(v));
    }
    Ok(())
}

/// PikPak 的报错多是英文，常见的翻成中文
fn zh_message(m: &str) -> String {
    let l = m.to_lowercase();
    if l.contains("task_daily_create_limit") || l.contains("daily") || l.contains("limit") || l.contains("quota") || m.contains("次数") {
        "PikPak 返回离线额度限制，请在账号页面核对剩余额度或稍后重试".into()
    } else if l.contains("space") || l.contains("capacity") || l.contains("storage") {
        "云盘空间不够了，请先清理云盘".into()
    } else if l.contains("too many") || l.contains("frequen") {
        "操作太频繁，稍后再试".into()
    } else if l.contains("captcha") {
        "PikPak 要求人机验证，请重新登录后再试".into()
    } else if l.contains("copyright") || l.contains("sensitive") || l.contains("forbidden") {
        "这个资源被 PikPak 拦截了，可能涉及版权".into()
    } else if l.contains("not found") || l.contains("invalid") {
        "PikPak 认不出这个链接，可能已经失效".into()
    } else if m.is_ascii() && !m.is_empty() {
        format!("PikPak 返回错误：{m}")
    } else {
        m.to_string()
    }
}

fn human(v: &Value) -> String {
    match err_code(v) {
        16 => "登录已过期，请在设置里重新登录 PikPak".into(),
        9 => format!(
            "PikPak 说：{}（账号或密码不对？）",
            message(v).unwrap_or_else(|| "unauthorized".into())
        ),
        _ => message(v).map(|m| zh_message(&m)).unwrap_or_else(|| "PikPak 那边出错了，稍后再试".into()),
    }
}

fn message(v: &Value) -> Option<String> {
    ["error_description", "message", "error"]
        .iter()
        .find_map(|k| {
            v.get(*k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
}

fn err_code(v: &Value) -> i64 {
    v.get("error_code")
        .map(|x| {
            x.as_i64()
                .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

fn expires_in(v: &Value) -> i64 {
    v.get("expires_in")
        .map(|x| {
            x.as_i64()
                .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
                .unwrap_or(7200)
        })
        .unwrap_or(7200)
        .clamp(60, 30 * 24 * 3600)
}

// ---------------- 小工具 ----------------

fn s_of(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// PikPak 的数字有的给 number、有的给字符串（size 一直是字符串），两种都吃
fn n_of(v: &Value, key: &str) -> i64 {
    v.get(key)
        .map(|x| {
            x.as_i64()
                .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

fn nonempty(a: String, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else {
        a
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 32 位十六进制设备号。和 Mac 版一样去掉连字符、转小写
fn new_device_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| char::from_digit(rng.gen_range(0..16), 16).unwrap_or('0'))
        .collect()
}

/// 查询串里的 JSON 要整体百分号编码。只留字母数字，和 Mac 的 .alphanumerics 一致
fn percent(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 手写 MD5。PikPak 的签名就是把一段串反复过 MD5，只为一处用，不想再引一个 crate
fn md5_hex(input: &str) -> String {
    let data = input.as_bytes();
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_le_bytes());

    let mut a0: u32 = 0x6745_2301;
    let mut b0: u32 = 0xefcd_ab89;
    let mut c0: u32 = 0x98ba_dcfe;
    let mut d0: u32 = 0x1032_5476;

    // 每轮的位移量，RFC 1321 写死的
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6,
        10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];

    for chunk in msg.chunks(64) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            // K[i] = floor(abs(sin(i+1)) * 2^32)，标准就这几个常量，现算省得抄错
            let k = ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32;
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let tmp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(k)
                    .wrapping_add(m[g])
                    .rotate_left(S[i]),
            );
            a = tmp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    // MD5 的输出是小端：每个 32 位字按字节序打印
    let hex = |w: u32| -> String {
        format!(
            "{:02x}{:02x}{:02x}{:02x}",
            w as u8,
            (w >> 8) as u8,
            (w >> 16) as u8,
            (w >> 24) as u8
        )
    };
    format!("{}{}{}{}", hex(a0), hex(b0), hex(c0), hex(d0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_labels_distinguish_pikpak_handoffs_from_direct_downloads() {
        let submitted = CloudJob {
            id: "offline".into(),
            state: "running".into(),
            ..Default::default()
        };
        let cloud = jobs_as_tasks_matching(&[submitted], None);
        assert_eq!(cloud[0].kind.as_deref(), Some("云盘离线"));
        assert_eq!(cloud[0].origin_label.as_deref(), Some("PikPak 中转 · 云端下载"));

        let jobs = [
            CloudJob {
                id: "offline".into(),
                state: "pulling".into(),
                local: vec!["local".into(), "nas:remote".into(), "finished".into()],
                ..Default::default()
            },
            CloudJob {
                id: "manual-file".into(),
                state: "pulling".into(),
                local: vec!["manual-pull".into()],
                ..Default::default()
            },
        ];
        let make_task = |id: &str, status: &str| {
            let mut task: crate::api::Task =
                serde_json::from_value(json!({"id": id, "status": status})).unwrap();
            task.kind = Some("PikPak".into());
            task
        };
        let mut tasks = [
            make_task("local", "running"),
            make_task("nas:remote", "pause"),
            make_task("finished", "done"),
            make_task("manual-pull", "running"),
            make_task("direct", "running"),
        ];
        annotate_pulling_tasks(&jobs, &mut tasks);
        assert_eq!(tasks[0].origin_label.as_deref(), Some("PikPak 中转 · 取回本机"));
        assert_eq!(tasks[1].origin_label.as_deref(), Some("NAS · PikPak 中转 · 取回 NAS"));
        assert!(tasks[2..].iter().all(|task| task.origin_label.is_none()));
        assert!(tasks.iter().all(|task| task.kind.as_deref() == Some("PikPak")));
    }
    fn task_for_pull(id:&str,status:&str,name:&str,size:u64,dir:&str)->crate::api::Task{
        serde_json::from_value(json!({"id":id,"status":status,"meta":{"opts":{"name":name,"path":dir},"res":{"name":name,"size":size,"files":[{"name":name,"size":size}]},"req":{"url":"https://download.invalid/original"}}})).unwrap()
    }
    #[test]
    fn missing_or_empty_pull_children_are_not_completed(){
        let mut job=CloudJob{local:vec!["one".into(),"two".into()],..Default::default()};
        let one=task_for_pull("one","done","a",10,"C:/owned");let two=task_for_pull("two","done","b",20,"C:/owned");
        assert!(!child_finished(&job,&[one.clone()]));assert!(child_finished(&job,&[one,two]));job.local.clear();assert!(!child_finished(&job,&[]));
    }
    #[test]
    fn historical_403_is_not_current_failure(){
        let mut task=task_for_pull("id","running","a",10,"C:/owned");task.shandian_error=Some("connection 0 failed: status=403".into());assert!(!expired_pull(&task));task.status="error".into();assert!(expired_pull(&task));task.shandian_error=Some("network timeout".into());assert!(!expired_pull(&task));
    }
    #[test]
    fn pull_recovery_requires_unique_owned_path_name_and_size(){
        let binding=PullBinding{dir:"C:/owned/job".into(),save_name:"a.mkv".into(),size:20,..Default::default()};let exact=task_for_pull("id","error","a.mkv",20,"C:/owned/job");
        assert_eq!(owned_child(&binding,&[exact.clone()]).unwrap().unwrap().id,"id");assert!(owned_child(&binding,&[exact.clone(),exact]).is_err());assert!(owned_child(&binding,&[task_for_pull("id","done","a.mkv",21,"C:/owned/job")]).unwrap().is_none());
    }
    #[test]
    fn cloud_identity_and_original_representation_are_required(){
        let mut info=json!({"id":"file-1","name":"a.mkv","size":"100","kind":"drive#file","web_content_link":"https://download.invalid/original?signature=private"});
        let file=parse_remote_file(&info,"file-1","folder/a.mkv").unwrap();assert_eq!(file.id,"file-1");assert_eq!(file.size,100);assert_eq!(file.relative,"folder/a.mkv");assert!(parse_remote_file(&info,"another-id","").is_err());
        info.as_object_mut().unwrap().remove("web_content_link");info["medias"]=json!([{"link":{"url":"https://download.invalid/transcoded"}}]);assert!(parse_remote_file(&info,"file-1","").is_err());
    }
    #[test]
    fn old_cloud_records_keep_progress_with_default_new_fields(){
        let job:CloudJob=serde_json::from_value(json!({"id":"old","name":"movie","url":"magnet:?xt=old","progress":42,"state":"pulling","local":["id1"]})).unwrap();assert_eq!(job.progress,42);assert_eq!(job.local,vec!["id1"]);assert!(job.pulls.is_empty());assert!(job.dir.is_empty());
    }
    #[test]
    fn unknown_acceptance_keeps_original_visible_and_quota_has_no_invented_numbers(){
        let job=CloudJob{url:"https://resource.invalid/a".into(),state:"confirm".into(),..Default::default()};assert!(!hides(&[job],"https://resource.invalid/a","running"));let message=zh_message("daily limit exceeded");assert!(!message.contains("3")&&!message.contains("20 GB"));
    }


    #[test]
    fn network_errors_explain_route_without_request_urls() {
        tauri::async_runtime::block_on(async {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/v1/tasks?token=private", listener.local_addr().unwrap());
            let client = reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_millis(50)).build().unwrap();
            // 端口保持打开但不响应，稳定制造真实请求超时。
            let timeout = client.get(&url).send().await.unwrap_err();
            assert!(timeout.is_timeout());
            for via_proxy in [false, true] {
                let text = network_error(&timeout, via_proxy);
                assert!(text.contains("超时"));
                assert!(text.contains(if via_proxy { "通过系统代理" } else { "直连" }));
                assert!(!text.contains("private") && !text.contains("http://"));
            }
            drop(listener);
            let client = reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(3)).build().unwrap();
            let connect = client.get(&url).send().await.unwrap_err();
            assert!(connect.is_connect());
            assert!(network_error(&connect, false).contains("无法建立"));
        });
    }

    #[test]
    fn md5_matches_rfc1321() {
        assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex("abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex("message digest"),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
        // 超过 64 字节，验证补位和长度字段
        assert_eq!(
            md5_hex("12345678901234567890123456789012345678901234567890123456789012345678901234567890"),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    #[test]
    fn query_encoding() {
        // 只留字母数字，花括号和引号都要编码
        assert_eq!(percent("{}"), "%7B%7D");
        assert_eq!(percent("a-b"), "a%2Db");
        // 下划线也要编码：Mac 的 .alphanumerics 只放行字母数字，这里保持一致
        assert_eq!(percent("A_B"), "A%5FB");
    }

    #[test]
    fn phase_mapping() {
        let t = |phase: &str, progress: Value| {
            to_task(&json!({
                "id": "t1", "name": "片子", "phase": phase, "progress": progress,
                "file_id": "f1", "size": "1234", "message": ""
            }))
        };
        let running = t("PHASE_TYPE_RUNNING", json!(37));
        assert_eq!(running.state, "running");
        assert_eq!(running.progress, 37);
        assert_eq!(running.size, 1234); // size 是字符串

        assert_eq!(t("PHASE_TYPE_COMPLETE", json!(100)).state, "done");
        assert_eq!(t("PHASE_TYPE_ERROR", json!("9")).state, "error");
        assert_eq!(t("PHASE_TYPE_ERROR", json!("9")).progress, 9);
        // 越界的进度别把 u8 撑爆
        assert_eq!(t("PHASE_TYPE_RUNNING", json!(1000)).progress, 100);
    }

    #[test]
    fn pick_video_and_subtitle() {
        let f = |name: &str, size: u64| RemoteFile {
            id: name.into(),
            relative: name.into(),
            name: name.into(),
            size,
            url: "https://x".into(),
        };
        let picked = pick_files(vec![
            f("a.nfo", 1),
            f("movie.mkv", 1000),
            f("movie.zh.srt", 2),
            f("sample.jpg", 3),
        ]);
        assert_eq!(picked.len(), 2);
        assert!(picked.iter().any(|x| x.name == "movie.mkv"));
        assert!(picked.iter().any(|x| x.name == "movie.zh.srt"));

        // 没正片就只拿最大的那个
        let picked = pick_files(vec![f("a.zip", 10), f("b.txt", 99)]);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].name, "b.txt");
    }
}
