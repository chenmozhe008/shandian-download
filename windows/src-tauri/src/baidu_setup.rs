use crate::AppState;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader, Cursor, Read, Write},
    path::Path,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager, State, WebviewWindow};
use url::Url;

// The archive is the Windows build listed by Baidu's official installer.
const PACKAGE: &str =
    "https://issuecdn.baidupcs.com/issue/netdisk/ai-bdpan/win/3.8.9/bdpan-3.8.9-windows-amd64.zip";
const SHA256: &str = "ea1058ceb658446225f0cfc9a025221b3f8b3496af7da3b18bf87da3f47a645c";
const MAX_PACKAGE: u64 = 32 * 1024 * 1024;
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);
const WEB_AUTH_TIMEOUT: Duration = Duration::from_secs(600);
const WEB_CODE_TIMEOUT: Duration = Duration::from_secs(45);

struct Session {
    owner: String,
    cancelled: Arc<AtomicBool>,
}

static PREPARING: Mutex<Option<Session>> = Mutex::new(None);

struct WebAuthorization {
    ticket: String,
    login_url: Option<Url>,
    received: mpsc::Sender<(String, mpsc::Sender<bool>)>,
}

static WEB_AUTHORIZATION: Mutex<Option<WebAuthorization>> = Mutex::new(None);

pub fn claim_web_authorization(ticket: &str) -> Option<Url> {
    WEB_AUTHORIZATION
        .lock()
        .unwrap()
        .as_mut()
        .filter(|authorization| authorization.ticket == ticket)?
        .login_url
        .take()
}

pub fn complete_web_authorization(ticket: &str, code: &str) -> bool {
    if !valid_web_code(code) {
        return false;
    }
    let (verified, result) = mpsc::channel();
    {
        let mut state = WEB_AUTHORIZATION.lock().unwrap();
        if !state.as_ref().is_some_and(|authorization| {
            authorization.ticket == ticket && authorization.login_url.is_none()
        }) {
            return false;
        }
        let authorization = state.take().expect("matched web authorization");
        if authorization
            .received
            .send((code.into(), verified))
            .is_err()
        {
            return false;
        }
    }
    result
        .recv_timeout(WEB_CODE_TIMEOUT + Duration::from_secs(5))
        .unwrap_or(false)
}

pub fn close_web_authorization(ticket: &str) {
    let mut state = WEB_AUTHORIZATION.lock().unwrap();
    if state
        .as_ref()
        .is_some_and(|authorization| authorization.ticket == ticket)
    {
        *state = None;
    }
}

struct Prepare {
    window: WebviewWindow,
    cancelled: Arc<AtomicBool>,
}

impl Prepare {
    fn start(window: WebviewWindow) -> Result<Self, String> {
        let mut active = PREPARING.lock().unwrap();
        if active.is_some() {
            return Err("已有一个百度下载正在授权，请完成或关闭那个窗口".into());
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        *active = Some(Session {
            owner: window.label().into(),
            cancelled: cancelled.clone(),
        });
        Ok(Self { window, cancelled })
    }

    fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::Relaxed)
            || self
                .window
                .app_handle()
                .get_webview_window(self.window.label())
                .is_none()
        {
            return Err("已取消百度网盘授权".into());
        }
        Ok(())
    }

    fn progress(&self, text: &str, qr: Option<&str>) {
        let _ = self.window.emit_to(
            self.window.label(),
            "baidu-prepare-progress",
            json!({ "text": text, "qr": qr }),
        );
    }
}

impl Drop for Prepare {
    fn drop(&mut self) {
        *PREPARING.lock().unwrap() = None;
    }
}

#[tauri::command]
pub fn cancel_baidu_prepare(window: WebviewWindow) {
    if let Some(session) = PREPARING
        .lock()
        .unwrap()
        .as_ref()
        .filter(|s| s.owner == window.label())
    {
        session.cancelled.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub async fn prepare_baidu_share(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let prepare = Prepare::start(window)?;
    let managed = state.baidu.tool_path().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        prepare.progress("正在检查百度网盘授权…", None);
        if crate::baidu::executable(&managed)
            .as_ref()
            .is_some_and(|bin| authorized(bin))
        {
            return Ok(true);
        }
        if !managed.is_file() {
            prepare.progress("正在准备百度网盘下载工具…", None);
            install(&managed, &prepare)?;
        }
        prepare.check()?;
        Ok(false)
    })
    .await
    .map_err(|_| "无法准备百度网盘下载")?
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaiduAuthorization {
    Device,
    Web,
}

#[tauri::command]
pub async fn authorize_baidu_share(
    window: WebviewWindow,
    state: State<'_, AppState>,
    method: BaiduAuthorization,
) -> Result<(), String> {
    let prepare = Prepare::start(window)?;
    let bin = state.baidu.tool_path().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        prepare.check()?;
        if authorized(&bin) {
            return Ok(());
        }
        match method {
            BaiduAuthorization::Device => {
                prepare.progress("正在获取百度网盘授权二维码…", None);
                authorize_device(&bin, &prepare)
            }
            BaiduAuthorization::Web => authorize_web(&bin, &prepare),
        }
    })
    .await
    .map_err(|_| "百度网盘授权未完成，请重试")?
}

