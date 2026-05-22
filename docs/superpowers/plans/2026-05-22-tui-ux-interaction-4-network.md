# Branch #4 — Network tab: drill-in + WAN-first sort + filter affordance

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Network tab feel useful: sort interfaces by primary-WAN-first (default-route holder), let Enter on an interface open a read-only detail modal, and surface the existing `/` filter via a footer hint so users discover it.

**Architecture:** Three coordinated changes. (1) `SysProvider` grows a `default_route_iface_name` method with a `Ok(None)` default impl so existing impls compile; `SysinfoProvider` overrides it to read `/proc/net/route` on Linux and shell out to `route` on macOS. (2) `SysSnapshot` carries the default-route name; `InterfacesSidebarState::set_data` sorts by a computed score using that name. (3) `NetworkWidget::handle_event` adds an `Enter` arm that opens an `InterfaceDetailModal` via the substrate from branch #1; `footer_hint()` advertises `/`, `s`, `K`, `Enter`. Editing is stubbed — pressing `E` toasts "Interface editing not yet supported".

**Tech Stack:** Rust 2024 edition, ratatui, crossterm, the modal substrate from branch #1, the existing `FilterInputState`, sysinfo + std::fs for `/proc/net/route` parsing, `Command::new("route")` for macOS fallback.

**Branch:** `feat/network-drill-in-and-sort`

**Depends on:** Branch #1 merged (modal substrate arrow keys, `Field::Display` rows already work pre-branch).

**Spec reference:** [`docs/superpowers/specs/2026-05-22-tui-ux-interaction-design.md`](../specs/2026-05-22-tui-ux-interaction-design.md) §§ 5.6, 6.

---

## File map

| File | Purpose | Action |
|---|---|---|
| `crates/sid-core/src/adapters/sys.rs` | new trait method `default_route_iface_name` with default impl | Modify (lines 265-283) |
| `crates/sid-core/src/sys_probe.rs` | propagate the value into `SysSnapshot` | Modify |
| `crates/sid-core/tests/sys_provider_contract.rs` | adversarial tests on the new method | Modify |
| `crates/sid-sysinfo/src/lib.rs` | override `default_route_iface_name` on `SysinfoProvider` | Modify |
| `crates/sid-sysinfo/src/default_route.rs` | NEW — `/proc/net/route` reader + macOS `route` shell-out | Create |
| `crates/sid-sysinfo/tests/default_route.rs` | unit + adversarial tests | Create |
| `crates/sid-widgets/src/network/interfaces_sidebar.rs` | sort by score in `set_data` | Modify |
| `crates/sid-widgets/src/network.rs` | Enter opens detail modal; footer hint update; E-toast | Modify |
| `crates/sid-widgets/tests/interfaces_sidebar.rs` | sort tests | Modify |
| `crates/sid-widgets/tests/network.rs` | Enter-opens-modal + footer-hint tests | Modify |
| `crates/sid-widgets/benches/interface_sort.rs` | criterion bench | Create |
| `crates/sid-widgets/Cargo.toml` | bench entry | Modify |

---

## Task 1 — `SysProvider::default_route_iface_name` with default impl

**Files:**
- Modify: `crates/sid-core/src/adapters/sys.rs:265-283`
- Modify: `crates/sid-core/tests/sys_provider_contract.rs`

`★ Insight ─────────────────────────────────────`
Returning `Result<Option<String>, SysError>` is the right shape: `Err` means the underlying probe failed (e.g., `/proc/net/route` missing on a stripped container), `Ok(None)` means probe succeeded but found no default route, `Ok(Some(name))` is the happy path. The sort callsite collapses Err+None to "no WAN to prioritize", which lets the renderer fall back to alphabetical without special-casing.
`─────────────────────────────────────────────────`

- [ ] **Step 1.1: Add failing test for the trait default**

Append to `crates/sid-core/tests/sys_provider_contract.rs`:

```rust
use sid_core::adapters::sys::{ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider};

struct Empty;
impl SysProvider for Empty {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> { Ok(vec![]) }
    fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> { Ok(vec![]) }
    fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> { Ok(vec![]) }
    fn kill_process(&mut self, _: Pid, _: Signal) -> Result<(), SysError> { Ok(()) }
}

#[test]
fn default_route_iface_name_default_impl_returns_ok_none() {
    let mut e = Empty;
    let v = e.default_route_iface_name().expect("default impl must return Ok");
    assert!(v.is_none());
}
```

- [ ] **Step 1.2: Run test, verify failure**

```bash
cargo test -p sid-core --test sys_provider_contract default_route_iface_name
```

Expected: FAIL — method not on trait yet.

- [ ] **Step 1.3: Add the method to `SysProvider` with a default impl**

In `crates/sid-core/src/adapters/sys.rs:265-283`, append to the trait body:

```rust
    /// Return the name of the network interface holding the default route,
    /// if one exists. Used by widgets to sort interfaces with the primary
    /// WAN first.
    ///
    /// The default implementation returns `Ok(None)` so existing impls
    /// compile unchanged. Concrete impls override this for their platform.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::sys::{ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider};
    ///
    /// struct Empty;
    /// impl SysProvider for Empty {
    ///     fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> { Ok(vec![]) }
    ///     fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> { Ok(vec![]) }
    ///     fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> { Ok(vec![]) }
    ///     fn kill_process(&mut self, _: Pid, _: Signal) -> Result<(), SysError> { Ok(()) }
    /// }
    /// let mut e = Empty;
    /// assert!(e.default_route_iface_name().unwrap().is_none());
    /// ```
    fn default_route_iface_name(&mut self) -> Result<Option<String>, SysError> {
        Ok(None)
    }
