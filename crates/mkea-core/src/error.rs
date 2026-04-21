use thiserror::Error;

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error("invalid image: {0}")]
    InvalidImage(String),
    #[error("memory error: {0}")]
    Memory(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
}
