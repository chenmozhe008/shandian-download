// 管理 gopeed.exe 子进程。
//
// 内核是「打过闪电补丁的 Gopeed v1.9.3」，编成一个独立 exe（见 script/build_engine_windows.sh），
// 启动参数与 Mac 版一致：-A 监听地址 -P 端口 -T token -d 数据目录 -c 配置文件。
//
// 三个要小心的点：
//   1. 子进程必须带 CREATE_NO_WINDOW，否则每次启动闪一个黑框
//   2. 内核起来是异步的，要轮询 /api/v1/info 才算真就绪
//   3. HTTP 请求必须 no_proxy，本机代理会拦 127.0.0.1（api.rs 里已处理）

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::State;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::api::Api;
use crate::AppState;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const DEFAULT_PORT: u16 = 9999;

pub struct Engine {
    pub exe: PathBuf,
    pub data_dir: PathBuf,
    pub port: u16,
    pub token: String,
    child: Option<Child>,
    /// 接管来的内核（不是我们起的），退出时不杀、也没法 kill，只能放弃管理
    adopted: Option<u16>,
}

#[derive(Serialize)]
pub struct Status {
    pub running: bool,
    pub port: u16,
    pub token: String,
    pub exe: String,
    pub exe_exists: bool,
    pub data_dir: String,
    pub pid: Option<u32>,
}

impl Engine {
    pub fn new(engine_dir: PathBuf) -> Self {
        let exe = locate_exe(&engine_dir);
        // token 要持久化：App 重启时要能连上还在跑的内核（Mac 版同理，退出不关内核）
        let token = load_or_create_token(&engine_dir);
        Self {
            exe,
            data_dir: engine_dir,
            port: DEFAULT_PORT,
            token,
            child: None,
            adopted: None,
        }
    }

    pub fn is_running(&mut self) -> bool {
        if let Some(c) = self.child.as_mut() {
            // try_wait 返回 Ok(None) 表示还活着
            return matches!(c.try_wait(), Ok(None));
        }
        // 接管来的内核：靠探活判断，它随时可能被别人关掉
        match self.adopted {
            Some(port) => {
                let alive = info_ok(port, &self.token);
                if !alive {
                    self.adopted = None;
                }
                alive
            }
            None => false,
        }
    }

    pub fn status(&mut self) -> Status {
        let running = self.is_running();
        Status {
            running,
            port: self.port,
            token: self.token.clone(),
            exe: self.exe.display().to_string(),
            exe_exists: self.exe.exists(),
            data_dir: self.data_dir.display().to_string(),
            pid: if running {
                self.child.as_ref().map(|c| c.id())
            } else {
                None
            },
        }
    }

    /// 拉起内核进程。同步返回，不等它就绪——等待放在命令层做，
    /// 因为 MutexGuard 不能跨 await（不是 Send，Tauri 的命令 future 要求 Send）。
    pub fn spawn(&mut self) -> Result<(u16, String), String> {
        if self.is_running() {
            return Ok((self.port, self.token.clone()));
        }
        // 上次被强制结束（任务管理器、崩溃）时内核还活着，直接接管它：
        // 再起一个会跟它抢同一个数据目录（gopeed.db 被锁），新内核起不来，
        // 表现就是「保存设置报连接失败」。
        if let Some(port) = self.adopt_running() {
            self.port = port;
            self.adopted = Some(port);
            return Ok((port, self.token.clone()));
        }
        if !self.exe.exists() {
            return Err(format!(
                "找不到内核：{}\n先跑 script/build_engine_windows.sh 编出来，或者把它放到这个路径。",
                self.exe.display()
            ));
        }
        std::fs::create_dir_all(&self.data_dir).map_err(|e| format!("建目录失败：{e}"))?;

        // 端口被占用就往后找，避免和别的 gopeed 撞
        self.port = pick_port(DEFAULT_PORT);

        let mut cmd = Command::new(&self.exe);
        cmd.arg("-A")
            .arg("127.0.0.1")
            .arg("-P")
            .arg(self.port.to_string())
            .arg("-T")
            .arg(&self.token)
            .arg("-d")
            .arg(&self.data_dir)
            .arg("-c")
            .arg(self.data_dir.join("config.json"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // 数据目录就是工作目录，内核往里写日志和配置
        cmd.current_dir(&self.data_dir);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let mut child = cmd.spawn().map_err(|e| format!("拉起内核失败：{e}"))?;
        // Unread child pipes eventually fill and can stall the kernel itself.
        if let Some(output) = child.stdout.take() { crate::app_log::drain_output(output, "engine"); }
        if let Some(output) = child.stderr.take() { crate::app_log::drain_output(output, "engine-error"); }
        self.child = Some(child);
        Ok((self.port, self.token.clone()))
    }

    pub fn stop(&mut self) -> Result<(), String> {
        // 接管来的内核不是我们的子进程，杀不掉也不该杀（可能还有别的客户端在用），
        // 这里只放弃管理，界面上会显示成「未启动」
        self.adopted = None;
        if let Some(mut c) = self.child.take() {
            c.kill().map_err(|e| format!("结束内核失败：{e}"))?;
            let _ = c.wait();
        }
        Ok(())
    }

    /// 找一个已经在跑、而且认我们 token 的内核。找到就接管（child 留空，退出时不杀它）。
    /// 手写 HTTP 而不是拉 reqwest：spawn 是同步函数，这里不能 await；只访问 127.0.0.1 也无需 TLS。
    fn adopt_running(&self) -> Option<u16> {
        for port in DEFAULT_PORT..DEFAULT_PORT + 10 {
            // 绑得上说明没人在听，不用去连：Windows 连没人听的本机端口要等满超时，
            // 10 个端口就是 3 秒，这期间还占着引擎锁（启动卡 3 秒就是它）
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
                continue;
            }
            if info_ok(port, &self.token) {
                return Some(port);
            }
        }
        None
    }
}

/// 对本机端口发一个最小 GET /api/v1/info，看返回里是不是 code:0
fn info_ok(port: u16, token: &str) -> bool {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let addr = match format!("127.0.0.1:{port}").parse() {
        Ok(a) => a,
        Err(_) => return false,
    };
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(800)));
    let req = format!(
        "GET /api/v1/info HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Api-Token: {token}\r\nConnection: close\r\n\r\n"
    );
    if s.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 512];
    let mut got = Vec::new();
    while got.len() < 512 {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => got.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&got);
    text.contains("\"code\":0")
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn gen_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| {
            let n: u8 = rng.gen_range(0..16);
            char::from_digit(n as u32, 16).unwrap()
        })
        .collect()
}

/// 读 token.txt，没有就生成一份。续用旧 token 是为了让 App 重启后能接上还在跑的内核。
fn load_or_create_token(dir: &Path) -> String {
    let p = dir.join("token.txt");
    if let Ok(s) = std::fs::read_to_string(&p) {
        let s = s.trim().to_string();
        if s.len() >= 16 {
            return s;
        }
    }
    let t = gen_token();
    if std::fs::create_dir_all(dir).is_ok() {
        let _ = std::fs::write(&p, &t);
    }
    t
}

fn pick_port(start: u16) -> u16 {
    use std::net::TcpListener;
    for p in start..start + 100 {
        if TcpListener::bind(("127.0.0.1", p)).is_ok() {
            return p;
        }
    }
    start
}

/// 内核 exe 的查找顺序：环境变量 → 数据目录 → 当前 exe 旁边 → 源码树里的构建产物
fn locate_exe(engine_dir: &Path) -> PathBuf {
    if let Ok(p) = std::env::var("SHANDIAN_ENGINE") {
        let p = PathBuf::from(p);
        if p.exists() {
            return p;
        }
    }
    let in_data = engine_dir.join("gopeed.exe");
    if in_data.exists() {
        return in_data;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside = dir.join("gopeed.exe");
            if beside.exists() {
                return beside;
            }
        }
    }
    // 开发期：直接指到 C:/ShandianBuild 的编译产物
    let dev = PathBuf::from(r"C:\ShandianBuild\gopeed-shandian-amd64.exe");
    if dev.exists() {
        return dev;
    }
    in_data
}

// ---------- Tauri 命令 ----------

/// 必须是 async：同步命令跑在主线程，而拉起内核时要占着这把锁（杀毒扫描 gopeed.exe 能占好几秒），
/// 界面每 2 秒查一次状态，主线程就跟着卡住
#[tauri::command]
pub async fn engine_status(state: State<'_, AppState>) -> Result<Status, String> {
    Ok(state.engine.lock().unwrap().status())
}

#[tauri::command]
pub async fn engine_start(state: State<'_, AppState>) -> Result<String, String> {
    start_kernel(&state).await
}

async fn start_kernel(state: &AppState) -> Result<String, String> {
    // 同步拉起进程后立刻放锁，后面的等待不持锁（MutexGuard 不能跨 await）
    let (port, token) = {
        let mut g = state.engine.lock().unwrap();
        g.spawn()?
    };
    let api = Api::new(port, token);
    // 设置先拿出来，就绪后要写进内核（不持锁跨 await）
    let (cfg, folders) = {
        let s = state.settings.lock().unwrap().clone();
        (s, state.folders.clone())
    };
    // 内核起来是异步的：最多等 20 秒，每 300ms 探一次
    for _ in 0..66 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if let Ok(info) = api.info().await {
            let _ = cfg.apply(&api, &folders).await;
            return Ok(info
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string());
        }
        let alive = { state.engine.lock().unwrap().is_running() };
        if !alive {
            return Err("内核启动后立刻退出了，看数据目录下的日志".into());
        }
    }
    Err("内核 20 秒内没就绪".into())
}

#[tauri::command(async)]
pub fn engine_stop(state: State<'_, AppState>) -> Result<(), String> {
    state.engine.lock().unwrap().stop()
}


/// 名字的「宽松指纹」：只留字母、数字和汉字，别的一律丢掉。
///
/// 用来把「记录下来的路径」和「磁盘上的真名」对上：yt-dlp 会把 Windows 不允许
/// 的字符换成同形的 Unicode（`/` → `⧸`），而我们读它日志时那个字符可能被系统
/// 代码页吃掉，记录下来的路径就少了字，看上去跟「文件被删了」一模一样。
fn loose_key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(c))
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 记录的路径不存在时，去同一个目录里按宽松指纹找真名。扩展名必须一致。
pub(crate) fn resolve_existing(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return Some(path.to_path_buf());
    }
    let dir = path.parent()?;
    let stem = path.file_stem()?.to_str()?;
    let want = loose_key(stem);
    if want.is_empty() {
        return None;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let p = e.path();
        if !p.is_file() {
            return None;
        }
        let got_ext = p
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if got_ext != ext {
            return None;
        }
        let got = loose_key(p.file_stem()?.to_str()?);
        (got == want).then_some(p)
    })
}

/// 文件是不是真的没了。对齐 Mac 的 isGone：只在「盘还在、文件没了」时算失效。
/// 移动硬盘没插上时盘符根目录都不存在，这时不算失效，免得一拔硬盘满屏「已删除」。
pub(crate) fn file_missing(path: &Path) -> bool {
    if path.exists() {
        return false;
    }
    if let Some(std::path::Component::Prefix(p)) = path.components().next() {
        let root = PathBuf::from(format!("{}{}", p.as_os_str().to_string_lossy(), std::path::MAIN_SEPARATOR));
        if !root.exists() {
            return false;
        }
    }
    true
}

