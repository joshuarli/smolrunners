use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use smolrunners_core::cache::{
    CacheAuthority, CacheOutcome, GenerationState, GenerationStore, PrivateView, TrustTier,
    record_outcome,
};
use super::CacheHost;

pub const DOCKER_EVENT_KIND: &str = "docker-cache-outcome";
pub const DOCKER_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DrainCounts {
    pub running_containers: u64,
    pub services: u64,
    pub volumes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemovedState {
    pub containers: Vec<String>,
    pub volumes: Vec<String>,
    pub credentials: Vec<String>,
    pub identities: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlushState {
    pub sync_complete: bool,
    pub build_cache_exported: bool,
    pub images_saved: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnmountState {
    pub data_root: String,
    pub unmounted: bool,
    pub remounted_clean: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DrainEvidence {
    pub schema_version: u32,
    pub job: String,
    pub drained: DrainCounts,
    pub removed: RemovedState,
    pub flush: FlushState,
    pub unmount: UnmountState,
    pub data_root_ext4: bool,
    pub image_store: String,
}

impl DrainEvidence {
    pub fn violations(&self) -> Vec<String> {
        let mut violations = Vec::new();
        if self.schema_version != DOCKER_SCHEMA_VERSION {
            violations.push(format!("schema version {}", self.schema_version));
        }
        if self.drained.running_containers != 0 {
            violations.push(format!(
                "{} containers still running",
                self.drained.running_containers
            ));
        }
        if self.drained.services != 0 {
            violations.push(format!("{} services still running", self.drained.services));
        }
        if self.drained.volumes != 0 {
            violations.push(format!("{} volumes still present", self.drained.volumes));
        }
        if !self.flush.sync_complete {
            violations.push("daemon was not flushed".to_owned());
        }
        if !self.flush.build_cache_exported {
            violations.push("build cache was not exported".to_owned());
        }
        if !self.flush.images_saved {
            violations.push("images were not saved".to_owned());
        }
        if !self.unmount.unmounted {
            violations.push("data root was not unmounted".to_owned());
        }
        if !self.unmount.remounted_clean {
            violations.push("data root was not remounted clean".to_owned());
        }
        if !self.data_root_ext4 {
            violations.push("data root is not ext4".to_owned());
        }
        violations
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DrainRecord {
    pub containers_removed: Vec<String>,
    pub volumes_removed: Vec<String>,
    pub credentials_removed: Vec<String>,
    pub identities_removed: Vec<String>,
    pub running_containers: u64,
    pub services: u64,
    pub volumes: u64,
    pub unmounted: bool,
    pub data_root_ext4: bool,
    pub image_store: String,
}

impl From<&DrainEvidence> for DrainRecord {
    fn from(evidence: &DrainEvidence) -> Self {
        Self {
            containers_removed: evidence.removed.containers.clone(),
            volumes_removed: evidence.removed.volumes.clone(),
            credentials_removed: evidence.removed.credentials.clone(),
            identities_removed: evidence.removed.identities.clone(),
            running_containers: evidence.drained.running_containers,
            services: evidence.drained.services,
            volumes: evidence.drained.volumes,
            unmounted: evidence.unmount.unmounted,
            data_root_ext4: evidence.data_root_ext4,
            image_store: evidence.image_store.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PhasesMs {
    pub transfer: u64,
    pub unpack: u64,
    pub build: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DockerOutcome {
    pub schema_version: u32,
    pub job: String,
    pub repository: String,
    pub trust_tier: String,
    pub action: String,
    pub generation: Option<String>,
    pub published_generation: Option<String>,
    pub hit: bool,
    pub drain: Option<DrainRecord>,
    pub phases_ms: Option<PhasesMs>,
    pub output: Option<Value>,
    pub prior_state: Option<Value>,
    pub fallback: Option<String>,
    pub recorded_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DockerBinding {
    generation: String,
    repository: String,
    tier: String,
    view: String,
    target: String,
    transfer_ms: u64,
}

#[derive(Debug, Clone)]
pub struct DockerCache {
    state: PathBuf,
}

impl DockerCache {
    pub fn new(state: impl AsRef<Path>) -> Self {
        Self {
            state: state.as_ref().to_path_buf(),
        }
    }

    fn host(&self) -> CacheHost {
        CacheHost::new(&self.state)
    }

    fn generations(&self) -> GenerationStore {
        self.host().generations()
    }

    fn binding_path(&self, job: &str) -> PathBuf {
        self.host()
            .state()
            .join("jobs")
            .join(format!("docker-{job}.json"))
    }

    fn binding(&self, job: &str) -> Result<DockerBinding, String> {
        let path = self.binding_path(job);
        let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        serde_json::from_slice(&bytes).map_err(|error| format!("parse {}: {error}", path.display()))
    }

    fn authority(repository: &str, tier: &str) -> Result<CacheAuthority, String> {
        let tier = TrustTier::parse(tier).map_err(|error| error.to_string())?;
        Ok(CacheAuthority::new(repository, tier))
    }

    fn write_event(&self, execution_id: i64, outcome: &DockerOutcome, now: &str) -> Result<(), String> {
        let payload = serde_json::to_string(outcome).map_err(|error| format!("encode docker outcome: {error}"))?;
        let store = self.host().store()?;
        store
            .connection()
            .execute(
                "INSERT INTO event (execution_attempt_id, kind, payload_json, occurred_at)
                 VALUES (?1, ?2, ?3, ?4)",
                (execution_id, DOCKER_EVENT_KIND, payload.as_str(), now),
            )
            .map_err(|error| format!("record docker outcome: {error}"))?;
        Ok(())
    }

    pub fn stage(
        &self,
        name: &str,
        source: &Path,
        repository: &str,
        tier: &str,
        now: &str,
    ) -> Result<Value, String> {
        let authority = Self::authority(repository, tier)?;
        if !source.is_dir() {
            return Err(format!("no staged Docker artifacts at {}", source.display()));
        }
        let generations = self.generations();
        let parent = generations.current(repository);
        let generation = generations
            .create(source, &authority, parent.clone(), name, now)
            .map_err(|error| format!("stage Docker artifacts: {error}"))?;
        let manifest = generations
            .manifest(&generation)
            .map_err(|error| format!("read staged manifest: {error}"))?;
        let bytes: u64 = manifest.files.iter().map(|entry| entry.bytes).sum();
        Ok(json!({
            "generation": generation.id,
            "parent": parent,
            "name": generation.name,
            "manifest": generation.manifest,
            "files": manifest.files.len(),
            "bytes": bytes,
            "repository": repository,
            "tier": authority.tier.as_str(),
        }))
    }

    pub fn publish(&self, generation: &str, repository: &str, now: &str) -> Result<Value, String> {
        let generations = self.generations();
        let prior = generations
            .publish(generation, now)
            .map_err(|error| format!("publish Docker generation: {error}"))?;
        let info = generations
            .get(generation)
            .map_err(|error| format!("inspect Docker generation: {error}"))?;
        Ok(json!({
            "generation": generation,
            "prior": prior,
            "current": generations.current(repository),
            "state": info.state.as_str(),
            "published_at": info.published_at,
        }))
    }

    pub fn transfer(
        &self,
        job: &str,
        generation: &str,
        repository: &str,
        tier: &str,
        target: &Path,
    ) -> Result<Value, String> {
        let principal = Self::authority(repository, tier)?;
        let generations = self.generations();
        let info = generations
            .get(generation)
            .map_err(|error| format!("find Docker generation: {error}"))?;
        principal
            .require_read(&info.owner)
            .map_err(|error| error.to_string())?;
        let manifest = generations
            .manifest(&info)
            .map_err(|error| format!("read Docker manifest: {error}"))?;
        let view = PrivateView::create(
            &self.host().views_root(),
            job,
            &manifest,
            generations.blobs(),
        )
        .map_err(|error| format!("create Docker reuse view: {error}"))?;
        fs::create_dir_all(target).map_err(|error| format!("create reuse target: {error}"))?;
        let started = Instant::now();
        for entry in &manifest.files {
            let source = view.root().join(&entry.path);
            let destination = target.join(&entry.path);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|error| format!("create reuse path: {error}"))?;
            }
            fs::copy(&source, &destination)
                .map_err(|error| format!("transfer {}: {error}", entry.path))?;
        }
        let transfer_ms = started.elapsed().as_millis() as u64;
        generations
            .acquire_writer(&info.id)
            .map_err(|error| format!("acquire Docker writer: {error}"))?;
        let binding = DockerBinding {
            generation: info.id.clone(),
            repository: repository.to_owned(),
            tier: tier.to_owned(),
            view: view.root().to_string_lossy().into_owned(),
            target: target.to_string_lossy().into_owned(),
            transfer_ms,
        };
        fs::create_dir_all(self.host().state().join("jobs"))
            .map_err(|error| format!("create jobs: {error}"))?;
        fs::write(
            self.binding_path(job),
            serde_json::to_vec_pretty(&binding).map_err(|error| format!("encode binding: {error}"))?,
        )
        .map_err(|error| format!("write binding: {error}"))?;
        Ok(json!({
            "granted": true,
            "job": job,
            "generation": info.id,
            "view": binding.view,
            "target": binding.target,
            "files": manifest.files.len(),
            "bytes": manifest.files.iter().map(|entry| entry.bytes).sum::<u64>(),
            "transfer_ms": transfer_ms,
        }))
    }

    pub fn drain(
        &self,
        job: &str,
        evidence_path: &Path,
        repository: &str,
        tier: &str,
        now: &str,
    ) -> Result<Value, String> {
        let bytes = fs::read(evidence_path)
            .map_err(|error| format!("read drain evidence: {error}"))?;
        let evidence: DrainEvidence = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse drain evidence: {error}"))?;
        let violations = evidence.violations();
        if !violations.is_empty() {
            return Err(format!("drain evidence invalid: {}", violations.join("; ")));
        }
        if evidence.job != job {
            return Err(format!(
                "drain evidence job {} does not match {job}",
                evidence.job
            ));
        }
        let authority = Self::authority(repository, tier)?;
        let generations = self.generations();
        let record = DrainRecord::from(&evidence);
        let base = CacheOutcome {
            schema_version: DOCKER_SCHEMA_VERSION,
            job: job.to_owned(),
            repository: repository.to_owned(),
            trust_tier: authority.tier.as_str().to_owned(),
            generation: None,
            private_view: None,
            hit: true,
            fallback: None,
            published_generation: generations.current(repository),
            recorded_at: now.to_owned(),
        };
        let store = self.host().store()?;
        let execution_id = record_outcome(&store, &base)
            .map_err(|error| format!("record drain outcome: {error}"))?;
        let outcome = DockerOutcome {
            schema_version: DOCKER_SCHEMA_VERSION,
            job: job.to_owned(),
            repository: repository.to_owned(),
            trust_tier: authority.tier.as_str().to_owned(),
            action: "drain".to_owned(),
            generation: None,
            published_generation: base.published_generation.clone(),
            hit: true,
            drain: Some(record),
            phases_ms: None,
            output: None,
            prior_state: None,
            fallback: None,
            recorded_at: now.to_owned(),
        };
        self.write_event(execution_id, &outcome, now)?;
        serde_json::to_value(outcome).map_err(|error| format!("encode drain record: {error}"))
    }

    pub fn complete(
        &self,
        job: &str,
        unpack_ms: u64,
        build_ms: u64,
        output_path: &Path,
        prior_path: Option<&Path>,
        now: &str,
    ) -> Result<Value, String> {
        let binding = self.binding(job)?;
        let generations = self.generations();
        let active = generations
            .release_writer(&binding.generation)
            .map_err(|error| format!("release Docker writer: {error}"))?;
        let bytes = fs::read(output_path).map_err(|error| format!("read reuse output: {error}"))?;
        let output: Value =
            serde_json::from_slice(&bytes).map_err(|error| format!("parse reuse output: {error}"))?;
        let prior = match prior_path {
            Some(path) => {
                let bytes = fs::read(path).map_err(|error| format!("read prior state: {error}"))?;
                Some(
                    serde_json::from_slice(&bytes)
                        .map_err(|error| format!("parse prior state: {error}"))?,
                )
            }
            None => None,
        };
        let base = CacheOutcome {
            schema_version: DOCKER_SCHEMA_VERSION,
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
        let store = self.host().store()?;
        let execution_id = record_outcome(&store, &base)
            .map_err(|error| format!("record reuse outcome: {error}"))?;
        let outcome = DockerOutcome {
            schema_version: DOCKER_SCHEMA_VERSION,
            job: job.to_owned(),
            repository: binding.repository.clone(),
            trust_tier: binding.tier.clone(),
            action: "reuse".to_owned(),
            generation: Some(binding.generation.clone()),
            published_generation: base.published_generation.clone(),
            hit: true,
            drain: None,
            phases_ms: Some(PhasesMs {
                transfer: binding.transfer_ms,
                unpack: unpack_ms,
                build: build_ms,
            }),
            output: Some(output),
            prior_state: prior,
            fallback: None,
            recorded_at: now.to_owned(),
        };
        self.write_event(execution_id, &outcome, now)?;
        Ok(json!({
            "job": job,
            "generation": binding.generation,
            "active_writers": active,
            "phases_ms": outcome.phases_ms,
            "output": outcome.output,
        }))
    }

    pub fn history(&self) -> Result<Value, String> {
        let store = self.host().store()?;
        let mut statement = store
            .connection()
            .prepare(
                "SELECT e.id, j.github_job_key, wr.repository, e.payload_json, e.occurred_at
                 FROM event e
                 JOIN execution_attempt ea ON ea.id = e.execution_attempt_id
                 JOIN job j ON j.id = ea.job_id
                 JOIN workflow_run_attempt wra ON wra.id = j.workflow_run_attempt_id
                 JOIN workflow_run wr ON wr.id = wra.workflow_run_id
                 WHERE e.kind = ?1
                 ORDER BY e.id",
            )
            .map_err(|error| format!("prepare docker history: {error}"))?;
        let rows = statement
            .query_map((DOCKER_EVENT_KIND,), |row| {
                let payload: String = row.get(3)?;
                Ok(json!({
                    "event_id": row.get::<_, i64>(0)?,
                    "job": row.get::<_, String>(1)?,
                    "repository": row.get::<_, String>(2)?,
                    "payload": serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
                    "occurred_at": row.get::<_, String>(4)?,
                }))
            })
            .map_err(|error| format!("query docker history: {error}"))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|error| format!("read docker history: {error}"))?);
        }
        serde_json::to_value(results).map_err(|error| format!("encode docker history: {error}"))
    }

    pub fn verify(&self, cold_path: &Path, warm_path: &Path, generation: &str) -> Result<Value, String> {
        let bytes = fs::read(cold_path).map_err(|error| format!("read cold output: {error}"))?;
        let cold: Value =
            serde_json::from_slice(&bytes).map_err(|error| format!("parse cold output: {error}"))?;
        let bytes = fs::read(warm_path).map_err(|error| format!("read warm output: {error}"))?;
        let warm: Value =
            serde_json::from_slice(&bytes).map_err(|error| format!("parse warm output: {error}"))?;
        let keys = ["build_marker", "image_digest", "cache_mode"];
        for key in keys {
            if cold[key] != warm[key] || cold[key].is_null() {
                return Err(format!(
                    "cold and warm outputs differ on {key}: {} vs {}",
                    cold[key], warm[key]
                ));
            }
        }
        let generations = self.generations();
        let info = generations
            .get(generation)
            .map_err(|error| format!("inspect published Docker generation: {error}"))?;
        if info.state != GenerationState::Published {
            return Err(format!(
                "Docker generation {generation} is {}",
                info.state.as_str()
            ));
        }
        let current = generations.current(&info.owner.repository);
        if current.as_deref() != Some(generation) {
            return Err(format!(
                "published generation is not current: {current:?} vs {generation}"
            ));
        }
        Ok(json!({
            "identical": true,
            "compared": keys,
            "generation": generation,
            "state": info.state.as_str(),
            "current": current,
            "parent": info.parent,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn scratch(label: &str) -> PathBuf {
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "smolrunners-dockercache-{label}-{}-{sequence}",
            std::process::id()
        ))
    }

    fn evidence(job: &str) -> DrainEvidence {
        DrainEvidence {
            schema_version: DOCKER_SCHEMA_VERSION,
            job: job.to_owned(),
            drained: DrainCounts {
                running_containers: 0,
                services: 0,
                volumes: 0,
            },
            removed: RemovedState {
                containers: vec!["sr-job-old".to_owned()],
                volumes: vec!["sr-job-data".to_owned()],
                credentials: vec!["registry auth".to_owned()],
                identities: vec!["daemon-1".to_owned()],
            },
            flush: FlushState {
                sync_complete: true,
                build_cache_exported: true,
                images_saved: true,
            },
            unmount: UnmountState {
                data_root: "/var/lib/docker".to_owned(),
                unmounted: true,
                remounted_clean: true,
            },
            data_root_ext4: true,
            image_store: "vfs".to_owned(),
        }
    }

    #[test]
    fn docker_cache_publishes_then_reuses_with_separate_timings() {
        let state = scratch("reuse");
        let host = CacheHost::new(&state);
        host.init().expect("init");
        let cache = DockerCache::new(&state);
        let source = state.join("artifacts");
        fs::create_dir_all(source.join("buildcache")).expect("artifact dirs");
        fs::write(source.join("image.tar"), b"image-layer").expect("image tar");
        fs::write(source.join("buildcache/index"), b"cache-index").expect("cache index");

        let staged: Value = serde_json::from_value(
            cache
                .stage("warm-artifacts", &source, "acme/widgets", "maintainer", "t1")
                .expect("stage"),
        )
        .unwrap();
        let generation = staged["generation"].as_str().unwrap().to_owned();
        let published: Value = serde_json::from_value(
            cache.publish(&generation, "acme/widgets", "t2").expect("publish"),
        )
        .unwrap();
        assert_eq!(published["state"], "published");
        assert_eq!(published["current"], generation);

        let evidence_path = state.join("drain.json");
        fs::write(&evidence_path, serde_json::to_vec(&evidence("cold")).unwrap()).expect("evidence");
        let drained: Value = serde_json::from_value(
            cache
                .drain("cold", &evidence_path, "acme/widgets", "maintainer", "t3")
                .expect("drain"),
        )
        .unwrap();
        assert_eq!(drained["action"], "drain");
        assert_eq!(drained["drain"]["identities_removed"][0], "daemon-1");

        let target = state.join("reuse-target");
        let transfer: Value = serde_json::from_value(
            cache
                .transfer("warm", &generation, "acme/widgets", "maintainer", &target)
                .expect("transfer"),
        )
        .unwrap();
        assert_eq!(transfer["granted"], true);
        assert_eq!(transfer["files"], 2);
        assert_eq!(fs::read(target.join("image.tar")).expect("copied"), b"image-layer");

        let output_path = state.join("output.json");
        let output = json!({
            "build_marker": "corpus-build-v1",
            "image_digest": "sha256:abc",
            "cache_mode": "max"
        });
        fs::write(&output_path, serde_json::to_vec(&output).unwrap()).expect("output");
        let complete: Value = serde_json::from_value(
            cache
                .complete("warm", 120, 240, &output_path, None, "t4")
                .expect("complete"),
        )
        .unwrap();
        assert_eq!(complete["phases_ms"]["unpack"], 120);
        assert_eq!(complete["phases_ms"]["build"], 240);
        assert!(transfer["transfer_ms"].as_u64().unwrap() <= complete["phases_ms"]["transfer"].as_u64().unwrap());

        let history = cache.history().expect("history");
        assert_eq!(history.as_array().unwrap().len(), 2);
        assert_eq!(history[0]["payload"]["action"], "drain");
        assert_eq!(history[1]["payload"]["action"], "reuse");

        fs::write(&output_path, serde_json::to_vec(&output).unwrap()).expect("output again");
        cache
            .verify(&output_path, &output_path, &generation)
            .expect("verify");
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn docker_cache_rejects_an_undrained_daemon() {
        let state = scratch("undrained");
        let host = CacheHost::new(&state);
        host.init().expect("init");
        let cache = DockerCache::new(&state);
        let mut bad = evidence("cold");
        bad.drained.running_containers = 2;
        bad.flush.build_cache_exported = false;
        bad.unmount.unmounted = false;
        let path = state.join("bad.json");
        fs::write(&path, serde_json::to_vec(&bad).unwrap()).expect("write bad evidence");
        let error = cache
            .drain("cold", &path, "acme/widgets", "maintainer", "t1")
            .expect_err("undrained daemon must fail");
        assert!(error.contains("containers still running"), "{error}");
        assert!(error.contains("build cache was not exported"), "{error}");
        assert!(error.contains("data root was not unmounted"), "{error}");
        let _ = fs::remove_dir_all(&state);
    }
}
