use crate::{
    api::{Api, Task},
    AppState,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::State;
#[path = "cloud_device.rs"]
mod device;
#[path = "cloud_jobs.rs"]
mod jobs;
#[path = "cloud_link.rs"]
mod linking;
#[path = "cloud_sdk.rs"]
mod sdk;
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
struct TaskControl {
    epoch: AtomicU64,
    gate: Arc<tokio::sync::Mutex<()>>,
}
fn task_control(id: &str) -> Arc<TaskControl> {
    static CONTROLS: OnceLock<Mutex<HashMap<String, Arc<TaskControl>>>> = OnceLock::new();
    CONTROLS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
        .entry(id.into())
        .or_insert_with(|| {
            Arc::new(TaskControl {
                epoch: AtomicU64::new(0),
                gate: Arc::new(tokio::sync::Mutex::new(())),
            })
        })
        .clone()
}
// Mark user intent before waiting for an in-flight commit; then serialize the
// kernel command so an automatic renewal cannot undo a pause or deletion.
pub async fn user_task_control(id: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let control = task_control(id);
    control.epoch.fetch_add(1, Ordering::SeqCst);
    control.gate.clone().lock_owned().await
}
pub fn init(app: tauri::AppHandle) {
    let _ = APP.set(app);
}

fn public_providers() -> Vec<Value> {
    let mut list = vec![
        json!({"id":"pikpak","name":"PikPak","canLink":true,"linkMethod":"browser","offlineTool":"PikPak","linkMessage":""}),
    ];
    list.extend(providers().iter().map(|p| json!(p)));
    list
}

#[tauri::command]
pub async fn cloud_account_link(provider: String, origin: Option<String>) -> Result<Value, String> {
    linking::start(provider, origin.unwrap_or_default()).await
}

#[tauri::command]
pub async fn cloud_resume_account_link(token: String) -> Result<Value, String> {
    let (kind, origin, fingerprint) = crate::engine::cloud_refresh_identity(&token).ok_or("续传请求已过期")?;
    let matching = providers().iter().find(|p| p.name == kind && p.can_link && p.link_method == "browser");
    let Some(provider) = matching else {
        return Err("当前网盘不支持在网页关联后续传".into());
    };
    linking::start_resume(provider.id.clone(), origin, token, fingerprint).await
}

pub(crate) fn refresh_target_fingerprint(state: &AppState, origin: &str) -> Result<String, String> {
    linking::target_fingerprint(state, origin)
}

#[tauri::command]
pub async fn cloud_account_remove(
    provider: String,
    origin: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let origin = origin.unwrap_or_default();
    linking::invalidate(&state, &provider, &origin)?;
    if origin.starts_with("nas:") {
        #[cfg(feature = "nas")]
        {
            let path = if provider == "pikpak" {
                "pikpak".into()
            } else {
                format!(
                    "cloud/accounts?provider={}",
                    url::form_urlencoded::byte_serialize(provider.as_bytes()).collect::<String>()
                )
            };
            crate::nas::cloud_rpc(&state.local, reqwest::Method::DELETE, &path, None).await?;
            return Ok(());
        }
        #[cfg(not(feature = "nas"))]
        {
            return Err("当前版本未启用NAS".into());
        }
    }
    let c = manager(&state);
    if provider == "pikpak" {
        crate::pikpak::pikpak_logout(state.clone()).await?;
        c.cache.lock().await.at = 0;
        return Ok(());
    }
    let _gate = c.account_gate.lock().await;
    let account = c.accounts.lock().unwrap().get(&provider).cloned();
    if let Some(a) = account {
        if a.storage_id > 0 {
            c.sdk().await?.remove_storage(a.storage_id).await?;
        }
        let mut all = c.accounts.lock().unwrap();
        let mut remaining = all.clone();
        remaining.remove(&provider);
        sdk::private_write(&c.root.join("accounts.json"), &remaining)?;
        *all = remaining;
    }
    c.cache.lock().await.at = 0;
    Ok(())
}

pub fn account_link_ping(value: &Value) {
    linking::register_extension(value)
}
pub fn account_link_legacy_busy(provider: &str) -> bool {
    linking::legacy_busy(provider)
}
pub async fn account_link_http(
    app: &tauri::AppHandle,
    method: &str,
    path: &str,
    origin: &str,
    body: &[u8],
) -> Value {
    linking::http(app, method, path, origin, body).await
}
async fn pause_origin(api: &Api, id: &str) -> Result<(), String> {
    if let Some(id) = id.strip_prefix("xl:") {
        use tauri::Manager;
        let app = APP.get().ok_or("应用尚未就绪")?;
        let state = app.state::<AppState>();
        crate::thunder::set_paused(&state, id, true).await
    } else {
        api.pause(id).await.map_err(|e| e.to_string())
    }
}
pub async fn renew_pull(
    api: &Api,
    t: &Task,
    url: &str,
    headers: &[(String, String)],
    size: u64,
) -> Result<(), String> {
    jobs::renew(api, t, url, headers, size).await
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub driver: String,
    #[serde(skip_serializing)]
    pub fields: Vec<String>,
    pub note: String,
    #[serde(default)]
    pub offline_tool: String,
    #[serde(default)]
    pub protocols: Vec<String>,
    #[serde(default)]
    pub device_login: bool,
    #[serde(default)]
    pub can_link: bool,
    #[serde(default)]
    pub link_method: String,
    #[serde(default)]
    pub login_url: String,
    #[serde(default)]
    pub transfer_session: bool,
    #[serde(default)]
    pub link_message: String,
}
fn providers() -> &'static Vec<Provider> {
    static P: OnceLock<Vec<Provider>> = OnceLock::new();
    P.get_or_init(|| {
        serde_json::from_str(include_str!("cloud-providers.json"))
            .expect("cloud provider specification")
    })
}
fn provider(id: &str) -> Result<Provider, String> {
    providers()
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| "不支持的网盘".into())
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Account {
    pub provider: String,
    pub name: String,
    pub storage_id: u64,
    pub state: String,
    pub message: String,
    pub channel: String,
    pub updated: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Source {
    pub id: String,
    pub name: String,
    pub logged_in: bool,
    pub account_state: String,
    pub protocols: Vec<String>,
    pub quota_known: bool,
    pub total_bytes: Option<u64>,
    pub used_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub single_file_limit: Option<u64>,
    pub offline_quota: Option<Value>,
    pub limit_note: String,
    pub warning: String,
    pub queried_at: u64,
    #[serde(default)]
    pub can_offline: bool,
    #[serde(default)] pub can_link:bool,
    pub suitable: bool,
    pub reason: String,
}
impl Source {
    fn apply_capacity(&mut self, value: &Value) {
        self.total_bytes = value["total_bytes"].as_u64();
        self.used_bytes = value["used_bytes"].as_u64();
        self.free_bytes = match (self.total_bytes, self.used_bytes) {
            (Some(total), Some(used)) if used > total => Some(0),
            _ => value["free_bytes"].as_u64(),
        };
        self.quota_known = value["quota_known"] == true && self.free_bytes.is_some();
    }

    fn checked(mut self, size: u64, scheme: &str) -> Self {
        self.suitable = false;
        self.reason = if self.account_state == "expired" {
            "关联失效，请重新关联".into()
        } else if self.account_state == "unknown" {
            "账号状态查询失败，请刷新".into()
        } else if !self.logged_in {
            "请先关联账号".into()
        } else if self.account_state != "ready" {
            "账号尚未就绪".into()
        } else if !self.can_offline {
            "暂不支持离线下载".into()
        } else if !self.protocols.iter().any(|p| p == scheme) {
            "当前接入不支持此链接类型".into()
        } else if !self.quota_known {
            "容量暂未查询成功，请刷新后选择".into()
        } else if self.free_bytes.unwrap_or(0) == 0
            || (size > 0 && self.free_bytes.is_some_and(|n| size > n))
        {
            "剩余空间不足".into()
        } else if size > 0 && self.single_file_limit.is_some_and(|n| size > n) {
            "超过服务商返回的单文件上限".into()
        } else if self
            .offline_quota
            .as_ref()
            .and_then(|v| v["remaining"].as_i64())
            == Some(0)
        {
            "服务商返回的离线额度已用完".into()
        } else {
            self.suitable = true;
            if size == 0 {
                "大小待云端解析".into()
            } else {
                String::new()
            }
        };
        self
    }
}

struct Cache {
    at: u64,
    identity: String,
    sources: Vec<Source>,
}
pub struct Cloud {
    root: PathBuf,
    runtime: tokio::sync::Mutex<Option<Arc<sdk::Sdk>>>,
    accounts: Mutex<HashMap<String, Account>>,
    account_signals: Mutex<HashMap<String, Arc<tokio::sync::Notify>>>,
    account_gate: tokio::sync::Mutex<()>,
    entries: Mutex<Vec<jobs::Job>>,
    cache: tokio::sync::Mutex<Cache>,
    preloaded_limits: Mutex<HashMap<String, (u64, Value)>>,
    stepping: AtomicBool,
    next: AtomicU64,
    control: tokio::sync::Mutex<()>,
}
static CLOUD: OnceLock<Cloud> = OnceLock::new();
fn read_json<T: serde::de::DeserializeOwned + Default>(p: &Path) -> T {
    std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
fn manager(state: &AppState) -> &'static Cloud {
    CLOUD.get_or_init(|| {
        let root = state.local.join("state/cloud");
        let mut accounts: HashMap<String, Account> = read_json(&root.join("accounts.json"));
        for a in accounts.values_mut() {
            if a.state == "connecting" {
                a.state = "error".into();
                a.message = "上次登录被中断，请重新保存".into()
            }
        }
        Cloud {
            entries: Mutex::new(read_json(&root.join("jobs.json"))),
            accounts: Mutex::new(accounts),
            account_signals: Mutex::new(HashMap::new()),
            account_gate: tokio::sync::Mutex::new(()),
            root,
            runtime: tokio::sync::Mutex::new(None),
            cache: tokio::sync::Mutex::new(Cache {
                at: 0,
                identity: String::new(),
                sources: Vec::new(),
            }),
            preloaded_limits: Mutex::new(HashMap::new()),
            stepping: AtomicBool::new(false),
            next: AtomicU64::new(0),
            control: tokio::sync::Mutex::new(()),
        }
    })
}
impl Cloud {
    async fn sdk(&self) -> Result<Arc<sdk::Sdk>, String> {
        let mut slot = self.runtime.lock().await;
        if let Some(s) = slot.as_ref() {
            if s.alive() {
                return Ok(s.clone());
            }
        }
        *slot = None;
        let data = self
            .root
            .parent()
            .and_then(Path::parent)
            .ok_or("本机数据目录不可用")?
            .join("auth/cloud-driver");
        let s = sdk::start(data).await?;
        *slot = Some(s.clone());
        Ok(s)
    }
    fn account_signal(&self, id: &str) -> Arc<tokio::sync::Notify> {
        self.account_signals
            .lock()
            .unwrap()
            .entry(id.into())
            .or_insert_with(|| Arc::new(tokio::sync::Notify::new()))
            .clone()
    }
    fn account_put(&self, a: Account) -> Result<(), String> {
        self.preloaded_limits.lock().unwrap().remove(&a.provider);
        let signal = self.account_signal(&a.provider);
        let mut data = self.accounts.lock().unwrap();
        data.insert(a.provider.clone(), a);
        sdk::private_write(&self.root.join("accounts.json"), &*data)?;
        signal.notify_waiters();
        Ok(())
    }
    async fn account_connect(&self, id: &str, fields: Value) -> Result<(), String> {
        let _gate = self.account_gate.lock().await;
        self.account_connect_inner(id, fields).await
    }
    async fn account_connect_inner(&self, id: &str, fields: Value) -> Result<(), String> {
        let p = provider(id)?;
        let client = self.sdk().await?;
        let mut a = self
            .accounts
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("账号状态不可用")?;
        let info = client
            .call(
                reqwest::Method::GET,
                &format!(
                    "/api/admin/driver/info?driver={}",
                    url::form_urlencoded::byte_serialize(p.driver.as_bytes()).collect::<String>()
                ),
                None,
            )
            .await?;
        let mut addition = json!({});
        for item in info["additional"].as_array().into_iter().flatten() {
            let key = item["name"].as_str().unwrap_or("");
            let value = item["default"].as_str().unwrap_or("");
            if value.is_empty() {
                continue;
            }
            addition[key] = match item["type"].as_str() {
                Some("bool") => json!(value == "true"),
                Some("number") => json!(value.parse::<i64>().unwrap_or(0)),
                _ => json!(value),
            }
        }
        let computer = fields["_computer_link"].as_bool() == Some(true);
        let mut storage = if a.storage_id > 0 {
            client
                .call(
                    reqwest::Method::GET,
                    &format!("/api/admin/storage/get?id={}", a.storage_id),
                    None,
                )
                .await?
        } else {
            json!({"mount_path":format!("/cloud/{id}"),"driver":p.driver,"cache_expiration":1,"disable_index":true,"web_proxy":false,"webdav_policy":"native_proxy"})
        };
        if let Some(s) = storage["addition"].as_str() {
            if let Ok(old) = serde_json::from_str::<Value>(s) {
                if !computer {
                    addition = old
                }
            }
        }
        if id == "guangya" && addition["client_id"].as_str().unwrap_or("").is_empty() {
            addition["client_id"] = json!("aMe-8VSlkrbQXpUR")
        }
        for key in p
            .fields
            .iter()
            .map(String::as_str)
            .chain(["device_id", "device_sign"])
        {
            if let Some(v) = fields.get(key) {
                if !v.as_str().is_some_and(str::is_empty) {
                    addition[key] = v.clone()
                }
            }
        }
        addition["use_online_api"] = json!(false);
        if let Some(m) = addition.as_object_mut() {
            m.remove("api_url_address");
        }
        if id == "baidu" {
            addition["download_api"] = json!("official")
        }
        if computer {
            storage["driver"] = json!(p.driver);
            if id == "pan123" {
                addition["preserve_web_session"] = json!(true);
            }
        }
        storage["addition"] = json!(addition.to_string());
        let endpoint = if a.storage_id == 0 {
            "/api/admin/storage/create"
        } else {
            "/api/admin/storage/update"
        };
        let created = client
            .call(reqwest::Method::POST, endpoint, Some(&storage))
            .await;
        match created {
            Ok(v) => {
                if let Some(n) = v["id"].as_u64() {
                    a.storage_id = n
                }
            }
            Err(e) => {
                if a.storage_id == 0 {
                    if let Ok(v) = client
                        .call(
                            reqwest::Method::GET,
                            "/api/admin/storage/list?page=1&per_page=100",
                            None,
                        )
                        .await
                    {
                        if let Some(v) = v["content"].as_array().and_then(|l| {
                            l.iter().find(|v| v["mount_path"] == format!("/cloud/{id}"))
                        }) {
                            a.storage_id = v["id"].as_u64().unwrap_or(0);
                            self.account_put(a.clone())?;
                        }
                    }
                }
                return Err(e);
            }
        }
        self.account_put(a.clone())?;
        let check = client
            .call(
                reqwest::Method::GET,
                &format!("/api/admin/storage/get?id={}", a.storage_id),
                None,
            )
            .await?;
        a.state = if check["status"]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("work"))
        {
            "ready"
        } else {
            "verify"
        }
        .into();
        a.message = if a.state == "ready" {
            "账号已关联"
        } else {
            "等待服务商验证，请补充验证码后保存"
        }
        .into();
        a.updated = now();
        if id == "guangya" && a.state == "ready" {
            let session: Value = serde_json::from_str(check["addition"].as_str().unwrap_or("{}"))
                .unwrap_or_default();
            if session["access_token"].as_str().unwrap_or("").is_empty()
                || session["refresh_token"].as_str().unwrap_or("").is_empty()
            {
                a.state = "verify".into();
                a.message = "请完成账号验证，才能独立续期".into();
            }
        }
        if id == "guangya" {
            a.channel = if addition["client_id"] == "aMe_SVSlkrbQXpUT" {
                "client"
            } else {
                "web"
            }
            .into()
        }
        self.account_put(a)?;
        self.cache.lock().await.at = 0;
        Ok(())
    }
}

