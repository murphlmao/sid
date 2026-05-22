//! Workspaces tab widget — full implementation for Plan 2.
//!
//! The module is split into:
//! - [`WorkspacesState`] — pure-Rust, testable state (tree + right-pane sub-views).
//! - [`WorkspacesWidget`] — thin `Widget` wrapper that delegates event handling
//!   to the state and rendering to the TUI layer.
//!
//! # Sub-view routing
//!
//! The right pane shows one of six sub-views represented by [`RightPane`].
//! Tab / Shift+Tab in the widget cycles through them.
//!
//! # Git provider caching
//!
//! Per-workspace git providers are cached in `open_repos`: a `HashMap` from
//! absolute path to `Arc<Mutex<Box<dyn GitProvider>>>`.
//!
//! **Option A (chosen):** wrap each opened repo in `Arc<Mutex<…>>` and acquire
//! short-term locks for reads (`list_branches`, `status`, etc.) and mutations
//! (`checkout_branch`, `commit`). This is preferable to re-opening each time
//! because:
//!   1. `git2::Repository::open` is not free (traverses `.git`).
//!   2. It gives a stable locking point for the borrow story (`&mut self` on
//!      `checkout_branch` is satisfied by `Mutex::lock`).

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use sid_core::adapters::git::{Branch, CommitInfo, DiffEntry, GitProvider, GitStatus};
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, FooterHint, RenderTarget, Widget, WidgetId};
use sid_core::workspace_metadata::{WorkspaceAction, WorkspaceKind};
use sid_store::Workspace;
use sid_ui::Theme;

// ─── EditorRunner trait ───────────────────────────────────────────────────────

/// Abstraction over spawning an external editor to write a commit message.
///
/// The real implementation:
/// 1. Saves and exits the alternate screen.
/// 2. Disables raw mode.
/// 3. Spawns `$EDITOR <tmp_file>` with inherited stdin/stdout.
/// 4. Waits for editor exit.
/// 5. Reads the file content.
/// 6. Re-enters alternate screen + enables raw mode.
///
/// Tests inject a [`MockEditorRunner`] that simulates a pre-written message
/// without actually spawning any process.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspaces::EditorRunner;
///
/// struct AlwaysEmpty;
///
/// impl EditorRunner for AlwaysEmpty {
///     fn run_editor(&self) -> Result<String, String> {
///         Ok(String::new())
///     }
/// }
///
/// let runner = AlwaysEmpty;
/// assert_eq!(runner.run_editor().unwrap(), "");
/// ```
pub trait EditorRunner: Send + Sync {
    /// Launch the editor and return the content written to the temp file.
    ///
    /// Returns `Err(String)` if the editor could not be started, the user
    /// discarded the message, or the temp file could not be read.
    fn run_editor(&self) -> Result<String, String>;
}

/// A mock `EditorRunner` for tests that returns a pre-set message.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspaces::{EditorRunner, MockEditorRunner};
///
/// let runner = MockEditorRunner::new("feat: add thing".into());
/// let msg = runner.run_editor().unwrap();
/// assert_eq!(msg, "feat: add thing");
/// ```
pub struct MockEditorRunner {
    message: String,
    should_fail: bool,
}

impl MockEditorRunner {
    /// Create a runner that returns `message` on `run_editor`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::workspaces::MockEditorRunner;
    ///
    /// let r = MockEditorRunner::new("fix: correct thing".into());
    /// assert!(!r.will_fail());
    /// ```
    pub fn new(message: String) -> Self {
        Self {
            message,
            should_fail: false,
        }
    }

    /// Create a runner that fails on `run_editor`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::workspaces::{EditorRunner, MockEditorRunner};
    ///
    /// let r = MockEditorRunner::failing("editor not found".into());
    /// assert!(r.run_editor().is_err());
    /// ```
    pub fn failing(error: String) -> Self {
        Self {
            message: error,
            should_fail: true,
        }
    }

    /// Whether this runner is configured to fail.
    pub fn will_fail(&self) -> bool {
        self.should_fail
    }
}

impl EditorRunner for MockEditorRunner {
    fn run_editor(&self) -> Result<String, String> {
        if self.should_fail {
            Err(self.message.clone())
        } else {
            Ok(self.message.clone())
        }
    }
}

/// The real editor runner — suspends TUI, spawns `$EDITOR`, restores TUI.
///
/// Only available in non-test code; tests use [`MockEditorRunner`].
pub struct SystemEditorRunner;

impl EditorRunner for SystemEditorRunner {
    /// Launch `$EDITOR` (or `$VISUAL`, or `vi` as fallback) with a temp file,
    /// suspend the TUI, wait for exit, and return the file's contents.
    ///
    /// # Errors
    ///
    /// Returns `Err(String)` if:
    /// - The temp file cannot be created.
    /// - The editor binary cannot be spawned.
    /// - Restoring the terminal fails (logged but not fatal).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use sid_widgets::workspaces::{EditorRunner, SystemEditorRunner};
    ///
    /// // This would actually launch $EDITOR in a terminal:
    /// let result = SystemEditorRunner.run_editor();
    /// match result {
    ///     Ok(msg) => println!("message: {msg}"),
    ///     Err(e) => eprintln!("editor error: {e}"),
    /// }
    /// ```
    fn run_editor(&self) -> Result<String, String> {
        use std::io::Write;
        use std::process::Command;

        // 1. Create a temp file for the commit message
        let tmp_path = std::env::temp_dir().join(format!("sid-COMMIT_EDITMSG-{}", uuid_simple()));
        {
            let mut f =
                std::fs::File::create(&tmp_path).map_err(|e| format!("create temp file: {e}"))?;
            writeln!(f).map_err(|e| format!("write temp file: {e}"))?;
        }

        // 2. Suspend TUI
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);

        // 3. Spawn $EDITOR
        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| "vi".into());
        let status = Command::new(&editor)
            .arg(&tmp_path)
            .status()
            .map_err(|e| format!("spawn editor '{editor}': {e}"))?;

        // 4. Re-enter TUI
        let _ = crossterm::terminal::enable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen);

        if !status.success() {
            return Err(format!("editor exited with status: {status}"));
        }

        // 5. Read back the message
        let message =
            std::fs::read_to_string(&tmp_path).map_err(|e| format!("read temp file: {e}"))?;
        let _ = std::fs::remove_file(&tmp_path);
        Ok(message)
    }
}

/// Generate a simple pseudo-unique string for temp file naming.
fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{t:032x}")
}

// ─── ActionRunner trait ──────────────────────────────────────────────────────

