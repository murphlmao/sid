//! Tiny job queue: spawn async work; deliver typed results to the App via a channel.
//!
//! `T` is bounded `Send + Clone + 'static` so results can be both stored in the
//! completions vec ([`JobQueue::drain_completed`]) and sent through the oneshot
//! ([`JobHandle::await_result`]) without re-running the future.
//!
//! # Example
//!
//! ```no_run
//! use sid_job::JobQueue;
//!
//! # async fn example() {
//! let queue: JobQueue<i32> = JobQueue::new();
//! let handle = queue.spawn(async { 42 });
//! let result = handle.await_result().await.unwrap();
//! assert_eq!(result, 42);
//! # }
//! ```

use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Errors that can be returned from a job result.
///
/// # Example
///
/// ```
/// use sid_job::JobError;
/// let e = JobError::Cancelled;
/// assert_eq!(format!("{e}"), "job cancelled");
/// ```
#[derive(Clone, Debug, thiserror::Error)]
pub enum JobError {
    /// The spawned future panicked.
    ///
    /// In v1, panics are not actively caught (tokio surfaces them via
    /// [`JoinHandle`] which we currently drop). This variant exists so callers
    /// can pattern-match future expansions without breaking changes.
    #[error("job panicked")]
    Panic,

    /// The job was cancelled, or the oneshot receiver was dropped before the
    /// task completed.
    #[error("job cancelled")]
    Cancelled,
}

/// A queue that spawns async jobs and collects their results.
///
/// Results are stored in an internal completions buffer that can be polled
/// with [`JobQueue::drain_completed`] from the App event loop, and also
/// delivered directly to a [`JobHandle`] via a oneshot channel.
///
/// # Example
///
/// ```no_run
/// use sid_job::JobQueue;
///
/// # async fn example() {
/// let queue: JobQueue<String> = JobQueue::new();
/// let handle = queue.spawn(async { "hello".to_string() });
/// let result = handle.await_result().await.unwrap();
/// assert_eq!(result, "hello");
/// # }
/// ```
pub struct JobQueue<T: Send + Clone + 'static> {
    completions: Arc<Mutex<Vec<Result<T, JobError>>>>,
}

impl<T: Send + Clone + 'static> Default for JobQueue<T> {
    /// Create a new empty `JobQueue` using the `Default` trait.
    ///
    /// Equivalent to [`JobQueue::new`].
    ///
    /// # Example
    ///
    /// ```
    /// use sid_job::JobQueue;
    /// let queue: JobQueue<i32> = JobQueue::default();
    /// // drain_completed on a fresh queue is always empty.
    /// assert!(queue.drain_completed().is_empty());
    /// ```
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + Clone + 'static> JobQueue<T> {
    /// Create a new empty `JobQueue`.
    ///
    /// # Example
    ///
    /// ```
    /// use sid_job::JobQueue;
    /// let queue: JobQueue<i32> = JobQueue::new();
    /// ```
    pub fn new() -> Self {
        Self { completions: Arc::new(Mutex::new(Vec::new())) }
    }

    /// Spawn a future as a job. Returns a [`JobHandle`] that resolves to the
    /// result when the future completes.
    ///
    /// The result is also pushed into the completions buffer so the App can
    /// poll for finished work via [`JobQueue::drain_completed`].
    ///
    /// # Example
    ///
    /// ```no_run
    /// use sid_job::JobQueue;
    ///
    /// # async fn example() {
    /// let queue: JobQueue<u32> = JobQueue::new();
    /// let handle = queue.spawn(async { 7u32 });
    /// let v = handle.await_result().await.unwrap();
    /// assert_eq!(v, 7);
    /// # }
    /// ```
    pub fn spawn<F>(&self, fut: F) -> JobHandle<T>
    where
        F: Future<Output = T> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel::<Result<T, JobError>>();
        let completions = Arc::clone(&self.completions);
        let join: JoinHandle<()> = tokio::spawn(async move {
            let value = fut.await;
            let ok: Result<T, JobError> = Ok(value);
            completions.lock().unwrap().push(ok.clone());
            let _ = tx.send(ok);
        });
        JobHandle { rx: Some(rx), _join: join }
    }

    /// Drain all results that have completed since the last call. Non-blocking.
    ///
    /// Each result is returned at most once — the internal buffer is cleared
    /// on every call, so subsequent calls will return only jobs that completed
    /// after the previous drain.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use sid_job::JobQueue;
    ///
    /// # async fn example() {
    /// let queue: JobQueue<i32> = JobQueue::new();
    /// let _ = queue.spawn(async { 1 });
    /// let _ = queue.spawn(async { 2 });
    /// tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    /// let results = queue.drain_completed();
    /// assert_eq!(results.len(), 2);
    /// # }
    /// ```
    pub fn drain_completed(&self) -> Vec<Result<T, JobError>> {
        let mut g = self.completions.lock().unwrap();
        std::mem::take(&mut *g)
    }
}

/// A handle to a spawned job.
///
/// Call [`JobHandle::await_result`] to wait for the job to finish and retrieve
/// its result. Dropping the handle without awaiting is safe — the underlying
/// future continues running on the Tokio runtime.
///
/// # Example
///
/// ```no_run
/// use sid_job::JobQueue;
///
/// # async fn example() {
/// let queue: JobQueue<String> = JobQueue::new();
/// let handle = queue.spawn(async { "done".to_string() });
/// // Drop the handle without awaiting — job still runs on the runtime.
/// drop(handle);
/// // Results are still accessible via drain_completed.
/// tokio::time::sleep(std::time::Duration::from_millis(10)).await;
/// let results = queue.drain_completed();
/// assert_eq!(results.len(), 1);
/// # }
/// ```
pub struct JobHandle<T: Send + Clone + 'static> {
    rx: Option<oneshot::Receiver<Result<T, JobError>>>,
    _join: JoinHandle<()>,
}

impl<T: Send + Clone + 'static> JobHandle<T> {
    /// Await the job's result. Consumes the handle.
    ///
    /// Returns [`JobError::Cancelled`] if the internal channel was dropped
    /// before the task sent its result.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use sid_job::JobQueue;
    ///
    /// # async fn example() {
    /// let queue: JobQueue<i32> = JobQueue::new();
    /// let handle = queue.spawn(async { 99 });
    /// assert_eq!(handle.await_result().await.unwrap(), 99);
    /// # }
    /// ```
    pub async fn await_result(mut self) -> Result<T, JobError> {
        match self.rx.take() {
            Some(rx) => rx.await.unwrap_or(Err(JobError::Cancelled)),
            None => Err(JobError::Cancelled),
        }
    }
}
