# Catalog, Sync & Stack — Design

**Status:** Draft (pending user review)
**Date:** 2026-05-17
**Sub-project:** #1 of N — foundational backbone for shoebox

## 1. Overview

Shoebox is a cross-platform desktop application for managing, developing, and
exporting digital RAW photos — broadly Lightroom-shaped, but with a
distinguishing capability: a single shared catalog accessed concurrently from
multiple machines, suitable for a family or a small studio of photographers
working off a NAS-hosted library.

The full product is a collection of interdependent subsystems (RAW pipeline,
develop module, library/browser UI, export pipeline, etc.), each of which gets
its own spec → plan → implementation cycle. This document specifies **only the
first sub-project**: the catalog and sync backbone, plus the cross-cutting
technology stack choices that constrain everything downstream.

When this sub-project is complete, an implementation will:

- Stand up a multi-client catalog server that runs on the user's NAS, in a
  Docker container, or on a Kubernetes cluster.
- Allow two or more desktop clients on macOS / Windows / Linux to discover the
  server via mDNS, enroll with mutual TLS, and maintain local libSQL replicas
  for snappy reads.
- Handle concurrent organizational edits (ratings, keywords, virtual copies,
  collections) without conflicts, and serialize concurrent develop-module edits
  via a pessimistic per-variant lock with a takeover-request mechanism.
- Index newly added RAW files on the NAS, generate shared thumbnail and preview
  caches, and emit those to clients on demand.

What it explicitly does **not** include: the RAW decoding pipeline, the develop
module, the polished library/browser UI, the export pipeline. Those each get
their own sub-specs and build on the contract this spec establishes.

## 2. Stack & technology decisions

| Decision | Choice | Rationale |
|---|---|---|
| Implementation language (server + client) | **Rust** | Required by libSQL embedded-replica client maturity, fits the cross-platform desktop story, has the strongest RAW-decoding crate ecosystem for future sub-projects. |
| Catalog database | **libSQL with embedded client replicas** | Provides server-authoritative writes plus near-instant local reads on clients (each client maintains a local SQLite replica that syncs over the libSQL wire protocol). Drops the cost of building a sync engine ourselves from months to weeks. |
| Server packaging | **Custom Rust binary (`shoebox-server`) that embeds sqld** plus an indexer and thumbnailer | We need the server to do more than serve the DB (filesystem watching, thumbnail generation, mTLS termination). Wrapping sqld gives us a single deployable artifact and a single trust boundary. |
| Client UI toolkit | **Iced** (pure-Rust, wgpu-rendered) | Render-heavy hot paths (large thumbnail grids, slider-driven develop preview, pan/zoom on full-res images) benefit from direct GPU access without a webview IPC hop. Single-language stack top-to-bottom. Accepts the cost of building more UI primitives ourselves vs. a webview stack. |
| Edit-storage model | **Catalog-authoritative** (all develop settings, ratings, keywords, etc. in libSQL) | Aligns with the multi-user shared-catalog architecture; sidecars reintroduce file-locking concerns on the NAS. An XMP sidecar exporter is in the backlog. |
| Collaboration model | **Hybrid: shared organization, per-user creative via virtual copies** | Keywords, folders, collections are catalog-shared. Ratings, flags, and color labels are per-(user, variant). Per-user "edit branches" are expressed as virtual copies — anyone can create one, all are visible, the creator is recorded but not exclusive. |
| Auth model | **Internal CA + mutual TLS, bootstrapped with a shared catalog secret** | Stronger than bearer-token over plaintext, identical first-run UX, identity is cryptographically bound to the client cert (not header-asserted). LAN-default; remote access is a separate future spec. |
| Server deployment | **Docker on NAS (primary), Helm chart for Kubernetes, standalone binary for non-Docker NASes** | Covers the realistic deployment landscape for the target audience. HA / scale-out is backlog. |
| Discovery | **mDNS broadcast + manual hostname fallback** | Zero-config on home/studio LANs, manual fallback covers VLANs and unusual networks. |

## 3. Architecture

Three actors on the network: client desktops, the shoebox-server, and the NAS
storage.