```

- [ ] **Step 1.4: Run test, verify PASS**

```bash
cargo test -p sid-core --test sys_provider_contract default_route_iface_name
```

Expected: PASS.

- [ ] **Step 1.5: Commit Task 1**

```bash
git add crates/sid-core/src/adapters/sys.rs crates/sid-core/tests/sys_provider_contract.rs
git commit -m "feat(sid-core): SysProvider::default_route_iface_name (defaulted)

Adds the trait method used by the Network tab to sort the primary WAN
interface first. Default impl returns Ok(None) so existing impls
compile unchanged — only SysinfoProvider needs to override (Task 2).

Result<Option<String>, SysError> is the chosen shape: Err means the
probe failed, Ok(None) means no default route, Ok(Some(name)) is the
happy path. Sort callsite collapses Err+None to 'no WAN', falling
back to alphabetical."
```

---

## Task 2 — `SysinfoProvider::default_route_iface_name` implementation

**Files:**
- Create: `crates/sid-sysinfo/src/default_route.rs`
- Modify: `crates/sid-sysinfo/src/lib.rs`
- Create: `crates/sid-sysinfo/tests/default_route.rs`

`★ Insight ─────────────────────────────────────`
`/proc/net/route` is a tab-separated text file. The default route's destination is `00000000` (all zeros). Parsing it without pulling a new crate is a few lines. On macOS we shell out to `route -n get default` and grep the `interface:` line — that's the canonical command pretty much every Unix admin already uses. Both paths are gated by `#[cfg(target_os = ...)]` so we don't carry dead code on the wrong platform.
`─────────────────────────────────────────────────`

- [ ] **Step 2.1: Create the platform-specific module**

Create `crates/sid-sysinfo/src/default_route.rs`:

```rust
//! Platform-specific default-route lookup. Returns the name of the network
//! interface that holds the default route (where outbound packets without a
//! more-specific destination go), or `None` if no default route is set.

use sid_core::adapters::sys::SysError;

/// Linux: parse `/proc/net/route` for a row with `Destination == 00000000`
/// and return the value in the first column (the interface name).
///
/// Format (tab-separated, one header line):
///
/// ```text
/// Iface	Destination	Gateway 	Flags	RefCnt	Use	Metric	Mask		MTU	Window	IRTT
/// wlan0	00000000	0102A8C0	0003	0	0	600	00000000	0	0	0
/// ```
#[cfg(target_os = "linux")]
pub fn read_default_route_iface() -> Result<Option<String>, SysError> {
    let bytes = match std::fs::read_to_string("/proc/net/route") {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SysError::Other(format!("/proc/net/route: {e}"))),
    };
    for line in bytes.lines().skip(1) {
        let mut cols = line.split('\t').filter(|s| !s.is_empty());
        let Some(iface) = cols.next() else {
            continue;
        };
        let Some(dest) = cols.next() else {
            continue;
        };
        if dest == "00000000" {
            return Ok(Some(iface.to_string()));
        }
    }
    Ok(None)
}

/// macOS: shell out to `route -n get default` and parse the `interface:` line.
#[cfg(target_os = "macos")]
pub fn read_default_route_iface() -> Result<Option<String>, SysError> {
    use std::process::Command;
    let out = Command::new("route")
        .args(["-n", "get", "default"])
        .output()
        .map_err(|e| SysError::Other(format!("spawn route: {e}")))?;
    if !out.status.success() {
        // Non-zero often just means "no default route" — return Ok(None).
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("interface:") {
            let name = rest.trim().to_string();
            if name.is_empty() {
                return Ok(None);
            }
            return Ok(Some(name));
        }
    }
    Ok(None)
}

/// Other platforms: not supported, return `Ok(None)`.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn read_default_route_iface() -> Result<Option<String>, SysError> {
    Ok(None)
}

/// Linux: parse the body string directly. Pulled out for testability.
///
/// # Examples
///
/// ```
/// use sid_sysinfo::default_route::parse_proc_net_route;
/// let body = "Iface\tDestination\tGateway\nlo\t00000000\t00000000\nwlan0\t00000000\t0102A8C0\n";
/// // First default-route row wins.
/// assert_eq!(parse_proc_net_route(body), Some("lo".to_string()));
/// ```
#[cfg(target_os = "linux")]
pub fn parse_proc_net_route(body: &str) -> Option<String> {
    for line in body.lines().skip(1) {
        let mut cols = line.split('\t').filter(|s| !s.is_empty());
        let iface = cols.next()?;
        let dest = cols.next()?;
        if dest == "00000000" {
            return Some(iface.to_string());
        }
    }
    None
}
```

- [ ] **Step 2.2: Wire the module into `crates/sid-sysinfo/src/lib.rs`**

Add at the top of the file (or near other `pub mod` declarations):

```rust
pub mod default_route;
```

In the `impl SysProvider for SysinfoProvider` block, override the method:

```rust
    fn default_route_iface_name(&mut self) -> Result<Option<String>, SysError> {
        default_route::read_default_route_iface()
    }
```

- [ ] **Step 2.3: Add unit tests**

Create `crates/sid-sysinfo/tests/default_route.rs`:

```rust
#[cfg(target_os = "linux")]
mod linux {
    use sid_sysinfo::default_route::parse_proc_net_route;

    #[test]
    fn parses_canonical_default_route() {
        let body = "Iface\tDestination\tGateway\twlan0\t00000000\t0102A8C0\n";
        // Single-line body with a header still has no actual data row;
        // the parser skips the header, finds nothing.
        assert_eq!(parse_proc_net_route(body), None);
    }

    #[test]
    fn parses_two_line_with_one_default_route() {
        let body = "Iface\tDestination\tGateway\nwlan0\t00000000\t0102A8C0\n";
        assert_eq!(parse_proc_net_route(body), Some("wlan0".to_string()));
    }