/// 分辨率兜底探测的缓存：任务 id → 探到的标签（探不到也记 None）。
/// list_tasks 每秒跑一次，ffprobe 一次几十毫秒，绝不能每秒都探一遍。
fn res_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, Option<String>>> {
    static C: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Option<String>>>> =
        std::sync::OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn probe_res_cached(key: &str, file: &Path) -> Option<String> {
    if let Some(v) = res_cache().lock().unwrap().get(key) {
        // 老格式（"480P"）不再复用：分辨率现在标成「宽 x 高」，缓存里存的旧值要重新探
        if v.as_deref().map_or(true, |s| s.contains(" x ")) {
            return v.clone();
        }
    }
    let got = crate::videos::probe_resolution(file);
    res_cache().lock().unwrap().insert(key.to_string(), got.clone());
    got
}

#[tauri::command]
pub async fn list_tasks(state: State<'_, AppState>) -> Result<Vec<crate::api::Task>, String> {
    collect_tasks(&state).await
}

/// 重启后接着下：每隔几秒记下正在下载和排队的任务（退出、崩溃、关机都来不及专门保存），
/// 下次启动先恢复这份名单，再开始重新记录。手动暂停的不在名单里，不会被恢复
pub fn start_resume_keeper(app: tauri::AppHandle) {
    use tauri::Manager;
    tauri::async_runtime::spawn(async move {
        let st = app.state::<AppState>();
        let state: &AppState = &st;
        let file = crate::migrate::local_path(&state.local, "resume-on-launch.json");
        let mut ready = false;
        for _ in 0..90 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if collect_tasks(state).await.is_ok() {
                ready = true;
                break;
            }
        }
        if ready {
            let ids: Vec<String> = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            let (videos, kernel): (Vec<String>, Vec<String>) = ids.into_iter().filter(|id| !id.starts_with("bd:")).partition(|id| id.starts_with("v:"));
            if !kernel.is_empty() {
                let (port, token) = {
                    let e = state.engine.lock().unwrap();
                    (e.port, e.token.clone())
                };
                // 名单里有已被删掉的任务时内核只处理还在的
                let _ = Api::new(port, token).resume_batch(&kernel).await;
            }
            let bin = state.tools.dir.clone();
            for id in &videos {
                let _ = state.videos.resume(id.trim_start_matches("v:"), &bin);
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        loop {
            // 内核连不上时不覆盖，名单留给下次
            if let Ok(list) = collect_tasks(state).await {
                let ids: Vec<&str> = list
                    .iter()
                    .filter(|t| matches!(t.status.as_str(), "running" | "wait") && !t.id.starts_with("pp:") && !t.id.starts_with("xl:") && !t.id.starts_with("cloud:"))
                    .map(|t| t.id.as_str())
                    .collect();
                if let Ok(text) = serde_json::to_vec(&ids) {
                    let _ = std::fs::write(&file, text);
                }
            }
            let (cloud_port,cloud_token)={let e=state.engine.lock().unwrap();(e.port,e.token.clone())};
            crate::cloud::tick(state,&Api::new(cloud_port,cloud_token)).await;
            // GitHub / Hugging Face 任务出错就换个加速站接着下（列表要内核原样的地址，不能用 collect_tasks 改过的）
            if state.settings.lock().unwrap().github_mirror {
                let (port, token) = {
                    let e = state.engine.lock().unwrap();
                    (e.port, e.token.clone())
                };
                let api = Api::new(port, token);
                if let Ok(raw) = api.list_tasks().await {
                    if raw.iter().any(|t| t.status == "error") {
                        crate::ghmirror::rescue(&api, &raw).await;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
}

/// 真正的列表逻辑：/purge_missing 这类内部调用也要用，所以吃 &AppState 而不是 State
pub async fn collect_tasks(state: &AppState) -> Result<Vec<crate::api::Task>, String> {
    collect_matching_tasks(state, None).await
}

// Duplicate checks hydrate only matching records. Ordinary list refresh still uses the complete path.
async fn collect_matching_tasks(state: &AppState, key: Option<&str>) -> Result<Vec<crate::api::Task>, String> {
    let (port, token) = {
        let e = state.engine.lock().unwrap();
        (e.port, e.token.clone())
    };
    let mut list = Api::new(port, token)
        .list_tasks()
        .await
        .map_err(|e| e.to_string())?;
    if let Some(key) = key {
        list.retain(|t| crate::links::canonical_url(&crate::ghmirror::original(&t.url())) == key);
    }
    // 类型标签（磁力／种子文件／直链）内核不返回，这里按 req.url 补上
    for t in &mut list {
        // GitHub / Hugging Face 任务在内核里是加速站地址，界面上显示、复制、查重都用原链接
        if let Some(r) = t.meta.as_mut().and_then(|m| m.req.as_mut()) {
            r.url = crate::ghmirror::original(&r.url);
        }
        if let Some(n) = display_names().lock().unwrap().get(&t.id) {
            if t.meta.as_ref().and_then(|m| m.res.as_ref()).map_or(true, |r| r.name.is_empty()) {
                t.name = Some(n.clone());
            }
        }
        t.tag_kind();
        // 分辨率标签从资源名/文件名里提取（"Movie.2024.1080p.x264" → "1080P"）；
        // 提不出来就不输出（前端按 null/缺省处理）
        let res_name = t
            .meta
            .as_ref()
            .and_then(|m| m.res.as_ref())
            .map(|r| r.name.clone())
            .unwrap_or_default();
        t.resolution = crate::videos::resolution_from_name(&res_name).or_else(|| {
            t.files()
                .iter()
                .find_map(|f| crate::videos::resolution_from_name(&f.name))
        });
        if t.status == "done" && completed_multi_has_short_file(t, &task_dir(t, state)) {
            t.status = "error".into();
            t.shandian_error = Some("多文件任务中仍有文件未下载完整".into());
        }
        if t.status == "done" {
            // 完成态才需要摸盘：创建时间兜底、分辨率兜底、文件是否还在，共用同一个目录
            let dir = task_dir(t, state);
            if let Some((path, is_video)) = resolve_task_file(t, &dir) {
                // createdAt 内核一般都带着（api.rs 已解析成毫秒）。真没有就拿文件 mtime 当创建时间
                if t.created.is_none() {
                    t.created = mtime_ms(&path);
                }
                // 视频任务的宽高一律以 ffprobe 探测为准：内核只给得了高度，
                // 缓存的旧值也可能是「480P」或宽高反了的「436 x 1024」。
                // 同一次运行里 probe_res_cached 会挡住重复探测，不会每秒开一次 ffprobe。
                if is_video {
                    let key = format!("k2:{}", t.id);
                    if let Some(got) = probe_res_cached(&key, &path) {
                        t.resolution = Some(got);
                    }
                }
                // 文件被手动删了/挪走了 → 标记失效，界面上变灰并写「文件已删除」
                t.missing = file_missing(&path);
                // 内核下的文件没有元数据，下完补一次来源链接（只补一次，见 tag_source）
                if !t.missing {
                    let url = t.url();
                    tag_source(state, &t.id, &path, &url);
                }
            }
        }
    }
    // 视频网站的任务在 yt-dlp 侧，拼进同一个列表，界面上不用区分来源
    let cloud = crate::pikpak::load_jobs(state);
    list.retain(|t| !crate::pikpak::hides(&cloud, &t.url(), &t.status));
    crate::pikpak::annotate_pulling_tasks(&cloud, &mut list);
    list.extend(crate::pikpak::jobs_as_tasks_matching(&cloud, key));
    list.retain(|t|!crate::cloud::hides(state,&t.id));
    list.extend(crate::cloud::views(state,key));
    // 交给迅雷的任务：迅雷那条接替显示，原来的内核任务（已暂停）先藏起来
    let thunder = crate::thunder::load_jobs(state);
    list.retain(|t| !crate::thunder::hides(&thunder, &t.id));
    list.extend(crate::thunder::jobs_as_tasks_matching(state, key));
    list.extend(state.baidu.tasks(key));
    for j in state.videos.list() {
        if key.is_some_and(|key| crate::links::canonical_url(&j.url) != key) { continue; }
        let mut task = video_to_task(&j);
        // 早期的视频任务没记分辨率：用落盘文件探一次（同样只探一次）
        if task.status == "done" && !j.file_path.is_empty() {
            let recorded = PathBuf::from(&j.file_path);
            // 记录的名字可能和磁盘真名差一两个字符（见 loose_key 的说明），以磁盘为准
            let real = resolve_existing(&recorded);
            let path = real.as_deref().unwrap_or(&recorded);
            if task.resolution.is_none() {
                let key = format!("v:{}", j.id);
                task.resolution = probe_res_cached(&key, path);
            }
            task.missing = file_missing(path);
            // 对不上就把真名写回去：下次直接命中，也顺带修好「打开文件夹」
            if real.as_deref().map_or(false, |r| r != recorded) {
                state.videos.adopt_real_path(&j.id, path);
            }
        }
        list.push(task);
    }
    Ok(list)
}

/// 这条内核任务存在哪。投任务时记了目录就用记录的（用户可能在确认框里改过），
/// 老任务才按当前设置推算——不然改过目录的任务打开、删除、判失效全都对不上。
fn task_dir(task: &crate::api::Task, state: &AppState) -> PathBuf {
    task.saved_dir().unwrap_or_else(|| {
        let kind = crate::links::LinkKind::of(&task.url()).unwrap_or(crate::links::LinkKind::Http);
        let s = state.settings.lock().unwrap();
        s.dir_for(kind, &state.folders)
    })
}

/// 暂停 / 继续 / 删除：三个都只是转发一下，锁不要跨 await
fn endpoint(state: &State<'_, AppState>) -> (u16, String) {
    let e = state.engine.lock().unwrap();
    (e.port, e.token.clone())
}

#[tauri::command]
pub async fn pause_task(id: String, state: State<'_, AppState>) -> Result<(), String> {
    if let Some(job) = id.strip_prefix("bd:") { return state.baidu.pause(job); }
    let _cloud_control=crate::cloud::user_task_control(&id).await;
    if id.starts_with("cloud:") {return crate::cloud::control(&state,&id,"pause").await;}
    if id.starts_with("xl:") {
        return crate::thunder::set_paused(&state, &id[3..], true).await;
    }
    if id.starts_with("pp:") {
        return Err("云端任务不能暂停，可以移除".into());
    }
    if let Some(vid) = id.strip_prefix("v:") {
        return state.videos.pause(vid).map_err(|e| e.to_string());
    }
    let (port, token) = endpoint(&state);
    Api::new(port, token).pause(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn resume_task(id: String, state: State<'_, AppState>) -> Result<(), String> {
    if let Some(job) = id.strip_prefix("bd:") { return state.baidu.resume(job); }
    let _cloud_control=crate::cloud::user_task_control(&id).await;
    if id.starts_with("cloud:") {return crate::cloud::control(&state,&id,"resume").await;}
    if id.starts_with("xl:") {
        return crate::thunder::set_paused(&state, &id[3..], false).await;
    }
    if let Some(pp) = id.strip_prefix("pp:") {
        return crate::pikpak::retry_job(&state, pp).await;
    }
    if let Some(vid) = id.strip_prefix("v:") {
        let bin = state.tools.dir.clone();
        return state.videos.resume(vid, &bin).map_err(|e| e.to_string());
    }
    let (port, token) = endpoint(&state);
    Api::new(port, token)
        .resume(&id)
        .await
        .map_err(|e| e.to_string())
}

/// 删除任务。also_files=false 只删任务记录（进下载历史）；
/// true 再把这条任务下到的文件移到系统回收站，绝不直接抹掉。
/// 文件移不走也要把记录删掉，错误报给界面。
#[tauri::command]
pub async fn delete_task(
    id: String,
    also_files: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if let Some(id) = id.strip_prefix("bd:") {
        let Some(job) = state.baidu.remove(id)? else { return Ok(()) };
        job.record(&state, also_files);
        return if also_files { recycle_files(job.files()).await } else { Ok(()) };
    }
    let _cloud_control=crate::cloud::user_task_control(&id).await;
    if id.starts_with("cloud:") {return crate::cloud::control(&state,&id,"delete").await;}
    if let Some(pp) = id.strip_prefix("pp:") {
        return crate::pikpak::cancel_job(&state, pp).await;
    }
    if let Some(xl) = id.strip_prefix("xl:") {
        let Some(job) = crate::thunder::find(&state, xl) else { return Ok(()) };
        let path = (!job.actual.is_empty()).then(|| PathBuf::from(&job.dir).join(&job.actual));
        let synced = crate::thunder::delete_in_thunder(&job).await;
        crate::thunder::remove_job(&state, xl).await;
        let title = if job.actual.is_empty() { job.name.clone() } else { job.actual.clone() };
        crate::trash::record(&state, &title, "迅雷", &job.url, path.as_ref().filter(|_| job.done).map(|p| p.display().to_string()), job.total, also_files, Some(id.clone()), None, None);
        if !also_files {
            return if synced { Ok(()) } else { Err("这里已删除；迅雷里没能同步删除，请到迅雷里删".into()) };
        }
        // 迅雷还在写的文件动不了，也不该动：让用户到迅雷里删
        if !job.done && !synced {
            return Err("迅雷里没能同步删除这条，这里已不再显示；任务和文件请到迅雷里删除".into());
        }
        // 迅雷放手文件要一点时间
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        return recycle_files(crate::thunder::files_of(&job)).await;
    }
    if let Some(vid) = id.strip_prefix("v:") {
        let Some(job) = state.videos.find(vid) else {
            return state.videos.remove(vid);
        };
        let title = if job.title.is_empty() {
            job.url.clone()
        } else {
            job.title.clone()
        };
        let path = (!job.file_path.is_empty()).then(|| job.file_path.clone());
        crate::trash::record(
            &state,
            &title,
            &job.site,
            &job.url,
            path,
            job.total,
            also_files,
            Some(vid.to_string()),
            job.resolution.clone(),
            if job.no_subs { None } else { job.options.subs_label() },
        );
        // 先结束进程再动文件：还在写的文件进不了回收站
        state.videos.remove(vid)?;
        if also_files {
            let files = crate::videos::job_files(&job, &state.videos.list());
            return recycle_files(files).await;
        }
        return Ok(());
    }
    let (port, token) = endpoint(&state);
    let api = Api::new(port, token);
    // 先取任务信息记下载历史（标题/链接）和要删的文件，再删内核里的记录
    let task = api
        .list_tasks()
        .await
        .ok()
        .and_then(|l| l.into_iter().find(|x| x.id == id));
    let mut files = Vec::new();
    if let Some(t) = &task {
        let url = t.url();
        let title = t
            .name
            .clone()
            .or_else(|| {
                t.meta
                    .as_ref()
                    .and_then(|m| m.res.as_ref())
                    .map(|r| r.name.clone())
            })
            .unwrap_or_else(|| url.clone());
        let origin = t.kind.clone().unwrap_or_else(|| {
            crate::links::LinkKind::of(&url)
                .map(|k| k.label().to_string())
                .unwrap_or_default()
        });
        let dir = task_dir(t, &state);
        // 路径：完成态才反推得到；拿不到就记 None（下载历史里显示「文件已不在」）
        let path = if t.status == "done" {
            resolve_task_file(t, &dir).map(|(p, _)| p.display().to_string())
        } else {
            None
        };
        // 分辨率是 collect_tasks 补上的，内核原始列表里没有
        let resolution = collect_tasks(&state)
            .await
            .ok()
            .and_then(|l| l.into_iter().find(|x| x.id == id))
            .and_then(|x| x.resolution);
        crate::trash::record(&state, &title, &origin, &url, path, t.total(), also_files, Some(id.clone()), resolution, None);
        // 多文件任务：记下文件列表，才能从历史重新下载
        if url.starts_with("shandian-multi:") {
            if let Some(extra) = t.meta.as_ref().and_then(|m| m.req.as_ref()).map(|r| r.extra.clone()) {
                crate::trash::attach_multi(&state, &id, serde_json::json!({ "folder": url.trim_start_matches("shandian-multi:"), "dir": dir.to_string_lossy(), "files": extra["files"] }));
            }
        }
        if also_files {
            files = task_files(t, &dir);
        }
    }
    // 永远 force=false：内核的 force 是永久删除（还会 RemoveAll 资源目录）
    // 内核重启（比如刚更新完）时连不上，等一下再试；还不行就说人话，别把内部地址抛给用户
    let mut tries = 0;
    while let Err(e) = api.delete(&id, false).await {
        tries += 1;
        if tries >= 4 {
            eprintln!("删除任务失败: {e}");
            return Err("下载内核暂时没有响应，请稍后再试".into());
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    let result = recycle_kernel_files(&state, files).await;
    if result.is_err() {
        crate::trash::mark_files_kept(&state.base, &id);
    }
    result
}

/// 内核任务的文件送回收站。BT 任务删掉后内核还占着文件（torrent 库不放句柄，
/// 实测等 40 秒也不放），系统会拒绝。这时重启内核：进程一退句柄必然释放，再试一次。
/// 重启前在下的任务重启后接着下；还是不行就是别的程序占着（比如播放器）。
async fn recycle_kernel_files(state: &AppState, files: Vec<PathBuf>) -> Result<(), String> {
    if files.is_empty() || recycle_files(files.clone()).await.is_ok() {
        return Ok(());
    }
    if let Err(e) = restart_kernel(state).await {
        return Err(format!("任务已删除，但文件被占用，重启内核也失败了：{e}"));
    }
    recycle_files(files).await.map_err(|e| format!("{e}。可能被播放器之类的程序占着，关掉后手动删除"))
}

async fn restart_kernel(state: &AppState) -> Result<(), String> {
    let (port, token, dir) = {
        let e = state.engine.lock().unwrap();
        (e.port, e.token.clone(), e.data_dir.clone())
    };
    let running: Vec<String> = Api::new(port, token)
        .list_tasks()
        .await
        .map(|l| l.into_iter().filter(|t| t.status == "running").map(|t| t.id).collect())
        .unwrap_or_default();
    let _ = state.engine.lock().unwrap().stop();
    // 接管来的内核不是子进程，stop 只是放手，还得真结束它
    let d = dir.clone();
    tauri::async_runtime::spawn_blocking(move || crate::migrate::stop_kernel_using(&d))
        .await
        .map_err(|e| e.to_string())?;
    start_kernel(state).await?;
    let (port, token) = {
        let e = state.engine.lock().unwrap();
        (e.port, e.token.clone())
    };
    let api = Api::new(port, token);
    for id in running {
        let _ = api.resume(&id).await;
    }
    Ok(())
}

/// 在后台线程移到回收站：SHFileOperation 是同步的，大文件夹要好几秒
async fn recycle_files(files: Vec<PathBuf>) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }
    tauri::async_runtime::spawn_blocking(move || crate::trash::recycle(&files))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("任务已删除，但{e}"))
}

/// 内核任务下到的文件：有资源名的是 dir/资源名（目录或单文件），否则是列表里的每个文件。
/// 名字必须是 dir 下面的一层普通路径——空名字、「..」、绝对路径都不认，
/// 否则一个坏记录就能把整个下载目录送进回收站。
fn task_files(task: &crate::api::Task, dir: &Path) -> Vec<PathBuf> {
    let safe = |name: &str| -> Option<PathBuf> {
        let rel = Path::new(name);
        let ok = !name.trim().is_empty()
            && rel
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)));
        ok.then(|| dir.join(rel))
    };
    let meta = task.meta.as_ref();
    let res = meta.and_then(|m| m.res.as_ref());
    let opt_name = meta
        .and_then(|m| m.opts.as_ref())
        .map(|o| o.name.clone())
        .unwrap_or_default();
    let mut out = Vec::new();
    // 用户改过名字时落盘的是改后的名字
    if let Some(p) = safe(&opt_name).filter(|p| p.exists()) {
        out.push(p);
    }
    if let Some(r) = res {
        if let Some(p) = safe(&r.name).filter(|p| p.exists()) {
            out.push(p);
        }
        if out.is_empty() {
            for f in &r.files {
                let rel = Path::new(&f.path).join(&f.name);
                if let Some(p) = safe(&rel.to_string_lossy()).filter(|p| p.exists()) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// 中文/英文各只留一个字幕：zh-Hans/zh-CN/zh 优先于繁体和 ai-zh，
/// en 优先于 en-orig。多余的同语系字幕直接删。
/// 双语字幕合并：把视频旁的中文、英文字幕合成一份（英文一行、中文一行）。
///
/// 选「中英」时 yt-dlp 会分别落 zh-Hans / en / en-orig / zh-Hant 好几个文件，
/// 看的时候得开两个字幕轮着对。这里收尾时合成一个 .srt，其余同语言变体删掉。
/// 找不到中文或英文任意一边就什么都不做（保持原样，别把人家的字幕删了）。
pub fn merge_bilingual_subs(
    video: &Path,
    scale: f32,
    colorize: bool,
    color_zh: &str,
    color_en: &str,
) {
    let Some(stem) = video.file_stem().map(|s| s.to_string_lossy().to_string()) else {
        return;
    };
    let Some(parent) = video.parent() else { return };
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    let prefix = format!("{}.", stem.to_lowercase());

    let mut sub_files: Vec<(String, PathBuf)> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let lower = name.to_lowercase();
        let Some(rest) = lower.strip_prefix(&prefix) else { continue };
        if !crate::video_files::is_subtitle_tail(rest) || !["srt", "vtt", "ass", "lrc"].iter().any(|ext| rest == *ext || rest.ends_with(&format!(".{ext}"))) {
            continue;
        }
        sub_files.push((lower.clone(), p));
    }

    // 挑一份中文、一份英文：优先人工字幕（无 ai / orig 后缀），其次按文件名排
    let rank = |n: &str| -> u8 {
        if n.contains("ai-") || n.contains("auto") { 3 }
        else if n.contains("orig") { 2 }
        else if n.contains(".srt") { 1 }
        else { 0 }
    };
    let pick = |want_zh: bool| -> Option<PathBuf> {
        let mut hits: Vec<(u8, String, PathBuf)> = sub_files
            .iter()
            .filter(|(n, _)| {
                let rest = n.strip_prefix(&prefix).unwrap_or(n);
                let lang = rest.split('.').next().unwrap_or("");
                if want_zh { lang.starts_with("zh") } else { lang == "en" || lang.starts_with("en-") }
            })
            .map(|(n, p)| (rank(n), n.clone(), p.clone()))
            .collect();
        hits.sort();
        hits.into_iter().map(|(_, _, p)| p).find(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
                Some("srt") | Some("vtt") | Some("ass") | Some("ssa")
            )
        })
    };

    let (Some(zh_file), Some(en_file)) = (pick(true), pick(false)) else { return };
    // ass 有样式和排版，不能按 srt 重写，走单独一条路：读它的 Dialogue、保留头部
    if zh_file.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("ass")).unwrap_or(false)
        || zh_file.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("ssa")).unwrap_or(false)
    {
        merge_ass(
            &zh_file,
            &en_file,
            parent,
            &stem,
            &sub_files,
            scale,
            colorize,
            color_zh,
            color_en,
        );
        return;
    }
    let zh = parse_subs(&zh_file);
    let en = parse_subs(&en_file);
    if zh.is_empty() || en.is_empty() {
        return;
    }

    // 以中文时间轴为准，把时间上重叠的英文句子接在下面
    let mut out = String::new();
    let mut idx = 1;
    for z in &zh {
        let mut lines: Vec<String> = Vec::new();
        if !z.text.trim().is_empty() {
            lines.push(z.text.trim().to_string());
        }
        let mut ens: Vec<String> = Vec::new();
        for e in &en {
            if e.start < z.end && e.end > z.start && !e.text.trim().is_empty() {
                let t = e.text.trim();
                if !ens.iter().any(|x| x == t) {
                    ens.push(t.to_string());
                }
            }
        }
        if !ens.is_empty() {
            lines.push(ens.join(" "));
        }
        if lines.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            idx,
            srt_time(z.start),
            srt_time(z.end),
            lines.join("\n")
        ));
        idx += 1;
    }
    if idx == 1 {
        return;
    }

    let merged = parent.join(format!("{}.zh-en.srt", stem));
    if std::fs::write(&merged, out.as_bytes()).is_err() {
        return;
    }
    // 合并完把同一视频的字幕全删掉，只留这一份
    for (_, p) in &sub_files {
        if p != &merged {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// ass/ssa 的双语合并：保留中文字幕的头部（[Script Info] / [V4+ Styles]），
/// 只把 Dialogue 换成「中文\\N英文」，样式和字体都沿用原来那份。
fn merge_ass(
    zh_file: &Path,
    en_file: &Path,
    parent: &Path,
    stem: &str,
    sub_files: &[(String, PathBuf)],
    scale: f32,
    colorize: bool,
    color_zh: &str,
    color_en: &str,
) {
    let zh_text = std::fs::read_to_string(zh_file).unwrap_or_default();
    let en_text = std::fs::read_to_string(en_file).unwrap_or_default();
    if zh_text.is_empty() || en_text.is_empty() {
        return;
    }
    let en = parse_ass(&en_text);
    if en.is_empty() {
        return;
    }
    let zh = parse_ass(&zh_text);
    if zh.is_empty() {
        return;
    }

    // 画布尺寸沿用源文件（决定字号的实际视觉大小），其余样式自己写：
    // 源样式的字号、对齐、边距各不相同（YouTube 那套是 PlayResY 288 + 字号 16 + 左上对齐），
    // 沿用下来双语两行会顶满画面、还贴在左下角。
    let mut play_y: f64 = 288.0;
    let mut play_x: f64 = 384.0;
    for line in zh_text.lines().take(20) {
        let low = line.trim().to_ascii_lowercase();
        if let Some(v) = low.strip_prefix("playresy:") {
            if let Ok(n) = v.trim().parse::<f64>() {
                if n > 0.0 {
                    play_y = n;
                }
            }
        } else if let Some(v) = low.strip_prefix("playresx:") {
            if let Ok(n) = v.trim().parse::<f64>() {
                if n > 0.0 {
                    play_x = n;
                }
            }
        }
    }
    let (head, font) = ass_head(play_x, play_y, scale);
    let mut out = head;


    let mut idx = 0;
    for (start, end, zh_text) in &zh {
        // 10ms 这种碎 cue 是源字幕的换行占位，留着只会在画面上闪一下
        if end.saturating_sub(*start) < 40 {
            continue;
        }
        let mut parts: Vec<String> = Vec::new();
        let zh_line = one_line(zh_text);
        if !zh_line.is_empty() {
            parts.push(zh_line.clone());
        }
        let mut ens: Vec<String> = Vec::new();
        for (es, ee, et) in &en {
            if es < end && ee > start {
                let t = one_line(et);
                if !t.is_empty() && !ens.iter().any(|x| x == &t) {
                    ens.push(t);
                }
            }
        }
        if !ens.is_empty() {
            parts.push(ens.join(" "));
        }
        if parts.is_empty() {
            continue;
        }
        // 着色走内联标签：中文加粗、英文常规且小一号。颜色由用户在确认框里选，
        // 这里把 #RRGGBB 转成 ASS 的 &HBBGGRR&。加粗只作用于中文那段，英文开头 \b0 收回
        let body = if colorize {
            let zh = if zh_line.is_empty() {
                String::new()
            } else {
                format!("{{\\c{}&\\b1}}{zh_line}", ass_color(color_zh))
            };
            let en = if ens.is_empty() {
                String::new()
            } else {
                let en_size = ((font as f64) * 0.86).round() as i64;
                format!(
                    "{{\\c{}&\\b0\\fs{en_size}}}{}",
                    ass_color(color_en),
                    ens.join(" ")
                )
            };
            [zh, en]
                .into_iter()
                .filter(|x| !x.is_empty())
                .collect::<Vec<_>>()
                .join("\\N")
        } else {
            parts.join("\\N")
        };
        out.push_str(&format!(
            "Dialogue: 0,{},{},Default,,0,0,0,,{}\n",
            ass_time(*start),
            ass_time(*end),
            body
        ));
        idx += 1;
    }
    if idx == 0 {
        return;
    }

    let merged = parent.join(format!("{}.zh-en.ass", stem));
    if std::fs::write(&merged, out.as_bytes()).is_err() {
        return;
    }
    for (_, p) in sub_files {
        if p != &merged {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// ass 的头部（到 Events 的 Format 行为止）和算出来的字号
fn ass_head(play_x: f64, play_y: f64, scale: f32) -> (String, i64) {
    // 字号 = 画面高度的 4.2%，再乘用户选的倍率：两行加起来约 9%，不挡画面
    let font = (play_y * 0.042 * scale as f64).clamp(10.0, play_y * 0.09).round() as i64;
    let mut out = String::new();
    out.push_str("[Script Info]\n");
    out.push_str("ScriptType: v4.00+\n");
    out.push_str(&format!("PlayResX: {}\n", play_x.round() as i64));
    out.push_str(&format!("PlayResY: {}\n", play_y.round() as i64));
    out.push_str("WrapStyle: 0\n\n");
    out.push_str("[V4+ Styles]\n");
    out.push_str("Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");
    out.push_str(&format!(
        // Alignment 2 = 底部居中；中文优先用雅黑（找不到就回退系统默认）
        "Style: Default,Microsoft YaHei,{},&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,1.5,0.8,2,20,20,{},1\n",
        font,
        (play_y * 0.03).round() as i64
    ));
    out.push_str("\n[Events]\n");
    out.push_str("Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    (out, font)
}

/// 新生成中英字幕用的样式（头部、中文标签、英文标签），和 merge_ass 写出来的一致
pub(crate) fn subtitle_style(scale: f32, colorize: bool, color_zh: &str, color_en: &str) -> (String, String, String) {
    let (head, font) = ass_head(384.0, 288.0, scale);
    if !colorize {
        return (head, String::new(), String::new());
    }
    let en_size = ((font as f64) * 0.86).round() as i64;
    (
        head,
        format!("{{\\c{}&\\b1}}", ass_color(color_zh)),
        format!("{{\\c{}&\\b0\\fs{en_size}}}", ass_color(color_en)),
    )
}

/// 字幕外观：字号倍率、双语着色、阴影、黑色底框。新下载和「字幕样式」窗口共用
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubLook {
    /// 中文（第一行）字号倍率，1 = 画面高度的 4.2%
    pub scale: f32,
    /// 英文（第二行）字号倍率，和中文同一个基准；默认 0.86（比中文小一号）
    #[serde(default = "en_scale")]
    pub scale_en: f32,
    pub colorize: bool,
    pub color_zh: String,
    pub color_en: String,
    pub shadow: bool,
    /// 阴影距离（ASS 的 Shadow，0～4），shadow 开着时用
    #[serde(default = "shadow_size")]
    pub shadow_size: f32,
    pub boxed: bool,
    /// 字体名（系统里装了才有效，没有时播放器回退默认字体）
    #[serde(default = "sub_font")]
    pub font: String,
    /// 阴影不透明度（0～100），默认 50
    #[serde(default = "shadow_alpha")]
    pub shadow_alpha: f32,
    /// 黑底不透明度（0～100），默认 62
    #[serde(default = "box_alpha")]
    pub box_alpha: f32,
    /// 描边粗细（0～4），默认 1.5；黑底时不用
    #[serde(default = "outline_size")]
    pub outline: f32,
    /// 离画面底部的距离（画面高度的百分比），默认 3
    #[serde(default = "margin_pct")]
    pub margin: f32,
}

pub(crate) fn shadow_alpha() -> f32 {
    50.0
}
pub(crate) fn box_alpha() -> f32 {
    62.0
}
pub(crate) fn outline_size() -> f32 {
    1.5
}
pub(crate) fn margin_pct() -> f32 {
    3.0
}

/// 不透明度百分比 → ASS 颜色里的 alpha 字节（00 不透明，FF 全透明），再拼上黑色
fn black_with(opacity: f32) -> String {
    let a = ((100.0 - opacity.clamp(0.0, 100.0)) / 100.0 * 255.0).round() as u8;
    format!("&H{a:02X}000000")
}

pub(crate) fn en_scale() -> f32 {
    0.86
}
pub(crate) fn shadow_size() -> f32 {
    0.8
}
pub(crate) fn sub_font() -> String {
    "Microsoft YaHei".to_string()
}

/// 按 look 重写一份 ass：换掉 Default 样式行，双语两段的行内颜色、字号标签按 look 重新写。
/// 画布（PlayResY）沿用文件自己的，字号和新生成时同一个算法
pub(crate) fn restyle_ass(text: &str, look: &SubLook) -> String {
    let play_y = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("PlayResY:").and_then(|v| v.trim().parse::<f64>().ok()))
        .filter(|v| *v > 0.0)
        .unwrap_or(288.0);
    let base = play_y * 0.042;
    let size = |s: f32| (base * s as f64).clamp(8.0, play_y * 0.09).round() as i64;
    let font = size(look.scale);
    let en_size = size(look.scale_en);
    let margin = (play_y * look.margin.clamp(0.0, 30.0) as f64 / 100.0).round() as i64;
    // 文字一直是描边 + 可调阴影（阴影色是 BackColour）。
    // 黑底不能和描边放在同一个样式里：ASS 的 BorderStyle 3 拿 OutlineColour 画底框，描边就没了。
    // 所以黑底单独一层：同样的文字用「Box」样式（字全透明、BorderStyle 3、底框留白够大，
    // 不然字顶着底框上沿）放在第 0 层，正常带描边的文字在第 1 层盖在上面。所有播放器都认这种写法
    let line_w = look.outline.clamp(0.0, 4.0) as f64;
    let shadow = if look.shadow && !look.boxed { look.shadow_size.clamp(0.0, 4.0) as f64 } else { 0.0 };
    let face = if look.font.trim().is_empty() { sub_font() } else { look.font.replace(',', " ") };
    let style = format!(
        "Style: Default,{face},{font},&H00FFFFFF,&H000000FF,&H00000000,{},0,0,0,0,100,100,0,0,1,{line_w:.1},{shadow:.1},2,20,20,{margin},1",
        black_with(look.shadow_alpha)
    );
    let box_style = look.boxed.then(|| {
        let b = black_with(look.box_alpha);
        let pad = (font as f64 * 0.32).max(2.0);
        format!("Style: Box,{face},{font},&HFF000000,&HFF000000,{b},{b},0,0,0,0,100,100,0,0,3,{pad:.1},0,2,20,20,{margin},1")
    });
    let strip_lead = |s: &str| -> String {
        let s = s.trim_start();
        if s.starts_with('{') {
            s.find('}').map_or(s.to_string(), |i| s[i + 1..].to_string())
        } else {
            s.to_string()
        }
    };
    let mut out = String::new();
    for line in text.lines() {
        let low = line.trim_start().to_ascii_lowercase();
        // 上一次改样式留下的底框层：整层去掉，下面按这次的设置重新生成
        if low.starts_with("style: box,") {
            continue;
        }
        if low.starts_with("style:") && line.contains("Default,") {
            out.push_str(&style);
            if let Some(b) = &box_style {
                out.push('\n');
                out.push_str(b);
            }
        } else if low.starts_with("dialogue:") {
            let mut it = line.splitn(10, ',');
            let mut head: Vec<String> = it.by_ref().take(9).map(|s| s.to_string()).collect();
            let body = it.next().unwrap_or("");
            if head.get(3).is_some_and(|s| s.trim() == "Box") {
                continue;
            }
            let (a, b) = match body.split_once("\\N") {
                Some((a, b)) => (a, Some(b)),
                None => (body, None),
            };
            let (a, b) = (strip_lead(a), b.map(strip_lead));
            let text = if look.colorize {
                let za = format!("{{\\c{}&\\b1}}{a}", ass_color(&look.color_zh));
                match b {
                    Some(b) => format!("{za}\\N{{\\c{}&\\b0\\fs{en_size}}}{b}", ass_color(&look.color_en)),
                    None => za,
                }
            } else {
                match b {
                    Some(b) => format!("{a}\\N{{\\fs{en_size}}}{b}"),
                    None => a,
                }
            };
            if box_style.is_some() && head.len() >= 4 {
                // 第 0 层：底框（文字透明）；第 1 层：带描边的文字
                let mut under = head.clone();
                under[0] = "Dialogue: 0".to_string();
                under[3] = "Box".to_string();
                out.push_str(&under.join(","));
                out.push(',');
                out.push_str(&text);
                out.push('\n');
                head[0] = "Dialogue: 1".to_string();
            }
            out.push_str(&head.join(","));
            out.push(',');
            out.push_str(&text);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// 毫秒 → ass 时间（0:00:04.08，百分秒）
/// #RRGGBB → ASS 的 &HBBGGRR（ASS 的字节序是倒着的）
fn ass_color(hex: &str) -> String {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return "&HFFFFFF".to_string();
    }
    let Ok(r) = u8::from_str_radix(&h[0..2], 16) else {
        return "&HFFFFFF".to_string();
    };
    let Ok(g) = u8::from_str_radix(&h[2..4], 16) else {
        return "&HFFFFFF".to_string();
    };
    let Ok(b) = u8::from_str_radix(&h[4..6], 16) else {
        return "&HFFFFFF".to_string();
    };
    format!("&H{b:02X}{g:02X}{r:02X}", b = b, g = g, r = r)
}

pub(crate) fn ass_time(ms: u64) -> String {
    format!(
        "{}:{:02}:{:02}.{:02}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000 / 10
    )
}

/// 把带 \N 折行的文本压成一行：把换行当空格，多余空白收成一个
fn one_line(s: &str) -> String {
    let mut t = s.replace("\\N", " ").replace("\\n", " ");
    while t.contains("  ") {
        t = t.replace("  ", " ");
    }
    t.trim().to_string()
}

/// 拆一行 Dialogue：返回（到 Text 字段为止的前缀、正文、起止毫秒）
fn split_ass_line(line: &str) -> (String, String, u64, u64) {
    let mut it = line.splitn(10, ',');
    let head: Vec<&str> = it.by_ref().take(9).collect();
    let text = it.next().unwrap_or("").to_string();
    let start = head.get(1).and_then(|t| ts_ms(t)).unwrap_or(0);
    let end = head.get(2).and_then(|t| ts_ms(t)).unwrap_or(0);
    (format!("{},", head.join(",")), strip_tags(&text), start, end)
}

/// 黑底那一层（样式名 Box）：只是同一句台词的底框，读台词时要跳过，否则每句读成两遍
pub(crate) fn is_box_line(line: &str) -> bool {
    line.splitn(5, ',').nth(3).is_some_and(|s| s.trim() == "Box")
}

/// 从 ass 文本里取出每条 Dialogue 的时间与正文
fn parse_ass(text: &str) -> Vec<(u64, u64, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        if !line.trim_start().to_ascii_lowercase().starts_with("dialogue:") || is_box_line(line) {
            continue;
        }
        let (_, body, start, end) = split_ass_line(line);
        let body = body.replace("\\N", " ").replace("\\n", " ");
        if !body.trim().is_empty() && end > start {
            out.push((start, end, body.trim().to_string()));
        }
    }
    out
}

/// 一条字幕（毫秒）
pub(crate) struct Cue {
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) text: String,
}

/// 读 srt / vtt，统一成 cue 列表。带过的标签、位置指令都清掉。
pub(crate) fn parse_subs(path: &Path) -> Vec<Cue> {
    let Ok(raw) = std::fs::read(path) else { return Vec::new() };
    let text = String::from_utf8_lossy(&raw).replace("\r\n", "\n").replace('\r', "\n");
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let lines: Vec<&str> = block.lines().map(|l| l.trim_end()).collect();
        let Some(pos) = lines.iter().position(|l| l.contains("-->")) else { continue };
        let arrow = lines[pos];
        let mut it = arrow.split("-->").map(|x| x.trim().split_whitespace().next().unwrap_or(""));
        let (Some(a), Some(b)) = (it.next(), it.next()) else { continue };
        let (Some(start), Some(end)) = (ts_ms(a), ts_ms(b)) else { continue };
        let body: Vec<String> = lines[pos + 1..]
            .iter()
            .map(|l| strip_tags(l))
            .filter(|l| !l.trim().is_empty())
            .collect();
        if body.is_empty() {
            continue;
        }
        out.push(Cue { start, end, text: body.join("\n") });
    }
    out
}

/// 去掉 <c>/<i>/<font> 这类标签和 {\an8} 这类位置指令
pub(crate) fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut depth_tag = 0;
    let mut depth_brace = 0;
    for ch in s.chars() {
        match ch {
            '<' => depth_tag += 1,
            '>' if depth_tag > 0 => depth_tag -= 1,
            '{' => depth_brace += 1,
            '}' => depth_brace = (depth_brace - 1).max(0),
            c if depth_tag == 0 && depth_brace == 0 => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ").replace("&amp;", "&").replace("&gt;", ">").trim().to_string()
}

/// "00:01:02,500" / "00:01.500" → 毫秒
pub(crate) fn ts_ms(t: &str) -> Option<u64> {
    let t = t.trim();
    let parts: Vec<&str> = t.split(':').collect();
    let (h, m, rest) = match parts.len() {
        3 => (parts[0].parse::<u64>().ok()?, parts[1].parse::<u64>().ok()?, parts[2]),
        2 => (0u64, parts[0].parse::<u64>().ok()?, parts[1]),
        _ => return None,
    };
    let rest = rest.replace(',', ".");
    let (sec, ms) = match rest.split_once('.') {
        Some((a, b)) => {
            let frac = format!("{:0<3}", &b[..b.len().min(3)]);
            (a.parse::<u64>().ok()?, frac.parse::<u64>().ok()?)
        }
        None => (rest.parse::<u64>().ok()?, 0),
    };
    Some(((h * 60 + m) * 60 + sec) * 1000 + ms)
}

fn srt_time(ms: u64) -> String {
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

pub fn dedupe_subs(video: &Path) {
    let Some(stem) = video.file_stem().map(|s| s.to_string_lossy().to_lowercase()) else {
        return;
    };
    let Some(parent) = video.parent() else { return };
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    let mut zh: Vec<(u8, PathBuf)> = Vec::new();
    let mut en: Vec<(u8, PathBuf)> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let lower = name.to_lowercase();
        let Some(rest) = lower.strip_prefix(&format!("{stem}.")) else {
            continue;
        };
        let is_sub = ["srt", "vtt", "ass", "lrc"]
            .iter()
            .any(|ext| rest == *ext || rest.ends_with(&format!(".{ext}")));
        if !is_sub || !crate::video_files::is_subtitle_tail(rest) {
            continue;
        }
        // rest 形如 "zh-hans.vtt" / "en-orig.vtt" / "srt"
        let lang = rest.split('.').next().unwrap_or("").to_string();
        if lang.starts_with("zh") || lang == "ai-zh" {
            let pri = if lang == "zh-hans" || lang == "zh-cn" {
                0
            } else if lang == "zh" {
                1
            } else if lang.starts_with("zh") {
                3
            } else {
                4
            };
            zh.push((pri, p));
        } else if lang.starts_with("en") {
            let pri = if lang == "en" {
                0
            } else if lang.contains("orig") {
                3
            } else {
                1
            };
            en.push((pri, p));
        }
    }
    for mut group in [zh, en] {
        group.sort_by_key(|(pri, _)| *pri);
        for (_, p) in group.into_iter().skip(1) {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// 在资源管理器里打开这条任务的目录。目录按任务类型算，和投任务时同一套规则。
#[tauri::command]
pub async fn open_task_folder(id: String, state: State<'_, AppState>) -> Result<(), String> {
    // 能定位到文件就打开并选中它
    if !id.starts_with("pp:") {
        if let Ok(path) = item_path(id.clone(), state.clone()).await {
            if Path::new(&path).is_file() {
                return crate::commands::reveal_path(path);
            }
        }
    }
    let dir = task_folder(&id, &state).await?;
    // 磁力、种子有自己的文件夹就打开它；还没建出来时打开保存目录
    if !id.starts_with("v:") && !id.starts_with("xl:") {
        let (port, token) = endpoint(&state);
        let list = Api::new(port, token).list_tasks().await.map_err(|e| e.to_string())?;
        if let Some(sub) = list.iter().find(|t| t.id == id).and_then(|t| t.content_folder()) {
            let inner = dir.join(sub);
            if inner.is_dir() {
                // 进资源文件夹并选中正片（最大的视频），没有视频就选第一个
                if let Some(f) = pick_in_folder(&inner) {
                    return crate::commands::reveal_path(f.to_string_lossy().into());
                }
                return crate::commands::open_dir(inner.to_string_lossy().into());
            }
        }
    }
    // 迅雷任务的资源文件夹也一样选中正片
    if let Some(job) = id.strip_prefix("xl:").and_then(|xl| crate::thunder::find(&state, xl)).filter(|j| !j.actual.is_empty() && dir.ends_with(&j.actual)) {
        let _ = job;
        if let Some(f) = pick_in_folder(&dir) {
            return crate::commands::reveal_path(f.to_string_lossy().into());
        }
    }
    crate::commands::open_dir(dir.to_string_lossy().into())
}

/// 文件夹里该选中哪一个：最大的视频（含一层子目录），否则排在第一个的
fn pick_in_folder(dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    entries.sort();
    let mut videos: Vec<PathBuf> = entries.iter().filter(|p| p.is_file()).cloned().collect();
    for d in entries.iter().filter(|p| p.is_dir()) {
        videos.extend(std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_file()));
    }
    videos.retain(|p| crate::thumbs::is_video(&p.to_string_lossy()));
    videos
        .into_iter()
        .max_by_key(|p| p.metadata().map(|m| m.len()).unwrap_or(0))
        .or_else(|| entries.into_iter().next())
}

/// 任务实际存在哪个目录：视频任务认 VideoJob.dir，内核任务认它自己记的目录。
/// 「打开所在位置」和「打开文件所在位置」都必须走这里，否则两边指的不是一处
async fn task_folder(id: &str, state: &State<'_, AppState>) -> Result<std::path::PathBuf, String> {
    if let Some(id) = id.strip_prefix("bd:") {
        let path = state.baidu.find(id).ok_or("找不到这个任务")?.path();
        return Ok(if path.is_dir() { path } else { path.parent().unwrap_or(&path).to_path_buf() });
    }
    if let Some(xl) = id.strip_prefix("xl:") {
        let job = crate::thunder::find(state, xl).ok_or("找不到这个任务")?;
        let inner = PathBuf::from(&job.dir).join(&job.actual);
        return Ok(if !job.actual.is_empty() && inner.is_dir() { inner } else { PathBuf::from(job.dir) });
    }
    if id.starts_with("pp:") {
        return Ok(state.settings.lock().unwrap().dir_for(crate::links::LinkKind::Http, &state.folders));
    }
    if let Some(vid) = id.strip_prefix("v:") {
        return state
            .videos
            .find(vid)
            .map(|j| std::path::PathBuf::from(j.dir))
            .ok_or_else(|| "找不到这个任务".to_string());
    }
    let (port, token) = endpoint(state);
    let list = Api::new(port, token)
        .list_tasks()
        .await
        .map_err(|e| e.to_string())?;
    let task = list.iter().find(|t| t.id == id).ok_or("找不到这个任务")?;
    Ok(task_dir(task, state.inner()).to_path_buf())
}

/// 反推这条内核任务下到哪个文件（thumb_get / item_open / item_reveal 共用）。
/// 单文件任务落在 dir/<文件名>，多文件（种子）落在 dir/<资源名>/<文件名>。
/// 单文件的落盘名有好几个候选：内核文件名、资源名、任务名——扩展投来的名字
/// 和内核自己取的可能不一致（比如带不带 .mp4），谁真实存在就用谁（Mac 版
/// DownloadStore.item(from:) 同款语义）；都不存在（还没下完）就退回第一个候选。
/// 返回 (路径, 是不是视频)。
/// 已经补过元数据的文件（"任务 id|路径"）。记号落盘：只放内存的话，
/// 每次启动都会把所有已完成的视频重写一遍
fn tagged_set(local: &Path) -> &'static Mutex<HashSet<String>> {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SEEN.get_or_init(|| {
        let text = std::fs::read_to_string(crate::migrate::local_path(local, "tagged.txt")).unwrap_or_default();
        Mutex::new(text.lines().map(String::from).collect())
    })
}

/// 同一时刻只跑一个补元数据的 ffmpeg。刚启动时命中一堆已完成任务，
/// 十几个 ffmpeg 一起重封装会把 CPU 和磁盘吃满，界面看着就是「启动后一直卡」
static TAGGING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 线程结束（包括中途 return）时把 TAGGING 放回去
struct TagGuard;
impl Drop for TagGuard {
    fn drop(&mut self) {
        TAGGING.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// 启动后先安静 20 秒再补：这段时间用户在等界面，别抢资源
fn tag_boot() -> std::time::Instant {
    static T0: OnceLock<std::time::Instant> = OnceLock::new();
    *T0.get_or_init(std::time::Instant::now)
}

/// ffmpeg 写得进去的容器。zip / exe / iso 这类不在表里的直接跳过——那些格式没有地方放标签
const TAGGABLE_EXT: &[&str] = &[
    "mp4", "m4v", "mov", "m4a", "mkv", "webm", "avi", "flv", "mp3", "flac", "ogg", "opus", "wav",
];

/// 直链 / BT / ed2k 是内核下的，容器里什么元数据都没有。下完之后补一次：
/// 原始链接写进 comment、文件名写进 title，播放器和 Windows 文件属性里就能看到「哪来的」。
/// 走 -c copy 重封装，不重编码；跑在独立线程里，不挡列表刷新。
fn tag_source(state: &AppState, task_id: &str, path: &Path, url: &str) {
    // BT 的文件绝不能改：内容一变校验就不过，做种出错，重新校验时还会整个重下
    if matches!(
        crate::links::LinkKind::of(url),
        Some(crate::links::LinkKind::Magnet | crate::links::LinkKind::TorrentFile)
    ) {
        return;
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if !TAGGABLE_EXT.contains(&ext.as_str()) || !path.is_file() {
        return;
    }
    let ff = state.tools.dir.join("ffmpeg.exe");
    if !ff.exists() {
        return;
    }
    if tag_boot().elapsed().as_secs() < 20 {
        return;
    }
    if TAGGING.load(std::sync::atomic::Ordering::Relaxed) {
        return; // 有一个在跑，等它完事（下一秒刷新会再来）
    }
    {
        let mut seen = tagged_set(&state.local).lock().unwrap_or_else(|e| e.into_inner());
        let key = format!("{task_id}|{}", path.display());
        if !seen.insert(key.clone()) {
            return;
        }
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(crate::migrate::local_path(&state.local, "tagged.txt"))
        {
            let _ = writeln!(f, "{key}");
        }
    }
    TAGGING.store(true, std::sync::atomic::Ordering::Relaxed);
    let path = path.to_path_buf();
    let url = url.to_string();
    std::thread::spawn(move || {
        let _guard = TagGuard;
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let tmp = path.with_file_name(format!("{stem}.sdtag.{ext}"));
        let _ = std::fs::remove_file(&tmp);
        let mut cmd = Command::new(&ff);
        cmd.args(["-y", "-loglevel", "error", "-i"])
            .arg(&path)
            // 所有轨道原样搬过去，只补两个标签
            .args(["-map", "0", "-c", "copy", "-map_metadata", "0"])
            .arg("-metadata")
            .arg(format!("title={stem}"))
            .arg("-metadata")
            .arg(format!("comment={url}"))
            .arg(&tmp);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let ok = cmd.status().map(|s| s.success()).unwrap_or(false);
        if !ok || !tmp.exists() {
            crate::mark(&format!(
                "补元数据失败 ffmpeg={ok} 产出={} {}",
                tmp.exists(),
                path.display()
            ));
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        // 换文件：先改成 .old 再换，换失败就还原，绝不出现「两个都没了」
        let bak = path.with_file_name(format!("{stem}.sdtag-old.{ext}"));
        let _ = std::fs::remove_file(&bak);
        if std::fs::rename(&path, &bak).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::rename(&bak, &path);
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        let _ = std::fs::remove_file(&bak);
    });
}

fn resolve_task_file(task: &crate::api::Task, dir: &Path) -> Option<(PathBuf, bool)> {
    let res_name = task
        .meta
        .as_ref()
        .and_then(|m| m.res.as_ref())
        .map(|r| r.name.clone())
        .unwrap_or_default();
    let files = task.files();

    // 多文件（种子）：dir/<资源名>/<文件名>。正片按最大的算：第一个常常是 sample 或预告片
    if files.len() > 1 {
        let f = files
            .iter()
            .filter(|f| crate::thumbs::is_video(&f.name))
            .max_by_key(|f| f.size)
            .or_else(|| files.first())?;
        let sub = task
            .meta
            .as_ref()
            .and_then(|m| m.res.as_ref())
            .and_then(|r| r.files.iter().find(|x| x.name == f.name && x.size == f.size))
            .map(|x| x.path.clone())
            .unwrap_or_default();
        return Some((
            dir.join(&res_name).join(sub).join(&f.name),
            crate::thumbs::is_video(&f.name),
        ));
    }

    let mut cands: Vec<(String, bool)> = Vec::new();
    if let Some(f) = files.first() {
        cands.push((f.name.clone(), crate::thumbs::is_video(&f.name)));
    }
    if !res_name.is_empty() {
        let vid = crate::thumbs::is_video(&res_name);
        cands.push((res_name, vid));
    }
    if let Some(n) = task.name.as_deref() {
        if !n.is_empty() && !cands.iter().any(|(c, _)| c == n) {
            cands.push((n.to_string(), crate::thumbs::is_video(n)));
        }
    }
    // 还没解析出名字的任务（刚投的磁力）一个候选都没有。空名字拼出来就是目录本身，也不算
    cands.retain(|(name, _)| !name.trim().is_empty());
    let hit = cands.iter().find(|(name, _)| dir.join(name).exists());
    let (name, vid) = hit.or(cands.first())?.clone();
    Some((dir.join(name), vid))
}

/// 打开这条任务下到的文件（Mac 版 DownloadStore.open）：
/// 用系统默认程序打开；文件不在就退化成 reveal（定位到所在文件夹）。
#[tauri::command]
pub async fn item_open(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let path = item_file(&id, &state).await?;
    // 路径可能是整个文件夹（迅雷下的 BT）：「播放」要打开里面的视频，不是打开文件夹
    if path.is_dir() {
        if let Some(v) = largest_video(&path) {
            return shell_open(&v);
        }
    }
    if path.exists() {
        return shell_open(&path);
    }
    reveal_file(&path)
}

pub(crate) fn largest_video(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if crate::thumbs::is_video(&p.to_string_lossy()) {
                let n = e.metadata().map_or(0, |m| m.len());
                if best.as_ref().map_or(true, |b| n > b.0) {
                    best = Some((n, p));
                }
            }
        }
    }
    best.map(|b| b.1)
}

/// 在资源管理器里定位这条任务的文件（Mac 版 DownloadStore.reveal）：
/// 文件在就选中它，不在就打开它本该在的文件夹。
#[tauri::command]
pub async fn item_reveal(id: String, state: State<'_, AppState>) -> Result<(), String> {
    // 和行内那个「在资源管理器里显示」走**同一个**实现：只打开目录，不选中文件。
    // 以前这里用 explorer /select，视觉上和右边那个按钮不是一回事
    open_task_folder(id, state).await
}

/// 这条任务的保存目录。重新下载默认下回原来的地方
#[tauri::command]
pub async fn task_folder_path(id: String, state: State<'_, AppState>) -> Result<String, String> {
    Ok(task_folder(&id, &state).await?.to_string_lossy().into())
}

/// 用户确认重新下载后，把旧文件送回收站。必须在确认之后才调。
/// 不先清掉的话 yt-dlp 看到同名文件会直接跳过，界面看着像「重新下载没反应」。
///
/// 和删除任务认的是同一批文件：视频连字幕、临时文件（job_files），内核任务是整个资源。
/// 别用 item_file：它在视频任务没记路径时返回保存目录（给「打开位置」用），
/// 拿它去删会把整个下载目录送进回收站。
#[tauri::command]
pub async fn recycle_task_file(id: String, state: State<'_, AppState>) -> Result<(), String> {
    if let Some(id) = id.strip_prefix("bd:") {
        let job = state.baidu.remove(id)?.ok_or("找不到这个任务")?;
        return recycle_files(job.files()).await;
    }
    let files = if let Some(vid) = id.strip_prefix("v:") {
        let job = state.videos.find(vid).ok_or("找不到这个任务")?;
        crate::videos::job_files(&job, &state.videos.list())
    } else {
        // 内核任务先从内核里移除才放得开文件，这是用户确认过的重新下载，旧记录本来就不留
        let (port, token) = endpoint(&state);
        let api = Api::new(port, token);
        let list = api.list_tasks().await.map_err(|e| e.to_string())?;
        let task = list.iter().find(|t| t.id == id).ok_or("找不到这个任务")?;
        let files = task_files(task, &task_dir(task, &state));
        api.delete(&id, false).await.map_err(|e| e.to_string())?;
        crate::trash::forget_link(&state.base, &task.url());
        return recycle_kernel_files(&state, files).await;
    };
    recycle_files(files).await
}

/// 这条任务下到哪个文件的完整路径（前端复制路径用）。文件不存在也返回算出来的
/// 路径；实在反推不出才报错。视频任务只认 VideoJob.file_path——还没定下文件名
/// （开下前）就没有可复制的路径；内核任务复用 item_file 的 resolve_task_file 反推。
#[tauri::command]
pub async fn item_path(id: String, state: State<'_, AppState>) -> Result<String, String> {
    if let Some(vid) = id.strip_prefix("v:") {
        let job = state.videos.find(vid).ok_or("找不到这个任务")?;
        if job.file_path.is_empty() {
            return Err("这个任务还没有落盘文件，等它下出文件再复制路径".into());
        }
        return Ok(job.file_path);
    }
    let p = item_file(&id, &state).await?;
    Ok(p.to_string_lossy().into_owned())
}

/// 这条任务下到哪个文件：视频网站的用 yt-dlp 记下的落盘路径，内核任务按规则反推。
/// 还没下完（或文件被删）时退化成所在目录，打开/定位不至于落空。
async fn item_file(id: &str, state: &State<'_, AppState>) -> Result<PathBuf, String> {
    if let Some(id) = id.strip_prefix("bd:") {
        return state.baidu.find(id).map(|job| job.path()).ok_or_else(|| "找不到这个任务".into());
    }
    if let Some(xl) = id.strip_prefix("xl:") {
        let job = crate::thunder::find(state, xl).ok_or("找不到这个任务")?;
        return Ok(crate::thunder::file_of(&job));
    }
    if let Some(vid) = id.strip_prefix("v:") {
        let job = state.videos.find(vid).ok_or("找不到这个任务")?;
        if !job.file_path.is_empty() {
            return Ok(PathBuf::from(job.file_path));
        }
        return Ok(PathBuf::from(job.dir));
    }
    let (port, token) = endpoint(state);
    let list = Api::new(port, token)
        .list_tasks()
        .await
        .map_err(|e| e.to_string())?;
    let task = list.iter().find(|t| t.id == id).ok_or("找不到这个任务")?;
    let dir = task_dir(task, state);
    resolve_task_file(task, &dir).map(|(p, _)| p).ok_or("这个任务还没有可打开的文件".into())
}

/// explorer /select,<路径>：存在就选中；不存在就退化成打开所在文件夹
fn reveal_file(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let arg = if path.exists() {
        format!("/select,{}", path.display())
    } else {
        path.parent().unwrap_or(path).display().to_string()
    };
    std::process::Command::new("explorer")
        .arg(arg)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("打开资源管理器失败：{e}"))?;
    Ok(())
}

/// 一键清掉所有「文件已失效」的记录（文件早就不在了，只删列表里的任务）。
/// 返回清掉的条数，前端拿来提示。
#[tauri::command]
pub async fn purge_missing(state: State<'_, AppState>) -> Result<usize, String> {
    let list = collect_tasks(&state).await?;
    let mut n = 0usize;
    for t in list.into_iter().filter(|t| t.missing) {
        // 只删记录：文件早就不在了，视频任务顺手清一下同名字幕。不进历史，封面也不留
        crate::thumbs::remove(&state.local, t.id.strip_prefix("v:").unwrap_or(&t.id));
        if let Some(vid) = t.id.strip_prefix("v:") {
            if state.videos.remove(vid).is_ok() {
                n += 1;
            }
            continue;
        }
        let (port, token) = {
            let e = state.engine.lock().unwrap();
            (e.port, e.token.clone())
        };
        if Api::new(port, token).delete(&t.id, false).await.is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

/// 只删列表记录，不记下载历史：失效条目重新下载成功后，旧的那条就没用了。
/// 不进历史就没人再用它的封面，一起清掉
#[tauri::command]
pub async fn forget_task(id: String, state: State<'_, AppState>) -> Result<(), String> {
    if let Some(id) = id.strip_prefix("bd:") { return state.baidu.remove(id).map(|_| ()); }
    if let Some(xl) = id.strip_prefix("xl:") {
        // 列表上立刻去掉；迅雷那边要点界面，要几秒，放后台做，用户不用等
        let job = crate::thunder::find(&state, xl);
        crate::thunder::remove_job(&state, xl).await;
        if let Some(job) = job {
            tauri::async_runtime::spawn(async move {
                let _ = crate::thunder::delete_in_thunder(&job).await;
            });
        }
        return Ok(());
    }
    if let Some(pp) = id.strip_prefix("pp:") {
        return crate::pikpak::cancel_job(&state, pp).await;
    }
    crate::thumbs::remove(&state.local, id.strip_prefix("v:").unwrap_or(&id));
    if let Some(vid) = id.strip_prefix("v:") {
        return state.videos.remove(vid);
    }
    let (port, token) = endpoint(&state);
    // 先记下链接：删完内核会把这条写进 removed 列表（就是我们的下载历史），
    // 不顺手清掉的话「重新下载」之后历史里会多一条一模一样的旧记录
    let link = Api::new(port, token.clone())
        .list_tasks()
        .await
        .ok()
        .and_then(|list| list.iter().find(|t| t.id == id).map(|t| t.url()))
        .unwrap_or_default();
    Api::new(port, token).delete(&id, false).await.map_err(|e| e.to_string())?;
    crate::trash::forget_link(&state.base, &link);
    Ok(())
}

/// 「文件已存在，已跳过」的视频任务，用户选了再下一份
#[tauri::command]
pub fn video_copy_anyway(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let vid = id.strip_prefix("v:").unwrap_or(&id);
    let bin = state.tools.dir.clone();
    state.videos.copy_anyway(vid, &bin)
}

/// 同上，用户选了不用：不再提示
#[tauri::command]
pub fn video_dismiss_collision(id: String, state: State<'_, AppState>) {
    state.videos.dismiss_collision(id.strip_prefix("v:").unwrap_or(&id));
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Duplicate {
    /// 任务 id；只在下载历史里有记录时为 None
    id: Option<String>,
    name: String,
    status: String,
    /// 下过，但文件已经不在了
    missing: bool,
    /// 下载历史里的记录 id，弹窗的「查看」用
    #[serde(rename = "historyId")]
    history_id: Option<String>,
    /// 只在「目录里已有同名文件」时：文件所在的文件夹
    folder: Option<String>,
}

/// 投任务前查一下这个资源是不是下过：列表里有、下载历史里有，或者目录里已有文件。
/// 文件丢了也要告诉用户（再下时旧条目要替换掉）。
/// 认链接的归一化形式，换个播放列表入口也算同一个。
#[tauri::command]
pub async fn find_duplicate(
    url: String,
    dir: Option<String>,
    state: State<'_, AppState>,
) -> Result<Option<Duplicate>, String> {
    let key = crate::links::canonical_url(&url);
    let list = collect_matching_tasks(&state, Some(&key)).await?;
    let hit = list
        .iter()
        .filter(|t| crate::links::canonical_url(&t.url()) == key)
        // 还在下的比下完的更值得告诉用户，文件还在的比丢了的更值得告诉用户
        .max_by_key(|t| (t.status != "done", !t.missing));
    if let Some(t) = hit {
        let name = t
            .name
            .clone()
            .filter(|n| !n.is_empty())
            .or_else(|| t.meta.as_ref().and_then(|m| m.res.as_ref()).map(|r| r.name.clone()))
            .unwrap_or_else(|| url.clone());
        return Ok(Some(Duplicate {
            id: Some(t.id.clone()),
            name,
            folder: None,
            status: t.status.clone(),
            missing: t.missing,
            history_id: None,
        }));
    }
    if let Some(r) = crate::trash::find_existing(&state.base, &key) {
        return Ok(Some(Duplicate {
            id: None,
            history_id: Some(r.id.clone()),
            name: r.title,
            folder: None,
            status: "history".into(),
            missing: false,
        }));
    }
    // 没有任何记录，但保存目录里已经有这个视频的文件
    let dir = dir.filter(|d| !d.trim().is_empty()).map(PathBuf::from).or_else(|| {
        let kind = crate::links::LinkKind::of(&url)?;
        let s = state.settings.lock().unwrap();
        Some(s.default_dir(kind, &state.folders))
    });
    Ok(dir.and_then(|d| crate::videos::existing_file_for(&url, &d)).map(|p| Duplicate {
        id: None,
        name: p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        status: "file".into(),
        folder: p.parent().map(|d| d.display().to_string()),
        missing: false,
        history_id: None,
    }).or_else(|| {
        // 目录里也没有，只剩一条文件已不在的下载历史
        crate::trash::find_any(&state.base, &key).map(|r| Duplicate {
            id: None,
            history_id: Some(r.id.clone()),
            name: r.title,
            folder: None,
            status: "history".into(),
            missing: true,
        })
    }))
}

/// 用系统默认程序打开一个链接（设置页的「GitHub 仓库」这类外链，
/// 以及任务右键的「打开链接」）。白名单放行，别让前端随手把任意字符串丢给 ShellExecute。
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    let url = if url == "http://127.0.0.1:6800/linkswift-settings" {
        let (nonce, _) = crate::browser_handoff::register(&url);
        format!("{url}#nonce={nonce}")
    } else { url };
    let u = url.trim();
    let ok = ["https://", "http://", "magnet:", "ed2k:", "thunder://", "ftp://"]
        .iter()
        .any(|p| u.starts_with(p));
    if !ok {
        return Err("这个链接系统打不开".into());
    }
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let file: Vec<u16> = std::ffi::OsStr::new(u).encode_wide().chain(std::iter::once(0)).collect();
    let r = unsafe {
        ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL)
    };
    if r as isize <= 32 {
        return Err("系统打不开这个链接".into());
    }
    Ok(())
}

/// ShellExecuteW "open"：文件交给默认程序（.mp4 开播放器），文件夹开资源管理器
pub(crate) fn shell_open(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let file: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let r = unsafe {
        ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL)
    };
    if r as isize <= 32 {
        return Err(format!("系统打不开这个文件：{}", path.display()));
    }
    Ok(())
}

/// 取缩略图（data URI）。视频网站的任务优先用站点封面，取不到再截帧；
/// 普通任务（磁力／直链）拿到本地文件后截一帧。界面上每行只问一次，
/// 返回 None 就继续显示标题卡——不做轮询，避免每帧都跑 ffmpeg。
/// 边下边播的地址。内核补丁给每个任务派生一个播放密钥（不带 token，
/// 播放器拿不到请求头），所以这里算 key → 拼 /play/{id}/{key}/{文件名}。
/// 拖动后的下载顺序：内核和视频各有一条等待队列，分别告诉它们
#[tauri::command]
pub async fn set_queue_order(ids: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    state.videos.set_order(ids.iter().filter_map(|i| i.strip_prefix("v:").map(String::from)).collect());
    let kernel: Vec<String> = ids.into_iter().filter(|i| !i.starts_with("v:")).collect();
    if kernel.is_empty() {
        return Ok(());
    }
    let (port, token) = endpoint(&state);
    Api::new(port, token).set_wait_order(&kernel).await.map_err(|e| e.to_string())
}

/// 这些链接里哪些已经在任务列表里
pub async fn known_urls(state: &AppState, urls: Vec<String>) -> Vec<String> {
    let (port, token) = {
        let e = state.engine.lock().unwrap();
        (e.port, e.token.clone())
    };
    let tasks = Api::new(port, token).list_tasks().await.unwrap_or_default();
    let mut have: Vec<String> = tasks.iter().map(|t| crate::links::canonical_url(&t.url())).collect();
    have.extend(state.videos.list().iter().map(|j| crate::links::canonical_url(&j.url)));
    urls.into_iter().filter(|u| have.contains(&crate::links::canonical_url(u))).collect()
}

/// 本机 Ollama 的模型，设置页和右键菜单用
#[tauri::command]
pub async fn ollama_models() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(crate::polish::models).await.map_err(|e| e.to_string())
}

/// 润色这个视频的字幕。Ollama 没开时只修结构（去重复、拼整句、标点归位）
#[tauri::command]
pub async fn polish_task(id: String, state: State<'_, AppState>) -> Result<bool, String> {
    let vid = id.strip_prefix("v:").ok_or("只有视频网站的任务有字幕")?.to_string();
    let job = state.videos.find(&vid).ok_or("任务不存在")?;
    if job.file_path.is_empty() || crate::polish::find_sub(Path::new(&job.file_path)).is_none() {
        return Err("这个视频没有中文字幕文件".into());
    }
    let local = state.local.clone();
    let preferred = state.settings.lock().unwrap().polish_model.clone();
    let model = tauri::async_runtime::spawn_blocking(move || crate::subtitle_ai::model(&local, &preferred))
        .await.map_err(|_| "AI 字幕检测未完成")?;
    let refined = model.is_some();
    if !refined && crate::videos::subtitle_file_label(Path::new(&job.file_path)) == Some("英文字幕") {
        return Err("视频已有英文字幕。翻译成中文需要先在「设置 → 视频网站工具」配置可用的 AI 字幕服务。".into());
    }
    state.videos.polish(&vid, model, &state.tools.dir);
    Ok(refined)
}

/// 电脑上装的字体（字幕样式里选）。用 GDI 枚举字体族，中文系统上中文字体给的是中文名（微软雅黑、黑体…），
/// 字幕渲染器认这个名字。去掉竖排用的「@」开头的和重复的，中文名排前面
#[tauri::command(async)]
pub fn system_fonts() -> Vec<String> {
    use std::collections::BTreeSet;
    use windows_sys::Win32::Graphics::Gdi::{EnumFontFamiliesExW, GetDC, ReleaseDC, DEFAULT_CHARSET, LOGFONTW, TEXTMETRICW};
    unsafe extern "system" fn cb(lf: *const LOGFONTW, _tm: *const TEXTMETRICW, _ty: u32, l: isize) -> i32 {
        let set = &mut *(l as *mut BTreeSet<String>);
        let face = &(*lf).lfFaceName;
        let len = face.iter().position(|c| *c == 0).unwrap_or(face.len());
        let name = String::from_utf16_lossy(&face[..len]);
        if !name.is_empty() && !name.starts_with('@') {
            set.insert(name);
        }
        1
    }
    let mut set: BTreeSet<String> = BTreeSet::new();
    unsafe {
        let dc = GetDC(std::ptr::null_mut());
        let mut lf: LOGFONTW = std::mem::zeroed();
        lf.lfCharSet = DEFAULT_CHARSET as u8;
        EnumFontFamiliesExW(dc, &lf, Some(cb), &mut set as *mut _ as isize, 0);
        ReleaseDC(std::ptr::null_mut(), dc);
    }
    let (mut cjk, mut rest): (Vec<String>, Vec<String>) = set.into_iter().partition(|n| n.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)));
    cjk.append(&mut rest);
    cjk
}

/// 已下载视频现在的字幕样式（「字幕样式」窗口打开时按它显示）
#[tauri::command]
pub fn subtitle_look(id: String, state: State<'_, AppState>) -> Result<SubLook, String> {
    let vid = id.strip_prefix("v:").ok_or("只有视频网站的任务有字幕")?;
    let job = state.videos.find(vid).ok_or("任务不存在")?;
    Ok(job.options.look())
}

/// 给已下载的视频重新获取字幕。返回 false = 站点没有字幕
#[tauri::command]
pub async fn subtitle_fetch(id: String, state: State<'_, AppState>) -> Result<bool, String> {
    let vid = id.strip_prefix("v:").ok_or("只有视频网站的任务有字幕")?.to_string();
    let videos = state.videos.clone();
    let bin = state.tools.dir.clone();
    tauri::async_runtime::spawn_blocking(move || videos.fetch_subs(&vid, &bin)).await.map_err(|e| e.to_string())?
}

/// 改已下载视频的字幕样式，改完重新嵌进视频
#[tauri::command]
pub async fn subtitle_restyle(id: String, look: SubLook, state: State<'_, AppState>) -> Result<(), String> {
    let vid = id.strip_prefix("v:").ok_or("只有视频网站的任务有字幕")?.to_string();
    let videos = state.videos.clone();
    let bin = state.tools.dir.clone();
    tauri::async_runtime::spawn_blocking(move || videos.restyle(&vid, look, &bin)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn polish_restore(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let vid = id.strip_prefix("v:").ok_or("只有视频网站的任务有字幕")?;
    state.videos.polish_restore(vid, &state.tools.dir)
}

/// 这个视频的字幕和清晰度（新建下载框在后台查）
#[tauri::command]
pub async fn probe_video(url: String, video: Option<crate::videos::VideoOptions>, state: State<'_, AppState>) -> Result<Option<crate::videos::VideoProbe>, String> {
    let (local, bin) = (state.local.clone(), state.tools.dir.clone());
    tauri::async_runtime::spawn_blocking(move || crate::videos::probe_video(&local, &bin, &url, video.as_ref()))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn probe_collection(url: String, state: State<'_, AppState>) -> Result<crate::videos::VideoCollection, String> {
    let (local, bin) = (state.local.clone(), state.tools.dir.clone());
    tauri::async_runtime::spawn_blocking(move || crate::videos::probe_collection(&local, &bin, &url))
        .await
        .map_err(|e| e.to_string())?
}

/// 这个包是不是带着没提交的改动打的（build.rs 判断）
#[tauri::command]
pub fn is_dev_build() -> bool {
    env!("SHANDIAN_DEV_BUILD") == "1"
}

/// 重新核对磁盘上已下好的数据，认得的块沿用，只补缺的
#[tauri::command]
pub async fn verify_task(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let (port, token) = endpoint(&state);
    Api::new(port, token).verify(&id).await.map_err(|e| e.to_string())
}

/// 调用前先 PUT tasks/{id}/stream，让内核按顺序往后下，拖进度条也不卡。
#[tauri::command]
pub async fn play_url(id: String, state: State<'_, AppState>) -> Result<String, String> {
    if id.starts_with("v:") {
        return Err("视频网站的任务下完再播，暂不支持边下边播".into());
    }
    let (port, token) = endpoint(&state);
    let api = Api::new(port, token.clone());
    let list = api.list_tasks().await.map_err(|e| e.to_string())?;
    let task = list.iter().find(|t| t.id == id).ok_or("找不到这个任务")?;
    // 内核放的是最大的那个文件，这里的名字只是地址里好看，也取最大的
    let name = task
        .files()
        .iter()
        .filter(|f| crate::thumbs::is_video(&f.name))
        .max_by_key(|f| f.size)
        .map(|f| f.name.clone())
        .or_else(|| {
            let n = task
                .meta
                .as_ref()
                .and_then(|m| m.res.as_ref())
                .map(|r| r.name.clone())
                .unwrap_or_default();
            crate::thumbs::is_video(&n).then_some(n)
        })
        .ok_or("这个任务不是视频，没法边下边播")?;
    // 内核只有运行中的任务才有下载器。暂停的先继续，再等它接上（磁力还要等元数据）
    if task.status != "running" {
        api.resume(&id).await.map_err(|e| format!("没能继续下载：{e}"))?;
    }
    let mut ready = false;
    for _ in 0..50 {
        if api.stream(&id).await.is_ok() {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    if !ready {
        return Err("任务还没准备好（可能还没连上做种的人），等有速度了再试".into());
    }
    let key = play_key(&token, &id);
    Ok(format!(
        "http://127.0.0.1:{port}/play/{id}/{key}/{}",
        encode_segment(&name)
    ))
}

/// 和内核 shandian.PlayKey 同一套算法
fn play_key(token: &str, id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(format!("shandian-play:{token}:{id}").as_bytes());
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    hex[..24].to_string()
}

/// 文件名进 URL 路径段：保留 unreserved，其余按 %XX 转（中文片名必须转）
fn encode_segment(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 缩略图和它的来历。cover=false 是截帧替补，界面之后还会再要原封面
#[derive(serde::Serialize)]
pub struct Thumb {
    src: String,
    cover: bool,
}

#[tauri::command]
pub async fn thumb_get(
    id: String,
    allow_frame: bool,
    state: State<'_, AppState>,
) -> Result<Option<Thumb>, String> {
    let base = state.local.clone();
    let bin = state.tools.dir.clone();

    if let Some(raw) = id.strip_prefix("bd:") {
        let (task, file) = match raw.split_once(':') {
            Some((task, name)) => {
                let Some((decoded, _)) = url::form_urlencoded::parse(name.as_bytes()).next() else {
                    return Ok(None);
                };
                (task, Some(decoded.into_owned()))
            }
            None => (raw, None),
        };
        if task.len() != 16 || !task.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(None);
        }
        let Some(job) = state.baidu.find(task) else { return Ok(None) };
        let task = task.to_owned();
        return tauri::async_runtime::spawn_blocking(move || {
            let path = job.drawer_video_path(file.as_deref())?;
            crate::thumbs::baidu_frame(&base, &bin, &task, &path)
                .map(|src| Thumb { src, cover: true })
        }).await.map_err(|error| error.to_string());
    }

    if let Some(vid) = id.strip_prefix("v:") {
        let job = state.videos.find(vid).ok_or("找不到这个任务")?;
        let vid = vid.to_string();
        // 能从链接推出来的地址最稳，排前面；站点给的（YouTube 的 maxresdefault_en 常 404）作备选。
        // 老任务没记地址时同样有图，暂停着也不用等继续
        let remotes: Vec<String> = crate::links::early_thumb_url(&job.url)
            .into_iter()
            .chain(job.thumbnail_url.clone())
            .collect();
        let file = if job.file_path.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(&job.file_path))
        };
        return tauri::async_runtime::spawn_blocking(move || {
            crate::thumbs::demote_old_frame(&base, &vid);
            let cover = |src| Ok(Some(Thumb { src, cover: true }));
            if let Some(img) = crate::thumbs::cached(&base, &vid) {
                return cover(img);
            }
            if let Some(img) = remotes.iter().find_map(|u| crate::thumbs::remote(&base, &vid, u)) {
                return cover(img);
            }
            // 原封面暂时取不到（代理刚断、站点抽风）：先用截帧顶着，界面过会儿再来要
            let fid = crate::thumbs::frame_id(&vid);
            let mut frame = crate::thumbs::cached(&base, &fid);
            if frame.is_none() && allow_frame {
                if let (Some(v), Some(f)) = (file.as_deref(), crate::thumbs::ffmpeg(&bin)) {
                    frame = crate::thumbs::frame(&base, &fid, v, &f);
                }
            }
            Ok(frame.map(|src| Thumb { src, cover: false }))
        })
        .await
        .map_err(|e| e.to_string())?;
    }

    // 内核任务：名字和目录都按投任务时那套规则反推
    let (port, token) = endpoint(&state);
    let list = Api::new(port, token)
        .list_tasks()
        .await
        .map_err(|e| e.to_string())?;
    let task = list.iter().find(|t| t.id == id).ok_or("找不到这个任务")?;
    let kind = crate::links::LinkKind::of(&task.url()).unwrap_or(crate::links::LinkKind::Http);
    let dir = task_dir(task, &state);
    let res_name = task
        .meta
        .as_ref()
        .and_then(|m| m.res.as_ref())
        .map(|r| r.name.clone())
        .unwrap_or_default();
    let done = |src| Thumb { src, cover: true };
    // 封面的来历记进 startup.log，排查「为什么是这张图」时有处可查
    let log_source = |from: &str| crate::mark(&format!("封面来自{from}：{res_name}"));
    // 种子自带的封面最准，先于一切：之前存的可能是豆瓣图或截帧。
    // 也不管主文件是不是视频：音乐种子常带专辑封面
    if let Some(img) = torrent_cover(task, &dir, &base, &id) {
        log_source("种子自带图片");
        return Ok(Some(done(img)));
    }
    let is_bt = matches!(
        kind,
        crate::links::LinkKind::Magnet | crate::links::LinkKind::TorrentFile
    );
    if is_bt {
        crate::thumbs::drop_unchecked_poster(&base, &id);
    }
    // 有缓存直接用（豆瓣/截帧都可能已经出过图），别每次进来都重查。
    // 要放在找文件之前：没下完的、文件挪走了的任务找不到文件，但存过的图照样能用（2026-11-06 重装后只剩一张图）
    if let Some(img) = crate::thumbs::cached(&base, &id) {
        return Ok(Some(done(img)));
    }
    // 单文件任务落在 dir/<名字>，多文件（种子）落在 dir/<资源名>/<文件名>
    let Some((path, is_vid)) = resolve_task_file(task, &dir) else {
        return Ok(None);
    };
    // 豆瓣和截帧都只对视频有意义
    if !is_vid {
        return Ok(None);
    }
    // allow_frame=false 是下载刚开始、文件还截不了帧的时候：海报只要片名，这时就能查
    // 磁力／种子的「电影」先按片名查豆瓣海报（Mac 版 requestPoster 同款语义），
    // 查不到再退到本地截帧；和视频站封面互不相干
    if is_bt {
        let title = if res_name.is_empty() {
            task.name.clone().unwrap_or_default()
        } else {
            res_name.clone()
        };
        let pretty = crate::links::pretty_title(&title);
        let id2 = id.clone();
        let base2 = base.clone();
        let hit = tauri::async_runtime::spawn_blocking(move || {
            crate::thumbs::douban(&base2, &id2, &pretty)
        })
        .await
        .map_err(|e| e.to_string())?;
        if let Some(img) = hit {
            log_source("豆瓣");
            return Ok(Some(done(img)));
        }
    }
    if !allow_frame {
        return Ok(None);
    }
    log_source("视频截帧");
    let id2 = id.clone();
    let img = tauri::async_runtime::spawn_blocking(move || {
        crate::thumbs::ffmpeg(&bin).and_then(|f| crate::thumbs::frame(&base, &id2, &path, &f))
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(img.map(done))
}

/// 种子里带的海报。资源名是目录：dir/<资源名>/<子目录>/<文件名>
fn torrent_cover(task: &crate::api::Task, dir: &Path, base: &Path, id: &str) -> Option<String> {
    let kind = crate::links::LinkKind::of(&task.url())?;
    if !matches!(kind, crate::links::LinkKind::Magnet | crate::links::LinkKind::TorrentFile) {
        return None;
    }
    let res = task.meta.as_ref()?.res.as_ref()?;
    let files: Vec<PathBuf> = res.files.iter().map(|f| Path::new(&f.path).join(&f.name)).collect();
    let image = crate::thumbs::pick_torrent_image(&files)?;
    crate::thumbs::from_torrent(base, id, &dir.join(&res.name).join(image))
}

/// 磁力、种子里有哪些文件（新建框让用户勾选）。元数据要从网上找，可能要等一会儿
#[tauri::command]
pub async fn bt_files(url: String, state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let (port, token) = endpoint(&state);
    // 冷门种子在 BT 网络里可能几分钟都拿不到元数据：同时查公开的种子缓存，谁先到用谁
    let (tx, mut rx) = tokio::sync::mpsc::channel(2);
    let from_swarm = tx.clone();
    let (p, t, u) = (port, token.clone(), url.clone());
    tauri::async_runtime::spawn(async move {
        let _ = from_swarm.send(Api::new(p, t).resolve_files(&u).await.map_err(|e| e.to_string())).await;
    });
    if let Some(hash) = magnet_btih(&url) {
        tauri::async_runtime::spawn(async move {
            // 不交回内核解析：同一个 hash 已在解析时内核不返回，这里自己读种子文件
            let result = cached_torrent(&hash)
                .await
                .and_then(|data| torrent_files(&data))
                .ok_or_else(|| "种子缓存里没有".to_string());
            let _ = tx.send(result).await;
        });
    } else {
        drop(tx);
    }
    let mut last = Err("没拿到文件列表".to_string());
    while let Some(result) = rx.recv().await {
        if result.is_ok() {
            return result;
        }
        last = result;
    }
    last
}

/// bencode 值。只读文件列表，够用就行
enum Ben {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Ben>),
    Dict(Vec<(Vec<u8>, Ben)>),
}

impl Ben {
    fn parse(b: &[u8], i: &mut usize) -> Option<Ben> {
        match *b.get(*i)? {
            b'i' => {
                let end = *i + b[*i..].iter().position(|&c| c == b'e')?;
                let n = std::str::from_utf8(&b[*i + 1..end]).ok()?.parse().ok()?;
                *i = end + 1;
                Some(Ben::Int(n))
            }
            b'l' | b'd' => {
                let dict = b[*i] == b'd';
                *i += 1;
                let (mut list, mut map) = (Vec::new(), Vec::new());
                while *b.get(*i)? != b'e' {
                    if dict {
                        let Ben::Bytes(k) = Ben::parse(b, i)? else { return None };
                        map.push((k, Ben::parse(b, i)?));
                    } else {
                        list.push(Ben::parse(b, i)?);
                    }
                }
                *i += 1;
                Some(if dict { Ben::Dict(map) } else { Ben::List(list) })
            }
            b'0'..=b'9' => {
                let colon = *i + b[*i..].iter().position(|&c| c == b':')?;
                let len: usize = std::str::from_utf8(&b[*i..colon]).ok()?.parse().ok()?;
                let end = (colon + 1).checked_add(len)?;
                let v = b.get(colon + 1..end)?.to_vec();
                *i = end;
                Some(Ben::Bytes(v))
            }
            _ => None,
        }
    }
    fn get(&self, key: &str) -> Option<&Ben> {
        let Ben::Dict(m) = self else { return None };
        m.iter().find(|(k, _)| k == key.as_bytes()).map(|(_, v)| v)
    }
    fn text(&self) -> Option<String> {
        let Ben::Bytes(v) = self else { return None };
        Some(String::from_utf8_lossy(v).into_owned())
    }
    fn int(&self) -> Option<i64> {
        let Ben::Int(n) = self else { return None };
        Some(*n)
    }
}

/// 种子文件 → 和内核 resolve 一样的结构。文件顺序不动，下标要和内核对上
fn torrent_files(data: &[u8]) -> Option<serde_json::Value> {
    let root = Ben::parse(data, &mut 0)?;
    let info = root.get("info")?;
    let utf8 = |d: &Ben, k: &str| d.get(&format!("{k}.utf-8")).or_else(|| d.get(k)).and_then(Ben::text);
    let name = utf8(info, "name")?;
    let files: Vec<serde_json::Value> = if let Some(Ben::List(list)) = info.get("files") {
        list.iter()
            .map(|f| {
                let Some(Ben::List(parts)) = f.get("path.utf-8").or_else(|| f.get("path")) else { return None };
                let mut parts: Vec<String> = parts.iter().filter_map(Ben::text).collect();
                let file = parts.pop()?;
                Some(serde_json::json!({ "name": file, "path": parts.join("/"), "size": f.get("length")?.int()? }))
            })
            .collect::<Option<_>>()?
    } else {
        vec![serde_json::json!({ "name": name, "path": "", "size": info.get("length")?.int()? })]
    };
    Some(serde_json::json!({ "res": { "name": name, "files": files } }))
}

/// 磁力里的 40 位十六进制 info hash（base32 的不查缓存）
fn magnet_btih(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("btih:")?;
    let hash = rest.split('&').next()?;
    (hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| hash.to_ascii_uppercase())
}

/// 查过的种子留在内存里：新建框、查重名、判完成都要用，不必各等一次网络
async fn cached_torrent(hash: &str) -> Option<Vec<u8>> {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>> = std::sync::OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(d) = memo.lock().unwrap().get(hash) {
        return Some(d.clone());
    }
    let data = fetch_cached_torrent(hash).await?;
    memo.lock().unwrap().insert(hash.to_string(), data.clone());
    Some(data)
}

async fn fetch_cached_torrent(hash: &str) -> Option<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .ok()?;
    // 批量时缓存站偶尔超时或限流，隔一会儿再试；404 才是真没有
    for attempt in 0..3u64 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(attempt * 3)).await;
        }
        let Ok(res) = client
            .get(format!("https://itorrents.org/torrent/{hash}.torrent"))
            .header("User-Agent", "Mozilla/5.0")
            .send()
            .await
        else {
            continue;
        };
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return None;
        }
        if !res.status().is_success() {
            continue;
        }
        let Ok(data) = res.bytes().await else { continue };
        // bencode 字典才是种子文件，缓存站的错误页不算
        if data.first() == Some(&b'd') {
            return Some(data.to_vec());
        }
    }
    None
}

#[tauri::command]
pub async fn add_task(
    url: String,
    name: Option<String>,
    dir: Option<String>,
    headers: Option<Vec<(String, String)>>,
    video: Option<crate::videos::VideoOptions>,
    force_new: Option<bool>,
    select_files: Option<Vec<usize>>,
    remember_dir: Option<String>,
    remember_video: Option<bool>,
    browser_handoff_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    crate::download_handoff::begin(browser_handoff_id.as_deref(), &url)?;
    let result = add_link_inner(
        &url,
        &state,
        name.as_deref().unwrap_or(""),
        dir.as_deref().unwrap_or(""),
        &headers.unwrap_or_default(),
        video,
        force_new.unwrap_or(false),
        &select_files.unwrap_or_default(),
        remember_dir.as_deref(),
        remember_video.unwrap_or(true),
    )
    .await;
    crate::download_handoff::finish(browser_handoff_id.as_deref(), &result);
    result
}

/// 命令行/深链/剪贴板这些「不是用户点按钮」的入口走这里，
/// 名字和目录都用默认值，保证和界面上的目录策略完全一致。
pub async fn add_link(url: &str, state: &AppState) -> Result<String, String> {
    add_link_opts(url, state, "", "", None).await
}

/// 投任务的真实实现。name 非空就用它当任务名，dir_override 非空就存到那里
/// （「新建下载」确认框里用户改过的值从这里进来）。
/// video 是确认框里选的视频站选项（画质／字幕／仅音频）；None 就用记住的选项。
pub async fn add_link_opts(
    url: &str,
    state: &AppState,
    name: &str,
    dir_override: &str,
    video: Option<crate::videos::VideoOptions>,
) -> Result<String, String> {
    add_link_full(url, state, name, dir_override, &[], video).await
}

/// 浏览器扩展 / 用户脚本交过来的任务：多一组请求头（网盘直链要 Cookie、UA、Referer）。
/// 其余路径和界面里加任务完全一样，存到哪、走内核还是 yt-dlp 都由同一套规则决定。
pub async fn add_link_full(
    url: &str,
    state: &AppState,
    name: &str,
    dir_override: &str,
    headers: &[(String, String)],
    video: Option<crate::videos::VideoOptions>,
) -> Result<String, String> {
    add_link_inner(url, state, name, dir_override, headers, video, false, &[], None, true).await
}

/// 用户明确要再下一份（重新下载按钮）：文件还在就按 (1)、(2) 另存
pub async fn add_link_new_copy(url: &str, state: &AppState) -> Result<String, String> {
    add_link_inner(url, state, "", "", &[], None, true, &[], None, true).await
}

#[tauri::command]
pub fn video_task_options(
    id: String,
    state: State<'_, AppState>,
) -> Result<crate::videos::VideoOptions, String> {
    id.strip_prefix("v:")
        .and_then(|id| state.videos.find(id))
        .map(|job| job.options)
        .ok_or_else(|| "找不到原视频任务".to_string())
}

/// 重新下载到指定目录，且**不**另存副本（force_new=false）：同名就按原样下，
/// 配合调用方先清掉旧文件，效果就是"覆盖原来那份"
pub async fn add_link_overwrite(
    url: &str,
    state: &AppState,
    dir: &str,
    video: Option<crate::videos::VideoOptions>,
    remember_video: bool,
) -> Result<String, String> {
    add_link_inner(url, state, "", dir, &[], video, false, &[], None, remember_video).await
}

async fn add_link_inner(
    url: &str,
    state: &AppState,
    name: &str,
    dir_override: &str,
    headers: &[(String, String)],
    video: Option<crate::videos::VideoOptions>,
    force_new: bool,
    select: &[usize],
    remember_dir: Option<&str>,
    remember_video: bool,
) -> Result<String, String> {
    if crate::baidu::share_url(url).is_some() {
        return Err("请在「新建下载」中确认百度网盘分享下载".into());
    }
    if crate::feishu::document_url(url).is_some() { return Err("请在新建下载中选择飞书附件".into()); }
    // Dropbox、Google Drive 的公开分享链接先换成直接下载的地址
    let direct = crate::links::cloud_direct(url);
    let url = direct.as_str();
    if crate::links::video_collection(url, false).is_some() {
        return Err("请在「新建下载」中选中要下载的合集视频".into());
    }
    let kind = crate::links::LinkKind::of(&url)
        .ok_or("这个链接不认识。支持磁力、种子文件、ed2k、直链")?;
    if kind == crate::links::LinkKind::VideoSite {
        // 视频站要走 yt-dlp，走内核只会下到一个网页
        let dir = if dir_override.trim().is_empty() {
            let s = state.settings.lock().unwrap();
            s.default_dir(kind, &state.folders)
        } else {
            PathBuf::from(dir_override)
        };
        if remember_dir.is_some() && !force_new {
            let key = crate::links::canonical_url(url);
            if state.videos.list().iter().any(|job| {
                job.state == "done" && Path::new(&job.dir) == dir
                    && crate::links::canonical_url(&job.url) == key
                    && !job.file_path.is_empty() && resolve_existing(Path::new(&job.file_path)).is_some()
            }) {
                return Err("这个链接已经在下载列表里了".into());
            }
        }
        let bin = state.tools.dir.clone();
        let ytdlp = bin.join("yt-dlp.exe");
        // 确认框里选过就按它来并记住（Mac 版 VideoOptions.current 同款语义）；
        // 浏览器扩展、剪贴板这些没选过的，用上次记住的
        let opts = match video {
            Some(v) => {
                if remember_video {
                    v.save(&state.base);
                }
                v
            }
            None => crate::videos::VideoOptions::load(&state.base),
        };
        // 记住的是用户的偏好，这条任务没字幕可下就只对它关掉，偏好不动
        let mut opts = opts;
        opts.subtitles = opts.subtitles && crate::links::supports_subtitles(url);
        {
            // 记住这次存到哪了，下次新建下载的下拉里能直接选
            let mut st = state.settings.lock().unwrap();
            let recent = remember_dir.map(Path::new).unwrap_or(&dir);
            st.remember_dir(recent);
            if !dir_override.trim().is_empty() {
                st.last_dirs.insert(kind.label().to_string(), recent.to_string_lossy().into_owned());
            }
            let _ = st.save(&state.base);
        }
        let copy_anyway = force_new && !opts.confirm_collision;
        return state
            .videos
            .start(&url, &dir, &ytdlp, &bin, opts, copy_anyway)
            .map_err(|e| e.to_string());
    }

    // 目录策略在 Rust 侧算：让前端传路径，早晚会和设置里的位置对不上
    let (port, token) = {
        let e = state.engine.lock().unwrap();
        (e.port, e.token.clone())
    };
    let dir = if dir_override.trim().is_empty() {
        let s = state.settings.lock().unwrap();
        s.default_dir(kind, &state.folders)
    } else {
        PathBuf::from(dir_override)
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("建下载目录失败：{e}"))?;
    {
        let mut st = state.settings.lock().unwrap();
        st.remember_dir(&dir);
            if !dir_override.trim().is_empty() {
                st.last_dirs.insert(kind.label().to_string(), dir.to_string_lossy().into_owned());
            }
        let _ = st.save(&state.base);
    }
    let api = Api::new(port, token);
    let mut name = name.to_string();
    if force_new {
        let key = crate::links::canonical_url(url);
        let same: Vec<crate::api::Task> = api
            .list_tasks()
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|t| crate::links::canonical_url(&t.url()) == key)
            .collect();
        if matches!(kind, crate::links::LinkKind::Magnet | crate::links::LinkKind::TorrentFile) {
            // BT 同一个 infohash 内核里只能有一条。下完的旧记录让位（文件留着，
            // 内核校验后缺什么补什么）；还在下的不动，新建一条也快不了
            if same.iter().any(|t| t.status != "done") {
                return Err("这个种子还在下载中，等它下完或先删掉再重新下载".into());
            }
            for t in &same {
                api.delete(&t.id, false).await.map_err(|e| e.to_string())?;
            }
        } else if name.trim().is_empty() {
            // 没给名字时内核自己按「x (1).mp4」改名，这里统一成 (1) 的写法
            if let Some(r) = same.iter().find_map(|t| t.meta.as_ref()?.res.as_ref()) {
                name = r.files.first().map(|f| f.name.clone()).unwrap_or_else(|| r.name.clone());
            }
        }
    }
    // 创建任务之前查重名：所有入口都经过这里。冲突就不建任务，交给界面问，选完再建
    let is_bt = matches!(kind, crate::links::LinkKind::Magnet | crate::links::LinkKind::TorrentFile);
    if is_bt || !force_new {
        if let Some(real) = planned_name(&url, &name, is_bt).await {
            if dir.join(&real).exists() {
                let token = format!("{:x}", rand::random::<u64>());
                let complete = is_bt && already_complete(&url, &dir, &real, select).await;
                pending_new().lock().unwrap().insert(token.clone(), PendingNew {
                    url: url.to_string(), dir: dir.clone(), name: real.clone(), headers: headers.to_vec(), select: select.to_vec(), bt: is_bt, history: None,
                });
                if let Some(app) = APP.get() {
                    use tauri::{Emitter, Manager};
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.show();
                        let _ = w.unminimize();
                        let _ = w.emit("name-conflict-new", serde_json::json!({ "token": token, "name": real, "dir": dir.to_string_lossy(), "bt": is_bt, "complete": complete, "url": url }));
                    }
                }
                return Ok(String::new()); // 空 id = 还没建，等用户选
            }
        }
    }
    // 目录里已经有同名文件就顺延成 (1)、(2)，别覆盖旧的
    let name = unique_name(&dir, &name);

    // GitHub / Hugging Face 文件：各加速站和原链接一起测，挑最快的交给内核（设置里可关）
    let gh = kind == crate::links::LinkKind::Http
        && state.settings.lock().unwrap().github_mirror
        && crate::ghmirror::is_supported_file(url);
    let (url, headers) = if gh {
        crate::ghmirror::route(url, headers).await
    } else {
        (url.to_string(), headers.to_vec())
    };

    api
        .add_task(&url, &dir.to_string_lossy(), &name, &headers, select)
        .await
        .map_err(|e| e.to_string())
}

// ---------------- 多文件任务 ----------------

struct PendingMulti {
    folder: String,
    dir: PathBuf,
    files: Vec<crate::api::MultiFile>,
}

fn pending_multi() -> &'static std::sync::Mutex<std::collections::HashMap<String, PendingMulti>> {
    static P: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, PendingMulti>>> = std::sync::OnceLock::new();
    P.get_or_init(Default::default)
}

/// 文件名去掉路径和 Windows 不认的字符
pub(crate) fn safe_component(name: &str) -> String {
    name.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect::<String>().trim().trim_end_matches('.').to_string()
}

/// 文件在任务文件夹里的位置。名字可带子文件夹（a/b/c.mkv），每段单独清理
fn multi_path(root: &Path, name: &str) -> Option<PathBuf> {
    let parts: Vec<String> = name.split(['/', '\\']).map(safe_component).filter(|p| !p.is_empty() && p != ".." && p != ".").collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.iter().fold(root.to_path_buf(), |p, s| p.join(s)))
}

fn multi_existing(root: &Path, files: &[crate::api::MultiFile]) -> Vec<PathBuf> {
    files.iter().filter_map(|f| multi_path(root, &f.name)).filter(|p| p.exists()).collect()
}

/// 网盘文件夹：一条任务下载多个文件。整组只问一次重名；有冲突时返回空 id，等 multi_conflict
#[tauri::command]
pub async fn add_multi(name: String, dir: Option<String>, files: Vec<crate::api::MultiFile>, state: State<'_, AppState>) -> Result<String, String> {
    let first = files.first().ok_or("没有要下载的文件")?;
    let kind = crate::links::LinkKind::of(&first.url).unwrap_or(crate::links::LinkKind::Http);
    let override_dir = dir.unwrap_or_default();
    let dir = if override_dir.trim().is_empty() {
        state.settings.lock().unwrap().default_dir(kind, &state.folders)
    } else {
        PathBuf::from(&override_dir)
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("建下载目录失败：{e}"))?;
    {
        let mut st = state.settings.lock().unwrap();
        st.remember_dir(&dir);
        if !override_dir.trim().is_empty() {
            st.last_dirs.insert(kind.label().to_string(), dir.to_string_lossy().into_owned());
        }
        let _ = st.save(&state.base);
    }
    let folder = safe_component(&name);
    let folder = if folder.is_empty() { "多文件下载".to_string() } else { folder };
    let existing = multi_existing(&dir.join(&folder), &files);
    if !existing.is_empty() {
        let token = format!("{:x}", rand::random::<u64>());
        let total = files.len();
        pending_multi().lock().unwrap().insert(token.clone(), PendingMulti { folder: folder.clone(), dir: dir.clone(), files });
        if let Some(app) = APP.get() {
            use tauri::{Emitter, Manager};
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.emit("multi-conflict", serde_json::json!({ "token": token, "name": folder, "dir": dir.to_string_lossy(), "total": total, "existing": existing.len() }));
            }
        }
        return Ok(String::new());
    }
    let (port, tok) = endpoint(&state);
    Api::new(port, tok).add_multi(&folder, &dir.to_string_lossy(), &files).await.map_err(|e| e.to_string())
}

