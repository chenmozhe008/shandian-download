// 闪电全能下载 —— Windows 客户端外壳
//
// 所有构建一律「窗口子系统」：双击启动绝不带控制台黑框。
// 调试要看日志：从终端里启动（debug 构建会接管父终端），或设
// SHANDIAN_DEBUG_CONSOLE=1 强制弹一个控制台出来。控制台只在调试时存在。
#![windows_subsystem = "windows"]
//
// 分四块：
//   engine.rs   管 gopeed.exe 子进程（起、停、健康检查、退出策略）
//   links.rs    链接识别与分类（移植自 Mac 版 Links.swift）
//   api.rs      跟内核 REST 说话（127.0.0.1:9999，X-Api-Token 鉴权）
//   settings.rs 偏好设置与目录策略（电影／视频／其他三类分开存）
//
// 前端是 ../ui 下的纯 HTML/CSS/JS，不引构建链。

mod api;
mod commands;
#[path = "../../shared/cloud_resume.rs"]
mod cloud_resume;
#[path = "../../shared/cloud.rs"]
mod cloud;
mod app_log;
mod cache;
#[path = "../../shared/browser_support.rs"]
mod browser_support;
mod dialog;
mod engine;
mod float;
mod gateway;
mod ghmirror;
mod links;
mod baidu;
mod feishu;
mod feishu_setup;
mod baidu_cleanup;
mod baidu_setup;
mod migrate;
mod mcp;
mod pikpak;
mod thunder;
mod proxy;
mod settings;
mod shell;
mod sources;
mod theme;
mod polish;
mod thumbs;
mod tools;
mod trash;
mod tray;
mod update;
mod video_files;
mod videos;
mod wechat;
mod browser_handoff;

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use tauri::{Emitter, Manager};

/// WebView2 的启动参数。
/// 关掉的都是本程序用不到的浏览器元件：PDF/Office 查看器和部分后台服务
/// （组件更新、同步）。少一个是一个，冷启动能快一点。
///
/// 注意：同一进程里所有窗口必须用**完全一样**的参数，否则第二个窗口会以
/// HRESULT(0x8007139F)「组或资源的状态不是执行请求操作的正确状态」创建失败——
/// 这是共享 WebView2 环境的限制，剪贴板浮窗就踩过。
pub const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI \
     --disable-component-update \
     --no-default-browser-check --disable-sync --no-first-run --no-proxy-server \
     --disable-http-cache";
// ↑ --disable-http-cache：换新版本后 WebView2 会拿缓存里的旧 index.html 顶着，
//   界面改了却看不到变化（之前「圆角调大了没反应」就是这么来的）。
//   页面都是随程序打包的本地文件，禁掉缓存不会影响加载速度。

/// 全局状态：内核句柄 + 设置 + 默认位置。
/// Tauri 的 manage 要求 Send + Sync，所以设置和内核都套 Mutex。
pub struct AppState {
    pub engine: Mutex<engine::Engine>,
    pub baidu: std::sync::Arc<baidu::Downloads>,
    /// 视频网站要用的 yt-dlp / ffmpeg（装在 local 的 bin 下）
    pub tools: tools::Tools,
    /// 视频网站下载任务（yt-dlp 子进程），和内核任务在界面上一视同仁
    pub videos: std::sync::Arc<videos::Videos>,
    pub settings: Mutex<settings::Settings>,
    /// 默认位置（不随设置变），用来算「恢复默认」和显示相对路径
    pub folders: settings::Folders,
    /// 用户配置（%APPDATA%）：设置、任务列表、下载历史
    pub base: PathBuf,
    /// 这台机器专用的（%LOCALAPPDATA%）：工具、任务库、封面、日志、登录信息。分工见 migrate.rs
    pub local: PathBuf,
}

/// 进程启动时刻。用来量「双击到界面可用」到底几秒——用户能感觉到的就是这一段，
/// 而不是窗口句柄什么时候建出来（那只要 100 ms）。
pub static STARTED_AT: OnceLock<Instant> = OnceLock::new();

