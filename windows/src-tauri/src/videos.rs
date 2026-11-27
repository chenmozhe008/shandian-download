// 视频网站下载：B 站、YouTube、小红书这些站走 yt-dlp 子进程，不走内核。
//
// 移植自 Mac 版 Sources/Shandian/VideoSite.swift，输出解析规则保持一致：
//   - 视频和音频是分两段下的，前一段下完的字节要累加，进度条才不会往回跳
//   - 速度自己按最近 3 秒实际下到的字节算（yt-dlp 每个新分块都从头估，开头能报出 200 MB/s）
//   - 总大小分四级可信度（见 Progress::display_total）：真实 filesize > 分格式
//     filesize_approx 之和 > progress 的分段外推。外推值（total_bytes_estimate）
//     在 DASH 分段下载早期能猜出真实值的 8 倍，只配当最后参考、绝不 max 锁死；
//     下完再用真实文件大小校正一次。
//
// 对外只暴露「任务」这一种形状：list_tasks 把视频任务和内核任务拼在一起，
// 界面不用关心某条任务到底是内核下的还是 yt-dlp 下的。

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 视频网站下载选项（Mac 版 VideoSite.swift 的 VideoOptions，选项会被记住）：
/// 确认框里可选画质、下载字幕、仅音频；选完存到 video-options.json，下次按它来。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VideoOptions {
    /// 0 = 最高；否则不超过这个分辨率
    pub max_height: u32,
    /// 下载字幕并嵌入视频
    pub subtitles: bool,
    /// 这一次要不要润色字幕。空 = 跟设置走；只对这条任务有效，不记住
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polish: Option<bool>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collection_member: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm_collision: bool,
    /// 只下音频（m4a）
    pub audio_only: bool,
    /// 字幕语言意图：`zh` / `en` / `zh,en` / `all`（确认框四个按钮）。
    /// 下载前会先探测站点实际有哪些代码，每个语系只挑一个精确代码传给 yt-dlp，
    /// 避免 `zh.*` 同时命中简繁、`en.*` 命中 en + en-orig 一次落多个文件。
    /// 兼容老配置里的正则（`zh.*,ai-zh` 等），见 normalize_sub_intent。
    pub sub_langs: String,
    /// 字幕格式：vtt（默认，yt-dlp 原生）/ srt / ass。非 vtt 时走 --convert-subs
    #[serde(default)]
    pub sub_format: String,
    /// 字幕字号倍率：合并双语时按画面高度算字号再乘它（0.8 小 / 1.0 标准 / 1.25 大）。
    /// 只对 ass 有效——srt/vtt 没有样式，字号由播放器决定
    #[serde(default = "one")]
    pub sub_scale: f32,
    /// 双语字幕着色。只有 ASS 支持颜色，srt/vtt 上没有意义
    /// （界面上也只在我们选 ASS 时才给这个开关）
    #[serde(default)]
    pub sub_colorize: bool,
    /// 中文行颜色（#RRGGBB），默认 #00FFAC
    #[serde(default = "color_zh")]
    pub sub_color_zh: String,
    /// 英文行颜色（#RRGGBB），默认 #516EFE
    #[serde(default = "color_en")]
    pub sub_color_en: String,
    /// 字幕阴影（默认开）。只对 ASS 有效
    #[serde(default = "yes")]
    pub sub_shadow: bool,
    /// 字幕黑色半透明底框（默认关）。只对 ASS 有效
    #[serde(default)]
    pub sub_box: bool,
    /// 英文行字号倍率（和中文同一基准），默认 0.86
    #[serde(default = "crate::engine::en_scale")]
    pub sub_scale_en: f32,
    /// 阴影距离，默认 0.8
    #[serde(default = "crate::engine::shadow_size")]
    pub sub_shadow_size: f32,
    /// 字体名
    #[serde(default = "crate::engine::sub_font")]
    pub sub_font: String,
    /// 阴影、黑底的不透明度（0～100）
    #[serde(default = "crate::engine::shadow_alpha")]
    pub sub_shadow_alpha: f32,
    #[serde(default = "crate::engine::box_alpha")]
    pub sub_box_alpha: f32,
    /// 描边粗细
    #[serde(default = "crate::engine::outline_size")]
    pub sub_outline: f32,
    /// 离画面底部的距离（画面高度的百分比）
    #[serde(default = "crate::engine::margin_pct")]
    pub sub_margin: f32,
}

fn yes() -> bool {
    true
}

fn color_zh() -> String {
    "#00FFAC".to_string()
}

fn color_en() -> String {
    "#516EFE".to_string()
}

fn one() -> f32 {
    1.0
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            max_height: 0,
            subtitles: false,
            polish: None,
            collection_member: false,
            confirm_collision: false,
            audio_only: false,
            // 默认只要中文（每个语系一个精确代码）
            sub_langs: "zh".to_string(),
            sub_format: "vtt".to_string(),
            sub_scale: 1.0,
            sub_colorize: false,
            sub_color_zh: color_zh(),
            sub_color_en: color_en(),
            sub_shadow: true,
            sub_box: false,
            sub_scale_en: crate::engine::en_scale(),
            sub_shadow_size: crate::engine::shadow_size(),
            sub_font: crate::engine::sub_font(),
            sub_shadow_alpha: crate::engine::shadow_alpha(),
            sub_box_alpha: crate::engine::box_alpha(),
            sub_outline: crate::engine::outline_size(),
            sub_margin: crate::engine::margin_pct(),
        }
    }
}

impl VideoOptions {
    pub fn look(&self) -> crate::engine::SubLook {
        crate::engine::SubLook {
            scale: self.sub_scale,
            colorize: self.sub_colorize,
            color_zh: self.sub_color_zh.clone(),
            color_en: self.sub_color_en.clone(),
            shadow: self.sub_shadow,
            boxed: self.sub_box,
            scale_en: self.sub_scale_en,
            shadow_size: self.sub_shadow_size,
            font: self.sub_font.clone(),
            shadow_alpha: self.sub_shadow_alpha,
            box_alpha: self.sub_box_alpha,
            outline: self.sub_outline,
            margin: self.sub_margin,
        }
    }

    /// 界面上显示的字幕语言标签（没勾字幕就是 None）
    pub fn subs_label(&self) -> Option<String> {
        if !self.subtitles || self.audio_only {
            return None;
        }
        let label = match normalize_sub_intent(&self.sub_langs) {
            SubIntent::Zh => "中文",
            SubIntent::En => "英文",
            SubIntent::ZhEn => "中英",
            SubIntent::All => "全部",
            SubIntent::Raw(_) => return None,
        };
        Some(format!("{}字幕", label))
    }

    fn format_arguments(&self) -> Vec<String> {
        if self.audio_only {
            return vec!["-f".into(), "ba/b".into()];
        }
        // 先比分辨率，同分辨率优先 H.264/AAC。
        vec![
            "-f".into(),
            "bv*+ba/b".into(),
            "-S".into(),
            format!(
                "{},vcodec:h264,acodec:aac",
                if self.max_height > 0 {
                    format!("res:{}", self.max_height)
                } else {
                    "res".into()
                }
            ),
        ]
    }

    /// 对应的 yt-dlp 参数（和 Mac 版 arguments 一字不差）
    pub fn arguments(&self) -> Vec<String> {
        let mut a = self.format_arguments();
        if self.audio_only {
            a.extend([
                "-x".into(),
                "--audio-format".into(),
                "m4a".into(),
                // 标题 / 作者 / 日期 / 简介写进容器，播放器和文件属性里直接能看到
                "--embed-metadata".into(),
                "--parse-metadata".into(),
                "webpage_url:%(comment)s".into(),
            ]);
            return a;
        }
        a.extend([
            "--merge-output-format".into(),
            "mp4".into(),
            // 把标题、作者（上传者）、日期、简介写进 mp4 的元数据：
            // 播放器信息面板、Windows 文件属性的「详细信息」都能看到。
            // 走 -c copy 重封装，不重编码、不损画质
            "--embed-metadata".into(),
            // 原始网址另外写一份到 comment 字段：简介里不一定有链接，
            // 有这条就能回溯「这文件是从哪下的」
            "--parse-metadata".into(),
            "webpage_url:%(comment)s".into(),
        ]);
        a
    }

    /// 字幕进程的参数。--write-subs 只下人工上传的字幕，YouTube 绝大多数视频只有
    /// 「自动生成字幕」，必须加 --write-auto-subs 才下得到。
    /// 不加 --embed-subs：收尾时我们自己把合并后那一份嵌进去，永远只有一条轨。
    fn sub_arguments(&self, langs: &str) -> Vec<String> {
        let mut a: Vec<String> =
            ["--write-subs", "--write-auto-subs", "--sub-langs", langs].map(String::from).to_vec();
        // 默认 vtt 不转；srt/ass 走 yt-dlp 内置转换
        let fmt = self.sub_format.trim().to_ascii_lowercase();
        if fmt == "srt" || fmt == "ass" {
            a.push("--convert-subs".into());
            a.push(fmt);
        }
        a
    }

