// 回收站：移除的任务留一条记录，能重新下载、找到文件（对齐 Mac 版 DownloadStore.removedRecords）。
// 和系统回收站是两回事——这里存的是「任务档案」；文件是否真删由移除时的勾选决定。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovedRecord {
    pub id: String,
    pub title: String,
    pub origin: String,
    pub link: String,
    #[serde(default)]
    pub path: Option<String>,
    pub size: u64,
    /// 移除时刻（毫秒）
    pub removed_at: u64,
    /// 移除时是否把文件删了/进了系统回收站
    pub files_trashed: bool,
    /// 原任务 id：缩略图缓存还在 thumbs/<id>.jpg，回收站封面靠它复用
    #[serde(default)]
    pub task_id: Option<String>,
    /// 多文件任务的文件夹、位置和文件列表
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi: Option<serde_json::Value>,
    /// 分辨率和字幕标签：文件没了也认得出是哪个版本。老记录没有
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subs: Option<String>,
    /// 文件现在的状况，每次读列表时摸盘算出来：here / gone / offline（盘没插）/ unknown（没记路径）。
    /// 不落盘——移除之后用户可能手动删了或挪了，存下来的只会过期
    #[serde(default, skip_deserializing)]
    pub file_state: String,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn file_of(base: &Path) -> PathBuf {
    base.join("removed.json")
}

fn load(base: &Path) -> Vec<RemovedRecord> {
    std::fs::read_to_string(file_of(base))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(base: &Path, list: &[RemovedRecord]) {
    if let Ok(s) = serde_json::to_string_pretty(list) {
        let _ = std::fs::write(file_of(base), s);
    }
}

/// 移除任务时记一条。pending 那种还没下起来的不用记（Mac 同）。
pub fn record(
    state: &AppState,
    title: &str,
    origin: &str,
    link: &str,
    path: Option<String>,
    size: u64,
    files_trashed: bool,
    task_id: Option<String>,
    resolution: Option<String>,
    subs: Option<String>,
) {
    let mut list = load(&state.base);
    list.insert(
        0,
        RemovedRecord {
            id: format!("r{}", now_ms()),
            title: title.to_string(),
            origin: origin.to_string(),
            link: link.to_string(),
            path,
            size,
            removed_at: now_ms(),
            files_trashed,
            task_id,
            multi: None,
            resolution,
            subs,
            file_state: String::new(),
        },
    );
    if list.len() > 500 {
        for r in list.drain(500..) {
            drop_thumb(&state.local, &r);
        }
    }
    save(&state.base, &list);
}

/// 记录没了封面也就没人用了（任务本身在进历史时已经从列表删掉）
fn drop_thumb(local: &Path, r: &RemovedRecord) {
    if let Some(id) = r.task_id.as_deref().filter(|id| !id.is_empty()) {
        crate::thumbs::remove(local, id);
    }
}

/// 删掉符合条件的记录和它们的封面
fn remove_records(base: &Path, local: &Path, hit: impl Fn(&RemovedRecord) -> bool) {
    let (gone, keep): (Vec<_>, Vec<_>) = load(base).into_iter().partition(|r| hit(r));
    for r in &gone {
        drop_thumb(local, r);
    }
    save(base, &keep);
}

/// 把文件/文件夹移到系统回收站（删除任务时「连文件一起删」只走这一条路，绝不直接抹掉）。
/// 文件可能刚被下载进程放开，占用时重试几次。没有回收站的盘（U 盘、网络盘）
/// 由系统弹窗提醒会永久删除，用户可以取消。
pub fn recycle(paths: &[PathBuf]) -> Result<(), String> {
    // 最后一道闸：上游算错路径时，宁可报错也不能把整个文件夹送走
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    if let Some(p) = paths.iter().find(|p| is_protected(p, home.as_deref())) {
        return Err(format!("「{}」是系统或用户目录，不会删除", p.display()));
    }
    let mut left: Vec<PathBuf> = paths.iter().filter(|p| p.exists()).cloned().collect();
    let mut last = String::new();
    for attempt in 0..4 {
        if left.is_empty() {
            return Ok(());
        }
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(600));
        }
        match recycle_once(&left) {
            Ok(()) => {}
            Err(e) => last = e,
        }
        left.retain(|p| p.exists());
    }
    if left.is_empty() {
        return Ok(());
    }
    let first = left[0].file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    Err(format!("「{first}」等 {} 个文件移不到回收站（可能被占用）{last}", left.len()))
}

