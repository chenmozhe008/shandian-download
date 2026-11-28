// 剪贴板浮窗：Mac 版是 NSPanel（.nonactivatingPanel + .floating），屏幕右下角（任务栏上方）弹一个
// 不抢焦点的小窗，15 秒自己消失。
//
// Windows 上没有「非激活面板」这种东西，等价物是 WS_EX_NOACTIVATE 窗口：
//   - .focused(false)  + set_focusable(false) → 弹出时不把主窗口的焦点抢走
//   - .always_on_top(true)                     → level = .floating
//   - .skip_taskbar(true) + .decorations(false) → 不占任务栏、没有标题栏
// 页面在 float.html，用独立窗口而不是在主窗口里画浮层，因为它必须浮在别的程序上面。

use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::settings::Settings;
use crate::AppState;

pub const FLOAT_ID: &str = "float";
/// 一行式布局：图标｜三行文字｜右侧一列按钮，宽度给足让网址有地方显示
const FLOAT_W: f64 = 488.0;
const FLOAT_H: f64 = 138.0;
/// 距屏幕右下角（任务栏上方）的距离（逻辑像素）
const MARGIN_X: f64 = 16.0;
const MARGIN_Y: f64 = 12.0;
const DONE_ID: &str = "done";
const DONE_W: f64 = 488.0;
const DONE_H: f64 = 96.0;

#[derive(Clone, Serialize)]
pub struct FloatItem {
    pub url: String,
    pub title: String,
    pub kind: String,
    pub dir: String,
}

fn pending() -> &'static Mutex<Vec<FloatItem>> {
    static PENDING: OnceLock<Mutex<Vec<FloatItem>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(Vec::new()))
}

/// 弹窗。内容为空就直接不弹（避免复制一段没有链接的文字也闪一下）
pub fn show(app: &tauri::AppHandle, links: Vec<String>) {
    crate::mark(&format!("浮窗：进入 show（{} 个链接）", links.len()));
    let items = decorate(app, &links);
    if items.is_empty() {
        crate::mark("浮窗：decorate 后为空，跳过");
        return;
    }
    *pending().lock().unwrap() = items;

    let pref = crate::theme::preference(app);

    // 已经存在就复用：先刷内容再亮出来，避免重复建窗口导致闪烁
    if let Some(w) = app.get_webview_window(FLOAT_ID) {
        let _ = w.emit("float-data", ());
        place(&w);
        let _ = w.show();
        return;
    }

    let url = WebviewUrl::App("float.html".into());
    // 真透明窗口：圆角和阴影都由页面 CSS 画（浏览器抗锯齿，没有区域硬剪的锯齿）。
    // 透明模式不能设窗口底色，否则画面全黑
    let built = WebviewWindowBuilder::new(app, FLOAT_ID, url)
        .title("检测到下载链接")
        .inner_size(FLOAT_W, FLOAT_H)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        // 必须和主窗口用同一串 WebView2 参数，否则共享环境会拒绝创建第二个 webview
        .initialization_script(crate::theme::init(&pref))
        .additional_browser_args(crate::BROWSER_ARGS)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .visible(false)
        // 等页面自己画完再亮出来：否则会先看到一个空白的透明框
        .on_page_load(move |w, payload| {
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                let pref = crate::theme::preference(w.app_handle());
                crate::theme::apply_view(&w, &pref);
                // 关键：先关掉可激活属性再显示，否则 show() 会把焦点从用户正在用的程序抢过来
                let _ = w.set_focusable(false);
                let shown = w.show();
                // 先显示再定位：隐藏状态下的 set_position 在 Windows 上偶尔会被忽略
                place(&w);
                round_corners(&w, crate::theme::is_dark(&pref));
                crate::mark(&format!("浮窗：显示结果 {shown:?}"));
            }
        })
        .build();

    if let Err(e) = built {
        crate::mark(&format!("浮窗：建窗口失败 {e}"));
    } else {
        crate::mark("浮窗：窗口已创建，等页面加载");
    }
}

pub fn hide(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window(FLOAT_ID) {
        let _ = w.hide();
    }
}