fn install(path: &Path, prepare: &Prepare) -> Result<(), String> {
    let response = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(60))
        .build()
        .and_then(|client| client.get(PACKAGE).send())
        .and_then(|response| response.error_for_status())
        .map_err(|_| "百度网盘工具下载失败，请检查网络后重试")?;
    let mut data = Vec::new();
    response
        .take(MAX_PACKAGE + 1)
        .read_to_end(&mut data)
        .map_err(|_| "百度网盘工具下载中断，请重试")?;
    if data.len() as u64 > MAX_PACKAGE || format!("{:x}", Sha256::digest(&data)) != SHA256 {
        return Err("百度网盘工具校验失败，请重试".into());
    }
    prepare.check()?;
    let mut zip = zip::ZipArchive::new(Cursor::new(data)).map_err(|_| "无法读取百度网盘工具")?;
    let mut exe = zip
        .by_name("bdpan/bdpan.exe")
        .map_err(|_| "找不到百度网盘工具")?;
    let tmp = path.with_extension(format!("{:016x}.tmp", rand::random::<u64>()));
    let result = (|| -> std::io::Result<()> {
        fs::create_dir_all(
            path.parent()
                .expect("tool is inside the app data directory"),
        )?;
        let mut output = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp)?;
        std::io::copy(&mut exe, &mut output)?;
        output.sync_all()?;
        drop(output);
        crate::baidu::move_file(&tmp, path, 1)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|_| "无法安装百度网盘工具，请检查下载器的数据目录".into())
}

pub fn authorized(bin: &Path) -> bool {
    let Ok(output) = crate::baidu::cli(bin).args(["whoami", "--json"]).output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    serde_json::from_slice::<Value>(&output.stdout).is_ok_and(|status| {
        status["authenticated"].as_bool() == Some(true)
            && status["has_valid_token"].as_bool() == Some(true)
    })
}

fn is_baidu_auth_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("openapi.baidu.com")
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
}

fn is_device_qr(url: &Url) -> bool {
    is_baidu_auth_url(url) && url.path().starts_with("/device/qrcode/")
}

fn web_authorization_url(output: &[u8]) -> Option<Url> {
    std::str::from_utf8(output)
        .ok()?
        .lines()
        .filter_map(|line| Url::parse(line.trim()).ok())
        .find(|url| is_baidu_auth_url(url) && url.path() == "/oauth/2.0/authorize")
}

