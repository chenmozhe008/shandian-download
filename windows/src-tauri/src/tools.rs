// 工具组件：yt-dlp.exe + ffmpeg.exe。
//
// 视频网站（B 站、YouTube、小红书…）必须靠 yt-dlp 解析，分轨的视频还要 ffmpeg 合并，
// 所以这两个 exe 是「视频网站」能用起来的前提。Mac 版把 yt-dlp 装在
// Application Support/…/bin 下，启动时缺了就下、每周查一次新版；这里照同一套逻辑：
//   - 放在 %APPDATA%\com.shandian.windows\bin
//   - 找不到就下载，装好后启动时静默检查更新（超过 7 天看一次版本）
//   - 下载地址优先 GitHub 直连，失败依次走国内镜像（这台机器实测直连 GitHub 常超时）
//
// 为什么单独写下载器而不复用内核：内核是 Go 写的子进程，它只认下载任务，不负责装工具。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tauri::{Emitter, Manager, State};

use crate::AppState;

/// yt-dlp 官方单文件版。目录版在 Windows 上没必要（单文件启动本来就快），
/// 而单文件版可以直接 -U 自更新，所以这里选 yt-dlp.exe
const YTDLP_NAME: &str = "yt-dlp.exe";
const YTDLP_URLS: &[&str] = &[
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe",
    "https://ghfast.top/https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe",
    "https://gh-proxy.com/https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe",
];

const FFMPEG_NAME: &str = "ffmpeg.exe";
/// gyan.dev 的 essentials 包（约 35 MB zip，解压后只取 ffmpeg.exe）。
/// 备选是 BtbN 的 GitHub 构建，体积大一倍但更及时
const FFMPEG_URLS: &[&str] = &[
    "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip",
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip",
    "https://ghfast.top/https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip",
];

const WEEK: u64 = 7 * 86400;

pub struct Tools {
    pub dir: PathBuf,
}

