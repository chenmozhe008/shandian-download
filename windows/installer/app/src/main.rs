// 闪电全能下载 · 安装 / 卸载程序的界面进程。
//
// 同一个 exe 两种身份：--payload <zip> 是安装，--uninstall 是卸载（安装时把自己复制成
// uninstall.exe）。它依赖旁边的 WebView2Loader.dll，所以发给用户的单文件是 ../stub，
// 由 stub 把这个 exe、dll 和 zip 解到临时目录再启动。
//
// 窗口不透明，圆角和细边框交给系统（DWM）画。透明窗口 + 页面 border-radius 的做法
// 在 WebView2 上会露出合成层的背景，是之前安装程序边框一直有问题的原因。
//
// 删除只认三样东西：安装清单里的文件、固定的注册表项、用户勾选后的数据目录。
// 绝不递归删安装目录——用户可能选了一个放着别的东西的目录。
#![cfg_attr(not(test), windows_subsystem = "windows")]

#[path = "../../../shared/browser_support.rs"]
mod browser_support;
use browser_support::browser_exe;
mod extension_page;

use std::fs;
use std::io::{Read, Write};
use std::os::windows::{fs::MetadataExt, process::CommandExt};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};
use windows::core::{Interface, HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, IPersistFile, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
    WaitForSingleObject, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE,
};
use windows::Win32::UI::Shell::{
    FOLDERID_CommonPrograms, FOLDERID_Desktop, FOLDERID_ProgramFiles, FOLDERID_Programs,
    FOLDERID_PublicDesktop, IShellLinkW, IsUserAnAdmin, SHChangeNotify, SHGetKnownFolderPath,
    ShellExecuteExW, ShellLink, KF_FLAG_DEFAULT, SEE_MASK_NOCLOSEPROCESS, SHCNE_ASSOCCHANGED,
    SHCNF_IDLIST, SHELLEXECUTEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONINFORMATION, MB_YESNO, SW_SHOWNORMAL,
};
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
use winreg::RegKey;

const APP_NAME: &str = "闪电全能下载";
static ELEVATION_HANDOFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const APP_EXE: &str = "闪电全能下载.exe";
const UNINSTALL_EXE: &str = "uninstall.exe";
const PORTABLE_CLEANUP_EXE: &str = "清除闪电数据.exe";
const LOADER_DLL: &str = "WebView2Loader.dll";
/// 装了哪些文件记在这里，卸载和升级都只认它
const MANIFEST: &str = "install-manifest.txt";
/// 主程序的数据目录名（tauri identifier）
const DATA_ID: &str = "com.shandian.windows";
const VERSION: &str = env!("SD_VERSION");
const EDITION: &str = env!("SD_EDITION");
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const WINDOW_WIDTH: f64 = 600.0;
const EXT_DIR: &str = "BrowserExtension";
const DETACHED_PROCESS: u32 = 0x0000_0008;
/// 卸载界面运行时所在的临时目录前缀；清理时再核对一次，别删错地方
const UNINSTALL_TEMP_PREFIX: &str = "shandian-uninstall-";

/// 浏览器参数和主程序一致（少了这些 WebView2 会走另一条合成路径）
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI \
     --disable-component-update \
     --no-default-browser-check --disable-sync --no-first-run --no-proxy-server";

// ---------- 路径 ----------

/// 所有写到系统里的位置都从这儿取。设了 SHANDIAN_SETUP_SANDBOX=<目录> 时，
/// 快捷方式、数据目录、注册表全部落到测试位置，测试不碰本机真实环境。
///
/// machine=true 是「为所有用户安装」：程序、快捷方式、卸载项都在系统级位置。
/// 下载记录、设置、开机启动、协议关联是主程序按用户写的，不管哪种安装都在当前用户下。
struct Places {
    machine: bool,
    sandbox: bool,
    desktop: PathBuf,
    programs: PathBuf,
    program_root: PathBuf,
    appdata: PathBuf,
    localappdata: PathBuf,
    /// 当前用户注册表项的前缀（测试时换地方）
    reg_prefix: String,
}

fn sandbox_dir() -> Option<PathBuf> {
    std::env::var_os("SHANDIAN_SETUP_SANDBOX").map(PathBuf::from)
}

impl Places {
    fn get(machine: bool) -> Self {
        if let Some(s) = sandbox_dir() {
            let (d, p) = if machine { ("PublicDesktop", "CommonPrograms") } else { ("Desktop", "Programs") };
            return Self {
                machine,
                sandbox: true,
                desktop: s.join(d),
                programs: s.join(p),
                program_root: s.join("ProgramFiles"),
                appdata: s.join("AppData"),
                localappdata: s.join("LocalAppData"),
                reg_prefix: r"Software\ShandianSetupTest\".into(),
            };
        }
        let env = |k: &str| PathBuf::from(std::env::var_os(k).unwrap_or_default());
        let (desktop, programs) = if machine {
            (
                known_folder(&FOLDERID_PublicDesktop).unwrap_or_else(|| env("PUBLIC").join("Desktop")),
                known_folder(&FOLDERID_CommonPrograms).unwrap_or_else(|| {
                    env("ProgramData").join(r"Microsoft\Windows\Start Menu\Programs")
                }),
            )
        } else {
            (
                known_folder(&FOLDERID_Desktop).unwrap_or_else(|| env("USERPROFILE").join("Desktop")),
                known_folder(&FOLDERID_Programs).unwrap_or_else(|| {
                    env("APPDATA").join(r"Microsoft\Windows\Start Menu\Programs")
                }),
            )
        };
        Self {
            machine,
            sandbox: false,
            desktop,
            programs,
            program_root: known_folder(&FOLDERID_ProgramFiles).unwrap_or_else(|| env("ProgramFiles")),
            appdata: env("APPDATA"),
            localappdata: env("LOCALAPPDATA"),
            reg_prefix: String::new(),
        }
    }

    /// 卸载项所在的注册表根：所有用户是 HKLM。测试时不碰 HKLM
    fn root(&self) -> RegKey {
        if self.machine && !self.sandbox {
            RegKey::predef(HKEY_LOCAL_MACHINE)
        } else {
            hkcu()
        }
    }