/// skip：跳过已有的文件；recycle：已有的进回收站后重新下；saveas：另建一个文件夹
#[tauri::command]
pub async fn multi_conflict(token: String, action: String, state: State<'_, AppState>) -> Result<(), String> {
    let Some(mut p) = pending_multi().lock().unwrap().remove(&token) else { return Ok(()) };
    let root = p.dir.join(&p.folder);
    match action.as_str() {
        "skip" => {
            p.files.retain(|f| !multi_path(&root, &f.name).is_some_and(|x| x.exists()));
            if p.files.is_empty() {
                return Ok(());
            }
        }
        "recycle" => recycle_files(multi_existing(&root, &p.files)).await?,
        "saveas" => p.folder = unique_name(&p.dir, &p.folder),
        _ => return Ok(()),
    }
    let (port, tok) = endpoint(&state);
    Api::new(port, tok).add_multi(&p.folder, &p.dir.to_string_lossy(), &p.files).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// 从历史重新下载多文件任务：已有的文件进回收站，下到原来的位置。
/// 链接可能已过期，照样尝试，失败由任务报错提示
pub async fn multi_redownload(multi: &serde_json::Value, dir: &str, state: &State<'_, AppState>) -> Result<String, String> {
    let folder = multi["folder"].as_str().unwrap_or_default().to_string();
    let dir = if dir.trim().is_empty() { multi["dir"].as_str().unwrap_or_default().to_string() } else { dir.to_string() };
    let files: Vec<crate::api::MultiFile> = multi["files"].as_array().map(|l| l.iter().map(|f| crate::api::MultiFile {
        url: f["url"].as_str().unwrap_or_default().to_string(),
        name: f["name"].as_str().unwrap_or_default().to_string(),
        headers: f["header"].as_object().map(|h| h.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect()).unwrap_or_default(),
    }).collect()).unwrap_or_default();
    if files.is_empty() || dir.is_empty() {
        return Err("这条记录没有保存文件列表，请到网盘重新推送".into());
    }
    let root = PathBuf::from(&dir).join(&folder);
    recycle_files(multi_existing(&root, &files)).await?;
    let (port, tok) = endpoint(state);
    Api::new(port, tok).add_multi(&folder, &dir, &files).await.map_err(|e| e.to_string())
}

/// 任务列表里重新下载多文件任务：用任务里存的文件列表再建一条，建好后去掉旧任务（文件已进回收站）
#[tauri::command]
pub async fn multi_task_redownload(id: String, state: State<'_, AppState>) -> Result<String, String> {
    let (port, tok) = endpoint(&state);
    let api = Api::new(port, tok);
    let list = api.list_tasks().await.map_err(|e| e.to_string())?;
    let t = list.iter().find(|t| t.id == id).ok_or("任务不存在")?;
    let url = t.url();
    let extra = t.meta.as_ref().and_then(|m| m.req.as_ref()).map(|r| r.extra.clone()).unwrap_or_default();
    let multi = serde_json::json!({ "folder": url.trim_start_matches("shandian-multi:"), "dir": task_dir(t, &state).to_string_lossy(), "files": extra["files"] });
    // 旧任务先让出文件，否则占着打不开、进不了回收站
    if t.status != "done" {
        let _ = api.pause(&id).await;
    }
    let new_id = multi_redownload(&multi, "", &state).await?;
    let _ = api.delete(&id, false).await;
    Ok(new_id)
}

/// 多文件任务的文件夹、位置和文件列表（带鉴权信息，只给本机界面弹新建框用）
#[tauri::command]
pub async fn multi_task_info(id: String, state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let (port, tok) = endpoint(&state);
    let list = Api::new(port, tok).list_tasks().await.map_err(|e| e.to_string())?;
    let t = list.iter().find(|t| t.id == id).ok_or("任务不存在")?;
    let url = t.url();
    let extra = t.meta.as_ref().and_then(|m| m.req.as_ref()).map(|r| r.extra.clone()).unwrap_or_default();
    Ok(serde_json::json!({ "folder": url.trim_start_matches("shandian-multi:"), "dir": task_dir(t, &state).to_string_lossy(), "files": extra["files"] }))
}

/// 更新原任务链接并继续：内核负责匹配原文件和保留 connections，不走删除/重建。
#[tauri::command]
pub async fn multi_refresh_links(id:String, files:Vec<crate::api::MultiFile>, state:State<'_,AppState>) -> Result<(),String> {
    let (api, raw_id) = multi_target(&state, &id)?;
    crate::cloud_resume::refresh(&api, &raw_id, &files).await
}

/// Resolve the kernel once; refresh intents retain this exact client until completion.
fn multi_target(state: &State<'_, AppState>, id: &str) -> Result<(Api, String), String> {
    if id.starts_with("nas:") { return Err("普通版不支持 NAS 任务".into()); }
    let (port, token) = endpoint(state);
    Ok((Api::new(port, token), id.to_string()))
}

/// 文件面板：任务里每个文件的进度
#[tauri::command]
pub async fn task_file_list(id: String, state: State<'_, AppState>) -> Result<Vec<serde_json::Value>, String> {
    if let Some(raw) = id.strip_prefix("bd:") {
        let job = state.baidu.find(raw).ok_or("任务不存在")?;
        return tauri::async_runtime::spawn_blocking(move || job.drawer_files())
            .await.map_err(|error| error.to_string());
    }
    if id.starts_with("xl:") {
        return crate::thunder::panel_files(&state, &id).await;
    }
    let (port, tok) = endpoint(&state);
    let api = Api::new(port, tok);
    let list = api.list_tasks().await.map_err(|e| e.to_string())?;
    let t = list.iter().find(|t| t.id == id).ok_or("任务不存在")?;
    let stats = api.stats(&id).await.unwrap_or_default();
    Ok(task_files_for_panel(t, &stats))
}

fn completed_multi_has_short_file(task: &crate::api::Task, dir: &Path) -> bool {
    if !task.url().starts_with("shandian-multi:") { return false; }
    let selected = task.meta.as_ref().and_then(|meta| meta.opts.as_ref()).map(|opts| opts.select_files.as_slice()).unwrap_or_default();
    task.files().iter().enumerate().any(|(index, file)| {
        file.size > 0 && (selected.is_empty() || selected.contains(&index))
            && drawer_file_relative(task, index).is_some_and(|relative| {
                std::fs::metadata(dir.join(relative))
                    .is_ok_and(|metadata| metadata.is_file() && metadata.len() < file.size)
            })
    })
}

#[cfg(test)]
#[test]
fn completed_multi_detects_a_partial_file_on_disk() {
    let task: crate::api::Task = serde_json::from_value(serde_json::json!({
        "id": "fixture", "status": "done", "meta": {
            "req": { "url": "shandian-multi:bundle" },
            "res": { "name": "bundle", "files": [{ "name": "音频.mp3", "size": 100 }] }
        }
    })).unwrap();
    let folder = std::env::temp_dir().join(format!("shandian-multi-check-{:016x}", rand::random::<u64>()));
    let bundle = folder.join("bundle");
    let file = bundle.join("音频.mp3");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(&file, [0u8; 79]).unwrap();
    assert!(completed_multi_has_short_file(&task, &folder));
    let stats = serde_json::json!({"files": [{"downloaded":79,"size":100,"status":"done"}]});
    assert_eq!(task_files_for_panel(&task, &stats)[0]["status"], "wait");
    std::fs::write(&file, [0u8; 100]).unwrap();
    assert!(!completed_multi_has_short_file(&task, &folder));
    std::fs::remove_file(file).unwrap();
    std::fs::remove_dir(bundle).unwrap();
    std::fs::remove_dir(folder).unwrap();
}

fn task_files_for_panel(t: &crate::api::Task, stats: &serde_json::Value) -> Vec<serde_json::Value> {
    let all = t.files();
    let picked: Vec<usize> = t.meta.as_ref().and_then(|m| m.opts.as_ref()).map(|o| o.select_files.clone()).unwrap_or_default();
    let is_on = |i: usize| picked.is_empty() || picked.contains(&i);
    let done_task = t.status == "done";
    // 多文件任务内核给每个文件的状态；BT 只有选中文件的进度，顺序同选中列表
    if let Some(files) = stats["files"].as_array().filter(|files| !files.is_empty()) {
        return files.iter().enumerate().map(|(i, f)| {
            let mut f = f.clone();
            f["index"] = serde_json::json!(i);
            f["selected"] = serde_json::json!(is_on(i));
            if let (Some(downloaded), Some(size)) = (f["downloaded"].as_u64(), f["size"].as_u64()) {
                if is_on(i) && f["status"].as_str() == Some("done") && downloaded < size {
                    f["status"] = serde_json::json!("wait");
                }
            }
            f
        }).collect();
    }
    let per = stats["shandianFiles"].as_array().cloned().unwrap_or_default();
    let on_list: Vec<usize> = (0..all.len()).filter(|&i| is_on(i)).collect();
    all.iter().enumerate().map(|(i, f)| {
        let on = is_on(i);
        let got = if !on { 0 } else if done_task { f.size } else { on_list.iter().position(|&x| x == i).and_then(|n| per.get(n)).and_then(|v| v.as_u64()).unwrap_or(0) };
        let status = if !on { "skip" } else if done_task || (got >= f.size && f.size > 0) { "done" } else if t.status == "running" { "running" } else { "wait" };
        serde_json::json!({ "index": i, "name": f.name, "size": f.size, "downloaded": got, "status": status, "selected": on })
    }).collect()
}

/// Build the exact path relative to the saved directory, using kernel metadata.
fn drawer_file_relative(task: &crate::api::Task, index: usize) -> Option<PathBuf> {
    use std::path::Component;

    let meta = task.meta.as_ref()?;
    let resource = meta.res.as_ref()?;
    let file = resource.files.get(index)?;
    let custom = meta.opts.as_ref().map(|opts| opts.name.as_str()).filter(|name| !name.is_empty());
    let mut relative = PathBuf::new();
    let folder = task.content_folder();
    let name = if folder.is_some() { file.name.as_str() } else { custom.unwrap_or(&file.name) };
    if name.is_empty() { return None; }
    for part in [folder.as_deref().map(|folder| custom.unwrap_or(folder)).unwrap_or(""), &file.path, name] {
        for component in Path::new(part).components() {
            let Component::Normal(segment) = component else { return None };
            relative.push(segment);
        }
    }
    (!relative.as_os_str().is_empty()).then_some(relative)
}

async fn completed_drawer_file(id: &str, index: usize, state: &State<'_, AppState>) -> Result<(PathBuf, PathBuf), String> {
    if let Some(raw) = id.strip_prefix("bd:") {
        let job = state.baidu.find(raw).ok_or("任务不存在")?;
        return tauri::async_runtime::spawn_blocking(move || job.drawer_path(index))
            .await.map_err(|error| error.to_string())?;
    }
    let (api, raw_id) = multi_target(state, id)?;
    let tasks = api.list_tasks().await.map_err(|error| error.to_string())?;
    let task = tasks.iter().find(|task| task.id == raw_id).ok_or("任务不存在")?;
    let stats = api.stats(&raw_id).await.unwrap_or_default();
    let rows = task_files_for_panel(task, &stats);
    let file = rows.get(index).ok_or("找不到这个文件")?;
    if file["selected"].as_bool() != Some(true) || file["status"].as_str() != Some("done") {
        return Err("文件尚未下载完成".into());
    }
    let relative = drawer_file_relative(task, index).ok_or("找不到文件的保存位置")?;
    let dir = task_dir(task, state.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let path = dir.join(&relative);
        #[cfg(feature = "nas")]
        let (dir, path) = if let Some(saved_dir) = nas_dir {
            let saved = saved_dir.to_string_lossy().replace('\\', "/");
            let full = format!("{}/{}", saved.trim_end_matches('/'), relative.to_string_lossy().replace('\\', "/"));
            let root = crate::nas::download_unc_path(&local, &saved)?;
            let file = crate::nas::download_unc_path(&local, &full)?;
            (PathBuf::from(root), PathBuf::from(file))
        } else { (dir, path) };
        if !path.is_file() { return Err("文件不在原位置，或当前无法访问".into()); }
        Ok((dir, path))
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn task_open_downloaded_file(id: String, index: usize, reveal: Option<bool>, state: State<'_, AppState>) -> Result<(), String> {
    let (_, path) = completed_drawer_file(&id, index, &state).await?;
    tauri::async_runtime::spawn_blocking(move || {
        if reveal.unwrap_or(false) {
            crate::commands::reveal_path(path.to_string_lossy().into_owned())
        } else {
            shell_open(&path)
        }
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn task_downloaded_file_path(id: String, index: usize, state: State<'_, AppState>) -> Result<String, String> {
    let (_, path) = completed_drawer_file(&id, index, &state).await?;
    Ok(path.to_string_lossy().into_owned())
}

#[derive(Serialize)]
pub struct DrawerAudioSource {
    path: String,
    size: u64,
}

#[tauri::command]
pub async fn task_drawer_audio(id: String, index: usize, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<DrawerAudioSource, String> {
    use tauri::Manager;
    let (dir, path) = completed_drawer_file(&id, index, &state).await?;
    tauri::async_runtime::spawn_blocking(move || {
        let ext = path.extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
        if !matches!(ext.as_str(), "mp3" | "flac" | "wav" | "aac" | "m4a" | "ogg" | "oga" |
            "opus" | "ape" | "wma" | "aif" | "aiff" | "amr" | "ac3" | "dts" | "mka" | "mid" | "midi") {
            return Err("这个文件不是音频".into());
        }
        let root = dir.canonicalize().map_err(|e| format!("无法访问保存目录：{e}"))?;
        let canonical = path.canonicalize().map_err(|e| format!("无法访问音频：{e}"))?;
        if !canonical.starts_with(&root) {
            return Err("音频不在任务的保存目录中".into());
        }
        let size = canonical.metadata().map_err(|e| format!("无法读取音频：{e}"))?.len();
        app.asset_protocol_scope().allow_file(&canonical).map_err(|e| format!("无法打开试听：{e}"))?;
        Ok(DrawerAudioSource { path: path.to_string_lossy().into_owned(), size })
    }).await.map_err(|error| error.to_string())?
}

/// 文件抽屉里改了要下载哪些文件
#[tauri::command]
pub async fn task_select_files(id: String, select: Vec<usize>, state: State<'_, AppState>) -> Result<(), String> {
    if select.is_empty() {
        return Err("至少要选一个文件".into());
    }
    if id.starts_with("xl:") {
        return crate::thunder::panel_select(&state, &id, select).await;
    }
    let (port, tok) = endpoint(&state);
    Api::new(port, tok).select_files(&id, &select).await.map_err(|e| e.to_string())
}

// ---------------- 创建前查重名 ----------------

pub(crate) static APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

struct PendingNew {
    url: String,
    dir: PathBuf,
    name: String,
    headers: Vec<(String, String)>,
    select: Vec<usize>,
    bt: bool,
    history: Option<String>,
}

pub fn history_waits_for_choice(url: &str, history_id: &str) {
    for p in pending_new().lock().unwrap().values_mut().filter(|p| p.url == url) {
        p.history = Some(history_id.to_string());
    }
}

fn pending_new() -> &'static std::sync::Mutex<std::collections::HashMap<String, PendingNew>> {
    static P: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, PendingNew>>> = std::sync::OnceLock::new();
    P.get_or_init(Default::default)
}

fn name_hints() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static H: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();
    H.get_or_init(Default::default)
}

/// 新建框已经查到真名：投任务前告诉后端，不用再查一遍
#[tauri::command]
pub fn name_hint(url: String, name: String) {
    name_hints().lock().unwrap().insert(url, name);
}

/// 任务将要落盘的名字。直链看给定的名字或链接里的文件名（服务器另给名字时要等内核才知道）；
/// 磁力查种子缓存，最多等 6 秒；查不到才退回 dn，并由 start_name_guard 在元数据到时再查
async fn planned_name(url: &str, name: &str, bt: bool) -> Option<String> {
    if let Some(n) = name_hints().lock().unwrap().remove(url) {
        return Some(n);
    }
    let parsed = url::Url::parse(url).ok()?;
    if bt {
        if let Some(hash) = magnet_btih(url) {
            let looked = tokio::time::timeout(std::time::Duration::from_secs(6), cached_torrent(&hash)).await;
            if let Some(v) = looked.ok().flatten().and_then(|d| torrent_files(&d)) {
                if let Some(n) = v["res"]["name"].as_str().filter(|n| !n.is_empty()) {
                    return Some(n.to_string());
                }
            }
        }
        return parsed.query_pairs().find(|(k, _)| k == "dn").map(|(_, v)| v.into_owned()).filter(|n| !n.is_empty());
    }
    if !name.trim().is_empty() {
        return Some(name.to_string());
    }
    let last = parsed.path_segments()?.next_back()?.to_string();
    let decoded = url::form_urlencoded::parse(last.replace('+', "%2B").as_bytes()).map(|(k, _)| k.into_owned()).next().unwrap_or(last);
    (!decoded.is_empty()).then_some(decoded)
}

/// 已有内容是不是这个种子下完的样子：要下的文件都在、大小都对，且没有下载工具的临时文件。
/// 只比大小，不读内容（几十 GB 校验太慢）；拿不到种子信息时返回 false
async fn already_complete(url: &str, dir: &Path, name: &str, select: &[usize]) -> bool {
    let Some(hash) = magnet_btih(url) else { return false };
    let looked = tokio::time::timeout(std::time::Duration::from_secs(6), cached_torrent(&hash)).await;
    let Some(v) = looked.ok().flatten().and_then(|d| torrent_files(&d)) else { return false };
    let Some(files) = v["res"]["files"].as_array() else { return false };
    if files.is_empty() {
        return false;
    }
    let root = dir.join(name);
    files.iter().enumerate().filter(|(i, _)| select.is_empty() || select.contains(i)).all(|(_, f)| {
        let fname = f["name"].as_str().unwrap_or_default();
        let inner = root.join(f["path"].as_str().unwrap_or_default()).join(fname);
        // 单文件种子：根就是文件本身
        let p = if root.is_file() { root.clone() } else { inner };
        let partial = p.with_file_name(format!("{fname}.bt.xltd")).exists() || p.with_file_name(format!("{fname}.xltd")).exists();
        !partial && std::fs::metadata(&p).map_or(false, |m| m.len() == f["size"].as_u64().unwrap_or(u64::MAX))
    })
}

/// 用户对重名的选择，选完才创建任务。saveas：加存；recycle：旧的进回收站后重新下；verify：接着已有内容下（仅 BT）
#[tauri::command]
pub async fn name_conflict_new(token: String, action: String, state: State<'_, AppState>) -> Result<(), String> {
    let Some(p) = pending_new().lock().unwrap().remove(&token) else { return Ok(()) };
    // 只是看看：下完的 BT 进它的文件夹，单个文件在资源管理器里选中。不建任务
    if action == "folder" {
        let target = p.dir.join(&p.name);
        let s = target.to_string_lossy().into_owned();
        return if target.is_dir() { crate::commands::open_dir(s) } else { crate::commands::reveal_path(s) };
    }
    let (port, tok) = endpoint(&state);
    let api = Api::new(port, tok);
    let (mut path, mut file) = (p.dir.clone(), if p.bt { String::new() } else { p.name.clone() });
    match action.as_str() {
        "saveas" if p.bt => {
            // 内核的 BT 文件名改不了，只能换外层文件夹
            path = (1..1000).map(|n| p.dir.join(format!("{}({n})", p.name))).find(|d| !d.exists()).unwrap_or_else(|| p.dir.join(format!("{}(1)", p.name)));
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        }
        "saveas" => file = unique_name(&p.dir, &p.name),
        "recycle" => recycle_files(vec![p.dir.join(&p.name)]).await?,
        "verify" if p.bt => {}
        _ => return Ok(()),
    }
    let id = api.add_task(&p.url, &path.to_string_lossy(), &file, &p.headers, &p.select).await.map_err(|e| e.to_string())?;
    name_seen().lock().unwrap().insert(id.clone());
    if let Some(h) = &p.history {
        crate::trash::forget_record(&state, h);
    }
    if p.bt {
        display_names().lock().unwrap().insert(id.clone(), p.name.clone());
    }
    if action == "verify" {
        let _ = verify_task(id, state).await;
    }
    Ok(())
}

// ---------------- BT 重名 ----------------
//
// 内核对 BT 不改名（AutoRename=false），也不查目录，同名就直接写进去。名字要等元数据到了才知道，
// 而且所有入口（新建框、关掉确认框、悬浮窗、扩展）都走内核，所以统一在这里盯：
// 元数据一到、发现目录里有早于任务的同名内容，立刻暂停，问用户。
// 轮询 1 秒，最坏情况暂停前可能已写入极少数据；彻底杜绝要改内核补丁。

fn name_ok_file(state: &AppState) -> PathBuf {
    crate::migrate::local_path(&state.local, "name-checked.json")
}

/// 这条任务的内容是否早于任务本身就在目录里（任务自己建的文件夹不算）
fn name_clash(t: &crate::api::Task, dir: &Path) -> Option<String> {
    let res = t.meta.as_ref()?.res.as_ref()?;
    // BT 看资源名；直链只有文件名（内核已自动改名避开，但用户仍要知道）
    let name = if res.name.is_empty() { res.files.first()?.name.clone() } else { res.name.clone() };
    older_than_task(t, dir, &name).then_some(name)
}

fn older_than_task(t: &crate::api::Task, dir: &Path, name: &str) -> bool {
    if name.trim().is_empty() {
        return false;
    }
    let born = std::fs::metadata(dir.join(name)).ok().and_then(|m| m.created().ok()).and_then(|c| c.duration_since(std::time::UNIX_EPOCH).ok());
    matches!((born, t.created), (Some(b), Some(c)) if (b.as_millis() as u64) + 3000 < c)
}

/// 确认框查到的真名。内核拿到元数据前只有磁力 dn（常常是别的名字），列表上先显示真名
fn display_names() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static NAMES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();
    NAMES.get_or_init(Default::default)
}

/// 已经问过或查过的任务，不再问
fn name_seen() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    SEEN.get_or_init(Default::default)
}

/// 暂停任务并让界面问用户。元数据没到时内核还没写任何数据，这时问最安全
async fn ask_name_conflict(app: &tauri::AppHandle, api: &Api, id: &str, name: &str, dir: &Path, bt: bool) {
    use tauri::{Emitter, Manager};
    let _ = api.pause(id).await;
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.emit("name-conflict", serde_json::json!({ "id": id, "name": name, "dir": dir.to_string_lossy(), "bt": bt }));
    }
}

/// 确认框点「开始下载」时已知真名（确认框查过种子），投完立刻查，不等内核拿元数据
#[tauri::command]
pub async fn name_precheck(app: tauri::AppHandle, id: String, name: String, state: State<'_, AppState>) -> Result<(), String> {
    let (port, token) = endpoint(&state);
    let api = Api::new(port, token);
    let list = api.list_tasks().await.map_err(|e| e.to_string())?;
    let Some(t) = list.iter().find(|t| t.id == id) else { return Ok(()) };
    display_names().lock().unwrap().insert(id.clone(), name.clone());
    let dir = task_dir(t, &state);
    if older_than_task(t, &dir, &name) && name_seen().lock().unwrap().insert(id.clone()) {
        ask_name_conflict(&app, &api, &id, &name, &dir, true).await;
    }
    Ok(())
}

pub fn start_name_guard(app: tauri::AppHandle) {
    let _ = APP.set(app.clone());
    use tauri::{Emitter, Manager};
    tauri::async_runtime::spawn(async move {
        let st = app.state::<AppState>();
        let file = name_ok_file(&st);
        let mut seen: Option<std::collections::HashSet<String>> =
            std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok());
        let mut looked_up: std::collections::HashSet<String> = Default::default();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let (port, token) = {
                let e = st.engine.lock().unwrap();
                (e.port, e.token.clone())
            };
            let Ok(list) = Api::new(port, token.clone()).list_tasks().await else { continue };
            // 第一次运行：现有任务都算查过，别对老任务一通问
            let known = seen.get_or_insert_with(|| list.iter().map(|t| t.id.clone()).collect());
            let mut changed = false;
            known.extend(name_seen().lock().unwrap().iter().cloned());
            let fresh: Vec<_> = list.iter().filter(|t| t.status != "done" && !known.contains(&t.id)).collect();
            for t in fresh {
                if t.meta.as_ref().and_then(|m| m.res.as_ref()).map_or(true, |r| r.name.is_empty() && r.files.is_empty()) {
                    // 元数据还没到，下一秒再看。磁力先查一次公开种子缓存，查到名字就能提前问
                    if t.status == "running" && looked_up.insert(t.id.clone()) {
                        if let Some(hash) = magnet_btih(&t.url()) {
                            let (app2, t2, dir) = (app.clone(), t.clone(), task_dir(t, &st));
                            let (p, k) = (port, token.clone());
                            tauri::async_runtime::spawn(async move {
                                let Some(v) = cached_torrent(&hash).await.and_then(|d| torrent_files(&d)) else { return };
                                let name = v["res"]["name"].as_str().unwrap_or_default().to_string();
                                if older_than_task(&t2, &dir, &name) && name_seen().lock().unwrap().insert(t2.id.clone()) {
                                    ask_name_conflict(&app2, &Api::new(p, k), &t2.id, &name, &dir, true).await;
                                }
                            });
                        }
                    }
                    continue;
                }
                known.insert(t.id.clone());
                changed = true;
                let dir = task_dir(t, &st);
                if let Some(name) = name_clash(t, &dir) {
                    let bt = !t.meta.as_ref().and_then(|m| m.res.as_ref()).map_or(true, |r| r.name.is_empty());
                    ask_name_conflict(&app, &Api::new(port, token.clone()), &t.id, &name, &dir, bt).await;
                }
            }
            if changed {
                let _ = std::fs::write(&file, serde_json::to_vec(known).unwrap_or_default());
            }
        }
    });
}

