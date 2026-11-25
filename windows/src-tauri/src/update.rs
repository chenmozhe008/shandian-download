// 自动更新：检查新版本 + 一键更新（移植自 Mac 版 Sources/Shandian/UpdateChecker.swift）。
//
// 数据源是 GitHub 的 releases 列表（Windows 的发版 tag 是 win-v…，Mac 版 /releases/latest
// 拿不到它，所以用列表自己挑）。每天最多查一次，结果缓存在 <数据目录>/update.json。
//
// 一键更新的路子是「绿色包整包替换」：下载 zip → SHA-256 校验 → 解压 →
// 把里面的文件拷进当前 exe 目录。Windows 上正在运行的 exe 删不掉但改得了名，
// 所以先把旧 exe 改名成 .old 再拷新的进来；启动时顺手把 *.old 清掉。
//
// SHANDIAN_UPDATE_FEED=<url> 可以把数据源整个换掉（file:// 也行），测安装流程用。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Emitter;

use crate::AppState;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const FEED: &str = "https://api.github.com/repos/chenmozhe008/shandian-download/releases";
/// 一天之内不重复查
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// 安装包在 zip 根目录里的固定名字（script/package_windows.sh 的布局）
const APP_EXE: &str = "闪电全能下载.exe";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 给前端的版本信息（update_check 的返回值）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRelease {
    pub version: String,
    pub page: String,
    pub notes: Vec<String>,
    /// 有没有一键更新的安装包；没有就只能去发布页手动下
    pub has_asset: bool,
}

