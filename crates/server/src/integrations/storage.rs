//! Object storage behind `stvdy_core::traits::storage::Storage`.
//!
//! Two backends, one client contract. A15 removed MinIO's community images, so
//! the local backend is what makes ingestion testable on a laptop and in CI
//! without a container. Clients never see the difference: both hand out a
//! short-lived URL, and on the local backend that URL points back at this
//! server, which verifies the signature and streams the bytes.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use secrecy::ExposeSecret;
use sha2::Sha256;
use stvdy_core::traits::storage::{Storage, StorageError};

use crate::config::{StorageBackend, StorageConfig};

pub fn from_config(cfg: &StorageConfig) -> anyhow::Result<std::sync::Arc<dyn Storage>> {
    match cfg.backend {
        StorageBackend::Local => Ok(std::sync::Arc::new(LocalStorage::new(cfg.clone())?)),
        StorageBackend::S3 => Ok(std::sync::Arc::new(S3Storage::new(cfg.clone())?)),
    }
}

/// The URL a client fetches a source's original file from.
///
/// One function decides, so the client contract cannot diverge between backends:
/// both produce a short-lived URL that needs no credentials. The local backend
/// signs for this server; S3 signs for the bucket.
pub async fn source_file_url(
    cfg: &StorageConfig,
    storage: &dyn Storage,
    source_id: &str,
    key: &str,
) -> Result<String, StorageError> {
    match cfg.backend {
        StorageBackend::Local => local_signed_url(
            &cfg.public_base_url,
            cfg.sign_key.expose_secret().as_bytes(),
            source_id,
            cfg.presign_ttl,
        ),
        StorageBackend::S3 => storage.presign_get(key, cfg.presign_ttl).await,
    }
}

/// Filesystem storage. Keys are hashed into a two-hex-character prefix directory
/// so a single flat directory does not accumulate every object in the system.
pub struct LocalStorage {
    dir: PathBuf,
    sign_key: Vec<u8>,
    base_url: String,
}

impl LocalStorage {
    pub fn new(cfg: StorageConfig) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&cfg.local_dir).map_err(|error| {
            anyhow::anyhow!(
                "creating the object directory {}: {error}",
                cfg.local_dir.display()
            )
        })?;
        Ok(Self {
            dir: cfg.local_dir,
            sign_key: cfg.sign_key.expose_secret().as_bytes().to_vec(),
            base_url: cfg.public_base_url,
        })
    }

    fn path_for(&self, key: &str) -> Result<PathBuf, StorageError> {
        // A key arrives from a client-controlled filename, so it must not be
        // able to escape the object directory.
        let safe = sanitize_key(key)?;
        let prefix = &safe[..2.min(safe.len())];
        Ok(self.dir.join(prefix).join(safe))
    }
}

