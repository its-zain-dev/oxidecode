use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

/// SQLite storage. SQLite itself is the C component: rusqlite's `bundled`
/// feature compiles the amalgamation with the same flags as the rest of the
/// native layer, giving us a fast, dependency-free database.
pub struct Db {
    connection: Mutex<Connection>,
    path: Option<std::path::PathBuf>,
}

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY,
  directory TEXT NOT NULL,
  data_json TEXT NOT NULL,
  time_created INTEGER NOT NULL,
  time_updated INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS projects_directory ON projects(directory);

CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  parent_id TEXT,
  directory TEXT NOT NULL,
  title TEXT NOT NULL DEFAULT '',
  time_created INTEGER NOT NULL,
  time_updated INTEGER NOT NULL,
  data_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS sessions_project ON sessions(project_id, time_updated DESC);
CREATE INDEX IF NOT EXISTS sessions_updated ON sessions(time_updated DESC);

CREATE TABLE IF NOT EXISTS messages (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  time_created INTEGER NOT NULL,
  data_json TEXT NOT NULL,
  FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS messages_session ON messages(session_id, seq);
CREATE UNIQUE INDEX IF NOT EXISTS messages_session_seq ON messages(session_id, seq);

CREATE TABLE IF NOT EXISTS permissions (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  time_created INTEGER NOT NULL,
  decision TEXT,
  data_json TEXT NOT NULL,
  FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS permissions_session ON permissions(session_id, time_created);

CREATE TABLE IF NOT EXISTS todos (
  session_id TEXT NOT NULL,
  position INTEGER NOT NULL,
  data_json TEXT NOT NULL,
  PRIMARY KEY(session_id, position),
  FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS kv (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
"#;

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating database directory {}", parent.display()))?;
        }
        let connection = Connection::open(path)
            .with_context(|| format!("opening database {}", path.display()))?;
        Self::initialize(connection, Some(path.to_path_buf()))
    }

    pub fn open_memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?, None)
    }

    fn initialize(connection: Connection, path: Option<std::path::PathBuf>) -> Result<Self> {
        connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
            .ok();
        connection.execute_batch(SCHEMA)?;
        Ok(Self {
            connection: Mutex::new(connection),
            path,
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        let guard = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&guard).map_err(Into::into)
    }

    pub fn with_mut<T>(&self, f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> Result<T> {
        let mut guard = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut guard).map_err(Into::into)
    }

    pub fn integrity_check(&self) -> Result<String> {
        self.with(|connection| {
            connection.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
        })
    }
}
