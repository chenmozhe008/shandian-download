// 新建框、确认框的独立窗口：能拖出主窗口、有系统关闭按钮。
//
// 窗口里跑的还是 index.html（?dialog=1），只负责让用户填；真正投任务和排队都回主窗口做。
// 用户点系统的关闭按钮时页面来不及说话，所以由这里发 dialog-closed，主窗口当取消处理。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::AppState;

const NEW_LABEL: &str = "dialog-new";

// 内容可能带请求头和 Cookie，不放进窗口地址，窗口打开后自己来取
fn payloads() -> &'static Mutex<HashMap<String, serde_json::Value>> {
    static P: OnceLock<Mutex<HashMap<String, serde_json::Value>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

// 必须是 async：同步命令里建 WebView2 窗口会卡死
#[tauri::command]
pub async fn open_dialog(
    app: tauri::AppHandle,
    kind: String,
    payload: serde_json::Value,
) -> Result<String, String> {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    let is_new = kind == "new";
    let wechat_ticket = payload["opts"]["wechatTicket"].as_str().map(String::from);
    // 新建框只开一个；确认框每条一个，互不挡
    let label = if is_new {
        NEW_LABEL.to_string()
    } else {
        format!("dialog-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    };
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(label);
    }
    payloads().lock().unwrap().insert(
        label.clone(),
        serde_json::json!({ "kind": kind, "payload": payload }),
    );
    let pref = crate::theme::preference(&app);
    let closed = app.clone();
    let closed_label = label.clone();
    let main = app.get_webview_window("main").ok_or("主窗口不在")?;
    let _ = main.unminimize();
    let _ = main.show();
    // 放在主窗口中央，不是屏幕中央（多屏、主窗口靠边时差很远）
    // 字幕样式窗口：预览 + 一列选项，默认高一些，可以自己拖大小
    let sub_style = kind == "substyle";
    let scale = main.scale_factor().unwrap_or(1.0);
    // 字幕样式：选项占了一大半高度，窗口小了预览画面就很小（11-13 用户反馈）。按屏幕可用高度尽量放大
    let screen_h = main.current_monitor().ok().flatten().map(|m| m.size().height as f64 / m.scale_factor()).unwrap_or(1080.0);
    let (width, height) = if sub_style { (660.0, 900.0_f64.min(screen_h - 90.0).max(640.0)) } else if is_new { (720.0, 280.0) } else { (720.0, 420.0) };
    let (pos, size) = (main.outer_position().map_err(|e| e.to_string())?, main.outer_size().map_err(|e| e.to_string())?);
    let x = pos.x as f64 / scale + (size.width as f64 / scale - width) / 2.0;
    let y = pos.y as f64 / scale + (size.height as f64 / scale - height) / 2.0;
    let window = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("index.html?dialog=1".into()))
        // 从属于主窗口：总在主窗口上面，主窗口仍可操作
        .parent(&main)
        .map_err(|e| e.to_string())?
        .title(if sub_style { "字幕样式" } else { "新建下载" })
        .inner_size(width, height)
        .position(x, y.max(0.0))
        .min_inner_size(480.0, 240.0)
        // 无系统标题栏，外观和主窗口的弹窗一样；只留页面里那个关闭按钮
        .decorations(false)
        .shadow(true)
        .always_on_top(!sub_style)
        .skip_taskbar(true)
        .visible(false)
        .theme(crate::theme::native(&pref))
        .initialization_script(crate::theme::init(&pref))
        // 必须和主窗口用同一串 WebView2 参数，否则共享环境建不起来
        .initialization_script(crate::theme::frame_init())
        .additional_browser_args(crate::BROWSER_ARGS)
        .build()
        .map_err(|e| e.to_string())?;
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            if let Some(ticket) = wechat_ticket.as_deref() { crate::wechat::cancel_pending(ticket); }
            payloads().lock().unwrap().remove(&closed_label);
            let _ = closed.emit_to("main", "dialog-closed", closed_label.clone());
        }
    });
    Ok(label)
}

#[tauri::command]
pub fn dialog_payload(window: tauri::WebviewWindow, state: tauri::State<'_, AppState>) -> Option<serde_json::Value> {
    let mut request = payloads().lock().unwrap().remove(window.label())?;
    let pref = state.settings.lock().unwrap().theme.clone();
    crate::theme::apply(&window, &pref);
    request["theme"] = crate::theme::normalize(&pref).into();
    if request["kind"] == "confirm" {
        let payload = &mut request["payload"];
        if let (Some(id), Some(url)) = (payload["opts"]["baiduRedownloadId"].as_str(), payload["url"].as_str()) {
            if let Some(code) = state.baidu.saved_code_for_redownload(id, url) {
                payload["opts"]["baiduSavedCode"] = serde_json::Value::String(code);
            }
        }
    }
    Some(request)
}