/// Result of a workspace action run.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspaces::ActionResult;
///
/// let r = ActionResult { stdout: "ok\n".into(), stderr: String::new(), exit_code: 0 };
/// assert!(r.success());
/// ```
#[derive(Debug, Clone, Default)]
pub struct ActionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl ActionResult {
    /// Whether the action exited with code 0.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::workspaces::ActionResult;
    ///
    /// let ok = ActionResult { stdout: "ok".into(), stderr: String::new(), exit_code: 0 };
    /// assert!(ok.success());
    ///
    /// let fail = ActionResult { stdout: String::new(), stderr: "err".into(), exit_code: 1 };
    /// assert!(!fail.success());
    /// ```
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Abstraction over spawning a workspace action command.
///
/// The real implementation uses `tokio::process::Command` to run the action's
/// `cmd` string with `sh -c` in the workspace's cwd.
///
/// Tests inject a [`MockActionRunner`] that returns pre-canned results without
/// actually spawning any process.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use sid_widgets::workspaces::{ActionRunner, ActionResult};
///
/// struct AlwaysOk;
///
/// impl ActionRunner for AlwaysOk {
///     fn run_action(&self, cwd: &Path, cmd: &str) -> Result<ActionResult, String> {
///         Ok(ActionResult { stdout: format!("ran: {cmd}"), stderr: String::new(), exit_code: 0 })
///     }
/// }
///
/// let r = AlwaysOk.run_action(Path::new("/tmp"), "echo hi").unwrap();
/// assert!(r.success());
/// assert!(r.stdout.contains("echo hi"));
/// ```
pub trait ActionRunner: Send + Sync {
    /// Run `cmd` via `sh -c` in `cwd`. Returns stdout/stderr/exit code.
    fn run_action(&self, cwd: &Path, cmd: &str) -> Result<ActionResult, String>;
}

/// A mock `ActionRunner` for tests.
///
/// Records each invocation for assertions.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use sid_widgets::workspaces::{ActionRunner, MockActionRunner};
///
/// let runner = MockActionRunner::new(0, "output from action".into());
/// let result = runner.run_action(Path::new("/tmp"), "echo hi").unwrap();
/// assert!(result.success());
///
/// let calls = runner.calls();
/// assert_eq!(calls.len(), 1);
/// assert_eq!(calls[0].1, "echo hi");
/// ```
pub struct MockActionRunner {
    exit_code: i32,
    stdout: String,
    should_fail: bool,
    calls: Arc<Mutex<Vec<(PathBuf, String)>>>,
}

impl MockActionRunner {
    /// Create a runner that will return `exit_code` + `stdout` on each invocation.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// use sid_widgets::workspaces::{ActionRunner, MockActionRunner};
    ///
    /// let r = MockActionRunner::new(0, "ok".into());
    /// assert!(r.run_action(Path::new("/tmp"), "cmd").unwrap().success());
    /// ```
    pub fn new(exit_code: i32, stdout: String) -> Self {
        Self {
            exit_code,
            stdout,
            should_fail: false,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create a runner that always returns `Err`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// use sid_widgets::workspaces::{ActionRunner, MockActionRunner};
    ///
    /// let r = MockActionRunner::failing("spawn error".into());
    /// assert!(r.run_action(Path::new("/tmp"), "cmd").is_err());
    /// ```
    pub fn failing(error: String) -> Self {
        Self {
            exit_code: -1,
            stdout: error,
            should_fail: true,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// All recorded `(cwd, cmd)` calls so far.
    pub fn calls(&self) -> Vec<(PathBuf, String)> {
        self.calls.lock().unwrap().clone()
    }
}

impl ActionRunner for MockActionRunner {
    fn run_action(&self, cwd: &Path, cmd: &str) -> Result<ActionResult, String> {
        self.calls
            .lock()
            .unwrap()
            .push((cwd.to_path_buf(), cmd.to_string()));
        if self.should_fail {
            return Err(self.stdout.clone());
        }
        Ok(ActionResult {
            stdout: self.stdout.clone(),
            stderr: String::new(),
            exit_code: self.exit_code,
        })
    }
}

/// The real action runner — spawns via `sh -c` in the workspace cwd.
///
/// This is a synchronous wrapper; for async use, call from within a
/// `JobQueue::spawn` future.
pub struct SystemActionRunner;

impl ActionRunner for SystemActionRunner {
    /// Run `cmd` in `cwd` via `sh -c`. Captures stdout + stderr.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::path::Path;
    /// use sid_widgets::workspaces::{ActionRunner, SystemActionRunner};
    ///
    /// let result = SystemActionRunner.run_action(Path::new("/tmp"), "echo hello");
    /// assert!(result.is_ok());
    /// ```
    fn run_action(&self, cwd: &Path, cmd: &str) -> Result<ActionResult, String> {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("spawn action: {e}"))?;
        Ok(ActionResult {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }
}

// ─── Right-pane sub-view state structs ───────────────────────────────────────

/// State for the Branches sub-view.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspaces::BranchListState;
///
/// let mut s = BranchListState::new(vec![]);
/// assert!(s.branches().is_empty());
/// s.select_next(); // noop on empty
/// ```
#[derive(Debug, Clone, Default)]
pub struct BranchListState {
    branches: Vec<Branch>,
    selected: usize,
}

impl BranchListState {
    /// Create a new branch list state from a pre-fetched branch list.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::git::Branch;
    /// use sid_widgets::workspaces::BranchListState;
    ///
    /// let b = Branch { name: "main".into(), head_oid: "a".repeat(40), upstream: None, is_current: true };
    /// let s = BranchListState::new(vec![b.clone()]);
    /// assert_eq!(s.branches()[0].name, "main");
    /// ```
    pub fn new(branches: Vec<Branch>) -> Self {
        Self {
            branches,
            selected: 0,
        }
    }

    /// The loaded branch list.
    pub fn branches(&self) -> &[Branch] {
        &self.branches
    }

    /// Replace the branch list (called after a git provider refresh).
    pub fn set_branches(&mut self, branches: Vec<Branch>) {
        let prev = self.selected;
        self.branches = branches;
        self.selected = prev.min(self.branches.len().saturating_sub(1));
    }

    /// Currently selected branch index.
    pub fn selected_idx(&self) -> usize {
        self.selected
    }

    /// The currently highlighted branch, if any.
    pub fn selected_branch(&self) -> Option<&Branch> {
        self.branches.get(self.selected)
    }

    /// Move selection down (wraps).
    pub fn select_next(&mut self) {
        let n = self.branches.len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected + 1) % n;
    }

    /// Move selection up (wraps).
    pub fn select_prev(&mut self) {
        let n = self.branches.len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected + n - 1) % n;
    }
}

/// State for the Status sub-view.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::git::GitStatus;
/// use sid_widgets::workspaces::StatusListState;
///
/// let s = StatusListState::new(GitStatus { entries: vec![], is_clean: true });
/// assert!(s.status().is_clean);
/// ```
#[derive(Debug, Clone)]
pub struct StatusListState {
    status: GitStatus,
    selected: usize,
}

impl Default for StatusListState {
    fn default() -> Self {
        Self {
            status: GitStatus {
                entries: vec![],
                is_clean: true,
            },
            selected: 0,
        }
    }
}

impl StatusListState {
    /// Create from a fetched [`GitStatus`].
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::git::GitStatus;
    /// use sid_widgets::workspaces::StatusListState;
    ///
    /// let st = StatusListState::new(GitStatus { entries: vec![], is_clean: true });
    /// assert!(st.status().is_clean);
    /// ```
    pub fn new(status: GitStatus) -> Self {
        Self {
            status,
            selected: 0,
        }
    }

    /// The current status snapshot.
    pub fn status(&self) -> &GitStatus {
        &self.status
    }

    /// Replace the status (called after a refresh job completes).
    pub fn set_status(&mut self, status: GitStatus) {
        let prev = self.selected;
        self.selected = prev.min(status.entries.len().saturating_sub(1));
        self.status = status;
    }

    /// Currently selected entry index.
    pub fn selected_idx(&self) -> usize {
        self.selected
    }

    /// Move selection down (wraps).
    pub fn select_next(&mut self) {
        let n = self.status.entries.len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected + 1) % n;
    }

    /// Move selection up (wraps).
    pub fn select_prev(&mut self) {
        let n = self.status.entries.len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected + n - 1) % n;
    }
}

/// State for the commit log sub-view (paginated).
///
/// # Examples
///
/// ```
/// use sid_widgets::workspaces::LogListState;
///
/// let s = LogListState::new(vec![], 50);
/// assert!(s.entries().is_empty());
/// assert_eq!(s.page_size(), 50);
/// ```
#[derive(Debug, Clone)]
pub struct LogListState {
    entries: Vec<CommitInfo>,
    selected: usize,
    page_size: usize,
    /// OID cursor for fetching the next page (last OID of current page).
    next_cursor: Option<String>,
    /// Stack of page cursors enabling backward navigation.
    cursor_stack: Vec<Option<String>>,
}

impl LogListState {
    /// Create with an initial set of entries and a page size.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::workspaces::LogListState;
    ///
    /// let s = LogListState::new(vec![], 50);
    /// assert_eq!(s.page_size(), 50);
    /// ```
    pub fn new(entries: Vec<CommitInfo>, page_size: usize) -> Self {
        Self {
            entries,
            selected: 0,
            page_size: page_size.max(1),
            next_cursor: None,
            cursor_stack: Vec::new(),
        }
    }

