#!/bin/bash
# signed 从已归档的 NSIS 正式包生成独立签名版。无该参数时沿用历史开发打包流程。
#
# 前置：C:/ShandianBuild/out/shandian-<应用版本>/ 里已经是要发的版本（主程序、gopeed.exe、
# WebView2Loader.dll、yt-dlp.exe、BrowserExtension）。这个目录原样打进安装包。
#
# 旧流程的结构说明见 stub/src/main.rs。
set -e

HERE="$(cd "$(dirname "$0")" && pwd)"
# 用户协议、隐私说明只维护 ui/legal.js 一份：打包前同步到安装器，两边内容永远一致
BUILD=/c/ShandianBuild
if [[ "${1:-}" == signed ]]; then
  : "${SHANDIAN_OUT:?Provide the archived release payload directory, e.g. output/windows/0.1.74/plain}"
  : "${SHANDIAN_SIGNED_DEST:?Provide the separate signed setup.exe path}"
  payload=$(cygpath -au "$SHANDIAN_OUT")
  edition=${SHANDIAN_EDITION:-plain}
  [[ "$(basename "$payload")" == "$edition" ]] || { echo "Payload folder and edition do not match" >&2; exit 1; }
  exec python "$(cygpath -w "$HERE/sign_nsis_release.py")" \
    "$(cygpath -w "$(dirname "$payload")")" --edition "$edition" --destination "$(cygpath -aw "$SHANDIAN_SIGNED_DEST")"
fi

VERSION=$(python -c 'import json, sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["version"])' "$(cygpath -w "$HERE/../src-tauri/tauri.conf.json")")
# SHANDIAN_OUT 可以指向别的发布目录（out 里的程序正开着、换不了时用）
OUT="${SHANDIAN_OUT:-$BUILD/out/shandian-$VERSION}"
python "$(cygpath -w "$HERE/sync_extension_version.py")" --check --manifest "$(cygpath -w "$OUT/BrowserExtension/manifest.json")"

STUB="$HERE/stub"
cp -f "$HERE/../ui/legal.js" "$HERE/app/ui/legal.js"
cp -f "$HERE/../../LICENSE" "$OUT/LICENSE"
mkdir -p "$OUT/ThirdPartyNotices"
cp -f "$HERE/../third-party-notices/ghost-downloader-reference.md" "$HERE/../third-party-notices/Ghost-Downloader-3-GPL-3.0.txt" "$OUT/ThirdPartyNotices/"
export PATH="$BUILD/rustup/toolchains/stable-x86_64-pc-windows-gnu/bin:$BUILD/cargo/bin:$BUILD/rustup/toolchains/1.99.1-x86_64-pc-windows-gnu/bin:$BUILD/msys64/mingw64/bin:$PATH"
export CARGO_HOME="$BUILD/cargo" RUSTUP_HOME="$BUILD/rustup"
# target 放 %TEMP%：C 盘工作目录里删文件会随机失败，见 windows/README.md「编外壳」
# 可以用 SHANDIAN_TARGET_DIR 换位置：%TEMP% 会被系统清理删掉一部分文件（2026-11-11 serde_core 找不到 private.rs）
# 两种版别分开缓存，避免 stub 复用另一版内嵌的安装数据。
TARGET_WIN="${SHANDIAN_TARGET_DIR:-$(cygpath -w "$TEMP")\\shandian-target-installer-${SHANDIAN_EDITION:-plain}}"
export CARGO_TARGET_DIR="$TARGET_WIN"
TARGET_UNIX="$(cygpath -u "$TARGET_WIN")"

PROFILE=release; FLAG=--release
if [ "$1" = "debug" ]; then PROFILE=debug; FLAG=; fi

(cd "$HERE/app" && cargo build $FLAG)

EMBED="$STUB/embed"
mkdir -p "$EMBED"
cp -f "$TARGET_UNIX/x86_64-pc-windows-gnu/$PROFILE/shandian-setup.exe" "$EMBED/" 2>/dev/null \
  || cp -f "$TARGET_UNIX/$PROFILE/shandian-setup.exe" "$EMBED/"
cp -f "$OUT/WebView2Loader.dll" "$EMBED/"
python "$(cygpath -w "$HERE/make_payload.py")" "$(cygpath -w "$OUT")" "$(cygpath -w "$EMBED/payload.zip")"

(cd "$STUB" && cargo build $FLAG)

# 调试版两百多兆、带调试端口，不能混进发布目录
# SHANDIAN_EDITION=nas 打 NAS 版：安装器界面标注版别，文件名带 -nas
SUFFIX=""
if [ "$SHANDIAN_EDITION" = "nas" ]; then SUFFIX="-nas"; fi
DEST="$BUILD/out/闪电全能下载_${VERSION}${SUFFIX}_x64-setup.exe"
if [ "$PROFILE" = "debug" ]; then DEST="$TARGET_UNIX/setup-debug.exe"; fi
cp -f "$TARGET_UNIX/x86_64-pc-windows-gnu/$PROFILE/shandian-stub.exe" "$DEST" 2>/dev/null \
  || cp -f "$TARGET_UNIX/$PROFILE/shandian-stub.exe" "$DEST"
ls -la "$DEST"
