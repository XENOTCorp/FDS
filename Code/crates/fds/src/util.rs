//! Small platform utilities shared by the engine and by applications
//! building on the FDS primitives.

use std::collections::BTreeMap;

/// Pin the calling thread to logical CPU `core` (`sched_setaffinity`).
pub fn pin_to_core(core: usize) -> std::io::Result<()> {
    let mut set = rustix::thread::CpuSet::new();
    set.set(core);
    rustix::thread::sched_setaffinity(None, &set).map_err(std::io::Error::from)
}

/// First logical CPU of each physical core, sorted, from sysfs sibling
/// groups. Two SMT threads of the same core share L1/L2; pinning two
/// workers there (logical 0 then 1 on this machine) puts both on one
/// core and leaves the other idle. Empty/unreadable sysfs yields `[0]`.
pub fn physical_cpus() -> Vec<usize> {
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    let Ok(rd) = std::fs::read_dir("/sys/devices/system/cpu") else {
        return vec![0];
    };
    for ent in rd.flatten() {
        let name = ent.file_name();
        let name = name.to_string_lossy();
        let Some(rest) = name.strip_prefix("cpu") else {
            continue;
        };
        if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(cpu) = rest.parse::<usize>() else {
            continue;
        };
        let path = ent.path().join("topology/thread_siblings_list");
        let Ok(s) = std::fs::read_to_string(path) else {
            continue;
        };
        let key = s.trim().to_string();
        if key.is_empty() {
            continue;
        }
        groups
            .entry(key)
            .and_modify(|c| *c = (*c).min(cpu))
            .or_insert(cpu);
    }
    let mut v: Vec<usize> = groups.into_values().collect();
    v.sort_unstable();
    if v.is_empty() {
        vec![0]
    } else {
        v
    }
}

/// L3 size in bytes from sysfs (`cpu0/cache/index3/size`). None when
/// the file is missing or unparseable.
pub fn l3_bytes() -> Option<u64> {
    let size = std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cache/index3/size").ok()?;
    parse_sysfs_size(size.trim())
}

fn parse_sysfs_size(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let digits_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let num: u64 = s[..digits_end].trim().parse().ok()?;
    let unit = s[digits_end..].trim().to_ascii_uppercase();
    let mult = match unit.as_str() {
        "" => 1,
        "K" | "KB" | "KIB" => 1 << 10,
        "M" | "MB" | "MIB" => 1 << 20,
        "G" | "GB" | "GIB" => 1 << 30,
        _ => return None,
    };
    Some(num * mult)
}

/// Coarse monotonic ticks (seconds since first call) for hot-state
/// activity stamps; no clock syscall per packet (Instant::elapsed reads
/// a vDSO time). Shared by the epoll and io_uring datapaths.
pub fn now_ticks() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_advance() {
        let a = now_ticks();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = now_ticks();
        assert!(b >= a);
    }

    #[test]
    fn pin_zero_is_ok_or_unavailable() {
        // Pinning to CPU 0 either works or reports EINVAL (no such CPU
        // / no permission); it must never panic.
        if let Ok(()) = pin_to_core(0) {}
    }

    #[test]
    fn physical_cpus_unique_sorted() {
        let cpus = physical_cpus();
        assert!(!cpus.is_empty());
        for w in cpus.windows(2) {
            assert!(w[0] < w[1], "physical_cpus not unique/sorted: {cpus:?}");
        }
    }

    #[test]
    fn parse_sysfs_size_l3_forms() {
        assert_eq!(parse_sysfs_size("3072K"), Some(3072 * 1024));
        assert_eq!(parse_sysfs_size("3M"), Some(3 << 20));
        assert_eq!(parse_sysfs_size("3145728"), Some(3145728));
    }
}
