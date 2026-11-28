use super::*;
use sha2::{Digest, Sha256};
use tauri::{Emitter, Manager};

const EXTENSION_ORIGIN: &str = "chrome-extension://pggchniffhkcgigajlapkboikmgdipel";
const TTL: u64 = 600;
#[derive(Clone)]
struct Target {
    nas: bool,
    fingerprint: String,
    label: String,
}
struct Progress {
    state: String,
    message: String,
    fingerprint: Option<String>,
}
struct OfficialGrant {
    id: String,
    code: String,
    expires_at: u64,
}
struct Ticket {
    id: String,
    provider: String,
    target: Target,
    login_url: String,
    transfer: bool,
    resume_token: Option<String>,
    official: Option<OfficialGrant>,
    created: u64,
    progress: Mutex<Progress>,
    notify: tokio::sync::Notify,
    working: AtomicBool,
    approved: AtomicBool,
}
static TICKETS: OnceLock<Mutex<HashMap<String, Arc<Ticket>>>> = OnceLock::new();
static EXT_OFFICIAL: AtomicBool = AtomicBool::new(false);
static EXT_RESUME_LINK: AtomicBool = AtomicBool::new(false);
static EXT_LINK: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
fn tickets() -> &'static Mutex<HashMap<String, Arc<Ticket>>> {
    TICKETS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn app() -> Result<tauri::AppHandle, String> {
    APP.get().cloned().ok_or_else(|| "应用尚未准备好".into())
}
fn target(state: &AppState, origin: &str) -> Result<Target, String> {
    if origin.starts_with("nas:") {
        #[cfg(feature = "nas")]
        {
            let (url, fingerprint) = crate::nas::cloud_identity(&state.local)?;
            let host = url::Url::parse(&url)
                .map_err(|_| "NAS地址无效")?
                .host_str()
                .unwrap_or("")
                .to_string();
            return Ok(Target {
                nas: true,
                fingerprint,
                label: format!("NAS（{host}）"),
            });
        }
        #[cfg(not(feature = "nas"))]
        {
            return Err("当前版本未启用NAS".into());
        }
    }
    Ok(Target {
        nas: false,
        fingerprint: "local".into(),
        label: "本机".into(),
    })
}
fn current_target(state: &AppState, t: &Target) -> bool {
    target(state, if t.nas { "nas:" } else { "" })
        .is_ok_and(|n| n.fingerprint == t.fingerprint && n.nas == t.nas)
}
fn scope(t: &Target) -> &'static str {
    if t.nas {
        "nas"
    } else {
        "local"
    }
}
fn valid_ticket(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|c| c.is_ascii_hexdigit())
}
fn get(id: &str) -> Result<Arc<Ticket>, String> {
    if !valid_ticket(id) {
        return Err("关联入口无效，请从云盘设置重新打开".into());
    }
    let t = tickets()
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .ok_or("关联入口已过期，请从云盘设置重新打开")?;
    if now().saturating_sub(t.created) > TTL {
        return Err("关联入口已过期，请重新打开".into());
    }
    Ok(t)
}
fn metadata(t: &Ticket) -> Value {
    let p = t.progress.lock().unwrap();
    let mut payload = json!({"ok":p.state!="error"&&p.state!="cancelled","ticket":t.id,"provider":t.provider,"target":scope(&t.target),"targetLabel":t.target.label,"loginUrl":t.login_url,"transferSession":t.transfer,"state":p.state,"message":p.message});
    if let Some(grant) = &t.official {
        payload["official"] = json!({"code":grant.code,"expiresAt":grant.expires_at});
    }
    if let Some(token) = &t.resume_token {
        payload["resumeToken"] = json!(token);
    }
    payload
}
fn finish(t: &Ticket, state: &str, message: String) {
    let mut p = t.progress.lock().unwrap();
    if p.state == "cancelled" && state != "cancelled" {
        t.working.store(false, Ordering::SeqCst);
        t.notify.notify_waiters();
        return;
    }
    p.state = state.into();
    p.message = message;
    let public_message=p.message.clone();
    drop(p);
    t.working.store(false, Ordering::SeqCst);
    t.notify.notify_waiters();
    if let Some(app) = APP.get() {
        if state == "ready" {
            crate::float::clear_cloud_account_notice(app, &t.provider, t.target.nas);
        }
        let _=app.emit("cloud-account-linked",json!({"provider":t.provider,"target":scope(&t.target),"targetLabel":t.target.label,"origin":if t.target.nas{"nas:"}else{""},"state":state,"message":public_message}));
    }
}

