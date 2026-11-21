use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Creating,
    Created,
    Starting,
    Started,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marker {
    pub machine: String,
    pub owner_pid: u32,
    pub incarnation: String,
    pub phase: Phase,
    pub image: String,
    pub uid: u32,
    pub cgroup: String,
}

pub fn directory(state: &Path) -> PathBuf {
    state.join("markers")
}

pub fn path(state: &Path, machine: &str) -> PathBuf {
    directory(state).join(format!("{machine}.json"))
}

pub fn write(state: &Path, marker: &Marker) -> io::Result<()> {
    let dir = directory(state);
    fs::create_dir_all(&dir)?;
    let final_path = path(state, &marker.machine);
    let temporary = dir.join(format!(".{}.{}.tmp", marker.machine, std::process::id()));
    let encoded = serde_json::to_vec(marker)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::write(&temporary, encoded)?;
    fs::rename(&temporary, &final_path)
}

pub fn read(path: &Path) -> Result<Marker, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse {}: {error}", path.display()))
}

pub fn scan(state: &Path) -> Result<Vec<(PathBuf, Marker)>, String> {
    let dir = directory(state);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(&dir).map_err(|error| format!("read {}: {error}", dir.display()))?;
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("read entry in {}: {error}", dir.display()))?;
        let entry_path = entry.path();
        if entry_path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        found.push((entry_path.clone(), read(&entry_path)?));
    }
    found.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(found)
}

pub fn remove(state: &Path, machine: &str) -> io::Result<()> {
    match fs::remove_file(path(state, machine)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