```
   ┌──────────────────────────────────────────────────────────────────────┐
   │                       NAS (file storage)                             │
   │                                                                      │
   │   /photos/2024/...                  ← user-owned RAW files (RO)      │
   │   /photos/2025/...                                                   │
   │                                                                      │
   │   /shoebox-cache/thumbnails/<hash>.jpg   ← shared thumbnail cache    │
   │   /shoebox-cache/previews/<hash>.jpg     ← shared 2k preview cache   │
   └──────────────────────────────────────────────────────────────────────┘
                ▲                                       ▲
                │ filesystem read (RO)                  │ filesystem read/write
                │                                       │
   ┌────────────┴───────────────────────────────────────┴─────────────────┐
   │                       shoebox-server (Rust)                          │
   │                                                                      │
   │   ┌──────────────┐  ┌──────────────┐  ┌──────────────────────────┐   │
   │   │ embedded     │  │ indexer      │  │ thumbnailer              │   │
   │   │ sqld         │  │ - FS watcher │  │ - extract embedded JPEG  │   │
   │   │ - libSQL     │  │ - periodic   │  │   preview from RAW (v1)  │   │
   │   │   protocol   │  │   rescan     │  │ - generate sized JPEGs   │   │
   │   │ - localhost  │  │ - BLAKE3     │  │ - write to NAS cache     │   │
   │   │   bind only  │  │   hashing    │  │                          │   │
   │   └──────────────┘  └──────────────┘  └──────────────────────────┘   │
   │                                                                      │
   │   ┌──────────────────────────────────────────────────────────────┐   │
   │   │ mTLS-terminating HTTPS/WS proxy + internal CA                │   │
   │   │ - enrollment / renewal / revocation endpoints                │   │
   │   │ - thumbnail/preview HTTP endpoints                           │   │
   │   │ - forwards libSQL protocol to localhost sqld                 │   │
   │   └──────────────────────────────────────────────────────────────┘   │
   │                                                                      │
   │   Also: mDNS broadcaster, /health, /metrics, janitor tasks           │
   └──────────────────────────────────────────────────────────────────────┘
                                      ▲
                                      │ mTLS-protected libSQL wire protocol
                                      │ mTLS-protected HTTP for thumbnails
                                      │ over LAN
                                      │
        ┌─────────────────────────────┼─────────────────────────────┐
        │                             │                             │
   ┌────┴────┐                  ┌─────┴───┐                   ┌─────┴───┐
   │ Client  │                  │ Client  │                   │ Client  │
   │ (Iced)  │                  │ (Iced)  │                   │ (Iced)  │
   │         │                  │         │                   │         │
   │ - local │                  │ - local │                   │ - local │
   │   libSQL│                  │   libSQL│                   │   libSQL│
   │   replca│                  │   replca│                   │   replca│
   │ - local │                  │ - local │                   │ - local │
   │   thumb │                  │   thumb │                   │   thumb │
   │   cache │                  │   cache │                   │   cache │
   │ - RAW   │                  │ - RAW   │                   │ - RAW   │
   │   render│                  │   render│                   │   render│
   └─────────┘                  └─────────┘                   └─────────┘
   Alice's Mac                  Bob's Win                     Carol's Mac
```

### 3.1 Component responsibilities

- **NAS** — dumb file storage. Holds user RAW files (read-only to shoebox) and
  a server-managed cache directory containing generated thumbnails and 2k
  previews keyed by photo content hash.
- **shoebox-server** — single Rust binary. Embeds sqld bound to localhost only.
  Runs an indexer (filesystem watcher + periodic rescan), a thumbnailer, an
  mTLS-terminating HTTPS/WS proxy in front of sqld and thumbnail endpoints, an
  internal certificate authority, an mDNS broadcaster, and periodic janitor
  tasks (lock expiry, abandoned session cleanup, orphaned thumbnail GC).
- **shoebox-client** — Iced desktop app. Maintains a local libSQL embedded
  replica (instant reads, writes round-trip to server). Local LRU thumbnail
  cache backed by HTTP fetches. Per-user mTLS client cert stored in OS
  keychain or app-data with strict file permissions.

### 3.2 Wire protocols

- **libSQL wire protocol** (HTTP / WebSocket) between client and shoebox-server
  for catalog operations and replication. Terminated by the mTLS proxy in
  shoebox-server; forwarded as plaintext to localhost sqld.
- **HTTP GET** from client to shoebox-server for thumbnail and preview fetches
  by content hash. Also mTLS-protected.
- **mDNS** broadcast from shoebox-server announcing `_shoebox._tcp.local` with
  TXT records: `name`, `schema`, `proto=libsql`.

## 4. Data model

The schema is grouped into five logical concerns. All `*_at` columns are Unix
milliseconds. UUIDs are 128-bit, stored as TEXT for portability.

### 4.1 Identity and config

```sql
-- Singleton key/value: server config, shared-secret hash (argon2id), etc.
config (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

-- Lightweight user profiles. No passwords.
users (
  id           TEXT PRIMARY KEY,        -- UUID
  display_name TEXT NOT NULL,
  avatar_blob  BLOB,                    -- small, optional
  created_at   INTEGER NOT NULL,
  last_seen_at INTEGER
);

-- Connected sessions. Lock holders are keyed by session so app-close
-- reliably releases locks regardless of multi-machine login.
sessions (
  id                TEXT PRIMARY KEY,        -- UUID, ephemeral per client launch
  user_id           TEXT NOT NULL REFERENCES users(id),
  client_machine_id TEXT NOT NULL,           -- stable per install
  established_at    INTEGER NOT NULL,
  last_active_at    INTEGER NOT NULL
);

-- Certificate revocation list. Server consults this on every mTLS handshake.
revoked_certs (
  serial_number TEXT PRIMARY KEY,      -- the leaf cert's serial as a hex string
  revoked_at    INTEGER NOT NULL,
  reason        TEXT,                  -- 'lost_device' | 'user_removed' | ...
  revoked_by    TEXT REFERENCES users(id)
);
```

### 4.2 Files on disk

```sql
-- Mirror of NAS folder structure under indexed roots.
folders (
  id              TEXT PRIMARY KEY,
  parent_id       TEXT REFERENCES folders(id),
  path            TEXT NOT NULL UNIQUE,    -- absolute path on NAS
  name            TEXT NOT NULL,
  last_indexed_at INTEGER
);

-- Photo identity = content hash. Stable across rename / move / re-import.
photos (
  id              TEXT PRIMARY KEY,        -- == BLAKE3 content hash
  file_size       INTEGER NOT NULL,
  file_format     TEXT NOT NULL,           -- 'PEF', 'RAF', 'DNG', ...
  captured_at     INTEGER,                 -- from EXIF
  camera_make     TEXT,
  camera_model    TEXT,
  lens            TEXT,
  iso             INTEGER,
  aperture        REAL,
  shutter_us      INTEGER,
  focal_length_mm REAL,
  width_px        INTEGER,
  height_px       INTEGER,
  orientation     INTEGER,                 -- EXIF rotation
  imported_at     INTEGER NOT NULL,
  exif_json       TEXT                     -- full EXIF for less-common fields
);

-- A photo can appear at multiple paths (duplicates) or move over time.
-- Indexer maintains this; cleanup job removes long-absent rows.
photo_files (
  id            TEXT PRIMARY KEY,
  photo_id      TEXT NOT NULL REFERENCES photos(id),
  folder_id     TEXT NOT NULL REFERENCES folders(id),
  path          TEXT NOT NULL UNIQUE,
  file_mtime    INTEGER NOT NULL,
  last_seen_at  INTEGER NOT NULL,
  is_present    INTEGER NOT NULL DEFAULT 1
);
```