/// Rejects a key that could resolve outside the object directory. Keys are
/// generated server-side today, but this is the boundary where a traversal would
/// become a read of any file on the host.
fn sanitize_key(key: &str) -> Result<String, StorageError> {
    if key.is_empty() || key.len() > 512 {
        return Err(StorageError::Invalid("object key length".to_string()));
    }
    if key.contains("..") || key.starts_with('/') || key.contains('\\') {
        return Err(StorageError::Invalid(format!("object key {key:?}")));
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
    {
        return Err(StorageError::Invalid(format!("object key {key:?}")));
    }
    Ok(key.to_string())
}

/// HMAC-SHA256 over `payload`.
///
/// `new_from_slice` reports a length the hash cannot accept rather than
/// panicking, and the workspace forbids `expect` outside tests, so the failure
/// is propagated: a signature that cannot be computed must not silently become
/// an empty string that then fails to verify.
fn sign(key: &[u8], payload: &str) -> Result<String, StorageError> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key)
        .map_err(|error| StorageError::Provider(format!("HMAC key: {error}")))?;
    mac.update(payload.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A presigned URL for the local backend: the server itself is the signer and
/// the reader. The shape (opaque URL plus expiry, no credentials) matches S3's,
/// which is the point: the client cannot tell the backends apart.
///
/// The path is `/objects/…`, not `/sources/…`: that one is the authenticated
/// JSON endpoint that *hands out* a URL, and overloading it would make the two
/// indistinguishable in a log.
pub fn local_signed_url(
    base_url: &str,
    sign_key: &[u8],
    source_id: &str,
    ttl: Duration,
) -> Result<String, StorageError> {
    let exp = now_secs() + ttl.as_secs();
    let sig = sign(sign_key, &format!("{source_id}:{exp}"))?;
    Ok(format!(
        "{base_url}/objects/{source_id}?exp={exp}&sig={sig}"
    ))
}

/// Verifies a signature produced by [`local_signed_url`]. Kept here rather than
/// in the route so the signing and the checking cannot drift apart.
pub fn verify_local_signature(
    sign_key: &[u8],
    source_id: &str,
    exp: u64,
    sig: &str,
) -> Result<(), StorageError> {
    if exp < now_secs() {
        return Err(StorageError::Invalid("signed URL expired".to_string()));
    }
    let expected = sign(sign_key, &format!("{source_id}:{exp}"))?;
    // Constant-time compare: a byte-by-byte comparison would leak the signature.
    let matches = expected.len() == sig.len()
        && expected
            .bytes()
            .zip(sig.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0;
    if matches {
        Ok(())
    } else {
        Err(StorageError::Invalid("bad signature".to_string()))
    }
}

#[async_trait::async_trait]
impl Storage for LocalStorage {
    async fn put(&self, key: &str, body: Bytes, _mime: &str) -> Result<(), StorageError> {
        let path = self.path_for(key)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| StorageError::Provider(e.to_string()))?;
        }
        tokio::fs::write(&path, &body)
            .await
            .map_err(|e| StorageError::Provider(e.to_string()))
    }

    async fn get(&self, key: &str) -> Result<Bytes, StorageError> {
        let path = self.path_for(key)?;
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(Bytes::from(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(StorageError::NotFound(key.to_string()))
            }
            Err(error) => Err(StorageError::Provider(error.to_string())),
        }
    }

    async fn presign_get(&self, key: &str, ttl: Duration) -> Result<String, StorageError> {
        let _ = self.path_for(key)?;
        local_signed_url(&self.base_url, &self.sign_key, key, ttl)
    }

    async fn presign_put(&self, key: &str, ttl: Duration) -> Result<String, StorageError> {
        // Uploads go through this server's own endpoint (multipart), so a PUT
        // URL has no local equivalent. Returning a signed GET keeps the shape
        // honest instead of inventing a second local protocol.
        let _ = self.path_for(key)?;
        local_signed_url(&self.base_url, &self.sign_key, key, ttl)
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        let path = self.path_for(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(StorageError::Provider(error.to_string())),
        }
    }
}

/// S3-compatible storage: MinIO-fork in dev, Cloudflare R2 in prod (A15).
pub struct S3Storage {
    client: aws_sdk_s3::Client,
    bucket: String,
}

impl S3Storage {
    pub fn new(cfg: StorageConfig) -> anyhow::Result<Self> {
        use aws_config::BehaviorVersion;

        let credentials = aws_sdk_s3::config::Credentials::new(
            cfg.access_key_id.clone().unwrap_or_default(),
            cfg.secret_access_key
                .as_ref()
                .map(|s| s.expose_secret().to_string())
                .unwrap_or_default(),
            None,
            None,
            "static",
        );

        // Built by hand rather than through `aws_config::defaults().load()`, so
        // no environment or profile lookup can silently point dev at prod.
        let s3_config = aws_sdk_s3::config::Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new(cfg.region.clone()))
            .credentials_provider(credentials)
            .force_path_style(cfg.force_path_style)
            .endpoint_url(
                cfg.endpoint
                    .clone()
                    .unwrap_or_else(|| "https://s3.amazonaws.com".to_string()),
            )
            .build();

