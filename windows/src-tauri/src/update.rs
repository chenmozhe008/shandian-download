// Windows 更新只接受当前版别的完整 NSIS 安装包。GitHub HTTPS 是哈希的可信来源。
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::blocking::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::Emitter;

use crate::AppState;

const REPO: &str = "cgeek-dev/shandian-download";
const FEED: &str = "https://api.github.com/repos/cgeek-dev/shandian-download/releases?per_page=50";
const ATOM_FEED: &str = "https://github.com/cgeek-dev/shandian-download/releases.atom";
static API_RETRY_AT: AtomicU64 = AtomicU64::new(0);
const CURRENT: &str = env!("CARGO_PKG_VERSION");
const EDITION: &str = "plain";
const DAY: u64 = 24 * 60 * 60;
const MAX_INSTALLER: u64 = 512 * 1024 * 1024;
static INSTALLING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize, Deserialize)]
struct Release {
    version: String,
    file: String,
    sha256: String,
    bytes: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    checked_at: u64,
    checked_for: Option<String>,
    latest: Option<Release>,
    notified: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRelease {
    version: String,
    can_install: bool,
    message: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn version(text: &str) -> Option<[u32; 3]> {
    let mut values = text.split('.');
    let mut next = || {
        let part = values.next()?;
        let number = part.parse::<u32>().ok()?;
        (number.to_string() == part).then_some(number)
    };
    let result = [next()?, next()?, next()?];
    values.next().is_none().then_some(result)
}

fn setup_name(version: &str) -> String {
    format!("Shandian-{version}-Windows-x64-setup.exe")
}
fn tag(version: &str) -> String {
    format!("v{version}-windows")
}
fn github_asset(tag: &str, filename: &str) -> String {
    format!("https://github.com/{REPO}/releases/download/{tag}/{filename}")
}

fn checked(release: &Release) -> bool {
    version(&release.version).is_some()
        && release.file == setup_name(&release.version)
        && release.sha256.len() == 64
        && release.sha256.bytes().all(|c| c.is_ascii_hexdigit())
        && (1..=MAX_INSTALLER).contains(&release.bytes)
}

fn candidates(feed: &Value) -> Vec<(String, Value)> {
    let mut result = Vec::new();
    for entry in feed.as_array().into_iter().flatten() {
        if entry["draft"].as_bool() != Some(false) || entry["prerelease"].as_bool() != Some(false) {
            continue;
        }
        let Some(tag) = entry["tag_name"].as_str() else {
            continue;
        };
        let Some(raw) = tag
            .strip_prefix('v')
            .and_then(|v| v.strip_suffix("-windows"))
        else {
            continue;
        };
        if version(raw) <= version(CURRENT) {
            continue;
        }
        let file = setup_name(raw);
        if let Some(asset) = entry["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|asset| asset["name"].as_str() == Some(file.as_str()))
        {
            result.push((raw.to_owned(), asset.clone()));
        }
    }
    result.sort_by_key(|(raw, _)| std::cmp::Reverse(version(raw)));
    result
}

fn plain_release(raw: String, asset: &Value) -> Option<Release> {
    let release = Release {
        version: raw,
        file: asset["name"].as_str()?.into(),
        sha256: asset["digest"].as_str()?.strip_prefix("sha256:")?.into(),
        bytes: asset["size"].as_u64()?,
    };
    checked(&release).then_some(release)
}

fn response_body(response: Response, limit: u64) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    response
        .take(limit + 1)
        .read_to_end(&mut body)
        .map_err(|_| "没有读完更新信息")?;
    if body.len() as u64 > limit {
        return Err("更新信息超出预期大小".into());
    }
    Ok(body)
}

fn json_body<T: serde::de::DeserializeOwned>(response: Response, limit: u64) -> Result<T, String> {
    serde_json::from_slice(&response_body(response, limit)?)
        .map_err(|_| "更新信息格式不正确".into())
}

fn client(url: &str) -> Result<Client, String> {
    let mut builder = Client::builder()
        .user_agent("shandian-windows-update")
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(15 * 60))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let url = attempt.url();
            if attempt.previous().len() < 5
                && url.scheme() == "https"
                && matches!(
                    url.host_str(),
                    Some(
                        "github.com"
                            | "release-assets.githubusercontent.com"
                            | "objects.githubusercontent.com"
                    )
                )
            {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }));
    if let Some(proxy) = crate::proxy::url_for(url).and_then(|url| reqwest::Proxy::all(url).ok()) {
        builder = builder.proxy(proxy);
    }
    builder.build().map_err(|_| "无法建立更新连接".into())
}

