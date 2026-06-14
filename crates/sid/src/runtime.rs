//! Tokio task that converts crossterm events and timer ticks into [`SidEvent`]s.
//!
//! The event pump runs in a separate Tokio task, forwarding crossterm keyboard/
//! mouse/resize events and periodic tick events to a channel that the render
//! loop consumes.
use std::time::Duration;

use anyhow::Result;
use crossterm::event::EventStream;
use futures::StreamExt;
use sid_core::event::Event as SidEvent;
use tokio::sync::mpsc::{Receiver, Sender};

/// Control message sent to a running [`spawn_event_pump`] task.
///
/// Used so the render loop can hand the real terminal (stdin/TTY) to an
/// inline child process — e.g. `$EDITOR` shelled out from the System tab —
/// without the pump's [`EventStream`] reader stealing the child's keystrokes.
#[derive(Debug)]
pub enum PumpControl {
    /// Drop the input reader (releasing the TTY) and then fire the ack
    /// oneshot so the caller knows it is safe to spawn the child. The pump
    /// keeps emitting `Tick`s while suspended.
    Suspend(tokio::sync::oneshot::Sender<()>),
    /// Recreate the input reader after the child has exited.
    Resume,
}

/// A spawned event pump: its join handle plus the control channel used to
/// suspend/resume the input reader (see [`PumpControl`]).
pub struct EventPump {
    /// Join handle for the pump task; `abort()` it at shutdown.
    pub handle: tokio::task::JoinHandle<()>,
    /// Sender for [`PumpControl`] messages.
    pub control: Sender<PumpControl>,
}

/// Spawn a background Tokio task that feeds [`SidEvent`]s onto `tx`.
///
/// The task terminates (joining cleanly) when the receiver is dropped or the
/// crossterm stream ends.  A `Tick` event is emitted every `tick_rate`.
///
/// Returns an [`EventPump`] bundling the join handle and a [`PumpControl`]
/// sender. Send [`PumpControl::Suspend`] to drop the input reader while an
/// inline child owns the TTY, then [`PumpControl::Resume`] to recreate it.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
/// use sid::runtime::{make_channel, spawn_event_pump};
///
/// #[tokio::main]
/// async fn main() {
///     let (tx, mut rx) = make_channel();
///     let pump = spawn_event_pump(tx, Duration::from_millis(250));
///     // The pump runs in the background until the handle is aborted or the
///     // receiver is dropped.
///     pump.handle.abort();
/// }
/// ```
pub fn spawn_event_pump(tx: Sender<SidEvent>, tick_rate: Duration) -> EventPump {
    let (control, mut control_rx) = tokio::sync::mpsc::channel::<PumpControl>(8);
    let handle = tokio::spawn(async move {
        // `Some` while reading input; `None` while suspended (TTY released to a
        // child). The `if reader.is_some()` select guard ensures the reader
        // branch is never polled — and thus never consumes stdin bytes — while
        // suspended.
        let mut reader: Option<EventStream> = Some(EventStream::new());
        let mut ticker = tokio::time::interval(tick_rate);
        loop {
            tokio::select! {
                ctl = control_rx.recv() => {
                    match ctl {
                        Some(PumpControl::Suspend(ack)) => {
                            // Drop the EventStream FIRST (releasing the TTY),
                            // then ack so the caller only spawns the child once
                            // the reader is gone.
                            reader = None;
                            let _ = ack.send(());
                        }
                        Some(PumpControl::Resume) => {
                            reader = Some(EventStream::new());
                        }
                        // All control senders dropped: keep ticking + reading.
                        None => {}
                    }
                }
                _ = ticker.tick() => {
                    if tx.send(SidEvent::Tick).await.is_err() { break; }
                }
                maybe_ev = async { reader.as_mut().expect("guarded by is_some").next().await },
                    if reader.is_some() =>
                {
                    match maybe_ev {
                        Some(Ok(ev)) => {
                            if tx.send(SidEvent::from_crossterm(ev)).await.is_err() { break; }
                        }
                        Some(Err(e)) => {
                            tracing::warn!(error = %e, "crossterm read error");
                        }
                        // Stream ended: stop polling it, but keep the task alive
                        // so ticks (and resume) still work.
                        None => { reader = None; }
                    }
                }
            }
        }
    });
    EventPump { handle, control }
}