    /// Current page entries.
    pub fn entries(&self) -> &[CommitInfo] {
        &self.entries
    }

    /// Page size.
    pub fn page_size(&self) -> usize {
        self.page_size
    }

    /// Replace the entry list (after a load operation).
    pub fn set_entries(&mut self, entries: Vec<CommitInfo>) {
        // Set the next-page cursor to the last OID so callers can fetch page+1
        self.next_cursor = entries.last().map(|c| c.oid.clone());
        let prev = self.selected;
        self.selected = prev.min(entries.len().saturating_sub(1));
        self.entries = entries;
    }

    /// Cursor for fetching the NEXT page (`from_oid` to pass to `commit_log`).
    pub fn next_page_cursor(&self) -> Option<&str> {
        self.next_cursor.as_deref()
    }

    /// Push the current cursor onto the stack (before fetching page N+1).
    pub fn push_cursor(&mut self, cursor: Option<String>) {
        self.cursor_stack.push(cursor);
    }

    /// Pop a cursor from the stack (for going back to page N-1).
    pub fn pop_cursor(&mut self) -> Option<Option<String>> {
        self.cursor_stack.pop()
    }

    /// Currently selected entry index.
    pub fn selected_idx(&self) -> usize {
        self.selected
    }

    /// Move selection down (wraps).
    pub fn select_next(&mut self) {
        let n = self.entries.len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected + 1) % n;
    }

    /// Move selection up (wraps).
    pub fn select_prev(&mut self) {
        let n = self.entries.len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected + n - 1) % n;
    }
}

impl Default for LogListState {
    fn default() -> Self {
        Self::new(vec![], 50)
    }
}

/// State for the diff sub-view.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspaces::DiffViewState;
///
/// let s = DiffViewState::new(vec![], false);
/// assert!(!s.staged());
/// assert!(s.entries().is_empty());
/// ```
#[derive(Debug, Clone, Default)]
pub struct DiffViewState {
    entries: Vec<DiffEntry>,
    selected_file: usize,
    scroll_offset: usize,
    staged: bool,
}

impl DiffViewState {
    /// Create with diff entries and staging mode.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::workspaces::DiffViewState;
    ///
    /// let s = DiffViewState::new(vec![], true);
    /// assert!(s.staged());
    /// ```
    pub fn new(entries: Vec<DiffEntry>, staged: bool) -> Self {
        Self {
            entries,
            selected_file: 0,
            scroll_offset: 0,
            staged,
        }
    }

    /// Whether we're viewing staged (index-vs-HEAD) or unstaged (workdir-vs-index) diffs.
    pub fn staged(&self) -> bool {
        self.staged
    }

    /// Toggle between staged and unstaged view.
    pub fn toggle_staged(&mut self) {
        self.staged = !self.staged;
        self.selected_file = 0;
        self.scroll_offset = 0;
    }

    /// The current diff entries (one per file).
    pub fn entries(&self) -> &[DiffEntry] {
        &self.entries
    }

    /// Replace entries (after re-fetching).
    pub fn set_entries(&mut self, entries: Vec<DiffEntry>) {
        let prev = self.selected_file;
        self.selected_file = prev.min(entries.len().saturating_sub(1));
        self.scroll_offset = 0;
        self.entries = entries;
    }

    /// Currently selected file index.
    pub fn selected_file(&self) -> usize {
        self.selected_file
    }

    /// Current scroll offset within the selected file's patch.
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Navigate to the next file.
    pub fn next_file(&mut self) {
        let n = self.entries.len();
        if n == 0 {
            return;
        }
        self.selected_file = (self.selected_file + 1) % n;
        self.scroll_offset = 0;
    }

    /// Navigate to the previous file.
    pub fn prev_file(&mut self) {
        let n = self.entries.len();
        if n == 0 {
            return;
        }
        self.selected_file = (self.selected_file + n - 1) % n;
        self.scroll_offset = 0;
    }

    /// Scroll down within the current file's patch.
    pub fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(1);
    }

    /// Scroll up within the current file's patch.
    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    /// Current file's patch lines, respecting the 200-line display cap.
    pub fn visible_patch_lines(&self) -> Vec<&str> {
        const MAX_LINES: usize = 200;
        let entry = match self.entries.get(self.selected_file) {
            Some(e) => e,
            None => return Vec::new(),
        };
        entry
            .patch
            .lines()
            .skip(self.scroll_offset)
            .take(MAX_LINES)
            .collect()
    }
}

/// State machine for the commit drafter.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspaces::CommitDraftState;
///
/// let s = CommitDraftState::default();
/// assert!(s.is_idle());
/// ```
#[derive(Debug, Clone, Default)]
pub struct CommitDraftState {
    phase: CommitDraftPhase,
    /// Message draft (populated after editor exits).
    draft_message: String,
    /// OID of the committed revision (if successful).
    committed_oid: Option<String>,
    /// Error message if committing failed.
    error: Option<String>,
}

/// Phase of the commit drafter state machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CommitDraftPhase {
    #[default]
    Idle,
    EditingMessage,
    Committing,
    Done,
    Failed,
}

impl CommitDraftState {
    /// Whether the drafter is idle (not in any active phase).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::workspaces::CommitDraftState;
    ///
    /// let s = CommitDraftState::default();
    /// assert!(s.is_idle());
    /// ```
    pub fn is_idle(&self) -> bool {
        self.phase == CommitDraftPhase::Idle
    }

    /// Transition to `EditingMessage` phase.
    pub fn start_editing(&mut self) {
        self.phase = CommitDraftPhase::EditingMessage;
        self.draft_message.clear();
        self.committed_oid = None;
        self.error = None;
    }

    /// Called when the editor has exited with a message draft.
    pub fn finish_editing(&mut self, message: String) {
        self.draft_message = message;
        self.phase = CommitDraftPhase::Committing;
    }

    /// Called when the commit job has completed successfully.
    pub fn mark_done(&mut self, oid: String) {
        self.committed_oid = Some(oid);
        self.phase = CommitDraftPhase::Done;
    }

    /// Called when the commit job has failed.
    pub fn mark_failed(&mut self, err: String) {
        self.error = Some(err);
        self.phase = CommitDraftPhase::Failed;
    }

    /// Reset back to idle (after user dismisses result).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The draft message set after editor exit.
    pub fn draft_message(&self) -> &str {
        &self.draft_message
    }

    /// The committed OID (if Done).
    pub fn committed_oid(&self) -> Option<&str> {
        self.committed_oid.as_deref()
    }

    /// Error message (if Failed).
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Current phase.
    pub fn phase(&self) -> &CommitDraftPhase {
        &self.phase
    }
}

