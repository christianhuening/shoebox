# shoebox

Cross-platform desktop application for managing, developing, and exporting
RAW digital photos. Multi-user shared catalog hosted on a NAS.

**Status:** in active development. See `CLAUDE.md` for sub-project status and
`docs/superpowers/specs/` for design documents.

## What works (Plan 1.3 complete)

The shoebox-server data plane is functional:

- **Catalog:** libSQL embedded `sqld` subprocess hosting the catalog DB;
  clients connect via the mTLS-protected wire proxy at `/v1/*` and `/v2/*`.
- **Indexer:** walks a NAS-mounted photo root on startup, then watches for FS
  changes. Recognises `.PEF` / `.RAF` / `.DNG` and BLAKE3-hashes each file.
- **Thumbnailer:** extracts embedded JPEG previews from RAW files, generates
  256 px (thumbnail) and 2 k (preview) JPEGs in a shared cache directory.
- **HTTP API (mTLS):** `/enroll`, `/renew`, `/whoami`, `/thumbs/<hash>`,
  `/previews/<hash>`, `/locks/:variant_id` (acquire/heartbeat/release/takeover).
- **Background tasks:** janitor (releases expired locks, sweeps abandoned
  sessions, GCs orphaned thumbnails), 6-hour `VACUUM INTO` backups with
  14-snapshot retention, 12-hour cert auto-renewal.
- **Observability:** `/health` and Prometheus `/metrics` on a loopback port
  (no auth needed for in-host scrapers).

Run locally:

```bash
cargo run -p shoebox-server
curl -sf http://127.0.0.1:9001/health
curl -sf http://127.0.0.1:9001/metrics | head
```

Run in Docker (requires `sqld` baked into the image, which the included
Dockerfile installs):

```bash
docker build -t shoebox-server:dev .
docker run --rm -p 9000:9000 -v shoebox-data:/var/lib/shoebox shoebox-server:dev
```

## Docker deployment details

The image build downloads a pinned `sqld` release binary into
`/usr/local/bin/sqld`; the version is controlled by the `SQLD_VERSION` build
arg (defaults to `v0.24.32`). The runtime stage exposes the mTLS-protected
catalog port (9000) and the unauthenticated loopback `/health` + `/metrics`
port (9001, only useful from container healthchecks or in-host scrapers).

On first run, the server prints a generated enrollment secret to the log
exactly once. Share it with users out-of-band; they'll need it to enroll
their clients. To pre-set the secret, pass `-e SHOEBOX_SECRET=your-phrase`.

Run with a host-mounted directory (matches a typical NAS deployment).
The container runs as UID 10001 (`shoebox`), so the host directory must
be owned by that UID:

```bash
mkdir -p /srv/shoebox-data
sudo chown 10001:10001 /srv/shoebox-data
docker run --rm -p 9000:9000 \
  -v /srv/shoebox-data:/var/lib/shoebox \
  shoebox-server:dev
```

A docker-compose template for typical NAS deployments (Synology, QNAP,
TrueNAS) ships in Plan 1.5.

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```
