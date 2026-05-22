//! State for the interfaces sidebar pane: a single-column list with
//! wrap-around selection. Unlike the ports/processes tables, the sidebar
//! has no user-driven sort: rows arrive pre-sorted by name from the
//! provider.
//!
//! Selection is preserved across data refreshes when an interface with the
//! same name is still present; otherwise it resets to 0.

use sid_core::adapters::sys::NetInterface;

/// State for the interfaces sidebar.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::interfaces_sidebar::InterfacesSidebarState;
/// let s = InterfacesSidebarState::new();
/// assert!(s.rows().is_empty());
/// assert!(s.selected_row().is_none());
/// ```
#[derive(Debug, Default)]
pub struct InterfacesSidebarState {
    data: Vec<NetInterface>,
    selected: usize,
}

impl InterfacesSidebarState {
    /// Construct a fresh, empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the displayed interfaces. Sorts alphabetically by name
    /// (equivalent to `set_data_with_default_route(data, None)`).
    pub fn set_data(&mut self, data: Vec<NetInterface>) {
        self.set_data_with_default_route(data, None);
    }

    /// Like [`Self::set_data`] but also takes the default-route interface
    /// name (if known) so the primary WAN sorts first. Selection is
    /// preserved across refreshes by interface name.
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

    /// Borrow the displayed rows.
    pub fn rows(&self) -> &[NetInterface] {
        &self.data
    }

    /// Current selection index.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Currently-selected interface, or `None` if the list is empty.
    pub fn selected_row(&self) -> Option<&NetInterface> {
        self.data.get(self.selected)
    }

    /// Advance selection by one, wrapping around the end. No-op when empty.
    pub fn select_next(&mut self) {
        if self.data.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.data.len();
    }

    /// Move selection back by one, wrapping around the start. No-op when
    /// empty.
    pub fn select_prev(&mut self) {
        if self.data.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.data.len() - 1
        } else {
            self.selected - 1
        };
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
/// let wlan = NetInterface {
///     name: "wlan0".into(),
///     addrs: vec![],
///     rx_bytes: 0,
///     tx_bytes: 0,
///     is_up: true,
/// };
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

/// In-place sort by `(score, name)`. Pulled out so benches can call it
/// without setting up the full sidebar state.
pub fn sort_interfaces(data: &mut [NetInterface], default_route: Option<&str>) {
    data.sort_by(|a, b| {
        let sa = iface_sort_score(a, default_route);
        let sb = iface_sort_score(b, default_route);
        sa.cmp(&sb).then_with(|| a.name.cmp(&b.name))
    });
}
