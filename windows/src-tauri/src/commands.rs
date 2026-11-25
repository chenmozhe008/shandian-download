// 设置页、目录选择、存放位置提示这几个命令。
//
// 目录选择用 tauri-plugin-dialog 的 Windows 原生对话框，必须在阻塞线程上调，
// 不能占住 async 运行时（否则整个窗口在选目录期间会假死）。

use std::path::PathBuf;

use serde::Serialize;
use tauri::{Manager, State};

use crate::links::LinkKind;
use crate::settings::{Folders, Settings};
use crate::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirView {
    pub path: String,
    /// 界面上给人看的名：下载 ／ 下载 › 电影 ／ D:\movies
    pub display: String,
    /// 是不是默认位置（决定「恢复默认」按钮显不显示）
    pub is_default: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPayload {
    pub settings: Settings,
    pub movie: DirView,
    pub video: DirView,
    pub other: DirView,
}

fn dir_view(custom: &str, fallback: &PathBuf, f: &Folders) -> DirView {
    let custom = crate::settings::normalize_dir(custom);
    let p = if custom.is_empty() {
        fallback.clone()
    } else {
        PathBuf::from(&custom)
    };
    DirView {
        display: f.display(&p),
        path: p.to_string_lossy().into(),
        is_default: custom.is_empty(),
    }
}

fn payload(s: &Settings, f: &Folders) -> SettingsPayload {
    SettingsPayload {
        settings: s.clone(),
        movie: dir_view(&s.dir_movie, &f.movie, f),
        video: dir_view(&s.dir_video, &f.video, f),
        other: dir_view(&s.dir_other, &f.other, f),
    }
}

#[tauri::command(async)]
pub fn get_settings(state: State<'_, AppState>) -> SettingsPayload {
    let s = state.settings.lock().unwrap().clone();
    payload(&s, &state.folders)
}

/// 保存设置并写进内核。返回一句提示（内核没在跑时设置会在下次启动时生效）。
#[tauri::command]
pub async fn save_settings(
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<String, String> {
    // 用户在设置页改了某类目录，就以新设置为准，别让上次的选择盖住它
    let mut settings = settings;
    settings.max_videos = settings.max_videos.clamp(1, 10);
    let refill_video_queue = {
        let cur = state.settings.lock().unwrap();
        // This internal routing hint may have changed since the settings form was loaded.
        settings.wechat_browser = cur.wechat_browser.clone();
        if cur.dir_movie != settings.dir_movie || cur.dir_video != settings.dir_video || cur.dir_other != settings.dir_other {
            settings.last_dirs.clear();
        }
        settings.max_videos > cur.max_videos
    };
    settings.save(&state.base)?;
    // 锁用完立刻放：后面要 await，MutexGuard 不能跨 await
    {
        let mut cur = state.settings.lock().unwrap();
        *cur = settings.clone();
    }
    if refill_video_queue {
        let videos = state.videos.clone();
        tauri::async_runtime::spawn_blocking(move || videos.pump());
    }
    let (port, token, folders) = {
        let e = state.engine.lock().unwrap();
        (e.port, e.token.clone(), state.folders.clone())
    };
    match settings.apply(&crate::api::Api::new(port, token), &folders).await {
        Ok(()) => Ok("设置已保存".into()),
        Err(e) => Ok(format!("设置已保存，但内核没接受（{e}），下次启动内核时会重试")),
    }
}

/// 弹系统的选文件夹对话框。取消返回 null。
#[tauri::command]
pub async fn pick_folder(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    title: String,
    current: String,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let mut dlg = app.dialog().file().set_parent(&window).set_title(title);
    if !current.trim().is_empty() {
        dlg = dlg.set_directory(current);
    }
    // blocking_pick_folder 会一直等用户选完，放阻塞线程，别卡 async 运行时
    let picked = tauri::async_runtime::spawn_blocking(move || dlg.blocking_pick_folder())
        .await
        .map_err(|e| format!("对话框出错：{e}"))?;
    Ok(picked.map(|p| p.to_string()))
}

/// 字幕样式导出成文件（JSON）：弹系统的另存为对话框。取消返回 false
#[tauri::command]
pub async fn export_sub_styles(app: tauri::AppHandle, text: String) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    let dlg = app.dialog().file().set_title("导出字幕样式").set_file_name("闪电字幕样式.json").add_filter("字幕样式", &["json"]);
    let picked = tauri::async_runtime::spawn_blocking(move || dlg.blocking_save_file()).await.map_err(|e| format!("对话框出错：{e}"))?;
    let Some(path) = picked else { return Ok(false) };
    let path = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("写文件失败：{e}"))?;
    Ok(true)
}

