use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

const APP_PATH: &str = "/apps/bdpan/";
const VISIBLE_PATH: &str = "我的应用数据/bdpan/";
const PAGE_SIZE: usize = 100;
const REPARSE_POINT: u32 = 0x400;

pub fn task_directory(id: &str) -> String {
    format!("Shandian/{id}")
}

fn valid_task_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn matches_receipt(id: &str, path: &str) -> bool {
    if !valid_task_id(id) {
        return false;
    }
    let path = path.trim_end_matches('/');
    path.strip_prefix(VISIBLE_PATH)
        .or_else(|| path.strip_prefix(APP_PATH))
        .unwrap_or(path)
        == task_directory(id)
}

#[derive(Debug, PartialEq, Eq)]
struct FileProof {
    size: u64,
    md5: Option<String>,
}

type Tree = BTreeMap<PathBuf, Option<FileProof>>;
pub(crate) type TransferLayout = BTreeMap<PathBuf, Option<u64>>;

fn directory_missing(result: &Value) -> bool {
    result["code"].as_i64().is_some_and(|code| code != 0)
        && (result["errno"].as_i64() == Some(-9)
            || result["error"].as_str().is_some_and(|error| {
                error.contains("（错误码 -9）") || error.contains("errno=-9,")
            }))
}

pub(crate) fn transfer_cleared(bin: &Path, id: &str, active: &impl Fn() -> bool) -> bool {
    valid_task_id(id)
        && command_json(bin, &["ls", &task_directory(id), "--limit", "1"], active)
            .is_some_and(|result| directory_missing(&result))
}

pub(crate) fn transfer_layout(
    bin: &Path,
    id: &str,
    active: &impl Fn() -> bool,
) -> Option<TransferLayout> {
    if !valid_task_id(id) {
        return None;
    }
    remote_tree(&task_directory(id), &|args| command_json(bin, args, active)).map(|tree| {
        tree.into_iter()
            .map(|(path, proof)| (path, proof.map(|file| file.size)))
            .collect()
    })
}

#[derive(Debug, PartialEq)]
pub enum CheckFailure {
    Unconfirmed,
    Different,
}

pub struct VerifiedTransfer {
    directory: String,
    tree: Tree,
    local: LocalProof,
}

#[derive(Default)]
struct LocalProof {
    write_times: BTreeMap<PathBuf, u64>,
    mismatches: BTreeMap<PathBuf, String>,
}

pub(crate) fn command_json(bin: &Path, args: &[&str], active: &impl Fn() -> bool) -> Option<Value> {
    command_json_with_progress(bin, args, active, || 0)
}

fn command_json_with_progress(
    bin: &Path,
    args: &[&str],
    active: &impl Fn() -> bool,
    progress: impl Fn() -> u64,
) -> Option<Value> {
    if !active() {
        return None;
    }
    let mut child = crate::baidu::cli(bin)
        .args(args)
        .arg("--json")
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || crate::baidu::read_response(stdout));
    let mut last_activity = Instant::now();
    let mut last_bytes = 0;
    let status = loop {
        let bytes = progress();
        if bytes > last_bytes {
            last_bytes = bytes;
            last_activity = Instant::now();
        }
        if !active() || last_activity.elapsed() >= Duration::from_secs(45) {
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break None,
        }
    };
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let output = reader.join().ok()?;
    if !status?.success() {
        return None;
    }
    serde_json::from_slice(&output).ok()
}

fn list_page(value: &Value) -> Option<&Vec<Value>> {
    if let Some(entries) = value.as_array() {
        return Some(entries);
    }
    if value["code"].as_i64() != Some(0) {
        return None;
    }
    let data = &value["data"];
    data.as_array().or_else(|| data["list"].as_array())
}

pub fn list_directory(bin: &Path, dir: &str, active: &impl Fn() -> bool) -> Option<Value> {
    let result = command_json(bin, &["ls", dir, "--limit", "2"], active)?;
    let path = format!("{APP_PATH}{dir}");
    let files = list_page(&result)?;
    // bdpan 3.8.9 returns the directory itself when it has no entries.
    let entries: Vec<_> = files
        .iter()
        .filter(|entry| entry["path"].as_str() != Some(path.as_str()))
        .cloned()
        .collect();
    Some(Value::Array(entries))
}

