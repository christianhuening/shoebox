# Quickstart — Standalone binary

For NASes without a container runtime, dedicated Mac mini hosts, or
operators who prefer systemd / launchd / OpenRC to Docker.

## Download

Pick the right tarball for your platform from
https://github.com/<owner>/shoebox/releases/tag/v0.1.0:

| Platform | Tarball |
|---|---|
| Linux amd64 (most x86 NASes, generic VMs) | `shoebox-server-v0.1.0-linux-amd64.tar.xz` |
| Linux arm64 (Synology DSx20+, RPi 4/5, ARM cloud) | `shoebox-server-v0.1.0-linux-arm64.tar.xz` |
| macOS Apple Silicon (Mac mini M1/M2/M3) | `shoebox-server-v0.1.0-macos-arm64.tar.xz` |

Verify the sha256:

```bash
# Linux:
sha256sum -c shoebox-server-v0.1.0-linux-amd64.tar.xz.sha256
# macOS:
shasum -a 256 -c shoebox-server-v0.1.0-macos-arm64.tar.xz.sha256
```
Expected: `OK`.

Extract:
```bash
tar -xJf shoebox-server-v0.1.0-linux-amd64.tar.xz
cd shoebox-server-v0.1.0-linux-amd64
```

The tarball contains:
- `bin/shoebox-server` — the server binary
- `bin/sqld` — the bundled sqld subprocess
- Linux: `share/systemd/shoebox-server.service` + `share/openrc/shoebox-server`
- macOS: `share/launchd/com.shoebox.server.plist`
- `share/config.example.toml` — full env-var contract
- `LICENSE`, `README.md`

## Install — Linux (systemd)

```bash
sudo useradd --system --home /var/lib/shoebox shoebox
sudo mkdir -p /opt/shoebox /etc/shoebox /var/lib/shoebox /var/cache/shoebox /srv/photos
sudo cp -r bin /opt/shoebox/
sudo cp share/systemd/shoebox-server.service /etc/systemd/system/
sudo chown -R shoebox:shoebox /var/lib/shoebox /var/cache/shoebox /srv/photos

# Bootstrap secret — clients need this
echo "SHOEBOX_SECRET=$(openssl rand -base64 24)" | sudo tee /etc/shoebox/shoebox.env
sudo chmod 600 /etc/shoebox/shoebox.env

# Optional: tweak paths in the unit file if /srv/photos isn't where your
# library lives. Or point SHOEBOX_PHOTOS_DIR at it via /etc/shoebox/shoebox.env.

sudo systemctl daemon-reload
sudo systemctl enable --now shoebox-server.service
sudo systemctl status shoebox-server.service
```

Health check:
```bash
curl http://127.0.0.1:9001/health    # expect: ok
```

Logs (systemd captures stdout/stderr to journald):
```bash
sudo journalctl -u shoebox-server -f
```

## Install — Linux (OpenRC, Alpine / Gentoo / OpenRC-based NAS)

```bash
sudo adduser -S -h /var/lib/shoebox shoebox    # busybox adduser syntax
sudo mkdir -p /opt/shoebox /etc/shoebox /etc/conf.d /var/lib/shoebox /var/cache/shoebox /srv/photos /var/log
sudo cp -r bin /opt/shoebox/
sudo install -m 0755 share/openrc/shoebox-server /etc/init.d/shoebox-server
sudo chown -R shoebox:shoebox /var/lib/shoebox /var/cache/shoebox /srv/photos

# Bootstrap secret + path overrides via conf.d
sudo tee /etc/conf.d/shoebox-server > /dev/null <<EOF
export SHOEBOX_SECRET="$(openssl rand -base64 24)"
export SHOEBOX_SQLD_PATH=/opt/shoebox/bin/sqld
export SHOEBOX_DATA_DIR=/var/lib/shoebox
export SHOEBOX_PHOTOS_DIR=/srv/photos
export SHOEBOX_CACHE_DIR=/var/cache/shoebox
EOF
sudo chmod 600 /etc/conf.d/shoebox-server

sudo rc-update add shoebox-server default
sudo rc-service shoebox-server start
sudo rc-service shoebox-server status
```

