# Sub-project 1.5 — Deployment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `shoebox-server` v0.1.0 through three production deployment paths — multi-arch Docker image on ghcr.io, GitHub Releases tarballs (linux-amd64, linux-arm64, macos-arm64), and a minimal single-replica Helm chart — plus compose example, systemd unit, three quickstart docs, and the CI workflows that publish and lint them.

**Architecture:** Pure infrastructure / configuration work. No Rust changes. New `deploy/` tree holds compose, helm, and systemd artifacts; new `docs/deployment/` tree holds quickstarts; new `.github/release/` shell scripts package per-target tarballs; new `.github/workflows/{release,helm-lint}.yml` drives CI. The existing `Dockerfile` is rewritten in place to be arch-aware via `TARGETARCH`. The existing `.github/workflows/ci.yml` gains a compose-smoke job and a binary-smoke job.

**Tech Stack:** Docker (buildx, multi-arch), Helm 3, GitHub Actions (`docker/build-push-action`, `softprops/action-gh-release`, `azure/setup-helm`), `cross-rs/cross` for linux-arm64 cross-compile, bash for packagers, markdown for docs.

**Spec:** `docs/superpowers/specs/2026-05-18-sub-1-5-deployment-design.md` (committed in `155c69e`).

**Commit policy:** All commits unsigned (`-c commit.gpgsign=false`) for this run — 1Password signing was failing in the prior session. Work on `main` branch (established Plan 1.3/1.4/1.4b pattern).

---

## File Map

### Files modified
- `Dockerfile` — sqld download becomes arch-aware (Task 1)
- `.github/workflows/ci.yml` — add `compose-smoke` and `binary-smoke` jobs (Tasks 3, 6)
- `README.md` — add Deployment section (Task 20)
- `CLAUDE.md` — record Plan 1.5 implementation status (Task 21)

### Files created
| Path | Owner task |
|---|---|
| `deploy/compose/docker-compose.yml` | 2 |
| `deploy/compose/.env.example` | 2 |
| `deploy/compose/README.md` | 2 |
| `deploy/systemd/shoebox-server.service` | 4 |
| `.github/release/package-linux-amd64.sh` | 5 |
| `.github/release/package-linux-arm64.sh` | 5 |
| `.github/release/package-macos-arm64.sh` | 5 |
| `deploy/helm/shoebox/Chart.yaml` | 7 |
| `deploy/helm/shoebox/values.yaml` | 7 |
| `deploy/helm/shoebox/values.schema.json` | 7 |
| `deploy/helm/shoebox/README.md` | 7 |
| `deploy/helm/shoebox/templates/_helpers.tpl` | 8 |
| `deploy/helm/shoebox/templates/secret.yaml` | 9 |
| `deploy/helm/shoebox/templates/pvc.yaml` | 10 |
| `deploy/helm/shoebox/templates/service.yaml` | 11 |
| `deploy/helm/shoebox/templates/deployment.yaml` | 12 |
| `deploy/helm/shoebox/templates/NOTES.txt` | 13 |
| `deploy/helm/shoebox/ci/values-cache-on.yaml` | 14 |
| `deploy/helm/shoebox/ci/golden-defaults.yaml` | 14 |
| `deploy/helm/shoebox/ci/golden-cache-on.yaml` | 14 |
| `.github/workflows/helm-lint.yml` | 15 |
| `.github/workflows/release.yml` | 16 |
| `docs/deployment/quickstart-docker.md` | 17 |
| `docs/deployment/quickstart-binary.md` | 18 |
| `docs/deployment/quickstart-kubernetes.md` | 19 |

---

## Pre-flight (do once before Task 1)

Verify these tools are available on the dev machine; install if not:
```bash
docker --version          # ≥ 24.0 for buildx
helm version              # ≥ 3.13
shellcheck --version      # for shell script linting (Task 5)
```

Optional: `actionlint` for workflow validation (Tasks 15, 16). If not installed, skip the actionlint steps — `helm-lint.yml` and `release.yml` get validated on first push to GitHub.

---

## Task 1: Multi-arch `Dockerfile`

**Files:**
- Modify: `Dockerfile` (lines 35-45 — the sqld download block)

- [ ] **Step 1: Look up and verify the arm64 sqld sha256**

The amd64 sha256 is already pinned to `71720fc8648c19efef416efebd47145ef59b62e198770533530a858e1336879f` for `libsql-server-x86_64-unknown-linux-gnu.tar.xz` from release `libsql-server-v0.24.32`.

For arm64, download the asset and compute the sha256:
```bash
cd /tmp
wget -q "https://github.com/tursodatabase/libsql/releases/download/libsql-server-v0.24.32/libsql-server-aarch64-unknown-linux-gnu.tar.xz"
sha256sum libsql-server-aarch64-unknown-linux-gnu.tar.xz
```
Record the hash (call it `<ARM64_SHA>` below).

- [ ] **Step 2: Rewrite the sqld block in `Dockerfile`**

Replace lines 31-45 of the existing `Dockerfile` with:

```dockerfile
# Install sqld so shoebox-server can spawn it as a subprocess (Plan 1.3).
# Pin a specific release for reproducibility; verify against the published sha256.
# Per-arch URLs and sums are selected via TARGETARCH at buildx time.
ARG SQLD_VERSION=v0.24.32
ARG SQLD_AMD64_SHA256=71720fc8648c19efef416efebd47145ef59b62e198770533530a858e1336879f
ARG SQLD_ARM64_SHA256=<ARM64_SHA>
ARG TARGETARCH
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64) sqld_target=x86_64-unknown-linux-gnu;  sha=${SQLD_AMD64_SHA256} ;; \
      arm64) sqld_target=aarch64-unknown-linux-gnu; sha=${SQLD_ARM64_SHA256} ;; \
      *) echo "unsupported TARGETARCH=${TARGETARCH}"; exit 1 ;; \
    esac; \
    cd /tmp; \
    asset="libsql-server-${sqld_target}.tar.xz"; \
    wget -q "https://github.com/tursodatabase/libsql/releases/download/libsql-server-${SQLD_VERSION}/${asset}"; \
    echo "${sha}  ${asset}" | sha256sum -c -; \
    tar -xJf "${asset}"; \
    mv "libsql-server-${sqld_target}/sqld" /usr/local/bin/sqld; \
    chmod +x /usr/local/bin/sqld; \
    rm -rf "${asset}" "libsql-server-${sqld_target}"
```

Substitute `<ARM64_SHA>` with the value from Step 1.

- [ ] **Step 3: Verify native (amd64) build still works**

```bash
docker build -t shoebox-server:plan15-task1 .
```
Expected: successful build. Note the image still builds for the host arch only (no buildx flag), which validates the case branch is reachable.

- [ ] **Step 4: Verify arm64 build via buildx + QEMU**

```bash
docker buildx create --use --name shoebox-multiarch 2>/dev/null || docker buildx use shoebox-multiarch
docker buildx build --platform linux/arm64 -t shoebox-server:plan15-task1-arm64 --load .
```
Expected: successful build that downloads the arm64 sqld and verifies the sha256.

- [ ] **Step 5: Commit**

```bash
git add Dockerfile
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
build(docker): make sqld download arch-aware (amd64 + arm64)

TARGETARCH selects the right libsql-server tarball + sha256 at buildx
time. Native (amd64) build works as before; arm64 build via buildx
verified locally with QEMU.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Compose example (`deploy/compose/`)

**Files:**
- Create: `deploy/compose/docker-compose.yml`
- Create: `deploy/compose/.env.example`
- Create: `deploy/compose/README.md`

- [ ] **Step 1: Create the directory**

```bash
mkdir -p deploy/compose
```

- [ ] **Step 2: Write `deploy/compose/docker-compose.yml`**

```yaml
# shoebox-server single-node compose deployment.
# See deploy/compose/README.md for the 4-step setup.

services:
  shoebox-server:
    image: ghcr.io/CHANGE-ME-OWNER/shoebox-server:v0.1.0
    container_name: shoebox-server
    restart: unless-stopped
    ports:
      - "9000:9000"                  # mTLS — exposed to LAN
      - "127.0.0.1:9001:9001"        # health + metrics — loopback only
    volumes:
      - shoebox-data:/var/lib/shoebox
      - shoebox-cache:/shoebox-cache
      - ${SHOEBOX_PHOTOS_DIR:?set SHOEBOX_PHOTOS_DIR in .env}:/photos
    env_file: .env
    healthcheck:
      test: ["CMD", "wget", "-qO-", "http://127.0.0.1:9001/health"]
      interval: 30s
      timeout: 3s
      retries: 3

volumes:
  shoebox-data:
  shoebox-cache:
```

The literal string `CHANGE-ME-OWNER` is on purpose; the quickstart docs (Task 17) tell operators to replace it.

- [ ] **Step 3: Write `deploy/compose/.env.example`**

```env
# Required: bootstrap secret. Generate with:
#   openssl rand -base64 24
SHOEBOX_SECRET=

# Required: absolute host path to the photos library.
SHOEBOX_PHOTOS_DIR=/srv/photos

# Optional: log level (info | debug | trace).
SHOEBOX_LOG=info
```

- [ ] **Step 4: Write `deploy/compose/README.md`**

````markdown
# Shoebox — Compose deployment

Single-box deployment of `shoebox-server` via Docker Compose. Suitable for
NASes with a Docker runtime, home servers, and dedicated boxes.

## Setup (4 steps)

```bash
# 1. Copy the env template
cp .env.example .env

