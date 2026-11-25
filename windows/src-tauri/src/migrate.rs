// 数据分两处放，按 Windows 的约定：
//   %APPDATA%（Roaming，base）      用户配置：设置、任务列表、下载历史、更新记录
//   %LOCALAPPDATA%（Local，local）  这台机器专用的：yt-dlp/ffmpeg、内核任务库、封面缓存、日志、登录信息
// Roaming 在域账户下会跟着账户同步，放 135 MB 的可执行文件会拖慢登录，也容易被安全软件拦。
//
// BrowserExtension 留在 Roaming：浏览器记的是这个路径，挪走扩展就失效了，只能让用户重装。
//
// 早期版本全放在 Roaming，这里在启动时搬一次。搬不动（文件被占用）不是错误：
// 工具/内核用 current_dir，分类数据用 local_path 找实际位置，下次启动再试。

use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub fn run(base: &Path, local: &Path) {
    let _ = std::fs::create_dir_all(local);
    for group in ["auth", "state", "cache", "logs"] {
        let _ = std::fs::create_dir_all(local.join(group));
    }
    let old_engine = base.join("engine");
    if old_engine.exists() && !local.join("engine").exists() {
        // 内核退出 App 后还在跑，占着任务库，不停掉就搬不动
        stop_kernel_using(&old_engine);
    }
    if base != local {
        for name in ["bin", "engine"] {
            move_dir(&base.join(name), &local.join(name));
        }
    }
    // Local 的现用文件优先于更早的 Roaming 文件。未知文件和 WebView 配置不参与迁移。
    for (index, root) in [local, base].into_iter().enumerate() {
        if index == 1 && base == local { continue; }
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(relative) = category(&name) else { continue };
            let target = local.join(relative);
            if root == base && local.join(&name).exists() && !target.exists() { continue; }
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                move_dir(&entry.path(), &target);
            } else if entry.file_type().is_ok_and(|t| t.is_file()) {
                move_classified_file(&entry.path(), &target);
            }
        }
    }
}

/// 只管理已知数据名；调用方统一由此获得分类后的路径。
fn category(name: &str) -> Option<PathBuf> {
    let group = match name {
        "pikpak.json" | "nas.json" => "auth",
        "pikpak-jobs.json" | "thunder-jobs.json" | "resume-on-launch.json" |
        "name-checked.json" | "sources.json" | "tagged.txt" => "state",
        "thumbs" | "nas-polish" => "cache",
        _ if name.starts_with("cookies-") && name.ends_with(".txt") => "auth",
        _ if name.starts_with("subsinfo-") && name.ends_with(".info.json") => "cache/process",
        _ if name.ends_with(".log") => "logs",
        _ => return None,
    };
    Some(Path::new(group).join(name))
}

/// 搬不动时继续使用旧路径；不存在的文件从新位置开始写。
pub fn local_path(local: &Path, name: &str) -> PathBuf {
    let new = local.join(category(name).unwrap_or_else(|| PathBuf::from(name)));
    let old = local.join(name);
    if !new.exists() && old.exists() { old } else { new }
}

fn move_classified_file(src: &Path, target: &Path) {
    let Some(parent) = target.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() { return; }
    let mut target = target.to_path_buf();
    if target.exists() {
        // 两份记录都保留：新位置优先，旧文件移到同类目录的 legacy，绝不覆盖或删除。
        let legacy = parent.join("legacy");
        if std::fs::create_dir_all(&legacy).is_err() { return; }
        let name = src.file_name().unwrap_or_default().to_string_lossy();
        target = legacy.join(name.as_ref());
        let mut suffix = 1;
        while target.exists() {
            target = legacy.join(format!("{name}.{suffix}"));
            suffix += 1;
        }
    }
    if std::fs::rename(src, &target).is_err() && std::fs::copy(src, &target).is_ok() {
        let _ = std::fs::remove_file(src);
    }
}

/// 这个目录现在在哪：新位置有就用新的；没有但旧位置还在（这次没搬动）就先用旧的
pub fn current_dir(base: &Path, local: &Path, name: &str) -> PathBuf {
    let (new, old) = (local.join(name), base.join(name));
    if !new.exists() && old.exists() {
        old
    } else {
        new
    }
}

