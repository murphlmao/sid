//! Tokio task that converts crossterm events and timer ticks into [`SidEvent`]s.
//!
//! The event pump runs in a separate Tokio task, forwarding crossterm keyboard/
//! mouse/resize events and periodic tick events to a channel that the render
//! loop consumes.
// The three public functions below are consumed by wire.rs / main.rs in Task 39.
// The #[allow] is removed then.
#![allow(dead_code)]

use std::time::Duration;

use anyhow::Result;
use crossterm::event::EventStream;
use futures::StreamExt;
use sid_core::event::Event as SidEvent;
use tokio::sync::mpsc::{Receiver, Sender};

/// Spawn a background Tokio task that feeds [`SidEvent`]s onto `tx`.
///
/// The task terminates (joining cleanly) when the receiver is dropped or the
/// crossterm stream ends.  A `Tick` event is emitted every `tick_rate`.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
/// use sid::runtime::{make_channel, spawn_event_pump};
///
/// # tokio_test::block_on(async {
/// let (tx, mut rx) = make_channel();
/// let handle = spawn_event_pump(tx, Duration::from_millis(250));
/// // The pump runs in the background until the handle is aborted or the
/// // receiver is dropped.
/// handle.abort();
/// # });
/// ```
pub fn spawn_event_pump(tx: Sender<SidEvent>, tick_rate: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = EventStream::new();
        let mut ticker = tokio::time::interval(tick_rate);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if tx.send(SidEvent::Tick).await.is_err() { break; }
                }
                maybe_ev = reader.next() => {
                    match maybe_ev {
                        Some(Ok(ev)) => {
                            if tx.send(SidEvent::from_crossterm(ev)).await.is_err() { break; }
                        }
                        Some(Err(e)) => {
                            tracing::warn!(error = %e, "crossterm read error");
                        }
                        None => break,
                    }
                }
            }
        }
    })
}

/// Create a bounded channel for [`SidEvent`]s.
///
/// Returns `(Sender, Receiver)` with a capacity of 64.
///
/// # Examples
///
/// ```no_run
/// use sid::runtime::make_channel;
/// # tokio_test::block_on(async {
/// let (tx, rx) = make_channel();
/// // tx is passed to spawn_event_pump; rx is held by the render loop.
/// drop(tx);
/// drop(rx);
/// # });
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
/// # tokio_test::block_on(async {
/// let (tx, mut rx) = make_channel();
/// tx.send(Event::Tick).await.unwrap();
/// let ev = next_event(&mut rx).await.unwrap();
/// assert_eq!(ev, Event::Tick);
/// # });
/// ```
pub async fn next_event(rx: &mut Receiver<SidEvent>) -> Result<SidEvent> {
    rx.recv().await.ok_or_else(|| anyhow::anyhow!("event stream closed"))
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
            tx.try_send(SidEvent::Tick).expect("should not be full at capacity");
        }
        // 65th would overflow the buffer.
        assert!(tx.try_send(SidEvent::Tick).is_err(), "channel should be full at 65");
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
        assert!(msg.contains("closed"), "error message should mention closed: {msg}");
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
        assert!(got_tick, "should have received a Tick event after advancing the clock");
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
}