### 4.3 Variants and develop edits

```sql
-- Master + virtual copies. variant_index = 0 is the master.
-- Anyone can create variants; creator is recorded but isn't exclusive.
variants (
  id                       TEXT PRIMARY KEY,        -- UUID
  photo_id                 TEXT NOT NULL REFERENCES photos(id),
  variant_index            INTEGER NOT NULL,        -- 0 = master, 1+ = virtual copies
  name                     TEXT,                    -- optional, "B&W version"
  created_by               TEXT NOT NULL REFERENCES users(id),
  created_at               INTEGER NOT NULL,
  develop_settings_json    TEXT NOT NULL,           -- JSON blob, see 4.6
  develop_settings_version INTEGER NOT NULL,
  develop_updated_at       INTEGER NOT NULL,
  develop_updated_by       TEXT NOT NULL REFERENCES users(id),
  UNIQUE (photo_id, variant_index)
);

-- Pessimistic soft lock for develop module. One row per locked variant.
-- Janitor releases on session-end or idle expiry.
develop_locks (
  variant_id             TEXT PRIMARY KEY REFERENCES variants(id),
  session_id             TEXT NOT NULL REFERENCES sessions(id),
  user_id                TEXT NOT NULL REFERENCES users(id),
  acquired_at            INTEGER NOT NULL,
  expires_at             INTEGER NOT NULL,
  takeover_requested_by  TEXT REFERENCES users(id),
  takeover_requested_at  INTEGER
);
```

### 4.4 Per-(user, variant) state

```sql
-- Star rating, flag, color label, all per-user per-variant.
-- Optimistic LWW per field; no locks (different rows = no semantic collision).
variant_user_state (
  variant_id   TEXT NOT NULL REFERENCES variants(id),
  user_id      TEXT NOT NULL REFERENCES users(id),
  rating       INTEGER,             -- 0..5, NULL = unrated
  flag         TEXT,                -- 'pick' | 'reject' | NULL
  color_label  TEXT,                -- 'red'|'yellow'|'green'|'blue'|'purple'|NULL
  updated_at   INTEGER NOT NULL,
  PRIMARY KEY (variant_id, user_id)
);
```

### 4.5 Shared organization

```sql
-- Hierarchical keywords ("Nature > Birds > Owls"). Catalog-shared.
keywords (
  id         TEXT PRIMARY KEY,
  parent_id  TEXT REFERENCES keywords(id),
  name       TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  UNIQUE (parent_id, name)
);

-- Keywords attach to the photo (not the variant): shared organizational view.
photo_keywords (
  photo_id   TEXT NOT NULL REFERENCES photos(id),
  keyword_id TEXT NOT NULL REFERENCES keywords(id),
  added_by   TEXT NOT NULL REFERENCES users(id),
  added_at   INTEGER NOT NULL,
  PRIMARY KEY (photo_id, keyword_id)
);

-- Collections are virtual buckets. Variants (not photos) are members,
-- so master and virtual copies can be collected separately, Lightroom-style.
collections (
  id         TEXT PRIMARY KEY,
  parent_id  TEXT REFERENCES collections(id),
  name       TEXT NOT NULL,
  created_by TEXT NOT NULL REFERENCES users(id),
  created_at INTEGER NOT NULL
);

collection_members (
  collection_id TEXT NOT NULL REFERENCES collections(id),
  variant_id    TEXT NOT NULL REFERENCES variants(id),
  added_by      TEXT NOT NULL REFERENCES users(id),
  added_at      INTEGER NOT NULL,
  sort_order    INTEGER NOT NULL,
  PRIMARY KEY (collection_id, variant_id)
);
```

### 4.6 `develop_settings_json` schema

A versioned JSON blob, indexed by `develop_settings_version`. v1 shape
(intentionally minimal — will be extended in the develop-module sub-spec):

```json
{
  "v": 1,
  "exposure_ev": 0.0,
  "contrast": 0.0,
  "highlights": 0.0,
  "shadows": 0.0,
  "whites": 0.0,
  "blacks": 0.0,
  "white_balance": { "mode": "as_shot",
                     "temp_k": 5500, "tint": 0 },
  "tone_curve": { "points": [[0,0],[1,1]] },
  "crop": { "left": 0.0, "top": 0.0, "right": 1.0, "bottom": 1.0,
            "rotation_deg": 0.0 },
  "masks": []
}
```

JSON instead of normalized columns because develop settings evolve fast and
adding a field should not require a schema migration. Cost: cross-library
queries on develop settings require a JSON scan, which is acceptable for an
infrequent operation.

### 4.7 Schema migrations

