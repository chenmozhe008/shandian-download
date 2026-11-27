//! One bounded runtime log. UI callers enqueue; only this writer rotates files.
use std::{fs::{File, OpenOptions}, io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write}, path::{Path, PathBuf}, sync::{mpsc, OnceLock}, time::{Duration, SystemTime, UNIX_EPOCH}};

const LIMIT: u64 = 2 * 1024 * 1024;
struct Entry { category: String, text: String, done: Option<mpsc::Sender<()>> }
enum LogRequest {
    Append(Entry),
    Export(mpsc::Sender<io::Result<String>>),
}

const EXPORT_WINDOW: u64 = 5 * 60;

fn log_time() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

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

fn export_entry(line: &str, now: u64) -> Option<String> {
    use regex::Regex;
    static SAFE_DETAIL: OnceLock<Regex> = OnceLock::new();
    static CODES: OnceLock<Regex> = OnceLock::new();
    let (time, rest) = line.strip_prefix('[')?.split_once("] [")?;
    let time = time.parse::<u64>().ok()?;
    if time < now.saturating_sub(EXPORT_WINDOW) || time > now { return None; }
    let (category, detail) = rest.split_once("] ")?;
    let category = match category {
        "app" | "startup" | "ui" | "cache" | "tools" | "wechat" | "gateway" | "mirror"
            | "engine" | "engine-error" | "video" | "panic" => category,
        // Imported historical lines are stamped when migrated, so they can't be dated reliably.
        _ => return None,
    };
    let detail = detail.trim();
    // Only copy known diagnostic formats with numeric fields or fixed vocabulary.
    // A generic string from a download engine can include an arbitrary filename or credential.
    let safe = SAFE_DETAIL.get_or_init(|| Regex::new(concat!(
        r"^(?:",
        r"\[\+\s*\d{1,9} ms\] (?:main 开始|setup 进入|数据目录就绪|设置读完|窗口建好（WebView2 环境就绪）|页面开始加载|页面加载完成|setup 结束（窗口交给 WebView2）|builder\.build 完成（窗口与 WebView2 已就绪）|浮窗：窗口已创建，等页面加载|浮窗：定位到 \(-?\d{1,6}, -?\d{1,6}\) 尺寸 \d{1,6}x\d{1,6} scale=\d{1,2}(?:\.\d{1,4})? 结果=None)",
        r"|启动：进程→首帧 \d{1,9} ms（前端自报导航起 \d{1,9} ms），窗口 \d{1,9} ms 显示",
        r"|accent=(?:amber|blue|teal|green|purple|rose|indigo|graphite|custom) elapsed_ms=\d{1,9}(?:\.\d{1,3})?",
        r"|script error; see reproduction steps|\[含凭据的日志行已省略\]",
        r"|(?:status|discovery) elapsed_ms=\d{1,9}(?: candidates=\d{1,6})?",
        r"|handoff browser=(?:chrome|edge) cached=(?:true|false) elapsed_ms=\d{1,9} ok=(?:true|false)",
        r"|freed_bytes=\d{1,16} files=\d{1,9} skipped=\d{1,9} pending_browser=(?:true|false)",
        r"|startup_cleanup freed_bytes=\d{1,16} skipped=\d{1,9}",
        r")$"
    )).expect("constant log detail pattern"));
    if safe.is_match(detail) {
        return Some(format!("[{time}] [{category}] {detail}"));
    }
    let lower = detail.to_ascii_lowercase();
    let event = [
        ("建窗口失败", "创建窗口失败"), ("超时", "超时"), ("timeout", "超时"), ("timed out", "超时"),
        ("拒绝", "请求被拒绝"), ("denied", "请求被拒绝"), ("拒绝连接", "连接失败"),
        ("connection reset", "连接中断"), ("retry", "重试"), ("重试", "重试"),
        ("失败", "失败"), ("error", "错误"),
    ].iter().find_map(|(pattern, label)| lower.contains(pattern).then_some(*label))
        .unwrap_or(if category == "panic" { "程序异常" } else { "已省略原始内容" });
    let codes = CODES.get_or_init(|| Regex::new(
        r"(?i)\b(?:http\s+[1-5][0-9]{2}|os error\s+[0-9]{1,5}|exit=-?[0-9]{1,10})\b"
    ).expect("constant log code pattern"));
    let info = codes.find_iter(detail).take(3).map(|m| m.as_str()).collect::<Vec<_>>().join(" · ");
    let extra = if info.is_empty() { String::new() } else { format!(" · {info}") };
    Some(format!("[{time}] [{category}] {event}{extra}"))
}