#[tauri::command]
pub async fn cloud_accounts(
    provider: Option<String>,
    refresh: Option<bool>,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let c = manager(&state);
    let stored = c.accounts.lock().unwrap().clone();
    let sources = if let Some(id) = provider.as_deref() {
        read_cloud_sources(c, &stored, crate::pikpak::logged_in(&state), refresh, state, Some(id))
            .await
            .into_iter()
            .map(|source| source.checked(0, "https"))
            .collect()
    } else {
        cloud_sources(Some(0), "https".into(), refresh, state).await?
    };
    if let Some(app) = APP.get() {
        for source in &sources {
            crate::float::update_cloud_account_notice(app, &source.id, &source.name, false, &source.account_state);
        }
    }
    let accounts: Vec<Account> = sources
        .clone()
        .into_iter()
        .map(|source| {
            let mut a = stored.get(&source.id).cloned().unwrap_or_default();
            a.provider = source.id;
            a.name = source.name;
            a.state = source.account_state;
            a.message = if !source.warning.is_empty() {
                source.warning
            } else {
                match a.state.as_str() {
                    "ready" => "本机已关联".into(),
                    "expired" => "本机登录已失效，请在电脑重新关联".into(),
                    "unlinked" => "本机尚未关联".into(),
                    _ => a.message,
                }
            };
            a
        })
        .collect();
    Ok(
        json!({"providers":public_providers(),"accounts":accounts,"sources":sources,"scope":"local"}),
    )
}

