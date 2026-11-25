"""Sign a verified NSIS release and write a separate test installer."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory
from zipfile import ZipFile


HERE = Path(__file__).resolve().parent
APP = "闪电全能下载.exe"
EMBEDDED = ("shandian-setup.exe", "WebView2Loader.dll", "payload.zip")


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def find_signtool():
    configured = os.environ.get("SHANDIAN_SIGNTOOL")
    if configured:
        return configured
    if on_path := shutil.which("signtool.exe"):
        return on_path
    sdk = Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Windows Kits/10/bin"
    for candidate in sorted(sdk.glob("*/x64/signtool.exe"), reverse=True):
        return str(candidate)
    raise ValueError("Install Windows SDK SignTool or set SHANDIAN_SIGNTOOL.")


def sign_file(tool, path, thumbprint, timestamp, store):
    machine = ["/sm"] if store == "LocalMachine" else []
    subprocess.run(
        [str(tool), "sign", "/s", "My", *machine, "/sha1", thumbprint,
         "/fd", "SHA256", "/tr", timestamp, "/td", "SHA256", str(path)],
        check=True,
    )
    subprocess.run([str(tool), "verify", "/pa", "/all", "/tw", str(path)], check=True)


def check_payload(zip_path, directory):
    files = {
        path.relative_to(directory).as_posix(): path
        for path in directory.rglob("*")
        if path.is_file() and path.name != "使用说明.txt"
    }
    with ZipFile(zip_path) as archive:
        names = [item.filename for item in archive.infolist() if not item.is_dir()]
        if len(names) != len(files) or set(names) != set(files):
            raise ValueError("Payload file list does not match its frozen release.")
        for name, path in files.items():
            with archive.open(name) as member:
                if hashlib.file_digest(member, "sha256").hexdigest() != sha256(path):
                    raise ValueError(f"Payload file differs from the frozen release: {name}")


def check_wrapper(installer, inputs):
    data = installer.read_bytes()
    if b"NullsoftInst" not in data:
        raise ValueError("Expected a standard NSIS installer.")
    for name in EMBEDDED:
        if (inputs / name).read_bytes() not in data:
            raise ValueError(f"Incorrect embedded installer asset: {name}")


def package_signed_release(release, edition, destination, signtool, makensis, thumbprint, timestamp, store):
    release = release.resolve()
    destination = destination.resolve()
    if destination.suffix.lower() != ".exe" or destination.is_relative_to(release):
        raise ValueError("Choose an EXE output path outside the original release directory.")
    if destination.exists():
        raise ValueError(f"Signed output already exists: {destination}")
    records = json.loads((release / "build-manifest.json").read_text(encoding="utf-8-sig"))
    record = next((item for item in records if item["edition"] == edition), None)
    if not record or record["wrapper"] != "NSIS":
        raise ValueError(f"No verified NSIS release for edition {edition}.")
    version = record["appVersion"]
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError("Invalid release version.")
    suffix = "-NAS" if edition == "nas" else ""
    expected_name = f"Shandian-{version}-Windows{suffix}-x64-setup.exe"
    original = release / expected_name
    if record["file"] != expected_name or sha256(original) != record["sha256"]:
        raise ValueError("Original installer does not match the release checksum.")
    inputs = release / "package-build" / edition
    snapshots = json.loads((release / "frozen-inputs.json").read_text(encoding="utf-8-sig"))
    snapshot = next((item for item in snapshots if item["edition"] == edition), None)
    if not snapshot or snapshot["version"] != version:
        raise ValueError("Missing frozen inputs for this release.")

    with TemporaryDirectory(prefix=".shandian-sign-", dir=destination.parent) as name:
        work = Path(name)
        payload = work / "payload"
        shutil.copytree(release / edition, payload)
        for asset in (*EMBEDDED, "icon.ico", "bootstrap.nsi"):
            shutil.copy2(inputs / asset, work / asset)
        if sha256(work / "bootstrap.nsi") != snapshot["files"]["windows/installer/bootstrap.nsi"]:
            raise ValueError("NSIS script differs from the original frozen input.")
        if sha256(work / "icon.ico") != snapshot["files"]["windows/src-tauri/icons/icon.ico"]:
            raise ValueError("Installer icon differs from the original frozen input.")
        check_wrapper(original, work)
        check_payload(work / "payload.zip", payload)
        sign_file(signtool, payload / APP, thumbprint, timestamp, store)
        sign_file(signtool, work / "shandian-setup.exe", thumbprint, timestamp, store)
        subprocess.run([sys.executable, str(HERE / "make_payload.py"), str(payload), str(work / "payload.zip")], check=True)
        title = "Shandian NAS" if edition == "nas" else "Shandian"
        subprocess.run(
            [str(makensis), "/WX", "/V3", f"/DAPP_VERSION={version}",
             "/DOUTPUT_NAME=signed-setup.exe", f"/DAPP_TITLE={title}", "bootstrap.nsi"],
            cwd=work,
            check=True,
        )
        built = work / "signed-setup.exe"
        sign_file(signtool, built, thumbprint, timestamp, store)
        check_wrapper(built, work)
        check_payload(work / "payload.zip", payload)
        result = sha256(built)
        built.rename(destination)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("release", type=Path)
    parser.add_argument("--edition", choices=("plain", "nas"), default="plain")
    parser.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    thumbprint = os.environ.get("SHANDIAN_SIGN_CERT_SHA1", "").replace(" ", "")
    timestamp = os.environ.get("SHANDIAN_TIMESTAMP_URL", "")
    store = os.environ.get("SHANDIAN_SIGN_STORE", "CurrentUser")
    if not re.fullmatch(r"[0-9a-fA-F]{40}", thumbprint):
        parser.error("Set SHANDIAN_SIGN_CERT_SHA1 to the certificate thumbprint.")
    if not timestamp:
        parser.error("Set SHANDIAN_TIMESTAMP_URL to the CA's RFC 3161 timestamp URL.")
    if store not in ("CurrentUser", "LocalMachine"):
        parser.error("SHANDIAN_SIGN_STORE must be CurrentUser or LocalMachine.")
    makensis = os.environ.get("SHANDIAN_MAKENSIS") or Path(os.environ["LOCALAPPDATA"]) / "tauri/NSIS/makensis.exe"
    try:
        checksum = package_signed_release(
            args.release, args.edition, args.destination, find_signtool(), makensis, thumbprint, timestamp, store,
        )
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Signing failed: {error}\n")
    print(f"{checksum}  {args.destination}")


if __name__ == "__main__":
    main()
