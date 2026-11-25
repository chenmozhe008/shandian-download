use crate::browser_support::{browser_exe, extension_profile};
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    os::windows::ffi::OsStrExt,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
use windows::{
    core::{Interface, BSTR},
    Win32::{
        System::{
            Com::{CoCreateGuid, CoCreateInstance, CoInitializeEx, CoUninitialize, IServiceProvider, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED},
            Variant::VARIANT,
        },
        UI::Shell::{
            IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellWindows, ShellWindows,
            SID_STopLevelBrowser, SVGIO_BACKGROUND, SWC_DESKTOP, SWFO_NEEDDISPATCH,
        },
    },
};

const EXTENSION_ID: &str = "pggchniffhkcgigajlapkboikmgdipel";
const EXTENSION_ORIGIN: &str = "chrome-extension://pggchniffhkcgigajlapkboikmgdipel";
const GATEWAY: &str = "127.0.0.1:6800";

pub(super) fn open(browser: &'static str) -> Result<bool, String> {
    let exe = browser_exe(browser).ok_or("没找到这个浏览器")?;
    let Some(profile) = extension_profile(browser, EXTENSION_ID) else {
        return Ok(false);
    };
    // The installed extension already uses this landing page and one-use token in the App.
    let listener = match TcpListener::bind(GATEWAY) {
        Ok(listener) => listener,
        Err(_) => return Ok(false),
    };
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let nonce = unsafe { CoCreateGuid() }.map_err(|e| e.to_string())?;
    let nonce = format!("{:032x}", nonce.to_u128());
    let page = format!("http://{GATEWAY}/channel-open#action=extensions&wechat-auto={nonce}");
    if crate::is_admin() {
        let arguments = format!("--profile-directory=\"{}\" {page}", profile.profile);
        open_from_desktop(&exe, &arguments).map_err(|e| format!("打不开浏览器：{e}"))?;
    } else {
        Command::new(exe)
            .arg(format!("--profile-directory={}", profile.profile))
            .arg(page)
            .spawn()
            .map_err(|e| format!("打不开浏览器：{e}"))?;
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((stream, _)) => {
                if serve(stream, &nonce).unwrap_or(false) {
                    return Ok(true);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(30));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(false)
}

pub(super) fn available(browser: &'static str) -> bool {
    extension_profile(browser, EXTENSION_ID).is_some()
}

// Explorer launches the browser with the desktop user's permissions even after installer elevation.
fn open_from_desktop(exe: &Path, arguments: &str) -> windows::core::Result<()> {
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let _apartment = Apartment;
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
        let empty = VARIANT::default();
        let mut hwnd = 0;
        let desktop = windows.FindWindowSW(&VARIANT::from(0i32), &empty, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)?;
        let services: IServiceProvider = desktop.cast()?;
        let browser: IShellBrowser = services.QueryService(&SID_STopLevelBrowser)?;
        let folder: IShellFolderViewDual = browser.QueryActiveShellView()?.GetItemObject(SVGIO_BACKGROUND)?;
        let shell: IShellDispatch2 = folder.Application()?.cast()?;
        let file = BSTR::from_wide(&exe.as_os_str().encode_wide().collect::<Vec<_>>());
        shell.ShellExecute(&file, &VARIANT::from(arguments), &empty, &empty, &VARIANT::from(1i32))
    }
}

fn serve(mut stream: TcpStream, nonce: &str) -> std::io::Result<bool> {
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    let mut request = BufReader::new((&stream).take(8192));
    let mut line = String::new();
    request.read_line(&mut line)?;
    let landing = line.starts_with("GET /channel-open HTTP/");
    let post = line.starts_with("POST /extension/wechat-auto HTTP/");
    let options = line.starts_with("OPTIONS /extension/wechat-auto HTTP/");
    let mut allowed = false;
    let mut length = 0;
    loop {
        line.clear();
        if request.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("origin") {
                allowed = value.trim() == EXTENSION_ORIGIN;
            } else if key.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().unwrap_or(0);
            }
        }
    }
    let claimed = if post && allowed && (1..=1024).contains(&length) {
        let mut data = vec![0; length];
        request.read_exact(&mut data)?;
        serde_json::from_slice::<Value>(&data).is_ok_and(|v| {
            v["token"].as_str() == Some(nonce) && v["share"] == "shandian:extension-manager"
        })
    } else {
        false
    };
    let (kind, body): (&str, &[u8]) = if landing {
        ("text/html; charset=utf-8", include_bytes!("../../../ui/channel-open.html"))
    } else if claimed {
        ("application/json", br#"{"ok":true}"#)
    } else {
        ("application/json", br#"{"ok":false}"#)
    };
    let code = if landing || ((post || options) && allowed) { "200 OK" } else { "404 Not Found" };
    write!(stream, "HTTP/1.1 {code}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len())?;
    if allowed {
        write!(stream, "Access-Control-Allow-Origin: {EXTENSION_ORIGIN}\r\nAccess-Control-Allow-Headers: Content-Type\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Private-Network: true\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    stream.write_all(body)?;
    Ok(claimed)
}