    #[test]
    fn first_default_route_wins_when_multiple() {
        let body = "Iface\tDestination\tGateway\nwlan0\t00000000\t1\neth0\t00000000\t2\n";
        assert_eq!(parse_proc_net_route(body), Some("wlan0".to_string()));
    }

    #[test]
    fn empty_body_returns_none() {
        assert_eq!(parse_proc_net_route(""), None);
    }

    #[test]
    fn header_only_returns_none() {
        assert_eq!(parse_proc_net_route("Iface\tDestination\tGateway\n"), None);
    }

    #[test]
    fn non_default_routes_are_ignored() {
        let body = "Iface\tDestination\tGateway\ndocker0\tABCDEF01\t0\n";
        assert_eq!(parse_proc_net_route(body), None);
    }

    #[test]
    fn malformed_row_does_not_panic() {
        let body = "Iface\tDestination\nwlan0\n00000000\n";
        let _ = parse_proc_net_route(body);
    }
}

// Live test: hits /proc/net/route on Linux. Marked ignored so CI/macOS
// don't fail; run with `cargo test -p sid-sysinfo -- --ignored` to
// exercise on a real Linux host.
#[cfg(target_os = "linux")]
#[test]
#[ignore]
fn live_proc_net_route_does_not_panic() {
    let r = sid_sysinfo::default_route::read_default_route_iface();
    // Just confirm it returns SOME Result without panicking.
    let _ = r;
}
```

- [ ] **Step 2.4: Run tests, verify PASS**

```bash
cargo test -p sid-sysinfo --test default_route
cargo test -p sid-sysinfo --doc default_route
```

Expected: PASS on Linux. On macOS, the `linux` mod is gated out via `cfg`, so the tests are empty — `cargo test` still passes.

- [ ] **Step 2.5: Commit Task 2**

```bash
git add crates/sid-sysinfo/src/default_route.rs crates/sid-sysinfo/src/lib.rs crates/sid-sysinfo/tests/default_route.rs
git commit -m "feat(sid-sysinfo): SysinfoProvider::default_route_iface_name impl

Linux reads /proc/net/route (first row with Destination == 00000000).
macOS shells out to 'route -n get default' and parses the
'interface:' line. Other platforms return Ok(None).

Adversarial tests cover: empty body, header-only, malformed rows,
non-default-route entries, and multiple default routes (first wins).
Live test marked #[ignore] to keep CI deterministic; run with
'cargo test -- --ignored' to exercise on a real host."
```

---

## Task 3 — Propagate `default_route` into `SysSnapshot`

**Files:**
- Modify: `crates/sid-core/src/sys_probe.rs`
- Modify: `crates/sid-core/tests/sys_probe_run.rs`

`★ Insight ─────────────────────────────────────`
The snapshot is what the widget sees on every probe tick. Adding the field to the snapshot (rather than re-querying the provider from the widget) keeps the widget pure: it renders the snapshot, no I/O at render time.
`─────────────────────────────────────────────────`

- [ ] **Step 3.1: Add `default_route_iface` to `SysSnapshot`**

Find the `SysSnapshot` struct in `crates/sid-core/src/sys_probe.rs`:

```bash
grep -n "pub struct SysSnapshot\|pub interfaces:\|pub processes:" crates/sid-core/src/sys_probe.rs
```

Add a field after `pub interfaces`:

```rust
    /// Name of the interface holding the default route at probe time, if any.
    /// Used by the Network tab to sort the primary WAN first.
    pub default_route_iface: Option<String>,
```

- [ ] **Step 3.2: Populate the field in `probe`**

Find the function that produces a `SysSnapshot`:

```bash
grep -n "fn probe\|SysSnapshot {" crates/sid-core/src/sys_probe.rs
```

After the existing `let interfaces = ...` line, add:

```rust
    let default_route_iface = guard
        .default_route_iface_name()
        .unwrap_or_else(|e| {
            tracing::debug!("default_route_iface_name failed: {e}");
            None
        });
```

And include it in the `SysSnapshot { ... }` literal.

- [ ] **Step 3.3: Update tests using `SysSnapshot { ... }` literals**

Search for callers and add `default_route_iface: None`:

```bash
grep -rn "SysSnapshot {" crates/sid-core --include="*.rs"
grep -rn "SysSnapshot {" crates/sid-widgets --include="*.rs"
```

Each `SysSnapshot { ... }` literal gets `default_route_iface: None,` appended. There should be 3-5 occurrences across tests.

- [ ] **Step 3.4: Compile and run all tests**

```bash
cargo test -p sid-core
cargo test -p sid-widgets
```

Expected: PASS. If any test fails because `SysSnapshot` literal is missing the new field, add `default_route_iface: None` and re-run.

- [ ] **Step 3.5: Commit Task 3**

```bash
git add crates/sid-core/src/sys_probe.rs crates/sid-core/tests crates/sid-widgets/tests
git commit -m "feat(sid-core): SysSnapshot.default_route_iface — probe-time default-route name

Carries the value from SysProvider::default_route_iface_name into the
snapshot so widgets can sort without doing I/O at render time. None
when the probe returns Err (logged at debug) or Ok(None).

Updates all SysSnapshot { ... } literals in tests to include the new
field (default None)."
```

---

## Task 4 — `InterfacesSidebarState::set_data` sorts by score

**Files:**
- Modify: `crates/sid-widgets/src/network/interfaces_sidebar.rs:38-47`
- Test: `crates/sid-widgets/tests/interfaces_sidebar.rs`

- [ ] **Step 4.1: Add failing tests**

Append to `crates/sid-widgets/tests/interfaces_sidebar.rs`:

```rust
use sid_core::adapters::sys::NetInterface;
use sid_widgets::network::interfaces_sidebar::InterfacesSidebarState;

fn iface(name: &str, is_up: bool) -> NetInterface {
    NetInterface {
        name: name.into(),
        is_up,
        // Other fields default; check the actual struct shape and fill in.
        ..Default::default()
    }
}

