mod cgroup;
mod clock;
mod procfs;
mod sample;

use std::io::{self, BufWriter, Write};
use std::time::Duration;

use procfs::{CpuJiffies, DiskStats, NetworkStats};
use sample::{MetricValue, SampleData};

struct Config {
    interval_ms: u64,
    generation: u64,
    batch_lines: usize,
    boot_id: String,
}

fn parse_value(args: &[String], index: usize, flag: &str) -> String {
    args.get(index)
        .unwrap_or_else(|| panic!("collector: {flag} needs a value"))
        .clone()
}

fn parse_args() -> Config {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut interval_ms = 1000u64;
    let mut generation = 1u64;
    let mut batch_lines = 18usize;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--interval-ms" => {
                index += 1;
                interval_ms = parse_value(&args, index, "--interval-ms")
                    .parse()
                    .expect("collector: invalid --interval-ms");
            }
            "--generation" => {
                index += 1;
                generation = parse_value(&args, index, "--generation")
                    .parse()
                    .expect("collector: invalid --generation");
            }
            "--batch-lines" => {
                index += 1;
                batch_lines = parse_value(&args, index, "--batch-lines")
                    .parse()
                    .expect("collector: invalid --batch-lines");
            }
            "--help" => {
                println!("usage: smolrunners-collector [--interval-ms N] [--generation N] [--batch-lines N]");
                std::process::exit(0);
            }
            other => panic!("collector: unknown argument {other}"),
        }
        index += 1;
    }
    assert!(interval_ms > 0, "collector: --interval-ms must be positive");
    assert!(batch_lines > 0, "collector: --batch-lines must be positive");
    let boot_id = clock::boot_id();
    assert!(!boot_id.is_empty(), "collector: no boot id available");
    Config {
        interval_ms,
        generation,
        batch_lines,
        boot_id,
    }
}

fn metric(name: &'static str, value: f64, unit: &'static str) -> MetricValue {
    MetricValue { name, value, unit }
}

fn flush_batch(writer: &mut BufWriter<io::StdoutLock<'_>>, batch: &mut Vec<String>) {
    if batch.is_empty() {
        return;
    }
    let payload = batch.concat();
    writer
        .write_all(payload.as_bytes())
        .expect("collector: write telemetry batch");
    writer.flush().expect("collector: flush telemetry batch");
    batch.clear();
}

