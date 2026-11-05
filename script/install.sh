#!/bin/bash
# 闪电全能下载 一键安装：下载最新版 → 放进「应用程序」→ 去掉「无法验证开发者」的拦截 → 装 ffmpeg（有 Homebrew 时）→ 打开
# 用法：curl -fsSL https://raw.githubusercontent.com/chenmozhe008/shandian-download/master/script/install.sh | bash
# 只装 App、不装 ffmpeg：在命令最后加  -s -- --no-ffmpeg
set -euo pipefail

REPO="chenmozhe008/shandian-download"
APP_NAME="闪电全能下载.app"
DEST="${SHANDIAN_DEST:-/Applications}"   # 测试时可以指定别的目录
TEST="${SHANDIAN_TEST:-0}"                # 测试时不退出、不打开正在用的 App
WITH_FFMPEG=1
[[ "${1:-}" == "--no-ffmpeg" ]] && WITH_FFMPEG=0

say() { printf '\033[1m%s\033[0m\n' "$*"; }

[[ "$(uname -s)" == "Darwin" ]] || { echo "只支持 macOS"; exit 1; }
# M 芯片和 Intel 分开出包；用 hw.optional.arm64 判断（终端在 Rosetta 下运行时 uname -m 也会报 x86_64）
if [[ "$(sysctl -n hw.optional.arm64 2>/dev/null)" == "1" ]]; then CHIP="AppleSilicon"; CHIP_NAME="Apple 芯片"; else CHIP="Intel"; CHIP_NAME="Intel 处理器"; fi
MAJOR=$(sw_vers -productVersion | cut -d. -f1)
(( MAJOR >= 14 )) || { echo "需要 macOS 14 或更新版本"; exit 1; }

say "1/4 查找最新版本（${CHIP_NAME}）"
RELEASE_JSON=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest") || {
  echo "连不上 GitHub：安装需要科学上网，请检查网络设置，或到 https://github.com/$REPO/releases 手动下载" >&2; exit 1;
}
URLS=$(printf '%s\n' "$RELEASE_JSON" | grep -o '"browser_download_url": *"[^"]*Shandian-[^"]*\.zip"' | sed 's/.*"\(https[^"]*\)"/\1/' || true)
URL=$(echo "$URLS" | grep -E -- "-${CHIP}\\.zip$" | head -1 || true)
# 0.16.0 及更早只有一个不带芯片标识的包（仅 Apple 芯片）
[[ -n "$URL" || "$CHIP" == "Intel" ]] || URL=$(echo "$URLS" | grep -E '/Shandian-[0-9]+(\.[0-9]+)*\.zip$' | head -1 || true)
[[ -n "$URL" ]] || { echo "没找到安装包，请到 https://github.com/$REPO/releases 手动下载"; exit 1; }
case "$URL" in
  "https://github.com/$REPO/releases/download/"*) ;;
  *) echo "安装包地址不属于官方发布页，已停止安装" >&2; exit 1 ;;
esac
echo "   $URL"

# 校验值必须来自同一个附件，不能把另一架构的 digest 用于当前安装包。
ASSET_COUNT=$(printf '%s' "$RELEASE_JSON" | /usr/bin/plutil -extract assets raw -o - - 2>/dev/null || true)
[[ "$ASSET_COUNT" =~ ^[0-9]+$ ]] || { echo "无法读取发布附件信息，已停止安装" >&2; exit 1; }
DIGEST=""
MATCHED=0
for (( i=0; i<ASSET_COUNT; i++ )); do
  ASSET_URL=$(printf '%s' "$RELEASE_JSON" | /usr/bin/plutil -extract "assets.$i.browser_download_url" raw -o - - 2>/dev/null || true)
  if [[ "$ASSET_URL" == "$URL" ]]; then
    MATCHED=1
    DIGEST=$(printf '%s' "$RELEASE_JSON" | /usr/bin/plutil -extract "assets.$i.digest" raw -o - - 2>/dev/null || true)
    break
  fi
done
[[ "$MATCHED" == 1 ]] || { echo "无法确认安装包的发布附件信息，已停止安装" >&2; exit 1; }
if [[ -n "$DIGEST" ]] && ! [[ "$DIGEST" =~ ^sha256:[0-9a-fA-F]{64}$ ]]; then
  echo "发布附件的校验值格式不正确，已停止安装" >&2; exit 1
fi

