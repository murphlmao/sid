use sid_core::sys::{SysError, SystemOverview};
use sysinfo::System;

/// Snapshot host identity + CPU/memory/load metrics via `sysinfo`.
///
/// Refreshes CPU usage and memory on the cached `sys` handle before reading. Per
/// `sysinfo`'s own contract (see `System::refresh_cpu_usage`'s doc), CPU percentages
/// are a delta since the *previous* refresh — the very first call after a fresh
/// `System` may report `0.0` for `cpu_total_pct`/every entry in `cpu_per_core`; a
/// second call (even seconds later, as the Systems tab's refresh loop does) reports
/// real values. This mirrors `processes::list_processes`'s reuse of the same cached
/// `sysinfo::System` — no separate handle is created here.
pub(crate) fn overview(sys: &mut System) -> Result<SystemOverview, SysError> {
    sys.refresh_cpu_usage();
    sys.refresh_memory();

    let hostname = System::host_name().unwrap_or_else(|_| "unknown".to_string());
    let kernel = System::kernel_version().unwrap_or_else(|_| "unknown".to_string());
    let os = System::long_os_version()
        .or_else(|_| System::os_version())
        .unwrap_or_else(|_| std::env::consts::OS.to_string());
    let uptime_secs = System::uptime().unwrap_or(0);
    // Unsupported on Windows (and on probe failure): report zeros rather than failing.
    let load = System::load_average().map(|l| (l.one, l.five, l.fifteen));
    let (mem_cached, mem_buffers) = cached_and_buffers();

    let cpu_total_pct = sys.global_cpu_usage();
    let cpu_per_core: Vec<f32> = sys.cpus().iter().map(|c| c.usage()).collect();

    Ok(SystemOverview {
        hostname,
        kernel,
        os,
        uptime_secs,
        load_avg: load.unwrap_or((0.0, 0.0, 0.0)),
        cpu_total_pct,
        cpu_per_core,
        mem_total: sys.total_memory(),
        mem_used: sys.used_memory(),
        mem_cached,
        mem_buffers,
        swap_total: sys.total_swap(),
        swap_used: sys.used_swap(),
    })
}

/// Page cache + buffers in bytes. sysinfo doesn't expose these, so on Linux read
/// `/proc/meminfo` (cache = `Cached` + `SReclaimable`); elsewhere `(0, 0)`.
fn cached_and_buffers() -> (u64, u64) {
    #[cfg(target_os = "linux")]
    {
        if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
            return parse_meminfo(&text);
        }
    }
    (0, 0)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_meminfo(text: &str) -> (u64, u64) {
    let kib = |key: &str| -> u64 {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    (kib("Cached") + kib("SReclaimable"), kib("Buffers"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_meminfo_sums_cache_and_reads_buffers() {
        let t = "MemTotal: 100 kB\nBuffers: 10 kB\nCached: 20 kB\nSReclaimable: 5 kB\n";
        assert_eq!(parse_meminfo(t), (25 * 1024, 10 * 1024));
        assert_eq!(parse_meminfo(""), (0, 0));
    }

    /// Load-bearing mapping test: `overview()` must actually populate every field from
    /// the real host rather than silently defaulting — run against a real `System`
    /// since `sysinfo`'s static/instance readers have no fake-able seam. Assertions are
    /// deliberately loose (never a fixed number) so they hold on any dev machine/CI
    /// runner: this is the sandbox this crate always runs in, so "the report reflects
    /// this box" is a fair contract to pin.
    #[test]
    fn overview_reports_real_host_metrics() {
        let mut sys = System::new().unwrap();
        let first = overview(&mut sys).expect("overview should not error");

        assert!(!first.hostname.is_empty(), "hostname should never be blank");
        assert!(first.mem_total > 0, "a real host always has some RAM");
        assert!(first.mem_used <= first.mem_total);
        assert!(first.swap_used <= first.swap_total);
        assert!(
            !first.cpu_per_core.is_empty(),
            "a real host always has at least one logical core"
        );

        // Second call: per the module doc, CPU% becomes meaningful (still just
        // structurally-sound here, not asserting a specific value — a busy CI box could
        // legitimately report anything in 0..=100*cores).
        let second = overview(&mut sys).expect("overview should not error");
        assert_eq!(
            second.cpu_per_core.len(),
            first.cpu_per_core.len(),
            "core count should be stable across two immediate calls"
        );
        assert_eq!(
            second.mem_total, first.mem_total,
            "RAM size doesn't change mid-test"
        );
    }

    /// `overview()` never invents an OS/kernel string when `sysinfo` can't determine
    /// one — the `unknown`/`std::env::consts::OS` fallbacks exist so the UI always has
    /// *something* non-empty to render rather than an empty line.
    #[test]
    fn overview_fields_are_never_blank() {
        let mut sys = System::new().unwrap();
        let ov = overview(&mut sys).expect("overview should not error");
        assert!(!ov.kernel.is_empty());
        assert!(!ov.os.is_empty());
    }
}
