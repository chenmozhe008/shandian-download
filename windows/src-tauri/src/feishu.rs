use crate::{api::MultiFile, AppState};
use quick_xml::{events::Event, Reader};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    io::{self, Read},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};
use url::Url;

const CLI_FAILED: &str = "无法读取飞书文档，请检查网络和文档访问权限";
const POLICY_BLOCKED: &str = "飞书数据安全策略阻止了应用授权（21001）";
pub(crate) const SCOPES_REQUIRED: &str = "飞书还未授予所需的文档和附件权限";
pub(crate) const READ_SCOPES: &str =
    "docx:document:readonly docs:document.media:download wiki:node:read";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuError {
    message: String,
    needs_authorization: bool,
    policy_blocked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    permission_url: Option<String>,
}

impl FeishuError {
    fn authorization() -> Self {
        Self {
            message: "授权飞书后即可读取文档音视频".into(),
            needs_authorization: true,
            policy_blocked: false,
            permission_url: None,
        }
    }

    pub(crate) fn with_permissions(mut self, local: &Path) -> Self {
        if self.policy_blocked || self.message == SCOPES_REQUIRED {
            let app_id = executable(local).and_then(|cli| {
                run_json(&cli, &["auth", "status", "--json"], CLI_FAILED)
                    .ok()?
                    .get("appId")?
                    .as_str()
                    .map(str::to_owned)
            });
            self.permission_url = Some(feishu_permissions_url(app_id.as_deref()));
        }
        self
    }

    pub(crate) fn authorization_failure(message: impl Into<String>) -> Self {
        let mut error = Self::from(message.into());
        error.needs_authorization = !error.policy_blocked && error.message != SCOPES_REQUIRED;
        error
    }
}

impl From<String> for FeishuError {
    fn from(message: String) -> Self {
        Self {
            policy_blocked: message == POLICY_BLOCKED,
            message,
            needs_authorization: false,
            permission_url: None,
        }
    }
}

impl From<&str> for FeishuError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
const MEDIA_FAILED: &str = "飞书未返回全部所选附件的下载地址，请检查附件权限";
const MAX_RESPONSE: u64 = 24 * 1024 * 1024;

fn on_domain(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn trusted_https(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
}

fn token(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|b| b.is_ascii_alphanumeric())
}

pub fn document_url(raw: &str) -> Option<Url> {
    let mut url = Url::parse(raw.trim()).ok()?;
    if !trusted_https(&url) || !on_domain(url.host_str()?, "feishu.cn") {
        return None;
    }
    let mut parts = url.path().trim_end_matches('/').split('/');
    if parts.next() != Some("")
        || !matches!(parts.next(), Some("docx" | "wiki"))
        || !parts.next().is_some_and(token)
        || parts.next().is_some()
    {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    Some(url)
}

pub(crate) fn attachment_url(url: &Url) -> bool {
    trusted_https(url)
        && url
            .host_str()
            .is_some_and(|host| on_domain(host, "feishu.cn"))
        && url.path().starts_with("/space/api/box/stream/download/")
}

fn media_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    (trusted_https(&url) && (on_domain(host, "feishu.cn") || on_domain(host, "feishucdn.com")))
        .then_some(url)
}

pub(crate) struct FeishuCli {
    pub(crate) bin: PathBuf,
    profile: Option<PathBuf>,
}

impl FeishuCli {
    pub(crate) fn managed(local: &Path) -> Self {
        Self {
            bin: local.join("bin/feishu/lark-cli.exe"),
            profile: Some(local.join("feishu-cli")),
        }
    }

