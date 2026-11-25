//! One-use acknowledgements shared by browser actions; no polling or persistent credentials.
use std::{collections::HashMap, sync::{LazyLock, Mutex}, time::Instant};
use tokio::sync::oneshot;

struct Intent { key: String, at: Instant, claimed: oneshot::Sender<()> }
static INTENTS: LazyLock<Mutex<HashMap<String, Intent>>> = LazyLock::new(Default::default);

pub fn register(key: &str) -> (String, oneshot::Receiver<()>) {
    let nonce = format!("{:032x}", rand::random::<u128>());
    let (claimed, receiver) = oneshot::channel();
    let mut intents = INTENTS.lock().unwrap();
    intents.retain(|_, intent| intent.at.elapsed().as_secs() < 120);
    intents.insert(nonce.clone(), Intent { key: crate::links::canonical_url(key), at: Instant::now(), claimed });
    (nonce, receiver)
}

pub fn claim(nonce: &str, key: &str) -> bool {
    let mut intents = INTENTS.lock().unwrap();
    if !intents.get(nonce).is_some_and(|intent| intent.at.elapsed().as_secs() < 120 && intent.key == crate::links::canonical_url(key)) { return false; }
    if let Some(intent) = intents.remove(nonce) { let _ = intent.claimed.send(()); }
    true
}

pub fn cancel(nonce: &str) { INTENTS.lock().unwrap().remove(nonce); }