fn record_name(version: &str) -> String {
    setup_name(version).replace("-setup.exe", "-update.json")
}

fn atom_versions(body: &[u8]) -> Result<Vec<String>, String> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_reader(body);
    let prefix = format!("https://github.com/{REPO}/releases/tag/v");
    let mut entry = false;
    let mut closed_feed = false;
    let mut versions = Vec::new();
    loop {
        match reader.read_event().map_err(|_| "备用发布列表格式不正确")? {
            Event::Start(node) | Event::Empty(node) => match node.local_name().as_ref() {
                "entry" => entry = true,
                "link" if entry => {
                    for attribute in node
                        .attributes()
                        .flatten()
                        .filter(|item| item.key.as_ref() == "href")
                    {
                        let Ok(href) =
                            attribute.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        else {
                            continue;
                        };
                        let Some(raw) = href
                            .strip_prefix(&prefix)
                            .and_then(|v| v.strip_suffix("-windows"))
                        else {
                            continue;
                        };
                        if version(raw) > version(CURRENT) {
                            versions.push(raw.to_owned());
                        }
                    }
                }
                _ => {}
            },
            Event::End(node) => match node.local_name().as_ref() {
                "entry" => entry = false,
                "feed" => closed_feed = true,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    if !closed_feed {
        return Err("备用发布列表不完整".into());
    }
    versions.sort_by_key(|raw| std::cmp::Reverse(version(raw)));
    versions.dedup();
    Ok(versions)
}

// A release record is explicitly published for stable clients. The Atom list
// alone does not identify prereleases or carry the installer SHA-256.
fn fetch_record(raw: &str) -> Result<Option<Release>, String> {
    let url = github_asset(&tag(raw), &record_name(raw));
    let response = client(&url)?
        .get(url)
        .timeout(Duration::from_secs(18))
        .send()
        .map_err(|_| "无法读取更新校验信息")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err("更新校验信息暂不可用".into());
    }
    let release: Release = json_body(response, 16 * 1024)?;
    if release.version != raw || !checked(&release) {
        return Err("发布的更新校验信息与安装包不一致".into());
    }
    Ok(Some(release))
}

fn fetch_from_atom() -> Result<Option<Release>, String> {
    let response = client(ATOM_FEED)?
        .get(ATOM_FEED)
        .timeout(Duration::from_secs(18))
        .send()
        .map_err(|_| "连不上备用发布列表")?;
    if !response.status().is_success() {
        return Err("备用发布列表暂不可用".into());
    }
    let versions = atom_versions(&response_body(response, 4 * 1024 * 1024)?)?;
    for raw in &versions {
        if let Some(release) = fetch_record(raw)? {
            return Ok(Some(release));
        }
    }
    if versions.is_empty() {
        Ok(None)
    } else {
        Err("发现较新的发布记录，但暂未取得可校验的更新信息".into())
    }
}

fn retry_after(headers: &reqwest::header::HeaderMap, current: u64) -> u64 {
    let number = |name: &str| -> Option<u64> { headers.get(name)?.to_str().ok()?.parse().ok() };
    if let Some(delay) = number("retry-after") {
        return current.saturating_add(delay.max(60));
    }
    if number("x-ratelimit-remaining") == Some(0) {
        if let Some(reset) = number("x-ratelimit-reset") {
            return reset.max(current + 60);
        }
    }
    current + 60
}

fn fetch_latest() -> Result<Option<Release>, String> {
    if now() < API_RETRY_AT.load(Ordering::Relaxed) {
        return fetch_from_atom()
            .map_err(|error| format!("GitHub 暂时限制更新查询。{error}，请稍后再试"));
    }
    let request = client(FEED)?.get(FEED).timeout(Duration::from_secs(18));
    let res = request.send().map_err(|_| "连不上更新服务器，请稍后重试")?;
    if matches!(res.status().as_u16(), 403 | 429) {
        API_RETRY_AT.store(retry_after(res.headers(), now()), Ordering::Relaxed);
        return fetch_from_atom()
            .map_err(|error| format!("GitHub 暂时限制更新查询。{error}，请稍后再试"));
    }
    if !res.status().is_success() {
        return Err(format!("更新服务器返回 HTTP {}", res.status().as_u16()));
    }
    let feed: Value = json_body(res, 4 * 1024 * 1024)?;
    if !feed.is_array() {
        return Err("更新服务器返回的列表无法识别".into());
    }
    Ok(candidates(&feed)
        .into_iter()
        .find_map(|(raw, asset)| plain_release(raw, &asset)))
}

fn cache_path(base: &Path) -> PathBuf {
    base.join(format!("update-windows-{EDITION}.json"))
}

fn load_version_cache(text: &[u8], current: &str) -> Cache {
    serde_json::from_slice::<Cache>(text)
        .ok()
        .filter(|cache| cache.checked_for.as_deref() == Some(current))
        .unwrap_or_default()
}

fn load(base: &Path) -> Cache {
    fs::read(cache_path(base))
        .ok()
        .map(|text| load_version_cache(&text, CURRENT))
        .unwrap_or_default()
}

fn saved_dir() -> Result<PathBuf, String> {
    use winreg::{
        enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
        RegKey,
    };
    let exe = std::env::current_exe().map_err(|_| "找不到正在运行的程序")?;
    let current = exe.parent().ok_or("找不到程序目录")?;
    let key = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\闪电全能下载";
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let Ok(entry) = RegKey::predef(root).open_subkey(key) else {
            continue;
        };
        let dir: String = entry.get_value("InstallLocation").unwrap_or_default();
        let edition: String = entry.get_value("ShandianEdition").unwrap_or_default();
        let version: String = entry.get_value("DisplayVersion").unwrap_or_default();
        if edition != EDITION || version != CURRENT {
            continue;
        }
        let matches = dir.eq_ignore_ascii_case(&current.to_string_lossy())
            || fs::canonicalize(&dir)
                .ok()
                .zip(fs::canonicalize(current).ok())
                .is_some_and(|(a, b)| {
                    a.to_string_lossy()
                        .eq_ignore_ascii_case(&b.to_string_lossy())
                });
        if matches {
            return Ok(current.to_owned());
        }
    }
    Err("当前运行的是便携版或未登记的版本，请手动下载新版".into())
}

