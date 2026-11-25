// 微信视频号（移植自 Mac 版 WeChatChannels.swift）。
//
// 用户在已登录的腾讯元宝页面用扩展解析，扩展把分享链接和临时播放地址交过来。
// 播放地址和密钥只在内存里用，不写进任务记录。文件前 128 KiB 用 ISAAC-64 加密，见 xor_head。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

const HEAD: usize = 131_072;
const RANDSIZ: usize = 256;

// 临时播放凭据只在 Rust 内存中等待确认；确认框只收到不含凭据的随机票据。
#[derive(Clone)]
struct Pending {
    share: String,
    playable: String,
    title: String,
    at: std::time::Instant,
    opened: bool,
}

static PENDING: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Pending>>> =
    std::sync::LazyLock::new(Default::default);

pub fn stage(share: &str, playable: &str, title: &str) -> String {
    let ticket = format!("{:032x}", rand::random::<u128>());
    let mut pending = PENDING.lock().unwrap();
    pending.retain(|_, p| p.at.elapsed().as_secs() < 600);
    let key=crate::links::canonical_url(share);
    if let Some((existing,_))=pending.iter().find(|(_,p)|crate::links::canonical_url(&p.share)==key){return existing.clone();}
    pending.insert(ticket.clone(), Pending { share: share.into(), playable: playable.into(), title: title.into(), opened:false, at: std::time::Instant::now() });
    ticket
}

static LAST_EXTENSION: std::sync::Mutex<Option<crate::browser_support::ExtensionProfile>> = std::sync::Mutex::new(None);

fn open_preview(share: &str, target: &crate::browser_support::ExtensionProfile) -> Result<(String, tokio::sync::oneshot::Receiver<()>), String> {
    if !is_share(share) { return Err("视频号分享链接无效".into()); }
    let (nonce, receiver) = crate::browser_handoff::register(share);
    // A normal loopback page wakes the existing content-script bridge at document_start.
    // Browser-private extension resources are not valid external navigation targets in Edge.
    let mut page = url::Url::parse("http://127.0.0.1:6800/channel-open").unwrap();
    let mut params = url::form_urlencoded::Serializer::new(String::new());
    params.append_pair("wechat-auto", &nonce).append_pair("share", share);
    page.set_fragment(Some(&params.finish()));
    if let Err(error) = crate::gateway::open_browser_profile_url(target.browser, page.as_str(), Some(&target.profile)) {
        crate::browser_handoff::cancel(&nonce);
        return Err(error);
    }
    Ok((nonce, receiver))
}

async fn handoff(share: &str, target: &crate::browser_support::ExtensionProfile, cached: bool) -> bool {
    let began = std::time::Instant::now();
    let Ok((nonce, receiver)) = open_preview(share, target) else { return false; };
    // A real acknowledgement wakes this immediately, rather than waiting for a 250 ms poll tick.
    let ok = matches!(tokio::time::timeout(std::time::Duration::from_secs(15), receiver).await, Ok(Ok(())));
    crate::browser_handoff::cancel(&nonce);
    crate::app_log::record("wechat", &format!("handoff browser={} cached={} elapsed_ms={} ok={ok}", target.browser, cached, began.elapsed().as_millis()));
    if ok { *LAST_EXTENSION.lock().unwrap() = Some(target.clone()); }
    ok
}

fn remember_browser(app: &tauri::AppHandle, target: &crate::browser_support::ExtensionProfile) {
    use tauri::Manager;
    let app = app.clone(); let browser = target.browser.to_string(); let profile = target.profile.clone();
    // Save the routing hint off the interaction path, only when it actually changes.
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<crate::AppState>();
        let mut settings = state.settings.lock().unwrap();
        let selected = Some((browser, profile));
        if settings.wechat_browser != selected {
            settings.wechat_browser = selected;
            if settings.save(&state.base).is_err() { crate::app_log::record("wechat", "remember browser save failed"); }
        }
    });
}

