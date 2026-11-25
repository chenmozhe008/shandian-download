//! Cache-only cleanup: never traverse a WebView profile or application-data root wholesale.
use std::{path::{Path, PathBuf}, time::{Duration, SystemTime}};
use serde::Serialize;
use tauri::State;
use crate::AppState;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheStats { bytes: u64, files: usize, skipped: usize, pending_browser: bool }

fn ordinary_metadata(path: &Path) -> Option<std::fs::Metadata> {
    use std::os::windows::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).ok()?;
    // Skip junctions and every other reparse point, not only symbolic links.
    (metadata.file_attributes() & 0x400 == 0).then_some(metadata)
}

fn walk(path: &Path, root: &Path, old_only: bool, deleting: bool, stats: &mut CacheStats) {
    let Some(metadata) = ordinary_metadata(path) else { return };
    let Ok(resolved) = path.canonicalize() else { return };
    if !resolved.starts_with(root) || resolved == root { return; }
    for ancestor in path.ancestors().take_while(|p| *p != root) {
        if ordinary_metadata(ancestor).is_none() { return; }
    }
    if metadata.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() { walk(&entry.path(), root, old_only, deleting, stats); }
        }
    } else if metadata.is_file() {
        if old_only && metadata.modified().ok().and_then(|at| SystemTime::now().duration_since(at).ok()).unwrap_or_default() < Duration::from_secs(7 * 86400) { return; }
        if deleting && std::fs::remove_file(path).is_err() { stats.skipped += 1; return; }
        stats.bytes += metadata.len(); stats.files += 1;
    }
}

fn cleanup(local: &Path, active_video: bool, deleting: bool, include_webview: bool) -> CacheStats {
    let mut stats = CacheStats::default();
    let Ok(root) = local.canonicalize() else { return stats };
    let local = root.as_path();
    walk(&crate::migrate::local_path(local, "thumbs"), &root, false, deleting, &mut stats);
    if !active_video { walk(&local.join("cache/process"), &root, false, deleting, &mut stats); }
    let Ok(entries) = std::fs::read_dir(local) else { return stats };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        if include_webview && name.starts_with("EBWebView") && ordinary_metadata(&path).is_some_and(|m| m.is_dir()) {
            for sub in ["Default/Cache", "Default/Code Cache", "Default/GPUCache", "Default/DawnCache", "ShaderCache", "GrShaderCache"] {
                walk(&path.join(sub), &root, false, deleting, &mut stats);
            }
            walk(&path.join("Crashpad/reports"), &root, true, deleting, &mut stats);
        } else if !active_video && name.starts_with("ytdlp-") && name.ends_with(".log") {
            walk(&path, &root, true, deleting, &mut stats);
        }
    }
    stats
}

async fn run(state: State<'_, AppState>, deleting: bool) -> Result<CacheStats, String> {
    let local = state.local.clone();
    let active = state.videos.list().iter().any(|job| matches!(job.state.as_str(), "running" | "wait"));
    tauri::async_runtime::spawn_blocking(move || -> Result<CacheStats, String> {
        if !deleting { return Ok(cleanup(&local, active, false, true)); }
        let total = cleanup(&local, active, false, true).bytes;
        let other = cleanup(&local, active, false, false).bytes;
        let mut result = cleanup(&local, active, true, false);
        if total > other {
            std::fs::create_dir_all(local.join("cache")).map_err(|e| e.to_string())?;
            std::fs::write(local.join("cache/clear-webview-on-start"), b"1").map_err(|e| e.to_string())?;
            result.pending_browser = true;
        }
        crate::app_log::record("cache", &format!("freed_bytes={} files={} skipped={} pending_browser={}", result.bytes, result.files, result.skipped, result.pending_browser));
        Ok(result)
    }).await.map_err(|e| e.to_string())?
}

/// Called after single-instance admission, before constructing any WebView.
pub fn apply_pending(local: &Path) {
    let marker = local.join("cache/clear-webview-on-start");
    if !marker.is_file() { return; }
    let result = cleanup(local, true, true, true);
    if result.skipped == 0 { let _ = std::fs::remove_file(marker); }
    crate::app_log::record("cache", &format!("startup_cleanup freed_bytes={} skipped={}", result.bytes, result.skipped));
}
#[tauri::command]
pub async fn cache_info(state: State<'_, AppState>) -> Result<CacheStats, String> { run(state, false).await }
#[tauri::command]
pub async fn clear_cache(state: State<'_, AppState>) -> Result<CacheStats, String> { run(state, true).await }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_only_removes_allowlisted_cache_and_preserves_data_tools_and_current_logs() {
        let root = std::env::temp_dir().join(format!("shandian-cache-test-{:x}", rand::random::<u64>()));
        let keep = ["auth/pikpak.json", "auth/nas.json", "auth/cookies-youtube.txt", "state/pikpak-jobs.json", "state/resume-on-launch.json", "settings.json", "videos.json", "engine/gopeed.db", "bin/ffmpeg.exe", "EBWebView/Default/Cookies", "EBWebView/Default/Local Storage/settings", "logs/app.log", "EBWebView/Crashpad/reports/recent.dmp"];
        let remove = ["cache/thumbs/a.jpg", "cache/process/ytdlp-test.log", "EBWebView/Default/Cache/data", "EBWebView.small/Default/GPUCache/data"];
        for name in keep.iter().chain(remove.iter()) { let file = root.join(name); std::fs::create_dir_all(file.parent().unwrap()).unwrap(); std::fs::write(file, b"test").unwrap(); }
        let before = cleanup(&root, false, false, true); assert_eq!(before.files, remove.len());
        let result = cleanup(&root, false, true, true); assert_eq!(result.files, remove.len());
        for name in keep { assert!(root.join(name).exists(), "{name}"); }
        for name in remove { assert!(!root.join(name).exists(), "{name}"); }
        // This test owns this random directory exclusively; all paths remain below it.
        assert!(root.file_name().unwrap().to_string_lossy().starts_with("shandian-cache-test-"));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn active_process_output_is_preserved() {
        let root = std::env::temp_dir().join(format!("shandian-cache-test-{:x}", rand::random::<u64>()));
        std::fs::create_dir_all(root.join("cache/process")).unwrap();
        std::fs::write(root.join("cache/process/ytdlp-test.log"), b"progress").unwrap();
        assert_eq!(cleanup(&root, true, true, true).files, 0);
        std::fs::remove_file(root.join("cache/process/ytdlp-test.log")).unwrap();
        std::fs::remove_dir(root.join("cache/process")).unwrap(); std::fs::remove_dir(root.join("cache")).unwrap(); std::fs::remove_dir(root).unwrap();
    }
}
