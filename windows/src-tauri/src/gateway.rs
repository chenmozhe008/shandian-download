// 本机网关 127.0.0.1:6800 —— 浏览器里的下载交给本应用的入口。
//
// 移植自 Mac 版 Sources/Shandian/Gateway.swift，端口、路径、Origin 校验保持一致，
// 所以仓库里 browser-extension/ 那份扩展一行都不用改，装到 Chrome / Edge 上就能用。
//
// 四条路：
//   POST /extension/add     自带扩展交下载（靠 Origin 认身份，不用密钥）
//   POST /extension/ping    扩展问「你在不在 / 内核好了没」，设置页也靠它显示「已连接」
//   POST /extension/status  扩展问这条下载成功了没，失败了它交回浏览器重下
//   POST /extension/batch   扩展抓到的一批链接，发给主窗口让用户勾选
//   POST /jsonrpc           Aria2 RPC，LinkSwift 这类网盘直链脚本用（改走 App 的添加流程）
//   /api/v1/*               代理 Gopeed 官方扩展；建任务同样走 App 的规则
//
// 为什么非要这个网关：浏览器只能往「普通 HTTP 地址」发请求，发不了自定义头，
// 更叫不起内核的 X-Api-Token；而且扩展交过来的 Cookie / Referer 要是不用上，
// 网盘直链会 403、抖音小红书只能拿到登录页。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;

pub const PORT: u16 = 6800;

/// manifest.json 里的 key 固定了扩展 ID，Chrome 和 Edge 都是这个。
/// 网页伪造不了 Origin 头，所以它够当凭证——扩展交任务不需要再让用户填密钥。
const EXT_ORIGIN: &str = "chrome-extension://pggchniffhkcgigajlapkboikmgdipel";

static STARTED: AtomicBool = AtomicBool::new(false);
/// 扩展最近一次连上来的时间戳（毫秒），0 = 从没连过。设置页显示「已连接」用它。
static LAST_SEEN: AtomicU64 = AtomicU64::new(0);
static BROWSER: Mutex<String> = Mutex::new(String::new());
/// 等确认框拍板的链接（url → 投进来的时刻）。开着「新建下载前确认」时扩展交来的
/// 任务先发 ext-add 给主窗口弹确认框，这期间扩展轮询 /extension/status 必须回
/// "pending"——回 none 会让它以为投丢了，交回浏览器重下。
static PENDING: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());
/// 确认框一直没人点的话，10 分钟后当没这回事（扩展那边 5 分钟就放弃轮询了）
const PENDING_TTL_MS: u64 = 10 * 60 * 1000;

fn pending_add(url: &str) -> bool {
    let now = now_ms();
    if let Ok(mut v) = PENDING.lock() {
        v.retain(|(_, t)| now.saturating_sub(*t) < PENDING_TTL_MS);
        if v.iter().any(|(u, _)| u == url) {
            return false;
        }
        v.push((url.to_string(), now));
        true
    } else {
        false
    }
}

fn pending_take(url: &str) {
    if let Ok(mut v) = PENDING.lock() {
        v.retain(|(u, _)| u != url);
    }
}

fn pending_has(url: &str) -> bool {
    let now = now_ms();
    if let Ok(mut v) = PENDING.lock() {
        v.retain(|(_, t)| now.saturating_sub(*t) < PENDING_TTL_MS);
        return v.iter().any(|(u, _)| u == url);
    }
    false
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn ok_json(v: Value) -> (u16, Vec<u8>) {
    (200, serde_json::to_vec(&v).unwrap_or_default())
}

fn bad(code: u16, msg: &str) -> (u16, Vec<u8>) {
    (code, json!({ "ok": false, "error": msg }).to_string().into_bytes())
}

// ---------- 给设置页看的状态 ----------

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtStatus {
    pub port: u16,
    /// 有没有扩展连上来过（10 分钟内算在线：扩展每 10 分钟 ping 一次）
    pub connected: bool,
    pub browser: String,
    pub last_seen: u64,
}

/// Aria2 RPC 的接入信息：给用户在 LinkSwift 里填（地址/端口/路径/密钥）。
/// 密钥就是内核 token——RPC 只监听 127.0.0.1，但同一台机器上任何程序都能连，
/// 所以还是要校验；用户从设置页复制过去即可。
#[tauri::command]
pub fn rpc_info(state: tauri::State<'_, crate::AppState>) -> serde_json::Value {
    let token = {
        let e = state.engine.lock().unwrap();
        e.token.clone()
    };
    serde_json::json!({
        "host": "127.0.0.1",
        "port": PORT,
        "path": "/jsonrpc",
        "token": token,
    })
}

#[tauri::command]
pub fn extension_status() -> ExtStatus {
    let last = LAST_SEEN.load(Ordering::SeqCst);
    let connected = last > 0 && now_ms().saturating_sub(last) < 11 * 60 * 1000;
    ExtStatus {
        port: PORT,
        connected,
        browser: BROWSER.lock().map(|s| s.clone()).unwrap_or_default(),
        last_seen: last,
    }
}

// ---------- 扩展的安装 ----------

/// 随包带的扩展在哪：解压后就在 exe 旁边（打包脚本把 browser-extension/ 拷成 BrowserExtension/）
fn bundled() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let d = exe.parent()?.join("BrowserExtension");
    d.is_dir().then_some(d)
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let target = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &target)?;
        } else {
            // 逐个覆盖、不删整个目录：浏览器可能正开着这个扩展，删了它会失效
            let _ = std::fs::remove_file(&target);
            std::fs::copy(e.path(), &target)?;
        }
    }
    Ok(())
}

