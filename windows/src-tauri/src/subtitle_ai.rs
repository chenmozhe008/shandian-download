//! AI subtitle service configuration and request transport for Windows.
use crate::{settings::protect_local_secret, AppState};
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, time::Duration};
use tauri::State;
use url::Url;

#[derive(Serialize)]
struct Provider {
    id: &'static str,
    name: &'static str,
    url: &'static str,
    suggested: &'static str,
    models: &'static [&'static str],
}

// First entry is the latest model available on the provider's general text API.
// Account-specific models are queried separately using the account's credentials.
const PROVIDERS: &[Provider] = &[
    provider_entry("ollama", "本机 Ollama", "", &[]),
    provider_entry("deepseek", "DeepSeek", "https://api.deepseek.com", &["deepseek-flash", "deepseek-v4-pro"]),
    provider_entry("kimi", "Kimi", "https://api.moonshot.cn/v1", &["kimi-k3.1-pro", "kimi-k3.1-pro-highspeed", "kimi-k2.6"]),
    provider_entry("qwen", "通义千问", "https://dashscope.aliyuncs.com/compatible-mode/v1", &["qwen4-max", "qwen3.8-max", "qwen3.8-flash", "qwen3.7-plus"]),
    provider_entry("doubao", "豆包", "https://ark.cn-beijing.volces.com/api/v3", &["doubao-seed-2-1-pro-260915", "doubao-seed-2-1-lite-260915", "doubao-seed-2-1-turbo-260628"]),
    provider_entry("zhipu", "智谱 GLM", "https://open.bigmodel.cn/api/paas/v4", &["glm-5.3", "glm-5.3-flash", "glm-5.2"]),
    provider_entry("openai", "OpenAI", "https://api.openai.com/v1", &["gpt-6.2-astra", "gpt-6.2-sol", "gpt-6.1-terra", "gpt-6-luna"]),
    provider_entry("gemini", "Gemini", "https://generativelanguage.googleapis.com/v1beta/openai", &["gemini-4-carbon", "gemini-4-neon", "gemini-4-argon", "gemini-4-hydrogen", "gemini-3.8-flash"]),
    provider_entry("mimo", "小米 MiMo", "https://api.xiaomimimo.com/v1", &["mimo-v2.6-pro", "mimo-v2.6-flash"]),
    provider_entry("anthropic", "Claude", "https://api.anthropic.com/v1", &["claude-fable-5-2", "claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"]),
    provider_entry("custom", "其他 OpenAI 兼容服务", "", &[]),
];

const fn provider_entry(id: &'static str, name: &'static str, url: &'static str, models: &'static [&'static str]) -> Provider {
    Provider { id, name, url, suggested: if models.is_empty() { "" } else { models[0] }, models }
}

fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|item| item.id == id)
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    selected: String,
    accounts: BTreeMap<String, Account>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Account {
    endpoint: String,
    model: String,
    explicit_model: bool,
    sealed_key: Vec<u8>,
}

fn config_path(local: &Path) -> std::path::PathBuf {
    local.join("subtitle-ai.json")
}

fn load(local: &Path) -> Saved {
    std::fs::read(config_path(local)).ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

impl Saved {
    fn selected(&self) -> &'static Provider {
        provider(&self.selected).unwrap_or(&PROVIDERS[0])
    }
}

fn is_loopback(url: &Url) -> bool {
    url.host().is_some_and(|host| match host {
        url::Host::Domain(host) => host == "localhost",
        url::Host::Ipv4(addr) => addr.is_loopback(),
        url::Host::Ipv6(addr) => addr.is_loopback(),
    })
}

fn validated_endpoint(value: &str) -> Result<Url, String> {
    let url = Url::parse(value.trim()).map_err(|_| "请输入有效的服务地址")?;
    if url.host().is_none() || !url.username().is_empty() || url.password().is_some()
        || url.query().is_some() || url.fragment().is_some()
        || !(url.scheme() == "https" || (url.scheme() == "http" && is_loopback(&url)))
    {
        return Err("服务地址须使用 HTTPS，本机服务可用 HTTP。请把密钥填在 API Key 栏。".into());
    }
    Ok(url)
}

pub enum Model {
    Ollama(String),
    Api(ApiModel),
}

pub struct ApiModel {
    base: Url,
    model: String,
    key: Option<String>,
    anthropic: bool,
}