#[derive(serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HandoffResult { Ready, Missing, Unresponsive }

#[tauri::command]
pub async fn wechat_open_preview(share: String, app: tauri::AppHandle, state: tauri::State<'_, crate::AppState>) -> Result<HandoffResult, String> {
    if !is_share(&share) { return Err("视频号分享链接无效".into()); }
    let remembered = LAST_EXTENSION.lock().unwrap().clone();
    let saved = state.settings.lock().unwrap().wechat_browser.clone();
    let cached = tauri::async_runtime::spawn_blocking(move || {
        remembered.and_then(|target| target.refresh(crate::gateway::extension_id()))
            .or_else(|| saved.and_then(|(browser, profile)| crate::browser_support::remembered_profile(&browser, &profile, crate::gateway::extension_id())))
    }).await.map_err(|e| e.to_string())?;
    let mut attempted = None;
    if let Some(target) = cached {
        if handoff(&share, &target, true).await { remember_browser(&app, &target); return Ok(HandoffResult::Ready); }
        attempted = Some((target.browser, target.profile));
    }
    *LAST_EXTENSION.lock().unwrap() = None;
    let began = std::time::Instant::now();
    let candidates = tauri::async_runtime::spawn_blocking(|| {
        crate::browser_support::browser_order().into_iter().filter_map(|browser| {
            crate::browser_support::browser_exe(browser)?;
            crate::browser_support::extension_profile(browser, crate::gateway::extension_id())
        }).collect::<Vec<_>>()
    }).await.map_err(|e| e.to_string())?;
    crate::app_log::record("wechat", &format!("discovery elapsed_ms={} candidates={}", began.elapsed().as_millis(), candidates.len()));
    let found = attempted.is_some() || !candidates.is_empty();
    for target in candidates {
        if attempted.as_ref().is_some_and(|(browser, profile)| *browser == target.browser && *profile == target.profile) { continue; }
        if handoff(&share, &target, false).await { remember_browser(&app, &target); return Ok(HandoffResult::Ready); }
    }
    crate::tray::reveal(&app);
    Ok(if found { HandoffResult::Unresponsive } else { HandoffResult::Missing })
}

pub fn claim_auto(nonce: &str, share: &str) -> bool { crate::browser_handoff::claim(nonce, share) }

pub fn begin_dialog(ticket:&str)->bool {
    let mut p=PENDING.lock().unwrap();
    let Some(item)=p.get_mut(ticket) else{return false};
    if item.opened{return false};item.opened=true;true
}

pub fn cancel_pending(ticket: &str) {
    PENDING.lock().unwrap().remove(ticket);
}

#[tauri::command]
pub fn wechat_cancel(ticket: String) { cancel_pending(&ticket); }

fn take_pending(ticket: &str) -> Result<Pending, String> {
    PENDING.lock().unwrap().remove(ticket)
        .filter(|p| p.at.elapsed().as_secs() < 600)
        .ok_or_else(|| "视频号解析已过期或已取消，请回到视频页重新点击闪电下载。".into())
}

#[tauri::command]
pub async fn wechat_confirm(ticket: String, name: String, dir: String, remember: bool, force: Option<bool>, state: tauri::State<'_, crate::AppState>) -> Result<String, String> {
    let folder = if dir.trim().is_empty() {
        state.settings.lock().unwrap().default_dir(crate::links::LinkKind::VideoSite, &state.folders)
    } else { PathBuf::from(dir.trim().replace('/', "\\")) };
    std::fs::create_dir_all(&folder).map_err(|e| format!("无法使用这个保存目录：{e}"))?;
    let pending = take_pending(&ticket)?;
    let title = if name.trim().is_empty() { pending.title.as_str() } else { name.trim() };
    let id = state.videos.start_wechat(&pending.share, &pending.playable, title, &folder, force.unwrap_or(false))?;
    let mut settings = state.settings.lock().unwrap();
    settings.remember_dir(&folder);
    settings.last_dirs.insert(crate::links::LinkKind::VideoSite.label().into(), folder.to_string_lossy().into_owned());
    if remember { settings.confirm_new = false; }
    let _ = settings.save(&state.base);
    Ok(id)
}

