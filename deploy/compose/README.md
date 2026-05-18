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
${EDITOR:-nano} .env

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
