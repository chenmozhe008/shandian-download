use crate::baidu_cleanup::{self, CheckFailure, TransferLayout};
use crate::{api::Task, AppState};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    fs, io,
    os::windows::{ffi::OsStrExt, fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::State;
use url::Url;

const SETUP: &str = "请重新打开这条百度分享，完成授权后继续下载";
const REENTER: &str = "请填写提取码后继续";
const OWN_SHARE: &str = "这是自己的分享，请打开原分享并选中文件下载";
const REPARSE_POINT: u32 = 0x400;
const INITIAL_FILE_CONCURRENCY: usize = 3;
const SMALL_FILE_CONCURRENCY: usize = 8;
const SMALL_FILE_BYTES: u64 = 1024 * 1024;
const MAX_FILE_CONCURRENCY: usize = 10;
const SPEED_SAMPLE: Duration = Duration::from_secs(6);
const CONCURRENCY_COOLDOWN: Duration = Duration::from_secs(45);
const MAX_FILE_RETRIES: u8 = 3;
const FILE_RESULT_UNKNOWN: &str = "百度网盘未返回成功结果，请打开原分享检查文件是否可下载";
const VERIFY_PENDING: &str = "正在校验本地文件…";
const CLEANUP_UNCONFIRMED: &str = "本地文件已保存，网盘转存未确认清理";
const COLLISION: &str = "保存位置已有同名文件";

fn share_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

pub fn share_url(link: &str) -> Option<Url> {
    let url = Url::parse(link.trim()).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || !matches!(url.host_str(), Some("pan.baidu.com" | "yun.baidu.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let mut clean = Url::parse("https://pan.baidu.com").ok()?;
    let param = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    let path = url.path().trim_end_matches('/');
    clean.set_path(path);
    if let Some(id) = path.strip_prefix("/s/") {
        if !share_component(id) {
            return None;
        }
    } else if path == "/share/init" {
        let id = param("surl").filter(|id| share_component(id))?;
        clean.set_path(&format!("/s/1{id}"));
    } else if path == "/share/link" {
        for key in ["shareid", "uk"] {
            let id = param(key).filter(|id| {
                !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_digit())
            })?;
            clean.query_pairs_mut().append_pair(key, &id);
        }
    } else {
        return None;
    }
    if let Some(code) =
        param("pwd").filter(|pwd| pwd.len() == 4 && pwd.bytes().all(|b| b.is_ascii_alphanumeric()))
    {
        clean.query_pairs_mut().append_pair("pwd", &code);
    }
    Some(clean)
}

pub fn public_url(url: &Url) -> String {
    let mut clean = url.clone();
    clean.set_query(None);
    for (key, value) in url.query_pairs().filter(|(key, _)| key != "pwd") {
        clean.query_pairs_mut().append_pair(&key, &value);
    }
    clean.into()
}

pub fn with_pasted_code(link: &str, tail: &str) -> Option<String> {
    static INLINE: OnceLock<regex::Regex> = OnceLock::new();
    let marker = INLINE
        .get_or_init(|| {
            regex::Regex::new(r"(?:提取码|提取密码|访问码|密码)(?:\s*[:：=]|$)").unwrap()
        })
        .find(link)
        .filter(|m| {
            share_url(link[..m.start()].trim_end_matches(['，', ',', '；', ';', '、'])).is_some()
        });
    let (link, inline) = marker
        .map(|m| {
            (
                link[..m.start()].trim_end_matches(['，', ',', '；', ';', '、']),
                &link[m.start()..],
            )
        })
        .unwrap_or((link, ""));
    let mut url = share_url(link)?;
    static CODE: OnceLock<regex::Regex> = OnceLock::new();
    let code = CODE.get_or_init(|| {
        regex::Regex::new(
            r#"^[\s，,。；;、）)】\]」』]*(?:提取码|提取密码|访问码|密码)\s*[:：=]?\s*[`“‘"']?([a-zA-Z0-9]{4})(?:[^a-zA-Z0-9]|$)"#,
        )
        .expect("valid clipboard pattern")
    });
    if !has_share_code(&url) {
        let pasted = if inline.is_empty() {
            std::borrow::Cow::Borrowed(tail)
        } else {
            std::borrow::Cow::Owned(format!("{inline}{tail}"))
        };
        if let Some(code) = code.captures(&pasted).and_then(|m| m.get(1)) {
            url.query_pairs_mut().append_pair("pwd", code.as_str());
        }
    }
    Some(url.into())
}

pub fn executable(managed: &Path) -> Option<PathBuf> {
    if managed.is_file() {
        return Some(managed.to_path_buf());
    }
    let local_bin = std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("bdpan"));
    let user_bin = std::env::var_os("USERPROFILE").map(|dir| PathBuf::from(dir).join(".local/bin"));
    let path = std::env::var_os("PATH").unwrap_or_default();
    local_bin
        .into_iter()
        .chain(user_bin)
        .chain(std::env::split_paths(&path))
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("bdpan.exe"))
        .find(|file| file.is_file())
}