/// 返回用户该加载的扩展文件夹。
/// 安装版就用安装目录里那份：位置固定，更新时原位替换。
/// 便携版同步到数据目录再用：解压出来的文件夹可能被删掉或挪走，浏览器里的扩展就失效了。
/// 数据目录里已有的（以前从那儿加载过）照样同步，不然那份扩展会一直停在旧版本。
pub fn install(base: &Path) -> Option<std::path::PathBuf> {
    let src = bundled()?;
    let dst = base.join("BrowserExtension");
    let installed = src.parent().is_some_and(|d| d.join("uninstall.exe").is_file());
    if !installed || dst.exists() {
        let synced = copy_dir(&src, &dst);
        if !installed {
            synced.ok()?;
            return Some(dst);
        }
    }
    Some(src)
}

#[tauri::command(async)]
pub fn extension_dir(state: tauri::State<'_, AppState>) -> String {
    // 只在启动时同步过一次，这里顺手再同步一下，保证看到的总是最新那份
    match install(&state.base) {
        Some(p) => p.display().to_string(),
        None => bundled().unwrap_or_else(|| state.base.join("BrowserExtension")).display().to_string(),
    }
}

/// 「显示文件夹」：资源管理器打开并选中这个目录，直接拖到扩展管理页就能装
#[tauri::command(async)]
pub fn extension_reveal(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let dir = install(&state.base)
        .ok_or("安装包里没带扩展文件夹（BrowserExtension），请用官方绿色包")?;
    crate::commands::open_dir(dir.display().to_string())?;
    Ok(dir.display().to_string())
}

/// 打开浏览器的扩展管理页。chrome:// 这类地址不能当普通网址丢给 shell，
/// 得让浏览器自己开（和 Mac 版用 AppleScript 开 chrome://extensions 一个意思）
#[tauri::command]
pub async fn open_browser_extensions(browser: String) -> Result<bool, String> {
    let browser = match browser.to_ascii_lowercase().as_str() { "chrome" => "chrome", "edge" => "edge", _ => return Err("请选择 Chrome 或 Edge".into()) };
    let profile = tauri::async_runtime::spawn_blocking(move || crate::browser_support::extension_profile(browser, extension_id())).await.map_err(|e| e.to_string())?;
    let Some(profile) = profile else { return Ok(false); };
    let (nonce, receiver) = crate::browser_handoff::register("shandian:extension-manager");
    let page = format!("http://127.0.0.1:6800/channel-open#action=extensions&wechat-auto={nonce}");
    if let Err(error) = open_browser_profile_url(browser, &page, Some(&profile.profile)) {
        crate::browser_handoff::cancel(&nonce); return Err(error);
    }
    let opened = matches!(tokio::time::timeout(Duration::from_secs(15), receiver).await, Ok(Ok(())));
    crate::browser_handoff::cancel(&nonce);
    Ok(opened)
}

/// Use the selected browser for extension-dependent flows; avoid waiting for a shell subprocess.
pub(crate) fn open_browser_url(browser: &str, url: &str) -> Result<(), String> {
    open_browser_profile_url(browser, url, None)
}