/// State for the run-action menu.
///
/// # Examples
///
/// ```
/// use sid_core::workspace_metadata::WorkspaceAction;
/// use sid_widgets::workspaces::ActionListState;
///
/// let s = ActionListState::new(vec![]);
/// assert!(s.actions().is_empty());
/// ```
#[derive(Debug, Clone, Default)]
pub struct ActionListState {
    actions: Vec<WorkspaceAction>,
    selected: usize,
}

impl ActionListState {
    /// Create from a list of actions (sourced from workspace metadata).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::workspace_metadata::WorkspaceAction;
    /// use sid_widgets::workspaces::ActionListState;
    ///
    /// let a = WorkspaceAction { label: "Build".into(), cmd: "cargo build".into(), key: Some('b') };
    /// let s = ActionListState::new(vec![a]);
    /// assert_eq!(s.actions().len(), 1);
    /// ```
    pub fn new(actions: Vec<WorkspaceAction>) -> Self {
        Self {
            actions,
            selected: 0,
        }
    }

    /// The action list.
    pub fn actions(&self) -> &[WorkspaceAction] {
        &self.actions
    }

    /// Currently selected action index.
    pub fn selected_idx(&self) -> usize {
        self.selected
    }

    /// The currently highlighted action, if any.
    pub fn selected_action(&self) -> Option<&WorkspaceAction> {
        self.actions.get(self.selected)
    }

    /// Move selection down (wraps).
    pub fn select_next(&mut self) {
        let n = self.actions.len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected + 1) % n;
    }

    /// Move selection up (wraps).
    pub fn select_prev(&mut self) {
        let n = self.actions.len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected + n - 1) % n;
    }
}

// ─── WsFocus enum ────────────────────────────────────────────────────────────

/// Which pane in the Workspaces tab currently owns keyboard input.
///
/// `Tab` cycles 2-way: `Tree ↔ SubView`. The accent border tracks the
/// focused pane.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspaces::WsFocus;
/// assert_ne!(WsFocus::Tree, WsFocus::SubView);
/// assert_eq!(WsFocus::default(), WsFocus::Tree);
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WsFocus {
    /// The left-hand workspace tree.
    #[default]
    Tree,
    /// The right-hand active sub-view ([`RightPane`]).
    SubView,
}

impl WsFocus {
    /// Cycle to the next focus (Tab).
    pub fn next(self) -> Self {
        match self {
            WsFocus::Tree => WsFocus::SubView,
            WsFocus::SubView => WsFocus::Tree,
        }
    }
    /// Cycle to the previous focus (Shift+Tab).
    pub fn prev(self) -> Self {
        // 2-way: prev == next.
        self.next()
    }
}

// ─── RightPane enum ──────────────────────────────────────────────────────────

/// The currently-active sub-view in the right pane of the Workspaces tab.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspaces::RightPane;
///
/// let pane = RightPane::default();
/// assert!(matches!(pane, RightPane::Branches(_)));
/// ```
#[derive(Debug, Clone)]
pub enum RightPane {
    Branches(BranchListState),
    Status(StatusListState),
    Log(LogListState),
    Diff(DiffViewState),
    Commit(CommitDraftState),
    Actions(ActionListState),
}

impl Default for RightPane {
    /// Default right pane is the Branches sub-view (first in cycle).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::workspaces::RightPane;
    ///
    /// let p = RightPane::default();
    /// assert!(matches!(p, RightPane::Branches(_)));
    /// ```
    fn default() -> Self {
        RightPane::Branches(BranchListState::default())
    }
}

impl RightPane {
    /// Cycle to the next sub-view.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::workspaces::RightPane;
    ///
    /// let mut p = RightPane::default(); // Branches
    /// p.cycle_next();
    /// assert!(matches!(p, RightPane::Status(_)));
    /// ```
    pub fn cycle_next(&mut self) {
        *self = match self {
            RightPane::Branches(_) => RightPane::Status(StatusListState::default()),
            RightPane::Status(_) => RightPane::Log(LogListState::default()),
            RightPane::Log(_) => RightPane::Diff(DiffViewState::default()),
            RightPane::Diff(_) => RightPane::Commit(CommitDraftState::default()),
            RightPane::Commit(_) => RightPane::Actions(ActionListState::default()),
            RightPane::Actions(_) => RightPane::Branches(BranchListState::default()),
        };
    }

    /// Cycle to the previous sub-view.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::workspaces::RightPane;
    ///
    /// let mut p = RightPane::default(); // Branches
    /// p.cycle_prev();
    /// assert!(matches!(p, RightPane::Actions(_)));
    /// ```
    pub fn cycle_prev(&mut self) {
        *self = match self {
            RightPane::Branches(_) => RightPane::Actions(ActionListState::default()),
            RightPane::Actions(_) => RightPane::Commit(CommitDraftState::default()),
            RightPane::Commit(_) => RightPane::Diff(DiffViewState::default()),
            RightPane::Diff(_) => RightPane::Log(LogListState::default()),
            RightPane::Log(_) => RightPane::Status(StatusListState::default()),
            RightPane::Status(_) => RightPane::Branches(BranchListState::default()),
        };
    }

    /// Human-readable label for the current sub-view.
    pub fn label(&self) -> &'static str {
        match self {
            RightPane::Branches(_) => "Branches",
            RightPane::Status(_) => "Status",
            RightPane::Log(_) => "Log",
            RightPane::Diff(_) => "Diff",
            RightPane::Commit(_) => "Commit",
            RightPane::Actions(_) => "Actions",
        }
    }
}

// ─── WorkspacesState ─────────────────────────────────────────────────────────

/// Pure-Rust state for the Workspaces tab. Separated from the widget so it can
/// be unit-tested without a TUI runtime.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_core::workspace_metadata::WorkspaceKind;
/// use sid_store::Workspace;
/// use sid_widgets::workspaces::WorkspacesState;
///
/// let ws = Workspace {
///     path: PathBuf::from("/vcs/myrepo"),
///     name: "myrepo".into(),
///     kind: WorkspaceKind::Repo,
///     manifest_hash: 0,
///     last_seen: 0,
///     parent: None,
/// };
/// let s = WorkspacesState::new(vec![ws]);
/// assert_eq!(s.selected_path().unwrap().to_string_lossy(), "/vcs/myrepo");
/// ```
pub struct WorkspacesState {
    workspaces: Vec<Workspace>,
    expanded: HashSet<PathBuf>,
    selected_visible_idx: usize,
    right_pane: RightPane,
}

impl WorkspacesState {
    /// Create from a list of workspaces. Selection starts at the first visible item.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::Workspace;
    /// use sid_widgets::workspaces::WorkspacesState;
    ///
    /// let s = WorkspacesState::new(vec![]);
    /// assert!(s.selected_path().is_none());
    /// assert_eq!(s.visible_count(), 0);
    /// ```
    pub fn new(workspaces: Vec<Workspace>) -> Self {
        Self {
            workspaces,
            expanded: HashSet::new(),
            selected_visible_idx: 0,
            right_pane: RightPane::default(),
        }
    }

    /// All workspaces (including hidden children).
    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    /// Workspaces currently visible in the tree.
    ///
    /// Children of a collapsed umbrella are hidden. Children of an expanded
    /// umbrella are shown immediately after their parent.
    pub fn visible_workspaces(&self) -> Vec<&Workspace> {
        let mut out = Vec::new();
        for w in &self.workspaces {
            match &w.parent {
                None => out.push(w),
                Some(p) if self.expanded.contains(p) => out.push(w),
                _ => {}
            }
        }
        out
    }

    /// Number of currently visible items.
    pub fn visible_count(&self) -> usize {
        self.visible_workspaces().len()
    }

