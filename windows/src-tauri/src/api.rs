// 跟内核 REST 说话。接口与 api.py 一致（那是调试用的 Python 版，行为已在内核上验证过）。
//
// 一个必须注意的点：本机如果开了全局代理，环境变量 HTTP_PROXY 会让请求走代理，
// 结果连 127.0.0.1 都被代理拦掉（实测报 502 upstream connect failed）。
// 所以 Client 必须 no_proxy()。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug)]
pub struct ApiError(pub String);

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        ApiError(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ApiError>;

/// 内核返回的外壳：{"code":0,"msg":"","data":...}
#[derive(Deserialize)]
struct Envelope<T> {
    code: i32,
    #[serde(default)]
    msg: String,
    data: Option<T>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Task {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub progress: Option<Progress>,
    /// 资源信息（名字、总大小、文件列表）。在内核返回里它挂在任务顶层，不在 progress 下面
    #[serde(default)]
    pub meta: Option<TaskMeta>,
    #[serde(default, rename = "shandianError")]
    pub shandian_error: Option<String>,
    /// 内核的校验状态（补丁字段）：checking / failed
    #[serde(default, rename = "shandianVerification", skip_serializing_if = "Option::is_none")]
    pub shandian_verification: Option<String>,
    /// 由 meta.req.url 推出来的类型名（磁力／种子文件／ed2k／视频网站／直链）。
    /// 内核不返回这一项，list_tasks 里补上，界面上「磁力 2.31 GB」那个标签用它。
    #[serde(default, skip_deserializing)]
    pub kind: Option<String>,
    /// 迅雷任务行上的「已下载 a/b 项」
    #[serde(default, skip_deserializing, rename = "xlItems", skip_serializing_if = "Option::is_none")]
    pub xl_items: Option<(u32, u32)>,
    /// 字幕语言标签（"中英" 这类）。只有勾了字幕的视频任务才有
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub subs: Option<String>,
    /// 下完的视频站任务现在没有字幕文件（当初没下到、或后来被删了），但站点提供字幕：
    /// 右键菜单给「获取字幕」，不给字幕样式、润色这些
    #[serde(default, skip_deserializing, rename = "subsFetchable", skip_serializing_if = "std::ops::Not::not")]
    pub subs_fetchable: bool,
    /// 已完成视频的字幕检查结果：available / none / failed。
    #[serde(default, rename = "subtitleState", skip_serializing_if = "Option::is_none")]
    pub subtitle_state: Option<String>,
    /// 分辨率标签（"1080P" 这类）。内核不返回：视频任务取 yt-dlp 的 %(height)s，
    /// 内核任务从资源名/文件名里提取；没有就不输出（前端按 null/缺省处理）。
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    /// 任务创建时刻（毫秒）。内核返回的字段叫 createdAt，形状是 Go time.Time 的
    /// RFC3339 字符串（2026-10-30T06:08:54.4263726+08:00），这里解析成毫秒；
    /// 给前端序列化出来的名字是 created，和 VideoJob.created 同名同单位。
    /// 解析不出来的（老数据、零值时间）留 None，list_tasks 里再按完成态用文件 mtime 兜底。
    #[serde(default, rename(deserialize = "createdAt"), deserialize_with = "de_created")]
    pub created: Option<u64>,
    /// 已完成页排序用。内核任务取 updatedAt（下完后不再变），视频任务取完成时记下的时间
    #[serde(default, rename(deserialize = "updatedAt", serialize = "finished"), deserialize_with = "de_created")]
    pub finished: Option<u64>,
    /// 字幕润色的状态文字（只有视频任务有）
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub polish: Option<String>,
    #[serde(default, skip_deserializing)]
    pub polished: bool,
    /// 已完成但本地文件不在了（被手动删掉/移走）。对齐 Mac 的 fileGone：
    /// 只在「盘还在、文件没了」时算失效，移动硬盘拔下来不算，免得满屏「已删除」。
    /// NAS 任务由网关判断后直接带过来；本机任务在 list_tasks 里算
    #[serde(default)]
    pub missing: bool,
    /// NAS 服务端的文件检查结果：present / missing / unknown。
    #[serde(default, rename = "fileState", skip_serializing_if = "Option::is_none")]
    pub file_state: Option<String>,
    /// 视频任务收尾阶段的状态文字（正在合并 / 处理字幕），下载中没有
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// 目录里已有同名文件，yt-dlp 跳过了下载，等用户决定要不要再下一份
    #[serde(default, skip_deserializing)]
    pub collided: bool,
}

/// createdAt → 毫秒。null / 零值时间（0001-01-01…）都当没有
fn de_created<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<u64>, D::Error> {
    let s = Option::<String>::deserialize(d)?;
    Ok(s.as_deref().and_then(rfc3339_ms))
}

/// RFC3339 → 毫秒。没引时间库，历法换算手算：把 1、2 月当作上一年的 13、14 月，
/// 算出 1970-01-01 起的天数（Howard Hinnant 的 civil calendar 算法）。
fn rfc3339_ms(s: &str) -> Option<u64> {
    // 形如 2026-10-30T06:08:54.4263726+08:00 或 …Z
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || (b[10] != b'T' && b[10] != b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |a: usize, z: usize| -> Option<i64> { s.get(a..z)?.parse().ok() };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, min, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut i = 19;
    let mut frac_ms = 0i64;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        let mut end = start;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
        }
        let digits = &s[start..end];
        // 前三位小数是毫秒，Go 写出来最多 9 位纳秒
        let ms: i64 = digits.chars().take(3).collect::<String>().parse().unwrap_or(0);
        frac_ms = match digits.len() {
            1 => ms * 100,
            2 => ms * 10,
            n if n >= 3 => ms,
            _ => 0,
        };
        i = end;
    }
    let tz = match b.get(i) {
        Some(b'Z') | Some(b'z') => 0,
        Some(&c) if c == b'+' || c == b'-' => {
            if b.len() < i + 6 || b[i + 3] != b':' {
                return None;
            }
            let (th, tm) = (num(i + 1, i + 3)?, num(i + 4, i + 6)?);
            let off = th * 3600 + tm * 60;
            if c == b'+' { -off } else { off }
        }
        _ => return None,
    };
    let (y, m) = if month <= 2 {
        (year - 1, month + 12)
    } else {
        (year, month)
    };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m - 3) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let ts = days * 86400 + hour * 3600 + min * 60 + sec + tz;
    let ms = ts * 1000 + frac_ms;
    // 零值时间算出来是负数，当没有
    if ms <= 0 {
        return None;
    }
    Some(ms as u64)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Progress {
    #[serde(default)]
    pub downloaded: u64,
    #[serde(default)]
    pub speed: u64,
    #[serde(default)]
    pub uploaded: u64,
    #[serde(default, rename = "uploadSpeed")]
    pub upload_speed: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TaskMeta {
    #[serde(default)]
    pub res: Option<Resource>,
    #[serde(default)]
    pub req: Option<Request>,
    /// 投任务时的保存位置和文件名。用户在确认框里改过目录时，只有这里是准的
    #[serde(default)]
    pub opts: Option<TaskOpts>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TaskOpts {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    /// 种子里选中的文件（res.files 的下标）。空 = 全部
    #[serde(default, rename = "selectFiles", skip_serializing_if = "Vec::is_empty")]
    pub select_files: Vec<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Request {
    #[serde(default, deserialize_with = "de_request_labels", skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub labels: std::collections::HashMap<String,String>,
    #[serde(default)]
    pub url: String,
    /// 多文件任务的文件列表（带鉴权信息）：只在后端用，不发给界面
    #[serde(default, skip_serializing)]
    pub extra: serde_json::Value,
}

// Go encodes a nil map as null; older requests may omit labels entirely.
fn de_request_labels<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<std::collections::HashMap<String,String>, D::Error> {
    Ok(Option::<std::collections::HashMap<String,String>>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Resource {
    #[serde(default)]
    pub name: String,
    /// BT 任务的 infohash
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileEntry {
    #[serde(default)]
    pub name: String,
    /// 种子里的子目录（相对资源目录）
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub size: u64,
}

impl Task {
    pub fn total(&self) -> u64 {
        self.meta
            .as_ref()
            .and_then(|m| m.res.as_ref())
            .map(|r| r.size)
            .unwrap_or(0)
    }

    /// 投任务时指定的保存目录。老任务或没记录时返回 None
    pub fn saved_dir(&self) -> Option<std::path::PathBuf> {
        let p = self.meta.as_ref()?.opts.as_ref()?.path.trim().to_string();
        (!p.is_empty()).then(|| std::path::PathBuf::from(p))
    }

    /// 磁力、种子的内容文件夹名（资源名）。单文件种子直接落在保存目录，没有文件夹
    pub fn content_folder(&self) -> Option<String> {
        use crate::links::LinkKind;
        if self.url().starts_with("shandian-multi:") {
            let name=&self.meta.as_ref()?.res.as_ref()?.name;
            return (!name.is_empty()).then(||name.clone());
        }
        if !matches!(LinkKind::of(&self.url()), Some(LinkKind::Magnet | LinkKind::TorrentFile)) {
            return None;
        }
        let res = self.meta.as_ref()?.res.as_ref()?;
        let folder = res.files.len() > 1 || res.files.iter().any(|f| !f.path.is_empty() || f.name != res.name);
        (folder && !res.name.is_empty()).then(|| res.name.clone())
    }

    /// 原始链接。核心里叫 meta.req.url，用它反推任务类型。
    /// GitHub 任务在内核里可能是加速站地址，这里还原成用户给的链接
    pub fn url(&self) -> String {
        self.meta
            .as_ref()
            .and_then(|m| m.req.as_ref())
            .map(|r| crate::ghmirror::original(&r.url))
            .unwrap_or_default()
    }

    /// 补上类型标签（内核不返回，界面上要显示「磁力」「种子文件」这类）
    pub fn tag_kind(&mut self) {
        let url = self.url();
        let req = self.meta.as_ref().and_then(|m|m.req.as_ref());
        let recorded = req.and_then(|r|r.labels.get("shandian.source")).filter(|s| !s.is_empty() && s.as_str() != "直链" && s.as_str() != "网盘").cloned();
        self.kind = recorded.or_else(||req.and_then(|r|request_label(&url,&r.extra))).or_else(|| {
            if url.starts_with("shandian-multi:") { Some("网盘".into()) } else { download_label(&url) }
        });
    }

    pub fn downloaded(&self) -> u64 {
        self.progress.as_ref().map(|p| p.downloaded).unwrap_or(0)
    }

    pub fn speed(&self) -> u64 {
        self.progress.as_ref().map(|p| p.speed).unwrap_or(0)
    }

    /// 种子里的文件列表，交给 links::pick_files 决定勾哪些
    pub fn files(&self) -> Vec<crate::links::FileMeta> {
        self.meta
            .as_ref()
            .and_then(|m| m.res.as_ref())
            .map(|r| {
                r.files
                    .iter()
                    .map(|f| crate::links::FileMeta {
                        name: f.name.clone(),
                        size: f.size,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// 确认框与任务列表使用同一份来源标签，协议分类仍由 LinkKind 决定。
pub(crate) fn material_site_label(url: &str) -> Option<&'static str> {
    let u=url::Url::parse(url).ok()?;let host=u.host_str()?;
    if host=="pixabay.com" || host.ends_with(".pixabay.com"){Some("Pixabay")}
    else if host=="aigei.com" || host.ends_with(".aigei.com"){Some("爱给")}
    else if host=="docs.qingque.cn"{Some("轻雀文档")}
    else if crate::feishu::document_url(url).is_some() || crate::feishu::attachment_url(&u){Some("飞书文档")}
    else{None}
}

// Source identity survives CDN URLs without changing the transport or credentials.
fn request_label(url: &str, extra: &Value) -> Option<String> {
    let specific = |value: &str| download_label(value).filter(|s| s != "直链" && s != "视频网站" && s != "网盘");
    let file = &extra["files"][0];
    specific(file["url"].as_str().unwrap_or(url)).or_else(|| {
        let headers = file["header"].as_object().or_else(||extra["header"].as_object())?;
        headers.iter().filter(|(k,_)|k.eq_ignore_ascii_case("referer") || k.eq_ignore_ascii_case("origin"))
            .find_map(|(_,v)|v.as_str().and_then(specific))
    })
}

pub(crate) fn download_label(url: &str) -> Option<String> {
    if let Some(site) = crate::videos::known_site_name(url) { return Some(site.to_string()); }
    material_site_label(url).map(String::from).or_else(|| crate::sources::lookup(url))
        .or_else(|| netdisk_label(url).map(String::from))
        .or_else(|| crate::links::LinkKind::of(url).map(|k| k.label().to_string()))
}

/// 从直链的域名认网盘。只列网盘专用的域名：阿里云 OSS 这类通用存储会把普通下载也标错
pub(crate) fn netdisk_label(url: &str) -> Option<&'static str> {
    let host = url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_lowercase)).unwrap_or_default();
    const NAMES: &[(&[&str], &str)] = &[
        (&["baidupcs.com", "pan.baidu.com", "pcs.baidu.com"], "百度网盘"),
        (&["drive.quark.cn", "pan.quark.cn", "drive-pc.quark.cn"], "夸克网盘"),
        (&["pan.xunlei.com", "sandai.net", "xunlei.com"], "迅雷云盘"),
        (&["aliyundrive.com", "alipan.com", "aliyundrive.net"], "阿里云盘"),
        (&["115.com", "115cdn.net", "115cdn.com"], "115 网盘"),
        (&["cloud.189.cn", "ctyunxs.cn"], "天翼云盘"),
        (&["caiyun.com", "yun.139.com", "caiyun.139.com", "mcloud.139.com"], "移动云盘"),
        (&["123pan.com", "123pan.cn", "123684.com", "123865.com", "123952.com", "123912.com", "123957.com"], "123 云盘"),
        (&["drive.uc.cn", "drive-pc.uc.cn", "pds.uc.cn"], "UC 网盘"),
        (&["guangyapan.com", "guangya.com"], "光鸭云盘"),
        (&["lanzou.com", "lanzoui.com", "lanzoux.com"], "蓝奏云"),
        (&["mypikpak.com"], "PikPak"),
        // 不是网盘，但下载来源一眼能认：Release 附件、源码包、raw 文件（跳转后的 githubusercontent 也算）
        (&["github.com", "githubusercontent.com", "githubassets.com"], "GitHub"),
        (&["gitee.com"], "Gitee"),
        (&["dropbox.com", "dropboxusercontent.com"], "Dropbox"),
        (&["drive.google.com", "drive.usercontent.google.com"], "Google Drive"),
        (&["1drv.ms", "onedrive.live.com", "sharepoint.com"], "OneDrive"),
        (&["mega.nz", "mega.io"], "MEGA"),
    ];
    NAMES
        .iter()
        .find(|(ds, _)| ds.iter().any(|d| host == *d || host.ends_with(&format!(".{d}"))))
        .map(|(_, n)| *n)
}

#[derive(Debug, Clone, Deserialize)]
pub struct MultiFile {
    pub url: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct Api {
    base: String,
    token: String,
    client: reqwest::Client,
}

impl Api {
    pub fn new(port: u16, token: String) -> Self {
        let client = reqwest::Client::builder()
            // 关键：绕开系统代理，否则 127.0.0.1 会被代理拦掉
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            base: format!("http://127.0.0.1:{port}/api/v1"),
            token,
            client,
        }
    }

    /// 发请求并拿到响应原文。解析分两种：要 data 的和不要 data 的。
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<String> {
        let mut req = self
            .client
            .request(method, format!("{}/{}", self.base, path))
            .header("X-Api-Token", &self.token);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?;
        Ok(resp.text().await?)
    }

    fn decode<T: for<'de> Deserialize<'de>>(text: &str) -> Result<Envelope<T>> {
        serde_json::from_str(text).map_err(|e| {
            ApiError(format!(
                "解析失败：{e}；原文：{}",
                text.chars().take(200).collect::<String>()
            ))
        })
    }

    async fn call<T: for<'de> Deserialize<'de>>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<T> {
        let text = self.send(method, path, body).await?;
        let env: Envelope<T> = Self::decode(&text)?;
        if env.code != 0 {
            return Err(ApiError(format!("内核返回错误 {}：{}", env.code, env.msg)));
        }
        env.data
            .ok_or_else(|| ApiError("内核没有返回 data".to_string()))
    }

    /// 不关心返回体的接口。注意：很多写接口成功时 data 是 null，
    /// 不能走 call（那样会把 null 当成失败）。
    async fn call_ok(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<()> {
        let text = self.send(method, path, body).await?;
        let env: Envelope<Value> = Self::decode(&text)?;
        if env.code != 0 {
            return Err(ApiError(format!("内核返回错误 {}：{}", env.code, env.msg)));
        }
        Ok(())
    }

    /// 探活。内核刚起来时前几次会连接失败，调用方负责重试。
    pub async fn info(&self) -> Result<Value> {
        self.call(reqwest::Method::GET, "info", None).await
    }

    pub async fn config(&self) -> Result<Value> {
        self.call(reqwest::Method::GET, "config", None).await
    }

    /// 写回整份配置。成功时 data 是 null，所以走 call_ok。
    pub async fn put_config(&self, cfg: &Value) -> Result<()> {
        self.call_ok(reqwest::Method::PUT, "config", Some(cfg)).await
    }

    pub async fn list_tasks(&self) -> Result<Vec<Task>> {
        let v: Option<Vec<Task>> = self.call(reqwest::Method::GET, "tasks", None).await?;
        Ok(v.unwrap_or_default())
    }

    /// name 非空时内核会用它当任务名（也会影响落盘文件名）。
    /// 用户在「新建下载」里改过名字就走这条，没改就留空让内核自己取。
    /// headers 来自浏览器（扩展把 Cookie / UA / Referer 一起交过来），网盘直链缺了它会 403。
    /// 内核认的形状是 `req.extra.header`，扁平的 名字→值 字符串表。
    /// 只解析不创建：拿磁力、种子里的文件列表（probe=1 不留缓存）
    pub async fn resolve_files(&self, url: &str) -> Result<Value> {
        // 普通请求 30 秒超时，磁力的元数据要从网上找，常常更久
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| ApiError(e.to_string()))?;
        let text = client
            .post(format!("{}/resolve?probe=1", self.base))
            .header("X-Api-Token", &self.token)
            .json(&json!({ "req": { "url": url } }))
            .send()
            .await
            .map_err(|e| ApiError(e.to_string()))?
            .text()
            .await
            .map_err(|e| ApiError(e.to_string()))?;
        let env: Envelope<Value> = Self::decode(&text)?;
        if env.code != 0 {
            return Err(ApiError(env.msg));
        }
        env.data.ok_or_else(|| ApiError("内核没有返回文件列表".into()))
    }

    pub async fn add_task(
        &self,
        url: &str,
        path: &str,
        name: &str,
        headers: &[(String, String)],
        select: &[usize],
    ) -> Result<String> {
        let mut req = json!({ "url": url });
        if !headers.is_empty() {
            let mut map = serde_json::Map::new();
            for (k, v) in headers {
                map.insert(k.clone(), json!(v));
            }
            req["extra"] = json!({ "header": map });
        }
        if let Some(source) = request_label(url, &req["extra"]) {
            req["labels"] = json!({"shandian.source":source});
        }
        let mut opts = if name.is_empty() {
            json!({ "path": path })
        } else {
            json!({ "path": path, "name": name })
        };
        if !select.is_empty() {
            opts["selectFiles"] = json!(select);
        }
        let body = json!({ "req": req, "opts": opts });
        let id: String = self
            .call(reqwest::Method::POST, "tasks", Some(&body))
            .await?;
        Ok(id)
    }

    /// 多文件任务（内核补丁）：一个文件夹一条任务，每个文件有自己的链接
    pub async fn add_multi(&self, folder: &str, path: &str, files: &[MultiFile]) -> Result<String> {
        let list: Vec<Value> = files
            .iter()
            .map(|f| {
                let header: serde_json::Map<String, Value> = f.headers.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
                json!({ "url": f.url, "name": f.name, "header": header })
            })
            .collect();
        let body = json!({
            "req": { "url": format!("shandian-multi:{folder}"), "extra": { "files": list }, "labels": { "shandian.source": list.first().and_then(|f|request_label(f["url"].as_str().unwrap_or(""), &json!({"header":f["header"]}))).unwrap_or_else(||"网盘".into()) } },
            "opts": { "path": path },
        });
        self.call(reqwest::Method::POST, "tasks", Some(&body)).await
    }

    /// 换下载地址（GitHub 换加速站）。内核保留已下的进度，请求头沿用原来的
    pub async fn patch_url(&self, id: &str, url: &str) -> Result<()> {
        let body = json!({ "req": { "url": url } });
        self.call_ok(reqwest::Method::PATCH, &format!("tasks/{id}"), Some(&body)).await
    }

    pub async fn probe_http(&self, url: &str, extra: &Value) -> Result<Value> {
        self.call(reqwest::Method::POST, "resolve?probe=1", Some(&json!({"req":{"url":url,"extra":extra}}))).await
    }

    pub async fn refresh_http_link(&self, id: &str, url: &str, extra: &Value) -> Result<()> {
        self.call_ok(reqwest::Method::PATCH, &format!("tasks/{id}"), Some(&json!({"req":{"url":url,"extra":extra}}))).await
    }

    pub async fn refresh_multi_links(&self, id: &str, files: &[MultiFile]) -> Result<()> {
        let list:Vec<Value> = files.iter().map(|f| {
            let header:serde_json::Map<String,Value>=f.headers.iter().map(|(k,v)|(k.clone(),json!(v))).collect();
            json!({"url":f.url,"name":f.name,"header":header})
        }).collect();
        self.call_ok(reqwest::Method::PATCH,&format!("tasks/{id}"),Some(&json!({"req":{"extra":{"files":list}}}))).await
    }

    pub async fn select_files(&self, id: &str, select: &[usize]) -> Result<()> {
        let body = json!({ "opts": { "selectFiles": select } });
        self.call_ok(reqwest::Method::PATCH, &format!("tasks/{id}"), Some(&body)).await
    }

    pub async fn stats(&self, id: &str) -> Result<Value> {
        self.call(reqwest::Method::GET, &format!("tasks/{id}/stats"), None).await
    }

    pub async fn pause(&self, id: &str) -> Result<()> {
        self.call_ok(reqwest::Method::PUT, &format!("tasks/{id}/pause"), None)
            .await
    }

    /// 边下边播：告诉内核这个任务要按顺序往后下（补丁接口）
    pub async fn stream(&self, id: &str) -> Result<()> {
        self.call_ok(reqwest::Method::PUT, &format!("tasks/{id}/stream"), None)
            .await
    }

    /// 调整内核等待队列的顺序（补丁接口）。已经在跑的不受影响
    pub async fn set_wait_order(&self, ids: &[String]) -> Result<()> {
        let body = serde_json::json!({ "ids": ids });
        self.call_ok(reqwest::Method::PUT, "tasks/order", Some(&body)).await
    }

    /// 让内核重新校验磁盘上已下好的数据（补丁接口 opts.extra.shandianVerifyExisting）
    pub async fn verify(&self, id: &str) -> Result<()> {
        let body = serde_json::json!({ "opts": { "extra": { "shandianVerifyExisting": true } } });
        self.call_ok(reqwest::Method::PATCH, &format!("tasks/{id}"), Some(&body))
            .await
    }

    /// 一批任务一起继续：内核按同时下载数开始，其余排队
    pub async fn resume_batch(&self, ids: &[String]) -> Result<()> {
        let query: Vec<String> = ids.iter().map(|id| format!("id={id}")).collect();
        self.call_ok(reqwest::Method::PUT, &format!("tasks/continue?{}", query.join("&")), None)
            .await
    }

    pub async fn resume(&self, id: &str) -> Result<()> {
        self.call_ok(reqwest::Method::PUT, &format!("tasks/{id}/continue"), None)
            .await
    }

    /// 删除任务。force=true 是「连下载下来的文件一起删」（内核 doDelete 里
    /// 有名字的资源删整个资源目录、单文件删那一个文件），false 只删任务记录。
    pub async fn delete(&self, id: &str, force: bool) -> Result<()> {
        self.call_ok(
            reqwest::Method::DELETE,
            &format!("tasks/{id}?force={force}"),
            None,
        )
        .await
    }
}

#[cfg(test)]
mod label_tests {
    #[test]
    fn multi_files_keep_their_task_folder() {
        let t: super::Task = serde_json::from_value(serde_json::json!({
            "id":"multi", "meta":{"req":{"url":"shandian-multi:pixabay"},"opts":{"path":"/video"},
                "res":{"name":"pixabay","files":[{"name":"first.mp3","size":100}]}}
        })).unwrap();
        assert_eq!(t.content_folder().as_deref(),Some("pixabay"));
    }

    #[test]
    fn cdn_source_survives_headers_and_labels() {
        let mut task: super::Task = serde_json::from_value(serde_json::json!({
            "id":"cdn", "meta":{"req":{"url":"https://cdn.example.test/a.m2ts", "labels":null,
                "extra":{"header":{"Referer":"https://www.guangyapan.com/"}}}}
        })).unwrap();
        task.tag_kind();assert_eq!(task.kind.as_deref(),Some("光鸭云盘"));
        task.meta.as_mut().unwrap().req.as_mut().unwrap().labels.insert("shandian.source".into(),"天翼云盘".into());
        task.tag_kind();assert_eq!(task.kind.as_deref(),Some("天翼云盘"));
        assert_eq!(super::request_label("shandian-multi:files", &serde_json::json!({"files":[{"url":"https://cdn.example.test/a.zip","header":{"Referer":"https://yun.123pan.cn/"}}]})).as_deref(),Some("123 云盘"));
    }

    #[test]
    fn null_or_missing_request_labels_do_not_hide_task_lists() {
        let mut tasks: Vec<super::Task> = serde_json::from_value(serde_json::json!([
            {"id":"old","status":"done","meta":{"req":{"url":"https://example.test/a.zip","labels":null}}},
            {"id":"legacy","status":"done","meta":{"req":{"url":"https://example.test/b.zip"}}},
            {"id":"cloud","meta":{"req":{"url":"shandian-multi:files","labels":{"shandian.source":"123 云盘"},"extra":{"files":[{"url":"https://cdn.example.test/c.zip"}]}}}}
        ])).expect("null labels must not reject the complete task list");
        assert_eq!(tasks.len(),3);
        assert!(tasks[0].meta.as_ref().unwrap().req.as_ref().unwrap().labels.is_empty());
        assert!(tasks[1].meta.as_ref().unwrap().req.as_ref().unwrap().labels.is_empty());
        tasks[2].tag_kind();
        assert_eq!(tasks[2].kind.as_deref(),Some("123 云盘"));
    }

    #[test]
    fn confirmation_and_task_use_the_same_source_label() {
        for (url, label) in [
            ("https://dl-pc-sz-cf.pds.uc.cn/example.mp4", "UC 网盘"),
            ("https://dl.drive.quark.cn/example.mp4", "夸克网盘"),
            ("https://pan.quark.cn/s/779e5f0a9427#/list/share", "夸克网盘"),
            ("https://drive.uc.cn/s/bfc2f9849c8e4?public=1#/list/share", "UC 网盘"),
            ("https://pan.baidu.com/s/1ExampleShare?pwd=demo", "百度网盘"),
            ("https://www.alipan.com/s/CSm75WGT8ED/folder/example", "阿里云盘"),
            ("https://cn-beijing-data.aliyundrive.net/example.mp4", "阿里云盘"),
            ("https://example.org/example.mp4", "直链"),
            ("https://mpvideo.qpic.cn/lesson.mp4", "微信公众号"),
            ("https://www.bilibili.com/video/BV123", "B 站"),
            ("https://b23.tv/short", "B 站"),
            ("https://www.youtube.com/watch?v=abc", "YouTube"),
            ("https://youtu.be/abc", "YouTube"),
            ("https://player.vimeo.com/video/123", "Vimeo"),
            ("https://www.douyin.com/video/123", "抖音"),
            ("https://www.xiaohongshu.com/explore/123", "小红书"),
            ("https://xhslink.com/short", "小红书"),
            ("https://www.kuaishou.com/short-video/abc", "快手"),
            ("https://weibo.com/123/abc", "微博"),
            ("https://m.weibo.cn/status/123", "微博"),
            ("https://www.ixigua.com/123", "西瓜视频"),
            ("https://x.com/user/status/123", "X"),
            ("https://twitter.com/user/status/123", "X"),
            ("https://www.xinpianchang.com/a123", "新片场"),
            ("https://movie.douban.com/trailer/123", "豆瓣"),
            ("https://channels.weixin.qq.com/finder-preview/pages/sph?id=abc", "微信视频号"),
            ("https://mp.weixin.qq.com/s/article", "微信公众号"),
            ("https://cdn.pixabay.com/download/audio/a.mp3?filename=ambient.mp3", "Pixabay"),
            ("https://s1.aigei.com/src/aud/wav/a.wav?download/test.wav", "爱给"),
            ("https://pixabay.com.evil.example/a.mp3", "直链"),
            ("https://vimeo.com.evil.example/file.mp4", "直链"),
            ("https://www.dropbox.com/file.zip", "Dropbox"),
        ] {
            let mut task: super::Task = serde_json::from_value(serde_json::json!({
                "id": "label-test", "meta": {"req": {"url": url}}
            })).unwrap();
            task.tag_kind();
            assert_eq!(super::download_label(url).as_deref(), Some(label));
            assert_eq!(task.kind.as_deref(), Some(label));
        }
    }

    #[test]
    fn known_sites_override_stale_cache_and_cdn_keeps_recorded_source() {
        let dir = std::env::temp_dir().join(format!("sd-source-label-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sources.json"), r#"[["https://www.youtube.com/watch?v=stale-label","视频网站"],["https://cdn.example.test/remembered-video.mp4","Vimeo"],["https://cdn.example.test/cloud-file.bin","阿里云盘"]]"#.as_bytes()).unwrap();
        crate::sources::init(&dir);
        assert_eq!(super::download_label("https://www.youtube.com/watch?v=stale-label").as_deref(), Some("YouTube"));
        assert_eq!(super::download_label("https://cdn.example.test/remembered-video.mp4").as_deref(), Some("Vimeo"));
        assert_eq!(super::download_label("https://unknown.example/file.mp4").as_deref(), Some("直链"));
        let mut multi: super::Task=serde_json::from_value(serde_json::json!({"id":"saved-source","meta":{"req":{"url":"shandian-multi:folder","extra":{"files":[{"url":"https://cdn.example.test/cloud-file.bin"}]}}}})).unwrap();
        multi.tag_kind();
        assert_eq!(multi.kind.as_deref(),Some("阿里云盘"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn multi_files_share_the_source_classifier() {
        for (url, expected) in [
            ("https://pan.quark.cn/s/example", "夸克网盘"),
            ("https://www.alipan.com/s/example", "阿里云盘"),
            ("https://unknown.example/file.bin", "网盘"),
        ] {
            let mut task: super::Task = serde_json::from_value(serde_json::json!({
                "id":"multi-label", "meta":{"req":{"url":"shandian-multi:folder","extra":{"files":[{"url":url}]}}}
            })).unwrap();
            task.tag_kind();
            assert_eq!(task.kind.as_deref(),Some(expected));
        }
    }

    #[test]
    fn labels_uc_download_cdn() {
        assert_eq!(super::netdisk_label("https://dl-pc-sz-cf.pds.uc.cn/file.mp4"), Some("UC 网盘"));
        assert_eq!(super::netdisk_label("https://drive.uc.cn/"), Some("UC 网盘"));
        assert_eq!(super::netdisk_label("https://pds.uc.cn.example.org/file.mp4"), None);
    }

    #[test]
    fn labels_code_hosts_and_cloud_drives() {
        assert_eq!(super::netdisk_label("https://github.com/putyy/res-downloader/releases/download/4.0.0-beta.6/x.exe"), Some("GitHub"));
        assert_eq!(super::netdisk_label("https://release-assets.githubusercontent.com/x"), Some("GitHub"));
        assert_eq!(super::netdisk_label("https://www.dropbox.com/scl/fi/a/b.zip?dl=1"), Some("Dropbox"));
        assert_eq!(super::netdisk_label("https://drive.usercontent.google.com/download?id=1"), Some("Google Drive"));
        assert_eq!(super::netdisk_label("https://example.com/x.zip"), None);
    }
}