    fn uninstall_key(&self) -> String {
        let scope = if self.machine && self.sandbox { r"HKLM\" } else { "" };
        format!(r"{}{scope}Software\Microsoft\Windows\CurrentVersion\Uninstall\{APP_NAME}", self.reg_prefix)
    }

    fn lnks(&self) -> [PathBuf; 2] {
        let name = format!("{APP_NAME}.lnk");
        [self.desktop.join(&name), self.programs.join(&name)]
    }

    fn default_dir(&self) -> PathBuf {
        if self.machine {
            self.program_root.join(APP_NAME)
        } else {
            // 用户目录：不需要管理员，主程序自动更新时也写得进去
            self.localappdata.join("Programs").join(APP_NAME)
        }
    }

    /// 要不要先提权（测试模式写的都是测试目录，不用）
    fn needs_admin(&self) -> bool {
        self.machine && !self.sandbox && !is_admin()
    }

    fn data_dirs(&self) -> [PathBuf; 2] {
        [self.appdata.join(DATA_ID), self.localappdata.join(DATA_ID)]
    }
}

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    unsafe {
        let p: PWSTR = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

fn hkcu() -> RegKey {
    RegKey::predef(HKEY_CURRENT_USER)
}

fn is_admin() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}

/// 已安装的版本和位置（注册表里记的，且主程序真的还在才算）
fn installed(places: &Places) -> Option<(String, PathBuf)> {
    let k = places.root().open_subkey(places.uninstall_key()).ok()?;
    let dir: String = k.get_value("InstallLocation").ok()?;
    let ver: String = k.get_value("DisplayVersion").unwrap_or_default();
    let dir = PathBuf::from(dir);
    dir.join(APP_EXE).is_file().then_some((ver, dir))
}

/// 两种安装方式里已经装了的那个：(版本, 目录, 是否所有用户)
fn find_installed() -> Option<(String, PathBuf, bool)> {
    [false, true].into_iter().find_map(|machine| {
        installed(&Places::get(machine)).map(|(v, d)| (v, d, machine))
    })
}

/// 以管理员身份启动，返回进程句柄。用户在 UAC 上点了「否」返回 Err(None)
fn run_as_admin(exe: &Path, params: &str) -> Result<windows::Win32::Foundation::HANDLE, Option<String>> {
    let (verb, file, args) = (HSTRING::from("runas"), HSTRING::from(exe.as_os_str()), HSTRING::from(params));
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: windows::core::PCWSTR(verb.as_ptr()),
        lpFile: windows::core::PCWSTR(file.as_ptr()),
        lpParameters: windows::core::PCWSTR(args.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut sei) }.map_err(|e| {
        // ERROR_CANCELLED：用户取消了 UAC
        (e.code().0 != 0x8007_04C7_u32 as i32).then(|| e.message())
    })?;
    Ok(sei.hProcess)
}

/// 命令行参数加引号。结尾的反斜杠会把引号转义掉，先去掉
fn quote(p: &str) -> String {
    format!("\"{}\"", p.trim_end_matches('\\'))
}

// ---------- 进程 ----------

fn lower(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\").to_lowercase()
}

/// 路径在不在这些目录下面
fn is_under(path: &str, dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|d| {
        let d = lower(d);
        let d = d.trim_end_matches('\\');
        path.starts_with(&format!("{d}\\"))
    })
}

/// 从这些目录里跑起来的进程：(pid, 文件名)。自己除外
fn procs_under(dirs: &[PathBuf]) -> Vec<(u32, String)> {
    matching_procs(|_, path| path.is_some_and(|path| is_under(&path.to_lowercase(), dirs)))
}

fn matching_procs(matches: impl Fn(&str, Option<&str>) -> bool) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    unsafe {
        let me = GetCurrentProcessId();
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            if e.th32ProcessID != me && e.th32ProcessID != 0 {
                let end = e.szExeFile.iter().position(|&ch| ch == 0).unwrap_or(e.szExeFile.len());
                let name = String::from_utf16_lossy(&e.szExeFile[..end]);
                let path = if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, e.th32ProcessID) {
                    let mut buf = [0u16; 1024];
                    let mut n = buf.len() as u32;
                    let got = QueryFullProcessImageNameW(
                        h,
                        PROCESS_NAME_WIN32,
                        PWSTR(buf.as_mut_ptr()),
                        &mut n,
                    );
                    let _ = CloseHandle(h);
                    got.is_ok().then(|| String::from_utf16_lossy(&buf[..n as usize]))
                } else {
                    None
                };
                if matches(&name, path.as_deref()) {
                    out.push((e.th32ProcessID, name));
                }
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

fn kill_procs(dirs: &[PathBuf]) {
    for (pid, _) in procs_under(dirs) {
        unsafe {
            if let Ok(h) = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) {
                let _ = TerminateProcess(h, 0);
                let _ = WaitForSingleObject(h, 3000);
                let _ = CloseHandle(h);
            }
        }
    }
}

/// 这次操作会占用文件的目录。卸载且要删数据时，yt-dlp、内核可能从数据目录里跑
fn busy_dirs(places: &Places, dir: &Path, with_data: bool) -> Vec<PathBuf> {
    let mut dirs = vec![dir.to_path_buf()];
    if with_data {
        dirs.extend(places.data_dirs());
    }
    dirs
}

// ---------- 给界面的命令 ----------

struct Mode {
    /// 安装：zip 路径
    payload: Option<PathBuf>,
    /// 卸载：安装目录
    uninstall_dir: Option<PathBuf>,
    /// 卸载的是「所有用户」那份
    machine: bool,
    /// 提权后重新启动时带来的安装选项，界面直接开始装
    auto: Option<InstallOpts>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Info {
    mode: &'static str,
    version: &'static str,
    /// 安装目录：升级时是原位置，卸载时是要卸载的位置
    dir: String,
    /// 已装的版本（升级时显示，目录和安装方式都不让改）
    installed_version: Option<String>,
    extension_version: Option<String>,
    installed_extension_version: Option<String>,
    /// 这个安装包的版别（plain / nas），和已装的版别（旧版没记录时为空）
    edition: &'static str,
    installed_edition: Option<String>,
    /// 这个包相对已装版本：upgrade / same / downgrade，没装过为 None
    version_cmp: Option<&'static str>,
    /// 当前选中的是不是「所有用户」：更新时是已装的那种，否则默认为只给自己
    machine: bool,
    /// 两种安装方式各自的默认位置
    user_dir: String,
    machine_dir: String,
    /// 提权后回来的：不用再选，直接开始
    auto: Option<InstallOpts>,
    /// 解压后的大小
    size_mb: u64,
    /// 用户的数据目录在不在（卸载界面决定要不要给「同时删除」这个选项）
    has_data: bool,
}

fn extension_version_from(text: &str) -> Option<String> {
    let manifest: serde_json::Value = serde_json::from_str(text).ok()?;
    ["version_name", "version"].into_iter().find_map(|key| {
        let version = manifest[key].as_str()?.trim();
        (!version.is_empty()).then(|| version.to_owned())
    })
}
fn extension_version_in(dir: &Path) -> Option<String> {
    extension_version_from(&fs::read_to_string(dir.join(EXT_DIR).join("manifest.json")).ok()?)
}

/// 这个包相对已装版本。按数字逐段比，0.1.10 比 0.1.9 新
fn compare_versions(new: &str, old: &str) -> &'static str {
    let parse = |s: &str| s.split('.').map(|x| x.trim().parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let (a, b) = (parse(new), parse(old));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return if x > y { "upgrade" } else { "downgrade" };
        }
    }
    "same"
}

