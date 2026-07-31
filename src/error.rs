use thiserror::Error;

#[derive(Error, Debug)]
pub enum AgfError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("No home directory found")]
    NoHomeDir,

    /// Platform data dir (`dirs::data_dir` / `dirs::data_local_dir`) is
    /// unavailable. Distinct from `NoHomeDir` so the message doesn't claim the
    /// home directory is missing when it isn't.
    #[error("No data directory found")]
    NoDataDir,
}