// ---------------- ISAAC-64，Bob Jenkins 的公共领域算法 ----------------

struct Isaac64 {
    aa: u64,
    bb: u64,
    cc: u64,
    rsl: [u64; RANDSIZ],
    mm: [u64; RANDSIZ],
}

fn mix(s: &mut [u64; 8]) {
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *s;
    a = a.wrapping_sub(e);
    f ^= h >> 9;
    h = h.wrapping_add(a);
    b = b.wrapping_sub(f);
    g ^= a << 9;
    a = a.wrapping_add(b);
    c = c.wrapping_sub(g);
    h ^= b >> 23;
    b = b.wrapping_add(c);
    d = d.wrapping_sub(h);
    a ^= c << 15;
    c = c.wrapping_add(d);
    e = e.wrapping_sub(a);
    b ^= d >> 14;
    d = d.wrapping_add(e);
    f = f.wrapping_sub(b);
    c ^= e << 20;
    e = e.wrapping_add(f);
    g = g.wrapping_sub(c);
    d ^= f >> 17;
    f = f.wrapping_add(g);
    h = h.wrapping_sub(d);
    e ^= g << 14;
    g = g.wrapping_add(h);
    *s = [a, b, c, d, e, f, g, h];
}

impl Isaac64 {
    fn new(seed: u64) -> Self {
        let mut r = Self { aa: 0, bb: 0, cc: 0, rsl: [0; RANDSIZ], mm: [0; RANDSIZ] };
        r.rsl[0] = seed;
        let mut s = [0x9e37_79b9_7f4a_7c13u64; 8];
        for _ in 0..4 {
            mix(&mut s);
        }
        for pass in 0..2 {
            for i in (0..RANDSIZ).step_by(8) {
                for k in 0..8 {
                    let v = if pass == 0 { r.rsl[i + k] } else { r.mm[i + k] };
                    s[k] = s[k].wrapping_add(v);
                }
                mix(&mut s);
                r.mm[i..i + 8].copy_from_slice(&s);
            }
        }
        r.generate();
        r
    }

    fn generate(&mut self) {
        let mut a = self.aa;
        self.cc = self.cc.wrapping_add(1);
        let mut b = self.bb.wrapping_add(self.cc);
        for i in 0..RANDSIZ {
            let x = self.mm[i];
            let mixed = match i % 4 {
                0 => !(a ^ (a << 21)),
                1 => a ^ (a >> 5),
                2 => a ^ (a << 12),
                _ => a ^ (a >> 33),
            };
            a = mixed.wrapping_add(self.mm[(i + RANDSIZ / 2) % RANDSIZ]);
            let y = self.mm[((x >> 3) as usize) % RANDSIZ].wrapping_add(a).wrapping_add(b);
            self.mm[i] = y;
            b = self.mm[((y >> 11) as usize) % RANDSIZ].wrapping_add(x);
            self.rsl[i] = b;
        }
        self.bb = b;
        self.aa = a;
    }
}

/// 只处理前 128 KiB。结果倒序取，每个字按大端展开
pub fn xor_head(seed: u64, bytes: &mut [u8]) {
    let mut rng = Isaac64::new(seed);
    let count = bytes.len().min(HEAD);
    let (mut offset, mut remaining) = (0, RANDSIZ - 1);
    while offset < count {
        let word = rng.rsl[remaining].to_be_bytes();
        if remaining == 0 {
            rng.generate();
            remaining = RANDSIZ - 1;
        } else {
            remaining -= 1;
        }
        for b in word {
            if offset == count {
                break;
            }
            bytes[offset] ^= b;
            offset += 1;
        }
    }
}

// ---------------- 下载 ----------------

