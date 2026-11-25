//! GitHub 和 Hugging Face 公开文件走公共加速站。
//! 部分链接识别、请求头过滤及探测逻辑参考并改写自 Ghost Downloader 的 github_pack（GPL-3.0）。
//! 上游作者 XiaoYouChR 及贡献者；2026-11-13 引入改写，详见 docs/third-party-notices/ghost-downloader-reference.md。
//!
//! GitHub 用「加速站 + / + 原链接」，Hugging Face 替换域名。建任务前把各加速站和原链接一起测一遍，挑最快的交给内核；
//! 任务出错时换下一个能用的站接着下（同一个文件字节一样，进度保留）。
//! 界面、查重、重新下载看到的都是原链接（`original`），加速站只在内核那一层。
//!
//! 只比首包快慢不够：2026-11-13 实测有的站 0.2 秒就回 206，之后几乎不出数据。
//! 所以分两轮：先看谁能回 206，再让最先回来的几个各下 512 KB 比实际速度。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 2026-11-13 本机实测可用（206 且实际出数据）的站。403、连不上、只回头不出数据的没收
pub const MIRRORS: &[&str] = &[
    "https://gh-proxy.com",
    "https://edgeone.gh-proxy.org",
    "https://hk.gh-proxy.org",
    "https://cdn.gh-proxy.org",
    "https://gh-proxy.org",
    "https://fastgit.cc",
    "https://github.ednovas.xyz",
    "https://gh.monlor.com",
    "https://ghfast.top",
    "https://gh.nxnow.top",
];

const HF_MIRRORS: &[&str] = &["https://hf-mirror.com"];

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Source {
    GitHub,
    HuggingFace,
}

impl Source {
    fn probe_timeout(self) -> Duration {
        match self {
            // resolve 会经仓库接口和 CDN 多次跳转，实测正常首包也可能超过 5 秒。
            Self::HuggingFace => Duration::from_secs(12),
            Self::GitHub => PROBE_TIMEOUT,
        }
    }

    fn sample_time(self) -> Duration {
        match self {
            Self::HuggingFace => Duration::from_secs(8),
            Self::GitHub => SAMPLE_TIME,
        }
    }

    fn mirrors(self) -> &'static [&'static str] {
        match self {
            Self::GitHub => MIRRORS,
            Self::HuggingFace => HF_MIRRORS,
        }
    }
}

/// 只接公开模型和数据集的 resolve 文件直链；页面、API、签名和带 Token 的 URL 不参与。
pub fn is_huggingface_file(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    if !matches!(u.scheme(), "http" | "https") || u.host_str() != Some("huggingface.co")
        || !u.username().is_empty() || u.password().is_some()
        || u.query_pairs().any(|(key, _)| key != "download") {
        return false;
    }
    let segments: Vec<_> = u.path().trim_start_matches('/').split('/').collect();
    let file = if segments.first() == Some(&"datasets") { &segments[1..] } else { &segments[..] };
    file.len() >= 5 && file[2] == "resolve" && file.iter().all(|s| !s.is_empty())
        && !matches!(file[0], "api" | "spaces" | "datasets")
}

fn source(url: &str) -> Option<Source> {
    if is_github_file(url) { Some(Source::GitHub) }
    else if is_huggingface_file(url) { Some(Source::HuggingFace) }
    else { None }
}