// The directory listing can return Baidu's encoded form of the content MD5.
// Normalize it before verifying the file; malformed hashes still prevent cleanup.
fn content_md5(value: &str) -> Option<String> {
    let bytes: &[u8; 32] = value.as_bytes().try_into().ok()?;
    if bytes.iter().all(u8::is_ascii_hexdigit) {
        return Some(value.to_ascii_lowercase());
    }

    let mut digest = [0; 32];
    for (index, byte) in bytes.iter().copied().enumerate() {
        let digit = if index == 9 {
            byte.checked_sub(b'g').filter(|digit| *digit < 16)?
        } else {
            char::from(byte).to_digit(16)? as u8
        };
        digest[index ^ 8] = b"0123456789abcdef"[usize::from(digit ^ (index as u8 & 15))];
    }
    String::from_utf8(digest.to_vec()).ok()
}

fn remote_tree(directory: &str, request: &impl Fn(&[&str]) -> Option<Value>) -> Option<Tree> {
    let mut tree = Tree::new();
    let mut folders = vec![(directory.to_string(), PathBuf::new())];
    while let Some((remote, relative)) = folders.pop() {
        let parent = format!("{APP_PATH}{remote}/");
        let mut start = 0usize;
        loop {
            let value = request(&[
                "ls",
                &remote,
                "--limit",
                &PAGE_SIZE.to_string(),
                "--start",
                &start.to_string(),
            ])?;
            let page = list_page(&value)?;
            for entry in page {
                let path = entry["path"].as_str()?.trim_end_matches('/');
                let is_directory = entry["isdir"].as_bool()?;
                if is_directory && path == parent.trim_end_matches('/') {
                    continue;
                }
                let name = path.strip_prefix(&parent)?;
                if name.is_empty()
                    || name.contains(['/', '\\', ':'])
                    || !matches!(
                        Path::new(name).components().collect::<Vec<_>>().as_slice(),
                        [Component::Normal(_)]
                    )
                {
                    return None;
                }
                let target = relative.join(name);
                let proof = if is_directory {
                    folders.push((format!("{remote}/{name}"), target.clone()));
                    None
                } else {
                    Some(FileProof {
                        size: entry["size"].as_u64()?,
                        md5: entry["md5"].as_str().and_then(content_md5),
                    })
                };
                if tree.insert(target, proof).is_some() {
                    return None;
                }
            }
            if page.len() < PAGE_SIZE {
                break;
            }
            start = start.checked_add(page.len())?;
        }
    }
    tree.values().any(Option::is_some).then_some(tree)
}

fn saved_path(
    output: &Path,
    source_name: Option<&std::ffi::OsStr>,
    relative: &Path,
) -> Option<PathBuf> {
    let relative = match source_name {
        Some(name) => relative.strip_prefix(name).ok()?,
        None => relative,
    };
    Some(if relative.as_os_str().is_empty() {
        output.to_path_buf()
    } else {
        output.join(relative)
    })
}

fn check_saved_tree(
    output: &Path,
    source_name: Option<&std::ffi::OsStr>,
    remote: &Tree,
    active: &impl Fn() -> bool,
) -> Result<LocalProof, CheckFailure> {
    let root = fs::symlink_metadata(output).map_err(|_| CheckFailure::Different)?;
    if root.file_attributes() & REPARSE_POINT != 0 {
        return Err(CheckFailure::Different);
    }
    let mut local = LocalProof::default();
    for (relative, expected) in remote {
        if !active() {
            return Err(CheckFailure::Unconfirmed);
        }
        let path = saved_path(output, source_name, relative).ok_or(CheckFailure::Different)?;
        let metadata = fs::symlink_metadata(&path).map_err(|_| CheckFailure::Different)?;
        if metadata.file_attributes() & REPARSE_POINT != 0 {
            return Err(CheckFailure::Different);
        }
        match expected {
            None if metadata.is_dir() => {}
            Some(proof) if metadata.is_file() && metadata.len() == proof.size => {
                let checksum = md5_file(&path, active).map_err(|_| CheckFailure::Unconfirmed)?;
                if proof.md5.as_deref() != Some(checksum.as_str()) {
                    local.mismatches.insert(relative.clone(), checksum);
                }
                let unchanged = fs::symlink_metadata(&path).is_ok_and(|after| {
                    after.is_file()
                        && after.file_attributes() & REPARSE_POINT == 0
                        && after.len() == metadata.len()
                        && after.last_write_time() == metadata.last_write_time()
                });
                if !unchanged {
                    return Err(CheckFailure::Different);
                }
                local
                    .write_times
                    .insert(relative.clone(), metadata.last_write_time());
            }
            _ => return Err(CheckFailure::Different),
        }
    }
    Ok(local)
}