A standard `_schema_migrations(version INTEGER PRIMARY KEY, applied_at INTEGER)`
table records applied versions. The server applies forward migrations
atomically on startup. Migrations should be additive where possible (add
columns, add tables) so older clients keep working through a window of mixed
versions. Each client release declares a min and max compatible server schema
version; mismatched clients refuse to connect with a clear error UI.

### 4.8 Non-obvious choices

- `photos.id` is the BLAKE3 content hash itself. Saves a join, makes the
  thumbnail filename `<photo_id>.jpg` trivial, makes re-imports deterministic.
- `develop_locks` are keyed by `session_id`, not `user_id` — closing the app
  reliably releases locks even when a user is signed in on multiple machines.
- `keywords` attach to photos, `ratings`/`flags`/`color_labels` per-(user,
  variant) — matches the "shared organization, per-user creative" model.
- `photo_files` is separate from `photos` — supports duplicates and rename
  history without losing identity.

## 5. Sync, concurrency, and conflict policies

### 5.1 Replication mechanics

- **Reads** hit the local libSQL replica — instant, no network. The snapshot
  reflects the last replication tick (typically sub-second behind primary).
- **Writes** round-trip to the primary on the server. The call returns when
  the primary has durably committed (WAL append + fsync).
- **Replication back to other clients** happens via libSQL's WebSocket
  subscription, with HTTP polling as fallback. Other clients see new changes
  within ~1 second under healthy LAN conditions.
- **First connect / bootstrap** does a full snapshot transfer of the catalog
  to the client. A 200k-photo catalog is on the order of tens of MB —
  seconds on a LAN. Subsequent connects are incremental WAL catchup.

### 5.2 Develop lock protocol

```
Acquire:
  INSERT INTO develop_locks (variant_id, session_id, user_id,
                             acquired_at, expires_at)
  VALUES (?, ?, ?, now, now + 15min);
  -- PK conflict on variant_id ⇒ another session holds it; query who.

Heartbeat (while editing):
  UPDATE develop_locks SET expires_at = now + 15min
    WHERE variant_id = ? AND session_id = ?;
  -- Fires every ~30s from the client.

Release (on close, or backgrounding the develop view):
  DELETE FROM develop_locks
    WHERE variant_id = ? AND session_id = ?;

Server-side janitor (every minute):
  DELETE FROM develop_locks WHERE expires_at < now;
```

The PK on `variant_id` makes lock acquisition naturally atomic — the database
serializes contending inserts.

### 5.3 Takeover request flow

```
Bob wants in (sees Alice holds the lock):
  UPDATE develop_locks
    SET takeover_requested_by = bob_user_id,
        takeover_requested_at = now
    WHERE variant_id = ?
    AND   takeover_requested_by IS NULL;

Alice's client sees the change via WS replication push and shows a banner:
  "Bob is requesting access to this variant.
   [Release] [Decline] [Snooze 5 min]"

[Release]  → client DELETE its lock. Bob's next acquire attempt wins.
[Decline]  → client UPDATE clears takeover_requested_by; Bob's UI updates.
[Snooze]   → client UPDATE extends expires_at by 5 min, clears takeover fields.
[Ignored]  → lock expires naturally after idle window; Bob wins on retry.
```

The mechanism is plain SQL rows. No separate messaging system; replication
carries the state change.

### 5.4 Real-time UI updates

The client wraps the libSQL WebSocket subscription with a **change observer
layer** that:

- Watches replicated tables it cares about (variants, variant_user_state,
  photo_keywords, develop_locks, photo_files for new imports).
- Emits typed events into Iced's `iced::Subscription` API
  (`rating_changed`, `variant_added`, `lock_state_changed`, `photos_imported`,
  etc.).
- UI hot paths (filmstrip, develop banner, indexer activity indicator) listen
  and redraw incrementally.

Polling fallback every 5 seconds if the WebSocket connection drops. A "catalog
is X seconds behind" indicator surfaces only when lag exceeds 5 seconds.

### 5.5 Offline behavior

libSQL embedded replicas are read-replica only: writes require the primary.
The v1 policy:

- **Browse, search, view existing edits, view cached thumbnails** — all work
  offline from the local replica plus local thumbnail cache.
- **Rate, edit, add keywords, create variants** — fail with a clear "Server
  unreachable — reconnect to make changes" UI. **Writes are not queued in
  v1.**
- **Locks held when going offline** — the server-side janitor releases them
  after the 15-minute idle window. On reconnect the lock is gone; re-acquire
  if free.
- **Indexer changes** — happen entirely server-side; clients catch up on
  reconnect.

Offline write queueing is a known limitation; see backlog item 11.3.3.

### 5.6 Conflict policy summary

| Field / operation | Policy | Mechanism |
|---|---|---|
| Develop settings on a variant | Pessimistic soft lock | `develop_locks` row with single PK |
| Rating / flag / color label (per-user) | LWW (no semantic conflict — separate rows) | `variant_user_state`, PK `(variant_id, user_id)` |
| Keyword add / remove | Additive set | `INSERT OR IGNORE` / `DELETE` |
| Variant create / rename | Append-only / LWW on `name` | append; field-level LWW |
| Collection membership | LWW per (collection, variant) | upsert / delete |
| Folder structure | Server-only writes | only the indexer modifies |
| Photo metadata (EXIF, paths) | Server-only writes | only the indexer modifies |

The pessimistic lock is scoped tightly — only develop edits on a single
variant. Everything else is optimistic or additive, which keeps the system
feeling fluid for the 99% of organizational interactions.

## 6. Storage layout

