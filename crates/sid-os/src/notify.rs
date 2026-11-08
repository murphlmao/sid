//! `org.freedesktop.Notifications` over the blocking zbus API.

use std::collections::HashMap;
use std::sync::Mutex;

use sid_core::notify::{Notification, Notifier, NotifyError, Urgency};
use zbus::zvariant::Value;

/// The `urgency` hint byte: 0 low, 1 normal, 2 critical (spec section "Hints").
pub(crate) fn urgency_byte(u: Urgency) -> u8 {
    match u {
        Urgency::Low => 0,
        Urgency::Normal => 1,
        Urgency::Critical => 2,
    }
}

/// Session-bus notifier. Connects lazily and reconnects after a failure.
#[derive(Default)]
pub struct DbusNotifier {
    conn: Mutex<Option<zbus::blocking::Connection>>,
}

impl DbusNotifier {
    /// A notifier that has not connected yet.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Notifier for DbusNotifier {
    fn notify(&self, n: &Notification) -> Result<(), NotifyError> {
        let mut guard = self.conn.lock().unwrap();
        if guard.is_none() {
            *guard = Some(
                zbus::blocking::Connection::session()
                    .map_err(|e| NotifyError::Unavailable(e.to_string()))?,
            );
        }
        let conn = guard.as_ref().expect("just set");
        let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
        hints.insert("urgency", Value::U8(urgency_byte(n.urgency)));
        let actions: Vec<&str> = Vec::new();
        let res = conn.call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "sid",
                0u32,
                "",
                n.title.as_str(),
                n.body.as_str(),
                actions,
                hints,
                -1i32,
            ),
        );
        match res {
            Ok(_) => Ok(()),
            Err(e) => {
                *guard = None; // drop a possibly dead connection
                Err(NotifyError::Failed(e.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urgency_maps_to_spec_bytes() {
        assert_eq!(urgency_byte(Urgency::Low), 0);
        assert_eq!(urgency_byte(Urgency::Normal), 1);
        assert_eq!(urgency_byte(Urgency::Critical), 2);
    }
}
