// 迅雷通道（对齐 Mac 版 ThunderBridge.swift）：把任务交给本机迅雷下，进度仍显示在闪电全能下载里。
//
// 只用公开入口和观察：不碰迅雷协议、会员和登录信息。
//   交接：磁力走 ThunderAgent COM（迅雷给浏览器扩展用的公开接口），直链走 thunder:// 协议，这样才弹「新建任务」窗口。
//         用无障碍接口（MSAA）改窗口里的下载路径、按「立即下载」，不模拟鼠标键盘。
//   保存位置：迅雷 25.x 忽略 AddTask / AddTask10 的目录参数，也会写回改过的 config.json（2026-11-05 实测），
//         唯一有效的是确认窗口里的下载路径：改完读回新值，再把焦点移开提交，然后才按。
//         迅雷偶尔不弹窗直接静默开始，这时文件在它的默认位置，下完再移到目标目录（move_to，同盘只是改名）。
//   进度和速度：**估算值，来自迅雷界面上显示的文字**（MSAA 读「71.1MB/209MB」「1.3MB/s」），精度只有界面那几位。
//         .xltd 文件一开始就占满总大小，看文件大小量不出进度。行不在屏幕上、切到别的页面时读不到，
//         就保留上次的数，速度记 0。
//   完成：只看磁盘：.xltd 临时文件连续几轮都没有、正式文件在。这一条不依赖界面。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::State;