        Ok(Self {
            client: aws_sdk_s3::Client::from_conf(s3_config),
            bucket: cfg.bucket,
        })
    }

    /// Idempotent: a missing bucket is created at boot rather than by a compose
    /// sidecar, because A15 removed the pullable `mc` image and R2 needs the
    /// bucket created out of band anyway.
    pub async fn ensure_bucket(&self) -> anyhow::Result<()> {
        match self.client.head_bucket().bucket(&self.bucket).send().await {
            Ok(_) => Ok(()),
            Err(_) => {
                self.client
                    .create_bucket()
                    .bucket(&self.bucket)
                    .send()
                    .await
                    .map_err(|error| anyhow::anyhow!("creating bucket {}: {error}", self.bucket))?;
                Ok(())
            }
        }
    }

    fn map_err(error: impl std::fmt::Display) -> StorageError {
        StorageError::Provider(error.to_string())
    }
}

#[async_trait::async_trait]
impl Storage for S3Storage {
    async fn put(&self, key: &str, body: Bytes, mime: &str) -> Result<(), StorageError> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(mime)
            .body(aws_sdk_s3::primitives::ByteStream::from(body))
            .send()
            .await
            .map_err(Self::map_err)?;
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Bytes, StorageError> {
        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| {
                if error.as_service_error().is_some_and(|e| e.is_no_such_key()) {
                    StorageError::NotFound(key.to_string())
                } else {
                    Self::map_err(error)
                }
            })?;
        let bytes = output.body.collect().await.map_err(Self::map_err)?;
        Ok(bytes.into_bytes())
    }

    async fn presign_get(&self, key: &str, ttl: Duration) -> Result<String, StorageError> {
        let request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(
                aws_sdk_s3::presigning::PresigningConfig::expires_in(ttl).map_err(Self::map_err)?,
            )
            .await
            .map_err(Self::map_err)?;
        Ok(request.uri().to_string())
    }

    async fn presign_put(&self, key: &str, ttl: Duration) -> Result<String, StorageError> {
        let request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(
                aws_sdk_s3::presigning::PresigningConfig::expires_in(ttl).map_err(Self::map_err)?,
            )
            .await
            .map_err(Self::map_err)?;
        Ok(request.uri().to_string())
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(Self::map_err)?;
        Ok(())
    }
}

/// Kept hand-written so the signing key can never reach a log line through a
/// derived `Debug`.
impl std::fmt::Debug for LocalStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalStorage")
            .field("dir", &self.dir)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_cannot_escape_the_object_directory() {
        assert!(sanitize_key("../../etc/passwd").is_err());
        assert!(sanitize_key("/etc/passwd").is_err());
        assert!(sanitize_key("a\\b").is_err());
        assert!(sanitize_key("").is_err());
        assert!(sanitize_key("users/ab/hash.pdf").is_ok());
    }

    #[test]
    fn a_signature_round_trips_and_expires() {
        let key = b"test-key";
        let url = local_signed_url(
            "http://localhost:8080",
            key,
            "src-1",
            Duration::from_secs(60),
        )
        .expect("signing works");
        let exp: u64 = url
            .split("exp=")
            .nth(1)
            .and_then(|rest| rest.split('&').next())
            .and_then(|v| v.parse().ok())
            .expect("exp in URL");
        let sig = url.split("sig=").nth(1).expect("sig in URL").to_string();
        assert!(verify_local_signature(key, "src-1", exp, &sig).is_ok());
        // A different object id must not validate the same signature.
        assert!(verify_local_signature(key, "src-2", exp, &sig).is_err());
        assert!(
            verify_local_signature(key, "src-1", exp + 1, &sig).is_err(),
            "signature is bound to the expiry"
        );
        assert!(verify_local_signature(key, "src-1", 1, &sig).is_err());
    }
}
