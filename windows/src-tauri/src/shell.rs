// Windows 专属的系统集成：单实例、深链/磁力协议、开机启动、完成通知、防睡眠、剪贴板监听。
//
// 这些在 Mac 版是「系统自带的功能」（UNUserNotificationCenter、NSWorkspace、IOPMAssertion…），
// Windows 上每个都要自己接一层：
//   - 单实例：第二次打开不新起进程，把链接转给已有窗口（同时避免两个内核抢同一个数据目录）
//   - 协议注册：写 HKCU\Software\Classes，用户点 magnet:/ed2k:/shandian: 能唤起我们
//   - 开机启动：HKCU\...\Run，不需要管理员权限
//   - 防睡眠：只在本机下载时持有系统电源请求
//   - 剪贴板：轮询，识别到能下的链接就提示（Mac 版是弹浮窗）

use std::sync::Mutex;
use std::time::Duration;

use tauri::{Emitter, Manager};

use crate::AppState;

/// 待处理的外来链接（启动参数、单实例转发）。后台循环取到就投任务。
pub static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// 从命令行参数里挑出能下的链接。
/// 浏览器/资源管理器双击磁力会给 "magnet:?xt=..."，shandian:// 是我们自己的协议。
pub fn links_from_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for a in args.iter().skip(1) {
        let s = a.trim();
        let low = s.to_lowercase();
        if low.starts_with("magnet:?") || low.starts_with("ed2k://") || low.starts_with("http://") || low.starts_with("https://") {
            out.push(s.to_string());
        } else if low.starts_with("shandian://open") || low.starts_with("shandian://activate") {
            // 扩展在 App 没开时用 shandian://open 把程序拉起来（launchApp），
            // 它只是「打开窗口」的意思，不是下载链接，别拿去投任务
            continue;
        } else if let Some(rest) = low.strip_prefix("shandian://") {
            // shandian://add?url=https%3A%2F%2F... 或 shandian://https://...
            let raw = &s[s.len() - rest.len()..];
            let inner = raw
                .split("url=")
                .nth(1)
                .map(percent_decode)
                .unwrap_or_else(|| raw.trim_start_matches('/').to_string());
            if !inner.is_empty() {
                out.push(inner);
            }
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

pub fn push_pending(link: &str) {
    let mut q = PENDING.lock().unwrap();
    if !q.iter().any(|x| x == link) {
        q.push(link.to_string());
    }
}

pub fn take_pending() -> Vec<String> {
    std::mem::take(&mut *PENDING.lock().unwrap())
}

/// 把 shandian:// 以及 magnet/ed2k 注册到当前用户（HKCU，不写 HKLM，不需要管理员）
pub fn register_protocols() {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let exe = exe.display().to_string();
    let Ok(classes) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
        r"Software\Classes",
        winreg::enums::KEY_WRITE,
    ) else {
        return;
    };

    for (scheme, label) in [
        ("shandian", "闪电全能下载"),
        ("magnet", "磁力链接"),
        ("ed2k", "电驴链接"),
    ] {
        if let Ok((key, _)) = classes.create_subkey(scheme) {
            let _ = key.set_value("", &format!("URL:{label}"));
            let _ = key.set_value("URL Protocol", &"");
            if let Ok((icon, _)) = classes.create_subkey(format!(r"{scheme}\DefaultIcon")) {
                let _ = icon.set_value("", &format!("\"{exe}\",0"));
            }
            if let Ok((cmd, _)) = classes.create_subkey(format!(r"{scheme}\shell\open\command")) {
                let _ = cmd.set_value("", &format!("\"{exe}\" \"%1\""));
            }
        }
    }
}

/// 开机启动（HKCU\Software\Microsoft\Windows\CurrentVersion\Run）
pub fn set_autostart(enable: bool) -> Result<(), String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = hkcu
        .create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")
        .map_err(|e| format!("写注册表失败：{e}"))?;
    if enable {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        run.set_value("ShandianDownload", &format!("\"{}\"", exe.display()))
            .map_err(|e| format!("写注册表失败：{e}"))?;
    } else {
        let _ = run.delete_value("ShandianDownload");
    }
    let _ = KEY_SET_VALUE; // 说明用：create_subkey 已带写权限
    Ok(())
}

pub fn autostart_enabled() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")
        .and_then(|k| k.get_value::<String, _>("ShandianDownload"))
        .is_ok()
}