    pub(crate) fn command(&self) -> Command {
        let mut cmd = Command::new(&self.bin);
        cmd.creation_flags(0x08000000)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null());
        if let Some(profile) = &self.profile {
            cmd.env("LARKSUITE_CLI_CONFIG_DIR", profile);
        }
        cmd
    }

    pub(crate) fn configured(&self) -> bool {
        run_json(self, &["config", "show"], CLI_FAILED).is_ok()
    }

    pub(crate) fn has_user_identity(&self) -> bool {
        run_json(self, &["auth", "status", "--json"], CLI_FAILED)
            .is_ok_and(|data| data["identity"] == "user")
    }

    pub(crate) fn has_required_scopes(&self) -> bool {
        run_json(
            self,
            &["auth", "check", "--scope", READ_SCOPES, "--json"],
            CLI_FAILED,
        )
        .is_ok_and(|data| data["ok"] == true)
    }

    pub(crate) fn authorized(&self) -> bool {
        self.has_user_identity() && self.has_required_scopes()
    }
}

pub(crate) fn executable(local: &Path) -> Option<FeishuCli> {
    let managed = FeishuCli::managed(local);
    if managed.bin.is_file() {
        return Some(managed);
    }
    let npm = std::env::var_os("APPDATA")
        .map(|dir| PathBuf::from(dir).join("npm/node_modules/@larksuite/cli/bin"));
    let home = std::env::var_os("USERPROFILE").map(|dir| PathBuf::from(dir).join(".local/bin"));
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::iter::once(local.join("bin"))
        .chain(npm)
        .chain(home)
        .chain(std::env::split_paths(&path))
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("lark-cli.exe"))
        .find(|path| path.is_file())
        .map(|bin| FeishuCli { bin, profile: None })
}

#[derive(Clone, Serialize)]
struct Attachment {
    #[serde(skip_serializing)]
    token: String,
    name: String,
    size: Option<u64>,
}

