"""Keep the Windows extension's display version aligned with the app."""

import argparse
import json
import os
from pathlib import Path
import re
from tempfile import NamedTemporaryFile


INSTALLER = Path(__file__).resolve().parent
APP_CONFIG = INSTALLER.parent / "src-tauri" / "tauri.conf.json"
SOURCE_MANIFEST = INSTALLER.parent / "browser-extension" / "manifest.json"


def next_browser_version(version: str) -> str:
    if not re.fullmatch(r"(?:0|[1-9][0-9]{0,4})(?:\.(?:0|[1-9][0-9]{0,4})){0,3}", version):
        raise ValueError(f"Invalid Chromium extension version: {version!r}")
    parts = [int(part) for part in version.split(".")]
    if not any(parts) or max(parts) > 65535:
        raise ValueError(f"Invalid Chromium extension version: {version!r}")
    for index in range(len(parts) - 1, -1, -1):
        if parts[index] < 65535:
            parts[index] += 1
            return ".".join(map(str, parts))
        parts[index] = 0
    if len(parts) < 4:
        return version + ".1"
    raise ValueError("The Chromium extension version cannot be incremented.")


def sync_manifest(path: Path, app_version: str, *, check: bool) -> None:
    previous = path.read_bytes()
    manifest = json.loads(previous)
    if manifest.get("version_name") == app_version:
        return
    if check:
        raise ValueError(
            f"{path}: extension display version must match Windows {app_version}. "
            "Synchronize the source manifest before freezing and copying the release files."
        )

    manifest["version"] = next_browser_version(manifest["version"])
    manifest["version_name"] = app_version
    updated = json.dumps(manifest, ensure_ascii=False, indent=2) + "\n"
    output = NamedTemporaryFile(
        mode="w", encoding="utf-8", newline="\n", dir=path.parent, delete=False
    )
    pending = Path(output.name)
    try:
        with output:
            output.write(updated)
        if path.read_bytes() != previous:
            raise ValueError(f"{path}: manifest changed during synchronization.")
        os.replace(pending, path)
    finally:
        pending.unlink(missing_ok=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=SOURCE_MANIFEST)
    parser.add_argument("--check", action="store_true", help="Validate a staged manifest without editing it.")
    args = parser.parse_args()
    try:
        version = json.loads(APP_CONFIG.read_text(encoding="utf-8"))["version"]
        sync_manifest(args.manifest, version, check=args.check)
    except (OSError, KeyError, TypeError, ValueError) as error:
        raise SystemExit(str(error)) from error


if __name__ == "__main__":
    main()