pub(super) fn invalidate(state: &AppState, id: &str, origin: &str) -> Result<(), String> {
    let dest = target(state, origin)?;
    let list: Vec<_> = tickets()
        .lock()
        .unwrap()
        .values()
        .filter(|t| t.provider == id && t.target.fingerprint == dest.fingerprint)
        .cloned()
        .collect();
    for t in list {
        finish(&t, "cancelled", "已退出当前端，原文件和下载任务保留".into())
    }
    Ok(())
}
pub(super) fn register_extension(value: &Value) {
    if value["cloudAccountLink"] != true {
        return;
    }
    EXT_OFFICIAL.store(value["cloudAccountOfficial"] == true, Ordering::SeqCst);
    EXT_RESUME_LINK.store(value["cloudResumeAccount"] == true, Ordering::SeqCst);
    let ids = value["cloudAccountProviders"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|id| {
            matches!(
                *id,
                "pikpak" | "guangya" | "quark" | "uc" | "pan115" | "pan123" | "mobile"
            )
        })
        .map(str::to_string)
        .collect();
    *EXT_LINK
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap() = ids;
}
fn browser_ready(id: &str) -> bool {
    crate::gateway::extension_status().connected
        && EXT_LINK
            .get()
            .is_some_and(|ids| ids.lock().unwrap().iter().any(|p| p == id))
}
pub(super) fn legacy_busy(id: &str) -> bool {
    tickets().lock().unwrap().values().any(|t| {
        t.provider == id
            && now().saturating_sub(t.created) <= TTL
            && matches!(
                t.progress.lock().unwrap().state.as_str(),
                "pending" | "linking" | "ready"
            )
    })
}
fn login_profile(id: &str) -> Result<(String, bool, bool), String> {
    if id == "pikpak" {
        return Ok(("https://mypikpak.com/drive/".into(), true, false));
    }
    let p = provider(id)?;
    if !p.can_link {
        return Err(p.link_message);
    }
    Ok((p.login_url, p.transfer_session, p.link_method == "desktop"))
}

pub(super) async fn start(provider_id: String, origin: String) -> Result<Value, String> {
    #[cfg(feature = "nas")]
    if provider_id == "pikpak" && origin.starts_with("nas:") {
        let app = app()?;
        let state = app.state::<AppState>();
        let dest = target(&state, &origin)?;
        let available = crate::nas::cloud_rpc(&state.local, reqwest::Method::GET, "pikpak/connect", None)
            .await.is_ok_and(|value| value["available"] == true);
        if available { return start_official(&state, dest).await; }
    }
    start_request(provider_id,origin,true,None).await
}

pub(super) fn target_fingerprint(state: &AppState, origin: &str) -> Result<String, String> {
    target(state, origin).map(|destination| destination.fingerprint)
}

pub(super) async fn start_resume(provider_id: String, origin: String, token: String, fingerprint: String) -> Result<Value, String> {
    if !browser_ready(&provider_id) || !EXT_RESUME_LINK.load(Ordering::SeqCst) {
        return Err("当前浏览器扩展不支持在续传时关联".into());
    }
    start_request(provider_id, origin, true, Some((token, fingerprint))).await
}

#[cfg(feature = "nas")]
async fn start_official(state: &AppState, dest: Target) -> Result<Value, String> {
    if !browser_ready("pikpak") || !EXT_OFFICIAL.load(Ordering::SeqCst) {
        return Err("请先在 Edge 或 Chrome 重新加载新版闪电扩展，再关联 PikPak".into());
    }
    let existing = tickets().lock().unwrap().values().find(|ticket| {
        ticket.target.fingerprint == dest.fingerprint
            && ticket.official.as_ref().is_some_and(|grant| grant.expires_at > now())
            && ticket.progress.lock().unwrap().state == "pending"
    }).cloned();
    if let Some(ticket) = existing {
        open_login_page(&ticket.login_url)?;
        return Ok(metadata(&ticket));
    }
    let grant = crate::nas::cloud_rpc(&state.local, reqwest::Method::POST, "pikpak/connect", Some(&json!({}))).await?;
    if !current_target(state, &dest) { return Err("NAS 连接已变化，请重新发起关联".into()); }
    let mut page = url::Url::parse(grant["url"].as_str().unwrap_or("")).map_err(|_| "PikPak 官方授权地址无效")?;
    if page.origin().ascii_serialization() != "https://mypikpak.com" || page.path() != "/drive/activate" || !page.username().is_empty() || page.password().is_some() {
        return Err("PikPak 官方授权地址无效".into());
    }
    let code = grant["code"].as_str().filter(|s| !s.is_empty() && s.len() <= 64 && s.bytes().all(|b|b.is_ascii_alphanumeric() || b == b'-' || b == b'_')).ok_or("PikPak 授权短码无效")?;
    let remote_id = grant["id"].as_str().filter(|s| s.starts_with("authorization-") && s.len() <= 64).ok_or("PikPak 授权记录无效")?;
    let expires_at = grant["expiresAt"].as_u64().filter(|t| *t > now() && t.saturating_sub(now()) <= TTL).ok_or("PikPak 授权已过期")?;
    let id = format!("{:032x}", rand::random::<u128>());
    page.query_pairs_mut().append_pair("shandian-account-link", &id);
    let ticket = Arc::new(Ticket {
        id: id.clone(), provider:"pikpak".into(), target:dest, login_url:page.into(), transfer:false, resume_token:None,
        official:Some(OfficialGrant{id:remote_id.into(),code:code.into(),expires_at}), created:now(),
        progress:Mutex::new(Progress{state:"pending".into(),message:"请在 PikPak 官方网页输入短码并确认".into(),fingerprint:None}),
        notify:tokio::sync::Notify::new(), working:AtomicBool::new(false), approved:AtomicBool::new(false),
    });
    {
        let mut all = tickets().lock().unwrap();
        all.retain(|_, old| now().saturating_sub(old.created) <= TTL);
        all.insert(id, ticket.clone());
    }
    if let Err(error) = open_login_page(&ticket.login_url) {
        finish(&ticket, "error", sdk::safe_error(&error));
        return Err(error);
    }
    Ok(metadata(&ticket))
}

