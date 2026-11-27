// 链接识别与分类。移植自 Mac 版 Sources/Shandian/Links.swift，规则逐条对齐。
// 纯逻辑，不碰网络和文件系统（除了判断本地 .torrent 文件是否存在）。

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Magnet,
    TorrentFile,
    Ed2k,
    VideoSite,
    Http,
}

/// 判断「是不是同一个资源」用的归一化链接。同一个 YouTube 视频从播放列表点进来会多一串
/// list/start_radio/index，磁力的 tracker 也会变，按原文比较就认不出是同一个。
/// 国外网盘的分享链接换成能直接下的地址（不需要登录的公开分享）：
/// Dropbox 把 dl=0 改成 dl=1；Google Drive 的 /file/d/<id>/view、open?id= 换成下载地址。其余原样返回
pub fn cloud_direct(link: &str) -> String {
    let Ok(mut u) = Url::parse(link.trim()) else { return link.to_string() };
    let host = u.host_str().unwrap_or("").to_lowercase();
    if host == "dropbox.com" || host == "www.dropbox.com" {
        if u.path().starts_with("/s/") || u.path().starts_with("/scl/") {
            let pairs: Vec<(String, String)> = u.query_pairs().filter(|(k, _)| k != "dl").map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
            u.query_pairs_mut().clear().extend_pairs(pairs).append_pair("dl", "1");
            return u.to_string();
        }
    }
    if host == "drive.google.com" {
        let id = u
            .path()
            .strip_prefix("/file/d/")
            .and_then(|r| r.split('/').next())
            .map(str::to_string)
            .or_else(|| u.query_pairs().find(|(k, _)| k == "id").map(|(_, v)| v.into_owned()));
        if let Some(id) = id.filter(|s| !s.is_empty()) {
            return format!("https://drive.usercontent.google.com/download?id={id}&export=download&confirm=t");
        }
    }
    link.to_string()
}

pub fn canonical_url(link: &str) -> String {
    let link = link.trim();
    if let Some(url) = crate::baidu::share_url(link) { return crate::baidu::public_url(&url); }
    if link.to_ascii_lowercase().starts_with("magnet:") {
        let low = link.to_ascii_lowercase();
        let hash = low
            .split(['?', '&'])
            .find_map(|p| p.strip_prefix("xt=urn:btih:"));
        return match hash {
            Some(h) => format!("btih:{h}"),
            None => low,
        };
    }
    let Ok(u) = url::Url::parse(link) else {
        return link.to_string();
    };
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    let host = host.strip_prefix("www.").or(host.strip_prefix("m.")).unwrap_or(&host).to_string();
    let seg: Vec<&str> = u.path().split('/').filter(|x| !x.is_empty()).collect();
    if host == "weixin.qq.com" && seg.len() == 2 && seg[0] == "sph" {
        return format!("wechat:{}", seg[1]);
    }
    if host == "channels.weixin.qq.com" && u.path().starts_with("/finder-preview/") {
        if let Some((_, id)) = u.query_pairs().find(|(k, _)| k == "id") { return format!("wechat:{id}"); }
    }
    if host == "youtu.be" {
        if let Some(id) = seg.first() {
            return format!("yt:{id}");
        }
    }
    if host.ends_with("youtube.com") {
        if let Some((_, v)) = u.query_pairs().find(|(k, _)| k == "v") {
            return format!("yt:{v}");
        }
        if let ["shorts" | "live" | "embed", id, ..] = seg.as_slice() {
            return format!("yt:{id}");
        }
    }
    // B 站的链接后面挂一长串来源参数（trackid、spm_id_from、vd_source…），而且一直在加。
    // 视频本身只由 BV 号和分集 p 决定
    if host.ends_with("bilibili.com") {
        if let Some(id) = seg.iter().find(|s| s.starts_with("BV") || s.starts_with("av")) {
            let p = u.query_pairs().find(|(k, _)| k == "p").map(|(_, v)| v.to_string());
            return format!("bili:{id}:{}", p.as_deref().unwrap_or("1"));
        }
    }
    // 其余站点去掉来源统计参数；决定内容的参数要留着
    const NOISE: &[&str] = &[
        "list", "index", "start_radio", "t", "pp", "si", "feature", "spm_id_from",
        "vd_source", "share_source", "share_medium", "share_plat", "from", "from_source",
    ];
    let mut q: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| !NOISE.contains(&k.as_ref()) && !k.starts_with("utm_"))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    q.sort();
    let q: Vec<String> = q.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("{host}{}?{}", u.path().trim_end_matches('/'), q.join("&"))
}

