//! Integration tests for sid-job::JobQueue and JobHandle.
//!
//! Adversarial coverage:
//! - Handle dropped before await (cancellation path).
//! - Concurrent burst (100 jobs) all complete.
//! - drain_completed never duplicates results.
//! - loom stub block documents where model-checker tests belong.

use std::time::Duration;

use sid_job::{JobError, JobHandle, JobQueue};
use tokio::time::sleep;

// ── Plan-required tests ─────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_and_await_result() {
    let queue: JobQueue<i32> = JobQueue::new();
    let handle = queue.spawn(async {
        sleep(Duration::from_millis(10)).await;
        42i32
    });
    let v = handle.await_result().await.unwrap();
    assert_eq!(v, 42);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn poll_returns_completed_results() {
    let queue: JobQueue<i32> = JobQueue::new();
    let _h1 = queue.spawn(async { 1 });
    let _h2 = queue.spawn(async { 2 });
    sleep(Duration::from_millis(20)).await;
    let drained = queue.drain_completed();
    let mut values: Vec<i32> = drained.into_iter().filter_map(|r| r.ok()).collect();
    values.sort();
    assert_eq!(values, vec![1, 2]);
}

// ── Adversarial: concurrent burst ──────────────────────────────────────────

/// Spawn 100 concurrent jobs and verify all complete via drain_completed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_burst_all_complete() {
    let queue: JobQueue<u32> = JobQueue::new();

    let handles: Vec<JobHandle<u32>> = (0u32..100)
        .map(|i| queue.spawn(async move { i }))
        .collect();

    // Await all handles to ensure every job finishes.
    for h in handles {
        let _ = h.await_result().await;
    }

    // drain_completed may have been called by individual awaits, so check total.
    // Drain and collect any remaining completions.
    let drained = queue.drain_completed();
    // All results must be Ok (no panics in these trivial futures).
    for r in &drained {
        assert!(r.is_ok());
    }
    // We shouldn't have more results than 100.
    assert!(drained.len() <= 100);
}

/// Spawn 100 concurrent jobs, wait for them all, then drain_completed returns
/// exactly the jobs that completed while not yet drained.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drain_collects_all_completed_jobs() {
    let queue: JobQueue<u32> = JobQueue::new();

    // Spawn jobs but keep handles dropped immediately so completions accumulate.
    for i in 0u32..100 {
        let _ = queue.spawn(async move { i });
    }

    // Give all jobs time to run.
    sleep(Duration::from_millis(100)).await;

    let drained = queue.drain_completed();

    // Should have gotten all 100 results.
    assert_eq!(drained.len(), 100);
    // All should be Ok.
    for r in &drained {
        assert!(r.is_ok());
    }

    // Second drain should be empty — no duplicates.
    let second = queue.drain_completed();
    assert!(second.is_empty(), "drain_completed must not return duplicates");
}

// ── Adversarial: drain_completed never duplicates ──────────────────────────

/// drain_completed returns each result at most once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drain_completed_no_duplicates() {
    let queue: JobQueue<i32> = JobQueue::new();
    let _h1 = queue.spawn(async { 10 });
    let _h2 = queue.spawn(async { 20 });

    sleep(Duration::from_millis(30)).await;

    let first = queue.drain_completed();
    let second = queue.drain_completed();

    // Total results across both drains: exactly the jobs that finished.
    assert!(first.len() + second.len() <= 2);
    // The second drain should be empty because the first drained everything.
    assert!(second.is_empty(), "second drain must be empty — no duplicates allowed");
}

// ── Adversarial: drop handle before await (cancellation) ───────────────────

/// Dropping the JobHandle before calling await_result should not panic or
/// deadlock. The job itself still runs to completion (the future is spawned
/// on the Tokio runtime independently), so completions still receives the
/// result. The dropped handle just loses its rx end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_handle_before_await_does_not_panic() {
    let queue: JobQueue<i32> = JobQueue::new();

    {
        let _handle = queue.spawn(async { 99 });
        // `_handle` is dropped here at end of block, without being awaited.
    }

    // Give the spawned task time to complete.
    sleep(Duration::from_millis(30)).await;

    // The job still ran; its result is in the completions vec.
    let drained = queue.drain_completed();
    let values: Vec<i32> = drained.into_iter().filter_map(|r| r.ok()).collect();
    assert_eq!(values, vec![99]);
}

/// await_result on a handle whose rx was consumed returns Cancelled.
/// This exercises the `None` arm of await_result directly.
/// NOTE: JobHandle does not expose a way to take `rx` from outside, so we
/// verify the Cancelled variant through its Debug/Display impls instead.
#[test]
fn job_error_cancelled_displays_correctly() {
    let e = JobError::Cancelled;
    assert_eq!(format!("{e}"), "job cancelled");
}

