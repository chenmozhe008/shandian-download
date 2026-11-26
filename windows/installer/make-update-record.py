"""Generate the public integrity record for a verified Windows installer."""

import argparse
import hashlib
import json
import re
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("release", type=Path, help="Windows release directory with build-manifest.json")
    args = parser.parse_args()
    records = json.loads((args.release / "build-manifest.json").read_text(encoding="utf-8"))
    candidates = [record for record in records if record.get("edition") == "plain"]
    if len(candidates) != 1:
        raise SystemExit("Expected exactly one ordinary edition in the build manifest.")
    record = candidates[0]
    version = record["appVersion"]
    if not isinstance(version, str) or re.fullmatch(r"(?:0|[1-9]\d*)(?:\.(?:0|[1-9]\d*)){2}", version) is None:
        raise SystemExit("Invalid Windows version.")
    name = f"Shandian-{version}-Windows-x64-setup.exe"
    if record.get("wrapper") != "NSIS" or record.get("file") != name:
        raise SystemExit("Build manifest does not identify the matching NSIS installer.")
    installer = args.release / name
    with installer.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    size = installer.stat().st_size
    if digest != record.get("sha256") or size != record.get("bytes"):
        raise SystemExit("Installer does not match its build manifest.")
    out = args.release / name.replace("-setup.exe", "-update.json")
    with out.open("x", encoding="utf-8") as file:
        json.dump({"version": version, "file": name, "sha256": digest, "bytes": size}, file, indent=2)
        file.write("\n")
    print(out)


if __name__ == "__main__":
    main()
