#!/usr/bin/env bash
# Package a macos-arm64 release tarball.
# Requires env: TARGET=aarch64-apple-darwin, VERSION=vX.Y.Z
set -euo pipefail

: "${TARGET:?TARGET env var required}"
: "${VERSION:?VERSION env var required}"

SQLD_VERSION="v0.24.32"
SQLD_ASSET="libsql-server-aarch64-apple-darwin.tar.xz"
SQLD_SHA256="ced2a9d65a5d4b6bd72c67e98ad6c63139e2a139d91769f07fdd15be935381dd"

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

STAGE="${WORK}/shoebox-server-${VERSION}-macos-arm64"
mkdir -p "${STAGE}/bin" "${STAGE}/share/launchd"

cp "target/${TARGET}/release/shoebox-server" "${STAGE}/bin/shoebox-server"
cp LICENSE "${STAGE}/LICENSE"

(
    cd "${WORK}"
    curl -fsSL -o "${SQLD_ASSET}" \
        "https://github.com/tursodatabase/libsql/releases/download/libsql-server-${SQLD_VERSION}/${SQLD_ASSET}"
    shasum -a 256 -c <(echo "${SQLD_SHA256}  ${SQLD_ASSET}")
    tar -xJf "${SQLD_ASSET}"
    cp "libsql-server-aarch64-apple-darwin/sqld" "${STAGE}/bin/sqld"
    chmod +x "${STAGE}/bin/sqld"
)

cat > "${STAGE}/share/launchd/com.shoebox.server.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.shoebox.server</string>
    <key>ProgramArguments</key>
    <array>
      <string>/opt/shoebox/bin/shoebox-server</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>SHOEBOX_SQLD_PATH</key><string>/opt/shoebox/bin/sqld</string>
        <key>SHOEBOX_DATA_DIR</key><string>/usr/local/var/shoebox</string>
        <key>SHOEBOX_PHOTOS_DIR</key><string>/Users/Shared/Photos</string>
        <key>SHOEBOX_CACHE_DIR</key><string>/usr/local/var/shoebox-cache</string>
    </dict>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>/usr/local/var/log/shoebox-server.log</string>
    <key>StandardErrorPath</key><string>/usr/local/var/log/shoebox-server.log</string>
</dict>
</plist>
PLIST

cat > "${STAGE}/share/config.example.toml" <<'TOML'
server_name = "shoebox"
bind_addr = "0.0.0.0:9000"
health_bind_addr = "127.0.0.1:9001"
data_dir = "/usr/local/var/shoebox"
photos_dir = "/Users/Shared/Photos"
cache_dir = "/usr/local/var/shoebox-cache"
extra_sans = []
# SHOEBOX_SECRET, SHOEBOX_SQLD_PATH, SHOEBOX_LOG via env.
TOML

cat > "${STAGE}/README.md" <<'MD'
# shoebox-server standalone (macOS Apple Silicon)

## Install (launchd, system-wide)

```bash
sudo mkdir -p /opt/shoebox /usr/local/var/shoebox /usr/local/var/shoebox-cache /usr/local/var/log
sudo cp -r bin /opt/shoebox/
sudo cp share/launchd/com.shoebox.server.plist /Library/LaunchDaemons/
sudo launchctl bootstrap system /Library/LaunchDaemons/com.shoebox.server.plist
```

Set `SHOEBOX_SECRET` inside the plist or via a wrapper script before bootstrapping.
MD

mkdir -p dist
TARBALL="dist/shoebox-server-${VERSION}-macos-arm64.tar.xz"
tar -C "${WORK}" -cJf "${TARBALL}" "shoebox-server-${VERSION}-macos-arm64"
( cd dist && shasum -a 256 "$(basename "${TARBALL}")" > "$(basename "${TARBALL}").sha256" )

echo "packaged: ${TARBALL}"