/// 读一次剪贴板文字（没有文字就返回 None）
pub fn clipboard_text() -> Option<String> {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};

    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        // 剪贴板可能被别的程序占着，抢不到就下次再读
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() {
            CloseClipboard();
            return None;
        }
        let ptr = GlobalLock(handle) as *const u16;
        if ptr.is_null() {
            CloseClipboard();
            return None;
        }
        let mut len = 0usize;
        while *ptr.add(len) != 0 && len < 32768 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
        GlobalUnlock(handle);
        CloseClipboard();
        Some(text)
    }
}

struct DownloadWakeRequest(std::os::windows::io::OwnedHandle);

impl DownloadWakeRequest {
    fn new() -> Option<Self> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Power::{PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest};
        use windows_sys::Win32::System::Threading::{
            REASON_CONTEXT, REASON_CONTEXT_0, POWER_REQUEST_CONTEXT_SIMPLE_STRING,
        };
        let mut reason: Vec<u16> = "闪电全能下载正在下载\0".encode_utf16().collect();
        let context = REASON_CONTEXT {
            Version: 0,
            Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
            Reason: REASON_CONTEXT_0 { SimpleReasonString: reason.as_mut_ptr() },
        };
        let handle = unsafe { PowerCreateRequest(&context) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        if unsafe { PowerSetRequest(handle.as_raw_handle(), PowerRequestSystemRequired) } == 0 {
            return None;
        }
        Some(Self(handle))
    }
}

impl Drop for DownloadWakeRequest {
    fn drop(&mut self) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Power::{PowerClearRequest, PowerRequestSystemRequired};
        // The request belongs to this handle, even if Tokio resumes on another thread.
        unsafe { PowerClearRequest(self.0.as_raw_handle(), PowerRequestSystemRequired); }
    }
}

fn needs_download_wake(
    enabled: bool,
    downloading: bool,
    speed: u64,
    last_transfer: &mut Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    if !enabled || !downloading {
        *last_transfer = None;
        return false;
    }
    if speed > 0 {
        *last_transfer = Some(now);
    }
    last_transfer.is_some_and(|t| now.saturating_duration_since(t) < Duration::from_secs(5 * 60))
}