pub(crate) fn open_browser_profile_url(browser: &str, url: &str, profile: Option<&str>) -> Result<(), String> {
    let exe = crate::browser_support::browser_exe(browser).ok_or("未找到所选浏览器")?;
    let mut command = std::process::Command::new(exe);
    if let Some(profile) = profile { command.arg(format!("--profile-directory={profile}")); }
    command.arg(url).spawn().map(|_| ()).map_err(|e| format!("无法打开浏览器：{e}"))
}

pub(crate) fn extension_id() -> &'static str { EXT_ORIGIN.trim_start_matches("chrome-extension://") }

// ---------- 启动 ----------

pub fn start(app: AppHandle) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let base = app.state::<AppState>().local.clone();
    std::thread::spawn(move || {
        // 只绑回环地址：同一局域网里的别人连不上，也不触发防火墙提示
        let listener = match TcpListener::bind(("127.0.0.1", PORT)) {
            Ok(l) => l,
            Err(e) => {
                log(&base, &format!("监听 127.0.0.1:{PORT} 失败：{e}（端口被占？扩展会连不上）"));
                return;
            }
        };
        log(&base, &format!("网关已监听 127.0.0.1:{PORT}"));
        for conn in listener.incoming() {
            let Ok(stream) = conn else { continue };
            let app = app.clone();
            // 一个连接一个线程：扩展一次只发一个请求，量很小，不值得上线程池
            std::thread::spawn(move || serve(app, stream));
        }
    });
}

fn log(_base: &Path, msg: &str) { crate::app_log::record("gateway", msg); }

// ---------- 极简 HTTP ----------

struct Req {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

fn read_req(stream: &TcpStream) -> Option<Req> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut it = line.split_whitespace();
    let method = it.next()?.to_string();
    let path = it.next()?.to_string();

    let mut headers = Vec::new();
    loop {
        let mut l = String::new();
        if reader.read_line(&mut l).ok()? == 0 {
            break;
        }
        let t = l.trim_end();
        if t.is_empty() {
            break;
        }
        if let Some(i) = t.find(':') {
            headers.push((t[..i].to_lowercase(), t[i + 1..].trim().to_string()));
        }
    }

    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    if len > 0 {
        reader.read_exact(&mut body).ok()?;
    }
    Some(Req {
        method,
        path,
        headers,
        body,
    })
}