#[tauri::command]
pub async fn update_check(
    force: bool,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<UpdateRelease>, String> {
    static CHECK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    let _guard = CHECK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let base = state.base.clone();
    let mut cache = load(&base);
    if force || cache.checked_at == 0 || now().saturating_sub(cache.checked_at) >= DAY {
        cache.latest = tauri::async_runtime::spawn_blocking(fetch_latest)
            .await
            .map_err(|_| "检查更新未完成")??;
        cache.checked_at = now();
        cache.checked_for = Some(CURRENT.into());
    }
    let release = cache
        .latest
        .as_ref()
        .filter(|release| checked(release) && version(&release.version) > version(CURRENT));
    let availability_message = release.and_then(|_| saved_dir().err());
    let result = release.map(|release| {
        let message = availability_message;
        let can_install = message.is_none();
        if !force && can_install && cache.notified.as_deref() != Some(&release.version) {
            crate::float::notify_update(&app, &release.version);
            cache.notified = Some(release.version.clone());
        }
        UpdateRelease {
            version: release.version.clone(),
            can_install,
            message,
        }
    });
    if let Ok(value) = serde_json::to_vec(&cache) {
        fs::write(cache_path(&base), value).map_err(|_| "无法保存更新检查记录")?;
    }
    Ok(result)
}

fn update_dir(local: &Path) -> PathBuf {
    local.join("update-installers")
}

fn sha256(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash).ok()?;
    Some(format!("{:x}", hash.finalize()))
}

fn request_installer(release: &Release) -> Result<Response, String> {
    let url = github_asset(&tag(&release.version), &release.file);
    client(&url)?
        .get(url)
        .send()
        .map_err(|_| "无法下载更新包，请稍后重试".into())
}

