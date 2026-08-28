//! Hardware detection for FDS: reads `/proc/cpuinfo`, `/proc/meminfo`, and
//! sysfs cache/topology files. No Python, no external tools; pure `std`
//! filesystem reads, deterministic on a given machine. `build/detect.sh` is
//! the bash twin of this module; the two agree on every value (fds-detect
//! cross-checks lscpu/sysfs where detect.sh prefers lscpu).

use std::collections::BTreeSet;
use std::fs;

/// rustc `target-feature` names keyed by their `/proc/cpuinfo` flag names.
/// The mapping mirrors what `target-cpu=native` would enable on this CPU,
/// and is what a pinned (non-`native`) `TARGET_CPU` build feeds back via
/// `-C target-feature=+...`.
const FLAG_TO_TARGET_FEATURE: &[(&str, &str)] = &[
    ("avx2", "avx2"),
    ("avx512f", "avx512f"),
    ("avx512bw", "avx512bw"),
    ("avx512cd", "avx512cd"),
    ("avx512dq", "avx512dq"),
    ("avx512vl", "avx512vl"),
    ("sse4_2", "sse4.2"),
    ("ssse3", "ssse3"),
    ("fma", "fma"),
    ("f16c", "f16c"),
    ("bmi1", "bmi1"),
    ("bmi2", "bmi2"),
    ("popcnt", "popcnt"),
    ("lzcnt", "lzcnt"),
    ("movbe", "movbe"),
    ("aes", "aes"),
    ("pclmulqdq", "pclmulqdq"),
];

/// Detected hardware facts. Every field is `Option`/empty when the source
/// file is missing or unparseable; absence is a fact too (reported as
/// "unknown"), never a crash.
#[derive(Clone, Debug, Default)]
pub(crate) struct Hardware {
    pub vendor: String,
    pub model: String,
    /// Detected SIMD capabilities as rustc `target-feature` names, sorted
    /// and deduplicated.
    pub simd: Vec<String>,
    pub l3_bytes: Option<u64>,
    pub physical_cores: Option<usize>,
    pub logical_cores: Option<usize>,
    pub numa_nodes: Option<usize>,
    pub hugepages_total: Option<u64>,
    pub hugepages_free: Option<u64>,
}

impl Hardware {
    /// Logical/physical ratio (1 on non-SMT machines).
    pub fn threads_per_core(&self) -> Option<usize> {
        match (self.logical_cores, self.physical_cores) {
            (Some(l), Some(p)) if p > 0 => Some((l / p).max(1)),
            _ => None,
        }
    }

    /// Hugepages are usable only when the kernel has allocated any
    /// (`HugePages_Total > 0`). Mount state is checked by `detect.sh` and
    /// the ops docs; this module reports the kernel-side fact.
    pub fn hugepages_available(&self) -> bool {
        self.hugepages_total.unwrap_or(0) > 0
    }
}

/// Read the real machine. All reads are best-effort; a missing file yields
/// an empty/`None` field rather than an error.
pub(crate) fn detect() -> Hardware {
    let mut hw = Hardware::default();
    parse_cpuinfo(&fs::read_to_string("/proc/cpuinfo").unwrap_or_default(), &mut hw);
    hw.l3_bytes = l3_bytes();
    hw.numa_nodes = numa_nodes();
    parse_meminfo(&fs::read_to_string("/proc/meminfo").unwrap_or_default(), &mut hw);
    hw.simd.sort();
    hw.simd.dedup();
    hw
}

/// Parse `/proc/cpuinfo` text: vendor, model name, SIMD flags (first CPU
/// only; every CPU on a homogeneous machine carries the same flag set),
/// logical core count (`processor` lines), and physical core count
/// (distinct `physical id`/`core id` pairs).
fn parse_cpuinfo(text: &str, hw: &mut Hardware) {
    let mut phys_id = String::new();
    let mut cores = BTreeSet::new();
    let mut seen_flags = false;
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let k = k.trim();
        let v = v.trim();
        match k {
            "vendor_id" if hw.vendor.is_empty() => hw.vendor = v.to_string(),
            "model name" if hw.model.is_empty() => hw.model = v.to_string(),
            "flags" if !seen_flags => {
                seen_flags = true;
                for &(flag, feat) in FLAG_TO_TARGET_FEATURE {
                    if v.split_whitespace().any(|f| f == flag) {
                        hw.simd.push(feat.to_string());
                    }
                }
            }
            "processor" => *hw.logical_cores.get_or_insert(0) += 1,
            "physical id" => phys_id = v.to_string(),
            "core id" => {
                cores.insert((phys_id.clone(), v.to_string()));
            }
            _ => {}
        }
    }
    hw.physical_cores = (!cores.is_empty()).then_some(cores.len());
}

/// L3 size in bytes from sysfs (`cpu0/cache/index3/size`).
fn l3_bytes() -> Option<u64> {
    let size = fs::read_to_string("/sys/devices/system/cpu/cpu0/cache/index3/size").ok()?;
    parse_size(size.trim())
}

/// Parse a sysfs/lscpu cache size string: `3072K`, `32M`, `1G`, `512K`,
/// `3 MiB`, or a bare byte count. Case-insensitive; unknown units are an
/// error (a missing/unparseable file reports `None` upstream).
pub(crate) fn parse_size(s: &str) -> Option<u64> {
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
        "T" | "TB" | "TIB" => 1 << 40,
        _ => return None,
    };
    Some(num * mult)
}