#[tauri::command]
pub async fn cloud_account_save(
    provider: String,
    fields: Value,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let p = self::provider(&provider)?;
    let c = manager(&state);
    let mut a = c
        .accounts
        .lock()
        .unwrap()
        .get(&provider)
        .cloned()
        .unwrap_or_default();
    if a.state == "connecting" {
        return Err("正在关联，请稍候".into());
    }
    a.provider = provider.clone();
    a.name = p.name;
    a.state = "connecting".into();
    a.message = "正在验证账号…".into();
    a.updated = now();
    c.account_put(a.clone())?;
    tauri::async_runtime::spawn(async move {
        if let Err(e) = c.account_connect(&provider, fields).await {
            let mut a = c
                .accounts
                .lock()
                .unwrap()
                .get(&provider)
                .cloned()
                .unwrap_or_default();
            a.state = "error".into();
            a.message = e;
            a.updated = now();
            let _ = c.account_put(a);
        }
    });
    Ok(json!(a))
}

#[tauri::command]
pub async fn cloud_account_result(
    provider: String,
    origin: Option<String>,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    if let Some(result) =
        linking::wait_for_provider(&state, &provider, origin.as_deref().unwrap_or("")).await?
    {
        return Ok(result);
    }
    if origin.as_deref().unwrap_or("").starts_with("nas:") {
        #[cfg(feature = "nas")]
        {
            return crate::nas::cloud_rpc(
                &state.local,
                reqwest::Method::GET,
                &format!("cloud/account-result?provider={}", provider),
                None,
            )
            .await;
        }
        #[cfg(not(feature = "nas"))]
        {
            return Err("当前版本未启用NAS".into());
        }
    }
    if provider == "pikpak" {
        let sources = cloud_sources(Some(0), "https".into(), Some(false), state.clone()).await?;
        let source = sources
            .iter()
            .find(|s| s.id == "pikpak")
            .ok_or("PikPak状态暂不可用")?;
        return Ok(
            json!({"provider":"pikpak","state":source.account_state,"message":if source.account_state=="ready"{"本机已关联"}else{"本机尚未关联或登录已失效，请在电脑关联"}}),
        );
    }
    let c = manager(&state);
    let signal = c.account_signal(&provider);
    let changed = signal.notified();
    tokio::pin!(changed);
    changed.as_mut().enable();
    let a = c
        .accounts
        .lock()
        .unwrap()
        .get(&provider)
        .cloned()
        .ok_or("账号不存在")?;
    if a.state != "connecting" {
        return Ok(json!(a));
    }
    let _ = tokio::time::timeout(Duration::from_secs(55), changed).await;
    let a = c
        .accounts
        .lock()
        .unwrap()
        .get(&provider)
        .cloned()
        .ok_or("账号不存在")?;
    Ok(json!(a))
}