/// 放到主屏右下角、任务栏上方。另一张卡片亮着时叠在它上面。用 primary_monitor 而不是当前窗口所在的屏幕：窗口刚建出来还没定位过
fn place(w: &WebviewWindow) {
    let monitor = w
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| w.current_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        crate::mark("浮窗：拿不到显示器信息，用默认位置");
        return;
    };
    let scale = monitor.scale_factor();
    let size = monitor.size();
    let pos = monitor.position();
    let _ = (size, pos);
    let is_done = w.label() == DONE_ID;
    let (card_w, card_h) = if is_done { (DONE_W, DONE_H) } else { (FLOAT_W, FLOAT_H) };
    let other = w.app_handle().get_webview_window(if is_done { FLOAT_ID } else { DONE_ID });
    let lift = other
        .filter(|o| o.is_visible().unwrap_or(false))
        .and_then(|o| o.outer_size().ok())
        .map_or(0.0, |s| s.height as f64);
    let area = monitor.work_area();
    let w_px = card_w * scale;
    let h_px = card_h * scale;
    let x = area.position.x as f64 + area.size.width as f64 - w_px - MARGIN_X * scale;
    let y = area.position.y as f64 + area.size.height as f64 - h_px - MARGIN_Y * scale - lift;
    // 尺寸和位置一起设：隐藏状态创建出来的窗口，位置有时不会被记住
    let _ = w.set_size(tauri::LogicalSize::new(card_w, card_h));
    let r = w.set_position(tauri::PhysicalPosition::new(x.round() as i32, y.round() as i32));
    crate::mark(&format!(
        "浮窗：定位到 ({}, {}) 尺寸 {}x{} scale={scale} 结果={:?}",
        x.round(),
        y.round(),
        w_px.round(),
        h_px.round(),
        r.err()
    ));
}

/// 让无边框窗口看起来像一张圆角卡片。
///
/// 两步：DWM 只管描边颜色（深色主题下默认那道白边很扎眼），圆角用窗口区域剪。
/// 只留 DWM 两件事：深色模式描边、去掉系统那圈 1px 窗口描边。
/// 圆角和阴影由 float.html 的 CSS 提供（透明窗口 + border-radius + box-shadow）。
/// 历史教训：SetWindowRgn 硬剪没有抗锯齿（锯齿），还会把阴影剪成灰毛边；
/// 而「透明窗口渲染成空白/黑屏」的记录是测试截图越界造成的误判，实际渲染正常。
fn round_corners(w: &WebviewWindow, dark: bool) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
    };

    let Ok(h) = w.hwnd() else { return };
    let hwnd = h.0 as _;

    let dark_mode: i32 = i32::from(dark);
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &dark_mode as *const i32 as *const core::ffi::c_void,
            4,
        );
        let no_border: u32 = DWMWA_COLOR_NONE;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR as u32,
            &no_border as *const u32 as *const core::ffi::c_void,
            4,
        );
    }
}

/// 把链接加工成界面要的三行字：标题、类型、存到哪儿
fn decorate(app: &tauri::AppHandle, links: &[String]) -> Vec<FloatItem> {
    let state = app.state::<AppState>();
    let settings: Settings = match state.settings.lock() {
        Ok(s) => s.clone(),
        Err(_) => return Vec::new(),
    };
    links
        .iter()
        .map(|url| {
            let kind = crate::links::LinkKind::of(url);
            let (label, dir) = match kind {
                Some(k) => {
                    let d = settings.default_dir(k, &state.folders);
                    (k.label(), folder_name(&d))
                }
                None => ("下载", String::new()),
            };
            FloatItem {
                title: crate::links::pretty_title(url),
                url: url.clone(),
                kind: crate::api::download_label(url).unwrap_or_else(|| label.to_string()),
                dir,
            }
        })
        .collect()
}

/// 只取最后一段目录名，「存到「电影」」比一长串绝对路径好读
fn folder_name(p: &std::path::Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.to_string_lossy().to_string())
}

// ---------- 给浮窗页面用的命令 ----------

/// 浮窗页面每次显示都来拉一次：比 emit 可靠（页面加载时机不确定）
#[tauri::command]
pub fn float_pending() -> Vec<FloatItem> {
    pending().lock().map(|v| v.clone()).unwrap_or_default()
}

#[tauri::command]
pub fn float_action(app: tauri::AppHandle, action: String) -> Result<(), String> {
    let items = {
        let mut p = pending().lock().map_err(|e| e.to_string())?;
        if action == "download" {
            std::mem::take(&mut *p)
        } else {
            p.clear();
            Vec::new()
        }
    };
    hide(&app);
    if items.is_empty() {
        return Ok(());
    }
    let urls: Vec<String> = items.into_iter().map(|i| i.url).collect();
    // 交给主界面去投任务：目录策略、确认框、列表刷新都在那边，浮窗不重复实现一遍
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.unminimize();
        let _ = main.set_focus();
        let _ = main.emit("float-download", urls);
    }
    Ok(())
}

