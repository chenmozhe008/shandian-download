#!/bin/bash
# 打 Windows 绿色包：编 release + 拼三个文件 + zip。
#
# 用法：./script/package_windows.sh [版本号]
#   产物：$OUT/shandian-windows-<版本>-x64.zip（默认 C:/ShandianBuild/out）
#
# 注意 target 目录必须放 %TEMP%：C 盘工作目录里 rustc 删中间 .o 会随机「拒绝访问」，
# 详见 windows/README.md「构建」。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
B="${BUILD_DIR:-C:/ShandianBuild}"
OUT="${OUT:-$B/out}"
VER=$(python -c 'import json, sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["version"])' "$ROOT/windows/src-tauri/tauri.conf.json")
[[ "${1:-$VER}" == "$VER" ]] || { echo "打包版本应与 Windows 应用版本 $VER 一致。" >&2; exit 1; }
TARGET="${CARGO_TARGET_DIR:-$TEMP/shandian-target}"
REL="$TARGET/x86_64-pc-windows-gnu/release"

python "$ROOT/windows/installer/sync_extension_version.py"

echo "== 编外壳（target: $TARGET）"
cd "$ROOT/windows/src-tauri"
CARGO_TARGET_DIR="$TARGET" cargo build --release
cd "$ROOT"

ENGINE="$B/gopeed-shandian-amd64.exe"
[ -f "$ENGINE" ] || { echo "找不到内核：$ENGINE（先跑 script/build_engine_windows.sh）"; exit 1; }

STAGE="$OUT/shandian-$VER"
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp "$REL/shandian-windows.exe" "$STAGE/闪电全能下载.exe"
cp "$ENGINE" "$STAGE/gopeed.exe"
# 少这个 dll 程序会一声不响地退出（日志报 api-ms-win-core-winrt 找不到，实际缺的是它）
cp "$REL/WebView2Loader.dll" "$STAGE/WebView2Loader.dll"
# Windows 浏览器扩展：设置页「浏览器扩展」把它同步到数据目录，
# 用户在 Chrome / Edge 里「加载已解压的扩展程序」指过去就能用
cp -r "$ROOT/windows/browser-extension" "$STAGE/BrowserExtension"
cat > "$STAGE/使用说明.txt" <<'TXT'
闪电全能下载 Windows 预览版

解压后双击「闪电全能下载.exe」即可，不用安装。
gopeed.exe 是下载内核，WebView2Loader.dll 是界面运行时，都要和主程序放在同一个目录。
数据目录：%APPDATA%\com.shandian.windows（内核日志在下面的 engine 子目录）

想接管浏览器里的下载：打开「设置 › 浏览器扩展」，点 Chrome 或 Edge 打开扩展管理页，
开启「开发者模式」，再点「显示文件夹」把 BrowserExtension 拖进去。
TXT

python - "$STAGE" "$OUT/shandian-windows-$VER-x64.zip" <<'PYZIP'
import os, sys, zipfile
stage, out = sys.argv[1], sys.argv[2]
if os.path.exists(out):
    os.remove(out)
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for root, dirs, files in os.walk(stage):
        dirs.sort()
        for f in sorted(files):
            full = os.path.join(root, f)
            z.write(full, os.path.relpath(full, stage))
print("   ", os.path.getsize(out) // 1024 // 1024, "MB", out)
PYZIP
echo "== 产物：$OUT/shandian-windows-$VER-x64.zip"
