// 窗口底色跟着主题走。
//
// 为什么需要这一层：WebView2 从进程起来到画出第一帧要 1.9–3.3 秒，这段时间窗口
// 显示的是 tauri.conf.json 里写死的 backgroundColor（浅色）。深色用户就会先看到
// 一块白板再变黑，非常刺眼。窗口在 setup 之前就被创建好了，所以在 setup 里把
// 底色和系统主题一起刷成对应的值，第一帧出来就是对的。
//
// 手动切主题时前端会调 set_app_theme，走同一个函数，保证「切完立刻切、下次打开也记得」。

use tauri::window::Color;
use tauri::{AppHandle, Manager, WebviewWindow};

/// 浅色底：和 style.css 的 --window 一致（rgb(251,250,248)）
pub const LIGHT_BG: Color = Color(251, 250, 248, 255);
/// 深色底：和 style.css 深色 --window 一致（rgb(27,27,29)）
pub const DARK_BG: Color = Color(27, 27, 29, 255);

/// 系统是不是深色模式：读注册表。读不到就当浅色（中文 Windows 默认都是浅色）
pub fn system_dark() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .and_then(|k| k.get_value::<u32, _>("AppsUseLightTheme"))
        .map(|light| light == 0)
        .unwrap_or(false)
}

/// 三种偏好（light / dark / system）折算成实际深浅
pub fn is_dark(pref: &str) -> bool {
    match pref {
        "dark" => true,
        "light" => false,
        _ => system_dark(),
    }
}

pub fn background(pref: &str) -> Color {
    if is_dark(pref) {
        DARK_BG
    } else {
        LIGHT_BG
    }
}

pub fn preference(app: &AppHandle) -> String {
    app.state::<crate::AppState>().settings.lock().unwrap().theme.clone()
}

pub fn normalize(pref: &str) -> &str {
    match pref {
        "light" | "dark" => pref,
        _ => "system",
    }
}

pub fn native(pref: &str) -> Option<tauri::Theme> {
    match pref {
        "light" => Some(tauri::Theme::Light),
        "dark" => Some(tauri::Theme::Dark),
        _ => None,
    }
}

pub fn init(pref: &str) -> String {
    format!("window.__sdTheme = '{}';", normalize(pref))
}

pub fn apply_view(window: &WebviewWindow, pref: &str) {
    let _ = window.eval(format!("{}window.__sdSetTheme?.(window.__sdTheme);", init(pref)));
}

/// 用户手动切主题时用：标题栏和窗口底色一起改（下次启动由建窗口时传入的值接管）
pub fn apply(window: &WebviewWindow, pref: &str) {
    let _ = window.set_theme(native(pref));
    // Win11 保留透明底色，由系统统一裁剪四角；Win10 继续使用不透明底色。
    if legacy_frame() {
        let _ = window.set_background_color(Some(background(pref)));
    }
}

/// Win11 从 build 22000 开始；不能用浏览器的 Windows NT 10.0 区分。
pub fn legacy_frame() -> bool {
    use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};
    let build = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .and_then(|key| key.get_value::<String, _>("CurrentBuildNumber"))
        .ok().and_then(|v| v.parse::<u32>().ok());
    build.map_or(true, |n| n < 22000)
}

pub fn frame_init() -> String {
    format!("window.__sdWindowFrame = '{}';", if legacy_frame() { "square" } else { "rounded" })
}