#[cfg(feature = "nas")]
async fn poll_official(state: &AppState, t: &Ticket, grant: &OfficialGrant) -> Result<Value, String> {
    if t.progress.lock().unwrap().state != "pending" { return Ok(metadata(t)); }
    if grant.expires_at <= now() {
        finish(t, "error", "PikPak 授权已过期，请重新关联".into());
        return Ok(metadata(t));
    }
    if t.working.swap(true, Ordering::SeqCst) { return Ok(metadata(t)); }
    let result = crate::nas::cloud_rpc(&state.local, reqwest::Method::POST, "pikpak/connect/result", Some(&json!({"id":grant.id}))).await;
    t.working.store(false, Ordering::SeqCst);
    let result = result?;
    if !current_target(state, &t.target) { return Err("NAS 连接已变化，请重新发起关联".into()); }
    match result["status"].as_str() {
        Some("ready") => finish(t, "ready", "PikPak 已授权给 NAS".into()),
        Some("pending") => (),
        _ => finish(t, "error", "PikPak 授权未完成，请重试".into()),
    }
    Ok(metadata(t))
}

async fn start_request(provider_id: String, origin: String, open_page: bool, resume: Option<(String,String)>) -> Result<Value, String> {
    let app = app()?;
    let state = app.state::<AppState>();
    let dest = target(&state, &origin)?;
    let resume_token = match resume {
        Some((token, fingerprint)) if fingerprint == dest.fingerprint => Some(token),
        Some(_) => return Err("下载位置的连接已变化，请在原任务重新点继续".into()),
        None => None,
    };
    let (url, transfer, desktop) = login_profile(&provider_id)?;
    if resume_token.is_some() && (desktop || transfer) {
        return Err("当前网盘不支持在续传时关联".into());
    }
    let existing = {
        let mut items = tickets().lock().unwrap();
        items.retain(|_, t| now().saturating_sub(t.created) <= TTL);
        items
            .values()
            .find(|t| {
                t.provider == provider_id && t.official.is_none() && t.resume_token == resume_token
                    && matches!(
                        t.progress.lock().unwrap().state.as_str(),
                        "pending" | "linking"
                    )
            })
            .cloned()
    };
    if let Some(t) = existing {
        if t.target.fingerprint == dest.fingerprint {
            if open_page {
                if desktop { spawn_desktop(t.clone()); launch_guangya()?; }
                else { open_login_page(&t.login_url)?; }
            }
            return Ok(metadata(&t));
        }
        if !open_page { return Err("关联目标已变化，请重新打开扩展确认目标".into()); }
        let mut progress = t.progress.lock().unwrap();
        if progress.state == "pending" {
            progress.state = "cancelled".into();
            progress.message = format!("已改为关联到{}，此入口已取消", dest.label);
            drop(progress);
            t.notify.notify_waiters();
        } else {
            return Err(format!(
                "正在验证到{}，请完成当前关联后再操作",
                t.target.label
            ));
        }
    }

    let id = format!("{:032x}", rand::random::<u128>());
    let login_url = if desktop {
        String::new()
    } else {
        let mut u = url::Url::parse(&url).map_err(|_| "网盘登录地址无效")?;
        u.query_pairs_mut()
            .append_pair("shandian-account-link", &id);
        u.into()
    };
    let t = Arc::new(Ticket {
        id: id.clone(),
        provider: provider_id,
        target: dest,
        login_url,
        transfer,
        resume_token,
        official: None,
        created: now(),
        progress: Mutex::new(Progress {
            state: "pending".into(),
            message: if desktop {
                "请在光鸭电脑版登录，软件会继续关联"
            } else if open_page {
                "请在打开的网盘网页完成登录，之后会自动关联"
            } else {
                "请在网盘网页或扩展弹窗中点击关联"
            }
            .into(),
            fingerprint: None,
        }),
        notify: tokio::sync::Notify::new(),
        working: AtomicBool::new(false),
        approved: AtomicBool::new(false),
    });
    {
        let mut all = tickets().lock().unwrap();
        if let Some(old) = all.values().find(|old| {
            old.provider == t.provider && old.official.is_none() && old.resume_token == t.resume_token
                && matches!(
                    old.progress.lock().unwrap().state.as_str(),
                    "pending" | "linking"
                )
        }) {
            if old.target.fingerprint == t.target.fingerprint {
                return Ok(metadata(old));
            }
            return Err(format!("正在关联到{}，请先完成当前关联", old.target.label));
        }
        all.insert(id, t.clone());
    }
    let opened:Result<(),String>=async {
 if !open_page { return Ok(()); }
 if desktop {
  let sdk=manager(&state).sdk().await?;let info=sdk.call(reqwest::Method::POST,"/api/fs/native_offline/desktop/status",Some(&json!({"provider":"guangya"}))).await?;
  if info["installed"]!=true{finish(&t,"error","请先在电脑安装并登录光鸭客户端，再点关联".into());return Err("请先在电脑安装并登录光鸭客户端，再点关联".into())}
  use tauri_plugin_dialog::{DialogExt,MessageDialogButtons};let (send,receive)=tokio::sync::oneshot::channel();
  app.dialog().message(format!("将光鸭电脑版当前登录关联到{}？\n关联成功后电脑版将退出登录，避免两端刷新冲突。下载文件和设置保留。",t.target.label)).title("关联光鸭账号").buttons(MessageDialogButtons::OkCancel).show(move|yes|{let _=send.send(yes);});
  if !receive.await.unwrap_or(false){finish(&t,"cancelled","已取消，原登录和文件保留".into());return Ok(())}
  t.approved.store(true,Ordering::SeqCst);
  if info["ready"]==true{spawn_desktop(t.clone())}else{launch_guangya()?;}
 } else {open_login_page(&t.login_url)?;}
 Ok(())}.await;
    if let Err(e) = opened {
        finish(&t, "error", sdk::safe_error(&e));
        return Err(e);
    }
    Ok(metadata(&t))
}
fn open_login_page(url: &str) -> Result<(), String> {
    let browser=crate::gateway::extension_status().browser.to_ascii_lowercase();
    if matches!(browser.as_str(),"edge"|"chrome") { crate::gateway::open_browser_url(&browser,url) }
    else { crate::engine::open_url(url.into()) }
}
fn launch_guangya() -> Result<(), String> {
    let exe = PathBuf::from(std::env::var_os("ProgramFiles").ok_or("没有找到光鸭电脑版")?)
        .join("guangyapan/guangyapan.exe");
    if !exe.is_file() {
        return Err("没有找到光鸭电脑版".into());
    }
    std::process::Command::new(exe)
        .spawn()
        .map(|_| ())
        .map_err(|_| "无法打开光鸭电脑版".into())
}
pub(super) fn tick() {
    let list: Vec<_> = tickets()
        .lock()
        .unwrap()
        .values()
        .filter(|t| {
            t.provider == "guangya"
                && t.approved.load(Ordering::SeqCst)
                && t.progress.lock().unwrap().state == "pending"
                && now().saturating_sub(t.created) <= TTL
        })
        .cloned()
        .collect();
    for t in list {
        spawn_desktop(t)
    }
}
fn spawn_desktop(t: Arc<Ticket>) {
    if !t.approved.load(Ordering::SeqCst) {
        return;
    }
    if t.working.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let outcome: Result<Option<String>, String> = async {
            let app = app()?;
            let state = app.state::<AppState>();
            if !current_target(&state, &t.target) {
                return Err("NAS连接已变化，请重新发起关联".into());
            }
            let c = manager(&state);
            let sdk = c.sdk().await?;
            let info = sdk
                .call(
                    reqwest::Method::POST,
                    "/api/fs/native_offline/desktop/status",
                    Some(&json!({"provider":"guangya"})),
                )
                .await?;
            if info["ready"] != true {
                return Ok(None);
            }
            {
                let mut progress = t.progress.lock().unwrap();
                if progress.state != "pending" {
                    return Ok(None);
                }
                progress.state = "linking".into();
            }
            let prepared = sdk
                .call(
                    reqwest::Method::POST,
                    "/api/fs/native_offline/desktop/prepare",
                    Some(&json!({"provider":"guangya","request_id":t.id})),
                )
                .await?;
            let stage = prepared["ticket"]
                .as_str()
                .ok_or("电脑版关联尚未准备好")?
                .to_string();
            let result =
                import_target(&state, &t.target, "guangya", prepared["fields"].clone()).await;
            if let Err(e) = result {
                let _ = sdk
                    .call(
                        reqwest::Method::POST,
                        "/api/fs/native_offline/desktop/cancel",
                        Some(&json!({"ticket":stage})),
                    )
                    .await;
                return Err(e);
            }
            let committed = sdk
                .call(
                    reqwest::Method::POST,
                    "/api/fs/native_offline/desktop/commit",
                    Some(&json!({"ticket":stage})),
                )
                .await;
            Ok(Some(if committed.is_ok() {
                format!(
                    "已关联到{}，电脑版登录已移交；文件和设置保留",
                    t.target.label
                )
            } else {
                format!(
                    "已关联到{}；电脑版当前登录有变化，未清除新的登录",
                    t.target.label
                )
            }))
        }
        .await;
        match outcome {
            Ok(Some(message)) => finish(&t, "ready", message),
            Ok(None) => t.working.store(false, Ordering::SeqCst),
            Err(e) => finish(&t, "error", sdk::safe_error(&e)),
        }
    });
}

