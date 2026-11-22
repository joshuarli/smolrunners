use std::fs;
use std::path::Path;

pub const WORKLOAD_PARENT: &str = "/sys/fs/cgroup/smolrunners-workload.slice";
const CGROUP_ROOT: &str = "/sys/fs/cgroup";
const WALK_BUDGET: usize = 512;

#[derive(Debug, Clone, Default)]
pub struct CgroupStats {
    pub cpu_usage_usec: u64,
    pub cpu_nr_throttled: u64,
    pub cpu_throttled_usec: u64,
    pub memory_current: u64,
    pub memory_peak: u64,
    pub memory_max: u64,
    pub memory_swap_current: u64,
    pub memory_events_oom: u64,
    pub memory_events_oom_kill: u64,
    pub memory_events_max: u64,
    pub memory_anon: u64,
    pub memory_file: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub descendant_count: u64,
}

pub fn ensure_workload_parent() -> Result<(), String> {
    fs::create_dir_all(WORKLOAD_PARENT).map_err(|error| format!("{WORKLOAD_PARENT}: {error}"))
}

pub fn workload_root(parent_ready: bool) -> String {
    if parent_ready && Path::new(WORKLOAD_PARENT).is_dir() {
        return WORKLOAD_PARENT.to_owned();
    }
    let cgroup = fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    let path = cgroup
        .lines()
        .find_map(|line| line.split_once("::"))
        .map(|(_, path)| path)
        .unwrap_or("/");
    format!("{CGROUP_ROOT}{path}")
}

fn read_number(path: impl AsRef<Path>) -> u64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

fn read_named(path: impl AsRef<Path>, key: &str) -> u64 {
    let text = fs::read_to_string(path).unwrap_or_default();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() == Some(key) {
            return fields.next().and_then(|value| value.parse().ok()).unwrap_or(0);
        }
    }
    0
}

fn read_limit(path: impl AsRef<Path>) -> u64 {
    let text = fs::read_to_string(path).unwrap_or_default();
    match text.trim() {
        "max" => u64::MAX,
        value => value.parse().unwrap_or(0),
    }
}

fn read_keyed_totals(path: impl AsRef<Path>, keys: &[&str]) -> Vec<u64> {
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut totals = vec![0u64; keys.len()];
    for field in text.split_whitespace() {
        if let Some((name, value)) = field.split_once('=') {
            if let Some(index) = keys.iter().position(|key| *key == name) {
                totals[index] = totals[index].saturating_add(value.parse::<u64>().unwrap_or(0));
            }
        }
    }
    totals
}

fn walk(path: &str, stats: &mut CgroupStats, is_root: bool, budget: &mut usize) {
    if *budget == 0 {
        return;
    }
    *budget -= 1;

    if is_root {
        stats.cpu_usage_usec = read_named(format!("{path}/cpu.stat"), "usage_usec");
        stats.cpu_nr_throttled = read_named(format!("{path}/cpu.stat"), "nr_throttled");
        stats.cpu_throttled_usec = read_named(format!("{path}/cpu.stat"), "throttled_usec");

        stats.memory_events_oom = read_named(format!("{path}/memory.events"), "oom");
        stats.memory_events_oom_kill = read_named(format!("{path}/memory.events"), "oom_kill");
        stats.memory_events_max = read_named(format!("{path}/memory.events"), "max");
    }

    let io_totals = read_keyed_totals(format!("{path}/io.stat"), &["rbytes", "wbytes"]);
    stats.io_read_bytes = stats.io_read_bytes.saturating_add(io_totals[0]);
    stats.io_write_bytes = stats.io_write_bytes.saturating_add(io_totals[1]);

    if is_root {
        stats.memory_current = read_number(format!("{path}/memory.current"));
        stats.memory_peak = read_number(format!("{path}/memory.peak"));
        if stats.memory_peak < stats.memory_current {
            stats.memory_peak = stats.memory_current;
        }
        stats.memory_max = read_limit(format!("{path}/memory.max"));
        stats.memory_swap_current = read_number(format!("{path}/memory.swap.current"));
        stats.memory_anon = read_named(format!("{path}/memory.stat"), "anon");
        stats.memory_file = read_named(format!("{path}/memory.stat"), "file");
    }

    let Ok(entries) = fs::read_dir(path) else { return };
    for entry in entries.flatten() {
        if *budget == 0 {
            break;
        }
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            stats.descendant_count += 1;
            walk(&entry.path().to_string_lossy(), stats, false, budget);
        }
    }
}

pub fn workload_stats(root: &str) -> CgroupStats {
    let mut stats = CgroupStats::default();
    let mut budget = WALK_BUDGET;
    walk(root, &mut stats, true, &mut budget);
    stats
}

pub fn host_stats() -> CgroupStats {
    let mut stats = CgroupStats::default();
    let mut budget = 1;
    walk(CGROUP_ROOT, &mut stats, true, &mut budget);
    stats
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(label: &str) -> Self {
            let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "smolrunners-cgroup-{label}-{}-{sequence}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create fake cgroup root");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: impl AsRef<Path>, text: impl AsRef<str>) {
        fs::write(path, text.as_ref()).expect("write fake cgroup file");
    }

    fn seed(root: &Path, usage: u64, oom: u64, read: u64) {
        write(root.join("cpu.stat"), format!("usage_usec {usage}\nnr_throttled 0\nthrottled_usec 0\n"));
        write(root.join("memory.events"), format!("low 0\nhigh 0\nmax 0\noom {oom}\noom_kill 0\n"));
        write(root.join("io.stat"), format!("8:0 rbytes={read} wbytes=0\n"));
        write(root.join("memory.current"), "1024\n");
        write(root.join("memory.peak"), "2048\n");
        write(root.join("memory.max"), "max\n");
        write(root.join("memory.swap.current"), "0\n");
        write(root.join("memory.stat"), "anon 100\nfile 200\n");
    }

    #[test]
    fn hierarchical_counters_are_not_added_to_descendants() {
        let root = TestRoot::new("hierarchy");
        seed(root.path(), 100, 1, 1000);
        let child = root.path().join("child.slice");
        fs::create_dir_all(&child).expect("create child cgroup");
        seed(&child, 900, 5, 9000);

        let stats = workload_stats(root.path().to_str().expect("utf8 path"));
        assert_eq!(stats.cpu_usage_usec, 100);
        assert_eq!(stats.memory_events_oom, 1);
        assert_eq!(stats.io_read_bytes, 10_000);
        assert_eq!(stats.descendant_count, 1);
        assert_eq!(stats.memory_current, 1024);
        assert_eq!(stats.memory_anon, 100);
    }
}