```
NAS (or wherever the user keeps photos)
├── photos/                          ← user RAW files (server reads RO)
│   ├── 2024/
│   ├── 2025/
│   └── …
└── shoebox-cache/                   ← shared thumbnail/preview cache
    ├── thumbnails/<photo_hash>.jpg  ← ~256 px JPEG, ~30 KB
    └── previews/<photo_hash>.jpg    ← ~2k JPEG, ~300 KB

Server data volume (server-local filesystem — see note below)
├── catalog.db                       ← libSQL catalog
├── catalog.db-wal                   ← WAL companion file
├── catalog.db-shm                   ← shared memory file
├── ca/                              ← internal CA material
│   ├── ca.key                       ← root CA private key, mode 0600
│   ├── ca.crt                       ← root CA certificate
│   ├── server.key                   ← server cert private key, mode 0600
│   └── server.crt                   ← server certificate (auto-renewed)
├── backups/                         ← rotated VACUUM INTO snapshots
│   └── catalog-<timestamp>.db
└── server.toml                      ← server bind addr, paths,
                                       hashed shared secret

Client (per machine, OS app-data dir)
├── replica.db                       ← local libSQL replica
├── replica.db-wal
├── replica.db-shm
├── certs/                           ← per-user mTLS material
│   ├── client.key                   ← private key in OS keychain or 0600 file
│   ├── client.crt                   ← signed cert from server CA
│   └── ca.crt                       ← server's root CA, for trust pinning
├── thumbnail-cache/<hash>.jpg       ← LRU subset of NAS thumbnails
├── preview-cache/<hash>.jpg         ← LRU subset of NAS previews
├── render-cache/<variant_id>.jpg    ← in-progress develop renders
└── client.toml                      ← server addr, user_id, last session
```

**Important constraint on `catalog.db` placement:** SQLite and libSQL are
unsafe over NFS/SMB. The catalog database file MUST live on a filesystem the
server process can lock natively — local disk, or a local NAS filesystem path
mounted into the container as a bind-mount, **not** a remote network mount.
The cache directories and `photos/` root are fine on network mounts because
access is single-writer-many-reader (thumbnails) or read-only (photos).

Per deployment mode:

- **Docker on NAS:** the server data volume is a local NAS path bind-mounted
  into the container; `photos/` and `shoebox-cache/` are also bind-mounts of
  NAS paths.
- **Standalone binary:** the admin picks any local data directory on the
  machine running the binary; photos and cache point at NAS shares.
- **Helm chart on Kubernetes:** a PersistentVolumeClaim backed by block
  storage for the data volume; photos and cache come from a CSI driver for
  the NAS (NFS / SMB / iSCSI).

Cache directories are content-addressable by photo hash, so cache entries are
immutable, safe to share across versions, and safe to delete to reclaim space.

## 7. Authentication and discovery

Authentication is built on an internal certificate authority and mutual TLS,
bootstrapped via a shared catalog secret. mDNS handles service discovery only —
it makes no security claim.

### 7.1 Server bootstrap

On first launch the server:

1. Generates an Ed25519 root CA keypair and self-signs the root certificate.
   The CA private key is written to the data volume at `ca/ca.key` with mode
   0600. Lifetime: 10 years.
2. Issues a server certificate signed by the root CA. SANs cover the
   OS-reported hostname, `<server-name>.local` (the mDNS broadcast name),
   and every non-loopback IP enumerated from the host's network interfaces
   at startup. Operators can extend the SAN list via the
   `SHOEBOX_EXTRA_SANS` env var or `server.toml` `extra_sans:` key (useful
   for k8s ingress hostnames, reverse proxies, or pinned external DNS).
   Server cert lifetime: 90 days; the server renews itself automatically
   when 30 days remain and re-enumerates interfaces on each renewal so
   that newly-assigned IPs get covered.
3. Generates a shared catalog secret (passphrase) if one is not set via the
   `SHOEBOX_SECRET` environment variable, and prints it **once** to the log:
   *"Your shoebox catalog secret is: `<phrase>` — share this with other
   users."* The secret is stored as an argon2id hash in `config`; the
   plaintext is never logged again or written anywhere on disk.

### 7.2 Client enrollment

```
1. mDNS discovery → user picks the server.
2. Client connects to /enroll on the server's HTTPS port. This first
   connection trusts the server cert on a TOFU basis (no root CA yet).
3. User enters the shared catalog secret.
4. Client generates a local Ed25519 keypair, builds a CSR with
   subject CN=<user_id>, OU=<machine_id>, and picks/creates the user
   profile.
5. Client POSTs (CSR, shared_secret, profile_choice) to /enroll.
6. Server validates the argon2id secret, creates the user profile if new,
   signs the CSR with the root CA, and returns:
     - client_cert (signed leaf cert),
     - ca_root_cert,
     - server_cert_fingerprint (for one extra integrity check).
7. Client stores: client_key in OS keychain or mode-0600 file,
   client_cert and ca_root_cert in the certs/ directory.
   Pins the server's cert chain via ca_root.
```

After enrollment the shared secret is never needed again for that client.

### 7.3 Steady-state mTLS

All subsequent client connections (libSQL wire protocol and HTTP thumbnail
fetches):

- Server presents its server cert; client validates against the stored
  `ca_root`. Mismatch → refuse connection.
- Client presents its client cert; server validates against the same
  `ca_root` and reads `user_id` from the cert CN. Mismatch or unknown user
  → refuse connection.
- `user_id` is authoritatively read from the verified cert subject — it
  cannot be spoofed by header injection.
