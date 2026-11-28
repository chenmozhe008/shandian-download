// 偏好设置 + 目录策略。移植自 Mac 版 SettingsView.swift 的 Prefs 与 Links.swift 的 Folders。
//
// 目录策略（三类各有一个位置，可在设置里改）：
//   磁力 / 种子 / ed2k → 电影
//   视频网站           → 视频
//   直链               → 其他
// Mac 上「电影」默认指向移动硬盘（片子太大），Windows 没这个约定，默认落在「下载\电影」，
// 想存到 D 盘就在设置里改。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::Api;
use crate::links::LinkKind;

/// 默认位置。Windows 的「下载」是 %USERPROFILE%\Downloads；
/// 万一被系统重定向到别处（OneDrive 常见），目录不存在就退到数据目录下的 downloads。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Folders {
    pub downloads: PathBuf,
    pub movie: PathBuf,
    pub video: PathBuf,
    pub other: PathBuf,
}

impl Folders {
    pub fn detect(app_data: &Path) -> Self {
        let downloads = std::env::var("USERPROFILE")
            .ok()
            .map(|u| PathBuf::from(u).join("Downloads"))
            .filter(|p| p.exists())
            .unwrap_or_else(|| app_data.join("downloads"));
        Self {
            // Mac 的兜底是「下载 › 电影」，这里保持一致
            movie: downloads.join("电影"),
            video: downloads.clone(),
            other: downloads.clone(),
            downloads,
        }
    }

    /// 界面上显示的位置名：下载 ／ 下载 › 电影 ／ 完整路径
    pub fn display(&self, p: &Path) -> String {
        if p == self.downloads {
            return "下载".into();
        }
        if p.parent() == Some(self.downloads.as_path()) {
            if let Some(name) = p.file_name() {
                return format!("下载 › {}", name.to_string_lossy());
            }
        }
        p.display().to_string()
    }
}

/// 规整用户给的路径：正斜杠换反斜杠、合并重复分隔符、去掉结尾分隔符（盘根 C:\ 除外）。
/// Windows 本身能认 C:/a//b 这种写法，但存下来要干净，否则界面上会显示成 C:////a////b。
pub fn normalize_dir(s: &str) -> String {
    let t = s.trim().replace('/', "\\");
    let mut out = String::with_capacity(t.len());
    let mut prev_sep = false;
    for c in t.chars() {
        if c == '\\' {
            if !prev_sep {
                out.push(c);
            }
            prev_sep = true;
        } else {
            out.push(c);
            prev_sep = false;
        }
    }
    if out.len() > 3 {
        out = out.trim_end_matches('\\').to_string();
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// 三类位置。空字符串 = 用默认（<downloads>\电影 等）
    pub dir_movie: String,
    pub dir_video: String,
    pub dir_other: String,
    /// 同时下载数，Mac 版默认 5
    pub max_running: u32,
    /// 视频网站同时下载数，默认 5。
    pub max_videos: u32,
    /// 做种分享率，到了就停，0 = 不看分享率
    pub seed_ratio: f64,
    /// 做种时长（秒），到了就停，0 = 不看时长。两项都是 0 就一直做种
    pub seed_time: i64,
    /// 下载限速，KB/s，0 = 不限（对应内核 extra.shandianDownloadLimit）
    pub download_limit: u32,
    pub notify_on_finish: bool,
    pub prevent_sleep: bool,
    pub watch_clipboard: bool,
    pub button_hover_hints: bool,
    pub confirm_new: bool,
    /// 外观：system / light / dark。Mac 版只跟系统，Windows 版给个手动选项，
    /// 同事装来测试时不用去改系统主题（也不影响深浅两套配色本身的正确性）。
    pub theme: String,
    /// 最近用过的下载目录（新建下载的「保存到」下拉里列出来），最新的在前
    #[serde(default)]
    pub recent_dirs: Vec<String>,
    /// 下完自动用本机 Ollama 润色中文字幕
    #[serde(default)]
    pub subtitle_polish: bool,
    /// 润色用的模型，空 = 本机最大的那个
    #[serde(default)]
    pub polish_model: String,
    /// polish = 参考英文润色机翻中文；translate = 不要机翻中文，直接从英文翻译。
    /// 其他语言的视频总是从原文翻译
    #[serde(default)]
    pub polish_mode: String,
    /// 新建下载上次选的目录，按链接类型记（磁力和直链常存不同地方）。下次默认用它
    #[serde(default)]
    pub last_dirs: std::collections::HashMap<String, String>,
    /// 磁力下不动时的接力：off / cloud / thunder / cloud_thunder（先云盘，不行再迅雷）。默认 off
    #[serde(default)]
    pub relay: String,
    /// GitHub 和 Hugging Face 公开文件经公共加速站下载（测速选最快、出错换站）。默认开；沿用 githubMirror 配置键
    pub github_mirror: bool,
    /// 上次真正接管成功的视频号扩展浏览器与配置；启动后仍需验证安装和启用状态。
    pub wechat_browser: Option<(String, String)>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dir_movie: String::new(),
            dir_video: String::new(),
            dir_other: String::new(),
            max_running: 5,
            max_videos: 5,
            seed_ratio: 1.0,
            seed_time: 7200,
            download_limit: 0,
            notify_on_finish: true,
            prevent_sleep: true,
            watch_clipboard: true,
            button_hover_hints: true,
            confirm_new: true,
            theme: "system".into(),
            recent_dirs: Vec::new(),
            subtitle_polish: false,
            polish_model: String::new(),
            polish_mode: String::new(),
            relay: "off".into(),
            last_dirs: Default::default(),
            github_mirror: true,
            wechat_browser: None,
        }
    }
}

