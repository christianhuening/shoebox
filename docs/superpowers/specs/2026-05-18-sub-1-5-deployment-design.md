# Sub-project 1.5 — Deployment Design

**Status:** Approved (2026-05-18)
**Parent spec:** `2026-05-17-catalog-sync-and-stack-design.md`
**Predecessor plans:** 1.1, 1.2, 1.3, 1.4, 1.4b (all executed)
**Scope tag in parent spec:** §10 "Deployment" — completes sub-project #1

---

## 1. Goal

Ship `shoebox-server` v0.1.0 through three production-ready deployment paths:

1. **Multi-arch Docker image** published to `ghcr.io` (`linux/amd64`,
   `linux/arm64`) — the primary path for NAS, home-server, and small-VM
   deployments.
2. **Standalone binary tarballs** on GitHub Releases (`linux-x86_64`,
   `linux-aarch64`, `macos-aarch64`) — the fallback for non-Docker NASes,
   dedicated Mac mini hosts, and operators who prefer systemd over a
   container runtime.
3. **Helm chart** for Kubernetes operators — single-replica, minimal
   moving parts, batteries-included secret bootstrap.

Each path is independently usable; each is exercised in CI on every tag
release. After Plan 1.5, sub-project #1 (catalog + sync + stack) is fully
done and sub-projects #2-#5 can start.

## 2. Non-goals (carried to backlog)

- High-availability / multi-replica (requires a sqld primary/replica
  topology — see parent spec §13 "Backlog").
- Kubernetes `Ingress` + `cert-manager` wiring (mTLS terminates at the
  server; users wire their own L4 exposure).
- `NetworkPolicy` defaults in the Helm chart.
- Prometheus Operator `ServiceMonitor`.
- Windows server build (server is Linux/macOS; clients are cross-platform).
- macOS Intel (`x86_64-apple-darwin`) build — Apple Silicon only for v1.
- NAS-vendor native packages (Synology `spk`, QNAP `qpkg`, Unraid plugin).
- Automated catalog schema upgrade across major versions — v0.x reserves
  the right to require a manual rebuild.
- OCI Helm chart registry as primary distribution (tarball on GitHub
  Releases is primary; OCI is optional add-on).
- Signed images via cosign and SBOM in releases.

## 3. Repo layout (additions only)

```
deploy/
├── compose/
│   ├── docker-compose.yml
│   ├── .env.example
│   └── README.md
├── helm/
│   └── shoebox/
│       ├── Chart.yaml
│       ├── values.yaml
│       ├── values.schema.json
│       ├── README.md
│       └── templates/
│           ├── _helpers.tpl
│           ├── deployment.yaml
│           ├── service.yaml
│           ├── pvc.yaml
│           ├── secret.yaml
│           └── NOTES.txt
└── systemd/
    └── shoebox-server.service

docs/deployment/
├── quickstart-docker.md
├── quickstart-binary.md
└── quickstart-kubernetes.md

.github/workflows/
├── release.yml          ← new, tag-triggered
└── helm-lint.yml        ← new, PR-triggered for deploy/helm/**

.github/release/
├── package-linux-amd64.sh
├── package-linux-arm64.sh
└── package-macos-arm64.sh
```

The existing `Dockerfile` is rewritten in place (not duplicated) to be
arch-aware. The existing `.github/workflows/ci.yml` is unchanged.

## 4. Docker image

### 4.1 Dockerfile changes

The current `Dockerfile` hardcodes the amd64 `sqld` URL + sha256. It is
rewritten so the `sqld` download is selected by `TARGETARCH`:

```dockerfile
# (in the runtime stage)
ARG SQLD_VERSION=v0.24.32
ARG SQLD_AMD64_SHA256=71720fc8648c19efef416efebd47145ef59b62e198770533530a858e1336879f
ARG SQLD_ARM64_SHA256=  # downloaded from upstream, sha256summed by the implementer at Plan-1.5 Task time
ARG TARGETARCH
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64) target=x86_64-unknown-linux-gnu;  sha=${SQLD_AMD64_SHA256} ;; \
      arm64) target=aarch64-unknown-linux-gnu; sha=${SQLD_ARM64_SHA256} ;; \
      *) echo "unsupported TARGETARCH=${TARGETARCH}"; exit 1 ;; \
    esac; \
    asset="libsql-server-${target}.tar.xz"; \
    wget -q "https://github.com/tursodatabase/libsql/releases/download/libsql-server-${SQLD_VERSION}/${asset}"; \
    echo "${sha}  ${asset}" | sha256sum -c -; \
    tar -xJf "${asset}"; \
    mv "libsql-server-${target}/sqld" /usr/local/bin/sqld; \
    chmod +x /usr/local/bin/sqld; \
    rm -rf "${asset}" "libsql-server-${target}"
```