/// 后台循环：处理外来链接、完成通知、防睡眠、剪贴板。
/// 用的都是「每秒看一眼」的做法，和界面轮询同一节奏，简单可靠。
pub fn start_background(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        // 启动时把「当前剪贴板内容」当成基准，而不是空串。
        // 否则第一次轮询（1 秒后）会把开程序之前就躺在剪贴板里的链接当成新复制的，
        // 于是每次启动都先弹一次浮窗——「一开就弹」就是这么来的。
        // 规矩：开程序之前复制的东西不算，开之后新复制的才提示。
        // 认剪贴板的序号，不认内容：同一个链接再复制一次也算新动作，用户是想再弹一次。
        // 启动时已经在剪贴板里的不弹
        let mut last_clip = clipboard_seq();
        let mut wake_request = None;
        // 最近一次「真在下」的时刻：速度掉下来后再等一会儿才放开，免得在两段之间来回切
        let mut last_busy: Option<std::time::Instant> = None;
        let mut tick: u32 = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(1000)).await;
            let state = app.state::<AppState>();

            // 1) 外来链接：内核没起来就等，起来再投。
            // 视频站和开着「新建前确认」时走 ext-add 弹确认框（字幕/画质选项在里面），
            // 别在后台静默下完——用户反馈过「粘贴/协议进来不弹选项」。
            let pending = take_pending();
            if !pending.is_empty() {
                let ready = {
                    let mut e = state.engine.lock().unwrap();
                    e.is_running()
                };
                if ready {
                    let confirm = state.settings.lock().unwrap().confirm_new;
                    for url in pending {
                        let is_video =
                            crate::links::LinkKind::of(&url) == Some(crate::links::LinkKind::VideoSite);
                        if confirm || is_video || crate::baidu::share_url(&url).is_some() || crate::feishu::document_url(&url).is_some() {
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.unminimize();
                                let _ = w.show();
                                let _ = w.set_focus();
                                let _ = w.emit("ext-add", serde_json::json!({ "url": url }));
                                continue;
                            }
                        }
                        // 别在后台循环里 await 下载：视频站起 yt-dlp 可能拖好几秒，
                        // 会把后面的剪贴板轮询一起卡住（复制链接没反应就是这么来的）。
                        // 丢进独立任务，后台循环继续每秒跑自己的事。
                        let app2 = app.clone();
                        let u = url.clone();
                        tauri::async_runtime::spawn(async move {
                            let st = app2.state::<AppState>();
                            let _ = crate::engine::add_link(&u, &st).await;
                        });
                    }
                } else {
                    for url in pending {
                        push_pending(&url);
                    }
                }
            }

            let settings = state.settings.lock().unwrap().clone();

            // 2) 托盘提示 + 防睡眠，每 2 秒看一次本机任务。
            if tick % 2 == 0 {
                let (running, speed, downloading) = activity(&app).await;
                crate::tray::set_hint(&app, &hint_text(running, speed));
                let prevent_sleep = state.settings.lock().unwrap().prevent_sleep;
                if needs_download_wake(prevent_sleep, downloading, speed, &mut last_busy, std::time::Instant::now()) {
                    if wake_request.is_none() {
                        wake_request = DownloadWakeRequest::new();
                    }
                } else {
                    wake_request = None;
                }
            }
            tick = tick.wrapping_add(1);

            // 4) 剪贴板：识别到能下的链接。
            //    程序在前台就只填进输入框（窗口里的提示条）；不在前台（比如用户正在浏览器里
            //    复制链接）就额外弹一个不抢焦点的小浮窗，和 Mac 版行为一致
            if settings.watch_clipboard {
                let seq = clipboard_seq();
                let text = if seq != last_clip { clipboard_text() } else { None };
                if let Some(text) = text {
                    last_clip = seq;
                    let t = text.trim().to_string();
                    let links = if t.len() < 4096 { downloadable_links(&t) } else { vec![] };
                    if let Some(first) = links.first().cloned() {
                        if crate::wechat::is_share(&first) { let _ = app.emit("wechat-share-detected", first); continue; }
                        let front = app
                            .get_webview_window("main")
                            .map(|w| w.is_focused().unwrap_or(false) && w.is_visible().unwrap_or(false))
                            .unwrap_or(false);
                        let sharing=crate::links::is_pan_share(&first);
                        let _ = app.emit("clipboard-link", first);
                        if !front || sharing {
                            // 一段文字里可能好几个链接（复制的是一整段），一起列在浮窗里。
                            // 建窗口必须回主线程：Tauri 的窗口只能在事件循环线程上创建，
                            // 在后台 async 任务里直接 build 会 panic（release 下是静默退出）
                            let h = app.clone();
                            crate::mark(&format!("剪贴板命中 {} 个链接，准备弹浮窗", links.len()));
                            let _ = app.run_on_main_thread(move || crate::float::show(&h, links));
                        }
                    }
                }
            }
        }
    });
}

fn clipboard_seq() -> u32 {
    unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() }
}