fn configured_model<'a>(service: &Provider, account: &'a Account) -> &'a str {
    let model = account.model.trim();
    if matches!((service.id, model),
        ("mimo", "mimo-v2.5-pro" | "mimo-v2.5") | ("deepseek", "deepseek-chat" | "deepseek-reasoner"))
        || (!account.explicit_model && matches!((service.id, model),
            ("kimi", "kimi-latest") | ("qwen", "qwen-plus") | ("zhipu", "glm-4.5-flash")
            | ("openai", "gpt-5.5-mini") | ("gemini", "gemini-3-flash") | ("anthropic", "claude-haiku-5-5")))
    {
        return "";
    }
    model
}

fn effective_model<'a>(service: &Provider, account: &'a Account) -> &'a str {
    let model = configured_model(service, account);
    if model.is_empty() { service.suggested } else { model }
}

impl ApiModel {
    fn from_account(provider: &Provider, account: &Account) -> Result<Self, String> {
        let base = validated_endpoint(if provider.id == "custom" { &account.endpoint } else { provider.url })?;
        let key = read_key(account)?;
        Self::with_key(provider, base, effective_model(provider, account).into(), key)
    }

    fn with_key(provider: &Provider, base: Url, model: String, key: Option<String>) -> Result<Self, String> {
        if provider.id != "custom" && key.is_none() {
            return Err("请先填写该服务的 API Key".into());
        }
        Ok(Self { base, model, key, anthropic: provider.id == "anthropic" })
    }

    fn url(&self, suffix: &str) -> Url {
        let mut url = self.base.clone();
        url.set_path(&format!("{}{suffix}", self.base.path().trim_end_matches('/')));
        url
    }

    fn authorize(&self, request: RequestBuilder) -> RequestBuilder {
        if self.anthropic {
            let request = request.header("anthropic-version", "2023-06-01");
            if let Some(key) = &self.key { request.header("x-api-key", key) } else { request }
        } else if let Some(key) = &self.key {
            request.bearer_auth(key)
        } else {
            request
        }
    }

    fn read(&self, request: RequestBuilder) -> Result<Value, String> {
        let response: Response = self.authorize(request).send().map_err(|_| "无法连接 AI 字幕服务")?;
        if !response.status().is_success() {
            return Err(match response.status().as_u16() {
                401 | 403 => "AI 字幕服务拒绝授权，请检查 API Key 或模型权限".into(),
                429 => "AI 字幕服务限流或额度不足，请稍后重试".into(),
                code => format!("AI 字幕服务返回 HTTP {code}，请检查地址和模型"),
            });
        }
        response.json().map_err(|_| "AI 字幕服务返回了无法解析的内容".into())
    }

    pub fn client(&self, timeout: u64) -> Result<Client, String> {
        let builder = Client::builder().timeout(Duration::from_secs(timeout))
            .redirect(reqwest::redirect::Policy::none());
        let builder = if is_loopback(&self.base) { builder.no_proxy() } else { builder };
        builder.build().map_err(|_| "无法建立 AI 字幕连接".into())
    }

    pub fn generate(&self, client: &Client, prompt: &str) -> Result<String, String> {
        let messages = json!([{"role":"user","content":prompt}]);
        if self.anthropic {
            let body = json!({"model":self.model,"messages":messages,"max_tokens":4096});
            let reply = self.read(client.post(self.url("/messages")).json(&body))?;
            return reply["content"].as_array().into_iter().flatten()
                .find_map(|block| block["text"].as_str()).map(str::to_owned)
                .ok_or("AI 字幕服务没有返回文本".into());
        }
        let body = json!({"model":self.model,"messages":messages,"stream":false});
        let reply = self.read(client.post(self.url("/chat/completions")).json(&body))?;
        reply["choices"][0]["message"]["content"].as_str().map(str::to_owned)
            .ok_or("AI 字幕服务没有返回文本".into())
    }
}

pub fn selected_provider(local: &Path) -> &'static str {
    load(local).selected().id
}

pub fn model(local: &Path, preferred: &str) -> Option<Model> {
    let config = load(local);
    let service = config.selected();
    if service.id == "ollama" {
        let models = crate::polish::models();
        return models.iter().find(|item| item.as_str() == preferred).or(models.first())
            .cloned().map(Model::Ollama);
    }
    let account = config.accounts.get(service.id)?;
    if effective_model(service, account).is_empty() { return None; }
    ApiModel::from_account(service, account).ok().map(Model::Api)
}

fn info(local: &Path) -> Value {
    let config = load(local);
    let selected = config.selected();
    let accounts: BTreeMap<_, _> = config.accounts.iter().map(|(id, account)| {
        let configured = provider(id).map_or(account.model.as_str(), |service| configured_model(service, account));
        (id, json!({"endpoint":account.endpoint,"model":configured,"hasKey":!account.sealed_key.is_empty()}))
    }).collect();
    json!({"providers":PROVIDERS,"selected":selected.id,"accounts":accounts})
}