The builder stage stays as-is — `cargo build --release -p shoebox-server`
runs natively on each platform's runner. No cross-compilation in Docker.

The runtime user (`shoebox`, uid 10001), volume layout, ports, env-var
defaults, and HEALTHCHECK from the current Dockerfile are unchanged.

### 4.2 Registry + tags

Repository: `ghcr.io/<github-owner>/shoebox-server`. The owner is whatever
the repo lives under at release time; CI uses `${{ github.repository_owner }}`.

Tag policy:

| Trigger | Tags pushed |
|---|---|
| `git tag v0.1.0 && git push --tags` | `v0.1.0`, `v0.1`, `v0` |
| push to `main` | `main`, `sha-<7char>` |

No `:latest` until v1.0 ships. Multi-arch manifests use the standard
`buildx` `--platform linux/amd64,linux/arm64` output.

### 4.3 Build flow in CI

`docker/setup-qemu-action@v3` → `docker/setup-buildx-action@v3` →
`docker/login-action@v3` (to `ghcr.io` using `GITHUB_TOKEN` with
`packages: write`) → `docker/build-push-action@v5` with:

- `platforms: linux/amd64,linux/arm64`
- `tags`: computed by `docker/metadata-action@v5` from the tag/branch
- `cache-from: type=gha`, `cache-to: type=gha,mode=max`

## 5. Standalone binary

### 5.1 Targets and runners

| Target triple | CI runner | Build method |
|---|---|---|
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | native `cargo build --release` |
| `aarch64-unknown-linux-gnu` | `ubuntu-latest` | `cross` (`cross-rs/cross`) |
| `aarch64-apple-darwin` | `macos-14` | native `cargo build --release` |

Linux arm64 via `cross` (which uses a containerised cross-toolchain) is
chosen over a separate `ubuntu-24.04-arm` runner so the matrix stays on
two runner images instead of three. If `cross` proves slow or flaky,
swapping in `actions/runner-images` arm64 is a CI-only change.

### 5.2 Tarball contents

Each tarball is named `shoebox-server-v<version>-<target>.tar.xz` and
unpacks to:

```
shoebox-server-v0.1.0-<target>/
├── bin/
│   ├── shoebox-server               built from this commit
│   └── sqld                         pinned v0.24.32, sha256-verified
├── share/
│   ├── config.example.toml          every SHOEBOX_* var documented
│   ├── systemd/shoebox-server.service
│   └── openrc/shoebox-server        small init.d-style script, NAS-friendly
├── LICENSE
└── README.md                        install / first-run / upgrade
```

The `sqld` binary in each tarball is the same pinned `v0.24.32`
release used in the Docker image, downloaded and sha256-verified by the
per-target packager script (`.github/release/package-<target>.sh`). The
macOS tarball ships `sqld` for `aarch64-apple-darwin` from the same
upstream release.

A `<tarball>.sha256` sidecar is uploaded next to each tarball.

### 5.3 systemd unit (shipped in tarball and in `deploy/systemd/`)

```ini
[Unit]
Description=shoebox server
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

`/etc/shoebox/shoebox.env` is operator-supplied and is where
`SHOEBOX_SECRET` lives. The README walks through `openssl rand -base64 24`
to generate one.

## 6. Helm chart

### 6.1 `Chart.yaml`

```yaml
apiVersion: v2
name: shoebox
description: Single-replica shoebox catalog server with embedded sqld.
type: application
version: 0.1.0           # chart version
appVersion: "0.1.0"      # server version (default image tag)
home: https://github.com/<owner>/shoebox
sources:
  - https://github.com/<owner>/shoebox
maintainers:
  - name: <maintainer>
kubeVersion: ">=1.25.0-0"
```

### 6.2 `values.yaml`

```yaml
image:
  repository: ghcr.io/<owner>/shoebox-server
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
    existingClaim: ""        # operator-supplied PVC; required if hostPath empty
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
  capabilities: { drop: ["ALL"] }
  readOnlyRootFilesystem: true

nodeSelector: {}
tolerations: []
affinity: {}

