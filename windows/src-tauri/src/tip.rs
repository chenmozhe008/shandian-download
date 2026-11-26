// 悬浮说明小窗：主窗口里的 position:fixed 会被 WebView 裁在窗口内，
// 贴边时提示像狗皮膏药。这里用独立的置顶小窗画 tip，鼠标在哪就能伸到哪，
// 不受主窗口边界限制。复用一个窗口，只改位置和文案。

use tauri::{Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const TIP_ID: &str = "tip";
const TIP_W: f64 = 280.0;
const TIP_H: f64 = 48.0;
/// 卡片四周留 10px 透明边（阴影 + 箭头）
const PAD: f64 = 10.0;

/// 显示提示。cx/cy 是**屏幕逻辑坐标**（主窗口 outer_position + 页面 client 坐标）。
/// 箭头朝下：小窗放在光标上方，卡片底边贴着光标。
#[tauri::command]
pub fn tip_show(app: tauri::AppHandle, text: String, cx: f64, cy: f64) -> Result<(), String> {
    if text.trim().is_empty() {
        return tip_hide(app);
    }
    let pref = crate::theme::preference(&app);
    let w = ensure_window(&app, &pref)?;
    crate::theme::apply_view(&w, &pref);
    let _ = w.emit("tip-data", serde_json::json!({ "text": text }));

    // 卡片中心对齐 cx；底边在 cy 上方一点，箭头探下去够到光标
    let x = cx - TIP_W / 2.0;
    let y = cy - TIP_H - 6.0;
    // 放到物理像素，和主窗口同一套 scale
    let scale = w.scale_factor().unwrap_or(1.0);
    let _ = w.set_size(tauri::LogicalSize::new(TIP_W, TIP_H));
    let _ = w.set_position(tauri::PhysicalPosition::new(
        (x * scale).round() as i32,
        (y * scale).round() as i32,
    ));
    let _ = w.set_focusable(false);
    let _ = w.show();
    let _ = w.set_focusable(false);
    Ok(())
}

#[tauri::command]
pub fn tip_hide(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(TIP_ID) {
        let _ = w.hide();
    }
    Ok(())
}

fn ensure_window(app: &tauri::AppHandle, pref: &str) -> Result<WebviewWindow, String> {
    if let Some(w) = app.get_webview_window(TIP_ID) {
        return Ok(w);
    }
    let url = WebviewUrl::App("tip.html".into());
    let built = WebviewWindowBuilder::new(app, TIP_ID, url)
        .title("提示")
        // 宽高含四周 10px 透明边
        .inner_size(TIP_W + PAD * 2.0, TIP_H + PAD * 2.0)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .initialization_script(crate::theme::init(pref))
        .additional_browser_args(crate::BROWSER_ARGS)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .visible(false)
        .on_page_load(move |w, payload| {
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                let _ = w.set_focusable(false);
            }
        })
        .build()
        .map_err(|e| e.to_string())?;
    Ok(built)
}