- All transport is TLS-encrypted.

### 7.4 Certificate lifecycle

- **Client cert lifetime: 90 days.** The client renews itself silently when
  30 days remain by signing a new CSR with the same key and POSTing to
  `/renew` over the existing mTLS connection. The server validates the
  authenticated identity and issues a new cert.
- **Revocation: a simple in-catalog CRL.** The server checks the
  `revoked_certs(serial_number TEXT PRIMARY KEY, revoked_at INTEGER,
  reason TEXT)` table on every connection. Admins can revoke certs via
  CLI command (`shoebox-server revoke --serial <n>`). Short cert
  lifetimes bound the blast radius of unrevoked compromise.
- **Lost cert / new machine:** user re-runs enrollment with the shared
  secret. A new cert is issued; the old one can optionally be revoked.
- **Clock skew:** certs are issued with a 5-minute `notBefore` backdate to
  absorb minor skew. NTP is recommended in install docs.
- **Root CA rotation:** out of scope for v1 (10-year horizon — see
  backlog item 11.3.6).

### 7.5 Discovery via mDNS

The server broadcasts service `_shoebox._tcp.local` with TXT records:
`name=<server display name>`, `schema=<version>`, `proto=libsql`. The
client on launch issues an mDNS query and presents discovered servers in
a picker. A manual fallback ("Add server by address" → hostname/IP + port)
covers networks that block mDNS.

### 7.6 Client first-run flow

```
1. App launches; no client.toml present.
2. Discovery picker shows mDNS-found servers plus "Add manually."
3. User picks a server.
4. App prompts for the shared catalog secret.
5. On success: enrollment completes, embedded replica opens, initial
   snapshot transfer runs.
6. Profile picker: "Who are you?" — choose existing user profile or
   create a new one.
7. App lands in library view.
```

Subsequent launches read `client.toml`, reconnect via mTLS, do an
incremental WAL catchup, and restore the last folder/collection view. If
the server is unreachable, the app shows an offline banner and works from
the local replica.

### 7.7 Trust boundary and scope

The v1 threat model is **trusted LAN**: anyone who already has the shared
secret can enroll and act as any user profile. This matches the family /
small-team / NAS-on-LAN target audience.

Stepping up to per-user passwords on top of the cert (auth model C) is a
future option — add `users.password_hash`, gate enrollment on individual
credentials. Internet-reachable deployment is its own future spec
(public-CA TLS, possibly OIDC, stronger auth — see backlog item 11.3.5).

## 8. Deployment

### 8.1 Docker on NAS (primary install path)

A multi-arch image (`linux/amd64`, `linux/arm64`) is published to a registry
under a stable name. A `docker-compose.yml` template ships with the project:

```yaml
services:
  shoebox-server:
    image: ghcr.io/<org>/shoebox-server:latest
    ports:
      - "9000:9000"           # mTLS HTTPS + libSQL WS
      - "5353:5353/udp"       # mDNS
    volumes:
      - /volume1/docker/shoebox/data:/var/lib/shoebox
      - /volume1/photos:/photos:ro
      - /volume1/shoebox-cache:/shoebox-cache
    environment:
      - SHOEBOX_DATA_DIR=/var/lib/shoebox
      - SHOEBOX_PHOTOS_DIR=/photos
      - SHOEBOX_CACHE_DIR=/shoebox-cache
      # - SHOEBOX_SECRET=...  # optional; auto-generated on first start
    restart: unless-stopped
```

The image's `CMD` runs `shoebox-server` directly. mDNS requires host networking
or appropriate avahi-reflector setup on the NAS; install docs cover both
Synology and QNAP specifics.

### 8.2 Standalone binary

A static binary for `linux-x86_64`, `linux-aarch64`, `darwin-arm64`,
`windows-x86_64`. Distributed as a tarball with the binary plus a sample
`server.toml`. The admin SCPs it to any always-on machine, points it at NAS
shares for `--photos-dir` and `--cache-dir`, and runs it as a service via
whatever init system the host has (systemd unit file shipped in the tarball).

### 8.3 Helm chart for Kubernetes

A standard chart targeting vanilla Kubernetes primitives:

- `Deployment` with `replicas: 1` (single-writer model; HA is backlog).
- `Service` of type ClusterIP / LoadBalancer / NodePort per the user's
  install option, exposing the mTLS port. mDNS is optional in k8s
  deployments — most clusters route service discovery internally.
- `PersistentVolumeClaim` backed by block storage for the data volume.
- `ConfigMap` for non-secret server config.
- `Secret` for the initial enrollment shared secret (or auto-generated on
  first start).
- CSI-backed PV mounts for `/photos` (read-only) and `/shoebox-cache`.

Chart is documented for k3s, k0s, and Rancher; production-grade HA setups
are out of scope until the HA backlog item lands.

## 9. Error handling, failure modes, and observability

### 9.1 Failure modes and recovery