fn move_dir(src: &Path, dst: &Path) {
    if !src.exists() {
        return;
    }
    if !dst.exists() {
        if std::fs::rename(src, dst).is_ok() {
            return;
        }
        // 跨盘（Roaming 被重定向时）rename 不了：复制完整了才换名、才删旧的，
        // 中途断电最多留下一个临时目录，下次重来。目录被占用时也会走到这里，复制后删旧失败
        // 就回滚，继续用旧位置
        let tmp = dst.with_extension("migrating");
        let _ = std::fs::remove_dir_all(&tmp);
        if !src_unlocked(src) || copy_tree(src, &tmp).is_err() || std::fs::rename(&tmp, dst).is_err() {
            let _ = std::fs::remove_dir_all(&tmp);
            return;
        }
        let _ = std::fs::remove_dir_all(src);
        return;
    }
    // 两边都有（上次搬了一半，或新版本已经在新位置写过）：只补缺的，已有的不覆盖
    let Ok(rd) = std::fs::read_dir(src) else { return };
    for e in rd.flatten() {
        let (from, to) = (e.path(), dst.join(e.file_name()));
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            move_dir(&from, &to);
        } else {
            move_file(&from, &to);
        }
    }
    let _ = std::fs::remove_dir(src);
}

fn move_file(src: &Path, dst: &Path) {
    if dst.exists() {
        // 新位置的更新，旧的没用了
        let _ = std::fs::remove_file(src);
        return;
    }
    if std::fs::rename(src, dst).is_err() && std::fs::copy(src, dst).is_ok() {
        let _ = std::fs::remove_file(src);
    }
}

/// 有文件正被独占打开（程序还在跑）就别复制：复制出来的任务库可能是不完整的
fn src_unlocked(dir: &Path) -> bool {
    let probe = dir.with_extension("lockprobe");
    if std::fs::rename(dir, &probe).is_ok() {
        let _ = std::fs::rename(&probe, dir);
        return true;
    }
    false
}

fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)?.flatten() {
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &to)?;
        } else {
            std::fs::copy(e.path(), to)?;
        }
    }
    Ok(())
}

/// 只结束数据目录是这一份的内核：命令行里带着这个路径。
/// 用户可能还开着自己的 Gopeed，进程名一样，不能按名字杀
#[cfg(windows)]
pub fn stop_kernel_using(engine_dir: &Path) {
    let dir = engine_dir.display().to_string().replace('\'', "''");
    let script = format!(
        "Get-CimInstance Win32_Process -Filter \"name='gopeed.exe'\" | \
         Where-Object {{ $_.CommandLine -and $_.CommandLine.Contains('{dir}') }} | \
         ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force; Wait-Process -Id $_.ProcessId -Timeout 3 -ErrorAction SilentlyContinue }}"
    );
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(0x0800_0000)
        .status();
}

