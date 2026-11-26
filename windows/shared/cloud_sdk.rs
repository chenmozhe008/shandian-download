use serde_json::{json, Value};
use std::os::windows::{io::AsRawHandle, process::CommandExt};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub struct Sdk {
    pub client: reqwest::Client,
    pub base: String,
    token: String,
    child: Mutex<Child>,
    job: usize,
}
impl Drop for Sdk {
    fn drop(&mut self) {
        let _ = self.child.lock().map(|mut c| {
            let _ = c.kill();
            let _ = c.wait();
        });
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job as _);
        }
    }
}
impl Sdk {
    pub fn alive(&self) -> bool {
        self.child
            .lock()
            .map(|mut c| matches!(c.try_wait(), Ok(None)))
            .unwrap_or(false)
    }
    pub async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, String> {
        let mut req = self
            .client
            .request(method, format!("{}{}", self.base, path))
            .header("Authorization", &self.token);
        if let Some(v) = body {
            req = req.json(v)
        }
        let r = req
            .send()
            .await
            .map_err(|_| "云盘组件暂时不可用".to_string())?;
        let status = r.status();
        let v: Value = r
            .json()
            .await
            .map_err(|_| "云盘组件返回信息不完整".to_string())?;
        if !status.is_success() || v["code"].as_i64() != Some(200) {
            let message = v["message"].as_str().unwrap_or("网盘拒绝了请求");
            if status.is_success() && matches!(path, "/api/fs/list" | "/api/fs/native_offline/storage_limits")
                && provider_auth_expired(message)
            {
                return Err(ACCOUNT_EXPIRED.into());
            }
            return Err(safe_error(message));
        }
        Ok(v["data"].clone())
    }

    pub async fn remove_storage(&self, id: u64) -> Result<(), String> {
        let route = format!("/api/admin/storage/delete?id={id}");
        match self.call(reqwest::Method::POST, &route, None).await {
            Ok(_) => Ok(()),
            Err(error) if error == "failed get storage: record not found" => Ok(()),
            Err(_) => Err("暂时无法解除网盘关联，请稍后再试".into()),
        }
    }
}

pub const ACCOUNT_EXPIRED: &str = "网盘关联失效，请重新关联";

fn provider_auth_expired(message: &str) -> bool {
    let text = message.to_lowercase().replace(['_', '-'], " ");
    [
        "invalid grant", "invalid token", "invalid refresh token", "token expired",
        "token is expired", "token has expired", "token is invalid", "token is revoked",
        "session expired", "cookie expired", "login expired", "session has expired",
        "not logged in", "status=401", "status: 401", "login failed: provide a valid access token, or refresh token",
        "登录已过期", "登录过期", "登录失效", "授权已过期", "授权过期", "授权失效", "请重新登录", "未登录",
    ].iter().any(|phrase| text.contains(*phrase))
}
pub fn safe_error(raw: &str) -> String {
    let text = regex::Regex::new(r#"https?://[^\s\"<>]+"#)
        .unwrap()
        .replace_all(raw, "<下载地址>");
    if regex::Regex::new(
        r"(?i)access_token|refresh_token|authorization|client_secret|cookie|eyJ[A-Za-z0-9_-]{12}",
    )
    .unwrap()
    .is_match(&text)
    {
        return "网盘登录或授权验证失败，请重新关联账号".into();
    }
    text.chars().take(220).collect()
}
pub fn private_write(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?
    }
    let tmp = path.with_extension(format!("{}.tmp", rand::random::<u64>()));
    std::fs::write(&tmp, serde_json::to_vec(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    // MoveFileEx replaces atomically on Windows; never remove the live state first.
    use std::os::windows::ffi::OsStrExt;
    let from: Vec<u16> = tmp.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        let _ = std::fs::remove_file(tmp);
        return Err("保存云盘状态失败".into());
    };
    Ok(())
}

fn runtime_binary() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("SHANDIAN_CLOUD_DRIVER") {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Ok(p);
        }
    }
    let p = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("程序目录不可用")?
        .join("cloud-driver.exe");
    if !p.is_file() {
        return Err("云盘组件尚未安装，请使用完整安装包更新".into());
    }
    Ok(p)
}
fn spawn(data: PathBuf) -> Result<(Child, usize, String, u16), String> {
    std::fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    let binary = runtime_binary()?;
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    drop(listener);
    let file = data.join("config.json");
    let mut config:Value=std::fs::read(&file).ok().and_then(|b|serde_json::from_slice(&b).ok()).unwrap_or_else(||json!({
        "database":{"type":"sqlite3","db_file":data.join("data.db"),"table_prefix":"x_"},
        "jwt_secret":format!("{:032x}{:032x}",rand::random::<u128>(),rand::random::<u128>()),
        "temp_dir":data.join("temp"),"bleve_dir":data.join("index"),"max_concurrency":8,
        "tasks":{"download":{"workers":2,"max_retry":0,"task_persistant":false},"transfer":{"workers":2,"max_retry":0,"task_persistant":false}}
    }));
    config["scheme"] = json!({"address":"127.0.0.1","http_port":port,"https_port":-1});
    config["log"] = json!({"enable":false});
    private_write(&file, &config)?;
    let output = Command::new(&binary)
        .args(["admin", "token", "--data"])
        .arg(&data)
        .creation_flags(0x08000000)
        .stderr(Stdio::null())
        .output()
        .map_err(|_| "云盘组件初始化失败")?;
    if !output.status.success() {
        return Err("云盘组件初始化失败".into());
    }
    let token = String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|l| {
            l.split_once("Admin token:")
                .map(|(_, v)| v.trim().to_string())
        })
        .filter(|v| !v.is_empty())
        .ok_or("无法取得私有云盘组件的连接状态")?;
    let mut child = Command::new(&binary)
        .args(["server", "--data"])
        .arg(&data)
        .creation_flags(0x08000000)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "启动云盘组件失败")?;
    use windows_sys::Win32::System::JobObjects::*;
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let guarded = !job.is_null()
        && unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                std::mem::size_of_val(&limits) as u32,
            )
        } != 0
        && unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as _) } != 0;
    if !guarded {
        let _ = child.kill();
        let _ = child.wait();
        if !job.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(job);
            }
        }
        return Err("无法管理云盘组件进程，已停止启动".into());
    }
    Ok((child, job as usize, token, port))
}