fn install(
    app: &tauri::AppHandle,
    base: &Path,
    local: &Path,
    expected: &str,
) -> Result<std::process::Child, String> {
    let dir = saved_dir()?;
    let release = load(base)
        .latest
        .filter(|r| r.version == expected && checked(r) && version(&r.version) > version(CURRENT))
        .ok_or("更新版本已变化，请重新检查")?;
    let updates = update_dir(local);
    fs::create_dir_all(&updates).map_err(|_| "无法创建更新缓存目录")?;
    let package = updates.join(&release.file);
    let emit = |phase, percent| {
        let _ = app.emit(
            "update-progress",
            json!({"phase": phase, "percent": percent, "version": expected}),
        );
    };
    let matches = fs::metadata(&package).is_ok_and(|m| m.len() == release.bytes)
        && sha256(&package).is_some_and(|hash| hash.eq_ignore_ascii_case(&release.sha256));
    if !matches {
        emit("downloading", 0);
        let partial = package.with_extension("part");
        let outcome = (|| {
            let mut response = request_installer(&release)?;
            if !response.status().is_success() {
                return Err(format!(
                    "安装包下载返回 HTTP {}",
                    response.status().as_u16()
                ));
            }
            let mut file = fs::File::create(&partial).map_err(|_| "无法写入安装包")?;
            let mut bytes = 0;
            let mut percent = 0;
            let mut hash = Sha256::new();
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = response
                    .read(&mut buf)
                    .map_err(|_| "下载安装包中断，请重试")?;
                if n == 0 {
                    break;
                }
                bytes += n as u64;
                if bytes > release.bytes {
                    return Err("安装包大小与发布记录不一致".into());
                }
                file.write_all(&buf[..n]).map_err(|_| "写入安装包失败")?;
                hash.update(&buf[..n]);
                let progress = (bytes * 100 / release.bytes).min(99);
                if percent != progress {
                    emit("downloading", progress);
                    percent = progress;
                }
            }
            file.sync_all().map_err(|_| "无法保存安装包")?;
            drop(file);
            emit("verifying", 100);
            if bytes != release.bytes
                || !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(&release.sha256)
            {
                return Err("安装包校验未通过，更新未开始".into());
            }
            if package.exists() {
                fs::remove_file(&package).map_err(|_| "旧安装包正在使用")?;
            }
            fs::rename(&partial, &package).map_err(|_| "无法保存已校验的安装包")?;
            Ok::<_, String>(())
        })();
        if outcome.is_err() {
            let _ = fs::remove_file(&partial);
        }
        outcome?;
    }
    emit("installing", 100);
    Command::new(&package)
        .arg("--in-app-update")
        .arg("--auto-dir")
        .arg(dir)
        .spawn()
        .map_err(|_| "安装程序未能启动，请重试".into())
}

#[tauri::command]
pub async fn update_install(
    version: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err("更新正在进行".into());
    }
    let (base, local) = (state.base.clone(), state.local.clone());
    let handle = app.clone();
    let expected = version.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || install(&handle, &base, &local, &expected))
            .await;
    match result {
        Ok(Ok(mut child)) => {
            std::thread::spawn(move || {
                let _ = child.wait();
                INSTALLING.store(false, Ordering::SeqCst);
                let _ = app.emit(
                    "update-progress",
                    json!({"phase":"retry","version":version}),
                );
            });
            Ok(())
        }
        failure => {
            INSTALLING.store(false, Ordering::SeqCst);
            let error = match failure {
                Ok(Err(error)) => error,
                _ => "更新未完成".into(),
            };
            let _ = app.emit(
                "update-progress",
                json!({"phase":"failed","message":error,"version":version}),
            );
            Err(error)
        }
    }
}