/// 导入别人分享的字幕样式文件：弹系统的打开对话框，返回文件内容。取消返回 null
#[tauri::command]
pub async fn import_sub_styles(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let dlg = app.dialog().file().set_title("导入字幕样式").add_filter("字幕样式", &["json"]);
    let picked = tauri::async_runtime::spawn_blocking(move || dlg.blocking_pick_file()).await.map_err(|e| format!("对话框出错：{e}"))?;
    let Some(path) = picked else { return Ok(None) };
    let path = path.into_path().map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("读文件失败：{e}"))?;
    Ok(Some(text))
}

/// 弹系统的选文件对话框（用来导 cookies.txt）。取消返回 null。
#[tauri::command]
pub async fn pick_text_file(
    app: tauri::AppHandle,
    title: String,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let dlg = app
        .dialog()
        .file()
        .set_title(title)
        .add_filter("Cookie 文件", &["txt"]);
    let picked = tauri::async_runtime::spawn_blocking(move || dlg.blocking_pick_file())
        .await
        .map_err(|e| format!("对话框出错：{e}"))?;
    Ok(picked.map(|p| p.to_string()))
}

/// 支持的站点和它们在 cookie 文件里的域名特征。
/// 顺序就是设置页里的显示顺序。
pub const COOKIE_SITES: &[(&str, &str, &str)] = &[
    ("youtube", "YouTube", "youtube.com"),
    ("douyin", "抖音", "douyin.com"),
    ("xhs", "小红书", "xiaohongshu.com"),
    ("xigua", "西瓜视频", "ixigua.com"),
    ("kuaishou", "快手", "kuaishou.com"),
    ("weibo", "微博", "weibo.com"),
    ("vimeo", "Vimeo", "vimeo.com"),
    ("xpc", "新片场", "xinpianchang.com"),
];

/// 已导入 Cookie 的站点（以数据目录里的 cookies-<站点>.txt 为准，不另存状态）
#[tauri::command(async)]
pub fn cookies_status(state: State<AppState>) -> Vec<String> {
    COOKIE_SITES
        .iter()
        .filter(|(site, _, _)| crate::migrate::local_path(&state.local, &format!("cookies-{site}.txt")).exists())
        .map(|(_, name, _)| name.to_string())
        .collect()
}

/// 导入一份浏览器导出的 cookies.txt（Netscape 格式）。
///
/// 一个文件里通常带好几个站的 Cookie，这里按域名拆开，各存一份 cookies-<站点>.txt，
/// 建任务时按站点自动带上。抖音、西瓜、小红书、YouTube 现在都要求「真实浏览器的 Cookie」，
/// 光靠程序自己请求首页拿到的匿名 Cookie 不够（实测抖音只给 1 条，西瓜一条都不给）。
#[tauri::command(async)]
pub fn import_cookies(state: State<AppState>, path: String) -> Result<String, String> {
    let src = std::fs::read_to_string(&path).map_err(|e| format!("读不了这个文件：{e}"))?;
    let low = src.to_lowercase();
    let mut done: Vec<&str> = Vec::new();
    for (site, name, host) in COOKIE_SITES {
        if !low.contains(host) {
            continue;
        }
        let dst = crate::migrate::local_path(&state.local, &format!("cookies-{site}.txt"));
        if std::fs::write(&dst, &src).is_ok() {
            done.push(name);
        }
    }
    if done.is_empty() {
        return Err(format!(
            "这个文件里没有支持站点的 Cookie。支持的站点：{}",
            COOKIE_SITES.iter().map(|(_, n, _)| *n).collect::<Vec<_>>().join("、")
        ));
    }
    Ok(format!(
        "已导入 {} 的 Cookie，这些站的任务会自动带上",
        done.join("、")
    ))
}