fn serve(app: AppHandle, mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let Some(req) = read_req(&stream) else { return };
    let landing_path = req.path.split('?').next().unwrap_or("");
    let landing = req.method == "GET" && ["/channel-open","/linkswift-settings","/baidu-authorize"].contains(&landing_path);
    let (status, body) = if landing && landing_path == "/linkswift-settings" { (200, include_bytes!("../../ui/linkswift-settings.html").to_vec()) }
        else if landing && landing_path == "/baidu-authorize" { (200, include_bytes!("../../ui/baidu-authorize.html").to_vec()) }
        else if landing { (200, include_bytes!("../../ui/channel-open.html").to_vec()) } else { route(&app, &req) };
    let content_type = if landing { "text/html; charset=utf-8" } else { "application/json" };

    // Chrome 对「公网页面 → 127.0.0.1」会先发一次 Private Network 预检，
    // 不回这个头的话请求会被挡在外面（扩展是 service worker 发的，一样会走）
    let head = format!(
        "HTTP/1.1 {status} {}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Headers: *\r\n\
         Access-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS\r\n\
         Access-Control-Allow-Private-Network: true\r\n\
         Connection: close\r\n\r\n",
        if status == 200 { "OK" } else { "Error" },
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
}

fn route(app: &AppHandle, req: &Req) -> (u16, Vec<u8>) {
    if req.method == "OPTIONS" {
        return (200, b"{}".to_vec());
    }
    let path = req.path.split('?').next().unwrap_or("");
    if path.starts_with("/extension/cloud-account/") {
        if req.header("origin")==Some(EXT_ORIGIN) {LAST_SEEN.store(now_ms(),Ordering::SeqCst);if let Ok(mut b)=BROWSER.lock(){*b=if req.header("user-agent").unwrap_or("").contains("Edg/"){ "Edge" }else{"Chrome"}.into();}}
        return ok_json(tauri::async_runtime::block_on(crate::cloud::account_link_http(app,&req.method,&req.path,req.header("origin").unwrap_or(""),&req.body)));
    }
    if path == "/extension/linkswift-settings" && req.method == "POST" {
        let value: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        return ok_json(json!({"ok":crate::browser_handoff::claim(value["nonce"].as_str().unwrap_or(""),"http://127.0.0.1:6800/linkswift-settings")}));
    }
    if path.starts_with("/jsonrpc") {
        return (200, aria2(app, req));
    }
    if path.starts_with("/extension/") {
        return from_extension(app, req);
    }
    if path.starts_with("/api/v1/") {
        return proxy(app, req);
    }
    (404, json!({ "ok": false, "error": "not found" }).to_string().into_bytes())
}

// ---------- 自带扩展 ----------

/// 只收抓取功能认的类型，名字和大小是网页上的文字，只用来显示
fn grab_batch(app: &AppHandle, req: &Req) -> (u16, Vec<u8>) {
    use crate::links::LinkKind;
    let v: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
    // 弹窗里的批量抓取只发链接（{ urls }），页面面板带名字和大小（{ links }）
    let from_urls: Vec<Value> = v["urls"]
        .as_array()
        .map(|a| a.iter().filter_map(|u| Some(json!({ "url": u.as_str()? }))).collect())
        .unwrap_or_default();
    let raw = v["links"].as_array().cloned().unwrap_or(from_urls);
    let links: Vec<Value> = raw
        .iter()
        .filter_map(|l| {
            let url = l["url"].as_str()?.trim();
            let fallback = match LinkKind::of(url)? {
                LinkKind::Magnet => "磁力",
                LinkKind::Ed2k => "ed2k",
                LinkKind::TorrentFile => "种子文件",
                LinkKind::Http => "直链",
                LinkKind::VideoSite => "视频网站",
                _ => return None,
            };
            crate::sources::remember(url, v["referrer"].as_str().unwrap_or(""));
            let kind = crate::api::download_label(url).unwrap_or_else(||fallback.to_string());
            let headers: Vec<(String,String)> = l["headers"].as_object().map(|m|m.iter().filter_map(|(k,v)|Some((k.clone(),v.as_str()?.to_string()))).collect()).unwrap_or_default();
            Some(json!({
                "url": url,
                "headers": headers,
                "kind": kind,
                "name": l["name"].as_str().unwrap_or(""),
                "size": l["size"].as_str().unwrap_or(""),
            }))
        })
        .take(1000)
        .collect();
    if links.is_empty() {
        return (400, json!({ "ok": false }).to_string().into_bytes());
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        let page = v["referrer"].as_str().unwrap_or("");
        let folder = v["folderName"].as_str().filter(|_| crate::api::material_site_label(page) == Some("轻雀文档"));
        let source = if folder.is_some() { Some(page) } else { None };
        let _ = w.emit("ext-grab", json!({ "links": links, "folderName": folder, "sourceUrl": source }));
    }
    ok_json(json!({ "ok": true }))
}

fn from_extension(app: &AppHandle, req: &Req) -> (u16, Vec<u8>) {
    if req.header("origin") != Some(EXT_ORIGIN) {
        return (403, json!({ "ok": false }).to_string().into_bytes());
    }
    LAST_SEEN.store(now_ms(), Ordering::SeqCst);
    let ua = req.header("user-agent").unwrap_or("");
    if let Ok(mut b) = BROWSER.lock() {
        *b = if ua.contains("Edg/") { "Edge" } else { "Chrome" }.to_string();
    }

    let path = req.path.split('?').next().unwrap_or("");

    if matches!(path, "/extension/feishu-media" | "/extension/feishu-media/download") {
        if req.method != "POST" || req.body.len() > 64 * 1024 {
            return bad(400, "飞书下载请求无效");
        }
        let Ok(value) = serde_json::from_slice::<Value>(&req.body) else {
            return bad(400, "飞书下载请求无效");
        };
        let Some(url) = value["url"].as_str().and_then(crate::feishu::document_url) else {
            return bad(400, "飞书文档链接无效");
        };
        let state = app.state::<AppState>();
        if path == "/extension/feishu-media" {
            return match crate::feishu::preview_document(url.as_str(), &state.local) {
                Ok(preview) => ok_json(json!({"ok": true, "document": preview})),
                Err(error) => ok_json(json!({"ok": false, "error": error})),
            };
        }
        let ticket = value["ticket"].as_str().unwrap_or("");
        if ticket.len() != 32 || !ticket.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return bad(400, "飞书附件清单已失效");
        }
        let Ok(selected) = serde_json::from_value::<Vec<usize>>(value["selected"].clone()) else {
            return bad(400, "飞书附件选择无效");
        };
        if selected.is_empty() || selected.len() > 10_000 {
            return bad(400, "飞书附件选择无效");
        }
        let result = tauri::async_runtime::block_on(crate::feishu::download_selection(
            ticket.to_owned(), selected, None, None, Some(url), state,
        ));
        return match result {
            Ok(id) => ok_json(json!({"ok": true, "id": id})),
            Err(error) => ok_json(json!({"ok": false, "error": error})),
        };
    }

    if path == "/extension/baidu-selection" {
        if req.method != "POST" {
            return bad(400, "只接受 POST");
        }
        let Ok(value) = serde_json::from_slice::<Value>(&req.body) else {
            return bad(400, "请求体不是 JSON");
        };
        let Some(share) = value["share"].as_str().and_then(crate::baidu::share_url) else {
            return bad(400, "百度网盘分享链接无效");
        };
        let Ok(files) = serde_json::from_value::<Vec<crate::baidu::SharePick>>(value["files"].clone()) else {
            return bad(400, "未收到勾选的文件");
        };
        let Ok(files) = crate::baidu::normalize_selection(Some(files)) else {
            return bad(400, "所选文件信息无效");
        };
        let Some(window) = app.get_webview_window("main") else {
            return bad(503, "请打开闪电全能下载");
        };
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        if window.emit("ext-add", json!({
            "url": share.as_str(), "baiduSelection": files, "name": files.first().map(|file| &file.name),
        })).is_err() {
            return bad(503, "无法打开新建下载");
        }
        return ok_json(json!({"ok":true,"selection":true}));
    }

    if path.starts_with("/extension/baidu-auth/") {
        if req.method != "POST" { return bad(400, "只接受 POST"); }
        let value: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let ticket = value["ticket"].as_str().unwrap_or("");
        return match path {
            "/extension/baidu-auth/start" => match crate::baidu_setup::claim_web_authorization(ticket) {
                Some(url) => ok_json(json!({"ok":true,"url":url.as_str()})),
                None => ok_json(json!({"ok":false})),
            },
            "/extension/baidu-auth/finish" => ok_json(json!({
                "ok": crate::baidu_setup::complete_web_authorization(ticket, value["code"].as_str().unwrap_or(""))
            })),
            "/extension/baidu-auth/close" => {
                crate::baidu_setup::close_web_authorization(ticket);
                ok_json(json!({"ok":true}))
            }
            _ => bad(404, "授权路径不存在"),
        };
    }

    if path=="/extension/refresh/info" || path=="/extension/refresh/apply" || path=="/extension/refresh/status" {
        let v:Value=serde_json::from_slice(&req.body).unwrap_or(Value::Null);let token=v["token"].as_str().unwrap_or("");
        if path.ends_with("/status"){crate::engine::cloud_refresh_notice(token,v["message"].as_str().unwrap_or(""));return ok_json(json!({"ok":true}))}
        if path.ends_with("/info"){return ok_json(json!({"ok":true,"info":crate::engine::cloud_refresh_info(token)}))}
        let files:Vec<crate::api::MultiFile>=match serde_json::from_value(v["files"].clone()){Ok(f)=>f,Err(_)=>return bad(400,"文件信息无效")};
        return match tauri::async_runtime::block_on(crate::engine::cloud_refresh_apply(token.into(),files,app.state::<AppState>())) {
            Ok(())=>ok_json(json!({"ok":true})),Err(e)=>ok_json(json!({"ok":false,"error":e}))
        };
    }

    if path.starts_with("/extension/batch") {
        return grab_batch(app, req);
    }

    // 弹窗批量抓取先问哪些已经在列表里。查不出来就都当没有，用户点下载时还会提示重复
    if path.starts_with("/extension/known") {
        let v: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let urls: Vec<String> = v["urls"].as_array().map(|a| a.iter().filter_map(|u| u.as_str().map(String::from)).collect()).unwrap_or_default();
        let st = app.state::<AppState>();
        let known = tauri::async_runtime::block_on(crate::engine::known_urls(&st, urls));
        return ok_json(json!({ "ok": true, "known": known }));
    }

    if path == "/extension/wechat-auto" {
        let v:Value=serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        return ok_json(json!({"ok":crate::wechat::claim_auto(v["token"].as_str().unwrap_or(""),v["share"].as_str().unwrap_or(""))}));
    }
    if path.starts_with("/extension/wechat") {
        let v: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let (share, playable) = (v["share"].as_str().unwrap_or_default().trim(), v["playable"].as_str().unwrap_or_default());
        if share.len() > 8192 || playable.len() > 16384 || !crate::wechat::is_share(share) || !crate::wechat::is_playable(playable) {
            return bad(400, "视频号链接无效");
        }
        let title: String = v["title"].as_str().unwrap_or("").chars().take(100).collect();
        let st = app.state::<AppState>();
        let (dir, confirm) = {
            let settings = st.settings.lock().unwrap();
            (settings.default_dir(crate::links::LinkKind::VideoSite, &st.folders), settings.confirm_new)
        };
        let duplicate = tauri::async_runtime::block_on(crate::engine::find_duplicate(share.to_string(), Some(dir.to_string_lossy().into_owned()), app.state::<AppState>())).ok().flatten().is_some();
        if confirm || duplicate {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize(); let _ = w.show(); let _ = w.set_focus();
            }
            let ticket = crate::wechat::stage(share, playable, &title);
            if !crate::wechat::begin_dialog(&ticket) { return ok_json(json!({"ok":true,"pending":true})); }
            let result = app.emit_to("main", "wechat-ready", json!({
                "url": share, "opts": {"name": title, "dir": dir.to_string_lossy(), "wechatTicket": ticket, "wechatSkipConfirm": !confirm}
            }));
            return match result {
                Ok(_) => ok_json(json!({"ok": true, "pending": true})),
                Err(e) => { crate::wechat::cancel_pending(&ticket); bad(500, &e.to_string()) }
            };
        }
        return match st.videos.start_wechat(share, playable, &title, &dir, false) {
            Ok(_) => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
                ok_json(json!({ "ok": true }))
            }
            Err(e) => ok_json(json!({ "ok": false, "error": e })),
        };
    }

    if path.starts_with("/extension/ping") {
        let link_info:Value=serde_json::from_slice(&req.body).unwrap_or(Value::Null);crate::cloud::account_link_ping(&link_info);
        let ready = {
            let st = app.state::<AppState>();
            let mut e = st.engine.lock().unwrap();
            e.is_running()
        };
        // version 留空：Windows 版不接管扩展的安装和更新，返回版本号会让扩展
        // 每隔 10 分钟把自己 reload 一次（它拿它跟自己的 manifest 比）
        return ok_json(json!({ "ok": true, "version": "", "ready": ready }));
    }

    if path.starts_with("/extension/pikpak") {
        if crate::cloud::account_link_legacy_busy("pikpak") {return ok_json(json!({"ok":false,"error":"请在当前云盘关联页完成已选目标的关联"}))}
        // 扩展把浏览器里登录着的 PikPak 账号交过来（Mac 版同款）
        let v: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        return match crate::pikpak::adopt_web_login(&app.state::<AppState>(), &v) {
            Ok(()) => {
                // 通知界面马上刷新登录状态：从扩展弹窗关联时 App 没在等，不通知要等设置页重开才看得到
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.emit("pikpak-linked", json!({}));
                }
                ok_json(json!({ "ok": true }))
            }
            Err(e) => ok_json(json!({ "ok": false, "error": e })),
        };
    }

    if req.method != "POST" {
        return bad(400, "只接受 POST");
    }
    let Ok(v) = serde_json::from_slice::<Value>(&req.body) else {
        return bad(400, "请求体不是 JSON");
    };
    let Some(url) = v["url"].as_str() else {
        return bad(400, "缺少 url");
    };
    if crate::links::LinkKind::of(url).is_none() {
        return bad(400, "这个链接不认识");
    }

    let st = app.state::<AppState>();

    if path.starts_with("/extension/status") {
        let state = task_state(&st, url);
        return ok_json(json!({ "ok": true, "state": state }));
    }

    // 抖音 / 小红书 / 新片场的登录信息：浏览器里已经登录了，直接拿来给 yt-dlp 用
    if let Some(arr) = v["cookies"].as_array() {
        // 下载地址可能是 CDN，认不出站点，这时按所在页面存，不然登录信息就丢了
        if crate::videos::save_cookies(&st.local, url, arr).is_none() {
            if let Some(page) = v["page"].as_str() {
                crate::videos::save_cookies(&st.local, page, arr);
            }
        }
    }

    let headers: Vec<(String, String)> = v["headers"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let name = v["name"].as_str().unwrap_or("");
    // 记下来源站：从页面取到的视频地址是 CDN，光看地址标不出是哪个站
    let baidu_link = crate::baidu::share_url(url);
    let baidu_share = baidu_link.is_some();
    if !baidu_share {
        crate::sources::remember(url, v["page"].as_str().unwrap_or(""));
    }

    // 对齐 Mac 版：开着「新建下载前确认」就不直接建任务，把链接发给主窗口弹确认框，
    // 用户点「开始下载」后前端走 add_task（headers 一起带下去，网盘直链全靠它）。
    // 确认框还没关掉之前扩展问状态要回 "pending"，不能让它以为投丢了。
    // group：一批文件（网盘文件夹），要由前端攒成一条任务，不能在这里各建各的
    let group = v["group"].as_bool().unwrap_or(false);
    let confirm_new = st.settings.lock().unwrap().confirm_new;
    // 视频号分享链接不能直接下，要交给主窗口给出元宝下载指引
    let wechat = crate::wechat::is_share(url);
    let collection = crate::links::video_collection(url, v["collection"].as_bool().unwrap_or(false)).is_some();
    let baidu_code_required = baidu_share && v["mediaType"].as_str() == Some("baiduShareLocked");
    if confirm_new || group || wechat || collection || baidu_share || crate::feishu::document_url(url).is_some() {
        // 已在 pending 也要重新发一次 ext-add：用户上次取消了确认框后，
        // 这条还留在 pending 里（TTL 10 分钟），不发就等于第二次点击完全没反应
        pending_add(url);
        if let Some(w) = app.get_webview_window("main") {
            // 扩展从浏览器点进来时窗口多半还在托盘/背后，不前置用户看不见确认框
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
            let _ = w.emit("ext-add", json!({ "url": url, "headers": headers, "name": name, "group": group, "collection": collection, "baiduCodeRequired": baidu_code_required }));
        }
        return ok_json(json!({ "ok": true }));
    }

    let r = tauri::async_runtime::block_on(crate::engine::add_link_full(
        url, &st, name, "", &headers, None,
    ));
    match r {
        Ok(_) => ok_json(json!({ "ok": true })),
        Err(e) => {
            log(&st.local, &format!("扩展投任务失败：{url} → {e}"));
            bad(500, &e)
        }
    }
}

