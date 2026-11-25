"""Build the ordinary portable ZIP from a verified NSIS release."""

import argparse
import json
from pathlib import Path
import shutil
from tempfile import TemporaryDirectory
from zipfile import ZIP_DEFLATED, ZipFile

from sign_nsis_release import APP, check_payload, check_wrapper, sha256


CLEANUP = "清除闪电数据.exe"


def package_portable(release: Path) -> Path:
    records = json.loads((release / "build-manifest.json").read_text("utf-8-sig"))
    record = next(item for item in records if item["edition"] == "plain")
    installer = release / record["file"]
    if record["wrapper"] != "NSIS" or sha256(installer) != record["sha256"]:
        raise ValueError("The ordinary installer does not match the release manifest.")
    payload = release / "plain"
    inputs = release / "package-build/plain"
    check_wrapper(installer, inputs)
    check_payload(inputs / "payload.zip", payload)
    helper = inputs / "shandian-setup.exe"
    if CLEANUP.encode("utf-8") not in helper.read_bytes():
        raise ValueError("Build a release containing the portable cleanup tool first.")
    version = record["appVersion"]
    name = f"Shandian-{version}-Windows-x64-Portable"
    destination = release / f"{name}.zip"
    if destination.exists():
        raise ValueError(f"Portable output already exists: {destination}")
    guide = (
        f"闪电全能下载 Windows {version} 便携版\n\n"
        f"完整解压到一个目录，再运行「{APP}」。若闪电已在运行，请先从托盘退出旧实例。\n"
        f"需要清理时，退出闪电后双击「{CLEANUP}」。这会清除当前 Windows 用户的闪电设置、任务记录、账号信息和缓存。"
        "安装版与便携版共用这些数据，已下载的文件会保留。最后可删除整个解压目录。\n"
    )
    with TemporaryDirectory(prefix=".portable-", dir=release) as temp:
        stage = Path(temp) / name
        shutil.copytree(payload, stage)
        shutil.copy2(helper, stage / CLEANUP)
        (stage / "使用说明.txt").write_text(guide, encoding="utf-8-sig")
        archive_path = Path(temp) / destination.name
        files = {p.relative_to(stage).as_posix(): p for p in stage.rglob("*") if p.is_file()}
        with ZipFile(archive_path, "w", ZIP_DEFLATED, compresslevel=9) as archive:
            for name, path in sorted(files.items()):
                archive.write(path, name)
        with ZipFile(archive_path) as archive:
            if set(archive.namelist()) != set(files) or any(
                archive.read(name) != path.read_bytes() for name, path in files.items()
            ):
                raise ValueError("Portable archive differs from its staged files.")
        archive_path.rename(destination)
    checksum = sha256(destination)
    manifest = {
        "appVersion": version,
        "edition": "plain",
        "extensionVersion": record["extensionVersion"],
        "file": destination.name,
        "bytes": destination.stat().st_size,
        "sha256": checksum,
    }
    (release / "portable-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    with (release / "SHA256SUMS.txt").open("a", encoding="utf-8") as sums:
        sums.write(f"{checksum}  {destination.name}\n")
    return destination


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("release", type=Path)
    args = parser.parse_args()
    print(package_portable(args.release.resolve()))