fn authorize_web(bin: &Path, prepare: &Prepare) -> Result<(), String> {
    let output = crate::baidu::cli(bin)
        .args(["login", "--get-auth-url", "--accept-disclaimer"])
        .output()
        .map_err(|_| "无法获取百度网页授权链接，请重试")?;
    let url = output
        .status
        .success()
        .then(|| web_authorization_url(&output.stdout))
        .flatten()
        .ok_or("百度未返回有效的网页授权链接，请重试")?;
    prepare.check()?;
    let ticket = format!("{:032x}", rand::random::<u128>());
    let (received, code) = mpsc::channel();
    *WEB_AUTHORIZATION.lock().unwrap() = Some(WebAuthorization {
        ticket: ticket.clone(),
        login_url: Some(url),
        received,
    });

    let result = (|| {
        crate::engine::open_url(format!("http://127.0.0.1:6800/baidu-authorize#{ticket}"))
            .map_err(|_| "无法打开百度授权网页，请重试")?;
        prepare.progress("在百度网页同意授权，下载会自动继续", None);
        let start = Instant::now();
        let mut notified = false;
        while start.elapsed() < WEB_AUTH_TIMEOUT {
            prepare.check()?;
            match code.recv_timeout(Duration::from_millis(200)) {
                Ok((code, verified)) => {
                    prepare.progress("正在确认百度网盘授权…", None);
                    let result = authorize_web_code(bin, prepare, &code);
                    let _ = verified.send(result.is_ok());
                    return result;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("百度授权网页已关闭，请重新选择授权方式".into());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if !notified && start.elapsed() >= Duration::from_secs(8) {
                notified = true;
                if WEB_AUTHORIZATION
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|auth| auth.login_url.is_some())
                {
                    prepare.progress(
                        "若网页未跳转，请重新加载「闪电全能下载」扩展，再刷新授权页",
                        None,
                    );
                }
            }
        }
        Err("网页授权已超时，请重新选择授权方式".into())
    })();
    close_web_authorization(&ticket);
    result
}

fn valid_web_code(code: &str) -> bool {
    code.len() == 32 && code.bytes().all(|c| c.is_ascii_hexdigit())
}

fn authorize_web_code(bin: &Path, prepare: &Prepare, code: &str) -> Result<(), String> {
    prepare.check()?;
    if !valid_web_code(code) {
        return Err("未收到有效的百度网页授权结果".into());
    }
    let mut child = crate::baidu::cli(bin)
        .args(["login", "--set-code-stdin", "--accept-disclaimer"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|_| "无法提交百度网页授权码，请重试")?;
    let result = (|| {
        let mut stdin = child.stdin.take().ok_or("无法提交百度网页授权码")?;
        writeln!(stdin, "{code}").map_err(|_| "无法提交百度网页授权码")?;
        drop(stdin);
        let start = Instant::now();
        while start.elapsed() < WEB_CODE_TIMEOUT {
            prepare.check()?;
            if let Some(status) = child.try_wait().map_err(|_| "无法检查百度网盘授权结果")?
            {
                return if status.success() && authorized(bin) {
                    Ok(())
                } else {
                    Err("授权码无效或已过期，请重新打开授权网页".into())
                };
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err("验证授权码超时，请重新打开授权网页".into())
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn authorize_device(bin: &Path, prepare: &Prepare) -> Result<(), String> {
    let mut child = crate::baidu::cli(bin)
        .args(["login", "--device-code", "--accept-disclaimer"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|_| "无法启动百度网盘授权")?;
    let result = (|| {
        // The confirmation pane explains this scope before the user clicks Start.
        // Device mode asks for this acknowledgment on stdin on first use.
        child
            .stdin
            .take()
            .ok_or("无法准备百度网盘授权")?
            .write_all(b"yes\n")
            .map_err(|_| "百度网盘授权未能打开")?;
        let output = child.stdout.take().ok_or("无法读取百度网盘授权状态")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut output = BufReader::new(output);
            for line in output
                .by_ref()
                .take(128 * 1024)
                .split(b'\n')
                .map_while(Result::ok)
            {
                let text = String::from_utf8_lossy(&line);
                if let Some(qr) = text
                    .split_whitespace()
                    .filter_map(|part| Url::parse(part).ok())
                    .find(is_device_qr)
                {
                    let _ = tx.send(qr);
                }
            }
            let _ = std::io::copy(&mut output, &mut std::io::sink());
        });
        let start = Instant::now();
        while start.elapsed() < AUTH_TIMEOUT {
            prepare.check()?;
            if let Ok(qr) = rx.try_recv() {
                prepare.progress("用百度 App 扫码授权，完成后自动开始下载", Some(qr.as_str()));
            }
            if let Some(status) = child.try_wait().map_err(|_| "无法检查百度网盘授权结果")?
            {
                return if status.success() && authorized(bin) {
                    Ok(())
                } else {
                    Err("百度网盘授权未完成，请重新扫码".into())
                };
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err("百度网盘授权已超时，请重新扫码".into())
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_qr_requires_the_provider_origin_and_path() {
        for (url, accepted) in [
            ("https://openapi.baidu.com/device/qrcode/example", true),
            ("http://openapi.baidu.com/device/qrcode/example", false),
            (
                "https://openapi.baidu.com.example/device/qrcode/example",
                false,
            ),
            ("https://openapi.baidu.com/oauth/2.0/authorize", false),
        ] {
            assert_eq!(is_device_qr(&Url::parse(url).unwrap()), accepted);
        }
    }

    #[test]
    fn web_login_accepts_only_the_official_authorization_endpoint() {
        let output = b"notice\nhttps://openapi.baidu.com/oauth/2.0/authorize?response_type=code\n";
        assert!(web_authorization_url(output).is_some());
        for url in [
            "https://openapi.baidu.com.example/oauth/2.0/authorize",
            "http://openapi.baidu.com/oauth/2.0/authorize",
            "https://openapi.baidu.com/device/qrcode/example",
            "https://user@openapi.baidu.com/oauth/2.0/authorize",
        ] {
            assert!(web_authorization_url(url.as_bytes()).is_none());
        }
        assert!(valid_web_code("0123456789abcdefABCDEF0123456789"));
        assert!(!valid_web_code("abc123"));
        assert!(!valid_web_code("0123456789abcdefABCDEF012345678z"));
    }

    #[test]
    fn browser_exchange_requires_one_claim_then_one_completion() {
        let ticket = "valid";
        let expected = "0123456789abcdef0123456789abcdef";
        let (sender, receiver) = mpsc::channel();
        *WEB_AUTHORIZATION.lock().unwrap() = Some(WebAuthorization {
            ticket: ticket.into(),
            login_url: Url::parse("https://openapi.baidu.com/oauth/2.0/authorize").ok(),
            received: sender,
        });
        assert!(!complete_web_authorization(ticket, expected));
        assert!(claim_web_authorization("wrong").is_none());
        assert!(claim_web_authorization(ticket).is_some());
        assert!(claim_web_authorization(ticket).is_none());
        assert!(!complete_web_authorization(ticket, "invalid"));
        let worker = std::thread::spawn(move || {
            let (code, verified) = receiver.recv().unwrap();
            assert_eq!(code, expected);
            verified.send(true).unwrap();
        });
        assert!(complete_web_authorization(ticket, expected));
        assert!(!complete_web_authorization(ticket, expected));
        worker.join().unwrap();
    }
}
