# shoebox

Cross-platform desktop application for managing, developing, and exporting
RAW digital photos. Multi-user shared catalog hosted on a NAS.

**Status:** in active development. See `CLAUDE.md` for sub-project status and
`docs/superpowers/specs/` for design documents.

## Running the server locally

```bash
cargo run -p shoebox-server
curl -s http://127.0.0.1:9000/health
```

## Building the Docker image

```bash
docker build -t shoebox-server:dev .
```

Run with a Docker named volume (recommended for local testing — no host
permission issues):

```bash
docker run --rm -p 9000:9000 \
  -v shoebox-data:/var/lib/shoebox \
  shoebox-server:dev
```

Run with a host-mounted directory (matches a typical NAS deployment).
The container runs as UID 10001 (`shoebox`), so the host directory must
be owned by that UID or the server will fail to open `catalog.db` with
`SQLITE_CANTOPEN`:

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