pub async fn start(data: PathBuf) -> Result<Arc<Sdk>, String> {
    let (child, job, token, port) = tauri::async_runtime::spawn_blocking(move || spawn(data))
        .await
        .map_err(|_| "云盘组件启动中断")??;
    let sdk = Arc::new(Sdk {
        client: reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(45))
            .build()
            .map_err(|e| e.to_string())?,
        base: format!("http://127.0.0.1:{port}"),
        token,
        child: Mutex::new(child),
        job,
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match sdk
            .call(reqwest::Method::GET, "/api/public/init_status", None)
            .await
        {
            Ok(v) => {
                if !v["initialized"].as_bool().unwrap_or(false) {
                    sdk.call(reqwest::Method::POST,"/api/public/init/setup",Some(&json!({"username":"shandian","password":format!("{:032x}{:032x}",rand::random::<u128>(),rand::random::<u128>()),"site_title":"闪电云盘组件"}))).await?;
                }
                return Ok(sdk);
            }
            Err(_) if Instant::now() < deadline && sdk.alive() => {
                tokio::time::sleep(Duration::from_millis(50)).await
            }
            Err(_) => return Err("云盘组件没有正常就绪".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "boots the explicitly supplied bundled private runtime in an isolated directory"]
    fn private_runtime_boots_and_stops_without_credentials() {
        let root =
            std::env::temp_dir().join(format!("闪电 cloud smoke {:016x}", rand::random::<u64>()));
        let run = tauri::async_runtime::block_on(async {
            let sdk = start(root.clone()).await?;
            let drivers = sdk
                .call(reqwest::Method::GET, "/api/admin/driver/list", None)
                .await?;
            let names = drivers.to_string();
            for name in ["GuangYaPan", "115 Open", "123Pan", "Thunder"] {
                if !names.contains(name) {
                    return Err(format!("driver missing: {name}"));
                }
            }
            let base = sdk.base.clone();
            drop(sdk);
            let closed = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap()
                .get(format!("{base}/ping"))
                .send()
                .await
                .is_err();
            if !closed {
                return Err("private child still running".into());
            }
            Ok::<(), String>(())
        });
        if root.exists() {
            let resolved = root.canonicalize().unwrap();
            assert!(resolved.starts_with(std::env::temp_dir().canonicalize().unwrap()));
            std::fs::remove_dir_all(resolved).unwrap();
        }
        run.unwrap();
    }
}
