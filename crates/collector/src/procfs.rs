use std::fs;

const USER_HZ: f64 = 100.0;

#[derive(Debug, Clone, Copy, Default)]
pub struct CpuJiffies {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuJiffies {
    pub fn busy(self) -> u64 {
        self.user + self.nice + self.system + self.irq + self.softirq + self.steal
    }

    pub fn total(self) -> u64 {
        self.busy() + self.idle + self.iowait
    }

    pub fn busy_usec(self) -> f64 {
        self.busy() as f64 * 1_000_000.0 / USER_HZ
    }

    pub fn total_usec(self) -> f64 {
        self.total() as f64 * 1_000_000.0 / USER_HZ
    }
}

pub fn cpu_jiffies() -> CpuJiffies {
    let text = fs::read_to_string("/proc/stat").unwrap_or_default();
    let mut result = CpuJiffies::default();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() != Some("cpu") {
            continue;
        }
        let values: Vec<u64> = fields.filter_map(|field| field.parse().ok()).collect();
        if values.len() < 8 {
            continue;
        }
        result = CpuJiffies {
            user: values[0],
            nice: values[1],
            system: values[2],
            idle: values[3],
            iowait: values[4],
            irq: values[5],
            softirq: values[6],
            steal: values[7],
        };
        break;
    }
    result
}

fn is_per_cpu_name(name: &str) -> bool {
    name.strip_prefix("cpu")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
}

pub fn cpu_count() -> usize {
    let text = fs::read_to_string("/proc/stat").unwrap_or_default();
    text.lines()
        .filter(|line| {
            line.split_whitespace()
                .next()
                .is_some_and(is_per_cpu_name)
        })
        .count()
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::is_per_cpu_name;

    #[test]
    fn collector_cpu_count_ignores_the_aggregate_line() {
        assert!(!is_per_cpu_name("cpu"));
        assert!(is_per_cpu_name("cpu0"));
        assert!(is_per_cpu_name("cpu17"));
        assert!(!is_per_cpu_name("cpu0a"));
        assert!(!is_per_cpu_name("cpufreq"));
        assert!(!is_per_cpu_name("intr"));
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub free_bytes: u64,
    pub cached_bytes: u64,
    pub buffers_bytes: u64,
    pub shmem_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_free_bytes: u64,
}

fn kib_value(line: &str) -> u64 {
    line.split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
        * 1024
}

pub fn memory_info() -> MemoryInfo {
    let text = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut info = MemoryInfo::default();
    for line in text.lines() {
        let key = line.split(':').next().unwrap_or("");
        let value = kib_value(line);
        match key {
            "MemTotal" => info.total_bytes = value,
            "MemAvailable" => info.available_bytes = value,
            "MemFree" => info.free_bytes = value,
            "Cached" => info.cached_bytes = value,
            "Buffers" => info.buffers_bytes = value,
            "Shmem" => info.shmem_bytes = value,
            "SwapTotal" => info.swap_total_bytes = value,
            "SwapFree" => info.swap_free_bytes = value,
            _ => {}
        }
    }
    info
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Pressure {
    pub some_total: u64,
    pub full_total: u64,
    pub some_avg10: f64,
    pub some_avg60: f64,
    pub some_avg300: f64,
}

pub fn pressure(path: &str) -> Pressure {
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut result = Pressure::default();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let kind = fields.next().unwrap_or("");
        let mut values = Pressure::default();
        for field in fields {
            let (name, value) = match field.split_once('=') {
                Some(pair) => pair,
                None => continue,
            };
            let Ok(number) = value.parse::<f64>() else { continue };
            match name {
                "total" => {
                    if kind == "some" {
                        values.some_total = number as u64;
                    } else if kind == "full" {
                        values.full_total = number as u64;
                    }
                }
                "avg10" => values.some_avg10 = number,
                "avg60" => values.some_avg60 = number,
                "avg300" => values.some_avg300 = number,
                _ => {}
            }
        }
        if kind == "some" {
            result.some_total = values.some_total;
            result.some_avg10 = values.some_avg10;
            result.some_avg60 = values.some_avg60;
            result.some_avg300 = values.some_avg300;
        } else if kind == "full" {
            result.full_total = values.full_total;
        }
    }
    result
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DiskStats {
    pub read_bytes: u64,
    pub write_bytes: u64,
}

pub fn disk_stats() -> DiskStats {
    let text = fs::read_to_string("/proc/diskstats").unwrap_or_default();
    let mut result = DiskStats::default();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 10 {
            continue;
        }
        let read_sectors = fields[5].parse::<u64>().unwrap_or(0);
        let write_sectors = fields[9].parse::<u64>().unwrap_or(0);
        result.read_bytes = result.read_bytes.saturating_add(read_sectors * 512);
        result.write_bytes = result.write_bytes.saturating_add(write_sectors * 512);
    }
    result
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NetworkStats {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

pub fn network_stats() -> NetworkStats {
    let text = fs::read_to_string("/proc/net/dev").unwrap_or_default();
    let mut result = NetworkStats::default();
    for line in text.lines().skip(2) {
        let (_, counters) = match line.split_once(':') {
            Some(pair) => pair,
            None => continue,
        };
        let fields: Vec<&str> = counters.split_whitespace().collect();
        if fields.len() < 9 {
            continue;
        }
        result.rx_bytes = result.rx_bytes.saturating_add(fields[0].parse::<u64>().unwrap_or(0));
        result.tx_bytes = result.tx_bytes.saturating_add(fields[8].parse::<u64>().unwrap_or(0));
    }
    result
}

pub fn delta(current: u64, previous: Option<u64>) -> u64 {
    match previous {
        Some(value) if current >= value => current - value,
        _ => 0,
    }
}