fn parse_document(xml: &str) -> Result<(String, Vec<Attachment>), String> {
    let mut reader = Reader::from_str(xml);
    let mut title = String::new();
    let mut in_title = false;
    let mut tokens = HashSet::new();
    let mut names = HashSet::new();
    let mut files = Vec::new();
    loop {
        match reader.read_event().map_err(|_| CLI_FAILED)? {
            Event::Start(tag) if tag.local_name().as_ref() == "title" => in_title = true,
            Event::End(tag) if tag.local_name().as_ref() == "title" => in_title = false,
            Event::Text(text) if in_title => title.push_str(text.as_ref()),
            Event::CData(text) if in_title => title.push_str(text.as_ref()),
            Event::GeneralRef(reference) if in_title => {
                if let Some(c) = reference.resolve_char_ref().map_err(|_| CLI_FAILED)? {
                    title.push(c);
                } else if let Some(entity) =
                    quick_xml::escape::resolve_xml_entity(reference.as_ref())
                {
                    title.push_str(entity);
                }
            }
            Event::Start(tag) | Event::Empty(tag) if tag.local_name().as_ref() == "source" => {
                let mut attrs = HashMap::new();
                for attr in tag.attributes() {
                    let attr = attr.map_err(|_| CLI_FAILED)?;
                    if matches!(attr.key.as_ref(), "mime" | "token" | "name" | "size") {
                        attrs.insert(
                            attr.key.as_ref().to_owned(),
                            attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .map_err(|_| CLI_FAILED)?
                                .into_owned(),
                        );
                    }
                }
                let (Some(mime), Some(id), Some(name)) =
                    (attrs.get("mime"), attrs.get("token"), attrs.get("name"))
                else {
                    continue;
                };
                let mime = mime.to_ascii_lowercase();
                if !(mime.starts_with("audio/") || mime.starts_with("video/"))
                    || !token(id)
                    || name.is_empty()
                    || !tokens.insert(id.clone())
                {
                    continue;
                }
                let original = safe_name(name, "飞书附件");
                let mut candidate = original.clone();
                let mut n = 2;
                while !names.insert(candidate.to_lowercase()) {
                    let path = Path::new(&original);
                    let stem = path
                        .file_stem()
                        .and_then(|part| part.to_str())
                        .unwrap_or(&original);
                    let suffix = path
                        .extension()
                        .and_then(|part| part.to_str())
                        .map(|ext| format!(".{ext}"))
                        .unwrap_or_default();
                    candidate = format!("{stem} ({n}){suffix}");
                    n += 1;
                }
                files.push(Attachment {
                    token: id.clone(),
                    name: candidate,
                    size: attrs
                        .get("size")
                        .and_then(|s| s.parse().ok())
                        .filter(|n| *n > 0),
                });
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok((safe_name(title.trim(), "飞书文档音视频"), files))
}

fn safe_name(raw: &str, fallback: &str) -> String {
    let mut name = crate::engine::safe_component(raw);
    if name.is_empty() {
        return fallback.to_string();
    }
    let head = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(
        head.as_str(),
        "CON" | "CONIN$" | "CONOUT$" | "PRN" | "AUX" | "NUL"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        head.strip_prefix(prefix).is_some_and(|part| {
            matches!(
                part,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    });
    if reserved {
        name.insert(0, '_');
    }
    name
}

fn read_pipe(mut pipe: impl Read, limit: u64) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    pipe.by_ref().take(limit + 1).read_to_end(&mut data)?;
    io::copy(&mut pipe, &mut io::sink())?;
    Ok(data)
}

fn feishu_permissions_url(app_id: Option<&str>) -> String {
    if let Some(id) = app_id.filter(|id| {
        id.strip_prefix("cli_").is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix.len() <= 64
                && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    }) {
        format!("https://open.feishu.cn/app/{id}/auth")
    } else {
        "https://open.feishu.cn/app".into()
    }
}

pub(crate) fn cli_permission_issue(data: &[u8]) -> Option<&'static str> {
    let start = data.iter().position(|&byte| byte == b'{')?;
    serde_json::Deserializer::from_slice(&data[start..])
        .into_iter::<Value>()
        .map_while(Result::ok)
        .find_map(|entry| match entry["error"]["code"].as_i64()? {
            21001 if entry["error"]["type"] == "policy" => Some(POLICY_BLOCKED),
            99991672 | 99991679 => Some(SCOPES_REQUIRED),
            _ => None,
        })
}

fn run_json(cli: &FeishuCli, args: &[&str], failure: &str) -> Result<Value, String> {
    let mut child = cli
        .command()
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| failure)?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let reader = std::thread::spawn(move || read_pipe(stdout, MAX_RESPONSE));
    let errors = std::thread::spawn(move || read_pipe(stderr, 64 * 1024));
    let since = Instant::now();
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if since.elapsed() < Duration::from_secs(30) => {
                std::thread::sleep(Duration::from_millis(50))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let out = reader
        .join()
        .ok()
        .and_then(Result::ok)
        .filter(|data| data.len() as u64 <= MAX_RESPONSE)
        .ok_or(failure)?;
    let stderr = errors.join().ok().and_then(Result::ok).unwrap_or_default();
    if let Some(message) = cli_permission_issue(&out).or_else(|| cli_permission_issue(&stderr)) {
        return Err(message.into());
    }
    if !ok {
        return Err(failure.into());
    }
    serde_json::from_slice(&out).map_err(|_| failure.into())
}

fn call(cli: &FeishuCli, args: &[&str], failure: &str) -> Result<Value, String> {
    let response = run_json(cli, args, failure)?;
    if response["ok"].as_bool() != Some(true) {
        return Err(failure.into());
    }
    Ok(response["data"].clone())
}

struct Document {
    url: Url,
    title: String,
    files: Vec<Attachment>,
    opened: Instant,
}

static DOCUMENTS: LazyLock<Mutex<HashMap<String, Document>>> = LazyLock::new(Default::default);

#[derive(Serialize)]
pub struct DocumentPreview {
    ticket: String,
    title: String,
    files: Vec<Attachment>,
}

#[tauri::command]
pub async fn probe_feishu_document(
    link: String,
    state: tauri::State<'_, AppState>,
) -> Result<DocumentPreview, FeishuError> {
    let local = state.local.clone();
    tauri::async_runtime::spawn_blocking(move || preview_document(&link, &local))
        .await
        .map_err(|_| CLI_FAILED)?
}

pub(crate) fn preview_document(link: &str, local: &Path) -> Result<DocumentPreview, FeishuError> {
    let url = document_url(link).ok_or("飞书文档链接无效")?;
    let cli = executable(local).ok_or_else(FeishuError::authorization)?;
    let data = call(
        &cli,
        &["docs", "+fetch", "--as", "user", "--doc", url.as_str()],
        CLI_FAILED,
    )
    .or_else(|error| {
        if error == POLICY_BLOCKED || error == SCOPES_REQUIRED || cli.authorized() {
            return Err(error.into());
        }
        Err(FeishuError::authorization())
    })
    .map_err(|error| error.with_permissions(local))?;
    let xml = data["document"]["content"].as_str().ok_or(CLI_FAILED)?;
    let (title, files) = parse_document(xml)?;
    if files.is_empty() {
        return Err("没有找到可下载的原始音视频附件".into());
    }
    let ticket = format!("{:032x}", rand::random::<u128>());
    let mut documents = DOCUMENTS.lock().unwrap();
    documents.retain(|_, doc| doc.opened.elapsed() < Duration::from_secs(900));
    documents.insert(
        ticket.clone(),
        Document {
            url,
            title: title.clone(),
            files: files.clone(),
            opened: Instant::now(),
        },
    );
    Ok(DocumentPreview {
        ticket,
        title,
        files,
    })
}

fn resolve(
    cli: &FeishuCli,
    document: &Document,
    selected: &[usize],
) -> Result<Vec<MultiFile>, String> {
    let mut seen = HashSet::new();
    let files: Vec<_> = selected
        .iter()
        .filter(|i| seen.insert(**i))
        .map(|&i| document.files.get(i).ok_or("飞书附件选择已失效"))
        .collect::<Result<_, _>>()?;
    if files.is_empty() {
        return Err("请至少勾选一个飞书附件".into());
    }
    let mut links = HashMap::new();
    for group in files.chunks(5) {
        let ids: HashSet<&str> = group.iter().map(|file| file.token.as_str()).collect();
        let params =
            json!({"file_tokens":group.iter().map(|file| &file.token).collect::<Vec<_>>()})
                .to_string();
        let data = call(
            cli,
            &[
                "api",
                "GET",
                "/open-apis/drive/v1/medias/batch_get_tmp_download_url",
                "--as",
                "user",
                "--params",
                &params,
            ],
            MEDIA_FAILED,
        )?;
        for item in data["tmp_download_urls"].as_array().ok_or(MEDIA_FAILED)? {
            let Some(id) = item["file_token"].as_str().filter(|id| ids.contains(id)) else {
                continue;
            };
            if let Some(link) = item["tmp_download_url"].as_str().and_then(media_url) {
                links.insert(id.to_string(), link);
            }
        }
    }
    files
        .into_iter()
        .map(|file| {
            let url = links.remove(&file.token).ok_or(MEDIA_FAILED)?;
            Ok(MultiFile {
                url: url.into(),
                name: file.name.clone(),
                headers: vec![("Referer".into(), document.url.to_string())],
            })
        })
        .collect()
}

#[tauri::command]
pub async fn add_feishu_document(
    ticket: String,
    selected: Vec<usize>,
    name: String,
    dir: String,
    state: tauri::State<'_, AppState>,
) -> Result<String, FeishuError> {
    download_selection(ticket, selected, Some(name), Some(dir), None, state).await
}

pub(crate) async fn download_selection(
    ticket: String,
    selected: Vec<usize>,
    name: Option<String>,
    dir: Option<String>,
    expected_url: Option<Url>,
    state: tauri::State<'_, AppState>,
) -> Result<String, FeishuError> {
    let dir = dir
        .map(|dir| dir.trim().replace('/', "\\"))
        .unwrap_or_default();
    if !dir.is_empty() && !Path::new(&dir).is_absolute() {
        return Err("请选择 Windows 保存目录".into());
    }
    if selected.is_empty() {
        return Err("请至少勾选一个飞书附件".into());
    }
    let document = {
        let mut documents = DOCUMENTS.lock().unwrap();
        let matches = documents.get(&ticket).is_some_and(|doc| {
            doc.opened.elapsed() < Duration::from_secs(900)
                && expected_url.as_ref().is_none_or(|url| url == &doc.url)
        });
        if !matches {
            return Err("附件清单已失效，请重新读取飞书文档".into());
        }
        documents
            .remove(&ticket)
            .expect("validated document ticket")
    };
    let local = state.local.clone();
    let (document, resolved) = tauri::async_runtime::spawn_blocking(move || {
        let result = executable(&local)
            .ok_or_else(|| "请重新打开飞书文档并授权".to_string())
            .and_then(|cli| resolve(&cli, &document, &selected))
            .map_err(|message| FeishuError::from(message).with_permissions(&local));
        (document, result)
    })
    .await
    .map_err(|_| MEDIA_FAILED)?;
    let result = match resolved {
        Ok(files) if files.len() == 1 => {
            let file = &files[0];
            crate::engine::add_link_full(&file.url, &state, &file.name, &dir, &file.headers, None)
                .await
                .map_err(FeishuError::from)
        }
        Ok(files) => {
            let folder = name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(&document.title);
            crate::engine::add_multi(safe_name(folder, "飞书文档音视频"), Some(dir), files, state)
                .await
                .map_err(FeishuError::from)
        }
        Err(error) => Err(error),
    };
    if result.is_err() {
        DOCUMENTS.lock().unwrap().insert(ticket, document);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_feishu_policy_refusal_even_after_cli_progress_text() {
        let output = br#"Querying app scopes...
{"ok":false,"error":{"type":"policy","subtype":"access_denied","code":21001}}
"#;
        assert_eq!(cli_permission_issue(output), Some(POLICY_BLOCKED));
        let authorization = br#"{"event":"device_authorization"}
{"ok":false,"error":{"type":"policy","code":21001}}
"#;
        assert_eq!(cli_permission_issue(authorization), Some(POLICY_BLOCKED));
        assert_eq!(
            cli_permission_issue(br#"{"error":{"type":"api","code":99991679}}"#),
            Some(SCOPES_REQUIRED)
        );
        assert_eq!(
            feishu_permissions_url(Some("cli_fixture123")),
            "https://open.feishu.cn/app/cli_fixture123/auth"
        );
        assert_eq!(
            feishu_permissions_url(Some("cli_../../elsewhere")),
            "https://open.feishu.cn/app"
        );
        let error = FeishuError::authorization_failure(POLICY_BLOCKED);
        assert!(error.policy_blocked && !error.needs_authorization);
    }
    #[test]
    fn document_media_preserves_order_and_safe_original_names() {
        let (title, files) = parse_document(
            r#"<title>项目 &amp; 测试</title>
<figure><source token="v1" mime="video/mp4" name="镜头 &amp; 一.mp4" size="1024"/></figure>
<source token="a1" mime="audio/wav" name="声音.wav"/>
<source token="v1" mime="video/mp4" name="duplicate.mp4"/>
<source token="a2" mime="audio/wav" name="声音.wav"/>
<source token="z1" mime="application/zip" name="ignore.zip"/>"#,
        )
        .unwrap();
        assert_eq!(title, "项目 & 测试");
        assert_eq!(
            files
                .iter()
                .map(|f| (f.name.as_str(), f.size))
                .collect::<Vec<_>>(),
            [
                ("镜头 & 一.mp4", Some(1024)),
                ("声音.wav", None),
                ("声音 (2).wav", None)
            ]
        );
    }

    #[test]
    fn document_and_media_use_provider_https_origins() {
        assert_eq!(
            document_url("https://example.feishu.cn/wiki/ABC1?from=x#point")
                .unwrap()
                .as_str(),
            "https://example.feishu.cn/wiki/ABC1"
        );
        assert!(document_url("https://example.feishu.cn.example/wiki/ABC1").is_none());
        assert!(document_url("https://example.feishu.cn/docx/../other").is_none());
        assert!(media_url("https://cdn.feishucdn.com/file?id=sample").is_some());
        assert!(media_url("https://feishucdn.com.example/file").is_none());
    }
}
