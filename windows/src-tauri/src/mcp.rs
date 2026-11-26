use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    time::Duration,
};

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;

pub(crate) const ROUTE: &str = "/internal/mcp";
const PROTOCOL: &str = "2025-11-25";

fn targets() -> &'static [&'static str] {
    &["local"]
}

#[tauri::command]
pub fn mcp_config() -> Result<Value, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(json!({"mcpServers": {"shandian": {
        "command": exe, "args": ["--mcp"]
    }}}))
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object", "properties":properties, "required":required, "additionalProperties":false})
}

fn tools() -> Value {
    let target = json!({"type":"string", "enum":targets(), "default":"local",
        "description":"下载位置，当前支持本机。"});
    let task = schema(
        json!({
            "id":{"type":"string","description":"list_downloads 返回的完整任务 ID。"},
            "target":target,
        }),
        &["id"],
    );
    json!([
        {"name":"add_download", "description":"提交磁力、ed2k 或 HTTP/HTTPS 下载。若返回 needs_confirmation，请在闪电确认。",
            "inputSchema":schema(json!({
                "url":{"type":"string"}, "target":target
            }), &["url"])},
        {"name":"list_downloads", "description":"查看指定位置的任务，包括进度和速度。",
            "inputSchema":schema(json!({
                "include_done":{"type":"boolean","default":false}, "target":target
            }), &[])},
        {"name":"pause_download", "description":"暂停一条下载。", "inputSchema":task},
        {"name":"resume_download", "description":"继续一条下载。", "inputSchema":task},
    ])
}

fn reply(
    message: Value,
    call: impl FnOnce(&str, &Value) -> Result<Value, String>,
) -> Option<Value> {
    let method = message["method"].as_str()?;
    let id = message.get("id")?;
    let params = &message["params"];
    let result = match method {
        "initialize" => {
            let requested = params["protocolVersion"].as_str().unwrap_or(PROTOCOL);
            let version = if matches!(
                requested,
                "2025-11-25" | "2025-06-18" | "2025-03-26" | "2024-11-05"
            ) {
                requested
            } else {
                PROTOCOL
            };
            json!({"protocolVersion":version,"capabilities":{"tools":{}},
                "serverInfo":{"name":"shandian","version":env!("CARGO_PKG_VERSION")}})
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools":tools()}),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call(name, &arguments) {
                Ok(value) => json!({"content":[{"type":"text","text":value.to_string()}]}),
                Err(error) => json!({"isError":true,"content":[{"type":"text","text":error}]}),
            }
        }
        _ => {
            return Some(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}}),
            )
        }
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

fn local_token() -> Result<String, String> {
    let app_data = |variable| {
        std::env::var_os(variable).map(|path| PathBuf::from(path).join("com.shandian.windows"))
    };
    let base = app_data("APPDATA").ok_or("无法找到闪电的配置目录")?;
    let local = app_data("LOCALAPPDATA").unwrap_or_else(|| base.clone());
    let file = crate::migrate::current_dir(&base, &local, "engine").join("token.txt");
    std::fs::read_to_string(file)
        .ok()
        .map(|token| token.trim().to_owned())
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "请先打开闪电全能下载".into())
}