#[tauri::command]
pub fn subtitle_ai_settings(state: State<'_, AppState>) -> Value {
    info(&state.local)
}

#[tauri::command]
pub fn save_subtitle_ai(provider_id: String, endpoint: String, model: String, key: Option<String>, forget: bool, state: State<'_, AppState>) -> Result<Value, String> {
    static SAVE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = SAVE.lock().map_err(|_| "暂时无法保存 AI 字幕配置")?;
    let service = provider(&provider_id).ok_or("请选择字幕服务")?;
    let mut saved = load(&state.local);
    saved.selected = service.id.into();
    if service.id != "ollama" {
        let account = saved.accounts.entry(service.id.into()).or_default();
        let endpoint = if service.id == "custom" && !endpoint.trim().is_empty() {
            validated_endpoint(&endpoint)?.to_string()
        } else {
            String::new()
        };
        // A credential for a custom URL must never follow a new URL.
        if account.endpoint != endpoint && service.id == "custom" {
            account.sealed_key.clear();
        }
        account.endpoint = endpoint;
        account.model = model.trim().into();
        account.explicit_model = !account.model.is_empty();
        if forget {
            account.sealed_key.clear();
        } else if let Some(key) = key.filter(|key| !key.trim().is_empty()) {
            account.sealed_key = protect_local_secret(key.trim().as_bytes().to_vec(), true)
                .ok_or("无法安全保存 API Key")?;
        }
    }
    std::fs::create_dir_all(&state.local).map_err(|_| "无法打开 AI 字幕配置目录")?;
    let bytes = serde_json::to_vec(&saved).map_err(|_| "无法保存 AI 字幕配置")?;
    let path = config_path(&state.local);
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes).and_then(|()| std::fs::rename(&temporary, path))
        .map_err(|_| "无法保存 AI 字幕配置")?;
    Ok(info(&state.local))
}

#[tauri::command]
pub async fn subtitle_ai_status(state: State<'_, AppState>) -> Result<Value, String> {
    let local = state.local.clone();
    let preferred = state.settings.lock().unwrap().polish_model.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let selected = load(&local).selected();
        json!({"available":model(&local,&preferred).is_some(),"local":selected.id=="ollama","name":selected.name,"id":selected.id})
    }).await.map_err(|_| "无法检测 AI 字幕服务".into())
}

fn read_key(account: &Account) -> Result<Option<String>, String> {
    if account.sealed_key.is_empty() { return Ok(None); }
    String::from_utf8(protect_local_secret(account.sealed_key.clone(), false)
        .ok_or("无法读取 API Key，请重新保存")?).map(Some).map_err(|_| "无法读取 API Key，请重新保存".into())
}

fn api_models(api: &ApiModel) -> Result<Vec<String>, String> {
    let conn = api.client(15)?;
    let list = api.read(conn.get(api.url("/models")))?;
    let mut models: Vec<_> = list["data"].as_array().into_iter().flatten().filter_map(|item| {
        let id = item["id"].as_str()?;
        let name = id.to_ascii_lowercase();
        let non_text = ["embedding", "rerank", "moderation", "transcribe", "whisper", "tts", "realtime", "image-gen", "gpt-image", "dall-e", "sora", "audio-preview"];
        (!id.is_empty() && !non_text.iter().any(|kind| name.contains(kind))).then(|| id.to_owned())
    }).collect();
    models.sort_unstable_by(|a, b| b.cmp(a));
    models.dedup();
    Ok(models)
}

#[tauri::command]
pub async fn subtitle_ai_models(provider_id: String, endpoint: String, key: Option<String>, state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let local = state.local.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let service = provider(&provider_id).filter(|p| p.id != "ollama").ok_or("请选择 API 字幕服务")?;
        let base = validated_endpoint(if service.id == "custom" { &endpoint } else { service.url })?;
        let mut key = key.map(|value| value.trim().to_owned()).filter(|value| !value.is_empty());
        if key.is_none() {
            let settings = load(&local);
            if let Some(saved) = settings.accounts.get(service.id).filter(|saved| {
                service.id != "custom" || validated_endpoint(&saved.endpoint).ok().as_ref() == Some(&base)
            }) {
                key = read_key(saved)?;
            }
        }
        api_models(&ApiModel::with_key(service, base, String::new(), key)?)
    }).await.map_err(|_| "模型列表查询未完成".to_string())?
}