#[test]
fn wan_iface_sorts_first_when_default_route_set() {
    let mut s = InterfacesSidebarState::new();
    let ifaces = vec![
        iface("lo", true),
        iface("docker0", true),
        iface("wlan0", true),
        iface("eth0", false),
    ];
    s.set_data_with_default_route(ifaces, Some("wlan0"));
    let order: Vec<&str> = s.rows().iter().map(|i| i.name.as_str()).collect();
    assert_eq!(order[0], "wlan0", "wlan0 must sort first; got {order:?}");
}

#[test]
fn loopback_and_docker_sort_last() {
    let mut s = InterfacesSidebarState::new();
    let ifaces = vec![
        iface("lo", true),
        iface("docker0", true),
        iface("wlan0", true),
    ];
    s.set_data_with_default_route(ifaces, Some("wlan0"));
    let order: Vec<&str> = s.rows().iter().map(|i| i.name.as_str()).collect();
    // wlan0 first; lo and docker0 in the bottom group, alphabetical.
    assert_eq!(order[0], "wlan0");
    assert_eq!(order[order.len() - 1], "lo");
}

#[test]
fn down_interfaces_sort_below_up_when_no_default_route() {
    let mut s = InterfacesSidebarState::new();
    let ifaces = vec![
        iface("eth0", false),
        iface("wlan0", true),
    ];
    s.set_data_with_default_route(ifaces, None);
    let order: Vec<&str> = s.rows().iter().map(|i| i.name.as_str()).collect();
    assert_eq!(order, vec!["wlan0", "eth0"], "up interface must sort above down");
}

#[test]
fn alphabetical_tiebreak_within_score_bucket() {
    let mut s = InterfacesSidebarState::new();
    let ifaces = vec![
        iface("wlan1", true),
        iface("wlan0", true),
    ];
    s.set_data_with_default_route(ifaces, None);
    let order: Vec<&str> = s.rows().iter().map(|i| i.name.as_str()).collect();
    assert_eq!(order, vec!["wlan0", "wlan1"]);
}

#[test]
fn no_default_route_falls_back_to_score_only() {
    let mut s = InterfacesSidebarState::new();
    let ifaces = vec![
        iface("lo", true),
        iface("eth0", true),
    ];
    s.set_data_with_default_route(ifaces, None);
    let order: Vec<&str> = s.rows().iter().map(|i| i.name.as_str()).collect();
    // eth0 sorts above lo (physical above virtual).
    assert_eq!(order, vec!["eth0", "lo"]);
}

#[test]
fn sort_is_stable_across_repeated_set_data() {
    let mut s = InterfacesSidebarState::new();
    let ifaces = vec![
        iface("eth0", true),
        iface("eth1", true),
        iface("wlan0", true),
    ];
    s.set_data_with_default_route(ifaces.clone(), Some("wlan0"));
    let first: Vec<String> = s.rows().iter().map(|i| i.name.clone()).collect();
    s.set_data_with_default_route(ifaces, Some("wlan0"));
    let second: Vec<String> = s.rows().iter().map(|i| i.name.clone()).collect();
    assert_eq!(first, second);
}
```

- [ ] **Step 4.2: Run, verify failure**

```bash
cargo test -p sid-widgets --test interfaces_sidebar wan_iface_sorts loopback_and_docker down_interfaces alphabetical_tiebreak no_default_route sort_is_stable
```

Expected: FAIL — `set_data_with_default_route` does not exist.

- [ ] **Step 4.3: Add `set_data_with_default_route` and the scoring helper**

In `crates/sid-widgets/src/network/interfaces_sidebar.rs`, add:

```rust
impl InterfacesSidebarState {
    /// Like `set_data`, but also takes the default-route interface name
    /// (if known) so the WAN sorts first. Existing `set_data` is kept as
    /// an alias that calls this with `None`.
    pub fn set_data_with_default_route(
        &mut self,
        mut data: Vec<NetInterface>,
        default_route: Option<&str>,
    ) {
        sort_interfaces(&mut data, default_route);
        let prev_name = self.data.get(self.selected).map(|i| i.name.clone());
        self.data = data;
        self.selected = prev_name
            .and_then(|n| self.data.iter().position(|i| i.name == n))
            .unwrap_or(0);
        if self.selected >= self.data.len() {
            self.selected = 0;
        }
    }
}

/// Lower scores sort first. Score formula:
///
/// - `+100` if not the default-route interface
/// - `+ 10` if `!is_up`
/// - `+  5` if name matches a virtual-interface prefix
///   (`lo`, `docker`, `br-`, `veth`, `tun`, `tap`, `virbr`, `vmnet`)
/// - alphabetical tiebreak by name within the same score bucket
///
/// # Examples
///
/// ```
/// use sid_core::adapters::sys::NetInterface;
/// use sid_widgets::network::interfaces_sidebar::iface_sort_score;
/// let mut wlan = NetInterface::default();
/// wlan.name = "wlan0".into();
/// wlan.is_up = true;
/// assert_eq!(iface_sort_score(&wlan, Some("wlan0")), 0);
/// ```
pub fn iface_sort_score(iface: &NetInterface, default_route: Option<&str>) -> u32 {
    let mut score = 0u32;
    if default_route != Some(iface.name.as_str()) {
        score += 100;
    }
    if !iface.is_up {
        score += 10;
    }
    if is_virtual_iface_name(&iface.name) {
        score += 5;
    }
    score
}