#[cfg(test)]
mod channel_identity_tests {
    #[test]
    fn clipboard_share_links_are_not_ignored() {
        for link in ["https://weixin.qq.com/sph/AqinEEb4Kf","https://pan.quark.cn/s/779e5f0a9427#/list/share","https://drive.uc.cn/s/bfc2f9849c8e4?public=1#/list/share","https://pan.baidu.com/s/1ExampleShare?pwd=demo#list/path=%2F","https://www.alipan.com/s/CSm75WGT8ED/folder/abc","https://www.123pan.cn/s/test","https://share.123865.com/123pan/test","https://cloud.189.cn/web/share?code=test","https://yun.139.com/link/w/test","https://pan.xunlei.com/s/test","https://www.guangyapan.com/s/test"] {
            assert!(super::worth_popup(link,&[]),"{link}");
        }
        assert!(!super::worth_popup("https://pan.quark.cn.evil.example/s/123",&[]));
    }
    #[test]
    fn share_preview_and_tracking_variants_are_the_same_video() {
        let original = super::canonical_url("https://weixin.qq.com/sph/AL7gWeEZUL");
        for link in ["https://channels.weixin.qq.com/finder-preview/pages/sph?id=AL7gWeEZUL", "http://weixin.qq.com/sph/AL7gWeEZUL/?from=share"] {
            assert_eq!(super::canonical_url(link),original);
        }
        assert_ne!(super::canonical_url("https://weixin.qq.com/sph/AnotherVideo"),original);
    }
}

/// 从链接就能认出来的视频 id（和 yt-dlp 的 %(id)s 一致，文件名里带着它）。
/// 认不出的站返回 None，要等 yt-dlp 解析完才知道文件名。
pub fn video_id_of(link: &str) -> Option<String> {
    if let Some(id) = canonical_url(link).strip_prefix("yt:") {
        return Some(id.to_string());
    }
    let u = url::Url::parse(link).ok()?;
    if !u.host_str()?.to_ascii_lowercase().ends_with("bilibili.com") {
        return None;
    }
    // 多 P 的第二集起 id 带 _p2，这里认不准，交给解析后的撞车逻辑
    if u.query_pairs().any(|(k, _)| k == "p") {
        return None;
    }
    u.path_segments()?
        .find(|s| s.starts_with("BV") || s.starts_with("av"))
        .map(String::from)
}