Health check + logs:
```bash
curl http://127.0.0.1:9001/health   # expect: ok
sudo tail -f /var/log/shoebox-server.log
```

## Install — macOS (launchd)

```bash
sudo mkdir -p /opt/shoebox /usr/local/var/shoebox /usr/local/var/shoebox-cache /usr/local/var/log
sudo cp -r bin /opt/shoebox/

# Edit the plist to embed SHOEBOX_SECRET before bootstrapping.
sudo cp share/launchd/com.shoebox.server.plist /Library/LaunchDaemons/
sudo vim /Library/LaunchDaemons/com.shoebox.server.plist
# Add inside <key>EnvironmentVariables</key><dict>:
#   <key>SHOEBOX_SECRET</key><string>...generated value...</string>

sudo launchctl bootstrap system /Library/LaunchDaemons/com.shoebox.server.plist
sudo launchctl print system/com.shoebox.server
```

Health check + logs:
```bash
curl http://127.0.0.1:9001/health
tail -f /usr/local/var/log/shoebox-server.log
```

## Sharing the bootstrap secret with clients

```bash
# Linux (systemd):
sudo grep ^SHOEBOX_SECRET= /etc/shoebox/shoebox.env | cut -d= -f2-

# Linux (OpenRC):
sudo grep SHOEBOX_SECRET /etc/conf.d/shoebox-server | sed -E 's/.*"(.+)"/\1/'

# macOS:
sudo plutil -p /Library/LaunchDaemons/com.shoebox.server.plist | grep SHOEBOX_SECRET
```

Hand that string to each desktop client during first-run enrollment.

## Upgrade

Stop the service, replace the binaries, start it again.

```bash
# Linux (systemd):
sudo systemctl stop shoebox-server
sudo cp ./bin/shoebox-server ./bin/sqld /opt/shoebox/bin/
sudo systemctl start shoebox-server

# Linux (OpenRC):
sudo rc-service shoebox-server stop
sudo cp ./bin/shoebox-server ./bin/sqld /opt/shoebox/bin/
sudo rc-service shoebox-server start

# macOS:
sudo launchctl bootout system/com.shoebox.server
sudo cp ./bin/shoebox-server ./bin/sqld /opt/shoebox/bin/
sudo launchctl bootstrap system /Library/LaunchDaemons/com.shoebox.server.plist
```

Migrations run automatically on startup. The catalog DB stays put in
`/var/lib/shoebox` (Linux) or `/usr/local/var/shoebox` (macOS).

## Configuration

All settings are env vars. Either put them in `/etc/shoebox/shoebox.env`
(loaded by the systemd unit), `/etc/conf.d/shoebox-server` (OpenRC), or
as `<key>...</key><string>...</string>` entries inside the launchd plist.

| Variable | Default | Notes |
|---|---|---|
| `SHOEBOX_SECRET` | _required_ | `openssl rand -base64 24` |
| `SHOEBOX_BIND_ADDR` | `0.0.0.0:9000` | mTLS port |
| `SHOEBOX_HEALTH_BIND_ADDR` | `127.0.0.1:9001` | health + metrics, loopback by default |
| `SHOEBOX_SERVER_NAME` | hostname | included in the issued server cert |
| `SHOEBOX_DATA_DIR` | (from unit/plist) | catalog.db lives here |
| `SHOEBOX_PHOTOS_DIR` | (from unit/plist) | source library |
| `SHOEBOX_CACHE_DIR` | (from unit/plist) | thumbnails |
| `SHOEBOX_SQLD_PATH` | (from unit/plist) | path to bundled sqld |
| `SHOEBOX_LOG` | `info` | `info`, `debug`, `trace` |
| `SHOEBOX_EXTRA_SANS` | _empty_ | comma-separated extra DNS SANs on the server cert |
