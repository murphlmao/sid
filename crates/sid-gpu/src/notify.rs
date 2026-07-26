//! Best-effort desktop notification for terminal pre-flight failures. sid may
//! have been launched from a desktop entry where stderr is invisible — this is
//! the only channel left when no window can exist. Strictly best-effort: no
//! notification daemon, no `notify-send`, a hang — all silently tolerated
//! (with a 2s kill so a stuck notifier can't stall the failure exit).

use sid_core::gpu::Diagnosis;
use std::time::Duration;

pub(crate) fn notify_failure(diagnosis: &Diagnosis) {
    let cmd: Vec<String> = vec![
        "notify-send".into(),
        "-a".into(),
        "sid".into(),
        "-u".into(),
        "critical".into(),
        format!("sid can't start: {}", diagnosis.summary),
        diagnosis.remedy.clone(),
    ];
    let _ = crate::probe::run_silenced(&cmd, Duration::from_secs(2));
}