    /// Path of the currently selected workspace, or `None` if the list is empty.
    pub fn selected_path(&self) -> Option<&Path> {
        self.visible_workspaces()
            .get(self.selected_visible_idx)
            .map(|w| w.path.as_path())
    }

    /// The currently selected workspace record, or `None`.
    pub fn selected_workspace(&self) -> Option<&Workspace> {
        self.visible_workspaces()
            .get(self.selected_visible_idx)
            .copied()
    }

    /// Move selection to the next visible item (wraps).
    pub fn select_next(&mut self) {
        let n = self.visible_count();
        if n == 0 {
            return;
        }
        self.selected_visible_idx = (self.selected_visible_idx + 1) % n;
    }

    /// Move selection to the previous visible item (wraps).
    pub fn select_prev(&mut self) {
        let n = self.visible_count();
        if n == 0 {
            return;
        }
        self.selected_visible_idx = (self.selected_visible_idx + n - 1) % n;
    }

    /// Expand or collapse the selected umbrella workspace. No-op on non-umbrella.
    pub fn toggle_expand_selected(&mut self) {
        let path = match self.visible_workspaces().get(self.selected_visible_idx) {
            Some(w) if w.kind == WorkspaceKind::Umbrella => w.path.clone(),
            _ => return,
        };
        if self.expanded.contains(&path) {
            self.expanded.remove(&path);
            // Clamp selection in case children were selected before collapse
            let n = self.visible_count();
            if n > 0 {
                self.selected_visible_idx = self.selected_visible_idx.min(n - 1);
            }
        } else {
            self.expanded.insert(path);
        }
    }

    /// Reference to the active right-pane sub-view.
    pub fn right_pane(&self) -> &RightPane {
        &self.right_pane
    }

    /// Mutable reference to the active right-pane sub-view.
    pub fn right_pane_mut(&mut self) -> &mut RightPane {
        &mut self.right_pane
    }

    /// Cycle to the next right-pane sub-view (Tab).
    pub fn cycle_pane_next(&mut self) {
        self.right_pane.cycle_next();
    }

    /// Cycle to the previous right-pane sub-view (Shift+Tab).
    pub fn cycle_pane_prev(&mut self) {
        self.right_pane.cycle_prev();
    }
}

// ─── WorkspacesWidget ─────────────────────────────────────────────────────────

/// Tab widget for the Workspaces tab.
///
/// Holds a [`WorkspacesState`] and an optional git provider factory
/// (`Arc<dyn GitProvider>`) used to open per-workspace repo handles.
/// Per-repo handles are cached in `open_repos` as `Arc<Mutex<Box<dyn
/// GitProvider>>>` (Option A from the module doc).
///
/// # Examples
///
/// ```
/// use sid_core::widget::Widget;
/// use sid_widgets::WorkspacesWidget;
///
/// let w = WorkspacesWidget::new(vec![], None);
/// assert_eq!(w.id().as_str(), "workspaces.root");
/// assert_eq!(w.title(), "Workspaces");
/// ```
pub struct WorkspacesWidget {
    state: WorkspacesState,
    id: WidgetId,
    /// Factory for opening git providers per workspace.
    git_factory: Option<Arc<dyn GitProvider>>,
    /// Cached per-workspace git providers. Wrapped in Arc<Mutex<>> (Option A).
    open_repos: HashMap<PathBuf, Arc<Mutex<Box<dyn GitProvider>>>>,
    /// Strict pane-focus marker. Tab toggles between [`WsFocus::Tree`] and
    /// [`WsFocus::SubView`].
    focused_pane: WsFocus,
}

impl WorkspacesWidget {
    /// Create a new `WorkspacesWidget`.
    ///
    /// - `workspaces`: initial workspace list (loaded from the store).
    /// - `git_factory`: optional git factory for opening per-repo handles.
    ///   Pass `None` for tests or when no git support is needed.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::Widget;
    /// use sid_widgets::WorkspacesWidget;
    ///
    /// let w = WorkspacesWidget::new(vec![], None);
    /// assert_eq!(w.id().as_str(), "workspaces.root");
    /// ```
    pub fn new(workspaces: Vec<Workspace>, git_factory: Option<Arc<dyn GitProvider>>) -> Self {
        Self {
            state: WorkspacesState::new(workspaces),
            id: WidgetId::new("workspaces.root"),
            git_factory,
            open_repos: HashMap::new(),
            focused_pane: WsFocus::default(),
        }
    }

    /// Currently-focused pane.
    pub fn focused_pane(&self) -> WsFocus {
        self.focused_pane
    }