fn trusted(raw: &str, exact: Option<&str>) -> bool {
    let Ok(u) = url::Url::parse(raw) else { return false };
    let host = u.host_str().unwrap_or_default().to_lowercase();
    if u.scheme() != "https" || !u.username().is_empty() || u.password().is_some() {
        return false;
    }
    match exact {
        Some(e) => host == e,
        None => host == "qq.com" || host.ends_with(".qq.com"),
    }
}

pub fn is_share(raw: &str) -> bool {
    let Ok(u) = url::Url::parse(raw.trim()) else { return false };
    let parts: Vec<&str> = u.path().split('/').filter(|p| !p.is_empty()).collect();
    matches!(u.scheme(), "http" | "https")
        && u.host_str().is_some_and(|h| h.eq_ignore_ascii_case("weixin.qq.com"))
        && parts.len() == 2
        && parts[0].eq_ignore_ascii_case("sph")
}

pub fn is_playable(raw: &str) -> bool {
    trusted(raw, Some("channels.weixin.qq.com"))
}

pub fn safe_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .take(60)
        .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { ' ' } else { c })
        .collect();
    let t = cleaned.trim().trim_end_matches('.').trim();
    if t.is_empty() { "微信视频号".into() } else { t.to_string() }
}

fn decode_key(feed: &Value) -> Result<Option<u64>, String> {
    let invalid = || "视频号解析结果无效，请重新解析".to_string();
    match &feed["decodeKey"] {
        Value::Null => Ok(None),
        Value::Number(n) => n.as_u64().map(Some).ok_or_else(invalid),
        Value::String(s) => s.parse().map(Some).map_err(|_| invalid()),
        _ => Err(invalid()),
    }
}