// ---------------- 下载完成卡片 ----------------

#[derive(Clone, Serialize)]
pub struct DoneItem {
    pub id: String,
    pub title: String,
    pub more: usize,
    /// 不是下载完成、而是润色这类耗时操作做完时的标题（「字幕已润色」）
    pub heading: Option<String>,
}

fn done_now() -> &'static Mutex<Option<DoneItem>> {
    static NOW: OnceLock<Mutex<Option<DoneItem>>> = OnceLock::new();
    NOW.get_or_init(|| Mutex::new(None))
}

const ACCOUNT_NOTICE_PREFIX: &str = "__cloud_account__:";
const UPDATE_NOTICE_PREFIX: &str = "__windows_update__:";

pub fn notify_update(app: &tauri::AppHandle, version: &str) {
    let handle = app.clone();
    let version = version.to_owned();
    let _ = app.run_on_main_thread(move || {
        let edition = if cfg!(feature = "nas") { " NAS 版" } else { "" };
        show_done(
            &handle, format!("{UPDATE_NOTICE_PREFIX}{version}"),
            format!("闪电全能下载{edition} {version}"), Some("发现新版本".into()),
        );
    });
}

fn account_notice_id(provider: &str, nas: bool) -> String {
    let target = if nas { "nas" } else { "local" };
    format!("{ACCOUNT_NOTICE_PREFIX}{target}:{provider}")
}

fn account_notice_times() -> &'static Mutex<std::collections::HashMap<String, std::time::Instant>> {
    static TIMES: OnceLock<Mutex<std::collections::HashMap<String, std::time::Instant>>> = OnceLock::new();
    TIMES.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

pub fn update_cloud_account_notice(app: &tauri::AppHandle, provider: &str, name: &str, nas: bool, state: &str) {
    if state == "ready" {
        clear_cloud_account_notice(app, provider, nas);
    } else if state == "expired" {
        let id = account_notice_id(provider, nas);
        let target = if nas { "NAS" } else { "本机" };
        let heading = format!("{name}关联失效");
        let app = app.clone();
        let _ = app.clone().run_on_main_thread(move || {
            let mut times = account_notice_times().lock().unwrap();
            if times.get(&id).is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(6 * 3600)) {
                return;
            }
            times.insert(id.clone(), std::time::Instant::now());
            drop(times);
            show_done(&app, id, format!("请重新关联{target}账号"), Some(heading));
        });
    }
}

pub fn clear_cloud_account_notice(app: &tauri::AppHandle, provider: &str, nas: bool) {
    let id = account_notice_id(provider, nas);
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        account_notice_times().lock().unwrap().remove(&id);
        let mut current = done_now().lock().unwrap();
        if current.as_ref().is_some_and(|item| item.id == id) {
            *current = None;
            if let Some(window) = app.get_webview_window(DONE_ID) {
                let _ = window.hide();
            }
        }
    });
}

static COLLECTION_MEMBERS: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[tauri::command]
pub fn sync_collection_members(ids: Vec<String>) {
    *COLLECTION_MEMBERS.lock().unwrap() = ids;
}

fn is_collection_member(id: &str) -> bool {
    COLLECTION_MEMBERS.lock().unwrap().iter().any(|member| member == id)
}

#[tauri::command]
pub fn notify_collection(app: tauri::AppHandle, id: String, title: String, heading: Option<String>) {
    if id.starts_with("grp:") {
        show_done(&app, id, title, heading);
    }
}

/// 润色等耗时操作做完：右下角弹同一张卡片（heading 是标题，title 是视频名）
pub fn notify_task(id: String, heading: String, title: String) {
    if is_collection_member(&id) { return; }
    if let Some(app) = crate::engine::APP.get() {
        let app = app.clone();
        let main = app.clone();
        let _ = main.run_on_main_thread(move || {
            done_now().lock().unwrap().take();
            show_done(&app, id, title, Some(heading));
        });
    }
}

