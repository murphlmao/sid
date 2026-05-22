//! End-to-end integration: SysProbe drives a NetworkWidget via the
//! broadcast channel, and a synthetic 'k' + 'y' event drives the modal
//! state machine. Uses tokio test-util to advance virtual time.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sid_core::adapters::sys::{
    ListeningPort, NetInterface, Pid, ProcessInfo, Protocol, Signal, SocketState, SysError,
    SysProvider,
};
use sid_core::context::WidgetCtx;
use sid_core::event::Event as SidEvent;
use sid_core::sys_probe::SysProbe;
use sid_core::widget::Widget;
use sid_widgets::NetworkWidget;
use sid_widgets::network::ToastLevel;

struct StubProvider {
    iter: std::sync::atomic::AtomicU32,
}

impl StubProvider {
    fn new() -> Self {
        Self {
            iter: std::sync::atomic::AtomicU32::new(0),
        }
    }
}

impl SysProvider for StubProvider {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
        self.iter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(vec![ProcessInfo {
            pid: Pid::from_u32(1),
            name: "init".into(),
            cmd: "/sbin/init".into(),
            cpu_pct: 0.0,
            rss_bytes: 0,
            started_unix_secs: 0,
            parent: None,
            user: Some("0".into()),
        }])
    }
    fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> {
        Ok(vec![ListeningPort {
            port: 22,
            pid: Some(Pid::from_u32(1)),
            command: "sshd".into(),
            protocol: Protocol::Tcp,
            state: SocketState::Listen,
            local_addr: "0.0.0.0".into(),
        }])
    }
    fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> {
        Ok(vec![NetInterface {
            name: "lo".into(),
            addrs: vec!["127.0.0.1".into()],
            rx_bytes: 0,
            tx_bytes: 0,
            is_up: true,
        }])
    }
    fn kill_process(&mut self, _: Pid, _: Signal) -> Result<(), SysError> {
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn snapshot_reaches_widget_after_one_tick() {
    let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(StubProvider::new()));
    let probe = SysProbe::new(provider, Duration::from_millis(100));
    let mut rx = probe.subscribe();
    let handle = tokio::spawn(async move { probe.run().await });

    // First tick fires immediately under tokio time-paused mode.
    let snap = rx.recv().await.expect("first snapshot");
    assert_eq!(snap.processes.len(), 1);
    assert_eq!(snap.listening_ports.len(), 1);

    let mut widget = NetworkWidget::new();
    widget.apply_snapshot(snap);
    assert_eq!(widget.processes().rows().len(), 1);
    assert_eq!(widget.ports().rows().len(), 1);
    assert_eq!(widget.interfaces().rows().len(), 1);

    handle.abort();
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn kill_modal_opens_via_keypress_against_focused_pid() {
    let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(StubProvider::new()));
    let probe = SysProbe::new(provider, Duration::from_millis(100));
    let mut rx = probe.subscribe();
    let handle = tokio::spawn(async move { probe.run().await });

    let snap = rx.recv().await.unwrap();
    let mut widget = NetworkWidget::new();
    widget.apply_snapshot(snap);

    // Capital 'K' opens the modal targeting the focused (ports) PID = 1.
    // Lowercase k is now vim-style "up" — see network.rs handle_event.
    let (tx, _r) = std::sync::mpsc::channel();
    let mut ctx = WidgetCtx::new(tx);
    let ev = SidEvent::from_crossterm(crossterm::event::Event::Key(KeyEvent::new(
        KeyCode::Char('K'),
        KeyModifiers::NONE,
    )));
    widget.handle_event(&ev, &mut ctx);
    assert!(widget.kill_modal().is_confirm_sigterm());
    assert_eq!(widget.kill_modal().target_pid(), Some(Pid::from_u32(1)));

    // 'y' transitions to AwaitingTerm.
    let yev = SidEvent::from_crossterm(crossterm::event::Event::Key(KeyEvent::new(
        KeyCode::Char('y'),
        KeyModifiers::NONE,
    )));
    widget.handle_event(&yev, &mut ctx);
    assert!(widget.kill_modal().is_awaiting_term());

    handle.abort();
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn kill_outcome_surfaces_as_toast_on_widget() {
    use sid_core::sys_probe::kill_job::{KillOutcome, run_kill_job};

    let provider: Arc<Mutex<dyn SysProvider>> = Arc::new(Mutex::new(StubProvider::new()));
    let _probe = SysProbe::new(Arc::clone(&provider), Duration::from_millis(100));

    let mut widget = NetworkWidget::new();
    // run_kill_job against StubProvider: kill_process is OK, processes
    // returns one row (pid=1). Targeting a different pid (42) means the
    // alive-check returns false, so we get Killed.
    let outcome = run_kill_job(Arc::clone(&provider), Pid::from_u32(42), Duration::ZERO)
        .await
        .expect("run_kill_job");
    assert_eq!(outcome, KillOutcome::Killed(Pid::from_u32(42)));
    widget.on_kill_outcome(outcome);
    let toast = widget.take_toast().expect("toast");
    assert_eq!(toast.level, ToastLevel::Success);
    assert!(toast.message.contains("42"));
}