extraEnv: []                 # e.g. [{ name: SHOEBOX_LOG, value: debug }]
```

`values.schema.json` enforces:
- `image.repository` non-empty
- `secret.create XOR (secret.existingSecret non-empty)`
- `storage.photos.existingClaim XOR storage.photos.hostPath` (exactly one)

The chart errors with a clear message via `{{ fail "..." }}` in
`_helpers.tpl` when constraints don't hold.

### 6.3 Templates

- **`deployment.yaml`** — `replicas: 1` (hardcoded; HA backlog noted in
  NOTES). One container `shoebox-server` from the chart image. Volume
  mounts: `/var/lib/shoebox` (data PVC), `/shoebox-cache` (cache PVC or
  emptyDir), `/photos` (photos PVC or hostPath, read-only-friendly).
  Env: `SHOEBOX_SECRET` from the Secret via `envFrom: secretRef`, plus
  every entry in `extraEnv`. Liveness + readiness probes hit
  `:9001/health`. `tmpfs` mount at `/tmp` to satisfy
  `readOnlyRootFilesystem`.
- **`service.yaml`** — single Service exposing ports `9000` (mtls) and
  `9001` (health). Type from `.Values.service.type`.
- **`pvc.yaml`** — emits the data PVC unconditionally; emits the cache
  PVC only if `storage.cache.enabled`; does not emit a photos PVC
  (operator-supplied).
- **`secret.yaml`** — `{{- if .Values.secret.create }}` block; uses
  `randAlphaNum 32` for the value. Annotated with
  `helm.sh/resource-policy: keep` so `helm uninstall` doesn't drop the
  bootstrap secret. Cluster operators who reinstall want to keep the
  same secret so existing client certs continue to authenticate.
- **`NOTES.txt`** — prints the in-cluster URL, the kubectl command to
  read the bootstrap secret, a reminder that mTLS terminates at the
  server so the Service is the integration point.

### 6.4 Distribution

Primary: `.tgz` attached to the GitHub Release for the matching version.
Operators `helm install shoebox ./shoebox-0.1.0.tgz`. OCI registry push
(`oci://ghcr.io/<owner>/charts`) is backlog.

## 7. Compose

`deploy/compose/docker-compose.yml`:

```yaml
services:
  shoebox-server:
    image: ghcr.io/<owner>/shoebox-server:v0.1.0
    container_name: shoebox-server
    restart: unless-stopped
    ports:
      - "9000:9000"           # mTLS — exposed to LAN
      - "127.0.0.1:9001:9001" # health + metrics — loopback only
    volumes:
      - shoebox-data:/var/lib/shoebox
      - shoebox-cache:/shoebox-cache
      - ${SHOEBOX_PHOTOS_DIR:?set in .env}:/photos
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

`.env.example`:

```env
# Required: bootstrap secret. Generate with: openssl rand -base64 24
SHOEBOX_SECRET=

# Required: absolute path on the host to the photos library.
SHOEBOX_PHOTOS_DIR=/srv/photos

# Optional: log level (info | debug | trace).
SHOEBOX_LOG=info
```

`deploy/compose/README.md` walks through: `cp .env.example .env`,
generate secret, set photos path, `docker compose up -d`, share secret
with clients.

## 8. Docs

Three quickstart files under `docs/deployment/`, each ≤ 100 lines:

- **`quickstart-docker.md`** — `docker run` one-liner + compose path,
  pointers to image tags, upgrade procedure (pull new tag, restart).
- **`quickstart-binary.md`** — download → verify sha256 → extract →
  systemd-enable. Covers Linux (systemd) and macOS (`launchd` plist,
  generated inline in the doc).
- **`quickstart-kubernetes.md`** — `helm install` example with a
  realistic `values.yaml`, plus `kubectl get secret` to retrieve the
  bootstrap secret, plus a note on exposing the Service.

The repo `README.md` grows a "Deployment" section that links to all three
and points at the Helm chart README inside `deploy/helm/shoebox/`.

## 9. CI

### 9.1 `.github/workflows/release.yml`

```yaml
name: release
on:
  push:
    tags: ["v*"]
jobs:
  image:
    runs-on: ubuntu-latest
    permissions: { contents: read, packages: write }
    steps:
      - uses: actions/checkout@v4
      - uses: docker/setup-qemu-action@v3
      - uses: docker/setup-buildx-action@v3
      - uses: docker/login-action@v3
        with: { registry: ghcr.io, username: ${{ github.actor }}, password: ${{ secrets.GITHUB_TOKEN }} }
      - uses: docker/metadata-action@v5
        id: meta
        with:
          images: ghcr.io/${{ github.repository_owner }}/shoebox-server
          tags: |
            type=semver,pattern={{version}}
            type=semver,pattern={{major}}.{{minor}}
            type=semver,pattern={{major}}
      - uses: docker/build-push-action@v5
        with:
          platforms: linux/amd64,linux/arm64
          push: true
          tags: ${{ steps.meta.outputs.tags }}
          cache-from: type=gha
          cache-to: type=gha,mode=max

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
        with: { toolchain: 1.85.0, targets: ${{ matrix.target }} }
      - if: matrix.cross
        run: cargo install cross --locked
      - name: build
        run: |
          if [ "${{ matrix.cross }}" = "true" ]; then
            cross build --release --target ${{ matrix.target }} -p shoebox-server
          else
            cargo build --release --target ${{ matrix.target }} -p shoebox-server
          fi
      - name: package
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
      - run: helm package deploy/helm/shoebox --version ${{ github.ref_name }} --app-version ${{ github.ref_name }}
      - uses: softprops/action-gh-release@v2
        with: { files: "shoebox-*.tgz" }