/// Count NUMA nodes from `/sys/devices/system/node/node<digits>` entries.
fn numa_nodes() -> Option<usize> {
    let entries = fs::read_dir("/sys/devices/system/node").ok()?;
    let n = entries
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            let rest = name.strip_prefix("node").unwrap_or("");
            !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
        })
        .count();
    (n > 0).then_some(n)
}

/// Parse `/proc/meminfo` hugepage counters.
fn parse_meminfo(text: &str, hw: &mut Hardware) {
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let n = v
            .split_whitespace()
            .next()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        match k.trim() {
            "HugePages_Total" => hw.hugepages_total = Some(n),
            "HugePages_Free" => hw.hugepages_free = Some(n),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CPUINFO_SMT: &str = "\
processor       : 0
vendor_id       : GenuineIntel
cpu family      : 6
model           : 61
model name      : Intel(R) Core(TM) i5-5200U CPU @ 2.20GHz
physical id     : 0
core id         : 0
flags           : fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush dts acpi mmx fxsr sse sse2 ss ht tm pbe syscall nx pdpe1gb rdtscp lm constant_tsc arch_perfmon pebs bts rep_good nopl xtopology nonstop_tsc aperfmperf eagerfpu pni pclmulqdq dtes64 monitor ds_cpl vmx est tm2 ssse3 fma cx16 xtpr pdcm pcid sse4_1 sse4_2 movbe popcnt tsc_deadline_timer aes xsave avx f16c rdrand lahf_lm abm 3dnowprefetch epb intel_pt tpr_shadow vnmi flexpriority ept vpid fsgsbase tsc_adjust bmi1 avx2 smep bmi2 erms invpcid avx512f avx512dq avx512bw avx512vl
processor       : 1
vendor_id       : GenuineIntel
model name      : Intel(R) Core(TM) i5-5200U CPU @ 2.20GHz
physical id     : 0
core id         : 0
processor       : 2
vendor_id       : GenuineIntel
model name      : Intel(R) Core(TM) i5-5200U CPU @ 2.20GHz
physical id     : 0
core id         : 1
processor       : 3
vendor_id       : GenuineIntel
model name      : Intel(R) Core(TM) i5-5200U CPU @ 2.20GHz
physical id     : 0
core id         : 1
";

    #[test]
    fn cpuinfo_vendor_model_cores_and_simd() {
        let mut hw = Hardware::default();
        parse_cpuinfo(CPUINFO_SMT, &mut hw);
        assert_eq!(hw.vendor, "GenuineIntel");
        assert!(hw.model.contains("i5-5200U"));
        assert_eq!(hw.logical_cores, Some(4));
        assert_eq!(hw.physical_cores, Some(2));
        assert_eq!(hw.threads_per_core(), Some(2));
        for feat in ["avx2", "sse4.2", "fma", "avx512f", "movbe", "bmi2"] {
            assert!(hw.simd.iter().any(|s| s == feat), "missing {feat}");
        }
        assert!(!hw.simd.iter().any(|s| s == "ssse3_x"));
        hw.simd.sort();
        hw.simd.dedup();
        let expected = [
            "aes", "avx2", "avx512bw", "avx512dq", "avx512f", "avx512vl", "bmi1", "bmi2",
            "f16c", "fma", "movbe", "pclmulqdq", "popcnt", "sse4.2", "ssse3",
        ];
        assert_eq!(hw.simd, expected, "unexpected simd set");
    }

    #[test]
    fn cpuinfo_no_topology_yields_unknown_physical() {
        let text = "processor       : 0\nmodel name      : ARMv8\n";
        let mut hw = Hardware::default();
        parse_cpuinfo(text, &mut hw);
        assert_eq!(hw.logical_cores, Some(1));
        assert_eq!(hw.physical_cores, None);
        assert!(hw.simd.is_empty());
    }

    #[test]
    fn size_parsing() {
        assert_eq!(parse_size("3072K"), Some(3 * 1024 * 1024));
        assert_eq!(parse_size("32M"), Some(32 << 20));
        assert_eq!(parse_size("1G"), Some(1 << 30));
        assert_eq!(parse_size("3 MiB"), Some(3 << 20));
        assert_eq!(parse_size("512K"), Some(512 << 10));
        assert_eq!(parse_size("4096"), Some(4096));
        assert_eq!(parse_size(""), None);
        assert_eq!(parse_size("lots"), None);
    }

    #[test]
    fn meminfo_hugepages() {
        let text = "\
MemTotal:       16294944 kB
HugePages_Total:       0
HugePages_Free:        0
";
        let mut hw = Hardware::default();
        parse_meminfo(text, &mut hw);
        assert_eq!(hw.hugepages_total, Some(0));
        assert!(!hw.hugepages_available());

        let text = "HugePages_Total:     256\nHugePages_Free:      128\n";
        let mut hw = Hardware::default();
        parse_meminfo(text, &mut hw);
        assert_eq!(hw.hugepages_total, Some(256));
        assert_eq!(hw.hugepages_free, Some(128));
        assert!(hw.hugepages_available());
    }
}