/// 主窗口上一次的最大化状态。只在变化时通知页面，避免拖边框时每帧都发消息。
static MAXED: Mutex<bool> = Mutex::new(false);

/// 启动时间轴：把关键节点写进 startup.log，用来定位「启动要好几秒」到底花在哪。
/// 只在启动时写十几行，运行期不用。
pub fn mark(what: &str) {
    let ms = STARTED_AT.get().map(|t| t.elapsed().as_millis()).unwrap_or(0);
    let line = format!("[+{ms:>5} ms] {what}\n");
    app_log::record("app", &line);
}

/// 前端首帧画完后回报一次（毫秒）：顺手把窗口亮出来。
/// 之前是等 WebView2 的 PageLoad Finished 才 show，那个事件偶尔会拖到 3 秒开外
/// （子资源、WebView2 进程冷启动），用户看到的就是「点了图标半天不出窗口」。
/// 前端自己说了首帧已画完，这时候 show 既不闪白也不用等别人。
#[tauri::command]
fn ui_ready(window: tauri::WebviewWindow, ui_ms: u64) {
    let Some(t0) = STARTED_AT.get() else {
        let _ = window.show();
        return;
    };
    let total = t0.elapsed().as_millis();
    let _ = window.show();
    if window.label().starts_with("dialog-") { let _ = window.unminimize(); let _ = window.set_focus(); }
    let shown = t0.elapsed().as_millis();
    let line = format!(
        "启动：进程→首帧 {total} ms（前端自报导航起 {ui_ms} ms），窗口 {shown} ms 显示\n"
    );
    app_log::record("startup", &line);
    eprintln!("{line}");
}

fn dirs_of_log() -> Option<PathBuf> { app_log::directory().ok() }

/// panic 落到 panic.log。窗口子系统没有控制台，默认的 panic 输出（stderr）看不见，
/// 出事了至少有处可查；有调试控制台时同时也打一份到那边。
fn install_panic_log() {
    std::panic::set_hook(Box::new(|info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let loc = info.location().map(|l| l.to_string()).unwrap_or_default();
        let line = format!("panic: {msg}（{loc}）\n");
        app_log::record_panic(&line);
        eprintln!("{line}");
    }));
}

/// 控制台只在调试时存在：
/// - 从终端启动（PowerShell/cmd）→ 接管父终端，日志直接打进去，不另开窗；
/// - 设 SHANDIAN_DEBUG_CONSOLE=1 → 自开一个控制台（排查启动问题用）；
/// - 双击图标 → 两者皆无，和发版行为一致，没有黑框陪跑。
/// release 构建编译为空，什么都不做。
#[cfg(debug_assertions)]
fn debug_console() {
    use std::os::windows::io::IntoRawHandle as _;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        AllocConsole, AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS,
        STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    unsafe {
        let got = if std::env::var_os("SHANDIAN_DEBUG_CONSOLE").is_some() {
            AllocConsole() != 0
        } else {
            AttachConsole(ATTACH_PARENT_PROCESS) != 0
        };
        if !got {
            return;
        }
        // GUI 进程接上控制台后标准句柄仍是空的，重开 CONOUT$ 接给 stdout/stderr。
        // 原本已有句柄的（比如测试脚本用管道拉起）不动，免得截走管道输出。
        for h in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let cur = GetStdHandle(h);
            if !cur.is_null() && cur != INVALID_HANDLE_VALUE {
                continue;
            }
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
                SetStdHandle(h, f.into_raw_handle());
            }
        }
    }
}

#[cfg(not(debug_assertions))]
fn debug_console() {}

#[tauri::command]
fn link_kind(link: String) -> Option<String> {
    links::LinkKind::of(&link).map(|k| k.label().to_string())
}

#[tauri::command]
fn extract_links(text: String) -> Vec<String> {
    links::extract(&text)
}