pub fn verify(
    bin: &Path,
    id: &str,
    output: &Path,
    source_name: Option<&std::ffi::OsStr>,
    expected: &TransferLayout,
    active: impl Fn() -> bool,
) -> Result<VerifiedTransfer, CheckFailure> {
    if !valid_task_id(id) {
        return Err(CheckFailure::Unconfirmed);
    }
    let directory = task_directory(id);
    let request = |args: &[&str]| command_json(bin, args, &active);
    let tree = remote_tree(&directory, &request).ok_or(CheckFailure::Unconfirmed)?;
    if tree.len() != expected.len()
        || !tree.iter().all(|(path, proof)| {
            expected
                .get(path)
                .is_some_and(|size| *size == proof.as_ref().map(|file| file.size))
        })
    {
        return Err(CheckFailure::Different);
    }
    let local = check_saved_tree(output, source_name, &tree, &active)?;
    Ok(VerifiedTransfer {
        directory,
        tree,
        local,
    })
}

impl VerifiedTransfer {
    pub fn cleanup(
        self,
        bin: &Path,
        output: &Path,
        source_name: Option<&std::ffi::OsStr>,
        active: impl Fn() -> bool,
    ) -> bool {
        self.cleanup_with(
            output,
            source_name,
            |args| command_json(bin, args, &active),
            |remote, name, size, checksum| {
                verify_remote_file(bin, remote, name, size, checksum, &active)
            },
        )
    }