pub fn cli(bin: &Path) -> Command {
    let mut command = Command::new(bin);
    command
        .creation_flags(0x08000000)
        .arg("--no-check-update")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PendingTransfer {
    task_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharePick {
    pub id: String,
    pub name: String,
}

pub(crate) fn normalize_selection(picks: Option<Vec<SharePick>>) -> Result<Vec<SharePick>, String> {
    let Some(picks) = picks else {
        return Ok(Vec::new());
    };
    if picks.is_empty() {
        return Err("请先勾选要下载的文件".into());
    }
    let mut ids = std::collections::HashSet::new();
    for pick in &picks {
        if pick
            .id
            .parse::<u64>()
            .ok()
            .filter(|id| *id > 0)
            .map(|id| id.to_string())
            .as_deref()
            != Some(&pick.id)
            || pick.name.trim().is_empty()
            || !ids.insert(&pick.id)
        {
            return Err("所选文件信息不完整，请在百度分享页重新选择".into());
        }
    }
    Ok(picks)
}

fn same_selection(left: &[SharePick], right: &[SharePick]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .all(|pick| right.iter().any(|other| other.id == pick.id))
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Job {
    id: String,
    url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    selection: Vec<SharePick>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    selection_roots: Vec<String>,
    #[serde(default)]
    files_done: usize,
    #[serde(default)]
    files_total: usize,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    parallel_files: bool,
    #[serde(default)]
    has_code: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    protected_share_code: Option<Vec<u8>>,
    dir: PathBuf,
    created: u64,
    status: String,
    error: String,
    bytes: u64,
    #[serde(default)]
    total_bytes: u64,
    #[serde(default)]
    speed: u64,
    output: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_root_name: Option<String>,
    #[serde(default)]
    ready_to_move: bool,
    #[serde(default)]
    confirmed_transfer: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_transfer: Option<PendingTransfer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transfer_layout: Option<TransferLayout>,
    #[serde(skip)]
    downloading_from_cloud: bool,
    #[serde(skip)]
    file_wait: Option<FileWait>,
    #[serde(default)]
    allow_duplicate: bool,
    #[serde(default)]
    cleanup_remote: bool,
    #[serde(default)]
    cleanup_note: String,
    #[serde(default)]
    browser_saving: bool,
    #[serde(default)]
    browser_saved: bool,
    #[serde(default)]
    requested_name: Option<String>,
    #[serde(default)]
    suggested_name: Option<String>,
}

impl Job {
    fn pending(url: &Url, dir: PathBuf, cleanup: bool) -> Self {
        Self {
            id: format!("{:016x}", rand::random::<u64>()),
            url: public_url(url),
            selection: Vec::new(),
            selection_roots: Vec::new(),
            files_done: 0,
            files_total: 0,
            parallel_files: false,
            has_code: has_share_code(url),
            protected_share_code: None,
            dir,
            created: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            status: "pause".into(),
            error: String::new(),
            bytes: 0,
            total_bytes: 0,
            speed: 0,
            output: None,
            output_root_name: None,
            ready_to_move: false,
            confirmed_transfer: false,
            pending_transfer: None,
            transfer_layout: None,
            downloading_from_cloud: false,
            file_wait: None,
            allow_duplicate: false,
            cleanup_remote: cleanup,
            cleanup_note: String::new(),
            browser_saving: false,
            browser_saved: false,
            requested_name: None,
            suggested_name: None,
        }
    }

    fn label(&self) -> String {
        if let Some(name) = self
            .output
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|name| name.to_str())
        {
            return name.into();
        }
        if let Some(name) = &self.requested_name {
            return name.clone();
        }
        if let Some(name) = &self.suggested_name {
            return if self.selection.len() > 1 {
                bundle_stem(name)
            } else {
                name.clone()
            };
        }
        "百度网盘分享".into()
    }

    fn staging(&self) -> PathBuf {
        self.dir.join(format!(".shandian-baidu-{}", self.id))
    }

    fn staging_contents(&self, from_cloud: bool) -> PathBuf {
        let staging = self.staging();
        if from_cloud {
            let wrapper = staging.join(&self.id);
            if is_plain_dir(&wrapper)
                && fs::read_dir(&staging).is_ok_and(|entries| entries.count() == 1)
            {
                return wrapper;
            }
        }
        staging
    }

    pub fn path(&self) -> PathBuf {
        self.output.clone().unwrap_or_else(|| self.dir.clone())
    }

    pub fn files(&self) -> Vec<PathBuf> {
        let path = self.output.clone().unwrap_or_else(|| self.staging());
        path.exists().then_some(path).into_iter().collect()
    }

    fn drawer_layout(&self) -> TransferLayout {
        if let Some(layout) = &self.transfer_layout {
            return layout.clone();
        }
        if let Some(output) = &self.output {
            if let Some(layout) = local_layout(output) {
                return layout;
            }
            if let (Some(name), Ok(metadata)) = (output.file_name(), fs::symlink_metadata(output)) {
                if metadata.is_file() && metadata.file_attributes() & REPARSE_POINT == 0 {
                    return TransferLayout::from([(PathBuf::from(name), Some(metadata.len()))]);
                }
            }
        }
        TransferLayout::new()
    }

    fn drawer_disk_path(&self, relative: &Path) -> Option<PathBuf> {
        if let Some(output) = &self.output {
            if fs::symlink_metadata(output).is_ok_and(|m| m.is_file()) {
                return Some(output.clone());
            }
            let child = if let Some(root) = self
                .output_root_name
                .as_ref()
                .filter(|_| self.transfer_layout.is_some())
            {
                relative.strip_prefix(root).ok()?
            } else {
                relative
            };
            return scoped_file(output, child, false).ok();
        }
        let root = self.staging_contents(self.pending_transfer.is_some() || self.browser_saved);
        scoped_file(&root, relative, false).ok()
    }

    pub(crate) fn drawer_files(&self) -> Vec<serde_json::Value> {
        let layout = self.drawer_layout();
        layout.iter().filter_map(|(path, size)| size.map(|size| (path, size)))
            .enumerate().map(|(index, (relative, size))| {
                let local = self.drawer_disk_path(relative);
                let (received, complete) = local.as_ref().map_or((None, false), |file| {
                    file_progress(file, relative, size, &layout, self.output.is_none())
                });
                let missing = self.status == "done" && !complete;
                json!({
                    "index": index,
                    "name": relative.to_string_lossy(),
                    "size": size,
                    "downloaded": received.unwrap_or(0).min(size),
                    "status": if complete { "done" } else if missing { "error" } else if self.status == "running" && received.is_some() { "running" } else { "wait" },
                    "selected": true,
                    "missing": missing,
                })
            }).collect()
    }

    pub(crate) fn drawer_path(&self, index: usize) -> Result<(PathBuf, PathBuf), String> {
        let layout = self.drawer_layout();
        let (relative, size) = layout
            .iter()
            .filter_map(|(path, size)| size.map(|size| (path, size)))
            .nth(index)
            .ok_or("找不到这个文件")?;
        let path = self.drawer_disk_path(relative).ok_or("文件尚未下载完成")?;
        if downloaded_size(&path) != Some(size) {
            return Err("文件尚未下载完成或已移走".into());
        }
        let directory = path.parent().ok_or("文件保存位置不正确")?.to_path_buf();
        Ok((directory, path))
    }

    pub(crate) fn drawer_video_path(&self, requested: Option<&str>) -> Option<PathBuf> {
        if self.status != "done" {
            return None;
        }
        self.drawer_layout()
            .iter()
            .filter_map(|(path, size)| size.map(|size| (path, size)))
            .filter(|(relative, _)| requested.is_none_or(|name| relative.to_string_lossy() == name))
            .find_map(|(relative, size)| {
                if !crate::thumbs::is_video(relative.to_str()?) {
                    return None;
                }
                let path = self.drawer_disk_path(relative)?;
                (downloaded_size(&path) == Some(size)).then_some(path)
            })
    }

    pub fn record(&self, state: &AppState, also_files: bool) {
        let name = self.label();
        crate::trash::record(
            state,
            &name,
            "百度网盘",
            &self.url,
            self.output
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            self.bytes,
            also_files,
            Some(format!("bd:{}", self.id)),
            None,
            None,
        );
    }

    fn as_task(&self) -> Option<Task> {
        let name = self.label();
        let selection = if self.selection.is_empty() {
            String::new()
        } else {
            serde_json::to_string(&self.selection).unwrap_or_default()
        };
        let waiting_for_transfer = self.status == "running"
            && !self.downloading_from_cloud
            && !self.browser_saved
            && !self.ready_to_move
            && (self.pending_transfer.is_some() || self.bytes == 0);
        let mut task: Task = serde_json::from_value(json!({
            "id": format!("bd:{}", self.id),
            "name": name,
            "protocol": "baidu",
            "status": self.status,
            "progress": { "downloaded": self.bytes, "speed": self.speed },
            "meta": { "req": { "url": self.url, "labels": { "baiduCleanup": self.cleanup_remote.to_string(), "baiduHasCode": self.has_code.to_string(), "baiduWaitingTransfer": waiting_for_transfer.to_string(), "baiduSelection": selection, "baiduFilesDone": self.files_done.to_string(), "baiduFilesTotal": self.files_total.to_string() } }, "res": { "name": name, "size": if self.status == "done" { self.bytes } else { self.total_bytes } },
                "opts": { "path": self.dir, "name": name } },
            "collided": self.status == "pause" && self.error == COLLISION,
            "shandianError": if self.error == COLLISION {
                ""
            } else if self.error.contains("自己的分享") {
                OWN_SHARE
            } else if self.bytes == 0 && self.error == "百度网盘未确认下载成功，已保留本机临时文件" {
                "百度网盘未下载到文件，请打开原分享检查"
            } else {
                &self.error
            },
        })).ok()?;
        task.created = Some(self.created);
        task.kind = Some("百度网盘".into());
        task.missing = self.output.as_ref().is_some_and(|path| !path.exists());
        if let Some(wait) = self.file_wait.filter(|_| self.status == "running") {
            task.stage = Some(
                match wait {
                    FileWait::RateLimited => "网盘限流，稍后自动重试…",
                    FileWait::Retry => "正在重试未完成的文件…",
                }
                .into(),
            );
        } else if self.status == "done" && !self.cleanup_note.is_empty() {
            task.stage = Some(self.cleanup_note.clone());
        } else if waiting_for_transfer {
            task.stage = Some("等待转存完成…".into());
        }
        Some(task)
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum AddBaiduShare {
    Started {
        id: String,
    },
    AlreadyDownloaded {
        id: String,
        name: String,
        folder: Option<String>,
    },
    AlreadyRunning {
        id: String,
        name: String,
    },
}

#[derive(Default)]
struct Jobs {
    records: Vec<Job>,
    links: HashMap<String, Url>,
    workers: HashMap<String, Option<Child>>,
    file_workers: HashMap<String, HashMap<PathBuf, Child>>,
    listing_shares: std::collections::HashSet<String>,
    share_lookup: Option<ShareLookup>,
    stopping: bool,
}

pub struct Downloads {
    file: PathBuf,
    tool: PathBuf,
    jobs: Mutex<Jobs>,
}

impl Downloads {
    pub fn new(local: &Path) -> Self {
        let file = crate::migrate::local_path(local, "baidu-downloads.json");
        let mut records: Vec<Job> = fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let destinations: std::collections::HashSet<_> =
            records.iter().map(|job| &job.dir).collect();
        for dir in destinations {
            tidy_staging(dir);
        }
        for job in &mut records {
            if let Some(link) = share_url(&job.url) {
                job.url = public_url(&link);
            }
            if job.browser_saving {
                job.browser_saving = false;
                job.status = "error".into();
                job.error = "网页转存已中断，请回百度分享页继续".into();
            }
            if job.error == "百度网盘仍在转存，请稍后到网盘查看" {
                job.pending_transfer
                    .get_or_insert_with(PendingTransfer::default);
                job.status = "pause".into();
                job.error.clear();
            }
            if job.status == "running" {
                job.status = "pause".into();
            }
            job.speed = 0;
            if job.cleanup_note == "正在清理网盘转存…" {
                job.cleanup_note = CLEANUP_UNCONFIRMED.into();
            }
        }
        Self {
            file,
            tool: local.join("bin/bdpan.exe"),
            jobs: Mutex::new(Jobs {
                records,
                ..Default::default()
            }),
        }
    }

    pub(crate) fn reconcile_cleanups(self: &Arc<Self>) {
        let Some(bin) = executable(&self.tool) else {
            return;
        };
        let ids: Vec<_> = self
            .jobs
            .lock()
            .unwrap()
            .records
            .iter()
            .filter(|job| {
                job.status == "done"
                    && job.cleanup_remote
                    && job.confirmed_transfer
                    && matches!(
                        job.cleanup_note.as_str(),
                        VERIFY_PENDING | CLEANUP_UNCONFIRMED
                    )
            })
            .map(|job| (job.id.clone(), job.cleanup_note == VERIFY_PENDING))
            .collect();
        if ids.is_empty() {
            return;
        }
        let downloads = Arc::clone(self);
        tauri::async_runtime::spawn_blocking(move || {
            for (id, verify_saved) in ids {
                let active = || !downloads.jobs.lock().unwrap().stopping;
                if !active() {
                    break;
                }
                if verify_saved {
                    downloads.finish_cleanup(&bin, &id);
                    continue;
                }
                if baidu_cleanup::transfer_cleared(&bin, &id, &active) {
                    let mut jobs = downloads.jobs.lock().unwrap();
                    if let Some(job) = jobs
                        .records
                        .iter_mut()
                        .find(|job| job.id == id && job.cleanup_note == CLEANUP_UNCONFIRMED)
                    {
                        job.cleanup_note = "网盘转存已清理".into();
                        let _ = downloads.save(&jobs.records);
                    }
                }
            }
        });
    }

    pub fn tool_path(&self) -> &Path {
        &self.tool
    }

    fn save(&self, records: &[Job]) -> Result<(), String> {
        use io::Write;
        let tmp = self
            .file
            .with_extension(format!("{:016x}.tmp", rand::random::<u64>()));
        let result = (|| -> io::Result<()> {
            let data = serde_json::to_vec(records)?;
            let mut f = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&tmp)?;
            f.write_all(&data)?;
            f.sync_all()?;
            drop(f);
            move_file(&tmp, &self.file, 1)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result.map_err(|_| "无法保存百度网盘任务".into())
    }

    pub fn tasks(&self, key: Option<&str>) -> Vec<Task> {
        self.jobs
            .lock()
            .unwrap()
            .records
            .iter()
            .filter(|job| key.map_or(true, |key| crate::links::canonical_url(&job.url) == key))
            .filter_map(Job::as_task)
            .collect()
    }

    pub fn find(&self, id: &str) -> Option<Job> {
        self.jobs
            .lock()
            .unwrap()
            .records
            .iter()
            .find(|job| job.id == id)
            .cloned()
    }

    pub(crate) fn saved_code_for_redownload(&self, task_id: &str, link: &str) -> Option<String> {
        let id = task_id.strip_prefix("bd:")?;
        let key = public_url(&share_url(link)?);
        let protected = {
            let jobs = self.jobs.lock().ok()?;
            let task = jobs
                .records
                .iter()
                .find(|job| job.id == id && job.url == key && job.status == "done")?;
            task.protected_share_code
                .as_ref()
                .or_else(|| {
                    jobs.records
                        .iter()
                        .rev()
                        .filter(|job| job.status == "done" && job.url == key)
                        .find_map(|job| job.protected_share_code.as_ref())
                })?
                .clone()
        };
        let code = String::from_utf8(protect_share_code(protected, false)?).ok()?;
        (code.len() == 4 && code.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(code)
    }

    fn add(
        self: &Arc<Self>,
        url: Url,
        dir: PathBuf,
        force: bool,
        allow_duplicate: bool,
        selection: Vec<SharePick>,
        cleanup_remote: Option<bool>,
        requested_name: Option<String>,
        suggested_name: Option<String>,
    ) -> Result<AddBaiduShare, String> {
        let public = public_url(&url);
        let id = {
            let mut jobs = self.jobs.lock().unwrap();
            if jobs.stopping {
                return Err("闪电正在退出".into());
            }
            if let Some(job) = jobs
                .records
                .iter()
                .filter(|job| {
                    (!force || job.status != "done")
                        && job.url == public
                        && same_selection(&job.selection, &selection)
                        && job.dir == dir
                        && (job.status != "done"
                            || job.output.as_ref().is_some_and(|path| path.exists()))
                })
                .min_by_key(|job| match job.status.as_str() {
                    "done" => 0,
                    "running" => 1,
                    _ => 2,
                })
            {
                let id = job.id.clone();
                if job.status == "done" {
                    let folder = job
                        .output
                        .as_ref()
                        .filter(|path| path.is_dir())
                        .map(|path| path.to_string_lossy().into_owned());
                    return Ok(AddBaiduShare::AlreadyDownloaded {
                        id: format!("bd:{id}"),
                        name: job.label(),
                        folder,
                    });
                }
                if job.status == "running" {
                    return Ok(AddBaiduShare::AlreadyRunning {
                        id: format!("bd:{id}"),
                        name: job.label(),
                    });
                }
                if cleanup_remote.is_some_and(|choice| choice != job.cleanup_remote)
                    || (allow_duplicate && !job.allow_duplicate)
                    || requested_name.is_some()
                    || (job.suggested_name.is_none() && suggested_name.is_some())
                {
                    let mut records = jobs.records.clone();
                    let record = records.iter_mut().find(|record| record.id == id).unwrap();
                    if let Some(cleanup) = cleanup_remote {
                        record.cleanup_remote = cleanup;
                    }
                    record.allow_duplicate |= allow_duplicate;
                    if requested_name.is_some() {
                        record.requested_name = requested_name;
                    }
                    if record.suggested_name.is_none() {
                        record.suggested_name = suggested_name;
                    }
                    self.save(&records)?;
                    jobs.records = records;
                }
                if has_share_code(&url) || !jobs.links.contains_key(&id) {
                    jobs.links.insert(id.clone(), url);
                }
                id
            } else {
                let mut job = Job::pending(&url, dir, cleanup_remote.unwrap_or(true));
                job.selection = selection;
                job.allow_duplicate = allow_duplicate;
                job.requested_name = requested_name;
                job.suggested_name = suggested_name;
                let id = job.id.clone();
                let mut records = jobs.records.clone();
                records.push(job);
                self.save(&records)?;
                jobs.records = records;
                jobs.links.insert(id.clone(), url);
                id
            }
        };
        self.resume(&id)?;
        Ok(AddBaiduShare::Started {
            id: format!("bd:{id}"),
        })
    }

    pub fn resume(self: &Arc<Self>, id: &str) -> Result<(), String> {
        self.resume_with_share(id, None)
    }

    fn approve_duplicate(self: &Arc<Self>, id: &str) -> Result<(), String> {
        {
            let mut jobs = self.jobs.lock().unwrap();
            let job = jobs
                .records
                .iter_mut()
                .find(|job| {
                    job.id == id
                        && job.status == "pause"
                        && job.error == COLLISION
                        && job.ready_to_move
                })
                .ok_or("这条任务已经处理")?;
            job.allow_duplicate = true;
        }
        self.resume(id)
    }

    fn resume_with_share(self: &Arc<Self>, id: &str, alternate: Option<Url>) -> Result<(), String> {
        let bin = executable(&self.tool).ok_or(SETUP)?;
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.stopping {
            return Err("闪电正在退出".into());
        }
        let index = jobs
            .records
            .iter()
            .position(|job| job.id == id)
            .ok_or("找不到下载任务")?;
        if matches!(jobs.records[index].status.as_str(), "running" | "done") {
            return Ok(());
        }
        if jobs.workers.contains_key(id) {
            return Err("正在暂停，请稍后再继续".into());
        }
        let mut job = jobs.records[index].clone();
        if job.status == "error"
            && job.bytes == 0
            && jobs.records.iter().any(|other| {
                other.status == "done"
                    && other.url == job.url
                    && same_selection(&other.selection, &job.selection)
                    && other.dir == job.dir
                    && other.requested_name == job.requested_name
                    && other
                        .output
                        .as_ref()
                        .is_some_and(|path| path.is_file() || path.is_dir())
            })
        {
            return Err("这份分享已下载，可在「已完成」查看".into());
        }
        if alternate
            .as_ref()
            .is_some_and(|url| public_url(url) != job.url)
        {
            return Err("提取码对应的分享与原任务不一致".into());
        }
        if job.confirmed_transfer
            && job.bytes > 0
            && job.error.contains("本地文件与百度网盘校验结果不一致")
        {
            job.ready_to_move = true;
        }
        if job.error == COLLISION && !job.allow_duplicate {
            return Err(COLLISION.into());
        }
        let link = alternate
            .filter(has_share_code)
            .or_else(|| jobs.links.get(id).cloned())
            .or_else(|| {
                let bytes = job.protected_share_code.clone()?;
                let code = String::from_utf8(protect_share_code(bytes, false)?).ok()?;
                share_url(&format!(
                    "{}{}pwd={code}",
                    job.url,
                    if job.url.contains('?') { "&" } else { "?" }
                ))
            })
            .or_else(|| {
                (!job.has_code || (job.pending_transfer.is_some() && job.transfer_layout.is_some()))
                    .then(|| share_url(&job.url))
                    .flatten()
            })
            .or_else(|| job.ready_to_move.then(|| share_url(&job.url)).flatten())
            .ok_or(REENTER)?;
        if !job.ready_to_move && job.pending_transfer.is_none() {
            job.has_code = has_share_code(&link);
        }
        job.status = "running".into();
        job.error.clear();
        job.speed = 0;
        job.file_wait = None;
        let mut records = jobs.records.clone();
        records[index] = job.clone();
        self.save(&records)?;
        jobs.records = records;
        jobs.links.insert(id.into(), link.clone());
        jobs.workers.insert(id.into(), None);
        drop(jobs);
        let downloads = Arc::clone(self);
        if job.selection.is_empty()
            && job.pending_transfer.is_some()
            && !job.ready_to_move
            && (job.transfer_layout.is_none()
                || job.total_bytes == 0
                || (job.requested_name.is_none() && job.suggested_name.is_none()))
        {
            let lookup = Arc::clone(self);
            let probe_job = job.clone();
            let probe_bin = bin.clone();
            let probe_link = link.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let _ = lookup.probe_share(&probe_job, &probe_bin, &probe_link);
            });
        }
        tauri::async_runtime::spawn_blocking(move || downloads.run(job, bin, link));
        Ok(())
    }

    fn probe_share(&self, job: &Job, bin: &Path, link: &Url) -> Result<(), String> {
        if !self
            .jobs
            .lock()
            .unwrap()
            .listing_shares
            .insert(job.id.clone())
        {
            return Ok(());
        }
        let result = self.read_share_metadata(job, bin, link);
        self.jobs.lock().unwrap().listing_shares.remove(&job.id);
        result
    }

    fn read_share_metadata(&self, job: &Job, bin: &Path, link: &Url) -> Result<(), String> {
        let active = || {
            let jobs = self.jobs.lock().unwrap();
            !jobs.stopping
                && jobs
                    .records
                    .iter()
                    .any(|record| record.id == job.id && record.status == "running")
        };
        let set_metadata = |name: Option<String>, manifest: Option<ShareManifest>| {
            let mut jobs = self.jobs.lock().unwrap();
            if let Some(record) = jobs
                .records
                .iter_mut()
                .find(|record| record.id == job.id && record.status == "running")
            {
                if record.suggested_name.is_none() && name.is_some() {
                    record.suggested_name = name;
                }
                if let Some(manifest) = manifest {
                    record.total_bytes = layout_size(&manifest.layout).unwrap_or(0);
                    record.files_total = manifest.layout.values().flatten().count();
                    record.transfer_layout = Some(manifest.layout);
                    record.selection_roots = manifest.roots;
                    if record.pending_transfer.is_some() {
                        let _ = self.save(&jobs.records);
                    }
                }
            }
        };
        if job.browser_saved {
            let directory = baidu_cleanup::task_directory(&job.id);
            let value = baidu_cleanup::list_directory(bin, &directory, &active);
            set_metadata(value.as_ref().and_then(name_from_listing), None);
            return Ok(());
        }
        if let Some(metadata) = self.share_metadata(bin, link, &job.selection, &active)? {
            let need_manifest = job.transfer_layout.is_none()
                || (!job.selection.is_empty() && job.selection_roots.is_empty());
            set_metadata(
                metadata.name,
                if need_manifest {
                    metadata.manifest
                } else {
                    None
                },
            );
        }
        Ok(())
    }

    fn share_metadata(
        &self,
        bin: &Path,
        link: &Url,
        selection: &[SharePick],
        active: &impl Fn() -> bool,
    ) -> Result<Option<ShareMetadata>, String> {
        use sha2::{Digest, Sha256};
        let key: [u8; 32] = Sha256::digest(link.as_str()).into();
        let result = {
            let mut jobs = self.jobs.lock().unwrap();
            if jobs
                .share_lookup
                .as_ref()
                .is_none_or(|lookup| lookup.key != key)
            {
                jobs.share_lookup = Some(ShareLookup {
                    key,
                    result: Arc::new(Mutex::new(None)),
                });
            }
            Arc::clone(&jobs.share_lookup.as_ref().unwrap().result)
        };
        // A preview and a download of the same selection share the directory lookup.
        let mut saved = result.lock().unwrap();
        if !active() {
            return Ok(None);
        }
        if let Some((_, previous_selection, value)) = saved
            .as_ref()
            .filter(|(at, _, _)| at.elapsed() < Duration::from_secs(45))
        {
            if previous_selection == selection {
                return Ok(Some(value.clone()));
            }
            if let Some(manifest) = value
                .manifest
                .as_ref()
                .and_then(|manifest| manifest.selected(selection))
            {
                return Ok(Some(ShareMetadata {
                    name: None,
                    manifest: Some(manifest),
                }));
            }
        }
        let list = |dir: &str, page: usize| {
            baidu_cleanup::command_json(
                bin,
                &[
                    "transfer",
                    "list",
                    link.as_str(),
                    "--source-dir",
                    dir,
                    "--page",
                    &page.to_string(),
                    "--page-size",
                    "100",
                ],
                active,
            )
        };
        let Some(first) = list("", 1) else {
            return Ok(None);
        };
        if !selection.is_empty() && first["code"].as_i64().is_some_and(|code| code != 0) {
            download_result(true, &first.to_string(), "")?;
        }
        let value = ShareMetadata {
            name: selection
                .is_empty()
                .then(|| name_from_listing(&first))
                .flatten(),
            manifest: share_manifest(first, &list, selection),
        };
        if value.manifest.is_some() {
            *saved = Some((Instant::now(), selection.to_vec(), value.clone()));
        }
        Ok(Some(value))
    }

    pub fn pause(&self, id: &str) -> Result<(), String> {
        let mut jobs = self.jobs.lock().unwrap();
        let mut records = jobs.records.clone();
        let job = records
            .iter_mut()
            .find(|job| job.id == id)
            .ok_or("找不到下载任务")?;
        if job.status != "running" {
            return Ok(());
        }
        if let Some(Some(child)) = jobs.workers.get_mut(id) {
            if child
                .try_wait()
                .map_err(|_| "无法检查百度下载状态")?
                .is_none()
            {
                child.kill().map_err(|_| "无法暂停百度下载")?;
            }
        }
        stop_file_workers(&mut jobs, id)?;
        job.status = "pause".into();
        job.speed = 0;
        let result = self.save(&records);
        jobs.records = records;
        result
    }

    pub fn remove(&self, id: &str) -> Result<Option<Job>, String> {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(Some(child)) = jobs.workers.get_mut(id) {
            if child
                .try_wait()
                .map_err(|_| "无法检查百度下载状态")?
                .is_none()
            {
                child.kill().map_err(|_| "无法停止百度下载")?;
            }
            child.wait().map_err(|_| "无法停止百度下载")?;
        }
        stop_file_workers(&mut jobs, id)?;
        let mut records = jobs.records.clone();
        let Some(index) = records.iter().position(|job| job.id == id) else {
            return Ok(None);
        };
        let job = records.remove(index);
        self.save(&records)?;
        jobs.records = records;
        jobs.workers.remove(id);
        jobs.links.remove(id);
        Ok(Some(job))
    }

    pub fn stop(&self) {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.stopping = true;
        for child in jobs.workers.values_mut().flatten() {
            let _ = child.kill();
        }
        for child in jobs
            .file_workers
            .values_mut()
            .flat_map(|children| children.values_mut())
        {
            let _ = child.kill();
        }
    }

    fn run(&self, job: Job, bin: PathBuf, link: Url) {
        let receipt = if job.ready_to_move {
            Ok(job.confirmed_transfer)
        } else {
            self.download_with_transfer(&job, &bin, &link)
        };
        let from_cloud = job.browser_saved
            || self
                .jobs
                .lock()
                .unwrap()
                .records
                .iter()
                .any(|record| record.id == job.id && record.pending_transfer.is_some());
        let contents = job.staging_contents(from_cloud);
        // A failed or interrupted batch may still have files running.
        let _ = stop_file_workers(&mut self.jobs.lock().unwrap(), &job.id);
        let receipt = receipt.and_then(|confirmed| {
            let actual = local_layout(&contents)
                .filter(|files| files.values().any(Option::is_some))
                .ok_or("百度网盘没有返回可用文件，已保留临时文件")?;
            let expected = self.find(&job.id).and_then(|record| record.transfer_layout);
            if expected
                .as_ref()
                .map_or(!job.selection.is_empty(), |files| *files != actual)
            {
                return Err("本地文件与下载清单不一致，已保留临时文件".into());
            }
            Ok((confirmed, actual))
        });
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(Some(mut child)) = jobs.workers.remove(&job.id) {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        let Some(index) = jobs.records.iter().position(|j| j.id == job.id) else {
            return;
        };
        if jobs.records[index].status != "running" || jobs.stopping {
            return;
        }
        let current = &mut jobs.records[index];
        current.ready_to_move = receipt.is_ok();
        let result = receipt.and_then(|(confirmed, layout)| {
            current.confirmed_transfer = confirmed;
            commit(current, &contents)
                .map(|(path, bytes, source_name)| (path, bytes, source_name, layout))
        });
        match result {
            Ok((path, bytes, source_name, layout)) => {
                if contents != job.staging() {
                    let _ = fs::remove_dir(job.staging());
                }
                current.status = "done".into();
                current.output = Some(path);
                current.output_root_name =
                    source_name.map(|root| root.to_string_lossy().into_owned());
                current.pending_transfer = None;
                current.bytes = bytes;
                current.files_done = layout.values().flatten().count();
                current.files_total = current.files_done;
                current.transfer_layout = Some(layout);
                if let Some((_, code)) = link.query_pairs().find(|(name, _)| name == "pwd") {
                    if let Some(protected) =
                        protect_share_code(code.into_owned().into_bytes(), true)
                    {
                        current.protected_share_code = Some(protected);
                    }
                }
                if current.cleanup_remote {
                    current.cleanup_note = if current.confirmed_transfer {
                        VERIFY_PENDING
                    } else {
                        "无法确认本次网盘转存，已保留副本"
                    }
                    .into();
                }
            }
            Err(error) => {
                current.status = if error == COLLISION { "pause" } else { "error" }.into();
                let staging = job.staging();
                current.bytes = measure(&staging);
                let _ = fs::remove_dir(staging);
                current.error =
                    if error != COLLISION && current.bytes > 0 && !error.contains("临时文件") {
                        format!("{error}。已保留本机临时文件")
                    } else {
                        error
                    };
            }
        }
        current.speed = 0;
        let completed = current.status == "done";
        let clean = completed && current.cleanup_note == VERIFY_PENDING;
        if completed {
            jobs.links.remove(&job.id);
        }
        let saved = self.save(&jobs.records).is_ok();
        if clean && !saved {
            jobs.records[index].cleanup_note = CLEANUP_UNCONFIRMED.into();
        }
        drop(jobs);
        if clean && saved {
            self.finish_cleanup(&bin, &job.id);
        }
    }

    fn finish_cleanup(&self, bin: &Path, id: &str) {
        let Some(job) = self.find(id).filter(|job| {
            job.status == "done"
                && job.cleanup_remote
                && job.confirmed_transfer
                && job.cleanup_note == VERIFY_PENDING
        }) else {
            return;
        };
        let active = || {
            let jobs = self.jobs.lock().unwrap();
            !jobs.stopping
                && jobs.records.iter().any(|current| {
                    current.id == id
                        && current.status == "done"
                        && current.output == job.output
                        && current.cleanup_note == VERIFY_PENDING
                })
        };
        let note = match (job.output.as_deref(), job.transfer_layout.as_ref()) {
            (Some(path), Some(layout)) => {
                let source_name = job.output_root_name.as_deref().map(std::ffi::OsStr::new);
                match baidu_cleanup::verify(bin, id, path, source_name, layout, &active) {
                    Ok(verified) => {
                        if verified.cleanup(bin, path, source_name, &active) {
                            "网盘转存已清理"
                        } else {
                            CLEANUP_UNCONFIRMED
                        }
                    }
                    Err(CheckFailure::Different) => "校验未通过，已保留网盘转存",
                    Err(CheckFailure::Unconfirmed) => {
                        if baidu_cleanup::transfer_cleared(bin, id, &active) {
                            "网盘转存已清理"
                        } else {
                            CLEANUP_UNCONFIRMED
                        }
                    }
                }
            }
            _ => CLEANUP_UNCONFIRMED,
        };
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.stopping {
            return;
        }
        if let Some(current) = jobs.records.iter_mut().find(|current| {
            current.id == id
                && current.output == job.output
                && current.cleanup_note == VERIFY_PENDING
        }) {
            current.cleanup_note = note.into();
            let _ = self.save(&jobs.records);
        }
    }

    fn download_with_transfer(&self, job: &Job, bin: &Path, link: &Url) -> Result<bool, String> {
        let active = || {
            let jobs = self.jobs.lock().unwrap();
            !jobs.stopping
                && jobs
                    .records
                    .iter()
                    .any(|record| record.id == job.id && record.status == "running")
        };
        let mut waiting = job.pending_transfer.is_some();
        let split_share = if !waiting && job.selection.is_empty() {
            let _ = self.probe_share(job, bin, link);
            !job.browser_saved
                && measure(&job.staging()) == 0
                && self.find(&job.id).is_some_and(|current| {
                    current.files_total > 1 && current.transfer_layout.is_some()
                })
        } else {
            false
        };
        while !waiting {
            let reply = if split_share {
                self.run_bdpan(job, transfer_command(bin, link, &job.id, &[]), true, false)?
            } else if job.selection.is_empty() {
                self.download(job, bin, link, job.browser_saved)?
            } else {
                self.probe_share(job, bin, link)?;
                let ids = self
                    .jobs
                    .lock()
                    .unwrap()
                    .records
                    .iter()
                    .find(|record| record.id == job.id)
                    .filter(|record| record.transfer_layout.is_some())
                    .map(|record| record.selection_roots.clone())
                    .filter(|ids| !ids.is_empty())
                    .ok_or("无法核对所选文件，请重新打开分享页选择")?;
                self.run_bdpan(job, transfer_command(bin, link, &job.id, &ids), true, false)?
            };
            match reply {
                DownloadReply::Completed(value) => {
                    return Ok(job.browser_saved
                        || value["data"]["saved_path"]
                            .as_str()
                            .is_some_and(|path| baidu_cleanup::matches_receipt(&job.id, path)))
                }
                DownloadReply::Submitted(pending) => {
                    let mut jobs = self.jobs.lock().unwrap();
                    let record = jobs
                        .records
                        .iter_mut()
                        .find(|record| record.id == job.id)
                        .ok_or("下载已移除")?;
                    record.pending_transfer = Some(pending);
                    record.speed = 0;
                    if let Some((_, code)) = link.query_pairs().find(|(name, _)| name == "pwd") {
                        record.protected_share_code =
                            protect_share_code(code.into_owned().into_bytes(), true);
                    }
                    self.save(&jobs.records)?;
                    waiting = true;
                }
                DownloadReply::Busy => {
                    if !wait_while_active(Duration::from_secs(300), &active) {
                        return Err("下载已停止".into());
                    }
                }
            }
        }

        let mut delay = 2;
        while active() {
            let expected = self
                .jobs
                .lock()
                .unwrap()
                .records
                .iter()
                .find(|record| record.id == job.id)
                .and_then(|record| record.transfer_layout.clone());
            if expected.is_none() {
                let _ = self.probe_share(job, bin, link);
            }
            if let Some(layout) = expected.filter(|expected| {
                baidu_cleanup::transfer_layout(bin, &job.id, &active).as_ref() == Some(expected)
            }) {
                let mut jobs = self.jobs.lock().unwrap();
                if let Some(record) = jobs.records.iter_mut().find(|record| record.id == job.id) {
                    record.downloading_from_cloud = true;
                }
                let parallel =
                    layout.values().flatten().count() > 1 && !has_partial_path_collision(&layout);
                if parallel && !job.parallel_files {
                    if let Some(record) = jobs.records.iter_mut().find(|record| record.id == job.id)
                    {
                        record.parallel_files = true;
                    }
                    self.save(&jobs.records)?;
                }
                drop(jobs);
                if parallel {
                    self.download_parallel(job, bin, &layout)?;
                    return Ok(true);
                }
                match self.download(job, bin, link, true)? {
                    DownloadReply::Completed(_) => return Ok(true),
                    _ => return Err("百度尚未提供已转存文件的下载结果，可以继续重试".into()),
                }
            }
            if !wait_while_active(Duration::from_secs(delay), &active) {
                break;
            }
            delay = (delay * 2).min(20);
        }
        Err("下载已停止".into())
    }

    fn download_parallel(
        &self,
        job: &Job,
        bin: &Path,
        layout: &TransferLayout,
    ) -> Result<(), String> {
        use std::collections::VecDeque;
        let mut pending = VecDeque::new();
        let staging = job.staging();
        fs::create_dir_all(&staging).map_err(|e| format!("无法创建下载目录：{e}"))?;
        if !is_plain_dir(&staging) {
            return Err("下载临时目录不可用".into());
        }
        let _ = hide_staging(&staging, true);
        let root = job.staging_contents(true);
        let ready_at = Instant::now();

        for (relative, size) in layout {
            let destination = scoped_file(&root, relative, true).map_err(|e| e.to_string())?;
            if let Some(size) = size {
                match fs::symlink_metadata(&destination) {
                    Ok(meta) if meta.is_file() && meta.file_attributes() & REPARSE_POINT == 0 => {
                        if meta.len() == *size {
                            continue;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    _ => return Err("下载目录内有无法确认的同名文件".into()),
                }
                match fs::symlink_metadata(partial_path(&destination)) {
                    Ok(meta) if meta.is_file() && meta.file_attributes() & REPARSE_POINT == 0 => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    _ => return Err("无法确认已有的临时文件".into()),
                }
                pending.push_back(FileTarget {
                    relative: relative.clone(),
                    local: destination,
                    size: *size,
                    retries: 0,
                    ready_at,
                });
            } else {
                match fs::create_dir(&destination) {
                    Ok(()) => {}
                    Err(e)
                        if e.kind() == io::ErrorKind::AlreadyExists
                            && is_plain_dir(&destination) => {}
                    Err(e) => return Err(format!("无法创建下载目录：{e}")),
                }
            }
        }

        let mut running: Vec<FileDownload> = Vec::new();
        let mut failure = None;
        let mut previous = (Instant::now(), transfer_progress(&root, layout).0);
        let small_batch =
            pending.len() > 1 && pending.iter().all(|file| file.size <= SMALL_FILE_BYTES);
        let mut concurrency = FileConcurrency::new(previous.0, small_batch);
        while !pending.is_empty() || !running.is_empty() {
            let now = Instant::now();
            {
                let mut jobs = self.jobs.lock().unwrap();
                if jobs.stopping {
                    return Err("下载已停止".into());
                }
                let index = jobs
                    .records
                    .iter()
                    .position(|record| record.id == job.id && record.status == "running")
                    .ok_or("下载已停止")?;
                while now >= concurrency.retry_at && running.len() < concurrency.limit {
                    let Some(ready) = pending.iter().position(|target| target.ready_at <= now)
                    else {
                        break;
                    };
                    let target = pending.remove(ready).unwrap();
                    if target.retries > 0 && downloaded_size(&target.local) == Some(target.size) {
                        continue;
                    }
                    let mut child =
                        download_file_command(bin, &job.id, &target.relative, &target.local)
                            .spawn()
                            .map_err(|e| format!("启动文件下载失败：{e}"))?;
                    let stdout = child.stdout.take().expect("piped stdout");
                    let stderr = child.stderr.take().expect("piped stderr");
                    jobs.file_workers
                        .entry(job.id.clone())
                        .or_default()
                        .insert(target.relative.clone(), child);
                    running.push(FileDownload {
                        target,
                        stdout: std::thread::spawn(move || read_response(stdout)),
                        stderr: std::thread::spawn(move || read_response(stderr)),
                    });
                }
                jobs.records[index].file_wait = if running.is_empty() && !pending.is_empty() {
                    Some(if now < concurrency.retry_at {
                        FileWait::RateLimited
                    } else {
                        FileWait::Retry
                    })
                } else {
                    None
                };
            }

            let saturated = running.len() == concurrency.limit
                && (pending.iter().any(|target| target.ready_at <= now)
                    || concurrency.baseline.is_some());
            let mut i = 0;
            while i < running.len() {
                let status = {
                    let mut jobs = self.jobs.lock().unwrap();
                    let children = jobs.file_workers.get_mut(&job.id).ok_or("下载已停止")?;
                    let key = &running[i].target.relative;
                    let child = children.get_mut(key).ok_or("下载已停止")?;
                    let status = child
                        .try_wait()
                        .map_err(|e| format!("无法检查文件下载状态：{e}"))?;
                    if status.is_some() {
                        children.remove(key);
                    }
                    status
                };
                if let Some(status) = status {
                    let file = running.swap_remove(i);
                    let stdout = file.stdout.join().unwrap_or_default();
                    let stderr = file.stderr.join().unwrap_or_default();
                    let stdout = String::from_utf8_lossy(&stdout);
                    let stderr = String::from_utf8_lossy(&stderr);
                    let mut target = file.target;
                    let result = download_result(status.success(), &stdout, &stderr);
                    if matches!(result, Ok(DownloadReply::Completed(_)))
                        && downloaded_size(&target.local) == Some(target.size)
                    {
                        continue;
                    }
                    let http_status = file_http_status(&stdout, &stderr);
                    let retry = file_retry_kind(&result, http_status, &stdout, &stderr);
                    let now = Instant::now();
                    if retry == Some(FileWait::RateLimited) {
                        concurrency.rate_limited(now, target.retries);
                    }
                    if let Some(wait) = retry.filter(|_| target.retries < MAX_FILE_RETRIES) {
                        target.ready_at = match wait {
                            FileWait::RateLimited => concurrency.retry_at,
                            FileWait::Retry => now + Duration::from_secs(2 << target.retries),
                        };
                        target.retries += 1;
                        pending.push_back(target);
                        continue;
                    }
                    let reason = match (retry, http_status, result) {
                        (Some(FileWait::RateLimited), _, _) => {
                            "百度网盘暂时限流，请稍后继续".into()
                        }
                        (_, Some(code), _) if code >= 400 => {
                            format!("百度网盘下载失败（HTTP {code}）")
                        }
                        (_, _, Err(error)) => error,
                        _ => "百度尚未确认文件下载完整".into(),
                    };
                    let name = target
                        .relative
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    let attempts = if target.retries > 0 {
                        format!("已自动重试 {} 次。", target.retries)
                    } else {
                        String::new()
                    };
                    failure.get_or_insert_with(|| format!("「{name}」{reason}。{attempts}"));
                } else {
                    i += 1;
                }
            }

            if previous.0.elapsed() >= Duration::from_secs(1) {
                let (bytes, done) = transfer_progress(&root, layout);
                let speed = (bytes.saturating_sub(previous.1) as f64
                    / previous.0.elapsed().as_secs_f64()) as u64;
                previous = (Instant::now(), bytes);
                let progress = if small_batch {
                    done as u64 * 1000
                } else {
                    bytes
                };
                concurrency.observe(previous.0, progress, saturated);
                if let Some(record) = self
                    .jobs
                    .lock()
                    .unwrap()
                    .records
                    .iter_mut()
                    .find(|record| record.id == job.id && record.status == "running")
                {
                    record.bytes = bytes;
                    record.speed = speed;
                    record.files_done = done;
                }
            }
            let now = Instant::now();
            if !running.is_empty()
                || (!pending.is_empty()
                    && (now < concurrency.retry_at
                        || pending.iter().all(|target| target.ready_at > now)))
            {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        if let Some(error) = failure {
            let (_, files_done) = transfer_progress(&root, layout);
            if let Some(record) = self
                .jobs
                .lock()
                .unwrap()
                .records
                .iter_mut()
                .find(|record| record.id == job.id && record.status == "running")
            {
                record.files_done = files_done;
            }
            return Err(error);
        }
        // bdpan's old directory downloads can leave a redundant partial beside a complete file.
        // Discard only our temporary sidecars once the corresponding final files are complete.
        for (relative, size) in layout {
            let Some(size) = size else { continue };
            let path = scoped_file(&root, relative, false).map_err(|error| error.to_string())?;
            if downloaded_size(&path) == Some(*size) {
                let partial = partial_path(&path);
                if downloaded_size(&partial).is_some() {
                    fs::remove_file(partial)
                        .map_err(|e| format!("无法清理已完成文件的临时数据：{e}"))?;
                }
            }
        }
        Ok(())
    }

    fn download(
        &self,
        job: &Job,
        bin: &Path,
        link: &Url,
        from_cloud: bool,
    ) -> Result<DownloadReply, String> {
        let cmd = download_command(bin, link, &job.staging(), &job.id, from_cloud);
        self.run_bdpan(job, cmd, false, from_cloud)
    }

    fn run_bdpan(
        &self,
        job: &Job,
        mut cmd: Command,
        transfer_only: bool,
        from_cloud: bool,
    ) -> Result<DownloadReply, String> {
        let staging = job.staging();
        let (stdout, stderr) = {
            let mut jobs = self.jobs.lock().unwrap();
            if jobs.stopping
                || !jobs
                    .records
                    .iter()
                    .any(|j| j.id == job.id && j.status == "running")
            {
                return Err("下载已停止".into());
            }
            fs::create_dir_all(&staging).map_err(|_| "无法创建下载目录")?;
            if !is_plain_dir(&staging) {
                return Err("下载临时目录不可用".into());
            }
            let _ = hide_staging(&staging, true);
            let mut child = cmd
                .spawn()
                .map_err(|_| "无法启动百度网盘工具，请检查安装")?;
            let stdout = child.stdout.take().expect("piped stdout");
            let stderr = child.stderr.take().expect("piped stderr");
            jobs.workers.insert(job.id.clone(), Some(child));
            (stdout, stderr)
        };
        let reader = std::thread::spawn(move || read_response(stdout));
        let errors = std::thread::spawn(move || read_response(stderr));
        let mut last = (Instant::now(), measure(&staging));
        let status = loop {
            let exit = {
                let mut jobs = self.jobs.lock().unwrap();
                let Some(Some(child)) = jobs.workers.get_mut(&job.id) else {
                    return Err("下载已移除".into());
                };
                child.try_wait().map_err(|_| "无法获取百度下载结果")?
            };
            if let Some(exit) = exit {
                break exit;
            }
            std::thread::sleep(Duration::from_secs(1));
            let layout = self
                .jobs
                .lock()
                .unwrap()
                .records
                .iter()
                .find(|record| record.id == job.id)
                .and_then(|record| record.transfer_layout.clone());
            let (bytes, files_done) = if let Some(layout) = &layout {
                let (bytes, done) = transfer_progress(&job.staging_contents(from_cloud), layout);
                (bytes, Some(done))
            } else {
                (measure(&staging), None)
            };
            let speed =
                (bytes.saturating_sub(last.1) as f64 / last.0.elapsed().as_secs_f64()) as u64;
            last = (Instant::now(), bytes);
            if let Some(current) = self
                .jobs
                .lock()
                .unwrap()
                .records
                .iter_mut()
                .find(|record| record.id == job.id && record.status == "running")
            {
                current.bytes = bytes;
                current.speed = speed;
                if let Some(count) = files_done {
                    current.files_done = count;
                }
            }
        };
        let data = reader.join().unwrap_or_default();
        let stderr = errors.join().unwrap_or_default();
        let stdout = String::from_utf8_lossy(&data);
        let response = response_json(&stdout);
        let saved_path = response.as_ref().and_then(|value| {
            value["data"]["saved_path"]
                .as_str()
                .or_else(|| value["saved_path"].as_str())
        });
        let legacy_transfer_success = transfer_only
            && status.success()
            && response.as_ref().is_some_and(|value| {
                value.get("code").is_none()
                    && (value["files"].is_array() || value["status"].as_str() == Some("submitted"))
            });
        let mut reply = if legacy_transfer_success {
            pending_receipt(response.as_ref())
        } else {
            download_result(status.success(), &stdout, &String::from_utf8_lossy(&stderr))?
        };
        if transfer_only && matches!(reply, DownloadReply::Completed(_)) {
            reply = pending_receipt(response.as_ref());
        }
        if matches!(reply, DownloadReply::Submitted(_))
            && saved_path.is_some_and(|path| !baidu_cleanup::matches_receipt(&job.id, path))
        {
            return Err("百度返回的转存位置不属于本次任务".into());
        }
        Ok(reply)
    }
}

fn wait_while_active(duration: Duration, active: &impl Fn() -> bool) -> bool {
    let until = Instant::now() + duration;
    while active() {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(left.min(Duration::from_millis(250)));
    }
    false
}

struct FileTarget {
    relative: PathBuf,
    local: PathBuf,
    size: u64,
    retries: u8,
    ready_at: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FileWait {
    RateLimited,
    Retry,
}

struct FileDownload {
    target: FileTarget,
    stdout: std::thread::JoinHandle<Vec<u8>>,
    stderr: std::thread::JoinHandle<Vec<u8>>,
}

struct FileConcurrency {
    limit: usize,
    baseline: Option<u64>,
    sample: Option<(Instant, u64)>,
    measure_after: Instant,
    retry_at: Instant,
    sample_window: Duration,
}

impl FileConcurrency {
    fn new(now: Instant, small_batch: bool) -> Self {
        Self {
            limit: if small_batch {
                SMALL_FILE_CONCURRENCY
            } else {
                INITIAL_FILE_CONCURRENCY
            },
            baseline: None,
            sample: None,
            measure_after: now + Duration::from_secs(if small_batch { 1 } else { 2 }),
            retry_at: now,
            sample_window: if small_batch {
                Duration::from_secs(3)
            } else {
                SPEED_SAMPLE
            },
        }
    }

    fn observe(&mut self, now: Instant, progress: u64, saturated: bool) {
        if now < self.measure_after || !saturated {
            self.sample = None;
            return;
        }
        if self.limit == MAX_FILE_CONCURRENCY && self.baseline.is_none() {
            return;
        }
        let (start, starting_progress) = *self.sample.get_or_insert((now, progress));
        let elapsed = now.duration_since(start);
        if elapsed < self.sample_window {
            return;
        }
        self.sample = None;
        let speed =
            (progress.saturating_sub(starting_progress) as f64 / elapsed.as_secs_f64()) as u64;
        if let Some(baseline) = self.baseline.take() {
            // Small changes in throughput are normal. Keep a new slot only for a clear gain.
            if speed.saturating_mul(10) < baseline.saturating_mul(11) {
                self.limit -= 1;
                self.measure_after = now + CONCURRENCY_COOLDOWN;
                return;
            }
        }
        if speed > 0 && self.limit < MAX_FILE_CONCURRENCY {
            self.baseline = Some(speed);
            self.limit += 1;
            self.measure_after = now + Duration::from_secs(2);
        }
    }

    fn rate_limited(&mut self, now: Instant, retries: u8) {
        if now >= self.retry_at {
            self.limit = self.limit.saturating_sub(1).max(1);
        }
        let delay = Duration::from_secs(5 << retries.min(MAX_FILE_RETRIES - 1));
        self.retry_at = self.retry_at.max(now + delay);
        self.measure_after = self.retry_at + CONCURRENCY_COOLDOWN;
        self.baseline = None;
        self.sample = None;
    }
}

fn file_http_status(output: &str, stderr: &str) -> Option<u16> {
    let structured = response_json(output).and_then(|value| {
        [
            &value["status"],
            &value["status_code"],
            &value["http_status"],
            &value["code"],
            &value["error"]["status"],
            &value["error"]["status_code"],
        ]
        .iter()
        .filter_map(|status| {
            status
                .as_u64()
                .and_then(|number| u16::try_from(number).ok())
                .or_else(|| {
                    status
                        .as_str()
                        .and_then(|number| number.parse::<u16>().ok())
                })
        })
        .find(|status| (400..=599).contains(status))
    });
    structured.or_else(|| {
        static HTTP_STATUS: OnceLock<regex::Regex> = OnceLock::new();
        let pattern = HTTP_STATUS.get_or_init(|| {
            regex::Regex::new(
                r"(?i)\b(?:http(?:/\d(?:\.\d)?)?(?: error)?|status(?:[ _]code)?)[\s:=]+([45]\d\d)\b",
            ).unwrap()
        });
        [stderr, output].iter().find_map(|text| pattern.captures(text)?.get(1)?.as_str().parse().ok())
    })
}

fn file_retry_kind(
    result: &Result<DownloadReply, String>,
    status: Option<u16>,
    output: &str,
    stderr: &str,
) -> Option<FileWait> {
    let message = [output, stderr].join("\n").to_ascii_lowercase();
    if status == Some(429)
        || [
            "too many requests",
            "rate limit exceeded",
            "rate limited",
            "rate-limited",
            "请求频繁",
            "请求过于频繁",
            "请求太频繁",
            "请求频率过高",
            "请求被限流",
            "触发限流",
        ]
        .iter()
        .any(|phrase| message.contains(phrase))
    {
        return Some(FileWait::RateLimited);
    }
    if [
        "permission denied",
        "access is denied",
        "no space left on device",
        "disk full",
        "拒绝访问",
        "磁盘空间不足",
    ]
    .iter()
    .any(|phrase| message.contains(phrase))
    {
        return None;
    }
    match status {
        Some(408 | 425 | 500..=599) => Some(FileWait::Retry),
        Some(_) => None,
        None if [
            "timed out",
            "context deadline exceeded",
            "connection reset",
            "unexpected eof",
            "请求超时",
            "连接被重置",
        ]
        .iter()
        .any(|phrase| message.contains(phrase)) =>
        {
            Some(FileWait::Retry)
        }
        None if matches!(result, Err(error) if error == FILE_RESULT_UNKNOWN) => {
            Some(FileWait::Retry)
        }
        None if result.is_ok() => Some(FileWait::Retry),
        None => None,
    }
}

fn stop_file_workers(jobs: &mut Jobs, id: &str) -> Result<(), String> {
    if let Some(children) = jobs.file_workers.get_mut(id) {
        for child in children.values_mut() {
            if child.try_wait().map_err(|e| e.to_string())?.is_none() {
                child.kill().map_err(|e| e.to_string())?;
                child.wait().map_err(|e| e.to_string())?;
            }
        }
    }
    jobs.file_workers.remove(id);
    Ok(())
}

fn download_file_command(bin: &Path, id: &str, relative: &Path, destination: &Path) -> Command {
    let remote = format!(
        "{}/{}",
        baidu_cleanup::task_directory(id),
        relative.to_string_lossy().replace('\\', "/")
    );
    let mut cmd = Command::new(bin);
    cmd.creation_flags(0x08000000)
        .arg("download")
        .arg(remote)
        .arg(destination)
        .args(["--no-check-update", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn transfer_command(bin: &Path, link: &Url, id: &str, picks: &[String]) -> Command {
    let mut child = Command::new(bin);
    child.creation_flags(0x08000000).arg("transfer");
    if !picks.is_empty() {
        child.arg("select");
    }
    child.arg(link.as_str());
    if !picks.is_empty() {
        child.args(["--fsid", &picks.join(",")]);
    }
    child
        .args([
            "--dir",
            &baidu_cleanup::task_directory(id),
            "--no-check-update",
            "--json",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    child
}

fn downloaded_size(path: &Path) -> Option<u64> {
    fs::symlink_metadata(path)
        .ok()
        .filter(|m| m.is_file() && m.file_attributes() & REPARSE_POINT == 0)
        .map(|m| m.len())
}

fn partial_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".part");
    PathBuf::from(name)
}

fn file_progress(
    path: &Path,
    relative: &Path,
    size: u64,
    layout: &TransferLayout,
    staged: bool,
) -> (Option<u64>, bool) {
    let final_bytes = downloaded_size(path);
    if final_bytes == Some(size) {
        return (final_bytes, true);
    }
    let partial_bytes = (staged && !layout.contains_key(&partial_path(relative)))
        .then(|| downloaded_size(&partial_path(path)))
        .flatten();
    (
        final_bytes
            .max(partial_bytes)
            .map(|bytes| bytes.min(size.saturating_sub(1))),
        false,
    )
}

fn has_partial_path_collision(layout: &TransferLayout) -> bool {
    let names: std::collections::HashSet<_> = layout
        .keys()
        .map(|path| path.to_string_lossy().to_lowercase())
        .collect();
    layout
        .keys()
        .any(|path| names.contains(&partial_path(path).to_string_lossy().to_lowercase()))
}

fn transfer_progress(root: &Path, layout: &TransferLayout) -> (u64, usize) {
    let mut bytes = 0u64;
    let mut complete = 0;
    for (relative, size) in layout {
        let Some(size) = size else { continue };
        let Ok(path) = scoped_file(root, relative, false) else {
            continue;
        };
        let (received, done) = file_progress(&path, relative, *size, layout, true);
        bytes = bytes.saturating_add(received.unwrap_or(0));
        complete += usize::from(done);
    }
    (bytes, complete)
}

fn scoped_file(root: &Path, relative: &Path, create_parent: bool) -> io::Result<PathBuf> {
    use std::path::Component;
    let safe = || io::Error::other("文件不在本次下载目录内");
    if !is_plain_dir(root) {
        return Err(safe());
    }
    let mut path = root.to_path_buf();
    let mut parts = relative.components().peekable();
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            return Err(safe());
        };
        path.push(name);
        if parts.peek().is_some() {
            if create_parent {
                match fs::create_dir(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
            }
            if !is_plain_dir(&path) {
                return Err(safe());
            }
        }
    }
    if path == root {
        return Err(safe());
    }
    Ok(path)
}

fn local_layout(root: &Path) -> Option<TransferLayout> {
    if !is_plain_dir(root) {
        return None;
    }
    let mut pending = vec![root.to_path_buf()];
    let mut layout = TransferLayout::new();
    while let Some(folder) = pending.pop() {
        for entry in fs::read_dir(folder).ok()? {
            let path = entry.ok()?.path();
            let meta = fs::symlink_metadata(&path).ok()?;
            if meta.file_attributes() & REPARSE_POINT != 0 {
                return None;
            }
            let size = if meta.is_dir() {
                pending.push(path.clone());
                None
            } else if meta.is_file() {
                Some(meta.len())
            } else {
                return None;
            };
            layout.insert(path.strip_prefix(root).ok()?.to_path_buf(), size);
        }
    }
    Some(layout)
}

fn download_command(
    bin: &Path,
    link: &Url,
    staging: &Path,
    id: &str,
    browser_saved: bool,
) -> Command {
    let mut child = Command::new(bin);
    child
        .creation_flags(0x08000000)
        .arg("download")
        .arg(if browser_saved {
            format!("{}/", baidu_cleanup::task_directory(id))
        } else {
            link.as_str().into()
        })
        .arg(staging.join(""));
    if !browser_saved {
        child.arg("-t").arg(baidu_cleanup::task_directory(id));
    }
    child
        .args(["--no-check-update", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    child
}

pub(crate) fn read_response(mut reader: impl io::Read) -> Vec<u8> {
    use io::Read;
    let mut data = Vec::new();
    let _ = reader.by_ref().take(256 * 1024).read_to_end(&mut data);
    let _ = io::copy(&mut reader, &mut io::sink());
    data
}

// Windows protects the extraction code for the current user. Task URLs stay public.
fn protect_share_code(mut input: Vec<u8>, encrypt: bool) -> Option<Vec<u8>> {
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

pub(crate) fn has_share_code(link: &Url) -> bool {
    link.query_pairs().any(|(key, _)| key == "pwd")
}

fn response_json(output: &str) -> Option<serde_json::Value> {
    serde_json::from_str(output.trim()).ok().or_else(|| {
        output
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str(line).ok())
    })
}

#[derive(Debug)]
enum DownloadReply {
    Completed(serde_json::Value),
    Submitted(PendingTransfer),
    Busy,
}

fn pending_receipt(value: Option<&serde_json::Value>) -> DownloadReply {
    let task_id = value
        .and_then(|v| v["data"].get("task_id").or_else(|| v.get("task_id")))
        .and_then(|id| {
            id.as_str()
                .map(str::to_owned)
                .or_else(|| id.as_u64().map(|id| id.to_string()))
        })
        .filter(|value| share_component(value));
    DownloadReply::Submitted(PendingTransfer { task_id })
}

fn download_result(success: bool, output: &str, stderr: &str) -> Result<DownloadReply, String> {
    let response = response_json(output);
    if let Some(response) = response
        .as_ref()
        .filter(|value| success && value["code"].as_i64() == Some(0))
    {
        if response["data"]["status"]
            .as_str()
            .or_else(|| response["status"].as_str())
            == Some("submitted")
        {
            return Ok(pending_receipt(Some(response)));
        }
        return Ok(DownloadReply::Completed(response.clone()));
    }
    let reported = response.as_ref().and_then(|result| {
        result["error"]
            .as_str()
            .or_else(|| result["error"]["message"].as_str())
            .or_else(|| result["message"].as_str())
    });
    let error = reported
        .or_else(|| {
            stderr
                .lines()
                .chain(output.lines())
                .find_map(|line| line.trim().strip_prefix("Error:"))
        })
        .unwrap_or_default()
        .to_ascii_lowercase();
    static ERRNO: OnceLock<regex::Regex> = OnceLock::new();
    let errno = ERRNO
        .get_or_init(|| regex::Regex::new(r"(?:errno|错误码)\s*[=:：]?\s*(-?\d{1,8})\b").unwrap())
        .captures(&error)
        .and_then(|value| value[1].parse::<i32>().ok())
        .or_else(|| {
            response.as_ref().and_then(|result| {
                result["errno"]
                    .as_i64()
                    .or_else(|| result["error"]["errno"].as_i64())
                    .and_then(|code| i32::try_from(code).ok())
            })
        });
    let message = if errno == Some(13002) {
        "百度网盘分享已过期"
    } else if matches!(errno, Some(13001 | 13004)) {
        "百度网盘分享已失效，请检查链接"
    } else if errno == Some(13045) {
        OWN_SHARE
    } else if errno == Some(13003) || error.contains("提取码") {
        "提取码缺失或有误，点「填写提取码」继续"
    } else if errno == Some(111)
        || error.contains("未登录")
        || error.contains("请先登录")
        || error.contains("unauthorized")
        || error.contains("not logged in")
        || error.contains("not authenticated")
        || error.contains("invalid access token")
        || error.contains("token expired")
        || error.contains("token 过期")
        || error.contains("token 已过期")
    {
        "百度网盘授权已失效或尚未授权，请完成授权后继续"
    } else if error.contains("容量不足") || error.contains("空间不足") {
        "百度网盘空间不足，无法转存分享"
    } else if errno == Some(13070) {
        return Ok(pending_receipt(response.as_ref()));
    } else if errno == Some(13071) {
        return Ok(DownloadReply::Busy);
    } else if matches!(errno, Some(13072 | 13073)) {
        "这份分享超过百度网盘单次转存的文件数量上限"
    } else {
        return Err(match errno {
            Some(code) => format!("百度网盘未完成下载（百度错误码 {code}）"),
            None => FILE_RESULT_UNKNOWN.into(),
        });
    };
    Err(message.into())
}

fn is_plain_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && m.file_attributes() & REPARSE_POINT == 0)
}

fn tidy_staging(dir: &Path) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        if name
            .to_str()
            .and_then(|name| name.strip_prefix(".shandian-baidu-"))
            .is_some_and(|id| id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()))
            && is_plain_dir(&entry.path())
            && fs::remove_dir(entry.path()).is_err()
        {
            let _ = hide_staging(&entry.path(), true);
        }
    }
}

pub(crate) fn hide_staging(path: &Path, hidden: bool) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        SetFileAttributesW, FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_NOT_CONTENT_INDEXED, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM,
    };
    let flags = fs::symlink_metadata(path)?.file_attributes();
    let writable = flags
        & (FILE_ATTRIBUTE_READONLY
            | FILE_ATTRIBUTE_SYSTEM
            | FILE_ATTRIBUTE_ARCHIVE
            | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED);
    let attributes = if hidden {
        writable | FILE_ATTRIBUTE_HIDDEN
    } else {
        writable
    };
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let attributes = if attributes == 0 {
        FILE_ATTRIBUTE_NORMAL
    } else {
        attributes
    };
    if unsafe { SetFileAttributesW(wide.as_ptr(), attributes) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(crate) fn measure(path: &Path) -> u64 {
    let Ok(info) = fs::symlink_metadata(path) else {
        return 0;
    };
    if info.file_attributes() & REPARSE_POINT != 0 {
        return 0;
    }
    if info.is_file() {
        return info.len();
    }
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| measure(&entry.path()))
        .sum()
}

pub fn move_file(source: &Path, dest: &Path, flags: u32) -> io::Result<()> {
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let (source, dest) = (wide(source), wide(dest));
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(source.as_ptr(), dest.as_ptr(), flags)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn clean_name(value: &str) -> Option<String> {
    let mut name: String = crate::engine::safe_component(value)
        .chars()
        .take(120)
        .collect();
    if name.is_empty() || matches!(name.as_str(), "." | ".." | "百度网盘分享") {
        return None;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "NUL" | "PRN" | "AUX")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix)
                .is_some_and(|tail| tail.len() == 1 && matches!(tail.as_bytes()[0], b'1'..=b'9'))
        })
    {
        name.insert(0, '_');
    }
    Some(name)
}

fn share_entries(value: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    let data = &value["data"];
    value
        .as_array()
        .or_else(|| data.as_array())
        .or_else(|| data["list"].as_array())
        .or_else(|| data["files"].as_array())
        .or_else(|| data["items"].as_array())
        .or_else(|| value["files"].as_array())
        .or_else(|| value["list"].as_array())
        .or_else(|| value["items"].as_array())
}

fn share_more(value: &serde_json::Value) -> bool {
    value["has_more"]
        .as_bool()
        .or_else(|| value["data"]["has_more"].as_bool())
        .unwrap_or(false)
}

#[derive(Clone)]
struct ShareManifest {
    layout: TransferLayout,
    roots: Vec<String>,
    ids: std::collections::BTreeMap<PathBuf, String>,
}

impl ShareManifest {
    fn selected(&self, selection: &[SharePick]) -> Option<Self> {
        if selection.is_empty() {
            return None;
        }
        let paths: HashMap<_, _> = self.ids.iter().map(|(path, id)| (id, path)).collect();
        if paths.len() != self.ids.len() {
            return None;
        }
        let selected = selection
            .iter()
            .map(|item| {
                let path = *paths.get(&item.id)?;
                (path.file_name()?.to_str()?.trim() == item.name.trim())
                    .then_some((path.as_path(), &item.id))
            })
            .collect::<Option<std::collections::BTreeMap<_, _>>>()?;
        let mut layout = TransferLayout::new();
        let mut ids = std::collections::BTreeMap::new();
        let mut roots = Vec::new();
        for (root, root_id) in &selected {
            if root
                .ancestors()
                .skip(1)
                .any(|parent| selected.contains_key(parent))
            {
                continue;
            }
            roots.push((*root_id).clone());
            for (source, size) in self.layout.range(root.to_path_buf()..) {
                let Ok(child) = source.strip_prefix(root) else {
                    break;
                };
                let target = if child.as_os_str().is_empty() {
                    PathBuf::from(root.file_name()?)
                } else {
                    PathBuf::from(root.file_name()?).join(child)
                };
                if layout.insert(target.clone(), *size).is_some() {
                    return None;
                }
                if let Some(id) = self.ids.get(source) {
                    ids.insert(target, id.clone());
                }
            }
        }
        layout
            .values()
            .any(Option::is_some)
            .then_some(Self { layout, roots, ids })
    }
}

#[derive(Clone)]
struct ShareMetadata {
    name: Option<String>,
    manifest: Option<ShareManifest>,
}

struct ShareLookup {
    key: [u8; 32],
    result: Arc<Mutex<Option<(Instant, Vec<SharePick>, ShareMetadata)>>>,
}

fn share_manifest(
    first: serde_json::Value,
    list: &impl Fn(&str, usize) -> Option<serde_json::Value>,
    selection: &[SharePick],
) -> Option<ShareManifest> {
    let requested: HashMap<_, _> = selection
        .iter()
        .map(|pick| (pick.id.as_str(), pick.name.trim()))
        .collect();
    let mut found = std::collections::HashSet::new();
    let mut pending = vec![(
        String::new(),
        PathBuf::new(),
        1,
        Some(first),
        selection.is_empty(),
    )];
    let mut seen = std::collections::HashSet::new();
    let mut layout = TransferLayout::new();
    let mut ids = std::collections::BTreeMap::new();
    let mut roots = Vec::new();
    while let Some((dir, relative, page, cached, inherited)) = pending.pop() {
        if !inherited && found.len() == requested.len() {
            continue;
        }
        let value = cached.or_else(|| list(&dir, page))?;
        let entries = share_entries(&value)?;
        for entry in entries {
            let path = entry["path"].as_str().filter(|path| !path.is_empty())?;
            if !seen.insert(path.to_owned()) {
                return None;
            }
            let name = entry["name"]
                .as_str()
                .or_else(|| entry["server_filename"].as_str())
                .or_else(|| path.trim_end_matches('/').rsplit('/').next())?;
            if name.is_empty() || name.contains(['/', '\\', ':']) || name == "." || name == ".." {
                return None;
            }
            let id = entry
                .get("fs_id")
                .or_else(|| entry.get("fsid"))
                .and_then(|id| {
                    id.as_str()
                        .map(str::to_owned)
                        .or_else(|| id.as_u64().map(|id| id.to_string()))
                });
            let chosen = if let Some((id, display)) = id
                .as_ref()
                .and_then(|id| requested.get(id.as_str()).map(|name| (id, name)))
            {
                if name.trim() != *display {
                    return None;
                }
                found.insert(id.clone());
                if !inherited {
                    roots.push(id.clone());
                }
                true
            } else {
                false
            };
            let include = inherited || chosen;
            let flag = entry.get("is_dir").or_else(|| entry.get("isdir"))?;
            let directory = flag.as_bool().or_else(|| flag.as_u64().map(|n| n != 0))?;
            let child = if inherited {
                relative.join(name)
            } else {
                PathBuf::from(name)
            };
            if directory {
                pending.push((path.to_owned(), child.clone(), 1, None, include));
            }
            if include {
                let size = if directory {
                    None
                } else {
                    Some(entry["size"].as_u64()?)
                };
                if let Some(id) = id {
                    ids.insert(child.clone(), id);
                }
                if layout.insert(child, size).is_some() {
                    return None;
                }
            }
        }
        if share_more(&value) {
            if entries.is_empty() {
                return None;
            }
            pending.push((dir, relative, page.checked_add(1)?, None, inherited));
        }
    }
    if found.len() != requested.len() || !layout.values().any(Option::is_some) {
        return None;
    }
    Some(ShareManifest { layout, roots, ids })
}

fn layout_size(layout: &TransferLayout) -> Option<u64> {
    layout
        .values()
        .flatten()
        .copied()
        .try_fold(0, u64::checked_add)
}

fn bundle_stem(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    for suffix in [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst"] {
        if name.len() > suffix.len() && lower.ends_with(suffix) {
            return name[..name.len() - suffix.len()].to_owned();
        }
    }
    if let Some((stem, extension)) = name.rsplit_once('.') {
        if !stem.is_empty()
            && extension.len() <= 15
            && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
            && extension.bytes().any(|byte| byte.is_ascii_alphabetic())
        {
            return stem.to_owned();
        }
    }
    name.to_owned()
}

fn name_from_listing(value: &serde_json::Value) -> Option<String> {
    let files = share_entries(value)?;
    let first = files.first()?;
    let name = clean_name(
        first["server_filename"]
            .as_str()
            .or_else(|| first["name"].as_str())?,
    )?;
    let directory = first["is_dir"]
        .as_bool()
        .or_else(|| first["isdir"].as_bool())
        .unwrap_or(false);
    if directory || (files.len() <= 1 && !share_more(value)) {
        Some(name)
    } else {
        Some(bundle_stem(&name))
    }
}

fn commit(job: &Job, staging: &Path) -> Result<(PathBuf, u64, Option<std::ffi::OsString>), String> {
    let entries = fs::read_dir(staging)
        .and_then(|entries| entries.collect::<io::Result<Vec<_>>>())
        .map_err(|_| "无法读取百度网盘下载结果")?;
    let source = match entries.as_slice() {
        [] => return Err("百度网盘没有返回文件".into()),
        [entry] => entry.path(),
        _ => staging.to_path_buf(),
    };
    let source_name = if source == staging {
        None
    } else {
        source.file_name().map(|name| name.to_os_string())
    };
    let name = if let Some(custom) = &job.requested_name {
        let name = if source.is_file() {
            let ext = source
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("");
            let suffix = format!(".{ext}");
            if ext.is_empty()
                || custom
                    .to_ascii_lowercase()
                    .ends_with(&suffix.to_ascii_lowercase())
            {
                custom.clone()
            } else {
                format!("{custom}{suffix}")
            }
        } else {
            custom.clone()
        };
        name.into()
    } else if source == staging {
        job.label().into()
    } else {
        source.file_name().unwrap_or_default().to_os_string()
    };
    let name = Path::new(&name);
    let bytes = measure(&source);
    if source == staging {
        hide_staging(staging, false).map_err(|_| "无法准备保存目录，临时文件已保留")?;
    }
    for n in 0..1000 {
        let path = if n == 0 {
            job.dir.join(name)
        } else if source.is_file() {
            let stem = name.file_stem().unwrap_or_default().to_string_lossy();
            let ext = name
                .extension()
                .map(|ext| format!(".{}", ext.to_string_lossy()))
                .unwrap_or_default();
            job.dir.join(format!("{stem} ({n}){ext}"))
        } else {
            job.dir.join(format!("{} ({n})", name.display()))
        };
        match move_file(
            &source,
            &path,
            windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
        ) {
            Ok(()) => {
                if source != staging {
                    let _ = fs::remove_dir(staging);
                }
                return Ok((path, bytes, source_name));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists || path.exists() => {
                if !job.allow_duplicate {
                    if source == staging {
                        let _ = hide_staging(staging, true);
                    }
                    return Err(COLLISION.into());
                }
            }
            Err(_) => break,
        }
    }
    if source == staging {
        let _ = hide_staging(staging, true);
    }
    Err("无法移动下载结果，临时文件已保留".into())
}

#[tauri::command]
pub async fn save_baidu_copy(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let id = id
        .strip_prefix("bd:")
        .ok_or("找不到百度网盘任务")?
        .to_owned();
    let downloads = Arc::clone(&state.baidu);
    tauri::async_runtime::spawn_blocking(move || downloads.approve_duplicate(&id))
        .await
        .map_err(|_| "无法继续保存百度网盘文件")?
}

#[tauri::command]
pub async fn preview_baidu_share(
    link: String,
    selection: Option<Vec<SharePick>>,
    state: State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let link = share_url(&link).ok_or("百度网盘分享链接无效")?;
    let selection = normalize_selection(selection)?;
    let downloads = Arc::clone(&state.baidu);
    tauri::async_runtime::spawn_blocking(move || {
        let bin = executable(&downloads.tool).ok_or("下载前可完成百度网盘授权")?;
        let deadline = Instant::now() + Duration::from_secs(15);
        let active = || Instant::now() < deadline && !downloads.jobs.lock().unwrap().stopping;
        let manifest = downloads
            .share_metadata(&bin, &link, &selection, &active)?
            .and_then(|metadata| metadata.manifest)
            .ok_or("暂时无法读取文件列表")?;
        Ok(manifest
            .layout
            .into_iter()
            .map(|(path, size)| {
                json!({
                    "path": path.to_string_lossy(),
                    "size": size,
                    "directory": size.is_none(),
                    "id": manifest.ids.get(&path),
                })
            })
            .collect())
    })
    .await
    .map_err(|_| "暂时无法读取文件列表")?
}

#[tauri::command]
pub async fn add_baidu_share(
    link: String,
    dir: String,
    force: Option<bool>,
    allow_duplicate: Option<bool>,
    selection: Option<Vec<SharePick>>,
    cleanup_remote: Option<bool>,
    retry_id: Option<String>,
    name: Option<String>,
    suggested_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<AddBaiduShare, String> {
    let url = share_url(&link).ok_or("百度网盘分享链接无效")?;
    let selection = normalize_selection(selection)?;
    let dir = PathBuf::from(dir.trim().replace('/', "\\"));
    if !dir.is_absolute() {
        return Err("请选择本机的保存目录".into());
    }
    let downloads = Arc::clone(&state.baidu);
    tauri::async_runtime::spawn_blocking(move || {
        executable(&downloads.tool).ok_or(SETUP)?;
        if let Some(retry) = retry_id {
            let id = retry
                .strip_prefix("bd:")
                .ok_or("找不到原来的百度下载任务")?;
            downloads.resume_with_share(id, Some(url))?;
            return Ok(AddBaiduShare::Started { id: retry });
        }
        downloads.add(
            url,
            dir,
            force.unwrap_or(false),
            force == Some(true) && allow_duplicate == Some(true),
            selection,
            cleanup_remote,
            name.as_deref().and_then(clean_name),
            suggested_name.as_deref().and_then(clean_name),
        )
    })
    .await
    .map_err(|_| "无法创建百度网盘任务")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawer_keeps_nested_files_and_allows_opening_a_finished_item() {
        let dir =
            std::env::temp_dir().join(format!("sd-baidu-drawer-{:016x}", rand::random::<u64>()));
        let mut job = Job::pending(
            &share_url("https://pan.baidu.com/s/sample").unwrap(),
            dir.clone(),
            false,
        );
        let first = PathBuf::from("来源").join("片段 1.mp4");
        let second = PathBuf::from("来源").join("片段 2.mp4");
        job.transfer_layout = Some(TransferLayout::from([
            (first.clone(), Some(3)),
            (second.clone(), Some(5)),
        ]));
        job.files_total = 2;
        let root = job.staging();
        fs::create_dir_all(root.join("来源")).unwrap();
        fs::write(root.join(&first), b"abc").unwrap();
        job.status = "running".into();
        let partial = partial_path(&root.join(&second));
        fs::write(&partial, b"ab").unwrap();
        let rows = job.drawer_files();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["status"], "done");
        assert_eq!(rows[1]["downloaded"], 2);
        assert_eq!(rows[1]["status"], "running");
        assert_eq!(job.drawer_path(0).unwrap().1, root.join(&first));
        assert!(job.drawer_path(1).is_err());
        fs::write(&partial, b"abcde").unwrap();
        let rows = job.drawer_files();
        assert_eq!(rows[1]["downloaded"], 4);
        assert_eq!(rows[1]["status"], "running");
        fs::rename(partial, root.join(&second)).unwrap();
        let final_dir = dir.join("课程");
        fs::rename(root.join("来源"), &final_dir).unwrap();
        job.output = Some(final_dir.clone());
        job.output_root_name = Some("来源".into());
        job.status = "done".into();
        assert_eq!(job.drawer_path(1).unwrap().1, final_dir.join("片段 2.mp4"));
        assert_eq!(
            job.drawer_video_path(None),
            Some(final_dir.join("片段 1.mp4"))
        );
        assert_eq!(
            job.drawer_video_path(Some("来源\\片段 2.mp4")),
            Some(final_dir.join("片段 2.mp4"))
        );
        fs::remove_file(final_dir.join("片段 1.mp4")).unwrap();
        assert_eq!(
            job.drawer_video_path(None),
            Some(final_dir.join("片段 2.mp4"))
        );
        assert!(job.drawer_video_path(Some("来源\\片段 1.mp4")).is_none());
        assert_eq!(bundle_stem("32 4K.mp4"), "32 4K");
        assert_eq!(bundle_stem("素材.tar.gz"), "素材");
        assert_eq!(bundle_stem("版本.5"), "版本.5");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn schedules_three_files_and_retries_a_rate_limited_file_in_place() {
        let dir =
            std::env::temp_dir().join(format!("sd-baidu-parallel-{:016x}", rand::random::<u64>()));
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("bdpan.cmd");
        fs::write(
            &bin,
            concat!(
                "@echo off\r\n",
                "ping -n 2 127.0.0.1 >nul\r\n",
                "if \"%~nx3\"==\"b.txt\" if not exist \"%~dp0limited\" (\r\n",
                "  type nul >\"%~dp0limited\"\r\n",
                "  echo {\"code\":429}\r\n",
                "  exit /b 1\r\n",
                ")\r\n",
                "echo x>>\"%~3.part\"\r\n",
                "move /y \"%~3.part\" \"%~3\">nul\r\n",
                "echo {\"code\":0}\r\n"
            ),
        )
        .unwrap();
        let mut job = Job::pending(
            &share_url("https://pan.baidu.com/s/test").unwrap(),
            dir.clone(),
            false,
        );
        job.status = "running".into();
        let root = job.staging();
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"done").unwrap();
        fs::write(root.join("a.txt.part"), b"stale").unwrap();
        fs::write(root.join("b.txt.part"), b"pre").unwrap();
        let id = job.id.clone();
        let downloads = Arc::new(Downloads {
            file: dir.join("state.json"),
            tool: bin.clone(),
            jobs: Mutex::new(Jobs {
                records: vec![job.clone()],
                ..Default::default()
            }),
        });
        let layout: TransferLayout = ["a.txt", "b.txt", "c.txt", "d.txt"]
            .into_iter()
            .map(|f| {
                (
                    PathBuf::from(f),
                    Some(match f {
                        "a.txt" => 4,
                        "b.txt" => 6,
                        _ => 3,
                    }),
                )
            })
            .collect();
        let worker = Arc::clone(&downloads);
        let run = std::thread::spawn(move || worker.download_parallel(&job, &bin, &layout));
        let mut observed = false;
        let mut waited = false;
        for _ in 0..160 {
            {
                let jobs = downloads.jobs.lock().unwrap();
                observed |= jobs
                    .file_workers
                    .get(&id)
                    .is_some_and(|children| children.len() == 3);
                waited |= jobs.records[0].file_wait == Some(FileWait::RateLimited);
            }
            if observed && waited {
                break;
            }
            if run.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        run.join().unwrap().unwrap();
        assert!(observed);
        assert!(waited);
        assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"done");
        assert_eq!(fs::read(root.join("b.txt")).unwrap(), b"prex\r\n");
        assert_eq!(
            local_layout(&root).as_ref().map(|files| files.len()),
            Some(4)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saves_and_reports_completion_before_cloud_cleanup() {
        let dir = std::env::temp_dir().join(format!(
            "sd-baidu-background-{:016x}",
            rand::random::<u64>()
        ));
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("bdpan.cmd");
        let link = share_url("https://pan.baidu.com/s/sample").unwrap();
        let mut job = Job::pending(&link, dir.clone(), true);
        job.status = "running".into();
        job.ready_to_move = true;
        job.confirmed_transfer = true;
        job.requested_name = Some("已保存".into());
        job.transfer_layout = Some(TransferLayout::from([(PathBuf::from("clip.bin"), Some(3))]));
        fs::create_dir(job.staging()).unwrap();
        fs::write(job.staging().join("clip.bin"), b"abc").unwrap();
        let id = job.id.clone();
        fs::write(
            &bin,
            format!(
                concat!(
                "@echo off\r\n",
                    "if \"%~2\"==\"ls\" (\r\n",
                "ping -n 2 127.0.0.1 >nul\r\n",
                "echo [{{\"path\":\"/apps/bdpan/Shandian/{}/clip.bin\",",
                "\"isdir\":false,\"size\":3,\"md5\":\"900150983cd24fb0d6963f7d28e17f72\"}}]\r\n",
                "exit /b 0\r\n",
                ")\r\n",
                    "if \"%~2\"==\"rm\" echo {{\"code\":0}}\r\n",
            ),
                id
            ),
        )
        .unwrap();
        let downloads = Arc::new(Downloads {
            file: dir.join("state.json"),
            tool: bin.clone(),
            jobs: Mutex::new(Jobs {
                records: vec![job.clone()],
                ..Default::default()
            }),
        });
        let worker = Arc::clone(&downloads);
        let run = std::thread::spawn(move || worker.run(job, bin, link));
        let mut saw_saved = false;
        for _ in 0..80 {
            let job = downloads.find(&id).unwrap();
            if job.status == "done" && job.cleanup_note == VERIFY_PENDING {
                assert_eq!(fs::read(job.output.unwrap()).unwrap(), b"abc");
                let saved: Vec<Job> =
                    serde_json::from_slice(&fs::read(&downloads.file).unwrap()).unwrap();
                assert_eq!(saved[0].status, "done");
                assert_eq!(saved[0].cleanup_note, VERIFY_PENDING);
                saw_saved = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        run.join().unwrap();
        assert!(saw_saved);
        assert_eq!(downloads.find(&id).unwrap().cleanup_note, "网盘转存已清理");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn small_batch_runs_eight_workers_and_recovers_failed_items() {
        let dir =
            std::env::temp_dir().join(format!("sd-baidu-small-{:016x}", rand::random::<u64>()));
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("bdpan.cmd");
        fs::write(
            &bin,
            concat!(
                "@echo off\r\n",
                "ping -n 2 127.0.0.1 >nul\r\n",
                "if \"%~nx3\"==\"00.txt\" if not exist \"%~dp0empty-once\" (\r\n",
                "  type nul >\"%~dp0empty-once\"\r\n",
                "  echo x>\"%~3.part\"\r\n",
                "  exit /b 1\r\n",
                ")\r\n",
                "if \"%~nx3\"==\"01.txt\" if not exist \"%~dp0unavailable-once\" (\r\n",
                "  type nul >\"%~dp0unavailable-once\"\r\n",
                "  echo Error: HTTP 503 1>&2\r\n",
                "  exit /b 1\r\n",
                ")\r\n",
                "if \"%~nx3\"==\"02.txt\" (\r\n",
                "  echo z>\"%~3\"\r\n",
                "  exit /b 1\r\n",
                ")\r\n",
                "echo y>>\"%~3.part\"\r\n",
                "move /y \"%~3.part\" \"%~3\">nul\r\n",
                "echo {\"code\":0}\r\n"
            ),
        )
        .unwrap();
        let link = share_url("https://pan.baidu.com/s/test").unwrap();
        let mut job = Job::pending(&link, dir.clone(), false);
        job.status = "running".into();
        let id = job.id.clone();
        let root = job.staging();
        let layout: TransferLayout = (0..18)
            .map(|index| {
                (
                    PathBuf::from(format!("{index:02}.txt")),
                    Some(if index == 0 { 6 } else { 3 }),
                )
            })
            .collect();
        let downloads = Arc::new(Downloads {
            file: dir.join("state.json"),
            tool: bin.clone(),
            jobs: Mutex::new(Jobs {
                records: vec![job.clone()],
                ..Default::default()
            }),
        });
        let expected = layout.clone();
        let worker = Arc::clone(&downloads);
        let run = std::thread::spawn(move || worker.download_parallel(&job, &bin, &layout));
        let mut eight_running = false;
        for _ in 0..100 {
            {
                let jobs = downloads.jobs.lock().unwrap();
                eight_running |= jobs
                    .file_workers
                    .get(&id)
                    .is_some_and(|files| files.len() == 8);
            }
            if eight_running || run.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        run.join().unwrap().unwrap();
        assert!(eight_running);
        assert_eq!(fs::read(root.join("00.txt")).unwrap(), b"x\r\ny\r\n");
        assert_eq!(fs::read(root.join("02.txt")).unwrap(), b"z\r\n");
        assert_eq!(local_layout(&root), Some(expected));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrency_probes_higher_throughput_and_releases_slots_on_limits() {
        let start = Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);
        let mut concurrency = FileConcurrency::new(start, false);
        assert_eq!(concurrency.limit, 3);
        concurrency.observe(at(2), 200, true);
        concurrency.observe(at(8), 800, true); // 100 bytes/s at 3
        assert_eq!(concurrency.limit, 4);
        concurrency.observe(at(10), 1_000, true);
        concurrency.observe(at(16), 1_780, true); // 130 bytes/s at 4
        assert_eq!(concurrency.limit, 5);
        concurrency.observe(at(18), 2_000, true);
        concurrency.observe(at(24), 2_960, true); // 160 bytes/s at 5
        assert_eq!(concurrency.limit, 6);
        concurrency.observe(at(26), 3_000, true);
        concurrency.observe(at(32), 4_140, true); // 190 bytes/s at 6
        assert_eq!(concurrency.limit, 7);
        concurrency.rate_limited(at(33), 0);
        assert_eq!(concurrency.limit, 6);
        assert_eq!(concurrency.retry_at, at(38));
        concurrency.rate_limited(at(34), 0);
        assert_eq!(concurrency.limit, 6); // the same burst
        assert_eq!(concurrency.retry_at, at(39));
        concurrency.rate_limited(at(39), 1);
        assert_eq!(concurrency.limit, 5);
        assert_eq!(concurrency.retry_at, at(49));
        concurrency.observe(at(50), 5_000, true);
        concurrency.observe(at(68), 7_000, true);
        assert_eq!(concurrency.limit, 5);
        assert!(concurrency.sample.is_none());
    }

    #[test]
    fn concurrency_drops_a_slot_without_a_clear_speed_gain() {
        let start = Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);
        let mut concurrency = FileConcurrency::new(start, false);
        concurrency.observe(at(2), 0, true);
        concurrency.observe(at(8), 600, true);
        assert_eq!(concurrency.limit, 4);
        concurrency.observe(at(10), 800, true);
        concurrency.observe(at(16), 1_430, true); // 5% improvement is too small
        assert_eq!(concurrency.limit, 3);
        let classify = |stdout, stderr| {
            file_retry_kind(
                &download_result(false, stdout, stderr),
                file_http_status(stdout, stderr),
                stdout,
                stderr,
            )
        };
        assert_eq!(
            classify("", "Error: HTTP Error 429"),
            Some(FileWait::RateLimited)
        );
        assert_eq!(
            classify(r#"{"error":{"status":"429"}}"#, ""),
            Some(FileWait::RateLimited)
        );
        assert_eq!(classify("", "请求过于频繁"), Some(FileWait::RateLimited));
        assert_eq!(classify(r#"{"code":1,"errno":111}"#, "HTTP 403"), None);
        assert_eq!(
            classify("", "Error: There is no space left on device"),
            None
        );
        assert_eq!(classify("", "downloaded 429 bytes"), Some(FileWait::Retry));
    }

    #[test]
    fn completed_duplicate_returns_a_choice_without_starting_a_worker() {
        let url = share_url("https://pan.baidu.com/s/sample").unwrap();
        let dir = std::env::temp_dir();
        let mut job = Job::pending(&url, dir.clone(), false);
        let task_id = format!("bd:{}", job.id);
        job.status = "done".into();
        job.output = Some(dir.clone());
        let downloads = Arc::new(Downloads {
            file: PathBuf::new(),
            tool: PathBuf::new(),
            jobs: Mutex::new(Jobs {
                records: vec![job],
                ..Default::default()
            }),
        });
        let result = downloads
            .add(url, dir.clone(), false, false, Vec::new(), None, None, None)
            .unwrap();
        assert!(
            matches!(&result, AddBaiduShare::AlreadyDownloaded { id, folder: Some(folder), .. }
            if id == &task_id && folder == &dir.to_string_lossy())
        );
        assert_eq!(
            serde_json::to_value(result).unwrap()["status"],
            "alreadyDownloaded"
        );
        let jobs = downloads.jobs.lock().unwrap();
        assert_eq!(jobs.records.len(), 1);
        assert!(jobs.workers.is_empty());
    }

    #[test]
    fn clean_share_retains_code_only_for_download() {
        let url = share_url("https://pan.baidu.com/s/test?pwd=ABc9&from=share#list").unwrap();
        assert_eq!(url.as_str(), "https://pan.baidu.com/s/test?pwd=ABc9");
        assert_eq!(public_url(&url), "https://pan.baidu.com/s/test");
        let extraction_page =
            share_url("https://pan.baidu.com/share/init?surl=Sample&pwd=ABc9").unwrap();
        assert_eq!(
            extraction_page.as_str(),
            "https://pan.baidu.com/s/1Sample?pwd=ABc9"
        );
        assert_eq!(
            crate::links::canonical_url("https://pan.baidu.com/share/init?surl=Sample"),
            crate::links::canonical_url("https://pan.baidu.com/s/1Sample?pwd=Good"),
        );
        assert!(share_url("https://pan.baidu.com.evil.test/s/test").is_none());
        assert!(share_url("https://pan.baidu.com/disk/main").is_none());
        assert_eq!(
            with_pasted_code("https://pan.baidu.com/s/test", "\n提取码：ABc9").as_deref(),
            Some("https://pan.baidu.com/s/test?pwd=ABc9")
        );
        assert_eq!(
            with_pasted_code("https://pan.baidu.com/s/test提取码：Ab12", "").as_deref(),
            Some("https://pan.baidu.com/s/test?pwd=Ab12")
        );
        assert_eq!(
            with_pasted_code(
                "https://pan.baidu.com/s/test#list/path=%2FHoudini+tools",
                ""
            )
            .as_deref(),
            Some("https://pan.baidu.com/s/test")
        );
        assert_eq!(
            with_pasted_code("https://pan.baidu.com/s/test?pwd=Good", "，密码 = ‘Ab12’").as_deref(),
            Some("https://pan.baidu.com/s/test?pwd=Good")
        );
    }

    #[test]
    fn completed_share_code_round_trips_without_entering_task_links() {
        let url = share_url("https://pan.baidu.com/s/test?pwd=Ab12").unwrap();
        let mut job = Job::pending(&url, PathBuf::new(), false);
        let ciphertext = protect_share_code(b"Ab12".to_vec(), true).unwrap();
        assert_ne!(ciphertext, b"Ab12");
        job.protected_share_code = Some(ciphertext);
        job.status = "done".into();
        let restored: Job = serde_json::from_str(&serde_json::to_string(&job).unwrap()).unwrap();
        let id = format!("bd:{}", job.id);
        let downloads = Downloads {
            file: PathBuf::new(),
            tool: PathBuf::new(),
            jobs: Mutex::new(Jobs {
                records: vec![restored],
                ..Default::default()
            }),
        };
        assert_eq!(
            downloads
                .saved_code_for_redownload(&id, &job.url)
                .as_deref(),
            Some("Ab12")
        );
        assert_eq!(
            downloads.saved_code_for_redownload(&id, "https://pan.baidu.com/s/other"),
            None
        );
        assert!(!serde_json::to_string(&job.as_task().unwrap())
            .unwrap()
            .contains("Ab12"));
    }

    #[test]
    fn cli_requires_confirmed_success() {
        assert!(download_result(true, r#"{"code":0}"#, "").is_ok());
        assert!(matches!(
            download_result(true, r#"{"code":0,"data":{"status":"submitted","task_id":"876"}} "#, ""),
            Ok(DownloadReply::Submitted(PendingTransfer { task_id: Some(id) })) if id == "876"
        ));
        assert!(matches!(
            download_result(false, r#"{"code":1,"errno":13071}"#, ""),
            Ok(DownloadReply::Busy)
        ));
        assert!(download_result(false, r#"{"code":0}"#, "").is_err());
        assert!(download_result(true, "", "").is_err());
        assert_eq!(
            download_result(true, r#"{"code":1,"error":"errno=13002,"}"#, "").unwrap_err(),
            "百度网盘分享已过期"
        );
        assert_eq!(
            download_result(true, r#"{"code":1,"errno":13045}"#, "").unwrap_err(),
            OWN_SHARE
        );
        assert_eq!(
            download_result(
                false,
                "",
                "Error: 下载失败（错误码 -9）\nUsage: --pwd 提取码"
            )
            .unwrap_err(),
            "百度网盘未完成下载（百度错误码 -9）"
        );
    }

    #[test]
    fn download_paths_and_existing_file_survive_commit() {
        let dir = std::env::temp_dir().join(format!(
            "shandian-baidu-test-{:016x}",
            rand::random::<u64>()
        ));
        let mut job = Job {
            id: "test".into(),
            url: "https://pan.baidu.com/s/test".into(),
            selection: Vec::new(),
            selection_roots: Vec::new(),
            files_done: 0,
            files_total: 0,
            parallel_files: false,
            has_code: true,
            protected_share_code: None,
            dir: dir.clone(),
            created: 0,
            status: "running".into(),
            error: String::new(),
            bytes: 0,
            total_bytes: 0,
            speed: 0,
            output: None,
            output_root_name: None,
            ready_to_move: false,
            confirmed_transfer: false,
            pending_transfer: None,
            transfer_layout: None,
            downloading_from_cloud: false,
            file_wait: None,
            allow_duplicate: false,
            cleanup_remote: true,
            cleanup_note: String::new(),
            browser_saving: false,
            browser_saved: false,
            requested_name: None,
            suggested_name: None,
        };
        let stage = job.staging();
        fs::create_dir_all(&stage).unwrap();
        fs::write(dir.join("示例 音频.mp3"), b"existing").unwrap();
        fs::write(stage.join("示例 音频.mp3"), b"download").unwrap();
        let link = share_url("https://pan.baidu.com/s/test?pwd=aBC9").unwrap();
        let cmd = download_command(Path::new("bdpan.exe"), &link, &stage, &job.id, false);
        let args = cmd.get_args().collect::<Vec<_>>();
        assert_eq!(args[0], "download");
        assert_eq!(args[1], link.as_str());
        assert!(args[2].to_string_lossy().ends_with('\\'));
        assert_eq!(args[4], "Shandian/test");
        assert_eq!(commit(&job, &stage).unwrap_err(), COLLISION);
        assert!(stage.join("示例 音频.mp3").exists());
        job.allow_duplicate = true;
        let (output, bytes, _) = commit(&job, &stage).unwrap();
        assert_eq!(output, dir.join("示例 音频 (1).mp3"));
        assert_eq!(bytes, 8);
        assert_eq!(fs::read(dir.join("示例 音频.mp3")).unwrap(), b"existing");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_confirmed_web_save_downloads_only_the_existing_task_directory() {
        let link = share_url("https://pan.baidu.com/s/test").unwrap();
        let id = "0123456789abcdef";
        let stage = Path::new(r"C:\Downloads\fixture");
        let cmd = download_command(Path::new("bdpan.exe"), &link, stage, id, true);
        let args = cmd.get_args().collect::<Vec<_>>();
        assert_eq!(args[0], "download");
        assert_eq!(args[1], "Shandian/0123456789abcdef/");
        assert!(!args.contains(&std::ffi::OsStr::new("-t")));
        assert!(!args.contains(&std::ffi::OsStr::new(link.as_str())));
    }

    #[test]
    fn selected_transfer_excludes_unchecked_files_and_keeps_folder_children() {
        let first = json!({"items":[
            {"path":"/episode1.mp4","name":"第一集.mp4","fs_id":"41","is_dir":false,"size":10},
            {"path":"/next","name":"后续课程","fs_id":"42","is_dir":true,"size":0}
        ]});
        let selected = vec![SharePick {
            id: "42".into(),
            name: "后续课程".into(),
        }];
        let result = share_manifest(first.clone(), &|dir, _| {
            assert_eq!(dir, "/next");
            Some(json!({"items":[{"path":"/next/episode2.mp4","name":"第二集.mp4","fs_id":"43","is_dir":false,"size":20}]}))
        }, &selected).unwrap();
        assert_eq!(result.roots, ["42"]);
        assert_eq!(layout_size(&result.layout), Some(20));
        assert_eq!(
            result
                .layout
                .get(Path::new("后续课程").join("第二集.mp4").as_path()),
            Some(&Some(20))
        );
        assert!(!result.layout.contains_key(Path::new("第一集.mp4")));
        let cmd = transfer_command(
            Path::new("bdpan.exe"),
            &share_url("https://pan.baidu.com/s/test").unwrap(),
            "00",
            &result.roots,
        );
        let args = cmd
            .get_args()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>();
        assert_eq!(&args[0..2], ["transfer", "select"]);
        assert_eq!(&args[3..5], ["--fsid", "42"]);
        assert!(normalize_selection(Some(Vec::new())).is_err());
        assert!(share_manifest(
            first,
            &|_, _| None,
            &[SharePick {
                id: "41".into(),
                name: "非所选.mp4".into()
            }]
        )
        .is_none());
    }

    #[test]
    fn share_total_includes_nested_and_paginated_files() {
        let first = json!({"items":[{"path":"/folder","is_dir":true,"size":0}],
            "has_more":true});
        let manifest = share_manifest(
            first,
            &|dir, page| match (dir, page) {
                ("", 2) => Some(json!({"items":[{"path":"/root.zip","is_dir":false,"size":9}]})),
                ("/folder", 1) => {
                    Some(json!({"items":[{"path":"/folder/file.zip","is_dir":false,"size":13}]}))
                }
                _ => None,
            },
            &[],
        );
        let total = manifest.as_ref().and_then(|m| layout_size(&m.layout));
        assert_eq!(total, Some(22));
        let mut job = Job::pending(
            &share_url("https://pan.baidu.com/s/test").unwrap(),
            PathBuf::new(),
            false,
        );
        job.total_bytes = total.unwrap();
        job.bytes = 11;
        job.status = "running".into();
        let task = serde_json::to_value(job.as_task().unwrap()).unwrap();
        assert_eq!(task["meta"]["res"]["size"], 22);
        assert_eq!(task["progress"]["downloaded"], 11);
        job.pending_transfer = Some(PendingTransfer::default());
        let waiting = serde_json::to_value(job.as_task().unwrap()).unwrap();
        assert_eq!(
            waiting["meta"]["req"]["labels"]["baiduWaitingTransfer"],
            "true"
        );
        assert_eq!(waiting["stage"], "等待转存完成…");
        job.downloading_from_cloud = true;
        let downloading = serde_json::to_value(job.as_task().unwrap()).unwrap();
        assert_eq!(
            downloading["meta"]["req"]["labels"]["baiduWaitingTransfer"],
            "false"
        );
        assert!(downloading["stage"].is_null());
    }

    #[test]
    fn selected_folder_preview_reuses_its_directory_lookup() {
        let dir =
            std::env::temp_dir().join(format!("sd-baidu-preview-{:016x}", rand::random::<u64>()));
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("bdpan.cmd");
        fs::write(&bin, concat!(
            "@echo off\r\n",
            "if not \"%~2\"==\"transfer\" exit /b 1\r\n",
            "if not \"%~3\"==\"list\" exit /b 1\r\n",
            "echo page>>\"%~dp0calls\"\r\n",
            "if \"%~6\"==\"\" (\r\n",
            "echo {\"items\":[{\"path\":\"/Tools\",\"name\":\"Tools\",\"is_dir\":true,\"fs_id\":\"21\"},",
            "{\"path\":\"/extra.txt\",\"name\":\"extra.txt\",\"is_dir\":false,\"size\":10,\"fs_id\":\"25\"}]}\r\n",
            "exit /b 0\r\n",
            ")\r\n",
            "if \"%~6\"==\"/Tools\" (\r\n",
            "echo {\"items\":[{\"path\":\"/Tools/work\",\"name\":\"work\",\"is_dir\":true,\"fs_id\":\"22\"}]}\r\n",
            "exit /b 0\r\n",
            ")\r\n",
            "if \"%~6\"==\"/Tools/work\" (\r\n",
            "echo {\"items\":[{\"path\":\"/Tools/work/clip.mov\",\"name\":\"clip.mov\",\"is_dir\":false,\"size\":42,\"fs_id\":\"23\"}]}\r\n",
            "exit /b 0\r\n",
            ")\r\n",
            "exit /b 1\r\n"
        )).unwrap();
        let select = vec![SharePick {
            id: "21".into(),
            name: "Tools".into(),
        }];
        let link = share_url("https://pan.baidu.com/s/test").unwrap();
        let downloads = Downloads {
            file: dir.join("state.json"),
            tool: bin.clone(),
            jobs: Mutex::new(Jobs::default()),
        };
        let first = downloads
            .share_metadata(&bin, &link, &select, &|| true)
            .unwrap()
            .unwrap()
            .manifest
            .unwrap();
        assert_eq!(first.roots, ["21"]);
        assert_eq!(
            first.layout,
            TransferLayout::from([
                (PathBuf::from("Tools"), None),
                (PathBuf::from("Tools").join("work"), None),
                (
                    PathBuf::from("Tools").join("work").join("clip.mov"),
                    Some(42)
                ),
            ])
        );
        let cached = downloads
            .share_metadata(&bin, &link, &select, &|| true)
            .unwrap()
            .unwrap()
            .manifest
            .unwrap();
        assert_eq!(cached.layout, first.layout);
        let subset = downloads
            .share_metadata(
                &bin,
                &link,
                &[SharePick {
                    id: "23".into(),
                    name: "clip.mov".into(),
                }],
                &|| true,
            )
            .unwrap()
            .unwrap()
            .manifest
            .unwrap();
        assert_eq!(subset.roots, ["23"]);
        assert_eq!(
            subset.layout,
            TransferLayout::from([(PathBuf::from("clip.mov"), Some(42))])
        );
        assert_eq!(
            fs::read_to_string(dir.join("calls"))
                .unwrap()
                .lines()
                .count(),
            3
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn share_name_uses_metadata_until_a_local_name_is_requested() {
        let link = share_url("https://pan.baidu.com/s/1Example").unwrap();
        let temp =
            std::env::temp_dir().join(format!("shandian-name-{:016x}", rand::random::<u64>()));
        let mut job = Job::pending(&link, temp.clone(), false);
        assert_eq!(job.label(), "百度网盘分享");
        assert_eq!(
            name_from_listing(&json!({"data":{"list":[{"server_filename":"正片.mov"}]}}))
                .as_deref(),
            Some("正片.mov")
        );
        job.suggested_name = name_from_listing(&json!({
            "items": [{"name":"正片.mov","is_dir":false,"size":80}],
            "count": 1,
            "has_more": false
        }));
        assert_eq!(job.label(), "正片.mov");
        job.requested_name = clean_name("自定名称");
        assert_eq!(job.label(), "自定名称");
        let staging = job.staging();
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("正片.mov"), b"data").unwrap();
        let (output, _, original) = commit(&job, &staging).unwrap();
        assert_eq!(output, temp.join("自定名称.mov"));
        assert_eq!(original.as_deref(), Some(std::ffi::OsStr::new("正片.mov")));
        fs::remove_file(output).unwrap();
        fs::remove_dir(temp).unwrap();
    }
}
