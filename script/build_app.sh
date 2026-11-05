#!/usr/bin/env bash
# build_app.sh [run|package] —— 编译、打包成 dist/闪电全能下载.app（run 时顺便打开）
# ARCH=arm64|x86_64 指定芯片（默认本机）；本机以外的芯片只打包到 dist/<芯片>/，不会打开
set -euo pipefail
APP_NAME="闪电全能下载"; EXECUTABLE_NAME="Shandian"; BUNDLE_ID="io.github.chenmozhe008.shandian"; APP_VERSION="0.18.52"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT"
ARCH="${ARCH:-$(uname -m)}"
case "$ARCH" in arm64) GOARCH=arm64 ;; x86_64) GOARCH=amd64 ;; *) echo "ARCH 只能是 arm64 或 x86_64"; exit 1 ;; esac
ENGINE_BIN="${SHANDIAN_ENGINE_BIN:-/Volumes/Jack Feng/Applications/ShandianBuild/gopeed-shandian-$GOARCH}"
if [[ ! -x "$ENGINE_BIN" ]]; then
  echo "找不到可执行的下载内核：${ENGINE_BIN}。请先运行 script/build_engine.sh 编译内核。" >&2
  exit 1
fi
if [[ "$ARCH" == "$(uname -m)" ]]; then APP="$ROOT/dist/$APP_NAME.app"; else APP="$ROOT/dist/$ARCH/$APP_NAME.app"; fi
APP="${SHANDIAN_BUILD_APP_PATH:-$APP}"
CODESIGN_IDENTITY="${CODESIGN_IDENTITY:-Apple Development}"
security find-identity -v -p codesigning 2>/dev/null | grep -q "$CODESIGN_IDENTITY" || CODESIGN_IDENTITY="-"

# 默认生成开发版；正式包必须明确指定，并从版本标签对应的干净源码构建。
DEV_BUILD=1
if [[ "${SHANDIAN_RELEASE_BUILD:-0}" == "1" ]]; then
  if [[ -n "$(git status --porcelain 2>/dev/null)" ]] || ! git rev-parse -q --verify "refs/tags/v${APP_VERSION}^{commit}" >/dev/null || [[ "$(git rev-parse HEAD)" != "$(git rev-parse "refs/tags/v${APP_VERSION}^{commit}")" ]]; then
    echo "正式打包需要干净的工作区，以及指向当前提交的 v${APP_VERSION} 标签。" >&2
    exit 1
  fi
  if grep -q '^## .*（开发版）' "$ROOT/CHANGELOG.md"; then
    echo "CHANGELOG 还有开发版小节：发版前要把它们汇总进正式版这一节。" >&2
    exit 1
  fi
  DEV_BUILD=0
fi

TRIPLE="$ARCH-apple-macosx14.0"
# 有外层沙箱（如编辑器内置终端）时 sandbox-exec 无法嵌套，可传 SWIFT_FLAGS=--disable-sandbox
SWIFT_FLAGS="${SWIFT_FLAGS:-}"
swift build -c release --triple "$TRIPLE" $SWIFT_FLAGS
BIN="$(swift build -c release --triple "$TRIPLE" --show-bin-path $SWIFT_FLAGS)/$EXECUTABLE_NAME"

# 图标：AppIcon-source.png 更新过才重做 icns
ICNS="$ROOT/Resources/AppIcon.icns"
if [[ ! -f "$ICNS" || "$ROOT/Resources/AppIcon-source.png" -nt "$ICNS" ]]; then
  SET="$ROOT/tmp/AppIcon.iconset"; rm -rf "$SET"; mkdir -p "$SET"
  for s in 16 32 128 256 512; do
    sips -z $s $s "$ROOT/Resources/AppIcon-source.png" --out "$SET/icon_${s}x${s}.png" >/dev/null
    sips -z $((s*2)) $((s*2)) "$ROOT/Resources/AppIcon-source.png" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$SET" -o "$ICNS"; rm -rf "$SET"
fi

# 固定官方目录版：首次打包下载并校验，安装包自带，运行时仍可后台更新。
YTDLP_VERSION="2026.10.26"
YTDLP_SHA256="20d98fa608998eff87d1d35a6e9fb65e4b874639a6cc89addc19da3becde3ef1"
YTDLP_CACHE="$ROOT/tmp/yt-dlp-$YTDLP_VERSION"
YTDLP_ZIP="$YTDLP_CACHE/yt-dlp_macos.zip"
mkdir -p "$YTDLP_CACHE"
if [[ ! -f "$YTDLP_ZIP" ]] || ! printf '%s  %s\n' "$YTDLP_SHA256" "$YTDLP_ZIP" | shasum -a 256 -c >/dev/null 2>&1; then
  curl -fL --retry 2 -o "$YTDLP_ZIP" "https://github.com/yt-dlp/yt-dlp/releases/download/$YTDLP_VERSION/yt-dlp_macos.zip"
