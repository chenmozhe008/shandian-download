// 缩略图：视频网站用站点给的封面，本地视频文件用 ffmpeg 截一帧。
//
// 和 Mac 版 Thumbnails.swift 一个思路：都缓存在 <数据目录>/cache/thumbs/<任务 id>.jpg，
// 有缓存就不再算。区别是返回值走 data URI —— WebView2 里没有 Mac 的 NSImage，
// 交给 <img src> 最省事，也不用给 Tauri 开 asset 协议。
//
// 为什么不引 base64 依赖：就为编码一张图，二十行就够了。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use regex::Regex;

/// 能截帧的扩展名。BT 下载的「电影」里混着字幕、nfo，别拿它们跑 ffmpeg
const VIDEO_EXT: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "flv", "wmv", "webm", "m4v", "ts", "rmvb", "mpg", "mpeg", "m2ts",
];

pub fn is_video(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXT.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn cache_path(base: &Path, id: &str) -> PathBuf {
    crate::migrate::local_path(base, "thumbs").join(format!("{id}.jpg"))
}

pub fn cached(base: &Path, id: &str) -> Option<String> {
    let p = cache_path(base, id);
    let meta = std::fs::metadata(&p).ok()?;
    if meta.len() < 2000 {
        return None;
    }
    let bytes = std::fs::read(&p).ok()?;
    Some(to_data_uri(&bytes))
}

/// 视频任务截的帧单独存：截帧只是原封面取不到时的替补，以后取到了要能换掉
pub fn frame_id(id: &str) -> String {
    format!("{id}.frame")
}

/// 这条任务的海报是按「名称 + 年份」核对过的（旧版只看年份，会配错片子）
fn poster_mark(base: &Path, id: &str) -> PathBuf {
    crate::migrate::local_path(base, "thumbs").join(format!("{id}.poster-ok"))
}

/// 旧版查到的海报作废，让它重新查一次。截帧（Lavc）不算海报，种子自带的图在这之前就返回了
pub fn drop_unchecked_poster(base: &Path, id: &str) {
    let p = cache_path(base, id);
    if poster_mark(base, id).exists() {
        return;
    }
    let Ok(bytes) = std::fs::read(&p) else { return };
    let head = &bytes[..bytes.len().min(256)];
    if !head.windows(4).any(|w| w == b"Lavc") {
        let _ = std::fs::remove_file(&p);
    }
}

/// 这条任务再也用不上封面了：原封面和截帧替补一起删
pub fn remove(base: &Path, id: &str) {
    let _ = std::fs::remove_file(cache_path(base, id));
    let _ = std::fs::remove_file(poster_mark(base, id));
    let _ = std::fs::remove_file(cache_path(base, &frame_id(id)));
}

/// 老版本把截帧和原封面存在同一个文件里。ffmpeg 写的 JPEG 头部带编码器标记 Lavc，
/// 认出来就挪成替补，这条任务才有机会换回原封面
pub fn demote_old_frame(base: &Path, id: &str) {
    let p = cache_path(base, id);
    let Ok(bytes) = std::fs::read(&p) else { return };
    let head = &bytes[..bytes.len().min(256)];
    if head.windows(4).any(|w| w == b"Lavc") {
        let _ = std::fs::rename(&p, cache_path(base, &frame_id(id)));
    }
}

/// 找 ffmpeg：优先用工具目录里下载的那份，其次看系统里有没有
pub fn ffmpeg(bin_dir: &Path) -> Option<PathBuf> {
    let local = bin_dir.join("ffmpeg.exe");
    if local.exists() {
        return Some(local);
    }
    let probe = Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .status();
    match probe {
        Ok(s) if s.success() => Some(PathBuf::from("ffmpeg")),
        _ => None,
    }
}

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// 从本地视频文件截一帧存进缓存。
/// 片头常是黑场或片商标，先试第 3 分钟，短片再退到第 10 秒和开头。
pub fn frame(base: &Path, id: &str, video: &Path, ffmpeg: &Path) -> Option<String> {
    if !video.exists() {
        return None;
    }
    let out = cache_path(base, id);
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    for ss in ["180", "10", "0"] {
        let _ = std::fs::remove_file(&out);
        let st = Command::new(ffmpeg)
            .args(["-v", "error", "-y", "-ss", ss, "-i"])
            .arg(video)
            .args([
                "-frames:v",
                "1",
                "-vf",
                "scale=480:-2",
                "-q:v",
                "4",
            ])
            .arg(&out)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status();
        if st.map(|s| s.success()).unwrap_or(false) {
            if let Ok(meta) = std::fs::metadata(&out) {
                if meta.len() > 2000 {
                    break;
                }
            }
        }
    }
    cached(base, id)
}

/// The batch cover and its corresponding drawer item use the same cached frame.
pub(crate) fn baidu_frame(base: &Path, bin: &Path, job: &str, video: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    use std::os::windows::fs::MetadataExt;
    static GENERATING: Mutex<()> = Mutex::new(());
    let metadata = std::fs::metadata(video).ok()?;
    let path_hash = Sha256::digest(video.to_string_lossy().as_bytes());
    let key = format!("bd-{job}-{path_hash:x}-{}-{}", metadata.len(), metadata.last_write_time());
    let _serial = GENERATING.lock().ok()?;
    cached(base, &key).or_else(|| frame(base, &key, video, &ffmpeg(bin)?))
}

/// 种子自带的封面：发布者放进去的海报最准。files 是相对资源目录的路径。
/// 名字像封面的优先；没有的话，全种子只有一张图也算（多张多半是截图，不猜）。
pub fn pick_torrent_image(files: &[PathBuf]) -> Option<&PathBuf> {
    const NAMES: &[&str] = &["poster", "cover", "folder", "front", "thumb", "thumbnail", "fanart", "海报", "封面"];
    let images: Vec<&PathBuf> = files
        .iter()
        .filter(|f| {
            let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            ["jpg", "jpeg", "png", "webp"].contains(&ext.as_str())
        })
        .collect();
    let named = images.iter().find(|f| {
        let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
        NAMES.iter().any(|n| stem == *n || stem.starts_with(&format!("{n}.")) || stem.starts_with(&format!("{n}-")) || stem.starts_with(&format!("{n}_")))
    });
    named.copied().or(if images.len() == 1 { images.first().copied() } else { None })
}

/// 图片下完整了吗。BT 的文件会先占好位置再慢慢填，大小对也不代表下完了，得看结尾标记
fn image_complete(b: &[u8]) -> bool {
    if b.len() < 2000 {
        return false;
    }
    let tail = &b[b.len().saturating_sub(64)..];
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return tail.windows(2).any(|w| w == [0xFF, 0xD9]);
    }
    if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        return tail.windows(4).any(|w| w == b"IEND");
    }
    if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        let declared = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize + 8;
        // RIFF 只在头里写长度：还得看最后一段不是占位的 0
        return declared == b.len() && tail.iter().any(|x| *x != 0);
    }
    false
}

