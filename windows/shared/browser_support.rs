use std::path::{Path, PathBuf};
use serde_json::Value;
use winreg::{enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE}, RegKey};

pub fn browser_exe(key: &str) -> Option<PathBuf> {
    let exe = match key { "edge" => "msedge.exe", "chrome" => "chrome.exe", _ => return None };
    let sub = format!(r"Software\Microsoft\Windows\CurrentVersion\App Paths\{exe}");
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().find_map(|root| {
        let value: String = RegKey::predef(root).open_subkey(&sub).ok()?.get_value("").ok()?;
        let path = PathBuf::from(value.trim_matches('"'));
        path.is_file().then_some(path)
    })
}

pub fn browser_order() -> [&'static str; 2] {
    let choice = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice")
        .and_then(|key| key.get_value::<String, _>("ProgId")).unwrap_or_default().to_lowercase();
    if choice.contains("edge") { ["edge", "chrome"] } else { ["chrome", "edge"] }
}

fn json_file(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn extension_enabled(value: &Value) -> bool {
    if value["state"].as_u64().is_some_and(|s| s != 1) { return false; }
    if value["blacklist_state"].as_u64().unwrap_or(0) != 0 || value["blocklist_state"].as_u64().unwrap_or(0) != 0 { return false; }
    match value.get("disable_reasons") {
        Some(Value::Array(reasons)) => reasons.is_empty(),
        Some(Value::Number(n)) => n.as_u64() == Some(0),
        // Current Chromium removes the empty disable-reasons preference entirely.
        // Still require an installed extension record with a path (checked again by the caller).
        None => value["state"].as_u64() == Some(1) || (value["path"].is_string() && value["location"].as_u64().is_some()),
        _ => false,
    }
}

/// Read only extension enablement metadata, not browser cookies or login data.
#[derive(Clone)]
pub struct ExtensionProfile {
    pub browser: &'static str,
    pub profile: String,
    files: Vec<(PathBuf, Option<(u64, std::time::SystemTime)>)>,
}

fn file_stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    meta.is_file().then_some((meta.len(), meta.modified().ok()?))
}

impl ExtensionProfile {
    // Preferences change when an extension is disabled/uninstalled; an unpacked directory alone
    // is insufficient evidence that it is still installed. No JSON parsing on the unchanged path.
    pub fn is_current(&self) -> bool {
        browser_exe(self.browser).is_some() && self.files_current()
    }
    fn files_current(&self) -> bool {
        !self.files.is_empty() && self.files.iter().all(|(path, stamp)| file_stamp(path) == *stamp)
    }
    pub fn refresh(&self, extension_id: &str) -> Option<Self> {
        if self.is_current() { return Some(self.clone()); }
        remembered_profile(self.browser, &self.profile, extension_id)
    }
}

fn profile_extension(browser: &'static str, root: &Path, profile: &str, extension_id: &str, state_stamp: Option<(u64, std::time::SystemTime)>) -> Option<ExtensionProfile> {
    if profile.is_empty() || !profile.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-')) { return None; }
    let dir = root.join(profile);
    let mut files = vec![(root.join("Local State"), state_stamp)];
    for name in ["Secure Preferences", "Preferences"] {
        let path = dir.join(name);
        files.push((path.clone(), file_stamp(&path)));
    }
    for name in ["Secure Preferences", "Preferences"] {
        let Some(prefs) = json_file(&dir.join(name)) else { continue; };
        let Some(entry) = prefs["extensions"]["settings"].get(extension_id) else { continue; };
        if !extension_enabled(entry) { return None; }
        let path = PathBuf::from(entry["path"].as_str()?);
        let extension_dir = if path.is_absolute() { path } else { dir.join("Extensions").join(path) };
        let manifest = extension_dir.join("manifest.json");
        if !manifest.is_file() { return None; }
        files.push((manifest.clone(), file_stamp(&manifest)));
        return Some(ExtensionProfile { browser, profile: profile.into(), files });
    }
    None
}

pub fn remembered_profile(browser: &str, profile: &str, extension_id: &str) -> Option<ExtensionProfile> {
    let (browser, relative) = match browser { "chrome" => ("chrome", "Google/Chrome/User Data"), "edge" => ("edge", "Microsoft/Edge/User Data"), _ => return None };
    browser_exe(browser)?;
    let root = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(relative);
    let state_path = root.join("Local State");
    let stamp = file_stamp(&state_path);
    let state = json_file(&state_path)?;
    state["profile"]["info_cache"].get(profile)?;
    profile_extension(browser, &root, profile, extension_id, stamp)
}

pub fn extension_profile(browser: &'static str, extension_id: &str) -> Option<ExtensionProfile> {
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    let root = local.join(match browser { "chrome" => "Google/Chrome/User Data", "edge" => "Microsoft/Edge/User Data", _ => return None });
    let local_state = root.join("Local State");
    let state_stamp = file_stamp(&local_state);
    let state = json_file(&local_state)?;
    let mut profiles: Vec<String> = state["profile"]["info_cache"].as_object()?.keys().cloned().collect();
    let last = state["profile"]["last_used"].as_str().unwrap_or("Default");
    profiles.sort_by_key(|p| if p == last { 0 } else if p == "Default" { 1 } else { 2 });
    profiles.into_iter().find_map(|profile| profile_extension(browser, &root, &profile, extension_id, state_stamp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn supports_current_and_legacy_enablement_without_accepting_disabled_extensions() {
        assert!(extension_enabled(&json!({"state":1})));
        assert!(extension_enabled(&json!({"disable_reasons":[]})));
        assert!(extension_enabled(&json!({"state":1,"disable_reasons":0})));
        assert!(extension_enabled(&json!({"path":"installed-extension","location":4})));
        for value in [json!({}), json!({"state":0,"disable_reasons":[]}), json!({"disable_reasons":[1]}), json!({"disable_reasons":4}), json!({"state":1,"blacklist_state":1})] { assert!(!extension_enabled(&value)); }
    }
    #[test]
    fn remembered_extension_invalidates_when_preferences_or_manifest_change() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("shandian-extension-cache-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let prefs = root.join("Secure Preferences");
        let manifest = root.join("manifest.json");
        std::fs::write(&prefs, b"enabled").unwrap(); std::fs::write(&manifest, b"{}").unwrap();
        let mut target = ExtensionProfile { browser:"edge", profile:"Default".into(), files:vec![(prefs.clone(), file_stamp(&prefs)),(manifest.clone(), file_stamp(&manifest))] };
        assert!(target.files_current());
        std::fs::write(&prefs,b"extension removed or disabled").unwrap();
        assert!(!target.files_current());
        target.files[0].1 = file_stamp(&prefs);
        assert!(target.files_current());
        std::fs::remove_file(&manifest).unwrap(); assert!(!target.files_current());
        std::fs::remove_file(&prefs).unwrap(); std::fs::remove_dir(&root).unwrap();
    }

}