#[tauri::command]
fn setup_info(mode: State<'_, Mode>) -> Result<Info, String> {
    let (user_dir, machine_dir) = (
        Places::get(false).default_dir().display().to_string(),
        Places::get(true).default_dir().display().to_string(),
    );
    if let Some(dir) = &mode.uninstall_dir {
        let places = Places::get(mode.machine);
        return Ok(Info {
            mode: "uninstall",
            version: VERSION,
            dir: dir.display().to_string(),
            installed_version: None,
            extension_version: None,
            installed_extension_version: None,
            edition: EDITION,
            installed_edition: None,
            version_cmp: None,
            machine: mode.machine,
            user_dir,
            machine_dir,
            auto: None,
            size_mb: 0,
            has_data: places.data_dirs().iter().any(|d| d.exists()),
        });
    }
    let payload = mode.payload.as_ref().ok_or("安装包不完整")?;
    let mut zip = open_zip(payload)?;
    let bytes: u64 = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.size()))
        .sum();
    let old = find_installed();
    let extension_version = zip.by_name("BrowserExtension/manifest.json").ok().and_then(|mut f| {
        let mut text = String::new(); f.read_to_string(&mut text).ok()?; extension_version_from(&text)
    });
    let installed_extension_version = old.as_ref().and_then(|(_,dir,_)|extension_version_in(dir));
    Ok(Info {
        mode: "install",
        version: VERSION,
        extension_version,
        installed_extension_version,
        dir: old.as_ref().map_or_else(|| user_dir.clone(), |(_, d, _)| d.display().to_string()),
        machine: old.as_ref().is_some_and(|(_, _, m)| *m),
        edition: EDITION,
        installed_edition: old.as_ref().and_then(|(_, _, m)| {
            let places = Places::get(*m);
            places.root().open_subkey(places.uninstall_key()).ok()?.get_value::<String, _>("ShandianEdition").ok()
        }),
        version_cmp: old.as_ref().map(|(v, _, _)| compare_versions(VERSION, v)),
        installed_version: old.map(|(v, _, _)| v),
        user_dir,
        machine_dir,
        auto: mode.auto.clone(),
        size_mb: bytes / 1024 / 1024 + 1,
        has_data: false,
    })
}

/// 安装目录必须是专门的文件夹。手输 C:\ 或 D:\Tools 时在里面建一个：
/// 安装时要结束目录下运行的程序，写成上级目录就会把别的程序一起结束掉
fn app_dir_of(input: &str) -> PathBuf {
    let p = PathBuf::from(input.trim());
    if p.file_name().is_some_and(|n| n == APP_NAME) {
        p
    } else {
        p.join(APP_NAME)
    }
}

/// 界面上显示的最终安装位置
#[tauri::command]
fn normalize_dir(dir: String) -> String {
    app_dir_of(&dir).display().to_string()
}

/// 选目录。选的不是专门的文件夹就在里面建一个，免得文件散在 D:\ 这种地方
#[tauri::command]
async fn pick_dir(app: tauri::AppHandle, current: String) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let start = PathBuf::from(&current);
    let mut d = app.dialog().file().set_title("选择安装位置");
    if let Some(p) = start.ancestors().find(|p| p.is_dir()) {
        d = d.set_directory(p);
    }
    let picked = tauri::async_runtime::spawn_blocking(move || d.blocking_pick_folder())
        .await
        .ok()??;
    let picked = picked.into_path().ok()?;
    Some(app_dir_of(&picked.to_string_lossy()).display().to_string())
}

/// 目录里有没有正在运行的程序，有就返回程序名给界面提示
#[tauri::command]
fn running_in(dir: String, with_data: bool) -> Vec<String> {
    let places = Places::get(false);
    let mut names: Vec<String> = procs_under(&busy_dirs(&places, &app_dir_of(&dir), with_data))
        .into_iter()
        .map(|(_, n)| n)
        .collect();
    names.sort();
    names.dedup();
    names
}

#[derive(Serialize, Clone)]
struct Prog {
    pct: u32,
    text: String,
}

fn emit(app: &tauri::AppHandle, pct: u32, text: &str) {
    let _ = app.emit("prog", Prog { pct, text: text.to_string() });
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct InstallOpts {
    dir: String,
    desktop: bool,
    machine: bool,
}

/// 所有入口（包括自动参数和提权重启）都保留已有安装的目录和范围。
fn locked_install_opts(opts: &InstallOpts) -> InstallOpts {
    if let Some((_, dir, machine)) = find_installed() {
        InstallOpts { dir: dir.display().to_string(), desktop: opts.desktop, machine }
    } else {
        opts.clone()
    }
}


/// 这次安装要不要先弹管理员确认
#[tauri::command]
fn needs_admin(machine: bool) -> bool {
    Places::get(machine).needs_admin()
}

/// 为所有用户安装：点了安装才提权，不是一打开就弹 UAC。
/// 以管理员身份再起一份自己（它直接开始装），这份藏起来等它结束——
/// 外壳在等这个进程退出后才清临时目录，提前退出会把安装数据删掉。
#[tauri::command]
fn elevate_install(
    opts: InstallOpts,
    app: tauri::AppHandle,
    mode: State<'_, Mode>,
) -> Result<(), String> {
    let opts = locked_install_opts(&opts);
    let payload = mode.payload.as_ref().ok_or("安装包不完整")?;
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let params = format!(
        "--payload {} --auto-dir {} --auto-desktop {}{}",
        quote(&payload.to_string_lossy()),
        quote(&opts.dir),
        if opts.desktop { "1" } else { "0" },
        if opts.machine { " --all-users" } else { "" },
    );
    use std::sync::atomic::Ordering;
    ELEVATION_HANDOFF.store(true, Ordering::SeqCst);
    let window = app.get_webview_window("main");
    if let Some(w) = &window {
        let _ = w.hide();
    }
    let h = match run_as_admin(&me, &params) {
        Ok(handle) => handle,
        Err(error) => {
            ELEVATION_HANDOFF.store(false, Ordering::SeqCst);
            if let Some(w) = window {
                let _ = w.show();
                let _ = w.set_focus();
            }
            return Err(error.unwrap_or_else(|| "cancelled".to_string()));
        }
    };
    let raw = h.0 as isize;
    std::thread::spawn(move || {
        let h = windows::Win32::Foundation::HANDLE(raw as _);
        unsafe {
            let _ = WaitForSingleObject(h, u32::MAX);
            let _ = CloseHandle(h);
        }
        app.exit(0);
    });
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallResult { exe: String, extension_changed: bool, extension_from: Option<String>, extension_to: Option<String> }

#[tauri::command]
async fn install(
    opts: InstallOpts,
    app: tauri::AppHandle,
    mode: State<'_, Mode>,
) -> Result<InstallResult, String> {
    let payload = mode.payload.clone().ok_or("安装包不完整")?;
    tauri::async_runtime::spawn_blocking(move || do_install(&opts, &payload, &app))
        .await
        .map_err(|e| e.to_string())?
}

fn open_zip(path: &Path) -> Result<zip::ZipArchive<fs::File>, String> {
    let f = fs::File::open(path).map_err(|e| format!("打不开安装数据：{e}"))?;
    zip::ZipArchive::new(f).map_err(|e| format!("安装数据已损坏：{e}"))
}

/// 只创建属于本次探测的文件；不能覆盖用户或旧版遗留的同名文件。
fn probe_install_write(target: &Path) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|e| no_access(target, &e))?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?.as_nanos();
    for attempt in 0..16 {
        let path = target.join(format!(".shandian-setup-probe-{}-{stamp}-{attempt}.tmp", std::process::id()));
        let mut file = match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(no_access(target, &e)),
        };
        let wrote = file.write_all(b"setup permission probe");
        drop(file);
        let removed = fs::remove_file(&path);
        wrote.map_err(|e| no_access(target, &e))?;
        removed.map_err(|e| no_access(target, &e))?;
        return Ok(());
    }
    Err("无法创建独立的写入检查文件，请重试".into())
}

