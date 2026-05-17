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
docker run --rm -p 9000:9000 \
  -v /tmp/shoebox-data:/var/lib/shoebox \
  shoebox-server:dev
```

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```