```

(Job order: `image` and `binary` run in parallel; `helm` waits on
`image` so a failed image build aborts the release.)

### 9.2 `.github/workflows/helm-lint.yml`

```yaml
name: helm-lint
on:
  pull_request:
    paths: ["deploy/helm/**"]
jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: azure/setup-helm@v4
      - run: helm lint deploy/helm/shoebox
      - run: helm template t deploy/helm/shoebox > /tmp/defaults.yaml
      - run: helm template t deploy/helm/shoebox -f deploy/helm/shoebox/ci/values-cache-on.yaml > /tmp/cache-on.yaml
      - run: diff -u deploy/helm/shoebox/ci/golden-defaults.yaml /tmp/defaults.yaml
      - run: diff -u deploy/helm/shoebox/ci/golden-cache-on.yaml /tmp/cache-on.yaml
```

Golden files are checked into `deploy/helm/shoebox/ci/`. Regenerating
them is a documented one-liner (`helm template t … > golden-*.yaml`).

## 10. Testing strategy

| Path | What CI does | What CI doesn't do |
|---|---|---|
| Docker image | Buildx + push on tag; the existing `ci.yml` docker job continues to build (no push) on every PR. On tag, an additional `docker run --rm --platform linux/arm64 <image> --version` smoke via QEMU verifies the arm64 layer works. | No end-to-end smoke against a running cluster. |
| Compose | A separate `compose-smoke` job in `ci.yml` runs `docker compose up -d`, waits for `:9001/health`, tears down. | No multi-host or persistence-across-restart test. |
| Binary | `binary-smoke` job (added to `ci.yml`) extracts the linux-amd64 tarball after a normal CI build, launches `./bin/shoebox-server`, hits `:9001/health`, SIGTERMs it. arm64 + macos targets get **build-only** validation; runtime smoke requires the actual hardware and is a backlog item. | No systemd integration test. |
| Helm | `helm lint` + `helm template` golden-file diff in `helm-lint.yml`. | No `helm install` against `kind` in CI (manual recipe documented). |

Acceptance criterion for Plan 1.5: from a clean checkout at `v0.1.0`, a
fresh operator can stand up shoebox-server via any one of the three paths
following only the corresponding `docs/deployment/quickstart-*.md`, with
no edits to repo files, and a desktop client can complete its first-run
enrollment against it.

## 11. Risks & mitigations

- **`cross` for linux-arm64 builds drifts from upstream `rust-toolchain`.**
  Mitigation: pin `cross` version; if it breaks, switch the arm64 matrix
  leg to a native `ubuntu-24.04-arm` runner (one-line CI change).
- **sqld upstream releases break the URL pattern between versions.**
  Mitigation: the version is pinned by `ARG`; bumping is a deliberate
  PR with a new sha256 — same workflow as today.
- **Multi-arch buildx is slow on cold cache.** Mitigation: `cache-to:
  type=gha,mode=max` keeps warm caches between releases; expected first-
  release build time ≈ 8 minutes, subsequent ≈ 2 minutes.
- **Helm chart's auto-generated Secret with `resource-policy: keep` will
  surprise operators who expect `helm uninstall` to leave nothing
  behind.** Mitigation: explicit note in `NOTES.txt` and in the chart
  README, with the kubectl command to delete the Secret manually.
- **macOS-arm64 release job consumes paid macOS minutes.** Mitigation:
  release-only (tag-triggered), not per PR. Expected ≈ 1 release per
  month during v0.x.
- **Operators on rootless Docker hit the read-only-rootfs friction.**
  Mitigation: compose example doesn't enable read-only-rootfs (only the
  Helm chart does, where the operator chose Kubernetes anyway).

## 12. Backlog (carried forward into the parent spec)

- High-availability / multi-replica via sqld primary/replica.
- Ingress + cert-manager + automated CA management.
- `NetworkPolicy` defaults in the Helm chart.
- Prometheus Operator `ServiceMonitor`.
- Windows server build (`x86_64-pc-windows-msvc`).
- macOS Intel build (`x86_64-apple-darwin`).
- NAS-vendor native packages (Synology spk, QNAP qpkg, Unraid plugin).
- Automated catalog schema upgrade across major versions.
- OCI Helm chart registry as primary distribution.
- Cosign-signed images and SBOM (CycloneDX) attached to releases.
- Image hardening via distroless or chainguard base.
- Runtime-smoke CI for linux-arm64 and macos-arm64 (requires hardware
  runners or self-hosted infrastructure).
