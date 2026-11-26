// 托盘图标。Mac 版是 NSStatusItem，Windows 上是 Shell_NotifyIcon（Tauri 的 TrayIcon）。
//
// 两个平台的行为差异就体现在这里：
//   - Windows 用户习惯「关窗口 = 收到托盘继续下」而不是退出，所以 CloseRequested 只隐藏
//   - 托盘提示要显示实时总速度，所以 shell.rs 的后台循环每秒推一次文字进来
//   - 退出只能从托盘菜单走，避免误关把正在下的任务丢了

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;

pub const TRAY_ID: &str = "shandian-tray";

/// 彻底退出：先停内核（Windows 上父进程没了，子进程会一直挂着），再退 App。
/// 关窗口只是收进托盘，托盘「退出」和 Ctrl+Q 都走这里。
pub fn quit(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.baidu.stop();
        let _ = state.engine.lock().map(|mut e| e.stop());
    }
    app.exit(0);
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "打开主窗口", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "暂停全部", true, None::<&str>)?;
    let resume = MenuItem::with_id(app, "resume", "继续全部", true, None::<&str>)?;
    let open_dir = MenuItem::with_id(app, "folder", "打开下载目录", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &sep1, &pause, &resume, &open_dir, &sep2, &quit])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        // Windows 上左键单击默认弹菜单，这里改成「左键开窗口、右键弹菜单」，
        // 和绝大多数下载工具的直觉一致
        .show_menu_on_left_click(false)
        .tooltip("闪电全能下载")
        .on_menu_event(|app, ev| match ev.id.as_ref() {
            "show" => reveal(app),
            "pause" => all_tasks(app, true),
            "resume" => all_tasks(app, false),
            "folder" => open_downloads(app),
            "quit" => crate::tray::quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                reveal(tray.app_handle());
            }
        });

    // default_window_icon 来自 tauri.conf 里配的 icons/icon.ico，
    // 托盘用它同一套（内嵌 16/24/32/48 多尺寸，系统自己挑）
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}

pub fn reveal(app: &AppHandle) {
    crate::mark("主窗口恢复：收到请求");
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || {
        let Some(w) = handle.get_webview_window("main") else { crate::mark("主窗口恢复：窗口不存在"); return };
        crate::mark(&format!("主窗口恢复前：visible={:?}, minimized={:?}",w.is_visible(),w.is_minimized()));
        if let Err(e) = w.show() { crate::mark(&format!("主窗口显示失败：{e}")); }
        if let Err(e) = w.unminimize() { crate::mark(&format!("主窗口取消最小化失败：{e}")); }
        #[cfg(windows)]
        if let Ok(hwnd) = w.hwnd() {
            use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, ShowWindow, SetForegroundWindow, BringWindowToTop, SW_RESTORE, SW_SHOW};
            unsafe {
                let h = hwnd.0 as _;
                ShowWindow(h, if IsIconic(h) != 0 { SW_RESTORE } else { SW_SHOW });
                BringWindowToTop(h);
                SetForegroundWindow(h);
            }
        }
        // 外接显示器断开后，窗口可能仍在原来的屏幕坐标。
        if let (Ok(pos),Ok(size),Ok(monitors)) = (w.outer_position(),w.outer_size(),w.available_monitors()) {
            let intersects = monitors.iter().any(|m| {
                let p=m.position(); let s=m.size();
                (pos.x as i64) < p.x as i64+s.width as i64 && pos.x as i64+size.width as i64 > p.x as i64
                    && (pos.y as i64) < p.y as i64+s.height as i64 && pos.y as i64+size.height as i64 > p.y as i64
            });
            if !monitors.is_empty() && !intersects { let _ = w.center(); }
        }
        if let Err(e) = w.set_focus() { crate::mark(&format!("主窗口聚焦失败：{e}")); }
        crate::mark(&format!("主窗口恢复后：visible={:?}, minimized={:?}",w.is_visible(),w.is_minimized()));
    }) { crate::mark(&format!("主窗口恢复调度失败：{e}")); }
}

fn open_downloads(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let dir = {
        let s = state.settings.lock().unwrap().clone();
        s.dir_for(crate::links::LinkKind::Http, &state.folders)
    };
    let _ = crate::commands::open_dir(dir.to_string_lossy().into());
}

/// 暂停／继续所有任务：内核侧和 yt-dlp 侧一起处理
fn all_tasks(app: &AppHandle, pause: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // state 借用的是这里的 app（已 clone 成 'static），所以只能放在 async 块里取
        let Some(state) = app.try_state::<AppState>() else { return };
        let (port, token) = {
            match state.engine.lock() {
                Ok(e) => (e.port, e.token.clone()),
                Err(_) => return,
            }
        };
        let api = crate::api::Api::new(port, token);
        if let Ok(list) = api.list_tasks().await {
            for t in list {
                if pause {
                    if t.status == "running" || t.status == "wait" || t.status == "ready" {
                        let _ = api.pause(&t.id).await;
                    }
                } else if t.status == "pause" {
                    let _ = api.resume(&t.id).await;
                }
            }
        }
        for task in state.baidu.tasks(None) {
            if let Some(id) = task.id.strip_prefix("bd:") {
                if pause && task.status == "running" {
                    let _ = state.baidu.pause(id);
                } else if !pause && task.status == "pause" {
                    let _ = state.baidu.resume(id);
                }
            }
        }
        let bin = state.tools.dir.clone();
        for j in state.videos.list() {
            if pause {
                if j.state == "running" {
                    let _ = state.videos.pause(&j.id);
                }
            } else if j.state == "pause" {
                let _ = state.videos.resume(&j.id, &bin);
            }
        }
        // 界面在轮询 tasks，托盘操作完让它立刻刷新一次
        let _ = app.emit("tasks-changed", ());
    });
}

/// 托盘提示里的速度：有任务在下就写「1.2 MB/s · 2 个任务」，空闲就写名字
pub fn set_hint(app: &AppHandle, text: &str) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(text));
    }
}
