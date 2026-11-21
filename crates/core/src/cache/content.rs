use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest as ShaDigest, Sha256};

use crate::cache::error::{CacheError, Result};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Digest([u8; 32]);

impl Digest {
    pub fn of(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self(hasher.finalize().into())
    }

    pub fn from_hex(text: &str) -> Result<Self> {
        if text.len() != 64 {
            return Err(CacheError::InvalidDigest(text.to_owned()));
        }
        let mut bytes = [0u8; 32];
        for (index, chunk) in text.as_bytes().chunks(2).enumerate() {
            let pair = std::str::from_utf8(chunk)
                .map_err(|_| CacheError::InvalidDigest(text.to_owned()))?;
            bytes[index] = u8::from_str_radix(pair, 16)
                .map_err(|_| CacheError::InvalidDigest(text.to_owned()))?;
        }
        Ok(Self(bytes))
    }

    pub fn hex(&self) -> String {
        let mut text = String::with_capacity(64);
        for byte in self.0 {
            text.push_str(&format!("{byte:02x}"));
        }
        text
    }
}

impl std::fmt::Display for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.hex())
    }
}

#[derive(Debug, Clone)]
pub struct ContentStore {
    root: PathBuf,
}

impl ContentStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn blob_path(&self, digest: &Digest) -> PathBuf {
        self.root.join("blobs").join(digest.hex())
    }

    pub fn put(&self, bytes: &[u8]) -> Result<Digest> {
        let digest = Digest::of(bytes);
        let destination = self.blob_path(&digest);
        fs::create_dir_all(destination.parent().unwrap_or(&self.root))?;
        if destination.exists() {
            let existing = fs::read(&destination)?;
            if existing != bytes {
                return Err(CacheError::CorruptContent(digest.hex()));
            }
            return Ok(digest);
        }
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temporary = self
            .root
            .join("blobs")
            .join(format!(".tmp-{}-{sequence}", std::process::id()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        use std::io::Write;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o444))?;
        }
        if let Err(error) = fs::rename(&temporary, &destination) {
            let _ = fs::remove_file(&temporary);
            if !destination.exists() {
                return Err(CacheError::Io(error));
            }
            let existing = fs::read(&destination)?;
            if existing != bytes {
                return Err(CacheError::CorruptContent(digest.hex()));
            }
        }
        Ok(digest)
    }

    pub fn get(&self, digest: &Digest) -> Result<Vec<u8>> {
        let path = self.blob_path(digest);
        let bytes = fs::read(&path).map_err(CacheError::Io)?;
        if Digest::of(&bytes) != *digest {
            return Err(CacheError::CorruptContent(digest.hex()));
        }
        Ok(bytes)
    }

    pub fn total_bytes(&self) -> Result<u64> {
        let blobs = self.root.join("blobs");
        let mut total = 0u64;
        if !blobs.exists() {
            return Ok(0);
        }
        for entry in fs::read_dir(&blobs)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".tmp-") {
                continue;
            }
            total = total.saturating_add(entry.metadata()?.len());
        }
        Ok(total)
    }
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_owned());
    let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_file_name(format!(".{name}.tmp-{}-{sequence}", std::process::id()));
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        use std::io::Write;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(CacheError::Io(error));
    }
    Ok(())
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::temp_db_path;

    fn scratch(label: &str) -> PathBuf {
        let mut path = temp_db_path(label);
        path.set_extension("cache");
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn cache_content_store_is_immutable_and_verified() {
        let root = scratch("content_immutable");
        let store = ContentStore::new(&root);
        let digest = store.put(b"toolchain-baseline").expect("put baseline");
        assert_eq!(store.get(&digest).expect("get baseline"), b"toolchain-baseline");
        assert_eq!(store.put(b"toolchain-baseline").expect("idempotent put"), digest);
        let manifest = digest.hex();
        assert_eq!(Digest::from_hex(&manifest).expect("parse digest"), digest);
        assert!(Digest::from_hex("abc").is_err());
        let path = store.blob_path(&digest);
        let permissions = fs::metadata(&path).expect("blob metadata").permissions();
        assert_eq!(permissions.readonly(), true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("make writable");
        }
        fs::write(&path, b"tampered").expect("attempt tamper");
        assert!(matches!(
            store.get(&digest),
            Err(CacheError::CorruptContent(_))
        ));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cache_content_store_counts_only_committed_blobs() {
        let root = scratch("content_bytes");
        let store = ContentStore::new(&root);
        let first = store.put(b"aaaa").expect("first");
        store.put(b"bbbbbb").expect("second");
        store.put(b"aaaa").expect("duplicate");
        assert_eq!(store.total_bytes().expect("total"), 10);
        assert_eq!(store.get(&first).expect("read"), b"aaaa");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cache_atomic_write_replaces_the_previous_value() {
        let root = scratch("atomic_write");
        let path = root.join("pointer").join("current");
        write_atomic(&path, b"old").expect("write old");
        write_atomic(&path, b"new").expect("write new");
        assert_eq!(fs::read(&path).expect("read pointer"), b"new");
        assert_eq!(fs::read_dir(path.parent().unwrap()).expect("dir").count(), 1);
        let _ = fs::remove_dir_all(&root);
    }
}
