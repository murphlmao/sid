//! Desktop-notification seam. The Linux impl (`org.freedesktop.Notifications`) lives in
//! `sid-os`.

use std::sync::Mutex;

/// How loudly to present a notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum Urgency {
    /// Background information.
    Low,
    /// Ordinary.
    #[default]
    Normal,
    /// Needs attention now.
    Critical,
}

/// One notification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notification {
    /// Short headline.
    pub title: String,
    /// Longer text; may be empty.
    pub body: String,
    /// Presentation urgency.
    pub urgency: Urgency,
}

impl Notification {
    /// A normal-urgency notification.
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            urgency: Urgency::Normal,
        }
    }
    /// Builder: set the urgency.
    pub fn urgency(mut self, u: Urgency) -> Self {
        self.urgency = u;
        self
    }
}

/// Notification delivery failure.
#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    /// No notification service reachable.
    #[error("notification service unavailable: {0}")]
    Unavailable(String),
    /// The service rejected or failed the call.
    #[error("notification failed: {0}")]
    Failed(String),
}

/// Shows desktop notifications.
pub trait Notifier: Send + Sync {
    /// Show `n`. Best-effort; callers should log, not surface, errors.
    fn notify(&self, n: &Notification) -> Result<(), NotifyError>;
}

/// Test fake that records what was sent.
#[derive(Default)]
pub struct FakeNotifier {
    sent: Mutex<Vec<Notification>>,
}

impl FakeNotifier {
    /// Empty recorder.
    pub fn new() -> Self {
        Self::default()
    }
    /// Everything sent so far.
    pub fn sent(&self) -> Vec<Notification> {
        self.sent.lock().unwrap().clone()
    }
}

impl Notifier for FakeNotifier {
    fn notify(&self, n: &Notification) -> Result<(), NotifyError> {
        self.sent.lock().unwrap().push(n.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_records() {
        let f = FakeNotifier::new();
        f.notify(&Notification::new("t", "b").urgency(Urgency::Critical))
            .unwrap();
        assert_eq!(f.sent()[0].urgency, Urgency::Critical);
    }
}