fn recent_export(dir: &Path, now: u64) -> io::Result<String> {
    let mut contents = String::new();
    for name in ["app.2.log", "app.1.log", "app.log"] {
        let file = match File::open(dir.join(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for line in BufReader::new(file.take(LIMIT)).split(b'\n') {
            let line = line?;
            if let Some(entry) = export_entry(String::from_utf8_lossy(&line).as_ref(), now) {
                contents.push_str(&entry);
                contents.push('\n');
            }
        }
    }
    Ok(contents)
}

struct Writer { dir: PathBuf, file: Option<File>, size: u64 }
impl Writer {
    fn new(dir: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(dir.join("app.log"))?;
        let size = file.metadata()?.len();
        Ok(Self { dir, file: Some(file), size })
    }
    fn append(&mut self, category: &str, text: &str) -> io::Result<()> {
        let secs = log_time();
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

fn sender() -> Option<&'static mpsc::SyncSender<LogRequest>> {
    static SENDER: OnceLock<Option<mpsc::SyncSender<LogRequest>>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let dir = directory().ok()?;
        let (send, receive) = mpsc::sync_channel::<LogRequest>(256);
        std::thread::Builder::new().name("app-log".into()).spawn(move || {
            let Ok(mut writer) = Writer::new(dir) else { return };
            writer.import_legacy();
            for request in receive {
                match request {
                    LogRequest::Append(entry) => {
                        let _ = writer.append(&entry.category, &entry.text);
                        if let Some(done) = entry.done { let _ = done.send(()); }
                    }
                    LogRequest::Export(reply) => {
                        let _ = reply.send(recent_export(&writer.dir, log_time()));
                    }
                }
            }
        }).ok()?;
        Some(send)
    }).as_ref()
}

pub fn record(category: &str, text: &str) {
    if let Some(send) = sender() { let _ = send.try_send(LogRequest::Append(Entry { category: category.into(), text: text.chars().take(8192).collect(), done: None })); }
}
pub fn record_panic(text: &str) {
    let (done, receive) = mpsc::channel();
    if let Some(send) = sender() {
        if send.try_send(LogRequest::Append(Entry { category: "panic".into(), text: text.into(), done: Some(done) })).is_ok() { let _ = receive.recv_timeout(Duration::from_millis(500)); }
    }
}

pub fn save_recent_export(dir: &Path) -> Result<PathBuf, String> {
    let (reply, receive) = mpsc::channel();
    sender().ok_or("日志不可用")?.try_send(LogRequest::Export(reply)).map_err(|_| "日志正在写入，请稍后重试")?;
    let contents = receive.recv_timeout(Duration::from_secs(5)).map_err(|_| "读取日志超时，请稍后重试")?
        .map_err(|e| format!("读取日志失败：{e}"))?;
    if contents.is_empty() { return Err("最近 5 分钟没有可导出的日志".into()); }

    let name = format!("闪电脱敏日志-{}-{:06x}.txt", log_time(), rand::random::<u32>() & 0xff_ffff);
    let path = dir.join(name);
    let mut file = OpenOptions::new().write(true).create_new(true).open(&path)
        .map_err(|e| format!("保存日志失败：{e}"))?;
    let result = writeln!(file, "闪电全能下载 {} · 最近 5 分钟\n记录时间为 Unix 秒，已省略可能包含个人信息的原始内容。\n\n{contents}", env!("CARGO_PKG_VERSION"));
    if let Err(error) = result {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(format!("写入日志失败：{error}"));
    }
    Ok(path)
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
    fn exports_only_recent_safe_details_across_rotated_logs() {
        let dir = std::env::temp_dir().join(format!("shandian-export-test-{:x}", rand::random::<u64>()));
        std::fs::create_dir(&dir).unwrap();
        let old = dir.join("app.2.log");
        let rotated = dir.join("app.1.log");
        let latest = dir.join("app.log");
        std::fs::write(&old, "[699] [ui] script error; see reproduction steps\n").unwrap();
        std::fs::write(&rotated, "[700] [app] [+ 481 ms] 页面加载完成\n").unwrap();
        std::fs::write(&latest, concat!(
            "[980] [engine] error HTTP 403 https://private.invalid/example\n",
            "[985] [video] title=private-media task=example_id os error 5\n",
            "[989] [gateway] 账号=example@private.invalid 路径=D:\\个人目录\\秘密.txt\n",
            "[992] [ui] accent=blue elapsed_ms=1.5\n",
            "[999] [旧记录:panic.log] imported error\n",
            "[1001] [panic] future panic\n"
        )).unwrap();
        let exported = recent_export(&dir, 1000).unwrap();
        assert_eq!(exported.lines().count(), 5);
        assert!(exported.starts_with("[700] [app] [+ 481 ms] 页面加载完成"));
        assert!(exported.contains("[980] [engine] 错误 · HTTP 403"));
        assert!(exported.contains("os error 5") && exported.contains("accent=blue elapsed_ms=1.5"));
        for raw in ["private.invalid", "private-media", "example_id", "个人目录", "秘密", "imported", "future"] {
            assert!(!exported.contains(raw), "{raw}");
        }
        for file in [old, rotated, latest] { std::fs::remove_file(file).unwrap(); }
        std::fs::remove_dir(dir).unwrap();
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
