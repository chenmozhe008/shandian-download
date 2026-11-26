//! One bounded runtime log. UI callers enqueue; only this writer rotates files.
use std::{fs::{File, OpenOptions}, io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write}, path::{Path, PathBuf}, sync::{mpsc, OnceLock}, time::{Duration, SystemTime, UNIX_EPOCH}};

const LIMIT: u64 = 2 * 1024 * 1024;
struct Entry { category: String, text: String, done: Option<mpsc::Sender<()>> }

pub fn directory() -> io::Result<PathBuf> {
    let root = std::env::var_os("LOCALAPPDATA").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA"))?;
    let dir = PathBuf::from(root).join("com.shandian.windows/logs");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn redact(text: &str) -> String {
    static URL: OnceLock<regex::Regex> = OnceLock::new();
    let low = text.to_ascii_lowercase();
    if ["token", "cookie", "authorization", "password"].iter().any(|key| low.contains(key)) || text.contains("密钥") {
        return "[含凭据的日志行已省略]".into();
    }
    let mut text = URL.get_or_init(|| regex::Regex::new(r#"(?i)(?:https?|ftp)://[^\s<>\"']+"#).expect("constant URL pattern")).replace_all(text, "[链接]").into_owned();
    if let Some(home) = std::env::var_os("USERPROFILE") { text = text.replace(&home.to_string_lossy().to_string(), "[用户目录]"); }
    text.chars().take(4096).collect()
}

struct Writer { dir: PathBuf, file: Option<File>, size: u64 }
impl Writer {
    fn new(dir: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(dir.join("app.log"))?;
        let size = file.metadata()?.len();
        Ok(Self { dir, file: Some(file), size })
    }
    fn append(&mut self, category: &str, text: &str) -> io::Result<()> {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let line = format!("[{secs}] [{category}] {}\n", redact(text).trim_end());
        if self.size + line.len() as u64 > LIMIT {
            self.file.take();
            let _ = std::fs::remove_file(self.dir.join("app.2.log"));
            if self.dir.join("app.1.log").exists() { std::fs::rename(self.dir.join("app.1.log"), self.dir.join("app.2.log"))?; }
            std::fs::rename(self.dir.join("app.log"), self.dir.join("app.1.log"))?;
            self.file = Some(OpenOptions::new().create(true).append(true).open(self.dir.join("app.log"))?);
            self.size = 0;
        }
        self.file.as_mut().ok_or_else(|| io::Error::other("log unavailable"))?.write_all(line.as_bytes())?;
        self.size += line.len() as u64;
        Ok(())
    }
    fn import_legacy(&mut self) {
        let Some(root) = self.dir.parent().map(Path::to_path_buf) else { return };
        for name in ["startup.log", "gateway.log", "github-mirror.log", "panic.log"] {
            let path = root.join(name);
            let Ok(mut file) = File::open(&path) else { continue };
            let offset = file.metadata().map(|m| m.len().saturating_sub(256 * 1024)).unwrap_or(0);
            if file.seek(SeekFrom::Start(offset)).is_err() { continue; }
            let mut lines = BufReader::new(file).lines();
            if offset > 0 { let _ = lines.next(); }
            let mut complete = true;
            for line in lines {
                match line { Ok(line) if self.append(&format!("旧记录:{name}"), &line).is_ok() => {}, _ => { complete = false; break; } }
            }
            if complete { let _ = std::fs::remove_file(path); }
        }
    }
}

fn sender() -> Option<&'static mpsc::SyncSender<Entry>> {
    static SENDER: OnceLock<Option<mpsc::SyncSender<Entry>>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let dir = directory().ok()?;
        let (send, receive) = mpsc::sync_channel::<Entry>(256);
        std::thread::Builder::new().name("app-log".into()).spawn(move || {
            let Ok(mut writer) = Writer::new(dir) else { return };
            writer.import_legacy();
            for entry in receive {
                let _ = writer.append(&entry.category, &entry.text);
                if let Some(done) = entry.done { let _ = done.send(()); }
            }
        }).ok()?;
        Some(send)
    }).as_ref()
}

pub fn record(category: &str, text: &str) {
    if let Some(send) = sender() { let _ = send.try_send(Entry { category: category.into(), text: text.chars().take(8192).collect(), done: None }); }
}
pub fn record_panic(text: &str) {
    let (done, receive) = mpsc::channel();
    if let Some(send) = sender() {
        if send.try_send(Entry { category: "panic".into(), text: text.into(), done: Some(done) }).is_ok() { let _ = receive.recv_timeout(Duration::from_millis(500)); }
    }
}
pub fn drain_output(reader: impl Read + Send + 'static, category: &'static str) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) { record(category, &line); }
    });
}

#[tauri::command]
pub fn log_ui_event(kind: String, detail: String, elapsed_ms: f64) {
    if kind == "accent" && ["amber", "blue", "teal", "green", "purple", "rose", "indigo", "graphite", "custom"].contains(&detail.as_str()) {
        record("ui", &format!("accent={detail} elapsed_ms={elapsed_ms:.2}"));
    } else if kind == "script-error" { record("ui", "script error; see reproduction steps"); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redacts_links_and_credentials() {
        assert!(!redact("failed https://example.org/private?id=123").contains("example.org"));
        assert!(!redact("Authorization: secret").contains("secret"));
        assert_eq!(redact("accent=blue elapsed_ms=0.2"), "accent=blue elapsed_ms=0.2");
    }
    #[test]
    fn rotation_keeps_at_most_three_bounded_files() {
        let dir = std::env::temp_dir().join(format!("shandian-log-test-{:x}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut writer = Writer::new(dir.clone()).unwrap();
        for _ in 0..1800 { writer.append("test", &"a".repeat(4096)).unwrap(); }
        drop(writer);
        let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(entries.len(), 3);
        for entry in entries { assert!(entry.metadata().unwrap().len() <= LIMIT); std::fs::remove_file(entry.path()).unwrap(); }
        std::fs::remove_dir(dir).unwrap();
    }
}
