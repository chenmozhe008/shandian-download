#!/bin/bash
# release_sign.sh <App 路径>：调用三个 App 共用的发布签名脚本（固定证书，用户的系统授权更新后还在）
set -euo pipefail
BUNDLE_ID="io.github.chenmozhe008.shandian"
exec "$HOME/Coding/tools/mac-app-workflow/release-sign.sh" "$1" "$BUNDLE_ID" --nested "Contents/MacOS/shandian-mcp:$BUNDLE_ID.mcp"