    /// 记住的选项。Mac 版存 UserDefaults("videoOptions")，这里存数据目录下一个 json
    pub fn load(base: &Path) -> Self {
        std::fs::read_to_string(base.join("video-options.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, base: &Path) {
        // 润色的勾选只管这一次，别让它变成下次的默认
        let remembered = Self { polish: None, collection_member: false, confirm_collision: false, ..self.clone() };
        if let Ok(s) = serde_json::to_string_pretty(&remembered) {
            let _ = std::fs::write(base.join("video-options.json"), s);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoJob {
    pub id: String,
    pub url: String,
    pub title: String,
    pub site: String,
    pub dir: String,
    pub file_path: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) staged_names: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    staged_file: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    staged_ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    publish_plan: Option<crate::video_files::PublishPlan>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    published_files: Vec<PathBuf>,
    pub downloaded: u64,
    pub total: u64,
    pub speed: u64,
    /// running / done / pause / error
    pub state: String,
    pub error: String,
    /// 解析/下载/合并，界面上状态行用
    pub stage: String,
    /// 同名任务的后缀（""/"(1)"/"(2)"…，老任务可能是 "_1"）：同一个链接下两份不同画质时用来区分文件
    #[serde(default)]
    pub name_suffix: String,
    /// 站点上的视频 id（yt-dlp 的 %(id)s）。这条任务落盘的所有文件名都带「 [id]后缀.」，
    /// 删除时靠它精确认文件；没有就绝不按模糊规则扫目录（曾经误删过整个目录的视频）
    #[serde(default)]
    pub vid: String,
    /// 用户明确说过「再下一份」：撞到同名文件时自动顺延后缀，而不是跳过
    #[serde(default)]
    pub rename_on_collision: bool,
    /// 目录里已有同名文件、yt-dlp 跳过了下载，等用户决定要不要再下一份
    #[serde(default)]
    pub collided: bool,
    /// 有字幕没下到（多半是被限流），视频本身不受影响
    #[serde(default)]
    pub subs_failed: bool,
    #[serde(default)]
    subtitle_retry_at: u64,
    #[serde(default)]
    subtitle_retries: usize,
    /// 勾了字幕，但这个视频没有想要的语言。列表上不能再标「中英字幕」
    #[serde(default)]
    pub no_subs: bool,
    /// 站点封面地址（yt-dlp 的 %(thumbnail)s），缩略图优先用它
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    /// 浏览器扩展交过来的登录信息（Netscape 格式的 cookies 文件），抖音/小红书/新片场要靠它
    #[serde(default)]
    pub cookies: Option<String>,
    /// 这条任务的下载选项（旧记录没有这一项，按默认处理）
    #[serde(default)]
    pub options: VideoOptions,
    /// 分辨率标签（"1080P" 这类，yt-dlp 的 %(height)s 换算来的），没有就不输出
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    pub created: u64,
    /// 下完的时刻（毫秒）。老任务没有，用文件修改时间兜底
    #[serde(default)]
    pub finished_at: u64,
    /// 字幕润色的状态文字，空 = 没润色过
    #[serde(default)]
    pub polish: String,
    /// 有留底的原字幕，可以换回去
    #[serde(default)]
    pub polished: bool,
}

impl VideoJob {
    fn new(url: &str, dir: &Path, options: VideoOptions) -> Self {
        Self {
            id: format!("v{}-{:016x}", now_ms(), rand::random::<u64>()),
            url: url.to_string(),
            title: String::new(),
            site: site_name(url),
            dir: dir.display().to_string(),
            file_path: String::new(),
            staged_names: false,
            staged_file: String::new(),
            staged_ready: false,
            publish_plan: None,
            published_files: Vec::new(),
            downloaded: 0,
            total: 0,
            speed: 0,
            state: "running".into(),
            error: String::new(),
            stage: "preparing".into(),
            name_suffix: String::new(),
            vid: String::new(),
            rename_on_collision: false,
            collided: false,
            subs_failed: false,
            subtitle_retry_at: 0,
            subtitle_retries: 0,
            no_subs: false,
            thumbnail_url: None,
            cookies: None,
            options,
            resolution: None,
            created: now_ms(),
            finished_at: 0,
            polish: String::new(),
            polished: false,
        }
    }
    fn working_dir(&self) -> PathBuf {
        let dir = PathBuf::from(&self.dir);
        if self.staged_names && !self.id.is_empty()
            && self.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            dir.join(format!(".shandian-video-{}", self.id))
        } else { dir }
    }

    fn needs_subtitle_translation(&self) -> bool {
        matches!(normalize_sub_intent(&self.options.sub_langs), SubIntent::Zh | SubIntent::ZhEn)
            && subtitle_file_label(Path::new(&self.file_path)) == Some("英文字幕")
    }

    fn working_suffix(&self) -> &str {
        if self.staged_names { "" } else { &self.name_suffix }
    }

    fn working_file(&self) -> &str {
        if self.staged_names { &self.staged_file } else { &self.file_path }
    }

    fn set_working_file(&mut self, path: String) {
        if self.staged_names { self.staged_file = path; }
        else { self.file_path = path; }
    }

}

pub const WECHAT_SITE: &str = "微信视频号";

pub struct Videos {
    /// 任务列表、选项（用户配置）
    pub base: PathBuf,
    /// 日志、封面、登录信息（这台机器专用，见 migrate.rs）
    pub local: PathBuf,
    jobs: Mutex<Vec<VideoJob>>,
    running: Mutex<HashMap<String, Child>>,
    /// 每次起进程/暂停/删除都换一代。旧进程的读日志线程和等待线程看到代数变了就退出，
    /// 不然「暂停后马上继续」时旧线程会读到新日志、抢走新进程的退出状态
    gens: Mutex<HashMap<String, u64>>,
    /// 字幕进程跑完了没有：视频下完后要等它，才能合并、嵌字幕
    subs_done: Mutex<HashMap<String, Arc<AtomicBool>>>,
    fetching_subtitles: Mutex<std::collections::HashSet<String>>,
    /// 判断「还能不能再起一条」和真正起进程要连成一步，不然同时投的几条都会判成有空位
    launching: Mutex<Option<PathBuf>>,
    /// 用户拖出来的顺序，排队的任务按它接上
    order: Mutex<Vec<String>>,
}

/// 同时跑的 yt-dlp 数量。每条还要开 4 路分片，批量投同一个站点时多了容易被限流

impl Videos {
    pub fn new(base: &Path, local: &Path) -> Self {
        let _ = std::fs::create_dir_all(local.join("cache/process"));
        let v = Self {
            base: base.to_path_buf(),
            local: local.to_path_buf(),
            jobs: Mutex::new(load(base)),
            running: Mutex::new(HashMap::new()),
            gens: Mutex::new(HashMap::new()),
            subs_done: Mutex::new(HashMap::new()),
            fetching_subtitles: Mutex::new(std::collections::HashSet::new()),
            launching: Mutex::new(None),
            order: Mutex::new(Vec::new()),
        };
        // 上次退出时还标着「下载中」的，重启后标记成暂停（进程已经没了）
        {
            let mut jobs = v.jobs.lock().unwrap();
            for j in jobs.iter_mut() {
                if j.state == "running" || j.state == "wait" {
                    j.state = "pause".into();
                    j.speed = 0;
                }
                // 上次润色到一半就退出了：字幕还是原样
                if j.polish.starts_with("润色字幕中") {
                    j.polish.clear();
                }
            }
        }
        v.save();
        v
    }

    pub fn list(&self) -> Vec<VideoJob> {
        self.jobs.lock().unwrap().clone()
    }

    fn save(&self) {
        let _ = self.save_checked();
    }

    fn save_checked(&self) -> std::io::Result<()> {
        use std::io::Write;
        let jobs = self.jobs.lock().unwrap();
        let final_path = self.base.join("videos.json");
        let tmp = final_path.with_extension(format!("{:016x}.tmp", rand::random::<u64>()));
        let result = (|| {
            let data = serde_json::to_vec_pretty(&*jobs)?;
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            file.write_all(&data)?;
            file.sync_all()?;
            drop(file);
            crate::baidu::move_file(&tmp, &final_path, 1)
        })();
        if result.is_err() { let _ = std::fs::remove_file(&tmp); }
        result
    }

    fn next_gen(&self, id: &str) -> u64 {
        let mut g = self.gens.lock().unwrap();
        let v = g.entry(id.to_string()).or_insert(0);
        *v += 1;
        *v
    }

    fn is_current(&self, id: &str, gen: u64) -> bool {
        self.gens.lock().unwrap().get(id).copied() == Some(gen)
    }

    fn patch<F: FnOnce(&mut VideoJob)>(&self, id: &str, f: F) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(j) = jobs.iter_mut().find(|j| j.id == id) {
            f(j);
        }
    }

    /// 起一条 yt-dlp。yt-dlp / ffmpeg 由 tools 模块负责准备
    pub fn start(
        self: &Arc<Self>,
        url: &str,
        dir: &Path,
        ytdlp: &Path,
        bin_dir: &Path,
        options: VideoOptions,
        force_new: bool,
    ) -> Result<String, String> {
        if !ytdlp.exists() {
            return Err("yt-dlp 还没装好，到「设置 › 视频网站工具」里点安装".into());
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("建目录失败：{e}"))?;
        // 同一个链接可以下多份（不同分辨率、补另一版字幕），但必须是用户明确要的：
        // 没说要再下一份时，还在下的同一条不能并行（两条 yt-dlp 会抢同一个临时文件）
        if !force_new {
            let key = crate::links::canonical_url(url);
            let dir_s = dir.display().to_string();
            let active = self.jobs.lock().unwrap().iter().any(|j| {
                j.dir == dir_s
                    && j.state != "done"
                    && j.state != "error"
                    && j.name_suffix.is_empty()
                    && crate::links::canonical_url(&j.url) == key
            });
            if active {
                return Err("这个链接已经在下载列表里了".into());
            }
        }
        let mut job = VideoJob::new(url, dir, options);
        job.staged_names = true;
        job.rename_on_collision = force_new;
        // 浏览器扩展把这几家的登录信息交过来过（cookies-<站点>.txt），有就带上，
        // 否则抖音、小红书只能拿到「请登录」的页面
        let id = job.id.clone();
        // 封面地址能从链接推出来的（YouTube），投任务的同时就去拿，不等 yt-dlp 解析页面
        let early = crate::links::early_thumb_url(url);
        job.thumbnail_url = early.clone();
        {
            let mut jobs = self.jobs.lock().unwrap();
            jobs.insert(0, job);
        }
        if let Some(u) = early {
            let (base, id2) = (self.local.clone(), id.clone());
            std::thread::spawn(move || {
                crate::thumbs::remote(&base, &id2, &u);
            });
        }
        self.launch(&id, bin_dir)?;
        Ok(format!("v:{id}"))
    }

    /// 同名文件撞车后接着往后找：path 是 yt-dlp 说「已存在」的那个文件
    fn suffix_after_collision(&self, path: &Path, current: &str) -> String {
        let base = strip_suffix(path, current).unwrap_or_else(|| path.to_path_buf());
        for n in 1..1000 {
            let suffix = format!("({n})");
            let free = with_suffix(&base, &suffix)
                .is_some_and(|p| crate::engine::resolve_existing(&p).is_none());
            if free {
                return suffix;
            }
        }
        format!("({})", now_ms())
    }

    /// 文件已存在被跳过后，用户选了「再下一份」：同一条任务改个后缀重新跑
    pub fn copy_anyway(self: &Arc<Self>, id: &str, bin_dir: &Path) -> Result<(), String> {
        let job = self.find(id).ok_or("任务不存在")?;
        if job.staged_names {
            if !job.collided { return Err("这条任务没有同名冲突".into()); }
            self.patch(id, |j| {
                j.rename_on_collision = true;
                j.collided = false;
            });
            return self.resume(id, bin_dir);
        }
        let suffix = if job.file_path.is_empty() {
            format!("({})", now_ms())
        } else {
            self.suffix_after_collision(Path::new(&job.file_path), &job.name_suffix)
        };
        self.patch(id, |j| {
            j.name_suffix = suffix;
            j.rename_on_collision = true;
            j.collided = false;
            j.file_path.clear();
            j.error.clear();
            j.state = "running".into();
            j.stage = "preparing".into();
            j.downloaded = 0;
            j.total = 0;
        });
        self.launch(id, bin_dir)
    }

    /// 有空位就起进程，满了就排队，等别的任务让出位置时由 pump 接上
    fn launch(self: &Arc<Self>, id: &str, bin_dir: &Path) -> Result<(), String> {
        let mut guard = self.launching.lock().unwrap();
        *guard = Some(bin_dir.to_path_buf());
        if self.running.lock().unwrap().len() >= self.video_limit() {
            self.patch(id, |j| {
                j.state = "wait".into();
                j.stage = String::new();
                j.speed = 0;
            });
            self.save();
            return Ok(());
        }
        self.spawn(id, bin_dir)
    }

    /// 设置里开了润色：下完自动处理。Ollama 没开就算了，不算失败
    fn auto_polish(self: &Arc<Self>, id: &str, bin_dir: &Path) {
        let s = crate::settings::Settings::load(&self.base);
        let Some(job) = self.find(id) else { return };
        let translate = job.needs_subtitle_translation();
        if (!translate && !job.options.polish.unwrap_or(s.subtitle_polish)) || job.state != "done"
            || !job.options.subtitles || job.no_subs || job.collided || job.polish.starts_with("润色字幕中") {
            return;
        }
        let available = crate::polish::models();
        if available.is_empty() {
            return;
        }
        let model = if available.contains(&s.polish_model) { s.polish_model } else { available[0].clone() };
        self.polish(id, Some(model), bin_dir);
    }

    /// 后台润色字幕，一次只跑一条（模型占满显存）。视频已经下完，失败也不影响它
    pub fn polish(self: &Arc<Self>, id: &str, model: Option<String>, bin_dir: &Path) {
        static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
        let mut claimed = false;
        self.patch(id, |job| {
            if !job.polish.starts_with("润色字幕中") {
                job.polish = "润色字幕中…".into();
                claimed = true;
            }
        });
        if !claimed { return; }
        let (me, id, bin) = (Arc::clone(self), id.to_string(), bin_dir.to_path_buf());
        std::thread::spawn(move || {
            let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
            let Some(job) = me.find(&id) else { return };
            let video = PathBuf::from(&job.file_path);
            let before = if job.staged_names { crate::video_files::sidecars(&video) } else { Vec::new() };
            let result = crate::polish::find_sub(&video)
                .ok_or_else(|| "没找到中文字幕".to_string())
                .and_then(|sub| {
                    let o = &job.options;
                    let opts = crate::polish::Options {
                        model: model.as_deref(),
                        drop_en: matches!(normalize_sub_intent(&o.sub_langs), SubIntent::Zh),
                        title: job.title.clone(),
                        style: crate::engine::subtitle_style(o.sub_scale, o.sub_colorize, &o.sub_color_zh, &o.sub_color_en),
                    };
                    crate::polish::run(&sub, &opts, &|a, b| {
                        me.patch(&id, |j| j.polish = format!("润色字幕中 {}%", a * 100 / b.max(1)));
                    })
                });
            let text = match result {
                Ok(r) => {
                    // 润色重写了字幕：按任务现在的样式再套一遍（黑底那一层、字体等），再嵌回视频
                    if let Some(sub) = crate::polish::find_sub(&video).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ass"))) {
                        let _ = restyle_file(&sub, &job.options.look());
                    }
                    embed_sub(&video, &bin);
                    crate::mark(&format!("字幕润色：{} 行 → {} 句，保留原文 {} 句", r.before, r.after, r.kept));
                    me.patch(&id, |j| j.polished = true);
                    match (r.refined, r.kept) {
                        (false, _) => "字幕已整理".to_string(),
                        (true, 0) => "字幕已润色".to_string(),
                        (true, n) => format!("字幕已润色，{n} 句保留原文"),
                    }
                }
                Err(e) => format!("润色失败：{e}"),
            };
            // 润色要好几分钟，做完在右下角提醒一下（失败只写「润色失败」，原因在任务行上看）
            let heading = if text.starts_with("润色失败") { "润色失败".to_string() } else { text.clone() };
            if !job.options.collection_member {
                crate::float::notify_task(format!("v:{id}"), heading, job.title.clone());
            }
            me.record_new_sidecars(&job, &video, &before);
            me.patch(&id, |j| j.polish = text);
            me.save();
        });
    }

    /// 给已下载的视频重新获取字幕（当初没下到，或后来字幕文件丢了）：
    /// 先问站点现在有哪些字幕，按任务当初选的语言（没选过就按中文）下到视频旁边，
    /// 再走一遍和下载时一样的收尾（合并双语、套样式、嵌进视频）。站点没有字幕返回 Ok(false)
    pub fn fetch_subs(self: &Arc<Self>, id: &str, bin_dir: &Path) -> Result<bool, String> {
        let mut active = self.fetching_subtitles.lock().unwrap();
        if !active.insert(id.to_string()) {
            return Err("正在获取这条视频的字幕".into());
        }
        drop(active);
        self.patch(id, |job| job.stage = "正在获取字幕".into());
        let result = self.fetch_subs_inner(id, bin_dir);
        self.patch(id, |job| {
            job.stage.clear();
            match &result {
                Ok(_) => {
                    job.subtitle_retry_at = 0;
                    job.subtitle_retries = 0;
                }
                Err(error) => {
                    job.no_subs = false;
                    job.subs_failed = true;
                    job.error = format!("字幕获取失败：{error}");
                    if subtitle_rate_limited(error) {
                        queue_subtitle_retry(job);
                    } else {
                        job.subtitle_retry_at = 0;
                    }
                }
            }
        });
        self.fetching_subtitles.lock().unwrap().remove(id);
        self.save();
        if matches!(result, Ok(true)) && self.find(id).is_some_and(|job| job.needs_subtitle_translation()) {
            self.auto_polish(id, bin_dir);
        }
        result
    }

    pub fn retry_limited_subtitles(self: &Arc<Self>, bin_dir: &Path) {
        let active = self.fetching_subtitles.lock().unwrap();
        if !active.is_empty() { return; }
        let due = {
            let mut jobs = self.jobs.lock().unwrap();
            jobs.iter_mut().find(|job| {
                job.state == "done" && job.stage.is_empty() && !job.polish.starts_with("润色字幕中")
                    && job.subtitle_retry_at != 0 && job.subtitle_retry_at <= now_ms()
            }).map(|job| {
                job.subtitle_retry_at = 0;
                job.subtitle_retries += 1;
                job.id.clone()
            })
        };
        drop(active);
        if let Some(id) = due {
            let (me, bin) = (Arc::clone(self), bin_dir.to_path_buf());
            std::thread::spawn(move || { let _ = me.fetch_subs(&id, &bin); });
        }
    }

    fn fetch_subs_inner(&self, id: &str, bin_dir: &Path) -> Result<bool, String> {
        let job = self.find(id).ok_or("任务不存在")?;
        let mut youtube = youtube_subtitle_limiter(&job.url);
        if youtube.as_ref().is_some_and(|cooldown| cooldown.is_some_and(|until| until > Instant::now())) {
            return Err("网站限制了字幕请求，请稍后再试".into());
        }
        let result = self.fetch_subs_from_site(id, bin_dir, job);
        if result.as_ref().is_err_and(|err| subtitle_rate_limited(err)) {
            if let Some(cooldown) = youtube.as_mut() {
                **cooldown = Some(Instant::now() + std::time::Duration::from_secs(120));
            }
            return Err("网站限制了字幕请求，请稍后再试".into());
        }
        result
    }

    fn fetch_subs_from_site(&self, id: &str, bin_dir: &Path, job: VideoJob) -> Result<bool, String> {
        let video = PathBuf::from(&job.file_path);
        if !video.is_file() {
            return Err("视频文件不在了，没法配字幕".into());
        }
        let before = if job.staged_names { crate::video_files::sidecars(&video) } else { Vec::new() };
        let stem = video.file_stem().map(|s| s.to_string_lossy().to_string()).ok_or("视频文件名不对")?;
        let ck = cookie_file(&self.local, &job.url);
        let base = |cmd: &mut Command| {
            if ck.is_file() {
                cmd.arg("--cookies").arg(&ck);
            }
            for a in crate::proxy::ytdlp_args(&job.url) {
                cmd.arg(a);
            }
            cmd.env_remove("NODE_OPTIONS").stdin(Stdio::null());
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW);
        };
        // 1. 站点现在有哪些字幕
        let mut probe = Command::new(ytdlp_path(bin_dir));
        probe.args(["--dump-single-json", "--skip-download", "--no-warnings", "--no-playlist", "--ignore-no-formats-error", "--write-auto-subs"]);
        base(&mut probe);
        let out = probe.arg(&job.url).output().map_err(|e| format!("启动 yt-dlp 失败：{e}"))?;
        if !out.status.success() { return Err(subtitle_request_error(&out.stderr)); }
        let avail = sub_langs_in_info(&out.stdout).ok_or("未获得有效字幕信息，请稍后重试")?;
        let mut opts = job.options.clone();
        opts.subtitles = true;
        if opts.sub_langs.trim().is_empty() {
            opts.sub_langs = "zh".into();
        }
        let langs = pick_sub_langs(&normalize_sub_intent(&opts.sub_langs), &avail);
        if langs.is_empty() {
            self.patch(id, |j| {
                j.no_subs = true;
                j.subs_failed = false;
                j.error = if avail.is_empty() { "这个视频没有字幕" } else { "没有所选语言的字幕" }.into();
            });
            self.save();
            return Ok(false);
        }
        // 2. 下字幕，文件名跟视频对上（视频名.语言.扩展名）
        let mut cmd = Command::new(ytdlp_path(bin_dir));
        cmd.args(["--skip-download", "--ignore-no-formats-error", "--no-progress", "--no-playlist"])
            .args(opts.sub_arguments(&langs))
            .args(["-P", job.dir.as_str(), "-o"])
            .arg(format!("{}.%(ext)s", stem.replace('%', "%%")));
        if bin_dir.join("ffmpeg.exe").exists() {
            cmd.arg("--ffmpeg-location").arg(bin_dir);
        }
        base(&mut cmd);
        let info = self.subs_info_path(id).with_extension("manual.json");
        std::fs::write(&info, &out.stdout).map_err(|error| format!("保存字幕信息失败：{error}"))?;
        let downloaded = cmd.arg("--load-info-json").arg(&info).stdout(Stdio::null()).output();
        let _ = std::fs::remove_file(&info);
        let out = downloaded.map_err(|e| format!("启动 yt-dlp 失败：{e}"))?;
        if !out.status.success() { return Err(subtitle_request_error(&out.stderr)); }
        if !job.staged_names { align_subtitles(&video)?; }
        if crate::polish::find_sub(&video).is_none() {
            let err = String::from_utf8_lossy(&out.stderr);
            let last = err.lines().rev().find(|l| l.contains("ERROR")).unwrap_or("站点没有返回字幕").trim().to_string();
            return Err(format!("没下到字幕：{last}"));
        }
        // 3. 和下载时一样收尾
        finish_subs(&video, &opts, bin_dir, false, !job.staged_names)?;
        self.record_new_sidecars(&job, &video, &before);
        self.patch(id, |j| {
            j.options.subtitles = true;
            j.options.sub_langs = opts.sub_langs.clone();
            j.no_subs = false;
            j.subs_failed = false;
            if j.error.contains("字幕") {
                j.error.clear();
            }
        });
        self.save();
        Ok(true)
    }

    /// 改已下载视频的字幕样式，改完重新嵌进视频，并记进任务的选项（下次打开窗口按它显示）
    pub fn restyle(&self, id: &str, look: crate::engine::SubLook, bin_dir: &Path) -> Result<(), String> {
        let job = self.find(id).ok_or("任务不存在")?;
        let video = PathBuf::from(&job.file_path);
        let before = if job.staged_names { crate::video_files::sidecars(&video) } else { Vec::new() };
        restyle_matching_subtitle(&video, &look, !job.staged_names)?;
        embed_sub(&video, bin_dir);
        self.record_new_sidecars(&job, &video, &before);
        self.patch(id, |j| {
            j.options.sub_format = "ass".into();
            j.options.sub_scale = look.scale;
            j.options.sub_colorize = look.colorize;
            j.options.sub_color_zh = look.color_zh.clone();
            j.options.sub_color_en = look.color_en.clone();
            j.options.sub_shadow = look.shadow;
            j.options.sub_box = look.boxed;
            j.options.sub_scale_en = look.scale_en;
            j.options.sub_shadow_size = look.shadow_size;
            j.options.sub_font = look.font.clone();
            j.options.sub_shadow_alpha = look.shadow_alpha;
            j.options.sub_box_alpha = look.box_alpha;
            j.options.sub_outline = look.outline;
            j.options.sub_margin = look.margin;
        });
        self.save();
        Ok(())
    }

    fn record_new_sidecars(&self, job: &VideoJob, video: &Path, before: &[PathBuf]) {
        if !job.staged_names { return; }
        let added = crate::video_files::sidecars(video);
        self.patch(&job.id, |record| {
            for path in added {
                if !before.contains(&path) && !record.published_files.contains(&path) {
                    record.published_files.push(path);
                }
            }
        });
    }

    /// 换回原字幕
    pub fn polish_restore(&self, id: &str, bin_dir: &Path) -> Result<(), String> {
        let job = self.find(id).ok_or("任务不存在")?;
        let video = PathBuf::from(&job.file_path);
        let sub = crate::polish::find_sub(&video).ok_or("没找到字幕")?;
        crate::polish::restore(&sub)?;
        embed_sub(&video, bin_dir);
        self.patch(id, |j| {
            j.polish.clear();
            j.polished = false;
        });
        self.save();
        Ok(())
    }

    fn video_limit(&self) -> usize {
        crate::settings::Settings::load(&self.base).max_videos as usize
    }

    pub fn set_order(&self, ids: Vec<String>) {
        *self.order.lock().unwrap() = ids;
    }

    /// 有任务结束、暂停或删除后调用：先按拖出来的顺序，其余让排队最久的接上
    pub(crate) fn pump(self: &Arc<Self>) {
        loop {
            let guard = self.launching.lock().unwrap();
            let Some(bin_dir) = guard.clone() else { return };
            if self.running.lock().unwrap().len() >= self.video_limit() {
                return;
            }
            let order = self.order.lock().unwrap().clone();
            let next = self
                .jobs
                .lock()
                .unwrap()
                .iter()
                .filter(|j| j.state == "wait")
                .min_by_key(|j| (order.iter().position(|i| *i == j.id).unwrap_or(usize::MAX), j.created))
                .map(|j| j.id.clone());
            let Some(id) = next else { return };
            self.patch(&id, |j| {
                j.state = "running".into();
                j.stage = "preparing".into();
            });
            if let Err(e) = self.spawn(&id, &bin_dir) {
                self.patch(&id, |j| {
                    j.state = "error".into();
                    j.error = e;
                });
                self.save();
            }
        }
    }

    /// 用户选了不再下：只是不再提示
    pub fn dismiss_collision(&self, id: &str) {
        self.patch(id, |j| j.collided = false);
        self.save();
    }