/// 关闭旧程序之前，先检查权限、安装数据和目标目录写入能力。
fn prepare_install(target: &Path, payload: &Path, needs_elevation: bool)
    -> Result<(zip::ZipArchive<fs::File>, u64), String>
{
    if !target.is_absolute() { return Err("安装位置要写完整路径".into()); }
    if needs_elevation {
        return Err("更新或安装到原系统目录需要管理员确认；操作尚未开始，原程序保持不变。".into());
    }
    let mut zip = open_zip(payload)?;
    let mut total = 0u64;
    let mut has_main = false;
    for i in 0..zip.len() {
        let file = zip.by_index(i).map_err(|e| format!("安装数据已损坏：{e}"))?;
        let rel = file.enclosed_name().ok_or("安装数据包含无效路径")?;
        has_main |= !file.is_dir() && rel == Path::new(APP_EXE);
        total = total.saturating_add(file.size());
    }
    if !has_main { return Err("安装数据里没有主程序".into()); }
    probe_install_write(target)?;
    Ok((zip, total.max(1)))
}

fn do_install(opts: &InstallOpts, payload: &Path, app: &tauri::AppHandle) -> Result<InstallResult, String> {
    let opts = locked_install_opts(opts);
    let places = Places::get(opts.machine);
    let target = app_dir_of(opts.dir.trim());
    let extension_from = extension_version_in(&target);
    emit(app, 2, "正在检查安装权限和安装数据…");
    let (mut zip, total) = prepare_install(&target, payload, places.needs_admin())?;
    // 只有所有预检成功之后才能关闭原程序，避免权限错误先打断下载。
    kill_procs(&[target.clone()]);
    let old_files = read_manifest(&target);
    let mut extension_changed = false;
    let mut done = 0u64;
    let mut files: Vec<String> = Vec::new();
    let mut buf = vec![0u8; 256 * 1024];
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| format!("安装数据已损坏：{e}"))?;
        // enclosed_name 会拒绝 ../ 这类越界路径
        let Some(rel) = f.enclosed_name() else { continue };
        let out = target.join(&rel);
        if f.is_dir() {
            fs::create_dir_all(&out).map_err(|e| no_access(&out, &e))?;
            continue;
        }
        if let Some(p) = out.parent() {
            fs::create_dir_all(p).map_err(|e| no_access(p, &e))?;
        }
        // 先写临时文件再替换：中途失败或断电时旧版本还是完整的
        let tmp = out.with_extension("sdtmp");
        {
            let mut w = fs::File::create(&tmp).map_err(|e| no_access(&tmp, &e))?;
            loop {
                let n = f.read(&mut buf).map_err(|e| format!("解压失败：{e}"))?;
                if n == 0 {
                    break;
                }
                w.write_all(&buf[..n]).map_err(|e| {
                    format!("写入失败（磁盘满了？）：{e}")
                })?;
                done += n as u64;
                emit(app, 4 + (done * 82 / total) as u32, "正在复制文件…");
            }
        }
        if rel.starts_with(EXT_DIR) {
            let incoming = fs::read(&tmp).map_err(|e| format!("读取扩展更新失败：{e}"))?;
            if fs::read(&out).ok().as_deref() != Some(incoming.as_slice()) { extension_changed = true; }
        }
        replace_file(&tmp, &out)?;
        files.push(rel.to_string_lossy().replace('/', "\\"));
    }
    let exe = target.join(APP_EXE);
    if !exe.is_file() {
        return Err("安装数据里没有主程序".into());
    }

    // 这个界面程序自己就是卸载程序
    emit(app, 88, "正在写入卸载程序…");
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let uninstaller = target.join(UNINSTALL_EXE);
    fs::copy(&me, &uninstaller).map_err(|e| no_access(&uninstaller, &e))?;
    files.push(UNINSTALL_EXE.to_string());

    // 升级：新版本已经没有的旧文件才清掉
    for rel in old_files.iter().filter(|o| !files.iter().any(|n| n.eq_ignore_ascii_case(o))) {
        if Path::new(rel).starts_with(EXT_DIR) { extension_changed = true; }
        if let Some(p) = safe_join(&target, rel) {
            let _ = fs::remove_file(p);
        }
    }
    remove_empty_dirs(&target);
    files.push(MANIFEST.to_string());
    fs::write(target.join(MANIFEST), files.join("\n")).map_err(|e| no_access(&target, &e))?;

    emit(app, 92, "正在创建快捷方式…");
    let [desktop, programs] = places.lnks();
    make_lnk(&programs, &exe, &target).map_err(|e| format!("创建开始菜单快捷方式失败：{e}"))?;
    if opts.desktop {
        make_lnk(&desktop, &exe, &target).map_err(|e| format!("创建桌面快捷方式失败：{e}"))?;
    } else {
        let _ = fs::remove_file(&desktop);
    }

    // 早期的安装程序在这里留过 WebView 缓存，顺手清掉
    let _ = fs::remove_dir_all(places.localappdata.join("com.shandian.installer"));

    emit(app, 96, "正在登记卸载信息…");
    write_uninstall_entry(&places, &target, total).map_err(|e| format!("写注册表失败：{e}"))?;
    emit(app, 100, "安装完成");
    Ok(InstallResult { exe: exe.display().to_string(), extension_changed, extension_from, extension_to: extension_version_in(&target) })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtInfo {
    dir: String,
    /// 本机浏览器：(标识, 显示名, 是否有可接力的扩展)
    browsers: Vec<(&'static str, &'static str, bool)>,
}

/// 浏览器的安装路径（Windows 登记的 App Paths）

/// dir 是安装目录：扩展就从那里加载，更新时原位替换，路径不变（和主程序 gateway::install 一致）
#[tauri::command(async)]
fn ext_info(dir: String) -> ExtInfo {
    let browsers = [("chrome", "Chrome"), ("edge", "Edge")]
        .into_iter()
        .filter(|(k, _)| browser_exe(k).is_some())
        .map(|(k, n)| (k, n, extension_page::available(k)))
        .collect();
    ExtInfo {
        dir: Path::new(&dir).join(EXT_DIR).display().to_string(),
        browsers,
    }
}

#[tauri::command]
async fn open_ext_page(browser: String) -> Result<bool, String> {
    let browser = match browser.as_str() {
        "chrome" => "chrome",
        "edge" => "edge",
        _ => return Err("请选择 Chrome 或 Edge".into()),
    };
    tauri::async_runtime::spawn_blocking(move || extension_page::open(browser))
        .await
        .map_err(|err| err.to_string())?
}

/// 打开并选中扩展文件夹
#[tauri::command]
fn reveal_ext(dir: String) {
    let dir = dir.replace('/', "\\");
    let _ = std::process::Command::new("explorer")
        .raw_arg(format!("/select,\"{dir}\""))
        .spawn();
}