fn main() {
    let config = parse_args();
    let parent = cgroup::ensure_workload_parent();
    if let Err(reason) = &parent {
        eprintln!("EVENT workload_cgroup_parent_unavailable reason={reason}");
    }
    let workload_root = cgroup::workload_root(parent.is_ok());

    let stdout = io::stdout();
    let mut writer = BufWriter::with_capacity(1 << 16, stdout.lock());
    let mut batch: Vec<String> = Vec::with_capacity(config.batch_lines);
    let mut sequence = 0u64;
    let mut previous_cpu: Option<CpuJiffies> = None;
    let mut previous_disk: Option<DiskStats> = None;
    let mut previous_network: Option<NetworkStats> = None;

    loop {
        let monotonic_ns = clock::monotonic_ns();
        let wall_unix_ms = clock::wall_unix_ms();
        sequence += 1;

        let cpu_now = procfs::cpu_jiffies();
        let cpu_delta_busy = previous_cpu
            .map(|previous| cpu_now.busy().saturating_sub(previous.busy()))
            .unwrap_or(0);
        let cpu_delta_total = previous_cpu
            .map(|previous| cpu_now.total().saturating_sub(previous.total()))
            .unwrap_or(0);
        let cpu_count = procfs::cpu_count() as f64;
        let busy_cores = if cpu_delta_total == 0 {
            0.0
        } else {
            cpu_delta_busy as f64 / cpu_delta_total as f64 * cpu_count
        };
        let utilization_percent = busy_cores / cpu_count * 100.0;
        previous_cpu = Some(cpu_now);

        let memory = procfs::memory_info();
        let cpu_pressure = procfs::pressure("/proc/pressure/cpu");
        let memory_pressure = procfs::pressure("/proc/pressure/memory");
        let io_pressure = procfs::pressure("/proc/pressure/io");
        let disk_now = procfs::disk_stats();
        let network_now = procfs::network_stats();
        let disk_read_delta = procfs::delta(disk_now.read_bytes, previous_disk.map(|value| value.read_bytes));
        let disk_write_delta = procfs::delta(disk_now.write_bytes, previous_disk.map(|value| value.write_bytes));
        let network_rx_delta = procfs::delta(network_now.rx_bytes, previous_network.map(|value| value.rx_bytes));
        let network_tx_delta = procfs::delta(network_now.tx_bytes, previous_network.map(|value| value.tx_bytes));
        previous_disk = Some(disk_now);
        previous_network = Some(network_now);

        let guest_metrics = vec![
            metric("cpu.busy_cores", busy_cores, "cores"),
            metric("cpu.utilization_percent", utilization_percent, "percent"),
            metric("cpu.busy_usec_total", cpu_now.busy_usec(), "us"),
            metric("cpu.total_usec_total", cpu_now.total_usec(), "us"),
            metric("cpu.iowait_usec_total", cpu_now.iowait as f64 * 10_000.0, "us"),
            metric("cpu.steal_jiffies", cpu_now.steal as f64, "count"),
            metric("mem.total_bytes", memory.total_bytes as f64, "bytes"),
            metric("mem.available_bytes", memory.available_bytes as f64, "bytes"),
            metric(
                "mem.used_bytes",
                memory.total_bytes.saturating_sub(memory.available_bytes) as f64,
                "bytes",
            ),
            metric("mem.cached_bytes", memory.cached_bytes as f64, "bytes"),
            metric("mem.buffers_bytes", memory.buffers_bytes as f64, "bytes"),
            metric("mem.shmem_bytes", memory.shmem_bytes as f64, "bytes"),
            metric("mem.swap_total_bytes", memory.swap_total_bytes as f64, "bytes"),
            metric(
                "mem.swap_used_bytes",
                memory.swap_total_bytes.saturating_sub(memory.swap_free_bytes) as f64,
                "bytes",
            ),
            metric("psi.cpu.some_total", cpu_pressure.some_total as f64, "count"),
            metric("psi.memory.some_total", memory_pressure.some_total as f64, "count"),
            metric("psi.io.some_total", io_pressure.some_total as f64, "count"),
            metric("psi.cpu.some_avg10", cpu_pressure.some_avg10, "percent"),
            metric("psi.memory.some_avg10", memory_pressure.some_avg10, "percent"),
            metric("disk.read_bytes_total", disk_now.read_bytes as f64, "bytes"),
            metric("disk.write_bytes_total", disk_now.write_bytes as f64, "bytes"),
            metric("disk.read_bytes_delta", disk_read_delta as f64, "bytes"),
            metric("disk.write_bytes_delta", disk_write_delta as f64, "bytes"),
            metric("net.rx_bytes_total", network_now.rx_bytes as f64, "bytes"),
            metric("net.tx_bytes_total", network_now.tx_bytes as f64, "bytes"),
            metric("net.rx_bytes_delta", network_rx_delta as f64, "bytes"),
            metric("net.tx_bytes_delta", network_tx_delta as f64, "bytes"),
        ];
        batch.push(sample::serialize(&SampleData {
            scope: "guest",
            sequence,
            monotonic_ns,
            wall_unix_ms,
            boot_id: &config.boot_id,
            generation: config.generation,
            metrics: guest_metrics,
        }));

        let workload = cgroup::workload_stats(&workload_root);
        let workload_metrics = vec![
            metric("cgroup.cpu.usage_usec", workload.cpu_usage_usec as f64, "us"),
            metric("cgroup.cpu.throttled_usec", workload.cpu_throttled_usec as f64, "us"),
            metric("cgroup.cpu.nr_throttled", workload.cpu_nr_throttled as f64, "count"),
            metric("cgroup.memory.current", workload.memory_current as f64, "bytes"),
            metric("cgroup.memory.peak", workload.memory_peak as f64, "bytes"),
            metric("cgroup.memory.max", workload.memory_max as f64, "bytes"),
            metric(
                "cgroup.memory.swap.current",
                workload.memory_swap_current as f64,
                "bytes",
            ),
            metric("cgroup.memory.events.oom", workload.memory_events_oom as f64, "count"),
            metric(
                "cgroup.memory.events.oom_kill",
                workload.memory_events_oom_kill as f64,
                "count",
            ),
            metric("cgroup.memory.events.max", workload.memory_events_max as f64, "count"),
            metric("cgroup.memory.anon", workload.memory_anon as f64, "bytes"),
            metric("cgroup.memory.file", workload.memory_file as f64, "bytes"),
            metric("cgroup.io.read_bytes", workload.io_read_bytes as f64, "bytes"),
            metric("cgroup.io.write_bytes", workload.io_write_bytes as f64, "bytes"),
            metric("cgroup.descendants", workload.descendant_count as f64, "count"),
        ];
        batch.push(sample::serialize(&SampleData {
            scope: "workload",
            sequence,
            monotonic_ns,
            wall_unix_ms,
            boot_id: &config.boot_id,
            generation: config.generation,
            metrics: workload_metrics,
        }));

        let host = cgroup::host_stats();
        let host_pressure = procfs::pressure("/sys/fs/cgroup/cpu.pressure");
        let host_memory_pressure = procfs::pressure("/sys/fs/cgroup/memory.pressure");
        let host_metrics = vec![
            metric("host.cgroup.cpu.usage_usec", host.cpu_usage_usec as f64, "us"),
            metric("host.cgroup.memory.current", host.memory_current as f64, "bytes"),
            metric("host.cpu.steal_jiffies", cpu_now.steal as f64, "count"),
            metric("host.pressure.cpu.some_total", host_pressure.some_total as f64, "count"),
            metric(
                "host.pressure.memory.some_total",
                host_memory_pressure.some_total as f64,
                "count",
            ),
        ];
        batch.push(sample::serialize(&SampleData {
            scope: "host",
            sequence,
            monotonic_ns,
            wall_unix_ms,
            boot_id: &config.boot_id,
            generation: config.generation,
            metrics: host_metrics,
        }));

        if batch.len() + 3 > config.batch_lines {
            flush_batch(&mut writer, &mut batch);
        }
        std::thread::sleep(Duration::from_millis(config.interval_ms));
    }
}