/// 复制的内容里值得弹窗的链接。不要求整段就是一个链接：
/// B 站分享复制出来是「【标题】 https://b23.tv/xxx」，手机端还会多带一段话
fn downloadable_links(text: &str) -> Vec<String> {
    crate::links::extract(text)
        .into_iter()
        .filter(|l| {
            let low = l.to_lowercase();
            low.starts_with("magnet:?")
                || low.starts_with("ed2k://")
                || crate::links::worth_popup(l, &[])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{downloadable_links, needs_download_wake};

    #[test]
    fn idle_or_disabled_releases_download_wake_immediately() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let mut last = None;
        assert!(!needs_download_wake(true, false, 0, &mut last, start));
        assert!(needs_download_wake(true, true, 1, &mut last, start));
        assert!(needs_download_wake(true, true, 0, &mut last, start + Duration::from_secs(10)));
        assert!(!needs_download_wake(true, false, 0, &mut last, start + Duration::from_secs(12)));
        assert!(last.is_none());
        assert!(!needs_download_wake(true, true, 0, &mut last, start + Duration::from_secs(20)));
        assert!(needs_download_wake(true, true, 1, &mut last, start));
        assert!(!needs_download_wake(false, true, 1, &mut last, start));
        assert!(last.is_none());
        assert!(needs_download_wake(true, true, 1, &mut last, start));
        assert!(!needs_download_wake(true, true, 0, &mut last, start + Duration::from_secs(300)));
    }

    #[test]
    fn finds_links_inside_copied_text() {
        // 地址栏复制
        let u = "https://www.bilibili.com/video/BV1A6HF6aE52/?trackid=web_pegasus_0.x.966&spm_id_from=333.1007.tianma.1-3-3.click&vd_source=15114f";
        assert_eq!(downloadable_links(u), [u]);
        // 分享按钮复制
        assert_eq!(downloadable_links("【标题-哔哩哔哩】 https://b23.tv/AbCd123"), ["https://b23.tv/AbCd123"]);
        assert_eq!(downloadable_links("magnet:?xt=urn:btih:abc"), ["magnet:?xt=urn:btih:abc"]);
        // 网盘链接不弹：分享链接 App 下不了（要先存到自己网盘，再在网页上用扩展的按钮下），网盘页面更不用说
        assert!(downloadable_links("https://pan.quark.cn/s/abc123").is_empty());
        assert!(downloadable_links("链接: https://pan.baidu.com/s/1AbC 提取码: x1y2").is_empty());
        assert!(downloadable_links("https://pan.quark.cn/list#/list/all").is_empty());
        assert!(downloadable_links("https://pan.baidu.com/disk/main#/index").is_empty());
        // 普通网页、普通文字不弹
        assert!(downloadable_links("https://www.example.com/page").is_empty());
        assert!(downloadable_links("今天天气不错").is_empty());
        // 一段话里混着网页和视频，只留视频
        assert_eq!(
            downloadable_links("见 https://www.example.com/page 和 https://www.youtube.com/watch?v=abc"),
            ["https://www.youtube.com/watch?v=abc"]
        );
    }

    #[test]
    fn detects_copied_gitee_release_urls() {
        let url = "https://gitee.com/mg-chao/snow-apps/releases/download/v1.6.3_snow-shot/snow-shot-1.6.3-windows-x64-offline.exe";
        assert_eq!(downloadable_links(url), [url]);
        let markdown = format!("[{url}\u{a0}]({url})");
        assert_eq!(downloadable_links(&markdown), [url]);
        let compact = format!("[{url}]({url})");
        assert_eq!(downloadable_links(&compact), [url]);
        let asset = "https://gitee.com/example/project/releases/download/v1/installer";
        assert_eq!(downloadable_links(asset), [asset]);
        assert!(downloadable_links("https://gitee.com/example/project/releases/latest").is_empty());
    }
}

/// 发一条 Windows 原生通知（通知插件不可用时静默失败）
pub trait NotifyExt {
    fn notify_rust(&self, title: &str, body: &str) -> Result<(), String>;
}

impl NotifyExt for tauri::AppHandle {
    fn notify_rust(&self, title: &str, body: &str) -> Result<(), String> {
        use tauri_plugin_notification::NotificationExt;
        self.notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|e| e.to_string())
    }
}

/// 托盘 tooltip 的文字：有任务在下就写总速度和任务数，空闲就写名字。
/// 这个函数在后台循环里「每两秒」被调一次，用的就是界面同一个内核接口。
/// 正在跑的任务数和总速度（内核任务 + 视频站任务）
async fn activity(app: &tauri::AppHandle) -> (usize, u64, bool) {
    use tauri::Manager;
    let Some(state) = app.try_state::<AppState>() else {
        return (0, 0, false);
    };
    let mut speed: u64 = 0;
    let mut running = 0usize;
    let mut downloading = false;
    for j in state.videos.list() {
        if j.state == "running" {
            running += 1;
            if j.stage.is_empty() || j.stage == "preparing" {
                downloading = true;
                speed += j.speed;
            }
        }
    }
    let (port, token) = match state.engine.lock() {
        Ok(e) => (e.port, e.token.clone()),
        Err(_) => (0, String::new()),
    };
    if port != 0 {
        let api = crate::api::Api::new(port, token);
        if let Ok(Ok(list)) = tokio::time::timeout(Duration::from_secs(2), api.list_tasks()).await {
            for t in list {
                let st = t.status.as_str();
                if st == "running" || st == "wait" {
                    running += 1;
                }
                let total = t.meta.as_ref().and_then(|meta| meta.res.as_ref()).map_or(0, |res| res.size);
                if st == "running" && (total == 0 || t.downloaded() < total) {
                    downloading = true;
                    speed += t.speed();
                }
            }
        }
    }
    (running, speed, downloading)
}

fn hint_text(running: usize, speed: u64) -> String {
    if running == 0 {
        "闪电全能下载 · 空闲".into()
    } else {
        format!(
            "闪电全能下载 · {} · {} 个任务",
            crate::links::speed_text(speed),
            running
        )
    }
}