fi
printf '%s  %s\n' "$YTDLP_SHA256" "$YTDLP_ZIP" | shasum -a 256 -c >/dev/null

rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/$EXECUTABLE_NAME"
cp "$ICNS" "$APP/Contents/Resources/AppIcon.icns"
cp "$ROOT/CHANGELOG.md" "$APP/Contents/Resources/CHANGELOG.md"  # 「关于 → 更新记录」直接读它
cp "$ROOT/Resources/done.wav" "$APP/Contents/Resources/done.wav"  # 下载完成提示音（script/make_done_sound.py 生成）
cp -R "$ROOT/browser-extension" "$APP/Contents/Resources/BrowserExtension"  # 启动时拷到 Application Support（浏览器认固定路径）
ditto -x -k "$YTDLP_ZIP" "$APP/Contents/Resources/yt-dlp_macos"
test -x "$APP/Contents/Resources/yt-dlp_macos/yt-dlp_macos"
# 打过补丁的内核（script/build_engine.sh 编出来的）；App 启动时发现和本机装的不一样就换上
cp "$ENGINE_BIN" "$APP/Contents/Resources/gopeed"
mkdir -p "$APP/Contents/Resources/zh-Hans.lproj"  # 声明中文，系统菜单（文件、编辑、窗口）才显示中文
# 强调色（琥珀）：侧栏选中行、按钮等系统控件跟随它；SwiftPM 不编译 xcassets，这里用 actool 单独编
# actool 必须写一份 partial Info.plist，但下面的 Info.plist 是手写的、用不到它：写到系统临时目录，用完删掉
ASSETS_PLIST="$(mktemp -t shandian-assets).plist"
xcrun actool "$ROOT/Resources/Assets.xcassets" --compile "$APP/Contents/Resources" --platform macosx \
  --minimum-deployment-target 14.0 --accent-color AccentColor --output-partial-info-plist "$ASSETS_PLIST" >/dev/null
rm -f "$ASSETS_PLIST" "${ASSETS_PLIST%.plist}"
BUILD_NUMBER="$(git rev-list --count HEAD 2>/dev/null || echo 1)"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleDevelopmentRegion</key><string>zh-Hans</string>
  <key>CFBundleLocalizations</key><array><string>zh-Hans</string></array>
  <key>CFBundleExecutable</key><string>$EXECUTABLE_NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleIconFile</key><string>AppIcon.icns</string>
  <key>NSAccentColorName</key><string>AccentColor</string>
  <key>CFBundleShortVersionString</key><string>$APP_VERSION</string>
  <key>CFBundleVersion</key><string>$BUILD_NUMBER</string>
  <key>SDDevBuild</key><string>$DEV_BUILD</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>CFBundleURLTypes</key><array>
    <dict><key>CFBundleURLName</key><string>磁力链接</string><key>CFBundleURLSchemes</key><array><string>magnet</string></array></dict>
    <dict><key>CFBundleURLName</key><string>ed2k 链接</string><key>CFBundleURLSchemes</key><array><string>ed2k</string></array></dict>
    <dict><key>CFBundleURLName</key><string>闪电全能下载</string><key>CFBundleURLSchemes</key><array><string>shandian</string></array></dict>
  </array>
  <key>CFBundleDocumentTypes</key><array>
    <dict><key>CFBundleTypeName</key><string>BitTorrent 种子</string><key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key><array><string>org.bittorrent.torrent</string></array>
      <key>CFBundleTypeExtensions</key><array><string>torrent</string></array></dict>
  </array>
</dict></plist>
PLIST
codesign --force --sign "$CODESIGN_IDENTITY" --identifier "$BUNDLE_ID" --timestamp=none "$APP"
codesign --verify --strict "$APP"
echo "打包完成：$APP"
lipo -info "$APP/Contents/MacOS/$EXECUTABLE_NAME" "$APP/Contents/Resources/gopeed"
if [[ "${1:-run}" == "--install-dev" ]]; then
  [[ "$DEV_BUILD" == 1 && "$ARCH" == "$(uname -m)" ]] || { echo '只能安装本机架构的开发构建。' >&2; exit 1; }
  "$HOME/Coding/tools/mac-app-workflow/install-dev.sh" shandian "$APP"
fi
if [[ "${1:-run}" == "run" && "$ARCH" == "$(uname -m)" ]]; then pkill -x "$EXECUTABLE_NAME" 2>/dev/null || true; sleep 0.5; open -n "$APP"; fi