fn no_access(p: &Path, e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        return format!("没有权限写入「{}」。请确认安装权限后重试；更新已有安装时须保留原安装目录和范围。", p.display());
    }
    format!("写入「{}」失败：{e}", p.display())
}

/// 文件可能刚被结束的进程占着，等一下再试
fn replace_file(tmp: &Path, out: &Path) -> Result<(), String> {
    let mut last = None;
    for _ in 0..10 {
        match fs::rename(tmp, out) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let _ = fs::remove_file(tmp);
    Err(no_access(out, &last.unwrap()))
}

fn make_lnk(lnk: &Path, exe: &Path, workdir: &Path) -> windows::core::Result<()> {
    if let Some(p) = lnk.parent() {
        let _ = fs::create_dir_all(p);
    }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        link.SetWorkingDirectory(&HSTRING::from(workdir.as_os_str()))?;
        link.SetIconLocation(&HSTRING::from(exe.as_os_str()), 0)?;
        let file: IPersistFile = link.cast()?;
        file.Save(&HSTRING::from(lnk.as_os_str()), true)?;
    }
    Ok(())
}

fn write_uninstall_entry(places: &Places, dir: &Path, bytes: u64) -> std::io::Result<()> {
    let (k, _) = places.root().create_subkey(places.uninstall_key())?;
    let exe = dir.join(APP_EXE).display().to_string();
    k.set_value("DisplayName", &APP_NAME.to_string())?;
    k.set_value("DisplayVersion", &VERSION.to_string())?;
    k.set_value("ShandianEdition", &EDITION.to_string())?;
    k.set_value("Publisher", &APP_NAME.to_string())?;
    k.set_value("DisplayIcon", &format!("{exe},0"))?;
    k.set_value("InstallLocation", &dir.display().to_string())?;
    k.set_value(
        "UninstallString",
        &format!(
            "\"{}\" --uninstall{}",
            dir.join(UNINSTALL_EXE).display(),
            if places.machine { " --all-users" } else { "" },
        ),
    )?;
    k.set_value("EstimatedSize", &((bytes / 1024) as u32))?;
    k.set_value("NoModify", &1u32)?;
    k.set_value("NoRepair", &1u32)?;
    Ok(())
}

fn read_manifest(dir: &Path) -> Vec<String> {
    fs::read_to_string(dir.join(MANIFEST))
        .map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}

/// 清单里的相对路径 → 真实路径。绝对路径、.. 一律不认：清单是磁盘上的文件，不能全信
fn safe_join(dir: &Path, rel: &str) -> Option<PathBuf> {
    let p = Path::new(rel);
    let ok = !rel.trim().is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)));
    ok.then(|| dir.join(p))
}

/// 只删空目录，自下而上。安装目录自己空了也删；有别的文件就留着
fn remove_empty_dirs(dir: &Path) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                remove_empty_dirs(&p);
                let _ = fs::remove_dir(&p); // 非空会失败，正好
            }
        }
    }
}

#[tauri::command]
async fn launch(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || launch_installed(path))
        .await
        .map_err(|e| format!("安装已完成，但自动启动失败：{e}"))?
}

fn launch_installed(path: String) -> Result<(), String> {
    // Explorer treats forward slashes as switches and can open Documents instead.
    let p = PathBuf::from(path.replace('/', "\\"));
    if !p.is_file() { return Err("安装已完成，但找不到主程序，请从安装目录打开。".into()); }
    // 提权安装时自己是管理员，直接启动主程序也会是管理员：之后从普通窗口往里拖东西都不行。
    // 交给资源管理器启动，回到普通权限
    if is_admin() {
        return std::process::Command::new("explorer")
            .arg(&p)
            .spawn()
            .map(|_| ())
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    "安装已完成，但通过资源管理器自动启动程序被阻止。请关闭安装器，再从桌面快捷方式或开始菜单打开闪电全能下载。".to_string()
                } else {
                    format!("安装已完成，但自动启动失败：{e}。请从桌面或开始菜单打开程序。")
                }
            });
    }
    let mut c = std::process::Command::new(&p);
    if let Some(d) = p.parent() {
        c.current_dir(d);
    }
    // 脱离安装程序：安装程序退出、清理临时目录都不影响它
    c.creation_flags(DETACHED_PROCESS);
    c.spawn().map(|_| ()).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            "安装已完成，但自动启动程序被系统或安全软件阻止。请关闭安装器，再从桌面快捷方式或开始菜单打开闪电全能下载。".to_string()
        } else {
            format!("安装已完成，但自动启动失败：{e}。请关闭安装器，从桌面或开始菜单打开程序。")
        }
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UninstallResult {
    /// 目录里没能删掉的文件数（被占用，或者不是我们装的）
    left: usize,
    dir: String,
}

#[tauri::command]
async fn uninstall(
    remove_data: bool,
    app: tauri::AppHandle,
    mode: State<'_, Mode>,
) -> Result<UninstallResult, String> {
    let dir = mode.uninstall_dir.clone().ok_or("不是卸载模式")?;
    let machine = mode.machine;
    tauri::async_runtime::spawn_blocking(move || do_uninstall(&dir, machine, remove_data, &app))
        .await
        .map_err(|e| e.to_string())
}

fn do_uninstall(dir: &Path, machine: bool, remove_data: bool, app: &tauri::AppHandle) -> UninstallResult {
    let places = Places::get(machine);
    emit(app, 5, "正在关闭程序…");
    kill_procs(&busy_dirs(&places, dir, remove_data));
    std::thread::sleep(Duration::from_millis(400));

    emit(app, 20, "正在删除快捷方式…");
    for l in places.lnks() {
        let _ = fs::remove_file(l);
    }

    emit(app, 30, "正在清理系统登记…");
    clear_registry(&places, dir);

    let files = read_manifest(dir);
    let total = files.len().max(1);
    for (i, rel) in files.iter().enumerate() {
        if let Some(p) = safe_join(dir, rel) {
            remove_retry(&p);
        }
        emit(app, 35 + (i * 45 / total) as u32, "正在删除程序文件…");
    }
    // 自动更新留下的旧版本
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let ours = [APP_EXE, "gopeed.exe", LOADER_DLL];
            if name.strip_suffix(".old").is_some_and(|n| ours.iter().any(|o| o.eq_ignore_ascii_case(n))) {
                remove_retry(&e.path());
            }
        }
    }
    remove_empty_dirs(dir);
    let _ = fs::remove_dir(dir);

    if remove_data {
        emit(app, 85, "正在删除下载记录和设置…");
        for d in places.data_dirs() {
            remove_data_dir(&d);
        }
    }
    emit(app, 100, "卸载完成");
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    UninstallResult { left: count_files(dir), dir: dir.display().to_string() }
}

