//! The store's tables.

pub(super) const SCHEMA: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE inputs (
  id INTEGER PRIMARY KEY,
  hash BLOB NOT NULL UNIQUE,
  size INTEGER NOT NULL,
  path TEXT NOT NULL,
  disk_size INTEGER NOT NULL,
  mtime INTEGER NOT NULL,
  format TEXT NOT NULL,
  records INTEGER NOT NULL,
  skipped INTEGER NOT NULL,
  first_ts INTEGER,
  last_ts INTEGER,
  summary TEXT NOT NULL,
  diag TEXT NOT NULL,
  ingested_at INTEGER NOT NULL
);
CREATE INDEX inputs_by_stat ON inputs (path, disk_size, mtime);
CREATE TABLE slices (
  input_id INTEGER NOT NULL REFERENCES inputs(id) ON DELETE CASCADE,
  hour INTEGER NOT NULL,
  data BLOB NOT NULL,
  PRIMARY KEY (input_id, hour)
);
CREATE INDEX slices_by_hour ON slices (hour);
";
