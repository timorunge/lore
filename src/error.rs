//! Typed error enum for the lore library.

use std::path::PathBuf;

use thiserror::Error;

/// Typed error variants for library operations.
#[derive(Debug, Error)]
pub enum LoreError {
    #[error("search query must not be empty")]
    EmptyQuery,
    #[error("invalid search query syntax")]
    InvalidQuery,
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Ambiguous(String),
    #[error("store not found at {path}")]
    StoreNotFound { path: PathBuf },
    #[error("store schema is outdated -- run `lore ingest --recreate` to rebuild")]
    SchemaOutdated,
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<tantivy::TantivyError> for LoreError {
    fn from(e: tantivy::TantivyError) -> Self {
        Self::Internal(anyhow::Error::from(e))
    }
}