/// 把种子里的封面存成这条任务的封面。图还没下完返回 None，之后再来
pub fn from_torrent(base: &Path, id: &str, image: &Path) -> Option<String> {
    let bytes = std::fs::read(image).ok()?;
    if !image_complete(&bytes) {
        return None;
    }
    let out = cache_path(base, id);
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&out, &bytes).ok()?;
    Some(to_data_uri(&bytes))
}

#[derive(serde::Deserialize)]
struct PosterHit {
    img: Option<String>,
    #[serde(rename = "type")]
    ty: Option<String>,
    year: Option<String>,
    title: Option<String>,
    sub_title: Option<String>,
}

/// 只留字母数字和汉字，判断豆瓣返回的是不是同一部片子
fn title_key(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// 宁缺毋滥（同 Mac 版 Thumbnails.pick）：名称要对得上，带年份还要年份对得上。
/// 年份信息缺失的只接受名称完全一致；没年份时同名的不止一部就不猜
fn pick_poster<'a>(hits: &'a [PosterHit], query: &str, year: Option<&str>) -> Option<&'a PosterHit> {
    let q = title_key(query);
    let names = |h: &PosterHit| {
        [h.title.as_deref(), h.sub_title.as_deref()]
            .into_iter()
            .flatten()
            .map(title_key)
            .collect::<Vec<_>>()
    };
    let named: Vec<&PosterHit> = hits
        .iter()
        .filter(|h| {
            let img = h.img.as_deref().unwrap_or("");
            !img.is_empty()
                && !img.contains("default")
                && matches!(h.ty.as_deref(), Some("movie") | Some("tv"))
        })
        .filter(|h| {
            names(h).iter().any(|n| {
                let (nl, ql) = (n.chars().count(), q.chars().count());
                nl >= 2 && ql >= 2 && (q.contains(n.as_str()) || (nl >= 4 && n.contains(q.as_str())))
            })
        })
        .collect();
    match year {
        Some(y) => named
            .iter()
            .find(|h| h.year.as_deref() == Some(y))
            .or_else(|| {
                named.iter().find(|h| {
                    h.year.as_deref().unwrap_or("").is_empty() && names(h).iter().any(|n| *n == q)
                })
            })
            .copied(),
        None if named.len() == 1 => Some(named[0]),
        None => None,
    }
}