pub fn is_supported_file(url: &str) -> bool {
    source(&original(url)).is_some()
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// 第一轮回来后最多再等多久收别的站
const FIRST_ROUND_GRACE: Duration = Duration::from_millis(1500);
/// 第二轮测速的站数、每个站下多少、最多测多久
const FINALISTS: usize = 5;
const SAMPLE_BYTES: u64 = 512 * 1024;
const SAMPLE_TIME: Duration = Duration::from_secs(3);
/// 小于这个大小的文件不测速，谁先回 206 用谁（测速本身就够把它下完了）
const SAMPLE_MIN_SIZE: u64 = 8 << 20;
/// 测出来的最快站按来源分别缓存，批量加链接时不用每条都测
const CACHE_TTL: Duration = Duration::from_secs(600);
/// 一个任务最多自动换几次站
const MAX_SWITCHES: usize = 3;

const GITHUB_HOSTS: &[&str] = &[
    "codeload.github.com",
    "gist.githubusercontent.com",
    "github-releases.githubusercontent.com",
    "media.githubusercontent.com",
    "objects.githubusercontent.com",
    "raw.githubusercontent.com",
    "raw.github.com",
    "release-assets.githubusercontent.com",
];

/// 是不是 GitHub 上的文件（Release 附件、源码包、raw 文件）。仓库页面、API 不算
pub fn is_github_file(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    if !matches!(u.scheme(), "http" | "https") {
        return false;
    }
    let host = u.host_str().unwrap_or("").to_lowercase();
    let host = host.trim_start_matches("www.");
    if GITHUB_HOSTS.contains(&host) {
        return true;
    }
    if host != "github.com" {
        return false;
    }
    let path = u.path().to_lowercase();
    ["/archive/", "/raw/", "/releases/download/", "/releases/latest/download/"]
        .iter()
        .any(|p| path.contains(p))
}

/// 源码包（archive、codeload）一般不支持断点续传，判断条件放宽
fn is_archive(url: &str) -> bool {
    url.contains("codeload.github.com") || url.contains("/archive/")
}

/// 加速站链接拆成（加速站, 原链接）；不是加速站链接返回 None
pub fn split(url: &str) -> Option<(&'static str, String)> {
    let hf = HF_MIRRORS.iter().find_map(|m| {
        let rest = url.strip_prefix(m)?.strip_prefix('/')?;
        let orig = format!("https://huggingface.co/{rest}");
        is_huggingface_file(&orig).then(|| (*m, orig))
    });
    hf.or_else(|| MIRRORS.iter().find_map(|m| {
        let rest = url.strip_prefix(m)?.strip_prefix('/')?;
        is_github_file(rest).then(|| (*m, rest.to_string()))
    }))
}

/// 用户看到的原链接
pub fn original(url: &str) -> String {
    split(url).map(|(_, o)| o).unwrap_or_else(|| url.to_string())
}

/// 发给加速站的请求头：去掉登录信息，别把 GitHub 的 token、Cookie 交给第三方
pub fn mirror_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(k, _)| !k.eq_ignore_ascii_case("authorization") && !k.eq_ignore_ascii_case("cookie"))
        .cloned()
        .collect()
}

fn candidate(mirror: Option<&str>, original: &str) -> String {
    match mirror {
        Some(m) if HF_MIRRORS.contains(&m) => {
            let u = url::Url::parse(original).expect("validated Hugging Face URL");
            let query = u.query().map(|q| format!("?{q}")).unwrap_or_default();
            format!("{m}{}{query}", u.path())
        }
        Some(m) => format!("{m}/{original}"),
        None => original.to_string(),
    }
}

fn client() -> Option<reqwest::Client> {
    // 同一个测速客户端复用连接池，批量选站和失败回退不用反复做 DNS / TLS 握手。
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    // 内核下 HTTP 直链不走代理，测速也不走，结果才对得上
    CLIENT.get_or_init(|| reqwest::Client::builder()
        .no_proxy()
        // 总请求时限还会约束连接阶段；不能让这里的 5 秒先截断 HF 的 12 秒探测。
        .connect_timeout(Source::HuggingFace.probe_timeout())
        .timeout(PROBE_TIMEOUT + SAMPLE_TIME)
        .user_agent(crate::settings::user_agent())
        .build()
        .ok()).clone()
}