/// Prefix-match the well-known virtual-interface names.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::interfaces_sidebar::is_virtual_iface_name;
/// assert!(is_virtual_iface_name("lo"));
/// assert!(is_virtual_iface_name("docker0"));
/// assert!(is_virtual_iface_name("br-abc123"));
/// assert!(is_virtual_iface_name("veth1234"));
/// assert!(is_virtual_iface_name("tun0"));
/// assert!(is_virtual_iface_name("tap0"));
/// assert!(is_virtual_iface_name("virbr0"));
/// assert!(is_virtual_iface_name("vmnet8"));
/// assert!(!is_virtual_iface_name("eth0"));
/// assert!(!is_virtual_iface_name("wlan0"));
/// ```
pub fn is_virtual_iface_name(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "lo", "docker", "br-", "veth", "tun", "tap", "virbr", "vmnet",
    ];
    PREFIXES.iter().any(|p| name.starts_with(p))
}

/// In-place sort by `(score, name)`. Pulled out so the bench can call it
/// without setting up the full sidebar state.
pub fn sort_interfaces(data: &mut [NetInterface], default_route: Option<&str>) {
    data.sort_by(|a, b| {
        let sa = iface_sort_score(a, default_route);
        let sb = iface_sort_score(b, default_route);
        sa.cmp(&sb).then_with(|| a.name.cmp(&b.name))
    });
}
```

- [ ] **Step 4.4: Update the existing `set_data` to call the new path**

In the existing `set_data` body, route through the new method:

```rust
    /// Replace the displayed interfaces (without default-route info).
    /// Equivalent to `set_data_with_default_route(data, None)`.
    pub fn set_data(&mut self, data: Vec<NetInterface>) {
        self.set_data_with_default_route(data, None);
    }
```

- [ ] **Step 4.5: Run, verify PASS**

```bash
cargo test -p sid-widgets --test interfaces_sidebar
cargo test -p sid-widgets --doc network::interfaces_sidebar
```

Expected: PASS.

- [ ] **Step 4.6: Wire `NetworkWidget::apply_snapshot` to pass the default route**

Find `apply_snapshot` in `crates/sid-widgets/src/network.rs`:

```bash
grep -n "fn apply_snapshot" crates/sid-widgets/src/network.rs
```

Update to call `set_data_with_default_route`:

```rust
    pub fn apply_snapshot(&mut self, snap: sid_core::sys_probe::SysSnapshot) {
        self.ports.set_data(snap.listening_ports);
        self.procs.set_data(snap.processes);
        self.ifs.set_data_with_default_route(
            snap.interfaces,
            snap.default_route_iface.as_deref(),
        );
    }
```

- [ ] **Step 4.7: Run, verify PASS**

```bash
cargo test -p sid-widgets
```

Expected: PASS.

- [ ] **Step 4.8: Property test — sort is stable for arbitrary input**

Append to `crates/sid-widgets/tests/interfaces_sidebar.rs`:

```rust
use proptest::prelude::*;

fn arbitrary_iface_name() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("wlan0".to_string()),
        Just("wlan1".to_string()),
        Just("eth0".to_string()),
        Just("eth1".to_string()),
        Just("lo".to_string()),
        Just("docker0".to_string()),
        Just("veth_abc".to_string()),
        Just("tun0".to_string()),
        Just("br-xyz".to_string()),
    ]
}

proptest! {
    #[test]
    fn sort_is_stable_for_arbitrary_inputs(
        names in prop::collection::vec(arbitrary_iface_name(), 0..15),
        default_route_idx in proptest::option::of(0usize..15),
    ) {
        let ifaces: Vec<NetInterface> = names
            .iter()
            .enumerate()
            .map(|(_, n)| iface(n, true))
            .collect();
        let dr = default_route_idx.and_then(|i| names.get(i)).cloned();
        let mut s1 = InterfacesSidebarState::new();
        s1.set_data_with_default_route(ifaces.clone(), dr.as_deref());
        let mut s2 = InterfacesSidebarState::new();
        s2.set_data_with_default_route(ifaces, dr.as_deref());
        let n1: Vec<&str> = s1.rows().iter().map(|i| i.name.as_str()).collect();
        let n2: Vec<&str> = s2.rows().iter().map(|i| i.name.as_str()).collect();
        prop_assert_eq!(n1, n2);
    }
}
```

- [ ] **Step 4.9: Run property test**

```bash
cargo test -p sid-widgets --test interfaces_sidebar sort_is_stable_for_arbitrary
```

Expected: PASS.

- [ ] **Step 4.10: Commit Task 4**

```bash
git add crates/sid-widgets/src/network/interfaces_sidebar.rs crates/sid-widgets/src/network.rs crates/sid-widgets/tests/interfaces_sidebar.rs
git commit -m "feat(sid-widgets): network interfaces sort — primary WAN first, virtual last

set_data_with_default_route sorts by an additive integer score:
+100 if not the default-route interface, +10 if down, +5 if virtual
prefix (lo / docker / br- / veth / tun / tap / virbr / vmnet),
alphabetical tiebreak. The result: wlan0 (default route) first,
eth0/eth1 next, lo / docker0 / tun* at the bottom.

apply_snapshot now passes snap.default_route_iface into the sidebar.

Property test confirms determinism across repeated set_data calls
with the same input."
```

---

## Task 5 — Enter opens `InterfaceDetailModal`; `E` toasts "coming soon"

**Files:**
- Modify: `crates/sid-widgets/src/network.rs`
- Modify: `crates/sid/src/wire.rs` (build the modal in the wire layer)
- Test: `crates/sid-widgets/tests/network.rs`

`★ Insight ─────────────────────────────────────`
The modal is built in `wire.rs` (where every other tab's modals live), keyed on `network.interface_detail`. The widget itself just emits an action when Enter is pressed; the wire layer reads `NetworkWidget::interfaces().selected_row()` to populate the modal fields. This keeps the widget free of modal-construction concerns and consistent with how SSH/Database/System modals work today.
`─────────────────────────────────────────────────`

- [ ] **Step 5.1: Add failing test for Enter-emits-detail-action**

Append to `crates/sid-widgets/tests/network.rs`:

```rust
use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::adapters::sys::NetInterface;
use sid_core::context::WidgetCtx;
use sid_core::event::{Event, KeyChord};
use sid_core::widget::Widget;
use sid_widgets::network::Focus;
use sid_widgets::NetworkWidget;

