// 闪电全能下载的安装包外壳：用户下载到的就是这一个 exe。
//
// 界面程序（../app）用 MinGW 编译，依赖旁边的 WebView2Loader.dll，自己没法做成单文件。
// 所以这里只用标准库，不依赖 WebView2，把界面程序、dll、安装数据解到临时目录再启动，
// 等它退出后把临时目录清掉。
#![windows_subsystem = "windows"]

use std::fs;
use std::path::Path;
use std::process::Command;

static APP: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/embed/shandian-setup.exe"));
static DLL: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/embed/WebView2Loader.dll"));
static ZIP: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/embed/payload.zip"));

#[link(name = "user32")]
extern "system" {
    fn MessageBoxW(hwnd: *mut core::ffi::c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
}

fn alert(text: &str) {
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    const MB_ICONERROR: u32 = 0x10;
    unsafe {
        MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("闪电全能下载").as_ptr(), MB_ICONERROR);
    }
}

fn run(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let exe = dir.join("shandian-setup.exe");
    let zip = dir.join("payload.zip");
    fs::write(&exe, APP).map_err(|e| e.to_string())?;
    fs::write(dir.join("WebView2Loader.dll"), DLL).map_err(|e| e.to_string())?;
    fs::write(&zip, ZIP).map_err(|e| e.to_string())?;
    Command::new(&exe)
        .arg("--payload")
        .arg(&zip)
        // Preserve the installer's existing automatic-update options when the
        // verified single-file package is launched from a deployment command.
        .args(std::env::args_os().skip(1))
        .current_dir(dir)
        .status()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn main() {
    let dir = std::env::temp_dir().join(format!("shandian-setup-{}", std::process::id()));
    if let Err(e) = run(&dir) {
        alert(&format!("安装程序启动失败：{e}\n\n可能是磁盘空间不足，或杀毒软件拦截了临时文件。"));
    }
    // 界面退出后 exe 和 WebView2 的子进程还要一小会儿才放开文件
    for _ in 0..40 {
        if fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}