    fn spawn(self: &Arc<Self>, id: &str, bin_dir: &Path) -> Result<(), String> {
        let gen = self.next_gen(id);
        let job = {
            let jobs = self.jobs.lock().unwrap();
            jobs.iter().find(|j| j.id == id).cloned().ok_or("任务不存在")?
        };

        // 字幕不在这条进程里下。yt-dlp 是先下完所有字幕才开始下视频的，一条被 429 拖住
        // 视频就跟着等；而且它不知道站点有哪些语言，只能给宽正则，一下七八条。
        // 这里只让它落一份元数据（不多发请求），由 spawn_subs 按元数据精确挑语言另外下。
        let mut opts = job.options.clone();
        let want_subs = opts.subtitles && !opts.audio_only;
        opts.subtitles = false;
        let info_path = self.subs_info_path(id);
        let _ = std::fs::remove_file(&info_path);

        let mut cmd = Command::new(ytdlp_path(bin_dir));
        // 格式选择按这条任务的选项来（画质上限、字幕、仅音频），和 Mac 版 VideoOptions.arguments 一致
        cmd.args(opts.arguments());
        cmd.args([
            "--no-playlist",
            // 字幕下不到（YouTube 常回 429）时 yt-dlp 默认整条任务直接失败，视频一个字节都不下。
            // 这条让它只报警告接着下视频；真正的下载失败照样是非 0 退出、照样报错
            "--ignore-errors",
            // --print 默认连带 quiet：「文件已存在」「正在合并」这些行就不输出了，
            // 撞车检测和合并状态全靠它们
            "--no-quiet",
            "--newline",
            "--no-colors",
            "--progress",
            // YouTube 单连接被限速，-N 4 实测比默认快一倍多
            "--concurrent-fragments",
            "4",
            // video 阶段比字幕还早：取消得再快，文件名里的 id 也已经知道了
            "--print",
            "video:SDID %(id)s",
            // 解析完马上报封面，别等到开始下载前
            "--print",
            "video:SDTHUMB %(thumbnail)s",
            "--print",
            "before_dl:SDTITLE %(title)s",
            // 大小来源分四级（见 feed 的 SDSIZE 解析）：顶层 filesize（精确）→
            // 各分格式 filesize 之和（精确）→ 各分格式 filesize_approx 之和（码率×时长，
            // 实测误差 0.2%）→ 顶层 filesize_approx。合并格式（视频流+音频流）按两路之和，
            // 别拿 progress.total_bytes_estimate 当准数——分段外推早期能猜出 1 GB（见 feed）。
            "--print",
            "before_dl:SDSIZE %(filesize)s %(filesize_approx)s \
             %(requested_formats.0.filesize)s %(requested_formats.0.filesize_approx)s \
             %(requested_formats.1.filesize)s %(requested_formats.1.filesize_approx)s",
            "--print",
            "before_dl:SDRES %(height)s %(width)s",
            "--print",
            "after_move:SDFILE %(filepath)s",
            "--progress-template",
            "download:SDPROG %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(progress.status)s %(info.ext)s",
        ]);
        // ffmpeg 可能装在本机 PATH 里（设置页显示「系统里已有」），这种情况**不要**传
        // --ffmpeg-location：传了等于告诉 yt-dlp「只在这个目录找」，反倒合并不了。
        if bin_dir.join("ffmpeg.exe").exists() {
            cmd.arg("--ffmpeg-location").arg(bin_dir);
        }
        // 有浏览器带来的登录信息就带上（Mac 版 SiteCookies.ytdlpArguments 同样规则）
        // 每次现查，不用任务里记的路径：扩展后来才交的登录信息也用得上，文件挪过位置也不失效
        let ck = cookie_file(&self.local, &job.url);
        if ck.is_file() {
            cmd.arg("--cookies").arg(ck);
        }
        // 境外站（YouTube、X 等）走系统代理：住宅 IP 绕过 bot 检测（Mac 版 LocalProxy 同样规则）。
        // 不走代理时 YouTube 只会报「Sign in to confirm you're not a bot」。
        for a in crate::proxy::ytdlp_args(&job.url) {
            cmd.arg(a);
        }
        // YouTube 解签名需要 JS 运行时，本机有 Node 就接上（Mac 版同样做法）。
        // 没有时部分格式缺失，但不影响下载。
        if let Some(node) = find_node() {
            cmd.arg("--js-runtimes").arg(format!("node:{node}"));
        }
        // NODE_OPTIONS 可能带受限 node shim（如 WorkBuddy 沙箱注入的
        // --require node-language-shim.cjs），会让 yt-dlp 解签名时读文件被拒，
        // 视频流全丢只剩音频。子进程用不上宿主的沙箱限制，直接摘掉。
        cmd.env_remove("NODE_OPTIONS");
        if want_subs {
            // yt-dlp 会自己补 .info.json；模板里的 % 要转义
            let stem = info_path.to_string_lossy().trim_end_matches(".info.json").replace('%', "%%");
            cmd.arg("--write-info-json").arg("-o").arg(format!("infojson:{stem}"));
            // 机翻语种（en-zh 这些）只有开了 --write-auto-subs 才进元数据。
            // 配一个谁都匹配不上的语言：拿到完整列表，但这条进程一条字幕也不下
            cmd.args(["--write-subs", "--write-auto-subs", "--sub-langs", "__none__"]);
        }
        let work_dir = job.working_dir();
        if job.staged_names {
            if work_dir == Path::new(&job.dir) { return Err("视频临时目录无效".into()); }
            std::fs::create_dir_all(&work_dir).map_err(|e| format!("无法创建视频临时目录：{e}"))?;
            crate::video_files::check_stage(&work_dir).map_err(|e| e.to_string())?;
            let _ = crate::baidu::hide_staging(&work_dir, true);
        }
        cmd.arg("-P")
            .arg(&work_dir)
            .arg("-o")
            // 结尾带 name_suffix：同一个视频下第二份（换了分辨率）时是 "_1"，
            // 这样两条任务各写各的文件，不会互相覆盖
            .arg(format!("%(title).80s [%(id)s]{}.%(ext)s", job.working_suffix()))
            .arg(&job.url)
            // stdin 一定要显式给 null：GUI 进程没有控制台，继承下来的 stdin 是无效句柄，
            // Python 拿它问终端尺寸会直接抛 [Errno 22] Invalid argument，任务就「0 B 出错」了
            // 输出编码不指望 yt-dlp 改：实测 --encoding、PYTHONIOENCODING、PYTHONUTF8
            // 都改不了它写出来的字节（它按系统代码页 cp936 写）。所以在读取侧解码，
            // 见 decode_line()。
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        // 输出不走管道，直接落到文件里再自己读。
        // 原因：Python（yt-dlp）在「没有控制台 + stdout 是管道」的组合下写一行就
        // OSError [Errno 22] Invalid argument（连 flush 都失败），任务立刻变成 0 B 出错。
        // 换成文件重定向后同样的参数在命令行里跑得好好的，说明是管道句柄的问题，绕开它。
        let log_path = self.local.join("cache/process").join(format!("ytdlp-{id}.log"));
        let log = std::fs::File::create(&log_path).map_err(|e| format!("建日志文件失败：{e}"))?;
        cmd.stdout(Stdio::from(
            log.try_clone().map_err(|e| format!("复制文件句柄失败：{e}"))?,
        ))
        .stderr(Stdio::from(log));
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        // 记下这条进程走的线路：下到一半代理开关了要换（见等待线程）
        let route = crate::proxy::ytdlp_args(&job.url);
        let mut child = cmd.spawn().map_err(|e| format!("启动 yt-dlp 失败：{e}"))?;
        self.running.lock().unwrap().insert(id.to_string(), child);
        if want_subs {
            self.spawn_subs(id, gen, bin_dir, &job);
        } else {
            self.subs_done.lock().unwrap().remove(id);
        }

        // 边写边读：每 250ms 看一眼日志文件，读到新内容就解析
        let done = Arc::new(AtomicBool::new(false));
        {
            let videos = Arc::clone(self);
            let id2 = id.to_string();
            let path = log_path.clone();
            let done2 = Arc::clone(&done);
            std::thread::spawn(move || {
                let mut st = Progress::default();
                let mut pos: u64 = 0;
                let mut pending: Vec<u8> = Vec::new();
                let mut idle = 0u32;
                loop {
                    if !videos.is_current(&id2, gen) {
                        break; // 这条被暂停/删除/重开了，日志已经不归这个线程管
                    }
                    let mut progressed = false;
                    if let Ok(mut f) = std::fs::File::open(&path) {
                        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                        if len > pos {
                            if f.seek(SeekFrom::Start(pos)).is_ok() {
                                let mut buf = Vec::new();
                                if f.take(len - pos).read_to_end(&mut buf).is_ok() {
                                    pos = len;
                                    progressed = true;
                                    pending.extend_from_slice(&buf);
                                    // 只处理完整的行，末尾半行留着下次拼
                                    while let Some(i) = pending.iter().position(|b| *b == b'\n') {
                                        let raw: Vec<u8> = pending.drain(..=i).collect();
                                        let line = decode_line(&raw);
                                        if line.starts_with("ERROR") {
                                            let msg =
                                                line.replace("ERROR: ", "").trim().to_string();
                                            let msg = friendly_error(&msg);
                                            videos.patch(&id2, |j| j.error = msg);
                                        }
                                        if line.contains("Unable to download video subtitles") {
                                            videos.patch(&id2, |j| j.subs_failed = true);
                                        }
                                        if let Some(stage) = post_stage(&line) {
                                            videos.patch(&id2, |j| {
                                                j.stage = stage.to_string();
                                                j.speed = 0;
                                            });
                                        }
                                        // 成品已经在盘上：yt-dlp 直接跳过并退出 0。
                                        // 分轨文件（.f137.mp4）也会报这句，那是续传时跳过已下完的一路，不算撞车。
                                        if let Some(existing) = already_downloaded(&line) {
                                            videos.patch(&id2, |j| {
                                                j.set_working_file(existing.display().to_string());
                                                // Staged output belongs to this job and may be reused on resume.
                                                if !j.staged_names && j.downloaded == 0 {
                                                    j.collided = true;
                                                }
                                            });
                                        }
                                        feed(&videos, &id2, &line, &mut st);
                                    }
                                    videos.save_throttled();
                                }
                            }
                        }
                    }
                    if progressed {
                        idle = 0;
                    } else {
                        idle += 1;
                    }
                    // 进程退了再等两轮，把尾巴读完
                    if done2.load(Ordering::Relaxed) && idle >= 2 {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                if done2.load(Ordering::Relaxed) && videos.is_current(&id2, gen) { let _ = std::fs::remove_file(path); }
            });
        }

        // 等进程退出的线程：成功标完成、失败标错误
        let done3 = Arc::clone(&done);
        let id4 = id.to_string();
        let url4 = job.url.clone();
        let bin4 = bin_dir.to_path_buf();
        let me3 = Arc::clone(self);
        std::thread::spawn(move || {
            let videos = me3;
            let mut seen_bytes = 0u64;
            let mut last_bytes = Instant::now();
            // 轮询子进程状态（Child 在 running 表里，拿不到所有权，用 try_wait）
            loop {
                std::thread::sleep(std::time::Duration::from_millis(400));
                if !videos.is_current(&id4, gen) {
                    done3.store(true, Ordering::Relaxed);
                    break; // 被暂停/取消了，或者已经有新进程接手
                }
                // 代理在这条进程启动之后开了或关了，而且它现在没在下东西：线路多半不通，
                // 不等它超时报错，换线路重新接上（已下的部分会续传）。下得好好的就不打断。
                let bytes = videos.find(&id4).map_or(0, |j| j.downloaded);
                if bytes != seen_bytes {
                    seen_bytes = bytes;
                    last_bytes = Instant::now();
                }
                let stalled = last_bytes.elapsed().as_secs() >= ROUTE_STALL_SECS;
                if stalled && crate::proxy::ytdlp_args(&url4) != route {
                    done3.store(true, Ordering::Relaxed);
                    videos.reroute(&id4, &bin4);
                    break;
                }
                let status = {
                    let mut map = videos.running.lock().unwrap();
                    match map.get_mut(&id4) {
                        Some(child) => match child.try_wait() {
                            Ok(Some(st)) => {
                                map.remove(&id4);
                                Some(st.code().unwrap_or(-1))
                            }
                            Ok(None) => None,
                            Err(_) => Some(-1),
                        },
                        None => {
                            done3.store(true, Ordering::Relaxed);
                            break; // 被取消了
                        }
                    }
                };
                if let Some(code) = status {
                    done3.store(true, Ordering::Relaxed);
                    videos.finish(&id4, code, gen, &bin4);
                    if let Some(job) = videos.find(&id4) { crate::app_log::record("video", &format!("task={} state={} exit={} error={}", id4, job.state, code, job.error)); }
                    videos.auto_polish(&id4, &bin4);
                    videos.pump();
                    break;
                }
            }
        });
        self.save();
        Ok(())
    }

    /// 换线路：结束当前进程，用现在的代理设置重新起一条
    fn reroute(self: &Arc<Self>, id: &str, bin_dir: &Path) {
        if let Some(mut c) = self.running.lock().unwrap().remove(id) {
            kill_tree(&mut c);
        }
        self.patch(id, |j| {
            j.stage = "preparing".into();
            j.speed = 0;
            j.error.clear();
        });
        if let Err(e) = self.spawn(id, bin_dir) {
            self.patch(id, |j| {
                j.state = "error".into();
                j.error = e;
            });
            self.save();
        }
    }

    fn subs_info_path(&self, id: &str) -> PathBuf {
        crate::migrate::local_path(&self.local, &format!("subsinfo-{id}.info.json"))
    }

    /// 另起一条 yt-dlp 只下字幕：等主进程落下元数据（通常一两秒，不多发请求），
    /// 按意图每个语系挑一条精确代码，再用 --load-info-json 下，不用重新解析页面。
    /// 视频完全不等它，它失败也不影响视频。
    fn spawn_subs(self: &Arc<Self>, id: &str, gen: u64, bin_dir: &Path, job: &VideoJob) {
        let flag = Arc::new(AtomicBool::new(false));
        self.subs_done.lock().unwrap().insert(id.to_string(), Arc::clone(&flag));
        let me = Arc::clone(self);
        let (id, bin, job) = (id.to_string(), bin_dir.to_path_buf(), job.clone());
        std::thread::spawn(move || {
            me.run_subs(&id, gen, &bin, &job);
            let _ = std::fs::remove_file(me.subs_info_path(&id));
            flag.store(true, Ordering::Relaxed);
        });
    }

    fn run_subs(&self, id: &str, gen: u64, bin_dir: &Path, job: &VideoJob) {
        let info = self.subs_info_path(id);
        let mut avail = None;
        while avail.is_none() {
            if !self.is_current(id, gen) {
                return;
            }
            avail = std::fs::read(&info).ok().and_then(|b| sub_langs_in_info(&b));
            if avail.is_none() {
                // 主进程已经结束还没有元数据：解析就失败了，错误由主进程报
                if !self.running.lock().unwrap().contains_key(id) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
        let mut intent = normalize_sub_intent(&job.options.sub_langs);
        let avail = avail.unwrap_or_default();
        let s = crate::settings::Settings::load(&self.base);
        let polishing = job.options.polish.unwrap_or(s.subtitle_polish)
            && matches!(intent, SubIntent::Zh | SubIntent::ZhEn)
            && !crate::polish::models().is_empty();
        let from_source = polishing
            .then(|| std::fs::read(&info).ok())
            .flatten()
            .and_then(|b| translate_source(&b, &avail, s.polish_mode == "translate"));
        // 只要中文但要润色：多下一份英文给模型参考，润色完不留
        if polishing && matches!(intent, SubIntent::Zh) {
            intent = SubIntent::ZhEn;
        }
        let langs = from_source.unwrap_or_else(|| pick_sub_langs(&intent, &avail));
        if langs.is_empty() {
            // 不算失败，但要记下来：列表上的标签看的是实际结果，不是当初勾了什么
            self.patch(id, |j| {
                j.no_subs = true;
                if !avail.is_empty() { j.error = "没有所选语言的字幕".into(); }
            });
            return;
        }
        let log_path = self.local.join("cache/process").join(format!("ytdlp-{id}-subs.log"));
        // 限流时等一等再试：视频不受影响，多等一会儿也不碍事
        for (attempt, wait_secs) in [0u64, 8, 20].into_iter().enumerate() {
            for _ in 0..wait_secs * 5 {
                if !self.is_current(id, gen) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            let mut youtube = youtube_subtitle_limiter(&job.url);
            if !self.is_current(id, gen) { return; }
            if youtube.as_ref().is_some_and(|cooldown| cooldown.is_some_and(|until| until > Instant::now())) {
                self.patch(id, |j| {
                    j.subs_failed = true;
                    j.error = "网站限制了字幕请求，视频已下载".into();
                });
                break;
            }
            let Ok(log) = std::fs::File::create(&log_path) else { return };
            let Ok(log2) = log.try_clone() else { return };
            let mut cmd = Command::new(ytdlp_path(bin_dir));
            cmd.args(["--skip-download", "--ignore-no-formats-error", "--ignore-errors", "--no-progress"])
                .arg("--load-info-json")
                .arg(&info)
                .args(job.options.sub_arguments(&langs))
                .arg("-P").arg(job.working_dir()).arg("-o")
                .arg(format!("%(title).80s [%(id)s]{}.%(ext)s", job.working_suffix()));
            if bin_dir.join("ffmpeg.exe").exists() {
                cmd.arg("--ffmpeg-location").arg(bin_dir);
            }
            let ck = cookie_file(&self.local, &job.url);
            if ck.is_file() {
                cmd.arg("--cookies").arg(ck);
            }
            for a in crate::proxy::ytdlp_args(&job.url) {
                cmd.arg(a);
            }
            cmd.env_remove("NODE_OPTIONS")
                .stdin(Stdio::null())
                .stdout(Stdio::from(log))
                .stderr(Stdio::from(log2));
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW);
            let Ok(mut child) = cmd.spawn() else { break };
            let child_status = loop {
                if !self.is_current(id, gen) {
                    kill_tree(&mut child);
                    let _ = std::fs::remove_file(&log_path);
                    return;
                }
                match child.try_wait() {
                    Ok(Some(status)) => break status.success(),
                    Err(_) => break false,
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(200)),
                }
            };
            let text = std::fs::read(&log_path).unwrap_or_default();
            let output = String::from_utf8_lossy(&text);
            let failed = !child_status || output.contains("ERROR:");
            if subtitle_rate_limited(&output) {
                if let Some(cooldown) = youtube.as_mut() {
                    **cooldown = Some(Instant::now() + std::time::Duration::from_secs(120));
                }
                self.patch(id, |j| {
                    j.subs_failed = true;
                    j.error = "网站限制了字幕请求，视频已下载".into();
                });
                break;
            }
            if !failed {
                let _ = std::fs::remove_file(&log_path);
                return;
            }
            if attempt == 2 {
                self.patch(id, |j| j.subs_failed = true);
            }
        }
        let _ = std::fs::remove_file(&log_path);
    }

    /// yt-dlp 退出后的收尾。state 要到字幕都处理完才翻成 done：
    /// 以前先报完成再嵌字幕，提示音响了文件还在被改写。
    fn finish(self: &Arc<Self>, id: &str, code: i32, gen: u64, bin_dir: &Path) {
        // 进程先退、日志后读：SDFILE 那行可能还没被读日志的线程消化，等它读完
        for _ in 0..8 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let ready = self
                .find(id)
                .map_or(true, |j| !j.working_file().is_empty() || j.collided || j.publish_plan.is_some());
            if ready {
                break;
            }
        }
        // 再等一轮：撞车那行和 SDFILE 都可能排在后面
        std::thread::sleep(std::time::Duration::from_millis(300));
        let Some(job) = self.find(id) else { return };
        if !self.is_current(id, gen) {
            return;
        }
        // 跳过了下载：字幕也不用下了（要再下一份会重新起一轮）
        if job.collided {
            self.next_gen(id);
        }
        // 撞上同名文件，用户又明确要再下一份：换个后缀重跑。
        // 算出来还是当前后缀说明找不到空位（文件名对不上），别原地反复重下
        let suffix = (code == 0 && job.collided && job.rename_on_collision)
            .then(|| self.suffix_after_collision(Path::new(&job.file_path), &job.name_suffix))
            .filter(|n| *n != job.name_suffix);
        if let Some(suffix) = suffix {
            self.patch(id, |j| {
                j.name_suffix = suffix;
                j.collided = false;
                j.file_path.clear();
                j.error.clear();
                j.stage = "preparing".into();
                j.downloaded = 0;
                j.total = 0;
            });
            if let Err(e) = self.spawn(id, bin_dir) {
                self.patch(id, |j| {
                    j.state = "error".into();
                    j.error = e;
                });
                self.save();
            }
            return;
        }
        let mut path = (!job.working_file().is_empty()).then(|| PathBuf::from(job.working_file()));
        let mut landed = path
            .as_deref()
            .and_then(|p| std::fs::metadata(p).ok())
            .is_some_and(|m| m.is_file() && m.len() > 0);
        // 视频已经合并好，只是最后改名时被占着（杀毒、索引在扫刚写完的大文件），yt-dlp 不重试。
        // 这种占用一般很快就放，我们替它改名，字幕接着处理
        if code != 0 && !landed && job.error.contains("WinError 32") {
            self.patch(id, |j| {
                j.stage = "正在收尾".into();
                j.speed = 0;
            });
            // 占用常常只有几秒，一开始就查，顺手记进日志：排查「到底是谁」全靠这一行
            let holders = merged_temp_path(&job.working_dir(), &job.vid, job.working_suffix())
                .map(|p| file_lockers(&p))
                .unwrap_or_default();
            if !holders.is_empty() {
                crate::mark(&format!("合并后的文件被占用：{}", holders.join("、")));
            }
            for _ in 0..60 {
                if !self.is_current(id, gen) {
                    return;
                }
                if let Some(p) = land_merged_temp(&job.working_dir(), &job.vid, job.working_suffix()) {
                    let s = p.display().to_string();
                    self.patch(id, |j| j.set_working_file(s));
                    path = Some(p);
                    landed = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            if !landed && !holders.is_empty() {
                let who = holders.join("、");
                self.patch(id, |j| {
                    let raw = j.error.split_once("\n\n").map_or(j.error.as_str(), |x| x.1).to_string();
                    j.error = format!("文件被 {who} 占用，关掉后点继续即可，不用重新下载。\n\n{raw}");
                });
            }
        }
        if (code != 0 || job.staged_names) && !landed && job.publish_plan.is_none() {
            self.patch(id, |j| {
                j.speed = 0;
                j.stage = String::new();
                j.state = "error".into();
                if j.error.is_empty() {
                    j.error = "下载失败".into();
                }
            });
            self.save();
            return;
        }
        if job.options.subtitles && !job.collided && !job.staged_ready {
            if let Some(p) = path.as_deref() {
                self.patch(id, |j| {
                    j.stage = "处理字幕".into();
                    j.speed = 0;
                });
                // The media child has exited. Subtitle work can run while the next video starts.
                self.pump();
                // 字幕进程可能还没跑完（视频小、或者它在等限流）。最多等 2 分钟，
                // 等不到也不能让任务一直挂着：视频是好的
                let flag = self.subs_done.lock().unwrap().get(id).cloned();
                if let Some(flag) = flag {
                    for _ in 0..600 {
                        if flag.load(Ordering::Relaxed) || !self.is_current(id, gen) {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(200));
                    }
                }
                if !self.is_current(id, gen) {
                    return;
                }
                // 这里没拿任何锁：ffmpeg 嵌字幕要几秒，不能卡住列表
                let polish_on = job.options.polish.unwrap_or_else(|| crate::settings::Settings::load(&self.base).subtitle_polish);
                if finish_subs(p, &job.options, bin_dir, polish_on, true).is_err() {
                    self.patch(id, |j| j.subs_failed = true);
                }
            }
        }
        if job.staged_names {
            self.patch(id, |j| j.staged_ready = true);
            match self.publish_video(id, gen) {
                Ok(output) => path = Some(output),
                Err(error) => {
                    if !self.is_current(id, gen) { return; }
                    use crate::video_files::PublishError;
                    self.patch(id, |j| {
                        j.speed = 0;
                        j.stage.clear();
                        j.collided = matches!(error, PublishError::Conflict);
                        j.state = (if j.collided { "pause" } else { "error" }).into();
                        j.error = match error {
                            PublishError::Conflict => "保存位置已有同名文件，已保留下载结果".into(),
                            PublishError::Io(error) => format!("保存成品失败，已保留下载结果：{error}"),
                        };
                    });
                    self.save();
                    return;
                }
            }
        }
        // 用真实文件大小把 total 校正为准确值（显示 129MB 而不是估算值）
        let size = path
            .as_deref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(0);
        self.patch(id, |j| {
            j.speed = 0;
            j.stage = String::new();
            j.state = "done".into();
            j.finished_at = now_ms();
            j.error = if j.collided {
                "文件已存在，已跳过下载".into()
            } else if j.subs_failed {
                if subtitle_rate_limited(&j.error) {
                    queue_subtitle_retry(j);
                    j.error.clone()
                } else {
                    "部分字幕没下到，可以在任务菜单里补字幕".into()
                }
            } else if j.no_subs && crate::links::supports_subtitles(&j.url) {
                // 不提供字幕的站（抖音这些）用户根本没看到选项，不用说
                if j.error == "没有所选语言的字幕" { j.error.clone() } else { "这个视频没有字幕".into() }
            } else {
                String::new()
            };
            if size > 0 {
                j.total = size;
                j.downloaded = size;
            }
        });
        self.save();
    }

    fn publish_video(&self, id: &str, gen: u64) -> Result<PathBuf, crate::video_files::PublishError> {
        use crate::video_files::PublishPlan;
        // Two downloads with the same title must check and publish in order.
        static PUBLISHING: Mutex<()> = Mutex::new(());
        let _turn = PUBLISHING.lock().unwrap();
        let active = || self.is_current(id, gen);
        if !active() { return Err(std::io::Error::other("任务已暂停").into()); }
        let job = self.find(id).ok_or_else(|| std::io::Error::other("任务不存在"))?;
        let stage = job.working_dir();
        let directory = Path::new(&job.dir);
        crate::video_files::check_stage(&stage)?;
        let plan = if let Some(plan) = job.publish_plan {
            plan
        } else {
            let input = Path::new(&job.staged_file);
            if input.parent() != Some(stage.as_path()) {
                return Err(std::io::Error::other("临时视频位置无法确认").into());
            }
            let plan = PublishPlan::new(input, directory, &job.name_suffix, job.rename_on_collision)?;
            self.patch(id, |j| j.publish_plan = Some(plan.clone()));
            self.save_checked()?;
            plan
        };
        let files = plan.apply(&stage, directory, active)?;
        self.patch(id, |j| {
            j.file_path = plan.media.to_string_lossy().into_owned();
            j.name_suffix = plan.suffix;
            j.published_files = files;
            j.staged_file.clear();
            j.publish_plan = None;
        });
        self.save_checked()?;
        let _ = std::fs::remove_dir(&stage);
        Ok(plan.media)
    }

    /// 记录的落盘路径和磁盘真名对不上时（见 engine::resolve_existing），用真名覆盖
    pub fn adopt_real_path(&self, id: &str, real: &Path) {
        let s = real.to_string_lossy().to_string();
        self.patch(id, |j| j.file_path = s.clone());
        self.save();
    }

    /// 暂停 = 杀掉进程树（yt-dlp 会再起 ffmpeg 子进程，只杀父进程文件还被占着），
    /// 状态留着以便继续
    pub fn pause(self: &Arc<Self>, id: &str) -> Result<(), String> {
        self.next_gen(id);
        if let Some(mut c) = self.running.lock().unwrap().remove(id) {
            kill_tree(&mut c);
        }
        self.patch(id, |j| {
            j.state = "pause".into();
            j.speed = 0;
        });
        self.save();
        self.pump();
        Ok(())
    }

    /// 微信视频号：扩展解析出临时播放地址后直接下载，不走 yt-dlp
    pub fn start_wechat(self: &Arc<Self>, share: &str, playable: &str, title: &str, dir: &Path, force: bool) -> Result<String, String> {
        let key = crate::links::canonical_url(share);
        let active = self.jobs.lock().unwrap().iter().any(|j| crate::links::canonical_url(&j.url) == key && j.state != "error");
        if active && !force {
            return Err("这个视频号链接已经在列表里了".into());
        }
        self.jobs.lock().unwrap().retain(|j| crate::links::canonical_url(&j.url) != key || j.state != "error");
        let mut job = VideoJob::new(share, dir, VideoOptions::default());
        job.title = if title.is_empty() { WECHAT_SITE.into() } else { title.to_string() };
        job.site = WECHAT_SITE.into();
        let id = job.id.clone();
        let (name, folder) = (job.title.clone(), dir.to_path_buf());
        self.jobs.lock().unwrap().insert(0, job);
        self.save();
        let gen = self.next_gen(&id);
        let (me, id2, playable) = (Arc::clone(self), id.clone(), playable.to_string());
        std::thread::spawn(move || {
            let current = |d: u64, t: u64, s: u64| {
                if !me.is_current(&id2, gen) {
                    return false;
                }
                me.patch(&id2, |j| {
                    (j.downloaded, j.total, j.speed) = (d, t, s);
                    j.stage = String::new();
                });
                true
            };
            let result = crate::wechat::download(&playable, &name, &folder, &current);
            if !me.is_current(&id2, gen) {
                return;
            }
            me.patch(&id2, |j| {
                j.speed = 0;
                j.stage = String::new();
                j.finished_at = now_ms();
                match &result {
                    Ok(p) => {
                        j.state = "done".into();
                        j.file_path = p.display().to_string();
                        j.title = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                        j.total = std::fs::metadata(p).map(|m| m.len()).unwrap_or(j.total);
                        j.downloaded = j.total;
                    }
                    Err(e) => {
                        j.state = "error".into();
                        j.error = if e.is_empty() { "已中断，请重新解析".into() } else { e.clone() };
                    }
                }
            });
            me.save();
        });
        Ok(format!("v:{id}"))
    }

    /// 继续 = 从头再下一遍（yt-dlp 里断点续传靠 --continue，默认开启，已下的分片会跳过）
    pub fn resume(self: &Arc<Self>, id: &str, bin_dir: &Path) -> Result<(), String> {
        let exists = self.jobs.lock().unwrap().iter().any(|j| j.id == id);
        if !exists {
            return Err("任务不存在".into());
        }
        // 播放地址是临时的，也不存盘，只能重新解析
        if self.find(id).is_some_and(|j| j.site == WECHAT_SITE) {
            return Err("请在已登录的腾讯元宝页面，用浏览器扩展重新解析这个视频号".into());
        }
        // 上次卡在合并后的改名：现在放开了就直接收尾，不用重新合并、重下字幕
        if let Some(j) = self.find(id).filter(|j| j.error.contains("WinError 32")) {
            if let Some(p) = land_merged_temp(&j.working_dir(), &j.vid, j.working_suffix()) {
                let s = p.display().to_string();
                self.patch(id, |j| {
                    j.set_working_file(s);
                    j.state = "running".into();
                    j.stage = "处理字幕".into();
                    j.error.clear();
                });
                let gen = self.next_gen(id);
                let (me, id2, bin) = (Arc::clone(self), id.to_string(), bin_dir.to_path_buf());
                std::thread::spawn(move || me.finish(&id2, 0, gen, &bin));
                return Ok(());
            }
        }
        // 成品已经落盘就直接标完成：暂停前文件可能已经写完（SDFILE 也报过了），
        // 再拉起 yt-dlp 去写会被占着的 .temp.mp4 / 成品 → WinError 5 拒绝访问。
        if let Some(j) = self.find(id) {
            if j.staged_names && (j.publish_plan.is_some() || crate::video_files::regular(Path::new(&j.staged_file)).is_ok()) {
                self.patch(id, |job| {
                    job.state = "running".into();
                    job.stage = "正在收尾".into();
                    job.error.clear();
                    job.collided = false;
                });
                self.save();
                let gen = self.next_gen(id);
                let (me, id, bin) = (Arc::clone(self), id.to_string(), bin_dir.to_path_buf());
                std::thread::spawn(move || {
                    me.finish(&id, 0, gen, &bin);
                    me.auto_polish(&id, &bin);
                    me.pump();
                });
                return Ok(());
            }
            if !j.file_path.is_empty() {
                if let Ok(md) = std::fs::metadata(&j.file_path) {
                    if md.is_file() && md.len() > 0 {
                        let len = md.len();
                        self.patch(id, |j| {
                            j.state = "done".into();
                            j.finished_at = now_ms();
                            j.speed = 0;
                            j.stage = String::new();
                            j.error.clear();
                            j.total = len;
                            j.downloaded = len;
                        });
                        self.save();
                        return Ok(());
                    }
                }
            }
        }
        self.patch(id, |j| {
            j.state = "running".into();
            j.error.clear();
            j.stage = "preparing".into();
            j.subs_failed = false;
            j.no_subs = false;
        });
        self.launch(id, bin_dir)
    }

    pub fn remove(self: &Arc<Self>, id: &str) -> Result<(), String> {
        self.next_gen(id);
        if let Some(mut c) = self.running.lock().unwrap().remove(id) {
            kill_tree(&mut c);
        }
        self.jobs.lock().unwrap().retain(|j| j.id != id);
        self.save();
        self.pump();
        // 这条任务的日志用不上了，留着只会越积越多
        let _ = std::fs::remove_file(self.local.join("cache/process").join(format!("ytdlp-{id}.log")));
        let _ = std::fs::remove_file(self.subs_info_path(id));
        let _ = std::fs::remove_file(self.local.join("cache/process").join(format!("ytdlp-{id}-subs.log")));
        self.gens.lock().unwrap().remove(id);
        self.subs_done.lock().unwrap().remove(id);
        Ok(())
    }

    pub fn find(&self, id: &str) -> Option<VideoJob> {
        self.jobs.lock().unwrap().iter().find(|j| j.id == id).cloned()
    }

    /// 限流保存：同一秒内多次调用只写一次
    fn save_throttled(&self) {
        static LAST: Mutex<Option<Instant>> = Mutex::new(None);
        {
            let mut last = LAST.lock().unwrap();
            if let Some(t) = *last {
                if t.elapsed().as_secs() < 1 {
                    return;
                }
            }
            *last = Some(Instant::now());
        }
        self.save();
    }
}

impl Drop for Videos {
    fn drop(&mut self) {
        // 退出时不杀正在下的视频任务，只记录状态（下次启动标暂停）
        if let Ok(mut map) = self.running.lock() {
            for (_, c) in map.iter_mut() {
                let _ = c.kill();
            }
        }
    }
}

/// 合并、转字幕这些收尾阶段的日志行 → 界面上的状态文字。
/// 这时已经没有进度可报了，得告诉用户在干什么，不然看着像卡死。
fn post_stage(line: &str) -> Option<&'static str> {
    if line.starts_with("[Merger]") || line.starts_with("[VideoRemuxer]") {
        return Some("正在合并");
    }
    if line.starts_with("[ExtractAudio]") {
        return Some("正在提取音频");
    }
    if line.starts_with("[Fixup") || line.starts_with("[Metadata]") {
        return Some("正在写入信息");
    }
    None
}

/// 「[download] xxx.mp4 has already been downloaded」里的成品路径。
/// 分轨文件（xxx.f137.mp4）返回 None：那只是续传时跳过下完的一路。
fn already_downloaded(line: &str) -> Option<PathBuf> {
    let rest = line.strip_prefix("[download] ")?;
    let path = rest.strip_suffix(" has already been downloaded")?;
    let stem = Path::new(path).file_stem()?.to_string_lossy().to_string();
    if let Some((_, fmt)) = stem.rsplit_once('.') {
        // yt-dlp 的分轨名是 .f<格式号>
        let is_part = fmt.len() > 1
            && fmt.starts_with('f')
            && fmt[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if is_part {
            return None;
        }
    }
    let p = PathBuf::from(path);
    Some(crate::engine::resolve_existing(&p).unwrap_or(p))
}

/// 任务文件名去掉自己的后缀，得到这个视频的基准文件名
fn strip_suffix(path: &Path, suffix: &str) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let base = if suffix.is_empty() { stem.as_str() } else { stem.strip_suffix(suffix)? };
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    Some(path.with_file_name(format!("{base}{ext}")))
}

/// base.mp4 + "_1" → base_1.mp4
fn with_suffix(base: &Path, suffix: &str) -> Option<PathBuf> {
    let stem = base.file_stem()?.to_string_lossy().to_string();
    let ext = base.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    Some(base.with_file_name(format!("{stem}{suffix}{ext}")))
}

/// 目录里已经有这个视频的成品吗（文件名里带「 [视频id]」）。
/// 从链接就能认出 id 的站才查得了，不用等 yt-dlp 解析，投任务前立刻知道。
pub fn existing_file_for(url: &str, dir: &Path) -> Option<PathBuf> {
    // 带后缀的（_1）也算：有第二份必有第一份或用户已知情，这里只认第一份的文件名，简单可靠
    let token = format!(" [{}].", crate::links::video_id_of(url)?).to_lowercase();
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        let name = p.file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        let ext = p.extension().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        let is_final = ["mp4", "mkv", "webm", "m4a", "mp3", "flv"].contains(&ext.as_str())
            && !name.contains(".temp.")
            && already_downloaded(&format!("[download] {name} has already been downloaded")).is_some();
        is_final && name.contains(&token)
    })
}

/// 这条视频任务在盘上的全部文件（成品、字幕、分轨、临时文件）。
/// 只认精确标记：文件名含「 [视频id]后缀.」，或以成品文件名 + 「.」开头。
/// 两样都不知道（刚开始解析就取消）说明还没写过文件，一个都不碰——
/// 以前这里按标题前缀甚至「名字里有 [」匹配，取消一条会删掉目录里所有视频。
/// `others` 里有任务指向同一批文件时也不删，文件还归它用。
pub fn job_files(job: &VideoJob, others: &[VideoJob]) -> Vec<PathBuf> {
    if job.staged_names {
        let stage = job.working_dir();
        let mut paths = job.published_files.clone();
        if let Some(plan) = &job.publish_plan {
            paths.extend(plan.moved_files(&stage, Path::new(&job.dir)));
        }
        paths.retain(|path| crate::video_files::regular(path).is_ok() && !others.iter().any(|other| {
            other.id != job.id && (Path::new(&other.file_path) == path || other.published_files.contains(path))
        }));
        if stage != Path::new(&job.dir) && crate::video_files::check_stage(&stage).is_ok() { paths.push(stage); }
        return paths;
    }
    let same_files = |o: &VideoJob| {
        o.id != job.id
            && o.dir == job.dir
            && o.name_suffix == job.name_suffix
            && ((!job.vid.is_empty() && o.vid == job.vid)
                || (!job.file_path.is_empty() && o.file_path == job.file_path))
    };
    if others.iter().any(same_files) {
        return vec![];
    }
    let token = (!job.vid.is_empty())
        .then(|| format!(" [{}]{}.", job.vid, job.name_suffix).to_lowercase());
    let prefix = Path::new(&job.file_path)
        .file_stem()
        .filter(|_| !job.file_path.is_empty())
        .map(|s| format!("{}.", s.to_string_lossy().to_lowercase()));
    if token.is_none() && prefix.is_none() {
        return vec![];
    }
    let Ok(entries) = std::fs::read_dir(&job.dir) else {
        return vec![];
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            token.as_deref().is_some_and(|t| name.contains(t))
                || prefix.as_deref().is_some_and(|t| name.starts_with(t))
        })
        .collect()
}