/// 用户对重名的选择。verify：校验已有内容，缺的补上；saveas：另存到「名字(1)」文件夹；
/// recycle：已有内容进回收站后重下；cancel：取消任务，已有内容不动
#[tauri::command]
pub async fn name_conflict_resolve(id: String, action: String, name: String, bt: bool, state: State<'_, AppState>) -> Result<(), String> {
    let (port, token) = endpoint(&state);
    let api = Api::new(port, token);
    let list = api.list_tasks().await.map_err(|e| e.to_string())?;
    let t = list.iter().find(|t| t.id == id).ok_or("找不到这个任务")?;
    let dir = task_dir(t, &state);
    let select = t.meta.as_ref().and_then(|m| m.opts.as_ref()).map(|o| o.select_files.clone()).unwrap_or_default();
    let url = t.url();
    let is_bt = bt;
    // 直链：内核已给新文件改好名，改不回去，也不能重建（网盘直链的鉴权头拿不回来），所以都接着下
    if !is_bt && action != "cancel" {
        if action == "recycle" {
            if let Some(f) = t.meta.as_ref().and_then(|m| m.res.as_ref()).and_then(|r| r.files.first()) {
                recycle_files(vec![dir.join(&f.name)]).await?;
            }
        }
        return api.resume(&id).await.map_err(|e| e.to_string());
    }
    match action.as_str() {
        "verify" => {
            api.resume(&id).await.map_err(|e| e.to_string())?;
            verify_task(id.clone(), state.clone()).await
        }
        "cancel" => api.delete(&id, false).await.map_err(|e| e.to_string()),
        "recycle" | "saveas" => {
            api.delete(&id, false).await.map_err(|e| e.to_string())?;
            let path = if action == "recycle" {
                recycle_files(vec![dir.join(&name)]).await?;
                dir.clone()
            } else {
                // 内核的 BT 文件名改不了，只能换外层文件夹
                let folder = (1..1000).map(|n| dir.join(format!("{name}({n})"))).find(|p| !p.exists()).unwrap_or_else(|| dir.join(format!("{name}(1)")));
                std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
                folder
            };
            api.add_task(&url, &path.to_string_lossy(), "", &[], &select).await.map_err(|e| e.to_string()).map(|_| ())
        }
        _ => Err("未知操作".into()),
    }
}

