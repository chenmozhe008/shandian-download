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

fn pipette_size(scale: f64) -> i32 {
    (24.0 * scale).round().clamp(24.0, 128.0) as i32
}

struct PickerMonitor {
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    cursor_size: i32,
}

fn pipette_size_at(
    monitors: &[PickerMonitor], point: windows_sys::Win32::Foundation::POINT, fallback: i32,
) -> i32 {
    monitors.iter().find(|monitor| {
        // Tauri monitor bounds and GetCursorPos use physical desktop coordinates.
        let x = i64::from(point.x) - i64::from(monitor.left);
        let y = i64::from(point.y) - i64::from(monitor.top);
        (0..i64::from(monitor.width)).contains(&x) && (0..i64::from(monitor.height)).contains(&y)
    }).map_or(fallback, |monitor| monitor.cursor_size)
}

/// Load the antialiased toolbar pipette artwork at the requested physical size.
fn pipette_cursor(size: i32, sample: u32) -> windows_sys::Win32::UI::WindowsAndMessaging::HCURSOR {
    use windows_sys::Win32::UI::WindowsAndMessaging::CreateIconFromResourceEx;

    // The CUR file was rendered from the color picker's SVG and has a hotspot at the pipette tip.
    const ART: &[u8] = include_bytes!("pipette.cur");
    let frame_count = u16::from_le_bytes([ART[4], ART[5]]) as usize;
    let entries = ART[6..6 + frame_count * 16].chunks_exact(16);
    let Some(frame) = entries.min_by_key(|entry| {
        let width = i32::from(entry[0]);
        (width < size, (width - size).abs())
    }) else {
        return std::ptr::null_mut();
    };
    let length = u32::from_le_bytes(frame[8..12].try_into().unwrap()) as usize;
    let offset = u32::from_le_bytes(frame[12..16].try_into().unwrap()) as usize;

    // Win32 expects a DWORD-aligned RT_CURSOR resource with the hotspot preceding the bitmap.
    let mut resource: Vec<u32> = std::iter::once(u32::from_le_bytes(frame[4..8].try_into().unwrap()))
        .chain(ART[offset..offset + length].chunks_exact(4).map(|word| {
            u32::from_le_bytes(word.try_into().unwrap())
        }))
        .collect();

    // The first word is the hotspot, then the 40-byte DIB header. Magenta marks
    // the tintable fill; neutral stroke pixels keep their original color and alpha.
    let frame_size = usize::from(frame[0]);
    let (red, green, blue) = (sample & 255, (sample >> 8) & 255, (sample >> 16) & 255);
    for pixel in &mut resource[11..11 + frame_size * frame_size] {
        let base = (*pixel >> 8) & 255;
        let coverage = ((*pixel >> 16) & 255).saturating_sub(base);
        if coverage != 0 {
            let mix = |channel| base + (coverage * channel + 127) / 255;
            *pixel = (*pixel & 0xff000000) | (mix(red) << 16) | (mix(green) << 8) | mix(blue);
        }
    }
    unsafe {
        CreateIconFromResourceEx(
            resource.as_ptr().cast(),
            (resource.len() * 4) as u32,
            0,
            0x00030000,
            size,
            size,
            0,
        )
    }
}

/// Capture a single left click for the screen picker on the installing thread.
#[tauri::command]
pub async fn pick_screen_color(window: tauri::WebviewWindow) -> Result<Option<String>, String> {
    let fallback = pipette_size(window.scale_factor().unwrap_or(1.0));
    let monitors: Vec<_> = window.available_monitors().unwrap_or_default().into_iter()
        .map(|monitor| PickerMonitor {
            left: monitor.position().x,
            top: monitor.position().y,
            width: monitor.size().width,
            height: monitor.size().height,
            cursor_size: pipette_size(monitor.scale_factor()),
        })
        .collect();
    tauri::async_runtime::spawn_blocking(move || pick_screen_color_blocking(&monitors, fallback))
        .await
        .map_err(|_| "取色线程未能完成".to_string())?
}