/// Windows 上 yt-dlp 会再起 ffmpeg 子进程：只 kill 父进程文件还被占着，
/// 续下时就报 WinError 5 拒绝访问。taskkill /T 把整棵进程树干掉。
fn kill_tree(c: &mut Child) {
    #[cfg(windows)]
    {
        let id = c.id();
        if id != 0 {
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &id.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .status();
            let _ = c.wait();
            return;
        }
    }
    let _ = c.kill();
    let _ = c.wait();
}

fn ytdlp_path(ffmpeg_dir: &Path) -> PathBuf {
    // bin 目录和 ffmpeg 是同一个，yt-dlp 也在里面
    let p = ffmpeg_dir.join("yt-dlp.exe");
    if p.exists() {
        p
    } else {
        PathBuf::from("yt-dlp.exe")
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn load(base: &Path) -> Vec<VideoJob> {
    std::fs::read_to_string(base.join("videos.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 站点名，界面任务行左侧的标签
pub fn site_name(url: &str) -> String {
    let host = site_host(url);
    site_for_host(&host).map(str::to_string).unwrap_or(host)
}

pub(crate) fn known_site_name(url: &str) -> Option<&'static str> {
    site_for_host(&site_host(url))
}

fn site_host(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_lowercase)).unwrap_or_default()
}

fn site_for_host(host: &str) -> Option<&'static str> {
    // 支持的视频站，按完整域名（或它的子域名）认：只看包含关系会把 dropbox.com 认成 X
    const NAMES: &[(&[&str], &str)] = &[
        (&["bilibili.com", "b23.tv"], "B 站"),
        (&["youtube.com", "youtu.be"], "YouTube"),
        (&["douyin.com"], "抖音"),
        (&["xiaohongshu.com", "xhslink.com"], "小红书"),
        (&["kuaishou.com"], "快手"),
        (&["weibo.com", "weibo.cn"], "微博"),
        (&["ixigua.com"], "西瓜视频"),
        (&["vimeo.com"], "Vimeo"),
        (&["x.com", "twitter.com"], "X"),
        (&["xinpianchang.com"], "新片场"),
        (&["movie.douban.com"], "豆瓣"),
        (&["mp.weixin.qq.com", "mpvideo.qpic.cn"], "微信公众号"),
        (&["channels.weixin.qq.com", "weixin.qq.com"], WECHAT_SITE),
    ];
    for (domains, name) in NAMES {
        if domains.iter().any(|d| host == *d || host.ends_with(&format!(".{d}"))) {
            return Some(*name);
        }
    }
    None
}

#[cfg(test)]
mod site_name_tests {
    #[test]
    fn names_all_supported_sites_and_not_lookalikes() {
        for (u, n) in [
            ("https://www.bilibili.com/video/x", "B 站"), ("https://b23.tv/x", "B 站"), ("https://youtu.be/x", "YouTube"),
            ("https://www.douyin.com/jingxuan", "抖音"), ("https://www.xiaohongshu.com/explore/1", "小红书"), ("https://www.kuaishou.com/x", "快手"),
            ("https://weibo.com/u/1", "微博"), ("https://m.weibo.cn/status/1", "微博"), ("https://www.ixigua.com/1", "西瓜视频"),
            ("https://vimeo.com/1", "Vimeo"), ("https://x.com/a/status/1", "X"), ("https://www.xinpianchang.com/a1", "新片场"),
            ("https://movie.douban.com/trailer/1/", "豆瓣"), ("https://channels.weixin.qq.com/finder-preview/pages/sph?id=1", "微信视频号"),
            ("https://mp.weixin.qq.com/s/article", "微信公众号"), ("https://mpvideo.qpic.cn/lesson.mp4", "微信公众号"),
        ] {
            assert_eq!(super::site_name(u), n, "{u}");
        }
        assert_eq!(super::site_name("https://www.dropbox.com/s/x"), "www.dropbox.com");
        assert_eq!(super::site_name("https://xbox.com/"), "xbox.com");
    }
}

/// 解析 yt-dlp 输出的一行。返回 true 表示任务状态变了、需要落盘。
/// 两条流共用：stdout 上的 SDTITLE/SDSIZE/SDFILE 和 stderr 上的 SDPROG 都认。
///
/// 进度只有一个分母：所有轨加起来的总量。以前分母用「当前这一路的大小」，
/// 视频轨下完就显示 99%，开始下音频分母才变大、进度往回掉，就是反复出现的
/// 「到 98% 停一下又继续」。规则：一旦知道过总量就再也不回到「未知」，
/// 界面的滚动条只会出现在最开头。
#[derive(Default)]
struct Progress {
    /// 已经下完的轨，字节要累加，否则进度条会往回跳
    finished_bytes: u64,
    /// 当前这条流已下到的字节
    current_done: u64,
    /// 当前流的 progress.total_bytes（精确）；0 = yt-dlp 没报
    current_total: u64,
    /// 当前流的 progress.total_bytes_estimate（分段外推的猜测）；0 = 没报
    current_est: u64,
    /// before_dl 报的每一路预计大小（视频、音频；单格式就一项）
    plan: Vec<u64>,
    /// 正在下 plan 里的第几路
    stream: usize,
    /// 上一次算出来的总量
    last_total: u64,
    samples: Vec<(Instant, u64)>,
}

/// 代理变了之后，没有新字节多久就换线路
const ROUTE_STALL_SECS: u64 = 4;

/// total_bytes_estimate 在头几个分片时能猜出真实值的 8 倍，下到这么多才开始信它
const EST_TRUST_BYTES: u64 = 2 * 1024 * 1024;

impl Progress {
    /// 显示用的总大小 = 已下完的轨 + 当前轨 + 还没开始的轨。0 = 从头到现在都不知道。
    /// 当前轨的大小按可信度取：progress 的精确值 > before_dl 的预计值 > 分段外推。
    fn display_total(&mut self) -> u64 {
        let downloaded = self.finished_bytes + self.current_done;
        // 每一路都下完了（在合并）：实际下到多少就是多少
        if !self.plan.is_empty() && self.stream >= self.plan.len() {
            self.last_total = downloaded;
            return downloaded;
        }
        let planned = self.plan.get(self.stream).copied().unwrap_or(0);
        let cur = if self.current_total > 0 {
            self.current_total
        } else if planned >= self.current_done && planned > 0 {
            planned
        } else if self.current_est > self.current_done && self.current_done >= EST_TRUST_BYTES {
            self.current_est
        } else if planned > 0 {
            self.current_done // 预计值偏小，已经超了：先按已下算
        } else {
            0
        };
        let rest: u64 = self.plan.iter().skip(self.stream + 1).sum();
        let t = if cur > 0 || rest > 0 {
            self.finished_bytes + cur + rest
        } else {
            self.last_total
        };
        let t = if t > 0 { t.max(downloaded) } else { 0 };
        self.last_total = t;
        t
    }
}

/// SDSIZE/SDPROG 里的数字：NA、0、负数都当「没报」
fn num(s: &str) -> u64 {
    s.parse::<f64>()
        .ok()
        .filter(|v| *v > 0.0)
        .map(|v| v as u64)
        .unwrap_or(0)
}

/// SDSIZE 的六个值：顶层 filesize / 顶层 filesize_approx /
/// 视频流 filesize / 视频流 filesize_approx / 音频流 filesize / 音频流 filesize_approx。
/// 返回每一路的预计大小。合并格式顶层是 NA，大小在两个分格式上；
/// 精确值没有就用 approx（码率×时长，实测误差 0.2%）。一个都没有返回空。
fn parse_plan(s: &str) -> Vec<u64> {
    let f: Vec<u64> = s.split_whitespace().map(num).collect();
    let at = |i: usize| f.get(i).copied().unwrap_or(0);
    let pick = |exact: u64, approx: u64| if exact > 0 { exact } else { approx };
    let streams: Vec<u64> = [pick(at(2), at(3)), pick(at(4), at(5))]
        .into_iter()
        .filter(|v| *v > 0)
        .collect();
    if !streams.is_empty() {
        return streams;
    }
    match pick(at(0), at(1)) {
        0 => vec![],
        v => vec![v],
    }
}

/// 分辨率档位标签：height 取不低于它的最大标准档（≥2160→"2160P"，≥1080→"1080P"…）
/// 分辨率标签统一成「宽 x 高」（1920 x 1080），不再压缩成 1080P 这种档位：
/// 档位是猜的，用户看到 480P 也不知道画面到底多大。
/// 只有高度时（文件名里抠出来的 1080p）按 16:9 估宽，估出来的值照样标出去。
fn res_label(w: u64, h: u64) -> Option<String> {
    if (h == 0 && w == 0) || h > 8640 || w > 8640 {
        return None;
    }
    let (w, h) = match (w, h) {
        (0, h) => (((h as f64) * 16.0 / 9.0).round() as u64, h),
        (w, 0) => (w, ((w as f64) * 9.0 / 16.0).round() as u64),
        (w, h) => (w, h),
    };
    Some(format!("{} x {}", w, h))
}

/// 老任务（这次更新前下的）没记分辨率，界面上就空着。
/// 兜底办法：拿 ffprobe 读一眼已经落盘的文件，取第一条视频流的高度换成档位标签。
/// ffprobe 跟着 ffmpeg 走（工具目录里的那份，或系统 PATH 里的），没有就当探不到。
pub fn probe_resolution(file: &Path) -> Option<String> {
    if !file.is_file() {
        return None;
    }
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Some(dir) = file.parent() {
        cands.push(dir.join("ffprobe.exe"));
    }
    cands.push(PathBuf::from("ffprobe"));
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    for exe in cands {
        let mut cmd = Command::new(&exe);
        cmd.args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=height,width",
                "-of",
                "csv=p=0",
            ])
            .arg(file)
            .stdin(Stdio::null());
        // 不加这个，每探一个任务就闪一个控制台黑框（启动时连续闪好几个）
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let out = cmd.output();
        let Ok(out) = out else { continue };
        if !out.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        // csv 输出是「高,宽」；竖屏按短边算，和 SDRES 那边同一套口径
        let nums: Vec<u64> = text
            .split(|c: char| c == '\n' || c == ',' || c == '\r')
            .filter_map(|x| x.trim().parse::<u64>().ok())
            .collect();
        // ffprobe 的 csv 按字段内部顺序输出，是「宽,高」——
        // 就算 -show_entries 写成 height,width 也一样，别按请求顺序猜
        let (w, h) = match nums.as_slice() {
            [] => (0, 0),
            [a] => (*a, 0),
            [a, b, ..] => (*a, *b),
        };
        return res_label(w, h);
    }
    None
}