pub(super) async fn wait_for_provider(
    state: &AppState,
    id: &str,
    origin: &str,
) -> Result<Option<Value>, String> {
    let dest = target(state, origin)?;
    let found = tickets()
        .lock()
        .unwrap()
        .values()
        .filter(|t| {
            t.provider == id
                && t.target.fingerprint == dest.fingerprint
                && now().saturating_sub(t.created) <= TTL
        })
        .max_by_key(|t| t.created)
        .cloned();
    let Some(t) = found else { return Ok(None) };
    wait(&t).await;
    let v = metadata(&t);
    Ok(Some(
        json!({"provider":id,"state":v["state"],"message":v["message"],"target":v["target"],"targetLabel":v["targetLabel"]}),
    ))
}
async fn wait(t: &Ticket) {
    let notify = t.notify.notified();
    tokio::pin!(notify);
    notify.as_mut().enable();
    if matches!(
        t.progress.lock().unwrap().state.as_str(),
        "pending" | "linking"
    ) {
        let _ = tokio::time::timeout(Duration::from_secs(55), notify).await;
    }
}

fn prepare_fields(id: &str, mut fields: Value) -> Result<Value, String> {
    if !fields.is_object() || fields.to_string().len() > 131072 {
        return Err("网页登录信息无效".into());
    }
    if id == "pikpak" {
        let c = fields["c"].as_object().ok_or("请先在PikPak网页完成登录")?;
        let allowed = [
            "access_token",
            "refresh_token",
            "token_type",
            "expires_in",
            "expires_at",
            "sub",
            "user_id",
            "username",
            "email",
        ];
        if c.keys().any(|k| !allowed.contains(&k.as_str()))
            || !["access_token", "refresh_token"]
                .iter()
                .all(|key| fields["c"][*key].as_str().is_some_and(|v| !v.is_empty()))
            || !fields["device"].as_str().is_some_and(|v| !v.is_empty())
        {
            return Err("请先在PikPak网页完成登录".into());
        }
        if !fields["key"]
            .as_str()
            .is_some_and(|k| k.starts_with("credentials_YUMx5nI8ZU8Ap8pm"))
        {
            return Err("PikPak网页登录类型不匹配，请重新登录".into());
        }
    } else {
        let p = provider(id)?;
        if !p.can_link {
            return Err("此网盘暂不支持电脑关联".into());
        }
        let allowed: Vec<&str> = match id {
            "guangya" => vec!["access_token", "refresh_token", "client_id", "device_id"],
            "quark" | "uc" | "pan115" => vec!["cookie"],
            "pan123" => vec!["AccessToken"],
            "mobile" => vec!["authorization", "user_domain_id"],
            _ => return Err("此网盘暂不支持电脑关联".into()),
        };
        if fields.as_object().unwrap().iter().any(|(k, v)| {
            !allowed.contains(&k.as_str())
                || !v.is_string()
                || v.as_str().is_some_and(|s| s.contains(['\r', '\n']))
        }) {
            return Err("网页登录信息不完整，请刷新网盘后重新关联".into());
        }
        let required = match id {
            "quark" | "uc" | "pan115" => vec!["cookie"],
            "pan123" => vec!["AccessToken"],
            "mobile" => vec!["authorization"],
            "guangya" => vec!["access_token", "refresh_token", "client_id", "device_id"],
            _ => Vec::new(),
        };
        if !required
            .iter()
            .all(|key| fields[*key].as_str().is_some_and(|v| !v.trim().is_empty()))
        {
            return Err("请先在网盘网页完成登录".into());
        }
        if id == "guangya" && !matches!(fields["client_id"].as_str(), Some("aMe-8VSlkrbQXpUR" | "aMe_SVSlkrbQXpUT")) {
            return Err("请从光鸭官方网页重新关联".into());
        }
        fields["_computer_link"] = json!(true);
    }
    Ok(fields)
}