/// 电影海报：用片名查豆瓣，名称和年份都核对过才用，取中号海报。
/// 完整片名（中文名 + 英文名）豆瓣常搜不到，搜不到再拿纯中文名试一次。
/// 磁力／种子下的「电影」类目用它，比截帧更像海报。
pub fn douban(base: &Path, id: &str, title: &str) -> Option<String> {
    static YEAR: OnceLock<Regex> = OnceLock::new();
    let year_re = YEAR.get_or_init(|| Regex::new(r"(19|20)\d{2}").unwrap());
    let year = year_re.find(title).map(|m| m.as_str().to_string());
    // 查词去掉年份（Conclave 2024 → Conclave），片名开头的数字不算年份的场景靠查询宽容度兜
    let query = year_re.replace_all(title, " ").split_whitespace().collect::<Vec<_>>().join(" ");
    if query.is_empty() {
        return None;
    }

    let mut builder = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36");
    if let Some(p) = crate::proxy::url_for("https://movie.douban.com/") {
        if let Ok(px) = reqwest::Proxy::all(&p) {
            builder = builder.proxy(px);
        }
    }
    let client = builder.build().ok()?;

    let cjk: String = query.chars().filter(|c| ('\u{4E00}'..='\u{9FFF}').contains(c) || ('\u{3400}'..='\u{4DBF}').contains(c)).collect();
    let mut queries = vec![query.clone()];
    if cjk != query && cjk.chars().count() >= 2 {
        queries.push(cjk);
    }
    let raw = queries.iter().find_map(|q| {
        let hits: Vec<PosterHit> = client
            .get("https://movie.douban.com/j/subject_suggest")
            .query(&[("q", q)])
            .send()
            .ok()?
            .json()
            .ok()?;
        // 豆瓣图床要带来源页；s_ratio_poster 是小图，换 m_ratio_poster 中号
        let hit = pick_poster(&hits, q, year.as_deref())?;
        Some(hit.img.as_deref()?.replace("s_ratio_poster", "m_ratio_poster"))
    })?;
    let bytes = client
        .get(&raw)
        .header("Referer", "https://movie.douban.com/")
        .send()
        .ok()?
        .bytes()
        .ok()?;
    if bytes.len() < 2000 {
        return None;
    }
    let out = cache_path(base, id);
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&out, &bytes).ok()?;
    let _ = std::fs::write(poster_mark(base, id), "");
    Some(to_data_uri(&bytes))
}

/// 下载站点封面。有的站（小红书）给的是 http://，系统默认拦不加密地址，换成 https。
pub fn remote(base: &Path, id: &str, url: &str) -> Option<String> {    let raw = if let Some(rest) = url.strip_prefix("http://") {
        format!("https://{rest}")
    } else {
        url.to_string()
    };
    // reqwest 不认 Windows 注册表的系统代理：YouTube 封面（i.ytimg.com）这类境外图床
    // 直连会超时，任务列表里就一直没封面。要走代理的显式挂上。
    let mut builder = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36");
    if let Some(p) = crate::proxy::url_for(&raw) {
        if let Ok(px) = reqwest::Proxy::all(&p) {
            builder = builder.proxy(px);
        }
    }
    let client = builder.build().ok()?;
    // B 站图床不带 Referer 会 403，按域名补一个
    let referer = if raw.contains("hdslb.com") || raw.contains("bilibili") {
        Some("https://www.bilibili.com/")
    } else if raw.contains("douyinpic") || raw.contains("douyin") {
        Some("https://www.douyin.com/")
    } else {
        None
    };
    let mut req = client.get(&raw);
    if let Some(r) = referer {
        req = req.header("Referer", r);
    }
    // 404 也可能回一个几 KB 的错误页，不查状态码会被当成图片存下来，之后一直显示不出
    let bytes = req.send().ok()?.error_for_status().ok()?.bytes().ok()?;
    if bytes.len() < 2000 {
        return None;
    }
    let out = cache_path(base, id);
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&out, &bytes).ok()?;
    Some(to_data_uri(&bytes))
}