use crate::AppState;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThunderJob {
    pub id: String,
    pub name: String,
    pub url: String,
    /// 被接替的内核任务，交接时已暂停并隐藏；迅雷这条被移除时它会回到列表
    pub source: String,
    pub dir: String,
    /// 迅雷实际用的名字。同名时迅雷会改成 name(1).ext，所以不能照搬 name
    pub actual: String,
    pub hash: String,
    pub total: u64,
    pub downloaded: u64,
    pub speed: u64,
    pub paused: bool,
    /// 交接后替用户按过几次「开始」。用户自己暂停后置为不再自动
    #[serde(default)]
    pub auto_start: u32,
    /// 迅雷任务行上的「已下载 a/b 项」
    #[serde(default)]
    pub items: (u32, u32),
    /// 勾选了几个文件（交接时或抽屉里读到的）。迅雷任务行上的总数不一定反映勾选
    #[serde(default)]
    pub selected: u32,
    /// 迅雷新建窗口已关、却还没找到这条任务的时刻（毫秒）。用户在迅雷里取消了新建就会一直这样
    #[serde(default)]
    pub missing_since: i64,
    /// 迅雷界面上写的失败原因（比如「任务连接超时，无法继续下载」）
    pub message: String,
    pub done: bool,
    /// 用户选的保存目录；下完从迅雷默认位置移过来
    pub target: String,
    pub moving: bool,
    /// 见过 .xltd：BT 文件夹里字幕先下完，没这个标记容易把刚开始的任务当成完成
    pub seen_partial: bool,
    pub done_streak: u32,
    pub created: i64,
    pub finished: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThunderStatus {
    pub installed: bool,
    pub save_dir: String,
}

static JOBS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn jobs_file(state: &AppState) -> PathBuf {
    crate::migrate::local_path(&state.local, "thunder-jobs.json")
}

pub fn load_jobs(state: &AppState) -> Vec<ThunderJob> {
    let _g = JOBS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::fs::read_to_string(jobs_file(state)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_jobs(state: &AppState, jobs: &[ThunderJob]) {
    let _g = JOBS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(t) = serde_json::to_string_pretty(jobs) {
        let _ = std::fs::write(jobs_file(state), t);
    }
}

fn edit_jobs(state: &AppState, edit: impl FnOnce(&mut Vec<ThunderJob>)) {
    let _guard = JOBS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = jobs_file(state);
    let mut jobs = std::fs::read_to_string(&path).ok()
        .and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    edit(&mut jobs);
    if let Ok(content) = serde_json::to_string_pretty(&jobs) {
        let _ = std::fs::write(path, content);
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ---------------- 安装位置和保存位置 ----------------

/// 迅雷安装目录（注册表里它自己登记的），程序不在就算没装
fn install_dir() -> Option<PathBuf> {
    use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    for key in [r"SOFTWARE\Thunder Network\ThunderOem\thunder_backwnd", r"SOFTWARE\WOW6432Node\Thunder Network\ThunderOem\thunder_backwnd"] {
        let Ok(k) = hklm.open_subkey(key) else { continue };
        let Ok(dir) = k.get_value::<String, _>("instdir") else { continue };
        let dir = PathBuf::from(dir);
        if dir.join("Program").join("Thunder.exe").exists() {
            return Some(dir);
        }
    }
    None
}

/// 迅雷可能存到哪：默认位置、上次用的位置、初始位置。只读这几项路径设置
fn save_roots() -> Vec<PathBuf> {
    let Some(dir) = install_dir() else { return Vec::new() };
    let text = std::fs::read_to_string(dir.join("Profiles").join("config.json")).unwrap_or_default();
    roots_from_config(&text)
}

fn roots_from_config(text: &str) -> Vec<PathBuf> {
    let v: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    let mut out: Vec<PathBuf> = Vec::new();
    for s in [
        v["TaskDefaultSettings"]["DefaultPath"].as_str(),
        v["PathAndCategory"]["LastUsedPath"].as_str(),
        v["TaskDefaultSettings"]["InitPath"].as_str(),
    ]
    .into_iter()
    .flatten()
    {
        let mut s = s.trim().replace('/', "\\");
        if s.is_empty() {
            continue;
        }
        // 配置里可能写成 Y:，那是「Y 盘当前目录」，要补斜杠
        if s.len() == 2 && s.ends_with(':') {
            s.push('\\');
        }
        let p = PathBuf::from(s);
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

// ---------------- 纯逻辑：解析界面文字、认文件 ----------------

/// "60.5MB" / "564.3KB" / "0B" → 字节数（界面只有三四位有效数字，所以是估算）
fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let n: f64 = s[..split].parse().ok()?;
    let mult = match s[split..].trim().to_ascii_uppercase().as_str() {
        "B" => 1.0,
        "KB" => 1024.0,
        "MB" => 1024.0 * 1024.0,
        "GB" => 1024.0 * 1024.0 * 1024.0,
        "TB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * mult) as u64)
}

#[derive(Debug, Default, PartialEq)]
struct RowInfo {
    downloaded: Option<u64>,
    total: Option<u64>,
    speed: Option<u64>,
    paused: bool,
    error: Option<String>,
    items: Option<(u32, u32)>,
}

/// 从主窗口的文字序列里找这条任务。一行的顺序是：名字、已下载 a/b 项、已下/总大小、[剩余]、速度或状态
fn parse_row(leaves: &[String], name: &str) -> Option<RowInfo> {
    let start = leaves.iter().position(|l| l == name)?;
    let rest = &leaves[start + 1..];
    // 没有「已下载 a/b 项」的同名文字不是下载中的任务行（比如已完成页、别处的标题）
    if !rest.first()?.starts_with("已下载") {
        return None;
    }
    let mut info = RowInfo::default();
    // 「已下载 0/5 项 」
    let digits: String = rest[0].chars().filter(|c| c.is_ascii_digit() || *c == '/').collect();
    if let Some((a, b)) = digits.split_once('/') {
        info.items = a.parse().ok().zip(b.parse().ok());
    }
    for l in &rest[1..] {
        if l.starts_with("已下载") {
            break; // 下一行了
        }
        if let Some(sp) = l.strip_suffix("/s") {
            info.speed = info.speed.or(parse_bytes(sp));
        } else if let Some((a, b)) = l.split_once('/') {
            if let (Some(a), Some(b)) = (parse_bytes(a), parse_bytes(b)) {
                info.downloaded = Some(a);
                info.total = Some(b);
            }
        } else if l == "已暂停" {
            info.paused = true;
        } else if l.contains("无法继续") || l.contains("失败") {
            info.error = Some(l.clone());
        }
    }
    Some(info)
}

/// n 是不是 base 或迅雷改名后的 base(1).ext
fn is_variant(base: &str, n: &str) -> bool {
    if n.eq_ignore_ascii_case(base) {
        return true;
    }
    let (stem, ext) = match base.rfind('.') {
        Some(i) if i > 0 => (&base[..i], &base[i..]),
        _ => (base, ""),
    };
    let lower = n.to_lowercase();
    let Some(mid) = lower.strip_prefix(&stem.to_lowercase()).and_then(|x| x.strip_suffix(&ext.to_lowercase())) else {
        return false;
    };
    mid.len() > 2 && mid.starts_with('(') && mid.ends_with(')') && mid[1..mid.len() - 1].bytes().all(|b| b.is_ascii_digit())
}

/// 迅雷的临时文件名还原成正式名：a.iso.xltd、a.iso.xltd.cfg、a.mp4.bt.xltd
fn strip_partial(n: &str) -> Option<&str> {
    n.strip_suffix(".xltd.cfg").or_else(|| n.strip_suffix(".xltd")).map(|x| x.strip_suffix(".bt").unwrap_or(x))
}

fn info_hash(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    let Some(i) = lower.find("btih:") else { return String::new() };
    url[i + 5..].split('&').next().unwrap_or("").to_ascii_uppercase()
}

fn url_name(url: &str) -> String {
    let Ok(u) = url::Url::parse(url) else { return String::new() };
    if u.scheme().eq_ignore_ascii_case("magnet") {
        return u.query_pairs().find(|(k, _)| k == "dn").map(|(_, v)| v.into_owned()).unwrap_or_default();
    }
    let Some(last) = u.path_segments().and_then(|mut s| s.next_back().map(str::to_string)) else { return String::new() };
    // 路径里的 %20 还原成文字，和迅雷显示的文件名一致
    url::form_urlencoded::parse(last.replace('+', "%2B").as_bytes()).map(|(k, _)| k.into_owned()).next().unwrap_or(last)
}

/// 种子 info 里的 name。只取第一个 4:name，后面跟的长度和内容
fn torrent_name(data: &[u8]) -> Option<String> {
    let at = data.windows(6).position(|w| w == b"4:name")? + 6;
    let colon = at + data[at..].iter().take(8).position(|b| *b == b':')?;
    let len: usize = std::str::from_utf8(&data[at..colon]).ok()?.parse().ok()?;
    let name = data.get(colon + 1..colon + 1 + len)?;
    String::from_utf8(name.to_vec()).ok()
}

/// 「已选 3/11 个文件」里的 3
fn selected_count(text: &str) -> Option<usize> {
    let rest = text.strip_prefix("已选 ")?;
    let (a, b) = rest.split_once('/')?;
    b.contains("个文件").then_some(())?;
    a.trim().parse().ok()
}

fn is_direct(url: &str) -> bool {
    let u = url.to_ascii_lowercase();
    u.starts_with("http") || u.starts_with("ftp")
}

/// 迅雷的公开链接格式：thunder://base64("AA" + 链接 + "ZZ")
fn thunder_link(url: &str) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let raw = format!("AA{url}ZZ");
    let mut out = String::from("thunder://");
    for c in raw.as_bytes().chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn mtime_ms(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 目录大小和里面还有没有 .xltd。迅雷放在目录里的 .torrent 不算内容
fn walk(p: &Path, depth: u32) -> (u64, bool) {
    let mut size = 0;
    let mut partial = false;
    let Ok(rd) = std::fs::read_dir(p) else { return (0, false) };
    for e in rd.flatten() {
        let path = e.path();
        if path.is_dir() {
            if depth < 8 {
                let (s, q) = walk(&path, depth + 1);
                size += s;
                partial |= q;
            }
            continue;
        }
        let n = e.file_name().to_string_lossy().to_string();
        if strip_partial(&n).is_some() {
            partial = true;
        } else if !n.to_ascii_lowercase().ends_with(".torrent") {
            size += e.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    (size, partial)
}

/// 找迅雷给这条任务落的文件。优先认 <infohash>.torrent（迅雷放在 BT 文件夹里），其次认名字。
/// 只认交接之后动过的，免得认到以前下过的同名文件
fn locate(job: &ThunderJob, roots: &[PathBuf]) -> Option<(PathBuf, String)> {
    let since = job.created - 10_000;
    for root in roots {
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        let entries: Vec<_> = rd.flatten().collect();
        if !job.hash.is_empty() {
            for e in &entries {
                if e.path().is_dir() && e.path().join(format!("{}.torrent", job.hash)).exists() {
                    return Some((root.clone(), e.file_name().to_string_lossy().into_owned()));
                }
            }
        }
        // 单文件种子直接落在目录里，旁边放着 <infohash>.torrent。同目录可能有别的任务，名字要从种子里读
        if !job.hash.is_empty() {
            if let Some(n) = std::fs::read(root.join(format!("{}.torrent", job.hash))).ok().and_then(|d| torrent_name(&d)) {
                return Some((root.clone(), n));
            }
        }
        if job.name.is_empty() {
            continue;
        }
        let mut best: Option<(i64, String)> = None;
        for e in &entries {
            let n = e.file_name().to_string_lossy().to_string();
            let real = strip_partial(&n).unwrap_or(&n).to_string();
            let t = mtime_ms(&e.path());
            if t >= since && is_variant(&job.name, &real) && best.as_ref().map_or(true, |b| t > b.0) {
                best = Some((t, real));
            }
        }
        if let Some((_, real)) = best {
            return Some((root.clone(), real));
        }
    }
    None
}

/// 看磁盘：返回 (正式内容的大小, 还有没有临时文件, 临时文件大小)。什么都没有返回 None
fn inspect(dir: &Path, actual: &str) -> Option<(u64, bool, u64)> {
    let main = dir.join(actual);
    if main.is_dir() {
        let (size, partial) = walk(&main, 0);
        return Some((size, partial, 0));
    }
    // 单文件的 .xltd 一开始就占满总大小，可以当总大小用，不能当进度用
    for suffix in ["xltd", "bt.xltd"] {
        if let Ok(m) = std::fs::metadata(dir.join(format!("{actual}.{suffix}"))) {
            return Some((0, true, m.len()));
        }
    }
    std::fs::metadata(&main).ok().map(|m| (m.len(), false, 0))
}

/// 根据这一轮观察更新任务。返回 true = 刚刚下完
fn observe(job: &mut ThunderJob, leaves: &[String], roots: &[PathBuf]) -> bool {
    if job.done || job.moving {
        return false;
    }
    if job.actual.is_empty() {
        // 先找用户选的目录（确认窗口改路径成功时就在这里），再找迅雷的默认位置（静默开始时）
        let mut all: Vec<PathBuf> = Vec::new();
        if !job.target.is_empty() {
            all.push(PathBuf::from(&job.target));
        }
        all.extend(roots.iter().filter(|p| p.as_os_str() != job.target.as_str()).cloned());
        if let Some((dir, actual)) = locate(job, &all) {
            job.dir = dir.to_string_lossy().into_owned();
            job.actual = actual;
        }
    }
    let row = if job.actual.is_empty() { None } else { parse_row(leaves, &job.actual) };
    match &row {
        Some(r) => {
            job.downloaded = r.downloaded.unwrap_or(job.downloaded);
            job.total = r.total.unwrap_or(job.total);
            job.speed = r.speed.unwrap_or(0);
            job.paused = r.paused;
            job.items = r.items.unwrap_or(job.items);
            job.message = r.error.clone().unwrap_or_default();
        }
        None => job.speed = 0,
    }
    if job.actual.is_empty() {
        return false;
    }
    let Some((size, partial, partial_total)) = inspect(Path::new(&job.dir), &job.actual) else {
        job.done_streak = 0;
        return false;
    };
    // 迅雷已经把这条挪出「下载中」，最后读到的字节数也到了总大小：剩下的临时文件是没勾选、
    // 但下过一点的文件留下的，迅雷不会再管它，不能因为它一直算没下完（2026-11-06 卡在 99%）
    let leftover = row.is_none() && job.total > 0 && job.downloaded >= job.total;
    if partial && !leftover {
        job.seen_partial = true;
        job.done_streak = 0;
        if job.total == 0 {
            job.total = partial_total;
        }
        return false;
    }
    if size == 0 {
        return false;
    }
    // 连续几轮都没有临时文件才算完：多文件种子两个文件交替、迅雷还没建下一个 .xltd 时不会误判。
    // 不能用「界面上还有这一行」否决：任务已挪到已完成页后，无障碍树里还会留着旧行，
    // 数字不再变化（2026-11-05 实测，屏幕上已经没有这一行）
    job.done_streak += 1;
    if job.done_streak < 3 {
        return false;
    }
    job.done = true;
    job.paused = false;
    job.message.clear();
    job.speed = 0;
    job.total = size;
    job.downloaded = size;
    job.finished = now_ms();
    true
}

// ---------------- 下完移到用户选的目录 ----------------

fn copy_all(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)?.flatten() {
            copy_all(&e.path(), &to.join(e.file_name()))?;
        }
        return Ok(());
    }
    let n = std::fs::copy(from, to)?;
    if n != std::fs::metadata(from)?.len() {
        return Err(std::io::Error::other("复制不完整"));
    }
    Ok(())
}

/// 目标里同名时加编号，不覆盖用户已有的文件
fn free_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && !dir.join(name).is_dir() => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..1000).map(|n| format!("{stem}({n}){ext}")).find(|c| !dir.join(c).exists()).unwrap_or_else(|| name.to_string())
}

/// 把 dir/actual 移到 target，返回最终名字。同盘改名；跨盘先复制到临时名、核对大小，再删原件。
/// 迅雷还占着文件时重试一会儿；一直不行就报错，文件留在原处（任务仍算完成，只是位置不同）
fn move_to(dir: &Path, actual: &str, wanted: &str, target: &Path) -> Result<String, String> {
    let from = dir.join(actual);
    if dir == target {
        return Ok(actual.to_string());
    }
    std::fs::create_dir_all(target).map_err(|e| e.to_string())?;
    // 迅雷放在 BT 文件夹里的种子不是用户要的内容，也别让它占着移动
    if from.is_dir() {
        if let Ok(rd) = std::fs::read_dir(&from) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".torrent") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
    let name = free_name(target, wanted);
    let to = target.join(&name);
    let mut last = String::new();
    for _ in 0..30 {
        match std::fs::rename(&from, &to) {
            Ok(()) => return Ok(name),
            // 17 = 跨盘，改名不行，走复制
            Err(e) if e.raw_os_error() == Some(17) => {
                let tmp = target.join(format!("{name}.sdmove"));
                let _ = std::fs::remove_dir_all(&tmp);
                let _ = std::fs::remove_file(&tmp);
                match copy_all(&from, &tmp) {
                    Ok(()) => {
                        std::fs::rename(&tmp, &to).map_err(|e| e.to_string())?;
                        // 新位置已核对过大小，原件删不掉（迅雷占着）也不影响使用
                        let _ = if from.is_dir() { std::fs::remove_dir_all(&from) } else { std::fs::remove_file(&from) };
                        return Ok(name);
                    }
                    Err(e) => {
                        let _ = std::fs::remove_dir_all(&tmp);
                        let _ = std::fs::remove_file(&tmp);
                        last = e.to_string();
                    }
                }
            }
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    Err(last)
}

// ---------------- 拼进任务列表 ----------------

/// 这条任务的文件：BT 文件夹里取最大的文件，播放、打开都用它
pub fn file_of(job: &ThunderJob) -> PathBuf {
    let dir = PathBuf::from(&job.dir);
    if job.actual.is_empty() {
        return dir;
    }
    let main = dir.join(&job.actual);
    if main.is_dir() {
        let mut best: Option<(u64, PathBuf)> = None;
        if let Ok(rd) = std::fs::read_dir(&main) {
            for e in rd.flatten() {
                let (p, n) = (e.path(), e.file_name().to_string_lossy().to_lowercase());
                if p.is_file() && !n.ends_with(".torrent") && strip_partial(&n).is_none() {
                    let s = e.metadata().map(|m| m.len()).unwrap_or(0);
                    if best.as_ref().map_or(true, |b| s > b.0) {
                        best = Some((s, p));
                    }
                }
            }
        }
        return best.map(|b| b.1).unwrap_or(main);
    }
    main
}

/// 迅雷任务转回本机：交接时原任务只是暂停并隐藏，直接恢复它，已下的进度还在。
/// 返回恢复的任务 id；原任务不在了（空）由界面新建一条
#[tauri::command]
pub async fn thunder_to_local(id: String, state: tauri::State<'_, AppState>) -> Result<String, String> {
    let xl = id.strip_prefix("xl:").unwrap_or(&id).to_string();
    let job = find(&state, &xl).ok_or("找不到这个任务")?;
    let (port, token) = {
        let e = state.engine.lock().unwrap();
        (e.port, e.token.clone())
    };
    let api = crate::api::Api::new(port, token);
    let exists = !job.source.is_empty() && api.list_tasks().await.map_err(|e| e.to_string())?.iter().any(|t| t.id == job.source);
    if !exists {
        return Ok(String::new());
    }
    // 先去掉迅雷这条（否则原任务继续被隐藏），迅雷那边后台删
    remove_job(&state, &xl).await;
    let source = job.source.clone();
    tauri::async_runtime::spawn(async move {
        let _ = delete_in_thunder(&job).await;
    });
    api.resume(&source).await.map_err(|e| e.to_string())?;
    Ok(source)
}

fn remember_selected(state: &AppState, id: &str, n: u32) {
    let key = id.strip_prefix("xl:").unwrap_or(id);
    let mut jobs = load_jobs(state);
    if let Some(j) = jobs.iter_mut().find(|j| j.id == key).filter(|j| j.selected != n) {
        j.selected = n;
        save_jobs(state, &jobs);
    }
}

fn panel_name(state: &AppState, id: &str) -> Result<String, String> {
    let job = find(state, id.strip_prefix("xl:").unwrap_or(id)).ok_or("找不到这个任务")?;
    if job.actual.is_empty() {
        return Err("迅雷还没开始这条任务，稍后再试".into());
    }
    Ok(job.actual)
}

/// 文件抽屉：读迅雷详情面板。第一次会打开面板，之后直接读
pub async fn panel_files(state: &AppState, id: &str) -> Result<Vec<serde_json::Value>, String> {
    let name = panel_name(state, id)?;
    let rows = tauri::async_runtime::spawn_blocking(move || win::quietly(|| win::panel_read(&name)))
        .await
        .map_err(|e| e.to_string())?
        .ok_or("迅雷窗口里读不到这条任务，请打开迅雷的「下载中」页面后再试")?;
    remember_selected(state, id, rows.iter().filter(|f| f.checked).count() as u32);
    Ok(rows
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let size = parse_bytes(&f.size).unwrap_or(0);
            let pct: f64 = f.percent.trim_end_matches('%').parse().unwrap_or(0.0);
            let status = if !f.checked { "skip" } else if pct >= 100.0 { "done" } else { "running" };
            json!({ "index": i, "name": f.name, "size": size, "downloaded": (size as f64 * pct / 100.0) as u64, "percent": f.percent, "status": status, "selected": f.checked })
        })
        .collect())
}

pub async fn panel_select(state: &AppState, id: &str, select: Vec<usize>) -> Result<(), String> {
    let name = panel_name(state, id)?;
    let picked = select.len() as u32;
    let result = tauri::async_runtime::spawn_blocking(move || {
        win::quietly(|| {
            let count = win::panel_read(&name).ok_or("迅雷窗口里读不到这条任务")?.len();
            let wanted: Vec<bool> = (0..count).map(|i| select.contains(&i)).collect();
            win::panel_apply(&name, &wanted)
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    if result.is_ok() {
        remember_selected(state, id, picked);
    }
    result
}

/// 抽屉收起时把迅雷的面板也关上
#[tauri::command]
pub async fn thunder_panel_close() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(win::panel_dismiss).await.map_err(|e| e.to_string())
}

pub fn find(state: &AppState, id: &str) -> Option<ThunderJob> {
    load_jobs(state).into_iter().find(|j| j.id == id)
}

pub fn hides(jobs: &[ThunderJob], engine_id: &str) -> bool {
    jobs.iter().any(|j| j.source == engine_id)
}

pub fn jobs_as_tasks(state: &AppState) -> Vec<crate::api::Task> {
    jobs_as_tasks_matching(state, None)
}

pub(crate) fn jobs_as_tasks_matching(state: &AppState, key: Option<&str>) -> Vec<crate::api::Task> {
    load_jobs(state)
        .into_iter()
          .filter(|j| key.map_or(true, |key| crate::links::canonical_url(&j.url) == key))
        .filter_map(|j| {
            let shown = if j.actual.is_empty() { &j.name } else { &j.actual };
            let status = if j.done { "done" } else if j.moving { "running" } else if !j.message.is_empty() { "error" } else if j.paused { "pause" } else { "running" };
            let mut t: crate::api::Task = serde_json::from_value(json!({
                "id": format!("xl:{}", j.id),
                "shandianError": j.message,
                "name": shown,
                "protocol": "thunder",
                "status": status,
                "progress": { "downloaded": j.downloaded, "speed": j.speed },
                "meta": {
                    "req": { "url": j.url },
                    "res": { "name": shown, "size": j.total, "files": [] },
                    "opts": { "name": shown, "path": j.dir },
                },
            }))
            .ok()?;
            // 分类和普通任务一样按链接算（磁力归电影），不单独归到「其他」
            t.kind = crate::links::LinkKind::of(&j.url).map(|k| k.label().to_string());
            t.created = Some(j.created.max(0) as u64);
            t.xl_items = (j.items.1 > 0).then_some((j.items.0, if j.selected > 0 { j.selected } else { j.items.1 }));
            if j.done {
                t.finished = Some(j.finished.max(0) as u64);
                t.missing = !Path::new(&j.dir).join(&j.actual).exists();
            } else if j.moving {
                t.stage = Some("正在保存".into());
            } else if !j.message.is_empty() {
                // 错误原因已经在 shandianError 里
            } else if j.paused {
                t.stage = Some("已在迅雷暂停".into());
            } else if j.speed == 0 && j.downloaded == 0 {
                // 一个字节都还没下时才写「连接中」（和普通任务一样，不把迅雷摆到台面上）。
                // 下过一段、暂时 0 速就照常显示进度，不再一直挂「连接中」
                t.stage = Some("连接中".into());
            }
            Some(t)
        })
        .collect()
}

/// 把迅雷里的这条任务也删掉（进迅雷回收站，文件不动）。返回 false = 没敢删或没删成，要提示用户自己去删
pub async fn delete_in_thunder(job: &ThunderJob) -> bool {
    if job.actual.is_empty() {
        return false;
    }
    let name = job.actual.clone();
    tauri::async_runtime::spawn_blocking(move || win::delete_row(&name)).await.unwrap_or(false)
}

/// 这条任务在磁盘上的全部东西：正式文件或文件夹、临时文件、迅雷放的种子
pub fn files_of(job: &ThunderJob) -> Vec<PathBuf> {
    if job.actual.is_empty() {
        return Vec::new();
    }
    let dir = PathBuf::from(&job.dir);
    let mut all = vec![
        dir.join(&job.actual),
        dir.join(format!("{}.xltd", job.actual)),
        dir.join(format!("{}.xltd.cfg", job.actual)),
        dir.join(format!("{}.bt.xltd", job.actual)),
        dir.join(format!("{}.bt.xltd.cfg", job.actual)),
    ];
    if !job.hash.is_empty() {
        all.push(dir.join(format!("{}.torrent", job.hash)));
    }
    all.retain(|p| p.exists());
    all
}

/// 用户移除：这里不再显示。迅雷那边由调用方先 delete_in_thunder。
/// 没下完时被接替的内核任务回到列表（已暂停，可以继续）；下完了就把它的记录也清掉
pub async fn remove_job(state: &AppState, id: &str) -> Option<ThunderJob> {
    let mut jobs = load_jobs(state);
    let job = jobs.iter().find(|j| j.id == id).cloned()?;
    jobs.retain(|j| j.id != id);
    save_jobs(state, &jobs);
    if job.done && !job.source.is_empty() {
        let (port, token) = {
            let e = state.engine.lock().unwrap();
            (e.port, e.token.clone())
        };
        let _ = crate::api::Api::new(port, token).delete(&job.source, false).await;
    }
    Some(job)
}

// ---------------- 交接 ----------------

/// 用户在闪电里勾的文件。全选或没选过时返回空，不用动迅雷的勾选。返回 (要下的, 全部文件名)
fn selection_of(t: &crate::api::Task) -> (Vec<String>, Vec<String>) {
    let all: Vec<String> = t.files().into_iter().map(|f| f.name).collect();
    let picked = t.meta.as_ref().and_then(|m| m.opts.as_ref()).map(|o| o.select_files.clone()).unwrap_or_default();
    if picked.is_empty() || picked.len() >= all.len() {
        return (Vec::new(), all);
    }
    (picked.iter().filter_map(|i| all.get(*i).cloned()).collect(), all)
}

async fn hand_off(state: &AppState, source: &str, url: &str, name: &str, size: u64, target: &Path, select: (Vec<String>, Vec<String>)) -> Result<(), String> {
    hand_off_inner(state, source, url, name, size, target, select, false).await.map_err(|error| error.message)
}

#[derive(Debug, Serialize)]
pub struct ThunderHandoffError {
    code: &'static str,
    message: String,
}

impl ThunderHandoffError {
    fn duplicate(message: &str) -> Self {
        Self { code: "duplicate", message: message.into() }
    }
}

impl From<String> for ThunderHandoffError {
    fn from(message: String) -> Self {
        Self { code: "handoff", message }
    }
}

impl From<&str> for ThunderHandoffError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

async fn hand_off_inner(state: &AppState, source: &str, url: &str, name: &str, size: u64, target: &Path, select: (Vec<String>, Vec<String>), verify: bool) -> Result<(), ThunderHandoffError> {
    if !source.is_empty() && load_jobs(state).iter().any(|j| j.source == source) {
        return Err(ThunderHandoffError::duplicate("这条任务已经交给迅雷了"));
    }
    if install_dir().is_none() {
        return Err("没有检测到迅雷，任务继续由闪电全能下载下".into());
    }
    let link = url.to_string();
    if let Err(e) = std::fs::create_dir_all(target) {
        if verify {
            return Err(format!("无法创建迅雷下载目录：{e}").into());
        }
    }
    // 先记下当前前台，交接后还给用户
    let scene = win::Scene::capture();
    let mut title = url_name(url);
    if title.is_empty() {
        title = name.to_string();
    }
    let picked = select.0.len() as u32;
    // 迅雷一收到任务就会抢前台弹窗：先锁住前台、先开始盯窗口，再把任务交过去，不留空档
    win::lock_foreground(true);
    let limit = if is_direct(url) { 10 } else { 30 };
    let wanted = target.to_string_lossy().into_owned();
    let confirm_title = title.clone();
    let watch = tauri::async_runtime::spawn_blocking(move || win::quiet_confirm(&confirm_title, &wanted, &select, limit, scene));
    let added = tauri::async_runtime::spawn_blocking(move || win::add_task(&link)).await.map_err(|e| e.to_string())?;
    if let Err(e) = added {
        win::lock_foreground(false);
        return Err(e.into());
    }

    let job = ThunderJob {
        id: format!("{:x}", rand::random::<u64>()),
        name: title.clone(),
        url: url.to_string(),
        source: source.to_string(),
        dir: target.to_string_lossy().into_owned(),
        target: target.to_string_lossy().into_owned(),
        hash: info_hash(url),
        total: size,
        created: now_ms(),
        selected: picked,
        ..Default::default()
    };
    let job_id = job.id.clone();
    edit_jobs(state, |jobs| jobs.push(job));

    // 两边别一起下
    let (port, token) = {
        let e = state.engine.lock().unwrap();
        (e.port, e.token.clone())
    };
    if !source.is_empty() {
        let _ = crate::api::Api::new(port, token).pause(source).await;
    }

    // 磁力、ed2k 这类迅雷要弹窗确认，直链只弹一个提示。等确认的结果：
    // 用户在迅雷里取消了 → 立刻撤回，原任务回来接着下；卡住了 → 马上告诉用户去迅雷里点
    if verify {
        let confirmed = watch.await.map_err(|_| "迅雷确认窗口中断".to_string())?;
        match confirmed {
            win::Confirm::Duplicate | win::Confirm::Cancelled => {
                let outcome = if confirmed == win::Confirm::Duplicate {
                    ThunderHandoffError::duplicate("迅雷已有这条任务")
                } else {
                    "迅雷没有确认新任务，原下载已保留".into()
                };
                edit_jobs(state, |jobs| jobs.retain(|j| j.id != job_id));
                return Err(outcome);
            }
            win::Confirm::Stuck => {
                return Err("迅雷还在等确认。原任务保留，请在迅雷窗口核对下载位置".into());
            }
            win::Confirm::Started => {}
        }
        let until = Instant::now() + std::time::Duration::from_secs(20);
        while Instant::now() < until {
            if load_jobs(state).iter().any(|j| j.id == job_id && !j.actual.is_empty() && Path::new(&j.dir) == target) {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        }
        return Err("尚未核对到迅雷在指定目录开始下载，原任务保留".into());
    }
    let (src, shown) = (source.to_string(), title.clone());
    tauri::async_runtime::spawn(async move {
        use tauri::{Emitter, Manager};
        let Ok(outcome) = watch.await else { return };
        let Some(app) = crate::engine::APP.get() else { return };
        let st = app.state::<AppState>();
        match outcome {
            win::Confirm::Cancelled | win::Confirm::Duplicate => {
                edit_jobs(&st, |jobs| jobs.retain(|j| j.id != job_id));
                if !src.is_empty() {
                    let (port, token) = {
                        let e = st.engine.lock().unwrap();
                        (e.port, e.token.clone())
                    };
                    let _ = crate::api::Api::new(port, token).resume(&src).await;
                }
                let _ = app.emit("thunder-abandoned", serde_json::json!({ "name": shown, "back": !src.is_empty(), "cancelled": outcome == win::Confirm::Cancelled, "duplicate": outcome == win::Confirm::Duplicate }));
            }
            win::Confirm::Stuck => {
                let _ = app.emit("thunder-stuck", serde_json::json!({ "name": shown }));
            }
            win::Confirm::Started => {}
        }
    });
    Ok(())
}

/// 暂停 / 继续：替用户按迅雷任务行上的按钮（无障碍接口），状态下一轮从迅雷读回来
pub async fn set_paused(state: &AppState, id: &str, pause: bool) -> Result<(), String> {
    let job = find(state, id).ok_or("找不到这个任务")?;
    if job.done || job.paused == pause && job.message.is_empty() {
        return Ok(());
    }
    if job.actual.is_empty() {
        return Err("迅雷还没开始这条任务，稍后再试".into());
    }
    let name = job.actual.clone();
    // 只按一次，不读回状态再补按（迅雷界面的读数有时滞后，补按会把状态按回去）：
    // 窗口没最小化时发点击消息（迅雷 11-06 更新后只认这个）；最小化时按钮没有位置，用无障碍的默认动作
    let ok = tauri::async_runtime::spawn_blocking(move || {
        // 迅雷缩在托盘时主窗口已被销毁：先叫出来（原位，会闪一下），按完再关回托盘
        // 迅雷窗口一冒出来就压到最底下、不让它抢焦点（守卫先建后放：窗口收回后才解除）
        let _behind = win::keep_behind();
        // 不再固定等 1.5 秒，下面轮询这一行出现
        let _main = win::summon_main_settle(0);
        // 窗口只显示一次：点完盯着这一行，状态到了立刻收回；没到在同一次显示里补按（刚叫出来时第一下可能没生效，11-13 实测 4 次里 1 次）。
        // 刚叫出来的窗口任务列表还没加载，这一行找不到时等一会儿再找（最多 5 秒）
        let started = std::time::Instant::now();
        loop {
            match win::toggle_row(&name, pause) {
                Some(_) => return true,
                None if started.elapsed().as_secs() < 5 => std::thread::sleep(std::time::Duration::from_millis(150)),
                None => return win::press_row_toggle(&name),
            }
        }
    })
        .await
        .unwrap_or(false);
    if !ok {
        return Err("迅雷窗口里读不到这一行，请打开迅雷的「下载中」页面后再试".into());
    }
    let mut jobs = load_jobs(state);
    if let Some(j) = jobs.iter_mut().find(|j| j.id == id) {
        j.paused = pause;
        // 用户动过手，后面不再自动开始
        j.auto_start = 3;
        j.message.clear();
        j.speed = 0;
    }
    save_jobs(state, &jobs);
    Ok(())
}

/// 交接前查保存目录里有没有同名的。迅雷也会查，但那时窗口已经弹出来了，要在这里先问用户。
/// 返回同名的名字；名字还不知道（磁力没解析完）时查不了，由 quiet_confirm 处理迅雷的提示
#[tauri::command]
pub async fn thunder_conflict(id: String, state: State<'_, AppState>) -> Result<Option<serde_json::Value>, String> {
    let (port, token) = {
        let e = state.engine.lock().unwrap();
        (e.port, e.token.clone())
    };
    let list = crate::api::Api::new(port, token).list_tasks().await.map_err(|e| e.to_string())?;
    let Some(t) = list.iter().find(|t| t.id == id) else { return Ok(None) };
    let name = t.meta.as_ref().and_then(|m| m.res.as_ref()).map(|r| r.name.clone()).unwrap_or_default();
    let own = t.progress.as_ref().map_or(0, |p| p.downloaded) > 0;
    Ok((!name.is_empty() && target_of(&state, t).join(&name).exists()).then(|| json!({ "name": name, "own": own })))
}

/// 没有本机内核任务的交接（NAS 任务）：给定链接和保存位置
#[tauri::command]
pub async fn thunder_handoff_url(url: String, name: String, size: u64, target: String, state: State<'_, AppState>) -> Result<(), ThunderHandoffError> {
    hand_off_checked(&state, &url, &name, size, Path::new(&target), Default::default()).await
}

async fn hand_off_checked(state: &AppState, url: &str, name: &str, size: u64, root: &Path, select: (Vec<String>, Vec<String>)) -> Result<(), ThunderHandoffError> {
    static CHECKED_HANDOFF: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _gate = CHECKED_HANDOFF.try_lock().map_err(|_| "另一条迅雷任务正在确认，请稍后重试".to_string())?;
    let key = crate::links::canonical_url(&url);
    if load_jobs(state).iter().any(|j| crate::links::canonical_url(&j.url) == key) {
        return Err(ThunderHandoffError::duplicate("迅雷已有这条任务"));
    }
    if win::duplicate_task_open() {
        return Err(ThunderHandoffError::duplicate("迅雷有一个「任务已存在」提示"));
    }
    let label: String = crate::engine::safe_component(name).chars().take(28).collect();
    let label = if label.is_empty() { "下载任务" } else { &label };
    let target = root.join(format!("{label} - {:08x}", rand::random::<u32>()));
    let result = hand_off_inner(state, "", url, name, size, &target, select, true).await;
    if result.is_err() {
        let _ = std::fs::remove_dir(&target); // Only succeeds if no download file was created.
    }
    result
}

#[tauri::command]
pub fn thunder_status() -> ThunderStatus {
    ThunderStatus {
        installed: install_dir().is_some(),
        save_dir: save_roots().first().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
    }
}

#[tauri::command]
pub fn thunder_open() -> Result<(), String> {
    win::open_for_user()
}

/// 这条任务该存哪：用户投任务时选的目录，老任务按当前设置算
fn target_of(state: &AppState, t: &crate::api::Task) -> PathBuf {
    t.saved_dir().unwrap_or_else(|| default_target(state, &t.url()))
}

fn default_target(state: &AppState, url: &str) -> PathBuf {
    let kind = crate::links::LinkKind::of(url).unwrap_or(crate::links::LinkKind::Http);
    state.settings.lock().unwrap().dir_for(kind, &state.folders)
}

/// 右键「交给迅雷」。id 是内核任务
#[tauri::command]
pub async fn thunder_handoff(id: String, mode: Option<String>, state: State<'_, AppState>) -> Result<(), ThunderHandoffError> {
    let (port, token) = {
        let e = state.engine.lock().unwrap();
        (e.port, e.token.clone())
    };
    let list = crate::api::Api::new(port, token).list_tasks().await.map_err(|e| e.to_string())?;
    let t = list.iter().find(|t| t.id == id).ok_or("找不到这个任务")?;
    let name = t.meta.as_ref().and_then(|m| m.res.as_ref()).map(|r| r.name.clone()).or_else(|| t.name.clone()).unwrap_or_default();
    let target = target_of(&state, t);
    // 种子文件任务的 url 不是迅雷能认的链接，用 infohash 拼成磁力
    let mut link = t.url();
    if !link.to_ascii_lowercase().starts_with("magnet:") && !link.to_ascii_lowercase().starts_with("ed2k:") {
        let hash = t.meta.as_ref().and_then(|m| m.res.as_ref()).map(|r| r.hash.clone()).unwrap_or_default();
        if hash.is_empty() {
            return Err("这个种子还没解析完，稍后再交给迅雷".into());
        }
        link = format!("magnet:?xt=urn:btih:{hash}");
    }
    let select = selection_of(t);
    let total = t.total();
    // replace：闪电这条暂停但留在列表里，迅雷另存一份；没传 mode（没有已下内容）才由迅雷那行接替；
    // trash：闪电这条连已下内容进回收站，迅雷从头下；both：闪电这条接着下，迅雷另下一份，可以比快慢
    match mode.as_deref() {
        Some("trash") => {
            hand_off_checked(&state, &link, &name, total, &target.join("迅雷下载"), select).await?;
            crate::engine::delete_task(id.clone(), true, state.clone()).await?;
            Ok(())
        }
        Some("replace") => {
            hand_off_checked(&state, &link, &name, total, &target.join("迅雷下载"), select).await?;
            let (port, token) = {
                let e = state.engine.lock().unwrap();
                (e.port, e.token.clone())
            };
            crate::api::Api::new(port, token).pause(&id).await.map_err(|e| e.to_string().into())
        }
        Some("both") => hand_off_checked(&state, &link, &name, total, &target.join("迅雷下载"), select).await,
        _ => hand_off_inner(&state, &id, &link, &name, total, &target, select, false).await,
    }
}

// ---------------- 定时观察 ----------------

/// 下不动时的接力（设置 relay，默认 off）。触发条件对齐 Mac 版：磁力连续 3 分钟低于 50 KB/s。
/// cloud_thunder 时云盘提交失败、云端失败或 10 分钟没进度，再交给迅雷
fn relay_wants(relay: &str) -> (bool, bool) {
    (matches!(relay, "cloud" | "cloud_thunder"), matches!(relay, "thunder" | "cloud_thunder"))
}

/// 云端任务算不算没戏：失败了，或提交 10 分钟还是 0%
fn cloud_gave_up(j: &crate::pikpak::CloudJob, now: i64) -> bool {
    j.state == "error" || (j.state != "pulling" && j.progress == 0 && now - j.created > 10 * 60 * 1000)
}

/// 任务 3 秒看一次，顺带做接力
pub fn start_monitor(app: tauri::AppHandle) {
    use tauri::{Emitter, Manager};
    tauri::async_runtime::spawn(async move {
        let mut slow: HashMap<String, Instant> = HashMap::new();
        let mut handed: HashSet<String> = HashSet::new();
        let mut moving: HashSet<String> = HashSet::new();
        let mut busy = false;
        loop {
            // 有迅雷任务在下时 1.5 秒读一次迅雷界面，进度和速度跟得紧一些；没有时 3 秒
            tokio::time::sleep(std::time::Duration::from_millis(if busy { 1500 } else { 3000 })).await;
            let st = app.state::<AppState>();
            let state: &AppState = &st;
            busy = load_jobs(state).iter().any(|j| !j.done && !j.paused);

            let mut jobs = load_jobs(state);
            if jobs.iter().any(|j| !j.done) {
                let observed_ids: HashSet<_> = jobs.iter().map(|j| j.id.clone()).collect();
                let leaves = tauri::async_runtime::spawn_blocking(win::main_leaves).await.unwrap_or_default();
                let roots = save_roots();
                for j in jobs.iter_mut() {
                    if observe(j, &leaves, &roots) {
                        j.done = false;
                        j.moving = true;
                    }
                }
                // 迅雷收到任务后有时停着不动，要手动继续才下（2026-11-05 反馈）。只在交接后
                // 2 分钟内替用户按这一行的按钮，最多 3 次；之后的暂停算用户自己的，不碰
                let now_ms_val = now_ms();
                let stuck: Vec<String> = jobs
                    .iter_mut()
                    .filter(|j| j.paused && !j.done && !j.moving && !j.actual.is_empty() && !j.source.is_empty() && j.auto_start < 3 && now_ms_val - j.created < 120_000)
                    .map(|j| {
                        j.auto_start += 1;
                        j.paused = false;
                        j.actual.clone()
                    })
                    .collect();
                for name in stuck {
                    let _ = tauri::async_runtime::spawn_blocking(move || win::press_row_toggle(&name)).await;
                }
                // 交接后迅雷的新建窗口关了，过了 2 分钟还既没有文件、也没在迅雷列表里出现：
                // 多半是用户在迅雷里取消了新建（或迅雷没接住）。不能一直挂着「连接中」：
                // 撤掉这条，原来的任务回到闪电接着下
                let dialog_open = tauri::async_runtime::spawn_blocking(win::new_task_open).await.unwrap_or(false);
                let mut abandoned: Vec<ThunderJob> = Vec::new();
                for j in jobs.iter_mut().filter(|j| !j.done && !j.moving && j.actual.is_empty() && j.downloaded == 0) {
                    if dialog_open || now_ms_val - j.created < 30_000 {
                        j.missing_since = 0;
                        continue;
                    }
                    if j.missing_since == 0 {
                        j.missing_since = now_ms_val;
                    } else if now_ms_val - j.missing_since > 120_000 {
                        abandoned.push(j.clone());
                    }
                }
                if !abandoned.is_empty() {
                    jobs.retain(|j| !abandoned.iter().any(|a| a.id == j.id));
                    let (port, token) = {
                        let e = state.engine.lock().unwrap();
                        (e.port, e.token.clone())
                    };
                    for a in &abandoned {
                        if !a.source.is_empty() {
                            let _ = crate::api::Api::new(port, token.clone()).resume(&a.source).await;
                        }
                        let _ = app.emit("thunder-abandoned", serde_json::json!({ "name": a.name, "back": !a.source.is_empty() }));
                    }
                }
                // Keep tasks submitted during this sweep and honour any removals.
                edit_jobs(state, |current| {
                    jobs.retain(|j| current.iter().any(|n| n.id == j.id));
                    current.retain(|j| !observed_ids.contains(&j.id));
                    current.extend(jobs.iter().cloned());
                });
                // 下完的移到用户选的目录，移完才算完成。App 中途退出的，下次打开接着移
                for j in jobs.iter().filter(|j| j.moving && moving.insert(j.id.clone())) {
                    let (app2, j) = (app.clone(), j.clone());
                    tauri::async_runtime::spawn(async move {
                        let (dir, actual, target) = (PathBuf::from(&j.dir), j.actual.clone(), PathBuf::from(&j.target));
                        // 单文件种子旁边那个 <infohash>.torrent 是迅雷的，不留在用户目录里
                        if !j.hash.is_empty() {
                            let _ = std::fs::remove_file(dir.join(format!("{}.torrent", j.hash)));
                        }
                        let wanted = if !j.name.is_empty() && is_variant(&j.name, &actual) { j.name.clone() } else { actual.clone() };
                        let (d, a, w, t) = (dir, actual, wanted, target.clone());
                        let moved = tauri::async_runtime::spawn_blocking(move || if t.as_os_str().is_empty() { Ok(a) } else { move_to(&d, &a, &w, &t) }).await;
                        let st = app2.state::<AppState>();
                        let mut all = load_jobs(&st);
                        let mut shown = j.actual.clone();
                        if let Some(x) = all.iter_mut().find(|x| x.id == j.id) {
                            x.moving = false;
                            x.done = true;
                            if let Ok(Ok(name)) = moved {
                                if !j.target.is_empty() {
                                    x.dir = j.target.clone();
                                }
                                x.actual = name.clone();
                                shown = name;
                            }
                        }
                        save_jobs(&st, &all);
                        let _ = shown;
                    });
                }
            }

            let relay = state.settings.lock().unwrap().relay.clone();
            let (to_cloud, to_thunder) = relay_wants(&relay);
            let to_thunder = to_thunder && install_dir().is_some();
            let to_cloud = to_cloud && crate::pikpak::logged_in(state);
            if !to_cloud && !to_thunder {
                slow.clear();
                continue;
            }
            let (port, token) = {
                let e = state.engine.lock().unwrap();
                (e.port, e.token.clone())
            };
            let Ok(list) = crate::api::Api::new(port.clone(), token.clone()).list_tasks().await else { continue };

            // 云盘这一棒没戏了，交给迅雷
            if to_thunder && relay == "cloud_thunder" {
                for c in crate::pikpak::load_jobs(state) {
                    if !cloud_gave_up(&c, now_ms()) {
                        continue;
                    }
                    let src = list.iter().find(|t| t.url() == c.url);
                    let source = src.map(|t| t.id.clone()).unwrap_or_default();
                    let target = src.map(|t| target_of(state, t)).unwrap_or_else(|| default_target(state, &c.url));
                    if hand_off(state, &source, &c.url, &c.name, c.size, &target, src.map(selection_of).unwrap_or_default()).await.is_ok() {
                        let _ = crate::pikpak::cancel_job(state, &c.id).await;
                    }
                }
            }

            let clouds = crate::pikpak::load_jobs(state);
            let mut stuck_now = HashSet::new();
            for t in &list {
                let url = t.url();
                let total = t.total();
                let (got, speed) = t.progress.as_ref().map_or((0, 0), |p| (p.downloaded, p.speed));
                // 只差最后一点的不算冷门：这时交出去会把几 GB 重下一遍
                let nearly = total > 0 && got * 100 > total * 99;
                if t.status == "running"
                    && url.to_ascii_lowercase().starts_with("magnet:")
                    && speed < 50 * 1024
                    && !nearly
                    && t.shandian_verification.as_deref() != Some("checking")
                    && !hides(&jobs, &t.id)
                    && !clouds.iter().any(|c| c.url == url)
                {
                    stuck_now.insert(t.id.clone());
                    let since = *slow.entry(t.id.clone()).or_insert_with(Instant::now);
                    if since.elapsed().as_secs() > 180 && handed.insert(t.id.clone()) {
                        let name = t.meta.as_ref().and_then(|m| m.res.as_ref()).map(|r| r.name.clone()).unwrap_or_default();
                        let mut sent = false;
                        if to_cloud {
                            sent = crate::pikpak::submit(state, &url).await.is_ok();
                            if sent {
                                let _ = crate::api::Api::new(port.clone(), token.clone()).pause(&t.id).await;
                            }
                        }
                        if !sent && to_thunder {
                            let target = target_of(state, t);
                            let _ = hand_off(state, &t.id, &url, &name, total, &target, selection_of(t)).await;
                        }
                    }
                }
            }
            slow.retain(|id, _| stuck_now.contains(id));
        }
    });
}

// ---------------- 系统接口 ----------------

mod win {
    use windows::core::{Interface, BOOL, BSTR, GUID, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::System::Com::{
        CLSIDFromProgID, CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch, CLSCTX_ALL, COINIT_APARTMENTTHREADED, DISPATCH_METHOD, DISPPARAMS,
    };
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows::Win32::System::Variant::{VARIANT, VT_DISPATCH};
    use windows::Win32::UI::Accessibility::{AccessibleChildren, AccessibleObjectFromWindow, IAccessible};
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GetForegroundWindow, GetWindow, GetWindowLongPtrW, GetWindowRect, IsWindow, GWL_EXSTYLE, GW_HWNDNEXT, GW_HWNDPREV, SWP_NOMOVE, WS_EX_TOPMOST, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetForegroundWindow, SetWindowPos,
        ShowWindow, OBJID_CLIENT, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOWMINNOACTIVE,
    };

    const ROLE_TEXT: i32 = 41;
    const ROLE_BUTTON: i32 = 43;
    const ROLE_EDIT: i32 = 42;
    const ROLE_CHECK: i32 = 44;

    struct Com;
    impl Com {
        fn enter() -> Self {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            Com
        }
    }
    impl Drop for Com {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    /// ThunderAgent.AddTask(url, 文件名, 目录, 备注, 引用页, 开始方式=1 立即, 只从原始地址=0, 原始线程=5) + CommitTasks
    pub fn add_task(url: &str) -> Result<(), String> {
        let _com = Com::enter();
        if super::is_direct(url) {
            // COM 交直链时不弹窗，改不了目录
            let wide = HSTRING::from(super::thunder_link(url));
            let r = unsafe { windows::Win32::UI::Shell::ShellExecuteW(None, windows::core::w!("open"), &wide, None, None, windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE) };
            return if r.0 as isize > 32 { Ok(()) } else { Err("迅雷没有接受这条任务".into()) };
        }
        unsafe {
            let clsid = CLSIDFromProgID(&HSTRING::from("ThunderAgent.Agent64"))
                .or_else(|_| CLSIDFromProgID(&HSTRING::from("ThunderAgent.Agent")))
                .map_err(|_| "没有找到迅雷的下载接口，请重新安装迅雷".to_string())?;
            let agent: IDispatch = CoCreateInstance(&clsid, None, CLSCTX_ALL).map_err(|e| format!("连不上迅雷：{e}"))?;
            let s = |x: &str| VARIANT::from(BSTR::from(x));
            // IDispatch 的参数是倒着放的
            let mut args = [VARIANT::from(5i32), VARIANT::from(0i32), VARIANT::from(1i32), s(""), s(""), s(""), s(""), s(url)];
            invoke(&agent, "AddTask", &mut args).map_err(|e| format!("迅雷没有接受这条任务：{e}"))?;
            invoke(&agent, "CommitTasks", &mut []).map_err(|e| format!("迅雷没有接受这条任务：{e}"))?;
        }
        Ok(())
    }

    unsafe fn invoke(obj: &IDispatch, name: &str, args: &mut [VARIANT]) -> windows::core::Result<()> {
        let wide = HSTRING::from(name);
        let names = [PCWSTR(wide.as_ptr())];
        let mut id = 0;
        obj.GetIDsOfNames(&GUID::zeroed(), names.as_ptr(), 1, 0, &mut id)?;
        let params = DISPPARAMS { rgvarg: args.as_mut_ptr(), rgdispidNamedArgs: std::ptr::null_mut(), cArgs: args.len() as u32, cNamedArgs: 0 };
        obj.Invoke(id, &GUID::zeroed(), 0, DISPATCH_METHOD, &params, None, None, None)
    }

    fn is_thunder(hwnd: HWND) -> bool {
        unsafe {
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
            let mut buf = [0u16; 512];
            let mut n = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut n).is_ok();
            let _ = windows::Win32::Foundation::CloseHandle(h);
            ok && String::from_utf16_lossy(&buf[..n as usize]).to_ascii_lowercase().ends_with("\\thunder.exe")
        }
    }

    /// 迅雷进程里标题正好是 title 的顶层窗口
    /// 「新建任务」窗口里的文字和按钮（排查卡住的原因用）：每项是「角色:文字」
    pub fn dialog_texts() -> Vec<String> {
        let _com = Com::enter();
        let mut out = Vec::new();
        for top in windows_titled("新建任务", false) {
            for root in roots_of(top) {
                let mut budget = 3000;
                walk(&root, 0, &mut budget, &mut |a, text, role| {
                    let t = text.trim();
                    if !t.is_empty() && matches!(role, ROLE_TEXT | ROLE_BUTTON | ROLE_EDIT | ROLE_CHECK) {
                        let me = VARIANT::from(0i32);
                        let state = unsafe { a.get_accState(&me) }.ok().and_then(|v| i32::try_from(&v).ok()).unwrap_or(0);
                        // 0x1 = 不可用（按钮灰着）
                        let tag = match role { ROLE_BUTTON => "按钮", ROLE_EDIT => "输入", ROLE_CHECK => "勾选", _ => "文字" };
                        let value = if role == ROLE_EDIT {
                            unsafe { a.get_accValue(&me) }.map(|b| format!(" = {b}")).unwrap_or_default()
                        } else if role == ROLE_BUTTON {
                            let (mut l, mut tp, mut w, mut h) = (0, 0, 0, 0);
                            unsafe { a.accLocation(&mut l, &mut tp, &mut w, &mut h, &me) }.map(|_| format!(" @({l},{tp} {w}x{h})")).unwrap_or_default()
                        } else {
                            String::new()
                        };
                        out.push(format!("{tag}{}:{}{value}", if state & 1 != 0 { "(灰)" } else { "" }, t.chars().take(60).collect::<String>()));
                    }
                    false
                });
            }
        }
        out
    }

    /// 关掉「新建任务」窗口（等于点取消）
    pub fn close_new_task() {
        for h in windows_titled("新建任务", false) {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(Some(h), windows::Win32::UI::WindowsAndMessaging::WM_CLOSE, windows::Win32::Foundation::WPARAM(0), LPARAM(0));
            }
        }
    }

    /// 迅雷的「新建任务」窗口还开着（包括被挪到屏幕外的）
    pub fn new_task_open() -> bool {
        !windows_titled("新建任务", false).is_empty()
    }

    pub fn duplicate_task_open() -> bool {
        !windows_titled("重复任务", true).is_empty()
    }

    pub fn open_for_user() -> Result<(), String> {
        use windows::Win32::UI::WindowsAndMessaging::{SW_RESTORE, SW_SHOWNORMAL};
        let current = windows_titled("重复任务", true).into_iter().next()
            .or_else(|| windows_titled("迅雷", true).into_iter().next());
        if let Some(h) = current {
            unsafe {
                if IsIconic(h).as_bool() {
                    let _ = ShowWindow(h, SW_RESTORE);
                }
                if SetForegroundWindow(h).as_bool() {
                    return Ok(());
                }
            }
        }
        let dir = super::install_dir().ok_or("没有找到迅雷")?;
        let exe = HSTRING::from(dir.join("Program").join("Thunder.exe").to_string_lossy().as_ref());
        let result = unsafe {
            windows::Win32::UI::Shell::ShellExecuteW(None, windows::core::w!("open"), &exe, None, None, SW_SHOWNORMAL)
        };
        if result.0 as isize > 32 { Ok(()) } else { Err("无法打开迅雷".into()) }
    }

    fn windows_titled(title: &str, must_visible: bool) -> Vec<HWND> {
        struct Ctx<'a> {
            title: &'a str,
            must_visible: bool,
            out: Vec<HWND>,
        }
        unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> BOOL {
            let c = &mut *(l.0 as *mut Ctx);
            let mut buf = [0u16; 64];
            let n = GetWindowTextW(h, &mut buf) as usize;
            if String::from_utf16_lossy(&buf[..n]) == c.title && (!c.must_visible || IsWindowVisible(h).as_bool()) && is_thunder(h) {
                c.out.push(h);
            }
            BOOL(1)
        }
        let mut ctx = Ctx { title, must_visible, out: Vec::new() };
        unsafe {
            let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut _ as isize));
        }
        ctx.out
    }

    /// 页面内容挂在子窗口 Chrome_RenderWidgetHostHWND 上，第一次取会让它打开无障碍树
    fn roots_of(top: HWND) -> Vec<IAccessible> {
        unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> BOOL {
            (*(l.0 as *mut Vec<HWND>)).push(h);
            BOOL(1)
        }
        let mut kids: Vec<HWND> = Vec::new();
        unsafe {
            let _ = EnumChildWindows(Some(top), Some(cb), LPARAM(&mut kids as *mut _ as isize));
        }
        kids.into_iter()
            .filter_map(|h| unsafe {
                let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
                AccessibleObjectFromWindow(h, OBJID_CLIENT.0 as u32, &IAccessible::IID, &mut p).ok()?;
                (!p.is_null()).then(|| IAccessible::from_raw(p))
            })
            .collect()
    }

    /// 深度优先走树，visit 返回 true 就停。节点数封顶，迅雷任务再多也不至于走太久
    fn walk(a: &IAccessible, depth: u32, budget: &mut u32, visit: &mut dyn FnMut(&IAccessible, &str, i32) -> bool) -> bool {
        if depth > 40 || *budget == 0 {
            return false;
        }
        *budget -= 1;
        unsafe {
            let me = VARIANT::from(0i32);
            let name = a.get_accName(&me).map(|b| b.to_string()).unwrap_or_default();
            let role = a.get_accRole(&me).ok().and_then(|v| i32::try_from(&v).ok()).unwrap_or(0);
            if visit(a, &name, role) {
                return true;
            }
            let n = a.accChildCount().unwrap_or(0);
            if n <= 0 {
                return false;
            }
            let mut kids = vec![VARIANT::default(); n as usize];
            let mut got = 0;
            if AccessibleChildren(a, 0, &mut kids, &mut got).is_err() {
                return false;
            }
            for k in kids.iter().take(got as usize) {
                if k.vt() != VT_DISPATCH {
                    continue;
                }
                let Some(d) = (*k.Anonymous.Anonymous).Anonymous.pdispVal.as_ref() else { continue };
                let Ok(child) = d.cast::<IAccessible>() else { continue };
                if walk(&child, depth + 1, budget, visit) {
                    return true;
                }
            }
        }
        false
    }

    /// 在「新建任务」窗口按「立即下载」（无障碍接口的默认动作，不动鼠标键盘）。
    /// 名字知道时要对得上，别按到用户自己在迅雷里开的窗口
    pub fn press_download(name: &str) -> bool {
        let _com = Com::enter();
        for top in windows_titled("新建任务", true) {
            for root in roots_of(top) {
                let mut matched = name.is_empty();
                let mut pressed = false;
                let mut budget = 3000;
                walk(&root, 0, &mut budget, &mut |a, text, role| {
                    if text == name {
                        matched = true;
                    }
                    if matched && role == ROLE_BUTTON && text == "立即下载" {
                        pressed = unsafe { a.accDoDefaultAction(&VARIANT::from(0i32)).is_ok() };
                        return true;
                    }
                    false
                });
                if pressed {
                    break;
                }
            }
            // 2026-11-07 实测：迅雷 11-06 更新（25.1.44）后，新建窗口不认无障碍的「默认动作」
            // （回成功但窗口不关，表现就是「卡在新建」）。再往按钮中心发一次点击消息：
            // 不动真实鼠标，窗口在屏幕外也收得到，实测任务能加进迅雷
            for hwnd in child_windows(top) {
                let Some(root) = root_of(hwnd) else { continue };
                let mut spot = None;
                let mut matched = name.is_empty();
                let mut budget = 3000;
                walk(&root, 0, &mut budget, &mut |a, text, role| {
                    if text == name {
                        matched = true;
                    }
                    if matched && role == ROLE_BUTTON && text == "立即下载" {
                        let (mut l, mut t, mut w, mut h) = (0, 0, 0, 0);
                        if unsafe { a.accLocation(&mut l, &mut t, &mut w, &mut h, &VARIANT::from(0i32)) }.is_ok() && w > 0 && h > 0 {
                            spot = Some((l + w / 2, t + h / 2));
                        }
                        return true;
                    }
                    false
                });
                if let Some((x, y)) = spot {
                    post_click(hwnd, x, y);
                    return true;
                }
            }
        }
        false
    }

    fn is_topmost(h: HWND) -> bool {
        unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0 }
    }

    /// 迅雷主窗口上面最近的可见普通窗口。迅雷本来就在最上面时返回 None
    fn window_above(main: HWND) -> Option<isize> {
        unsafe {
            let mut h = GetWindow(main, GW_HWNDPREV).ok()?;
            while !h.is_invalid() {
                if IsWindowVisible(h).as_bool() && !is_topmost(h) && !is_thunder(h) {
                    return Some(h.0 as isize);
                }
                h = GetWindow(h, GW_HWNDPREV).ok()?;
            }
            None
        }
    }

    /// main 现在是不是在 other 上面
    fn is_above(main: HWND, other: HWND) -> bool {
        unsafe {
            let mut h = GetWindow(main, GW_HWNDNEXT).ok();
            while let Some(x) = h {
                if x.is_invalid() {
                    return false;
                }
                if x == other {
                    return true;
                }
                h = GetWindow(x, GW_HWNDNEXT).ok();
            }
            false
        }
    }

    /// 交接前的样子：前台是谁、迅雷主窗口是不是藏着或最小化。COM 交接时迅雷会把自己弹到最前
    pub struct Scene {
        foreground: isize,
        main_visible: bool,
        main_minimized: bool,
        dialog_open: bool,
        /// 交接前压在迅雷主窗口上面最近的普通窗口，用来还原次序
        above: Option<isize>,
    }

    impl Scene {
        pub fn capture() -> Self {
            unsafe {
                let main = windows_titled("迅雷", false).first().copied();
                Scene {
                    foreground: GetForegroundWindow().0 as isize,
                    main_visible: main.map_or(false, |h| IsWindowVisible(h).as_bool()),
                    main_minimized: main.map_or(false, |h| IsIconic(h).as_bool()),
                    dialog_open: !windows_titled("新建任务", true).is_empty(),
                    above: main.and_then(window_above),
                }
            }
        }

        /// 迅雷抢了前台就还回去，主窗口恢复交接前的样子。迅雷本来就在前台时不动
        fn restore(&self) {
            unsafe {
                let before = HWND(self.foreground as *mut _);
                if is_thunder(before) {
                    return;
                }
                if let Some(&main) = windows_titled("迅雷", false).first() {
                    if !self.main_visible {
                        let _ = ShowWindow(main, SW_HIDE);
                    } else if self.main_minimized {
                        let _ = ShowWindow(main, SW_SHOWMINNOACTIVE);
                    }
                    // 别用 SetWindowPos 把主窗口插到 before 后面：before 是置顶窗口时迅雷也会变成
                    // 置顶，用户点别的窗口都盖不住它（2026-11-05 踩过）。靠下面把前台还回去
                }
                if is_thunder(GetForegroundWindow()) {
                    let _ = SetForegroundWindow(before);
                }
                // 迅雷抢前台时被提到最上面，还原到交接前的次序，不只是压在闪电下面。
                // 只插到普通窗口后面：插到置顶窗口后面它也会变成置顶（2026-11-05 踩过）
                if let (Some(above), Some(&main), true) = (self.above, windows_titled("迅雷", false).first(), self.main_visible && !self.main_minimized) {
                    let above = HWND(above as *mut _);
                    if IsWindow(Some(above)).as_bool() && !is_topmost(above) && is_above(main, above) {
                        let _ = SetWindowPos(main, Some(above), 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOMOVE);
                    }
                }
            }
        }
    }

    /// 迅雷的确认窗口和「已开始静默下载」提示一出现就挪到屏幕外，并替用户按「立即下载」。
    /// 磁力解析元数据时按钮可能还按不动，所以一直按到窗口关掉；30 秒还关不掉才放回屏幕让用户处理。
    /// 迅雷没有不弹窗的公开开关，窗口可能闪一下，这是能做到的最接近静默的办法
    /// 只有前台程序（用户刚点了闪电的菜单）才锁得上；后台接力时锁不上，靠后面把前台还回去
    pub fn lock_foreground(lock: bool) {
        use windows::Win32::UI::WindowsAndMessaging::{LockSetForegroundWindow, LSFW_LOCK, LSFW_UNLOCK};
        unsafe {
            let _ = LockSetForegroundWindow(if lock { LSFW_LOCK } else { LSFW_UNLOCK });
        }
    }

    /// 交接结果：迅雷收下了 / 用户在迅雷里取消了新建 / 时间到了新建窗口还开着（交给用户自己点）
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Confirm {
        Started,
        Cancelled,
        Stuck,
        Duplicate,
    }

    pub fn quiet_confirm(name: &str, target: &str, select: &(Vec<String>, Vec<String>), limit: u64, scene: Scene) -> Confirm {
        const OFF: i32 = -20000;
        // 交接前就开着的窗口是用户自己的：名字对得上才按。之后出现的就是这次交接弹的，
        // 不看名字（没解析完时窗口里还没有真名，比对会一直按不下去）
        let hint = if scene.dialog_open { name } else { "" };
        let mapped_path = mapped_download_path(target);
        let input_path = mapped_path.as_deref().unwrap_or(target);
        let mut path_attempt = None;
        let mut committed = false;
        let mut user_clicked = false;
        let started = std::time::Instant::now();
        let mut moved: Vec<(isize, i32, i32)> = Vec::new();
        let mut last_press: Option<std::time::Instant> = None;
        let mut closed_at: Option<std::time::Instant> = None;
        let mut seen_open = false;
        let mut cancelled = false;
        let mut duplicate = false;
        let mut first_press: Option<std::time::Instant> = None;
        let hide = |title: &str, moved: &mut Vec<(isize, i32, i32)>| unsafe {
            for h in windows_titled(title, true) {
                if moved.iter().any(|m| m.0 == h.0 as isize) {
                    continue;
                }
                let mut r = RECT::default();
                if GetWindowRect(h, &mut r).is_ok() && r.left > OFF / 2 {
                    moved.push((h.0 as isize, r.left, r.top));
                    let _ = SetWindowPos(h, None, OFF, OFF, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER);
                }
            }
        };
        while started.elapsed().as_secs() < limit {
            hide("新建任务", &mut moved);
            hide(" 已开始静默下载 ", &mut moved);
            // 保存位置有同名文件时迅雷会问怎么办，倒计时结束自己走默认操作（文件夹加编号），不覆盖东西
            hide("发现重名文件", &mut moved);
            // 走到这里用户已经同意了，或是自动接力：用迅雷的默认操作（加编号），不用等它倒计时
            press_rename_number();
            // Preserve the existing task and leave the provider's prompt visible.
            if duplicate_task_open() {
                duplicate = true;
                break;
            }
            // 用户按了鼠标就不再抢前台：TA 可能正要点迅雷，一直抢回来就怎么也点不到（2026-11-05 踩过）
            // 只看「此刻按着」，并跳过开头 1 秒：右键菜单那一下点击还没松开或刚松开，不能算用户要点迅雷
            // （2026-11-05 踩过：把那一下算进去，迅雷就一直留在最前面）
            if started.elapsed().as_millis() > 1000 {
                user_clicked |= unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x01) } < 0;
            }
            if !user_clicked {
                scene.restore();
            }
            let open = !windows_titled("新建任务", true).is_empty();
            // 新建窗口出现过、我们还没按下载它就没了，也没看到「已开始静默下载」：是用户在迅雷里取消了
            if open {
                seen_open = true;
            } else if seen_open && last_press.is_none() && !toast_seen(&moved) && windows_titled("发现重名文件", true).is_empty() {
                cancelled = true;
                break;
            }
            // 路径读回来是目标、且改完过了 1 秒才按：太快按迅雷还用旧路径（实测落到了默认位置）。
            // 改不成就不按，时间到了窗口放回屏幕，绝不存到别处
            if open && selection_ready(select) && path_ready(target, input_path, &mut path_attempt) && last_press.map_or(true, |t| t.elapsed().as_millis() >= 500) {
                if !committed {
                    commit_path();
                    committed = true;
                    if !path_ready(target, input_path, &mut path_attempt) {
                        committed = false;
                        continue;
                    }
                }
                if press_download(hint) {
                    last_press = Some(std::time::Instant::now());
                    first_press = first_press.or(last_press);
                }
            }
            // 按了 3 秒窗口还在：迅雷没认这一按，别再耗着，交给用户
            if open && first_press.is_some_and(|t| t.elapsed().as_secs() >= 3) {
                break;
            }
            // 迅雷没弹窗、直接静默开始了：文件在默认位置，下完由 move_to 处理
            if !open && last_press.is_none() && toast_seen(&moved) {
                closed_at = closed_at.or(Some(std::time::Instant::now()));
            }
            if open || !windows_titled("发现重名文件", true).is_empty() {
                closed_at = None;
            } else if last_press.is_some() {
                closed_at = closed_at.or(Some(std::time::Instant::now()));
            }
            if closed_at.map_or(false, |t| t.elapsed().as_millis() > 1500) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        // 卡在新建窗口时记下是哪一步没过，好查原因（用户反馈「总卡在新建那里」）
        if last_press.is_none() && !windows_titled("新建任务", false).is_empty() {
            crate::mark(&format!(
                "迅雷新建窗口没按下去：勾选就绪={} 路径就绪={} 改过路径={}",
                selection_ready(select),
                path_ready(target, input_path, &mut path_attempt),
                path_attempt.is_some()
            ));
        }
        // 窗口已被迅雷关掉后放回原位，下次用户自己新建任务时位置是对的；没关掉就是交给用户了
        std::thread::sleep(std::time::Duration::from_millis(300));
        put_back(&mut moved);
        lock_foreground(false);
        let stuck = !cancelled && !windows_titled("新建任务", false).is_empty();
        if duplicate {
            for h in windows_titled("重复任务", true) {
                unsafe { let _ = SetForegroundWindow(h); }
            }
        } else if stuck {
            // 交给用户：新建窗口放回屏幕并提到最前，不再把前台抢回去（以前要等好一会儿才点得到）
            for h in windows_titled("新建任务", false) {
                unsafe {
                    let _ = SetForegroundWindow(h);
                }
            }
        } else if !user_clicked {
            scene.restore();
        }
        if duplicate {
            Confirm::Duplicate
        } else if cancelled {
            Confirm::Cancelled
        } else if stuck {
            Confirm::Stuck
        } else {
            Confirm::Started
        }
    }

    fn toast_seen(moved: &[(isize, i32, i32)]) -> bool {
        windows_titled(" 已开始静默下载 ", true).iter().any(|h| moved.iter().any(|m| m.0 == h.0 as isize))
    }

    fn same_path(a: &str, b: &str) -> bool {
        let mut a = std::path::Path::new(a).components();
        let mut b = std::path::Path::new(b).components();
        loop {
            match (a.next(), b.next()) {
                (None, None) => return true,
                (Some(left), Some(right)) if left.as_os_str().eq_ignore_ascii_case(right.as_os_str()) => {}
                _ => return false,
            }
        }
    }

    /// 迅雷优先用本机已有的网络盘符，实际保存位置仍是原 NAS 共享。
    fn mapped_download_path(target: &str) -> Option<String> {
        use std::path::{Component, Path, Prefix};
        use windows::Win32::Foundation::ERROR_SUCCESS;
        use windows::Win32::NetworkManagement::WNet::WNetGetConnectionW;
        use windows::core::PWSTR;

        let Some(Component::Prefix(prefix)) = Path::new(target).components().next() else { return None };
        if !matches!(prefix.kind(), Prefix::UNC(..)) {
            return None;
        }
        let mut longest = 0;
        let mut mapped = None;
        for letter in b'A'..=b'Z' {
            let drive = [u16::from(letter), u16::from(b':'), 0];
            let mut remote = [0u16; 512];
            let mut size = remote.len() as u32;
            let status = unsafe { WNetGetConnectionW(PCWSTR(drive.as_ptr()), Some(PWSTR(remote.as_mut_ptr())), &mut size) };
            if status != ERROR_SUCCESS {
                continue;
            }
            let end = remote.iter().position(|&c| c == 0).unwrap_or(remote.len());
            let remote = String::from_utf16_lossy(&remote[..end]);
            let remote = remote.trim_end_matches('\\');
            let Some(base) = target.get(..remote.len()) else { continue };
            let rest = &target[remote.len()..];
            if remote.len() > longest && base.eq_ignore_ascii_case(remote)
                && (rest.is_empty() || rest.starts_with('\\'))
            {
                longest = remote.len();
                mapped = Some(format!("{}:\\{}", char::from(letter), rest.trim_start_matches('\\')));
            }
        }
        mapped
    }

    #[test]
    fn download_path_requires_the_same_absolute_directory() {
        assert!(same_path(r"Y:\", "y:/"));
        assert!(same_path(r"Y:\电影\", r"y:\电影"));
        assert!(same_path(r"\\NAS.Example\video\电影", r"\\nas.example\Video\电影\"));
        assert!(!same_path(r"Y:\", "Y:"));
        assert!(!same_path(r"\\nas.example\video", r"\nas.example\video"));
        assert!(!same_path(r"Y:\电影", r"Y:\下载"));
    }

    /// 读回下载路径后才确认。没有盘符的共享先填写原路径，旧版迅雷需要额外转义时再重试。
    fn path_ready(target: &str, input_path: &str, attempt: &mut Option<(std::time::Instant, bool)>) -> bool {
        if target.is_empty() {
            return true;
        }
        let _com = Com::enter();
        for top in windows_titled("新建任务", true) {
            for root in roots_of(top) {
                let mut result = None;
                let mut budget = 3000;
                walk(&root, 0, &mut budget, &mut |a, text, role| {
                    if role != ROLE_EDIT || !text.ends_with("路径") || text.contains("云盘") {
                        return false;
                    }
                    let me = VARIANT::from(0i32);
                    let now = unsafe { a.get_accValue(&me) }.map(|b| b.to_string()).unwrap_or_default();
                    if same_path(&now, target) || same_path(&now, input_path) {
                        result = Some(attempt.map_or(true, |(at, _)| at.elapsed().as_millis() >= 1000));
                    } else {
                        if attempt.map_or(true, |(at, _)| at.elapsed().as_millis() >= 1000) {
                            let escaped = input_path.starts_with("\\\\") && attempt.is_some_and(|(_, escaped)| !escaped);
                            let wanted = if escaped { BSTR::from(format!("\\\\{input_path}")) } else { BSTR::from(input_path) };
                            let _ = unsafe { a.put_accValue(&me, &wanted) };
                            *attempt = Some((std::time::Instant::now(), escaped));
                        }
                        result = Some(false);
                    }
                    true
                });
                if let Some(r) = result {
                    return r;
                }
            }
        }
        false
    }

    fn press_rename_number() {
        for top in windows_titled("发现重名文件", true) {
            for root in roots_of(top) {
                let mut budget = 3000;
                walk(&root, 0, &mut budget, &mut |a, text, role| {
                    if role == ROLE_BUTTON && text.contains("加编号") {
                        let _ = unsafe { a.accDoDefaultAction(&VARIANT::from(0i32)) };
                        return true;
                    }
                    false
                });
            }
        }
    }

    /// 把闪电里的文件勾选同步到确认窗口，以窗口上「已选 a/b 个文件」对上数才算好。
    /// 列表很长时迅雷只渲染看得见的几行，够不到的勾不掉，数对不上就不按，时间到了窗口放回屏幕让用户自己勾
    fn selection_ready(select: &(Vec<String>, Vec<String>)) -> bool {
        let (wanted, all) = select;
        if wanted.is_empty() {
            return true;
        }
        const CHECKED: i32 = 0x10;
        let _com = Com::enter();
        for top in windows_titled("新建任务", true) {
            for root in roots_of(top) {
                let mut pending: Option<IAccessible> = None;
                let mut shown: Option<usize> = None;
                let mut budget = 3000;
                walk(&root, 0, &mut budget, &mut |a, text, role| {
                    if role == ROLE_CHECK {
                        pending = text.is_empty().then(|| a.clone());
                    } else if role == ROLE_TEXT && !text.trim().is_empty() {
                        if shown.is_none() {
                            shown = super::selected_count(text);
                        }
                        if let Some(c) = pending.take() {
                            if all.iter().any(|n| n == text) {
                                let me = VARIANT::from(0i32);
                                let on = unsafe { c.get_accState(&me) }.ok().and_then(|v| i32::try_from(&v).ok()).unwrap_or(0) & CHECKED != 0;
                                if on != wanted.iter().any(|n| n == text) {
                                    let _ = unsafe { c.accDoDefaultAction(&me) };
                                }
                            }
                        }
                    }
                    false
                });
                if shown.is_some() {
                    return shown == Some(wanted.len());
                }
            }
        }
        false
    }

    /// 路径要等输入框失去焦点才真正提交（人点按钮时自然会发生，2026-11-05 实测：
    /// 不做这一步，界面已显示新路径，任务还是落到旧目录）。焦点先给输入框，再给「立即下载」
    fn commit_path() {
        let _com = Com::enter();
        const TAKEFOCUS: i32 = 1;
        for (edit, wait) in [(true, 300), (false, 500)] {
            for top in windows_titled("新建任务", true) {
                for root in roots_of(top) {
                    let mut budget = 3000;
                    walk(&root, 0, &mut budget, &mut |a, text, role| {
                        let hit = if edit { role == ROLE_EDIT && text.ends_with("路径") && !text.contains("云盘") } else { role == ROLE_BUTTON && text == "立即下载" };
                        if hit {
                            let _ = unsafe { a.accSelect(TAKEFOCUS, &VARIANT::from(0i32)) };
                        }
                        hit
                    });
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(wait));
        }
    }

    fn put_back(moved: &mut Vec<(isize, i32, i32)>) {
        for (h, x, y) in moved.drain(..) {
            unsafe {
                let _ = SetWindowPos(HWND(h as *mut _), None, x, y, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER);
            }
        }
    }

    /// 删除迅雷里的一条任务。任务行上没有删除按钮，只能「点选这一行 → 顶部删除 → 确认」。
    /// 顶部删除作用于选中项，所以每步都要确认，否则宁可不删：
    /// 无障碍树里有已离开列表的残留行，名字对得上也可能不在屏幕上，要用命中测试确认那个位置真是这一行；
    /// 同名多行分不清不删；确认窗口写的必须是单条的「确定删除该下载任务」。
    /// 点选用发给窗口的消息，不动真实鼠标。迅雷没有公开的任务 id，只能这样认行
    pub fn delete_row(name: &str) -> bool {
        let _com = Com::enter();
        for top in windows_titled("迅雷", false) {
            for hwnd in child_windows(top) {
                let Some(root) = root_of(hwnd) else { continue };
                if click_row(hwnd, &root, name) || (switch_tab(hwnd, &root, "已完成") && click_row(hwnd, &root, name)) {
                    let ok = confirm_delete(&root);
                    let _ = switch_tab(hwnd, &root, "下载中");
                    return ok;
                }
                let _ = switch_tab(hwnd, &root, "下载中");
            }
        }
        false
    }

    fn post_click(hwnd: HWND, sx: i32, sy: i32) {
        use windows::Win32::Foundation::{POINT, WPARAM};
        use windows::Win32::Graphics::Gdi::ScreenToClient;
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};
        let mut p = POINT { x: sx, y: sy };
        unsafe {
            let _ = ScreenToClient(hwnd, &mut p);
            let l = LPARAM(((p.y as isize) << 16) | (p.x as isize & 0xffff));
            let _ = PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(0), l);
            std::thread::sleep(std::time::Duration::from_millis(150));
            let _ = PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(1), l);
            let _ = PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(0), l);
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    /// 文字真的显示在屏幕上：那个位置命中的就是它自己。返回可点击的屏幕坐标
    fn visible_at(root: &IAccessible, a: &IAccessible, text: &str) -> Option<(i32, i32)> {
        let me = VARIANT::from(0i32);
        let (mut l, mut t, mut w, mut h) = (0, 0, 0, 0);
        unsafe {
            a.accLocation(&mut l, &mut t, &mut w, &mut h, &me).ok()?;
            if w <= 0 || h <= 0 {
                return None;
            }
            let (x, y) = (l + w.min(40) / 2, t + h / 2);
            let hit = root.accHitTest(x, y).ok()?;
            if hit.vt() != VT_DISPATCH {
                return None;
            }
            let d = (*hit.Anonymous.Anonymous).Anonymous.pdispVal.as_ref()?;
            let got = d.cast::<IAccessible>().ok()?;
            (got.get_accName(&me).ok()?.to_string() == text).then_some((x, y))
        }
    }

    fn visible_points(root: &IAccessible, text: &str) -> Vec<(i32, i32)> {
        let mut found = Vec::new();
        let mut budget = 3000;
        walk(root, 0, &mut budget, &mut |a, t, role| {
            if role == ROLE_TEXT && t == text {
                if let Some(p) = visible_at(root, a, text) {
                    found.push(p);
                }
            }
            false
        });
        found
    }

    fn click_row(hwnd: HWND, root: &IAccessible, name: &str) -> bool {
        let points = visible_points(root, name);
        if points.len() != 1 {
            return false;
        }
        post_click(hwnd, points[0].0, points[0].1);
        true
    }

    fn switch_tab(hwnd: HWND, root: &IAccessible, tab: &str) -> bool {
        let Some(&(x, y)) = visible_points(root, tab).first() else { return false };
        post_click(hwnd, x, y);
        true
    }

    fn press_named(root: &IAccessible, label: &str) -> bool {
        let mut pressed = false;
        let mut budget = 3000;
        walk(root, 0, &mut budget, &mut |a, text, role| {
            if role == ROLE_BUTTON && text == label {
                pressed = unsafe { a.accDoDefaultAction(&VARIANT::from(0i32)).is_ok() };
                return true;
            }
            false
        });
        pressed
    }

    fn confirm_delete(root: &IAccessible) -> bool {
        if !press_named(root, "删除") {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(700));
        if visible_points(root, "确定删除该下载任务").is_empty() {
            return false; // 不是单条删除的确认窗口，不按
        }
        let ok = press_named(root, "确定删除");
        std::thread::sleep(std::time::Duration::from_millis(500));
        ok
    }

    fn child_windows(top: HWND) -> Vec<HWND> {
        unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> BOOL {
            (*(l.0 as *mut Vec<HWND>)).push(h);
            BOOL(1)
        }
        let mut kids: Vec<HWND> = Vec::new();
        unsafe {
            let _ = EnumChildWindows(Some(top), Some(cb), LPARAM(&mut kids as *mut _ as isize));
        }
        kids
    }

    fn root_of(h: HWND) -> Option<IAccessible> {
        unsafe {
            let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
            AccessibleObjectFromWindow(h, OBJID_CLIENT.0 as u32, &IAccessible::IID, &mut p).ok()?;
            (!p.is_null()).then(|| IAccessible::from_raw(p))
        }
    }

    /// 按任务行右侧的开始/暂停按钮。行内顺序是「删除、开始/暂停」，只取最后一个带图标的按钮，
    /// 绝不能按到前面的删除；顶部工具栏的按钮在任务名之前，不会算进来
    /// 任务行上的暂停/继续按钮（这一行里最后一个带图标的按钮）
    fn row_toggle_button(root: &IAccessible, name: &str) -> Option<IAccessible> {
        let mut in_row = false;
        let mut seen_status = false;
        let mut last: Option<IAccessible> = None;
        let mut budget = 3000;
        walk(root, 0, &mut budget, &mut |a, text, role| {
            if !in_row {
                in_row = role == ROLE_TEXT && text == name;
                return false;
            }
            if role == ROLE_TEXT && text.starts_with("已下载") {
                if seen_status {
                    return true; // 下一行了
                }
                seen_status = true;
            }
            if text == "file icon" {
                return true;
            }
            if role == ROLE_BUTTON && unsafe { a.accChildCount().unwrap_or(0) } > 0 {
                last = Some(a.clone());
            }
            false
        });
        last
    }

    #[cfg(test)]
    pub fn trace_tops(name: &str) {
        let _com = Com::enter();
        for top in windows_titled("迅雷", false) {
            let mut r = windows::Win32::Foundation::RECT::default();
            unsafe {
                let _ = GetWindowRect(top, &mut r);
                println!("top {:?} visible={} iconic={} rect={:?}", top.0, IsWindowVisible(top).as_bool(), IsIconic(top).as_bool(), (r.left, r.top, r.right, r.bottom));
            }
            for hwnd in child_windows(top) {
                let Some(root) = root_of(hwnd) else { continue };
                let Some(b) = row_toggle_button(&root, name) else { continue };
                let (mut l, mut t, mut w, mut h) = (0, 0, 0, 0);
                let _ = unsafe { b.accLocation(&mut l, &mut t, &mut w, &mut h, &VARIANT::from(0i32)) };
                println!("  child {:?} button {l},{t} {w}x{h}", hwnd.0);
            }
        }
    }
    /// 迅雷窗口藏着时无障碍树不刷新：临时（不激活、沉底）显示出来再读
    #[cfg(test)]
    pub fn leaves_shown() -> Vec<String> {
        let _com = Com::enter();
        let mut guards = Vec::new();
        for top in windows_titled("迅雷", false) {
            guards.push(OffscreenShow::new(top));
        }
        std::thread::sleep(std::time::Duration::from_millis(1500));
        main_leaves()
    }
    /// 实验：藏着的主窗口用不同方式显示后点一次。mode: top / bottom / action
    #[cfg(test)]
    pub fn experiment(name: &str, mode: &str) {
        use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, HWND_BOTTOM};
        let _com = Com::enter();
        for top in windows_titled("迅雷", false) {
            let found = child_windows(top).into_iter().any(|h| root_of(h).and_then(|r| row_toggle_button(&r, name)).is_some());
            if !found { continue; }
            let was_hidden = !unsafe { IsWindowVisible(top) }.as_bool();
            unsafe { let _ = ShowWindow(top, SW_SHOWNOACTIVATE); }
            if mode == "bottom" {
                unsafe { let _ = SetWindowPos(top, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOMOVE); }
            }
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let row = || { let v = main_leaves(); let i = v.iter().position(|x| x == name).unwrap_or(0); v[i.saturating_sub(4)..(i + 8).min(v.len())].join(" | ") };
            println!("前 {}", row());
            for hwnd in child_windows(top) {
                let Some(root) = root_of(hwnd) else { continue };
                let Some(b) = row_toggle_button(&root, name) else { continue };
                if mode == "action" {
                    println!("action={:?}", unsafe { b.accDoDefaultAction(&VARIANT::from(0i32)) });
                } else {
                    let (mut l, mut t, mut w, mut h) = (0, 0, 0, 0);
                    let _ = unsafe { b.accLocation(&mut l, &mut t, &mut w, &mut h, &VARIANT::from(0i32)) };
                    println!("click at {l},{t} {w}x{h}");
                    post_click(hwnd, l + w / 2, t + h / 2);
                }
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2500));
            println!("显示中读 paused={:?}", super::parse_row(&main_leaves(), name).map(|r| r.paused));
            println!("后 {}", row());
            if was_hidden { unsafe { let _ = ShowWindow(top, SW_HIDE); } }
            return;
        }
        println!("没找到这一行");
    }
    #[cfg(test)]
    pub fn main_count() -> usize {
        windows_titled("迅雷", false).len()
    }
    #[cfg(test)]
    pub fn close_main_for_test() {
        for h in windows_titled("迅雷", false) {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(Some(h), windows::Win32::UI::WindowsAndMessaging::WM_CLOSE, windows::Win32::Foundation::WPARAM(0), LPARAM(0));
            }
        }
    }

    pub fn press_row_toggle(name: &str) -> bool {
        let _com = Com::enter();
        for top in windows_titled("迅雷", false) {
            for root in roots_of(top) {
                if let Some(b) = row_toggle_button(&root, name) {
                    return unsafe { b.accDoDefaultAction(&VARIANT::from(0i32)).is_ok() };
                }
            }
        }
        false
    }

    /// 迅雷缩到托盘时会把主窗口整个销毁（2026-11-12 实测：一个「迅雷」窗口都没有），按不到任何按钮。
    /// 这时再启动一次迅雷（单实例，只会把主窗口叫出来），窗口一出现就挪到屏幕外；
    /// 返回的守卫离开作用域时关掉这个窗口，迅雷回到托盘
    pub struct Summoned(Option<HWND>);
    impl Drop for Summoned {
        fn drop(&mut self) {
            if let Some(h) = self.0 {
                std::thread::sleep(std::time::Duration::from_millis(300));
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(Some(h), windows::Win32::UI::WindowsAndMessaging::WM_CLOSE, windows::Win32::Foundation::WPARAM(0), LPARAM(0));
                }
            }
        }
    }
    pub fn summon_main() -> Summoned {
        summon_main_settle(1500)
    }
    /// settle：窗口出现后固定等多久。自己轮询这一行的调用方（set_paused）给 0
    pub fn summon_main_settle(settle: u64) -> Summoned {
        // 缩到托盘时窗口可能还在、只是藏着；从外面 ShowWindow 它不会真的画出来、点击也不认（11-13 截图实测），
        // 要让迅雷自己把窗口叫出来
        if !windows_titled("迅雷", true).is_empty() {
            return Summoned(None);
        }
        let Some(dir) = super::install_dir() else { return Summoned(None) };
        let exe = HSTRING::from(dir.join("Program").join("Thunder.exe").to_string_lossy().as_ref());
        let prev = unsafe { GetForegroundWindow() };
        unsafe {
            // SW_SHOWMINNOACTIVE：就算迅雷照这个参数显示，也是最小化、不抢焦点
            let _ = windows::Win32::UI::Shell::ShellExecuteW(None, windows::core::w!("open"), &exe, None, None, windows::Win32::UI::WindowsAndMessaging::SW_SHOWMINNOACTIVE);
        }
        let started = std::time::Instant::now();
        while started.elapsed().as_secs() < 6 {
            if let Some(h) = windows_titled("迅雷", true).into_iter().next() {
                // 不能挪到屏幕外：迅雷不处理屏幕外窗口的点击（11-12、11-13 实测，关窗后按不动就是这个原因）。
                // 留在原位，最小化或藏着的由 OffscreenShow 临时正常显示，按完这里再关回托盘。
                // 沉到所有窗口最底下、焦点还给用户：压在后面的迅雷照样能点中（11-13 用户实测）
                send_to_back(h);
                unsafe {
                    if !prev.is_invalid() {
                        let _ = SetForegroundWindow(prev);
                    }
                }
                // 等页面画好、任务列表加载出来
                std::thread::sleep(std::time::Duration::from_millis(settle));
                #[cfg(test)]
                println!("  summon: 叫出 {:?} 用时 {:?}", h.0, started.elapsed());
                return Summoned(Some(h));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        Summoned(None)
    }

    /// 守卫期间迅雷的窗口一显示就压到最底下、想抢焦点就把焦点还回去。
    /// 迅雷缩在托盘时只能让它自己把窗口叫出来，它会画在最前面；原来靠 50ms 轮询再压下去，
    /// 中间会挡一下。改成订阅系统事件（窗口显示、前台切换），几毫秒内就压下去
    pub struct KeepBehind {
        tid: u32,
        join: Option<std::thread::JoinHandle<()>>,
    }
    static KEEP_PREV: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

    unsafe extern "system" fn keep_behind_cb(
        _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
        event: u32,
        hwnd: HWND,
        id_object: i32,
        _child: i32,
        _thread: u32,
        _time: u32,
    ) {
        use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, EVENT_SYSTEM_FOREGROUND, GA_ROOT};
        if id_object != 0 || hwnd.is_invalid() || unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd || !is_thunder(hwnd) {
            return;
        }
        if event == windows::Win32::UI::WindowsAndMessaging::EVENT_OBJECT_SHOW {
            // 刚建出来的窗口第一帧是空白框，压下去之前会盖住用户的窗口（用户看到 App「消失了一下」）：
            // 先完全透明，压到最底下后 120ms 再恢复（透明时迅雷不认点击，所以不能一直透明）
            use windows::Win32::UI::WindowsAndMessaging::{SetLayeredWindowAttributes, SetWindowLongPtrW, LWA_ALPHA, WS_EX_LAYERED};
            let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
            if ex & WS_EX_LAYERED.0 as isize == 0 {
                unsafe {
                    let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as isize);
                    let _ = SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), 0, LWA_ALPHA);
                }
                send_to_back(hwnd);
                let raw = hwnd.0 as isize;
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(120));
                    let h = HWND(raw as *mut _);
                    unsafe {
                        send_to_back(h);
                        let _ = SetWindowLongPtrW(h, GWL_EXSTYLE, ex);
                    }
                });
                return;
            }
        }
        send_to_back(hwnd);
        if event == EVENT_SYSTEM_FOREGROUND {
            let prev = HWND(KEEP_PREV.load(std::sync::atomic::Ordering::Relaxed) as *mut _);
            if !prev.is_invalid() && prev != hwnd {
                unsafe {
                    let _ = SetForegroundWindow(prev);
                }
            }
        }
    }

    pub fn keep_behind() -> KeepBehind {
        use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent};
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, GetMessageW, LockSetForegroundWindow, EVENT_OBJECT_SHOW, EVENT_SYSTEM_FOREGROUND, LSFW_LOCK, MSG, WINEVENT_OUTOFCONTEXT,
            WINEVENT_SKIPOWNPROCESS,
        };
        let prev = unsafe { GetForegroundWindow() };
        KEEP_PREV.store(prev.0 as isize, std::sync::atomic::Ordering::Relaxed);
        // 别的程序（迅雷）在这期间调 SetForegroundWindow 不生效。只有前台程序锁得上，锁不上也无妨
        unsafe {
            let _ = LockSetForegroundWindow(LSFW_LOCK);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let join = std::thread::spawn(move || unsafe {
            let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
            let show = SetWinEventHook(EVENT_OBJECT_SHOW, EVENT_OBJECT_SHOW, None, Some(keep_behind_cb), 0, 0, flags);
            let fg = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(keep_behind_cb), 0, 0, flags);
            let _ = tx.send(windows::Win32::System::Threading::GetCurrentThreadId());
            // 事件要靠这个线程的消息循环送进来
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                DispatchMessageW(&msg);
            }
            let _ = UnhookWinEvent(show);
            let _ = UnhookWinEvent(fg);
        });
        let tid = rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap_or(0);
        KeepBehind { tid, join: Some(join) }
    }
    impl Drop for KeepBehind {
        fn drop(&mut self) {
            use windows::Win32::UI::WindowsAndMessaging::{LockSetForegroundWindow, PostThreadMessageW, LSFW_UNLOCK, WM_QUIT};
            unsafe {
                let _ = LockSetForegroundWindow(LSFW_UNLOCK);
                if self.tid != 0 {
                    let _ = PostThreadMessageW(self.tid, WM_QUIT, windows::Win32::Foundation::WPARAM(0), LPARAM(0));
                }
            }
            if let Some(j) = self.join.take() {
                let _ = j.join();
            }
            // 收尾再查一次：焦点还落在迅雷上就还给原来的窗口
            let prev = HWND(KEEP_PREV.load(std::sync::atomic::Ordering::Relaxed) as *mut _);
            unsafe {
                let fg = GetForegroundWindow();
                if !prev.is_invalid() && IsWindow(Some(prev)).as_bool() && !fg.is_invalid() && fg != prev && is_thunder(fg) {
                    let _ = SetForegroundWindow(prev);
                }
            }
        }
    }

    /// 排查用：把「闪电全能下载」主窗口放到前台（模拟用户在 App 里点按钮）
    #[cfg(test)]
    pub fn focus_app_for_test() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP};
        use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, ShowWindow, SW_RESTORE};
        unsafe {
            if let Ok(h) = FindWindowW(None, windows::core::w!("闪电全能下载")) {
                let _ = ShowWindow(h, SW_RESTORE);
                // 后台进程直接 SetForegroundWindow 会被系统拒：先按一下 Alt 取得前台权限
                keybd_event(0x12, 0, Default::default(), 0);
                keybd_event(0x12, 0, KEYEVENTF_KEYUP, 0);
                let _ = SetForegroundWindow(h);
            }
        }
    }

    /// 排查用：（前台是不是迅雷, 最上层的普通窗口是不是迅雷）
    #[cfg(test)]
    pub fn front_is_thunder() -> (bool, bool) {
        use windows::Win32::UI::WindowsAndMessaging::GetTopWindow;
        unsafe {
            let fg = GetForegroundWindow();
            let fg_t = !fg.is_invalid() && is_thunder(fg);
            let mut h = GetTopWindow(None).unwrap_or_default();
            while !h.is_invalid() {
                let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
                let mut r = RECT::default();
                let _ = GetWindowRect(h, &mut r);
                if IsWindowVisible(h).as_bool() && ex & WS_EX_TOPMOST.0 == 0 && r.right - r.left > 200 && r.bottom - r.top > 200 {
                    return (fg_t, is_thunder(h));
                }
                h = GetWindow(h, GW_HWNDNEXT).unwrap_or_default();
            }
            (fg_t, false)
        }
    }

    /// 放到所有窗口最底下，不激活：被挡住的迅雷照样处理点击消息
    fn send_to_back(h: HWND) {
        unsafe {
            let _ = SetWindowPos(h, Some(windows::Win32::UI::WindowsAndMessaging::HWND_BOTTOM), 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOMOVE);
        }
    }

    /// 迅雷窗口没显示着时，临时在屏幕外显示出来（不激活、用户看不到），离开作用域时恢复原状
    /// 迅雷窗口没显示着（缩在托盘、最小化）时，临时显示出来按按钮，离开作用域时恢复原状。
    /// 不能挪到屏幕外：迅雷的内嵌浏览器把完全在屏幕外的窗口当成被挡住，不处理点击（2026-11-12 实测）。
    /// 所以留在原位、设成完全透明且鼠标穿透再显示：用户看不到也点不到，迅雷照常响应
    struct OffscreenShow {
        hwnd: HWND,
        place: Option<windows::Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT>,
        ex_style: isize,
        was_hidden: bool,
        /// 收回前多留多久（毫秒）：按完不读状态的调用方要给迅雷留处理时间
        linger: u64,
    }
    impl OffscreenShow {
        fn new(hwnd: HWND) -> Self {
            Self::with_waits(hwnd, 700, 400)
        }
        /// settle：显示后固定等多久再返回；linger：收回前多留多久。
        /// 自己轮询按钮位置、核对状态的调用方（toggle_row）两个都给很小的值
        fn with_waits(hwnd: HWND, settle: u64, linger: u64) -> Self {
            use windows::Win32::UI::WindowsAndMessaging::{
                GetWindowPlacement, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPlacement, LWA_ALPHA, SW_SHOWNOACTIVATE, WINDOWPLACEMENT, WS_EX_LAYERED,
                WS_EX_TRANSPARENT,
            };
            let was_hidden = !unsafe { IsWindowVisible(hwnd) }.as_bool();
            let iconic = unsafe { IsIconic(hwnd) }.as_bool();
            if !was_hidden && !iconic {
                return Self { hwnd, place: None, ex_style: 0, was_hidden, linger };
            }
            let mut p = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
            if unsafe { GetWindowPlacement(hwnd, &mut p) }.is_err() {
                return Self { hwnd, place: None, ex_style: 0, was_hidden: false, linger };
            }
            let saved = p;
            let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
            // 实测透明、屏幕外都不行：只好正常显示（不抢焦点），按完马上收回托盘，会闪一下
            let _ = (SetLayeredWindowAttributes, LWA_ALPHA, WS_EX_LAYERED, WS_EX_TRANSPARENT, SetWindowLongPtrW);
            unsafe {
                p.showCmd = SW_SHOWNOACTIVATE.0 as u32;
                let _ = SetWindowPlacement(hwnd, &p);
            }
            send_to_back(hwnd);
            // 等页面画出来、无障碍树里的按钮有了位置
            std::thread::sleep(std::time::Duration::from_millis(settle));
            Self { hwnd, place: Some(saved), ex_style, was_hidden, linger }
        }
    }
    impl Drop for OffscreenShow {
        fn drop(&mut self) {
            use windows::Win32::UI::WindowsAndMessaging::{SetWindowLongPtrW, SetWindowPlacement};
            let Some(p) = self.place else { return };
            std::thread::sleep(std::time::Duration::from_millis(self.linger));
            unsafe {
                if self.was_hidden {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
                let _ = SetWindowPlacement(self.hwnd, &p);
                if self.was_hidden {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
                let _ = SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, self.ex_style);
            }
        }
    }

    /// 排查用：窗口临时显示的同一段时间里，点两次这一行的按钮，每次前后都读这一行
    #[cfg(test)]
    pub fn toggle_trace(name: &str) {
        let _com = Com::enter();
        for top in windows_titled("迅雷", false) {
            let _shown = OffscreenShow::new(top);
            let read = || {
                let mut v = Vec::new();
                for root in roots_of(top) {
                    let mut b = 3000;
                    walk(&root, 0, &mut b, &mut |_, t, _| {
                        if !t.trim().is_empty() {
                            v.push(t.to_string());
                        }
                        false
                    });
                }
                super::parse_row(&v, name).map(|r| (r.paused, r.speed))
            };
            println!("显示后 {:?}", read());
            for i in 0..2 {
                for hwnd in child_windows(top) {
                    let Some(root) = root_of(hwnd) else { continue };
                    let Some(b) = row_toggle_button(&root, name) else { continue };
                    let (mut l, mut t, mut w, mut h) = (0, 0, 0, 0);
                    let _ = unsafe { b.accLocation(&mut l, &mut t, &mut w, &mut h, &VARIANT::from(0i32)) };
                    println!("按钮位置 {l},{t} {w}x{h}");
                    post_click(hwnd, l + w / 2, t + h / 2);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
                println!("第 {} 次后 {:?}", i + 1, read());
            }
        }
    }

    /// 暂停/继续一行，窗口只显示一次：按钮有位置就点，点完每 100ms 读这一行，状态到了立刻收回窗口；
    /// 没到就在同一次显示里补按一次，不用再叫一遍窗口。
    /// 返回 None = 窗口里找不到这一行；Some(是否已确认到位)
    pub fn toggle_row(name: &str, want_paused: bool) -> Option<bool> {
        let _com = Com::enter();
        let read = |top: HWND| -> Option<bool> {
            let mut v = Vec::new();
            for root in roots_of(top) {
                let mut b = 3000;
                walk(&root, 0, &mut b, &mut |_, t, _| {
                    if !t.trim().is_empty() {
                        v.push(t.to_string());
                    }
                    false
                });
            }
            super::parse_row(&v, name).map(|r| r.paused)
        };
        for top in windows_titled("迅雷", false) {
            if !child_windows(top).into_iter().any(|h| root_of(h).and_then(|r| row_toggle_button(&r, name)).is_some()) {
                continue;
            }
            let _shown = OffscreenShow::with_waits(top, 0, 80);
            let started = std::time::Instant::now();
            for attempt in 0..2 {
                // 等这一行的按钮有了位置（刚叫出来的窗口先是空框）
                let mut clicked = false;
                while !clicked && started.elapsed().as_secs() < 8 {
                    for hwnd in child_windows(top) {
                        let Some(root) = root_of(hwnd) else { continue };
                        let Some(b) = row_toggle_button(&root, name) else { continue };
                        let (mut l, mut t, mut w, mut h) = (0, 0, 0, 0);
                        if unsafe { b.accLocation(&mut l, &mut t, &mut w, &mut h, &VARIANT::from(0i32)) }.is_ok() && w > 0 && h > 0 {
                            // 位置刚出来时页面可能还没接上事件，稍等一下再点
                            std::thread::sleep(std::time::Duration::from_millis(if attempt == 0 { 150 } else { 50 }));
                            post_click(hwnd, l + w / 2, t + h / 2);
                            clicked = true;
                            break;
                        }
                    }
                    if !clicked {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
                if !clicked {
                    // 一直没有位置：退回无障碍默认动作
                    let ok = roots_of(top).iter().find_map(|r| row_toggle_button(r, name))
                        .is_some_and(|b| unsafe { b.accDoDefaultAction(&VARIANT::from(0i32)).is_ok() });
                    return ok.then_some(false);
                }
                // 迅雷界面的读数会滞后一点：给 2 秒，到了就收
                let t0 = std::time::Instant::now();
                while t0.elapsed().as_millis() < 2000 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    if read(top) == Some(want_paused) {
                        #[cfg(test)]
                        println!("  toggle_row: 第 {} 次点击后 {:?} 到位，总用时 {:?}", attempt + 1, t0.elapsed(), started.elapsed());
                        return Some(true);
                    }
                }
            }
            return Some(false);
        }
        None
    }

    /// 补救按法：往这一行的暂停/继续按钮发一次点击消息（不动真实鼠标）。
    /// 迅雷 11-06 更新后不一定认上面的「默认动作」。迅雷窗口最小化时按钮没有位置，点不了
    pub fn click_row_toggle(name: &str) -> bool {
        let _com = Com::enter();
        for top in windows_titled("迅雷", false) {
            // 迅雷有两个叫「迅雷」的窗口，另一个是空的：只动列表里有这一行的那个，
            // 否则会把空窗口显示出来再白等 8 秒（11-13 实测）
            if !child_windows(top).into_iter().any(|h| root_of(h).and_then(|r| row_toggle_button(&r, name)).is_some()) {
                continue;
            }
            // 最小化时按钮没有位置：临时正常显示（不抢焦点），按完放回原样
            let _shown = OffscreenShow::new(top);
            // 刚叫出来的窗口先是一个空框，页面画好、这一行的按钮有了位置才能按（11-13 用户看到虚边框一闪就没了）。
            // 最多等 8 秒；按完多留一会儿让迅雷处理完再收回
            let started = std::time::Instant::now();
            while started.elapsed().as_secs() < 8 {
                for hwnd in child_windows(top) {
                    let Some(root) = root_of(hwnd) else { continue };
                    let Some(b) = row_toggle_button(&root, name) else { continue };
                    let (mut l, mut t, mut w, mut h) = (0, 0, 0, 0);
                    let got = unsafe { b.accLocation(&mut l, &mut t, &mut w, &mut h, &VARIANT::from(0i32)) }.is_ok();
                    #[cfg(test)]
                    println!("  [{:?}] top={:?} vis={} child={:?} loc={got} {l},{t} {w}x{h}", started.elapsed(), top.0, unsafe { IsWindowVisible(top) }.as_bool(), hwnd.0);
                    if got && w > 0 && h > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        post_click(hwnd, l + w / 2, t + h / 2);
                        std::thread::sleep(std::time::Duration::from_millis(1200));
                        return true;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
        false
    }


    // ---------- 任务详情面板（点任务行上的「已下载 x/y 项」滑出来） ----------

    /// 面板里一行。size 是迅雷显示的文字（2.2GB），没有精确字节
    #[derive(Debug, Clone, serde::Serialize)]
    pub struct PanelFile {
        pub name: String,
        pub percent: String,
        pub size: String,
        pub checked: bool,
    }

    const STATE_CHECKED: i32 = 0x10;

    fn panel_roots() -> Vec<(HWND, IAccessible)> {
        let mut out = Vec::new();
        for top in windows_titled("迅雷", false) {
            for hwnd in child_windows(top) {
                if let Some(root) = root_of(hwnd) {
                    out.push((hwnd, root));
                }
            }
        }
        out
    }

    fn panel_is_open(root: &IAccessible) -> bool {
        !visible_points(root, "全部文件").is_empty()
    }

    /// 点这一行的「已下载 x/y 项」打开面板，并切到「全部文件」
    fn panel_open(hwnd: HWND, root: &IAccessible, name: &str) -> bool {
        let mut in_row = false;
        let mut point = None;
        let mut budget = 3000;
        walk(root, 0, &mut budget, &mut |a, text, role| {
            if role != ROLE_TEXT {
                return false;
            }
            if text == name {
                in_row = true;
                return false;
            }
            if in_row && text.starts_with("已下载") {
                point = visible_at(root, a, text);
                return true;
            }
            false
        });
        let Some((x, y)) = point else { return false };
        post_click(hwnd, x, y);
        std::thread::sleep(std::time::Duration::from_millis(400));
        if !panel_is_open(root) {
            return false;
        }
        switch_tab(hwnd, root, "全部文件");
        std::thread::sleep(std::time::Duration::from_millis(300));
        true
    }

    /// 读「大小」表头到「全选」之间的行：勾选框、名字、进度、大小。第一行是文件夹本身时去掉
    fn panel_rows(root: &IAccessible) -> Vec<(IAccessible, PanelFile)> {
        let mut rows: Vec<(IAccessible, PanelFile)> = Vec::new();
        let mut started = false;
        let mut budget = 4000;
        walk(root, 0, &mut budget, &mut |a, text, role| {
            if !started {
                started = role == ROLE_TEXT && text == "大小";
                return false;
            }
            if role == ROLE_TEXT && text == "全选" {
                return true;
            }
            if role == ROLE_CHECK {
                let st = unsafe { a.get_accState(&VARIANT::from(0i32)).ok().and_then(|v| i32::try_from(&v).ok()).unwrap_or(0) };
                rows.push((a.clone(), PanelFile { name: String::new(), percent: String::new(), size: String::new(), checked: st & STATE_CHECKED != 0 }));
            } else if role == ROLE_TEXT && !text.is_empty() {
                if let Some((_, f)) = rows.last_mut() {
                    if f.name.is_empty() {
                        f.name = text.to_string();
                    } else if f.percent.is_empty() {
                        f.percent = text.to_string();
                    } else if f.size.is_empty() {
                        f.size = text.to_string();
                    }
                }
            }
            false
        });
        // 最后一个勾选框是「全选」前的那个，它没有名字
        rows.retain(|(_, f)| !f.name.is_empty());
        rows
    }

    /// 文件夹那一行在最前面，名字是种子名，和文件不是一回事
    fn panel_files(rows: Vec<(IAccessible, PanelFile)>, task: &str) -> Vec<(IAccessible, PanelFile)> {
        let mut rows = rows;
        if rows.len() > 1 && rows[0].1.name == task {
            rows.remove(0);
        }
        rows
    }

    /// 关闭按钮没有名字，而且不能按位置找（迅雷标题栏的窗口按钮比它更靠右，点错会把窗口关掉）。
    /// 无障碍树里它是面板标题和「已选文件」之前的最后一个无名按钮
    fn panel_close(root: &IAccessible) {
        let me = VARIANT::from(0i32);
        let mut last: Option<IAccessible> = None;
        let mut found = false;
        let mut budget = 4000;
        walk(root, 0, &mut budget, &mut |a, text, role| {
            if role == ROLE_TEXT && text == "已选文件" {
                found = true;
                return true;
            }
            if role == ROLE_BUTTON && text.trim().is_empty() {
                last = Some(a.clone());
            }
            false
        });
        if let (true, Some(b)) = (found, last) {
            let _ = unsafe { b.accDoDefaultAction(&me) };
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }

    /// 面板标题是不是这条任务：标题紧挨在「已选文件」前面
    fn panel_title_is(root: &IAccessible, name: &str) -> bool {
        let mut last = String::new();
        let mut hit = false;
        let mut budget = 4000;
        walk(root, 0, &mut budget, &mut |_, text, role| {
            if role == ROLE_TEXT {
                if text == "已选文件" {
                    hit = last == name;
                    return true;
                }
                if !text.is_empty() {
                    last = text.to_string();
                }
            }
            false
        });
        hit
    }

    fn panel_ready(hwnd: HWND, root: &IAccessible, name: &str) -> bool {
        if panel_is_open(root) && panel_title_is(root, name) {
            switch_tab(hwnd, root, "全部文件");
            return true;
        }
        panel_open(hwnd, root, name)
    }

    /// 操作迅雷面板时不让它跑到前面：锁住前台，做完把前台和窗口次序还原
    pub fn quietly<R>(f: impl FnOnce() -> R) -> R {
        let scene = Scene::capture();
        lock_foreground(true);
        let r = f();
        lock_foreground(false);
        scene.restore();
        r
    }

    /// 读任务里的文件。面板留着：抽屉开着时会反复读，开关只在第一次和最后一次，迅雷不会一直闪
    pub fn panel_read(name: &str) -> Option<Vec<PanelFile>> {
        let _com = Com::enter();
        for (hwnd, root) in panel_roots() {
            if panel_ready(hwnd, &root, name) {
                return Some(panel_files(panel_rows(&root), name).into_iter().map(|(_, f)| f).collect());
            }
        }
        None
    }

    /// 按 wanted 改勾选（顺序同 panel_read），再按「下载选中的文件」
    pub fn panel_apply(name: &str, wanted: &[bool]) -> Result<(), String> {
        let _com = Com::enter();
        for (hwnd, root) in panel_roots() {
            if !panel_ready(hwnd, &root, name) {
                continue;
            }
            let rows = panel_files(panel_rows(&root), name);
            if rows.len() != wanted.len() {
                return Err("迅雷里的文件列表变了，请重新打开".into());
            }
            let me = VARIANT::from(0i32);
            for ((a, f), want) in rows.iter().zip(wanted) {
                if f.checked != *want {
                    let _ = unsafe { a.accDoDefaultAction(&me) };
                    std::thread::sleep(std::time::Duration::from_millis(120));
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
            let mut pressed = false;
            let mut budget = 4000;
            walk(&root, 0, &mut budget, &mut |a, text, role| {
                if role == ROLE_BUTTON && text.contains("下载选中的文件") {
                    pressed = unsafe { a.accDoDefaultAction(&me) }.is_ok();
                    return true;
                }
                false
            });
            return if pressed { Ok(()) } else { Err("迅雷里没有找到「下载选中的文件」".into()) };
        }
        Err("迅雷窗口里读不到这条任务，请打开迅雷的「下载中」页面后再试".into())
    }

    pub fn panel_dismiss() {
        let _com = Com::enter();
        for (_, root) in panel_roots() {
            if panel_is_open(&root) {
                panel_close(&root);
            }
        }
    }

    pub fn panel_debug(name: &str) {
        let show = |tag: &str| {
            let l = panel_read(name).unwrap_or_default();
            println!("{tag} {:?}", l.iter().map(|f| (f.checked, f.percent.clone(), f.size.clone())).collect::<Vec<_>>());
            l
        };
        let before = show("BEFORE");
        if before.is_empty() {
            return;
        }
        let mut want: Vec<bool> = before.iter().map(|f| f.checked).collect();
        want[0] = !want[0];
        println!("APPLY1 {:?}", panel_apply(name, &want));
        std::thread::sleep(std::time::Duration::from_millis(1500));
        show("AFTER1");
        want[0] = !want[0];
        println!("APPLY2 {:?}", panel_apply(name, &want));
        std::thread::sleep(std::time::Duration::from_millis(1500));
        show("AFTER2");
        panel_dismiss();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let _com = Com::enter();
        println!("OPEN_AFTER_CLOSE {:?}", panel_roots().iter().map(|(_, r)| panel_is_open(r)).collect::<Vec<_>>());
    }

    /// Chromium 的无障碍树默认关着，要有辅助工具对顶层窗口发标准请求（IAccessible2）才打开
    fn wake_accessibility() {
        use windows::Win32::System::Com::IServiceProvider;
        const IID_IACCESSIBLE2: windows::core::GUID = windows::core::GUID::from_u128(0xE89F726E_C4F4_4c19_BB19_B647D7FA8478);
        let _com = Com::enter();
        for top in windows_titled("迅雷", false) {
            for h in std::iter::once(top).chain(child_windows(top)) {
                if let Some(r) = root_of(h) {
                    // 顶层窗口也问一遍，并实际读一下子节点：有的版本要有真实的读取才开
                    let _ = unsafe { r.accChildCount() };
                    if let Ok(sp) = r.cast::<IServiceProvider>() {
                        let mut out: *mut core::ffi::c_void = std::ptr::null_mut();
                        let _ = unsafe { (windows::core::Interface::vtable(&sp).QueryService)(windows::core::Interface::as_raw(&sp), &IAccessible::IID, &IID_IACCESSIBLE2, &mut out) };
                        if !out.is_null() {
                            unsafe { let _ = windows::core::IUnknown::from_raw(out); }
                        }
                    }
                }
            }
        }
    }

    /// 迅雷主窗口上所有文字，按界面顺序。窗口不在或读不到就是空的
    pub fn main_leaves() -> Vec<String> {
        let out = read_leaves();
        if !out.is_empty() || windows_titled("迅雷", false).is_empty() {
            return out;
        }
        // 窗口在但一个字都读不到：迅雷开机自启后没人请求过，Chromium 的无障碍树还关着。
        // 请求一次，下一轮就读得到（2026-11-06 实测：最小化、收到托盘后也照样能读）
        wake_accessibility();
        out
    }

    fn read_leaves() -> Vec<String> {
        let _com = Com::enter();
        let mut out = Vec::new();
        for top in windows_titled("迅雷", false) {
            for root in roots_of(top) {
                let mut budget = 3000;
                walk(&root, 0, &mut budget, &mut |_, text, role| {
                    if role == ROLE_TEXT && !text.trim().is_empty() {
                        out.push(text.to_string());
                    }
                    false
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// 实测迅雷详情面板，会操作迅雷窗口：SHANDIAN_XL_NAME=<任务名> cargo test thunder_panel -- --ignored --nocapture
    #[test]
    #[ignore]
    fn thunder_panel() {
        win::panel_debug(std::fs::read_to_string(std::env::var("SHANDIAN_XL_NAME_FILE").unwrap()).unwrap().trim());
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sd-xl-{:x}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn leftover_partial_of_unselected_file_does_not_block_done() {
        let dir = std::env::temp_dir().join(format!("sd-xl-left-{}", std::process::id()));
        let task = dir.join("task");
        std::fs::create_dir_all(&task).unwrap();
        std::fs::write(task.join("a.mkv"), vec![0u8; 100]).unwrap();
        std::fs::write(task.join("b.mkv.bt.xltd"), vec![0u8; 10]).unwrap();
        let roots = vec![dir.clone()];
        let mut job = ThunderJob { actual: "task".into(), dir: dir.to_string_lossy().into_owned(), total: 100, downloaded: 100, created: now_ms(), ..Default::default() };
        // 还在下载中（读得到这一行）：临时文件说明没下完
        let row = s(&["task", "已下载 0/1 项 ", "10B/100B"]);
        assert!(!observe(&mut job, &row, &roots));
        assert_eq!(job.done_streak, 0);
        // 这一行没了、字节数已满：残留的临时文件不挡完成
        job.downloaded = 100;
        let done = (0..3).any(|_| observe(&mut job, &[], &roots));
        assert!(done);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn row_reads_size_and_speed() {
        let l = s(&["下载中", "a(1).iso", "已下载 0/1 项 ", "60.5MB/209MB", "剩余 00:02:09", "1.1MB/s", "b.iso", "已下载 0/1 项 ", "未知大小", "资源连接中..."]);
        let r = parse_row(&l, "a(1).iso").unwrap();
        assert_eq!(r.downloaded, Some((60.5 * 1048576.0) as u64));
        assert_eq!(r.total, Some(209 * 1048576));
        assert_eq!(r.speed, Some((1.1 * 1048576.0) as u64));
        // 下一行的「未知大小」不能算到别人头上；b 自己也没有数字
        assert_eq!(parse_row(&l, "b.iso").unwrap(), RowInfo { items: Some((0, 1)), ..Default::default() });
    }

    #[test]
    fn row_ignores_same_text_elsewhere_and_reads_paused() {
        assert!(parse_row(&s(&["Sintel", "别的文字"]), "Sintel").is_none());
        let r = parse_row(&s(&["Sintel", "已下载 10/11 项 ", "3MB/123.3MB", "已暂停"]), "Sintel").unwrap();
        assert!(r.paused);
        let e = parse_row(&s(&["a", "已下载 0/1 项 ", "任务连接超时，无法继续下载"]), "a").unwrap();
        assert_eq!(e.error.as_deref(), Some("任务连接超时，无法继续下载"));
        assert_eq!(r.speed, None);
    }

    #[test]
    fn variant_names() {
        assert!(is_variant("a.iso", "a.iso"));
        assert!(is_variant("a.iso", "a(12).iso"));
        assert!(!is_variant("a.iso", "a(x).iso"));
        assert!(!is_variant("a.iso", "ab.iso"));
        assert!(is_variant("Sintel", "Sintel(1)"));
    }

    #[test]
    fn partial_names() {
        assert_eq!(strip_partial("a.iso.xltd"), Some("a.iso"));
        assert_eq!(strip_partial("a.iso.xltd.cfg"), Some("a.iso"));
        assert_eq!(strip_partial("a.mp4.bt.xltd"), Some("a.mp4"));
        assert_eq!(strip_partial("a.iso"), None);
    }

    #[test]
    fn config_roots_fix_bare_drive() {
        let r = roots_from_config(r#"{"TaskDefaultSettings":{"DefaultPath":"Y:\\","InitPath":"C:\\迅雷下载"},"PathAndCategory":{"LastUsedPath":"Y:"}}"#);
        assert_eq!(r, vec![PathBuf::from("Y:\\"), PathBuf::from("C:\\迅雷下载")]);
    }

    #[test]
    fn single_file_finishes_only_after_partial_gone() {
        let dir = temp_dir();
        let roots = vec![dir.clone()];
        let mut job = ThunderJob { name: "a.iso".into(), created: now_ms(), ..Default::default() };
        assert!(!observe(&mut job, &[], &roots));
        std::fs::write(dir.join("a(1).iso.xltd"), vec![0u8; 100]).unwrap();
        assert!(!observe(&mut job, &[], &roots));
        assert_eq!(job.actual, "a(1).iso");
        assert_eq!(job.total, 100);
        std::fs::rename(dir.join("a(1).iso.xltd"), dir.join("a(1).iso")).unwrap();
        assert!(!observe(&mut job, &[], &roots));
        // 无障碍树里残留的旧行不能挡住完成
        let ghost = s(&["a(1).iso", "已下载 0/1 项 ", "1MB/2MB", "1MB/s"]);
        assert!(!observe(&mut job, &ghost, &roots));
        assert!(observe(&mut job, &ghost, &roots));
        assert!(job.done);
        assert_eq!(job.downloaded, 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bt_folder_found_by_hash_waits_for_partial() {
        let dir = temp_dir();
        let sub = dir.join("Movie");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("ABC.torrent"), b"x").unwrap();
        std::fs::write(sub.join("a.srt"), b"sub").unwrap();
        std::fs::write(sub.join("a.mp4.bt.xltd"), vec![0u8; 10]).unwrap();
        let roots = vec![dir.clone()];
        let mut job = ThunderJob { hash: "ABC".into(), created: now_ms(), ..Default::default() };
        assert!(!observe(&mut job, &[], &roots));
        assert_eq!(job.actual, "Movie");
        std::fs::rename(sub.join("a.mp4.bt.xltd"), sub.join("a.mp4")).unwrap();
        for _ in 0..2 {
            assert!(!observe(&mut job, &[], &roots));
        }
        assert!(observe(&mut job, &[], &roots));
        assert_eq!(job.total, 13); // 不算 .torrent
        assert_eq!(file_of(&job).file_name().unwrap(), "a.mp4");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_same_name_is_not_this_task() {
        let dir = temp_dir();
        std::fs::write(dir.join("a.iso"), b"old").unwrap();
        let mut job = ThunderJob { name: "a.iso".into(), created: now_ms() + 60_000, ..Default::default() };
        assert!(!observe(&mut job, &[], &[dir.clone()]));
        assert!(job.actual.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 实机验证，要本机装着迅雷，会真的下东西。用法：
    /// SHANDIAN_XL_URL=<公开链接> cargo test --bin shandian-windows thunder_live -- --ignored --nocapture
    #[test]
    #[ignore]
    fn thunder_live() {
        let url = std::env::var("SHANDIAN_XL_URL").unwrap();
        let scene = win::Scene::capture();
        win::add_task(&url).unwrap();
        let title = url_name(&url);
        let target = std::env::var("SHANDIAN_XL_TARGET").unwrap_or_default();
        let limit = if is_direct(&url) { 10 } else { 30 };
        let wanted = target.clone();
        let names = |k: &str| std::env::var(k).unwrap_or_default().split('|').filter(|x| !x.is_empty()).map(str::to_string).collect::<Vec<_>>();
        let select = (names("SHANDIAN_XL_SELECT"), names("SHANDIAN_XL_ALL"));
        std::thread::spawn(move || win::quiet_confirm(&title, &wanted, &select, limit, scene));
        let mut job = ThunderJob { name: url_name(&url), url: url.clone(), hash: info_hash(&url), target, created: now_ms(), ..Default::default() };
        let pressed = true;
        for tick in 0..600 {
            // 暂停、继续各按一次，看迅雷读回来的状态
            if (tick == 12 || tick == 24) && !job.actual.is_empty() {
                println!("{tick}s toggle={}", win::press_row_toggle(&job.actual));
            }
            let finished = observe(&mut job, &win::main_leaves(), &save_roots());
            if tick % 5 == 0 || finished {
                println!("{tick}s pressed={pressed} dir={} actual={} {}/{} speed={} paused={} msg={}", job.dir, job.actual, job.downloaded, job.total, job.speed, job.paused, job.message);
            }
            if finished {
                println!("done file={:?} size={:?}", file_of(&job), std::fs::metadata(file_of(&job)).map(|m| m.len()));
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        panic!("10 分钟没下完");
    }

    #[test]
    fn move_keeps_existing_files_and_drops_torrent() {
        let (a, b) = (temp_dir(), temp_dir());
        std::fs::create_dir_all(a.join("M(1)")).unwrap();
        std::fs::write(a.join("M(1)").join("H.torrent"), b"x").unwrap();
        std::fs::write(a.join("M(1)").join("a.mp4"), b"new").unwrap();
        std::fs::create_dir_all(b.join("M")).unwrap();
        std::fs::write(b.join("M").join("old"), b"old").unwrap();
        assert_eq!(move_to(&a, "M(1)", "M", &b).unwrap(), "M(1)");
        assert!(b.join("M").join("old").exists());
        assert!(b.join("M(1)").join("a.mp4").exists());
        assert!(!b.join("M(1)").join("H.torrent").exists());
        assert!(!a.join("M(1)").exists());
        let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b));
    }

    /// 实机：SHANDIAN_XL_DELETE=<迅雷里的任务名>。会真的删这条（进迅雷回收站）
    #[test]
    #[ignore]
    fn thunder_delete_live() {
        println!("deleted={}", win::delete_row(&std::env::var("SHANDIAN_XL_DELETE").unwrap()));
    }

    /// 实机：把已在迅雷下完的东西移走（跨盘、迅雷可能占着）。SHANDIAN_XL_MOVE=<目录>|<名字>|<目标>
    #[test]
    #[ignore]
    fn thunder_move_live() {
        let v = std::env::var("SHANDIAN_XL_MOVE").unwrap();
        let p: Vec<&str> = v.split('|').collect();
        println!("{:?}", move_to(Path::new(p[0]), p[1], p[1], Path::new(p[2])));
    }

    #[test]
    fn single_file_torrent_flat_in_target() {
        let dir = temp_dir();
        std::fs::write(dir.join("ABC.torrent"), b"d4:infod6:lengthi1e4:name5:a.isoee").unwrap();
        std::fs::write(dir.join("other.iso.bt.xltd"), vec![0u8; 10]).unwrap();
        std::fs::write(dir.join("a.iso.bt.xltd"), vec![0u8; 10]).unwrap();
        let mut job = ThunderJob { hash: "ABC".into(), target: dir.to_string_lossy().into_owned(), created: now_ms(), ..Default::default() };
        assert!(!observe(&mut job, &[], &[]));
        assert_eq!(job.actual, "a.iso");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn selected_count_text() {
        assert_eq!(selected_count("已选 3/11 个文件"), Some(3));
        assert_eq!(selected_count("已选 11/11 个文件，共 123.31 MB"), Some(11));
        assert_eq!(selected_count("别的"), None);
    }

    #[test]
    fn thunder_link_format() {
        assert_eq!(thunder_link("http://a"), "thunder://QUFodHRwOi8vYVpa");
    }

    #[test]
    fn names_from_links() {
        assert_eq!(url_name("https://x.cn/a/b%20c.iso?x=1"), "b c.iso");
        assert_eq!(url_name("magnet:?xt=urn:btih:ABC&dn=Sintel"), "Sintel");
        assert_eq!(info_hash("magnet:?xt=urn:btih:abc&dn=x"), "ABC");
    }
}

/// 手动排查：把 SHANDIAN_DIAG_URL 交给迅雷，20 秒内每秒打印新建窗口里的内容，然后关掉窗口（不点下载）。
/// cargo test --bin shandian-windows thunder_dialog_diag -- --ignored --nocapture
#[cfg(test)]
mod diag {
    #[test]
    #[ignore]
    fn thunder_dialog_diag() {
        let Ok(url) = std::env::var("SHANDIAN_DIAG_URL") else { return };
        super::win::add_task(&url).expect("add_task");
        let mut last = Vec::new();
        for i in 0..20 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let now = super::win::dialog_texts();
            if now != last {
                println!("--- {i}s ---\n{}", now.join("\n"));
                last = now;
            }
        }
        super::win::close_new_task();
    }
}

/// 手动排查：和「交给迅雷」完全同一套自动确认，看卡在哪一步。
/// SHANDIAN_DIAG_URL=磁力 SHANDIAN_DIAG_DIR=保存目录 cargo test --bin shandian-windows thunder_handoff_diag -- --ignored --nocapture
#[cfg(test)]
mod diag_handoff {
    #[test]
    #[ignore]
    fn thunder_handoff_diag() {
        let (Ok(url), Ok(dir)) = (std::env::var("SHANDIAN_DIAG_URL"), std::env::var("SHANDIAN_DIAG_DIR")) else { return };
        let title = super::url_name(&url);
        let scene = super::win::Scene::capture();
        super::win::lock_foreground(true);
        let (t, d) = (title.clone(), dir.clone());
        let started = std::time::Instant::now();
        let h = std::thread::spawn(move || super::win::quiet_confirm(&t, &d, &(Vec::new(), Vec::new()), 30, scene));
        std::thread::sleep(std::time::Duration::from_millis(200));
        super::win::add_task(&url).expect("add_task");
        let outcome = h.join().unwrap();
        println!("结果：{outcome:?}");
        println!("quiet_confirm 结束，用时 {:?}，新建窗口还开着：{}", started.elapsed(), super::win::new_task_open());
        if super::win::new_task_open() {
            println!("{}", super::win::dialog_texts().join("\n"));
        }
    }
}

/// 手动排查：打印当前开着的新建窗口（含路径框的值），再关掉
#[cfg(test)]
mod diag_dump {
    #[test]
    #[ignore]
    fn thunder_dialog_dump() {
        println!("{}", super::win::dialog_texts().into_iter().filter(|l| l.starts_with("输入") || l.starts_with("按钮")).collect::<Vec<_>>().join("\n"));
        if std::env::var("SHANDIAN_DIAG_CLOSE").is_ok() {
            super::win::close_new_task();
        }
    }
}

/// 手动排查：直接按一次「立即下载」，看窗口会不会关
#[cfg(test)]
mod diag_press {
    #[test]
    #[ignore]
    fn thunder_press_once() {
        let pressed = super::win::press_download("");
        std::thread::sleep(std::time::Duration::from_secs(2));
        println!("按下={pressed} 两秒后窗口还开着={}", super::win::new_task_open());
    }
}

/// 手动排查：删掉迅雷里一条任务（排查时交过去的）
#[cfg(test)]
mod diag_delete {
    #[test]
    #[ignore]
    fn thunder_delete_diag() {
        let Ok(name) = std::env::var("SHANDIAN_DIAG_NAME") else { return };
        println!("删掉={}", super::win::delete_row(&name));
    }
}

#[cfg(test)]
mod diag_rows {
    #[test]
    #[ignore]
    fn thunder_rows_diag() {
        let leaves = super::win::main_leaves();
        println!("ROWS {}", leaves.join(" | "));
        println!("共 {} 项", leaves.len());
    }
}


/// 手动排查：迅雷缩在托盘时把主窗口叫到屏幕外，读一下任务列表，再关回托盘
#[cfg(test)]
mod diag_summon {
    #[test]
    #[ignore]
    fn thunder_summon_diag() {
        let t = std::time::Instant::now();
        {
            let _m = super::win::summon_main();
            let leaves = super::win::main_leaves();
            println!("ROWS {:?} 用时 {:?}", leaves.iter().take(30).collect::<Vec<_>>(), t.elapsed());
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// 手动排查：迅雷窗口关着时，对一行按两次暂停/继续（回到原状），每次后读这一行的状态
#[cfg(test)]
mod diag_toggle {
    #[test]
    #[ignore]
    fn thunder_toggle_diag() {
        let Ok(name) = std::env::var("SHANDIAN_DIAG_NAME") else { return };
        let row = || super::parse_row(&super::win::main_leaves(), &name).map(|r| (r.paused, r.speed));
        println!("开始 {:?}", row());
        super::win::toggle_trace(&name);
    }
}

/// 手动排查：和 set_paused 一样的流程（叫出窗口 → 按一次），按完读状态；SHANDIAN_DIAG_CLOSE 时先把迅雷关到托盘
#[cfg(test)]
mod diag_set_paused {
    #[test]
    #[ignore]
    fn thunder_set_paused_diag() {
        let Ok(name) = std::env::var("SHANDIAN_DIAG_NAME") else { return };
        let read = || {
            let _m = super::win::summon_main();
            format!("叫出来读 paused={:?}", super::parse_row(&super::win::main_leaves(), &name).map(|r| r.paused))
        };
        println!("开始 {}", read());
        std::thread::sleep(std::time::Duration::from_secs(4));
        for i in 0..2 {
            if std::env::var("SHANDIAN_DIAG_CLOSE").is_ok() {
                super::win::close_main_for_test();
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            println!("第{i}次 主窗口数 {}", super::win::main_count());
            super::win::trace_tops(&name);
            let t = std::time::Instant::now();
            let want = !super::parse_row(&{ let _m = super::win::summon_main(); super::win::main_leaves() }, &name).map(|r| r.paused).unwrap_or(false);
            std::thread::sleep(std::time::Duration::from_secs(3));
            if std::env::var("SHANDIAN_DIAG_CLOSE").is_ok() {
                super::win::close_main_for_test();
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            // 模拟真实情况：焦点在我们的 App 上时点暂停/继续
            super::win::focus_app_for_test();
            std::thread::sleep(std::time::Duration::from_millis(500));
            println!("  开始前 前台/最上层是迅雷 = {:?}", super::win::front_is_thunder());
            let t = std::time::Instant::now(); // 只计按的这一段（和 set_paused 同一套步骤）
            // 每 2ms 采样：迅雷在前台、迅雷挡在最上层各累计多久
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let sampler = {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    let (mut fg, mut top, mut n) = (0u32, 0u32, 0u32);
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        let (a, b) = super::win::front_is_thunder();
                        fg += a as u32;
                        top += b as u32;
                        n += 1;
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                    (fg, top, n)
                })
            };
            let ok = {
                let _behind = std::env::var("SHANDIAN_DIAG_NOGUARD").is_err().then(super::win::keep_behind);
                let _main = super::win::summon_main_settle(0);
                let started = std::time::Instant::now();
                loop {
                    match super::win::toggle_row(&name, want) {
                        Some(done) => { println!("  到位={done}"); break true; }
                        None if started.elapsed().as_secs() < 5 => std::thread::sleep(std::time::Duration::from_millis(150)),
                        None => break super::win::press_row_toggle(&name),
                    }
                }
            };
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            let (fg, top, n) = sampler.join().unwrap();
            println!("按下={ok} 用时 {:?}；采样 {n} 次，迅雷在前台 {fg} 次、挡在最上层 {top} 次（约 {} ms）", t.elapsed(), top * 2);
            std::thread::sleep(std::time::Duration::from_secs(4));
            println!("之后 藏着读 paused={:?}", super::parse_row(&super::win::main_leaves(), &name).map(|r| r.paused));
            println!("之后 {:?}", read());
            std::thread::sleep(std::time::Duration::from_secs(4));
        }
    }
}

#[cfg(test)]
mod diag_experiment {
    #[test]
    #[ignore]
    fn thunder_experiment() {
        let (Ok(name), Ok(mode)) = (std::env::var("SHANDIAN_DIAG_NAME"), std::env::var("SHANDIAN_DIAG_MODE")) else { return };
        super::win::experiment(&name, &mode);
    }
}
