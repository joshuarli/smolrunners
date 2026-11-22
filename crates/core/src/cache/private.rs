use std::fs;
use std::path::{Path, PathBuf};

use crate::cache::content::{ContentStore, Digest};
use crate::cache::error::{CacheError, Result};
use crate::cache::generation::Manifest;

#[derive(Debug, Clone)]
pub struct PrivateView {
    root: PathBuf,
    blobs: ContentStore,
}

impl PrivateView {
    pub fn create(
        views_root: &Path,
        job: &str,
        manifest: &Manifest,
        blobs: &ContentStore,
    ) -> Result<Self> {
        let root = views_root.join(job);
        if root.exists() {
            fs::remove_dir_all(&root)?;
        }
        fs::create_dir_all(&root)?;
        let view = Self {
            root,
            blobs: blobs.clone(),
        };
        for entry in &manifest.files {
            let target = view.root.join(&entry.path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let digest = Digest::from_hex(&entry.digest)?;
            let blob = view.blobs.blob_path(&digest);
            fs::copy(&blob, &target)?;
            make_writable(&target)?;
        }
        Ok(view)
    }

    pub fn open(root: PathBuf, blobs: ContentStore) -> Self {
        Self { root, blobs }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn read(&self, relative: &str) -> Result<Vec<u8>> {
        let path = self.resolve(relative)?;
        fs::read(path).map_err(CacheError::Io)
    }

    pub fn write(&self, relative: &str, bytes: &[u8]) -> Result<()> {
        let target = self.resolve(relative)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if target.exists() || target.symlink_metadata().is_ok() {
            fs::remove_file(&target)?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        use std::io::Write;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }

    pub fn is_shared_link(&self, relative: &str) -> Result<bool> {
        let path = self.resolve(relative)?;
        Ok(fs::symlink_metadata(path)?.file_type().is_symlink())
    }

    fn resolve(&self, relative: &str) -> Result<PathBuf> {
        let relative = Path::new(relative);
        if relative.is_absolute() || relative.components().any(|part| part.as_os_str() == "..") {
            return Err(CacheError::InvalidState(format!(
                "unsafe private view path {relative:?}"
            )));
        }
        Ok(self.root.join(relative))
    }
}

#[cfg(unix)]
fn make_writable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).map_err(CacheError::Io)
}

#[cfg(not(unix))]
fn make_writable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::content::ContentStore;
    use crate::cache::generation::GenerationStore;
    use crate::cache::scope::{CacheAuthority, TrustTier};
    use crate::store::test_support::temp_db_path;

    fn scratch(label: &str) -> PathBuf {
        let mut path = temp_db_path(label);
        path.set_extension("cache");
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn cache_private_views_of_one_baseline_never_share_mutable_state() {
        let root = scratch("private_isolation");
        let generations = GenerationStore::new(&root);
        let source = root.join("source");
        fs::create_dir_all(&source).expect("create source");
        fs::write(source.join("tool"), b"baseline").expect("write tool");
        let owner = CacheAuthority::new("acme/widgets", TrustTier::Maintainer);
        let generation = generations
            .create(&source, &owner, None, "toolchain", "t1")
            .expect("create baseline");
        let manifest = generations.manifest(&generation).expect("manifest");
        let views = root.join("views");
        let a = PrivateView::create(&views, "job-a", &manifest, generations.blobs())
            .expect("view a");
        let b = PrivateView::create(&views, "job-b", &manifest, generations.blobs())
            .expect("view b");

        assert!(!a.is_shared_link("tool").expect("a is an independent copy"));
        assert!(!b.is_shared_link("tool").expect("b is an independent copy"));
        a.write("tool", b"from-a").expect("a writes");
        a.write("private", b"only-a").expect("a adds private file");
        assert!(!a.is_shared_link("tool").expect("a copy-on-write"));
        assert_eq!(a.read("tool").expect("a read"), b"from-a");
        assert_eq!(b.read("tool").expect("b read"), b"baseline");
        assert!(!b.is_shared_link("tool").expect("b is still independent"));
        assert!(b.read("private").is_err());
        assert_eq!(
            generations.blobs().get(&Digest::from_hex(&manifest.files[0].digest).unwrap()).expect("blob"),
            b"baseline"
        );

        let escaping = PrivateView::create(&views, "job-c", &manifest, generations.blobs()).expect("view c");
        assert!(escaping.read("../job-a/tool").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_ordinary_filesystem_write_cannot_modify_the_shared_blob() {
        let root = scratch("private_ordinary_write");
        let blobs = ContentStore::new(root.join("content"));
        let digest = blobs.put(b"baseline").expect("blob");
        let manifest = Manifest {
            schema_version: 1,
            files: vec![crate::cache::generation::ManifestEntry {
                path: "tool".to_owned(),
                digest: digest.hex(),
                bytes: 8,
            }],
        };
        let views = root.join("views");
        let first = PrivateView::create(&views, "job-a", &manifest, &blobs).expect("view a");
        let second = PrivateView::create(&views, "job-b", &manifest, &blobs).expect("view b");

        fs::write(first.root().join("tool"), b"dirty-by-a").expect("ordinary write");
        assert_eq!(first.read("tool").expect("a read"), b"dirty-by-a");
        assert_eq!(second.read("tool").expect("b read"), b"baseline");
        assert_eq!(blobs.get(&digest).expect("blob"), b"baseline");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cache_private_view_replaces_a_previous_view_for_the_same_job() {
        let root = scratch("private_replace");
        let blobs = ContentStore::new(root.join("content"));
        let digest = blobs.put(b"v1").expect("blob v1");
        let manifest = Manifest {
            schema_version: 1,
            files: vec![crate::cache::generation::ManifestEntry {
                path: "tool".to_owned(),
                digest: digest.hex(),
                bytes: 2,
            }],
        };
        let views = root.join("views");
        let first = PrivateView::create(&views, "job", &manifest, &blobs).expect("first view");
        first.write("tool", b"dirty").expect("dirty first view");
        let second = PrivateView::create(&views, "job", &manifest, &blobs).expect("fresh view");
        assert_eq!(second.read("tool").expect("fresh content"), b"v1");
        let _ = fs::remove_dir_all(&root);
    }
}