/// 一条发版记录（缓存进 update.json 的形状）
#[derive(Clone, Serialize, Deserialize)]
struct Release {
    version: String,
    page: String,
    asset: Option<String>,
    /// GitHub 给的 "sha256:…"；没有就跳过校验
    digest: Option<String>,
    notes: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cache {
    last_check: u64,
    latest_release: Option<Release>,
}

fn cache_path(base: &Path) -> PathBuf {
    base.join("update.json")
}

fn load_cache(base: &Path) -> Option<Cache> {
    let text = std::fs::read_to_string(cache_path(base)).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_cache(base: &Path, release: &Release) {
    let c = Cache {
        last_check: now_ms(),
        latest_release: Some(release.clone()),
    };
    if let Ok(text) = serde_json::to_string_pretty(&c) {
        let _ = std::fs::write(cache_path(base), text);
    }
}

fn public(r: &Release) -> UpdateRelease {
    UpdateRelease {
        version: r.version.clone(),
        page: r.page.clone(),
        notes: r.notes.clone(),
        has_asset: r.asset.is_some(),
    }
}

// ---------- Tauri 命令 ----------

/// 检查更新。force=false 时每天最多查一次（今天查过就用缓存）；
/// force=true 立刻查。返回 None = 已是最新版本。
#[tauri::command]
pub async fn update_check(
    force: bool,
    state: tauri::State<'_, AppState>,
) -> Result<Option<UpdateRelease>, String> {
    let base = state.base.clone();
    tauri::async_runtime::spawn_blocking(move || check_blocking(force, &base))
        .await
        .map_err(|e| e.to_string())?
}

/// 一键更新：下载 → 校验 → 解压 → 替换 exe 目录里的文件 → 拉起新版退出。
/// 进程在成功路径上直接退出，调用方只会等到失败原因。
#[tauri::command]
pub async fn update_install(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let base = state.base.clone();
    // 先停内核：gopeed.exe 还占着文件，覆盖它会被 Windows 拦下
    {
        let mut e = state.engine.lock().unwrap();
        let _ = e.stop();
    }
    let release = load_cache(&base)
        .and_then(|c| c.latest_release)
        .ok_or("没有可用的更新信息，请先检查更新")?;
    let asset = release
        .asset
        .clone()
        .ok_or("这个版本没有提供 Windows 安装包，请到发布页手动下载")?;
    tauri::async_runtime::spawn_blocking(move || install_now(&app, &asset, &release, &base))
        .await
        .map_err(|e| e.to_string())?
}

// ---------- 检查 ----------

fn check_blocking(force: bool, base: &Path) -> Result<Option<UpdateRelease>, String> {
    let current = env!("CARGO_PKG_VERSION");
    let feed = std::env::var("SHANDIAN_UPDATE_FEED").ok().filter(|s| !s.is_empty());
    // 换过数据源就每次都查（测试用）；否则每天最多查一次，今天查过直接用缓存
    if feed.is_none() && !force {
        if let Some(c) = load_cache(base) {
            if now_ms().saturating_sub(c.last_check) < DAY_MS {
                return Ok(c
                    .latest_release
                    .filter(|r| is_newer(&r.version, current))
                    .as_ref()
                    .map(public));
            }
        }
    }
    let text = fetch(feed.as_deref().unwrap_or(FEED))?;
    let release = parse_feed(&text).ok_or("更新数据源返回的内容看不懂")?;
    save_cache(base, &release);
    Ok(if is_newer(&release.version, current) {
        Some(public(&release))
    } else {
        None
    })
}

/// 取数据源原文。file:// 是测试用的本地 JSON，其余走 HTTP（挂系统代理）。
fn fetch(url: &str) -> Result<String, String> {
    if let Some(rest) = url.strip_prefix("file://") {
        // file:///C:/x.json → C:/x.json
        let p = rest
            .strip_prefix('/')
            .filter(|r| r.as_bytes().get(1) == Some(&b':'))
            .unwrap_or(rest);
        return std::fs::read_to_string(p).map_err(|e| format!("读不了更新数据源：{e}"));
    }
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("shandian-download-windows");
    if let Some(p) = crate::proxy::url_for(url) {
        if let Ok(px) = reqwest::Proxy::all(&p) {
            builder = builder.proxy(px);
        }
    }
    let client = builder.build().map_err(|e| e.to_string())?;
    let resp = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("无法连接更新服务器：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("更新服务器返回 HTTP {}", resp.status().as_u16()));
    }
    resp.text().map_err(|e| format!("读不了更新服务器的回复：{e}"))
}

/// 解析数据源。GitHub 的 releases 是数组：挑最新的非 draft、tag 以 win-v 开头那条，
/// 安装包取其中的 shandian-windows-*-x64.zip（没有就是 has_asset=false，只能手动下）。
/// 单个 release 对象（测试数据源）直接用。
fn parse_feed(text: &str) -> Option<Release> {
    let v: Value = serde_json::from_str(text).ok()?;
    if let Some(arr) = v.as_array() {
        for item in arr {
            if item["draft"].as_bool().unwrap_or(false) {
                continue;
            }
            let tag = item["tag_name"].as_str().unwrap_or("");
            if !tag.starts_with("win-v") {
                continue;
            }
            let zip = find_asset(item["assets"].as_array().map(|a| a.as_slice()).unwrap_or(&[]));
            return Some(Release {
                version: tag.strip_prefix("win-v").unwrap_or(tag).to_string(),
                page: item["html_url"].as_str().unwrap_or("").to_string(),
                asset: zip.and_then(|z| z["browser_download_url"].as_str()).map(String::from),
                digest: zip.and_then(|z| z["digest"].as_str()).map(String::from),
                notes: notes(item["body"].as_str().unwrap_or("")),
            });
        }
        return None;
    }
    let tag = v["tag_name"].as_str()?;
    let version = tag
        .strip_prefix("win-v")
        .or_else(|| tag.strip_prefix('v'))
        .unwrap_or(tag)
        .to_string();
    let zip = find_asset(v["assets"].as_array().map(|a| a.as_slice()).unwrap_or(&[]));
    Some(Release {
        version,
        page: v["html_url"].as_str().unwrap_or("").to_string(),
        asset: zip.and_then(|z| z["browser_download_url"].as_str()).map(String::from),
        digest: zip.and_then(|z| z["digest"].as_str()).map(String::from),
        notes: notes(v["body"].as_str().unwrap_or("")),
    })
}

fn find_asset(assets: &[Value]) -> Option<&Value> {
    assets.iter().find(|a| {
        let n = a["name"].as_str().unwrap_or("");
        n.starts_with("shandian-windows-") && n.ends_with("-x64.zip")
    })
}

/// 发布说明「更新内容」小节下的 "- " 各行（和 Mac 版 UpdateChecker.notes 同款）
fn notes(body: &str) -> Vec<String> {
    let mut in_section = false;
    let mut out = Vec::new();
    for line in body.split('\n') {
        if line.starts_with('#') {
            in_section = line.contains("更新内容");
            continue;
        }
        if in_section && line.starts_with("- ") {
            out.push(line[2..].to_string());
        }
    }
    out
}

/// 数字段逐段比：1.10.0 比 1.9.2 新。段不是纯数字（"开发版"这类）就不折腾了。
fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Option<Vec<i64>> {
        v.split('.').map(|p| p.parse::<i64>().ok()).collect()
    };
    let (Some(new), Some(old)) = (parse(candidate), parse(current)) else {
        return false;
    };
    if old.is_empty() {
        return false;
    }
    for i in 0..new.len().max(old.len()) {
        let a = new.get(i).copied().unwrap_or(0);
        let b = old.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

// ---------- 一键更新 ----------

fn install_now(
    app: &tauri::AppHandle,
    asset_url: &str,
    release: &Release,
    _base: &Path,
) -> Result<(), String> {
    let emit = |phase: &str, percent: u8| {
        let _ = app.emit("update-progress", json!({ "phase": phase, "percent": percent }));
    };

    // 为所有用户安装时程序在 Program Files，普通权限写不进去。先查，别下完才报错
    let exe_dir = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("找不到程序目录")?
        .to_path_buf();
    let probe = exe_dir.join(".update-write-test");
    if std::fs::write(&probe, b"").is_err() {
        return Err("这份是为所有用户安装的，没有权限自动更新。请到发布页下载新版安装包，运行后会原位更新".into());
    }
    let _ = std::fs::remove_file(&probe);

    // 1. 下载安装包到临时目录
    emit("downloading", 0);
    let zip_path = std::env::temp_dir().join("shandian-update.zip");
    let _ = std::fs::remove_file(&zip_path);
    download(asset_url, &zip_path, |pct| emit("downloading", pct))?;

    // 2. SHA-256 校验（GitHub 没给 digest 就跳过）
    if let Some(hex) = release.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) {
        let got = sha256_file(&zip_path).map_err(|e| format!("读不了下载的文件：{e}"))?;
        if !got.eq_ignore_ascii_case(hex) {
            return Err("下载的文件不完整或已损坏（SHA-256 校验不过）".into());
        }
    }

    // 3. 解压到临时目录，确认里面是闪电全能下载
    emit("installing", 0);
    let dir = std::env::temp_dir().join("shandian-update-extract");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("建临时目录失败：{e}"))?;
    {
        let f = std::fs::File::open(&zip_path).map_err(|e| format!("打不开下载的文件：{e}"))?;
        let mut zip =
            zip::ZipArchive::new(f).map_err(|e| format!("这不是能解开的压缩包：{e}"))?;
        zip.extract(&dir).map_err(|e| format!("解压失败：{e}"))?;
    }
    if !dir.join(APP_EXE).exists() {
        return Err("压缩包里没有「闪电全能下载.exe」，不是 Windows 版的安装包".into());
    }

    // 4. 整包替换到当前 exe 目录（gopeed.exe 等同目录文件一并更新）
    for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let src = entry.path();
        let dst = exe_dir.join(entry.file_name());
        if src.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            install_file(&src, &dst)?;
        }
    }

    // 5. 拉起新版再退出。必须等旧进程退干净才能 start：新进程要是撞上单实例插件，
    //    会被当成「第二份」把参数交出来后自杀，新旧都剩不下——所以让 cmd 隔 2 秒再拉。
    #[cfg(windows)]
    {
        let new_exe = exe_dir.join(APP_EXE);
        std::process::Command::new("cmd")
            .raw_arg(format!(
                "/c ping -n 3 127.0.0.1 >nul & start \"\" \"{}\"",
                new_exe.display()
            ))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("拉起新版本失败：{e}"))?;
    }
    std::process::exit(0);
}