async fn import_target(
    state: &AppState,
    dest: &Target,
    id: &str,
    mut fields: Value,
) -> Result<(), String> {
    if !current_target(state, dest) {
        return Err("NAS连接已变化，请重新发起关联".into());
    }
    fields = prepare_fields(id, fields)?;
    if dest.nas {
        #[cfg(feature = "nas")]
        {
            crate::nas::cloud_rpc(
                &state.local,
                reqwest::Method::POST,
                "cloud/associate",
                Some(&json!({"provider":id,"fields":fields})),
            )
            .await?;
            return Ok(());
        }
        #[cfg(not(feature = "nas"))]
        {
            return Err("当前版本未启用NAS".into());
        }
    }
    let c = manager(state);
    if id == "pikpak" {
        let previous = c.accounts.lock().unwrap().get(id).cloned();
        c.account_put(Account {
            provider: id.into(),
            name: "PikPak".into(),
            state: "connecting".into(),
            message: "正在验证电脑关联…".into(),
            updated: now(),
            ..previous.clone().unwrap_or_default()
        })?;
        let result = crate::pikpak::associate_web_login(state, &fields).await;
        if result.is_ok() {
            c.account_put(Account {
                provider: id.into(),
                name: "PikPak".into(),
                state: "ready".into(),
                message: "本机已关联".into(),
                updated: now(),
                ..Default::default()
            })?
        } else if let Some(a) = previous {
            c.account_put(a)?
        } else {
            let mut all = c.accounts.lock().unwrap();
            all.remove(id);
            sdk::private_write(&c.root.join("accounts.json"), &*all)?;
        }
        c.cache.lock().await.at = 0;
        return result;
    }
    let _gate = c.account_gate.lock().await;
    let client = c.sdk().await?;
    let previous = c.accounts.lock().unwrap().get(id).cloned();
    let snapshot = if let Some(a) = previous.as_ref().filter(|a| a.storage_id > 0) {
        Some(
            client
                .call(
                    reqwest::Method::GET,
                    &format!("/api/admin/storage/get?id={}", a.storage_id),
                    None,
                )
                .await?,
        )
    } else {
        None
    };
    let p = provider(id)?;
    let a = Account {
        provider: id.into(),
        name: p.name,
        state: "connecting".into(),
        message: "正在验证电脑关联…".into(),
        updated: now(),
        ..previous.clone().unwrap_or_default()
    };
    c.account_put(a)?;
    let checked: Result<(), String> = async {
        c.account_connect_inner(id, fields).await?;
        if !c
            .accounts
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|a| a.state == "ready")
        {
            return Err("网盘尚未验证这次登录，网页登录已保留".into());
        }
        let limits=client.call(reqwest::Method::POST,"/api/fs/native_offline/storage_limits",Some(&json!({"path":format!("/cloud/{id}")}))).await?;
        if limits["quota_known"]!=true {client.call(reqwest::Method::POST,"/api/fs/list",Some(&json!({"path":format!("/cloud/{id}"),"page":1,"per_page":1,"refresh":true}))).await?;}
        c.preloaded_limits.lock().unwrap().insert(id.into(),(now(),limits));
        Ok(())
    }
    .await;
    if let Err(e) = checked {
        if let Some(old) = snapshot {
            let _ = client
                .call(
                    reqwest::Method::POST,
                    "/api/admin/storage/update",
                    Some(&old),
                )
                .await;
        } else {
            let latest = c.accounts.lock().unwrap().get(id).cloned();
            if let Some(a) = latest.filter(|a| a.storage_id > 0) {
                let _ = client.remove_storage(a.storage_id).await;
            }
        }
        if let Some(old) = previous {
            c.account_put(old)?
        } else {
            let mut data = c.accounts.lock().unwrap();
            data.remove(id);
            sdk::private_write(&c.root.join("accounts.json"), &*data)?;
        }
        return Err(e);
    }
    c.cache.lock().await.at = 0;
    Ok(())
}