/// HF 的连接失败在同一时间预算内重试一次。
async fn send_request(req: reqwest::RequestBuilder, url: &str, timeout: Duration) -> Result<reqwest::Response, reqwest::Error> {
    let retry = is_huggingface_file(&original(url)).then(|| req.try_clone()).flatten();
    let start = Instant::now();
    match req.timeout(timeout).send().await {
        // 实测 HF 直连偶发 TLS 握手 EOF。只重试一次连接失败，且共用原来的总时限。
        // 不重试 HTTP 拒绝，不关闭证书校验，也不延长整个选站的等待时间。
        Err(error) if error.is_connect() && start.elapsed() < timeout => {
            match retry {
                Some(req) => req.timeout(timeout.saturating_sub(start.elapsed())).send().await,
                None => Err(error),
            }
        }
        result => result,
    }
}

/// 第一轮：Range 0-0。能用返回文件总大小（源码包可能不知道大小，给 0）。
async fn probe(c: &reqwest::Client, url: &str, headers: &[(String, String)], archive: bool) -> Option<u64> {
    let timeout = source(&original(url)).map_or(PROBE_TIMEOUT, Source::probe_timeout);
    let mut req = c.get(url).header("Range", "bytes=0-0").timeout(timeout);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let resp = send_request(req, url, timeout).await.ok()?;
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    // 加速站出错时常回一个网页，当成文件存下来就坏了
    if ctype.starts_with("text/html") {
        return None;
    }
    let total = resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit('/').next())
        .and_then(|v| v.trim().parse::<u64>().ok());
    match resp.status().as_u16() {
        206 => {
            // 读完一字节探测响应，让连接归还池中；不下载错误服务器返回的大响应体。
            if resp.content_length() == Some(1) {
                resp.bytes().await.ok()?;
            }
            total.filter(|&t| t > 0)
        }
        200 if archive && !ctype.starts_with("text/") => Some(resp.content_length().unwrap_or(0)),
        _ => None,
    }
}

/// 第二轮：下一小段看实际速度（字节/秒）
async fn sample(c: &reqwest::Client, url: &str, headers: &[(String, String)], sample_time: Duration) -> f64 {
    let mut req = c.get(url).header("Range", format!("bytes=0-{}", SAMPLE_BYTES - 1)).timeout(sample_time);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let start = Instant::now();
    let Ok(mut resp) = send_request(req, url, sample_time).await else { return 0.0 };
    if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return 0.0;
    }
    let mut got = 0u64;
    loop {
        let left = sample_time.saturating_sub(start.elapsed());
        if left.is_zero() || got >= SAMPLE_BYTES {
            break;
        }
        match tokio::time::timeout(left, resp.chunk()).await {
            Ok(Ok(Some(b))) => got += b.len() as u64,
            _ => break,
        }
    }
    got as f64 / start.elapsed().as_secs_f64().max(0.001)
}

