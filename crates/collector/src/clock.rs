#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

unsafe extern "C" {
    fn clock_gettime(clock_id: i32, timespec: *mut Timespec) -> i32;
}

const CLOCK_REALTIME: i32 = 0;
const CLOCK_MONOTONIC: i32 = 1;

fn read(clock_id: i32) -> Timespec {
    let mut timespec = Timespec::default();
    let result = unsafe { clock_gettime(clock_id, &mut timespec) };
    assert_eq!(result, 0, "clock_gettime failed for clock {clock_id}");
    timespec
}

pub fn monotonic_ns() -> u64 {
    let timespec = read(CLOCK_MONOTONIC);
    (timespec.tv_sec as u64) * 1_000_000_000 + timespec.tv_nsec as u64
}

pub fn wall_unix_ms() -> i64 {
    let timespec = read(CLOCK_REALTIME);
    timespec.tv_sec * 1000 + timespec.tv_nsec / 1_000_000
}

pub fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .unwrap_or_default()
        .trim()
        .to_owned()
}