    /// Stable string label for the focused pane.
    pub fn focused_pane_label(&self) -> &'static str {
        match self.focused_pane {
            WsFocus::Tree => "Tree",
            WsFocus::SubView => "SubView",
        }
    }

    /// Cycle focus forward (Tab).
    pub fn focus_next(&mut self) {
        self.focused_pane = self.focused_pane.next();
    }

    /// Cycle focus backward (Shift+Tab).
    pub fn focus_prev(&mut self) {
        self.focused_pane = self.focused_pane.prev();
    }

    /// Focus the pane that contains the given coordinate. No-op when the
    /// coordinate falls outside `area`.
    ///
    /// Layout mirrors [`Self::render_into_frame`]: a 30/70 horizontal split.
    /// Columns left of the 30% boundary focus [`WsFocus::Tree`]; everything
    /// else focuses [`WsFocus::SubView`].
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::layout::Rect;
    /// use sid_widgets::WorkspacesWidget;
    /// use sid_widgets::workspaces::WsFocus;
    /// let mut w = WorkspacesWidget::new(vec![], None);
    /// let area = Rect { x: 0, y: 0, width: 100, height: 24 };
    /// w.focus_at(area, 80, 5);
    /// assert_eq!(w.focused_pane(), WsFocus::SubView);
    /// w.focus_at(area, 5, 5);
    /// assert_eq!(w.focused_pane(), WsFocus::Tree);
    /// ```
    pub fn focus_at(&mut self, area: Rect, col: u16, row: u16) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        if col < area.x || col >= area.x.saturating_add(area.width) {
            return;
        }
        if row < area.y || row >= area.y.saturating_add(area.height) {
            return;
        }
        let split_col = area.x.saturating_add(area.width.saturating_mul(30) / 100);
        self.focused_pane = if col < split_col {
            WsFocus::Tree
        } else {
            WsFocus::SubView
        };
    }

    /// Borrow the inner state.
    pub fn state(&self) -> &WorkspacesState {
        &self.state
    }

    /// Mutably borrow the inner state.
    pub fn state_mut(&mut self) -> &mut WorkspacesState {
        &mut self.state
    }

    /// Render the widget into a ratatui [`Frame`]. Used by the binary's wire
    /// layer (`crates/sid/src/wire.rs`) so the Workspaces tab has the same
    /// bordered, multi-pane look as every other per-tab widget.
    ///
    /// Layout: a 30/70 horizontal split. The left pane shows the visible
    /// workspaces as a tree, bordered and titled `" Workspaces "` (accent
    /// border — the tree is the always-focused navigation surface). The right
    /// pane is a single bordered block titled with the active sub-view label
    /// (`Branches`, `Status`, `Log`, …) and is rendered with a muted border
    /// because pane-internal rendering is still Plan-2 territory.
    pub fn render_into_frame(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(area);
        let left = split[0];
        let right = split[1];

        self.render_tree(frame, left, theme);
        self.render_right_pane(frame, right, theme);
    }

    fn render_tree(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let workspaces = self.state.workspaces();
        let visible = self.state.visible_workspaces();
        let selected_path = self.state.selected_path();

        let focused = self.focused_pane == WsFocus::Tree;
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let mut title_style = Style::default().fg(theme.foreground.into());
        if focused {
            title_style = title_style.add_modifier(Modifier::BOLD);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(" Workspaces ")
            .title_style(title_style);

        if workspaces.is_empty() {
            let body = vec![
                Line::from(Span::styled(
                    "no workspaces registered yet",
                    Style::default().fg(theme.muted.into()),
                )),
                Line::from(Span::raw("")),
                Line::from(Span::styled(
                    "  sid workspace add /path/to/repo",
                    Style::default().fg(theme.foreground.into()),
                )),
                Line::from(Span::styled(
                    "  (or put repos under ~/vcs/)",
                    Style::default().fg(theme.muted.into()),
                )),
            ];
            frame.render_widget(Paragraph::new(body).block(block), rect);
            return;
        }

        let mut lines: Vec<Line<'_>> = Vec::with_capacity(visible.len());
        for w in &visible {
            let is_selected = selected_path
                .map(|p| p == w.path.as_path())
                .unwrap_or(false);
            let indent = if w.parent.is_some() { "    " } else { "" };
            let glyph = match w.kind {
                WorkspaceKind::Umbrella => '▾',
                WorkspaceKind::Repo => '·',
            };
            let marker = if is_selected { '>' } else { ' ' };
            let label = format!("{marker} {indent}{glyph} {}", w.name);
            let style = if is_selected {
                Style::default()
                    .fg(theme.background.into())
                    .bg(theme.accent_primary.into())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground.into())
            };
            lines.push(Line::from(Span::styled(label, style)));
        }
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }

    fn render_right_pane(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let label = self.state.right_pane().label();
        let title = format!(" {label} ");
        let focused = self.focused_pane == WsFocus::SubView;
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let mut title_style = Style::default().fg(theme.foreground.into());
        if focused {
            title_style = title_style.add_modifier(Modifier::BOLD);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(title)
            .title_style(title_style);

        let body: Vec<Line<'_>> = match self.state.right_pane() {
            RightPane::Branches(s) => {
                if s.branches().is_empty() {
                    vec![Line::from(Span::styled(
                        "(no branches loaded — select a workspace)",
                        Style::default().fg(theme.muted.into()),
                    ))]
                } else {
                    let sel = s.selected_idx();
                    s.branches()
                        .iter()
                        .enumerate()
                        .map(|(i, b)| {
                            let marker = if b.is_current { '●' } else { '○' };
                            let cursor = if i == sel { '>' } else { ' ' };
                            let label = format!("{cursor} {marker} {}", b.name);
                            let style = if i == sel {
                                Style::default()
                                    .fg(theme.background.into())
                                    .bg(theme.accent_primary.into())
                            } else {
                                Style::default().fg(theme.foreground.into())
                            };
                            Line::from(Span::styled(label, style))
                        })
                        .collect()
                }
            }
            RightPane::Status(s) => {
                if s.status().entries.is_empty() {
                    vec![Line::from(Span::styled(
                        if s.status().is_clean {
                            "(clean — nothing to commit)"
                        } else {
                            "(no status loaded)"
                        },
                        Style::default().fg(theme.muted.into()),
                    ))]
                } else {
                    let sel = s.selected_idx();
                    s.status()
                        .entries
                        .iter()
                        .enumerate()
                        .map(|(i, e)| {
                            let marker = if i == sel { '>' } else { ' ' };
                            let label = format!("{marker} {:?}  {}", e.kind, e.path);
                            let style = if i == sel {
                                Style::default()
                                    .fg(theme.background.into())
                                    .bg(theme.accent_primary.into())
                            } else {
                                Style::default().fg(theme.foreground.into())
                            };
                            Line::from(Span::styled(label, style))
                        })
                        .collect()
                }
            }
            RightPane::Log(s) => {
                if s.entries().is_empty() {
                    vec![Line::from(Span::styled(
                        "(no commits loaded)",
                        Style::default().fg(theme.muted.into()),
                    ))]
                } else {
                    s.entries()
                        .iter()
                        .map(|c| {
                            let short = c.oid.chars().take(8).collect::<String>();
                            Line::from(vec![
                                Span::styled(
                                    short,
                                    Style::default().fg(theme.accent_warning.into()),
                                ),
                                Span::raw("  "),
                                Span::styled(
                                    c.summary.clone(),
                                    Style::default().fg(theme.foreground.into()),
                                ),
                            ])
                        })
                        .collect()
                }
            }
            RightPane::Diff(s) => {
                if s.entries().is_empty() {
                    vec![Line::from(Span::styled(
                        if s.staged() {
                            "(no staged changes)"
                        } else {
                            "(no unstaged changes)"
                        },
                        Style::default().fg(theme.muted.into()),
                    ))]
                } else {
                    s.visible_patch_lines()
                        .into_iter()
                        .map(|l| Line::from(Span::raw(l.to_string())))
                        .collect()
                }
            }
            RightPane::Commit(s) => {
                let phase = format!("{:?}", s.phase());
                vec![
                    Line::from(Span::styled(
                        format!("phase: {phase}"),
                        Style::default().fg(theme.muted.into()),
                    )),
                    Line::from(Span::styled(
                        s.draft_message().to_string(),
                        Style::default().fg(theme.foreground.into()),
                    )),
                ]
            }
            RightPane::Actions(s) => {
                if s.actions().is_empty() {
                    vec![Line::from(Span::styled(
                        "(no actions registered for this workspace)",
                        Style::default().fg(theme.muted.into()),
                    ))]
                } else {
                    let sel = s.selected_idx();
                    s.actions()
                        .iter()
                        .enumerate()
                        .map(|(i, a)| {
                            let marker = if i == sel { '>' } else { ' ' };
                            let key = a.key.map(|c| format!(" [{c}]")).unwrap_or_default();
                            let label = format!("{marker} {}{key}  {}", a.label, a.cmd);
                            let style = if i == sel {
                                Style::default()
                                    .fg(theme.background.into())
                                    .bg(theme.accent_primary.into())
                            } else {
                                Style::default().fg(theme.foreground.into())
                            };
                            Line::from(Span::styled(label, style))
                        })
                        .collect()
                }
            }
        };
        frame.render_widget(Paragraph::new(body).block(block), rect);
    }

    /// Open (or return cached) a git provider for the given path.
    ///
    /// Returns `None` if no factory was set or opening fails.
    pub fn get_or_open_repo(&mut self, path: &Path) -> Option<Arc<Mutex<Box<dyn GitProvider>>>> {
        if self.open_repos.contains_key(path) {
            return Some(Arc::clone(self.open_repos.get(path).unwrap()));
        }
        let factory = self.git_factory.as_ref()?;
        match factory.open(path) {
            Ok(provider) => {
                let arc = Arc::new(Mutex::new(provider));
                self.open_repos.insert(path.to_path_buf(), Arc::clone(&arc));
                Some(arc)
            }
            Err(_) => None,
        }
    }

    fn handle_branch_pane_event(&mut self, chord: &sid_core::event::KeyChord) -> EventOutcome {
        use crossterm::event::KeyCode;
        match chord.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if let RightPane::Branches(ref mut s) = self.state.right_pane {
                    s.select_next();
                }
                EventOutcome::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let RightPane::Branches(ref mut s) = self.state.right_pane {
                    s.select_prev();
                }
                EventOutcome::Consumed
            }
            // 'c' = checkout selected branch
            KeyCode::Char('c') => {
                // Dispatch checkout via JobQueue is done by the App layer which
                // owns the runtime context; here we just signal the intent.
                // The App queries state.right_pane for the selected branch name.
                EventOutcome::Consumed
            }
            _ => EventOutcome::Bubble,
        }
    }

    fn handle_status_pane_event(&mut self, chord: &sid_core::event::KeyChord) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        match (chord.code, chord.mods) {
            (KeyCode::Char('j'), _) | (KeyCode::Down, _) => {
                if let RightPane::Status(ref mut s) = self.state.right_pane {
                    s.select_next();
                }
                EventOutcome::Consumed
            }
            (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
                if let RightPane::Status(ref mut s) = self.state.right_pane {
                    s.select_prev();
                }
                EventOutcome::Consumed
            }
            // Ctrl+R = refresh status (App layer handles the job)
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => EventOutcome::Consumed,
            // 'c' from status = enter commit drafter
            (KeyCode::Char('c'), KeyModifiers::NONE) => {
                let mut draft = CommitDraftState::default();
                draft.start_editing();
                self.state.right_pane = RightPane::Commit(draft);
                EventOutcome::Consumed
            }
            _ => EventOutcome::Bubble,
        }
    }

    fn handle_log_pane_event(&mut self, chord: &sid_core::event::KeyChord) -> EventOutcome {
        use crossterm::event::KeyCode;
        match chord.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if let RightPane::Log(ref mut s) = self.state.right_pane {
                    s.select_next();
                }
                EventOutcome::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let RightPane::Log(ref mut s) = self.state.right_pane {
                    s.select_prev();
                }
                EventOutcome::Consumed
            }
            // 'n' = next page, 'p' = prev page (App layer handles job dispatch)
            KeyCode::Char('n') | KeyCode::Char('p') => EventOutcome::Consumed,
            _ => EventOutcome::Bubble,
        }
    }

    fn handle_diff_pane_event(&mut self, chord: &sid_core::event::KeyChord) -> EventOutcome {
        use crossterm::event::KeyCode;
        match chord.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if let RightPane::Diff(ref mut s) = self.state.right_pane {
                    s.scroll_down();
                }
                EventOutcome::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let RightPane::Diff(ref mut s) = self.state.right_pane {
                    s.scroll_up();
                }
                EventOutcome::Consumed
            }
            KeyCode::Char('n') => {
                if let RightPane::Diff(ref mut s) = self.state.right_pane {
                    s.next_file();
                }
                EventOutcome::Consumed
            }
            KeyCode::Char('p') => {
                if let RightPane::Diff(ref mut s) = self.state.right_pane {
                    s.prev_file();
                }
                EventOutcome::Consumed
            }
            // Tab toggles staged/unstaged within Diff pane
            KeyCode::Tab => {
                if let RightPane::Diff(ref mut s) = self.state.right_pane {
                    s.toggle_staged();
                }
                EventOutcome::Consumed
            }
            _ => EventOutcome::Bubble,
        }
    }

    fn handle_commit_pane_event(&mut self, chord: &sid_core::event::KeyChord) -> EventOutcome {
        use crossterm::event::KeyCode;
        match chord.code {
            // Escape resets the drafter
            KeyCode::Esc => {
                if let RightPane::Commit(ref mut s) = self.state.right_pane {
                    s.reset();
                }
                EventOutcome::Consumed
            }
            _ => EventOutcome::Bubble,
        }
    }

    fn handle_actions_pane_event(&mut self, chord: &sid_core::event::KeyChord) -> EventOutcome {
        use crossterm::event::KeyCode;
        match chord.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if let RightPane::Actions(ref mut s) = self.state.right_pane {
                    s.select_next();
                }
                EventOutcome::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let RightPane::Actions(ref mut s) = self.state.right_pane {
                    s.select_prev();
                }
                EventOutcome::Consumed
            }
            // Enter = run selected action (App layer handles job dispatch)
            KeyCode::Enter => EventOutcome::Consumed,
            _ => EventOutcome::Bubble,
        }
    }
}