/// 下载到 out，每涨一个百分点回调一次
fn download(url: &str, out: &Path, mut progress: impl FnMut(u8)) -> Result<(), String> {
    let mut builder = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(1800))
        .user_agent("shandian-download-windows");
    // GitHub 的安装包在境外图床同款线路上，直连常超时，走系统代理
    if let Some(p) = crate::proxy::url_for(url) {
        if let Ok(px) = reqwest::Proxy::all(&p) {
            builder = builder.proxy(px);
        }
    }
    let client = builder.build().map_err(|e| e.to_string())?;
    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| format!("连接失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("下载失败（HTTP {}）", resp.status().as_u16()));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(out).map_err(|e| format!("存不下安装包：{e}"))?;
    let mut buf = [0u8; 64 * 1024];
    let mut done: u64 = 0;
    let mut last = 0u8;
    loop {
        let n = resp
            .read(&mut buf)
            .map_err(|e| format!("下载中断：{e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("存不下安装包：{e}"))?;
        done += n as u64;
        if total > 0 {
            let pct = ((done * 100 / total) as u8).min(100);
            if pct != last {
                last = pct;
                progress(pct);
            }
        }
    }
    progress(100);
    Ok(())
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// 把新文件放到目标位置。目标正被占用（在跑的 exe）删不掉，但 Windows 允许改名——
/// 改名成 .old 再拷新的进来，旧进程退了下次启动会把 *.old 清掉。
fn install_file(src: &Path, dst: &Path) -> Result<(), String> {
    if dst.exists() && std::fs::remove_file(dst).is_err() {
        let mut s = dst.as_os_str().to_owned();
        s.push(".old");
        let old = PathBuf::from(s);
        let _ = std::fs::remove_file(&old);
        std::fs::rename(dst, &old).map_err(|e| format!("替换不了 {}：{e}", dst.display()))?;
    }
    if let Some(p) = dst.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    std::fs::copy(src, dst).map_err(|e| format!("拷贝 {} 失败：{e}", dst.display()))?;
    Ok(())
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let e = e.map_err(|e| e.to_string())?;
        let s = e.path();
        let d = dst.join(e.file_name());
        if s.is_dir() {
            copy_tree(&s, &d)?;
        } else {
            install_file(&s, &d)?;
        }
    }
    Ok(())
}

/// 更新替换留下的 *.old（旧 exe 改名的产物）启动就删；删不掉说明旧进程还没退干净，
/// 不管它，下次启动再试。
pub fn cleanup_old_backups() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    // 只删自己留下的：便携版可能放在用户自己的文件夹里，别的 .old 不是我们的
    let exe_name = exe.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let ours = [exe_name.as_str(), "gopeed.exe", "WebView2Loader.dll"];
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.strip_suffix(".old").is_some_and(|n| ours.iter().any(|o| o.eq_ignore_ascii_case(n))) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_by_numbers() {
        assert!(is_newer("1.10.0", "1.9.2"));
        assert!(is_newer("2.0", "1.99.99"));
        assert!(!is_newer("1.9.2", "1.10.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("开发版", "0.1.0"));
    }

    #[test]
    fn release_notes() {
        let body = "## 修复\n- a\n## 更新内容\n- 封面\n- 速度\n## 其它\n- x";
        assert_eq!(notes(body), vec!["封面", "速度"]);
    }

    #[test]
    fn pick_win_asset() {
        let v = json!([{
            "draft": false,
            "tag_name": "win-v1.2.3",
            "html_url": "u",
            "body": "## 更新内容\n- ok",
            "assets": [{
                "name": "shandian-windows-1.2.3-x64.zip",
                "browser_download_url": "d",
                "digest": "sha256:ab"
            }]
        }]);
        let r = parse_feed(&v.to_string()).unwrap();
        assert_eq!(r.version, "1.2.3");
        assert_eq!(r.asset.as_deref(), Some("d"));
        assert_eq!(r.notes, vec!["ok"]);
    }
}