/// 目标目录里已经有同名文件时顺延：name.ext → name(1).ext → name(2).ext；
/// 传进来的已经是 name(1).ext 时接着往后数，不会变成 name(1)(1).ext。
fn unique_name(dir: &Path, name: &str) -> String {
    if name.trim().is_empty() || !dir.join(name).exists() {
        return name.to_string();
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    let base = match stem.strip_suffix(')').and_then(|s| s.rsplit_once('(')) {
        Some((b, n)) if !b.is_empty() && !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()) => b,
        _ => stem.as_str(),
    };
    for i in 1..10000 {
        let cand = format!("{base}({i}){ext}");
        if !dir.join(&cand).exists() {
            return cand;
        }
    }
    name.to_string()
}

/// 文件修改时间（毫秒）。给没有 createdAt 的老任务兜底创建时间
fn mtime_ms(p: &Path) -> Option<u64> {
    std::fs::metadata(p)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

/// 把 yt-dlp 任务转成内核任务的形状，界面只认这一种
/// 下完的视频旁边现在有没有字幕文件。列表每秒刷新，按所在目录的修改时间缓存，目录没变就不重新列
fn cached_subtitle_label(video: &str, align_by_id: bool) -> Option<&'static str> {
    use std::collections::HashMap;
    static CACHE: OnceLock<Mutex<HashMap<String, (std::time::SystemTime, Option<&'static str>)>>> = OnceLock::new();
    let p = Path::new(video);
    let Some(dir) = p.parent() else { return None };
    let Ok(mtime) = std::fs::metadata(dir).and_then(|m| m.modified()) else { return None };
    let cache = CACHE.get_or_init(Default::default);
    if let Some((t, v)) = cache.lock().unwrap().get(video) {
        if *t == mtime {
            return *v;
        }
    }
    if align_by_id { let _ = crate::videos::align_subtitles(p); }
    let v = crate::videos::subtitle_file_label(p);
    cache.lock().unwrap().insert(video.to_string(), (mtime, v));
    v
}

fn video_to_task(j: &crate::videos::VideoJob) -> crate::api::Task {
    use crate::api::{Progress, Request, Resource, Task, TaskMeta};
    let status = match j.state.as_str() {
        "wait" => "wait",
        "done" => "done",
        "error" => "error",
        "pause" => "pause",
        _ => "running",
    };
    let name = if j.title.is_empty() { j.url.clone() } else { j.title.clone() };
    let subtitle = (status == "done" && !j.file_path.is_empty())
        .then(|| cached_subtitle_label(&j.file_path, !j.staged_names)).flatten();
    let has_sub = subtitle.is_some();
    Task {
        xl_items: None,
        id: format!("v:{}", j.id),
        name: Some(name.clone()),
        protocol: Some("video".into()),
        status: status.into(),
        subs: if status == "done" { subtitle.map(str::to_owned) } else if j.no_subs { None } else { j.options.subs_label() },
        subs_fetchable: status == "done" && !has_sub && !j.options.audio_only && crate::links::supports_subtitles(&j.url) && Path::new(&j.file_path).is_file(),
        file_state: None,
        subtitle_state: if status != "done" { None } else if j.subs_failed { Some("failed".into()) }
            else if has_sub { Some("available".into()) } else if j.no_subs { Some("none".into()) } else { None },
        progress: Some(Progress {
            downloaded: j.downloaded,
            speed: if status == "running" { j.speed } else { 0 },
            uploaded: 0,
            upload_speed: 0,
        }),
        meta: Some(TaskMeta {
            res: Some(Resource {
                hash: String::new(),
                name,
                size: j.total,
                files: vec![],
            }),
            req: Some(Request { url: j.url.clone(), labels: Default::default(), extra: Default::default() }),
            opts: None,
        }),
        shandian_error: if j.error.is_empty() { None } else { Some(j.error.clone()) },
        shandian_verification: None,
        kind: Some(if j.site.is_empty() { "视频网站".to_string() } else { j.site.clone() }),
        origin_label: None,
        resolution: j.resolution.clone(),
        created: Some(j.created),
        polish: Some(j.polish.clone()).filter(|p| !p.is_empty()),
        polished: j.polished,
        finished: if j.finished_at > 0 { Some(j.finished_at) } else { mtime_ms(Path::new(&j.file_path)) },
        // 是否失效由 list_tasks 摸盘后再填（这里先按「在」处理）
        missing: false,
        stage: (matches!(status, "running" | "done") && !j.stage.is_empty() && j.stage != "preparing")
            .then(|| j.stage.clone()),
        collided: j.collided,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_name_counts_up() {
        let dir = std::env::temp_dir().join(format!("sd-uniq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_name(&dir, "a.mp4"), "a.mp4");
        std::fs::write(dir.join("a.mp4"), b"").unwrap();
        assert_eq!(unique_name(&dir, "a.mp4"), "a(1).mp4");
        std::fs::write(dir.join("a(1).mp4"), b"").unwrap();
        assert_eq!(unique_name(&dir, "a.mp4"), "a(2).mp4");
        assert_eq!(unique_name(&dir, "a(1).mp4"), "a(2).mp4");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_files_rejects_unsafe_names() {
        use crate::api::{Resource, Task, TaskMeta, TaskOpts};
        let dir = std::env::temp_dir().join(format!("sd-tf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("keep.mp4"), b"").unwrap();
        let mk = |res_name: &str, opt: &str| -> Task {
            let mut t: Task = serde_json::from_str(r#"{"id":"a"}"#).unwrap();
            t.meta = Some(TaskMeta {
                res: Some(Resource { name: res_name.into(), size: 0, files: vec![], hash: String::new() }),
                req: None,
                opts: Some(TaskOpts { name: opt.into(), path: String::new(), select_files: Vec::new() }),
            });
            t
        };
        // 空名字、上级目录都不能指到下载目录本身
        assert!(task_files(&mk("", ""), &dir).is_empty());
        assert!(task_files(&mk("..", ""), &dir).is_empty());
        assert!(task_files(&mk(".", ""), &dir).is_empty());
        assert_eq!(task_files(&mk("keep.mp4", ""), &dir), vec![dir.join("keep.mp4")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bilingual_merge_puts_zh_above_en_and_cleans_up() {
        let dir = std::env::temp_dir().join("shandian-subtest");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("Clip.mp4");
        std::fs::write(&video, b"x").unwrap();
        std::fs::write(
            dir.join("Clip.zh-Hans.srt"),
            "1\n00:00:01,000 --> 00:00:03,000\n我们来清理一下\n\n2\n00:00:03,000 --> 00:00:05,000\n绑定到第一个输入\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("Clip.en.srt"),
            "1\n00:00:01,000 --> 00:00:03,000\nlet's clean this up\n\n2\n00:00:03,000 --> 00:00:05,000\nbind it to the first input\n",
        )
        .unwrap();
        // 多余的语言变体应该在合并后消失
        std::fs::write(dir.join("Clip.en-orig.srt"), "1\n00:00:01,000 --> 00:00:02,000\norig\n").unwrap();

        merge_bilingual_subs(&video, 1.0, false, "#00FFAC", "#516EFE");

        let merged = dir.join("Clip.zh-en.srt");
        let body = std::fs::read_to_string(&merged).expect("没生成合并后的字幕");
        assert!(body.contains("我们来清理一下\nlet's clean this up"), "中文应在上、英文在下：\n{body}");
        assert!(body.contains("00:00:01,000 --> 00:00:03,000"));
        assert!(!dir.join("Clip.zh-Hans.srt").exists(), "原中文字幕没删");
        assert!(!dir.join("Clip.en.srt").exists(), "原英文字幕没删");
        assert!(!dir.join("Clip.en-orig.srt").exists(), "多余变体没删");
        assert!(video.exists(), "视频文件被误删了");
        let _ = std::fs::remove_dir_all(&dir);
    }


    #[test]
    fn ass_merge_centers_and_keeps_style() {
        let dir = std::env::temp_dir().join("shandian-ass-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("A.mp4");
        std::fs::write(&video, b"x").unwrap();
        let header = "[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,20,&Hffffff,&Hffffff,&H0,&H0,0,0,0,0,100,100,0,0,1,2,0,7,10,10,10,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";
        std::fs::write(
            dir.join("A.zh-Hans.ass"),
            format!("{header}Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,我们来清理一下\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("A.en.ass"),
            format!("{header}Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,let's clean this up\n"),
        )
        .unwrap();

        merge_bilingual_subs(&video, 1.0, false, "#00FFAC", "#516EFE");

        let body = std::fs::read_to_string(dir.join("A.zh-en.ass")).expect("没生成合并后的 ass");
        assert!(body.contains("我们来清理一下\\Nlet's clean this up"), "中英没合到一行：\n{body}");
        // Alignment 从 7（左上）改成 2（底部居中）
        assert!(body.contains(",2,20,20,"), "Alignment 没改成居中：\n{body}");
        // 字号按画面高度重算：没有 PlayResY 按 720 算 × 0.042 ≈ 30（原样式是 20）
        assert!(body.contains("Style: Default,Microsoft YaHei,12,"), "字号没按画面高度重算：\n{body}");
        assert!(body.contains("[V4+ Styles]"), "样式段丢了");
        assert!(!dir.join("A.zh-Hans.ass").exists());
        assert!(!dir.join("A.en.ass").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }


    #[test]
    fn vtt_timestamps_and_tags_are_handled() {
        let dir = std::env::temp_dir().join("shandian-subtest2");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("V.mp4");
        std::fs::write(&video, b"x").unwrap();
        std::fs::write(
            dir.join("V.zh.vtt"),
            "WEBVTT\n\n00:00:01.500 --> 00:00:04.000\n<v 说话人>你好</v>\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("V.en.vtt"),
            "WEBVTT\n\n00:00:01.500 --> 00:00:04.000\nhi there\n",
        )
        .unwrap();
        merge_bilingual_subs(&video, 1.0, false, "#00FFAC", "#516EFE");
        let body = std::fs::read_to_string(dir.join("V.zh-en.srt")).unwrap();
        assert!(body.contains("00:00:01,500 --> 00:00:04,000"), "时间戳转换不对：\n{body}");
        assert!(body.contains("你好\nhi there"), "内容不干净：\n{body}");
        assert!(!body.contains('<'), "标签没清掉");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod torrent_files_tests {
    #[test]
    fn reads_multi_and_single() {
        let multi = b"d4:infod5:filesld6:lengthi5e4:pathl1:a5:b.mkveee4:name1:xee";
        let v = super::torrent_files(multi).unwrap();
        assert_eq!(v["res"]["name"], "x");
        assert_eq!(v["res"]["files"][0]["path"], "a");
        assert_eq!(v["res"]["files"][0]["name"], "b.mkv");
        assert_eq!(v["res"]["files"][0]["size"], 5);
        let single = b"d4:infod6:lengthi9e4:name3:a.bee";
        assert_eq!(super::torrent_files(single).unwrap()["res"]["files"][0]["size"], 9);
        assert!(super::torrent_files(b"<html>").is_none());
    }
}

#[cfg(test)]
mod restyle_tests {
    use super::*;

    #[test]
    fn restyle_rewrites_style_and_both_lines() {
        let src = r#"[Script Info]
PlayResY: 288

[V4+ Styles]
Style: Default,Microsoft YaHei,12,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,1.5,0.8,2,20,20,9,1

[Events]
Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,{\c&HACFF00&\b1}你好\N{\c&HFE6E51&\b0\fs10}Hello
"#;
        let look = SubLook { scale: 1.0, scale_en: 0.86, colorize: true, color_zh: "#FF0000".into(), color_en: "#0000FF".into(), shadow: true, shadow_size: 0.8, boxed: true, font: "Microsoft YaHei".into(), shadow_alpha: 50.0, box_alpha: 62.0, outline: 1.5, margin: 3.0 };
        let out = restyle_ass(src, &look);
        // 黑底：文字样式仍带描边（1,1.5），底框单独一个 Box 样式（3,3.8）
        assert!(out.contains("Style: Default,Microsoft YaHei,12,") && out.contains(",1,1.5,0.0,2,20,20,9,1"), "{out}");
        assert!(out.contains("Style: Box,Microsoft YaHei,12,&HFF000000,&HFF000000,&H61000000,&H61000000,") && out.contains(",3,3.8,0,2,20,20,9,1"), "{out}");
        assert!(out.contains("Dialogue: 0,0:00:01.00,0:00:02.00,Box,") && out.contains("Dialogue: 1,0:00:01.00,0:00:02.00,Default,"), "{out}");
        // 再套一次不带黑底的样式：底框层整个去掉，不会越叠越多
        let again = restyle_ass(&out, &SubLook { boxed: false, ..look.clone() });
        assert!(!again.contains("Box"), "{again}");
        assert_eq!(parse_ass(&out).len(), 1);
        assert!(out.contains(r"{\c&H0000FF&\b1}你好\N{\c&HFF0000&\b0\fs10}Hello"), "{out}");
        let plain = SubLook { colorize: false, boxed: false, shadow: false, ..look.clone() };
        let out = restyle_ass(src, &plain);
        assert!(out.contains(",1,1.5,0.0,2,"), "{out}");
        assert!(out.contains(r",,你好\N{\fs10}Hello"), "{out}");
        // 中英字号分开、换字体、阴影加大
        let custom = SubLook { scale: 1.25, scale_en: 1.0, shadow_size: 2.5, font: "SimHei".into(), colorize: false, boxed: false, ..look };
        let out = restyle_ass(src, &custom);
        assert!(out.contains("Style: Default,SimHei,15,") && out.contains(",1,1.5,2.5,2,"), "{out}");
        assert!(out.contains(r"你好\N{\fs12}Hello"), "{out}");
    }
}

#[cfg(test)]
mod font_tests {
    #[test]
    fn lists_installed_fonts() {
        let fonts = super::system_fonts();
        assert!(fonts.len() > 10, "{fonts:?}");
        assert!(!fonts.iter().any(|f| f.starts_with('@')));
    }
}

#[derive(Clone)]
struct CloudRefreshIntent { id:String, kind:String, fingerprint:String, files:Vec<serde_json::Value>, target:CloudRefreshTarget, at:std::time::Instant }
#[derive(Clone)]
enum CloudRefreshTarget { Engine { api: Api, raw_id: String }, Baidu }

fn refresh_intents()->&'static Mutex<std::collections::HashMap<String,CloudRefreshIntent>> {
 static P:OnceLock<Mutex<std::collections::HashMap<String,CloudRefreshIntent>>>=OnceLock::new();P.get_or_init(Default::default)
}
#[tauri::command]
pub async fn baidu_http_renew(id:String, state:State<'_,AppState>) -> Result<Option<String>,String> {
    if id.contains(':') { return Err("请选择本机的 HTTP 下载任务".into()); }
    let (port, token) = endpoint(&state);
    crate::cloud_resume::refresh_baidu_http(&Api::new(port, token), &id).await
}

#[tauri::command]
pub async fn multi_refresh_prepare(id:String,state:State<'_,AppState>)->Result<String,String>{
 let (fingerprint, kind, files, target) = if let Some(baidu_id) = id.strip_prefix("bd:") {
     let file = state.baidu.prepare_web_resume(baidu_id).await?;
     (String::new(), "百度网盘".to_owned(), vec![file], CloudRefreshTarget::Baidu)
 } else {
     let fingerprint = crate::cloud::refresh_target_fingerprint(&state, &id)?;
     let (api, raw_id) = multi_target(&state, &id)?;
     let (kind, files) = crate::cloud_resume::prepare(&api, &raw_id).await?;
     (fingerprint, kind, files, CloudRefreshTarget::Engine { api, raw_id })
 };
 let token=format!("{:016x}{:016x}",rand::random::<u64>(),rand::random::<u64>());
 let mut p=refresh_intents().lock().map_err(|_|"状态不可用")?;
 p.retain(|_,v|v.at.elapsed().as_secs()<600 && v.id!=id);
 p.insert(token.clone(),CloudRefreshIntent{id,kind,fingerprint,files,target,at:std::time::Instant::now()});Ok(token)
}

pub fn cloud_refresh_info(token:&str)->Option<serde_json::Value>{
 let p=refresh_intents().lock().ok()?;let v=p.get(token).filter(|v|v.at.elapsed().as_secs()<600)?;
 Some(serde_json::json!({"kind":v.kind,"files":v.files,"baiduResume":matches!(v.target,CloudRefreshTarget::Baidu)}))
}
pub(crate) fn cloud_refresh_identity(token: &str) -> Option<(String, String, String)> {
    let intents = refresh_intents().lock().ok()?;
    let intent = intents.get(token).filter(|v| v.at.elapsed().as_secs() < 600)?;
    Some((intent.kind.clone(), intent.id.clone(), intent.fingerprint.clone()))
}
#[derive(serde::Deserialize)]
pub(crate) struct CloudRefreshFile {
    #[serde(flatten)]
    file: crate::api::MultiFile,
    md5: Option<String>,
}

pub(crate) async fn cloud_refresh_apply(token:String,files:Vec<CloudRefreshFile>,state:State<'_,AppState>)->Result<(),String>{
 use tauri::Emitter;
 let job=refresh_intents().lock().map_err(|_|"状态不可用")?.get(&token).filter(|v|v.at.elapsed().as_secs()<600).cloned().ok_or("换链请求已过期，请在原任务重新点击继续")?;
 let names:std::collections::HashSet<&str>=files.iter().map(|f|f.file.name.as_str()).collect();
 if names.len()!=job.files.len() || files.len()!=job.files.len() || !job.files.iter().all(|f|f["name"].as_str().is_some_and(|n|names.contains(n))) {return Err("未完整匹配原任务文件，进度未改动".into())}
 match job.target {
     CloudRefreshTarget::Engine { api, raw_id } => {
         let files: Vec<_> = files.iter().map(|f| f.file.clone()).collect();
         crate::cloud_resume::refresh(&api, &raw_id, &files).await?;
     }
     CloudRefreshTarget::Baidu => {
         let id = job.id.strip_prefix("bd:").ok_or("找不到原百度下载任务")?;
         state.baidu.resume_download_url(id, &files[0].file, files[0].md5.as_deref(), job.files[0]["md5"].as_str())?;
         refresh_intents().lock().map_err(|_|"状态不可用")?.remove(&token);
         if let Some(app) = APP.get() {
             let _ = app.emit("cloud-resume-started", serde_json::json!({"id": job.id}));
         }
         return Ok(());
     }
 }

 let page=match job.kind.as_str(){"阿里云盘"=>"https://www.alipan.com/drive/file/all","夸克网盘"=>"https://pan.quark.cn/list","UC 网盘"=>"https://drive.uc.cn/","123 云盘"=>"https://yun.123pan.cn/","天翼云盘"=>"https://cloud.189.cn/web/main/","移动云盘"=>"https://yun.139.com/w/","迅雷云盘"=>"https://pan.xunlei.com/","光鸭云盘"=>"https://www.guangyapan.com/",_=>"https://pan.baidu.com/disk/main"};
 for file in files {crate::sources::remember(&file.file.url,page);}
 refresh_intents().lock().map_err(|_|"状态不可用")?.remove(&token);
 if let Some(app)=APP.get(){let _=app.emit("cloud-refreshed",serde_json::json!({"id":job.id}));}
 Ok(())
}

pub fn cloud_refresh_notice(token:&str,message:&str){
 use tauri::Emitter;
 let job=refresh_intents().lock().ok().and_then(|p|p.get(token).filter(|v|v.at.elapsed().as_secs()<600).cloned());
 if let (Some(v),Some(app))=(job,APP.get()){let _=app.emit("cloud-refresh-status",serde_json::json!({"id":v.id,"message":message.chars().take(200).collect::<String>()}));}
}
