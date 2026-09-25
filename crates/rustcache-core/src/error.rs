use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("tls: {0}")]
    Tls(String),

    #[error("cert: {0}")]
    Cert(String),

    #[error("cache: {0}")]
    Cache(String),

    #[error("config: {0}")]
    Config(String),

    #[error("protocol: {0}")]
    Protocol(String),

    #[error("excluded: {0}")]
    Excluded(String),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}
