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
    aliases: std::collections::HashMap<String, String>,
    pinned_names: std::collections::HashSet<String>,
    /// Name of the interface that holds the default route as of the last
    /// `set_data_with_prefs` / `set_data_with_default_route` call.
    default_route: Option<String>,
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
        data: Vec<NetInterface>,
        default_route: Option<&str>,
    ) {
        self.set_data_with_prefs(data, default_route, &[]);
    }

    /// Like [`Self::set_data_with_default_route`] but also accepts a slice of
    /// interface names that should sort before all unpinned interfaces.
    ///
    /// Selection is preserved across refreshes by interface name.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::sys::NetInterface;
    /// use sid_widgets::network::interfaces_sidebar::InterfacesSidebarState;
    ///
    /// let mut s = InterfacesSidebarState::new();
    /// let data = vec![
    ///     NetInterface { name: "eth0".into(), addrs: vec![], rx_bytes: 0, tx_bytes: 0, is_up: true },
    ///     NetInterface { name: "wlan0".into(), addrs: vec![], rx_bytes: 0, tx_bytes: 0, is_up: true },
    /// ];
    /// s.set_data_with_prefs(data, Some("wlan0"), &["eth0"]);
    /// // Pinned eth0 must be first despite wlan0 holding the default route.
    /// assert_eq!(s.rows()[0].name, "eth0");
    /// ```
    pub fn set_data_with_prefs(
        &mut self,
        mut data: Vec<NetInterface>,
        default_route: Option<&str>,
        pinned: &[&str],
    ) {
        sort_interfaces_with_pins(&mut data, default_route, pinned);
        self.default_route = default_route.map(|s| s.to_string());
        let prev_name = self.data.get(self.selected).map(|i| i.name.clone());
        self.data = data;
        self.selected = prev_name
            .and_then(|n| self.data.iter().position(|i| i.name == n))
            .unwrap_or(0);
        if self.selected >= self.data.len() {
            self.selected = 0;
        }
    }

    /// Replace the alias map. Keys are raw interface names; values are the
    /// display aliases. An empty string means "no alias for this interface".
    pub fn set_aliases(&mut self, aliases: std::collections::HashMap<String, String>) {
        self.aliases = aliases;
    }

    /// Replace the pinned-names set.
    pub fn set_pinned_names(&mut self, pinned: std::collections::HashSet<String>) {
        self.pinned_names = pinned;
    }

    /// Whether `name` is the default-route interface as of the last data
    /// refresh. The answer comes from `SysSnapshot.default_route_iface`,
    /// stored by `set_data_with_prefs` / `set_data_with_default_route`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::sys::NetInterface;
    /// use sid_widgets::network::interfaces_sidebar::InterfacesSidebarState;
    ///
    /// let mut s = InterfacesSidebarState::new();
    /// let data = vec![
    ///     NetInterface { name: "eth0".into(), addrs: vec![], rx_bytes: 0, tx_bytes: 0, is_up: true },
    /// ];
    /// s.set_data_with_default_route(data, Some("eth0"));
    /// assert!(s.is_default_route("eth0"));
    /// assert!(!s.is_default_route("wlan0"));
    /// ```
    pub fn is_default_route(&self, name: &str) -> bool {
        self.default_route.as_deref() == Some(name)
    }

    /// Display label for an interface: `alias (raw_name)` when an alias is
    /// set, raw name only when none.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::network::interfaces_sidebar::InterfacesSidebarState;
    /// let mut s = InterfacesSidebarState::new();
    /// s.set_aliases(std::collections::HashMap::from([
    ///     ("eth0".into(), "work-lan".into()),
    /// ]));
    /// assert_eq!(s.display_label("eth0"), "work-lan (eth0)");
    /// assert_eq!(s.display_label("wlan0"), "wlan0");
    /// ```
    pub fn display_label<'a>(&'a self, name: &'a str) -> String {
        match self.aliases.get(name) {
            Some(a) if !a.is_empty() => format!("{a} ({name})"),
            _ => name.to_string(),
        }
    }

    /// Whether `name` is in the pinned set.
    pub fn is_pinned(&self, name: &str) -> bool {
        self.pinned_names.contains(name)
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

/// In-place sort: pinned names first (sorted alphabetically among themselves),
/// then by the existing `(score, name)` key for the rest.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::sys::NetInterface;
/// use sid_widgets::network::interfaces_sidebar::sort_interfaces_with_pins;
///
/// let mut data = vec![
///     NetInterface { name: "wlan0".into(), addrs: vec![], rx_bytes: 0, tx_bytes: 0, is_up: true },
///     NetInterface { name: "eth0".into(), addrs: vec![], rx_bytes: 0, tx_bytes: 0, is_up: true },
/// ];
/// sort_interfaces_with_pins(&mut data, Some("wlan0"), &["eth0"]);
/// assert_eq!(data[0].name, "eth0"); // pinned wins over default-route
/// ```
pub fn sort_interfaces_with_pins(
    data: &mut [NetInterface],
    default_route: Option<&str>,
    pinned: &[&str],
) {
    data.sort_by(|a, b| {
        let a_pinned = pinned.contains(&a.name.as_str());
        let b_pinned = pinned.contains(&b.name.as_str());
        match (a_pinned, b_pinned) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (true, true) => a.name.cmp(&b.name),
            (false, false) => {
                let sa = iface_sort_score(a, default_route);
                let sb = iface_sort_score(b, default_route);
                sa.cmp(&sb).then_with(|| a.name.cmp(&b.name))
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str, is_up: bool) -> NetInterface {
        NetInterface {
            name: name.into(),
            addrs: vec![],
            rx_bytes: 0,
            tx_bytes: 0,
            is_up,
        }
    }

    #[test]
    fn pinned_iface_sorts_before_default_route_holder() {
        let mut state = InterfacesSidebarState::new();
        let data = vec![iface("eth0", true), iface("wlan0", true), iface("lo", true)];
        // wlan0 is default route; eth0 is pinned. eth0 must sort first.
        state.set_data_with_prefs(data, Some("wlan0"), &["eth0"]);
        assert_eq!(state.rows()[0].name, "eth0");
    }

    #[test]
    fn no_pins_behaves_like_default_route_sort() {
        let mut state = InterfacesSidebarState::new();
        let data = vec![iface("eth0", true), iface("wlan0", true)];
        state.set_data_with_prefs(data.clone(), Some("wlan0"), &[]);
        let mut state2 = InterfacesSidebarState::new();
        state2.set_data_with_default_route(data, Some("wlan0"));
        assert_eq!(
            state.rows().iter().map(|i| &i.name).collect::<Vec<_>>(),
            state2.rows().iter().map(|i| &i.name).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn multiple_pins_respect_alphabetical_tiebreak() {
        let mut state = InterfacesSidebarState::new();
        let data = vec![iface("zz0", true), iface("aa0", true), iface("mm0", true)];
        state.set_data_with_prefs(data, None, &["mm0", "aa0"]);
        // Both pinned; alphabetical within pinned group.
        assert_eq!(state.rows()[0].name, "aa0");
        assert_eq!(state.rows()[1].name, "mm0");
    }

    #[test]
    fn pinning_unknown_name_does_not_panic() {
        let mut state = InterfacesSidebarState::new();
        let data = vec![iface("eth0", true)];
        // "ghost" not in the list — must not panic.
        state.set_data_with_prefs(data, None, &["ghost"]);
        assert_eq!(state.rows()[0].name, "eth0");
    }
}
