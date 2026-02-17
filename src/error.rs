use thiserror::Error;

#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum AgfError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Hex decode error: {0}")]
    Hex(#[from] hex::FromHexError),

    #[error("No home directory found")]
    NoHomeDir,

    #[error("Scanner error ({agent}): {message}")]
    Scanner { agent: String, message: String },
}