#[tauri::command]
pub async fn cloud_sources(
    size: Option<u64>,
    scheme: String,
    refresh: Option<bool>,
    state: State<'_, AppState>,
) -> Result<Vec<Source>, String> {
    let requested = now();
    let c = manager(&state);
    let pp = crate::pikpak::logged_in(&state);
    let accounts = c.accounts.lock().unwrap().clone();
    let mut account_keys = accounts
        .iter()
        .map(|(k, a)| (k, a.updated, &a.state))
        .collect::<Vec<_>>();
    account_keys.sort_by_key(|v| v.0);
    let identity = format!(
        "{}:{}:{:?}",
        pp,
        crate::pikpak::login_revision(&state),
        account_keys
    );
    let mut cache = c.cache.lock().await;
    if (refresh.unwrap_or(false) && cache.at < requested)
        || cache.identity != identity
        || now().saturating_sub(cache.at) > 45
    {
        cache.sources = read_cloud_sources(c, &accounts, pp, refresh, state, None).await;
        cache.at = now();
        cache.identity = identity;
    }
    Ok(cache
        .sources
        .iter()
        .cloned()
        .map(|s| s.checked(size.unwrap_or(0), &scheme))
        .collect())
}

async fn read_cloud_sources(
    c: &Cloud,
    accounts: &HashMap<String, Account>,
    pp: bool,
    refresh: Option<bool>,
    state: State<'_, AppState>,
    only: Option<&str>,
) -> Vec<Source> {
    let mut sources = Vec::new();
    if only.is_none_or(|id| id == "pikpak") {
        let mut pp_source = Source {
            can_offline: true,
            can_link:true,
            id: "pikpak".into(),
            name: "PikPak".into(),
            logged_in: pp,
            account_state: if pp { "ready" } else { "unlinked" }.into(),
            protocols: vec!["http".into(), "https".into(), "magnet".into()],
            limit_note: "单文件和离线次数以服务商返回为准".into(),
            queried_at: now(),
            ..Default::default()
        };
        if accounts
            .get("pikpak")
            .is_some_and(|a| a.state == "connecting")
        {
            pp_source.account_state = "connecting".into();
        }
        if pp && pp_source.account_state == "ready" {
            match crate::pikpak::pikpak_quota(state.clone()).await {
                Ok(Some((total, used))) => {
                    pp_source.quota_known = true;
                    pp_source.total_bytes = Some(total);
                    pp_source.used_bytes = Some(used);
                    pp_source.free_bytes = Some(total.saturating_sub(used));
                }
                Err(e) => {
                    pp_source.warning = sdk::safe_error(&e);
                    if e.contains("过期") || e.contains("重新登录") || e.contains("没有登录")
                    {
                        pp_source.account_state = "expired".into();
                        pp_source.logged_in = false;
                    }
                }
                _ => pp_source.warning = "PikPak 容量暂未查询成功".into(),
            }
        }
        sources.push(pp_source);
    }
    let needs_driver = accounts.iter().any(|(id, account)| {
        id != "pikpak" && account.state == "ready" && only.is_none_or(|wanted| wanted == id)
    });
    let client = if needs_driver {
        c.sdk().await.ok()
    } else {
        None
    };
    let mut pending = Vec::new();
    for p in providers().iter().filter(|p| only.is_none_or(|id| id == p.id)) {
        let a = accounts.get(&p.id);
        let mut source = Source {
            can_offline: !p.offline_tool.is_empty(),
            can_link:p.can_link,
            id: p.id.clone(),
            name: p.name.clone(),
            logged_in: a.is_some(),
            account_state: a
                .map(|a| a.state.clone())
                .unwrap_or_else(|| "unlinked".into()),
            protocols: p.protocols.clone(),
            queried_at: now(),
            limit_note: "服务商未提供的单文件/离线限制显示为未知".into(),
            ..Default::default()
        };
        if p.id == "guangya" && a.is_some_and(|a| a.channel != "client") {
            source.warning = "网页授权可能无法拉回大文件，请使用客户端授权".into()
        }
        if let Some(client) = client
            .clone()
            .filter(|_| a.is_some_and(|a| a.state == "ready"))
        {
            let p = p.clone();
            let preloaded = c
                .preloaded_limits
                .lock()
                .unwrap()
                .get(&p.id)
                .cloned()
                .filter(|(at, _)| !refresh.unwrap_or(false) && now().saturating_sub(*at) < 15)
                .map(|(_, value)| value);
            pending.push(tauri::async_runtime::spawn(async move {
                let answer = if let Some(value) = preloaded {
                    Ok(value)
                } else {
                    client
                        .call(
                            reqwest::Method::POST,
                            "/api/fs/native_offline/storage_limits",
                            Some(&json!({"path":format!("/cloud/{}",p.id)})),
                        )
                        .await
                };
                match &answer {
                    Ok(v) => {
                        source.apply_capacity(v);
                        source.single_file_limit = v["single_file_limit"].as_u64();
                        source.offline_quota = if v["offline_quota"].is_object() {
                            Some(v["offline_quota"].clone())
                        } else {
                            None
                        };
                        if let Some(note) = v["limit_note"].as_str() {
                            source.limit_note = note.into()
                        }
                    }
                    Err(e) => source.warning = e.clone(),
                };
                if !source.quota_known {
                    let probe = if answer.as_ref().err().is_some_and(|e| e == sdk::ACCOUNT_EXPIRED) {
                        Err(sdk::ACCOUNT_EXPIRED.to_string())
                    } else {
                        client.call(
                            reqwest::Method::POST,
                            "/api/fs/list",
                            Some(&json!({"path":format!("/cloud/{}",p.id),"page":1,"per_page":1,"refresh":true})),
                        ).await.map(|_| ())
                    };
                    match probe {
                        Ok(()) => source.warning = "容量未能查询成功".into(),
                        Err(error) if error == sdk::ACCOUNT_EXPIRED => {
                            source.account_state = "expired".into();
                            source.logged_in = false;
                            source.warning = error;
                        }
                        Err(_) => {
                            source.account_state = "unknown".into();
                            source.warning = "暂时无法核实账号状态，请刷新".into();
                        }
                    }
                }
                source
            }));
        } else {
            sources.push(source)
        }
    }
    for task in pending {
        if let Ok(source) = task.await {
            sources.push(source)
        }
    }
    sources
}