/// 文件和它带的字幕。只有 yt-dlp 的命名（标题 [视频id].mp4）才认旁边的文件：
/// 普通文件 Movie.mkv 要是按前缀匹配，会把 Movie.2024.1080p.mkv 这种别的文件一起删掉
fn with_sidecars(file: &Path) -> Vec<PathBuf> {
    let mut out = vec![file.to_path_buf()];
    let stem = file.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let ends_with_id = stem.ends_with(']')
        || stem
            .rsplit_once("]_")
            .is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()));
    let (true, Some(dir)) = (ends_with_id, file.parent()) else { return out };
    let prefix = format!("{stem}.");
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.starts_with(&prefix) && p != file && p.is_file() {
                out.push(p);
            }
        }
    }
    out
}

/// 盘符根、用户目录本身及其上级、用户目录下的直属文件夹（下载、桌面、文档…）
fn is_protected(p: &Path, home: Option<&Path>) -> bool {
    let norm = |x: &Path| x.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    if p.as_os_str().is_empty() || p.parent().is_none() || !p.is_absolute() {
        return true;
    }
    let Some(home) = home else { return false };
    let (p_s, h_s) = (norm(p), norm(home));
    h_s == p_s
        || h_s.starts_with(&format!("{p_s}\\"))
        || p.parent().is_some_and(|d| norm(d) == h_s)
}

#[cfg(windows)]
fn recycle_once(paths: &[PathBuf]) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
        FOF_WANTNUKEWARNING, FO_DELETE, SHFILEOPSTRUCTW,
    };
    // pFrom 是「路径\0路径\0\0」的格式
    let mut from: Vec<u16> = Vec::new();
    for p in paths {
        from.extend(p.as_os_str().encode_wide());
        from.push(0);
    }
    from.push(0);
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = from.as_ptr();
    op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT | FOF_WANTNUKEWARNING) as _;
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 {
        return Err(format!("（错误码 {code}）"));
    }
    if op.fAnyOperationsAborted != 0 {
        return Err("（已取消）".into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn recycle_once(_paths: &[PathBuf]) -> Result<(), String> {
    Err("当前系统不支持".into())
}

/// 文件其实没删掉（移不到回收站）：记录改回来，下载历史才不会写「已删除」
pub fn mark_files_kept(base: &Path, task_id: &str) {
    let mut list = load(base);
    if let Some(r) = list.iter_mut().find(|r| r.task_id.as_deref() == Some(task_id)) {
        r.files_trashed = false;
    }
    save(base, &list);
}

/// 这个链接在回收站记录里、文件还在盘上的那一条（重复下载提示用）
pub fn find_existing(base: &Path, key: &str) -> Option<RemovedRecord> {
    load(base).into_iter().find(|r| {
        crate::links::canonical_url(&r.link) == key
            && !r.files_trashed
            && r.path.as_deref().is_some_and(|p| Path::new(p).exists())
    })
}

/// 这个链接在下载历史里的任意一条（文件在不在都算）
pub fn find_any(base: &Path, key: &str) -> Option<RemovedRecord> {
    load(base).into_iter().find(|r| crate::links::canonical_url(&r.link) == key)
}

/// 回收站封面：缩略图还在 thumbs/<原任务 id>.jpg，按 task_id 读缓存。
/// 没有缓存就返回 None（前端画黄底占位）。
#[tauri::command]
pub fn trash_thumb(task_id: String, state: State<'_, AppState>) -> Option<String> {
    crate::thumbs::cached(&state.local, &task_id)
        .or_else(|| crate::thumbs::cached(&state.local, &crate::thumbs::frame_id(&task_id)))
}

#[tauri::command]
pub async fn trash_list(check_files: Option<bool>, state: State<'_, AppState>) -> Result<Vec<RemovedRecord>, String> {
    let base = state.base.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let content = match std::fs::read_to_string(file_of(&base)) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("读取下载历史失败：{error}")),
        };
        let mut list: Vec<RemovedRecord> = serde_json::from_str(content.trim_start_matches('\u{feff}'))
            .map_err(|error| format!("下载历史格式有误：{error}"))?;
        if check_files.unwrap_or(true) {
            set_history_file_state(&mut list);
        } else {
            for record in &mut list {
                record.file_state = if record.files_trashed { "gone" } else { "unknown" }.into();
            }
        }
        Ok(list)
    }).await.map_err(|e| format!("读取下载历史失败：{e}"))?
}

fn set_history_file_state(list: &mut [RemovedRecord]) {
    for r in list {
        // 来源是删除时存下的；后来新认识的来源（GitHub、Dropbox 这些）按链接重新标一遍
        if r.origin == "直链" {
            if let Some(n) = crate::api::netdisk_label(&r.link) {
                r.origin = n.to_string();
            }
        }
        r.file_state = match r.path.as_deref().filter(|p| !p.is_empty()) {
            None => "unknown",
            Some(p) => {
                let p = Path::new(p);
                if crate::engine::resolve_existing(p).is_some() {
                    "here"
                } else if crate::engine::file_missing(p) {
                    "gone"
                } else {
                    "offline"
                }
            }
        }
        .to_string();
    }
}