fn claim_fields(t: &Ticket, fingerprint: String) -> Result<bool, String> {
    let claimed = {
        let mut p = t.progress.lock().unwrap();
        if let Some(old) = p.fingerprint.as_ref() {
            if old != &fingerprint {
                return Err("这次关联正在处理另一份登录，请完成后重新发起".into());
            }
            false
        } else {
            if p.state != "pending" {
                return Err("关联入口已结束，请重新打开".into());
            }
            p.fingerprint = Some(fingerprint);
            p.state = "linking".into();
            p.message = format!("正在验证{}的账号…", t.target.label);
            true
        }
    };
    Ok(claimed)
}

fn ensure_provider_matches(t: &Ticket, provider_id: &str) -> Result<(), String> {
    if t.provider != provider_id {
        return Err("关联入口与当前网盘不匹配".into());
    }
    Ok(())
}
pub(super) async fn submit(id: &str, provider_id: &str, fields: Value) -> Result<Value, String> {
    let t = get(id)?;
    ensure_provider_matches(&t,provider_id)?;
    let app = app()?;
    let state = app.state::<AppState>();
    if !current_target(&state, &t.target) {
        return Err("NAS连接已变化，请重新打开关联页".into());
    }
    if let Some(token) = &t.resume_token {
        if crate::engine::cloud_refresh_identity(token).is_none() && t.progress.lock().unwrap().state != "ready" {
            return Err("本次续传请求已失效，请回到原任务重新继续".into());
        }
    }
    #[cfg(feature = "nas")]
    if let Some(grant) = &t.official { return poll_official(&state, &t, grant).await; }
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&fields).map_err(|_| "网页登录信息无效")?)
    );
    let start = claim_fields(&t, fingerprint)?;
    if start {
        match import_target(&state, &t.target, provider_id, fields).await {
            Ok(()) => finish(
                &t,
                "ready",
                format!("已关联到{}，可以回软件查看", t.target.label),
            ),
            Err(e) => finish(&t, "error", sdk::safe_error(&e)),
        }
    } else {
        wait(&t).await
    }
    Ok(metadata(&t))
}