/// 弹完成卡片。上一张还亮着时换成新的，并记上「另有 n 项」
fn show_done(app: &tauri::AppHandle, id: String, title: String, heading: Option<String>) {
    let visible = app.get_webview_window(DONE_ID).map_or(false, |w| w.is_visible().unwrap_or(false));
    {
        let mut now = done_now().lock().unwrap();
        let more = if visible { now.as_ref().map_or(0, |d| d.more + 1) } else { 0 };
        *now = Some(DoneItem { id, title, more: if heading.is_some() { 0 } else { more }, heading });
    }
    if let Some(w) = app.get_webview_window(DONE_ID) {
        let _ = w.emit("done-data", ());
        let _ = w.show();
        place(&w);
        return;
    }
    let pref = crate::theme::preference(app);
    let url = WebviewUrl::App("done.html".into());
    let built = WebviewWindowBuilder::new(app, DONE_ID, url)
        .title("下载完成")
        .inner_size(DONE_W, DONE_H)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .initialization_script(crate::theme::init(&pref))
        .additional_browser_args(crate::BROWSER_ARGS)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .visible(false)
        .on_page_load(move |w, payload| {
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                let pref = crate::theme::preference(w.app_handle());
                crate::theme::apply_view(&w, &pref);
                let _ = w.set_focusable(false);
                if done_now().lock().unwrap().is_some() {
                    let _ = w.show();
                    place(&w);
                }
                round_corners(&w, crate::theme::is_dark(&pref));
            }
        })
        .build();
    if let Err(e) = built {
        crate::mark(&format!("完成卡片：建窗口失败 {e}"));
    }
}

/// 所有来源（内核、视频、迅雷、云盘）统一在这里看完成。启动时已经完成的不算
pub fn start_done_watcher(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut known: Option<std::collections::HashSet<String>> = None;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            let st = app.state::<AppState>();
            st.videos.retry_limited_subtitles(&st.tools.dir);
            let Ok(list) = crate::engine::collect_tasks(&st).await else { continue };
            // 内核刚启动时列表是空的，不能拿来当基准，否则之后所有老任务都会被当成刚下完
            if list.is_empty() && known.is_none() {
                continue;
            }
            let done: Vec<_> = list.iter().filter(|t| t.status == "done").collect();
            let Some(seen) = known.as_mut() else {
                known = Some(done.iter().map(|t| t.id.clone()).collect());
                continue;
            };
            let on = st.settings.lock().unwrap().notify_on_finish;
            for t in done {
                if seen.insert(t.id.clone()) && on && !is_collection_member(&t.id)
                    && !t.id.strip_prefix("v:").and_then(|id| st.videos.find(id)).is_some_and(|job| job.options.collection_member)
                {
                    let title = t.name.clone().filter(|n| !n.is_empty())
                        .or_else(|| t.meta.as_ref().and_then(|m| m.res.as_ref()).map(|r| r.name.clone()))
                        .unwrap_or_else(|| t.url());
                    show_done(&app, t.id.clone(), title, None);
                }
            }
        }
    });
}

#[tauri::command]
pub fn done_pending() -> Option<DoneItem> {
    done_now().lock().unwrap().clone()
}

#[tauri::command]
pub fn done_close(app: tauri::AppHandle) {
    *done_now().lock().unwrap() = None;
    if let Some(w) = app.get_webview_window(DONE_ID) {
        let _ = w.hide();
    }
}

/// 打开文件或文件夹，然后收起卡片
#[tauri::command]
pub async fn done_action(app: tauri::AppHandle, action: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let id = done_now().lock().unwrap().as_ref().map(|d| d.id.clone()).ok_or("任务不在了")?;
    if id.starts_with(UPDATE_NOTICE_PREFIX) {
        return Err("请使用卡片上的更新按钮".into());
    }
    if let Some((target, provider)) = id.strip_prefix(ACCOUNT_NOTICE_PREFIX).and_then(|id|id.split_once(':')) {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
            let _ = w.emit("cloud-account-relink", serde_json::json!({"provider":provider,"target":target}));
        }
        done_close(app);
        return Ok(());
    }
    if id.starts_with("grp:") {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
            let _ = window.emit("collection-notice-open", serde_json::json!({ "id": id, "action": action }));
        }
        done_close(app);
        return Ok(());
    }
    let r = if action == "open" {
        crate::engine::item_open(id, state).await
    } else {
        crate::engine::item_reveal(id, state).await
    };
    done_close(app);
    r
}

/// 用户点了浮窗里的「忽略」或者按了 Esc
#[tauri::command]
pub fn float_close(app: tauri::AppHandle) {
    if let Ok(mut p) = pending().lock() {
        p.clear();
    }
    hide(&app);
}
