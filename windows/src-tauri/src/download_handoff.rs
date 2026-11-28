//! Correlates a browser's original download with one newly created App task.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::AppState;

#[derive(Clone)]
enum TransferState {
    Pending,
    Starting,
    Task(String),
    Released,
}

struct Transfer {
    url: String,
    until: Instant,
    state: TransferState,
}

const LIMIT: Duration = Duration::from_secs(120);
fn transfers() -> &'static Mutex<HashMap<String, Transfer>> {
    static TRANSFERS: OnceLock<Mutex<HashMap<String, Transfer>>> = OnceLock::new();
    TRANSFERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn register(id: &str, url: &str) -> bool {
    let now = Instant::now();
    let mut map = transfers().lock().unwrap();
    map.retain(|_, value| value.until + LIMIT > now || matches!(value.state, TransferState::Starting));
    if map.len() >= 120 || map.contains_key(id) { return false; }
    map.insert(id.into(), Transfer { url: url.into(), until: now + LIMIT, state: TransferState::Pending });
    true
}

pub fn begin(id: Option<&str>, url: &str) -> Result<(), String> {
    let Some(id) = id else { return Ok(()); };
    let mut map = transfers().lock().unwrap();
    if let Some(transfer) = map.get_mut(id) {
        if transfer.url == url && transfer.until > Instant::now()
            && matches!(transfer.state, TransferState::Pending) {
            transfer.state = TransferState::Starting;
            return Ok(());
        }
    }
    Err("这次接管已结束，请回原网页使用浏览器下载。".into())
}

pub fn finish(id: Option<&str>, result: &Result<String, String>) {
    let Some(id) = id else { return; };
    if let Some(transfer) = transfers().lock().unwrap().get_mut(id) {
        if !matches!(transfer.state, TransferState::Starting) { return; }
        transfer.state = match result {
            Ok(task) if !task.is_empty() => TransferState::Task(task.clone()),
            _ => TransferState::Released,
        };
    }
}

#[tauri::command]
pub fn cancel_browser_handoff(id: String) {
    if let Some(transfer) = transfers().lock().unwrap().get_mut(&id) {
        if matches!(transfer.state, TransferState::Pending) {
            transfer.state = TransferState::Released;
        }
    }
}

fn client(state: &AppState, id: &str) -> Result<(crate::api::Api, String), String> {
    #[cfg(feature = "nas")]
    if let Some(remote) = id.strip_prefix("nas:") {
        return Ok((crate::nas::task_client(&state.local)?, remote.to_string()));
    }
    let engine = state.engine.lock().unwrap();
    Ok((crate::api::Api::new(engine.port, engine.token.clone()), id.to_string()))
}

pub async fn status(state: &AppState, token: &str, release: bool) -> &'static str {
    let task_id = {
        let mut records = transfers().lock().unwrap();
        let Some(current) = records.get_mut(token) else { return "released"; };
        match &current.state {
            TransferState::Pending if release || current.until <= Instant::now() => {
                current.state = TransferState::Released;
                return "released";
            }
            TransferState::Pending => return "pending",
            TransferState::Starting => return "starting",
            TransferState::Released => return "released",
            TransferState::Task(id) => id.clone(),
        }
    };

    let Ok((api, id)) = client(state, &task_id) else { return "unavailable"; };
    let Ok(tasks) = api.list_tasks().await else { return "unavailable"; };
    if let Some(task) = tasks.iter().find(|task| task.id == id) {
        if task.status == "done" { return "received"; }
        if matches!(task.status.as_str(), "error" | "failed") { return "failed"; }
        if task.downloaded() > 0 { return "received"; }
        if !release { return "waiting"; }
        if task.status != "pause" {
            if api.pause(&id).await.is_err() { return "unavailable"; }
            let Ok(tasks) = api.list_tasks().await else { return "unavailable"; };
            if let Some(current) = tasks.iter().find(|task| task.id == id) {
                if current.status == "done" { return "received"; }
                if !matches!(current.status.as_str(), "pause" | "error" | "failed") {
                    return "unavailable";
                }
            }
        }
    } else if !release {
        return "waiting";
    }
    if let Some(transfer) = transfers().lock().unwrap().get_mut(token) {
        transfer.state = TransferState::Released;
    }
    "released"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_only_the_new_task_to_its_browser_request() {
        let id = "11111111111111111111111111111111";
        let url = "https://example.test/private-download";
        assert!(register(id, url));
        assert!(!register(id, url));
        assert!(begin(Some(id), "https://example.test/wrong").is_err());
        assert!(begin(Some(id), url).is_ok());
        assert!(begin(Some(id), url).is_err());
        finish(Some(id), &Ok("new-task-id".into()));
        assert!(matches!(&transfers().lock().unwrap()[id].state, TransferState::Task(task) if task == "new-task-id"));
    }

    #[test]
    fn returned_or_expired_browser_requests_cannot_create_tasks() {
        let url = "https://example.test/download";
        let returned = "22222222222222222222222222222222";
        let expired = "33333333333333333333333333333333";
        assert!(register(returned, url));
        cancel_browser_handoff(returned.into());
        assert!(begin(Some(returned), url).is_err());
        assert!(register(expired, url));
        transfers().lock().unwrap().get_mut(expired).unwrap().until = Instant::now();
        assert!(begin(Some(expired), url).is_err());
    }
}