/// progress(已下, 总量, 秒速)，返回 false 表示用户暂停或删除了
pub fn download(playable: &str, title: &str, dir: &Path, progress: &dyn Fn(u64, u64, u64) -> bool) -> Result<PathBuf, String> {
    let invalid = || "视频号解析结果无效，请重新解析".to_string();
    let page = url::Url::parse(playable).map_err(|_| invalid())?;
    if !is_playable(playable) {
        return Err(invalid());
    }
    let query = |k: &str| page.query_pairs().find(|(n, v)| n == k && !v.is_empty()).map(|(_, v)| v.to_string());
    let (token, eid) = (query("token").ok_or_else(invalid)?, query("eid").ok_or_else(invalid)?);

    // 腾讯的地址，不走代理。跳转也只能留在腾讯域名内
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(None::<std::time::Duration>)
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() < 10 && trusted(a.url().as_str(), None) { a.follow() } else { a.stop() }
        }))
        .user_agent("Mozilla/5.0")
        .build()
        .map_err(|e| e.to_string())?;

    let info = client
        .post("https://channels.weixin.qq.com/finder-preview/api/feed/get_feed_info")
        .timeout(std::time::Duration::from_secs(60))
        .header("Content-Type", "application/json")
        .header("Origin", "https://channels.weixin.qq.com")
        .header("Referer", playable)
        .body(json!({ "baseReq": { "generalToken": token }, "exportId": eid }).to_string())
        .send()
        .map_err(|_| "获取视频信息失败，请稍后重新解析".to_string())?;
    if !info.status().is_success() {
        return Err(format!("获取视频信息失败（HTTP {}），请稍后重新解析", info.status().as_u16()));
    }
    let root: Value = info.json().map_err(|_| "未获取到视频信息，请重新解析".to_string())?;
    let feed = &root["data"]["feedInfo"];
    let video = [&feed["videoUrl"], &feed["h264VideoInfo"]["videoUrl"], &feed["originVideoUrl"]]
        .into_iter()
        .filter_map(|v| v.as_str())
        .find(|s| !s.is_empty())
        .ok_or("未获取到视频信息，请重新解析")?
        .to_string();
    if !trusted(&video, None) {
        return Err("视频地址不在腾讯域名内，已停止下载".into());
    }
    let key = decode_key(feed)?;

    let mut resp = client
        .get(&video)
        .header("Referer", "https://channels.weixin.qq.com/")
        .send()
        .map_err(|_| "视频下载失败，请重新解析后重试".to_string())?;
    if resp.status().as_u16() != 200 {
        return Err("视频下载失败，请重新解析后重试".into());
    }
    let total = resp.content_length().unwrap_or(0);

    std::fs::create_dir_all(dir).map_err(|e| format!("建目录失败：{e}"))?;
    let base = safe_name(title);
    let temp = dir.join(format!("{base}.{:032x}.part", rand::random::<u128>()));
    let result = (|| -> Result<PathBuf, String> {
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp).map_err(|e| format!("写文件失败：{e}"))?;
        let mut head = Vec::with_capacity(HEAD);
        let mut buf = vec![0u8; 256 * 1024];
        let (mut done, mut head_done) = (0u64, false);
        let (mut mark, mut mark_bytes, mut speed) = (std::time::Instant::now(), 0u64, 0u64);
        let write_head = |head: &mut Vec<u8>, out: &mut std::fs::File| -> Result<(), String> {
            if let Some(k) = key.filter(|k| *k != 0) {
                xor_head(k, head);
            }
            if head.len() <= 12 || &head[4..8] != b"ftyp" {
                return Err("视频格式无法识别，未保存文件".into());
            }
            out.write_all(head).map_err(|e| format!("写文件失败：{e}"))
        };
        loop {
            let n = resp.read(&mut buf).map_err(|_| "视频下载失败，请重新解析后重试".to_string())?;
            if n == 0 {
                break;
            }
            done += n as u64;
            let mut rest = &buf[..n];
            if !head_done {
                let take = (HEAD - head.len()).min(rest.len());
                head.extend_from_slice(&rest[..take]);
                rest = &rest[take..];
                if head.len() == HEAD {
                    write_head(&mut head, &mut out)?;
                    head_done = true;
                }
            }
            if !rest.is_empty() {
                out.write_all(rest).map_err(|e| format!("写文件失败：{e}"))?;
            }
            let elapsed = mark.elapsed().as_secs_f64();
            if elapsed >= 0.5 {
                speed = ((done - mark_bytes) as f64 / elapsed) as u64;
                (mark, mark_bytes) = (std::time::Instant::now(), done);
                if !progress(done, total, speed) {
                    return Err(String::new());
                }
            }
        }
        if !head_done {
            write_head(&mut head, &mut out)?;
        }
        if total > 0 && done != total {
            return Err("视频下载失败，请重新解析后重试".into());
        }
        drop(out);
        progress(done, total.max(done), speed);
        publish_copy(&temp, dir, &base)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

// 原子占用新名称；同名已存在时顺延，任何情况下都不覆盖原文件。
fn publish_copy(temp: &Path, dir: &Path, base: &str) -> Result<PathBuf, String> {
    for n in 0..10000 {
        let name = if n == 0 { format!("{base}.mp4") } else { format!("{base} ({n}).mp4") };
        let dest = dir.join(name);
        match std::fs::hard_link(temp, &dest) {
            Ok(()) => { let _ = std::fs::remove_file(temp); return Ok(dest); }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => {}, // 不支持硬链接的共享盘，使用独占创建后复制。
        }
        let mut out = match std::fs::OpenOptions::new().write(true).create_new(true).open(&dest) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("保存文件失败：{e}")),
        };
        let copied = std::fs::File::open(temp).and_then(|mut src| std::io::copy(&mut src, &mut out)).and_then(|_| out.flush());
        drop(out);
        if let Err(e) = copied { let _ = std::fs::remove_file(&dest); return Err(format!("保存文件失败：{e}")); }
        let _ = std::fs::remove_file(temp);
        return Ok(dest);
    }
    Err("同名副本过多，请更换文件名".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn auto_intent_is_bound_to_video_and_consumed_once() {
        let (nonce, mut receiver) = crate::browser_handoff::register("https://weixin.qq.com/sph/testVideo");
        assert!(!claim_auto(&nonce,"https://weixin.qq.com/sph/anotherVideo"));
        assert!(claim_auto(&nonce,"https://weixin.qq.com/sph/testVideo"));
        assert!(receiver.try_recv().is_ok());
        assert!(!claim_auto(&nonce,"https://weixin.qq.com/sph/testVideo"));
    }

    #[test]
    fn same_video_only_opens_one_pending_dialog() {
        let a=stage("https://weixin.qq.com/sph/pendingTest","https://channels.weixin.qq.com/x?token=a&eid=b","test");
        let b=stage("https://weixin.qq.com/sph/pendingTest?from=share","https://channels.weixin.qq.com/x?token=c&eid=d","test");
        assert_eq!(a,b);assert!(begin_dialog(&a));assert!(!begin_dialog(&b));cancel_pending(&a);
    }

    #[test]
    fn duplicate_copies_keep_original_and_each_other() {
        let dir = std::env::temp_dir().join(format!("sd-copy-{:032x}",rand::random::<u128>()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("lesson.mp4");
        std::fs::write(&original,b"original").unwrap();
        let handles: Vec<_> = (0..2).map(|i| {
            let dir=dir.clone();
            std::thread::spawn(move || {
                let temp=dir.join(format!("{i}.part"));std::fs::write(&temp,format!("copy{i}")).unwrap();
                publish_copy(&temp,&dir,"lesson").unwrap()
            })
        }).collect();
        let paths:Vec<_>=handles.into_iter().map(|h|h.join().unwrap()).collect();
        assert_ne!(paths[0],paths[1]);
        assert_eq!(std::fs::read(&original).unwrap(),b"original");
        for (i,p) in paths.iter().enumerate(){assert_eq!(std::fs::read_to_string(p).unwrap(),format!("copy{i}"));}
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn confirmation_ticket_is_single_use_and_cancelable() {
        let ticket = stage("https://weixin.qq.com/sph/test123", "https://channels.weixin.qq.com/play?token=temporary&eid=test", "示例视频");
        assert_eq!(take_pending(&ticket).unwrap().title, "示例视频");
        assert!(take_pending(&ticket).is_err());
        let ticket = stage("https://weixin.qq.com/sph/test456", "https://channels.weixin.qq.com/play?token=temporary&eid=test", "示例视频");
        cancel_pending(&ticket);
        assert!(take_pending(&ticket).is_err());
    }

    #[test]
    fn expired_confirmation_ticket_is_rejected() {
        let ticket = stage("https://weixin.qq.com/sph/test789", "https://channels.weixin.qq.com/play?token=temporary&eid=test", "示例视频");
        PENDING.lock().unwrap().get_mut(&ticket).unwrap().at = std::time::Instant::now() - std::time::Duration::from_secs(601);
        assert!(take_pending(&ticket).is_err());
    }

    #[test]
    fn keystream_matches_mac_reference() {
        let mut bytes = vec![0u8; HEAD];
        xor_head(12345, &mut bytes);
        let hex: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "932e7fed86b329540fd72561f3401d0f11412b1fe976217efa016e2a9a17fcbc");
    }

    #[test]
    fn only_first_128k_changes_and_roundtrips() {
        let mut bytes = vec![0x55u8; HEAD + 28];
        xor_head(54321, &mut bytes);
        assert!(bytes[HEAD..].iter().all(|b| *b == 0x55));
        xor_head(54321, &mut bytes);
        assert!(bytes.iter().all(|b| *b == 0x55));
    }

    #[test]
    fn link_rules() {
        assert!(is_share("https://weixin.qq.com/sph/AnYC9T2a4d"));
        assert!(!is_share("https://example.com/sph/test"));
        assert!(is_playable("https://channels.weixin.qq.com/x?token=1&eid=2"));
        assert!(!is_playable("https://evil.qq.com/x"));
        assert!(!trusted("https://qq.com.evil.com/x", None));
        assert_eq!(safe_name("a/b:c."), "a b c");
    }
}
