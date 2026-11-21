#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use smolrunners_core::cache::{
    CacheAuthority, CacheOutcome, GenerationStore, PrivateView, Quota, TrustTier,
    read_outcomes, record_outcome,
};
use smolrunners_core::store::{Store, StoreError};

pub const CACHE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct JobBinding {
    generation: String,
    view: String,
    repository: String,
    tier: String,
    accepted_at: String,
}

#[derive(Debug, Clone)]
pub struct CacheHost {
    state: PathBuf,
}

impl CacheHost {
    pub fn new(state: impl AsRef<Path>) -> Self {
        Self {
            state: state.as_ref().to_path_buf(),
        }
    }

    pub fn state(&self) -> &Path {
        &self.state
    }

    pub fn database(&self) -> PathBuf {
        self.state.join("store.sqlite")
    }

    pub fn cache_root(&self) -> PathBuf {
        self.state.join("content")
    }

    pub fn views_root(&self) -> PathBuf {
        self.cache_root().join("views")
    }

    pub fn input_root(&self) -> PathBuf {
        self.state.join("input")
    }

    fn jobs_root(&self) -> PathBuf {
        self.state.join("jobs")
    }

    pub fn store(&self) -> Result<Store, String> {
        for _ in 0..40 {
            match Store::open(self.database()) {
                Ok(store) => return Ok(store),
                Err(StoreError::Busy { .. }) => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(error) => return Err(format!("open cache store: {error}")),
            }
        }
        Err("open cache store: database remained busy".to_owned())
    }

    pub fn generations(&self) -> GenerationStore {
        GenerationStore::new(self.cache_root())
    }

    fn binding_path(&self, job: &str) -> PathBuf {
        self.jobs_root().join(format!("{job}.json"))
    }

    fn binding(&self, job: &str) -> Result<JobBinding, String> {
        let path = self.binding_path(job);
        let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        serde_json::from_slice(&bytes).map_err(|error| format!("parse {}: {error}", path.display()))
    }

    fn authority(repository: &str, tier: &str) -> Result<CacheAuthority, String> {
        let tier = TrustTier::parse(tier).map_err(|error| error.to_string())?;
        Ok(CacheAuthority::new(repository, tier))
    }

    fn persist_outcome(&self, outcome: &CacheOutcome) -> Result<(), String> {
        let store = self.store()?;
        record_outcome(&store, outcome)
            .map(|_| ())
            .map_err(|error| format!("record outcome: {error}"))
    }

    pub fn init(&self) -> Result<Value, String> {
        fs::create_dir_all(&self.state).map_err(|error| format!("create state: {error}"))?;
        fs::create_dir_all(self.input_root()).map_err(|error| format!("create input: {error}"))?;
        fs::create_dir_all(self.jobs_root()).map_err(|error| format!("create jobs: {error}"))?;
        fs::create_dir_all(self.views_root()).map_err(|error| format!("create views: {error}"))?;
        let store = self.store()?;
        let version: String = store
            .connection()
            .query_row("SELECT sqlite_version()", [], |row| row.get(0))
            .map_err(|error| format!("read sqlite version: {error}"))?;
        Ok(json!({
            "schema_version": CACHE_SCHEMA_VERSION,
            "initialized": true,
            "state": self.state.to_string_lossy(),
            "sqlite_version": version,
        }))
    }