/// 从资源名/文件名里提取分辨率标签（内核任务用）："Movie.2024.1080p.x264" → "1080P"。
/// 认 `(\d{3,4})p`（不分大小写）和 4K/8K（→2160P/4320P），取档位最高的那个。
pub fn resolution_from_name(s: &str) -> Option<String> {
    let mut best = 0u64;
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        // 4K / 8K：前后不能紧挨字母数字（排除 "mp4k"、"h8k" 这类）
        let c = b[i].to_ascii_lowercase();
        if (c == b'4' || c == b'8')
            && b.get(i + 1).map(|x| x.to_ascii_lowercase()) == Some(b'k')
            && b.get(i + 2).map(|x| x.is_ascii_alphanumeric()) != Some(true)
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric())
        {
            best = best.max(if c == b'8' { 4320 } else { 2160 });
            i += 2;
            continue;
        }
        // (\d{3,4})p：数字前面不能是字母（排除 "x264p"）
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let len = i - start;
            let prev_ok = start == 0 || !b[start - 1].is_ascii_alphabetic();
            let tail_ok = (3..=4).contains(&len)
                && b.get(i).map(|x| x.to_ascii_lowercase()) == Some(b'p');
            if prev_ok && tail_ok {
                if let Ok(n) = s[start..i].parse::<u64>() {
                    best = best.max(n.min(4320));
                }
            }
            continue;
        }
        i += 1;
    }
    res_label(0, best)
}

fn feed(videos: &Arc<Videos>, id: &str, line: &str, st: &mut Progress) -> bool {
    if let Some(t) = line.strip_prefix("SDTITLE ") {
        videos.patch(id, |j| j.title = t.trim().to_string());
        return true;
    }
    if let Some(u) = line.strip_prefix("SDTHUMB ") {
        let u = u.trim();
        if !u.is_empty() && u != "NA" && !u.eq_ignore_ascii_case("none") {
            videos.patch(id, |j| j.thumbnail_url = Some(u.to_string()));
            // 封面下载会阻塞几百毫秒，别卡住读日志的线程。已经有图（提前拿到了）就不重复下
            let (base, id2, url) = (videos.local.clone(), id.to_string(), u.to_string());
            if crate::thumbs::cached(&base, &id2).is_none() {
                std::thread::spawn(move || {
                    crate::thumbs::remote(&base, &id2, &url);
                });
            }
        }
        return true;
    }
    if let Some(v) = line.strip_prefix("SDID ") {
        videos.patch(id, |j| j.vid = v.trim().to_string());
        return true;
    }
    if let Some(s) = line.strip_prefix("SDSIZE ") {
        st.plan = parse_plan(s);
        st.stream = 0;
        let t = st.display_total();
        if t > 0 {
            videos.patch(id, |j| j.total = t);
        }
        return true;
    }
    if let Some(r) = line.strip_prefix("SDRES ") {
        // "SDRES 1080 1920"：竖屏视频按短边算（1080x1920 叫 1080P，不叫 1920P）
        let mut it = r.split_whitespace().map(num);
        let (h, w) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
        if let Some(label) = res_label(w, h) {
            videos.patch(id, |j| j.resolution = Some(label));
            return true;
        }
        return false;
    }
    if let Some(f) = line.strip_prefix("SDFILE ") {
        let path = f.trim().to_string();
        // yt-dlp 报的路径里可能有 ⧸ 这种字符，读日志时被系统代码页吃掉了，
        // 存下来就和磁盘真名对不上。落盘时以磁盘为准，找不到才用原文。
        let real = crate::engine::resolve_existing(Path::new(&path))
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or(path);
        videos.patch(id, |j| j.set_working_file(real));
        return true;
    }
    let Some(p) = line.strip_prefix("SDPROG ") else {
        return false;
    };
    let f: Vec<&str> = p.split_whitespace().collect();
    if f.len() < 4 {
        return false;
    }
    let done = num(f[0]);
    // f[1] total_bytes（精确）、f[2] total_bytes_estimate（猜测）分开存：
    // 只有完全没有精确值时才轮到 estimate 上场（见 display_total）
    let total_bytes = num(f[1]);
    let total_est = num(f[2]);
    let status = f[3];
    // 字幕也走同一个下载器：几百字节一条，每条都报「下完了」。算进去的话
    // 开头进度条会被一条条字幕反复顶到 99%，又把几百字节当成一路视频流
    if f.get(4).is_some_and(|e| is_sub_ext(e)) {
        return false;
    }
    if status == "finished" {
        // 一路流下完：只累加实际下到的字节。done 为 0 时才退回 total_bytes
        //（个别格式 finished 行不报 downloaded）。别用 max——total_bytes 可能是
        // 偏大的估算，累进 finished_bytes 会把进度直接顶到 100%。
        st.finished_bytes += if done > 0 { done } else { total_bytes };
        st.stream += 1;
        st.current_done = 0;
        st.current_total = 0;
        st.current_est = 0;
    } else {
        st.current_done = done;
        st.current_total = total_bytes;
        st.current_est = total_est;
    }
    let downloaded = st.finished_bytes + st.current_done;
    // 速度自己算：yt-dlp 每换一个分块都从头估，开头能报出几百 MB/s
    let now = Instant::now();
    st.samples.push((now, downloaded));
    st.samples
        .retain(|(t, _)| now.duration_since(*t).as_secs_f64() <= 3.0);
    let speed = match st.samples.first() {
        Some((t0, d0)) if now.duration_since(*t0).as_secs_f64() > 0.5 => {
            ((downloaded.saturating_sub(*d0)) as f64 / now.duration_since(*t0).as_secs_f64()) as u64
        }
        _ => 0,
    };
    let total_now = st.display_total();
    videos.patch(id, |j| {
        j.downloaded = downloaded;
        // 这一轮还算不出来时别覆盖：续传刚开始，上次的总量还是对的
        if total_now > 0 {
            j.total = total_now;
        }
        j.speed = speed;
    });
    true
}

pub(crate) fn is_sub_ext(ext: &str) -> bool {
    ["vtt", "srt", "ass", "ssa", "lrc", "ttml", "dfxp", "sbv", "json3", "srv1", "srv2", "srv3"]
        .iter()
        .any(|e| ext.eq_ignore_ascii_case(e))
}

/// 把 yt-dlp 写出来的一行字节解成字符串。
///
/// Windows 上 yt-dlp（Python）按系统代码页写输出，中文标题在 UTF-8 解码下就是
/// 「¡¾¹Ù·½ MV¡¿」这种乱码——真实文件名是对的，只是我们读日志时解错了。
/// 先按 UTF-8 试（英文和未来的版本），不行再按系统代码页转。
fn decode_line(raw: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.trim_end_matches(['\r', '\n']).to_string();
    }
    let s = cp_to_string(raw, 0);
    s.trim_end_matches(['\r', '\n']).to_string()
}

/// 用 Windows 的 MultiByteToWideChar 按代码页转（cp 传 0 = 系统 ANSI 代码页）。
/// 之所以不引 encoding_rs：只为解一行日志加一个依赖不划算，系统 API 就够了。
#[cfg(windows)]
fn cp_to_string(bytes: &[u8], cp: u32) -> String {
    use windows_sys::Win32::Globalization::MultiByteToWideChar;
    let cp = if cp == 0 { 0 } else { cp };
    unsafe {
        let need = MultiByteToWideChar(cp, 0, bytes.as_ptr(), bytes.len() as i32, std::ptr::null_mut(), 0);
        if need <= 0 {
            return String::from_utf8_lossy(bytes).to_string();
        }
        let mut wide = vec![0u16; need as usize];
        let got = MultiByteToWideChar(cp, 0, bytes.as_ptr(), bytes.len() as i32, wide.as_mut_ptr(), need);
        if got <= 0 {
            return String::from_utf8_lossy(bytes).to_string();
        }
        String::from_utf16_lossy(&wide[..got as usize])
    }
}

#[cfg(not(windows))]
fn cp_to_string(bytes: &[u8], _cp: u32) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

/// 把 yt-dlp 的英文报错换成「知道该干什么」的中文说明。
/// 原文留着，出问题好排查；只是前面加一句人话，别让用户对着一整段 stack 发呆。
fn friendly_error(raw: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("sign in to confirm") || low.contains("not a bot") {
        return format!(
            "YouTube 要求确认是真人（出口 IP 没登录过会被拦）。解决办法二选一：\n\
             ① 在浏览器里打开这个视频，点浏览器工具栏上的闪电全能下载图标，再点「下载这个视频」——登录信息会自动带过来；\n\
             ② 去「设置 › 视频网站工具 › 站点 Cookie」导入浏览器导出的 cookies.txt。\n\n\
             原文：{raw}"
        );
    }
    if low.contains("only works when logged-in") || low.contains("cookies are needed") || low.contains("login required") {
        return format!(
            "这个网站要登录后才能下载，App 还没拿到登录信息。在浏览器里打开这个视频，点浏览器工具栏上的闪电全能下载图标，再点「下载这个视频」；或者导出 cookies.txt，到「设置 › 视频网站工具 › 站点 Cookie」导入，再点继续。\n\n原文：{raw}"
        );
    }
    if low.contains("video unavailable") || low.contains("this video is private") {
        return format!("这个视频看不了（可能已删除、设为私享，或地区限制）。\n\n原文：{raw}");
    }
    if low.contains("http error 429") || low.contains("too many requests") {
        return format!("网站限流了。可以调低「视频网站同时下载」的数量，等几分钟再点继续。\n\n原文：{raw}");
    }
    if low.contains("winerror 5") || low.contains("winerror 32") {
        return format!("文件被别的程序占着（播放器或杀毒软件），关掉后再点继续。\n\n原文：{raw}");
    }
    if low.contains("unsupported url") {
        return format!("yt-dlp 不认识这个链接。\n\n原文：{raw}");
    }
    if low.contains("connection") && (low.contains("reset") || low.contains("timed out")) {
        return format!("连不上网站，检查网络或代理。\n\n原文：{raw}");
    }
    if low.contains("requested format is not available") {
        return format!("这个清晰度拿不到（可能需要登录或大会员）。\n\n原文：{raw}");
    }
    raw.to_string()
}

fn youtube_subtitle_limiter(link: &str) -> Option<std::sync::MutexGuard<'static, Option<Instant>>> {
    static COOLDOWN: Mutex<Option<Instant>> = Mutex::new(None);
    let url = url::Url::parse(link).ok()?;
    let host = url.host_str()?;
    (host == "youtu.be" || host == "youtube.com" || host.ends_with(".youtube.com"))
        .then(|| COOLDOWN.lock().unwrap_or_else(|error| error.into_inner()))
}

fn queue_subtitle_retry(job: &mut VideoJob) {
    const DELAYS_MS: [u64; 3] = [600_000, 3_600_000, 21_600_000];
    if let Some(delay) = DELAYS_MS.get(job.subtitle_retries) {
        job.subtitle_retry_at = now_ms().saturating_add(*delay);
        if !job.error.contains("自动补字幕") {
            job.error.push_str("，稍后自动补字幕");
        }
    }
}

fn subtitle_rate_limited(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("http error 429") || lower.contains("too many requests") || text.contains("网站限制了字幕请求")
}

fn subtitle_request_error(stderr: &[u8]) -> String {
    let err = String::from_utf8_lossy(stderr);
    if subtitle_rate_limited(&err) {
        return "网站限制了字幕请求，请稍后再试".into();
    }
    let lower = err.to_ascii_lowercase();
    if lower.contains("sign in") || lower.contains("login required") || lower.contains("cookies are needed") {
        return "网站要求登录，请在浏览器登录后用扩展发送一次视频，再点获取字幕".into();
    }
    "网站未返回可下载的字幕，请稍后重试".into()
}

// ---------- 浏览器登录信息（照 Mac 版 SiteCookies.swift） ----------
/// 哪些站需要浏览器里的登录态，以及对应的文件名。抖音网页有签名校验、
/// 小红书不登录只给 5 秒预览，新片场有防爬检查。
pub fn cookie_site(link: &str) -> Option<&'static str> {
    let host = url::Url::parse(link).ok()?.host_str()?.to_lowercase();
    if host.ends_with("douyin.com") || host.ends_with("iesdouyin.com") {
        return Some("douyin");
    }
    if host.ends_with("xiaohongshu.com") || host.ends_with("xhslink.com") {
        return Some("xhs");
    }
    if host.ends_with("xinpianchang.com") {
        return Some("xpc");
    }
    // 这几家也是「不需要登录但要真实浏览器的 Cookie」：抖音/西瓜会直接报
    // Fresh cookies are needed，Vimeo 的网页接口也要求带 Cookie。
    if host.ends_with("ixigua.com") || host.ends_with("toutiao.com") {
        return Some("xigua");
    }
    if host.ends_with("kuaishou.com") {
        return Some("kuaishou");
    }
    if host.ends_with("weibo.com") || host.ends_with("weibo.cn") {
        return Some("weibo");
    }
    if host.ends_with("vimeo.com") {
        return Some("vimeo");
    }
    // YouTube 现在的风控很硬：机房/代理出口基本都会弹「Sign in to confirm you're not a bot」，
    // 带一份登录 Cookie 才能过。文件由设置页的「导入 cookies.txt」放进来。
    if host.ends_with("youtube.com") || host.ends_with("youtu.be") {
        return Some("youtube");
    }
    None
}