fn pick_screen_color_blocking(monitors: &[PickerMonitor], fallback: i32) -> Result<Option<String>, String> {
    use std::{cell::Cell, ptr::null_mut, sync::Mutex, thread, time::{Duration, Instant}};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        Graphics::{Dwm::DwmFlush, Gdi::{GetDC, GetPixel, GetStockObject, ReleaseDC, BLACK_BRUSH}},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::*,
    };

    static PICKER: Mutex<()> = Mutex::new(());
    let _active = PICKER.try_lock().map_err(|_| "取色已在进行".to_string())?;
    thread_local! {
        static CLICK: Cell<Option<POINT>> = const { Cell::new(None) };
    }
    unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        if msg == WM_LBUTTONUP {
            let mut point = POINT { x: 0, y: 0 };
            if GetCursorPos(&mut point) != 0 { CLICK.set(Some(point)); }
            return 0;
        }
        DefWindowProcW(hwnd, msg, w, l)
    }

    let class: Vec<u16> = "ShandianScreenColorPicker\0".encode_utf16().collect();
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let mut point = POINT::default();
    let mut size = if unsafe { GetCursorPos(&mut point) } != 0 {
        pipette_size_at(monitors, point, fallback)
    } else {
        fallback
    };
    let mut preview_color = 0x006e6e6e;
    let mut cursor = pipette_cursor(size, preview_color);
    if cursor.is_null() { return Err("无法创建吸管光标".into()); }
    let registered = unsafe {
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: cursor,
            hbrBackground: GetStockObject(BLACK_BRUSH),
            lpszClassName: class.as_ptr(),
            ..Default::default()
        })
    };
    if registered == 0 {
        unsafe { DestroyCursor(cursor); }
        return Err("无法启动 Windows 取色".into());
    }

    let overlay = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(), std::ptr::null(), WS_POPUP,
            GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN),
            null_mut(), null_mut(), instance, std::ptr::null(),
        )
    };
    let point = if !overlay.is_null() && unsafe { SetLayeredWindowAttributes(overlay, 0, 1, LWA_ALPHA) } != 0 {
        CLICK.set(None);
        unsafe {
            ShowWindow(overlay, SW_SHOWNOACTIVATE);
            SetCursor(cursor);
        }
        let until = Instant::now() + Duration::from_secs(15);
        let mut msg = MSG::default();
        let preview_dc = unsafe { GetDC(null_mut()) };
        let mut next_preview = Instant::now();
        let picked = loop {
            unsafe {
                while PeekMessageW(&mut msg, overlay, 0, 0, PM_REMOVE) != 0 {
                    DispatchMessageW(&msg);
                }
            }
            if let Some(point) = CLICK.get() { break Ok(Some(point)); }
            if Instant::now() >= until || unsafe { GetAsyncKeyState(VK_ESCAPE.0.into()) } < 0 {
                break Ok(None);
            }
            let now = Instant::now();
            if now >= next_preview {
                next_preview = now + Duration::from_millis(33);
                let mut point = POINT { x: 0, y: 0 };
                if unsafe { GetCursorPos(&mut point) } != 0 {
                    let next_size = pipette_size_at(monitors, point, size);
                    let mut color = preview_color;
                    if !preview_dc.is_null() {
                        let sample = unsafe { GetPixel(preview_dc, point.x, point.y) };
                        if sample != u32::MAX { color = sample; }
                    }
                    if next_size != size || color != preview_color {
                        let next = pipette_cursor(next_size, color);
                        if !next.is_null() {
                            unsafe {
                                if SetClassLongPtrW(overlay, GCLP_HCURSOR, next as isize) == 0 {
                                    DestroyCursor(next);
                                } else {
                                    SetCursor(next);
                                    DestroyCursor(cursor);
                                    cursor = next;
                                    size = next_size;
                                    preview_color = color;
                                }
                            }
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(8));
        };
        if !preview_dc.is_null() {
            unsafe { ReleaseDC(null_mut(), preview_dc); }
        }
        picked
    } else {
        Err("无法显示取色层")
    };
    unsafe {
        if !overlay.is_null() { DestroyWindow(overlay); }
        UnregisterClassW(class.as_ptr(), instance);
        if GetCursor() == cursor { SetCursor(LoadCursorW(null_mut(), IDC_ARROW)); }
        DestroyCursor(cursor);
    }

    let Some(point) = point? else { return Ok(None) };
    let color = unsafe {
        // Let the compositor remove the picker before sampling the underlying desktop.
        DwmFlush();
        let dc = GetDC(null_mut());
        if dc.is_null() { return Err("无法读取屏幕颜色".into()); }
        let color = GetPixel(dc, point.x, point.y);
        ReleaseDC(null_mut(), dc);
        color
    };
    if color == u32::MAX { return Err("无法读取屏幕颜色".into()); }
    Ok(Some(format!("#{:02X}{:02X}{:02X}", color & 255, (color >> 8) & 255, (color >> 16) & 255)))
}

#[cfg(test)]
mod picker_tests {
    use super::*;
    use windows_sys::Win32::{
        Foundation::POINT,
        Graphics::Gdi::{BITMAP, DeleteObject, GetObjectW},
        UI::WindowsAndMessaging::{DestroyCursor, GetIconInfo, ICONINFO},
    };

    #[test]
    fn cursor_size_follows_the_pointer_across_scaled_displays() {
        let monitors = [
            PickerMonitor { left: 0, top: 0, width: 1920, height: 1080, cursor_size: pipette_size(1.0) },
            PickerMonitor { left: -3840, top: -220, width: 3840, height: 2160, cursor_size: pipette_size(2.0) },
        ];
        assert_eq!(pipette_size_at(&monitors, POINT { x: 0, y: 500 }, 24), 24);
        assert_eq!(pipette_size_at(&monitors, POINT { x: -1, y: 500 }, 24), 48);
        assert_eq!(pipette_size_at(&monitors, POINT { x: -3840, y: -220 }, 24), 48);
        assert_eq!(pipette_size_at(&monitors, POINT { x: 50, y: -1 }, 24), 24);
    }

    #[test]
    fn native_cursor_retains_tip_at_high_dpi() {
        for (scale, expected_size, tip) in [(1.0, 24, (5, 20)), (1.5, 36, (7, 30)), (2.0, 48, (9, 40))] {
            let cursor = pipette_cursor(pipette_size(scale), 0x004080c0);
            assert!(!cursor.is_null());
            let mut info = ICONINFO::default();
            let mut bitmap = BITMAP::default();
            let loaded = unsafe { GetIconInfo(cursor, &mut info) };
            let bytes = unsafe {
                let bytes = GetObjectW(info.hbmColor, std::mem::size_of::<BITMAP>() as i32, (&mut bitmap as *mut BITMAP).cast());
                DeleteObject(info.hbmColor);
                DeleteObject(info.hbmMask);
                DestroyCursor(cursor);
                bytes
            };
            assert_ne!(loaded, 0);
            assert_ne!(bytes, 0);
            assert_eq!((bitmap.bmWidth, bitmap.bmHeight), (expected_size, expected_size));
            assert_eq!((info.xHotspot, info.yHotspot), tip);
        }
    }
}