impl Default for WorkspacesWidget {
    /// Create a default `WorkspacesWidget` with no workspaces and no git factory.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::widget::Widget;
    /// use sid_widgets::WorkspacesWidget;
    ///
    /// let w = WorkspacesWidget::default();
    /// assert_eq!(w.id().as_str(), "workspaces.root");
    /// ```
    fn default() -> Self {
        Self::new(Vec::new(), None)
    }
}

impl Widget for WorkspacesWidget {
    fn id(&self) -> &WidgetId {
        &self.id
    }

    fn title(&self) -> &str {
        "Workspaces"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn footer_hint(&self) -> Vec<FooterHint> {
        vec![
            FooterHint::new("N", "new workspace"),
            FooterHint::new("A", "add repo"),
            FooterHint::new("R", "remove"),
            FooterHint::new("Enter", "promote"),
            FooterHint::new("?", "help"),
        ]
    }

    fn render(&self, _target: &mut dyn RenderTarget) {
        // Real rendering happens in the binary's draw() function via match-on-tab-id.
        // The widget keeps its state pure; rendering is a TUI-layer concern.
    }

    fn handle_event(&mut self, ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        if let Event::Key(chord) = ev {
            // Tab / Shift+Tab cycles the focused PANE (Tree ↔ SubView).
            // This replaces the previous cycle-right-pane behavior; the
            // right-pane sub-view is changed via in-sub-view shortcuts
            // (`c` from Status → Commit, etc.) or the `r` shortcut for
            // the Actions menu.
            if chord.code == KeyCode::Tab && chord.mods == KeyModifiers::NONE {
                self.focus_next();
                return EventOutcome::Consumed;
            }
            if chord.code == KeyCode::Tab && chord.mods.contains(KeyModifiers::SHIFT) {
                self.focus_prev();
                return EventOutcome::Consumed;
            }
            if chord.code == KeyCode::BackTab {
                self.focus_prev();
                return EventOutcome::Consumed;
            }
            // Alt+<key> is reserved for future cross-pane actions.
            if chord.mods.contains(KeyModifiers::ALT) {
                // TODO: cross-pane actions on Alt+<key>
                return EventOutcome::Bubble;
            }

            // Widget-global keybinds (fire regardless of focused pane).
            // 'r' from any sub-view opens the action menu — preserved
            // because it's a "widget action" not "pane navigation".
            if chord.code == KeyCode::Char('r') && chord.mods == KeyModifiers::NONE {
                if !matches!(self.state.right_pane, RightPane::Actions(_)) {
                    self.state.right_pane = RightPane::Actions(ActionListState::default());
                }
                return EventOutcome::Consumed;
            }

            // Pane-gated routing.
            match self.focused_pane {
                WsFocus::Tree => match (chord.code, chord.mods) {
                    (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                        self.state.select_next();
                        return EventOutcome::Consumed;
                    }
                    (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                        self.state.select_prev();
                        return EventOutcome::Consumed;
                    }
                    (KeyCode::Enter, KeyModifiers::NONE) => {
                        self.state.toggle_expand_selected();
                        return EventOutcome::Consumed;
                    }
                    _ => {}
                },
                WsFocus::SubView => {
                    // Route to the active sub-view's handler.
                    let chord = *chord;
                    match &self.state.right_pane {
                        RightPane::Branches(_) => return self.handle_branch_pane_event(&chord),
                        RightPane::Status(_) => return self.handle_status_pane_event(&chord),
                        RightPane::Log(_) => return self.handle_log_pane_event(&chord),
                        RightPane::Diff(_) => return self.handle_diff_pane_event(&chord),
                        RightPane::Commit(_) => return self.handle_commit_pane_event(&chord),
                        RightPane::Actions(_) => return self.handle_actions_pane_event(&chord),
                    }
                }
            }
        }
        EventOutcome::Bubble
    }
}

// ─── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use sid_core::widget::Widget;