fn to_data_uri(bytes: &[u8]) -> String {
    // YouTube 现在给的是 webp 封面（i.ytimg.com/vi_webp/...），扩展名虽叫 .jpg，
    // 但字节是 RIFF/WEBP。MIME 写死 image/jpeg 会让 WebView2 解码失败显示占位卡，
    // 所以按 magic bytes 认真实格式。
    let mime = if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else {
        "image/jpeg"
    };
    format!("data:{mime};base64,{}", base64(bytes))
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poster_checks_name_and_year() {
        let hit = |t: &str, y: &str| PosterHit {
            img: Some("https://x/s_ratio_poster/1.jpg".into()),
            ty: Some("movie".into()),
            year: Some(y.into()),
            title: Some(t.into()),
            sub_title: None,
        };
        // 年份对、名称不对：旧版会配错的情况
        assert!(pick_poster(&[hit("别的片子", "2024")], "Conclave", Some("2024")).is_none());
        let good = [hit("别的片子", "2024"), hit("Conclave", "2024")];
        assert_eq!(pick_poster(&good, "Conclave", Some("2024")).unwrap().title.as_deref(), Some("Conclave"));
        // 名称对、年份不对
        assert!(pick_poster(&[hit("Conclave", "1999")], "Conclave", Some("2024")).is_none());
        // 没年份、同名两部不猜
        assert!(pick_poster(&[hit("Dune", "1984"), hit("Dune", "2021")], "Dune", None).is_none());
    }

    #[test]
    fn b64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn video_ext() {
        assert!(is_video("A.mkv"));
        assert!(is_video("a/b/c.MP4"));
        assert!(!is_video("a.srt"));
        assert!(!is_video("noext"));
    }

    #[test]
    fn data_uri_mime() {
        // YouTube 的 .jpg 缓存实际是 webp，必须声明 image/webp
        let webp = b"RIFF\x00\x00\x00\x00WEBPVP8 ";
        assert!(to_data_uri(webp).starts_with("data:image/webp;base64,"));
        let jpeg = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0];
        assert!(to_data_uri(jpeg).starts_with("data:image/jpeg;base64,"));
        let png = &[0x89, b'P', b'N', b'G', 0, 0, 0, 0];
        assert!(to_data_uri(png).starts_with("data:image/png;base64,"));
    }
}

#[cfg(test)]
mod poster_tests {
    use super::*;

    #[test]
    fn picks_named_image_or_the_only_one() {
        let p = |v: &[&str]| v.iter().map(PathBuf::from).collect::<Vec<_>>();
        let a = p(&["Sintel.mp4", "shots/1.jpg", "poster.jpg", "shots/2.jpg"]);
        assert_eq!(pick_torrent_image(&a), Some(&PathBuf::from("poster.jpg")));
        let b = p(&["movie.mkv", "Cover-Large.PNG"]);
        assert_eq!(pick_torrent_image(&b), Some(&PathBuf::from("Cover-Large.PNG")));
        let c = p(&["movie.mkv", "site-logo.jpg"]);
        assert_eq!(pick_torrent_image(&c), Some(&PathBuf::from("site-logo.jpg")));
        // 一堆截图，猜不出哪张是封面
        let d = p(&["movie.mkv", "s1.jpg", "s2.jpg"]);
        assert_eq!(pick_torrent_image(&d), None);
        // "postergirl" 不是海报
        let e = p(&["m.mkv", "postergirl.jpg", "a.jpg"]);
        assert_eq!(pick_torrent_image(&e), None);
    }

    #[test]
    fn half_downloaded_image_is_rejected() {
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE0];
        jpg.resize(4000, 7);
        assert!(!image_complete(&jpg), "没有结束标记");
        jpg.extend_from_slice(&[0xFF, 0xD9]);
        assert!(image_complete(&jpg));
        let mut holes = vec![0xFF, 0xD8, 0xFF, 0xE0];
        holes.resize(4000, 0); // 占位后还没填的部分全是 0
        assert!(!image_complete(&holes));
    }
}