fn kernel(state: &AppState) -> Api {
    let e = state.engine.lock().unwrap();
    Api::new(e.port, e.token.clone())
}
fn source_link(t: &Task) -> Result<String, String> {
    let u = t.url();
    if u.starts_with("magnet:")
        || u.starts_with("http://")
        || u.starts_with("https://")
        || u.starts_with("ed2k:")
    {
        return Ok(u);
    }
    let hash = t
        .meta
        .as_ref()
        .and_then(|m| m.res.as_ref())
        .map(|r| r.hash.trim())
        .unwrap_or("");
    if t.protocol.as_deref() == Some("bt") && !hash.is_empty() {
        return Ok(format!("magnet:?xt=urn:btih:{hash}"));
    }
    Err("这条任务暂时没有可交给云盘的链接".into())
}

#[tauri::command]
pub async fn cloud_offline(
    provider: String,
    origin: String,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let api = kernel(&state);
    let task = if origin.starts_with("xl:") {
        crate::thunder::jobs_as_tasks_matching(&state, None)
            .into_iter()
            .find(|t| t.id == origin)
    } else {
        api.list_tasks()
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|t| t.id == origin)
    }
    .ok_or("原任务不存在")?;
    if task.status == "done" {
        return Err("原任务已经完成".into());
    }
    let link = source_link(&task)?;
    let scheme = link.split(':').next().unwrap_or("").to_lowercase();
    let sources = cloud_sources(Some(task.total()), scheme, false.into(), state.clone()).await?;
    let source = sources
        .iter()
        .find(|s| s.id == provider)
        .ok_or("不支持此云盘离线来源")?;
    if !source.suitable {
        return Err(source.reason.clone());
    }
    if provider == "pikpak" {
        let message = crate::pikpak::submit_task(&state, &link, &task).await?;
        if matches!(task.status.as_str(), "running" | "ready" | "wait") {
            pause_origin(&api, &origin).await?;
        }
        return Ok(json!({"legacy":true,"message":message}));
    }
    let c = manager(&state);
    jobs::submit(c, api, task, &provider, &link).await
}