/// 同一时刻只允许装一个工具（用户点两下「安装」，或启动补齐与手动安装撞车）。
/// 放全局 static 而不是放 Tools 里：命令层会临时构造 Tools，字段会丢
static INSTALLING: Mutex<Option<String>> = Mutex::new(None);

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolState {
    pub key: String,
    pub title: String,
    pub path: String,
    pub exists: bool,
    /// 自带的 / 系统里已有的 / 没有
    pub source: String,
    pub version: String,
    pub size: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ToolProgress {
    pub key: String,
    /// download / extract / done / error
    pub stage: String,
    pub received: u64,
    pub total: u64,
    pub message: String,
}

impl Tools {
    pub fn new(dir: &Path) -> Self {
        let dir = dir.to_path_buf();
        let _ = std::fs::create_dir_all(&dir);
        // 安装包自带一份 yt-dlp，第一次运行拷到工具目录：装好马上能下视频，
        // 也不用往 Program Files 里写就能更新。拷贝保留修改时间，太旧的话一周检查会照常更新
        let target = dir.join(YTDLP_NAME);
        if !target.exists() {
            if let Some(src) = std::env::current_exe().ok().and_then(|p| Some(p.parent()?.join(YTDLP_NAME))) {
                let _ = std::fs::copy(src, target);
            }
        }
        Self { dir }
    }

    fn path_of(&self, key: &str) -> PathBuf {
        self.dir.join(if key == "ffmpeg" { FFMPEG_NAME } else { YTDLP_NAME })
    }

    /// 先看自带的，再看系统 PATH 里有没有（有人已经装过）
    pub fn locate(&self, key: &str) -> (PathBuf, &'static str) {
        let bundled = self.path_of(key);
        if bundled.exists() {
            return (bundled, "bundled");
        }
        if let Some(p) = which(if key == "ffmpeg" { "ffmpeg.exe" } else { "yt-dlp.exe" }) {
            return (p, "system");
        }
        (bundled, "")
    }

    pub fn state(&self, key: &str) -> ToolState {
        let (path, source) = self.locate(key);
        let exists = path.exists();
        ToolState {
            key: key.to_string(),
            title: if key == "ffmpeg" { "ffmpeg" } else { "yt-dlp" }.to_string(),
            path: path.display().to_string(),
            exists,
            source: source.to_string(),
            version: if exists { version_of(&path, key) } else { String::new() },
            size: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        }
    }

    /// 需要时下载。force = true 时不管有没有都重下（「更新」按钮用）
    pub fn install(&self, app: &tauri::AppHandle, key: &str, force: bool) -> Result<ToolState, String> {
        {
            let mut busy = INSTALLING.lock().unwrap_or_else(|e| e.into_inner());
            if busy.as_deref() == Some(key) {
                return Err("正在下载，稍等".into());
            }
            *busy = Some(key.to_string());
        }
        let result = self.install_inner(app, key, force);
        *INSTALLING.lock().unwrap_or_else(|e| e.into_inner()) = None;
        result
    }

    fn install_inner(&self, app: &tauri::AppHandle, key: &str, force: bool) -> Result<ToolState, String> {
        let current = self.state(key);
        if current.exists && current.source == "system" && !force {
            // 系统里已经有一份可用的，不再下（避免和用户自己装的打架）
            return Ok(current);
        }
        if current.exists && current.source == "bundled" && !force {
            return Ok(current);
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("建目录失败：{e}"))?;

        let urls: Vec<&str> = if key == "ffmpeg" {
            FFMPEG_URLS.to_vec()
        } else {
            YTDLP_URLS.to_vec()
        };
        let mut last_err = String::new();
        for url in urls {
            match self.fetch(app, key, url) {
                Ok(()) => {
                    let st = self.state(key);
                    emit(app, ToolProgress {
                        key: key.into(),
                        stage: "done".into(),
                        received: st.size,
                        total: st.size,
                        message: format!("{} 已就绪", st.title),
                    });
                    return Ok(st);
                }
                Err(e) => {
                    last_err = e.clone();
                    // 换下一个源之前告诉界面一句，用户能看到在换镜像
                    emit(app, ToolProgress {
                        key: key.into(),
                        stage: "download".into(),
                        received: 0,
                        total: 0,
                        message: format!("这个源不行（{e}），换下一个"),
                    });
                }
            }
        }
        emit(app, ToolProgress {
            key: key.into(),
            stage: "error".into(),
            received: 0,
            total: 0,
            message: last_err.clone(),
        });
        Err(format!("下载失败：{last_err}"))
    }

    /// 从一个地址下载并装好（yt-dlp 直接落盘，ffmpeg 解 zip 取 exe）
    fn fetch(&self, app: &tauri::AppHandle, key: &str, url: &str) -> Result<(), String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(180))
            .connect_timeout(Duration::from_secs(15))
            .user_agent(crate::settings::user_agent())
            .build()
            .map_err(|e| e.to_string())?;

        let resp = client.get(url).send().map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status().as_u16()));
        }
        let total = resp.content_length().unwrap_or(0);
        let tmp = self.dir.join(format!(".{key}.part"));
        let mut file = std::fs::File::create(&tmp).map_err(|e| format!("写临时文件失败：{e}"))?;
        let mut got: u64 = 0;
        let mut resp = resp;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = resp.read(&mut buf).map_err(|e| format!("下载中断：{e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| format!("写盘失败：{e}"))?;
            got += n as u64;
            // 每 256 KB 报一次，界面进度条才跟得上
            if got % (256 * 1024) < 64 * 1024 {
                emit(app, ToolProgress {
                    key: key.into(),
                    stage: "download".into(),
                    received: got,
                    total,
                    message: "正在下载".into(),
                });
            }
        }
        file.flush().ok();
        drop(file);
        if total > 0 && got != total {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("只下到 {got}/{total} 字节"));
        }

        if key == "ffmpeg" {
            emit(app, ToolProgress {
                key: key.into(),
                stage: "extract".into(),
                received: got,
                total: got,
                message: "正在解压".into(),
            });
            let out = self.dir.join(FFMPEG_NAME);
            let ok = extract_member(&tmp, "ffmpeg.exe", &out);
            let _ = std::fs::remove_file(&tmp);
            ok?
        } else {
            let out = self.dir.join(YTDLP_NAME);
            let _ = std::fs::remove_file(&out);
            std::fs::rename(&tmp, &out).map_err(|e| busy_hint(YTDLP_NAME, &e))?;
        }
        Ok(())
    }

    /// 启动时补齐：没有就下；yt-dlp 超过一周就看一眼有没有新版（ffmpeg 只有缺了才下，几十 MB 不值当每周重下）
    pub fn ensure_async(&self, app: tauri::AppHandle) {
        for key in ["yt-dlp", "ffmpeg"] {
            let key = key.to_string();
            let app2 = app.clone();
            let dir = self.dir.clone();
            std::thread::spawn(move || {
                let tools = Tools { dir: dir.clone() };
                let st = tools.state(&key);
                let stale = if st.exists {
                    std::fs::metadata(&st.path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| SystemTime::now().duration_since(t).ok())
                        .map(|d| d.as_secs() > WEEK)
                        .unwrap_or(true)
                } else {
                    true
                };
                if !stale {
                    return;
                }
                // 首次安装（不是每周更新）先跟界面说一声：设置页那两行能看到进度，
                // 但用户这会儿多半不在设置页，不提示会以为程序没反应
                if !st.exists {
                    emit(&app2, ToolProgress {
                        key: key.clone(),
                        stage: "start".into(),
                        received: 0,
                        total: 0,
                        message: format!(
                            "首次使用视频网站下载，正在后台准备 {}（约 {} MB）",
                            if key == "ffmpeg" { "ffmpeg" } else { "yt-dlp" },
                            if key == "ffmpeg" { 35 } else { 23 }
                        ),
                    });
                }
                if key == "ffmpeg" {
                    // ffmpeg 只有缺失才装
                    if !st.exists {
                        let _ = tools.install(&app2, &key, false);
                    }
                    return;
                }
                if st.exists && st.source == "bundled" && !update_available(&st) {
                    return;
                }
                let _ = tools.install(&app2, &key, false);
            });
        }
    }
}