/// 不用解析页面就知道的封面地址，投任务的同时就能开始拿。
/// mqdefault 每个视频都有（maxres 不一定有），16:9 不带黑边，缩略图尺寸也够用。
pub fn early_thumb_url(link: &str) -> Option<String> {
    let key = canonical_url(link);
    let id = key.strip_prefix("yt:")?;
    Some(format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg"))
}

/// 这个链接能下字幕吗。只有长视频站有成规模的字幕，抖音、小红书这类没有
pub fn supports_subtitles(link: &str) -> bool {
    const HOSTS: &[&str] = &["youtube.com", "youtu.be", "bilibili.com", "b23.tv", "vimeo.com"];
    let Ok(u) = url::Url::parse(link.trim()) else { return false };
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    HOSTS.iter().any(|h| host == *h || host.ends_with(&format!(".{h}")))
}

/// 视频站域名。命中就走 yt-dlp，不走内核。
const VIDEO_HOSTS: &[&str] = &[
    "bilibili.com",
    "b23.tv",
    "youtube.com",
    "youtu.be",
    "douyin.com",
    "xiaohongshu.com",
    "xhslink.com",
    "kuaishou.com",
    "weibo.com",
    "ixigua.com",
    "vimeo.com",
    "x.com",
    "twitter.com",
    "xinpianchang.com",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CollectionSite {
    YouTube,
    Bilibili,
}

/// Explicit list pages always open a preview. A playback page only does so on request.
pub fn video_collection(link: &str, requested: bool) -> Option<CollectionSite> {
    let u = Url::parse(link).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let host = u.host_str()?;
    let parts: Vec<_> = u.path().trim_matches('/').split('/').collect();
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    let valid_id = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'));
    if (host == "youtube.com" || host.ends_with(".youtube.com"))
        && parts == ["playlist"]
        && u.query_pairs().any(|(k, v)| k == "list" && valid_id(&v))
    {
        return Some(CollectionSite::YouTube);
    }
    let is_bili = match (host, parts.as_slice()) {
        ("space.bilibili.com", [mid, "lists", sid]) => digits(mid) && digits(sid),
        ("space.bilibili.com", [mid, "channel", "collectiondetail" | "seriesdetail"]) => {
            digits(mid) && u.query_pairs().any(|(k, v)| k == "sid" && digits(&v))
        }
        ("www.bilibili.com" | "m.bilibili.com" | "bilibili.com", ["list", id] | ["medialist", "play", id]) => {
            valid_id(id) && (requested || !u.query_pairs().any(|(k, v)| k == "bvid" && !v.is_empty()))
        }
        ("www.bilibili.com" | "m.bilibili.com" | "bilibili.com", ["video", id]) => requested && valid_id(id),
        _ => false,
    };
    is_bili.then_some(CollectionSite::Bilibili)
}

/// 视频站上这个地址是不是一个视频。路径规则确定的站才判断，其余（快手、微博、西瓜）只看域名
fn is_video_page(host: &str, url: &Url) -> bool {
    let on = |d: &str| host == d || host.ends_with(&format!(".{d}"));
    let path = url.path();
    let digits_after = |prefix: &str| path.strip_prefix(prefix).is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()));
    if on("x.com") || on("twitter.com") {
        return path.contains("/status/");
    }
    if on("youtube.com") {
        return path.starts_with("/watch") || path.starts_with("/shorts/") || path.starts_with("/live/") || path.starts_with("/embed/");
    }
    if on("bilibili.com") {
        return path.starts_with("/video/") || path.starts_with("/bangumi/play/") || path.starts_with("/list/") || path.starts_with("/medialist/play/");
    }
    if on("douyin.com") {
        return path.starts_with("/video/") || path.starts_with("/note/") || url.query_pairs().any(|(k, _)| k == "modal_id" || k == "vid");
    }
    if on("xiaohongshu.com") {
        return path.starts_with("/explore/") || path.starts_with("/discovery/item/");
    }
    if on("vimeo.com") {
        return digits_after("/") || path.contains("/video/");
    }
    if on("xinpianchang.com") {
        return digits_after("/a");
    }
    if on("weibo.com") || on("weibo.cn") {
        // 视频页 /tv/show/…、单条微博 /<用户>/<编号>、手机版 /status/…；首页、登录页不算
        return host.starts_with("video.") || path.starts_with("/tv/show/") || path.contains("/status/") || {
            let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
            parts.len() == 2 && parts[0].chars().all(|c| c.is_ascii_digit()) && !parts[1].is_empty()
        };
    }
    // youtu.be、b23.tv、xhslink.com 这类短链本身就指向一个视频
    true
}

/// 复制后值得弹浮窗的文件扩展名
const FILE_EXTENSIONS: &[&str] = &[
    "zip", "rar", "7z", "gz", "tgz", "bz2", "xz", "dmg", "pkg", "exe", "msi", "iso", "img", "apk",
    "ipa", "mp4", "mkv", "avi", "mov", "m4v", "ts", "flv", "wmv", "rmvb", "webm", "mp3", "flac",
    "wav", "m4a", "aac",
];

impl LinkKind {
    pub fn of(raw: &str) -> Option<Self> {
        let s = raw.trim();
        let lower = s.to_lowercase();
        if lower.starts_with("magnet:?") {
            return Some(Self::Magnet);
        }
        if lower.starts_with("ed2k://") {
            return Some(Self::Ed2k);
        }
        // 本地种子文件：路径存在才算（否则一个叫 xx.torrent 的字符串不该被当成文件）
        if lower.ends_with(".torrent") && Path::new(s).exists() {
            return Some(Self::TorrentFile);
        }
        let url = Url::parse(s).ok()?;
        let scheme = url.scheme().to_lowercase();
        if !matches!(scheme.as_str(), "http" | "https" | "ftp") {
            return None;
        }
        let host = url.host_str()?.to_lowercase();
        if VIDEO_HOSTS
            .iter()
            .any(|h| host == *h || host.ends_with(&format!(".{h}")))
        {
            // 视频站的首页、个人主页这类不是视频，不算链接（复制 x.com/home 不该弹下载卡片）
            return (is_video_page(&host, &url) || video_collection(s, false).is_some()).then_some(Self::VideoSite);
        }
        // 网页上的种子链接（/down.php?id=1&name=xxx.torrent）按种子处理
        if url.path().to_lowercase().ends_with(".torrent") {
            return Some(Self::TorrentFile);
        }
        Some(Self::Http)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Magnet => "磁力",
            Self::TorrentFile => "种子文件",
            Self::Ed2k => "ed2k",
            Self::VideoSite => "视频网站",
            Self::Http => "直链",
        }
    }
}