/// 从回收站删掉这一条记录（不动文件）
#[tauri::command]
/// 按链接把历史里的旧记录抹掉。内核删任务时会顺手写一条 removed 记录（那就是我们的
/// 下载历史），「重新下载」之后这条旧记录必须一起清，否则历史里会多一条重复的
pub fn forget_link(base: &Path, link: &str) {
    let link = link.trim();
    if link.is_empty() {
        return;
    }
    let mut list = load(base);
    let before = list.len();
    list.retain(|r| r.link.trim() != link);
    if list.len() != before {
        save(base, &list);
    }
}

#[tauri::command(async)]
pub fn trash_delete(id: String, also_files: Option<bool>, state: State<'_, AppState>) -> Result<(), String> {
    // 用户勾了「同时删除文件」：文件移到系统回收站（能还原），失败就不删记录，免得找不到文件在哪
    if also_files == Some(true) {
        if let Some(r) = load(&state.base).into_iter().find(|r| r.id == id) {
            let targets = record_files(&r);
            if !targets.is_empty() {
                recycle(&targets)?;
            }
        }
    }
    // 封面只有这条记录在用，一起清掉
    remove_records(&state.base, &state.local, |r| r.id == id);
    Ok(())
}

/// 记录对应的磁盘内容：多文件任务是整个任务文件夹；文件在同名的资源文件夹里（BT）时是那个文件夹；否则是文件和它的字幕等附带文件
fn record_files(r: &RemovedRecord) -> Vec<PathBuf> {
    if let Some(m) = &r.multi {
        let (dir, folder) = (m["dir"].as_str().unwrap_or_default(), m["folder"].as_str().unwrap_or_default());
        let root = PathBuf::from(dir).join(folder);
        return if !dir.is_empty() && !folder.is_empty() && root.exists() { vec![root] } else { Vec::new() };
    }
    let Some(p) = r.path.as_deref().filter(|p| !p.trim().is_empty()).map(PathBuf::from) else { return Vec::new() };
    if let Some(parent) = p.parent().filter(|d| d.file_name().is_some_and(|n| n.to_string_lossy() == r.title)) {
        return if parent.exists() { vec![parent.to_path_buf()] } else { Vec::new() };
    }
    if p.exists() { with_sidecars(&p) } else { Vec::new() }
}

/// 清空回收站（清记录和封面，已下载的文件不受影响——和 Mac 弹窗文案一致）
#[tauri::command]
pub fn trash_clear(state: State<'_, AppState>) -> Result<(), String> {
    remove_records(&state.base, &state.local, |_| true);
    Ok(())
}

/// 重新下载：把链接再投一次，并从回收站拿掉这条（Mac redownload 同款）
#[tauri::command]
pub async fn trash_redownload(
    id: String,
    dir: Option<String>,
    video: Option<crate::videos::VideoOptions>,
    remember_video: Option<bool>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let list = load(&state.base);
    let Some(r) = list.iter().find(|r| r.id == id).cloned() else {
        return Err("回收站里没有这条记录".into());
    };
    if r.link.trim().is_empty() {
        return Err("这条记录没有保存链接，无法重新下载".into());
    }
    if let Some(m) = &r.multi {
        let new_id = crate::engine::multi_redownload(m, dir.as_deref().unwrap_or(""), &state).await?;
        remove_records(&state.base, &state.local, |x| x.id == id);
        return Ok(new_id);
    }
    if r.link.starts_with("shandian-multi:") {
        return Err("这条记录是早期版本留下的，没有保存文件列表，请到网盘重新推送".into());
    }
    // 从历史里点「重新下载」= 覆盖原来那份，不是再存一份：旧文件先送回收站
    //（不删的话 yt-dlp 看到同名文件会直接跳过），下到原来的目录，
    // 记录随之移除，之后按新任务算。投成功了才拿掉记录，失败时记录还在
    let mut old_dir = String::new();
    // BT 记的路径是资源文件夹里的一个文件：它的上级是资源文件夹，不是保存目录，当目录用会
    // 下成「资源/资源/…」。文件也不动，内核校验已有内容，缺什么补什么
    let is_bt = matches!(
        crate::links::LinkKind::of(&r.link),
        Some(crate::links::LinkKind::Magnet | crate::links::LinkKind::TorrentFile)
    );
    if let Some(path) = r.path.as_deref().filter(|p| !p.trim().is_empty() && !is_bt) {
        let p = PathBuf::from(path);
        // 只认文件。界面已经问过用户了（见 app.js trashAction）
        if p.is_file() {
            let result = recycle(&with_sidecars(&p));
            if remember_video == Some(false) {
                result?;
            }
        }
        if let Some(parent) = p.parent() {
            old_dir = parent.to_string_lossy().to_string();
        }
    }
    // 确认框里改过保存位置就听它的；选过的画质、字幕存下来，投任务时会读
    let dir = dir.filter(|d| !d.trim().is_empty()).unwrap_or(old_dir);
    let new_id = crate::engine::add_link_overwrite(
        &r.link, &state, &dir, video, remember_video.unwrap_or(true),
    ).await?;
    // 同名待用户选择，任务还没建：记录先留着，真建了再清，用户关掉弹窗就什么都没变
    if new_id.is_empty() {
        crate::engine::history_waits_for_choice(&r.link, &id);
        return Ok(new_id);
    }
    // 新任务有自己的 id 和封面，旧记录的那张一起清掉
    remove_records(&state.base, &state.local, |x| x.id == id);
    Ok(new_id)
}