pub(super) fn trusted_nas_origin(state: &AppState, origin: &str) -> bool {
    if origin.is_empty() {
        return false;
    }
    #[cfg(feature = "nas")]
    {
        if let Ok((url, _)) = crate::nas::cloud_identity(&state.local) {
            return url::Url::parse(&url)
                .ok()
                .zip(url::Url::parse(origin).ok())
                .is_some_and(|(a, b)| a.origin() == b.origin() && b.path() == "/");
        }
    }
    let _ = state;
    false
}
fn popup_context(state: &AppState, provider_id: &str) -> Result<Value, String> {
    let (_,_,desktop)=login_profile(provider_id)?;
    if desktop { return Err("请在软件中发起关联".into()); }
    let active=tickets().lock().unwrap().values().filter(|t|t.provider==provider_id && t.official.is_none() && now().saturating_sub(t.created)<=TTL && matches!(t.progress.lock().unwrap().state.as_str(),"pending"|"linking")).max_by_key(|t|t.created).cloned();
    match active {
        Some(t) if current_target(state,&t.target)=>Ok(metadata(&t)),
        _=>Ok(json!({"ok":true,"inactive":true,"provider":provider_id})),
    }
}
pub(super) async fn http(
    app: &tauri::AppHandle,
    method: &str,
    path: &str,
    origin: &str,
    body: &[u8],
) -> Value {
    let state = app.state::<AppState>();
    let route = path.split('?').next().unwrap_or("");
    let ext = origin == EXTENSION_ORIGIN;
    let nas = trusted_nas_origin(&state, origin);
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let result: Result<Value, String> = async {
        match route {
            "/extension/cloud-account/popup-context" if method == "POST" && ext => {
                let p=value["provider"].as_str().ok_or("请选择网盘")?;
                popup_context(&state,p)
            }
            "/extension/cloud-account/begin-local" if method == "POST" && ext => {
                let p=value["provider"].as_str().ok_or("请选择网盘")?;
                let (_,_,desktop)=login_profile(p)?;
                if desktop { return Err("请在软件中发起关联".into()); }
                start_request(p.into(),String::new(),false,None).await
            }
            "/extension/cloud-account/start" if method == "POST" && (ext || nas) => {
                let p = value["provider"].as_str().ok_or("请选择网盘")?;
                start(p.into(), "nas:".into()).await
            }
            "/extension/cloud-account/link-context" if matches!(method,"GET"|"POST") && ext => {
                let id = if method=="POST" {value["ticket"].as_str().unwrap_or("").to_string()} else {url::form_urlencoded::parse(
                    path.split_once('?')
                        .map(|(_, q)| q)
                        .unwrap_or("")
                        .as_bytes(),
                )
                .find(|(k, _)| k == "ticket")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default()};
                let t = get(&id)?;
                if !current_target(&state, &t.target) {
                    return Err("NAS连接已变化，请重新打开".into());
                }
                if method == "POST" && value["retry"] == true {
                    let all = tickets().lock().unwrap();
                    if all.values().any(|other| {
                        other.id != t.id && other.provider == t.provider
                            && now().saturating_sub(other.created) <= TTL
                            && (other.created > t.created || matches!(other.progress.lock().unwrap().state.as_str(), "pending" | "linking"))
                    }) {
                        return Err("关联请求已更新，请回闪电使用最新的关联入口".into());
                    }
                    let mut progress = t.progress.lock().unwrap();
                    if progress.state == "error" {
                        progress.state = "pending".into();
                        progress.message.clear();
                        progress.fingerprint = None;
                    }
                }
                Ok(metadata(&t))
            }
            "/extension/cloud-account/link-result" if method == "GET" && (ext || nas) => {
                let id = url::form_urlencoded::parse(
                    path.split_once('?')
                        .map(|(_, q)| q)
                        .unwrap_or("")
                        .as_bytes(),
                )
                .find(|(k, _)| k == "ticket")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
                let t = get(&id)?;
                if nas && !t.target.nas {
                    return Err("关联来源不匹配".into());
                }
                wait(&t).await;
                Ok(metadata(&t))
            }
            "/extension/cloud-account/link" if method == "POST" && ext => {
                submit(
                    value["ticket"].as_str().unwrap_or(""),
                    value["provider"].as_str().unwrap_or(""),
                    value["fields"].clone(),
                )
                .await
            }
            _ => Err("请从已配置的云盘设置发起电脑关联".into()),
        }
    }
    .await;
    match result {
        Ok(v) => v,
        Err(e) => json!({"ok":false,"error":sdk::safe_error(&e),"state":"error"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ticket() -> Arc<Ticket> {
        Arc::new(Ticket {
            id: "a".repeat(32),
            provider: "quark".into(),
            target: Target {
                nas: false,
                fingerprint: "local".into(),
                label: "本机".into(),
            },
            login_url: "https://pan.quark.cn/".into(),
            transfer: false,
            resume_token: None,
            official: None,
            created: now(),
            progress: Mutex::new(Progress {
                state: "pending".into(),
                message: String::new(),
                fingerprint: None,
            }),
            notify: tokio::sync::Notify::new(),
            working: AtomicBool::new(false),
            approved: AtomicBool::new(false),
        })
    }
    #[test]
    fn credentials_stay_private_and_replay_is_single_use() {
        let t = ticket();
        assert!(claim_fields(&t, "synthetic-one".into()).unwrap());
        assert!(!claim_fields(&t, "synthetic-one".into()).unwrap());
        assert!(claim_fields(&t, "another-login".into()).is_err());
        let public = metadata(&t).to_string();
        assert!(!public.contains("synthetic-one"));
        assert!(!public.contains("fields"));
        finish(&t, "cancelled", "cancelled".into());
        finish(&t, "ready", "must not override cancellation".into());
        assert_eq!(metadata(&t)["state"], "cancelled");
    }
    #[test]
    fn desktop_is_never_read_before_user_confirmation() {
        let t = ticket();
        spawn_desktop(t.clone());
        assert!(!t.working.load(Ordering::SeqCst));
        assert_eq!(metadata(&t)["state"], "pending");
    }
    #[test]
    fn only_expected_computer_login_fields_are_accepted() {
        assert!(prepare_fields("quark", json!({"cookie":"synthetic"})).is_ok());
        assert!(prepare_fields("quark", json!({"cookie":""})).is_err());
        assert!(prepare_fields(
            "quark",
            json!({"cookie":"synthetic","password":"synthetic"})
        )
        .is_err());
        assert!(prepare_fields("pan123", json!({"AccessToken":"synthetic"})).is_ok());
        assert!(prepare_fields("guangya",json!({"client_id":"aMe-8VSlkrbQXpUR","access_token":"synthetic-access","refresh_token":"synthetic-refresh","device_id":"0123456789abcdef0123456789abcdef"})).is_ok());
        assert!(prepare_fields("pan123", json!({"access_token":"wrong-case"})).is_err());
        assert!(prepare_fields(
            "guangya",
            json!({"client_id":"web","access_token":"x","refresh_token":"y","device_id":"z"})
        )
        .is_err());
    }
    #[test]
    fn browser_submit_uses_the_same_provider_rule_for_guangya_and_pikpak() {
        let mut t=Arc::try_unwrap(ticket()).ok().expect("unique test ticket");
        for provider in ["pikpak","guangya","quark","uc","pan115","pan123","mobile"] {
            t.provider=provider.into();
            assert!(ensure_provider_matches(&t,provider).is_ok());
            assert!(ensure_provider_matches(&t,"different-provider").is_err());
        }
    }
    #[test]
    fn ticket_and_profile_boundaries() {
        assert!(!valid_ticket("other"));
        assert!(!valid_ticket(&"g".repeat(32)));
        assert!(valid_ticket(&"a".repeat(32)));
        assert!(login_profile("pikpak").unwrap().1);
        assert!(!login_profile("guangya").unwrap().2);
        assert_eq!(login_profile("guangya").unwrap().0,"https://www.guangyapan.com/");
        assert!(login_profile("baidu").is_err());
        assert!(login_profile("quark").is_ok());
    }
}