/// 扩展交过来的链接现在什么状态：failed / done / running / waiting / pending / none
/// （pending = 确认框还没拍板；none = 不在列表里，比如确认框里被取消）。
/// 失败时扩展会交回浏览器重新下载，下载不丢。
fn task_state(st: &AppState, url: &str) -> &'static str {
    if let Some(share) = crate::baidu::share_url(url) {
        let key = crate::baidu::public_url(&share);
        if let Some(task) = st.baidu.tasks(Some(&key)).into_iter().last() {
            pending_take(url);
            return match task.status.as_str() {
                "done" => "done",
                "error" => "failed",
                "pause" => "waiting",
                _ => "running",
            };
        }
    }
    if let Some(j) = st.videos.list().into_iter().find(|j| j.url == url) {
        pending_take(url);
        return match j.state.as_str() {
            "done" => "done",
            "error" => "failed",
            "pause" => "waiting",
            _ => "running",
        };
    }
    let (port, token) = {
        let e = st.engine.lock().unwrap();
        (e.port, e.token.clone())
    };
    let list = tauri::async_runtime::block_on(async {
        crate::api::Api::new(port, token).list_tasks().await.ok()
    })
    .unwrap_or_default();
    for t in list {
        if t.url() == url {
            pending_take(url);
            return match t.status.as_str() {
                "done" => "done",
                "error" => "failed",
                "pause" | "waiting" => "waiting",
                _ => "running",
            };
        }
    }
    // 任务还没建（确认框在等用户）→ pending；确认框里被取消 → none
    if pending_has(url) {
        return "pending";
    }
    "none"
}