/// 让窗口（标题栏 + 底色）跟着界面主题走。
/// 界面自己的配色是 data-theme 控制的，但标题栏是系统画的：手动选「深色」而系统是浅色时，
/// 不调这个就会出现「白标题栏 + 黑界面」。底色一起改，下次启动才不会先闪一下浅色。
#[tauri::command]
pub fn set_app_theme(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    theme: String,
) -> Result<(), String> {
    if window.label() != "main" {
        return Ok(());
    }
    let pref = crate::theme::normalize(&theme);
    // 先更新当前偏好，切换后立即新开的窗口不必等待设置写盘。
    state.settings.lock().unwrap().theme = pref.to_owned();
    crate::theme::apply(&window, pref);
    for (label, window) in app.webview_windows() {
        if label.starts_with("dialog-") {
            crate::theme::apply(&window, pref);
            crate::theme::apply_view(&window, pref);
        } else if matches!(label.as_str(), "float" | "done" | "tip") {
            crate::theme::apply_view(&window, pref);
        }
    }
    Ok(())
}

/// 开机启动状态：以注册表为准（settings.json 里不存，避免两处说法不一致）
#[tauri::command]
pub fn autostart_status() -> bool {
    crate::shell::autostart_enabled()
}

#[tauri::command]
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    crate::shell::set_autostart(enabled)
}

/// 资源管理器打开一个目录（不存在就先建出来）
#[tauri::command]
pub fn open_dir(path: String) -> Result<(), String> {
    let path = path.replace('/', "\\");
    let p = PathBuf::from(&path);
    std::fs::create_dir_all(&p).map_err(|e| format!("建目录失败：{e}"))?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // explorer 找不到目录时会自己去「文档」，所以先建
        std::process::Command::new("explorer")
            .arg(&p)
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|e| format!("打开目录失败：{e}"))?;
    }
    Ok(())
}

/// After removing the group's owned files, discard only an empty directory.
#[tauri::command(async)]
pub fn remove_empty_collection_dir(path: String, wait_for_files: Option<bool>) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    let path = PathBuf::from(path);
    if !path.is_absolute() || path.parent().is_none() {
        return Err("合集目录无效".into());
    }
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if !meta.is_dir() || meta.file_attributes() & 0x400 != 0 { return Ok(()); }
    let result = std::fs::remove_dir(&path);
    let result = if wait_for_files == Some(true) && result.as_ref().is_err_and(|error| error.kind() == std::io::ErrorKind::DirectoryNotEmpty) {
        // The NAS gateway releases video workers before unlinking their owned files.
        std::thread::sleep(std::time::Duration::from_millis(1500));
        std::fs::remove_dir(&path)
    } else { result };
    match result {
        Ok(()) => Ok(()),
        Err(error) if matches!(error.kind(), std::io::ErrorKind::DirectoryNotEmpty | std::io::ErrorKind::NotFound) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// 在资源管理器里打开所在文件夹并选中它。文件不在了就打开它的上级目录
#[tauri::command(async)]
pub fn reveal_path(path: String) -> Result<(), String> {
    // explorer /select 只认反斜杠：保存目录、种子里的子路径常带「/」，不换的话会跳到「文档」
    let path = path.replace('/', "\\");
    let p = PathBuf::from(&path);
    if !p.exists() {
        return open_dir(p.parent().map(|d| d.display().to_string()).unwrap_or(path));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // 路径里有空格时要原样带引号，走 raw_arg，普通 arg 会被转义
        std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", p.display()))
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|e| format!("打开目录失败：{e}"))?;
    }
    Ok(())
}

/// 播放：文件直接打开，文件夹打开里面最大的视频（NAS 任务的「播放」按钮）
#[tauri::command(async)]
pub fn open_media(path: String) -> Result<(), String> {
    // 「播放」用：路径是文件夹（种子的资源目录）就打开里面最大的视频，和本机任务的 item_open 一样
    let p = PathBuf::from(&path);
    if p.is_dir() {
        if let Some(v) = crate::engine::largest_video(&p) {
            return open_path(v.to_string_lossy().into_owned());
        }
    }
    open_path(path)
}

/// 用系统默认程序打开一个文件（下载历史双击「文件还在原位置」时用）
#[tauri::command(async)]
pub fn open_path(path: String) -> Result<(), String> {
    let path = PathBuf::from(path.replace('/', "\\"));
    if !path.exists() {
        return Err("文件或文件夹不在原位置，或当前无法访问".into());
    }
    crate::engine::shell_open(&path)
}

