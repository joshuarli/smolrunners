use std::fmt;

use crate::store::StoreError;

pub type Result<T> = std::result::Result<T, CacheError>;

#[derive(Debug)]
pub enum CacheError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Store(StoreError),
    InvalidDigest(String),
    CorruptContent(String),
    ScopeDenied {
        repository: String,
        trust_tier: String,
        reason: String,
    },
    QuotaExceeded {
        limit: String,
    },
    NotQuiescent {
        generation: String,
        active_writers: u64,
    },
    MissingGeneration(String),
    InvalidState(String),
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "cache I/O error: {error}"),
            Self::Json(error) => write!(f, "cache JSON error: {error}"),
            Self::Store(error) => write!(f, "cache store error: {error}"),
            Self::InvalidDigest(value) => write!(f, "invalid content digest {value:?}"),
            Self::CorruptContent(value) => write!(f, "content {value} failed its digest check"),
            Self::ScopeDenied {
                repository,
                trust_tier,
                reason,
            } => write!(
                f,
                "cache scope {repository}/{trust_tier} denied: {reason}"
            ),
            Self::QuotaExceeded { limit } => write!(f, "cache quota exceeded: {limit}"),
            Self::NotQuiescent {
                generation,
                active_writers,
            } => write!(
                f,
                "generation {generation} has {active_writers} active writers"
            ),
            Self::MissingGeneration(id) => write!(f, "unknown cache generation {id}"),
            Self::InvalidState(message) => write!(f, "invalid cache state: {message}"),
        }
    }
}

impl std::error::Error for CacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Store(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for CacheError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for CacheError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<StoreError> for CacheError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}