fn remove_retry(p: &Path) {
    for _ in 0..6 {
        if fs::remove_file(p).is_ok() || !p.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// 卸载项、开机启动、协议关联。开机启动和协议都要指向这个目录才动：
/// 用户可能还有一份绿色版，那份的登记不能被顺手删掉
fn clear_registry(places: &Places, dir: &Path) {
    let _ = places.root().delete_subkey_all(places.uninstall_key());
    clear_launch_registrations(places, dir);
}

fn clear_launch_registrations(places: &Places, dir: &Path) {
    let dirs = [dir.to_path_buf()];
    let hk = hkcu();

    let run = format!(r"{}Software\Microsoft\Windows\CurrentVersion\Run", places.reg_prefix);
    if let Ok(k) = hk.open_subkey_with_flags(&run, KEY_READ | winreg::enums::KEY_SET_VALUE) {
        let names: Vec<String> = k.enum_values().flatten().map(|(n, _)| n).collect();
        for n in names {
            let v: String = k.get_value(&n).unwrap_or_default();
            if points_into(&v, &dirs) {
                let _ = k.delete_value(&n);
            }
        }
    }
    for scheme in ["shandian", "magnet", "ed2k"] {
        let key = format!(r"{}Software\Classes\{scheme}", places.reg_prefix);
        let cmd: String = hk
            .open_subkey(format!(r"{key}\shell\open\command"))
            .and_then(|k| k.get_value(""))
            .unwrap_or_default();
        if points_into(&cmd, &dirs) {
            let _ = hk.delete_subkey_all(&key);
        }
    }
}

/// 命令行（"C:\x\a.exe" "%1"）里的程序在不在这些目录下
fn points_into(cmd: &str, dirs: &[PathBuf]) -> bool {
    let c = cmd.trim().trim_start_matches('"').to_lowercase();
    is_under(&c, dirs)
}

/// 删主程序的数据目录，但 downloads 子目录不动：系统下载目录不可用时，
/// 视频会落在数据目录的 downloads 里，那是用户下载的东西
fn remove_data_dir(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("downloads")) {
            continue;
        }
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = fs::remove_dir_all(&p);
        } else {
            let _ = fs::remove_file(&p);
        }
    }
    let _ = fs::remove_dir(dir);
}

fn portable_cleanup_dirs(places: &Places) -> Result<[PathBuf; 2], String> {
    let dirs = places.data_dirs();
    for dir in &dirs {
        if !dir.is_absolute() {
            return Err("找不到当前 Windows 用户的数据目录。".into());
        }
        match fs::symlink_metadata(dir) {
            Ok(meta) if !meta.is_dir() || meta.file_attributes() & 0x400 != 0 => {
                return Err(format!("数据目录不是普通文件夹，未清理：{}", dir.display()));
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("无法检查数据目录：{}（{error}）", dir.display()));
            }
            _ => {}
        }
    }
    Ok(dirs)
}

fn clear_portable_data(places: &Places, dir: &Path) -> Result<(), String> {
    let dirs = portable_cleanup_dirs(places)?;
    if !matching_procs(|name, path| {
        name.eq_ignore_ascii_case(APP_EXE)
            || path.is_some_and(|path| is_under(&path.to_lowercase(), &dirs))
    }).is_empty() {
        return Err("闪电或下载内核仍在运行。请先从托盘完整退出闪电，再重新打开清理工具。".into());
    }
    for root in &dirs {
        let bytes = match fs::read(root.join("settings.json")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("无法读取下载目录设置，未清理数据（{error}）。")),
        };
        let settings: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| "无法读取下载目录设置，未清理数据。")?;
        let defaults = ["dirMovie", "dirVideo", "dirOther"].iter().map(|key| &settings[*key]);
        let recent = settings["recentDirs"].as_array().into_iter().flatten();
        let last = settings["lastDirs"].as_object().into_iter().flat_map(|map| map.values());
        for original in defaults.chain(recent).chain(last).filter_map(|v| v.as_str()) {
            let normalized = lower(Path::new(original));
            let path = normalized.trim_end_matches('\\');
            if dirs.iter().any(|data| {
                let saved = lower(&data.join("downloads"));
                (path == lower(data) || is_under(path, std::slice::from_ref(data)))
                    && path != saved && !path.starts_with(&(saved + "\\"))
            }) {
                let has_entries = match fs::read_dir(original) {
                    Ok(mut entries) => entries.next().is_some(),
                    Err(error) => error.kind() != std::io::ErrorKind::NotFound,
                };
                if has_entries {
                    return Err("有自定义下载目录位于闪电数据目录内。请先把下载文件移到其它目录，再清理数据。".into());
                }
            }
        }
    }
    for data in &dirs {
        remove_data_dir(data);
    }
    clear_launch_registrations(places, dir);
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    for data in &dirs {
        let entries = match fs::read_dir(data) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("无法确认清理结果：{}（{error}）", data.display())),
        };
        if entries.into_iter().any(|entry| {
            !entry.is_ok_and(|entry| entry.file_name().eq_ignore_ascii_case("downloads"))
        }) {
            return Err(format!("部分数据仍被占用，请关闭闪电后重试。\n{}", data.display()));
        }
    }
    Ok(())
}

fn portable_cleanup_from_launcher() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false; };
    if !exe.file_name().is_some_and(|name| name.eq_ignore_ascii_case(PORTABLE_CLEANUP_EXE)) {
        return false;
    }
    let Some(dir) = exe.parent().filter(|dir| dir.join(APP_EXE).is_file() && !dir.join(MANIFEST).exists()) else {
        message("请从完整解压的绿色版文件夹运行此工具。");
        return true;
    };
    let prompt = HSTRING::from("清除当前 Windows 用户的闪电设置、任务记录、账号信息和缓存？\n\n绿色版和安装版共用这些数据。清除后不可恢复，已下载的文件和程序文件会保留。\n\n请先从托盘退出闪电。");
    let title = HSTRING::from("清除闪电数据");
    let confirmed = unsafe { MessageBoxW(Some(HWND::default()), &prompt, &title, MB_YESNO | MB_ICONINFORMATION | MB_DEFBUTTON2) } == IDYES;
    if confirmed {
        match clear_portable_data(&Places::get(false), dir) {
            Ok(()) => message("闪电用户数据已清除。若浏览器仍加载了闪电扩展，可在浏览器的扩展管理中移除。"),
            Err(error) => message(&error),
        }
    }
    true
}

fn count_files(dir: &Path) -> usize {
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                count_files(&e.path())
            } else {
                1
            }
        })
        .sum()
}

#[tauri::command]
fn open_folder(path: String) {
    let _ = std::process::Command::new("explorer").arg(path.replace('/', "\\")).spawn();
}

/// 窗口随页面调整高度。启动时居中，之后保留窗口位置。
#[tauri::command]
fn fit_window(height: f64, center: bool, app: tauri::AppHandle) -> Result<(), String> {
    use tauri::LogicalSize;
    if ELEVATION_HANDOFF.load(std::sync::atomic::Ordering::SeqCst) { return Ok(()); }
    let win = app.get_webview_window("main").ok_or("找不到窗口")?;
    let visible = win.is_visible().unwrap_or(true);
    let size = LogicalSize::new(WINDOW_WIDTH, height);
    // 先放开限制再改大小，否则新高度会被上一页的限制卡住
    win.set_min_size(None::<LogicalSize<f64>>).map_err(|e| e.to_string())?;
    win.set_max_size(None::<LogicalSize<f64>>).map_err(|e| e.to_string())?;
    win.set_size(size).map_err(|e| e.to_string())?;
    win.set_min_size(Some(size)).map_err(|e| e.to_string())?;
    win.set_max_size(Some(size)).map_err(|e| e.to_string())?;
    if center || !visible {
        let _ = win.center();
    }
    if !visible {
        let _ = win.show();
        let _ = win.set_focus();
    }
    Ok(())
}

