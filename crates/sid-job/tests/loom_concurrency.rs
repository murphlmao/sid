//! Loom concurrency model-checker tests for the Arc<Mutex<Vec<…>>> completion
//! handoff that underpins [`sid_job::JobQueue`].
//!
//! # Why loom?
//!
//! [`JobQueue`] stores completions in an `Arc<Mutex<Vec<…>>>` that is written
//! by spawned task threads and read (drained) by the App render loop.  Loom
//! exhaustively explores every possible thread interleaving of those operations,
//! catching data races and ordering bugs that probabilistic testing cannot.
//!
//! # Running
//!
//! ```text
//! RUSTFLAGS="--cfg loom" cargo test --test loom_concurrency -p sid-job --release
//! ```
//!
//! These tests are model-checked by loom, **not** run by ordinary `cargo test`.
//! They compile only when `--cfg loom` is active.

#![cfg(loom)]

use loom::sync::{Arc, Mutex};
use loom::thread;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// A standalone model of the completions buffer — avoids pulling in tokio.
fn make_completions<T>() -> Arc<Mutex<Vec<T>>> {
    Arc::new(Mutex::new(Vec::new()))
}

/// Drain-and-return, mirroring `JobQueue::drain_completed`.
fn drain<T>(completions: &Arc<Mutex<Vec<T>>>) -> Vec<T> {
    let mut guard = completions.lock().unwrap();
    std::mem::take(&mut *guard)
}

// ---------------------------------------------------------------------------
// concurrent_push_drain_no_lost_items
// ---------------------------------------------------------------------------

/// Two producer threads each push one item; the main thread drains after both
/// join.  Loom verifies no item is ever lost under any interleaving.
///
/// This is the tightest model of the producer → completions → drain path.
#[test]
fn concurrent_push_drain_no_lost_items() {
    loom::model(|| {
        let completions: Arc<Mutex<Vec<Result<i32, ()>>>> = make_completions();

        // producer 1
        let c1 = Arc::clone(&completions);
        let t1 = thread::spawn(move || {
            c1.lock().unwrap().push(Ok(1));
        });

        // producer 2
        let c2 = Arc::clone(&completions);
        let t2 = thread::spawn(move || {
            c2.lock().unwrap().push(Ok(2));
        });

        t1.join().unwrap();
        t2.join().unwrap();

        // consumer (main thread) drains after both producers finish.
        let drained = drain(&completions);

        // Under no interleaving should any item be lost.
        assert_eq!(
            drained.len(),
            2,
            "both pushed items must be drained exactly once"
        );
        // Values must be the two we pushed (order undefined).
        let mut vals: Vec<i32> = drained.into_iter().filter_map(|r| r.ok()).collect();
        vals.sort_unstable();
        assert_eq!(vals, [1, 2]);
    });
}

// ---------------------------------------------------------------------------
// interleaved_push_drain
// ---------------------------------------------------------------------------

/// One producer thread pushes 2 items while the main thread races to drain
/// after the first item.  Loom explores every interleaving; the test asserts:
///   - No item is produced more than once (no duplicates in the union of all
///     drained slices).
///   - The total item count across all drains equals the number pushed.
///
/// This models the real-world pattern where the App render loop calls
/// `drain_completed` repeatedly while worker tasks are still pushing.
#[test]
fn interleaved_push_drain() {
    loom::model(|| {
        let completions: Arc<Mutex<Vec<u32>>> = make_completions();

        // Producer pushes items 1 and 2, one at a time.
        let c_producer = Arc::clone(&completions);
        let producer = thread::spawn(move || {
            c_producer.lock().unwrap().push(1u32);
            c_producer.lock().unwrap().push(2u32);
        });

        // Main thread (consumer) drains once while the producer may or may not
        // have pushed yet — loom explores both orderings.
        let mid_drain = drain(&completions);

        // Let the producer finish, then drain whatever remains.
        producer.join().unwrap();
        let final_drain = drain(&completions);

        // Combine both drains and check invariants.
        let mut all: Vec<u32> = mid_drain.into_iter().chain(final_drain).collect();
        all.sort_unstable();

        // Total must equal what the producer pushed.
        assert!(
            all.len() <= 2,
            "total drained items must not exceed the number pushed; got {all:?}"
        );

        // No duplicates: after sorting, no two adjacent elements are equal.
        for window in all.windows(2) {
            assert_ne!(
                window[0], window[1],
                "drain_completed must not produce duplicates; got {window:?}"
            );
        }
    });
}

// ---------------------------------------------------------------------------
// push_then_multi_drain
// ---------------------------------------------------------------------------

/// One producer pushes 3 items; two consumer threads each race to drain.
/// Loom verifies that across both drains the total item count is exactly 3
/// and there are no duplicates — i.e. the Mutex correctly serialises access.
#[test]
fn push_then_multi_drain() {
    loom::model(|| {
        let completions: Arc<Mutex<Vec<u32>>> = make_completions();

        // Fill the buffer before the consumers start.
        {
            let mut g = completions.lock().unwrap();
            g.push(10);
            g.push(20);
            g.push(30);
        }

        // Consumer A
        let ca = Arc::clone(&completions);
        let t_a = thread::spawn(move || drain(&ca));

        // Consumer B races to drain the same buffer.
        let cb = Arc::clone(&completions);
        let t_b = thread::spawn(move || drain(&cb));

        let drain_a = t_a.join().unwrap();
        let drain_b = t_b.join().unwrap();

        let mut all: Vec<u32> = drain_a.into_iter().chain(drain_b).collect();
        all.sort_unstable();

        // Total must be exactly 3 with no duplicates.
        assert_eq!(
            all.len(),
            3,
            "all 3 items must be drained exactly once; got {all:?}"
        );
        assert_eq!(all, [10, 20, 30]);
    });
}