| Failure | Detection | User-visible behavior | Recovery |
|---|---|---|---|
| Server unreachable | libSQL connection error; HTTP timeout | Offline banner; reads work from local replica, writes disabled | Auto-reconnect with exp backoff (1s → 30s cap); replica catches up on reconnect |
| NAS unreachable from server | Indexer FS errors; thumbnailer can't read RAWs | Server logs; indexer pauses; existing catalog state intact | Server retries with backoff; full reconciliation pass when NAS returns |
| Local replica corruption | libSQL open or integrity check fails | "Local cache corrupted — rebuilding" toast | Replica file deleted, fresh snapshot pulled |
| Catalog DB corruption (server) | sqld startup integrity check fails | Server refuses to start | Manual restore from latest backup (see 9.2) |
| Disk full on server data volume | Write returns SQLITE_FULL; health flips to degraded | Writes fail with clear error; reads still work | Clear thumbnail cache; expand volume |
| Indexer falls behind | Queue depth metric rises | "Indexing N photos — ~M min remaining" status indicator | Time |
| Thumbnail/preview gen failure | Per-file error from thumbnailer | Broken placeholder in grid; clicking shows underlying error | Retry on rescan; mark "ignore" to suppress |
| Client mTLS cert expired | TLS handshake fails | "Your shoebox cert expired — please re-enroll" toast | Re-run enrollment with shared secret |
| Clock skew | TLS handshake fails | Same as cert expired | NTP recommendation in docs; 5-min `notBefore` backdate absorbs minor skew |
| Schema migration in progress | health reports mid-upgrade state | "Server is upgrading — try again in a moment" | Auto-retry; migrations should be quick |
| Stale develop lock (crashed client) | Janitor runs every 60s | Lock appears held for up to 15 min + 60 s | Other users can submit takeover request immediately, or wait |
| Replication lag spike | WS subscription falls behind | "Catalog is X seconds behind" indicator when > 5s | Auto-recover; manual "force resync" button available |
| Photo file disappeared | Thumbnailer or decoder gets ENOENT | `photo_files.is_present = 0`; "file missing" overlay | Indexer rescans periodically; mark present if file returns at same hash |

### 9.2 Backup and recovery

The catalog is the precious artifact — losing it means losing every edit,
rating, keyword, and variant. The photo files themselves are independently
backed up by whatever the user does for the NAS.

**Built-in backup:**

- The server runs `VACUUM INTO <backup_dir>/catalog-<timestamp>.db` every
  6 hours (configurable). Backups go to `./backups/` under the data
  volume; the last 14 are retained by default.
- Each backup is a consistent point-in-time snapshot.
- Backup file size is small relative to the photo library (~100-300 MB
  for a 200k-photo catalog).
- An optional `backup_to:` path in `server.toml` writes backups to a
  separate location (e.g., a second NAS share). Recommended in install
  docs.

**Restore procedure (documented in operator runbook):**

1. Stop the server.
2. Replace `catalog.db` in the data volume with the chosen backup file.
   Delete the `-wal` and `-shm` companions.
3. Start the server. Forward migrations apply if the backup is older than
   the current schema version.
4. Clients detect a fresh server snapshot and re-bootstrap their replicas
   automatically.

**Not backed up:** the thumbnail/preview cache (regenerable from RAWs)
and client replicas (rebuilt from server on next connect).

### 9.3 Observability

- **`/health`** returns
  `{status: ok|degraded|down, schema_version, sqld_ok, indexer_ok, disk_space_pct}`.
  Used by Docker healthchecks and Kubernetes readiness probes.
- **`/metrics`** returns Prometheus-format counters and gauges:
  `indexer_queue_depth`, `indexer_rate_files_per_min`,
  `thumbnailer_queue_depth`, `active_sessions`, `active_develop_locks`,
  `replication_clients_count`, `replication_lag_seconds_p50`,
  `replication_lag_seconds_p95`, `disk_bytes_free`,
  `cert_days_until_expiry`. Default-on; can be disabled.
- **Structured logs** (JSON, stderr) with event-class fields
  (`event=lock.acquired`, `event=variant.created`,
  `event=enrollment.completed`) and correlation IDs across operations.
  Default level INFO; per-component log levels via env vars.
- **No telemetry sent anywhere external.** Privacy by default; users with
  Prometheus stacks can scrape `/metrics`.

## 10. Testing strategy

**Unit tests** (pure functions, small modules):

- Schema migration application — each migration tested forward, and
  roundtrip where reversible.
- `develop_settings_json` validation, version upgrade, default merging.
- BLAKE3 hashing against known vectors.
- Lock acquire / release / expire / takeover-request state transitions.
- mTLS cert generation, CSR signing, CRL evaluation.

**Integration tests** — in-process harness spawning one shoebox-server and
N libSQL replica clients over loopback:

- End-to-end enrollment flow (mDNS → shared-secret → CSR → mTLS).
- Two clients editing different photos in parallel — both succeed.
- Two clients targeting the same variant — lock acquire ordering,
  takeover request flow, idle expiry.
- Indexer reacts to FS events: drop a RAW into the watched dir, observe
  the catalog row appears within N seconds.
- Replica catchup on reconnect after simulated network drop.
- Schema compat matrix: `client@vN-1` against `server@vN`, and
  `client@vN` against `server@vN-1`, for the last 3 supported deltas.

**Property tests** (via `proptest`):

- Concurrent operation interleavings — random sequences of
  `[rate, keyword_add, variant_create, develop_lock_acquire]` across 2-5
  simulated clients; assert invariants (no double-locked variant, no
  duplicate keyword on a photo, etc.).
- Indexer state machine — random sequences of
  `[create, rename, move, delete, modify]` events; assert eventual
  consistency with FS state.

**Fault injection:**

- `toxiproxy` (or equivalent) between client and server for latency,
  dropped packets, bandwidth caps.
- A chaos test that randomly kills the server every 60s while clients
  run — clients must reconnect cleanly.
- Disk-full simulation via small-tmpfs data volume.

**Load and scale tests** (run pre-release, not on every CI):

- 200k-photo synthetic catalog — bootstrap a fresh client, measure
  snapshot transfer time.