    pub fn seed(&self, name: &str, path: &str, data: &str) -> Result<Value, String> {
        let directory = self.input_root().join(name);
        fs::create_dir_all(&directory).map_err(|error| format!("create input dir: {error}"))?;
        let target = directory.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("create input parent: {error}"))?;
        }
        fs::write(&target, data).map_err(|error| format!("seed {}: {error}", target.display()))?;
        Ok(json!({"seeded": path, "input": name}))
    }

    pub fn set_quota(&self, generations: usize, bytes: u64) -> Result<Value, String> {
        let quota = Quota {
            max_generations: generations,
            max_bytes: bytes,
        };
        self.generations()
            .set_quota(&quota)
            .map_err(|error| format!("set quota: {error}"))?;
        Ok(serde_json::to_value(quota).map_err(|error| format!("encode quota: {error}"))?)
    }

    pub fn baseline(
        &self,
        name: &str,
        repository: &str,
        tier: &str,
        now: &str,
    ) -> Result<Value, String> {
        let store = self.generations();
        let owner = Self::authority(repository, tier)?;
        let source = self.input_root().join(name);
        if !source.is_dir() {
            return Err(format!("no seeded input for {name}"));
        }
        let parent = store.current(repository);
        let generation = store
            .create(&source, &owner, parent.clone(), name, now)
            .map_err(|error| format!("create baseline: {error}"))?;
        Ok(json!({
            "generation": generation.id,
            "name": generation.name,
            "parent": parent,
            "manifest": generation.manifest,
            "repository": generation.owner.repository,
            "tier": generation.owner.tier.as_str(),
        }))
    }

    pub fn terminate(&self, generation: &str) -> Result<Value, String> {
        let generation = self
            .generations()
            .retire(generation)
            .map_err(|error| format!("retire generation: {error}"))?;
        Ok(json!({"generation": generation.id, "state": generation.state.as_str()}))
    }

    pub fn acquire(
        &self,
        job: &str,
        generation: &str,
        repository: &str,
        tier: &str,
        now: &str,
    ) -> Result<Value, String> {
        let principal = Self::authority(repository, tier)?;
        let generations = self.generations();
        let target = match generations.get(generation) {
            Ok(target) => target,
            Err(error) => {
                let outcome = CacheOutcome::fallback(job, repository, principal.tier, error.to_string(), now);
                self.persist_outcome(&outcome)?;
                return Ok(json!({
                    "granted": false,
                    "fallback_reason": error.to_string(),
                    "fallback_visible": true,
                }));
            }
        };
        if let Err(error) = principal.require_read(&target.owner) {
            let outcome = CacheOutcome::fallback(job, repository, principal.tier, error.to_string(), now);
            self.persist_outcome(&outcome)?;
            return Ok(json!({
                "granted": false,
                "fallback_reason": error.to_string(),
                "fallback_visible": true,
            }));
        }
        let manifest = generations
            .manifest(&target)
            .map_err(|error| format!("read manifest: {error}"))?;
        let view = PrivateView::create(&self.views_root(), job, &manifest, generations.blobs())
            .map_err(|error| format!("create private view: {error}"))?;
        let active = generations
            .acquire_writer(&target.id)
            .map_err(|error| format!("acquire writer: {error}"))?;
        let binding = JobBinding {
            generation: target.id.clone(),
            view: view.root().to_string_lossy().into_owned(),
            repository: repository.to_owned(),
            tier: tier.to_owned(),
            accepted_at: now.to_owned(),
        };
        fs::create_dir_all(self.jobs_root()).map_err(|error| format!("create jobs: {error}"))?;
        fs::write(
            self.binding_path(job),
            serde_json::to_vec_pretty(&binding).map_err(|error| format!("encode binding: {error}"))?,
        ).map_err(|error| format!("write binding: {error}"))?;
        let outcome = CacheOutcome {
            schema_version: CACHE_SCHEMA_VERSION,
            job: job.to_owned(),
            repository: repository.to_owned(),
            trust_tier: tier.to_owned(),
            generation: Some(target.id.clone()),
            private_view: Some(binding.view.clone()),
            hit: true,
            fallback: None,
            published_generation: generations.current(repository),
            recorded_at: now.to_owned(),
        };
        self.persist_outcome(&outcome)?;
        Ok(json!({
            "granted": true,
            "generation": target.id,
            "view": binding.view,
            "active_writers": active,
        }))
    }

    pub fn write(&self, job: &str, path: &str, data: &str) -> Result<Value, String> {
        let binding = self.binding(job)?;
        let view = PrivateView::open(PathBuf::from(&binding.view), self.generations().blobs().clone());
        view.write(path, data.as_bytes())
            .map_err(|error| format!("private write: {error}"))?;
        Ok(json!({"job": job, "written": path, "bytes": data.len()}))
    }

    pub fn read(&self, job: &str, path: &str) -> Result<Value, String> {
        let binding = self.binding(job)?;
        let view = PrivateView::open(PathBuf::from(&binding.view), self.generations().blobs().clone());
        let bytes = view.read(path).map_err(|error| format!("private read: {error}"))?;
        Ok(json!({
            "job": job,
            "path": path,
            "data": String::from_utf8_lossy(&bytes).into_owned(),
        }))
    }

    pub fn shared(&self, job: &str, path: &str) -> Result<Value, String> {
        let binding = self.binding(job)?;
        let view = PrivateView::open(PathBuf::from(&binding.view), self.generations().blobs().clone());
        let shared = view
            .is_shared_link(path)
            .map_err(|error| format!("private link check: {error}"))?;
        Ok(json!({"job": job, "path": path, "shared": shared}))
    }

    pub fn release(&self, job: &str, now: &str) -> Result<Value, String> {
        let binding = self.binding(job)?;
        let generations = self.generations();
        let active = generations
            .release_writer(&binding.generation)
            .map_err(|error| format!("release writer: {error}"))?;
        let outcome = CacheOutcome {
            schema_version: CACHE_SCHEMA_VERSION,
            job: job.to_owned(),
            repository: binding.repository.clone(),
            trust_tier: binding.tier.clone(),
            generation: Some(binding.generation.clone()),
            private_view: Some(binding.view.clone()),
            hit: true,
            fallback: None,
            published_generation: generations.current(&binding.repository),
            recorded_at: now.to_owned(),
        };
        self.persist_outcome(&outcome)?;
        Ok(json!({
            "released": job,
            "generation": binding.generation,
            "active_writers": active,
        }))
    }

    pub fn publish(&self, generation: &str, now: &str) -> Result<Value, String> {
        let prior = self
            .generations()
            .publish(generation, now)
            .map_err(|error| format!("publish: {error}"))?;
        Ok(json!({"published": generation, "prior": prior}))
    }

    pub fn interrupt(&self, generation: &str) -> Result<Value, String> {
        let prior = self
            .generations()
            .simulate_interrupted_publish(generation)
            .map_err(|error| format!("interrupt publish: {error}"))?;
        Ok(json!({"interrupted": generation, "prior": prior}))
    }

    pub fn recover(&self, repository: &str) -> Result<Value, String> {
        let current = self
            .generations()
            .recover_publish(repository)
            .map_err(|error| format!("recover publish: {error}"))?;
        Ok(json!({"repository": repository, "current": current}))
    }

    pub fn current(&self, repository: &str) -> Result<Value, String> {
        Ok(json!({
            "repository": repository,
            "generation": self.generations().current(repository),
        }))
    }

    pub fn pin(&self, generation: &str) -> Result<Value, String> {
        let pins = self
            .generations()
            .pin(generation)
            .map_err(|error| format!("pin: {error}"))?;
        Ok(json!({"generation": generation, "pins": pins}))
    }

    pub fn unpin(&self, generation: &str) -> Result<Value, String> {
        let pins = self
            .generations()
            .unpin(generation)
            .map_err(|error| format!("unpin: {error}"))?;
        Ok(json!({"generation": generation, "pins": pins}))
    }

    pub fn gc(&self) -> Result<Value, String> {
        let removed = self.generations().gc().map_err(|error| format!("gc: {error}"))?;
        Ok(json!({"removed": removed}))
    }

    pub fn action_cache(&self, job: &str, repository: &str, tier: &str, now: &str) -> Result<Value, String> {
        let authority = Self::authority(repository, tier)?;
        let reason = "action archives are owned by the runner's actions/cache mechanism";
        let outcome = CacheOutcome::fallback(job, repository, authority.tier, reason, now);
        self.persist_outcome(&outcome)?;
        Ok(json!({
            "job": job,
            "managed_by": "runner",
            "mechanism": "actions/cache",
            "repository": repository,
            "tier": authority.tier.as_str(),
            "fallback_reason": reason,
        }))
    }

    pub fn history(&self) -> Result<Value, String> {
        let store = self.store()?;
        let rows = read_outcomes(&store).map_err(|error| format!("read outcomes: {error}"))?;
        serde_json::to_value(rows).map_err(|error| format!("encode outcomes: {error}"))
    }

    pub fn inspect(&self, generation: &str) -> Result<Value, String> {
        let generation = self
            .generations()
            .get(generation)
            .map_err(|error| format!("inspect generation: {error}"))?;
        serde_json::to_value(generation).map_err(|error| format!("encode generation: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn scratch(label: &str) -> PathBuf {
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "smolrunners-cachehost-{label}-{}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn cache_host_binds_a_private_view_and_records_history() {
        let state = scratch("host_private");
        let host = CacheHost::new(&state);
        host.init().expect("init");
        host.seed("toolchain", "bin/gcc", "v1").expect("seed");
        let baseline: Value = serde_json::from_value(
            host.baseline("toolchain", "acme/widgets", "maintainer", "t1").expect("baseline"),
        ).unwrap();
        let generation = baseline["generation"].as_str().unwrap().to_owned();
        let acquired: Value = serde_json::from_value(
            host.acquire("job-a", &generation, "acme/widgets", "maintainer", "t2").expect("acquire"),
        ).unwrap();
        assert_eq!(acquired["granted"], true);
        host.write("job-a", "bin/gcc", "v2").expect("write");
        assert_eq!(host.read("job-a", "bin/gcc").expect("read")["data"], "v2");
        assert_eq!(host.shared("job-a", "bin/gcc").expect("shared")["shared"], false);
        host.release("job-a", "t3").expect("release");
        let history = host.history().expect("history");
        assert_eq!(history.as_array().unwrap().len(), 2);
        assert_eq!(history[0]["payload"]["job"], "job-a");
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn cache_host_denies_cross_repository_and_tier_broadening() {
        let state = scratch("host_scope");
        let host = CacheHost::new(&state);
        host.init().expect("init");
        host.seed("toolchain", "bin/gcc", "v1").expect("seed");
        let baseline: Value = serde_json::from_value(
            host.baseline("toolchain", "acme/widgets", "maintainer", "t1").expect("baseline"),
        ).unwrap();
        let generation = baseline["generation"].as_str().unwrap().to_owned();
        let foreign = host.acquire("job-b", &generation, "other/repo", "maintainer", "t2").expect("foreign");
        assert_eq!(foreign["granted"], false);
        assert!(foreign["fallback_reason"].as_str().unwrap().contains("repository"));
        let low = host.acquire("job-c", &generation, "acme/widgets", "contributor", "t3").expect("low tier");
        assert_eq!(low["granted"], false);
        assert!(low["fallback_reason"].as_str().unwrap().contains("broaden"));
        let history = host.history().expect("history");
        assert_eq!(history.as_array().unwrap().len(), 2);
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn cache_host_publishes_after_quiescence_and_survives_gc() {
        let state = scratch("host_lifetime");
        let host = CacheHost::new(&state);
        host.init().expect("init");
        host.seed("one", "tool", "v1").expect("seed one");
        let first: Value = serde_json::from_value(
            host.baseline("one", "acme/widgets", "maintainer", "t1").expect("baseline one"),
        ).unwrap();
        let first_id = first["generation"].as_str().unwrap().to_owned();
        let holder = host.acquire("holder", &first_id, "acme/widgets", "maintainer", "t2").expect("holder");
        assert_eq!(holder["granted"], true);
        assert!(host.publish(&first_id, "t3").is_err());
        host.release("holder", "t4").expect("release");
        host.publish(&first_id, "t5").expect("publish");
        assert_eq!(host.current("acme/widgets").expect("current")["generation"], first_id);

        host.seed("two", "tool", "v2").expect("seed two");
        let second: Value = serde_json::from_value(
            host.baseline("two", "acme/widgets", "maintainer", "t6").expect("lineage"),
        ).unwrap();
        assert_eq!(second["parent"], first_id);
        let second_id = second["generation"].as_str().unwrap().to_owned();
        host.pin(&second_id).expect("pin");
        host.interrupt(&second_id).expect("interrupt");
        assert_eq!(host.current("acme/widgets").expect("current")["generation"], first_id);
        host.recover("acme/widgets").expect("recover");

        host.seed("three", "tool", "v3").expect("seed three");
        let doomed: Value = serde_json::from_value(
            host.baseline("three", "acme/widgets", "maintainer", "t7").expect("doomed"),
        ).unwrap();
        let doomed_id = doomed["generation"].as_str().unwrap().to_owned();
        host.terminate(&doomed_id).expect("terminate");
        let removed = host.gc().expect("gc");
        assert_eq!(removed["removed"][0], doomed_id);
        assert!(host.inspect(&first_id).is_ok());
        assert!(host.inspect(&second_id).is_ok());
        assert!(host.inspect(&doomed_id).is_err());
        let _ = fs::remove_dir_all(&state);
    }
}
