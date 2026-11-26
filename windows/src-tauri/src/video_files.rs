use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PublishPlan {
    pub(crate) media: PathBuf,
    pub(crate) suffix: String,
    moves: Vec<AssetMove>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AssetMove {
    from: PathBuf,
    to: PathBuf,
    len: u64,
    write_time: u64,
}

pub(crate) enum PublishError {
    Conflict,
    Io(io::Error),
}

impl From<io::Error> for PublishError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub(crate) fn check_stage(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() && meta.file_attributes() & 0x400 == 0 {
        Ok(())
    } else {
        Err(io::Error::other("视频临时目录无效"))
    }
}

pub(crate) fn regular(path: &Path) -> io::Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_attributes() & 0x400 != 0 {
        return Err(io::Error::other("文件类型无法确认"));
    }
    Ok(meta)
}

pub(crate) fn is_subtitle_tail(tail: &str) -> bool {
    let (language, ext) = tail.rsplit_once('.').unwrap_or(("", tail));
    crate::videos::is_sub_ext(ext)
        && language
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub(crate) fn sidecars(media: &Path) -> Vec<PathBuf> {
    let Some((dir, stem)) = media
        .parent()
        .zip(media.file_stem().and_then(|x| x.to_str()))
    else {
        return Vec::new();
    };
    let prefix = format!("{stem}.");
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let tail = name.to_str()?.strip_prefix(&prefix)?;
            let inner = [".original.bak", ".original_bak", ".bak"]
                .into_iter()
                .find_map(|suffix| tail.strip_suffix(suffix))
                .unwrap_or(tail);
            if !is_subtitle_tail(inner) {
                return None;
            }
            let path = entry.path();
            regular(&path).is_ok().then_some(path)
        })
        .collect()
}

impl PublishPlan {
    pub(crate) fn new(
        source: &Path,
        directory: &Path,
        suffix: &str,
        allow_copy: bool,
    ) -> Result<Self, PublishError> {
        let stem = source
            .file_stem()
            .and_then(|v| v.to_str())
            .ok_or_else(|| io::Error::other("视频文件名无效"))?;
        let title = stem
            .rsplit_once(" [")
            .filter(|(_, mark)| mark.ends_with(']'))
            .map(|(title, _)| title)
            .filter(|title| !title.is_empty())
            .ok_or_else(|| io::Error::other("无法识别下载临时文件的标题"))?;
        let ext = source
            .extension()
            .and_then(|v| v.to_str())
            .ok_or_else(|| io::Error::other("视频扩展名无效"))?;
        let mut assets = sidecars(source);
        // Publish the video last, after the matching sidecars are in place.
        assets.push(source.to_path_buf());
        let mut endings = Vec::with_capacity(assets.len());
        for asset in assets {
            let name = asset
                .file_name()
                .and_then(|v| v.to_str())
                .ok_or_else(|| io::Error::other("成品文件名无效"))?;
            let ending = name
                .strip_prefix(stem)
                .ok_or_else(|| io::Error::other("找不到字幕对应的视频"))?
                .to_owned();
            let meta = regular(&asset)?;
            endings.push((asset, ending, meta.len(), meta.last_write_time()));
        }
        let candidate = |suffix: &str| Self {
            media: directory.join(format!("{title}{suffix}.{ext}")),
            suffix: suffix.into(),
            moves: endings
                .iter()
                .map(|(from, ending, len, write_time)| AssetMove {
                    from: from.clone(),
                    to: directory.join(format!("{title}{suffix}{ending}")),
                    len: *len,
                    write_time: *write_time,
                })
                .collect(),
        };
        let free = |plan: &Self| {
            plan.moves.iter().all(|asset| {
                fs::symlink_metadata(&asset.to)
                    .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
            })
        };
        let starting = candidate(suffix);
        if free(&starting) {
            return Ok(starting);
        }
        if !allow_copy {
            return Err(PublishError::Conflict);
        }
        for n in 1..1000 {
            let next = candidate(&format!("({n})"));
            if free(&next) {
                return Ok(next);
            }
        }
        Err(io::Error::other("找不到可用的文件名").into())
    }

    pub(crate) fn apply(
        &self,
        stage: &Path,
        output: &Path,
        active: impl Fn() -> bool,
    ) -> io::Result<Vec<PathBuf>> {
        for asset in &self.moves {
            if asset.from.parent() != Some(stage) || asset.to.parent() != Some(output) || !active()
            {
                return Err(io::Error::other("保存已中断"));
            }
            let matches = |file: &Path| {
                regular(file).is_ok_and(|meta| {
                    meta.len() == asset.len && meta.last_write_time() == asset.write_time
                })
            };
            if fs::symlink_metadata(&asset.from).is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
                && matches(&asset.to)
            {
                continue; // A previous attempt moved this particular file.
            }
            if !matches(&asset.from) {
                return Err(io::Error::other("临时文件已发生变化"));
            }
            crate::baidu::move_file(
                &asset.from,
                &asset.to,
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )?;
        }
        Ok(self.destinations().cloned().collect())
    }

    pub(crate) fn moved_files(&self, stage: &Path, output: &Path) -> Vec<PathBuf> {
        self.moves
            .iter()
            .filter(|asset| {
                asset.from.parent() == Some(stage)
                    && asset.to.parent() == Some(output)
                    && fs::symlink_metadata(&asset.from)
                        .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
                    && regular(&asset.to).is_ok_and(|meta| {
                        meta.len() == asset.len && meta.last_write_time() == asset.write_time
                    })
            })
            .map(|asset| asset.to.clone())
            .collect()
    }

    pub(crate) fn destinations(&self) -> impl Iterator<Item = &PathBuf> {
        self.moves.iter().map(|asset| &asset.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishes_clean_video_and_subtitles_together_and_recovers_after_interruption() {
        let dir =
            std::env::temp_dir().join(format!("sd-clean-video-{:016x}", rand::random::<u64>()));
        let stage = dir.join(".shandian-video-fixture");
        fs::create_dir_all(&stage).unwrap();
        let video = stage.join("教程 [2024] [ytID].mp4");
        fs::write(&video, b"new media").unwrap();
        fs::write(stage.join("教程 [2024] [ytID].zh-en.ass"), b"subtitle").unwrap();
        fs::write(dir.join("教程 [2024].mp4"), b"old media").unwrap();

        assert!(matches!(
            PublishPlan::new(&video, &dir, "", false),
            Err(PublishError::Conflict)
        ));
        let plan = PublishPlan::new(&video, &dir, "", true).ok().unwrap();
        assert_eq!(plan.media, dir.join("教程 [2024](1).mp4"));
        let calls = std::cell::Cell::new(0);
        assert!(plan
            .apply(&stage, &dir, || {
                calls.set(calls.get() + 1);
                calls.get() == 1
            })
            .is_err());
        assert_eq!(
            plan.moved_files(&stage, &dir),
            [dir.join("教程 [2024](1).zh-en.ass")]
        );
        assert!(plan.apply(&stage, &dir, || true).is_ok());
        assert_eq!(fs::read(&plan.media).unwrap(), b"new media");
        assert_eq!(
            fs::read(dir.join("教程 [2024](1).zh-en.ass")).unwrap(),
            b"subtitle"
        );
        assert_eq!(fs::read(dir.join("教程 [2024].mp4")).unwrap(), b"old media");
        fs::remove_dir_all(dir).unwrap();
    }
}