    use super::*;

    #[test]
    fn id_and_title_correct() {
        let w = WorkspacesWidget::new(vec![], None);
        assert_eq!(w.id().as_str(), "workspaces.root");
        assert_eq!(w.title(), "Workspaces");
    }

    #[test]
    fn save_state_is_empty() {
        let w = WorkspacesWidget::new(vec![], None);
        assert!(w.save_state().is_empty());
    }

    #[test]
    fn load_state_is_noop() {
        let mut w = WorkspacesWidget::new(vec![], None);
        w.load_state(&[0xFF, 0x00]); // must not panic
        assert_eq!(w.id().as_str(), "workspaces.root");
    }

    #[test]
    fn default_right_pane_is_branches() {
        let w = WorkspacesWidget::default();
        assert!(matches!(w.state().right_pane(), RightPane::Branches(_)));
    }

    #[test]
    fn cycle_pane_next_advances_through_all_variants() {
        let mut s = WorkspacesState::new(vec![]);
        assert!(matches!(s.right_pane(), RightPane::Branches(_)));
        s.cycle_pane_next();
        assert!(matches!(s.right_pane(), RightPane::Status(_)));
        s.cycle_pane_next();
        assert!(matches!(s.right_pane(), RightPane::Log(_)));
        s.cycle_pane_next();
        assert!(matches!(s.right_pane(), RightPane::Diff(_)));
        s.cycle_pane_next();
        assert!(matches!(s.right_pane(), RightPane::Commit(_)));
        s.cycle_pane_next();
        assert!(matches!(s.right_pane(), RightPane::Actions(_)));
        s.cycle_pane_next(); // wraps back
        assert!(matches!(s.right_pane(), RightPane::Branches(_)));
    }

    #[test]
    fn cycle_pane_prev_reverses_through_all_variants() {
        let mut s = WorkspacesState::new(vec![]);
        s.cycle_pane_prev(); // from Branches -> Actions
        assert!(matches!(s.right_pane(), RightPane::Actions(_)));
    }

    #[test]
    fn right_pane_label_matches_variant() {
        assert_eq!(
            RightPane::Branches(BranchListState::default()).label(),
            "Branches"
        );
        assert_eq!(
            RightPane::Status(StatusListState::default()).label(),
            "Status"
        );
        assert_eq!(RightPane::Log(LogListState::default()).label(), "Log");
        assert_eq!(RightPane::Diff(DiffViewState::default()).label(), "Diff");
        assert_eq!(
            RightPane::Commit(CommitDraftState::default()).label(),
            "Commit"
        );
        assert_eq!(
            RightPane::Actions(ActionListState::default()).label(),
            "Actions"
        );
    }

    #[test]
    fn commit_draft_state_machine_transitions() {
        let mut s = CommitDraftState::default();
        assert!(s.is_idle());
        s.start_editing();
        assert_eq!(s.phase(), &CommitDraftPhase::EditingMessage);
        s.finish_editing("feat: add thing".into());
        assert_eq!(s.phase(), &CommitDraftPhase::Committing);
        assert_eq!(s.draft_message(), "feat: add thing");
        s.mark_done("abc123".into());
        assert_eq!(s.phase(), &CommitDraftPhase::Done);
        assert_eq!(s.committed_oid(), Some("abc123"));
        s.reset();
        assert!(s.is_idle());
    }

    #[test]
    fn commit_draft_failed_state() {
        let mut s = CommitDraftState::default();
        s.start_editing();
        s.finish_editing("msg".into());
        s.mark_failed("git error".into());
        assert_eq!(s.phase(), &CommitDraftPhase::Failed);
        assert_eq!(s.error(), Some("git error"));
    }

    #[test]
    fn branch_list_state_navigation() {
        use sid_core::adapters::git::Branch;
        let branches = vec![
            Branch {
                name: "main".into(),
                head_oid: "a".repeat(40),
                upstream: None,
                is_current: true,
            },
            Branch {
                name: "dev".into(),
                head_oid: "b".repeat(40),
                upstream: None,
                is_current: false,
            },
        ];
        let mut s = BranchListState::new(branches);
        assert_eq!(s.selected_branch().unwrap().name, "main");
        s.select_next();
        assert_eq!(s.selected_branch().unwrap().name, "dev");
        s.select_next(); // wraps
        assert_eq!(s.selected_branch().unwrap().name, "main");
        s.select_prev();
        assert_eq!(s.selected_branch().unwrap().name, "dev");
    }

    #[test]
    fn status_list_state_set_status_clamps_selection() {
        use sid_core::adapters::git::{GitStatus, StatusEntry, StatusKind};
        let mut s = StatusListState::new(GitStatus {
            entries: (0..5)
                .map(|i| StatusEntry {
                    path: format!("f{i}.rs"),
                    kind: StatusKind::Modified,
                    staged: false,
                    old_path: None,
                })
                .collect(),
            is_clean: false,
        });
        s.selected = 4;
        // Replace with 2-entry status
        s.set_status(GitStatus {
            entries: vec![
                StatusEntry {
                    path: "a.rs".into(),
                    kind: StatusKind::Modified,
                    staged: false,
                    old_path: None,
                },
                StatusEntry {
                    path: "b.rs".into(),
                    kind: StatusKind::Added,
                    staged: true,
                    old_path: None,
                },
            ],
            is_clean: false,
        });
        assert!(s.selected_idx() < 2);
    }

    #[test]
    fn diff_view_state_toggle_staged_resets_scroll() {
        let mut s = DiffViewState::new(vec![], false);
        s.scroll_offset = 10;
        s.toggle_staged();
        assert!(s.staged());
        assert_eq!(s.scroll_offset(), 0);
    }

    #[test]
    fn diff_view_scroll_bounded_by_saturating_sub() {
        let mut s = DiffViewState::default();
        s.scroll_up(); // should not underflow
        assert_eq!(s.scroll_offset(), 0);
        s.scroll_down();
        s.scroll_down();
        assert_eq!(s.scroll_offset(), 2);
    }

    #[test]
    fn log_state_cursor_stack_round_trip() {
        let mut s = LogListState::new(vec![], 50);
        s.push_cursor(None); // initial page has None cursor
        s.push_cursor(Some("abc123".into()));
        let top = s.pop_cursor().unwrap();
        assert_eq!(top, Some("abc123".into()));
        let bottom = s.pop_cursor().unwrap();
        assert!(bottom.is_none());
    }

    #[test]
    fn action_list_state_empty_is_safe() {
        let mut s = ActionListState::new(vec![]);
        s.select_next(); // noop
        s.select_prev(); // noop
        assert!(s.selected_action().is_none());
    }

    #[test]
    fn action_list_state_navigation() {
        use sid_core::workspace_metadata::WorkspaceAction;
        let actions = vec![
            WorkspaceAction {
                label: "Build".into(),
                cmd: "cargo build".into(),
                key: Some('b'),
            },
            WorkspaceAction {
                label: "Test".into(),
                cmd: "cargo test".into(),
                key: Some('t'),
            },
        ];
        let mut s = ActionListState::new(actions);
        assert_eq!(s.selected_action().unwrap().label, "Build");
        s.select_next();
        assert_eq!(s.selected_action().unwrap().label, "Test");
        s.select_prev();
        assert_eq!(s.selected_action().unwrap().label, "Build");
    }
}
