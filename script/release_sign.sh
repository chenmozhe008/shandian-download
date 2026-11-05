#!/bin/bash
# release_sign.sh <App 路径>：用发布专用的自签名证书签名。
# 身份固定，系统才认得出是同一个 App，用户给的「辅助功能」授权更新后还在；临时签名每次更新都会失效。
# 证书和私钥只放本机 SHANDIAN_SIGN_DIR，不进仓库；丢了可重建，代价是用户要再授权一次。
set -euo pipefail
APP="$1"; BUNDLE_ID="io.github.chenmozhe008.shandian"
DIR="${SHANDIAN_SIGN_DIR:-$HOME/Library/Application Support/ShandianRelease}"
KC="$DIR/release.keychain-db"; PASS_FILE="$DIR/keychain-pass"
if [[ ! -f "$KC" ]]; then
  (umask 077; mkdir -p "$DIR"; openssl rand -hex 24 > "$PASS_FILE")
  WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
  cat > "$WORK/c.cnf" <<CNF
[req]
distinguished_name=dn
x509_extensions=v3
prompt=no
[dn]
CN=Shandian Download Manager Signing
[v3]
basicConstraints=critical,CA:false
keyUsage=critical,digitalSignature
extendedKeyUsage=critical,codeSigning
CNF
  openssl req -x509 -newkey rsa:2048 -nodes -keyout "$WORK/k.pem" -out "$WORK/c.pem" -days 7300 -config "$WORK/c.cnf" 2>/dev/null
  P12PASS="$(openssl rand -hex 12)"
  openssl pkcs12 -export -legacy -inkey "$WORK/k.pem" -in "$WORK/c.pem" -out "$WORK/c.p12" -passout "pass:$P12PASS" 2>/dev/null \
    || openssl pkcs12 -export -inkey "$WORK/k.pem" -in "$WORK/c.pem" -out "$WORK/c.p12" -passout "pass:$P12PASS"
  security create-keychain -p "$(cat "$PASS_FILE")" "$KC"
  security unlock-keychain -p "$(cat "$PASS_FILE")" "$KC"
  security import "$WORK/c.p12" -k "$KC" -P "$P12PASS" -T /usr/bin/codesign >/dev/null
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$(cat "$PASS_FILE")" "$KC" >/dev/null
fi
security unlock-keychain -p "$(cat "$PASS_FILE")" "$KC"
IDENTITY="$(security find-identity -p codesigning "$KC" | grep -m1 '1)' | awk '{print $2}')"
[[ -n "$IDENTITY" ]] || { echo "找不到发布证书" >&2; exit 1; }
codesign --force --keychain "$KC" --sign "$IDENTITY" --identifier "$BUNDLE_ID.mcp" --timestamp=none "$APP/Contents/MacOS/shandian-mcp"
codesign --force --keychain "$KC" --sign "$IDENTITY" --identifier "$BUNDLE_ID" --timestamp=none "$APP"
codesign --verify --strict "$APP"
codesign -d -r- "$APP" 2>&1 | tail -1