pub async fn tick(state: &AppState, api: &Api) {
    linking::tick();
    if CLOUD.get().is_none() && !state.local.join("state/cloud/jobs.json").is_file() {
        return;
    }
    let c = manager(state);
    if now() < c.next.load(Ordering::Relaxed) || c.stepping.swap(true, Ordering::SeqCst) {
        return;
    }
    c.next.store(now() + 15, Ordering::Relaxed);
    let api = api.clone();
    tauri::async_runtime::spawn(async move {
        jobs::step(c, &api).await;
        c.stepping.store(false, Ordering::SeqCst);
    });
}
pub fn views(state: &AppState, key: Option<&str>) -> Vec<Task> {
    if let Some(c) = CLOUD.get() {
        jobs::views(c, key)
    } else {
        let _ = state;
        Vec::new()
    }
}
pub fn hides(state: &AppState, id: &str) -> bool {
    if let Some(c) = CLOUD.get() {
        jobs::hides(c, id)
    } else {
        let _ = state;
        false
    }
}
pub async fn shutdown() {
    if let Some(c) = CLOUD.get() {
        *c.runtime.lock().await = None;
    }
}
#[tauri::command]
pub async fn cloud_job_control(
    id: String,
    action: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    jobs::control(manager(&state), &kernel(&state), &id, &action).await
}
pub async fn control(state: &AppState, id: &str, action: &str) -> Result<(), String> {
    jobs::control(manager(state), &kernel(state), id, action).await
}

