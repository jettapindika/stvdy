//! Object storage capability.

use std::time::Duration;

use bytes::Bytes;

/// Failure modes for object storage.
///
/// `NotFound` is separated from `Provider` so a caller can map a missing object
/// to a 404 without string matching; `Invalid` is a caller bug (bad key, bad
/// TTL) and is never retried.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Provider(String),
    #[error("{0}")]
    Invalid(String),
}

/// Keyed blob storage with presigned URLs.
///
/// Both backends (S3 and the local directory) must return a URL from the
/// `presign_*` calls that the client can use directly, so the client contract
/// does not change when the backend does.
#[async_trait::async_trait]
pub trait Storage: Send + Sync {
    async fn put(&self, key: &str, body: Bytes, mime: &str) -> Result<(), StorageError>;

    async fn get(&self, key: &str) -> Result<Bytes, StorageError>;

    async fn presign_get(&self, key: &str, ttl: Duration) -> Result<String, StorageError>;

    async fn presign_put(&self, key: &str, ttl: Duration) -> Result<String, StorageError>;

    async fn delete(&self, key: &str) -> Result<(), StorageError>;
}