say "2/4 下载并安装到 $DEST"
TMP=$(mktemp -d)
STAGE=""
cleanup() { rm -rf "$TMP"; [[ -z "$STAGE" ]] || rm -rf "$STAGE"; }
trap cleanup EXIT
curl -fL --progress-bar -o "$TMP/app.zip" "$URL" || {
  echo "下载安装包失败：GitHub 需要科学上网，请检查网络设置" >&2; exit 1;
}
if [[ -n "$DIGEST" ]]; then
  EXPECTED=$(printf '%s' "${DIGEST#sha256:}" | tr '[:upper:]' '[:lower:]')
  ACTUAL=$(/usr/bin/shasum -a 256 "$TMP/app.zip" | awk '{print $1}')
  [[ "$ACTUAL" == "$EXPECTED" ]] || { echo "安装包校验失败，文件可能损坏，已停止安装" >&2; exit 1; }
else
  echo "   此发布未提供 SHA-256，无法校验下载文件的哈希值"
fi
ditto -x -k "$TMP/app.zip" "$TMP/out"
[[ -d "$TMP/out/$APP_NAME" ]] || { echo "安装包内容不对"; exit 1; }
APP_SOURCE="$TMP/out/$APP_NAME"
APP_ID=$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$APP_SOURCE/Contents/Info.plist" 2>/dev/null || true)
[[ "$APP_ID" == "io.github.chenmozhe008.shandian" ]] || { echo "安装包的应用标识不正确，已停止安装" >&2; exit 1; }
[[ -x "$APP_SOURCE/Contents/MacOS/Shandian" && -x "$APP_SOURCE/Contents/Resources/gopeed" ]] || {
  echo "安装包缺少应用程序或下载内核，已停止安装" >&2; exit 1;
}
/usr/bin/codesign --verify --deep --strict "$APP_SOURCE" || { echo "安装包签名结构校验失败，已停止安装" >&2; exit 1; }
STAGE=$(mktemp -d "$DEST/.shandian-install.XXXXXX")
ditto "$TMP/out/$APP_NAME" "$STAGE/$APP_NAME"
if [[ "$TEST" != 1 ]]; then
  osascript -e 'tell application id "io.github.chenmozhe008.shandian" to quit' >/dev/null 2>&1 || true
  sleep 1
fi
BACKUP=""
if [[ -e "$DEST/$APP_NAME" ]]; then
  # 旧版本移到废纸篓，不直接删
  BACKUP="$HOME/.Trash/闪电全能下载-旧版-$(date +%Y%m%d%H%M%S)-$$.app"
  mv "$DEST/$APP_NAME" "$BACKUP"
fi
if ! mv "$STAGE/$APP_NAME" "$DEST/$APP_NAME"; then
  if [[ -n "$BACKUP" ]]; then
    if mv "$BACKUP" "$DEST/$APP_NAME"; then
      echo "安装失败，已恢复原来的应用。" >&2
    else
      echo "安装失败，旧版位于：${BACKUP}，请手动恢复。" >&2
    fi
  else
    echo "安装失败，未替换已有应用。" >&2
  fi
  exit 1
fi
# 没经过苹果公证的 App 从网上下载后会被拦（「无法验证开发者」），去掉下载标记即可正常打开
xattr -dr com.apple.quarantine "$DEST/$APP_NAME" 2>/dev/null || true

say "3/4 ffmpeg（下载视频网站时合并画面和声音）"
if command -v ffmpeg >/dev/null 2>&1; then
  echo "   已安装"
elif (( WITH_FFMPEG )) && command -v brew >/dev/null 2>&1; then
  brew install ffmpeg
else
  echo "   未安装。下载视频网站需要它：先装 Homebrew（https://brew.sh），再运行 brew install ffmpeg"
fi

say "4/4 打开闪电全能下载"
[[ "$TEST" == 1 ]] || open "$DEST/$APP_NAME"
cat <<'EOF'

安装完成。还差一步：装浏览器扩展（网页上点下载自动交给它；下载抖音、小红书、新片场的视频也要用它）
  1. 在闪电全能下载顶部菜单栏选「闪电全能下载 → 浏览器扩展…」，点「Chrome」或「Edge」
  2. 在打开的扩展管理页里，打开「开发者模式」（Chrome 在右上角，Edge 在左侧栏）
  3. 回到闪电全能下载的窗口，点「显示文件夹」，把 BrowserExtension 文件夹拖进扩展管理页
EOF
