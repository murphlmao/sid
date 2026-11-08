//! Editor-launch seam: open a path in the user's editor and optionally await its exit.
//!
//! Concrete Linux behaviour (`$VISUAL`/`$EDITOR`, terminal vs GUI editors, `xdg-open`)
//! lives in `sid-os`; this module is only the trait, its error, and a test fake.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, atomic::AtomicBool, atomic::Ordering};

/// How an editor process ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorExit {
    /// Exited with status 0.
    pub success: bool,
    /// Exit code, when the OS reported one.
    pub code: Option<i32>,
}

/// Editor launch / wait failures.
#[derive(Debug, thiserror::Error)]
pub enum EditorError {
    /// Nothing usable to launch.
    #[error("no editor available: {0}")]
    NoEditor(String),
    /// The process could not be started.
    #[error("could not start editor: {0}")]
    Spawn(String),
    /// Waiting on the process failed.
    #[error("waiting for editor failed: {0}")]
    Wait(String),
    /// The launch path (e.g. `xdg-open`) hands off and cannot report when the user is done.
    #[error("this launch method cannot report when editing finished")]
    NotAwaitable,
}

/// A running editor. Blocking calls: run `wait` on a worker thread.
pub trait EditorHandle: Send {
    /// Whether exit can be observed. `false` for hand-off launchers such as `xdg-open`
    /// (the caller must then end the session some other way, e.g. a "Done" button).
    fn awaitable(&self) -> bool;
    /// Non-blocking exit check. `Err(NotAwaitable)` when [`Self::awaitable`] is false.
    fn try_wait(&mut self) -> Result<Option<EditorExit>, EditorError>;
    /// Block until the editor exits. `Err(NotAwaitable)` when [`Self::awaitable`] is false.
    fn wait(&mut self) -> Result<EditorExit, EditorError>;
}

/// Opens files in the user's editor.
pub trait EditorLauncher: Send + Sync {
    /// Launch an editor on `path` and return a handle to it.
    fn open(&self, path: &Path) -> Result<Box<dyn EditorHandle>, EditorError>;
}

/// Test fake: records opened paths; the test ends the "editor" with [`Self::finish_all`].
#[derive(Clone, Default)]
pub struct FakeEditorLauncher {
    opened: Arc<Mutex<Vec<PathBuf>>>,
    done: Arc<AtomicBool>,
    /// When true, handles report `awaitable() == false`.
    pub hand_off: bool,
}

impl FakeEditorLauncher {
    /// A fake whose handles are awaitable and not yet finished.
    pub fn new() -> Self {
        Self::default()
    }
    /// Paths passed to `open`, in order.
    pub fn opened(&self) -> Vec<PathBuf> {
        self.opened.lock().unwrap().clone()
    }
    /// Make every handle report exit (success).
    pub fn finish_all(&self) {
        self.done.store(true, Ordering::SeqCst);
    }
}

struct FakeHandle {
    done: Arc<AtomicBool>,
    hand_off: bool,
}

impl EditorHandle for FakeHandle {
    fn awaitable(&self) -> bool {
        !self.hand_off
    }
    fn try_wait(&mut self) -> Result<Option<EditorExit>, EditorError> {
        if self.hand_off {
            return Err(EditorError::NotAwaitable);
        }
        Ok(self.done.load(Ordering::SeqCst).then_some(EditorExit {
            success: true,
            code: Some(0),
        }))
    }
    fn wait(&mut self) -> Result<EditorExit, EditorError> {
        loop {
            if let Some(x) = self.try_wait()? {
                return Ok(x);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

impl EditorLauncher for FakeEditorLauncher {
    fn open(&self, path: &Path) -> Result<Box<dyn EditorHandle>, EditorError> {
        self.opened.lock().unwrap().push(path.to_path_buf());
        Ok(Box::new(FakeHandle {
            done: self.done.clone(),
            hand_off: self.hand_off,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_records_and_finishes() {
        let l = FakeEditorLauncher::new();
        let mut h = l.open(Path::new("/a/b")).unwrap();
        assert_eq!(l.opened(), [PathBuf::from("/a/b")]);
        assert_eq!(h.try_wait().unwrap(), None);
        l.finish_all();
        assert!(h.wait().unwrap().success);
    }

    #[test]
    fn hand_off_handles_refuse_to_wait() {
        let l = FakeEditorLauncher {
            hand_off: true,
            ..Default::default()
        };
        let mut h = l.open(Path::new("x")).unwrap();
        assert!(!h.awaitable());
        assert!(matches!(h.wait(), Err(EditorError::NotAwaitable)));
    }
}
