-- File system mirror and photo identity.

CREATE TABLE folders (
    id              TEXT PRIMARY KEY,
    parent_id       TEXT REFERENCES folders(id),
    path            TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    last_indexed_at INTEGER
);

CREATE INDEX folders_parent_idx ON folders(parent_id);

CREATE TABLE photos (
    id              TEXT PRIMARY KEY,
    file_size       INTEGER NOT NULL,
    file_format     TEXT NOT NULL,
    captured_at     INTEGER,
    camera_make     TEXT,
    camera_model    TEXT,
    lens            TEXT,
    iso             INTEGER,
    aperture        REAL,
    shutter_us      INTEGER,
    focal_length_mm REAL,
    width_px        INTEGER,
    height_px       INTEGER,
    orientation     INTEGER,
    imported_at     INTEGER NOT NULL,
    exif_json       TEXT
);

CREATE INDEX photos_captured_idx ON photos(captured_at);
CREATE INDEX photos_camera_idx   ON photos(camera_make, camera_model);

CREATE TABLE photo_files (
    id            TEXT PRIMARY KEY,
    photo_id      TEXT NOT NULL REFERENCES photos(id),
    folder_id     TEXT NOT NULL REFERENCES folders(id),
    path          TEXT NOT NULL UNIQUE,
    file_mtime    INTEGER NOT NULL,
    last_seen_at  INTEGER NOT NULL,
    is_present    INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX photo_files_photo_idx  ON photo_files(photo_id);
CREATE INDEX photo_files_folder_idx ON photo_files(folder_id);
