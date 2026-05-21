use sid_core::adapters::sys::{NetInterface, SysError};
use sysinfo::Networks;

/// List network interfaces via sysinfo. sysinfo's `Networks` lives separately
/// from `System` and is rebuilt per-call — cheap relative to a full `System`
/// refresh and avoids stale-counter issues.
pub(crate) fn list_interfaces(_sys: &mut sysinfo::System) -> Result<Vec<NetInterface>, SysError> {
    let mut nets = Networks::new_with_refreshed_list();
    // A second refresh allows rx/tx delta-since-baseline; the absolute
    // counters we report are sysinfo's "total_received" / "total_transmitted".
    nets.refresh();

    let mut out = Vec::with_capacity(nets.len());
    for (name, data) in nets.iter() {
        let addrs: Vec<String> = data
            .ip_networks()
            .iter()
            .map(|n| n.addr.to_string())
            .collect();
        let has_addrs = !addrs.is_empty();
        let rx = data.total_received();
        let tx = data.total_transmitted();
        out.push(NetInterface {
            name: name.to_string(),
            addrs,
            rx_bytes: rx,
            tx_bytes: tx,
            // sysinfo doesn't expose UP/DOWN on every platform; treat any
            // interface with addresses or activity as up.
            is_up: has_addrs || rx > 0 || tx > 0,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}
