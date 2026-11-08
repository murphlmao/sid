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

/// Longest title / body sent to the server, in chars (an ellipsis marks a cut).
const MAX_TITLE: usize = 120;
const MAX_BODY: usize = 400;

/// Drop control characters (keeping `\n` in bodies) and cap the length.
fn clean(s: &str, max: usize, keep_newlines: bool) -> String {
    let mut out: String = s
        .chars()
        .filter(|c| {
            (!c.is_control() || (keep_newlines && *c == '\n'))
                && !sid_core::text::is_deceptive(*c)
        })
        .take(max + 1)
        .collect();
    if out.chars().count() > max {
        out = out.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
    }
    out
}

/// The body is parsed as markup by servers that advertise `body-markup` (`<b>`, `<a>`,
/// `<img>` …), and it interpolates names from config a repository can commit — so it
/// is always XML-escaped.
pub(crate) fn sanitize_body(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in clean(s, MAX_BODY, true).chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// The summary is plain text per the spec, so entities would show literally on a
/// compliant server; angle brackets are swapped for look-alikes instead, in case a
/// non-compliant server parses it anyway.
pub(crate) fn sanitize_title(s: &str) -> String {
    clean(s, MAX_TITLE, false)
        .replace('<', "‹")
        .replace('>', "›")
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
        let (title, body) = (sanitize_title(&n.title), sanitize_body(&n.body));
        let res = conn.call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "sid",
                0u32,
                "",
                title.as_str(),
                body.as_str(),
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
    fn bidi_and_zero_width_are_stripped() {
        assert_eq!(sanitize_title("a\u{202E}b\u{200D}c\u{2066}d"), "abcd");
        assert_eq!(sanitize_body("x\u{200F}y\nz\u{FEFF}"), "xy\nz");
    }

    #[test]
    fn body_markup_from_config_names_is_escaped() {
        let body =
            sanitize_body(r#"<a href="http://x">prod</a> & <img src="file:///etc/passwd"/>: done"#);
        assert!(!body.contains('<') && !body.contains('>'), "{body}");
        assert!(body.starts_with("&lt;a href=&quot;http://x&quot;&gt;prod&lt;/a&gt; &amp; "));
    }

    #[test]
    fn title_has_no_angle_brackets_or_controls_and_both_are_capped() {
        assert_eq!(sanitize_title("<b>db</b>\u{7}\nx"), "‹b›db‹/b›x");
        assert_eq!(sanitize_title(&"t".repeat(500)).chars().count(), MAX_TITLE);
        assert!(sanitize_title(&"t".repeat(500)).ends_with('…'));
        // Escaping happens after the cap, so the escaped body may be longer, but the
        // visible text never exceeds MAX_BODY chars.
        assert_eq!(sanitize_body(&"b".repeat(1000)).chars().count(), MAX_BODY);
        assert_eq!(sanitize_body("line1\nline2"), "line1\nline2");
    }

    #[test]
    fn urgency_maps_to_spec_bytes() {
        assert_eq!(urgency_byte(Urgency::Low), 0);
        assert_eq!(urgency_byte(Urgency::Normal), 1);
        assert_eq!(urgency_byte(Urgency::Critical), 2);
    }
}