fn make_ctx() -> (WidgetCtx, tokio::sync::mpsc::UnboundedReceiver<sid_core::action::ActionId>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (WidgetCtx::new(tx), rx)
}

fn iface(name: &str) -> NetInterface {
    let mut i = NetInterface::default();
    i.name = name.into();
    i.is_up = true;
    i
}

#[test]
fn enter_on_interfaces_pane_emits_show_detail_action() {
    let mut w = NetworkWidget::new();
    // Force-set focus to Interfaces and inject a row.
    use sid_widgets::network::interfaces_sidebar::InterfacesSidebarState;
    // The widget doesn't expose a direct setter today; we simulate by
    // applying a snapshot that contains one interface and tabbing.
    use sid_core::sys_probe::SysSnapshot;
    let snap = SysSnapshot {
        processes: vec![],
        listening_ports: vec![],
        interfaces: vec![iface("wlan0")],
        default_route_iface: Some("wlan0".into()),
    };
    w.apply_snapshot(snap);
    // Tab twice to land on Interfaces (Ports -> Processes -> Interfaces).
    let (mut ctx, _rx) = make_ctx();
    w.handle_event(&Event::Key(KeyChord::new(KeyCode::Tab, KeyModifiers::NONE)), &mut ctx);
    w.handle_event(&Event::Key(KeyChord::new(KeyCode::Tab, KeyModifiers::NONE)), &mut ctx);
    assert_eq!(w.focus(), Focus::Interfaces);

    let (mut ctx, mut rx) = make_ctx();
    w.handle_event(&Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE)), &mut ctx);
    let action = rx.try_recv().expect("expected an action to be emitted");
    assert_eq!(action.as_str(), "network.interface_detail");
}

#[test]
fn capital_e_on_interfaces_pane_toasts_coming_soon() {
    // We can't directly inspect the toast queue from sid-widgets, but the
    // widget should emit a sentinel action that the wire layer consumes
    // as a toast push. For now: assert the widget emits
    // "network.interface_edit_stub" so the wire layer can toast it.
    let mut w = NetworkWidget::new();
    use sid_core::sys_probe::SysSnapshot;
    let snap = SysSnapshot {
        processes: vec![],
        listening_ports: vec![],
        interfaces: vec![iface("wlan0")],
        default_route_iface: None,
    };
    w.apply_snapshot(snap);
    let (mut ctx, _rx) = make_ctx();
    w.handle_event(&Event::Key(KeyChord::new(KeyCode::Tab, KeyModifiers::NONE)), &mut ctx);
    w.handle_event(&Event::Key(KeyChord::new(KeyCode::Tab, KeyModifiers::NONE)), &mut ctx);

    let (mut ctx, mut rx) = make_ctx();
    w.handle_event(&Event::Key(KeyChord::new(KeyCode::Char('E'), KeyModifiers::SHIFT)), &mut ctx);
    let action = rx.try_recv().expect("E must emit a stub action");
    assert_eq!(action.as_str(), "network.interface_edit_stub");
}
```

- [ ] **Step 5.2: Run, verify failure**

```bash
cargo test -p sid-widgets --test network enter_on_interfaces capital_e_on_interfaces
```

Expected: FAIL.

- [ ] **Step 5.3: Add Enter + E handling to `NetworkWidget::handle_event`**

In `crates/sid-widgets/src/network.rs`, find `handle_event` around line 713 and add new arms inside the main match (after the existing `s` and `K` arms):

```rust
            KeyCode::Enter if self.focus == Focus::Interfaces => {
                if self.ifs.selected_row().is_some() {
                    ctx.emit_action("network.interface_detail");
                }
                EventOutcome::Consumed
            }
            KeyCode::Char('E')
                if self.focus == Focus::Interfaces && self.ifs.selected_row().is_some() =>
            {
                ctx.emit_action("network.interface_edit_stub");
                EventOutcome::Consumed
            }