impl Settings {
    pub fn load(app_data: &Path) -> Self {
        let p = app_data.join("settings.json");
        let mut s: Self = std::fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // 手改过 json 或旧版本留下的怪值时退回「跟随系统」，别让界面卡在一个没定义的主题上
        if !matches!(s.theme.as_str(), "system" | "light" | "dark") {
            s.theme = "system".into();
        }
        s.max_videos = s.max_videos.clamp(1, 10);
        s
    }

    /// 记一个用过的目录：去重、最新在前、最多 6 条。
    /// 存进设置文件，下次「保存到」的下拉里就有得选
    pub fn remember_dir(&mut self, dir: &Path) {
        let d = dir.to_string_lossy().trim().to_string();
        if d.is_empty() {
            return;
        }
        self.recent_dirs.retain(|x| !x.eq_ignore_ascii_case(&d));
        self.recent_dirs.insert(0, d);
        self.recent_dirs.truncate(6);
    }

    pub fn save(&self, app_data: &Path) -> Result<(), String> {
        std::fs::create_dir_all(app_data).map_err(|e| format!("建目录失败：{e}"))?;
        // 存之前把路径写法规整掉，别把 C:/a//b 这种原样留在文件里
        let mut clean = self.clone();
        clean.dir_movie = normalize_dir(&self.dir_movie);
        clean.dir_video = normalize_dir(&self.dir_video);
        clean.dir_other = normalize_dir(&self.dir_other);
        let s = serde_json::to_string_pretty(&clean).map_err(|e| e.to_string())?;
        std::fs::write(app_data.join("settings.json"), s).map_err(|e| format!("写设置失败：{e}"))
    }

    /// 新建下载默认存到哪：上次给这类链接选的目录；没选过或那个目录已不在（移动硬盘拔了）就按设置
    pub fn default_dir(&self, kind: LinkKind, f: &Folders) -> PathBuf {
        match self.last_dirs.get(kind.label()).map(PathBuf::from) {
            Some(p) if p.exists() => p,
            _ => self.dir_for(kind, f),
        }
    }

    /// 这个链接该存到哪。传进来的 dir_* 是空的就用默认。
    pub fn dir_for(&self, kind: LinkKind, f: &Folders) -> PathBuf {
        let pick = |custom: &str, fallback: &Path| -> PathBuf {
            let c = normalize_dir(custom);
            if c.is_empty() {
                fallback.to_path_buf()
            } else {
                PathBuf::from(c)
            }
        };
        match kind {
            LinkKind::Magnet | LinkKind::TorrentFile | LinkKind::Ed2k => {
                pick(&self.dir_movie, &f.movie)
            }
            LinkKind::VideoSite => pick(&self.dir_video, &f.video),
            LinkKind::Http => pick(&self.dir_other, &f.other),
        }
    }