/// <数据目录>/cookies-<站点>.txt，yt-dlp 认的 Netscape 格式
pub fn cookie_file(base: &Path, link: &str) -> PathBuf {
    match cookie_site(link) {
        Some(site) => crate::migrate::local_path(base, &format!("cookies-{site}.txt")),
        None => PathBuf::new(),
    }
}

/// 扩展传来的 Cookie 数组写成 yt-dlp 能读的文件：
/// [{domain, path, secure, expires, name, value}]
pub fn save_cookies(base: &Path, link: &str, cookies: &[serde_json::Value]) -> Option<PathBuf> {
    let path = cookie_file(base, link);
    if path.as_os_str().is_empty() || cookies.is_empty() {
        return None;
    }
    let mut out = String::from("# Netscape HTTP Cookie File\n# 闪电全能下载：浏览器扩展带来的登录信息，供 yt-dlp 使用\n");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for c in cookies {
        let (Some(name), Some(value)) = (c["name"].as_str(), c["value"].as_str()) else {
            continue;
        };
        let Some(domain) = c["domain"].as_str() else { continue };
        let domain = if domain.starts_with('.') {
            domain.to_string()
        } else {
            format!(".{domain}")
        };
        let expiry = c["expires"]
            .as_f64()
            .map(|e| e as u64)
            .filter(|e| *e > 0)
            .unwrap_or(now + 86400 * 30);
        let secure = if c["secure"].as_bool().unwrap_or(false) { "TRUE" } else { "FALSE" };
        let path_v = c["path"].as_str().unwrap_or("/");
        out.push_str(&format!("{domain}\tTRUE\t{path_v}\t{secure}\t{expiry}\t{name}\t{value}\n"));
    }
    std::fs::write(&path, out).ok()?;
    Some(path)
}

/// 找 Node.js 的路径。Windows 上常见安装位置 + PATH 里找
fn find_node() -> Option<String> {
    // 常见安装位置优先：PATH 里可能有沙箱/宿主注入的 shim node（.workbuddy、
    // app.asar 下那种），它们的 fs 权限被锁，yt-dlp 拿去解签名会全挂。
    for p in [
        "C:\\Program Files\\nodejs\\node.exe",
        "C:\\Program Files (x86)\\nodejs\\node.exe",
    ] {
        if std::path::Path::new(p).exists() {
            return Some(p.to_string());
        }
    }
    // nvm / fnm 等版本管理器
    if let Ok(home) = std::env::var("USERPROFILE") {
        for sub in [".nvm", ".fnm", "AppData\\Roaming\\nvm"] {
            let dir = std::path::Path::new(&home).join(sub);
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    let p = e.path().join("node.exe");
                    if p.exists() {
                        return Some(p.display().to_string());
                    }
                }
            }
        }
    }
    // PATH 里找（跳过沙箱 shim）
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(';') {
            if dir.contains(".workbuddy") || dir.contains("app.asar") {
                continue;
            }
            let p = std::path::Path::new(dir).join("node.exe");
            if p.exists() {
                return Some(p.display().to_string());
            }
        }
    }
    None
}

// ---------- 字幕语言：先查再下 ----------
//
// 确认框四个按钮存的是语义标签（zh / en / zh+en / all），不是 yt-dlp 正则。
// 站点有哪些字幕代码从主进程落的元数据里读，每个语系只挑一个精确代码传给 --sub-langs，
// 避免 zh.* 同时命中简繁和各种机翻变体，一次落七八个文件、还招来 429。

/// 字幕语言意图
#[derive(Debug, Clone, PartialEq)]
enum SubIntent {
    Zh,
    En,
    ZhEn,
    All,
    /// 用户传了自定义正则（设置里手改），原样透传
    Raw(String),
}

/// 把配置里的 sub_langs（含老正则）归一成意图
fn normalize_sub_intent(s: &str) -> SubIntent {
    let t = s.trim();
    match t {
        "" | "zh" => SubIntent::Zh,
        "en" => SubIntent::En,
        "zh+en" | "zh,en" => SubIntent::ZhEn,
        "all" | ".*" => SubIntent::All,
        // 老配置里的宽正则：按语系意图理解，下载前仍会探测精选
        old if old.contains("zh") && old.contains("en") => SubIntent::ZhEn,
        old if old.contains("en") && !old.contains("zh") => SubIntent::En,
        old if old.contains("zh") => SubIntent::Zh,
        old => SubIntent::Raw(old.to_string()),
    }
}

/// 元数据里的字幕语言代码（人工 + 自动生成的 key 并集）。
/// 文件还在写（解析不了）返回 None，调用方过会儿再读。
fn sub_langs_in_info(bytes: &[u8]) -> Option<Vec<String>> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let mut langs = Vec::new();
    for key in ["subtitles", "automatic_captions"] {
        if let Some(obj) = v.get(key).and_then(|x| x.as_object()) {
            for k in obj.keys() {
                if !langs.iter().any(|x: &String| x == k) {
                    langs.push(k.clone());
                }
            }
        }
    }
    Some(langs)
}

/// 开了润色时，这个视频该不该只下原文来翻译，下哪条。有人工中文就不用翻。
/// 其他语言总从原文翻，不走机翻中文或英文中转；英文要看设置
fn translate_source(info: &[u8], avail: &[String], from_english: bool) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(info).ok()?;
    let human_zh = v["subtitles"].as_object().is_some_and(|m| m.keys().any(|k| k.to_ascii_lowercase().starts_with("zh")));
    if human_zh {
        return None;
    }
    // 视频语言：yt-dlp 给的，没有就看哪条自动字幕标了 -orig
    let lang = v["language"]
        .as_str()
        .map(|l| l.split('-').next().unwrap_or(l).to_ascii_lowercase())
        .or_else(|| avail.iter().find_map(|a| a.strip_suffix("-orig").map(|l| l.to_ascii_lowercase())))?;
    if lang == "zh" {
        return None;
    }
    if lang == "en" && !from_english {
        return None;
    }
    let orig = format!("{lang}-orig");
    avail
        .iter()
        .find(|a| a.eq_ignore_ascii_case(&orig))
        .or_else(|| avail.iter().find(|a| a.eq_ignore_ascii_case(&lang)))
        .or_else(|| avail.iter().find(|a| a.to_ascii_lowercase().starts_with(&format!("{lang}-"))))
        .cloned()
}

/// 新建下载框预查的结果：有没有字幕、实际有哪些清晰度（从高到低）
#[derive(Debug, Serialize)]
pub struct VideoProbe {
    pub title: String,
    pub subs: bool,
    pub heights: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

fn exact_selected_video_size(info: &serde_json::Value) -> Option<u64> {
    let selected = match info["requested_downloads"].as_array() {
        Some(downloads) => match downloads.as_slice() {
            [one] => one,
            _ => return None,
        },
        None => info,
    };
    let exact = |format: &serde_json::Value| format["filesize"].as_u64().filter(|size| *size > 0);
    if let Some(parts) = selected["requested_formats"].as_array().filter(|parts| !parts.is_empty()) {
        parts.iter().try_fold(0u64, |total, part| total.checked_add(exact(part)?))
    } else {
        exact(selected)
    }
}

fn heights_in_info(bytes: &[u8]) -> Vec<u32> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) else { return Vec::new() };
    let mut out: Vec<u32> = v["formats"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        // 只音频和封面图（mhtml 故事板）不算清晰度
        .filter(|f| f["vcodec"].as_str() != Some("none") && f["ext"].as_str() != Some("mhtml"))
        .filter_map(|f| f["height"].as_u64())
        .filter(|h| *h >= 144)
        .map(|h| h as u32)
        .collect();
    out.sort_unstable_by(|a, b| b.cmp(a));
    out.dedup();
    out
}

fn probe_metadata(local: &Path, bin_dir: &Path, url: &str, args: &[&str]) -> Option<Vec<u8>> {
    // Keep stdout off a pipe while waiting for the process, including for long lists.
    let out = std::env::temp_dir().join(format!("shandian-video-probe-{:016x}.json", rand::random::<u64>()));
    let file = std::fs::File::create_new(&out).ok()?;
    let mut cmd = Command::new(ytdlp_path(bin_dir));
    cmd.args(args);
    let ck = cookie_file(local, url);
    if ck.is_file() {
        cmd.arg("--cookies").arg(ck);
    }
    for a in crate::proxy::ytdlp_args(url) {
        cmd.arg(a);
    }
    if let Some(node) = find_node() {
        cmd.arg("--js-runtimes").arg(format!("node:{node}"));
    }
    cmd.arg("--").arg(url)
        .env_remove("NODE_OPTIONS")
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let result = (|| {
        let mut child = cmd.spawn().ok()?;
        drop(cmd);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
        loop {
            match child.try_wait() {
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Ok(Some(st)) if st.success() => break,
                _ => {
                    kill_tree(&mut child);
                    return None;
                }
            }
        }
        std::fs::read(&out).ok()
    })();
    let _ = std::fs::remove_file(&out);
    result
}

/// 新建下载框用：这个视频的字幕和清晰度。查不出来（超时、要登录、网络问题）返回 None，不下结论
pub fn probe_video(local: &Path, bin_dir: &Path, url: &str, video: Option<&VideoOptions>) -> Option<VideoProbe> {
    let mut args = vec![
        "--dump-single-json", "--skip-download", "--no-warnings", "--no-playlist", "--ignore-no-formats-error",
        "--write-subs", "--write-auto-subs", "--sub-langs", "__none__",
    ];
    let format = video.map(VideoOptions::format_arguments).unwrap_or_default();
    args.extend(format.iter().map(String::as_str));
    let bytes = probe_metadata(local, bin_dir, url, &args)?;
    let langs = sub_langs_in_info(&bytes)?;
    let info: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some(VideoProbe {
        title: info["title"].as_str().unwrap_or_default().to_owned(),
        subs: langs.iter().any(|l| l != "danmaku" && l != "live_chat"),
        heights: heights_in_info(&bytes),
        size: video.and_then(|_| exact_selected_video_size(&info)),
    })
}

#[derive(Serialize)]
pub struct VideoCollection {
    title: String,
    items: Vec<CollectionItem>,
    unavailable: usize,
}

#[derive(Serialize)]
struct CollectionItem {
    url: String,
    name: String,
}

fn collection_items(info: &serde_json::Value, site: crate::links::CollectionSite) -> Option<VideoCollection> {
    use crate::links::CollectionSite;
    let entries = info["entries"].as_array()?;
    let mut seen = std::collections::HashSet::new();
    let mut unavailable = 0;
    let mut items = Vec::new();
    for (index, item) in entries.iter().enumerate() {
        let url = match site {
            CollectionSite::YouTube => item["id"].as_str().filter(|id| {
                id.len() == 11 && id.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
                    && item["_type"].as_str() != Some("playlist")
            }).map(|id| format!("https://www.youtube.com/watch?v={id}")),
            CollectionSite::Bilibili => item["url"].as_str().and_then(|link| {
                let url = url::Url::parse(link).ok()?;
                let segments: Vec<_> = url.path().trim_matches('/').split('/').collect();
                let ["video", id] = segments.as_slice() else { return None; };
                if !matches!(url.scheme(), "http" | "https")
                    || !matches!(url.host_str(), Some("www.bilibili.com" | "bilibili.com" | "m.bilibili.com"))
                    || !(id.starts_with("BV") || id.starts_with("av")) || !id.bytes().all(|c| c.is_ascii_alphanumeric())
                {
                    return None;
                }
                let mut link = format!("https://www.bilibili.com/video/{id}");
                if let Some((_, part)) = url.query_pairs().find(|(k, _)| k == "p") {
                    link.push_str(&format!("?p={}", part.parse::<u32>().ok().filter(|p| *p > 0)?));
                }
                Some(link)
            }),
        };
        let Some(url) = url else { unavailable += 1; continue; };
        if !seen.insert(crate::links::canonical_url(&url)) {
            continue;
        }
        let title = item["title"].as_str().filter(|s| !s.trim().is_empty())
            .or_else(|| item["id"].as_str()).unwrap_or("视频");
        items.push(CollectionItem { url, name: format!("{:02}. {title}", index + 1) });
    }
    let title = info["title"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or("视频合集").into();
    Some(VideoCollection { title, items, unavailable })
}

fn bili_api(client: &reqwest::blocking::Client, link: &str, endpoint: &str, query: &[(&str, &str)]) -> Option<serde_json::Value> {
    let value: serde_json::Value = client.get(format!("https://api.bilibili.com{endpoint}"))
        .header("Referer", link).query(query).send().ok()?.error_for_status().ok()?.json().ok()?;
    (value["code"].as_i64()? == 0).then(|| value["data"].clone())
}

/// The flat Bilibili extractor often returns only IDs. The list APIs include titles
/// alongside those IDs, so there is no need to open each video to name it.
fn bili_collection_info(link: &str, deadline: Instant) -> Option<serde_json::Value> {
    use serde_json::json;
    let url = url::Url::parse(link).ok()?;
    let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
    let client = reqwest::blocking::Client::builder().no_proxy()
        .timeout(std::time::Duration::from_secs(6)).user_agent("Mozilla/5.0").build().ok()?;
    if let ["video", id] = parts.as_slice() {
        let (key, value) = id.strip_prefix("av").map_or(("bvid", *id), |aid| ("aid", aid));
        let data = bili_api(&client, link, "/x/web-interface/view", &[(key, value)])?;
        let id = data["bvid"].as_str()?;
        let pages = data["pages"].as_array()?;
        if pages.len() < 2 { return None; }
        let entries = pages.iter().filter_map(|page| {
            let part = page["page"].as_u64()?;
            Some(json!({
                "url": format!("https://www.bilibili.com/video/{id}?p={part}"),
                "title": page["part"],
            }))
        }).collect::<Vec<_>>();
        return Some(json!({ "title": data["title"], "entries": entries }));
    }
    let sid_query = url.query_pairs().find(|(key, _)| key == "sid").map(|(_, v)| v);
    let series = url.query_pairs().any(|(key, v)| key == "type" && v == "series")
        || parts.last() == Some(&"seriesdetail");
    let (mid, sid) = match (url.host_str()?, parts.as_slice()) {
        ("space.bilibili.com", [mid, "lists", sid]) => (*mid, *sid),
        ("space.bilibili.com", [mid, "channel", "collectiondetail" | "seriesdetail"]) => (*mid, sid_query.as_deref()?),
        _ => return None,
    };
    let (endpoint, sid_key, page_key, size_key) = if series {
        ("/x/series/archives", "series_id", "pn", "ps")
    } else {
        ("/x/polymer/web-space/seasons_archives_list", "season_id", "page_num", "page_size")
    };
    let mut entries = Vec::new();
    let mut page = 1;
    let mut title = if series {
        bili_api(&client, link, "/x/series/series", &[("series_id", sid)])
            .and_then(|data| data["meta"]["name"].as_str().map(str::to_owned))
    } else { None };
    while Instant::now() < deadline {
        let data = bili_api(&client, link, endpoint, &[
            ("mid", mid), (sid_key, sid), (page_key, &page.to_string()), (size_key, "30"),
        ])?;
        if title.is_none() {
            title = data["meta"]["name"].as_str().map(str::to_owned);
        }
        let archives = data["archives"].as_array()?;
        entries.extend(archives.iter().filter_map(|entry| {
            let id = entry["bvid"].as_str()?;
            Some(json!({ "url": format!("https://www.bilibili.com/video/{id}"), "id": id, "title": entry["title"] }))
        }));
        if archives.is_empty() || page * 30 >= data["page"]["total"].as_u64()? {
            return Some(json!({ "title": title?, "entries": entries }));
        }
        page += 1;
    }
    None
}

fn bili_medialist_titles(link: &str, info: &mut serde_json::Value) {
    let Some((kind, id)) = info["id"].as_str().and_then(|id| id.split_once('_')) else { return; };
    let (kind, id) = (kind.to_owned(), id.to_owned());
    let Some(entries) = info["entries"].as_array_mut() else { return; };
    let mut names = HashMap::new();
    let Ok(client) = reqwest::blocking::Client::builder().no_proxy().user_agent("Mozilla/5.0")
        .timeout(std::time::Duration::from_secs(5)).build() else { return; };
    let deadline = Instant::now() + std::time::Duration::from_secs(12);
    let mut offset = "0".to_string();
    while Instant::now() < deadline {
        let Some(data) = bili_api(&client, link, "/x/v2/medialist/resource/list", &[
            ("type", &kind), ("biz_id", &id), ("ps", "20"), ("with_current", "false"), ("oid", &offset),
        ]) else { break; };
        let Some(page) = data["media_list"].as_array() else { break; };
        for item in page {
            if let (Some(id), Some(title)) = (item["bv_id"].as_str(), item["title"].as_str()) {
                names.insert(id.to_owned(), title.to_owned());
            }
        }
        let Some(next) = page.last().and_then(|item| item["id"].as_u64()).map(|id| id.to_string()) else { break; };
        if offset == next || data["has_more"].as_bool() != Some(true) { break; }
        offset = next;
    }
    for entry in entries {
        if let Some(name) = entry["id"].as_str().and_then(|id| names.get(id)) {
            entry["title"] = name.clone().into();
        }
    }
}

pub fn probe_collection(local: &Path, bin_dir: &Path, link: &str) -> Result<VideoCollection, String> {
    use crate::links::CollectionSite;
    let site = crate::links::video_collection(link, true).ok_or("这不是支持的合集链接")?;
    let bin = ytdlp_path(bin_dir);
    if !bin.is_file() {
        return Err("请先在「设置 › 视频网站工具」安装 yt-dlp".into());
    }
    let mut page = url::Url::parse(link).map_err(|_| "合集链接无效")?;
    if site == CollectionSite::Bilibili && page.host_str() == Some("m.bilibili.com") {
        let _ = page.set_host(Some("www.bilibili.com"));
    }
    if site == CollectionSite::Bilibili && page.path().starts_with("/video/") {
        // yt-dlp treats ?p= as an instruction to extract just that part, even with --yes-playlist.
        page.set_query(None);
        let _ = page.set_host(Some("www.bilibili.com"));
    }
    let link = page.as_str();
    if site == CollectionSite::Bilibili {
        let deadline = Instant::now() + std::time::Duration::from_secs(15);
        if let Some(result) = bili_collection_info(link, deadline).and_then(|info| collection_items(&info, site)) {
            return Ok(result);
        }
    }
    let bytes = probe_metadata(local, bin_dir, link, &[
        "--ignore-config", "--dump-single-json", "--skip-download", "--flat-playlist", "--yes-playlist", "--no-warnings",
    ]).ok_or("读取合集失败，请确认网页能打开后重试")?;
    let mut info = serde_json::from_slice(&bytes).map_err(|_| "未能读取合集数据")?;
    if site == CollectionSite::Bilibili && (page.path().starts_with("/list/") || page.path().starts_with("/medialist/play/")) {
        bili_medialist_titles(link, &mut info);
    }
    collection_items(&info, site).ok_or_else(|| "这个页面没有可列出的合集".into())
}

#[cfg(test)]
mod collection_tests {
    use super::*;
    use crate::links::{video_collection, CollectionSite};
    use serde_json::json;

    #[test]
    fn playlist_pages_are_distinct_from_playback() {
        let watch = "https://www.youtube.com/watch?v=6dBlxB1pBa8&list=UUz75RVbH8q2jdBJ4SnwuZZQ";
        assert!(video_collection(watch, false).is_none());
        assert!(video_collection("https://www.youtube.com/playlist?list=UUz75RVbH8q2jdBJ4SnwuZZQ", false) == Some(CollectionSite::YouTube));
        assert!(video_collection("https://notyoutube.com/playlist?list=UUz75RVbH8q2jdBJ4SnwuZZQ", true).is_none());
        let part = "https://www.bilibili.com/video/BV1bK411W797?p=2";
        assert!(video_collection(part, false).is_none());
        assert!(video_collection(part, true) == Some(CollectionSite::Bilibili));
        assert!(video_collection("https://www.bilibili.com/list/1958703906?sid=547718&bvid=BV1bK411W797", false).is_none());
    }

    #[test]
    fn normalized_entries_keep_bilibili_parts_and_isolate_youtube_videos() {
        let bili = json!({"title":"合集","entries":[
            {"url":"https://www.bilibili.com/video/BV1bK411W797?p=1","title":"第一集"},
            {"url":"https://www.bilibili.com/video/BV1bK411W797?p=2&tracking=foo","title":"第二集"},
            {"url":"https://www.bilibili.com/video/BV1bK411W797?p=2"},
            {"url":"https://bilibili.com.example/video/BV1bK411W797?p=3"},
        ]});
        let result = collection_items(&bili, CollectionSite::Bilibili).unwrap();
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.items[1].url, "https://www.bilibili.com/video/BV1bK411W797?p=2");
        assert_eq!(result.items[1].name, "02. 第二集");
        assert_eq!(result.unavailable, 1);
        let youtube = json!({"entries":[{"id":"6dBlxB1pBa8","url":"https://www.youtube.com/watch?v=6dBlxB1pBa8&list=UUlist"}]});
        assert_eq!(collection_items(&youtube, CollectionSite::YouTube).unwrap().items[0].url, "https://www.youtube.com/watch?v=6dBlxB1pBa8");
    }
}