- 5 concurrent clients each doing realistic actions (browse + rate +
  tag) for 1 hour; measure replication lag distribution and server
  CPU/RAM.

**Out of test scope (deferred to other sub-specs):** RAW format correctness
on real Pentax / Fuji files — that belongs to the RAW pipeline spec.

## 11. Scope boundary, backlog, definition of done

### 11.1 In scope

Everything needed to stand up a working, multi-client catalog backbone that
the rest of shoebox will be built on top of.

**Server (`shoebox-server`):**

- Embedded sqld with the schema from §4.
- Filesystem indexer: watches photo folders, hashes new files (BLAKE3),
  populates `photos` / `photo_files` / `folders`.
- Thumbnailer: generates 256 px thumbnails + 2k previews to the shared
  cache directory. v1 uses the embedded JPEG preview extracted from the
  RAW file (fast, no decoder dependency); full RAW decode arrives with
  the RAW pipeline spec.
- Internal CA + mTLS-terminating HTTPS/WS proxy in front of sqld.
- Enrollment / renewal / revocation endpoints.
- mDNS broadcaster.
- `/health` and `/metrics` endpoints, structured logs.
- Periodic backup of `catalog.db` via VACUUM INTO.
- Janitor tasks: stale lock cleanup, abandoned session cleanup,
  orphaned thumbnail GC.

**Client (`shoebox-client`):**

- Iced shell with: discovery picker, enrollment flow, profile picker,
  library list view that shows catalog state.
- libSQL embedded replica wiring, mTLS client config, cert storage.
- Change-observer layer translating replicated table updates into Iced
  subscriptions.
- Local thumbnail cache (LRU) backed by HTTP fetches from server.
- Develop lock acquire / heartbeat / release / takeover-request UI
  banners — even though the actual develop module is in another spec,
  the locking primitives ship here.
- A minimum *demo* library view: folder tree, photo grid populated from
  thumbnails, click-to-see-EXIF, rate a photo, add a keyword, create a
  virtual copy. Enough to prove the catalog round-trips end-to-end.
  **Not the polished library experience.**

**Deployment artifacts:**

- Multi-arch Docker image.
- `docker-compose.yml` template with NAS-typical paths.
- Standalone static binaries for Linux / macOS / Windows server hosts.
- Helm chart targeting standard Kubernetes primitives.
- Install docs for Synology, QNAP, TrueNAS Scale, and "any Linux host."

**Test coverage:** all of §10.

### 11.2 Explicitly out of scope (each gets its own sub-spec)

| Sub-system | Why deferred | Depends on |
|---|---|---|
| **RAW pipeline** (PEF / RAF / DNG decode, demosaic, color management, lens profiles) | Substantial work in its own right; dedicated design needed (decoder choice, color science, perf budgets) | This spec |
| **Develop module UI + rendering** (sliders, curves, masks, real-time preview) | Big UX surface; needs careful design of the GPU rendering pipeline through wgpu | RAW pipeline + this spec |
| **Library / browser UI polish** (smooth-scroll 100k-thumbnail grid, filmstrip, search, faceted filter, multi-select operations) | Custom Iced widgets, real engineering effort | This spec |
| **Export pipeline** (render variants → JPEG/TIFF/HEIC, metadata embedding, presets, batch queue) | Self-contained subsystem with its own UX | Develop module + RAW pipeline |
| **Tethered shooting, plugins, AI features** | Far-future | Everything |

### 11.3 Backlog

Features that have come up during this brainstorm but are explicitly post-v1:

1. **XMP sidecar exporter** (catalog → XMP next to each RAW for interop /
   backup with Lightroom / darktable / RawTherapee).
2. **High-availability / scale-out** for shoebox-server (multi-writer
   libSQL, leader election, hot standby).
3. **Offline write queueing** on clients (local write log + conflict
   resolution UI on reconnect).
4. **Real per-user passwords** (upgrade from auth model B → C) — enables
   mixed-trust user populations.
5. **Remote access** (catalog reachable beyond LAN) — needs public-TLS,
   possibly OIDC, stronger threat model.
6. **CA root rotation** (10-year horizon for the v1 self-signed CA).
7. **Two-way XMP sync** (read external XMP edits back into the catalog).
8. **Parallel format-specific decoder pools** in the indexer for
   huge-batch imports.
9. **Admin web UI** for managing users, sessions, certs, revocations, and
   backups (v1 ships with CLI commands only).

### 11.4 Definition of done

The spec is complete when an implementation matching it can demonstrate:

- Two clients on different OSes (macOS + Windows) discover the server on
  a LAN via mDNS, complete enrollment with the shared secret, and
  establish authenticated mTLS sessions.
- A user drops 100 RAW files into a watched NAS folder; both clients see
  them appear in the library within ~10 seconds with thumbnails.
- Both clients rate, keyword, and create virtual copies on the same set
  of photos concurrently — all changes propagate, none are lost.
- Client A acquires a develop lock on variant X; Client B sees the lock
  and successfully requests takeover; A releases; B acquires.
- Kill the server mid-session; both clients show the offline banner;
  server restarts; both clients reconnect and resume without manual
  action.
- Restore a 24-hour-old backup; clients re-bootstrap their replicas
  cleanly.
- Helm chart deploys to a k3s cluster with a NAS-backed PVC; the same
  functional tests pass.

No develop module. No real library UI. No RAW pipeline. Just the
backbone — solid enough that the rest of shoebox can be built on top of
it without revisiting these decisions.
