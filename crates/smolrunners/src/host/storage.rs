use crate::host::config::Config;
use crate::host::smolvm::Smolvm;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Allocation {
    Sparse,
    Reflink,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeBlueprint {
    pub role: String,
    pub guest_path: String,
    pub byte_capacity: u64,
    pub inode_capacity: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Admission {
    pub schema_version: u32,
    pub kind: String,
    pub job: String,
    pub requested_bytes: u64,
    pub requested_inodes: u64,
    pub available_bytes: u64,
    pub available_inodes: u64,
    pub apparent_bytes: u64,
    pub allocated_bytes: u64,
    pub shared_bytes: u64,
    pub charge_bytes: u64,
    pub charge_inodes: u64,
    pub allocation: Allocation,
    pub admitted: bool,
    pub reason: Option<String>,
    pub volumes: Vec<VolumeBlueprint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeRecord {
    pub role: String,
    pub guest_path: String,
    pub format: String,
    pub disk_format: String,
    pub allocation: Allocation,
    pub backing_chain: Vec<String>,
    pub disk_path: String,
    pub apparent_bytes: u64,
    pub allocated_bytes: u64,
    pub shared_bytes: u64,
    pub inode_capacity: u64,
    pub filesystem_uuid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageRecord {
    pub schema_version: u32,
    pub kind: String,
    pub job: String,
    pub machine: String,
    pub storage_gb: u32,
    pub volumes: Vec<VolumeRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    pub schema_version: u32,
    pub kind: String,
    pub job: String,
    pub machine: String,
    pub verified: bool,
    pub checks: Vec<String>,
    pub mismatch: Option<String>,
}

fn directory(state: &Path) -> PathBuf {
    state.join("storage")
}

fn admission_path(state: &Path, job: &str) -> PathBuf {
    directory(state).join(format!("{job}.admission.json"))
}

fn record_path(state: &Path, job: &str) -> PathBuf {
    directory(state).join(format!("{job}.record.json"))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    fs::create_dir_all(path.parent().unwrap_or(path))
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    let encoded = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("encode {}: {error}", path.display()))?;
    fs::write(path, encoded).map_err(|error| format!("write {}: {error}", path.display()))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("parse {}: {error}", path.display()))
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn tool(name: &str, args: &[&str]) -> Result<Output, String> {
    Command::new(name)
        .args(args)
        .output()
        .map_err(|error| format!("spawn {name}: {error}"))
}

fn filesystem_stats(path: &Path) -> Result<(u64, u64, u64), String> {
    let display = path.to_string_lossy();
    let output = tool("stat", &["-f", "-c", "%S %a %d", &display])?;
    if !output.status.success() {
        return Err(format!("stat -f {}: {}", display, String::from_utf8_lossy(&output.stderr).trim()));
    }
    let fields: Vec<u64> = stdout(&output)
        .split_whitespace()
        .map(|value| value.parse::<u64>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("parse stat -f {display}: {error}"))?;
    if fields.len() != 3 {
        return Err(format!("unexpected stat -f output for {display}"));
    }
    Ok((fields[0], fields[1], fields[2]))
}

fn file_allocation(path: &Path) -> Result<(u64, u64), String> {
    let display = path.to_string_lossy();
    let output = tool("stat", &["-c", "%s %b %B", &display])?;
    if !output.status.success() {
        return Err(format!("stat {}: {}", display, String::from_utf8_lossy(&output.stderr).trim()));
    }
    let fields: Vec<u64> = stdout(&output)
        .split_whitespace()
        .map(|value| value.parse::<u64>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("parse stat {display}: {error}"))?;
    if fields.len() != 3 {
        return Err(format!("unexpected stat output for {display}"));
    }
    Ok((fields[0], fields[1] * fields[2]))
}

fn reflink_supported(state: &Path) -> bool {
    let scratch = directory(state).join("probe");
    if fs::create_dir_all(&scratch).is_err() {
        return false;
    }
    let source = scratch.join("source");
    let target = scratch.join("target");
    if fs::write(&source, b"smolrunners storage reflink probe").is_err() {
        return false;
    }
    let _ = fs::remove_file(&target);
    let output = Command::new("cp")
        .args([
            "--reflink=always",
            &source.to_string_lossy(),
            &target.to_string_lossy(),
        ])
        .output();
    let _ = fs::remove_file(&source);
    let _ = fs::remove_file(&target);
    output.is_ok_and(|value| value.status.success())
}

fn smolvm_json(config: &Config, args: &[&str]) -> Result<Value, String> {
    let smolvm = Smolvm::new(config);
    let output = smolvm.invoke(args, None)?;
    let text = stdout(&output);
    serde_json::from_str(&text).map_err(|error| format!("parse smolvm {} json: {error}", args.join(" ")))
}

fn machine_data_dir(config: &Config, machine: &str) -> Result<PathBuf, String> {
    let smolvm = Smolvm::new(config);
    let output = smolvm.invoke(&["machine", "data-dir", "--name", machine], None)?;
    let path = stdout(&output);
    if path.is_empty() {
        return Err(format!("smolvm reported no data dir for {machine}"));
    }
    Ok(PathBuf::from(path))
}

fn ext4_properties(raw: &Path) -> Result<(String, u64), String> {
    let display = raw.to_string_lossy();
    let output = tool("dumpe2fs", &["-h", &display])?;
    if !output.status.success() {
        return Err(format!("dumpe2fs {}: {}", display, String::from_utf8_lossy(&output.stderr).trim()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut uuid = String::new();
    let mut inodes = 0u64;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("Filesystem UUID:") {
            uuid = value.trim().to_string();
        }
        if let Some(value) = line.strip_prefix("Inode count:") {
            inodes = value
                .trim()
                .parse()
                .map_err(|error| format!("parse inode count of {display}: {error}"))?;
        }
    }
    if uuid.is_empty() || inodes == 0 {
        return Err(format!("storage image {display} does not report ext4 properties"));
    }
    Ok((uuid, inodes))
}

fn blueprints(bytes: u64, inodes: u64) -> Vec<VolumeBlueprint> {
    [
        ("workspace", "/workspace"),
        ("docker", "/var/lib/docker"),
        ("containerd", "/var/lib/containerd"),
    ]
    .into_iter()
    .map(|(role, guest_path)| VolumeBlueprint {
        role: role.to_string(),
        guest_path: guest_path.to_string(),
        byte_capacity: bytes,
        inode_capacity: inodes,
    })
    .collect()
}

fn available_after_charges(
    free_bytes: u64,
    free_inodes: u64,
    charged_bytes: u64,
    charged_inodes: u64,
) -> (u64, u64) {
    (
        free_bytes.saturating_sub(charged_bytes),
        free_inodes.saturating_sub(charged_inodes),
    )
}

fn committed_charges(state: &Path, job: &str) -> Result<(u64, u64), String> {
    let mut bytes = 0u64;
    let mut inodes = 0u64;
    let entries = match fs::read_dir(directory(state)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(error) => return Err(format!("list admissions: {error}")),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("read admission entry: {error}"))?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json")
            || path.file_name().and_then(|name| name.to_str()) == Some(format!("{job}.admission.json").as_str())
        {
            continue;
        }
        let raw = match fs::read(&path) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        let admission: Admission = match serde_json::from_slice(&raw) {
            Ok(admission) => admission,
            Err(_) => continue,
        };
        if admission.kind == "storage-admission" && admission.admitted {
            bytes = bytes.saturating_add(admission.charge_bytes);
            inodes = inodes.saturating_add(admission.charge_inodes);
        }
    }
    Ok((bytes, inodes))
}

pub fn admit(config: &Config, job: &str, bytes: u64, inodes: u64) -> Result<Admission, String> {
    let (block_size, free_blocks, free_inodes) = filesystem_stats(&config.state)?;
    let (charged_bytes, charged_inodes) = committed_charges(&config.state, job)?;
    let (available_bytes, available_inodes) = available_after_charges(
        free_blocks.saturating_mul(block_size),
        free_inodes,
        charged_bytes,
        charged_inodes,
    );
    let allocation = if reflink_supported(&config.state) {
        Allocation::Reflink
    } else {
        Allocation::Sparse
    };
    let template = config.runtime.join("storage-template.ext4");
    let (apparent_bytes, shared_bytes) = match file_allocation(&template) {
        Ok((apparent, allocated)) => (apparent, allocated),
        Err(_) => (0, 0),
    };
    let charge_bytes = match allocation {
        Allocation::Sparse => bytes,
        Allocation::Reflink => bytes.saturating_sub(shared_bytes).max(bytes / 4),
    };
    let charge_inodes = inodes;
    let byte_ok = bytes <= available_bytes;
    let inode_ok = inodes <= available_inodes;
    let reason = if !byte_ok && !inode_ok {
        Some(format!(
            "byte budget {bytes} exceeds available {available_bytes} and inode budget {inodes} exceeds available {available_inodes}"
        ))
    } else if !byte_ok {
        Some(format!("byte budget {bytes} exceeds available {available_bytes}"))
    } else if !inode_ok {
        Some(format!("inode budget {inodes} exceeds available {available_inodes}"))
    } else {
        None
    };
    let admission = Admission {
        schema_version: SCHEMA_VERSION,
        kind: "storage-admission".to_string(),
        job: job.to_string(),
        requested_bytes: bytes,
        requested_inodes: inodes,
        available_bytes,
        available_inodes,
        apparent_bytes,
        allocated_bytes: shared_bytes,
        shared_bytes,
        charge_bytes,
        charge_inodes,
        allocation,
        admitted: reason.is_none(),
        reason,
        volumes: blueprints(bytes, inodes),
    };
    write_json(&admission_path(&config.state, job), &admission)?;
    Ok(admission)
}

pub fn provision(
    config: &Config,
    job: &str,
    machine: &str,
    image: &str,
    storage_gb: u32,
    overlay_gb: u32,
) -> Result<StorageRecord, String> {
    let admission: Admission = read_json(&admission_path(&config.state, job))?;
    if !admission.admitted {
        return Err(format!("job {job} was not admitted: {}", admission.reason.unwrap_or_default()));
    }
    let smolvm = Smolvm::new(config);
    let storage = storage_gb.to_string();
    let overlay = overlay_gb.to_string();
    smolvm.invoke(
        &[
            "machine",
            "create",
            "--name",
            machine,
            "-I",
            image,
            "--storage",
            &storage,
            "--overlay",
            &overlay,
            "-v",
            "/var/lib/docker",
            "-v",
            "/var/lib/containerd",
        ],
        None,
    )?;
    smolvm.invoke(&["machine", "start", "--name", machine], None)?;
    record(config, job, machine)
}

pub fn record(config: &Config, job: &str, machine: &str) -> Result<StorageRecord, String> {
    let data_dir = machine_data_dir(config, machine)?;
    let raw = data_dir.join("storage.raw");
    let seed_file = data_dir.join("storage.seed");
    let seed = fs::read_to_string(&seed_file)
        .map_err(|error| format!("read {}: {error}", seed_file.display()))?
        .trim()
        .to_string();
    if seed.is_empty() {
        return Err(format!("{} has no backing seed", seed_file.display()));
    }
    let (apparent_bytes, allocated_bytes) = file_allocation(&raw)?;
    let (filesystem_uuid, inode_capacity) = ext4_properties(&raw)?;
    let (seed_apparent, seed_allocated) = file_allocation(Path::new(&seed))?;
    let template = config.runtime.join("storage-template.ext4").to_string_lossy().to_string();
    let raw_display = raw.to_string_lossy().to_string();
    let allocation = if reflink_supported(&config.state) {
        Allocation::Reflink
    } else {
        Allocation::Sparse
    };
    let shared_bytes = match allocation {
        Allocation::Reflink => seed_allocated,
        Allocation::Sparse => 0,
    };
    let _ = seed_apparent;
    let status = smolvm_json(config, &["machine", "status", "--name", machine, "--json"])?;
    let storage_gb = status
        .get("storage_gb")
        .and_then(Value::as_u64)
        .ok_or("smolvm status has no storage_gb")? as u32;
    let volume = |role: &str, guest_path: &str| VolumeRecord {
        role: role.to_string(),
        guest_path: guest_path.to_string(),
        format: "ext4".to_string(),
        disk_format: "raw".to_string(),
        allocation,
        backing_chain: vec![template.clone(), seed.clone(), raw_display.clone()],
        disk_path: raw_display.clone(),
        apparent_bytes,
        allocated_bytes,
        shared_bytes,
        inode_capacity,
        filesystem_uuid: filesystem_uuid.clone(),
    };
    let storage = StorageRecord {
        schema_version: SCHEMA_VERSION,
        kind: "storage-record".to_string(),
        job: job.to_string(),
        machine: machine.to_string(),
        storage_gb,
        volumes: vec![
            volume("workspace", "/workspace"),
            volume("docker", "/var/lib/docker"),
            volume("containerd", "/var/lib/containerd"),
        ],
    };
    write_json(&record_path(&config.state, job), &storage)?;
    Ok(storage)
}

pub fn report(config: &Config, job: &str) -> Result<StorageRecord, String> {
    read_json(&record_path(&config.state, job))
}

fn mismatch(checks: &mut Vec<String>, condition: bool, name: &str) -> Result<(), String> {
    if condition {
        checks.push(name.to_string());
        Ok(())
    } else {
        Err(format!("storage check failed: {name}"))
    }
}

pub fn verify(config: &Config, job: &str) -> Result<VerifyReport, String> {
    let stored: StorageRecord = report(config, job)?;
    let data_dir = machine_data_dir(config, &stored.machine)?;
    let raw = data_dir.join("storage.raw");
    let seed_file = data_dir.join("storage.seed");
    let seed = fs::read_to_string(&seed_file)
        .map_err(|error| format!("read {}: {error}", seed_file.display()))?
        .trim()
        .to_string();
    let (apparent_bytes, allocated_bytes) = file_allocation(&raw)?;
    let (filesystem_uuid, inode_capacity) = ext4_properties(&raw)?;
    let status = smolvm_json(
        config,
        &["machine", "status", "--name", &stored.machine, "--json"],
    )?;
    let storage_gb = status
        .get("storage_gb")
        .and_then(Value::as_u64)
        .ok_or("smolvm status has no storage_gb")? as u32;
    let mut checks = Vec::new();
    let result = (|| {
        mismatch(&mut checks, storage_gb == stored.storage_gb, "storage_gb matches smolvm status")?;
        for volume in &stored.volumes {
            mismatch(&mut checks, volume.format == "ext4", &format!("{} format is ext4", volume.role))?;
            mismatch(&mut checks, volume.disk_format == "raw", &format!("{} image is raw", volume.role))?;
            mismatch(
                &mut checks,
                volume.backing_chain.len() == 3 && volume.backing_chain[2] == raw.to_string_lossy(),
                &format!("{} raw disk is the backing-chain tip", volume.role),
            )?;
            mismatch(
                &mut checks,
                volume.backing_chain[1] == seed,
                &format!("{} seed matches smolvm storage.seed", volume.role),
            )?;
            mismatch(
                &mut checks,
                volume.allocated_bytes == allocated_bytes,
                &format!("{} allocated bytes match stat", volume.role),
            )?;
            mismatch(
                &mut checks,
                volume.apparent_bytes == apparent_bytes,
                &format!("{} apparent bytes match stat", volume.role),
            )?;
            mismatch(
                &mut checks,
                volume.inode_capacity == inode_capacity,
                &format!("{} inode capacity matches ext4 superblock", volume.role),
            )?;
            mismatch(
                &mut checks,
                volume.filesystem_uuid == filesystem_uuid,
                &format!("{} filesystem uuid matches ext4 superblock", volume.role),
            )?;
        }
        Ok(())
    })();
    Ok(VerifyReport {
        schema_version: SCHEMA_VERSION,
        kind: "storage-verify".to_string(),
        job: job.to_string(),
        machine: stored.machine,
        verified: result.is_ok(),
        checks,
        mismatch: result.err(),
    })
}

fn value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("storage {flag} requires a value"))
}

fn uint(args: &[String], index: &mut usize, flag: &str) -> Result<u64, String> {
    value(args, index, flag)?
        .parse()
        .map_err(|_| format!("storage: bad {flag}"))
}

pub fn dispatch(config: &Config, args: &[String]) -> Result<(), String> {
    let command = args.first().map(String::as_str).unwrap_or("");
    if command.is_empty() {
        return Err("storage: missing subcommand (admit|provision|record|report|verify)".to_string());
    }
    let mut job = String::new();
    let mut machine = String::new();
    let mut image = String::new();
    let mut bytes = 0u64;
    let mut inodes = 0u64;
    let mut storage_gb = 2u32;
    let mut overlay_gb = 1u32;
    let mut index = 1usize;
    while index < args.len() {
        match args[index].as_str() {
            "--job" => job = value(args, &mut index, "--job")?,
            "--machine" => machine = value(args, &mut index, "--machine")?,
            "--image" => image = value(args, &mut index, "--image")?,
            "--bytes" => bytes = uint(args, &mut index, "--bytes")?,
            "--inodes" => inodes = uint(args, &mut index, "--inodes")?,
            "--storage" => {
                storage_gb = uint(args, &mut index, "--storage")? as u32;
            }
            "--overlay" => {
                overlay_gb = uint(args, &mut index, "--overlay")? as u32;
            }
            other => return Err(format!("storage {command}: unknown option {other}")),
        }
        index += 1;
    }
    if job.is_empty() {
        return Err("storage: missing --job".to_string());
    }
    match command {
        "admit" => {
            let result = admit(config, &job, bytes, inodes)?;
            println!("{}", serde_json::to_string_pretty(&result).map_err(|error| format!("encode admission: {error}"))?);
            if !result.admitted {
                return Err(format!("job {job} refused: {}", result.reason.unwrap_or_default()));
            }
            Ok(())
        }
        "provision" => {
            if machine.is_empty() {
                return Err("storage provision: missing --machine".to_string());
            }
            if image.is_empty() {
                return Err("storage provision: missing --image".to_string());
            }
            let result = provision(config, &job, &machine, &image, storage_gb, overlay_gb)?;
            println!("PROVISIONED {machine}");
            println!("{}", serde_json::to_string_pretty(&result).map_err(|error| format!("encode record: {error}"))?);
            Ok(())
        }
        "record" => {
            if machine.is_empty() {
                return Err("storage record: missing --machine".to_string());
            }
            let result = record(config, &job, &machine)?;
            println!("{}", serde_json::to_string_pretty(&result).map_err(|error| format!("encode record: {error}"))?);
            Ok(())
        }
        "report" => {
            let result = report(config, &job)?;
            println!("{}", serde_json::to_string_pretty(&result).map_err(|error| format!("encode report: {error}"))?);
            Ok(())
        }
        "verify" => {
            let result = verify(config, &job)?;
            println!("{}", serde_json::to_string_pretty(&result).map_err(|error| format!("encode verify: {error}"))?);
            if !result.verified {
                return Err(format!("job {job} storage mismatch: {}", result.mismatch.unwrap_or_default()));
            }
            Ok(())
        }
        other => Err(format!("storage: unknown subcommand {other}")),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::host::config::Config;

    static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let sequence = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "smolrunners-admission-{}-{sequence}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp state dir");
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn config(state: &Path) -> Config {
        Config {
            state: state.to_path_buf(),
            runtime: state.to_path_buf(),
            launcher: state.join("launcher"),
            uid: 60000,
            cpu_max: "200000 100000".to_owned(),
            memory_max: "2147483648".to_owned(),
            pids_max: "512".to_owned(),
            net_allow: "127.0.0.1".to_owned(),
            hold_after_create_ms: 0,
        }
    }

    #[test]
    fn committed_admissions_reserve_free_space() {
        let dir = TestDir::new();
        let config = config(&dir.0);
        let first = Admission {
            schema_version: SCHEMA_VERSION,
            kind: "storage-admission".to_owned(),
            job: "job-a".to_owned(),
            requested_bytes: 100,
            requested_inodes: 4,
            available_bytes: 1000,
            available_inodes: 40,
            apparent_bytes: 100,
            allocated_bytes: 100,
            shared_bytes: 0,
            charge_bytes: 400,
            charge_inodes: 16,
            allocation: Allocation::Sparse,
            admitted: true,
            reason: None,
            volumes: Vec::new(),
        };
        write_json(&admission_path(&config.state, "job-a"), &first).expect("write first admission");

        let rejected = Admission { admitted: false, charge_bytes: 900, ..first.clone() };
        write_json(&admission_path(&config.state, "job-b"), &rejected).expect("write rejected admission");

        let (charged_bytes, charged_inodes) = committed_charges(&config.state, "job-c").expect("charges");
        assert_eq!(charged_bytes, 400);
        assert_eq!(charged_inodes, 16);
        let (available_bytes, available_inodes) =
            available_after_charges(1000, 40, charged_bytes, charged_inodes);
        assert_eq!(available_bytes, 600);
        assert_eq!(available_inodes, 24);
        assert!(1000 > available_bytes);
    }
}