fn merged_temp_path(dir: &Path, vid: &str, suffix: &str) -> Option<PathBuf> {
    if vid.is_empty() {
        return None;
    }
    let token = format!("[{vid}]{suffix}.temp.");
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        p.file_name().is_some_and(|n| n.to_string_lossy().contains(&token))
    })
}

/// 哪些程序占着这个文件（系统的 Restart Manager，只读查询，不会动它们）。查不出来就是空的
#[cfg(windows)]
pub fn file_lockers(path: &Path) -> Vec<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::RestartManager::{
        RmEndSession, RmGetList, RmRegisterResources, RmStartSession, RM_PROCESS_INFO,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut out = Vec::new();
    unsafe {
        let mut session = 0u32;
        let mut key = [0u16; 33];
        if RmStartSession(&mut session, 0, key.as_mut_ptr()) != 0 {
            return out;
        }
        let files = [wide.as_ptr()];
        if RmRegisterResources(session, 1, files.as_ptr(), 0, std::ptr::null(), 0, std::ptr::null()) == 0 {
            let mut infos: [RM_PROCESS_INFO; 8] = std::mem::zeroed();
            let (mut needed, mut count, mut reasons) = (0u32, infos.len() as u32, 0u32);
            if RmGetList(session, &mut needed, &mut count, infos.as_mut_ptr(), &mut reasons) == 0 {
                for p in &infos[..count as usize] {
                    let len = p.strAppName.iter().position(|c| *c == 0).unwrap_or(p.strAppName.len());
                    let name = String::from_utf16_lossy(&p.strAppName[..len]);
                    if !name.is_empty() && !out.contains(&name) {
                        out.push(name);
                    }
                }
            }
        }
        RmEndSession(session);
    }
    out
}

/// 把合并好的 .temp 改成成品名，再删掉用完的分轨（f313.webm 这些）。还被占着或没有就返回 None
fn land_merged_temp(dir: &Path, vid: &str, suffix: &str) -> Option<PathBuf> {
    if vid.is_empty() {
        return None;
    }
    let token = format!("[{vid}]{suffix}.");
    let names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains(&token))
        .collect();
    let temp = names.iter().find(|n| n.contains(&format!("{token}temp.")))?;
    let target = dir.join(temp.replacen(&format!("{token}temp."), &token, 1));
    if target.exists() {
        return None;
    }
    std::fs::rename(dir.join(temp), &target).ok()?;
    let stream = format!("{token}f");
    for n in names.iter().filter(|n| {
        n.split_once(&stream).is_some_and(|(_, rest)| rest.split('.').next().is_some_and(|f| f.chars().all(|c| c.is_ascii_digit()) && !f.is_empty()))
    }) {
        let _ = std::fs::remove_file(dir.join(n));
    }
    Some(target)
}

/// 机翻变体 <前缀>-<原语言>（zh-Hans-ja = 从日语机翻成简中）：优先从视频原语言翻的，
/// 同时存在 zh-Hans-zh 和 zh-Hans-ja 时 zh 那条才贴合这个视频
fn translated_from_original(prefix: &str, available: &[String]) -> Option<String> {
    let lead = format!("{prefix}-");
    let cands: Vec<&String> = available
        .iter()
        .filter(|a| a.to_ascii_lowercase().starts_with(&lead))
        .collect();
    cands
        .iter()
        .find(|a| {
            let orig = &a[lead.len()..];
            available.iter().any(|x| x.as_str() == orig)
        })
        .or(cands.first())
        .map(|a| a.to_string())
}

/// 挑一个中文代码：简体优先，其次通用 zh，再次 ai-zh，最后才接受繁体
fn pick_zh(available: &[String]) -> Option<String> {
    for p in ["zh-Hans", "zh-CN", "zh-cn", "zh"] {
        if available.iter().any(|a| a == p) {
            return Some(p.to_string());
        }
    }
    if let Some(a) = translated_from_original("zh-hans", available) {
        return Some(a);
    }
    // 非繁体的任意 zh-*
    for a in available {
        let l = a.to_ascii_lowercase();
        if l.starts_with("zh")
            && !l.contains("hant")
            && !l.contains("tw")
            && !l.contains("hk")
            && !l.contains("th")
            && !l.contains("cht")
        {
            return Some(a.clone());
        }
    }
    if available.iter().any(|a| a == "ai-zh") {
        return Some("ai-zh".into());
    }
    // 只剩繁体也比没有强
    available
        .iter()
        .find(|a| a.to_ascii_lowercase().starts_with("zh"))
        .cloned()
}

/// 挑一个英文代码：人工 en 优先于自动生成 en-orig
fn pick_en(available: &[String]) -> Option<String> {
    if available.iter().any(|a| a == "en") {
        return Some("en".into());
    }
    for p in ["en-US", "en-GB", "en-CA", "en-AU"] {
        if available.iter().any(|a| a == p) {
            return Some(p.to_string());
        }
    }
    // 中英合并时两条要对得上：有中文就优先从中文翻的英文，别拿另一种语言转译的
    if available.iter().any(|a| a == "zh") && available.iter().any(|a| a == "en-zh") {
        return Some("en-zh".into());
    }
    if let Some(a) = translated_from_original("en", available).filter(|a| !a.ends_with("orig")) {
        if a[3..].chars().count() <= 3 {
            return Some(a);
        }
    }
    // 其它 en-*，但跳过 en-orig（自动生成的「原始语言」字幕）
    for a in available {
        let l = a.to_ascii_lowercase();
        if l.starts_with("en") && !l.contains("orig") {
            return Some(a.clone());
        }
    }
    // 实在没有才接受 en-orig
    available
        .iter()
        .find(|a| a.to_ascii_lowercase().starts_with("en"))
        .cloned()
}

/// 按意图从可用代码里精选。站点一条字幕都没有就返回空，不用再跑一趟
fn pick_sub_langs(intent: &SubIntent, available: &[String]) -> String {
    if available.is_empty() {
        return String::new();
    }
    match intent {
        SubIntent::All => ".*".into(),
        SubIntent::Raw(s) => s.clone(),
        SubIntent::Zh => pick_zh(available).or_else(|| pick_en(available)).unwrap_or_default(),
        SubIntent::En => pick_en(available).unwrap_or_default(),
        SubIntent::ZhEn => [pick_zh(available), pick_en(available)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(","),
    }
}

pub(crate) fn subtitle_file_label(video: &Path) -> Option<&'static str> {
    let prefix = format!("{}.", video.file_stem()?.to_str()?);
    let mut found = false;
    let (mut zh, mut en) = (false, false);
    for path in crate::video_files::sidecars(video) {
        let Some(tail) = path.file_name().and_then(|name| name.to_str()).and_then(|name| name.strip_prefix(&prefix)) else { continue };
        if !crate::video_files::is_subtitle_tail(tail) { continue; }
        found = true;
        let (language, _) = tail.rsplit_once('.').unwrap_or(("", tail));
        let lower = language.to_ascii_lowercase();
        zh |= lower == "zh" || lower.starts_with("zh-") || lower.starts_with("zh_") || lower == "ai-zh";
        en |= lower == "en" || lower.starts_with("en-") || lower.starts_with("en_") || lower == "zh-en";
    }
    match (zh, en, found) {
        (true, true, _) => Some("中英字幕"),
        (true, false, _) => Some("中文字幕"),
        (false, true, _) => Some("英文字幕"),
        (false, false, true) => Some("有字幕"),
        _ => None,
    }
}

/// 下载完成后的字幕收尾：去重 → （中英时）合并 → 嵌进视频。
/// 三步都只在这儿做，别散在别处。
fn finish_subs(video: &Path, opts: &VideoOptions, bin_dir: &Path, polish_on: bool, align_by_id: bool) -> Result<(), String> {
    if align_by_id { align_subtitles(video)?; }
    crate::engine::dedupe_subs(video);
    if polish_on || matches!(normalize_sub_intent(&opts.sub_langs), SubIntent::ZhEn) {
        crate::engine::merge_bilingual_subs(
            video,
            opts.sub_scale,
            opts.sub_colorize,
            &opts.sub_color_zh,
            &opts.sub_color_en,
        );
    }
    // 阴影、黑底不是默认值时，按选的样式把合出来的 ass 再写一遍
    // 只有偏离默认样式时才重写（默认样式就是合并时写出来的那套）
    let d = VideoOptions { sub_scale: opts.sub_scale, sub_colorize: opts.sub_colorize, sub_color_zh: opts.sub_color_zh.clone(), sub_color_en: opts.sub_color_en.clone(), ..VideoOptions::default() };
    if opts.look() != d.look() {
        if let Some(sub) = crate::polish::find_sub(video).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ass"))) {
            let _ = restyle_file(&sub, &opts.look());
        }
    }
    embed_sub(video, bin_dir);
    Ok(())
}

/// 使用视频的真实文件名；只匹配同一视频 ID 和副本后缀，保留语言、格式及留底后缀。
pub(crate) fn align_subtitles(video: &Path) -> Result<(), String> {
    let stem = video.file_stem().and_then(|s| s.to_str()).ok_or("视频文件名无效")?;
    let Some(at) = stem.rfind('[') else { return Ok(()) };
    let mark = format!("{}.", &stem[at..]);
    let dir = video.parent().ok_or("视频目录无效")?;
    let entries = std::fs::read_dir(dir).map_err(|e| format!("无法读取字幕目录：{e}"))?;
    for e in entries.flatten() {
        let src = e.path();
        if !src.is_file() { continue }
        let name = e.file_name().to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        let subtitle = ["ass", "srt", "vtt", "ssa"].iter().any(|ext| [format!(".{ext}"), format!(".{ext}.bak"), format!(".{ext}.original.bak"), format!(".{ext}.original_bak")].iter().any(|suffix| lower.ends_with(suffix)));
        if !subtitle { continue }
        let Some((_, rest)) = name.rsplit_once(&mark) else { continue };
        let dst = dir.join(format!("{stem}.{rest}"));
        if src == dst { continue }
        if dst.exists() {
            if let (Ok(a), Ok(b)) = (std::fs::read(&src), std::fs::read(&dst)) { if a == b { continue } }
            return Err("字幕名称冲突，已保留原文件，请检查同名字幕".into());
        }
        std::fs::rename(&src, &dst).map_err(|e| format!("无法对齐字幕文件名：{e}"))?;
    }
    Ok(())
}

/// 样式入口优先使用当前成品；SRT/VTT 转为 ASS 后保留原格式留底。
pub(crate) fn restyle_subtitle(video: &Path, look: &crate::engine::SubLook) -> Result<PathBuf, String> {
    restyle_matching_subtitle(video, look, true)
}

fn restyle_matching_subtitle(video: &Path, look: &crate::engine::SubLook, align_by_id: bool) -> Result<PathBuf, String> {
    use std::io::Write;
    if align_by_id { align_subtitles(video)?; }
    let stem = video.file_stem().ok_or("视频文件名无效")?.to_string_lossy().to_lowercase();
    let prefix = format!("{stem}.");
    let dir = video.parent().ok_or("视频目录无效")?;
    let mut candidates: Vec<_> = std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten().filter_map(|e| {
        let p = e.path();
        if !p.is_file() { return None }
        let name = e.file_name().to_string_lossy().to_lowercase();
        let rest = name.strip_prefix(&prefix)?;
        let ext = p.extension()?.to_string_lossy().to_ascii_lowercase();
        if !["ass", "srt", "vtt"].contains(&ext.as_str()) { return None }
        let lang = if rest.starts_with("zh-en.") { 0 } else if rest.starts_with("zh") { 1 } else { 2 };
        Some(((lang, ext != "ass"), p))
    }).collect();
    candidates.sort();
    let sub = candidates.into_iter().next().map(|(_, p)| p).ok_or("这个视频没有可用的字幕文件")?;
    if sub.extension().is_some_and(|e| e.eq_ignore_ascii_case("ass")) {
        restyle_file(&sub, look)?;
        return Ok(sub);
    }
    let cues: Vec<_> = crate::engine::parse_subs(&sub).into_iter().filter(|c| c.end > c.start).collect();
    if cues.is_empty() { return Err("字幕内容无法解析，未转换格式".into()) }
    let (mut text, _, _) = crate::engine::subtitle_style(1.0, false, "", "");
    for c in cues {
        if c.end <= c.start { continue }
        text.push_str(&format!("Dialogue: 0,{},{},Default,,0,0,0,,{}\n", crate::engine::ass_time(c.start), crate::engine::ass_time(c.end), c.text.replace('\n', "\\N")));
    }
    let target = sub.with_extension("ass");
    let backup = crate::polish::backup_of(&sub);
    if backup.exists() { return Err("原字幕已有留底，请检查后重试；未覆盖任何文件".into()) }
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&target).map_err(|e| format!("无法创建 ASS 字幕：{e}"))?;
    if let Err(e) = file.write_all(crate::engine::restyle_ass(&text, look).as_bytes()) {
        drop(file);
        let _ = std::fs::remove_file(&target);
        return Err(format!("无法写入 ASS 字幕：{e}"));
    }
    drop(file);
    std::fs::rename(&sub, &backup).map_err(|e| format!("ASS 已生成，但原字幕留底失败：{e}"))?;
    Ok(target)
}

/// 按样式重写一份 ass 字幕；润色留底也一起改，之后重新润色不会把样式改回去
pub(crate) fn restyle_file(sub: &Path, look: &crate::engine::SubLook) -> Result<(), String> {
    let text = std::fs::read_to_string(sub).map_err(|e| format!("读不了字幕：{e}"))?;
    std::fs::write(sub, crate::engine::restyle_ass(&text, look)).map_err(|e| format!("写不了字幕：{e}"))?;
    let backup = crate::polish::backup_of(sub);
    if let Ok(t) = std::fs::read_to_string(&backup) {
        let _ = std::fs::write(&backup, crate::engine::restyle_ass(&t, look));
    }
    Ok(())
}

/// 把剩下的那一份字幕嵌进视频，始终只有一条轨。
///
/// 之前用 yt-dlp 的 --embed-subs，会把 zh-Hans / zh-Hant / en / en-orig 全嵌进去，
/// 播放器的字幕菜单里就是四条「zho · mov_text / eng · mov_text」。
pub(crate) fn embed_sub(video: &Path, bin_dir: &Path) {
    let ff = bin_dir.join("ffmpeg.exe");
    if !ff.exists() {
        return;
    }
    let Some(stem) = video.file_stem().map(|s| s.to_string_lossy().to_string()) else {
        return;
    };
    let Some(parent) = video.parent() else { return };
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    let prefix = format!("{}.", stem.to_lowercase());

    let mut cands: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            let name = p.file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            let Some(rest) = name.strip_prefix(&prefix) else { return false };
            crate::video_files::is_subtitle_tail(rest) && ["srt", "vtt", "ass", "ssa"].iter().any(|e| rest.ends_with(&format!(".{e}")))
        })
        .collect();
    if cands.is_empty() {
        return;
    }
    // 合并版优先，其次按文件名
    cands.sort_by(|a, b| {
        let ka = a.to_string_lossy().contains("zh-en");
        let kb = b.to_string_lossy().contains("zh-en");
        kb.cmp(&ka).then_with(|| a.cmp(b))
    });
    let sub = &cands[0];

    let tmp = parent.join(format!("{stem}.embed.tmp.mp4"));
    let _ = std::fs::remove_file(&tmp);
    let mut cmd = Command::new(&ff);
    cmd.args(["-y", "-loglevel", "error", "-i"]).arg(video);
    cmd.arg("-i").arg(sub);
    cmd.args([
        // 只留视频和音频，重新塞一条字幕：老文件里那些重复轨也一并不见了
        "-map", "0:v?", "-map", "0:a?", "-map", "1:0",
        // 元数据跟着原件走（标题、作者、原始网址），别在重封装时丢掉
        "-map_metadata", "0",
        "-c:v", "copy", "-c:a", "copy", "-c:s", "mov_text",
        "-metadata:s:s:0", "language=chi",
        // 不设为默认：旁边的 .ass 带着色，两条都开就叠着显示。视频拷走时可以手动打开这条
        "-disposition:s:0", "0",
    ]);
    cmd.arg(&tmp);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let ok = cmd.status().map(|s| s.success()).unwrap_or(false);
    if !ok || !tmp.exists() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    // 换文件：先把原件改成 .old，再换上新的；换失败就还原，绝不出现「两个都没了」
    let bak = parent.join(format!("{stem}.old.mp4"));
    let _ = std::fs::remove_file(&bak);
    if std::fs::rename(video, &bak).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    if std::fs::rename(&tmp, video).is_err() {
        let _ = std::fs::rename(&bak, video);
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    let _ = std::fs::remove_file(&bak);
}


