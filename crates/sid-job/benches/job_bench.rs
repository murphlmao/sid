//! Criterion benchmarks for [`sid_job::JobQueue`].
//!
//! Run with:
//! ```text
//! cargo bench -p sid-job
//! ```
//!
//! Or to compile without running:
//! ```text
//! cargo bench -p sid-job --no-run
//! ```
//!
//! All benchmarks wrap async operations in a `tokio::runtime::Runtime` because
//! criterion is synchronous.

use criterion::{Criterion, criterion_group, criterion_main};
use sid_job::JobQueue;
use std::time::Duration;

// ---------------------------------------------------------------------------
// bench_spawn_single_job
// ---------------------------------------------------------------------------

/// Measures the round-trip latency of spawning a single trivial future and
/// awaiting its result via [`JobHandle::await_result`].
fn bench_spawn_single_job(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    let queue: JobQueue<i32> = JobQueue::new();

    c.bench_function("spawn_single_job", |b| {
        b.iter(|| {
            rt.block_on(async {
                let handle = queue.spawn(async { 42i32 });
                handle.await_result().await.unwrap()
            })
        })
    });
}

// ---------------------------------------------------------------------------
// bench_spawn_drain_100
// ---------------------------------------------------------------------------

/// Spawns 100 trivial jobs sequentially, waits for all to complete, then
/// drains the completions buffer in a single call.
///
/// Measures the combined cost of spawning + draining at moderate queue depth.
fn bench_spawn_drain_100(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    c.bench_function("spawn_drain_100", |b| {
        b.iter(|| {
            rt.block_on(async {
                let queue: JobQueue<u32> = JobQueue::new();

                // Spawn 100 jobs, await each handle so they complete before drain.
                let mut handles = Vec::with_capacity(100);
                for i in 0u32..100 {
                    handles.push(queue.spawn(async move { i }));
                }
                for h in handles {
                    let _ = h.await_result().await;
                }

                // Drain whatever landed in the completions buffer.
                let drained = queue.drain_completed();
                drained.len()
            })
        })
    });
}

// ---------------------------------------------------------------------------
// bench_drain_empty
// ---------------------------------------------------------------------------

/// Measures the overhead of `drain_completed` on an empty queue — the
/// hot-path cost the render loop pays on every tick when there's nothing to
/// collect.
fn bench_drain_empty(c: &mut Criterion) {
    let queue: JobQueue<i32> = JobQueue::new();

    c.bench_function("drain_empty", |b| {
        b.iter(|| {
            // No runtime needed — drain_completed is synchronous.
            queue.drain_completed()
        })
    });
}

// ---------------------------------------------------------------------------
// bench_concurrent_spawn
// ---------------------------------------------------------------------------

/// Spawns 100 trivial jobs concurrently via `futures::future::join_all` and
/// measures total wall-clock time until all results are available.
///
/// This stresses the `Arc<Mutex<Vec<…>>>` under maximum write concurrency.
fn bench_concurrent_spawn(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    c.bench_function("concurrent_spawn_100", |b| {
        b.iter(|| {
            rt.block_on(async {
                let queue: JobQueue<u32> = JobQueue::new();

                // Launch all 100 jobs simultaneously.
                let handles: Vec<_> = (0u32..100).map(|i| queue.spawn(async move { i })).collect();

                // Wait for all of them concurrently.
                let results =
                    futures::future::join_all(handles.into_iter().map(|h| h.await_result())).await;

                results.len()
            })
        })
    });
}

// ---------------------------------------------------------------------------
// criterion harness
// ---------------------------------------------------------------------------

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(3));
    targets =
        bench_spawn_single_job,
        bench_spawn_drain_100,
        bench_drain_empty,
        bench_concurrent_spawn,
}
criterion_main!(benches);