    /// 把设置写进内核配置。
    ///
    /// 内核的 PUT /config 要整份配置，所以先 GET 再改再 PUT（Mac 版同样做法）。
    /// 几个值是有来由的，别随手改：
    ///   - userAgent 不冒充浏览器：Go 的 TLS 握手配 Chrome UA 会被识别成伪装爬虫而 403，
    ///     如实写下载工具名才 200（Mac 版 10-27 在中科大实测，见 Engine.swift:317）
    ///   - bt.listenPort 固定 6881 而不是随机端口，端口映射和防火墙规则才有意义
    ///   - http.connections = 8：内核默认 16，对国内大多数源反而容易被限
    pub async fn apply(&self, api: &Api, f: &Folders) -> Result<(), String> {
        let mut cfg = api.config().await.map_err(|e| e.to_string())?;

        cfg["downloadDir"] = serde_json::json!(f.downloads.to_string_lossy());
        cfg["maxRunning"] = serde_json::json!(self.max_running.max(1));
        // 补丁读的是 KB/s，0 表示不限
        cfg["extra"]["shandianDownloadLimit"] = serde_json::json!(self.download_limit);

        if let Some(http) = cfg["protocolConfig"]["http"].as_object_mut() {
            http.insert("connections".into(), serde_json::json!(8));
            http.insert("userAgent".into(), serde_json::json!(user_agent()));
        }
        if let Some(bt) = cfg["protocolConfig"]["bt"].as_object_mut() {
            bt.insert("listenPort".into(), serde_json::json!(6881));
            // 没有 tracker 时磁力只能靠 DHT 冷启动，常常几分钟拿不到元数据（实测 200 秒超时，配上后 9.5 秒）
            bt.insert("trackers".into(), serde_json::json!(tracker_list().await));
            bt.insert("seedKeep".into(), serde_json::json!(self.seed_ratio <= 0.0 && self.seed_time <= 0));
            bt.insert("seedRatio".into(), serde_json::json!(self.seed_ratio.max(0.0)));
            bt.insert("seedTime".into(), serde_json::json!(self.seed_time.max(0)));
        }

        api.put_config(&cfg).await.map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// 拉不到列表时也有这一份可用
const TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://tracker.dler.org:6969/announce",
    "udp://explodie.org:6969/announce",
    "https://tracker.tamersunion.org:443/announce",
    "udp://tracker.qu.ax:6969/announce",
    "udp://opentracker.io:6969/announce",
];

/// 内置 + 在线合并（和 Mac 版一样用 trackerslist、ngosang）。每次运行只拉一次，设置保存时不用再等网络
async fn tracker_list() -> Vec<String> {
    static CACHE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    {
        let c = CACHE.lock().unwrap();
        if !c.is_empty() {
            return c.clone();
        }
    }
    let mut list: Vec<String> = TRACKERS.iter().map(|s| s.to_string()).collect();
    for url in ["https://cf.trackerslist.com/best.txt", "https://raw.githubusercontent.com/ngosang/trackerslist/master/trackers_best.txt"] {
        let mut b = reqwest::Client::builder().timeout(std::time::Duration::from_secs(6));
        if let Some(px) = crate::proxy::url_for(url).and_then(|p| reqwest::Proxy::all(&p).ok()) {
            b = b.proxy(px);
        }
        let Ok(client) = b.build() else { continue };
        let Ok(resp) = client.get(url).send().await else { continue };
        let Ok(text) = resp.text().await else { continue };
        for line in text.lines().map(str::trim) {
            if (line.starts_with("udp://") || line.starts_with("http")) && !list.iter().any(|x| x == line) {
                list.push(line.to_string());
            }
        }
    }
    *CACHE.lock().unwrap() = list.clone();
    list
}

pub fn user_agent() -> String {
    format!("ShandianDownload/{} (Windows)", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_download_default_and_saved_limit() {
        assert_eq!(serde_json::from_str::<Settings>("{}").unwrap().max_videos, 5);
        assert_eq!(serde_json::from_str::<Settings>(r#"{"maxVideos":3}"#).unwrap().max_videos, 3);
    }

    #[test]
    fn normalize_paths() {
        assert_eq!(normalize_dir(" C:/a//b/ "), r"C:\a\b");
        assert_eq!(normalize_dir(r"C:\a\b\"), r"C:\a\b");
        assert_eq!(normalize_dir(r"C:\"), r"C:\"); // 盘根不能把结尾分隔符吃掉
        assert_eq!(normalize_dir(""), "");
        assert_eq!(normalize_dir("   "), "");
    }

    #[test]
    fn dir_by_kind() {
        let f = Folders::detect(Path::new(&std::env::temp_dir()));
        let s = Settings::default();
        // 默认：磁力、种子、ed2k 进「电影」，视频网站和直链进下载
        assert!(s.dir_for(LinkKind::Magnet, &f).ends_with("电影"));
        assert!(s.dir_for(LinkKind::TorrentFile, &f).ends_with("电影"));
        assert!(s.dir_for(LinkKind::Ed2k, &f).ends_with("电影"));
        assert_eq!(s.dir_for(LinkKind::VideoSite, &f), f.downloads);
        assert_eq!(s.dir_for(LinkKind::Http, &f), f.downloads);

        // 自定义位置生效，且正斜杠会被规整
        let mut c = s.clone();
        c.dir_movie = "D:/movies/".into();
        c.dir_video = r"E:\v".into();
        assert_eq!(c.dir_for(LinkKind::Magnet, &f), PathBuf::from(r"D:\movies"));
        assert_eq!(c.dir_for(LinkKind::VideoSite, &f), PathBuf::from(r"E:\v"));
    }

    #[test]
    fn display_names() {
        let f = Folders::detect(Path::new(&std::env::temp_dir()));
        assert_eq!(f.display(&f.downloads), "下载");
        let movie = f.downloads.join("电影");
        assert_eq!(f.display(&movie), "下载 › 电影");
        assert_eq!(f.display(Path::new(r"D:\movies")), r"D:\movies");
    }
}

// Bind encrypted local secrets to the current Windows user.
pub(crate) fn protect_local_secret(mut input: Vec<u8>, encrypt: bool) -> Option<Vec<u8>> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };

    let data = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(input.len()).ok()?,
        pbData: input.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    let result = unsafe {
        if encrypt {
            CryptProtectData(
                &data,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &data,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if output.pbData.is_null() {
        return None;
    }
    let value = (result != 0).then(|| unsafe {
        std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
    });
    unsafe {
        LocalFree(output.pbData.cast());
    }
    value
}
