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

    let hostname = System::host_name().unwrap_or_else(|| "unknown".to_string());
    let kernel = System::kernel_version().unwrap_or_else(|| "unknown".to_string());
    let os = System::long_os_version()
        .or_else(System::os_version)
        .unwrap_or_else(|| std::env::consts::OS.to_string());
    let uptime_secs = System::uptime();
    let load = System::load_average();

    let cpu_total_pct = sys.global_cpu_usage();
    let cpu_per_core: Vec<f32> = sys.cpus().iter().map(|c| c.cpu_usage()).collect();

    Ok(SystemOverview {
        hostname,
        kernel,
        os,
        uptime_secs,
        load_avg: (load.one, load.five, load.fifteen),
        cpu_total_pct,
        cpu_per_core,
        mem_total: sys.total_memory(),
        mem_used: sys.used_memory(),
        swap_total: sys.total_swap(),
        swap_used: sys.used_swap(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Load-bearing mapping test: `overview()` must actually populate every field from
    /// the real host rather than silently defaulting — run against a real `System`
    /// since `sysinfo`'s static/instance readers have no fake-able seam. Assertions are
    /// deliberately loose (never a fixed number) so they hold on any dev machine/CI
    /// runner: this is the sandbox this crate always runs in, so "the report reflects
    /// this box" is a fair contract to pin.
    #[test]
    fn overview_reports_real_host_metrics() {
        let mut sys = System::new();
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
        let mut sys = System::new();
        let ov = overview(&mut sys).expect("overview should not error");
        assert!(!ov.kernel.is_empty());
        assert!(!ov.os.is_empty());
    }
}