```

Rename the `_ctx` parameter in `handle_event`'s signature to `ctx` so the calls work:

```rust
    fn handle_event(&mut self, ev: &Event, ctx: &mut WidgetCtx) -> EventOutcome {
```

- [ ] **Step 5.4: Run, verify PASS**

```bash
cargo test -p sid-widgets --test network enter_on_interfaces capital_e_on_interfaces
```

Expected: PASS.

- [ ] **Step 5.5: Wire layer — build the modal and the stub toast**

In `crates/sid/src/wire.rs`, find the action dispatch (the same place where `workspaces.open_detail` is handled). Add handlers:

```rust
if action.as_str() == "network.interface_detail" {
    open_network_interface_detail_modal(sid_app);
}
if action.as_str() == "network.interface_edit_stub" {
    sid_app.toasts.push_info("Interface editing not yet supported — see backlog");
}
```

Define `open_network_interface_detail_modal`:

```rust
/// Build a read-only `InterfaceDetail` modal from the currently-selected
/// interface in the Network widget. No-op when no interface is selected.
fn open_network_interface_detail_modal(sid_app: &mut SidApp) {
    use sid_widgets::{Field, ModalSpec};
    // Find the Network widget on the network tab and read the selected row.
    let net = sid_app
        .app
        .tabs()
        .tabs()
        .iter()
        .find(|t| t.id.as_str() == "network")
        .and_then(|t| t.layout.iter_widgets().next())
        .and_then(|w| w.as_any().downcast_ref::<sid_widgets::NetworkWidget>());
    let Some(net) = net else { return; };
    let Some(iface) = net.interfaces().selected_row() else { return; };

    // Format the per-row fields.
    let mac = iface.mac_addr.clone().unwrap_or_else(|| "(none)".into());
    let ipv4s = if iface.ipv4_addrs.is_empty() {
        "(none)".to_string()
    } else {
        iface.ipv4_addrs.join(", ")
    };
    let ipv6s = if iface.ipv6_addrs.is_empty() {
        "(none)".to_string()
    } else {
        iface.ipv6_addrs.join(", ")
    };
    let mtu = iface.mtu.map(|m| m.to_string()).unwrap_or_else(|| "?".into());
    let speed = iface
        .link_speed_mbps
        .map(|m| format!("{m} Mbps"))
        .unwrap_or_else(|| "?".into());

    let fields = vec![
        Field::Display { label: "name".into(), body: iface.name.clone() },
        Field::Display { label: "status".into(), body: if iface.is_up { "up" } else { "down" }.into() },
        Field::Display { label: "MAC".into(), body: mac },
        Field::Display { label: "IPv4".into(), body: ipv4s },
        Field::Display { label: "IPv6".into(), body: ipv6s },
        Field::Display { label: "MTU".into(), body: mtu },
        Field::Display { label: "link speed".into(), body: speed },
    ];
    let modal = ModalSpec::new(
        format!("network.interface_detail:{}", iface.name),
        format!("Interface: {}", iface.name),
        fields,
    )
    .with_help("Edit (E) coming soon — read-only for now. Esc to close.");
    sid_app.modal_stack.push(modal);
}
```

> Note: confirm `NetInterface` has fields `mac_addr`, `ipv4_addrs`, `ipv6_addrs`, `mtu`, `link_speed_mbps`. If field names differ, adapt the calls. If a field doesn't exist (e.g., `link_speed_mbps`), drop that `Field::Display` row and add a comment naming the missing data for a follow-up. The substrate handles `Field::Display` rendering via `field_body_lines` already.

- [ ] **Step 5.6: Commit Task 5**

```bash
git add crates/sid-widgets/src/network.rs crates/sid-widgets/tests/network.rs crates/sid/src/wire.rs
git commit -m "feat(sid-widgets,sid): Network — Enter opens interface detail modal; E stubbed

Enter on a focused interface emits network.interface_detail; the wire
layer builds a read-only modal with the substrate (Field::Display
rows for name/status/MAC/IPv4/IPv6/MTU/speed) and pushes it.

Capital E emits network.interface_edit_stub which the wire layer
toasts as 'Interface editing not yet supported'. Locks in the
chord so muscle memory works once the real adapter lands."
```

---

## Task 6 — Footer hint advertises `/`, `s`, `K`, `Enter`

**Files:**
- Modify: `crates/sid-widgets/src/network.rs` (the `footer_hint` impl around line 698)
- Test: `crates/sid-widgets/tests/footer_hints.rs`

- [ ] **Step 6.1: Add failing test**

Append to `crates/sid-widgets/tests/footer_hints.rs`:

```rust
use sid_core::widget::Widget;
use sid_widgets::NetworkWidget;

#[test]
fn network_footer_hints_include_filter_sort_kill_enter() {
    let w = NetworkWidget::new();
    let hints: Vec<String> = w
        .footer_hint()
        .into_iter()
        .map(|h| format!("{}: {}", h.chord(), h.label()))
        .collect();
    let joined = hints.join(" · ");
    assert!(joined.contains("/"), "filter hint missing: {joined}");
    assert!(joined.contains("filter"), "filter hint missing: {joined}");
    assert!(joined.contains("s"), "sort hint missing: {joined}");
    assert!(joined.contains("K"), "kill hint missing: {joined}");
    assert!(joined.contains("Enter"), "Enter hint missing: {joined}");
}
```

> Confirm `FooterHint::chord()` and `FooterHint::label()` exist. If they're named differently, swap to the actual accessors.

- [ ] **Step 6.2: Run, verify failure**

```bash
cargo test -p sid-widgets --test footer_hints network_footer_hints_include_filter_sort_kill_enter
```

Expected: FAIL — current hint list is missing some.

- [ ] **Step 6.3: Update `NetworkWidget::footer_hint`**

In `crates/sid-widgets/src/network.rs` around line 698, replace:

```rust
    fn footer_hint(&self) -> Vec<FooterHint> {
        vec![
            FooterHint::new("/", "filter"),
            FooterHint::new("s", "sort"),
            FooterHint::new("K", "kill"),
            FooterHint::new("Enter", "detail"),
            FooterHint::new("Tab", "pane"),
            FooterHint::new("R", "refresh"),
        ]
    }
```

- [ ] **Step 6.4: Run, verify PASS**

```bash
cargo test -p sid-widgets --test footer_hints
```

Expected: PASS.

- [ ] **Step 6.5: Refresh any insta snapshots that include the footer**

```bash
cargo insta test -p sid-widgets --review
```

Accept diffs that match the new hint set.

- [ ] **Step 6.6: Commit Task 6**

```bash
git add crates/sid-widgets/src/network.rs crates/sid-widgets/tests/footer_hints.rs crates/sid-widgets/tests/snapshots/
git commit -m "feat(sid-widgets): Network footer hint advertises / filter, s sort, K kill, Enter detail

The / filter has always worked but was undiscoverable. Adding the
explicit hint line — '/' filter · 's' sort · 'K' kill · 'Enter' detail
· 'Tab' pane · 'R' refresh — surfaces it.

Doc test asserts each chord appears so a future refactor can't
silently drop one."
```

---

## Task 7 — Criterion bench: `bench_network_interface_sort_for_n`

**Files:**
- Create: `crates/sid-widgets/benches/interface_sort.rs`
- Modify: `crates/sid-widgets/Cargo.toml`

- [ ] **Step 7.1: Declare bench**

Append to `crates/sid-widgets/Cargo.toml`:

```toml
[[bench]]
name = "interface_sort"
harness = false
```

- [ ] **Step 7.2: Write bench**

Create `crates/sid-widgets/benches/interface_sort.rs`:

```rust
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use sid_core::adapters::sys::NetInterface;
use sid_widgets::network::interfaces_sidebar::sort_interfaces;

fn make_ifaces(n: usize) -> Vec<NetInterface> {
    (0..n)
        .map(|i| {
            let mut iface = NetInterface::default();
            iface.name = match i % 5 {
                0 => format!("wlan{i}"),
                1 => format!("eth{i}"),
                2 => format!("docker{i}"),
                3 => format!("veth_{i}"),
                _ => format!("tun{i}"),
            };
            iface.is_up = i % 2 == 0;
            iface
        })
        .collect()
}

fn bench_sort(c: &mut Criterion) {
    let mut group = c.benchmark_group("interface_sort");
    for n in [5usize, 20, 100] {
        let data = make_ifaces(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let mut d = data.clone();
                sort_interfaces(&mut d, Some("wlan0"));
                black_box(d.len())
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_sort);
criterion_main!(benches);
```

- [ ] **Step 7.3: Run, confirm budget ≤ 50 µs at n=100**

```bash
cargo bench -p sid-widgets --bench interface_sort
```

- [ ] **Step 7.4: Save baseline**

```bash
cargo bench -p sid-widgets --bench interface_sort -- --save-baseline main
```

- [ ] **Step 7.5: Commit Task 7**

```bash
git add crates/sid-widgets/Cargo.toml crates/sid-widgets/benches/interface_sort.rs
git commit -m "perf(sid-widgets): criterion bench for interface sort

50 µs budget at n=100 per the interaction spec. Sort runs on every
SysSnapshot apply; gating on this budget prevents regressions if the
score function gets fancier later."
```

---

## Task 8 — Adversarial: default-route returns Err, sort still works

**Files:**
- Modify: `crates/sid-widgets/tests/interfaces_sidebar.rs`

- [ ] **Step 8.1: Add the test**

Append to `crates/sid-widgets/tests/interfaces_sidebar.rs`:

```rust
#[test]
fn err_from_default_route_collapses_to_alphabetical_via_none() {
    // Simulating SysProvider returning Err: sys_probe collapses to None
    // (per Task 3 behaviour); the sidebar then sorts with default_route=None.
    let mut s = InterfacesSidebarState::new();
    let ifaces = vec![
        iface("eth0", true),
        iface("wlan0", true),
    ];
    s.set_data_with_default_route(ifaces, None);
    let order: Vec<&str> = s.rows().iter().map(|i| i.name.as_str()).collect();
    // No default route → alphabetical among up-physical: eth0, wlan0.
    assert_eq!(order, vec!["eth0", "wlan0"]);
}
```

- [ ] **Step 8.2: Run, verify PASS**

```bash
cargo test -p sid-widgets --test interfaces_sidebar err_from_default_route
```

Expected: PASS.

- [ ] **Step 8.3: Commit Task 8**

```bash
git add crates/sid-widgets/tests/interfaces_sidebar.rs
git commit -m "test(sid-widgets): adversarial — default_route Err collapses to alphabetical sort

sys_probe maps Err to Ok(None) at the snapshot layer; this test
locks in that the sidebar's score function handles None cleanly
(no special-case branch, just 'no WAN to prioritize')."
```

---

## Task 9 — Workspace-wide gate + merge

- [ ] **Step 9.1: /sid-gate**

```bash
/sid-gate
```

Expected: green.

- [ ] **Step 9.2: /sid-perf-check**

```bash
/sid-perf-check
```

Expected: no regressions on `interface_sort`, `tab_render_network`, `app_handle_event_noop`, or any baseline from branches #1–#3.

- [ ] **Step 9.3: Merge to main**

```bash
git checkout main
git merge --no-ff feat/network-drill-in-and-sort -m "Merge branch #4: Network drill-in + WAN-first sort + filter affordance"
```

---

## Definition of done

- [x] `SysProvider::default_route_iface_name` exists with a default impl returning `Ok(None)`.
- [x] `SysinfoProvider` overrides it on Linux (`/proc/net/route`) and macOS (`route -n get default`).
- [x] `SysSnapshot.default_route_iface: Option<String>` is populated each probe tick.
- [x] Interface list sorts primary WAN first, virtual last; alphabetical tiebreak; deterministic.
- [x] `Enter` on a focused interface opens an `InterfaceDetailModal` (read-only `Field::Display` rows).
- [x] `E` on a focused interface toasts "Interface editing not yet supported".
- [x] Network footer hint advertises `/`, `s`, `K`, `Enter` (and `Tab`, `R`).
- [x] Criterion bench for sort saved; 50 µs budget at n=100 met.
- [x] `/sid-gate` clean; `/sid-perf-check` no regressions.
- [x] Branch merged.

## Risks and rollback

- The `/proc/net/route` parser is byte-fragile; the property test + adversarial tests in Task 2 cover malformed input. If a real-world `/proc/net/route` format breaks the parser (e.g., a future kernel changes the column count), the live test surfaces it on next run.
- The Network widget's `apply_snapshot` signature changed (it now reads `default_route_iface` from the snapshot). Callers that construct `SysSnapshot` literals must include the field — Task 3 already updated them.
- Adding fields to `NetInterface` (if needed for the detail modal) is its own breaking change for any downstream user. We assume `mac_addr`, `ipv4_addrs`, `ipv6_addrs`, `mtu`, `link_speed_mbps` already exist; if not, fall back to displaying only the fields that do, and file a follow-up to add the missing ones in `sid-sysinfo`.