// ---------- Aria2 RPC（LinkSwift 这类网盘直链脚本用） ----------

fn aria2(app: &AppHandle, req: &Req) -> Vec<u8> {
    let v: Value = serde_json::from_slice(&req.body).unwrap_or(json!({}));
    let token = {
        let st = app.state::<AppState>();
        let e = st.engine.lock().unwrap();
        e.token.clone()
    };
    if let Some(batch) = v.as_array() {
        let out: Vec<Value> = batch.iter().map(|c| aria2_call(app, c, &token)).collect();
        return serde_json::to_vec(&out).unwrap_or_default();
    }
    serde_json::to_vec(&aria2_call(app, &v, &token)).unwrap_or_default()
}

fn aria2_call(app: &AppHandle, call: &Value, token: &str) -> Value {
    let id = call.get("id").cloned().unwrap_or(Value::Null);
    let mut params: Vec<Value> = call
        .get("params")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();

    let fail = |code: i32, msg: &str| json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": msg } });

    // 脚本都把密钥放在第一个参数里：token:<内核 token>
    let unauthorized = match params.first().and_then(|p| p.as_str()) {
        Some(s) if s.starts_with("token:") => &s[6..] != token,
        _ => true,
    };
    if unauthorized {
        return fail(1, "Unauthorized");
    }
    params.remove(0);

    match call.get("method").and_then(|m| m.as_str()).unwrap_or("") {
        "aria2.getVersion" => json!({
            "jsonrpc": "2.0", "id": id,
            "result": { "version": "1.37.0", "enabledFeatures": ["Shandian"] }
        }),
        "aria2.addUri" => {
            let Some(url) = params.first().and_then(|u| u.as_array()).and_then(|a| a.first()).and_then(|u| u.as_str()) else {
                return fail(1, "No URI");
            };
            let opts = params.get(1).cloned().unwrap_or(json!({}));
            // Aria2 的 header 是 ["Key: Value", ...] 这种数组
            let headers: Vec<(String, String)> = opts["header"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|h| h.as_str())
                        .filter_map(|h| h.split_once(':'))
                        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let name = opts["out"].as_str().unwrap_or("");
            let st = app.state::<AppState>();
            // 和扩展同一条规矩：开了「新建下载前确认」就先弹确认框，别绕过用户的选择
            if st.settings.lock().unwrap().confirm_new {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                    let _ = w.emit("ext-add", json!({ "url": url, "headers": headers, "name": name, "group": true }));
                }
                return json!({ "jsonrpc": "2.0", "id": id, "result": "shandian-gid" });
            }
            match tauri::async_runtime::block_on(crate::engine::add_link_full(
                url, &st, name, "", &headers, None,
            )) {
                Ok(_) => json!({ "jsonrpc": "2.0", "id": id, "result": "shandian-gid" }),
                Err(e) => fail(1, &e),
            }
        }
        // 脚本推完常顺手问一下队列，回空数组它就满意了（Mac 版同样处理）
        "aria2.tellActive" | "aria2.tellWaiting" | "aria2.tellStopped" => {
            json!({ "jsonrpc": "2.0", "id": id, "result": [] })
        }
        _ => fail(1, "Method not supported"),
    }
}