/// 从一段文字里找出所有能下的链接（粘贴一整段时用）
pub fn extract(text: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // 中文标点也算链接结束：B 站分享是「【标题】 https://b23.tv/xxx」，
        // 还有「看这个：https://…，很不错」这种，标点不能粘进链接里
        Regex::new(r#"(magnet:\?[^\s"'<>，。；！？、）】》」』]+|ed2k://[^\s"'<>，。；！？、）】》」』]+|https?://[^\s"'<>，。；！？、）】》」』]+)"#).unwrap()
    });
    let mut seen: HashSet<&str> = HashSet::new();
    re.find_iter(text)
        .filter_map(|m| {
            let mut link = m.as_str();
            if text[..m.start()].ends_with('[') {
                if let Some((_, target)) = link.split_once("](").filter(|(_, target)| target.starts_with("https://") || target.starts_with("http://")) {
                    link = target;
                }
            }
            let mut parens = link.matches(')').count().saturating_sub(link.matches('(').count());
            let mut brackets = link.matches(']').count().saturating_sub(link.matches('[').count());
            loop {
                match link.as_bytes().last() {
                    Some(b')') if parens > 0 => parens -= 1,
                    Some(b']') if brackets > 0 => brackets -= 1,
                    _ => break,
                }
                link = &link[..link.len() - 1];
            }
            if !seen.insert(link) {
                return None;
            }
            Some(crate::baidu::with_pasted_code(link, &text[m.end()..]).unwrap_or_else(|| link.into()))
        })
        .collect()
}

/// 复制后是否值得弹浮窗：磁力、ed2k、种子、视频网站一律弹；
/// 直链只有落在网盘域名或常见文件扩展名上才弹（普通网页地址不弹）
pub fn is_pan_share(link: &str) -> bool {
    if crate::baidu::share_url(link).is_some() { return true; }
    let Ok(u)=Url::parse(link.trim()) else{return false};
    let host=u.host_str().unwrap_or("");
    ["pan.quark.cn","drive.uc.cn","pan.baidu.com","alipan.com","aliyundrive.com","123pan.com","123pan.cn","123684.com","123865.com","123952.com","123912.com","123957.com","cloud.189.cn","yun.139.com","caiyun.139.com","pan.xunlei.com","guangyapan.com"].iter().any(|h|host==*h || host.ends_with(&format!(".{h}")))
        && (["/s/","/share/","/t/"].iter().any(|p|u.path().starts_with(p))
            || host=="cloud.189.cn" && u.path().starts_with("/web/share")
            || ["yun.139.com","caiyun.139.com"].contains(&host) && ["/link/","/shareweb"].iter().any(|p|u.path().starts_with(p))
            || host.contains("123") && u.path().starts_with("/123pan/"))
}

pub fn worth_popup(link: &str, pan_hosts: &[&str]) -> bool {
    if crate::ghmirror::is_huggingface_file(link) { return true; }
    if is_pan_share(link) || crate::wechat::is_share(link) || crate::feishu::document_url(link).is_some() { return true; }
    let Some(kind) = LinkKind::of(link) else {
        return false;
    };
    if kind != LinkKind::Http {
        return true;
    }
    let Ok(url) = Url::parse(link) else {
        return false;
    };
    let host = url.host_str().unwrap_or("").to_lowercase();
    if (host == "gitee.com" || host.ends_with(".gitee.com")) && is_gitee_file_path(&url) {
        return true;
    }
    if pan_hosts.iter().any(|h| host.contains(h)) {
        return true;
    }
    let ext = url
        .path()
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.').map(|(_, e)| e.to_lowercase()))
        .unwrap_or_default();
    FILE_EXTENSIONS.contains(&ext.as_str())
}