#[tauri::command(async)]
fn close_app(app: tauri::AppHandle) {
    clean_uninstall_temp();
    app.exit(0);
}

/// 卸载界面跑在临时目录里，退出后让 cmd 过几秒删掉它（exe 还在跑时删不了自己）
fn clean_uninstall_temp() {
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) else {
        return;
    };
    let is_ours = dir
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with(UNINSTALL_TEMP_PREFIX))
        && dir.starts_with(std::env::temp_dir());
    if is_ours {
        let _ = std::process::Command::new("cmd")
            .raw_arg(format!("/c ping -n 5 127.0.0.1 >nul & rmdir /s /q \"{}\"", dir.display()))
            .current_dir(std::env::temp_dir())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
}

// ---------- 启动 ----------

/// 没有 WebView2 就进不了界面，用系统弹窗说清楚。Win11 自带，老的 Win10 可能没有
fn webview2_ready() -> bool {
    const GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let paths = [
        (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{GUID}")),
        (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{GUID}")),
        (HKEY_CURRENT_USER, format!(r"Software\Microsoft\EdgeUpdate\Clients\{GUID}")),
    ];
    paths.iter().any(|(root, p)| {
        RegKey::predef(*root)
            .open_subkey(p)
            .and_then(|k| k.get_value::<String, _>("pv"))
            .is_ok_and(|v| !v.is_empty() && v != "0.0.0.0")
    })
}

fn tell_webview2_missing() {
    let text = HSTRING::from(
        "这台电脑缺少 Microsoft Edge WebView2 运行库，闪电全能下载需要它才能运行。\n\n要打开微软的下载页面吗？装好后再运行一次安装程序。",
    );
    let title = HSTRING::from(APP_NAME);
    let r = unsafe { MessageBoxW(Some(HWND::default()), &text, &title, MB_YESNO | MB_ICONINFORMATION) };
    if r == IDYES {
        let _ = std::process::Command::new("explorer")
            .arg("https://developer.microsoft.com/microsoft-edge/webview2/")
            .spawn();
    }
}

/// 卸载要删掉 uninstall.exe 自己和旁边的 dll，所以先把这两个拷到临时目录，从那里重新启动
fn relaunch_from_temp(machine: bool) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = me.parent().ok_or("找不到程序目录")?.to_path_buf();
    let temp = std::env::temp_dir().join(format!("{UNINSTALL_TEMP_PREFIX}{}", std::process::id()));
    fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
    let exe = temp.join(UNINSTALL_EXE);
    fs::copy(&me, &exe).map_err(|e| e.to_string())?;
    fs::copy(dir.join(LOADER_DLL), temp.join(LOADER_DLL)).map_err(|e| e.to_string())?;
    // 所有用户那份要管理员才删得动，卸载界面一开始就提权（用户已经点了卸载）
    if Places::get(machine).needs_admin() {
        let params = format!("--uninstall-run {} --all-users", quote(&dir.to_string_lossy()));
        return match run_as_admin(&exe, &params) {
            Ok(h) => {
                unsafe { let _ = CloseHandle(h); }
                Ok(())
            }
            Err(None) => Ok(()), // 用户取消了 UAC，就是不卸载了
            Err(Some(e)) => Err(e),
        };
    }
    let mut c = std::process::Command::new(&exe);
    c.arg("--uninstall-run").arg(&dir).current_dir(&temp);
    if machine {
        c.arg("--all-users");
    }
    c.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// 自己是不是装好的 uninstall.exe（名字对，旁边还有安装清单）
fn is_installed_uninstaller() -> bool {
    std::env::current_exe().is_ok_and(|me| {
        me.file_name().is_some_and(|n| n.eq_ignore_ascii_case(UNINSTALL_EXE))
            && me.parent().is_some_and(|d| d.join(MANIFEST).is_file())
    })
}

/// 这个安装目录是不是登记在「所有用户」下
fn installed_for_all_users() -> bool {
    let Some(me) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) else {
        return false;
    };
    installed(&Places::get(true)).is_some_and(|(_, d)| lower(&d) == lower(&me))
}

fn message(text: &str) {
    let (t, c) = (HSTRING::from(text), HSTRING::from(APP_NAME));
    unsafe { MessageBoxW(Some(HWND::default()), &t, &c, MB_ICONINFORMATION) };
}

fn main() {
    if portable_cleanup_from_launcher() {
        return;
    }
    let args: Vec<String> = std::env::args().collect();
    let value = |flag: &str| {
        args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(PathBuf::from)
    };
    let mut machine = args.iter().any(|a| a == "--all-users");
    // 用户在安装目录里直接双击 uninstall.exe 是不带参数的，和系统设置调用一样对待
    let double_clicked = args.len() == 1 && is_installed_uninstaller();
    if args.iter().any(|a| a == "--uninstall") || double_clicked {
        if double_clicked {
            machine = installed_for_all_users();
        }
        if let Err(e) = relaunch_from_temp(machine) {
            message(&format!("无法启动卸载程序：{e}"));
        }
        return;
    }
    let auto = value("--auto-dir").map(|dir| InstallOpts {
        dir: dir.display().to_string(),
        desktop: value("--auto-desktop").is_some_and(|v| v.as_os_str() == "1"),
        machine,
    });
    let mode = Mode {
        payload: value("--payload"),
        uninstall_dir: value("--uninstall-run"),
        machine,
        auto,
    };
    if mode.payload.is_none() && mode.uninstall_dir.is_none() {
        message("请运行安装包，或从系统设置里卸载。");
        return;
    }
    if !webview2_ready() {
        tell_webview2_missing();
        return;
    }
    let title = if mode.uninstall_dir.is_some() {
        format!("卸载{APP_NAME}")
    } else {
        format!("安装{APP_NAME}")
    };
    // WebView 的缓存跟 exe 放一起（都在临时目录，退出后整个清掉）：
    // 不然会在 %LOCALAPPDATA% 留下一个安装程序的文件夹
    let data_dir = std::env::current_exe()
        .ok()
        // 提权的这份和普通权限那份不能共用：WebView2 不允许不同权限共享缓存目录
        .and_then(|p| p.parent().map(|d| d.join(if is_admin() { "webview-admin" } else { "webview" })))
        .unwrap_or_else(|| std::env::temp_dir().join("shandian-setup-webview"));

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(mode)
        .invoke_handler(tauri::generate_handler![
            setup_info, pick_dir, running_in, needs_admin, elevate_install, install, uninstall,
            launch, open_folder, fit_window, ext_info, open_ext_page, reveal_ext, normalize_dir,
            close_app
        ])
        .setup(move |app| {
            #[cfg(debug_assertions)]
            let args = format!("{BROWSER_ARGS} --remote-debugging-port=9223");
            #[cfg(not(debug_assertions))]
            let args = BROWSER_ARGS;
            let win = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title(&title)
            // 高度由页面按内容算（fit_window），不同页面内容差很多，固定高度总有大片空白
            .inner_size(WINDOW_WIDTH, 320.0)
            // 固定大小靠最小/最大尺寸（fit_window 里设），不用 resizable(false)：不可调整的
            // 无边框窗口，系统仍会留一圈 6px 的边框区，页面铺不满，四周就是一圈白边
            .resizable(true)
            .maximizable(false)
            .visible(false)
            .decorations(false)
            .theme(Some(tauri::Theme::Light))
            .shadow(true)
            .center()
            .data_directory(data_dir.clone())
            .additional_browser_args(&args)
            .build()?;
            // 正常由页面量好高度后调 fit_window 显示。脚本万一没跑起来，也不能一直不出窗口
            let late = win.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(5));
                let window = late.clone();
                let _ = late.run_on_main_thread(move || {
                    if !ELEVATION_HANDOFF.load(std::sync::atomic::Ordering::SeqCst) && !window.is_visible().unwrap_or(true) {
                        let _ = window.show();
                    }
                });
            });
            // 和页面同色，页面加载前不闪白
            let _ = win.set_background_color(Some(tauri::window::Color(251, 250, 248, 255)));
            if let Ok(h) = win.hwnd() {
                let hwnd = HWND(h.0 as _);
                let round = DWMWCP_ROUND;
                // COLORREF 是 0x00BBGGRR：和主程序的 --line 差不多的浅灰。Win10 没有这两项，调用失败也无妨
                let border: u32 = 0x00D9_DADC;
                unsafe {
                    let _ = DwmSetWindowAttribute(
                        hwnd,
                        DWMWA_WINDOW_CORNER_PREFERENCE,
                        &round as *const _ as *const _,
                        std::mem::size_of_val(&round) as u32,
                    );
                    let _ = DwmSetWindowAttribute(
                        hwnd,
                        DWMWA_BORDER_COLOR,
                        &border as *const _ as *const _,
                        4,
                    );
                }
            }
            Ok(())
        })
        .build(tauri::generate_context!());
    let app = match app {
        Ok(a) => a,
        Err(e) => {
            message(&format!("安装程序启动失败：{e}"));
            return;
        }
    };
    app.run(|_, _| {});
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_preflight_preserves_existing_files() {
        // Run only with the existing setup sandbox: never create an install or
        // registry entry in the real Windows installation during verification.
        let sandbox = sandbox_dir().expect("set SHANDIAN_SETUP_SANDBOX for installer tests");
        assert!(sandbox.is_absolute());
        fs::create_dir_all(&sandbox).unwrap();
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let base = sandbox.join(format!("preflight-{}-{stamp}", std::process::id()));
        assert!(base.starts_with(&sandbox));
        let target = base.join(APP_NAME);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join(APP_EXE), b"old-program").unwrap();
        fs::write(target.join(".write-test"), b"user-sentinel").unwrap();
        let payload = base.join("payload.zip");
        {
            let file = fs::File::create(&payload).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer.start_file(APP_EXE, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(b"new-program").unwrap();
            writer.finish().unwrap();
        }
        assert!(prepare_install(&target, &payload, true).unwrap_err().contains("管理员确认"));
        assert_eq!(fs::read(target.join(APP_EXE)).unwrap(), b"old-program");
        let broken = base.join("broken.zip");
        fs::write(&broken, b"not-an-archive").unwrap();
        assert!(prepare_install(&target, &broken, false).is_err());
        let blocked = base.join("blocked-target");
        fs::write(&blocked, b"existing-file").unwrap();
        assert!(prepare_install(&blocked, &payload, false).is_err());
        assert_eq!(fs::read(&blocked).unwrap(), b"existing-file");
        let (archive, _) = prepare_install(&target, &payload, false).unwrap();
        drop(archive);
        assert_eq!(fs::read(target.join(APP_EXE)).unwrap(), b"old-program");
        assert_eq!(fs::read(target.join(".write-test")).unwrap(), b"user-sentinel");
        assert!(!fs::read_dir(&target).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".shandian-setup-probe-")));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn quote_survives_trailing_backslash() {
        assert_eq!(quote(r"C:\Program Files\App"), "\"C:\\Program Files\\App\"");
        assert_eq!(quote("D:\\"), "\"D:\"");
    }

    #[test]
    fn install_dir_is_always_dedicated() {
        let want = PathBuf::from(r"C:\闪电全能下载");
        assert_eq!(app_dir_of(r"C:\"), want);
        assert_eq!(app_dir_of(r"C:\闪电全能下载"), want);
        assert_eq!(app_dir_of(r"D:\Tools"), PathBuf::from(r"D:\Tools\闪电全能下载"));
    }

    #[test]
    fn manifest_paths_cannot_escape() {
        let d = Path::new(r"C:\app");
        assert_eq!(safe_join(d, r"a\b.txt"), Some(PathBuf::from(r"C:\app\a\b.txt")));
        assert_eq!(safe_join(d, r"..\evil.txt"), None);
        assert_eq!(safe_join(d, r"C:\Windows\x.dll"), None);
        assert_eq!(safe_join(d, ""), None);
        assert_eq!(safe_join(d, "."), None);
    }

    #[test]
    fn under_is_a_real_path_prefix() {
        let dirs = [PathBuf::from(r"C:\Apps\闪电全能下载")];
        assert!(is_under(&r"C:\Apps\闪电全能下载\a.exe".to_lowercase(), &dirs));
        // 名字开头相同的另一个目录不算
        assert!(!is_under(&r"C:\Apps\闪电全能下载2\a.exe".to_lowercase(), &dirs));
        assert!(points_into(r#""C:\Apps\闪电全能下载\闪电全能下载.exe" "%1""#, &dirs));
        assert!(!points_into(r#""D:\green\闪电全能下载.exe" "%1""#, &dirs));
    }

    #[test]
    fn data_removal_keeps_downloads() {
        let d = std::env::temp_dir().join(format!("sd-data-{}", std::process::id()));
        fs::create_dir_all(d.join("downloads")).unwrap();
        fs::create_dir_all(d.join("engine")).unwrap();
        fs::write(d.join("downloads").join("movie.mp4"), b"x").unwrap();
        fs::write(d.join("engine").join("db"), b"x").unwrap();
        fs::write(d.join("settings.json"), b"x").unwrap();
        remove_data_dir(&d);
        assert!(d.join("downloads").join("movie.mp4").exists());
        assert!(!d.join("engine").exists());
        assert!(!d.join("settings.json").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn portable_cleanup_rejects_unresolved_user_directories() {
        let places = Places {
            machine: false,
            sandbox: true,
            desktop: PathBuf::new(),
            programs: PathBuf::new(),
            program_root: PathBuf::new(),
            appdata: PathBuf::new(),
            localappdata: PathBuf::new(),
            reg_prefix: String::new(),
        };
        assert!(portable_cleanup_dirs(&places).is_err());
    }
}