async fn commit_device(c: &Cloud, fields: Value) -> Result<(), String> {
    let _gate = c.account_gate.lock().await;
    let previous = c.accounts.lock().unwrap().get("guangya").cloned();
    let client = c.sdk().await?;
    let storage = if let Some(a) = previous.as_ref().filter(|a| a.storage_id > 0) {
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
    let a = Account {
        provider: "guangya".into(),
        name: "光鸭云盘".into(),
        state: "connecting".into(),
        message: "正在保存客户端授权…".into(),
        updated: now(),
        ..previous.clone().unwrap_or_default()
    };
    c.account_put(a)?;
    let outcome = c.account_connect_inner("guangya", fields).await;
    let ready = c
        .accounts
        .lock()
        .unwrap()
        .get("guangya")
        .is_some_and(|a| a.state == "ready" && a.channel == "client");
    if outcome.is_ok() && ready {
        return Ok(());
    }
    if let Some(storage) = storage {
        client
            .call(
                reqwest::Method::POST,
                "/api/admin/storage/update",
                Some(&storage),
            )
            .await
            .map_err(|_| "客户端授权保存失败，旧授权恢复失败，请重新关联")?;
    }
    if let Some(a) = previous {
        c.account_put(a)?
    } else {
        let mut a = c
            .accounts
            .lock()
            .unwrap()
            .get("guangya")
            .cloned()
            .unwrap_or_default();
        a.state = "error".into();
        a.message = "客户端授权未能保存，请重新关联".into();
        c.account_put(a)?
    }
    Err("客户端授权未能保存，旧账号已保留".into())
}
#[tauri::command]
pub async fn cloud_device_start(
    provider: String,
    state: State<'_, AppState>,
) -> Result<device::DeviceStart, String> {
    let c = manager(&state);
    let client = c.sdk().await?;
    let mut login = device::start(&provider, move |fields| async move {
        commit_device(c, fields).await
    })
    .await?;
    match client
        .call(
            reqwest::Method::POST,
            "/api/fs/native_offline/qr",
            Some(&json!({"text":login.verification_uri})),
        )
        .await
    {
        Ok(v) => {
            login.qr_image = v["image"]
                .as_str()
                .filter(|s| s.starts_with("data:image/png;base64,"))
                .ok_or("二维码加载失败，请重试")?
                .into();
            Ok(login)
        }
        Err(e) => {
            let _ = device::cancel(&login.id);
            Err(e)
        }
    }
}
#[tauri::command]
pub async fn cloud_device_result(id: String) -> Result<device::DeviceResult, String> {
    device::result(&id).await
}
#[tauri::command]
pub fn cloud_device_cancel(id: String) -> Result<(), String> {
    device::cancel(&id)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> Source {
        Source {
            id: "test".into(),
            logged_in: true,
            account_state: "ready".into(),
            protocols: vec!["http".into()],
            can_offline: true,
            quota_known: true,
            free_bytes: Some(100),
            ..Default::default()
        }
    }
    #[test]
    fn suitability_uses_known_capacity_and_protocol() {
        assert!(source().checked(50, "http").suitable);
        assert!(source().checked(0, "http").suitable);
        assert!(!source().checked(101, "http").suitable);
        assert!(!source().checked(50, "magnet").suitable);
        let mut s = source();
        s.quota_known = false;
        assert!(!s.checked(50, "http").suitable);
        let mut s = source();
        s.single_file_limit = Some(40);
        assert!(!s.checked(50, "http").suitable);
        let mut s = source();
        s.offline_quota = Some(json!({"remaining":0}));
        assert!(!s.checked(50, "http").suitable);
    }
    #[test]
    fn exceeded_capacity_is_known_and_has_zero_free_space() {
        let mut s = source();
        s.apply_capacity(&json!({"quota_known":true,"total_bytes":10,"used_bytes":1900,"free_bytes":-1890}));
        assert!(s.quota_known);
        assert_eq!(s.free_bytes, Some(0));
        assert!(!s.checked(1, "http").suitable);
        let mut s = source();
        s.apply_capacity(&json!({"quota_known":false,"total_bytes":null,"used_bytes":null,"free_bytes":null}));
        assert!(!s.quota_known);
        assert_eq!(s.free_bytes, None);
    }
    #[test]
    fn limits_unknown_stay_null() {
        let v = json!(source());
        assert!(v["singleFileLimit"].is_null());
        assert!(v["offlineQuota"].is_null());
        assert!(!provider("pan123")
            .unwrap()
            .protocols
            .contains(&"magnet".into()));
    }
}