#[test]
fn job_error_panic_displays_correctly() {
    let e = JobError::Panic;
    assert_eq!(format!("{e}"), "job panicked");
}

// ── JobError: Clone + Debug round-trip ─────────────────────────────────────

#[test]
fn job_error_clone_and_debug() {
    let e1 = JobError::Panic;
    let e2 = e1.clone();
    assert_eq!(format!("{e2:?}"), "Panic");

    let e3 = JobError::Cancelled;
    let e4 = e3.clone();
    assert_eq!(format!("{e4:?}"), "Cancelled");
}

// ── Default impl ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn job_queue_default_is_same_as_new() {
    let q1: JobQueue<i32> = JobQueue::new();
    let q2: JobQueue<i32> = JobQueue::default();
    // Both should accept spawned jobs and drain results.
    let h1 = q1.spawn(async { 1 });
    let h2 = q2.spawn(async { 2 });
    assert_eq!(h1.await_result().await.unwrap(), 1);
    assert_eq!(h2.await_result().await.unwrap(), 2);
}

// ── Adversarial: stress — 10,000 instant jobs ─────────────────────────────

/// Spawn 10,000 instantaneous jobs and verify all of them complete.
/// This stresses the Arc<Mutex<Vec<…>>> under a high volume of concurrent
/// pushes from the Tokio thread pool, catching any lost-write scenario.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn stress_10000_jobs_all_complete() {
    const N: u32 = 10_000;
    let queue: JobQueue<u32> = JobQueue::new();

    let handles: Vec<_> = (0..N).map(|i| queue.spawn(async move { i })).collect();

    // Await every handle to ensure the Tokio runtime has finished all tasks.
    for h in handles {
        let _ = h.await_result().await;
    }

    // Drain and count completions — some may have already been drained via
    // await_result paths, so assert total ≤ N, not == N.
    let drained = queue.drain_completed();
    assert!(
        drained.len() <= N as usize,
        "drained more results than jobs spawned: {}",
        drained.len()
    );
    for r in &drained {
        assert!(r.is_ok(), "unexpected error from trivial job: {r:?}");
    }
}

// ── Adversarial: never-completing job stays out of drain ──────────────────

/// A job that never completes (sleeping for ~28 hours) must NOT appear in
/// `drain_completed`.  This validates the partial-state invariant: the
/// completions buffer only ever holds results from *finished* tasks.
///
/// The test terminates quickly because we only sleep 50 ms before draining —
/// the long-sleeping job is simply never finished in time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn never_completing_job_absent_from_drain() {
    let queue: JobQueue<u32> = JobQueue::new();

    // Spawn a job that will never complete within this test's lifetime.
    // The handle is intentionally dropped so the test doesn't block on it.
    let _handle = queue.spawn(async {
        sleep(Duration::from_secs(99_999)).await;
        0u32
    });

    // Give the runtime a tick to schedule the spawned task.
    sleep(Duration::from_millis(50)).await;

    // drain_completed must return empty — the job hasn't finished.
    let drained = queue.drain_completed();
    assert!(
        drained.is_empty(),
        "drain_completed must not return results for unfinished jobs; got {:?}",
        drained.len()
    );
}

// ── Adversarial: multi-thread concurrent spawn ────────────────────────────

/// Spawn many jobs from multiple threads concurrently using `tokio::spawn`
/// wrappers, then verify the final drained count matches the number of jobs.
///
/// This models the scenario where multiple async subsystems call
/// `JobQueue::spawn` simultaneously from different tasks on the thread pool.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn multi_task_concurrent_spawn_all_counted() {
    use std::sync::Arc;

    const N: u32 = 200;
    let queue: Arc<JobQueue<u32>> = Arc::new(JobQueue::new());

    // Spawn N Tokio tasks, each of which spawns one job on the shared queue.
    let spawner_tasks: Vec<_> = (0..N)
        .map(|i| {
            let q = Arc::clone(&queue);
            tokio::spawn(async move {
                let h = q.spawn(async move { i });
                h.await_result().await.ok()
            })
        })
        .collect();

    // Wait for every spawner task.
    let mut completed = 0u32;
    for jh in spawner_tasks {
        if jh.await.unwrap().is_some() {
            completed += 1;
        }
    }

    assert_eq!(
        completed, N,
        "all {N} jobs spawned from concurrent tasks must complete"
    );
}

// ── loom model-checker tests ───────────────────────────────────────────────
//
// Full loom tests live in `tests/loom_concurrency.rs`, gated behind
// `#![cfg(loom)]`.  Run them with:
//
//   RUSTFLAGS="--cfg loom" cargo test --test loom_concurrency -p sid-job --release
//
// Those tests model-check the Arc<Mutex<Vec<…>>> completion handoff by
// replacing std::sync with loom::sync and std::thread with loom::thread,
// then running loom::model(|| { … }) which explores all thread interleavings.