#[cfg(test)]
#[test]
fn clipboard_accepts_hugging_face_files_not_model_pages() {
    assert!(worth_popup("https://huggingface.co/org/model/resolve/main/config.json", &[]));
    assert!(worth_popup("https://huggingface.co/org/model/resolve/main/model.gguf?download=true", &[]));
    assert!(!worth_popup("https://huggingface.co/org/model/tree/main", &[]));
}

fn is_gitee_file_path(url: &Url) -> bool {
    let parts: Vec<_> = url.path_segments().into_iter().flatten().collect();
    match parts.as_slice() {
        [_, _, "releases", "download", _, file, ..] |
        [_, _, "archive", "refs", "tags", file, ..] |
        [_, _, "repository", "archive", file, ..] |
        [_, _, "raw", _, file, ..] => !file.is_empty(),
        _ => false,
    }
}

// ---------- 种子选片 ----------

pub struct FileMeta {
    pub name: String,
    pub size: u64,
}

const VIDEO_EXT: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "m4v", "ts", "m2ts", "webm", "wmv", "flv", "rmvb", "mpg", "mpeg",
    "ogv",
];
const SUBTITLE_EXT: &[&str] = &["srt", "ass", "ssa", "sub", "idx", "vtt", "sup"];

fn ext_of(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default()
}

/// 去掉扩展名和码率/画质后缀，用来判断是不是同一部片的不同版本
fn stem(name: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)[._\- ]?(512kb|256kb|1024kb|hq|lq|ia|low|high)$").unwrap()
    });
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let no_ext = match base.rsplit_once('.') {
        Some((s, _)) => s,
        None => base,
    };
    re.replace(&no_ext.to_lowercase(), "").to_string()
}

/// 格式偏好：mp4/mkv 兼容最好，其次 avi/mov 等，ogv/webm/flv 最后
fn rank(ext: &str) -> u8 {
    match ext {
        "mp4" | "mkv" | "m4v" => 3,
        "mov" | "avi" | "ts" | "m2ts" | "mpg" | "mpeg" | "wmv" | "rmvb" => 2,
        _ => 1,
    }
}

fn is_sample(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("sample") || n.contains("预告") || n.contains("trailer")
}

/// 种子里该勾哪些文件：主视频（最大的那个）+ 体积相近的其他视频（剧集合集）+ 字幕。
/// 样片、海报、同一部片的低清副本、说明文件都不下。
/// 返回 Some(要下的下标) 表示需要显式选片；None 表示「全选即可」（没有视频，或全都要）。
pub fn pick_files(files: &[FileMeta]) -> Option<Vec<usize>> {
    let videos: Vec<usize> = (0..files.len())
        .filter(|&i| {
            VIDEO_EXT.contains(&ext_of(&files[i].name).as_str()) && !is_sample(&files[i].name)
        })
        .collect();
    let biggest = videos.iter().map(|&i| files[i].size).max()?;
    if biggest == 0 {
        return None;
    }
    // 体积不到最大那个 40% 的是低清副本或花絮
    let big: Vec<usize> = videos
        .iter()
        .copied()
        .filter(|&i| (files[i].size as f64) >= (biggest as f64) * 0.4)
        .collect();

    // 同一部片的多个格式/码率只留一个：按 stem 分组，组内取（格式偏好，体积）最大的
    let mut groups: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for &i in &big {
        let key = stem(&files[i].name);
        groups
            .entry(key)
            .and_modify(|cur| {
                let a = (rank(&ext_of(&files[*cur].name)), files[*cur].size);
                let b = (rank(&ext_of(&files[i].name)), files[i].size);
                if b > a {
                    *cur = i;
                }
            })
            .or_insert(i);
    }
    let mut picked: Vec<usize> = groups.into_values().collect();
    picked.extend(
        (0..files.len()).filter(|&i| SUBTITLE_EXT.contains(&ext_of(&files[i].name).as_str())),
    );
    picked.sort_unstable();
    picked.dedup();
    if picked.len() == files.len() {
        None
    } else {
        Some(picked)
    }
}

// ---------- 显示格式 ----------

/// 1.09 GB / 680 MB / 42 KB
pub fn bytes_text(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 {
        format!("{:.0} {}", v, UNITS[u])
    } else if v >= 10.0 {
        format!("{:.1} {}", v, UNITS[u])
    } else {
        format!("{:.2} {}", v, UNITS[u])
    }
}