# 2. Generate a bootstrap secret and paste it into .env
echo "SHOEBOX_SECRET=$(openssl rand -base64 24)" >> .env

# 3. Edit .env: set SHOEBOX_PHOTOS_DIR to the host path of your library
$EDITOR .env

# 4. Start the server
docker compose up -d
```

The server is now listening on port 9000 (mTLS). Health + Prometheus
metrics are on `127.0.0.1:9001` (loopback only).

## Sharing the bootstrap secret with clients

Print it back from `.env`:

```bash
grep ^SHOEBOX_SECRET= .env | cut -d= -f2-
```

Hand that string to each desktop client during its first-run enrollment.

## Upgrade

```bash
docker compose pull
docker compose up -d
```

Server schema migrations run automatically on startup. Backups (every 6h,
14-snapshot rotation) are written into the `shoebox-data` volume.

## Volumes

| Mount | Purpose | Persistence |
|---|---|---|
| `shoebox-data` | catalog.db, secret, internal CA | **must persist** |
| `shoebox-cache` | thumbnails | safe to drop; will rebuild |
| `${SHOEBOX_PHOTOS_DIR}` | source photo library (host bind) | operator-managed |
````

- [ ] **Step 5: Lint the compose file**

```bash
docker compose -f deploy/compose/docker-compose.yml config --quiet
```
Expected: warning about missing `.env` but exit code 0 (compose tolerates missing env vars at `config` time when their consumers also use `:?` or `:-` defaults — for our `:?` consumer it errors. So do this instead):

```bash
SHOEBOX_PHOTOS_DIR=/tmp docker compose -f deploy/compose/docker-compose.yml config --quiet
```
Expected: exit code 0, no output.

- [ ] **Step 6: Commit**

```bash
git add deploy/compose/
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
feat(deploy): add compose example for single-box deployment

docker-compose.yml + .env.example + README walking through the 4-step
NAS setup. Image tag references CHANGE-ME-OWNER as a placeholder; the
quickstart-docker doc points operators at the right value.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: CI `compose-smoke` job

**Files:**
- Modify: `.github/workflows/ci.yml` (append a new job after the existing `docker` job)

- [ ] **Step 1: Append the new job to `.github/workflows/ci.yml`**

Add at the end of the file (after the existing `docker` job):

```yaml
  compose-smoke:
    runs-on: ubuntu-latest
    needs: docker
    steps:
      - uses: actions/checkout@v4
      - name: Build image
        run: docker build -t shoebox-server:smoke .
      - name: Prep env
        working-directory: deploy/compose
        run: |
          cp .env.example .env
          echo "SHOEBOX_SECRET=$(openssl rand -base64 24)" >> .env
          PHOTOS_DIR=$(mktemp -d)
          echo "SHOEBOX_PHOTOS_DIR=${PHOTOS_DIR}" >> .env
          # Pin the locally-built image so we don't pull from ghcr.
          sed -i 's|image: ghcr.io/CHANGE-ME-OWNER/shoebox-server:v0.1.0|image: shoebox-server:smoke|' docker-compose.yml
      - name: Compose up
        working-directory: deploy/compose
        run: docker compose up -d
      - name: Wait for /health
        run: |
          for i in $(seq 1 30); do
            if curl -fsS http://127.0.0.1:9001/health >/dev/null; then
              echo "healthy after ${i}s"; exit 0
            fi
            sleep 1
          done
          echo "server never became healthy"
          docker compose -f deploy/compose/docker-compose.yml logs
          exit 1
      - name: Compose down
        if: always()
        working-directory: deploy/compose
        run: docker compose down -v
```

- [ ] **Step 2: Validate the workflow syntactically (optional if actionlint installed)**

```bash
actionlint .github/workflows/ci.yml
```
Expected: no output. If actionlint isn't installed, skip — GitHub will validate on push.

- [ ] **Step 3: Run the smoke locally to make sure it actually works**

Reproduce the job's body locally:

```bash
docker build -t shoebox-server:smoke .
cd deploy/compose
cp .env.example .env
echo "SHOEBOX_SECRET=$(openssl rand -base64 24)" >> .env
PHOTOS_DIR=$(mktemp -d)
echo "SHOEBOX_PHOTOS_DIR=${PHOTOS_DIR}" >> .env
sed -i 's|image: ghcr.io/CHANGE-ME-OWNER/shoebox-server:v0.1.0|image: shoebox-server:smoke|' docker-compose.yml
docker compose up -d
for i in $(seq 1 30); do
  if curl -fsS http://127.0.0.1:9001/health >/dev/null; then echo "healthy after ${i}s"; break; fi
  sleep 1
done
docker compose down -v
# Restore the docker-compose.yml change before committing
git checkout docker-compose.yml
cd ../..
```
Expected: "healthy after Ns" within ~30 seconds. Clean teardown.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
ci: add compose-smoke job

Builds the image, runs deploy/compose with a synthetic SHOEBOX_SECRET
and a tmpdir for photos, waits for :9001/health, tears down. Catches
regressions in either the Dockerfile or the compose file.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: systemd + OpenRC units

**Files:**
- Create: `deploy/systemd/shoebox-server.service`
- Create: `deploy/openrc/shoebox-server`

Both ship in the Linux release tarballs (Task 5) and are also discoverable in the repo for operators bootstrapping by hand.

- [ ] **Step 1: Create the directories**

```bash
mkdir -p deploy/systemd deploy/openrc
```

- [ ] **Step 2: Write `deploy/systemd/shoebox-server.service`**

```ini
[Unit]
Description=shoebox server
Documentation=https://github.com/CHANGE-ME-OWNER/shoebox
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=shoebox
Group=shoebox
ExecStart=/opt/shoebox/bin/shoebox-server
Environment=SHOEBOX_SQLD_PATH=/opt/shoebox/bin/sqld
Environment=SHOEBOX_DATA_DIR=/var/lib/shoebox
Environment=SHOEBOX_PHOTOS_DIR=/srv/photos
Environment=SHOEBOX_CACHE_DIR=/var/cache/shoebox
EnvironmentFile=-/etc/shoebox/shoebox.env
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/shoebox /var/cache/shoebox /srv/photos
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

- [ ] **Step 3: Write `deploy/openrc/shoebox-server`**

```bash
#!/sbin/openrc-run
# OpenRC init script for shoebox-server. Ships in linux tarballs under
# share/openrc/ for Alpine, Gentoo, and OpenRC-based NAS distros.

name="shoebox-server"
description="shoebox catalog server"
command="/opt/shoebox/bin/shoebox-server"
command_background=true
command_user="shoebox:shoebox"
pidfile="/run/${name}.pid"
output_log="/var/log/${name}.log"
error_log="/var/log/${name}.log"

# Operators set SHOEBOX_SECRET and overrides in /etc/conf.d/shoebox-server.
# OpenRC sources that file automatically before launch.

depend() {
    need net
    after firewall
}
```

A companion `/etc/conf.d/shoebox-server` (operator-supplied, documented in the binary quickstart) holds:

```
export SHOEBOX_SECRET="..."
export SHOEBOX_DATA_DIR="/var/lib/shoebox"
export SHOEBOX_PHOTOS_DIR="/srv/photos"
export SHOEBOX_CACHE_DIR="/var/cache/shoebox"
export SHOEBOX_SQLD_PATH="/opt/shoebox/bin/sqld"
```

- [ ] **Step 4: Verify the systemd unit parses**

If `systemd-analyze` is available on the dev box:
```bash
systemd-analyze verify deploy/systemd/shoebox-server.service
```
Expected: no errors. (Warnings about missing User/Group/ExecStart paths are fine — those don't exist on the dev box.)

If `systemd-analyze` is not available, skip — the file is exercised in the binary quickstart at Task 18.

- [ ] **Step 5: Shellcheck the OpenRC script**

```bash
shellcheck -s sh deploy/openrc/shoebox-server
```
Expected: clean. If shellcheck complains about `command_background=true` style (it's OpenRC DSL, not pure sh), accept and move on.

- [ ] **Step 6: Commit**

```bash
git add deploy/systemd/shoebox-server.service deploy/openrc/shoebox-server
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
feat(deploy): add systemd + OpenRC units for standalone-binary deployments

Both ship in the GH Releases tarballs (systemd under share/systemd/,
OpenRC under share/openrc/) and are also discoverable in the repo for
operators bootstrapping by hand. OpenRC covers Alpine, Gentoo, and
OpenRC-based NAS distros.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Per-target packager scripts

**Files:**
- Create: `.github/release/package-linux-amd64.sh`
- Create: `.github/release/package-linux-arm64.sh`
- Create: `.github/release/package-macos-arm64.sh`