/// 这条链接会存到哪，输入时显示在输入框旁边
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetInfo {
    pub kind: String,
    pub dir: String,
    pub display: String,
    /// 视频网站才有：记住的下载选项（画质／字幕／仅音频），确认框预填用
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_options: Option<crate::videos::VideoOptions>,
    /// 这个站有没有字幕可下，没有就不显示字幕选项
    pub supports_subs: bool,
    pub collection: bool,
    pub baidu_share: bool,
    pub feishu_document: bool,
}

#[tauri::command]
pub fn target_info(link: String, prepare: Option<bool>, headers: Option<Vec<(String, String)>>, state: State<'_, AppState>) -> Option<TargetInfo> {
    let kind = LinkKind::of(&link)?;
    let s = state.settings.lock().unwrap().clone();
    let baidu_share = crate::baidu::share_url(&link).is_some();
    let feishu_document = crate::feishu::document_url(&link).is_some();
    if !baidu_share && !feishu_document && prepare.unwrap_or(false) && s.github_mirror {
        crate::ghmirror::prepare(&link, &headers.unwrap_or_default());
    }
    let dir = s.default_dir(kind, &state.folders);
    Some(TargetInfo {
        kind: if baidu_share { "百度网盘".into() } else {
            crate::api::download_label(&link).unwrap_or_else(|| kind.label().to_string())
        },
        baidu_share,
        feishu_document,
        display: state.folders.display(&dir),
        dir: dir.to_string_lossy().into(),
        video_options: (kind == LinkKind::VideoSite)
            .then(|| crate::videos::VideoOptions::load(&state.base)),
        supports_subs: crate::links::supports_subtitles(&link),
        collection: crate::links::video_collection(&link, false).is_some(),
    })
}

/// 合集按一次确认保存默认设置，之后每集使用收到的设置快照。
#[tauri::command]
pub fn remember_video_options(video: crate::videos::VideoOptions, state: State<'_, AppState>) {
    video.save(&state.base);
}

/// Ctrl+Q：彻底退出。关窗口只是收进托盘，退出和托盘菜单走同一条路（先停内核再退）
#[tauri::command]
pub fn quit_app(app: tauri::AppHandle) {
    crate::tray::quit(&app);
}

/// 用户点击复制/粘贴时使用系统剪贴板，不触发 WebView 网页授权。
#[tauri::command]
pub fn clipboard_read_text() -> String {
    crate::shell::clipboard_text().unwrap_or_default()
}

#[tauri::command]
pub fn clipboard_write_text(window: tauri::WebviewWindow, text: String) -> Result<(), String> {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{OpenClipboard, CloseClipboard, EmptyClipboard, SetClipboardData};
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        if OpenClipboard(hwnd.0 as _) == 0 { return Err("剪贴板正被占用，请重试".into()); }
        let result = (|| {
            let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
            if mem.is_null() { return Err("无法分配剪贴板内存".into()); }
            let ptr = GlobalLock(mem);
            if ptr.is_null() { GlobalFree(mem); return Err("无法写入剪贴板".into()); }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr as *mut u16, wide.len());
            GlobalUnlock(mem);
            if EmptyClipboard() == 0 || SetClipboardData(13, mem).is_null() {
                GlobalFree(mem); return Err("复制失败，请重试".into());
            }
            Ok(()) // 成功后内存归系统剪贴板所有。
        })();
        CloseClipboard();
        result
    }
}

/// Open the existing log location; do not copy or upload user logs automatically.
#[tauri::command(async)]
pub fn open_logs_dir() -> Result<(), String> {
    let dir = crate::dirs_of_log().ok_or("找不到日志目录")?;
    open_dir(dir.display().to_string())
}

#[tauri::command(async)]
pub fn diagnostic_info() -> String {
    use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};
    let build = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .and_then(|key| key.get_value::<String, _>("CurrentBuildNumber")).unwrap_or_else(|_| "未知".into());
    let os = if build.parse::<u32>().unwrap_or(0) >= 22000 { "Windows 11" } else { "Windows 10" };
    let browsers = ["chrome", "edge"].into_iter().filter(|b| crate::browser_support::browser_exe(b).is_some()).collect::<Vec<_>>().join(", ");
    let ext = crate::gateway::extension_status();
    format!("应用版本：{}\n系统：{}（{}）\n架构：{}\n浏览器：{}\n扩展连接：{}\n", env!("CARGO_PKG_VERSION"), os, build, std::env::consts::ARCH, if browsers.is_empty() { "未检测到 Chrome/Edge" } else { &browsers }, if ext.connected { "已连接" } else { "未连接或尚未激活" })
}