pub fn cleanup_installers(local: PathBuf) {
    std::thread::spawn(move || {
        let output = update_dir(&local);
        let Ok(entries) = fs::read_dir(output) else {
            return;
        };
        let suffix = "-Windows-x64-setup.exe";
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name
                .to_str()
                .and_then(|name| name.strip_suffix(".part"))
                .is_some_and(|part| {
                    let executable = format!("{part}.exe");
                    executable
                        .strip_prefix("Shandian-")
                        .and_then(|rest| rest.strip_suffix(suffix))
                        .and_then(version)
                        .is_some()
                })
            {
                let _ = fs::remove_file(entry.path());
                continue;
            }
            let Some(raw) = name
                .to_str()
                .and_then(|value| value.strip_prefix("Shandian-"))
                .and_then(|value| value.strip_suffix(suffix))
            else {
                continue;
            };
            if version(raw).is_none() || version(raw) > version(CURRENT) {
                continue;
            }
            for _ in 0..20 {
                if fs::remove_file(entry.path()).is_ok() || !entry.path().exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    });
}

/// 更新替换留下的 *.old（旧 exe 改名的产物）启动就删；删不掉说明旧进程还没退干净，
/// 不管它，下次启动再试。
pub fn cleanup_old_backups() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else { return };
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    // 只删自己留下的：绿色版可能放在用户自己的文件夹里，别的 .old 不是我们的
    let exe_name = exe
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let ours = [exe_name.as_str(), "gopeed.exe", "WebView2Loader.dll"];
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name
            .strip_suffix(".old")
            .is_some_and(|n| ours.iter().any(|o| o.eq_ignore_ascii_case(n)))
        {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discards_update_checks_from_other_app_versions() {
        let stored = br#"{"checked_at":1234,"checked_for":"0.2.15","latest":null,"notified":"0.2.16"}"#;
        let same = load_version_cache(stored, "0.2.15");
        assert_eq!(same.checked_at, 1234);
        assert_eq!(same.notified.as_deref(), Some("0.2.16"));

        for current in ["0.2.14", "0.2.16"] {
            let refreshed = load_version_cache(stored, current);
            assert_eq!(refreshed.checked_at, 0);
            assert!(refreshed.notified.is_none());
        }
        let legacy = br#"{"checked_at":1234,"latest":null,"notified":null}"#;
        assert_eq!(load_version_cache(legacy, "0.2.15").checked_at, 0);
    }

    #[test]
    fn fallback_sorts_versions_and_ignores_other_channels() {
        let body = br#"<feed xmlns="http://www.w3.org/2005/Atom">
            <entry><link href="https://github.com/cgeek-dev/shandian-download/releases/tag/v1.1.9-windows"/></entry>
            <entry><link href="https://github.com/cgeek-dev/shandian-download/releases/tag/v1.1.10-windows"/></entry>
            <entry><link href="https://github.com/cgeek-dev/shandian-download/releases/tag/v2.0.0-macos"/></entry>
            <entry><link href="https://github.com/other/shandian-download/releases/tag/v9.0.0-windows"/></entry>
            <entry><link href="https://github.com/cgeek-dev/shandian-download/releases/tag/v9.0.0-rc-windows"/></entry>
        </feed>"#;
        assert_eq!(atom_versions(body).unwrap(), ["1.1.10", "1.1.9"]);
        assert!(atom_versions(b"<feed><entry>",).is_err());
        assert_eq!(
            record_name("1.1.10"),
            "Shandian-1.1.10-Windows-x64-update.json"
        );
    }

    #[test]
    fn feed_rejects_portable_and_prerelease() {
        let entry = |version: &str, file: String, prerelease: bool| {
            json!({
                "tag_name": tag(version), "draft": false, "prerelease": prerelease,
                "assets": [{"name": file, "size": 75, "digest": format!("sha256:{}", "a".repeat(64))}]
            })
        };
        let feed = json!([
            entry("1.2.3", setup_name("1.2.3"), false),
            entry("2.1.3", setup_name("2.1.3"), true),
            entry(
                "3.1.3",
                "Shandian-3.1.3-Windows-x64-Portable.zip".into(),
                false
            )
        ]);
        let [(raw, asset)] = candidates(&feed).try_into().unwrap();
        assert!(plain_release(raw, &asset).is_some());
    }

    #[test]
    fn respects_the_github_retry_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(retry_after(&headers, 10), 70);
        headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
        headers.insert("x-ratelimit-reset", "1600".parse().unwrap());
        assert_eq!(retry_after(&headers, 10), 1600);
        headers.insert("retry-after", "120".parse().unwrap());
        assert_eq!(retry_after(&headers, 10), 130);
    }
}