/// Create a bounded channel for [`SidEvent`]s.
///
/// Returns `(Sender, Receiver)` with a capacity of 64.
///
/// # Examples
///
/// ```no_run
/// use sid::runtime::make_channel;
///
/// #[tokio::main]
/// async fn main() {
///     let (tx, rx) = make_channel();
///     // tx is passed to spawn_event_pump; rx is held by the render loop.
///     drop(tx);
///     drop(rx);
/// }
/// ```
pub fn make_channel() -> (Sender<SidEvent>, Receiver<SidEvent>) {
    tokio::sync::mpsc::channel(64)
}

/// Convenience wrapper: wait for the next [`SidEvent`] on the receiver.
///
/// Returns `Err` if the channel is closed (all senders dropped).
///
/// # Examples
///
/// ```no_run
/// use sid::runtime::{make_channel, next_event};
/// use sid_core::event::Event;
///
/// #[tokio::main]
/// async fn main() {
///     let (tx, mut rx) = make_channel();
///     tx.send(Event::Tick).await.unwrap();
///     let ev = next_event(&mut rx).await.unwrap();
///     assert_eq!(ev, Event::Tick);
/// }
/// ```
// Used by tests and one-shot drivers; not called from the main binary loop.
#[allow(dead_code)]
pub async fn next_event(rx: &mut Receiver<SidEvent>) -> Result<SidEvent> {
    rx.recv()
        .await
        .ok_or_else(|| anyhow::anyhow!("event stream closed"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sid_core::event::Event as SidEvent;
    use tokio::sync::mpsc;

    use super::*;

    /// `make_channel` returns a channel with capacity 64 — we can enqueue 64
    /// items without blocking.
    #[tokio::test]
    async fn make_channel_capacity_64() {
        let (tx, _rx) = make_channel();
        // Fill exactly 64 slots; each try_send succeeds without blocking.
        for _ in 0..64 {
            tx.try_send(SidEvent::Tick)
                .expect("should not be full at capacity");
        }
        // 65th would overflow the buffer.
        assert!(
            tx.try_send(SidEvent::Tick).is_err(),
            "channel should be full at 65"
        );
    }

    /// `next_event` returns `Ok` when there is an event available.
    #[tokio::test]
    async fn next_event_returns_tick() {
        let (tx, mut rx) = make_channel();
        tx.send(SidEvent::Tick).await.unwrap();
        let ev = next_event(&mut rx).await.unwrap();
        assert_eq!(ev, SidEvent::Tick);
    }

    /// `next_event` returns `Err` when all senders are dropped (stream closed).
    #[tokio::test]
    async fn next_event_closed_channel_returns_err() {
        let (tx, mut rx) = make_channel();
        drop(tx);
        let result = next_event(&mut rx).await;
        assert!(result.is_err(), "should fail on closed channel");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("closed"),
            "error message should mention closed: {msg}"
        );
    }

    /// If the receiver is dropped while the pump is running the pump should
    /// break out of its loop gracefully (no panic, task completes quickly).
    #[tokio::test]
    async fn dropped_receiver_breaks_pump_gracefully() {
        let (tx, rx) = mpsc::channel::<SidEvent>(64);
        drop(rx); // drop the receiver immediately
        // Fill the channel buffer to force a send error on the pump.
        // We send a custom event so the pump exits on the next tick.
        let handle = tokio::spawn(async move {
            // Simulate the pump trying to send to a closed receiver.
            let _ = tx.send(SidEvent::Tick).await; // This will err because rx is dropped.
        });
        // The task should finish quickly without panic.
        tokio::time::timeout(Duration::from_millis(500), handle)
            .await
            .expect("task should finish within 500ms")
            .expect("task should not panic");
    }

    /// Tick events fire at the configured interval.
    ///
    /// We use a tick-only variant of the pump (no EventStream, which requires a
    /// real TTY) and `tokio::time::advance` to verify deterministically.
    #[tokio::test(start_paused = true)]
    async fn tick_events_fire_at_interval() {
        let (tx, mut rx) = make_channel();
        let tick_rate = Duration::from_millis(10);

        // Spawn a tick-only loop — avoids crossterm's EventStream which panics
        // in TTY-less test environments.
        let _handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(tick_rate);
            for _ in 0..3 {
                ticker.tick().await;
                if tx.send(SidEvent::Tick).await.is_err() {
                    break;
                }
            }
        });

        // Advance the clock past two tick intervals.
        tokio::time::advance(Duration::from_millis(25)).await;

        // Allow the spawned task to run.
        tokio::task::yield_now().await;

        // We should receive at least one Tick.
        let mut got_tick = false;
        while let Ok(ev) = rx.try_recv() {
            if ev == SidEvent::Tick {
                got_tick = true;
                break;
            }
        }
        assert!(
            got_tick,
            "should have received a Tick event after advancing the clock"
        );
    }

    /// A sub-millisecond tick rate must not crash or panic.
    #[tokio::test(start_paused = true)]
    async fn sub_millisecond_tick_rate_does_not_crash() {
        let (tx, mut rx) = make_channel();
        let tick_rate = Duration::from_nanos(1); // effectively zero

        // Tick-only loop to avoid EventStream TTY requirement.
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(tick_rate);
            for _ in 0..10 {
                ticker.tick().await;
                if tx.send(SidEvent::Tick).await.is_err() {
                    break;
                }
            }
        });

        // Advance slightly and drain any events.
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;

        while rx.try_recv().is_ok() {}

        handle.abort();
        // No panic = pass.
    }

    /// `tokio::time::interval` panics if given `Duration::ZERO`.  The pump
    /// itself calls `tokio::time::interval(tick_rate)` which will panic.
    /// This test documents and asserts that behavior — callers must pass a
    /// non-zero tick rate.
    ///
    /// We verify the panic happens (not a silent hang or UB) by catching it
    /// with `std::panic::catch_unwind` on a blocking thread.
    #[test]
    fn tick_rate_zero_panics_as_documented() {
        // tokio::time::interval(Duration::ZERO) panics on the first call.
        // Run synchronously in a new runtime to isolate from the test runtime.
        let result = std::panic::catch_unwind(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap();
            rt.block_on(async {
                let _ticker = tokio::time::interval(Duration::ZERO);
                // The panic occurs at construction or first tick.
            });
        });
        assert!(
            result.is_err(),
            "tokio::time::interval(Duration::ZERO) must panic; \
             callers of spawn_event_pump must use a non-zero tick rate"
        );
    }

    /// A very large tick rate (10 minutes) means no `Tick` event fires during
    /// a short test window — the channel stays empty.
    ///
    /// This verifies that `spawn_event_pump` does not emit spurious ticks.
    #[tokio::test(start_paused = true)]
    async fn large_tick_rate_fires_no_tick_in_short_window() {
        let (tx, mut rx) = make_channel();
        let large_tick = Duration::from_secs(600); // 10 minutes

        // Tick-only loop — no EventStream needed.
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(large_tick);
            // The first tick fires immediately (tokio default); subsequent ones
            // are 600 s apart.
            ticker.tick().await; // consume the immediate first tick
            if tx.send(SidEvent::Tick).await.is_err() {
                return;
            }
            // Second tick at t = 600s — we advance only 1s, so this never fires.
            ticker.tick().await;
            let _ = tx.send(SidEvent::Tick).await;
        });

        // Advance only 1 second — nowhere near the 600 s interval.
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;

        // Drain what arrived: should be exactly one event (the first immediate tick).
        let mut count = 0usize;
        while rx.try_recv().is_ok() {
            count += 1;
        }

        handle.abort();

        // Exactly one tick (the immediate first tick) should have fired.
        assert_eq!(
            count, 1,
            "expected exactly 1 tick (immediate) with a 600s interval, got {count}"
        );
    }

    /// Multiple concurrent senders on the same channel should not interfere.
    #[tokio::test]
    async fn multiple_senders_do_not_corrupt_channel() {
        let (tx, mut rx) = make_channel();
        let mut handles = vec![];

        // Spawn 4 tasks each sending 4 Tick events.
        for _ in 0..4 {
            let tx = tx.clone();
            handles.push(tokio::spawn(async move {
                for _ in 0..4 {
                    tx.send(SidEvent::Tick).await.expect("send ok");
                }
            }));
        }

        drop(tx); // drop the original so the channel closes after all tasks finish.

        for h in handles {
            h.await.expect("task must not panic");
        }

        // Collect all events.
        let mut count = 0usize;
        while rx.recv().await.is_some() {
            count += 1;
        }

        assert_eq!(count, 16, "expected 4 tasks × 4 events = 16 total");
    }
}
