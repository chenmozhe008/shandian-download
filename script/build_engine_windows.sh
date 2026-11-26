#!/bin/bash
# 编译打过补丁的 Gopeed 内核（Windows 版，给闪电全能下载用）。对应 macOS 的 build_engine.sh。
# Windows 补丁：windows/engine/shandian-gopeed-v1.9.3.patch
#   - BT 块完成记录落盘 + 不用 .part 文件：重启内核不再整片重新校验
#   - 全局下载限速（BT 和直链共用，配置 extra.shandianDownloadLimit，KB/s）
#   - PUT /api/v1/tasks/{id}/stream：边下边播
#
# 前置：Go 1.24.x 与 MinGW-w64（含 g++）。路径用环境变量给，默认放在 C:/ShandianBuild。
#   GO_BIN      Go 的 bin 目录
#   MINGW_BIN   MinGW-w64 的 bin 目录（里面要有 gcc.exe、g++.exe）
#   BUILD_DIR   构建目录（源码、模块缓存、产物都在这里）
#
# 必须开 CGO：不开时 anacrolix 用纯 Go 的 uTP，连 uTP 用户很差（macOS 上实测只剩 1 个连接、
# 几十 KB/s），Windows 同样。编完会检查二进制里确实链进了 libutp，没链上直接报错。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
B="${BUILD_DIR:-C:/ShandianBuild}"
GO_BIN="${GO_BIN:-$B/go/bin}"
MINGW_BIN="${MINGW_BIN:-$B/msys64/mingw64/bin}"
# 上游版本可以覆盖，用来试新版本（补丁能否打上见 check_upstream.sh）：
#   GOPEED_VER=v2.0.0-beta.3 ./script/build_engine_windows.sh
GOPEED_VER="${GOPEED_VER:-v1.9.3}"
PATCH="$ROOT/windows/engine/shandian-gopeed-$GOPEED_VER.patch"
# 补丁只有 v1.9.3 一份；试别的版本时沿用这份，让 patch 自己 fuzz 找插入点
[ -f "$PATCH" ] || PATCH="$ROOT/windows/engine/shandian-gopeed-v1.9.3.patch"

[ -x "$GO_BIN/go.exe" ] || [ -x "$GO_BIN/go" ] || { echo "找不到 Go：$GO_BIN"; exit 1; }
[ -x "$MINGW_BIN/gcc.exe" ] || { echo "找不到 gcc：$MINGW_BIN"; exit 1; }
[ -f "$PATCH" ] || { echo "找不到补丁：$PATCH"; exit 1; }

mkdir -p "$B"
cd "$B"

# 源码：用 codeload 拿 tar 包（不用 git clone，国内代理下 git 协议常被 502）
SRC="$B/gopeed-$GOPEED_VER"
if [ ! -d "$SRC" ]; then
  echo "拉取 Gopeed $GOPEED_VER 源码"
  curl -L --retry 5 --retry-delay 3 -sS -o "$B/gopeed-$GOPEED_VER.tar.gz" \
    "https://codeload.github.com/GopeedLab/gopeed/tar.gz/refs/tags/$GOPEED_VER"
  tar -xzf "$B/gopeed-$GOPEED_VER.tar.gz" -C "$B"
  # 补丁文件是 CRLF 存的，源码是 LF，直接 apply 会整片对不上，先转成 LF
  tr -d '\r' < "$PATCH" > "$B/shandian-lf.patch"
  ( cd "$SRC" && patch -p1 --batch --forward < "$B/shandian-lf.patch" )
fi

# cmd/web 要 web/dist，headless 版不需要真前端，放个占位
mkdir -p "$SRC/cmd/web/dist"
[ -f "$SRC/cmd/web/dist/index.html" ] || printf '<!doctype html><title>Shandian engine</title>' > "$SRC/cmd/web/dist/index.html"

cd "$SRC"
export PATH="$GO_BIN:$MINGW_BIN:$PATH"
export GOPATH="$B/gopath" GOMODCACHE="$B/gopath/pkg/mod" GOCACHE="$B/gocache"
export GOFLAGS=-mod=mod GOTOOLCHAIN=local GOPROXY="${GOPROXY:-https://goproxy.cn,direct}"
export CGO_ENABLED=1 GOOS=windows

# 必须静态链接：MinGW 动态链接会把 libstdc++-6.dll、libwinpthread-1.dll 挂成外部依赖，
# 拷到没装 MinGW 的机器上直接起不来。编完只剩 KERNEL32/WS2_32/msvcrt 三个系统 DLL。
for A in amd64 arm64; do
  OUT="$B/gopeed-shandian-$A.exe"
  echo "编译 $A"
  CC="$MINGW_BIN/gcc.exe" CXX="$MINGW_BIN/g++.exe" GOARCH=$A \
    go build -tags web -trimpath \
      -ldflags "-s -w -linkmode external -extldflags -static" -o "$OUT" ./cmd/web
  # 没链进 libutp 就是回落到了纯 Go uTP，BT 会明显变慢，宁可在这里失败
  if ! grep -aq "go-libutp" "$OUT"; then
    echo "$A 没编进 libutp，检查 CGO 与 MinGW"; exit 1
  fi
  imports=$("$MINGW_BIN/objdump.exe" -p "$OUT" | awk '/DLL Name:/ {print tolower($3)}')
  if [ -z "$imports" ] || grep -Eq '^lib(stdc\+\+|gcc|winpthread|ssp)' <<< "$imports"; then
    echo "$A 内核的静态运行库校验失败"; exit 1
  fi
  ls -lh "$OUT" | awk '{print "  产物", $NF, $5}'
done
echo "内核完成：$B/gopeed-shandian-amd64.exe"
