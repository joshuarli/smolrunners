use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use smolrunners_core::store::SampleRow;
use smolrunners_core::telemetry::{History, IngestContext, Sample, SCHEMA_VERSION};

fn fingerprint(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Installation {
    pub app_id: i64,
    pub installation_id: i64,
    pub account_login: String,
    pub scope: String,
    pub runner_group: String,
    pub fake: bool,
    pub connected_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invitation {
    pub id: i64,
    pub token: String,
    pub host_key: String,
    pub key_fingerprint: String,
    pub cert_fingerprint: Option<String>,
    pub single_use: bool,
    pub consumed: bool,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostRecord {
    pub id: i64,
    pub host_key: String,
    pub hostname: String,
    pub arch: String,
    pub total_vcpu: u16,
    pub total_memory_gib: u32,
    pub used_vcpu: u16,
    pub used_memory_gib: u32,
    pub admission_state: String,
    pub incarnation: u64,
    pub capabilities: Vec<String>,
    pub key_fingerprint: String,
    pub cert_fingerprint: String,
    pub mtls: bool,
    pub drain_revision: u64,
    pub revoked: bool,
    pub last_contact_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileRecord {
    pub name: String,
    pub revision_id: String,
    pub revision_number: u32,
    pub vcpu: u16,
    pub memory_gib: u32,
    pub family: Option<String>,
    pub distro: String,
    pub release: String,
    pub arch: String,
    pub promoted: bool,
    pub applied_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageRecord {
    pub id: i64,
    pub distro: String,
    pub release: String,
    pub arch: String,
    pub digest: String,
    pub qualified: bool,
    pub promoted: bool,
    pub built_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheRecord {
    pub id: i64,
    pub kind: String,
    pub scope: String,
    pub generation: u32,
    pub lineage: Vec<String>,
    pub size_bytes: u64,
    pub pinned: bool,
    pub active: bool,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: i64,
    pub execution_attempt_id: i64,
    pub protocol_job_id: String,
    pub request_id: i64,
    pub job_key: String,
    pub name: String,
    pub profile: String,
    pub state: String,
    pub host_id: Option<i64>,
    pub labels: Vec<String>,
    pub demand_at_ms: i64,
    pub start_at_ms: Option<i64>,
    pub finish_at_ms: Option<i64>,
    pub result: Option<String>,
    pub admission_reason: String,
    pub github_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StateData {
    pub initialized: bool,
    pub next_id: i64,
    pub installation: Option<Installation>,
    pub invitations: Vec<Invitation>,
    pub hosts: Vec<HostRecord>,
    pub profiles: Vec<ProfileRecord>,
    pub images: Vec<ImageRecord>,
    pub caches: Vec<CacheRecord>,
    pub jobs: Vec<JobRecord>,
}

pub struct State {
    pub data: StateData,
    json_path: PathBuf,
    db_path: PathBuf,
    history: History,
}

impl State {
    pub fn open(state_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(state_dir).map_err(|error| format!("create state dir: {error}"))?;
        let json_path = state_dir.join("state.json");
        let db_path = state_dir.join("telemetry.sqlite");
        let data = if json_path.exists() {
            let raw = fs::read_to_string(&json_path)
                .map_err(|error| format!("read state: {error}"))?;
            serde_json::from_str(&raw).map_err(|error| format!("decode state: {error}"))?
        } else {
            StateData::default()
        };
        let history = History::open(&db_path).map_err(|error| format!("open history: {error}"))?;
        Ok(Self {
            data,
            json_path,
            db_path,
            history,
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn persist(&self) -> Result<(), String> {
        let raw = serde_json::to_string_pretty(&self.data)
            .map_err(|error| format!("encode state: {error}"))?;
        let temporary = self.json_path.with_extension("json.tmp");
        fs::write(&temporary, raw).map_err(|error| format!("write state: {error}"))?;
        fs::rename(&temporary, &self.json_path).map_err(|error| format!("commit state: {error}"))?;
        Ok(())
    }

    pub fn next_id(&mut self) -> i64 {
        let id = self.data.next_id;
        self.data.next_id += 1;
        id
    }

    pub fn init(&mut self) -> Result<(), String> {
        self.data.initialized = true;
        self.persist()
    }

    pub fn connect_github(
        &mut self,
        app_id: i64,
        installation_id: i64,
        account_login: &str,
        now_ms: i64,
    ) -> Result<Installation, String> {
        let installation = Installation {
            app_id,
            installation_id,
            account_login: account_login.to_owned(),
            scope: "all".to_owned(),
            runner_group: "default".to_owned(),
            fake: true,
            connected_at_ms: now_ms,
        };
        self.data.installation = Some(installation.clone());
        self.persist()?;
        Ok(installation)
    }

    pub fn create_invitation(
        &mut self,
        host_key: &str,
        cert_fingerprint: Option<String>,
        now_ms: i64,
    ) -> Result<Invitation, String> {
        let id = self.next_id();
        let invitation = Invitation {
            id,
            token: format!("sr-invite-{id}-{}", now_ms),
            host_key: host_key.to_owned(),
            key_fingerprint: fingerprint(host_key),
            cert_fingerprint,
            single_use: true,
            consumed: false,
            created_at_ms: now_ms,
        };
        self.data.invitations.push(invitation.clone());
        self.persist()?;
        Ok(invitation)
    }

    pub fn join_host(
        &mut self,
        token: &str,
        hostname: &str,
        arch: &str,
        total_vcpu: u16,
        total_memory_gib: u32,
        host_key: &str,
        cert_fingerprint: &str,
        now_ms: i64,
    ) -> Result<HostRecord, String> {
        let invitation = self
            .data
            .invitations
            .iter()
            .find(|invitation| invitation.token == token && !invitation.consumed)
            .ok_or_else(|| "invite_not_found_or_used".to_owned())?;
        if fingerprint(host_key) != invitation.key_fingerprint {
            return Err("fingerprint_mismatch".to_owned());
        }
        if cert_fingerprint.is_empty() {
            return Err("missing_certificate".to_owned());
        }
        if let Some(expected) = invitation.cert_fingerprint.as_deref() {
            if cert_fingerprint != expected {
                return Err("certificate_mismatch".to_owned());
            }
        }
        let key_fingerprint = invitation.key_fingerprint.clone();
        let bound_host_key = invitation.host_key.clone();
        let invitation_id = invitation.id;
        let capabilities = vec![
            format!("arch={arch}"),
            format!("vcpu={total_vcpu}"),
            format!("memory_gib={total_memory_gib}"),
            "mtls=true".to_owned(),
        ];
        if let Some(invitation) = self
            .data
            .invitations
            .iter_mut()
            .find(|invitation| invitation.id == invitation_id)
        {
            invitation.consumed = true;
        }
        if let Some(host) = self
            .data
            .hosts
            .iter_mut()
            .find(|host| host.host_key == bound_host_key)
        {
            host.hostname = hostname.to_owned();
            host.arch = arch.to_owned();
            host.total_vcpu = total_vcpu;
            host.total_memory_gib = total_memory_gib;
            host.used_vcpu = 0;
            host.used_memory_gib = 0;
            host.admission_state = "eligible".to_owned();
            host.incarnation += 1;
            host.capabilities = capabilities;
            host.key_fingerprint = key_fingerprint;
            host.cert_fingerprint = cert_fingerprint.to_owned();
            host.mtls = true;
            host.revoked = false;
            host.last_contact_ms = Some(now_ms);
            host.drain_revision += 1;
            return Ok(host.clone());
        }
        let id = self.next_id();
        let host = HostRecord {
            id,
            host_key: bound_host_key,
            hostname: hostname.to_owned(),
            arch: arch.to_owned(),
            total_vcpu,
            total_memory_gib,
            used_vcpu: 0,
            used_memory_gib: 0,
            admission_state: "eligible".to_owned(),
            incarnation: 1,
            capabilities,
            key_fingerprint,
            cert_fingerprint: cert_fingerprint.to_owned(),
            mtls: true,
            drain_revision: 0,
            revoked: false,
            last_contact_ms: Some(now_ms),
        };
        self.data.hosts.push(host.clone());
        self.persist()?;
        Ok(host)
    }

    fn transition_host(
        &mut self,
        id: i64,
        state: &str,
        revoke: bool,
        clear_contact: bool,
    ) -> Result<(String, String, u64), String> {
        let host = self
            .data
            .hosts
            .iter_mut()
            .find(|host| host.id == id)
            .ok_or_else(|| "host_not_found".to_owned())?;
        let previous = host.admission_state.clone();
        host.admission_state = state.to_owned();
        host.drain_revision += 1;
        if revoke {
            host.revoked = true;
        }
        if clear_contact {
            host.last_contact_ms = None;
        }
        let revision = host.drain_revision;
        self.persist()?;
        Ok((previous, state.to_owned(), revision))
    }

    pub fn drain_host(&mut self, id: i64) -> Result<(String, String, u64), String> {
        self.transition_host(id, "drained", false, false)
    }

    pub fn resume_host(&mut self, id: i64) -> Result<(String, String, u64), String> {
        self.transition_host(id, "eligible", false, false)
    }

    pub fn revoke_host(&mut self, id: i64) -> Result<(String, String, u64), String> {
        self.transition_host(id, "revoked", true, false)
    }

    pub fn kill_host(&mut self, id: i64) -> Result<(String, String, u64), String> {
        self.transition_host(id, "lost", false, true)
    }

    pub fn heartbeat_host(&mut self, id: i64, incarnation: Option<u64>, now_ms: i64) -> Result<HostRecord, String> {
        let host = self
            .data
            .hosts
            .iter_mut()
            .find(|host| host.id == id)
            .ok_or_else(|| "host_not_found".to_owned())?;
        if let Some(observed) = incarnation {
            if observed != host.incarnation {
                return Err("incarnation_mismatch".to_owned());
            }
        }
        host.last_contact_ms = Some(now_ms);
        let result = host.clone();
        self.persist()?;
        Ok(result)
    }

    fn profile_shape(&self, profile: &str) -> (u16, u32, String) {
        if let Some(record) = self.data.profiles.iter().find(|record| record.name == profile) {
            return (record.vcpu, record.memory_gib, record.arch.clone());
        }
        let stem = profile.rsplit('-').next().unwrap_or(profile);
        let vcpu = stem.split('c').next().and_then(|value| value.parse().ok()).unwrap_or(1);
        let memory_gib = stem.split('c').nth(1).and_then(|value| value.trim_end_matches('g').parse().ok()).unwrap_or(2);
        (vcpu, memory_gib, "arm64".to_owned())
    }

    pub fn admit_job(
        &mut self,
        id: i64,
        explicit_host_id: Option<i64>,
        incarnation: Option<u64>,
        now_ms: i64,
    ) -> Result<JobRecord, String> {
        let (profile_name, required_vcpu, required_memory, required_arch) = {
            let job = self.job(id).ok_or_else(|| "job_not_found".to_owned())?;
            let (vcpu, memory, mut arch) = self.profile_shape(&job.profile);
            for label in &job.labels {
                if label.contains("-x64-") {
                    arch = "x64".to_owned();
                } else if label.contains("-arm64-") {
                    arch = "arm64".to_owned();
                }
            }
            (job.profile.clone(), vcpu, memory, arch)
        };
        let candidates: Vec<HostRecord> = self
            .data
            .hosts
            .iter()
            .filter(|host| match explicit_host_id {
                Some(host_id) => host.id == host_id,
                None => true,
            })
            .cloned()
            .collect();
        if candidates.is_empty() {
            return Err("no such host".to_owned());
        }
        let mut eligible: Vec<HostRecord> = candidates
            .iter()
            .filter(|host| {
                host.admission_state == "eligible"
                    && host.revoked == false
                    && host.last_contact_ms.is_some()
                    && host.arch == required_arch
                    && host.total_vcpu >= required_vcpu
                    && host.total_memory_gib >= required_memory
                    && host.total_vcpu.saturating_sub(host.used_vcpu) >= required_vcpu
                    && host.total_memory_gib.saturating_sub(host.used_memory_gib) >= required_memory
            })
            .cloned()
            .collect();
        if eligible.is_empty() {
            let reason = match explicit_host_id {
                Some(host_id) => match candidates.first() {
                    Some(host) if host.revoked => format!("host {} revoked", host.host_key),
                    Some(host) if host.admission_state == "drained" => format!("host {} drained", host.host_key),
                    Some(host) if host.admission_state == "lost" => format!("host {} lost; capacity fenced", host.host_key),
                    Some(host) if host.last_contact_ms.is_none() => format!("host {} unreachable; capacity fenced", host_id),
                    Some(host) => format!(
                        "host {} capabilities {} arch={} vcpu>={} memory_gib>={} do not match profile {}",
                        host.host_key, host.capabilities.join(","), required_arch, required_vcpu, required_memory, profile_name
                    ),
                    None => format!("host {host_id} not found"),
                },
                None => format!(
                    "no eligible host matching arch={} vcpu>={} memory_gib>={}",
                    required_arch, required_vcpu, required_memory
                ),
            };
            return Err(reason);
        }
        eligible.sort_by(|left, right| {
            (left.total_vcpu, left.total_memory_gib, left.id).cmp(&(right.total_vcpu, right.total_memory_gib, right.id))
        });
        let chosen = eligible.remove(0);
        if let Some(observed) = incarnation {
            if observed != chosen.incarnation {
                return Err(format!("incarnation_mismatch: host {} at {}", chosen.host_key, chosen.incarnation));
            }
        }
        let host_key = chosen.host_key.clone();
        let host_id = chosen.id;
        if let Some(host) = self.data.hosts.iter_mut().find(|host| host.id == host_id) {
            host.used_vcpu += required_vcpu;
            host.used_memory_gib += required_memory;
            host.last_contact_ms = Some(now_ms);
        }
        let job = self.job_mut(id).ok_or_else(|| "job_not_found".to_owned())?;
        job.state = "busy".to_owned();
        job.host_id = Some(host_id);
        job.start_at_ms = Some(now_ms);
        job.admission_reason = format!("admitted to matching host {host_key}");
        let result = job.clone();
        self.persist()?;
        Ok(result)
    }

    pub fn apply_profile(&mut self, record: ProfileRecord) -> Result<(), String> {
        if let Some(existing) = self
            .data
            .profiles
            .iter_mut()
            .find(|profile| profile.name == record.name)
        {
            *existing = record;
        } else {
            self.data.profiles.push(record);
        }
        self.persist()
    }

    pub fn promote_profile(&mut self, name: &str) -> Result<(), String> {
        let mut found = false;
        for profile in &mut self.data.profiles {
            if profile.name == name {
                profile.promoted = true;
                found = true;
            } else {
                profile.promoted = false;
            }
        }
        if !found {
            return Err("profile_not_found".to_owned());
        }
        self.persist()
    }

    pub fn image(&self, id: i64) -> Option<&ImageRecord> {
        self.data.images.iter().find(|image| image.id == id)
    }

    pub fn build_image(
        &mut self,
        distro: &str,
        release: &str,
        arch: &str,
        digest: &str,
        now_ms: i64,
    ) -> Result<ImageRecord, String> {
        let id = self.next_id();
        let image = ImageRecord {
            id,
            distro: distro.to_owned(),
            release: release.to_owned(),
            arch: arch.to_owned(),
            digest: digest.to_owned(),
            qualified: false,
            promoted: false,
            built_at_ms: now_ms,
        };
        self.data.images.push(image.clone());
        self.persist()?;
        Ok(image)
    }

    pub fn qualify_image(&mut self, id: i64, qualified: bool) -> Result<(), String> {
        let image = self
            .data
            .images
            .iter_mut()
            .find(|image| image.id == id)
            .ok_or_else(|| "image_not_found".to_owned())?;
        image.qualified = qualified;
        self.persist()
    }

    pub fn promote_image(&mut self, id: i64) -> Result<(), String> {
        let image = self
            .data
            .images
            .iter_mut()
            .find(|image| image.id == id)
            .ok_or_else(|| "image_not_found".to_owned())?;
        if !image.qualified {
            return Err("image_not_qualified".to_owned());
        }
        image.promoted = true;
        self.persist()
    }

    pub fn add_cache(&mut self, cache: CacheRecord) -> Result<(), String> {
        self.data.caches.push(cache);
        self.persist()
    }

    pub fn job(&self, id: i64) -> Option<&JobRecord> {
        self.data.jobs.iter().find(|job| job.id == id)
    }

    pub fn job_mut(&mut self, id: i64) -> Option<&mut JobRecord> {
        self.data.jobs.iter_mut().find(|job| job.id == id)
    }

    pub fn record_demand(
        &mut self,
        protocol_job_id: &str,
        request_id: i64,
        job_key: &str,
        profile: &str,
        labels: Vec<String>,
        now_ms: i64,
    ) -> Result<JobRecord, String> {
        if let Some(existing) = self
            .data
            .jobs
            .iter()
            .find(|job| job.protocol_job_id == protocol_job_id && job.request_id == request_id)
        {
            return Ok(existing.clone());
        }
        let id = self.next_id();
        let execution_attempt_id = self.next_id();
        let job = JobRecord {
            id,
            execution_attempt_id,
            protocol_job_id: protocol_job_id.to_owned(),
            request_id,
            job_key: job_key.to_owned(),
            name: job_key.to_owned(),
            profile: profile.to_owned(),
            state: "reserved".to_owned(),
            host_id: None,
            labels,
            demand_at_ms: now_ms,
            start_at_ms: None,
            finish_at_ms: None,
            result: None,
            admission_reason: "profile matched advertised labels".to_owned(),
            github_url: None,
        };
        self.data.jobs.push(job.clone());
        self.persist()?;
        Ok(job)
    }

    pub fn finish_job(&mut self, id: i64, result: &str, now_ms: i64) -> Result<JobRecord, String> {
        let (host_id, profile_name) = {
            let job = self.job(id).ok_or_else(|| "job_not_found".to_owned())?;
            (job.host_id, job.profile.clone())
        };
        let (required_vcpu, required_memory, _) = self.profile_shape(&profile_name);
        let fence = host_id
            .and_then(|id| self.data.hosts.iter().find(|host| host.id == id))
            .map(|host| host.admission_state == "lost")
            .unwrap_or(false);
        if let Some(host_id) = host_id {
            if !fence {
                if let Some(host) = self.data.hosts.iter_mut().find(|host| host.id == host_id) {
                    host.used_vcpu = host.used_vcpu.saturating_sub(required_vcpu);
                    host.used_memory_gib = host.used_memory_gib.saturating_sub(required_memory);
                }
            }
        }
        let job = self.job_mut(id).ok_or_else(|| "job_not_found".to_owned())?;
        job.state = "done".to_owned();
        job.result = Some(result.to_owned());
        job.finish_at_ms = Some(now_ms);
        let finished = job.clone();
        self.persist()?;
        Ok(finished)
    }

    pub fn attach_github(&mut self, id: i64, url: &str) -> Result<(), String> {
        let job = self.job_mut(id).ok_or_else(|| "job_not_found".to_owned())?;
        job.github_url = Some(url.to_owned());
        self.persist()
    }

    pub fn seed_context(&self, execution_attempt_id: i64, host_id: i64) -> Result<(), String> {
        let connection = smolrunners_core::store::open_connection(&self.db_path)
            .map_err(|error| format!("open context connection: {error}"))?;
        connection
            .execute_batch(&format!(
                "INSERT OR IGNORE INTO github_installation
                    (id, installation_id, app_id, account_login, scope,
                     runner_group_id, runner_group_name, created_at, updated_at)
                 VALUES (1, 1, 1, 'cli-e2e', 'all', 1, 'rg', 'now', 'now');
                 INSERT OR IGNORE INTO workflow_run
                    (id, installation_id, repository, workflow_name,
                     workflow_run_id, head_branch, created_at)
                 VALUES (1, 1, 'o/r', 'wf', 1, 'main', 'now');
                 INSERT OR IGNORE INTO workflow_run_attempt
                    (id, workflow_run_id, attempt, created_at)
                 VALUES (1, 1, 1, 'now');
                 INSERT OR IGNORE INTO job
                    (id, workflow_run_attempt_id, github_job_key, name, created_at)
                 VALUES (1, 1, 'cli_e2e', 'cli-e2e', 'now');
                 INSERT OR IGNORE INTO execution_attempt
                    (id, job_id, attempt, state, started_at)
                 VALUES ({execution_attempt_id}, 1, 1, 'busy', 'now');
                 INSERT OR IGNORE INTO host
                    (id, host_key, incarnation, hostname, arch, total_vcpu,
                     total_memory_gib, total_disk_bytes, capabilities_json,
                     admission_state, created_at)
                 VALUES ({host_id}, 'host-{host_id}', 1, 'sr-host-a', 'arm64',
                         4, 8, 1000, '{{}}', 'eligible', 'now');"
            ))
            .map_err(|error| format!("seed sample context: {error}"))
    }

    pub fn ingest_sample(
        &mut self,
        sample: Sample,
        execution_attempt_id: i64,
        host_id: Option<i64>,
    ) -> Result<Value, String> {
        let outcome = self
            .history
            .ingest(
                sample,
                IngestContext {
                    host_id,
                    execution_attempt_id: Some(execution_attempt_id),
                },
            )
            .map_err(|error| format!("ingest sample: {error}"))?;
        self.history
            .flush()
            .map_err(|error| format!("flush samples: {error}"))?;
        Ok(json!({
            "version": SCHEMA_VERSION,
            "cohort": outcome.cohort.id(),
            "gap": outcome.gap,
            "gap_reason": outcome.gap_reason.map(|reason| format!("{reason:?}")),
            "rates": outcome.rates.len(),
        }))
    }

    pub fn metric_rows(&self, execution_attempt_id: i64) -> Result<Vec<Value>, String> {
        let rows = self
            .history
            .rows_for(execution_attempt_id)
            .map_err(|error| format!("read samples: {error}"))?;
        Ok(rows
            .into_iter()
            .map(|row| {
                json!({
                    "scope": row.scope,
                    "metric": row.metric,
                    "value": row.value,
                    "unit": row.unit,
                    "sequence": row.sequence,
                    "captured_at": row.captured_at,
                })
            })
            .collect())
    }

    pub fn host(&self, id: i64) -> Option<&HostRecord> {
        self.data.hosts.iter().find(|host| host.id == id)
    }

    pub fn profile(&self, name: &str) -> Option<&ProfileRecord> {
        self.data.profiles.iter().find(|profile| profile.name == name)
    }

    pub fn sample_rows_for(&self, execution_attempt_id: i64) -> Vec<SampleRow> {
        self.history.rows_for(execution_attempt_id).unwrap_or_default()
    }

    pub fn summary_by_job(&self) -> HashMap<i64, Value> {
        let mut result = HashMap::new();
        for job in &self.data.jobs {
            let rows = self
                .history
                .rows_for(job.execution_attempt_id)
                .unwrap_or_default();
            let mut peak_cpu = 0.0f64;
            let mut peak_memory = 0.0f64;
            for row in &rows {
                if row.metric == "cpu.busy_cores" && row.value > peak_cpu {
                    peak_cpu = row.value;
                }
                if row.metric == "mem.used_bytes" && row.value > peak_memory {
                    peak_memory = row.value;
                }
            }
            result.insert(
                job.id,
                json!({
                    "peak_cpu_cores": peak_cpu,
                    "peak_memory_bytes": peak_memory,
                    "samples": rows.len(),
                }),
            );
        }
        result
    }
}