// ---------- 代理 Gopeed 官方扩展 ----------

fn proxy(app: &AppHandle, req: &Req) -> (u16, Vec<u8>) {
    let st = app.state::<AppState>();
    let (port, token) = {
        let e = st.engine.lock().unwrap();
        (e.port, e.token.clone())
    };

    // 建任务：走 App 的添加流程，才能套上「存哪、磁力只勾正片、片名整理」这些规则
    if req.method == "POST" && req.path.split('?').next() == Some("/api/v1/tasks") {
        if let Ok(v) = serde_json::from_slice::<Value>(&req.body) {
            let url = v["req"]["url"].as_str().unwrap_or("");
            if crate::links::LinkKind::of(url).is_some() {
                let headers: Vec<(String, String)> = v["req"]["extra"]["header"]
                    .as_object()
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                let name = v["opts"]["name"].as_str().unwrap_or("");
                let r = tauri::async_runtime::block_on(crate::engine::add_link_full(
                    url, &st, name, "", &headers, None,
                ));
                if let Err(e) = r {
                    return (500, json!({ "code": 1, "msg": e }).to_string().into_bytes());
                }
                return ok_json(json!({ "code": 0, "msg": "", "data": "shandian-task" }));
            }
        }
    }

    let path = req.path.clone();
    let method = match req.method.as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "DELETE" => reqwest::Method::DELETE,
        _ => return bad(405, "不支持的方法"),
    };
    let body = req.body.clone();
    let fut = async move {
        let client = reqwest::Client::builder()
            .no_proxy() // 本机开了全局代理时 127.0.0.1 会被拦掉
            .build()
            .map_err(|e| e.to_string())?;
        let url = format!("http://127.0.0.1:{port}{path}");
        let mut rb = client.request(method, &url).header("X-Api-Token", token);
        if !body.is_empty() {
            rb = rb.header("Content-Type", "application/json").body(body);
        }
        let resp = rb.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
        Ok::<(u16, Vec<u8>), String>((status, bytes))
    };
    match tauri::async_runtime::block_on(fut) {
        Ok(r) => r,
        Err(e) => {
            log(&st.local, &format!("转发 /api/v1 失败：{e}"));
            (502, Vec::new())
        }
    }
}