pub fn speed_text(n: u64) -> String {
    if n < 1024 {
        "0 KB/s".to_string()
    } else {
        format!("{}/s", bytes_text(n))
    }
}

pub fn remaining_text(secs: f64) -> String {
    if !secs.is_finite() || secs <= 0.0 {
        return String::new();
    }
    let s = secs as u64;
    if s < 60 {
        format!("剩余 {s} 秒")
    } else if s < 3600 {
        format!("剩余 {} 分钟", s / 60)
    } else {
        format!("剩余 {} 小时 {} 分", s / 3600, s % 3600 / 60)
    }
}

// ---------- 片名整理 ----------

/// 把发布名整理成人能读的片名：Conclave.2024.1080p.AMZN.WEBRip-GalaxyRG → Conclave 2024
pub fn pretty_title(raw: &str) -> String {
    static JUNK: OnceLock<Regex> = OnceLock::new();
    static BRACKET: OnceLock<Regex> = OnceLock::new();
    static RES: OnceLock<Regex> = OnceLock::new();
    static TAIL: OnceLock<Regex> = OnceLock::new();
    static SPACES: OnceLock<Regex> = OnceLock::new();
    static EMPTY_PAREN: OnceLock<Regex> = OnceLock::new();
    static CAMEL: OnceLock<Regex> = OnceLock::new();
    static YEAR: OnceLock<Regex> = OnceLock::new();

    let junk = JUNK.get_or_init(|| Regex::new(r"(?i)\b(2160p|1080p|720p|480p|4k|uhd|hdr10?\+?|dv|blu-?ray|bdrip|brrip|web-?dl|webrip|web|hdtv|hr-hdtv|dvdrip|remux|x26[45]|h\.?26[45]|hevc|avc|10bit|8bit|aac(2\.0)?|ac3|dts(-hd)?( ma)?|ddp?5\.1|dd2\.0|truehd|atmos|5\.1|7\.1|2\.0|amzn|nf|hmax|hbomax|ddp|dsnp|proper|repack|extended|remastered|imax|chs|cht|eng|chs-eng|中英双字|国英双语|双语字幕|esub|multi)\b").unwrap());
    let bracket = BRACKET.get_or_init(|| Regex::new(r"\[[^\]]*\]|【[^】]*】").unwrap());
    let res = RES.get_or_init(|| Regex::new(r"\b[0-9]{3,4}[xX×][0-9]{3,4}\b").unwrap());
    let tail = TAIL.get_or_init(|| Regex::new(r"-[^\s\-0-9][^\s\-]*$").unwrap());
    let spaces = SPACES.get_or_init(|| Regex::new(r"[\s\-\.]+").unwrap());
    let empty_paren = EMPTY_PAREN.get_or_init(|| Regex::new(r"\(\s*\)").unwrap());
    let camel = CAMEL.get_or_init(|| Regex::new(r"([a-z])([A-Z])").unwrap());
    let year_re = YEAR.get_or_init(|| Regex::new(r"19[2-9][0-9]|20[0-4][0-9]").unwrap());

    let mut s = raw.to_string();
    // 去扩展名
    let lower = s.to_lowercase();
    for e in VIDEO_EXT {
        let suffix = format!(".{e}");
        if lower.ends_with(&suffix) {
            s.truncate(s.len() - suffix.len());
            break;
        }
    }
    s = bracket.replace_all(&s, " ").to_string();
    if !s.contains(' ') {
        s = s.replace('.', " ").replace('_', " ");
    }

    // 年份之后基本都是技术参数，截到年份（片名开头的数字不算，如「2001太空漫游」）。
    // Swift 版用 lookbehind/lookahead，Rust 的 regex crate 不支持，这里手工判边界。
    let mut cut: Option<usize> = None;
    for m in year_re.find_iter(&s) {
        let start = m.start();
        if start == 0 {
            continue; // 开头的年份属于片名
        }
        let before = s[..start].chars().next_back();
        let after = s[m.end()..].chars().next();
        if before.map_or(true, |c| !c.is_ascii_digit()) && after.map_or(true, |c| !c.is_ascii_digit())
        {
            cut = Some(m.end());
        }
    }
    if let Some(end) = cut {
        s.truncate(end);
        // Dreams (1990 → Dreams 1990
        static OPEN_YEAR: OnceLock<Regex> = OnceLock::new();
        let re = OPEN_YEAR.get_or_init(|| Regex::new(r"\(\s*([0-9]{4})$").unwrap());
        s = re.replace(&s, "$1").to_string();
    }

    let had_junk = junk.is_match(&s);
    s = junk.replace_all(&s, " ").to_string();
    s = res.replace_all(&s, " ").to_string();
    if had_junk {
        // 结尾的 -GalaxyRG / -人人影视制作
        s = tail.replace(&s, "").to_string();
    }
    s = spaces.replace_all(&s, " ").to_string();
    s = empty_paren.replace_all(&s, "").to_string();
    s = s.trim_matches(|c: char| c.is_whitespace() || "-·.".contains(c)).to_string();
    if !s.contains(' ') {
        // JungleBook → Jungle Book
        s = camel.replace_all(&s, "$1 $2").to_string();
    }
    if !s.is_empty() && s == s.to_lowercase() && s.chars().any(|c| c.is_ascii_lowercase()) {
        // his girl friday → His Girl Friday
        s = s
            .split(' ')
            .map(|w| {
                let mut cs = w.chars();
                match cs.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
    }
    if s.is_empty() {
        raw.to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_recognizes_same_resource() {
        let a = canonical_url("https://www.youtube.com/watch?v=8BJ_hsghH6E&list=RD8BJ_hsghH6E&start_radio=1");
        assert_eq!(a, "yt:8BJ_hsghH6E");
        assert!(supports_subtitles("https://www.youtube.com/watch?v=a"));
        assert!(supports_subtitles("https://b23.tv/abc"));
        assert!(!supports_subtitles("https://www.douyin.com/video/1"));
        assert!(!supports_subtitles("magnet:?xt=urn:btih:abc"));
        assert!(!supports_subtitles("https://notyoutube.com/a"));
        // B 站分享复制出来的格式
        assert_eq!(
            extract("【如果你真的进入风之谷-哔哩哔哩】 https://b23.tv/AbCd123"),
            ["https://b23.tv/AbCd123"]
        );
        assert_eq!(extract("看这个：https://www.bilibili.com/video/BV1，很不错"), ["https://www.bilibili.com/video/BV1"]);
        assert_eq!(video_id_of("https://youtu.be/8BJ_hsghH6E?si=abc").as_deref(), Some("8BJ_hsghH6E"));
        assert_eq!(video_id_of("https://www.bilibili.com/video/BV1GJ411x7h7/").as_deref(), Some("BV1GJ411x7h7"));
        assert_eq!(video_id_of("https://www.bilibili.com/video/BV1GJ411x7h7?p=2"), None);
        assert_eq!(video_id_of("https://example.com/a.mp4"), None);
        assert_eq!(a, canonical_url("https://youtu.be/8BJ_hsghH6E?si=abc"));
        assert_eq!(a, canonical_url("https://m.youtube.com/watch?v=8BJ_hsghH6E"));
        assert_ne!(a, canonical_url("https://www.youtube.com/watch?v=other"));
        // B 站不同分集不是同一个
        assert_ne!(
            canonical_url("https://www.bilibili.com/video/BV1?p=1"),
            canonical_url("https://www.bilibili.com/video/BV1?p=2")
        );
        assert_eq!(
            canonical_url("https://www.bilibili.com/video/BV1/?spm_id_from=x&p=2"),
            canonical_url("https://bilibili.com/video/BV1?p=2")
        );
        // 同一个视频从不同入口点开，来源参数全不一样
        assert_eq!(
            canonical_url("https://www.bilibili.com/video/BV1A6HF6aE52/?trackid=web_pegasus_0.x.1793.966&spm_id_from=333.1007.tianma.1-3-3.click&vd_source=15114f"),
            canonical_url("https://www.bilibili.com/video/BV1A6HF6aE52")
        );
        assert_eq!(
            canonical_url("https://www.bilibili.com/video/BV1?p=1"),
            canonical_url("https://www.bilibili.com/video/BV1")
        );
        assert_eq!(
            canonical_url("magnet:?xt=urn:btih:ABC&tr=x"),
            canonical_url("magnet:?dn=y&xt=urn:btih:abc")
        );
    }

    #[test]
    fn video_site_home_pages_are_not_links() {
        for u in ["https://x.com/home", "https://www.youtube.com/", "https://www.bilibili.com/", "https://www.douyin.com/jingxuan", "https://vimeo.com/explore", "https://weibo.com/newlogin?tabtype=weibo"] {
            assert_eq!(LinkKind::of(u), None, "{u}");
        }
        for u in ["https://x.com/a/status/1", "https://www.youtube.com/watch?v=x", "https://youtu.be/x", "https://www.bilibili.com/video/BV1x", "https://www.douyin.com/jingxuan?modal_id=1", "https://vimeo.com/123", "https://www.xinpianchang.com/a123", "https://b23.tv/x", "https://weibo.com/1234567/AbCdEf", "https://weibo.com/tv/show/1034:1"] {
            assert_eq!(LinkKind::of(u), Some(LinkKind::VideoSite), "{u}");
        }
    }

    #[test]
    fn kinds() {
        assert_eq!(
            LinkKind::of("magnet:?xt=urn:btih:abc"),
            Some(LinkKind::Magnet)
        );
        assert_eq!(
            LinkKind::of("ed2k://|file|x.mkv|123|ABC|/"),
            Some(LinkKind::Ed2k)
        );
        assert_eq!(
            LinkKind::of("https://www.bilibili.com/video/BV1xx"),
            Some(LinkKind::VideoSite)
        );
        assert_eq!(
            LinkKind::of("https://example.com/a.zip"),
            Some(LinkKind::Http)
        );
        assert_eq!(
            LinkKind::of("https://example.com/movie.torrent"),
            Some(LinkKind::TorrentFile)
        );
        assert_eq!(LinkKind::of("随便一段话"), None);
    }

    #[test]
    fn local_torrent_path() {
        // 拖进来的就是这种：以 .torrent 结尾的本地路径必须判成种子文件
        let dir = std::env::temp_dir().join("shandian_links_test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("Three.Billboards.2017.1080p.torrent");
        std::fs::write(&p, b"d").unwrap();
        assert_eq!(
            LinkKind::of(p.to_str().unwrap()),
            Some(LinkKind::TorrentFile)
        );
        // 不存在的 .torrent 字符串不该被当成文件
        let missing = dir.join("definitely-missing.torrent");
        assert_eq!(LinkKind::of(missing.to_str().unwrap()), None);
    }

    #[test]
    fn extract_dedup() {
        let t = "看这个 magnet:?xt=urn:btih:aaa 还有 https://a.com/b.zip 再来一次 magnet:?xt=urn:btih:aaa";
        let got = extract(t);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn titles() {
        assert_eq!(
            pretty_title("Conclave.2024.1080p.AMZN.WEBRip-GalaxyRG.mkv"),
            "Conclave 2024"
        );
        assert_eq!(pretty_title("2001太空漫游.1968.1080p.mkv"), "2001太空漫游 1968");
        assert_eq!(pretty_title("JungleBook.mp4"), "Jungle Book");
    }

    #[test]
    fn pick_main_video() {
        let f = |n: &str, s: u64| FileMeta {
            name: n.to_string(),
            size: s,
        };
        let files = vec![
            f("Movie.2024.1080p.mkv", 5_000),
            f("Movie.2024.720p.mp4", 1_000), // 体积不足 40%，低清副本
            f("sample.mkv", 50),
            f("poster.jpg", 10),
            f("Movie.srt", 1),
        ];
        let picked = pick_files(&files).unwrap();
        assert_eq!(picked, vec![0, 4]);
    }

    #[test]
    fn sizes() {
        assert_eq!(bytes_text(1024), "1.00 KB");
        assert_eq!(bytes_text(1_073_741_824), "1.00 GB");
        assert_eq!(speed_text(500), "0 KB/s");
    }
}

#[cfg(test)]
mod cloud_direct_tests {
    #[test]
    fn share_links_become_direct() {
        assert_eq!(super::cloud_direct("https://www.dropbox.com/scl/fi/abc/f.zip?rlkey=k&dl=0"), "https://www.dropbox.com/scl/fi/abc/f.zip?rlkey=k&dl=1");
        assert_eq!(super::cloud_direct("https://drive.google.com/file/d/XYZ/view?usp=sharing"), "https://drive.usercontent.google.com/download?id=XYZ&export=download&confirm=t");
        assert_eq!(super::cloud_direct("https://example.com/a?dl=0"), "https://example.com/a?dl=0");
    }
}