pub(crate) fn run_stdio() {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(25))
        .build();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str(&line) else {
            continue;
        };
        let response = reply(message, |name, arguments| {
            let token = local_token()?;
            let client = client.as_ref().map_err(|_| "无法连接闪电")?;
            let url = format!("http://127.0.0.1:{}{ROUTE}", crate::gateway::PORT);
            let response = client
                .post(url)
                .header("X-Api-Token", token)
                .json(&json!({"name":name,"arguments":arguments}))
                .send()
                .map_err(|error| {
                    if error.is_timeout() {
                        "闪电响应超时，请先查询任务状态再重试"
                    } else {
                        "连不上闪电全能下载，请先打开应用"
                    }
                })?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                return Err("运行中的闪电拒绝了连接，请重新打开应用".into());
            }
            let value: Value = response
                .json()
                .map_err(|_| "当前运行的闪电版本没有提供 MCP 接口")?;
            if let Some(error) = value["error"].as_str() {
                return Err(error.to_owned());
            }
            value
                .get("result")
                .cloned()
                .ok_or_else(|| "闪电未返回任务结果".into())
        });
        if let Some(value) = response {
            if serde_json::to_writer(&mut stdout, &value).is_err()
                || stdout
                    .write_all(b"\n")
                    .and_then(|_| stdout.flush())
                    .is_err()
            {
                break;
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Add {
    url: String,
    target: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    #[serde(default)]
    include_done: bool,
    target: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    id: String,
    target: Option<String>,
}

fn parse<T: serde::de::DeserializeOwned>(arguments: &Value) -> Result<T, String> {
    serde_json::from_value(arguments.clone()).map_err(|_| "工具参数格式不正确".into())
}

fn target(value: Option<&str>) -> Result<&str, String> {
    let value = value.unwrap_or("local");
    if targets().contains(&value) {
        Ok(value)
    } else {
        Err("这个版本不支持所选下载位置".into())
    }
}

fn supported_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| match url.scheme() {
        "http" | "https" => url.has_host(),
        "magnet" | "ed2k" => crate::links::LinkKind::of(value).is_some(),
        _ => false,
    })
}

async fn add(app: &AppHandle, state: &AppState, args: Add) -> Result<Value, String> {
    let destination = target(args.target.as_deref())?;
    let url = args.url.trim();
    if !supported_url(url) {
        return Err("请提供磁力、ed2k 或 HTTP/HTTPS 链接".into());
    }
    let special = crate::links::is_pan_share(url)
        || crate::baidu::share_url(url).is_some()
        || crate::feishu::document_url(url).is_some()
        || crate::wechat::is_share(url);
    let confirm = special
        || crate::links::video_collection(url, false).is_some()
        || state.settings.lock().unwrap().confirm_new;
    if confirm {
        let window = app.get_webview_window("main").ok_or("闪电主窗口未就绪")?;
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        window
            .emit("mcp-add", json!({"url":url,"target":destination}))
            .map_err(|e| e.to_string())?;
        return Ok(json!({"status":"needs_confirmation","target":destination}));
    }
    let id = crate::engine::add_link(url, state).await?;
    Ok(json!({"status":"submitted","target":destination,"id":id}))
}

async fn list(app: &AppHandle, args: List) -> Result<Value, String> {
    let destination = target(args.target.as_deref())?;
    let state = app.state::<AppState>();
    let tasks = crate::engine::collect_tasks(&state).await?;
    let rows: Vec<_> = tasks
        .into_iter()
        .filter(|task| args.include_done || task.status != "done")
        .map(|task| {
            let name = task
                .name
                .as_deref()
                .filter(|name| !name.is_empty())
                .or_else(|| {
                    task.meta
                        .as_ref()?
                        .res
                        .as_ref()
                        .map(|res| res.name.as_str())
                })
                .unwrap_or("未命名任务");
            json!({"id":task.id,"name":name,"status":task.status,"target":destination,
                "downloaded_bytes":task.downloaded(),"total_bytes":task.total(),
                "speed_bytes_per_second":task.speed()})
        })
        .collect();
    Ok(json!({"target":destination,"tasks":rows}))
}

async fn control(app: &AppHandle, args: Control, pause: bool) -> Result<Value, String> {
    let id = args.id;
    let destination = target(args.target.as_deref())?;
    if id.is_empty()
        || id.ends_with(':')
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_:".contains(&c))
    {
        return Err("任务 ID 格式不正确".into());
    }
    let state = app.state::<AppState>();
    if pause {
        crate::engine::pause_task(id, state).await?;
    } else {
        crate::engine::resume_task(id, state).await?;
    }
    Ok(json!({"ok":true,"target":destination}))
}

pub(crate) async fn dispatch(app: &AppHandle, message: Value) -> Result<Value, String> {
    let args = &message["arguments"];
    match message["name"].as_str().unwrap_or("") {
        "add_download" => add(app, &app.state::<AppState>(), parse(args)?).await,
        "list_downloads" => list(app, parse(args)?).await,
        "pause_download" => control(app, parse(args)?, true).await,
        "resume_download" => control(app, parse(args)?, false).await,
        _ => Err("没有此下载工具".into()),
    }
}