#[cfg(test)]
mod tests {
    #[test]
    fn subtitle_wait_wakes_the_video_queue() {
        use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
        use std::time::Duration;
        let dir = std::env::temp_dir().join(format!("sd-video-queue-{:016x}", rand::random::<u64>()));
        std::fs::create_dir(&dir).unwrap();
        let mut settings = crate::settings::Settings::default();
        settings.max_videos = 1;
        settings.save(&dir).unwrap();
        // Deliberately invalid local executable, so the test can observe a launch
        // attempt without depending on yt-dlp or making any network requests.
        std::fs::write(dir.join("yt-dlp.exe"), b"fixture").unwrap();
        let media = dir.join("Finished [fixture].mp4");
        std::fs::write(&media, b"video").unwrap();
        let queue = Arc::new(super::Videos::new(&dir, &dir));
        let mut first = super::VideoJob::new(
            "https://www.youtube.com/watch?v=fixture", &dir, super::VideoOptions::default(),
        );
        first.state = "running".into();
        first.options.subtitles = true;
        first.options.sub_langs = "zh".into();
        first.file_path = media.display().to_string();
        let mut next = super::VideoJob::new(
            "https://www.youtube.com/watch?v=next", &dir, super::VideoOptions::default(),
        );
        next.state = "wait".into();
        let (first_id, next_id) = (first.id.clone(), next.id.clone());
        *queue.jobs.lock().unwrap() = vec![first, next];
        *queue.launching.lock().unwrap() = Some(dir.clone());
        let subtitle_ready = Arc::new(AtomicBool::new(false));
        queue.subs_done.lock().unwrap().insert(first_id.clone(), Arc::clone(&subtitle_ready));
        let gen = queue.next_gen(&first_id);
        let runner = Arc::clone(&queue);
        let (id, bin) = (first_id.clone(), dir.clone());
        let finish = std::thread::spawn(move || runner.finish(&id, 0, gen, &bin));
        let launched_during_subtitles = (0..120).any(|_| {
            let finishing = queue.find(&first_id).unwrap();
            let waiting = queue.find(&next_id).unwrap();
            if finishing.state == "running" && finishing.stage == "处理字幕"
                && waiting.state == "error" && waiting.error.starts_with("启动 yt-dlp 失败")
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
            false
        });
        subtitle_ready.store(true, Ordering::Relaxed);
        finish.join().unwrap();
        assert!(launched_during_subtitles);
        assert_eq!(queue.find(&first_id).unwrap().state, "done");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn subtitle_alignment_keeps_language_backup_and_copy_identity() {
        let dir = std::env::temp_dir().join(format!("sd-sub-align-{}-{}",std::process::id(),super::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("Actual ｜ title [id]_1.mp4");
        let old = dir.join("Longer ｜ raw title [id]_1.zh-en.ass");
        std::fs::write(&old,"subtitle").unwrap();
        std::fs::write(dir.join("Longer ｜ raw title [id]_1.zh-en.ass.bak"),"backup").unwrap();
        std::fs::write(dir.join("Other [id].en.vtt"),"other copy").unwrap();
        super::align_subtitles(&video).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("Actual ｜ title [id]_1.zh-en.ass")).unwrap(),"subtitle");
        assert_eq!(std::fs::read_to_string(dir.join("Actual ｜ title [id]_1.zh-en.ass.bak")).unwrap(),"backup");
        assert!(dir.join("Other [id].en.vtt").is_file());
        assert!(!old.exists());
        std::fs::write(&old,"conflict").unwrap();
        assert!(super::align_subtitles(&video).is_err());
        assert_eq!(std::fs::read_to_string(&old).unwrap(),"conflict");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn subtitle_style_converts_srt_and_vtt_without_changing_cues() {
        let dir = std::env::temp_dir().join(format!("sd-sub-convert-{}-{}",std::process::id(),super::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut look = super::VideoOptions::default().look();
        look.font = "Test Font".into();
        for (ext,text) in [("srt","1\n00:00:01,230 --> 00:00:04,560\n你好\nhello\n\n"),("vtt","WEBVTT\n\n00:00:01.230 --> 00:00:04.560\n<c>你好</c>\nhello\n\n")] {
            let video = dir.join(format!("Lesson [{ext}].mp4"));
            let source = dir.join(format!("Lesson [{ext}].zh-en.{ext}"));
            std::fs::write(&source,text).unwrap();
            let ass = super::restyle_subtitle(&video,&look).unwrap();
            let converted = std::fs::read_to_string(&ass).unwrap();
            assert!(converted.contains("Style: Default,Test Font,"));
            assert!(converted.contains("0:00:01.23,0:00:04.56"));
            assert!(converted.contains("你好\\N"));
            assert!(converted.contains("hello"));
            assert!(!source.exists());
            assert_eq!(std::fs::read_to_string(crate::polish::backup_of(&source)).unwrap(),text);
            super::restyle_subtitle(&video,&look).unwrap();
        }
        let bad = dir.join("Bad [bad].en.srt");
        std::fs::write(&bad,"not a subtitle").unwrap();
        assert!(super::restyle_subtitle(&dir.join("Bad [bad].mp4"),&look).is_err());
        assert!(bad.is_file());
        assert!(!bad.with_extension("ass").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
    use super::*;

    #[test]
    fn chinese_request_keeps_english_source_and_reports_actual_files() {
        let en = vec!["en-orig".to_string()];
        assert_eq!(pick_sub_langs(&SubIntent::Zh, &en), "en-orig");
        assert_eq!(pick_sub_langs(&SubIntent::ZhEn, &en), "en-orig");
        assert_eq!(pick_sub_langs(&SubIntent::Zh, &["en".into(), "zh-Hans".into()]), "zh-Hans");
        assert!(pick_sub_langs(&SubIntent::Zh, &["ja".into()]).is_empty());
        let dir = std::env::temp_dir().join(format!("sd-english-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir(&dir).unwrap();
        let video = dir.join("Lesson.mp4");
        std::fs::write(dir.join("Lesson.en-orig.vtt"), "original").unwrap();
        std::fs::write(dir.join("Lesson.zh-en.ass.bak"), "backup").unwrap();
        assert_eq!(subtitle_file_label(&video), Some("英文字幕"));
        std::fs::write(dir.join("Lesson.zh-en.ass"), "translation").unwrap();
        assert_eq!(subtitle_file_label(&video), Some("中英字幕"));
        std::fs::remove_file(dir.join("Lesson.zh-en.ass")).unwrap();
        std::fs::remove_file(dir.join("Lesson.en-orig.vtt")).unwrap();
        assert_eq!(subtitle_file_label(&video), None);
        std::fs::remove_file(dir.join("Lesson.zh-en.ass.bak")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn translate_source_picks_original_language() {
        let av = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let ja = br#"{"language":"ja","subtitles":{},"automatic_captions":{}}"#;
        assert_eq!(translate_source(ja, &av(&["ja-orig", "ja", "zh-Hans-ja", "en"]), false).as_deref(), Some("ja-orig"));
        let en = br#"{"language":"en","subtitles":{}}"#;
        assert_eq!(translate_source(en, &av(&["en", "zh-Hans"]), false), None);
        assert_eq!(translate_source(en, &av(&["en-orig", "en", "zh-Hans"]), true).as_deref(), Some("en-orig"));
        // 有人工中文就润色它，不翻
        let human = br#"{"language":"ko","subtitles":{"zh-CN":[]}}"#;
        assert_eq!(translate_source(human, &av(&["ko", "zh-CN"]), false), None);
        // 不知道语言也没有 -orig：走原来的逻辑
        assert_eq!(translate_source(b"{}", &av(&["ru"]), false), None);
    }

    #[test]
    fn selected_size_requires_exact_sizes_for_all_streams() {
        use serde_json::json;

        let merged = json!({"requested_downloads":[{"requested_formats":[
            {"filesize": 1000}, {"filesize": 200}
        ]}]});
        assert_eq!(exact_selected_video_size(&merged), Some(1200));
        let incomplete = json!({"filesize": 1000, "requested_formats":[
            {"filesize": 1000}, {"filesize_approx": 200}
        ]});
        assert_eq!(exact_selected_video_size(&incomplete), None);
        assert_eq!(exact_selected_video_size(&json!({"filesize_approx": 1234})), None);
        assert_eq!(exact_selected_video_size(&json!({"filesize": 1234})), Some(1234));
    }

    #[test]
    fn heights_skip_audio_and_storyboards() {
        let info = br#"{"formats":[
            {"vcodec":"none","height":null,"ext":"m4a"},
            {"vcodec":"none","height":90,"ext":"mhtml"},
            {"vcodec":"avc1","height":720,"ext":"mp4"},
            {"vcodec":"vp9","height":2160,"ext":"webm"},
            {"vcodec":"avc1","height":720,"ext":"mp4"},
            {"height":480,"ext":"mp4"}]}"#;
        assert_eq!(heights_in_info(info), vec![2160, 720, 480]);
        assert!(heights_in_info(b"{}").is_empty());
    }

    #[test]
    fn lockers_finds_process_holding_a_file() {
        let p = std::env::temp_dir().join(format!("sd-lock-{}.txt", now_ms()));
        assert!(file_lockers(&p).is_empty() || !p.exists());
        // 自己占着，查出来应该至少有一个进程
        let f = std::fs::File::create(&p).unwrap();
        assert!(!file_lockers(&p).is_empty());
        drop(f);
        assert!(file_lockers(&p).is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn landing_temp_keeps_subs_and_removes_streams() {
        let dir = std::env::temp_dir().join(format!("sd-land-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["A [x1].temp.mp4", "A [x1].f313.webm", "A [x1].f140.m4a", "A [x1].en.ass", "B [x2].f140.m4a"] {
            std::fs::write(dir.join(f), b"1").unwrap();
        }
        assert_eq!(land_merged_temp(&dir, "x1", ""), Some(dir.join("A [x1].mp4")));
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        left.sort();
        assert_eq!(left, ["A [x1].en.ass", "A [x1].mp4", "B [x2].f140.m4a"]);
        // 没有 temp 就不动
        assert_eq!(land_merged_temp(&dir, "x1", ""), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_uses_per_stream_sizes() {
        // YouTube 实测：顶层 approx 是两路之和，分格式有精确值
        assert_eq!(
            parse_plan("NA 596814145 591807878 591807872 5006267 5006229"),
            vec![591807878, 5006267]
        );
        // B 站：只有 approx
        assert_eq!(parse_plan("NA 75219776 NA 69809437 NA 5410339"), vec![69809437, 5410339]);
        // 单格式
        assert_eq!(parse_plan("NA 5410339 NA NA NA NA"), vec![5410339]);
        assert_eq!(parse_plan("100 999 NA NA NA NA"), vec![100]);
        assert!(parse_plan("NA NA NA NA NA NA").is_empty());
    }

    #[test]
    fn existing_file_needs_finished_video_with_same_id() {
        let dir = std::env::temp_dir().join(format!("sd-exist-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let url = "https://youtu.be/abc123";
        std::fs::write(dir.join("T [abc123].zh.ass"), b"").unwrap();
        std::fs::write(dir.join("T [abc123].f137.mp4"), b"").unwrap();
        std::fs::write(dir.join("T [abc123].mp4.part"), b"").unwrap();
        std::fs::write(dir.join("T [other].mp4"), b"").unwrap();
        assert_eq!(existing_file_for(url, &dir), None);
        std::fs::write(dir.join("T [abc123]_1.mp4"), b"").unwrap();
        assert_eq!(existing_file_for(url, &dir), None); // 严格按「 [id]」后跟点才算第一份
        std::fs::write(dir.join("T [abc123].mp4"), b"").unwrap();
        assert_eq!(existing_file_for(url, &dir), Some(dir.join("T [abc123].mp4")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn already_downloaded_ignores_format_parts() {
        assert_eq!(
            already_downloaded(r"[download] C:\d\T [a1].mp4 has already been downloaded"),
            Some(PathBuf::from(r"C:\d\T [a1].mp4"))
        );
        assert_eq!(already_downloaded(r"[download] C:\d\T [a1].f137.mp4 has already been downloaded"), None);
        assert_eq!(already_downloaded(r"[download] C:\d\T [a1].f313-1.webm has already been downloaded"), None);
        assert_eq!(already_downloaded("SDTITLE has already been downloaded"), None);
    }

    #[test]
    fn job_files_only_touches_its_own() {
        let dir = std::env::temp_dir().join(format!("sd-jobfiles-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let names = [
            "Song [abc].mp4",
            "Song [abc].f137.mp4.part",
            "Song [abc].zh.ass",
            "Song [abc]_1.mp4",
            "Song Remix [xyz].mp4",
            "Other [def].mp4",
            "Song.mp4",
        ];
        for n in names {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let mut job = VideoJob::new("https://a/b", &dir, VideoOptions::default());
        let found = |j: &VideoJob, others: &[VideoJob]| {
            let mut v: Vec<String> = job_files(j, others)
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
                .collect();
            v.sort();
            v
        };
        // 刚开始解析就取消：什么都不知道，一个都不能碰（曾经把整个目录删了）
        job.title = "Song".into();
        assert!(found(&job, &[]).is_empty());
        // 知道 id：只有自己这一份，_1 那份和标题相近的都不动
        job.vid = "abc".into();
        assert_eq!(found(&job, &[]), ["Song [abc].f137.mp4.part", "Song [abc].mp4", "Song [abc].zh.ass"]);
        job.name_suffix = "_1".into();
        assert_eq!(found(&job, &[]), ["Song [abc]_1.mp4"]);
        // 另一条任务指向同一批文件：只删记录
        let mut other = job.clone();
        other.id = "other".into();
        assert!(found(&job, &[other]).is_empty());
        // 老任务没记 id：只认成品文件名
        let mut old = VideoJob::new("https://a/b", &dir, VideoOptions::default());
        old.file_path = dir.join("Other [def].mp4").display().to_string();
        assert_eq!(found(&old, &[]), ["Other [def].mp4"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn feed_prog(st: &mut Progress, done: u64, total: u64, status: &str) -> (u64, u64) {
        if status == "finished" {
            st.finished_bytes += done;
            st.stream += 1;
            st.current_done = 0;
            st.current_total = 0;
            st.current_est = 0;
        } else {
            st.current_done = done;
            st.current_total = total;
        }
        (st.finished_bytes + st.current_done, st.display_total())
    }

    #[test]
    fn sub_intent_normalizes_old_regex() {
        assert_eq!(normalize_sub_intent("zh"), SubIntent::Zh);
        assert_eq!(normalize_sub_intent("zh.*,ai-zh"), SubIntent::Zh);
        assert_eq!(normalize_sub_intent("en.*"), SubIntent::En);
        assert_eq!(normalize_sub_intent("zh.*,en.*,ai-zh"), SubIntent::ZhEn);
        assert_eq!(normalize_sub_intent("zh+en"), SubIntent::ZhEn);
        assert_eq!(normalize_sub_intent(".*"), SubIntent::All);
        assert_eq!(normalize_sub_intent("all"), SubIntent::All);
    }

    #[test]
    fn pick_one_per_family() {
        // TED 实测：同时有简繁 + en + en-orig
        let av: Vec<String> = ["zh", "zh-cn", "zh-tw", "en", "en-orig"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(pick_sub_langs(&SubIntent::Zh, &av), "zh-cn");
        assert_eq!(pick_sub_langs(&SubIntent::En, &av), "en");
        assert_eq!(pick_sub_langs(&SubIntent::ZhEn, &av), "zh-cn,en");

        // YouTube：简繁 auto + 英文 auto
        let yt: Vec<String> = ["zh-Hans", "zh-Hant", "en", "en-orig"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(pick_sub_langs(&SubIntent::Zh, &yt), "zh-Hans");
        assert_eq!(pick_sub_langs(&SubIntent::En, &yt), "en");

        // 站点没字幕：不用跑
        assert_eq!(pick_sub_langs(&SubIntent::ZhEn, &[]), "");
        // 只有英文时选中英也只下英文
        let en: Vec<String> = vec!["en".into()];
        assert_eq!(pick_sub_langs(&SubIntent::ZhEn, &en), "en");

        // 用户那条视频实测：中文原声，英文和简中都是机翻变体，只要从中文翻的
        let real: Vec<String> = ["ja", "en-ja", "en-zh", "zh-Hans-ja", "zh-Hans-zh", "zh-Hant-ja", "zh-Hant-zh", "zh"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(pick_sub_langs(&SubIntent::ZhEn, &real), "zh,en-zh");
        // 有原生英文就用原生的，绝不换成 en-zh
        let native: Vec<String> = ["en", "en-zh", "zh"].iter().map(|s| s.to_string()).collect();
        assert_eq!(pick_sub_langs(&SubIntent::ZhEn, &native), "zh,en");
        // 没有中文源字幕（日语视频）：英文从日语翻，不会出现 en-zh
        let ja: Vec<String> = ["ja", "en-ja", "zh-Hans-ja"].iter().map(|s| s.to_string()).collect();
        assert_eq!(pick_sub_langs(&SubIntent::ZhEn, &ja), "zh-Hans-ja,en-ja");
        // 英语视频：原生英文 + 简中
        let enzh: Vec<String> = ["en", "en-orig", "zh-Hans", "zh-Hant"].iter().map(|s| s.to_string()).collect();
        assert_eq!(pick_sub_langs(&SubIntent::ZhEn, &enzh), "zh-Hans,en");

        // 全部：永远透传 .*
        assert_eq!(pick_sub_langs(&SubIntent::All, &yt), ".*");
    }

    #[test]
    fn two_streams_never_hit_99_early() {
        let mut st = Progress::default();
        st.plan = parse_plan("NA NA 100000000 NA 5000000 NA");
        // 视频轨下完时分母必须已经带上音频，进度不能先到 100%
        let (d, t) = feed_prog(&mut st, 50_000_000, 100_000_000, "downloading");
        assert_eq!((d, t), (50_000_000, 105_000_000));
        let (d, t) = feed_prog(&mut st, 100_000_000, 100_000_000, "finished");
        assert_eq!((d, t), (100_000_000, 105_000_000));
        let (d, t) = feed_prog(&mut st, 1_000_000, 5_000_000, "downloading");
        assert_eq!((d, t), (101_000_000, 105_000_000));
        // 全下完：分母等于实际字节，不会再变回未知
        let (d, t) = feed_prog(&mut st, 5_000_000, 5_000_000, "finished");
        assert_eq!((d, t), (105_000_000, 105_000_000));
    }

    #[test]
    fn total_never_returns_to_unknown() {
        let mut st = Progress::default();
        // 一开始什么都不知道，或者 estimate 还不可信：未知（前端画滚动条）
        st.current_done = 1000;
        st.current_est = 1_000_000_000;
        assert_eq!(st.display_total(), 0);
        // 分段任务下了一段：开始信 estimate
        st.current_done = 3_000_000;
        st.current_est = 50_000_000;
        assert_eq!(st.display_total(), 50_000_000);
        // estimate 掉到比已下还小：不许退回未知
        st.current_done = 60_000_000;
        st.current_est = 40_000_000;
        assert_eq!(st.display_total(), 60_000_000);
        // 预计值偏小同理
        let mut st = Progress::default();
        st.plan = vec![1000];
        st.current_done = 5000;
        assert_eq!(st.display_total(), 5000);
    }

    #[test]
    fn res_labels() {
        // 宽高都有：原样输出
        assert_eq!(res_label(1920, 1080).as_deref(), Some("1920 x 1080"));
        // 竖屏也照实写，不再按短边改成 1080P 口径
        assert_eq!(res_label(1080, 1920).as_deref(), Some("1080 x 1920"));
        // 只有高度（文件名里抠的）：按 16:9 估宽
        assert_eq!(res_label(0, 1080).as_deref(), Some("1920 x 1080"));
        assert_eq!(res_label(0, 480).as_deref(), Some("853 x 480"));
        assert_eq!(res_label(0, 0), None);
    }

    #[test]
    fn resolution_from_release_names() {
        // 输出已经是「宽 x 高」：文件名里只有高度，宽度按 16:9 估
        assert_eq!(resolution_from_name("Movie.2024.1080p.x264").as_deref(), Some("1920 x 1080"));
        assert_eq!(resolution_from_name("Show.S01E05.2160P.WEB").as_deref(), Some("3840 x 2160"));
        assert_eq!(resolution_from_name("Show.S01E05.4K.HEVC").as_deref(), Some("3840 x 2160"));
        assert_eq!(resolution_from_name("Film.8K.Dolby").as_deref(), Some("7680 x 4320"));
        assert_eq!(resolution_from_name("movie.720p.mkv").as_deref(), Some("1280 x 720"));
        // "x264" 不是分辨率（数字前是字母）
        assert_eq!(resolution_from_name("movie.x264.mkv"), None);
        assert_eq!(resolution_from_name("readme.txt"), None);
    }
}
