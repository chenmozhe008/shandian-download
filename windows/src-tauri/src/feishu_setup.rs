use crate::feishu::{self, FeishuCli, FeishuError, READ_SCOPES, SCOPES_REQUIRED};
use crate::AppState;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, BufRead, BufReader, Cursor, Read},
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

// Official Windows release, verified before installing into the app's own data directory.
const PACKAGE: &str = "https://github.com/larksuite/cli/releases/download/v1.0.100/lark-cli-1.0.100-windows-amd64.zip";
const SHA256: &str = "93118ca1992b4e5f1a40c14301f6fbde703d5e9160a9113099e409b4bd1c1d8d";
const MAX_PACKAGE: u64 = 25 * 1024 * 1024;
const MAX_EXECUTABLE: u64 = 72 * 1024 * 1024;
const AUTH_TIMEOUT: Duration = Duration::from_secs(600);

struct Session {
    owner: String,
    cancelled: Arc<AtomicBool>,
}

static ACTIVE: Mutex<Option<Session>> = Mutex::new(None);

struct Setup {
    window: WebviewWindow,
    cancelled: Arc<AtomicBool>,
}

impl Setup {
    fn start(window: WebviewWindow) -> Result<Self, String> {
        let mut active = ACTIVE.lock().unwrap();
        if active.is_some() {
            return Err("另一个下载窗口正在授权飞书，请先完成或关闭它".into());
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        *active = Some(Session {
            owner: window.label().to_string(),
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
            return Err("已取消飞书授权".into());
        }
        Ok(())
    }

    fn progress(&self, text: &str) {
        let _ = self
            .window
            .emit_to(self.window.label(), "feishu-authorization-progress", text);
    }

    fn authorize(&self, cli: &FeishuCli, registration: bool) -> Result<(), String> {
        let login = ["auth", "login", "--scope", READ_SCOPES, "--json"];
        let setup = [
            "config", "init", "--new", "--brand", "feishu", "--lang", "zh",
        ];
        let (args, waiting, failure): (&[&str], &str, &str) = if registration {
            (
                &setup,
                "在飞书网页确认首次开通，闪电会继续下一步",
                "飞书首次开通未完成，请重试",
            )
        } else {
            (
                &login,
                "在飞书网页同意文档和附件授权，完成后会自动读取",
                "飞书授权未完成，请重试",
            )
        };
        self.check()?;
        let mut child = cli
            .command()
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| "无法启动飞书授权，请重试")?;
        let (sender, receiver) = mpsc::channel();
        relay_lines(child.stdout.take().expect("piped stdout"), 0, sender.clone());
        relay_lines(child.stderr.take().expect("piped stderr"), 1, sender);
        let started = Instant::now();
        let mut opened = false;
        let mut output = [String::new(), String::new()];
        let result = (|| loop {
            self.check()?;
            if started.elapsed() >= AUTH_TIMEOUT
                || (!opened && started.elapsed() >= Duration::from_secs(45))
            {
                return Err("飞书授权已超时，请重试".into());
            }
            match receiver.recv_timeout(Duration::from_millis(150)) {
                Ok((stream, line)) => {
                    output[stream].push_str(&line);
                    output[stream].push('\n');
                    if !opened {
                        if let Some(url) = authorization_url(&line, registration) {
                            self.check()?;
                            crate::engine::open_url(url.into())
                                .map_err(|_| "无法打开飞书授权网页，请重试")?;
                            opened = true;
                            self.progress(waiting);
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if let Some(status) = child.try_wait().map_err(|_| failure)? {
                if status.success() {
                    return Ok(());
                }
                while let Ok((stream, line)) = receiver.recv_timeout(Duration::from_millis(150)) {
                    output[stream].push_str(&line);
                    output[stream].push('\n');
                }
                let reason = output
                    .iter()
                    .find_map(|stream| feishu::cli_permission_issue(stream.as_bytes()))
                    .unwrap_or(failure);
                return Err(reason.into());
            }
        })();
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        result
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        *ACTIVE.lock().unwrap() = None;
    }
}

fn relay_lines(
    reader: impl Read + Send + 'static,
    stream: usize,
    sender: mpsc::Sender<(usize, String)>,
) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        for line in reader
            .by_ref()
            .take(256 * 1024)
            .lines()
            .map_while(Result::ok)
        {
            if sender.send((stream, line)).is_err() {
                break;
            }
        }
        let _ = io::copy(&mut reader, &mut io::sink());
    });
}

fn authorization_url(line: &str, registration: bool) -> Option<Url> {
    let url = if registration {
        Url::parse(line.trim()).ok()?
    } else {
        let event: Value = serde_json::from_str(line).ok()?;
        if event["event"] != "device_authorization" {
            return None;
        }
        Url::parse(event["verification_uri_complete"].as_str()?).ok()?
    };
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let official = if registration {
        url.host_str() == Some("open.feishu.cn") && url.path() == "/page/cli"
    } else {
        matches!(
            url.host_str(),
            Some("open.feishu.cn" | "accounts.feishu.cn" | "passport.feishu.cn")
        )
    };
    official.then_some(url)
}

fn install(path: &Path, setup: &Setup) -> Result<(), String> {
    let response = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(4))
        .timeout(Duration::from_secs(60))
        .build()
        .and_then(|client| client.get(PACKAGE).send())
        .and_then(|response| response.error_for_status())
        .map_err(|_| "飞书下载工具获取失败，请检查网络后重试")?;
    let mut data = Vec::new();
    response
        .take(MAX_PACKAGE + 1)
        .read_to_end(&mut data)
        .map_err(|_| "飞书下载工具下载中断，请重试")?;
    if data.len() as u64 > MAX_PACKAGE || format!("{:x}", Sha256::digest(&data)) != SHA256 {
        return Err("飞书下载工具校验失败，请重试".into());
    }
    setup.check()?;
    let mut zip = zip::ZipArchive::new(Cursor::new(data)).map_err(|_| "无法读取飞书下载工具")?;
    let mut executable = zip
        .by_name("lark-cli.exe")
        .map_err(|_| "找不到飞书下载工具")?;
    if executable.size() > MAX_EXECUTABLE {
        return Err("飞书下载工具大小异常".into());
    }
    let temporary = path.with_extension(format!("{:016x}.tmp", rand::random::<u64>()));
    let result = (|| {
        fs::create_dir_all(path.parent().expect("tool directory"))?;
        let mut output = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        io::copy(&mut executable, &mut output)?;
        output.sync_all()?;
        drop(output);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|_| "无法安装飞书下载工具，请检查闪电的数据目录".into())
}

#[tauri::command]
pub async fn authorize_feishu(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), FeishuError> {
    let setup = Setup::start(window).map_err(FeishuError::authorization_failure)?;
    let local = state.local.clone();
    tauri::async_runtime::spawn_blocking(move || {
        authorize(&setup, &local)
            .map_err(|message| FeishuError::authorization_failure(message).with_permissions(&local))
    })
    .await
    .map_err(|_| FeishuError::authorization_failure("飞书授权未完成，请重试"))?
}

fn authorize(setup: &Setup, local: &Path) -> Result<(), String> {
    setup.progress("正在检查飞书授权…");
    if feishu::executable(&local).is_some_and(|cli| cli.authorized()) {
        return Ok(());
    }
    let cli = FeishuCli::managed(&local);
    if !cli.bin.is_file() {
        setup.progress("正在准备飞书下载工具…");
        install(&cli.bin, &setup)?;
    }
    setup.check()?;
    if !cli.configured() {
        setup.progress("正在打开飞书首次开通页面…");
        setup.authorize(&cli, true)?;
    }
    setup.check()?;
    setup.progress("正在打开飞书账号授权页面…");
    if let Err(error) = setup.authorize(&cli, false) {
        setup.check()?;
        if cli.has_user_identity() && !cli.has_required_scopes() {
            return Err(SCOPES_REQUIRED.into());
        }
        return Err(error);
    }
    setup.check()?;
    if !cli.authorized() {
        return Err(SCOPES_REQUIRED.into());
    }
    Ok(())
}

#[tauri::command]
pub fn cancel_feishu_authorization(window: WebviewWindow) {
    let active = ACTIVE.lock().unwrap();
    if let Some(session) = active
        .as_ref()
        .filter(|session| session.owner == window.label())
    {
        session.cancelled.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_opens_feishu_registration_and_device_consent_pages() {
        assert!(authorization_url("  https://open.feishu.cn/page/cli?example=1  ", true).is_some());
        assert!(authorization_url("https://open.feishu.cn.example/page/cli", true).is_none());
        assert!(
            authorization_url("https://open.feishu.cn@elsewhere.test/page/cli", true).is_none()
        );
        let event = |url: &str| {
            serde_json::json!({
                "event": "device_authorization",
                "verification_uri_complete": url
            })
            .to_string()
        };
        assert!(
            authorization_url(&event("https://accounts.feishu.cn/oauth/device"), false).is_some()
        );
        assert!(
            authorization_url(&event("http://accounts.feishu.cn/oauth/device"), false).is_none()
        );
        assert!(authorization_url(&event("https://example.test/oauth/device"), false).is_none());
    }
}
