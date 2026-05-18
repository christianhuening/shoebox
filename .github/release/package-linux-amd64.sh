#!/usr/bin/env bash
# Package a linux-amd64 release tarball.
# Requires env: TARGET=x86_64-unknown-linux-gnu, VERSION=vX.Y.Z
set -euo pipefail

: "${TARGET:?TARGET env var required (e.g. x86_64-unknown-linux-gnu)}"
: "${VERSION:?VERSION env var required (e.g. v0.1.0)}"

SQLD_VERSION="v0.24.32"
SQLD_ASSET="libsql-server-x86_64-unknown-linux-gnu.tar.xz"
SQLD_SHA256="71720fc8648c19efef416efebd47145ef59b62e198770533530a858e1336879f"

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

STAGE="${WORK}/shoebox-server-${VERSION}-linux-amd64"
mkdir -p "${STAGE}/bin" "${STAGE}/share/systemd" "${STAGE}/share/openrc"

cp "target/${TARGET}/release/shoebox-server" "${STAGE}/bin/shoebox-server"
cp deploy/systemd/shoebox-server.service "${STAGE}/share/systemd/"
cp deploy/openrc/shoebox-server "${STAGE}/share/openrc/shoebox-server"
chmod +x "${STAGE}/share/openrc/shoebox-server"
cp LICENSE "${STAGE}/LICENSE"

# Download + verify sqld
(
    cd "${WORK}"
    curl -fsSL -o "${SQLD_ASSET}" \
        "https://github.com/tursodatabase/libsql/releases/download/libsql-server-${SQLD_VERSION}/${SQLD_ASSET}"
    echo "${SQLD_SHA256}  ${SQLD_ASSET}" | sha256sum -c -
    tar -xJf "${SQLD_ASSET}"
    cp "libsql-server-x86_64-unknown-linux-gnu/sqld" "${STAGE}/bin/sqld"
    chmod +x "${STAGE}/bin/sqld"
)

# Render config.example.toml + tarball README
cat > "${STAGE}/share/config.example.toml" <<'TOML'
# Optional: place at /etc/shoebox/server.toml and point SHOEBOX_CONFIG at it.
# Every value below is overridable by the matching SHOEBOX_* environment variable.

server_name = "shoebox"             # SHOEBOX_SERVER_NAME
bind_addr = "0.0.0.0:9000"          # SHOEBOX_BIND_ADDR
health_bind_addr = "127.0.0.1:9001" # SHOEBOX_HEALTH_BIND_ADDR
data_dir = "/var/lib/shoebox"       # SHOEBOX_DATA_DIR
photos_dir = "/srv/photos"          # SHOEBOX_PHOTOS_DIR
cache_dir = "/var/cache/shoebox"    # SHOEBOX_CACHE_DIR
extra_sans = []                     # SHOEBOX_EXTRA_SANS (comma-separated)

# Not in this file — must come from env:
#   SHOEBOX_SECRET     bootstrap secret, e.g. `openssl rand -base64 24`
#   SHOEBOX_SQLD_PATH  defaults to `sqld` on PATH
#   SHOEBOX_LOG        e.g. "info", "debug"
TOML

cat > "${STAGE}/README.md" <<'MD'
# shoebox-server standalone

Linux amd64 build of shoebox-server with the matching sqld bundled.

## Install (systemd)

```bash
sudo useradd --system --home /var/lib/shoebox shoebox
sudo mkdir -p /opt/shoebox /etc/shoebox /var/lib/shoebox /var/cache/shoebox /srv/photos
sudo cp -r bin /opt/shoebox/
sudo cp share/systemd/shoebox-server.service /etc/systemd/system/
sudo chown -R shoebox:shoebox /var/lib/shoebox /var/cache/shoebox /srv/photos

# bootstrap secret (clients need this)
echo "SHOEBOX_SECRET=$(openssl rand -base64 24)" | sudo tee /etc/shoebox/shoebox.env
sudo chmod 600 /etc/shoebox/shoebox.env

sudo systemctl daemon-reload
sudo systemctl enable --now shoebox-server.service
```

Server listens on `:9000` (mTLS) and `:9001` (health on loopback only).
Read share/config.example.toml for the full env-var contract.
MD

# Tarball + sha256
mkdir -p dist
TARBALL="dist/shoebox-server-${VERSION}-linux-amd64.tar.xz"
tar -C "${WORK}" -cJf "${TARBALL}" "shoebox-server-${VERSION}-linux-amd64"
( cd dist && sha256sum "$(basename "${TARBALL}")" > "$(basename "${TARBALL}").sha256" )

echo "packaged: ${TARBALL}"
