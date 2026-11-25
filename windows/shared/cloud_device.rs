// Guangya device authorization. Secrets stay in the worker and are handed once
// to the owning executor only after a same-client refresh has been verified.
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Notify;

const CLIENT_ID: &str = "aMe_SVSlkrbQXpUT";
const ACCOUNT: &str = "https://account.guangyapan.com";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStart {
    pub qr_image: String,
    pub id: String,
    pub verification_uri: String,
    pub expires_at: u64,
    pub interval: u64,
}
#[derive(Clone, Serialize)]
pub struct DeviceResult {
    pub state: String,
    pub message: String,
}
#[cfg(not(test))]
type Worker = tauri::async_runtime::JoinHandle<()>;
#[cfg(test)]
type Worker = tokio::task::JoinHandle<()>;
#[cfg(not(test))]
fn spawn_worker<F: Future<Output = ()> + Send + 'static>(task: F) -> Worker {
    tauri::async_runtime::spawn(task)
}
#[cfg(test)]
fn spawn_worker<F: Future<Output = ()> + Send + 'static>(task: F) -> Worker {
    tokio::spawn(task)
}
struct Session {
    public: DeviceStart,
    deadline: tokio::time::Instant,
    result: Mutex<DeviceResult>,
    notify: Notify,
    worker: Mutex<Option<Worker>>,
}
struct Service {
    client: reqwest::Client,
    base: String,
    begin: tokio::sync::Mutex<()>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    unit: Duration,
}
static SERVICE: OnceLock<Arc<Service>> = OnceLock::new();
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn service() -> &'static Arc<Service> {
    SERVICE.get_or_init(|| {
        Arc::new(Service {
            client: reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(20))
                .build()
                .expect("device HTTP client"),
            base: ACCOUNT.into(),
            begin: tokio::sync::Mutex::new(()),
            sessions: Mutex::new(HashMap::new()),
            unit: Duration::from_secs(1),
        })
    })
}
fn result_value(state: &str, message: &str) -> DeviceResult {
    DeviceResult {
        state: state.into(),
        message: message.into(),
    }
}
fn finish(session: &Session, state: &str, message: &str) {
    let mut current = session.result.lock().unwrap();
    if terminal(&current.state) {
        return;
    }
    *current = result_value(state, message);
    drop(current);
    session.notify.notify_waiters();
}
fn terminal(state: &str) -> bool {
    matches!(state, "ready" | "error" | "cancelled")
}
fn verification_uri(value: &Value) -> Result<String, String> {
    let raw = value["verification_uri_complete"]
        .as_str()
        .or_else(|| value["short_uri_complete"].as_str())
        .ok_or("光鸭未返回官方授权地址")?;
    let uri = url::Url::parse(raw).map_err(|_| "光鸭授权地址无效")?;
    let host = uri.host_str().unwrap_or("").to_ascii_lowercase();
    if uri.scheme() != "https"
        || !(host == "guangyapan.com" || host.ends_with(".guangyapan.com"))
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.port().is_some_and(|p| p != 443)
    {
        return Err("光鸭返回的授权地址不属于官方站点".into());
    }
    let mut uri = uri;
    uri.query_pairs_mut()
        .append_pair("device", "闪电下载 Windows");
    Ok(uri.to_string())
}
fn error_kind(value: &Value) -> &str {
    value["error"]
        .as_str()
        .or_else(|| value["error_type"].as_str())
        .unwrap_or("")
}
fn oauth_error(value: &Value) -> String {
    match error_kind(value) {
        "invalid_client" | "unauthorized_client" => {
            "光鸭暂未允许此客户端完成授权，请使用服务商支持的登录方式"
        }
        "expired_token" => "光鸭授权已过期，请重新发起",
        "access_denied" => "已拒绝光鸭授权",
        _ => "光鸭授权未完成，原账号已保留",
    }
    .into()
}
fn credentials(value: &Value) -> Option<(&str, &str)> {
    Some((
        value["access_token"].as_str().filter(|s| !s.is_empty())?,
        value["refresh_token"].as_str().filter(|s| !s.is_empty())?,
    ))
}
impl Service {
    async fn post(&self, path: &str, device: &str, body: &Value) -> Result<Value, String> {
        let response = self
            .client
            .post(format!("{}{path}", self.base))
            .header("X-Client-Id", CLIENT_ID)
            .header("X-Sdk-Version", "0.2.1")
            .header("X-Protocol-Version", "301")
            .header("X-Device-Id", device)
            .json(body)
            .send()
            .await
            .map_err(|_| "连接光鸭授权服务失败，原账号已保留")?;
        response
            .json()
            .await
            .map_err(|_| "光鸭授权返回信息不完整，原账号已保留".into())
    }
    async fn begin<F, Fut>(
        self: &Arc<Self>,
        provider: &str,
        commit: F,
    ) -> Result<DeviceStart, String>
    where
        F: FnOnce(Value) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        if provider != "guangya" {
            return Err("此网盘暂不支持客户端设备授权".into());
        }
        let _begin = self.begin.lock().await;
        {
            let sessions = self.sessions.lock().unwrap();
            if let Some(s) = sessions
                .values()
                .find(|s| !terminal(&s.result.lock().unwrap().state))
            {
                return Ok(s.public.clone());
            }
        }
        let device = format!("{:032x}", rand::random::<u128>());
        let challenge = self
            .post(
                "/v1/auth/device/code",
                &device,
                &json!({"client_id":CLIENT_ID,"scope":"user","meta":{"scene":"pc_login"}}),
            )
            .await?;
        let code = challenge["device_code"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| oauth_error(&challenge))?
            .to_string();
        let interval = challenge["interval"].as_u64().unwrap_or(5).max(1);
        let expires = challenge["expires_in"]
            .as_u64()
            .filter(|n| *n > 0)
            .unwrap_or(300)
            .min(3600);
        let public = DeviceStart {
            qr_image: String::new(),
            id: format!("{:032x}", rand::random::<u128>()),
            verification_uri: verification_uri(&challenge)?,
            expires_at: now() + expires,
            interval,
        };
        let session = Arc::new(Session {
            public: public.clone(),
            deadline: tokio::time::Instant::now() + self.unit.mul_f64(expires as f64),
            result: Mutex::new(result_value(
                "pending",
                "请在光鸭官方页面完成授权，原账号暂不变更",
            )),
            notify: Notify::new(),
            worker: Mutex::new(None),
        });
        {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.retain(|_, s| {
                !terminal(&s.result.lock().unwrap().state) || s.public.expires_at + 300 > now()
            });
            sessions.insert(public.id.clone(), session.clone());
        }
        let service = self.clone();
        let running = session.clone();
        let handle = spawn_worker(async move {
            let result = service.authorize(&running, &device, &code, commit).await;
            if let Err(message) = result {
                finish(&running, "error", &message)
            }
        });
        {
            let mut slot = session.worker.lock().unwrap();
            *slot = Some(handle);
            if session.result.lock().unwrap().state == "cancelled" {
                if let Some(handle) = slot.as_ref() {
                    handle.abort()
                }
            }
        }
        Ok(public)
    }
    async fn authorize<F, Fut>(
        &self,
        session: &Session,
        device: &str,
        code: &str,
        commit: F,
    ) -> Result<(), String>
    where
        F: FnOnce(Value) -> Fut,
        Fut: Future<Output = Result<(), String>>,
    {
        let mut interval = session.public.interval;
        let mut failures = 0;
        loop {
            let remaining = session
                .deadline
                .saturating_duration_since(tokio::time::Instant::now());
            if interval as f64 >= remaining.as_secs_f64() / self.unit.as_secs_f64() {
                return Err("光鸭授权已过期，请重新发起".into());
            }
            tokio::time::sleep(self.unit.mul_f64(interval as f64)).await;
            let token=match self.post("/v1/auth/token",device,&json!({"client_id":CLIENT_ID,"grant_type":"urn:ietf:params:oauth:grant-type:device_code","device_code":code})).await {Ok(v)=>{failures=0;v},Err(e)=>{failures+=1;if failures<3 {continue}return Err(e)}};
            if let Some((_, refresh)) = credentials(&token) {
                // Refresh before changing the existing SDK account. A web token
                // must never be presented as a verified PC session.
                let refreshed=self.post("/v1/auth/token",device,&json!({"client_id":CLIENT_ID,"grant_type":"refresh_token","refresh_token":refresh})).await?;
                let Some((access, refresh)) = credentials(&refreshed) else {
                    return Err("光鸭客户端授权的独立续期未通过，原账号已保留".into());
                };
                {
                    let mut state = session.result.lock().unwrap();
                    if state.state == "cancelled" {
                        return Ok(());
                    }
                    *state = result_value("committing", "授权及续期已验证，正在保存到当前设备…");
                }
                let fields = json!({"client_id":CLIENT_ID,"device_id":device,"access_token":access,"refresh_token":refresh});
                commit(fields)
                    .await
                    .map_err(|_| "保存客户端授权失败，请重试；未确认替换原账号".to_string())?;
                finish(
                    session,
                    "ready",
                    "光鸭客户端授权已关联，可由当前设备独立续期",
                );
                return Ok(());
            }
            match error_kind(&token) {
                "authorization_pending" => continue,
                "slow_down" => {
                    interval = interval.saturating_mul(2);
                    continue;
                }
                "" if token["error_code"].as_i64() == Some(4050) => continue,
                _ => return Err(oauth_error(&token)),
            }
        }
    }
    async fn result(&self, id: &str) -> Result<DeviceResult, String> {
        let session = self
            .sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("授权会话已结束，请重新发起")?;
        let notified = session.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let current = session.result.lock().unwrap().clone();
        if terminal(&current.state) {
            return Ok(current);
        }
        let _ = tokio::time::timeout(Duration::from_secs(55), notified).await;
        let mut result = session.result.lock().unwrap().clone();
        if result.state == "committing" {
            result.state = "pending".into()
        }
        Ok(result)
    }
    fn cancel(&self, id: &str) -> Result<(), String> {
        let session = self
            .sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("授权会话已结束")?;
        {
            let mut state = session.result.lock().unwrap();
            if terminal(&state.state) {
                return Ok(());
            }
            if state.state == "committing" {
                return Err("授权正在保存，请等待结果".into());
            }
            *state = result_value("cancelled", "已取消授权，原账号已保留");
        }
        if let Some(worker) = session.worker.lock().unwrap().as_ref() {
            worker.abort()
        }
        session.notify.notify_waiters();
        Ok(())
    }
}
pub async fn start<F, Fut>(provider: &str, commit: F) -> Result<DeviceStart, String>
where
    F: FnOnce(Value) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    service().begin(provider, commit).await
}
pub async fn result(id: &str) -> Result<DeviceResult, String> {
    service().result(id).await
}
pub fn cancel(id: &str) -> Result<(), String> {
    service().cancel(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        thread,
    };
    struct Mock {
        service: Arc<Service>,
        codes: Arc<AtomicUsize>,
        polls: Arc<AtomicUsize>,
        times: Arc<Mutex<Vec<std::time::Instant>>>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }
    impl Drop for Mock {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }
    fn mock(steps: Vec<&'static str>, refresh: bool, expires: u64) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let (codes, polls, times, stop) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(AtomicBool::new(false)),
        );
        let (c, p, tm, st) = (codes.clone(), polls.clone(), times.clone(), stop.clone());
        let worker = thread::spawn(move || {
            while !st.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut data = Vec::new();
                let mut part = [0; 2048];
                let header_end = loop {
                    let n = stream.read(&mut part).unwrap();
                    assert!(n > 0);
                    data.extend_from_slice(&part[..n]);
                    if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                };
                let header = String::from_utf8_lossy(&data[..header_end]).to_ascii_lowercase();
                let length: usize = header
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("content-length:")
                            .and_then(|n| n.trim().parse().ok())
                    })
                    .unwrap();
                while data.len() < header_end + length {
                    let n = stream.read(&mut part).unwrap();
                    assert!(n > 0);
                    data.extend_from_slice(&part[..n]);
                }
                let body: Value =
                    serde_json::from_slice(&data[header_end..header_end + length]).unwrap();
                assert_eq!(body["client_id"], CLIENT_ID);
                assert!(body.get("client_secret").is_none());
                assert!(header.contains("x-device-id:"));
                let reply = if header.starts_with("post /v1/auth/device/code ") {
                    c.fetch_add(1, Ordering::SeqCst);
                    json!({"device_code":"private-device-code","verification_uri_complete":"https://account.guangyapan.com/consent?user_code=public","interval":1,"expires_in":expires})
                } else if body["grant_type"] == "refresh_token" {
                    assert_eq!(body["refresh_token"], "fixture_refresh_token_04");
                    if refresh {
                        json!({"access_token":"fixture_access_token_01","refresh_token":"fixture_refresh_token_02"})
                    } else {
                        json!({"error":"invalid_client","error_description":"private-never-relay"})
                    }
                } else {
                    let n = p.fetch_add(1, Ordering::SeqCst);
                    tm.lock().unwrap().push(std::time::Instant::now());
                    match steps.get(n).copied().unwrap_or("authorization_pending") {
                        "success" => {
                            json!({"access_token":"fixture_access_token_03","refresh_token":"fixture_refresh_token_04"})
                        }
                        error => json!({"error":error}),
                    }
                };
                let text = reply.to_string();
                let response=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",text.len());
                let _ = stream.write_all(response.as_bytes());
            }
        });
        let service = Arc::new(Service {
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap(),
            base: format!("http://{address}"),
            begin: tokio::sync::Mutex::new(()),
            sessions: Mutex::new(HashMap::new()),
            unit: Duration::from_millis(10),
        });
        Mock {
            service,
            codes,
            polls,
            times,
            stop,
            worker: Some(worker),
        }
    }
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }
    #[test]
    fn pending_slow_down_and_refresh_commit_once() {
        runtime().block_on(async {
            let mock = mock(
                vec!["authorization_pending", "slow_down", "success"],
                true,
                40,
            );
            let count = Arc::new(AtomicUsize::new(0));
            let commit_count = count.clone();
            let public = mock
                .service
                .begin("guangya", move |fields| async move {
                    assert_eq!(fields["access_token"], "fixture_access_token_01");
                    assert_eq!(fields["refresh_token"], "fixture_refresh_token_02");
                    assert_eq!(fields["client_id"], CLIENT_ID);
                    commit_count.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
                .unwrap();
            let serialized = serde_json::to_string(&public).unwrap();
            assert!(!serialized.contains("private"));
            assert!(!serialized.contains("refresh_token"));
            let duplicate = mock
                .service
                .begin("guangya", |_| async { panic!("duplicate commit") })
                .await
                .unwrap();
            assert_eq!(public.id, duplicate.id);
            let result = mock.service.result(&public.id).await.unwrap();
            assert_eq!(result.state, "ready");
            assert_eq!(count.load(Ordering::SeqCst), 1);
            assert_eq!(mock.codes.load(Ordering::SeqCst), 1);
            assert_eq!(mock.polls.load(Ordering::SeqCst), 3);
            let times = mock.times.lock().unwrap();
            assert!(times[2].duration_since(times[1]) >= Duration::from_millis(18));
        });
    }
    #[test]
    fn failed_refresh_preserves_previous_account() {
        runtime().block_on(async {
            let mock = mock(vec!["success"], false, 40);
            let public = mock
                .service
                .begin("guangya", |_| async {
                    panic!("invalid refresh must never commit")
                })
                .await
                .unwrap();
            let result = mock.service.result(&public.id).await.unwrap();
            assert_eq!(result.state, "error");
            assert!(!result.message.contains("private"));
        });
    }
    #[test]
    fn cancel_aborts_worker_without_commit() {
        runtime().block_on(async {
            let mock = mock(vec![], true, 40);
            let public = mock
                .service
                .begin("guangya", |_| async { panic!("cancelled grant committed") })
                .await
                .unwrap();
            mock.service.cancel(&public.id).unwrap();
            assert_eq!(
                mock.service.result(&public.id).await.unwrap().state,
                "cancelled"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
            assert_eq!(mock.polls.load(Ordering::SeqCst), 0);
        });
    }
    #[test]
    fn pending_grant_expires() {
        runtime().block_on(async {
            let mock = mock(vec![], true, 4);
            let public = mock
                .service
                .begin("guangya", |_| async { panic!("expired grant committed") })
                .await
                .unwrap();
            let result = mock.service.result(&public.id).await.unwrap();
            assert_eq!(result.state, "error");
            assert!(result.message.contains("过期"));
        });
    }
    #[test]
    fn authorization_uri_must_be_official_https() {
        assert!(verification_uri(&json!({"verification_uri_complete":"https://account.guangyapan.com/authorize?user_code=public"})).is_ok());
        for raw in [
            "http://account.guangyapan.com/a",
            "https://guangyapan.com.evil.test/a",
            "https://user@guangyapan.com/a",
            "https://guangyapan.com:444/a",
        ] {
            assert!(verification_uri(&json!({"verification_uri_complete":raw})).is_err());
        }
    }
    #[test]
    fn credentials_require_both_tokens() {
        assert!(credentials(&json!({"access_token":"a"})).is_none());
        assert!(credentials(&json!({"access_token":"a","refresh_token":""})).is_none());
        assert!(credentials(&json!({"access_token":"a","refresh_token":"r"})).is_some());
    }
    #[test]
    fn provider_errors_never_relay_sensitive_response() {
        assert!(!oauth_error(
            &json!({"error":"unknown","error_description":"refresh_token=private"})
        )
        .contains("private"));
    }
}
