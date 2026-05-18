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
  -e SHOEBOX_HEALTH_BIND_ADDR=0.0.0.0:9001 \
  -v shoebox-data:/var/lib/shoebox \
  -v shoebox-cache:/shoebox-cache \
  -v /path/to/your/photos:/photos \
  ghcr.io/<owner>/shoebox-server:v0.1.0
```

The `SHOEBOX_HEALTH_BIND_ADDR=0.0.0.0:9001` override is required for
the host-side port mapping to reach the health server; the default
`127.0.0.1:9001` would only be reachable from inside the container.
The outer `127.0.0.1:9001:9001` mapping still restricts host exposure
to loopback only.

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
${EDITOR:-nano} .env  # set SHOEBOX_PHOTOS_DIR to your photo library path

# Replace CHANGE-ME-OWNER with the actual repo owner in docker-compose.yml
sed -i 's|ghcr.io/CHANGE-ME-OWNER|ghcr.io/<owner>|' docker-compose.yml

docker compose up -d
```

See `deploy/compose/README.md` for the full walkthrough and upgrade
instructions. The compose file already includes the
`SHOEBOX_HEALTH_BIND_ADDR` override so curl from the host works
out of the box.

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