pub fn attach_multi(state: &AppState, task_id: &str, multi: serde_json::Value) {
    let mut list = load(&state.base);
    if let Some(r) = list.iter_mut().find(|r| r.task_id.as_deref() == Some(task_id)) {
        r.multi = Some(multi);
        save(&state.base, &list);
    }
}

/// 从历史重下、同名选择完才真正建好任务时清记录
pub fn forget_record(state: &AppState, id: &str) {
    remove_records(&state.base, &state.local, |x| x.id == id);
}

/// 打开文件所在文件夹（回收站行上的「打开位置」）
#[tauri::command]
pub fn trash_open_path(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    let _ = p;
    crate::commands::reveal_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, task: Option<&str>) -> RemovedRecord {
        RemovedRecord {
            multi: None,
            id: id.into(),
            title: String::new(),
            origin: String::new(),
            link: String::new(),
            path: None,
            size: 0,
            removed_at: 0,
            files_trashed: false,
            task_id: task.map(String::from),
            resolution: None,
            subs: None,
            file_state: String::new(),
        }
    }

    #[test]
    fn sidecars_only_for_ytdlp_names() {
        let d = std::env::temp_dir().join(format!("sd-side-{}", now_ms()));
        std::fs::create_dir_all(&d).unwrap();
        for n in ["T [a1].mp4", "T [a1].zh-en.ass", "T [a1].f137.mp4.part", "T [a1]_1.mp4", "T [a1]_1.zh.ass", "T [b2].mp4", "Movie.mkv", "Movie.2024.1080p.mkv", "Movie.srt"] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        let names = |f: &str| {
            let mut v: Vec<String> = with_sidecars(&d.join(f))
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
                .collect();
            v.sort();
            v
        };
        assert_eq!(names("T [a1].mp4"), ["T [a1].f137.mp4.part", "T [a1].mp4", "T [a1].zh-en.ass"]);
        assert_eq!(names("T [a1]_1.mp4"), ["T [a1]_1.mp4", "T [a1]_1.zh.ass"]);
        // 普通文件只有它自己
        assert_eq!(names("Movie.mkv"), ["Movie.mkv"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn protected_paths_are_refused() {
        let home = Path::new(r"C:\Users\me");
        for p in [r"C:\", r"U:\", r"C:\Users", r"C:\Users\me", r"C:\Users\ME\", r"C:\Users\me\Downloads", r"C:\Users\me\Desktop", "", "relative\\x"] {
            assert!(is_protected(Path::new(p), Some(home)), "{p} 应该被拒绝");
        }
        for p in [r"C:\Users\me\Downloads\a.mp4", r"C:\Users\me\Downloads\电影合集", r"D:\movies\a.mkv"] {
            assert!(!is_protected(Path::new(p), Some(home)), "{p} 应该放行");
        }
    }

    #[test]
    fn removing_records_cleans_only_their_thumbs() {
        let base = std::env::temp_dir().join(format!("sd-trash-{}", now_ms()));
        std::fs::create_dir_all(base.join("thumbs")).unwrap();
        let thumb = |n: &str| base.join("thumbs").join(n);
        for n in ["a.jpg", "a.frame.jpg", "b.jpg", "live.jpg"] {
            std::fs::write(thumb(n), b"x").unwrap();
        }
        save(&base, &[rec("r1", Some("a")), rec("r2", Some("b")), rec("r3", None)]);

        // 删一条：只动它自己的原封面和截帧
        remove_records(&base, &base, |r| r.id == "r1");
        assert!(!thumb("a.jpg").exists() && !thumb("a.frame.jpg").exists());
        assert!(thumb("b.jpg").exists());
        assert_eq!(load(&base).len(), 2);

        // 清空：记录全没了，列表里还在用的任务封面不受影响
        remove_records(&base, &base, |_| true);
        assert!(!thumb("b.jpg").exists());
        assert!(thumb("live.jpg").exists());
        assert!(load(&base).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }
}