All three scripts are tiny variations on the same template. They expect these env vars set by the caller (Task 16's release.yml binary job):
- `TARGET` — Rust target triple (e.g. `x86_64-unknown-linux-gnu`)
- `VERSION` — Tag name (e.g. `v0.1.0`)

Each script reads the matching `shoebox-server` binary from `target/${TARGET}/release/`, downloads the matching upstream `sqld`, sha256-verifies it, assembles a tarball under `dist/`, and writes a sidecar sha256.

- [ ] **Step 1: Create the directory**

```bash
mkdir -p .github/release
```

- [ ] **Step 2: Write `.github/release/package-linux-amd64.sh`**

```bash
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
```

- [ ] **Step 3: Write `.github/release/package-linux-arm64.sh`**

Copy the amd64 script verbatim, then apply these substitutions:

| Variable / line | amd64 value | arm64 value |
|---|---|---|
| `SQLD_ASSET` | `libsql-server-x86_64-unknown-linux-gnu.tar.xz` | `libsql-server-aarch64-unknown-linux-gnu.tar.xz` |
| `SQLD_SHA256` | (pinned amd64) | `<ARM64_SHA from Task 1 Step 1>` |
| `cp "libsql-server-x86_64-unknown-linux-gnu/sqld" ...` | (current) | `cp "libsql-server-aarch64-unknown-linux-gnu/sqld" ...` |
| `STAGE=...linux-amd64` | `linux-amd64` | `linux-arm64` (everywhere it appears, including the final `tar -C "${WORK}" -cJf` line) |
| `TARBALL=dist/...linux-amd64.tar.xz` | `linux-amd64` | `linux-arm64` |

The `share/systemd/` and `share/openrc/` copies, the `share/config.example.toml` HEREDOC, and the `README.md` HEREDOC are byte-identical to the amd64 ones — both Linux tarballs ship the same init-system artifacts and the same README copy.

- [ ] **Step 4: Write `.github/release/package-macos-arm64.sh`**

Same template, but:
- Drops the systemd unit (use launchd plist instead).
- Different sqld asset name and sha256 (compute and verify the macos-aarch64 sha at script-write time the same way Task 1 Step 1 did for linux-arm64).
- Different README content.

```bash
#!/usr/bin/env bash
# Package a macos-arm64 release tarball.
# Requires env: TARGET=aarch64-apple-darwin, VERSION=vX.Y.Z
set -euo pipefail

: "${TARGET:?TARGET env var required}"
: "${VERSION:?VERSION env var required}"

SQLD_VERSION="v0.24.32"
SQLD_ASSET="libsql-server-aarch64-apple-darwin.tar.xz"
SQLD_SHA256="<MACOS_ARM64_SHA — verify from upstream release>"

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
```

Replace `<MACOS_ARM64_SHA — verify from upstream release>` with the actual sha256 you verify by downloading `libsql-server-aarch64-apple-darwin.tar.xz` from the upstream release page.

- [ ] **Step 5: Mark all three scripts executable**

```bash
chmod +x .github/release/package-linux-amd64.sh \
         .github/release/package-linux-arm64.sh \
         .github/release/package-macos-arm64.sh
```

- [ ] **Step 6: Shellcheck**

```bash
shellcheck .github/release/package-linux-amd64.sh \
           .github/release/package-linux-arm64.sh \
           .github/release/package-macos-arm64.sh
```
Expected: clean. If shellcheck flags `SC2086` for any `${VAR}` you're confident is safe, accept and move on; otherwise fix.

- [ ] **Step 7: Smoke-test the linux-amd64 packager locally**

```bash
cargo build --release -p shoebox-server   # populates target/release/shoebox-server
# The packager expects target/${TARGET}/release/, so symlink for the test:
mkdir -p target/x86_64-unknown-linux-gnu/release
cp target/release/shoebox-server target/x86_64-unknown-linux-gnu/release/shoebox-server

TARGET=x86_64-unknown-linux-gnu VERSION=v0.0.0-dev \
  bash .github/release/package-linux-amd64.sh

ls dist/
tar -tJf dist/shoebox-server-v0.0.0-dev-linux-amd64.tar.xz | head -20
```
Expected: tarball + sha256 in `dist/`. `tar -tJf` lists `shoebox-server-v0.0.0-dev-linux-amd64/bin/{shoebox-server,sqld}`, `share/systemd/shoebox-server.service`, `share/config.example.toml`, `LICENSE`, `README.md`.

- [ ] **Step 8: Clean up the smoke-test artifacts**

```bash
rm -rf dist/ target/x86_64-unknown-linux-gnu/
```

- [ ] **Step 9: Commit**

```bash
git add .github/release/
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
feat(release): add per-target tarball packagers

Three small bash scripts (linux-amd64, linux-arm64, macos-arm64) that
assemble shoebox-server + bundled sqld + share/ extras into a sha256-
verified tarball under dist/. Consumed by the release.yml binary
matrix job.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: CI `binary-smoke` job

**Files:**
- Modify: `.github/workflows/ci.yml` (append new job)

- [ ] **Step 1: Append the new job to `.github/workflows/ci.yml`**

```yaml
  binary-smoke:
    runs-on: ubuntu-latest
    needs: test
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { toolchain: 1.85.0, targets: x86_64-unknown-linux-gnu }
      - uses: Swatinem/rust-cache@v2
      - name: Build release binary
        run: cargo build --release --target x86_64-unknown-linux-gnu -p shoebox-server
      - name: Package tarball
        env:
          TARGET: x86_64-unknown-linux-gnu
          VERSION: v0.0.0-ci
        run: bash .github/release/package-linux-amd64.sh
      - name: Extract
        run: |
          mkdir -p smoke
          tar -xJf dist/shoebox-server-v0.0.0-ci-linux-amd64.tar.xz -C smoke
      - name: Run + health check
        run: |
          export SHOEBOX_SECRET=$(openssl rand -base64 24)
          export SHOEBOX_DATA_DIR=$(mktemp -d)
          export SHOEBOX_PHOTOS_DIR=$(mktemp -d)
          export SHOEBOX_CACHE_DIR=$(mktemp -d)
          export SHOEBOX_SQLD_PATH=$(pwd)/smoke/shoebox-server-v0.0.0-ci-linux-amd64/bin/sqld
          ./smoke/shoebox-server-v0.0.0-ci-linux-amd64/bin/shoebox-server &
          SERVER_PID=$!
          for i in $(seq 1 30); do
            if curl -fsS http://127.0.0.1:9001/health >/dev/null; then
              echo "healthy after ${i}s"
              kill ${SERVER_PID}
              wait ${SERVER_PID} 2>/dev/null || true
              exit 0
            fi
            sleep 1
          done
          echo "server never became healthy"
          kill ${SERVER_PID} 2>/dev/null || true
          exit 1
```

- [ ] **Step 2: Run shellcheck on the embedded script body (light check)**

The job's inline bash is small; visually inspect for `SC2086`-style issues.

- [ ] **Step 3: Actionlint (optional)**

```bash
actionlint .github/workflows/ci.yml
```
Expected: clean.

- [ ] **Step 4: Reproduce the smoke locally**

```bash
cargo build --release --target x86_64-unknown-linux-gnu -p shoebox-server
TARGET=x86_64-unknown-linux-gnu VERSION=v0.0.0-ci \
  bash .github/release/package-linux-amd64.sh
mkdir -p smoke && tar -xJf dist/shoebox-server-v0.0.0-ci-linux-amd64.tar.xz -C smoke
export SHOEBOX_SECRET=$(openssl rand -base64 24)
export SHOEBOX_DATA_DIR=$(mktemp -d)
export SHOEBOX_PHOTOS_DIR=$(mktemp -d)
export SHOEBOX_CACHE_DIR=$(mktemp -d)
export SHOEBOX_SQLD_PATH=$(pwd)/smoke/shoebox-server-v0.0.0-ci-linux-amd64/bin/sqld
./smoke/shoebox-server-v0.0.0-ci-linux-amd64/bin/shoebox-server &
SERVER_PID=$!
sleep 5
curl -fsS http://127.0.0.1:9001/health && echo "OK"
kill ${SERVER_PID}; wait ${SERVER_PID} 2>/dev/null || true
rm -rf smoke dist target/x86_64-unknown-linux-gnu
```
Expected: `OK`.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
ci: add binary-smoke job for linux-amd64

Builds release tarball via package-linux-amd64.sh, extracts, runs the
binary against synthetic SHOEBOX_* env, hits /health. arm64 + macos
targets only get build-only validation in release.yml (running them
requires hardware).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Helm chart skeleton

**Files:**
- Create: `deploy/helm/shoebox/Chart.yaml`
- Create: `deploy/helm/shoebox/values.yaml`
- Create: `deploy/helm/shoebox/values.schema.json`
- Create: `deploy/helm/shoebox/README.md`

- [ ] **Step 1: Create the chart directory**

```bash
mkdir -p deploy/helm/shoebox/templates deploy/helm/shoebox/ci
```

- [ ] **Step 2: Write `deploy/helm/shoebox/Chart.yaml`**

```yaml
apiVersion: v2
name: shoebox
description: Single-replica shoebox catalog server with embedded sqld.
type: application
version: 0.1.0
appVersion: "0.1.0"
home: https://github.com/CHANGE-ME-OWNER/shoebox
sources:
  - https://github.com/CHANGE-ME-OWNER/shoebox
maintainers:
  - name: shoebox-maintainers
kubeVersion: ">=1.25.0-0"
```

- [ ] **Step 3: Write `deploy/helm/shoebox/values.yaml`**

```yaml
image:
  repository: ghcr.io/CHANGE-ME-OWNER/shoebox-server
  tag: ""                    # defaults to .Chart.AppVersion
  pullPolicy: IfNotPresent
  pullSecrets: []

secret:
  create: true               # auto-generate a SHOEBOX_SECRET on first install
  existingSecret: ""         # name of pre-created Secret (mutually exclusive with create)
  key: SHOEBOX_SECRET        # key inside the Secret

storage:
  data:
    size: 1Gi
    storageClassName: ""
    accessMode: ReadWriteOnce
  cache:
    enabled: false           # true => dedicated PVC; false => emptyDir
    size: 10Gi
    storageClassName: ""
    accessMode: ReadWriteOnce
  photos:
    existingClaim: ""        # operator-supplied PVC; exclusive with hostPath
    hostPath: ""             # exclusive with existingClaim
    subPath: ""

service:
  type: ClusterIP
  ports:
    mtls: 9000
    health: 9001

resources: {}                # operator-supplied; no defaults

podSecurityContext:
  fsGroup: 10001
  runAsNonRoot: true
  runAsUser: 10001
  runAsGroup: 10001

securityContext:
  allowPrivilegeEscalation: false
  capabilities:
    drop: ["ALL"]
  readOnlyRootFilesystem: true

nodeSelector: {}
tolerations: []
affinity: {}

extraEnv: []                 # e.g. [{ name: SHOEBOX_LOG, value: debug }]
```

- [ ] **Step 4: Write `deploy/helm/shoebox/values.schema.json`**

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "required": ["image", "secret", "storage", "service"],
  "properties": {
    "image": {
      "type": "object",
      "required": ["repository"],
      "properties": {
        "repository": { "type": "string", "minLength": 1 },
        "tag": { "type": "string" },
        "pullPolicy": { "type": "string", "enum": ["Always", "IfNotPresent", "Never"] },
        "pullSecrets": { "type": "array", "items": { "type": "object" } }
      }
    },
    "secret": {
      "type": "object",
      "properties": {
        "create": { "type": "boolean" },
        "existingSecret": { "type": "string" },
        "key": { "type": "string", "minLength": 1 }
      }
    },
    "storage": {
      "type": "object",
      "required": ["data", "cache", "photos"],
      "properties": {
        "data": {
          "type": "object",
          "properties": {
            "size": { "type": "string" },
            "storageClassName": { "type": "string" },
            "accessMode": { "type": "string" }
          }
        },
        "cache": {
          "type": "object",
          "properties": {
            "enabled": { "type": "boolean" },
            "size": { "type": "string" },
            "storageClassName": { "type": "string" },
            "accessMode": { "type": "string" }
          }
        },
        "photos": {
          "type": "object",
          "properties": {
            "existingClaim": { "type": "string" },
            "hostPath": { "type": "string" },
            "subPath": { "type": "string" }
          }
        }
      }
    },
    "service": {
      "type": "object",
      "required": ["type", "ports"],
      "properties": {
        "type": { "type": "string" },
        "ports": {
          "type": "object",
          "required": ["mtls", "health"],
          "properties": {
            "mtls": { "type": "integer" },
            "health": { "type": "integer" }
          }
        }
      }
    },
    "resources": { "type": "object" },
    "podSecurityContext": { "type": "object" },
    "securityContext": { "type": "object" },
    "nodeSelector": { "type": "object" },
    "tolerations": { "type": "array" },
    "affinity": { "type": "object" },
    "extraEnv": { "type": "array" }
  }
}
```

Note: JSON schema can express `oneOf` for the create-vs-existingSecret and existingClaim-vs-hostPath constraints, but those go inside `_helpers.tpl` as `fail` calls instead (Task 8) so the error messages are more useful.

- [ ] **Step 5: Write `deploy/helm/shoebox/README.md`**

````markdown
# shoebox Helm chart

Single-replica `shoebox-server` for Kubernetes ≥ 1.25.

## Quick install

```bash
helm install shoebox ./shoebox \
  --set image.repository=ghcr.io/<owner>/shoebox-server \
  --set storage.photos.existingClaim=my-photos-pvc
```

The chart auto-generates `SHOEBOX_SECRET` on first install. Retrieve it
afterwards:

```bash
kubectl get secret shoebox-bootstrap \
  -o jsonpath='{.data.SHOEBOX_SECRET}' | base64 -d
```

Share that string with each desktop client during enrollment.

## Values

See `values.yaml` for the full surface. Constraints:

- Either `secret.create=true` **or** `secret.existingSecret` non-empty.
- Either `storage.photos.existingClaim` **or** `storage.photos.hostPath`.

Chart install fails fast with a clear message if either constraint is
violated.

## Uninstall

```bash
helm uninstall shoebox
# The bootstrap Secret is kept (resource-policy: keep) so existing
# client certs continue to authenticate after reinstall.
# Drop it manually if you really want a fresh start:
kubectl delete secret shoebox-bootstrap
```

## What's NOT in the chart

By design (backlog, see spec §12):
- Ingress (mTLS terminates at the server; expose via Service + LB/VPN).
- NetworkPolicy.
- ServiceMonitor (use a manual PodMonitor or scrape config against :9001).
- HPA (single replica only in v0.x).
````

- [ ] **Step 6: `helm lint`**

```bash
helm lint deploy/helm/shoebox
```
Expected: warnings about missing templates (since we haven't written them yet). The chart itself is structurally valid; we'll re-lint after each template task. Acceptable to see "1 chart(s) linted, 1 chart(s) failed" if it's only complaining about no templates yet.

- [ ] **Step 7: Commit**

```bash
git add deploy/helm/shoebox/Chart.yaml \
        deploy/helm/shoebox/values.yaml \
        deploy/helm/shoebox/values.schema.json \
        deploy/helm/shoebox/README.md
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
feat(deploy): scaffold Helm chart skeleton

Chart.yaml + values.yaml + values.schema.json + README. Templates land
in the next tasks. README documents the photos-PVC-vs-hostPath and
secret-create-vs-existing constraints (enforced via _helpers.tpl in
Task 8).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Helm `_helpers.tpl`

**Files:**
- Create: `deploy/helm/shoebox/templates/_helpers.tpl`

- [ ] **Step 1: Write the helpers**

```yaml
{{/* Fully qualified release name. */}}
{{- define "shoebox.fullname" -}}
{{- printf "%s-%s" .Release.Name .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/* Chart label. */}}
{{- define "shoebox.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/* Common labels block. */}}
{{- define "shoebox.labels" -}}
helm.sh/chart: {{ include "shoebox.chart" . }}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end -}}

{{/* Selector labels (must match deployment & service). */}}
{{- define "shoebox.selectorLabels" -}}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{/* Name of the Secret holding SHOEBOX_SECRET. */}}
{{- define "shoebox.secretName" -}}
{{- if .Values.secret.existingSecret -}}
{{- .Values.secret.existingSecret -}}
{{- else -}}
{{- printf "%s-bootstrap" (include "shoebox.fullname" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{/* Validate the secret config. */}}
{{- define "shoebox.validateSecret" -}}
{{- if and .Values.secret.create .Values.secret.existingSecret -}}
{{- fail "secret.create=true and secret.existingSecret are mutually exclusive — set one or the other." -}}
{{- end -}}
{{- if and (not .Values.secret.create) (not .Values.secret.existingSecret) -}}
{{- fail "Either secret.create=true or secret.existingSecret must be set." -}}
{{- end -}}
{{- end -}}

{{/* Validate the photos volume config. */}}
{{- define "shoebox.validatePhotos" -}}
{{- if and .Values.storage.photos.existingClaim .Values.storage.photos.hostPath -}}
{{- fail "storage.photos.existingClaim and storage.photos.hostPath are mutually exclusive — set one or the other." -}}
{{- end -}}
{{- if and (not .Values.storage.photos.existingClaim) (not .Values.storage.photos.hostPath) -}}
{{- fail "Either storage.photos.existingClaim or storage.photos.hostPath must be set so the server can find the photo library." -}}
{{- end -}}
{{- end -}}
```

- [ ] **Step 2: Verify `helm template` calls both validators when needed**

```bash
# Should succeed:
helm template t deploy/helm/shoebox --set storage.photos.hostPath=/srv/photos > /dev/null && echo "ok"

# Should fail with the photos message:
helm template t deploy/helm/shoebox 2>&1 | grep -q "storage.photos" && echo "photos validator fires"

# Should fail with the secret message:
helm template t deploy/helm/shoebox \
  --set secret.create=false \
  --set storage.photos.hostPath=/srv/photos 2>&1 \
  | grep -q "secret.create" && echo "secret validator fires"
```

Expected each: `ok` / `photos validator fires` / `secret validator fires`.

Note: the validators only fire if templates `include` them — that happens in Task 12 (deployment.yaml). Until then `helm template` won't actually evaluate them; the assertions above will pass after Task 12. For Task 8, just visually confirm the file parses without `helm lint` errors.

- [ ] **Step 3: `helm lint`**

```bash
helm lint deploy/helm/shoebox
```
Expected: same status as Task 7 (no _helpers-specific errors).

- [ ] **Step 4: Commit**

```bash
git add deploy/helm/shoebox/templates/_helpers.tpl
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
feat(deploy): add Helm _helpers.tpl with validation

Standard fullname/chart/labels/selectorLabels helpers, plus a Secret
name resolver that respects existingSecret, plus fail-fast validators
for the secret-create-xor-existing and photos-existingClaim-xor-hostPath
constraints (invoked from deployment.yaml in Task 12).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Helm `secret.yaml`

**Files:**
- Create: `deploy/helm/shoebox/templates/secret.yaml`

- [ ] **Step 1: Write the template**

```yaml
{{- if .Values.secret.create }}
apiVersion: v1
kind: Secret
metadata:
  name: {{ include "shoebox.secretName" . }}
  labels:
    {{- include "shoebox.labels" . | nindent 4 }}
  annotations:
    # Keep the bootstrap secret across `helm uninstall` so client certs
    # signed by the existing CA continue to authenticate after reinstall.
    helm.sh/resource-policy: keep
type: Opaque
data:
  {{ .Values.secret.key }}: {{ randAlphaNum 32 | b64enc | quote }}
{{- end }}
```

- [ ] **Step 2: Render and inspect**

```bash
helm template t deploy/helm/shoebox \
  --set storage.photos.hostPath=/srv/photos \
  --show-only templates/secret.yaml
```
Expected output includes:
- `kind: Secret`
- `name: t-shoebox-bootstrap`
- `helm.sh/resource-policy: keep`
- A non-empty base64 value under `SHOEBOX_SECRET:`.

- [ ] **Step 3: Verify `secret.create=false` suppresses it**

```bash
helm template t deploy/helm/shoebox \
  --set secret.create=false \
  --set secret.existingSecret=pre-made \
  --set storage.photos.hostPath=/srv/photos \
  --show-only templates/secret.yaml
```
Expected: error like "could not find template templates/secret.yaml" (Helm's polite way of saying the template rendered empty), or empty output. Either is acceptable.

- [ ] **Step 4: Commit**

```bash
git add deploy/helm/shoebox/templates/secret.yaml
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
feat(deploy): add Helm secret.yaml template

Auto-generates SHOEBOX_SECRET via randAlphaNum on first install. Marked
helm.sh/resource-policy: keep so helm uninstall leaves it behind —
existing client certs depend on the CA tied to this bootstrap secret.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Helm `pvc.yaml`

**Files:**
- Create: `deploy/helm/shoebox/templates/pvc.yaml`

- [ ] **Step 1: Write the template**

```yaml
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: {{ include "shoebox.fullname" . }}-data
  labels:
    {{- include "shoebox.labels" . | nindent 4 }}
spec:
  accessModes:
    - {{ .Values.storage.data.accessMode }}
  resources:
    requests:
      storage: {{ .Values.storage.data.size }}
  {{- if .Values.storage.data.storageClassName }}
  storageClassName: {{ .Values.storage.data.storageClassName }}
  {{- end }}
---
{{- if .Values.storage.cache.enabled }}
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: {{ include "shoebox.fullname" . }}-cache
  labels:
    {{- include "shoebox.labels" . | nindent 4 }}
spec:
  accessModes:
    - {{ .Values.storage.cache.accessMode }}
  resources:
    requests:
      storage: {{ .Values.storage.cache.size }}
  {{- if .Values.storage.cache.storageClassName }}
  storageClassName: {{ .Values.storage.cache.storageClassName }}
  {{- end }}
{{- end }}
```

- [ ] **Step 2: Render with defaults**

```bash
helm template t deploy/helm/shoebox \
  --set storage.photos.hostPath=/srv/photos \
  --show-only templates/pvc.yaml
```
Expected: one PVC named `t-shoebox-data`, no cache PVC (cache.enabled is false by default).

- [ ] **Step 3: Render with cache enabled**

```bash
helm template t deploy/helm/shoebox \
  --set storage.photos.hostPath=/srv/photos \
  --set storage.cache.enabled=true \
  --show-only templates/pvc.yaml
```
Expected: both `t-shoebox-data` and `t-shoebox-cache` PVCs.

- [ ] **Step 4: Commit**

```bash
git add deploy/helm/shoebox/templates/pvc.yaml
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
feat(deploy): add Helm pvc.yaml template

Unconditional data PVC; optional cache PVC behind storage.cache.enabled.
Photos volume comes from operator-supplied existingClaim or hostPath
(handled in deployment.yaml, Task 12).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Helm `service.yaml`

**Files:**
- Create: `deploy/helm/shoebox/templates/service.yaml`

- [ ] **Step 1: Write the template**

```yaml
apiVersion: v1
kind: Service
metadata:
  name: {{ include "shoebox.fullname" . }}
  labels:
    {{- include "shoebox.labels" . | nindent 4 }}
spec:
  type: {{ .Values.service.type }}
  ports:
    - name: mtls
      port: {{ .Values.service.ports.mtls }}
      targetPort: mtls
      protocol: TCP
    - name: health
      port: {{ .Values.service.ports.health }}
      targetPort: health
      protocol: TCP
  selector:
    {{- include "shoebox.selectorLabels" . | nindent 4 }}
```

- [ ] **Step 2: Render and inspect**

```bash
helm template t deploy/helm/shoebox \
  --set storage.photos.hostPath=/srv/photos \
  --show-only templates/service.yaml
```
Expected: a Service named `t-shoebox` of type `ClusterIP` with two ports `mtls` (9000) and `health` (9001), selectors `app.kubernetes.io/{name,instance}`.

- [ ] **Step 3: Commit**

```bash
git add deploy/helm/shoebox/templates/service.yaml
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
feat(deploy): add Helm service.yaml template

ClusterIP service exposing :9000 (mtls) and :9001 (health). No Ingress
by design — mTLS terminates at the server, operators wire LB / Tailscale /
Wireguard as they prefer.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Helm `deployment.yaml`

**Files:**
- Create: `deploy/helm/shoebox/templates/deployment.yaml`

This is the chunkiest template — it's also where the `_helpers.tpl` validators get invoked.

- [ ] **Step 1: Write the template**

```yaml
{{- include "shoebox.validateSecret" . -}}
{{- include "shoebox.validatePhotos" . -}}
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {{ include "shoebox.fullname" . }}
  labels:
    {{- include "shoebox.labels" . | nindent 4 }}
spec:
  replicas: 1
  strategy:
    type: Recreate
  selector:
    matchLabels:
      {{- include "shoebox.selectorLabels" . | nindent 6 }}
  template:
    metadata:
      labels:
        {{- include "shoebox.selectorLabels" . | nindent 8 }}
    spec:
      securityContext:
        {{- toYaml .Values.podSecurityContext | nindent 8 }}
      {{- with .Values.image.pullSecrets }}
      imagePullSecrets:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      containers:
        - name: shoebox-server
          image: "{{ .Values.image.repository }}:{{ .Values.image.tag | default .Chart.AppVersion }}"
          imagePullPolicy: {{ .Values.image.pullPolicy }}
          securityContext:
            {{- toYaml .Values.securityContext | nindent 12 }}
          ports:
            - name: mtls
              containerPort: {{ .Values.service.ports.mtls }}
              protocol: TCP
            - name: health
              containerPort: {{ .Values.service.ports.health }}
              protocol: TCP
          env:
            - name: SHOEBOX_HEALTH_BIND_ADDR
              value: "0.0.0.0:{{ .Values.service.ports.health }}"
            {{- with .Values.extraEnv }}
            {{- toYaml . | nindent 12 }}
            {{- end }}
          envFrom:
            - secretRef:
                name: {{ include "shoebox.secretName" . }}
          volumeMounts:
            - name: data
              mountPath: /var/lib/shoebox
            - name: cache
              mountPath: /shoebox-cache
            - name: photos
              mountPath: /photos
              {{- with .Values.storage.photos.subPath }}
              subPath: {{ . }}
              {{- end }}
            - name: tmp
              mountPath: /tmp
          livenessProbe:
            httpGet:
              path: /health
              port: health
            initialDelaySeconds: 10
            periodSeconds: 30
            timeoutSeconds: 3
          readinessProbe:
            httpGet:
              path: /health
              port: health
            initialDelaySeconds: 2
            periodSeconds: 5
            timeoutSeconds: 3
          resources:
            {{- toYaml .Values.resources | nindent 12 }}
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: {{ include "shoebox.fullname" . }}-data
        - name: cache
          {{- if .Values.storage.cache.enabled }}
          persistentVolumeClaim:
            claimName: {{ include "shoebox.fullname" . }}-cache
          {{- else }}
          emptyDir: {}
          {{- end }}
        - name: photos
          {{- if .Values.storage.photos.existingClaim }}
          persistentVolumeClaim:
            claimName: {{ .Values.storage.photos.existingClaim }}
          {{- else }}
          hostPath:
            path: {{ .Values.storage.photos.hostPath | quote }}
            type: Directory
          {{- end }}
        - name: tmp
          emptyDir: {}
      {{- with .Values.nodeSelector }}
      nodeSelector:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      {{- with .Values.tolerations }}
      tolerations:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      {{- with .Values.affinity }}
      affinity:
        {{- toYaml . | nindent 8 }}
      {{- end }}
```

Key choices:
- `SHOEBOX_HEALTH_BIND_ADDR=0.0.0.0:...` overrides the default loopback bind so the probe can reach it from outside the container (containers don't share loopback with the kubelet).
- `strategy: Recreate` because there's only one replica and the data PVC is RWO.
- `tmp` emptyDir satisfies `readOnlyRootFilesystem: true`.

- [ ] **Step 2: Render with defaults + photos.hostPath**

```bash
helm template t deploy/helm/shoebox \
  --set storage.photos.hostPath=/srv/photos
```
Expected: full output (Deployment + PVC + Service + Secret). No errors.

- [ ] **Step 3: Render with existingClaim instead**

```bash
helm template t deploy/helm/shoebox \
  --set storage.photos.existingClaim=my-photos-pvc
```
Expected: Deployment uses `persistentVolumeClaim.claimName: my-photos-pvc` for the photos volume.

- [ ] **Step 4: Verify both validators fire**

```bash
# No photos set:
helm template t deploy/helm/shoebox 2>&1 | grep -q "storage.photos" && echo "photos validator fires"

# Both photos set:
helm template t deploy/helm/shoebox \
  --set storage.photos.existingClaim=x \
  --set storage.photos.hostPath=/y 2>&1 \
  | grep -q "mutually exclusive" && echo "photos exclusivity fires"

# Both secret options:
helm template t deploy/helm/shoebox \
  --set secret.existingSecret=foo \
  --set storage.photos.hostPath=/srv/photos 2>&1 \
  | grep -q "secret.create" && echo "secret exclusivity fires"

# Neither secret option:
helm template t deploy/helm/shoebox \
  --set secret.create=false \
  --set storage.photos.hostPath=/srv/photos 2>&1 \
  | grep -q "secret.create" && echo "secret required fires"
```
Expected each line: matching "fires" message.

- [ ] **Step 5: `helm lint`**

```bash
helm lint deploy/helm/shoebox --set storage.photos.hostPath=/srv/photos
```
Expected: `1 chart(s) linted, 0 chart(s) failed`.

- [ ] **Step 6: Commit**

```bash
git add deploy/helm/shoebox/templates/deployment.yaml
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
feat(deploy): add Helm deployment.yaml template

Single-replica Recreate-strategy Deployment with three volume mounts
(data, cache, photos), tmpfs at /tmp for readOnlyRootFilesystem, both
liveness and readiness against /health, and envFrom the bootstrap
Secret. Invokes _helpers.tpl validators so misconfigured installs fail
fast with a clear message.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Helm `NOTES.txt`

**Files:**
- Create: `deploy/helm/shoebox/templates/NOTES.txt`

- [ ] **Step 1: Write the notes**

```text
shoebox-server v{{ .Chart.AppVersion }} installed as release "{{ .Release.Name }}" in namespace "{{ .Release.Namespace }}".

In-cluster URL (mTLS):
    {{ include "shoebox.fullname" . }}.{{ .Release.Namespace }}.svc.cluster.local:{{ .Values.service.ports.mtls }}

Health + Prometheus metrics (in-cluster):
    {{ include "shoebox.fullname" . }}.{{ .Release.Namespace }}.svc.cluster.local:{{ .Values.service.ports.health }}/{health,metrics}

Bootstrap secret (share with each desktop client at enrollment):
    kubectl -n {{ .Release.Namespace }} get secret {{ include "shoebox.secretName" . }} \
        -o jsonpath='{.data.{{ .Values.secret.key }}}' | base64 -d ; echo

{{ if .Values.secret.create }}
NOTE: The bootstrap Secret is annotated helm.sh/resource-policy: keep,
so `helm uninstall {{ .Release.Name }}` will NOT delete it. Existing
client certificates depend on the CA derived from this secret. Drop it
manually with `kubectl delete secret {{ include "shoebox.secretName" . }}`
only if you intend to invalidate every enrolled client.
{{ end }}

Exposing the service:
    mTLS terminates at the server. Expose via a LoadBalancer Service,
    a NodePort, or a sidecar (Tailscale/Wireguard/etc.). Do NOT put a
    Layer-7 proxy in front — it cannot terminate mTLS for shoebox
    clients.

High availability is NOT supported in v0.x (single replica only).
```

- [ ] **Step 2: Render and inspect**

```bash
helm template t deploy/helm/shoebox \
  --set storage.photos.hostPath=/srv/photos \
  --show-only templates/NOTES.txt
```
Note: `helm template --show-only` doesn't render NOTES.txt; use `helm install --dry-run` instead:

```bash
helm install t deploy/helm/shoebox \
  --dry-run \
  --set storage.photos.hostPath=/srv/photos
```
Expected: full chart render plus the NOTES.txt block at the end with substituted values.

- [ ] **Step 3: Commit**

```bash
git add deploy/helm/shoebox/templates/NOTES.txt
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
feat(deploy): add Helm NOTES.txt

Printed by helm install. Shows the in-cluster URL, the kubectl command
to read the bootstrap secret, the resource-policy: keep warning, and a
reminder that mTLS rules out L7 proxies.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: Helm CI values + golden files

**Files:**
- Create: `deploy/helm/shoebox/ci/values-cache-on.yaml`
- Create: `deploy/helm/shoebox/ci/golden-defaults.yaml`
- Create: `deploy/helm/shoebox/ci/golden-cache-on.yaml`

- [ ] **Step 1: Write the alternate values file**

`deploy/helm/shoebox/ci/values-cache-on.yaml`:

```yaml
# Used by helm-lint.yml to exercise the cache-PVC and existingClaim paths.
storage:
  cache:
    enabled: true
    size: 5Gi
  photos:
    existingClaim: my-photos-pvc
extraEnv:
  - name: SHOEBOX_LOG
    value: debug
```

- [ ] **Step 2: Render and capture the default golden file**

```bash
helm template release deploy/helm/shoebox \
  --set storage.photos.hostPath=/srv/photos \
  > deploy/helm/shoebox/ci/golden-defaults.yaml
```

Inspect the output briefly to make sure it's sensible.

- [ ] **Step 3: Render and capture the cache-on golden file**

```bash
helm template release deploy/helm/shoebox \
  -f deploy/helm/shoebox/ci/values-cache-on.yaml \
  > deploy/helm/shoebox/ci/golden-cache-on.yaml
```

The cache-on values file sets `storage.photos.existingClaim`, so the photos validator is satisfied without `--set hostPath=...`.

- [ ] **Step 4: Document regeneration**

Add a `deploy/helm/shoebox/ci/README.md`:

```markdown
# Helm chart CI fixtures

Golden files for the helm-lint workflow's `helm template` diff check.

## Regenerate after intentional chart changes

```bash
helm template release deploy/helm/shoebox \
  --set storage.photos.hostPath=/srv/photos \
  > deploy/helm/shoebox/ci/golden-defaults.yaml

helm template release deploy/helm/shoebox \
  -f deploy/helm/shoebox/ci/values-cache-on.yaml \
  > deploy/helm/shoebox/ci/golden-cache-on.yaml
```
```

- [ ] **Step 5: Commit**

```bash
git add deploy/helm/shoebox/ci/
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
feat(deploy): add Helm CI fixtures + golden files

values-cache-on.yaml exercises the cache PVC and existingClaim paths.
Two golden files captured from the current chart state; helm-lint.yml
diffs against them to catch unintended chart changes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 15: `.github/workflows/helm-lint.yml`

**Files:**
- Create: `.github/workflows/helm-lint.yml`

- [ ] **Step 1: Write the workflow**

```yaml
name: helm-lint
on:
  pull_request:
    paths:
      - "deploy/helm/**"
      - ".github/workflows/helm-lint.yml"
jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: azure/setup-helm@v4
        with:
          version: "v3.14.0"
      - name: helm lint
        run: helm lint deploy/helm/shoebox --set storage.photos.hostPath=/srv/photos
      - name: helm template (defaults)
        run: |
          helm template release deploy/helm/shoebox \
            --set storage.photos.hostPath=/srv/photos \
            > /tmp/defaults.yaml
          diff -u deploy/helm/shoebox/ci/golden-defaults.yaml /tmp/defaults.yaml
      - name: helm template (cache-on)
        run: |
          helm template release deploy/helm/shoebox \
            -f deploy/helm/shoebox/ci/values-cache-on.yaml \
            > /tmp/cache-on.yaml
          diff -u deploy/helm/shoebox/ci/golden-cache-on.yaml /tmp/cache-on.yaml
```

- [ ] **Step 2: Reproduce both diff steps locally**

```bash
helm template release deploy/helm/shoebox \
  --set storage.photos.hostPath=/srv/photos \
  > /tmp/defaults.yaml
diff -u deploy/helm/shoebox/ci/golden-defaults.yaml /tmp/defaults.yaml

helm template release deploy/helm/shoebox \
  -f deploy/helm/shoebox/ci/values-cache-on.yaml \
  > /tmp/cache-on.yaml
diff -u deploy/helm/shoebox/ci/golden-cache-on.yaml /tmp/cache-on.yaml
```
Expected: no diff output, exit code 0 on both.

- [ ] **Step 3: Actionlint (optional)**

```bash
actionlint .github/workflows/helm-lint.yml
```
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/helm-lint.yml
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
ci: add helm-lint workflow

helm lint + helm template golden-file diff for both default values and
the cache-on/existingClaim values. Fires on PRs touching deploy/helm/**.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 16: `.github/workflows/release.yml`

**Files:**
- Create: `.github/workflows/release.yml`

The release workflow has three jobs: `image` (multi-arch buildx push), `binary` (matrix of three targets), and `helm` (chart package + upload). Job ordering: `image` and `binary` run in parallel; `helm` waits on `image`.

- [ ] **Step 1: Write the workflow**

```yaml
name: release
on:
  push:
    tags: ["v*"]

permissions:
  contents: write       # to upload to GitHub Releases
  packages: write       # to push to ghcr.io

jobs:
  image:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: docker/setup-qemu-action@v3
      - uses: docker/setup-buildx-action@v3
      - uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}
      - id: meta
        uses: docker/metadata-action@v5
        with:
          images: ghcr.io/${{ github.repository_owner }}/shoebox-server
          tags: |
            type=semver,pattern={{version}}
            type=semver,pattern={{major}}.{{minor}}
            type=semver,pattern={{major}}
      - uses: docker/build-push-action@v5
        with:
          context: .
          platforms: linux/amd64,linux/arm64
          push: true
          tags: ${{ steps.meta.outputs.tags }}
          cache-from: type=gha
          cache-to: type=gha,mode=max
      - name: arm64 layer smoke (QEMU)
        run: |
          # Pick the first pushed tag (the fully-qualified vX.Y.Z one) and
          # verify the arm64 layer actually runs under QEMU — catches glibc
          # mismatches or a broken arm64 sqld download in the Dockerfile.
          IMAGE_TAG=$(echo "${{ steps.meta.outputs.tags }}" | head -1)
          docker pull --platform linux/arm64 "${IMAGE_TAG}"
          docker run --rm --platform linux/arm64 --entrypoint /usr/local/bin/sqld "${IMAGE_TAG}" --help | head -5

  binary:
    strategy:
      fail-fast: false
      matrix:
        include:
          - target: x86_64-unknown-linux-gnu
            runner: ubuntu-latest
            cross: false
            package_script: package-linux-amd64.sh
          - target: aarch64-unknown-linux-gnu
            runner: ubuntu-latest
            cross: true
            package_script: package-linux-arm64.sh
          - target: aarch64-apple-darwin
            runner: macos-14
            cross: false
            package_script: package-macos-arm64.sh
    runs-on: ${{ matrix.runner }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: 1.85.0
          targets: ${{ matrix.target }}
      - if: matrix.cross
        run: cargo install cross --locked --version 0.2.5
      - name: Build
        run: |
          if [ "${{ matrix.cross }}" = "true" ]; then
            cross build --release --target ${{ matrix.target }} -p shoebox-server
          else
            cargo build --release --target ${{ matrix.target }} -p shoebox-server
          fi
      - name: Package
        env:
          TARGET: ${{ matrix.target }}
          VERSION: ${{ github.ref_name }}
        run: bash .github/release/${{ matrix.package_script }}
      - uses: softprops/action-gh-release@v2
        with:
          files: |
            dist/*.tar.xz
            dist/*.sha256

  helm:
    runs-on: ubuntu-latest
    needs: image
    steps:
      - uses: actions/checkout@v4
      - uses: azure/setup-helm@v4
        with:
          version: "v3.14.0"
      - name: Package
        run: |
          # Strip the leading 'v' from tag for Helm SemVer compliance.
          VERSION="${GITHUB_REF_NAME#v}"
          helm package deploy/helm/shoebox \
            --version "${VERSION}" \
            --app-version "${VERSION}"
      - uses: softprops/action-gh-release@v2
        with:
          files: "shoebox-*.tgz"
```

- [ ] **Step 2: Actionlint (optional)**

```bash
actionlint .github/workflows/release.yml
```
Expected: clean.

- [ ] **Step 3: Smoke-render the helm package step locally**

```bash
helm package deploy/helm/shoebox --version 0.0.0-dev --app-version 0.0.0-dev
ls shoebox-0.0.0-dev.tgz
tar -tzf shoebox-0.0.0-dev.tgz | head -10
rm shoebox-0.0.0-dev.tgz
```
Expected: tarball created, contains `shoebox/Chart.yaml`, `shoebox/values.yaml`, etc.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
ci: add release workflow (image + binary + helm)

Triggered on v* tag push. Three jobs:
- image: buildx multi-arch push to ghcr.io with semver tags
- binary: matrix over linux-amd64, linux-arm64 (via cross), macos-arm64;
  packages via .github/release/package-*.sh, uploads tarballs + sha256s
- helm: helm package + upload chart .tgz; waits on image

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 17: `docs/deployment/quickstart-docker.md`

**Files:**
- Create: `docs/deployment/quickstart-docker.md`

- [ ] **Step 1: Create the directory**

```bash
mkdir -p docs/deployment
```

- [ ] **Step 2: Write the doc**

````markdown
# Quickstart — Docker

The fastest way to run shoebox-server. Two paths: bare `docker run` and
`docker compose`. Compose is easier to maintain long-term; bare `docker
run` is good for a 60-second smoke test.

## Path A: `docker run` (60-second smoke test)

```bash
docker volume create shoebox-data
docker volume create shoebox-cache

docker run -d \
  --name shoebox-server \
  -p 9000:9000 \
  -p 127.0.0.1:9001:9001 \
  -e SHOEBOX_SECRET="$(openssl rand -base64 24)" \
  -v shoebox-data:/var/lib/shoebox \
  -v shoebox-cache:/shoebox-cache \
  -v /path/to/your/photos:/photos \
  ghcr.io/<owner>/shoebox-server:v0.1.0
```

Then check health:
```bash
curl http://127.0.0.1:9001/health
```
Expected: `ok`.

To get the bootstrap secret you set:
```bash
docker inspect shoebox-server --format '{{ range .Config.Env }}{{ println . }}{{ end }}' | grep SHOEBOX_SECRET
```

## Path B: Docker Compose (recommended)

```bash
git clone https://github.com/<owner>/shoebox.git
cd shoebox/deploy/compose

cp .env.example .env
echo "SHOEBOX_SECRET=$(openssl rand -base64 24)" >> .env
$EDITOR .env  # set SHOEBOX_PHOTOS_DIR to your photo library path

# Replace CHANGE-ME-OWNER with the actual repo owner in docker-compose.yml
sed -i 's|ghcr.io/CHANGE-ME-OWNER|ghcr.io/<owner>|' docker-compose.yml

docker compose up -d
```

See `deploy/compose/README.md` for the full walkthrough and upgrade
instructions.

## Sharing the bootstrap secret with clients

```bash
# For Path A:
docker inspect shoebox-server --format '{{ range .Config.Env }}{{ println . }}{{ end }}' \
  | grep ^SHOEBOX_SECRET= | cut -d= -f2-

# For Path B:
grep ^SHOEBOX_SECRET= deploy/compose/.env | cut -d= -f2-
```

Hand that string to each desktop client during first-run enrollment.

## Tags

| Tag | Use case |
|---|---|
| `v0.1.0` | pinned, recommended for production |
| `v0.1` | floats to latest 0.1.x patch |
| `v0` | floats to latest 0.x.x (will move on 0.2.0 release) |
| `main` | trunk; expect breakage |

No `:latest` tag until v1.0 ships.

## Upgrade

Both paths: pull the new tag, restart the container. Migrations run
automatically on startup; the catalog DB is held in the `shoebox-data`
volume across restarts.

```bash
# Path A:
docker pull ghcr.io/<owner>/shoebox-server:v0.2.0
docker stop shoebox-server && docker rm shoebox-server
docker run ...  # same command as before with the new tag

# Path B:
docker compose pull
docker compose up -d
```
````

- [ ] **Step 3: Render check**

Open `docs/deployment/quickstart-docker.md` in any markdown viewer (or just `cat` it) and confirm the structure reads cleanly.

- [ ] **Step 4: Commit**

```bash
git add docs/deployment/quickstart-docker.md
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
docs(deploy): add Docker quickstart

Two paths (bare docker run + docker compose) with the actual commands
operators will copy-paste. Covers bootstrap-secret retrieval, tag
strategy, and upgrades.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 18: `docs/deployment/quickstart-binary.md`

**Files:**
- Create: `docs/deployment/quickstart-binary.md`

- [ ] **Step 1: Write the doc**

````markdown
# Quickstart — Standalone binary

For NASes without a container runtime, dedicated Mac mini hosts, or
operators who prefer systemd / launchd to Docker.

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
- `share/systemd/shoebox-server.service` (Linux) or `share/launchd/com.shoebox.server.plist` (macOS)
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

Health check:
```bash
curl http://127.0.0.1:9001/health
```

## Sharing the bootstrap secret with clients

```bash
# Linux:
sudo grep ^SHOEBOX_SECRET= /etc/shoebox/shoebox.env | cut -d= -f2-

# macOS:
sudo plutil -p /Library/LaunchDaemons/com.shoebox.server.plist | grep SHOEBOX_SECRET
```

Hand that string to each desktop client during first-run enrollment.

## Upgrade

Stop the service, replace the binaries, start it again.

```bash
# Linux:
sudo systemctl stop shoebox-server
sudo cp ./bin/shoebox-server ./bin/sqld /opt/shoebox/bin/
sudo systemctl start shoebox-server

# macOS:
sudo launchctl bootout system/com.shoebox.server
sudo cp ./bin/shoebox-server ./bin/sqld /opt/shoebox/bin/
sudo launchctl bootstrap system /Library/LaunchDaemons/com.shoebox.server.plist
```

Migrations run automatically on startup. The catalog DB stays put in
`/var/lib/shoebox` (Linux) or `/usr/local/var/shoebox` (macOS).

## Configuration

All settings are env vars. Either put them in `/etc/shoebox/shoebox.env`
(loaded by the systemd unit) or as `<key>...</key><string>...</string>`
entries inside the launchd plist.

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
````

- [ ] **Step 2: Render check**

`cat docs/deployment/quickstart-binary.md` and confirm structure.

- [ ] **Step 3: Commit**

```bash
git add docs/deployment/quickstart-binary.md
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
docs(deploy): add standalone binary quickstart

Linux (systemd) and macOS (launchd) install walkthroughs, sha256
verification, upgrade procedure, and the full env-var contract table.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 19: `docs/deployment/quickstart-kubernetes.md`

**Files:**
- Create: `docs/deployment/quickstart-kubernetes.md`

- [ ] **Step 1: Write the doc**

````markdown
# Quickstart — Kubernetes (Helm)

For operators running an existing Kubernetes cluster. The chart is
single-replica only in v0.x (HA is backlog).

## Prerequisites

- Kubernetes 1.25+
- Helm 3.13+
- A PersistentVolumeClaim (or hostPath on a single node) pointing at
  your photo library — the chart will not create one.

## Install

Download the chart from the release:
```bash
curl -fsSL -o shoebox-0.1.0.tgz \
  https://github.com/<owner>/shoebox/releases/download/v0.1.0/shoebox-0.1.0.tgz
```

Minimal install with hostPath photos (single-node clusters, k3s, etc.):
```bash
helm install shoebox ./shoebox-0.1.0.tgz \
  --set image.repository=ghcr.io/<owner>/shoebox-server \
  --set storage.photos.hostPath=/srv/photos
```

Realistic install with an existing photos PVC:
```bash
cat > shoebox-values.yaml <<'EOF'
image:
  repository: ghcr.io/<owner>/shoebox-server
  tag: v0.1.0

storage:
  data:
    size: 5Gi
    storageClassName: longhorn
  cache:
    enabled: true
    size: 50Gi
    storageClassName: longhorn
  photos:
    existingClaim: photos-library

service:
  type: ClusterIP

extraEnv:
  - name: SHOEBOX_LOG
    value: info
EOF

helm install shoebox ./shoebox-0.1.0.tgz -f shoebox-values.yaml
```

## Retrieve the bootstrap secret

```bash
kubectl get secret shoebox-shoebox-bootstrap \
  -o jsonpath='{.data.SHOEBOX_SECRET}' | base64 -d ; echo
```

Hand that string to each desktop client during first-run enrollment.

## Exposing the service

The chart creates a `ClusterIP` Service called `shoebox-shoebox` with
ports `mtls` (9000) and `health` (9001). mTLS terminates at the server,
so **do not** put a Layer-7 proxy (nginx, traefik, gateway-api HTTPRoute)
in front. Options:

- **`LoadBalancer` Service** — easiest:
  ```bash
  helm upgrade shoebox ./shoebox-0.1.0.tgz \
    -f shoebox-values.yaml \
    --set service.type=LoadBalancer
  ```
- **`NodePort`** — for clusters without a cloud LB.
- **`Ingress` with TCP/SSL passthrough** — works with traefik
  IngressRouteTCP or contour TCPProxy; do NOT use a vanilla L7 Ingress.
- **In-cluster Tailscale subnet router** — clients reach the Service via
  the cluster's tailnet.

## Upgrade

```bash
curl -fsSL -o shoebox-0.2.0.tgz \
  https://github.com/<owner>/shoebox/releases/download/v0.2.0/shoebox-0.2.0.tgz
helm upgrade shoebox ./shoebox-0.2.0.tgz -f shoebox-values.yaml
```

The chart uses `strategy: Recreate` (single replica + RWO data PVC), so
expect a brief downtime during rollout while the old pod releases the
PVC and the new one acquires it.

## Uninstall

```bash
helm uninstall shoebox
```

The bootstrap Secret (`shoebox-shoebox-bootstrap`) is annotated
`helm.sh/resource-policy: keep` and remains after uninstall — existing
client certs are signed by the CA that's derived from this secret, so
reinstalling reuses it and clients keep working. To wipe everything:

```bash
kubectl delete secret shoebox-shoebox-bootstrap
kubectl delete pvc shoebox-shoebox-data shoebox-shoebox-cache
```

## What the chart does NOT do

By design — see the spec's backlog:

- No `Ingress` (mTLS rules out L7).
- No `NetworkPolicy` (write your own; allow `:9000` from client subnets).
- No `ServiceMonitor` (write a `PodMonitor` against `:9001/metrics` if
  using prometheus-operator).
- No HPA / multi-replica (HA is a sub-project #1 backlog item).
- No automated CA / cert-manager wiring.
````

- [ ] **Step 2: Render check**

`cat docs/deployment/quickstart-kubernetes.md` and confirm structure.

- [ ] **Step 3: Commit**

```bash
git add docs/deployment/quickstart-kubernetes.md
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
docs(deploy): add Kubernetes quickstart

Helm install paths (hostPath single-node + existingClaim production),
bootstrap secret retrieval, exposure options that respect mTLS, upgrade
procedure, uninstall with resource-policy: keep caveat.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 20: README Deployment section

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Read the current README to find the right insertion point**

```bash
cat README.md | head -50
```

Note where existing sections start (look for the table of contents or a "Getting started" / "Usage" heading). The Deployment section belongs after any "Running locally" / "Development" section, before "Contributing" / "License" if those exist.

- [ ] **Step 2: Add this section**

Append (or insert at the right spot — engineer judgment):

```markdown
## Deployment

shoebox-server v0.1.0 ships three deployment paths. Pick one:

| Path | Best for | Docs |
|---|---|---|
| **Docker** | NASes with a Docker runtime, home servers, generic VMs | [docs/deployment/quickstart-docker.md](docs/deployment/quickstart-docker.md) |
| **Standalone binary** | NASes without Docker, dedicated Mac mini hosts | [docs/deployment/quickstart-binary.md](docs/deployment/quickstart-binary.md) |
| **Helm chart** | Existing Kubernetes clusters | [docs/deployment/quickstart-kubernetes.md](docs/deployment/quickstart-kubernetes.md) |

All three start a server that listens on `:9000` (mTLS) and `:9001`
(health + Prometheus metrics, loopback by default). Each desktop client
enrolls against the server using the shared bootstrap secret.

Images: `ghcr.io/<owner>/shoebox-server` (multi-arch: `linux/amd64`,
`linux/arm64`).
Release artifacts: https://github.com/<owner>/shoebox/releases
Helm chart README: [`deploy/helm/shoebox/README.md`](deploy/helm/shoebox/README.md)

Sub-project #1 (catalog + sync + stack) is complete with v0.1.0.
RAW pipeline, library UI, develop module, and export are sub-projects
#2-#5, not yet started.
```

- [ ] **Step 3: Verify the README still renders cleanly**

```bash
cat README.md
```

- [ ] **Step 4: Commit**

```bash
git add README.md
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
docs(readme): add Deployment section linking the three quickstarts

Three-row table pointing at the docker / binary / helm quickstarts,
plus the registry path and a note that sub-projects #2-#5 are next.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 21: Update CLAUDE.md with Plan 1.5 status

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update the sub-project status table**

Find the table starting `| # | Sub-project | Status | Spec |` in CLAUDE.md and update row 1:

Before:
```
| 1 | **Catalog, sync & stack** | Plans 1.1–1.4b implemented. Plan 1.5 (deployment) pending. | [spec](docs/superpowers/specs/2026-05-17-catalog-sync-and-stack-design.md) |
```

After:
```
| 1 | **Catalog, sync & stack** | Plans 1.1–1.5 implemented. Sub-project complete. | [spec](docs/superpowers/specs/2026-05-17-catalog-sync-and-stack-design.md) |
```

- [ ] **Step 2: Add a "Plan 1.5" subsection to "Implementation status"**

Insert after the existing "`crates/shoebox-client` — demo library view (Plan 1.4b):" subsection:

```markdown
- `deploy/` + `.github/release/` + `.github/workflows/{release,helm-lint}.yml` — full deployment plane (Plan 1.5):
  - Multi-arch Docker image (`linux/amd64` + `linux/arm64`) published to `ghcr.io/<owner>/shoebox-server` on every `v*` tag and on `main`.
  - GitHub Releases standalone tarballs: `linux-amd64`, `linux-arm64` (via `cross`), `macos-arm64`. Each bundles `shoebox-server` + matching `sqld` + systemd or launchd unit + `config.example.toml` + README, with sha256 sidecar.
  - Helm chart (`deploy/helm/shoebox/`): single-replica, two PVCs (data + optional cache), photos via existingClaim or hostPath, auto-generated bootstrap Secret with `helm.sh/resource-policy: keep`. `helm-lint.yml` enforces `helm lint` + `helm template` golden-file diff on PRs touching the chart.
  - Compose example (`deploy/compose/`): single-service `docker-compose.yml` + `.env.example` + README. CI smoke-tests it (`compose-smoke` job in `ci.yml`).
  - Binary smoke (`binary-smoke` job in `ci.yml`): builds the linux-amd64 tarball, extracts it, runs the server against synthetic env, hits `/health`.
  - Three deployment quickstarts under `docs/deployment/`.
```

- [ ] **Step 3: Update the "Known limitations" section** (if Plan 1.5 introduced new ones — it didn't, so this step is just a verification)

Re-read the "Known limitations (Plan 1.3+1.4+1.4b v1)" section in CLAUDE.md and confirm nothing in Plan 1.5 invalidates the items already listed (it shouldn't — Plan 1.5 is pure packaging/CI).

- [ ] **Step 4: Verify**

```bash
grep -A1 "Plan 1.1–1.5" CLAUDE.md
grep "deploy/" CLAUDE.md
```
Expected: both grep matches present.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
docs(claude.md): record Plan 1.5 implementation status

Sub-project #1 (catalog + sync + stack) is now complete (Plans 1.1-1.5).
Deployment plane summarized under "Implementation status".

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## End-of-plan checks

After Task 21, run these sanity checks:

```bash
# All tests still pass (Plan 1.5 didn't touch Rust, but verify):
cargo test --workspace --all-targets

# Workspace is clean:
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Docker image still builds for amd64:
docker build -t shoebox-server:plan15-final .

# Helm lint final:
helm lint deploy/helm/shoebox --set storage.photos.hostPath=/srv/photos

# Compose config still valid:
SHOEBOX_PHOTOS_DIR=/tmp docker compose -f deploy/compose/docker-compose.yml config --quiet

# Commit log:
git log --oneline | head -25
```

Expected:
- All Rust checks green.
- Image builds.
- Helm lint shows `1 chart(s) linted, 0 chart(s) failed`.
- Compose config exits 0.
- Commit log shows the Plan 1.5 commit chain in order.

## Acceptance criterion (from spec §10)

From a clean checkout at `v0.1.0`, a fresh operator must be able to
stand up `shoebox-server` via any one of the three paths following only
the corresponding `docs/deployment/quickstart-*.md`, with no edits to
repo files, and a desktop client must be able to complete its first-run
enrollment against it.

This is verified at the first real release (cut `v0.1.0` tag, watch
`release.yml` succeed, walk through each quickstart on a clean machine).
Not gated by the plan itself.