    fn cleanup_with(
        &self,
        output: &Path,
        source_name: Option<&std::ffi::OsStr>,
        request: impl Fn(&[&str]) -> Option<Value>,
        verify_copy: impl Fn(&str, &Path, u64, &str) -> bool,
    ) -> bool {
        let absent = || {
            request(&["ls", &self.directory, "--limit", "1"])
                .is_some_and(|value| directory_missing(&value))
        };
        // An unreliable directory hash needs independent content verification before deletion.
        for (path, checksum) in &self.local.mismatches {
            let Some(Some(proof)) = self.tree.get(path) else {
                return false;
            };
            let relative = path
                .iter()
                .map(|part| part.to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let remote = format!("{}/{relative}", self.directory);
            if !verify_copy(&remote, path, proof.size, checksum) {
                return absent();
            }
        }
        // Recheck after hashing so unrelated additions to this folder are preserved.
        if remote_tree(&self.directory, &request).as_ref() != Some(&self.tree) {
            return absent();
        }
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
        // Keep the verified copies available until the cloud deletion returns.
        // Other programs can still open files for reading in this short interval.
        let mut saved_files = Vec::with_capacity(self.tree.len());
        for (relative, proof) in &self.tree {
            let Some(path) = saved_path(output, source_name, relative) else {
                return false;
            };
            let Ok(file) = fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
                .open(path)
            else {
                return false;
            };
            let unchanged = file.metadata().is_ok_and(|metadata| {
                metadata.file_attributes() & REPARSE_POINT == 0
                    && match proof {
                        Some(proof) => {
                            metadata.is_file()
                                && metadata.len() == proof.size
                                && self.local.write_times.get(relative)
                                    == Some(&metadata.last_write_time())
                        }
                        None => metadata.is_dir(),
                    }
            });
            if !unchanged {
                return false;
            }
            saved_files.push(file);
        }
        request(&["rm", &self.directory, "--force"]).is_some_and(|value| {
            value["code"].as_i64() == Some(0)
                && value["data"]["status"].as_str() != Some("submitted")
        }) || absent()
    }
}

fn verify_remote_file(
    bin: &Path,
    remote: &str,
    name: &Path,
    size: u64,
    checksum: &str,
    active: &impl Fn() -> bool,
) -> bool {
    struct CheckDirectory(PathBuf);
    impl Drop for CheckDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let directory = std::env::temp_dir().join(format!(
        "shandian-baidu-check-{:016x}",
        rand::random::<u64>()
    ));
    if !active() || fs::create_dir(&directory).is_err() {
        return false;
    }
    let scratch = CheckDirectory(directory);
    let Some(leaf) = name.file_name() else {
        return false;
    };
    let file = scratch.0.join(leaf);
    let result = command_json_with_progress(
        bin,
        &["download", remote, &file.to_string_lossy()],
        active,
        || crate::baidu::measure(&scratch.0),
    );
    result.is_some_and(|value| value["code"].as_i64() == Some(0))
        && fs::symlink_metadata(&file).is_ok_and(|m| {
            m.is_file() && m.file_attributes() & REPARSE_POINT == 0 && m.len() == size
        })
        && md5_file(&file, active).is_ok_and(|digest| digest == checksum)
}

fn md5_file(path: &Path, active: &impl Fn() -> bool) -> io::Result<String> {
    use windows_sys::Win32::Security::Cryptography::{
        BCryptCreateHash, BCryptDestroyHash, BCryptFinishHash, BCryptHashData, BCRYPT_HASH_HANDLE,
        BCRYPT_MD5_ALG_HANDLE,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_SEQUENTIAL_SCAN;
    struct Hash(BCRYPT_HASH_HANDLE);
    impl Drop for Hash {
        fn drop(&mut self) {
            unsafe {
                BCryptDestroyHash(self.0);
            }
        }
    }
    let status = |code: i32| {
        if code < 0 {
            Err(io::Error::other("MD5 provider failed"))
        } else {
            Ok(())
        }
    };
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_SEQUENTIAL_SCAN)
        .open(path)?;
    let mut handle = std::ptr::null_mut();
    unsafe {
        status(BCryptCreateHash(
            BCRYPT_MD5_ALG_HANDLE,
            &mut handle,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            0,
            0,
        ))?;
    }
    let hash = Hash(handle);
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        if !active() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "verification cancelled",
            ));
        }
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        unsafe {
            status(BCryptHashData(hash.0, buffer.as_ptr(), size as u32, 0))?;
        }
    }
    let mut digest = [0u8; 16];
    unsafe {
        status(BCryptFinishHash(
            hash.0,
            digest.as_mut_ptr(),
            digest.len() as u32,
            0,
        ))?;
    }
    file.sync_all()?;
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_can_only_prove_a_jobs_own_transfer_directory() {
        let id = "4e6b89003315dace";
        assert!(matches_receipt(
            id,
            "我的应用数据/bdpan/Shandian/4e6b89003315dace/"
        ));
        assert!(matches_receipt(id, "/apps/bdpan/Shandian/4e6b89003315dace"));
        assert!(!matches_receipt(id, "/apps/bdpan/Shandian"));
        assert!(!matches_receipt(
            id,
            "/apps/bdpan/Shandian/4e6b89003315dace/extra"
        ));
        assert!(!matches_receipt("..", "Shandian/.."));
    }

    #[test]
    fn verifier_requires_every_file_and_checks_content() {
        let root = std::env::temp_dir().join(format!(
            "shandian-baidu-proof-{:016x}",
            rand::random::<u64>()
        ));
        let file = root.join("test.bin");
        fs::create_dir(&root).unwrap();
        fs::write(&file, b"abc").unwrap();
        let mut tree = Tree::from([(
            PathBuf::from("test.bin"),
            Some(FileProof {
                size: 3,
                md5: Some("900150983cd24fb0d6963f7d28e17f72".into()),
            }),
        )]);
        assert!(check_saved_tree(&root, None, &tree, &|| true).is_ok());
        fs::write(&file, b"xyz").unwrap();
        assert!(check_saved_tree(&root, None, &tree, &|| true)
            .unwrap()
            .mismatches
            .contains_key(Path::new("test.bin")));
        tree.insert(PathBuf::from("absent.bin"), None);
        assert!(matches!(
            check_saved_tree(&root, None, &tree, &|| true),
            Err(CheckFailure::Different)
        ));
        fs::remove_file(file).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn cleanup_only_deletes_the_verified_job_directory_after_local_save() {
        use serde_json::json;
        use std::cell::Cell;
        let id = "4e6b89003315dace";
        let directory = task_directory(id);
        let root = std::env::temp_dir().join(format!(
            "shandian-baidu-commit-{:016x}",
            rand::random::<u64>()
        ));
        fs::create_dir(&root).unwrap();
        let staged = root.join("音频.bin");
        let saved = root.join("音频 (1).bin");
        fs::write(&staged, b"abc").unwrap();
        let remote = json!({"path":format!("{APP_PATH}{directory}/音频.bin"), "isdir":false,
            "size":3, "md5":"3df10ad71paa9d7729c23a155f3df292"});
        let count = Cell::new(0);
        let changed = Cell::new(false);
        let request = |args: &[&str]| -> Option<Value> {
            assert_eq!(args[1], directory);
            if args[0] == "ls" {
                if args[2..] == ["--limit", "1"] {
                    return Some(json!([remote.clone()]));
                }
                assert_eq!(&args[2..], ["--limit", "100", "--start", "0"]);
                let mut items = vec![remote.clone()];
                if changed.get() {
                    items.push(
                        json!({"path":format!("{APP_PATH}{directory}/另存.bin"), "isdir":false,
                        "size":3, "md5":"900150983cd24fb0d6963f7d28e17f72"}),
                    );
                }
                return Some(json!(items));
            }
            assert_eq!(args, ["rm", &directory, "--force"]);
            assert_eq!(fs::read(&saved).unwrap(), b"abc");
            assert!(fs::remove_file(&saved).is_err());
            count.set(count.get() + 1);
            Some(json!({"code":0}))
        };
        let tree = remote_tree(&directory, &request).unwrap();
        let source_name = Some(std::ffi::OsStr::new("音频.bin"));
        assert!(matches!(
            check_saved_tree(&saved, source_name, &tree, &|| true),
            Err(CheckFailure::Different)
        ));
        fs::rename(&staged, &saved).unwrap();
        let mut local = check_saved_tree(&saved, source_name, &tree, &|| true).unwrap();
        assert!(local.mismatches.is_empty());
        local.mismatches.insert(
            PathBuf::from("音频.bin"),
            "900150983cd24fb0d6963f7d28e17f72".into(),
        );
        let verified = VerifiedTransfer {
            directory: directory.clone(),
            tree,
            local,
        };
        let matching_copy = |remote: &str, path: &Path, size: u64, checksum: &str| {
            assert_eq!(remote, format!("{directory}/音频.bin"));
            assert_eq!(path, Path::new("音频.bin"));
            assert_eq!(size, 3);
            assert_eq!(checksum, "900150983cd24fb0d6963f7d28e17f72");
            true
        };
        assert!(!verified.cleanup_with(&saved, source_name, request, |_, _, _, _| false));
        changed.set(true);
        assert!(!verified.cleanup_with(&saved, source_name, request, matching_copy));
        assert_eq!(count.get(), 0);
        changed.set(false);
        assert!(verified.cleanup_with(&saved, source_name, request, matching_copy));
        assert_eq!(count.get(), 1);
        let missing = |args: &[&str]| {
            if args[0] == "ls" && args[2..] == ["--limit", "1"] {
                Some(
                    json!({"code":1,"error":"找不到指定的文件或目录（错误码 -9），请检查路径是否正确。"}),
                )
            } else if args[0] == "ls" {
                Some(json!([remote.clone()]))
            } else {
                None
            }
        };
        assert!(verified.cleanup_with(&saved, source_name, missing, matching_copy));
        assert!(!directory_missing(&json!({"code":1,"error":"错误码 -6"})));
        fs::remove_file(saved).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