#[tauri::command]
pub async fn test_subtitle_ai(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let local = state.local.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let config = load(&local);
        let service = config.selected();
        if service.id == "ollama" {
            let models = crate::polish::models();
            return if models.is_empty() { Err("未发现可用的本机 Ollama 模型".into()) } else { Ok(models) };
        }
        let account = config.accounts.get(service.id).ok_or("请先保存服务配置")?;
        let api = ApiModel::from_account(service, account)?;
        api_models(&api)
    }).await.map_err(|_| "连接测试未完成".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn subtitle_ai_migrates_old_defaults_and_keeps_explicit_models() {
        let mimo = provider("mimo").unwrap();
        let account = Account { model: "mimo-v2.5-pro".into(), ..Default::default() };
        assert_eq!(configured_model(mimo, &account), "");
        assert_eq!(effective_model(mimo, &account), "mimo-v2.6-pro");
        let qwen = provider("qwen").unwrap();
        let mut account = Account { model: "qwen-plus".into(), ..Default::default() };
        assert_eq!(effective_model(qwen, &account), "qwen4-max");
        account.explicit_model = true;
        assert_eq!(effective_model(qwen, &account), "qwen-plus");
        for provider in PROVIDERS.iter().filter(|provider| !provider.url.is_empty()) {
            assert_eq!(provider.models.first(), Some(&provider.suggested));
        }
    }

    #[test]
    fn subtitle_ai_model_catalog_uses_saved_protocol_and_filters_non_text() {
        let server = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let base = format!("http://{}/v1", server.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let (socket, _) = server.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut connection = BufReader::new(socket);
            let mut request = String::new();
            connection.read_line(&mut request).unwrap();
            assert!(request.starts_with("GET /v1/models "));
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                connection.read_line(&mut line).unwrap();
                if line == "\r\n" { break; }
                assert!(!line.is_empty());
                headers.push_str(&line);
            }
            assert!(headers.to_ascii_lowercase().contains("x-api-key: fixture-key"));
            let body = r#"{"data":[{"id":"fixture-text"},{"id":"fixture-tts"},{"id":"fixture-text"},{"id":"text-embedding-3-small"}]}"#;
            write!(connection.get_mut(), "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        });
        let api = ApiModel {
            base: validated_endpoint(&base).unwrap(), model: String::new(),
            key: Some("fixture-key".into()), anthropic: true,
        };
        assert_eq!(api_models(&api).unwrap(), ["fixture-text"]);
        worker.join().unwrap();
    }

    #[test]
    fn subtitle_ai_uses_the_provider_protocol() {
        for anthropic in [false, true] {
            let server = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let base = format!("http://{}/v1", server.local_addr().unwrap());
            let worker = thread::spawn(move || {
                let (socket, _) = server.accept().unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut connection = BufReader::new(socket);
                let mut request = String::new();
                connection.read_line(&mut request).unwrap();
                assert!(request.starts_with(if anthropic { "POST /v1/messages " } else { "POST /v1/chat/completions " }));
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    connection.read_line(&mut line).unwrap();
                    if line == "\r\n" { break; }
                    assert!(!line.is_empty());
                    headers.push_str(&line);
                }
                let lower = headers.to_ascii_lowercase();
                assert!(lower.contains(if anthropic { "x-api-key: fixture-key" } else { "authorization: bearer fixture-key" }));
                let count = lower.lines().find_map(|line| line.strip_prefix("content-length:")).unwrap().trim().parse().unwrap();
                let mut body = vec![0; count];
                connection.read_exact(&mut body).unwrap();
                let request: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(request["messages"][0]["content"], "字幕测试");
                let body = if anthropic {
                    r#"{"content":[{"type":"text","text":"已翻译"}]}"#
                } else {
                    r#"{"choices":[{"message":{"content":"已翻译"}}]}"#
                };
                write!(connection.get_mut(), "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            });
            let api = ApiModel { base: validated_endpoint(&base).unwrap(), model: "fixture-model".into(), key: Some("fixture-key".into()), anthropic };
            let conn = Client::builder().no_proxy().timeout(Duration::from_secs(5)).build().unwrap();
            assert_eq!(api.generate(&conn, "字幕测试").unwrap(), "已翻译");
            worker.join().unwrap();
        }
    }

    #[test]
    fn subtitle_ai_does_not_accept_credentials_in_service_urls() {
        assert!(validated_endpoint("https://example.com/v1").is_ok());
        assert!(validated_endpoint("http://localhost:1234/v1").is_ok());
        for url in ["http://example.com/v1", "https://user:pass@example.com/v1", "https://example.com/v1?key=abc"] {
            assert!(validated_endpoint(url).is_err(), "{url}");
        }
    }
}