fn main() {
    if std::env::args_os().len() == 2 && std::env::args_os().nth(1).is_some_and(|arg| arg == "--mcp") {
        mcp::run_stdio();
        return;
    }

    // 越早越好：这个时刻到首帧的差值就是用户感觉到的「启动要几秒」
    let _ = STARTED_AT.set(Instant::now());
    // 窗口子系统下 stderr 是无效句柄，panic 默认输出会直接丢失——落一份到 panic.log
    install_panic_log();
    // 上次自动更新改名留下的 *.old 旧文件，现在没人占着了，删掉
    update::cleanup_old_backups();
    // 控制台只在调试时存在：双击启动（debug / release 都一样）永远不带黑框
    debug_console();
    // WebView2 会把嵌入式资源（style.css 等）按 URL 落盘缓存：升级 exe 后它仍按旧缓存
    // 出图，界面样式「改了不生效」。界面全在本地，磁盘缓存没有意义，直接掐掉。
    if std::env::var_os("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").is_none() {
        std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", "--disk-cache-size=1");
    }
    mark("main 开始");

    // 单实例插件必须第一个注册：第二次双击图标（或从浏览器点磁力）时它让新进程
    // 立刻退出，把 argv 转给已经开着的窗口。否则两个进程会抢同一个内核数据目录。
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            for link in shell::links_from_args(&argv) {
                shell::push_pending(&link);
            }
            crate::tray::reveal(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        // 点「关闭」不退出：收进托盘继续下。真要退出走托盘菜单的「退出」。
        // 这是 Windows 下载工具的通行做法，也是 Mac 版的行为（关窗口 LPS 还在跑）。
        // WebView2 什么时候真的开始/结束加载页面：这是「窗口早就在了但一直白着」的分界
        .on_page_load(|w, payload| {
            match payload.event() {
                tauri::webview::PageLoadEvent::Started => mark("页面开始加载"),
                tauri::webview::PageLoadEvent::Finished => {
                    mark("页面加载完成");
                    // 兜底：正常路径是前端首帧回报（ui_ready）时亮窗口，
                    // 万一页面脚本没跑起来，这里也别一直藏着
                    let _ = w.window().show();
                }
            }
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // 新建框、确认框是真关，不收进托盘
                if window.label().starts_with("dialog-") {
                    return;
                }
                api.prevent_close();
                let _ = window.hide();
                // 第一次收起时告诉用户一声，免得以为程序没了
                use shell::NotifyExt;
                let _ = window.app_handle().notify_rust("还在后台下载", "窗口已收到托盘，右键托盘图标可以退出");
            }
        })
        .setup(|app| {
            mark("setup 进入");
            let base = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| PathBuf::from("."));
            let local = app.path().app_local_data_dir().unwrap_or_else(|_| base.clone());
            // 工具和内核都还没起，这时搬旧数据最安全
            migrate::run(&base, &local);
            update::cleanup_installers(local.clone());
            sources::init(&local);
            cache::apply_pending(&local);
            mark("数据目录就绪");

            let settings = settings::Settings::load(&base);
            let folders = settings::Folders::detect(&local);

            // 主窗口在这里手动创建，不用 tauri.conf.json 里那套静态配置。
            // 原因只有一个：底色要跟着主题走，而配置文件里的 backgroundColor 是写死的浅色。
            // WebView2 实例化要 2 秒左右，这期间窗口已经可见——深色用户会先看到一块白板。
            // 手动建窗口就能把深色底、深色标题栏、以及「第一帧用哪套配色」一起传进去。
            mark("设置读完");
            let pref = settings.theme.clone();
            let main_window = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("闪电全能下载")
            .inner_size(1100.0, 720.0)
            .min_inner_size(880.0, 560.0)
            .resizable(true)
            // 去掉系统标题栏：标题里那个「闪电全能下载」和侧栏的品牌重复了一次，
            // 窗口按钮改成页面里自绘（顶部右上角那三个）
            // 先不显示：窗口一出来 WebView 还是白底，页面画好之前会先闪一下。
            // 等页面加载完成再 show，用户看到的第一眼就是界面本身
            .visible(false)
            .decorations(false)
            // Win11 的四角与阴影统一交给系统裁剪；Win10 保留不透明直角窗口。
            // 透明模式不能设 background_color，否则整窗变黑；底色改由 #app 自己给。
            .transparent(!theme::legacy_frame())
            .shadow(true)
            .center()
            .theme(theme::native(&pref))
            // 页面里的内联脚本会先读 localStorage，第一次启动时它是空的，
            // 这里直接把结论注入进去，保证第一帧就是对的
            .initialization_script(theme::init(&pref))
            .initialization_script(theme::frame_init())
            .additional_browser_args(BROWSER_ARGS)
            .build()?;
            if theme::legacy_frame() {
                let _ = main_window.set_background_color(Some(theme::background(&pref)));
            }
            mark(if theme::legacy_frame() { "窗口框架：Win10 直角（启动标记 v2）" } else { "窗口框架：Win11 圆角（启动标记 v2）" });
            mark("窗口建好（WebView2 环境就绪）");
            // 文件拖进窗口没反应的根源：Tauri v2 默认接管了 WebView 的原生拖放
            // （dragDropEnabled），页面里的 HTML5 dragover/drop 不会触发，路径只能在
            // Rust 侧从 Tauri 的拖放事件里拿。Drop 的每个路径发一条 ext-add 给主窗口，
            // 前端走和粘贴/浏览器扩展同一条流程（确认框或直接投任务），别在这边直接
            // 调 add_link_full。多个文件就发多条事件。
            {
                let win = main_window.clone();
                main_window.on_window_event(move |event| {
                    // 最大化时把圆角切成方角：圆角处会漏出桌面，看着像四角缺了一块
                    if let tauri::WindowEvent::Resized(_) = event {
                        let mx = win.is_maximized().unwrap_or(false);
                        let mut last = MAXED.lock().unwrap();
                        if *last != mx {
                            *last = mx;
                            let _ = win.emit("win-max", mx);
                        }
                        return;
                    }
                    // 只关心 Drop 完成，Enter/Over/Leave 不用管
                    if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) =
                        event
                    {
                        for path in paths {
                            let url = path.to_string_lossy().into_owned();
                            // LinkKind::of 判断认不认识：.torrent 本地路径（文件存在才算）、
                            // 磁力、ed2k、直链文本都认识。认识的发 ext-add 走前端全套流程；
                            // 不认识的（.txt/.jpg 之类）也发一条，前端会弹「这个链接不认识」，
                            // 行为和粘贴一段看不懂的文字一致
                            match links::LinkKind::of(&url) {
                                Some(kind) => eprintln!("拖入{}：{url}", kind.label()),
                                None => eprintln!("拖入不认识的文件：{url}"),
                            }
                            let _ = win.emit(
                                "ext-add",
                                serde_json::json!({ "url": url, "headers": [], "name": "" }),
                            );
                        }
                    }
                });
            }

            let tools = tools::Tools::new(&migrate::current_dir(&base, &local, "bin"));
            let videos = std::sync::Arc::new(videos::Videos::new(&base, &local));

            app.manage(AppState {
                baidu: std::sync::Arc::new(baidu::Downloads::new(&local)),
                engine: Mutex::new(engine::Engine::new(migrate::current_dir(&base, &local, "engine"))),
                tools,
                videos,
                settings: Mutex::new(settings),
                folders,
                base,
                local,
            });

            // 托盘：Windows 上关窗口只是收起来，任务继续下（Mac 版是 NSStatusItem）
            if let Err(e) = tray::build(app.handle()) {
                eprintln!("托盘图标建不出来：{e}");
            }
            // 后台循环：外来链接投任务、完成通知、防睡眠、剪贴板监听、托盘提示
            shell::start_background(app.handle().clone());
            app.state::<AppState>().baidu.reconcile_cleanups();

            // 其余全部挪到后台线程。原因很实际：杀毒软件的实时保护会在 CreateProcess
            // 时扫描 29 MB 的 gopeed.exe，实测能卡住 7 秒——放在 setup 里就是「双击后
            // 窗口白着好几秒」。挪走之后窗口和界面先出来，内核随后自己就绪，
            // 界面上那几秒显示的是「内核启动中」，不是空白。
            mark("setup 结束（窗口交给 WebView2）");
            let handle = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                // 启动即拉内核：这是产品行为不是调试代码，用户双击图标就该能用
                let spawned = {
                    let st = handle.state::<AppState>();
                    // 先落到变量里再返回：块尾的临时 MutexGuard 会把 st 的借用拖到块外
                    let r = st.engine.lock().unwrap().spawn();
                    r
                };
                if let Ok((port, token)) = spawned {
                    let api = api::Api::new(port, token);
                    let (want, folders) = {
                        let st = handle.state::<AppState>();
                        let s = st.settings.lock().unwrap().clone();
                        (s, st.folders.clone())
                    };
                    // 就绪后把设置写进内核（同时下载数、限速、分类目录）
                    tauri::async_runtime::block_on(async move {
                        for _ in 0..66 {
                            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                            if api.info().await.is_ok() {
                                let _ = want.apply(&api, &folders).await;
                                break;
                            }
                        }
                    });
                }

                // 协议注册：让 magnet: / ed2k: / shandian: 能唤起本程序（写 HKCU，不需要管理员）
                shell::register_protocols();
                // 本机网关 127.0.0.1:6800：浏览器扩展 / LinkSwift / Gopeed 官方扩展都从这里交任务。
                // 内核只认 X-Api-Token，浏览器的 fetch 带不了自定义头，所以必须有一层网关
                gateway::start(handle.clone());
                // 把随包的扩展同步到数据目录（有就同步，没有就算了）
                {
                    let st = handle.state::<AppState>();
                    gateway::install(&st.base);
                }
                // 启动后补齐 yt-dlp / ffmpeg（缺了才下）
                tools::bootstrap(&handle);

                // 本次启动参数里带的链接：用户双击磁力链接/种子时就从这里进来的。
                // 放在内核拉起之后，免得内核还没就绪就把任务投丢了
                let args: Vec<String> = std::env::args().collect();
                for link in shell::links_from_args(&args) {
                    shell::push_pending(&link);
                }
            });
            // 都要用 AppState，必须在 manage 之后
            cloud::init(app.handle().clone());
            pikpak::start_monitor(app.handle().clone());
            thunder::start_monitor(app.handle().clone());
            float::start_done_watcher(app.handle().clone());
            engine::start_name_guard(app.handle().clone());
            engine::start_resume_keeper(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            link_kind,
            ui_ready,
            extract_links,
            engine::engine_status,
            engine::engine_start,
            engine::engine_stop,
            engine::add_task,
            baidu_setup::prepare_baidu_share,
            baidu_setup::authorize_baidu_share,
            baidu_setup::cancel_baidu_prepare,
            baidu::add_baidu_share,
            baidu::preview_baidu_share,
            baidu::save_baidu_copy,
            engine::add_multi,
            feishu::probe_feishu_document,
            feishu::add_feishu_document,
            feishu_setup::authorize_feishu,
            feishu_setup::cancel_feishu_authorization,
            engine::multi_task_info,
            engine::multi_refresh_links,
            engine::multi_refresh_prepare,
            engine::task_select_files,
            thunder::thunder_to_local,
            thunder::thunder_panel_close,
            engine::multi_task_redownload,
            engine::multi_conflict,
            engine::task_file_list,
            engine::task_open_downloaded_file,
            engine::task_downloaded_file_path,
            engine::task_drawer_audio,
            engine::thumb_get,
            engine::play_url,
            engine::verify_task,
            engine::probe_video,
            engine::probe_collection,
            engine::polish_task,
            engine::polish_restore,
            engine::subtitle_look,
            engine::subtitle_fetch,
            engine::system_fonts,
            engine::subtitle_restyle,
            engine::ollama_models,
            engine::set_queue_order,
            engine::is_dev_build,
            engine::list_tasks,
            engine::pause_task,
            engine::resume_task,
            engine::delete_task,
            engine::forget_task,
            engine::find_duplicate,
            engine::video_copy_anyway,
            engine::video_dismiss_collision,
            engine::open_task_folder,
            engine::item_open,
            engine::item_reveal,
            engine::item_path,
            engine::open_url,
            engine::purge_missing,
            trash::trash_list,
            trash::trash_delete,
            trash::trash_clear,
            trash::trash_redownload,
            trash::trash_open_path,
            trash::trash_thumb,
            update::update_check,
            update::update_install,
            commands::reveal_path,
            commands::quit_app,
            commands::open_logs_dir,
            commands::export_recent_logs,
            commands::diagnostic_info,
            app_log::log_ui_event,
            cache::cache_info,
            cache::clear_cache,
            engine::bt_files,
            commands::get_settings,
            commands::clipboard_read_text,
            commands::clipboard_write_text,
            gateway::extension_status,
            gateway::rpc_info,
            mcp::mcp_config,
            gateway::extension_dir,
            gateway::extension_reveal,
            gateway::open_browser_extensions,
            dialog::open_dialog,
            dialog::dialog_payload,
            wechat::wechat_confirm,
            wechat::wechat_cancel,
            wechat::wechat_open_preview,
            float::float_pending,
            float::float_action,
            float::float_close,
            float::done_pending,
            float::sync_collection_members,
            float::notify_collection,
            float::done_close,
            float::done_action,
            engine::name_conflict_resolve,
            engine::name_precheck,
            engine::name_hint,
            engine::name_conflict_new,
            commands::save_settings,
            commands::pick_folder,
            commands::pick_text_file,
            commands::export_sub_styles,
            commands::import_sub_styles,
            commands::import_cookies,
            commands::cookies_status,
            commands::open_dir,
            commands::remove_empty_collection_dir,
            commands::open_path,
            commands::open_media,
            engine::recycle_task_file,
            engine::video_task_options,
            engine::task_folder_path,
            commands::target_info,
            commands::remember_video_options,
            commands::set_app_theme,
            commands::autostart_status,
            commands::set_autostart,
            thunder::thunder_status,
            thunder::thunder_open,
            thunder::thunder_handoff_url,
            cloud::cloud_device_start,
            cloud::cloud_device_result,
            cloud::cloud_device_cancel,
            cloud::cloud_account_link,
            cloud::cloud_resume_account_link,
            cloud::cloud_account_remove,
            cloud::cloud_accounts,
            cloud::cloud_account_save,
            cloud::cloud_account_result,
            cloud::cloud_sources,
            cloud::cloud_offline,
            cloud::cloud_job_control,
            pikpak::pikpak_quota,
            thunder::thunder_conflict,
            thunder::thunder_handoff,
            pikpak::pikpak_login,
            pikpak::pikpak_pulled,
            pikpak::pikpak_status,
            pikpak::pikpak_logout,
            pikpak::pikpak_offline,
            pikpak::pikpak_pull,
            pikpak::pikpak_tasks,
            pikpak::pikpak_remove,
            tools::tools_status,
            tools::install_tool,
            tools::open_tools_dir,
        ])
        .build(tauri::generate_context!())
        .expect("启动失败");
    // build() 返回时窗口（含 WebView2 环境）已经建好，run() 里才跑 setup
    mark("builder.build 完成（窗口与 WebView2 已就绪）");
    app.run(|_app, _event| {});
}