fn emit(app: &tauri::AppHandle, p: ToolProgress) {
    let _ = app.emit("tool-progress", p);
}

/// 版本号：yt-dlp 打版本号，ffmpeg 打一大段，只取第一行的版本字段
fn version_of(path: &Path, key: &str) -> String {
    use std::process::Command;
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(path);
    cmd.arg(if key == "ffmpeg" { "-version" } else { "--version" });
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    // File-backed output avoids a full stdout pipe blocking the child before it exits.
    let capture_path = std::env::temp_dir().join(format!("shandian-tool-version-{:032x}.tmp", rand::random::<u128>()));
    let Ok(capture) = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&capture_path) else { return String::new() };
    let output = (|| -> Option<Vec<u8>> {
        cmd.stdout(capture.try_clone().ok()?).stderr(std::process::Stdio::null());
        let mut child = cmd.spawn().ok()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
                _ => { let _ = child.kill(); let _ = child.wait(); return None; }
            }
        }
        std::fs::read(&capture_path).ok()
    })();
    drop(cmd);
    drop(capture);
    let _ = std::fs::remove_file(capture_path);
    let output = output.unwrap_or_default();
    let text = String::from_utf8_lossy(&output);
    let first = text.lines().next().unwrap_or("").trim().to_string();
    if key == "ffmpeg" {
        // ffmpeg version 7.1-full_build-www.gyan.dev Copyright (c) ...
        first
            .split_whitespace()
            .nth(2)
            .unwrap_or(&first)
            .to_string()
    } else {
        first
    }
}

/// 只在 PATH 里找（不自建搜索路径：Windows 上 which 就是查 PATH + PATHEXT）
fn which(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// 覆盖正在运行或被占用的 exe 时，Windows 报的是 os error 32（共享冲突）或 5（拒绝访问）。
/// 系统那句「另一个程序正在使用此文件」甩给用户没意义，这里说清楚是什么被占了。
fn busy_hint(name: &str, e: &std::io::Error) -> String {
    let code = e.raw_os_error().unwrap_or(0);
    if code == 32 || code == 33 || e.kind() == std::io::ErrorKind::PermissionDenied {
        format!("{name} 正在被占用（多半是还在下视频），等这次下载结束后再试一次")
    } else {
        format!("写入 {name} 失败：{e}")
    }
}

/// 从 zip 里取出名字以 suffix 结尾的第一个文件，写到 out
fn extract_member(zip_path: &Path, suffix: &str, out: &Path) -> Result<(), String> {
    let label = out
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "文件".into());
    let file = std::fs::File::open(zip_path).map_err(|e| format!("打不开安装包：{e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("安装包损坏：{e}"))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();
        if entry.is_dir() || !name.to_lowercase().ends_with(suffix) {
            continue;
        }
        let mut f = std::fs::File::create(out).map_err(|e| busy_hint(&label, &e))?;
        std::io::copy(&mut entry, &mut f).map_err(|e| format!("解压失败：{e}"))?;
        return Ok(());
    }
    Err(format!("安装包里没找到 {suffix}"))
}

/// 问一下 GitHub 最新 tag 是不是比本机新（不下载，只看版本号）
fn update_available(st: &ToolState) -> bool {
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
    else {
        return false;
    };
    let Ok(resp) = client
        .get("https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .send()
    else {
        return false;
    };
    let Ok(v) = resp.json::<serde_json::Value>() else {
        return false;
    };
    let tag = v.get("tag_name").and_then(|t| t.as_str()).unwrap_or("");
    !tag.is_empty() && tag != st.version
}

// ---------- Tauri 命令 ----------

#[tauri::command]
pub async fn tools_status(state: State<'_, AppState>) -> Result<Vec<ToolState>, String> {
    let tools = Tools { dir: state.tools.dir.clone() };
    tauri::async_runtime::spawn_blocking(move || {
        let at = std::time::Instant::now();
        let result = vec![tools.state("yt-dlp"), tools.state("ffmpeg")];
        crate::app_log::record("tools", &format!("status elapsed_ms={}", at.elapsed().as_millis()));
        result
    })
        .await.map_err(|e| format!("读取工具状态失败：{e}"))
}

/// 下载/更新一个工具。阻塞操作放 spawn_blocking，别卡住 async 运行时
#[tauri::command]
pub async fn install_tool(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    key: String,
    force: Option<bool>,
) -> Result<ToolState, String> {
    let tools = Tools {
        dir: state.tools.dir.clone(),
    };
    let key2 = key.clone();
    let force = force.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || tools.install(&app, &key2, force))
        .await
        .map_err(|e| e.to_string())?
}

/// 打开工具目录（设置页里给人看「装到哪了」）
#[tauri::command]
pub fn open_tools_dir(state: State<'_, AppState>) -> Result<(), String> {
    crate::commands::open_dir(state.tools.dir.to_string_lossy().into())
}

/// 由 main.rs 调：启动时按需补齐，不阻塞窗口显示
pub fn bootstrap(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let dir = state.tools.dir.clone();
    let app = app.clone();
    std::thread::spawn(move || {
        let tools = Tools { dir };
        tools.ensure_async(app);
    });
}