type ResourceKey = (String, Vec<(String, String)>);
struct PreparedRoute { started: Instant, running: bool, url: Option<String> }
#[derive(Default)]
struct WinnerCache {
    winners: HashMap<Source, (Instant, Option<&'static str>)>,
    prepared: HashMap<ResourceKey, PreparedRoute>,
}

fn cache() -> &'static Mutex<WinnerCache> {
    static C: OnceLock<Mutex<WinnerCache>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// 挑最快的下载地址。`skip` 里的不测（换站时排除已经失败过的）；`want_size` 非 0 时大小必须一致。
/// 返回 None 表示一个能用的都没有
pub async fn pick(original: &str, headers: &[(String, String)], skip: &[String], want_size: u64) -> Option<String> {
    pick_inner(original, headers, skip, want_size, None).await
}

async fn pick_inner(original: &str, headers: &[(String, String)], skip: &[String], want_size: u64, prepared: Option<&ResourceKey>) -> Option<String> {
    let source = source(original)?;
    // HF 先只支持匿名公开下载，测速也不能把凭据发给第三方。
    if source == Source::HuggingFace && mirror_headers(headers).len() != headers.len() {
        return None;
    }
    let c = client()?;
    let archive = source == Source::GitHub && is_archive(original);
    let mut cands: Vec<Option<&'static str>> = source.mirrors().iter().map(|m| Some(*m)).collect();
    cands.push(None);
    cands.retain(|m| !skip.contains(&candidate(*m, original)));
    if cands.is_empty() {
        return None;
    }

    // 刚测过、还在有效期内的最快站，直接确认一下能用就用
    if skip.is_empty() && want_size == 0 {
        let cached = cache().lock().unwrap().winners.get(&source).copied().filter(|(t, _)| t.elapsed() < CACHE_TTL).map(|(_, m)| m);
        if let Some(m) = cached {
            let url = candidate(m, original);
            let h = if m.is_some() { mirror_headers(headers) } else { headers.to_vec() };
            if probe(&c, &url, &h, archive).await.is_some() {
                return Some(url);
            }
        }
    }

    // 第一轮：一起发，按回来的先后排
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    for m in &cands {
        let (c, tx, m) = (c.clone(), tx.clone(), *m);
        let url = candidate(m, original);
        let h = if m.is_some() { mirror_headers(headers) } else { headers.to_vec() };
        tauri::async_runtime::spawn(async move {
            let size = probe(&c, &url, &h, archive).await;
            let _ = tx.send((m, size));
        });
    }
    drop(tx);
    let mut ok: Vec<(Option<&'static str>, u64)> = Vec::new();
    let mut deadline: Option<Instant> = None;
    loop {
        let wait = deadline.map(|d| d.saturating_duration_since(Instant::now())).unwrap_or(source.probe_timeout() + Duration::from_secs(1));
        match tokio::time::timeout(wait, rx.recv()).await {
            Ok(Some((m, Some(size)))) => {
                if want_size == 0 || size == want_size {
                    ok.push((m, size));
                    if let Some(key) = prepared {
                        if let Some(entry) = cache().lock().unwrap().prepared.get_mut(key) {
                            entry.url.get_or_insert_with(|| candidate(m, original));
                        }
                    }
                    deadline.get_or_insert(Instant::now() + FIRST_ROUND_GRACE);
                    if ok.len() >= FINALISTS {
                        break;
                    }
                }
            }
            Ok(Some(_)) => {}
            _ => break, // 都回来了或等够了
        }
    }
    let (first, size) = *ok.first()?;
    let winner = if ok.len() == 1 || archive || size < SAMPLE_MIN_SIZE {
        first
    } else {
        // 第二轮：几个一起下一小段，看谁真的快
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        for (m, _) in &ok {
            let (c, tx, m) = (c.clone(), tx.clone(), *m);
            let url = candidate(m, original);
            let h = if m.is_some() { mirror_headers(headers) } else { headers.to_vec() };
            tauri::async_runtime::spawn(async move {
                let _ = tx.send((m, sample(&c, &url, &h, source.sample_time()).await));
            });
        }
        drop(tx);
        let mut best = (first, 0.0f64);
        while let Some((m, speed)) = rx.recv().await {
            if speed > best.1 {
                best = (m, speed);
            }
        }
        best.0
    };
    if skip.is_empty() {
        cache().lock().unwrap().winners.insert(source, (Instant::now(), winner));
    }
    Some(candidate(winner, original))
}

fn resource_key(url: &str, headers: &[(String, String)]) -> Option<ResourceKey> {
    let orig = original(url);
    if !is_supported_file(&orig) || mirror_headers(headers).len() != headers.len() { return None; }
    let parsed = url::Url::parse(&orig).ok()?;
    if !parsed.username().is_empty() || parsed.password().is_some() || parsed.query_pairs().any(|(k, _)| {
        let key = k.to_ascii_lowercase(); key.contains("token") || key.contains("signature") || key == "auth"
    }) { return None; }
    let mut key_headers = headers.to_vec();
    for (key, _) in &mut key_headers { *key = key.to_ascii_lowercase(); }
    key_headers.sort();
    Some((orig, key_headers))
}

/// 在确认面板显示时预选。与点击开始共用同一份有界缓存和在途请求。
pub fn prepare(url: &str, headers: &[(String, String)]) {
    let Some(key) = resource_key(url, headers) else { return };
    {
        let mut cache = cache().lock().unwrap();
        cache.prepared.retain(|_, entry| entry.running || entry.started.elapsed() < Duration::from_secs(60));
        if cache.prepared.contains_key(&key) || cache.prepared.len() >= 32 || cache.prepared.values().filter(|e| e.running).count() >= 2 { return; }
        cache.prepared.insert(key.clone(), PreparedRoute { started: Instant::now(), running: true, url: None });
    }
    let headers = headers.to_vec();
    tauri::async_runtime::spawn(async move {
        let winner = pick_inner(&key.0, &headers, &[], 0, Some(&key)).await;
        if let Some(entry) = cache().lock().unwrap().prepared.get_mut(&key) {
            entry.running = false;
            if winner.is_some() { entry.url = winner; }
        }
    });
}

/// 建任务只读预选结果，绝不等待竞速；未准备好时先用用户给的地址。
pub async fn route(url: &str, headers: &[(String, String)]) -> (String, Vec<(String, String)>) {
    prepare(url, headers);
    let selected = resource_key(url, headers).and_then(|key| cache().lock().unwrap().prepared.get(&key).and_then(|entry| entry.url.clone()));
    match selected {
        Some(u) if u != url => {
            note(&format!("走加速站 {u}"));
            (u, mirror_headers(headers))
        }
        _ => (url.to_string(), headers.to_vec()),
    }
}

/// 选站记录写进 %LOCALAPPDATA%\com.shandian.windows\github-mirror.log，排查「为什么走了这个站」
fn note(msg: &str) { crate::app_log::record("mirror", msg); }

fn switched() -> &'static Mutex<HashMap<String, Vec<String>>> {
    static S: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// 出错的 GitHub / Hugging Face 任务换个站接着下。内核任务列表每轮调一次
pub async fn rescue(api: &crate::api::Api, tasks: &[crate::api::Task]) {
    for t in tasks {
        if t.status != "error" {
            continue;
        }
        let Some(req) = t.meta.as_ref().and_then(|m| m.req.as_ref()) else { continue };
        let current = req.url.clone();
        let orig = original(&current);
        if !is_supported_file(&orig) {
            continue;
        }
        // 每个地址只换走一次：换过之后还停在这个地址（没有能用的站），就留给用户处理，别每轮都测
        let tried = {
            let mut s = switched().lock().unwrap();
            let tried = s.entry(t.id.clone()).or_default();
            if tried.contains(&current) || tried.len() >= MAX_SWITCHES {
                continue;
            }
            tried.push(current.clone());
            tried.clone()
        };
        let headers: Vec<(String, String)> = req.extra["header"]
            .as_object()
            .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect())
            .unwrap_or_default();
        // 带登录信息的任务从原链接出错时不换加速站：内核换地址会沿用原来的请求头，等于把 Cookie 交给第三方
        let has_cred = mirror_headers(&headers).len() != headers.len();
        let next = if split(&current).is_none() && has_cred {
            None
        } else {
            pick(&orig, &headers, &tried, t.total()).await
        };
        if let Some(next) = next {
            note(&format!("任务 {} 换站：{current} -> {next}", t.id));
            if api.patch_url(&t.id, &next).await.is_ok() {
                let _ = api.resume(&t.id).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_route_is_immediate_and_reuses_pending_selection() {
        let orig = "https://github.com/shandian-feedback/test/releases/download/v1/item.bin";
        let key = resource_key(orig, &[]).unwrap();
        let selected = candidate(Some(MIRRORS[0]), orig);
        cache().lock().unwrap().prepared.insert(key.clone(), PreparedRoute {
            started: Instant::now(), running: true, url: Some(selected.clone()),
        });
        let start = Instant::now();
        let (url, _) = tauri::async_runtime::block_on(route(orig, &[]));
        assert_eq!(url, selected);
        assert!(start.elapsed() < Duration::from_millis(100), "ready selection must not probe again");
        cache().lock().unwrap().prepared.get_mut(&key).unwrap().url = None;
        let start = Instant::now();
        let (url, _) = tauri::async_runtime::block_on(route(orig, &[]));
        assert_eq!(url, orig);
        assert!(start.elapsed() < Duration::from_millis(100), "pending selection must not block task creation");
        assert!(cache().lock().unwrap().prepared.get(&key).unwrap().running);
        cache().lock().unwrap().prepared.remove(&key);
        assert!(resource_key(orig, &[("Cookie".into(), "private".into())]).is_none());
        assert!(resource_key("https://example.test/file.bin", &[]).is_none());
    }

    #[test]
    fn recognizes_github_files() {
        assert!(is_github_file("https://github.com/cli/cli/releases/download/v2.62.0/gh.tar.gz"));
        assert!(is_github_file("https://github.com/a/b/archive/refs/heads/main.zip"));
        assert!(is_github_file("https://raw.githubusercontent.com/a/b/main/x.txt"));
        assert!(!is_github_file("https://github.com/a/b"));
        assert!(!is_github_file("https://api.github.com/repos/a/b/releases/latest"));
        assert!(!is_github_file("https://example.com/releases/download/x"));
    }

    #[test]
    fn splits_mirror_links() {
        let o = "https://github.com/a/b/releases/download/v1/x.zip";
        let m = format!("https://ghfast.top/{o}");
        assert_eq!(split(&m), Some(("https://ghfast.top", o.to_string())));
        assert_eq!(original(&m), o);
        assert_eq!(original(o), o);
        // 加速站上的非 GitHub 路径不当成加速链接
        assert_eq!(split("https://ghfast.top/https://example.com/x"), None);
    }

    #[test]
    fn mirror_headers_drop_credentials() {
        let h = vec![
            ("Authorization".to_string(), "token x".to_string()),
            ("Cookie".to_string(), "a=b".to_string()),
            ("Referer".to_string(), "https://github.com/".to_string()),
        ];
        assert_eq!(mirror_headers(&h), vec![("Referer".to_string(), "https://github.com/".to_string())]);
    }

    #[test]
    fn huggingface_files_and_mirror_roundtrip() {
        for orig in [
            "https://huggingface.co/openai-community/gpt2/resolve/main/config.json",
            "https://huggingface.co/org/model/resolve/abc123/folder/model.safetensors?download=true",
            "https://huggingface.co/datasets/Salesforce/wikitext/resolve/main/README.md",
        ] {
            assert!(is_huggingface_file(orig), "{orig}");
            let mirror = candidate(Some(HF_MIRRORS[0]), orig);
            assert_eq!(original(&mirror), orig);
            assert!(is_supported_file(&mirror));
            assert_eq!(split(&mirror), Some((HF_MIRRORS[0], orig.to_string())));
        }
    }

    #[test]
    fn huggingface_rejects_pages_credentials_and_other_hosts() {
        for url in [
            "https://huggingface.co/org/model",
            "https://huggingface.co/org/model/blob/main/config.json",
            "https://huggingface.co/org/model/resolve/main/",
            "https://huggingface.co/api/models/resolve/main/config.json",
            "https://huggingface.co/spaces/org/resolve/main/app.py",
            "https://huggingface.co/datasets/org/resolve/main/file",
            "https://huggingface.co/org/model/resolve/main/config.json?token=secret",
            "https://huggingface.co/org/model/resolve/main/config.json?access_token=secret",
            "https://huggingface.co/org/model/resolve/main/config.json?X-Amz-Signature=secret",
            "http://fixture-user:fixture-password@proxy.example.invalid:8080",
            "https://huggingface.co.evil.test/org/model/resolve/main/config.json",
            "https://cdn-lfs.huggingface.co/org/model/resolve/main/config.json",
            "ftp://huggingface.co/org/model/resolve/main/config.json",
        ] {
            assert!(!is_supported_file(url), "{url}");
        }
        assert!(split("https://hf-mirror.com.evil.test/org/model/resolve/main/a").is_none());
        assert!(split("https://hf-mirror.com/org/model/blob/main/a").is_none());
    }

    #[test]
    fn huggingface_credentials_never_enter_mirror_selection() {
        let orig = "https://huggingface.co/org/model/resolve/main/config.json";
        for key in ["Authorization", "cookie"] {
            let headers = vec![(key.to_string(), "secret".to_string())];
            tauri::async_runtime::block_on(async {
                assert!(pick(orig, &headers, &[], 0).await.is_none());
                assert_eq!(route(orig, &headers).await, (orig.to_string(), headers));
            });
        }
    }

    #[test]
    #[ignore]
    fn live_huggingface_pick_and_fallback() {
        for (orig, size) in [
            ("https://huggingface.co/openai-community/gpt2/resolve/main/config.json", 665),
            ("https://huggingface.co/datasets/Salesforce/wikitext/resolve/main/README.md", 10464),
        ] {
            tauri::async_runtime::block_on(async {
                let first = pick(orig, &[], &[], 0).await.expect("no working source");
                let next = pick(orig, &[], &[first.clone()], size).await.expect("no fallback source");
                println!("{orig}: selected {first}, fallback {next}, size {size}");
                assert_ne!(first, next);
                assert_eq!(original(&first), orig);
                assert_eq!(original(&next), orig);
            });
        }
    }

    #[test]
    #[ignore]
    fn live_huggingface_kernel_rescue() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::os::windows::process::CommandExt;
        struct Kernel(std::process::Child, std::path::PathBuf);
        impl Drop for Kernel {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
                let _ = std::fs::remove_dir_all(&self.1);
            }
        }
        let root = std::env::temp_dir().join(format!("shandian-hf-test-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&root).unwrap();
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let engine = std::env::var("SHANDIAN_ENGINE").unwrap_or_else(|_| "C:/ShandianBuild/gopeed-shandian-amd64.exe".into());
        let child = std::process::Command::new(engine)
            .args(["-A", "127.0.0.1", "-P", &port.to_string(), "-T", "hf-isolated-test", "-d"])
            .arg(&root).arg("-c").arg(root.join("config.json"))
            .creation_flags(0x08000000)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .spawn().unwrap();
        let _kernel = Kernel(child, root.clone());
        tauri::async_runtime::block_on(async {
            let api = crate::api::Api::new(port, "hf-isolated-test".into());
            for _ in 0..60 {
                if api.info().await.is_ok() { break; }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            let mut cfg = api.config().await.unwrap();
            cfg["proxy"] = serde_json::json!({"enable": false});
            cfg["protocolConfig"]["http"]["connections"] = serde_json::json!(8);
            api.put_config(&cfg).await.unwrap();
            for (index, path) in ["openai-community/gpt2/resolve/main/config.json", "datasets/Salesforce/wikitext/resolve/main/README.md"].iter().enumerate() {
                let orig = format!("https://huggingface.co/{path}");
                let expected = send_request(client().unwrap().get(&orig), &orig, Duration::from_secs(20)).await.unwrap().error_for_status().unwrap().bytes().await.unwrap();
                for mirror_failed in [true, false] {
                    // 元数据可读，首包截断，后续返回 404，触发内核的有限次数重试。
                    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                    listener.set_nonblocking(true).unwrap();
                    let addr = listener.local_addr().unwrap();
                    let data = expected.clone();
                    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let stopping = stop.clone();
                    let server = std::thread::spawn(move || {
                        let mut requests = 0;
                        while !stopping.load(std::sync::atomic::Ordering::Relaxed) {
                            if let Ok((mut stream, _)) = listener.accept() {
                                stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                                let mut request = [0; 4096];
                                let _ = stream.read(&mut request);
                                requests += 1;
                                if requests > 1 {
                                    let _ = write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                                    continue;
                                }
                                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n", data.len());
                                let _ = stream.write_all(&data[..64]);
                            } else { std::thread::sleep(Duration::from_millis(10)); }
                        }
                    });
                    let name = format!("{index}-{mirror_failed}.bin");
                    let id = api.add_task(&format!("http://{addr}/{name}"), root.to_str().unwrap(), &name, &[], &[]).await.unwrap();
                    let mut failed = None;
                    for _ in 0..180 {
                        let task = api.list_tasks().await.unwrap().into_iter().find(|t| t.id == id).unwrap();
                        if task.status == "error" { failed = Some(task); break; }
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    server.join().unwrap();
                    assert!(failed.is_some(), "truncated response did not produce an error");
                    let current = if mirror_failed { candidate(Some(HF_MIRRORS[0]), &orig) } else { orig.clone() };
                    // 给真实失败任务标记要验证的源，随后只调用生产 rescue。
                    api.patch_url(&id, &current).await.unwrap();
                    let task = api.list_tasks().await.unwrap().into_iter().find(|t| t.id == id).unwrap();
                    assert_eq!(task.status, "error");
                    let before = task.progress.as_ref().unwrap().downloaded;
                    rescue(&api, &[task]).await;
                    let mut done = false;
                    for _ in 0..240 {
                        let task = api.list_tasks().await.unwrap().into_iter().find(|t| t.id == id).unwrap();
                        let url = &task.meta.as_ref().unwrap().req.as_ref().unwrap().url;
                        assert_ne!(url, &current, "rescue did not patch the URL");
                        assert_eq!(original(url), orig);
                        assert!(task.progress.as_ref().unwrap().downloaded >= before);
                        if task.status == "done" { done = true; break; }
                        assert_ne!(task.status, "error", "resumed download failed");
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    assert!(done, "resumed download timed out");
                    assert_eq!(std::fs::read(root.join(&name)).unwrap(), expected.as_ref());
                    println!("kernel rescue passed: {path}, mirror_failed={mirror_failed}, preserved={before}, final={} bytes", expected.len());
                }
            }
        });
    }

    #[test]
    #[ignore]
    fn live_huggingface_large_file_sample() {
        tauri::async_runtime::block_on(async {
            let c = client().unwrap();
            let orig = "https://huggingface.co/openai-community/gpt2/resolve/main/pytorch_model.bin";
            let mirror = candidate(Some(HF_MIRRORS[0]), orig);
            let mut samples = Vec::new();
            for url in [orig, mirror.as_str()] {
                assert_eq!(probe(&c, url, &[], false).await, Some(548118077));
                let response = c.get(url).header("Range", "bytes=0-524287")
                    .timeout(Duration::from_secs(20)).send().await.unwrap();
                assert_eq!(response.status(), reqwest::StatusCode::PARTIAL_CONTENT);
                let bytes = response.bytes().await.unwrap();
                assert_eq!(bytes.len(), SAMPLE_BYTES as usize);
                samples.push(bytes);
                let speed = sample(&c, url, &[], Source::HuggingFace.sample_time()).await;
                assert!(speed > 0.0, "sample returned no data: {url}");
                println!("large file sample: {url}, {speed:.0} bytes/s");
            }
            assert_eq!(samples[0], samples[1], "mirror bytes differ from original");
            assert!(pick(orig, &[], &[], 0).await.is_some());
        });
    }

    /// 联网实测（默认不跑）：cargo test ghmirror -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_pick() {
        let o = "https://github.com/cli/cli/releases/download/v2.62.0/gh_2.62.0_linux_amd64.tar.gz";
        let t = Instant::now();
        let got = tauri::async_runtime::block_on(pick(o, &[], &[], 0));
        println!("选中 {got:?}，用时 {:?}", t.elapsed());
        assert!(got.is_some());
        let first = got.unwrap();
        let t = Instant::now();
        let next = tauri::async_runtime::block_on(pick(o, &[], &[first.clone()], 0));
        println!("排除它之后 {next:?}，用时 {:?}", t.elapsed());
        assert!(next.is_some() && next.unwrap() != first);
    }
}