#[cfg(not(windows))]
pub fn stop_kernel_using(_engine_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categorizes_local_data_preserves_conflicts_and_is_idempotent() {
        let (b, l) = roots("categorized");
        let files = [
            ("cookies-youtube.txt", "auth/cookies-youtube.txt"),
            ("pikpak.json", "auth/pikpak.json"), ("nas.json", "auth/nas.json"),
            ("pikpak-jobs.json", "state/pikpak-jobs.json"),
            ("thunder-jobs.json", "state/thunder-jobs.json"),
            ("resume-on-launch.json", "state/resume-on-launch.json"),
            ("sources.json", "state/sources.json"), ("tagged.txt", "state/tagged.txt"),
            ("name-checked.json", "state/name-checked.json"),
            ("thumbs/a.jpg", "cache/thumbs/a.jpg"),
            ("nas-polish/id/sub.srt", "cache/nas-polish/id/sub.srt"),
            ("subsinfo-job.info.json", "cache/process/subsinfo-job.info.json"),
            ("ytdlp-old.log", "logs/ytdlp-old.log"),
        ];
        for (from, _) in files { touch(l.join(from), from); }
        touch(l.join("auth/pikpak.json"), "current account");
        touch(l.join("state/legacy/name-checked.json"), "existing backup");
        touch(l.join("state/name-checked.json"), "current state");
        for name in ["unknown.json", "EBWebView/Default/Cookies", "EBWebView.small/Default/Cookies", "EBWebView.x1/Default/Cookies", "bin/ffmpeg.exe", "engine/gopeed.db"] {
            touch(l.join(name), "keep");
        }
        run(&b, &l);
        for (from, to) in files {
            assert!(!l.join(from).exists(), "old path remains: {from}");
            assert!(l.join(to).exists(), "new path missing: {to}");
        }
        assert_eq!(std::fs::read_to_string(l.join("auth/pikpak.json")).unwrap(), "current account");
        assert_eq!(std::fs::read_to_string(l.join("auth/legacy/pikpak.json")).unwrap(), "pikpak.json");
        assert_eq!(std::fs::read_to_string(l.join("state/legacy/name-checked.json.1")).unwrap(), "name-checked.json");
        for name in ["unknown.json", "EBWebView/Default/Cookies", "EBWebView.small/Default/Cookies", "EBWebView.x1/Default/Cookies", "bin/ffmpeg.exe", "engine/gopeed.db"] {
            assert_eq!(std::fs::read_to_string(l.join(name)).unwrap(), "keep");
        }
        run(&b, &l);
        assert_eq!(std::fs::read_dir(l.join("auth/legacy")).unwrap().count(), 1);
        std::fs::remove_dir_all(b.parent().unwrap()).unwrap();
    }

    #[test]
    fn locked_file_uses_legacy_path_until_next_start() {
        let (b, l) = roots("locked-auth");
        touch(l.join("pikpak.json"), "local account");
        touch(b.join("pikpak.json"), "older roaming account");
        let held = hold_exclusive(&l.join("pikpak.json"));
        run(&b, &l);
        assert_eq!(local_path(&l, "pikpak.json"), l.join("pikpak.json"));
        assert!(!l.join("auth/pikpak.json").exists());
        drop(held);
        run(&b, &l);
        assert_eq!(std::fs::read_to_string(local_path(&l, "pikpak.json")).unwrap(), "local account");
        assert_eq!(std::fs::read_to_string(l.join("auth/legacy/pikpak.json")).unwrap(), "older roaming account");
        std::fs::remove_dir_all(b.parent().unwrap()).unwrap();
    }

    fn touch(p: PathBuf, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    fn roots(tag: &str) -> (PathBuf, PathBuf) {
        let t = std::env::temp_dir().join(format!("sd-mig-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        (t.join("Roaming"), t.join("Local"))
    }

    #[test]
    fn moves_machine_data_and_keeps_user_config() {
        let (b, l) = roots("a");
        for f in ["settings.json", "videos.json", "removed.json", "video-options.json", "update.json"] {
            touch(b.join(f), "keep");
        }
        touch(b.join("BrowserExtension/manifest.json"), "keep");
        touch(b.join("bin/ffmpeg.exe"), "ff");
        touch(b.join("engine/gopeed.db"), "db");
        touch(b.join("engine/data/x"), "x");
        touch(b.join("thumbs/a.jpg"), "t");
        touch(b.join("cookies-youtube.txt"), "c");
        touch(b.join("pikpak.json"), "p");
        touch(b.join("ytdlp-v1.log"), "l");
        touch(b.join("startup.log"), "l");
        run(&b, &l);
        // 配置和扩展留在原地
        for f in ["settings.json", "videos.json", "removed.json", "video-options.json", "update.json", "BrowserExtension/manifest.json"] {
            assert!(b.join(f).exists(), "{f} 应该留在 Roaming");
        }
        // 其余到 Local，内容不变
        assert_eq!(std::fs::read_to_string(l.join("engine/gopeed.db")).unwrap(), "db");
        assert!(l.join("engine/data/x").exists());
        assert!(l.join("bin/ffmpeg.exe").exists());
        assert!(l.join("cache/thumbs/a.jpg").exists());
        assert!(l.join("auth/cookies-youtube.txt").exists());
        assert!(l.join("auth/pikpak.json").exists());
        for f in ["bin", "engine", "thumbs", "cookies-youtube.txt", "pikpak.json", "ytdlp-v1.log", "startup.log"] {
            assert!(!b.join(f).exists(), "{f} 应该已离开 Roaming");
        }
        // 再跑一次什么都不变
        run(&b, &l);
        assert!(l.join("engine/gopeed.db").exists());
        let _ = std::fs::remove_dir_all(b.parent().unwrap());
    }

    #[test]
    fn never_overwrites_newer_data() {
        let (b, l) = roots("b");
        touch(b.join("thumbs/a.jpg"), "old");
        touch(b.join("thumbs/only-old.jpg"), "old");
        touch(l.join("thumbs/a.jpg"), "new");
        touch(b.join("cookies-x.txt"), "old");
        touch(l.join("cookies-x.txt"), "new");
        run(&b, &l);
        assert_eq!(std::fs::read_to_string(l.join("cache/thumbs/a.jpg")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(l.join("cache/thumbs/only-old.jpg")).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(l.join("auth/cookies-x.txt")).unwrap(), "new");
        let _ = std::fs::remove_dir_all(b.parent().unwrap());
    }

    #[test]
    fn locked_dir_stays_usable_in_old_place() {
        let (b, l) = roots("c");
        touch(b.join("engine/gopeed.db"), "db");
        // 文件被独占打开时目录改不了名
        let held = hold_exclusive(&b.join("engine/gopeed.db"));
        run(&b, &l);
        assert!(b.join("engine/gopeed.db").exists(), "搬不动时旧数据必须还在");
        assert!(!l.join("engine").exists(), "不能留下半份");
        assert!(!l.join("engine.migrating").exists());
        assert_eq!(current_dir(&b, &l, "engine"), b.join("engine"));
        drop(held);
        // 下次启动就搬过去了
        run(&b, &l);
        assert_eq!(current_dir(&b, &l, "engine"), l.join("engine"));
        assert!(l.join("engine/gopeed.db").exists());
        let _ = std::fs::remove_dir_all(b.parent().unwrap());
    }

    fn hold_exclusive(p: &Path) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().read(true).write(true).share_mode(0).open(p).unwrap()
    }
}
