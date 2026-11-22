use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cache::content::{ContentStore, Digest, read_json, write_atomic, write_json};
use crate::cache::error::{CacheError, Result};
use crate::cache::scope::CacheAuthority;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    pub digest: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub files: Vec<ManifestEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationState {
    Private,
    Published,
    Retired,
}

impl GenerationState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Published => "published",
            Self::Retired => "retired",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Generation {
    pub id: String,
    pub name: String,
    pub owner: CacheAuthority,
    pub parent: Option<String>,
    pub manifest: String,
    pub state: GenerationState,
    pub active_writers: u64,
    pub pins: u64,
    pub created_at: String,
    pub published_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quota {
    pub max_generations: usize,
    pub max_bytes: u64,
}

impl Default for Quota {
    fn default() -> Self {
        Self {
            max_generations: 64,
            max_bytes: 8 * 1024 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GenerationStore {
    root: PathBuf,
    blobs: ContentStore,
}

impl GenerationStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        let blobs = ContentStore::new(root.join("content"));
        Self { root, blobs }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn blobs(&self) -> &ContentStore {
        &self.blobs
    }

    fn generations_dir(&self) -> PathBuf {
        self.root.join("generations")
    }

    fn metadata_path(&self, id: &str) -> PathBuf {
        self.generations_dir().join(format!("{id}.json"))
    }

    fn pointer_dir(&self, repository: &str) -> PathBuf {
        let key = Digest::of(repository.as_bytes()).hex();
        self.root.join("pointers").join(&key[..16])
    }

    fn pointer_path(&self, repository: &str) -> PathBuf {
        self.pointer_dir(repository).join("current")
    }

    pub fn quota(&self) -> Result<Quota> {
        let path = self.root.join("quota.json");
        if !path.exists() {
            return Ok(Quota::default());
        }
        read_json(&path)
    }

    pub fn set_quota(&self, quota: &Quota) -> Result<()> {
        write_json(&self.root.join("quota.json"), quota)
    }

    pub fn get(&self, id: &str) -> Result<Generation> {
        let path = self.metadata_path(id);
        if !path.exists() {
            return Err(CacheError::MissingGeneration(id.to_owned()));
        }
        read_json(&path)
    }

    pub fn list(&self) -> Result<Vec<Generation>> {
        let mut generations = Vec::new();
        let dir = self.generations_dir();
        if !dir.exists() {
            return Ok(generations);
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".json") {
                continue;
            }
            let id = name.trim_end_matches(".json").to_owned();
            generations.push(self.get(&id)?);
        }
        generations.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(generations)
    }

    pub fn manifest(&self, generation: &Generation) -> Result<Manifest> {
        let digest = Digest::from_hex(&generation.manifest)?;
        let bytes = self.blobs.get(&digest)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn create(
        &self,
        source: &Path,
        owner: &CacheAuthority,
        parent: Option<String>,
        name: &str,
        now: &str,
    ) -> Result<Generation> {
        let quota = self.quota()?;
        let existing = self.list()?;
        if existing.len() >= quota.max_generations {
            return Err(CacheError::QuotaExceeded {
                limit: format!(
                    "generation count {} reaches limit {}",
                    existing.len(),
                    quota.max_generations
                ),
            });
        }
        let mut files = Vec::new();
        let mut source_bytes = 0u64;
        collect_files(source, source, &mut files, &mut source_bytes)?;
        files.sort_by(|a, b| a.0.cmp(&b.0));
        if self.blobs.total_bytes()?.saturating_add(source_bytes) > quota.max_bytes {
            return Err(CacheError::QuotaExceeded {
                limit: format!(
                    "content bytes {} plus source {} exceed limit {}",
                    self.blobs.total_bytes()?,
                    source_bytes,
                    quota.max_bytes
                ),
            });
        }
        let mut entries = Vec::with_capacity(files.len());
        for (relative, bytes) in &files {
            let digest = self.blobs.put(bytes)?;
            entries.push(ManifestEntry {
                path: relative.clone(),
                digest: digest.hex(),
                bytes: bytes.len() as u64,
            });
        }
        let manifest_bytes = serde_json::to_vec_pretty(&Manifest {
            schema_version: 1,
            files: entries,
        })?;
        let manifest_digest = self.blobs.put(&manifest_bytes)?;
        let id = format!("gen-{}", manifest_digest.hex());
        if let Ok(found) = self.get(&id) {
            return Ok(found);
        }
        let generation = Generation {
            id: id.clone(),
            name: name.to_owned(),
            owner: owner.clone(),
            parent,
            manifest: manifest_digest.hex(),
            state: GenerationState::Private,
            active_writers: 0,
            pins: 0,
            created_at: now.to_owned(),
            published_at: None,
        };
        write_json(&self.metadata_path(&id), &generation)?;
        Ok(generation)
    }

    fn update<F>(&self, id: &str, mutate: F) -> Result<Generation>
    where
        F: FnOnce(&mut Generation) -> Result<()>,
    {
        let mut generation = self.get(id)?;
        mutate(&mut generation)?;
        write_json(&self.metadata_path(id), &generation)?;
        Ok(generation)
    }

    pub fn retire(&self, id: &str) -> Result<Generation> {
        self.update(id, |generation| {
            generation.state = GenerationState::Retired;
            Ok(())
        })
    }

    pub fn acquire_writer(&self, id: &str) -> Result<u64> {
        let generation = self.update(id, |generation| {
            generation.active_writers = generation.active_writers.checked_add(1).ok_or_else(|| {
                CacheError::InvalidState("active writer count overflow".to_owned())
            })?;
            Ok(())
        })?;
        Ok(generation.active_writers)
    }

    pub fn release_writer(&self, id: &str) -> Result<u64> {
        let generation = self.update(id, |generation| {
            generation.active_writers = generation.active_writers.saturating_sub(1);
            Ok(())
        })?;
        Ok(generation.active_writers)
    }

    pub fn pin(&self, id: &str) -> Result<u64> {
        let generation = self.update(id, |generation| {
            generation.pins = generation.pins.checked_add(1).ok_or_else(|| {
                CacheError::InvalidState("generation pin count overflow".to_owned())
            })?;
            Ok(())
        })?;
        Ok(generation.pins)
    }

    pub fn unpin(&self, id: &str) -> Result<u64> {
        let generation = self.update(id, |generation| {
            generation.pins = generation.pins.saturating_sub(1);
            Ok(())
        })?;
        Ok(generation.pins)
    }

    pub fn current(&self, repository: &str) -> Option<String> {
        let path = self.pointer_path(repository);
        let bytes = fs::read(path).ok()?;
        let text = String::from_utf8(bytes).ok()?;
        let id = text.trim().to_owned();
        if id.is_empty() {
            None
        } else {
            Some(id)
        }
    }

    pub fn publish(&self, id: &str, now: &str) -> Result<Option<String>> {
        let generation = self.get(id)?;
        if generation.active_writers != 0 {
            return Err(CacheError::NotQuiescent {
                generation: id.to_owned(),
                active_writers: generation.active_writers,
            });
        }
        let prior = self.current(&generation.owner.repository);
        write_atomic(&self.pointer_path(&generation.owner.repository), id.as_bytes())?;
        self.update(id, |generation| {
            generation.state = GenerationState::Published;
            generation.published_at = Some(now.to_owned());
            Ok(())
        })?;
        Ok(prior)
    }

    pub fn simulate_interrupted_publish(&self, id: &str) -> Result<Option<String>> {
        let generation = self.get(id)?;
        let pointer_dir = self.pointer_dir(&generation.owner.repository);
        fs::create_dir_all(&pointer_dir)?;
        let staging = self.root.join("staging").join(id);
        fs::create_dir_all(&staging)?;
        let prior = self.current(&generation.owner.repository);
        write_atomic(&pointer_dir.join("publish.lock"), b"interrupted")?;
        write_atomic(
            &pointer_dir.join(format!(".current.tmp-{id}")),
            id.as_bytes(),
        )?;
        Ok(prior)
    }

    pub fn recover_publish(&self, repository: &str) -> Result<Option<String>> {
        let pointer_dir = self.pointer_dir(repository);
        if pointer_dir.exists() {
            for entry in fs::read_dir(&pointer_dir)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name == "publish.lock" || name.starts_with(".current.tmp-") {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        Ok(self.current(repository))
    }

    pub fn gc(&self) -> Result<Vec<String>> {
        let mut current_ids = HashSet::new();
        let pointers = self.root.join("pointers");
        if pointers.exists() {
            for repository in fs::read_dir(&pointers)? {
                let repository = repository?;
                if !repository.file_type()?.is_dir() {
                    continue;
                }
                if let Ok(bytes) = fs::read(repository.path().join("current")) {
                    if let Ok(id) = String::from_utf8(bytes) {
                        let id = id.trim().to_owned();
                        if !id.is_empty() {
                            current_ids.insert(id);
                        }
                    }
                }
            }
        }
        let mut removed = Vec::new();
        for generation in self.list()? {
            if generation.state == GenerationState::Retired
                && generation.pins == 0
                && generation.active_writers == 0
                && !current_ids.contains(&generation.id)
            {
                let _ = fs::remove_file(self.metadata_path(&generation.id));
                let _ = fs::remove_dir_all(self.root.join("staging").join(&generation.id));
                removed.push(generation.id);
            }
        }
        Ok(removed)
    }
}

fn collect_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<(String, Vec<u8>)>,
    total: &mut u64,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files(root, &entry.path(), output, total)?;
        } else if file_type.is_file() {
            let bytes = fs::read(entry.path())?;
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|error| CacheError::Io(std::io::Error::other(error)))?
                .to_string_lossy()
                .replace('\\', "/");
            *total = total.saturating_add(bytes.len() as u64);
            output.push((relative, bytes));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::scope::TrustTier;
    use crate::store::test_support::temp_db_path;

    fn scratch(label: &str) -> PathBuf {
        let mut path = temp_db_path(label);
        path.set_extension("cache");
        let _ = fs::remove_dir_all(&path);
        path
    }

    fn source(root: &Path, label: &str, files: &[(&str, &str)]) -> PathBuf {
        let source = root.join(label);
        for (path, data) in files {
            let target = source.join(path);
            fs::create_dir_all(target.parent().unwrap_or(&source)).expect("create source dir");
            fs::write(target, data).expect("write source file");
        }
        source
    }

    fn owner() -> CacheAuthority {
        CacheAuthority::new("acme/widgets", TrustTier::Maintainer)
    }

    #[test]
    fn cache_generation_has_lineage_and_immutable_content() {
        let root = scratch("generation_lineage");
        let store = GenerationStore::new(&root);
        let first_source = source(&root, "first", &[("toolchain/gcc", "v1")]);
        let first = store
            .create(&first_source, &owner(), None, "toolchain", "t1")
            .expect("create first generation");
        let second_source = source(&root, "second", &[("toolchain/gcc", "v1"), ("toolchain/ld", "v2")]);
        let second = store
            .create(
                &second_source,
                &owner(),
                Some(first.id.clone()),
                "toolchain",
                "t2",
            )
            .expect("create second generation");
        assert_eq!(second.parent.as_deref(), Some(first.id.as_str()));
        assert_ne!(first.manifest, second.manifest);
        assert_eq!(store.get(&first.id).expect("read first"), first);
        assert_eq!(store.manifest(&second).expect("manifest").files.len(), 2);
        let again = store
            .create(&first_source, &owner(), None, "toolchain", "t3")
            .expect("duplicate create");
        assert_eq!(again.id, first.id);
        assert_eq!(store.list().expect("list").len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cache_publish_requires_quiescence_and_preserves_prior_generation() {
        let root = scratch("generation_publish");
        let store = GenerationStore::new(&root);
        let src1 = source(&root, "src1", &[("bin/tool", "v1")]);
        let baseline = store
            .create(&src1, &owner(), None, "toolchain", "t1")
            .expect("baseline");
        store.publish(&baseline.id, "t2").expect("publish baseline");
        assert_eq!(store.current("acme/widgets").as_deref(), Some(baseline.id.as_str()));

        let src2 = source(&root, "src2", &[("bin/tool", "v2")]);
        let next = store
            .create(&src2, &owner(), Some(baseline.id.clone()), "toolchain", "t3")
            .expect("next generation");
        store.acquire_writer(&next.id).expect("hold writer");
        assert!(matches!(
            store.publish(&next.id, "t4"),
            Err(CacheError::NotQuiescent { active_writers: 1, .. })
        ));
        store.release_writer(&next.id).expect("release writer");
        store.publish(&next.id, "t5").expect("publish after quiescence");
        assert_eq!(store.current("acme/widgets").as_deref(), Some(next.id.as_str()));

        store.simulate_interrupted_publish(&baseline.id).expect("interrupt");
        assert_eq!(store.current("acme/widgets").as_deref(), Some(next.id.as_str()));
        assert_eq!(
            store.recover_publish("acme/widgets").expect("recover").as_deref(),
            Some(next.id.as_str())
        );
        let valid = store.get(&next.id).expect("prior generation still valid");
        assert_eq!(valid.state, GenerationState::Published);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cache_gc_preserves_pinned_and_active_generations() {
        let root = scratch("generation_gc");
        let store = GenerationStore::new(&root);
        let one = source(&root, "s1", &[("a", "1")]);
        let two = source(&root, "s2", &[("b", "2")]);
        let three = source(&root, "s3", &[("c", "3")]);
        let current = store.create(&one, &owner(), None, "current", "t1").expect("current");
        store.publish(&current.id, "t2").expect("publish current");
        let pinned = store.create(&two, &owner(), None, "pinned", "t3").expect("pinned");
        store.pin(&pinned.id).expect("pin");
        let active = store.create(&three, &owner(), None, "active", "t4").expect("active");
        store.acquire_writer(&active.id).expect("active writer");
        let doomed = store.create(&source(&root, "s4", &[("d", "4")]), &owner(), None, "doomed", "t5")
            .expect("doomed");
        store.retire(&doomed.id).expect("retire doomed");
        let removed = store.gc().expect("gc");
        assert_eq!(removed, vec![doomed.id.clone()]);
        assert!(store.get(&current.id).is_ok());
        assert!(store.get(&pinned.id).is_ok());
        assert!(store.get(&active.id).is_ok());
        assert!(store.get(&doomed.id).is_err());
        store.unpin(&pinned.id).expect("unpin");
        store.retire(&pinned.id).expect("retire pinned");
        assert_eq!(store.gc().expect("gc again"), vec![pinned.id]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cache_quota_refuses_an_over_budget_generation() {
        let root = scratch("generation_quota");
        let store = GenerationStore::new(&root);
        store.set_quota(&Quota { max_generations: 1, max_bytes: 1_000_000 })
            .expect("set quota");
        let first = store.create(&source(&root, "q1", &[("a", "x")]), &owner(), None, "one", "t1")
            .expect("first");
        let second = store.create(&source(&root, "q2", &[("b", "y")]), &owner(), None, "two", "t2");
        assert!(matches!(
            second,
            Err(CacheError::QuotaExceeded { limit }) if limit.contains("generation count")
        ));
        store.set_quota(&Quota { max_generations: 8, max_bytes: 2 })
            .expect("tight byte quota");
        let third = store.create(&source(&root, "q3", &[("c", "many bytes here")]), &owner(), None, "three", "t3");
        assert!(matches!(
            third,
            Err(CacheError::QuotaExceeded { limit }) if limit.contains("exceed limit")
        ));
        assert_eq!(store.get(&first.id).expect("first intact").name, "one");
        let _ = fs::remove_dir_all(&root);
    }
}
