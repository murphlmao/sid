# sid Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the foundational scaffolding for `sid` — a Cargo workspace producing a working binary that renders six tab stubs in the cosmos theme, switches between them with `Ctrl+arrows` and `Ctrl+1..6`, opens a command palette with `Ctrl+F`, quits cleanly, and persists the active-tab + session state to a redb file across restarts.

**Architecture:** Multi-crate Cargo workspace with the adapter pattern enforced at the crate level — widget code never names external crates. A synchronous Ratatui render loop is driven by a crossterm event source on one Tokio task; long-running work goes to a `JobQueue` on other Tokio tasks. State changes flow through a debounced `StatePersister` to a redb file.

**Tech Stack:**
- Rust edition 2024 (MSRV 1.85)
- Ratatui 0.30 (immediate-mode TUI) + crossterm 0.29 (backend)
- Tokio 1.47 (multi-thread async runtime)
- redb 4.1 (pure-Rust ACID embedded DB; wrapped behind the `Store` trait)
- postcard 1.1 (binary serialization with versioned blobs)
- tracing 0.1 + tracing-appender 0.2 (logging)
- clap 4.5 (CLI args)
- thiserror 2 + anyhow 1 + color-eyre 0.6 (errors)
- directories 5 (XDG paths)
- insta 1 (snapshot testing), proptest 1 (property tests), tempfile 3

**Out of scope (handled in later plans):**
- Real content for any tab (Plans 2–7)
- Adapter *implementations* beyond stubs — git2, russh, db clients, sysinfo, etc. (Plans 2–6)
- Detach + IPC + multi-process redb (Plan 8)
- Workspace discovery beyond persisting the active workspace ID (Plan 2)
- Full Settings tab features — theme picker, keybind editor, etc. (Plan 7)

---

## File structure

```
sid/
├── Cargo.toml                            # workspace manifest
├── deny.toml                             # cargo-deny config
├── rustfmt.toml                          # formatting rules
├── .editorconfig                         # editor hints
├── crates/
│   ├── sid-core/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                    # crate root, re-exports
│   │       ├── error.rs                  # SidError type, Result alias
│   │       ├── widget.rs                 # Widget trait, WidgetId, EventOutcome
│   │       ├── layout.rs                 # Layout enum (Single only in v1)
│   │       ├── event.rs                  # Event type wrapping crossterm + custom events
│   │       ├── context.rs                # WidgetCtx handed to handle_event
│   │       ├── tab.rs                    # Tab, TabId, TabManager
│   │       ├── action.rs                 # Action, ActionId, ActionRegistry
│   │       ├── keybind.rs                # KeyBinding, KeybindMap, default profile
│   │       ├── palette.rs                # CommandPalette state + fuzzy filter
│   │       ├── app.rs                    # App struct, event loop
│   │       └── adapters/
│   │           ├── mod.rs                # re-exports adapter traits
│   │           ├── git.rs                # GitProvider trait (empty in this plan)
│   │           ├── ssh.rs                # SshClient trait (empty)
│   │           ├── pty.rs                # PtyProvider trait (empty)
│   │           ├── db_client.rs          # DbClient trait (empty)
│   │           ├── sys.rs                # SysProvider trait (empty)
│   │           ├── notifier.rs           # Notifier trait + ToastNotifier impl
│   │           └── clipboard.rs          # Clipboard trait (empty)
│   ├── sid-ui/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                    # re-exports
│   │       ├── theme.rs                  # Theme struct, color tokens
│   │       ├── themes.rs                 # COSMOS, VOID, DUSK, COSMOS_LIGHT consts
│   │       └── helpers.rs                # themed Block / Paragraph builders
│   ├── sid-store/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                    # Store trait + domain types
│   │       ├── codec.rs                  # postcard helpers + versioned blob wrapper
│   │       ├── schema.rs                 # redb TableDefinition consts
│   │       └── redb_impl.rs              # RedbStore: trait impl over redb::Database
│   ├── sid-job/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs                    # JobQueue, JobHandle, JobResult
│   ├── sid-widgets/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                    # re-exports
│   │       ├── stub.rs                   # shared "Coming soon" widget body
│   │       ├── workspaces.rs             # WorkspacesWidget stub
│   │       ├── ssh.rs                    # SshWidget stub
│   │       ├── database.rs               # DatabaseWidget stub
│   │       ├── network.rs                # NetworkWidget stub
│   │       ├── system.rs                 # SystemWidget stub
│   │       └── settings.rs               # SettingsWidget stub
│   └── sid/
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs                   # CLI args, sets up runtime, hands off
│           ├── runtime.rs                # Tokio runtime + crossterm event source
│           └── wire.rs                   # builds the App with concrete impls wired in
└── docs/superpowers/plans/
```

**Rationale.** `sid-core` defines the abstractions (Widget, Layout, traits) but does not pull in any external runtime crate (no Ratatui, no Tokio, no redb). `sid-ui` knows about Ratatui. `sid-store` is the only crate that knows about redb. The binary crate `sid/` is the only place that names every adapter implementation; this is what enforces the adapter pattern via Cargo's dependency graph.

---

## Task index

| # | Task | Phase |
|---|---|---|
| 1 | Cargo workspace setup | A. Infra |
| 2 | rustfmt + editorconfig | A. Infra |
| 3 | cargo-deny config | A. Infra |
| 4 | `sid-core` skeleton + `SidError` | B. Core |
| 5 | Widget trait | B. Core |
| 6 | Layout enum | B. Core |
| 7 | Event type | B. Core |
| 8 | WidgetCtx | B. Core |
| 9 | Tab + TabManager | B. Core |
| 10 | Action + ActionRegistry | B. Core |
| 11 | KeyBinding + KeybindMap + defaults | B. Core |
| 12 | CommandPalette fuzzy filter | B. Core |
| 13 | Adapter trait shells | C. Adapters |
| 14 | `sid-ui` Theme struct | D. UI |
| 15 | Built-in theme consts (cosmos et al.) | D. UI |
| 16 | Themed widget helpers | D. UI |
| 17 | `sid-store` Store trait + domain types | E. Store |
| 18 | codec + versioned blob wrapper | E. Store |
| 19 | RedbStore: open + schema | E. Store |
| 20 | RedbStore: settings get/put | E. Store |
| 21 | RedbStore: sessions + heartbeat | E. Store |
| 22 | RedbStore: widget_state blob | E. Store |
| 23 | `sid-job` JobQueue | F. Job |
| 24 | shared ComingSoon stub body | G. Widgets |
| 25 | WorkspacesWidget stub | G. Widgets |
| 26 | SshWidget stub | G. Widgets |
| 27 | DatabaseWidget stub | G. Widgets |
| 28 | NetworkWidget stub | G. Widgets |
| 29 | SystemWidget stub | G. Widgets |
| 30 | SettingsWidget stub | G. Widgets |
| 31 | App struct + initialization | H. App |
| 32 | StatePersister with debounce | H. App |
| 33 | App event loop | H. App |
| 34 | Tab nav keybind handler | H. App |
| 35 | CommandPalette wiring | H. App |
| 36 | Session restore prompt on launch | H. App |
| 37 | `sid` binary: CLI args | I. Binary |
| 38 | `sid` binary: runtime + event source | I. Binary |
| 39 | `sid` binary: wire & main | I. Binary |
| 40 | Integration test: spawn + nav + quit | J. Tests |
| 41 | README build instructions | J. Tests |

---

## Phase A — Infra

### Task 1: Cargo workspace setup

**Files:**
- Create: `Cargo.toml`

- [ ] **Step 1: Create workspace `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = [
    "crates/sid",
    "crates/sid-core",
    "crates/sid-ui",
    "crates/sid-store",
    "crates/sid-job",
    "crates/sid-widgets",
]

[workspace.package]
version = "0.0.1"
edition = "2024"
rust-version = "1.85"
license = "GPL-3.0-only"
repository = "https://github.com/murphlmao/sid"
authors = ["Murphy Malcolm"]

[workspace.dependencies]
# Internal
sid-core = { path = "crates/sid-core" }
sid-ui = { path = "crates/sid-ui" }
sid-store = { path = "crates/sid-store" }
sid-job = { path = "crates/sid-job" }
sid-widgets = { path = "crates/sid-widgets" }

# TUI
ratatui = "0.30"
crossterm = "0.29"

# Async
tokio = { version = "1.47", features = ["rt-multi-thread", "macros", "sync", "signal", "time", "fs", "io-util"] }
tokio-util = "0.7"
futures = "0.3"

# Storage
redb = "4.1"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
postcard = { version = "1.1", features = ["use-std"] }

# CLI
clap = { version = "4.5", features = ["derive"] }

# Errors / logging
thiserror = "2"
anyhow = "1"
color-eyre = "0.6"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
tracing-appender = "0.2"

# Paths
directories = "5"

# Test
insta = { version = "1", features = ["yaml"] }
proptest = "1"
tempfile = "3"

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = true
```

- [ ] **Step 2: Verify the workspace resolves**

Run: `cargo metadata --no-deps --format-version 1 > /dev/null`
Expected: exit code 0 (errors will name missing member crates — that's fine until Task 4 creates them; if the command errors, scaffold an empty `crates/sid-core/Cargo.toml` and `crates/sid-core/src/lib.rs` to get past resolution).

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "chore: add Cargo workspace manifest"
```

---

### Task 2: rustfmt + editorconfig

**Files:**
- Create: `rustfmt.toml`
- Create: `.editorconfig`

- [ ] **Step 1: Create `rustfmt.toml`**

```toml
edition = "2024"
max_width = 100
imports_granularity = "Crate"
group_imports = "StdExternalCrate"
reorder_imports = true
newline_style = "Unix"
use_field_init_shorthand = true
use_try_shorthand = true
```

- [ ] **Step 2: Create `.editorconfig`**

```
root = true

[*]
charset = utf-8
end_of_line = lf
indent_style = space
indent_size = 4
insert_final_newline = true
trim_trailing_whitespace = true

[*.{toml,yaml,yml,json,md}]
indent_size = 2
```

- [ ] **Step 3: Commit**

```bash
git add rustfmt.toml .editorconfig
git commit -m "chore: add rustfmt and editorconfig"
```

---

### Task 3: cargo-deny config

**Files:**
- Create: `deny.toml`

- [ ] **Step 1: Create `deny.toml`**

```toml
[graph]
targets = []

[advisories]
db-path = "~/.cargo/advisory-db"
vulnerability = "deny"
unmaintained = "warn"
yanked = "deny"
notice = "warn"

[licenses]
unlicensed = "deny"
allow = [
  "MIT",
  "Apache-2.0",
  "Apache-2.0 WITH LLVM-exception",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "Unicode-DFS-2016",
  "Unicode-3.0",
  "GPL-3.0-only",
  "MPL-2.0",
  "CC0-1.0",
  "Zlib",
]
exceptions = []

[bans]
multiple-versions = "warn"
wildcards = "deny"
deny = []

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
```

- [ ] **Step 2: Commit**

```bash
git add deny.toml
git commit -m "chore: add cargo-deny configuration"
```

Note: cargo-deny will be run from CI once crates compile; we'll add CI in a later plan.

---

## Phase B — Core abstractions (`sid-core`)

### Task 4: `sid-core` skeleton + `SidError`

**Files:**
- Create: `crates/sid-core/Cargo.toml`
- Create: `crates/sid-core/src/lib.rs`
- Create: `crates/sid-core/src/error.rs`
- Test: `crates/sid-core/tests/error.rs`

- [ ] **Step 1: Create `crates/sid-core/Cargo.toml`**

```toml
[package]
name = "sid-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
thiserror.workspace = true
tracing.workspace = true
serde = { workspace = true }
crossterm.workspace = true

[dev-dependencies]
```

(Keep `crossterm` in `sid-core` because it owns the `Event` type. No `ratatui`, no `tokio`.)

- [ ] **Step 2: Create `crates/sid-core/src/error.rs`**

```rust
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SidError {
    #[error("storage error: {0}")]
    Storage(String),

    #[error("io error reading {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("widget '{0}' not registered")]
    UnknownWidget(String),

    #[error("action '{0}' not registered")]
    UnknownAction(String),

    #[error("invalid keybind: {0}")]
    InvalidKeybind(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = SidError> = std::result::Result<T, E>;
```

- [ ] **Step 3: Create `crates/sid-core/src/lib.rs`**

```rust
//! Core abstractions for sid: the Widget trait, App, tabs, keybinds, actions.
//! No knowledge of Ratatui, Tokio, or storage backends lives here.

pub mod error;

pub use error::{Result, SidError};
```

- [ ] **Step 4: Write a basic test**

Create `crates/sid-core/tests/error.rs`:

```rust
use sid_core::SidError;

#[test]
fn error_display_includes_message() {
    let e = SidError::Other("boom".into());
    let msg = format!("{e}");
    assert!(msg.contains("boom"));
}
```

- [ ] **Step 5: Build & test**

Run: `cargo test -p sid-core`
Expected: 1 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add sid-core crate skeleton and SidError"
```

---

### Task 5: Widget trait

**Files:**
- Create: `crates/sid-core/src/widget.rs`
- Modify: `crates/sid-core/src/lib.rs`

**Note:** `sid-core` does not depend on Ratatui; we use a small opaque `RenderTarget` trait so widgets can be rendered by any frontend later. For v1 the only implementation lives in `sid-ui`, but `Widget` stays Ratatui-agnostic at this layer.

- [ ] **Step 1: Write the test first**

Create `crates/sid-core/tests/widget_trait.rs`:

```rust
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

struct Dummy {
    id: WidgetId,
    title: &'static str,
}

impl Widget for Dummy {
    fn id(&self) -> WidgetId {
        self.id
    }
    fn title(&self) -> &str {
        self.title
    }
    fn render(&self, _: &mut dyn RenderTarget) {}
    fn handle_event(
        &mut self,
        _ev: &sid_core::event::Event,
        _ctx: &mut sid_core::context::WidgetCtx,
    ) -> EventOutcome {
        EventOutcome::Consumed
    }
    fn save_state(&self) -> Vec<u8> {
        Vec::new()
    }
    fn load_state(&mut self, _: &[u8]) {}
}

#[test]
fn dummy_widget_reports_metadata() {
    let d = Dummy {
        id: WidgetId::new("dummy"),
        title: "Dummy",
    };
    assert_eq!(d.id().as_str(), "dummy");
    assert_eq!(d.title(), "Dummy");
}
```

- [ ] **Step 2: Run the test — should fail to compile**

Run: `cargo test -p sid-core --test widget_trait`
Expected: compile error — `widget` module missing.

- [ ] **Step 3: Create `crates/sid-core/src/widget.rs`**

```rust
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::context::WidgetCtx;
use crate::event::Event;

/// Stable identity of a widget instance. Used for state restoration and keybind scope.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct WidgetId(String);

impl WidgetId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WidgetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Outcome of an event passed to a widget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventOutcome {
    /// Widget handled the event.
    Consumed,
    /// Widget did not handle the event; let parent / global handlers see it.
    Bubble,
}

/// Render target abstraction. `sid-ui` provides the only impl for now (over Ratatui).
/// Keeping the trait here means widgets don't depend on Ratatui directly.
pub trait RenderTarget {
    /// Width of the area the widget should render into, in cells.
    fn width(&self) -> u16;
    /// Height of the area, in cells.
    fn height(&self) -> u16;
}

/// A focused, self-contained UI module. In v1 each tab contains exactly one Widget.
pub trait Widget: Send {
    fn id(&self) -> WidgetId;
    fn title(&self) -> &str;
    fn render(&self, target: &mut dyn RenderTarget);
    fn handle_event(&mut self, ev: &Event, ctx: &mut WidgetCtx) -> EventOutcome;
    /// Serialize widget UI state for restoration. Default: empty.
    fn save_state(&self) -> Vec<u8> {
        Vec::new()
    }
    /// Restore widget UI state. Default: no-op.
    fn load_state(&mut self, _bytes: &[u8]) {}
}
```

- [ ] **Step 4: Add module declaration to `lib.rs`**

Modify `crates/sid-core/src/lib.rs`:

```rust
//! Core abstractions for sid.

pub mod context;
pub mod error;
pub mod event;
pub mod widget;

pub use error::{Result, SidError};
pub use widget::{EventOutcome, RenderTarget, Widget, WidgetId};
```

(`context` and `event` are added in Tasks 6 and 7; create empty placeholders now so the crate compiles.)

Create `crates/sid-core/src/event.rs`:

```rust
//! Placeholder — filled in Task 7.
#[derive(Debug)]
pub struct Event;
```

Create `crates/sid-core/src/context.rs`:

```rust
//! Placeholder — filled in Task 8.
pub struct WidgetCtx;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p sid-core --test widget_trait`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add Widget trait, WidgetId, EventOutcome, RenderTarget"
```

---

### Task 6: Layout enum

**Files:**
- Create: `crates/sid-core/src/layout.rs`
- Modify: `crates/sid-core/src/lib.rs`
- Test: `crates/sid-core/tests/layout.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-core/tests/layout.rs`:

```rust
use sid_core::layout::{Layout, Dir};
use sid_core::widget::{Widget, WidgetId, EventOutcome, RenderTarget};
use sid_core::context::WidgetCtx;
use sid_core::event::Event;

struct W(&'static str);
impl Widget for W {
    fn id(&self) -> WidgetId { WidgetId::new(self.0) }
    fn title(&self) -> &str { self.0 }
    fn render(&self, _: &mut dyn RenderTarget) {}
    fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
}

#[test]
fn single_layout_holds_one_widget() {
    let layout: Layout = Layout::Single(Box::new(W("only")));
    let titles: Vec<String> = layout.iter_widgets().map(|w| w.title().to_string()).collect();
    assert_eq!(titles, vec!["only".to_string()]);
}

#[test]
fn split_layout_iterates_in_order() {
    let layout = Layout::Split {
        dir: Dir::Horizontal,
        ratio: 0.5,
        a: Box::new(Layout::Single(Box::new(W("a")))),
        b: Box::new(Layout::Single(Box::new(W("b")))),
    };
    let titles: Vec<String> = layout.iter_widgets().map(|w| w.title().to_string()).collect();
    assert_eq!(titles, vec!["a".to_string(), "b".to_string()]);
}
```

- [ ] **Step 2: Run the test — should fail to compile**

Run: `cargo test -p sid-core --test layout`
Expected: compile error (no `layout` module).

- [ ] **Step 3: Create `crates/sid-core/src/layout.rs`**

```rust
use crate::widget::Widget;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dir {
    Horizontal,
    Vertical,
}

/// Tree of widgets inside a single tab.
///
/// v1 only constructs `Single`. v2+ uses `Split` for Hyprland-style composition;
/// the variant is present here so future composition is a non-breaking addition.
pub enum Layout {
    Single(Box<dyn Widget>),
    Split {
        dir: Dir,
        ratio: f32,
        a: Box<Layout>,
        b: Box<Layout>,
    },
}

impl Layout {
    /// In-order traversal of every widget in the layout.
    pub fn iter_widgets(&self) -> WidgetIter<'_> {
        WidgetIter { stack: vec![self] }
    }

    /// In-order mutable traversal of every widget in the layout.
    pub fn iter_widgets_mut(&mut self) -> WidgetIterMut<'_> {
        WidgetIterMut { stack: vec![self] }
    }
}

pub struct WidgetIter<'a> {
    stack: Vec<&'a Layout>,
}

impl<'a> Iterator for WidgetIter<'a> {
    type Item = &'a dyn Widget;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(node) = self.stack.pop() {
            match node {
                Layout::Single(w) => return Some(w.as_ref()),
                Layout::Split { a, b, .. } => {
                    self.stack.push(b);
                    self.stack.push(a);
                }
            }
        }
        None
    }
}

pub struct WidgetIterMut<'a> {
    stack: Vec<&'a mut Layout>,
}

impl<'a> Iterator for WidgetIterMut<'a> {
    type Item = &'a mut dyn Widget;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(node) = self.stack.pop() {
            match node {
                Layout::Single(w) => return Some(w.as_mut()),
                Layout::Split { a, b, .. } => {
                    self.stack.push(b);
                    self.stack.push(a);
                }
            }
        }
        None
    }
}
```

- [ ] **Step 4: Add module to `lib.rs`**

Modify `crates/sid-core/src/lib.rs` — add `pub mod layout;` and re-export `pub use layout::{Dir, Layout};`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p sid-core --test layout`
Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add Layout enum (Single, Split) and traversal iterators"
```

---

### Task 7: Event type

**Files:**
- Modify: `crates/sid-core/src/event.rs` (replace placeholder)
- Test: `crates/sid-core/tests/event.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-core/tests/event.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sid_core::event::{Event, KeyChord};

#[test]
fn from_crossterm_key_extracts_chord() {
    let crossterm_ev = crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    let ev = Event::from_crossterm(crossterm_ev);
    match ev {
        Event::Key(chord) => {
            assert_eq!(chord, KeyChord::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        }
        other => panic!("expected Key, got {other:?}"),
    }
}

#[test]
fn tick_event_constructs() {
    let _ = Event::Tick;
}
```

- [ ] **Step 2: Run the test — should fail**

Run: `cargo test -p sid-core --test event`
Expected: compile error (placeholder Event has no variants).

- [ ] **Step 3: Replace `crates/sid-core/src/event.rs`**

```rust
use crossterm::event::{Event as CtEvent, KeyCode, KeyEvent, KeyModifiers, MouseEvent};

/// Normalized event passed through the App event loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A key was pressed; modifiers are normalized.
    Key(KeyChord),
    /// A mouse event (raw crossterm value).
    Mouse(MouseEvent),
    /// The terminal was resized.
    Resize { width: u16, height: u16 },
    /// Periodic tick from the runtime (e.g., for animation, heartbeat).
    Tick,
    /// A focus-gained / focus-lost notification from the terminal.
    Focus(bool),
    /// A custom event injected by the runtime (e.g., job completion).
    Custom(String),
}

impl Event {
    pub fn from_crossterm(ev: CtEvent) -> Self {
        match ev {
            CtEvent::Key(KeyEvent { code, modifiers, .. }) => Event::Key(KeyChord::new(code, modifiers)),
            CtEvent::Mouse(m) => Event::Mouse(m),
            CtEvent::Resize(w, h) => Event::Resize { width: w, height: h },
            CtEvent::FocusGained => Event::Focus(true),
            CtEvent::FocusLost => Event::Focus(false),
            CtEvent::Paste(_) => Event::Custom("paste".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct KeyChord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl KeyChord {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        Self { code, mods }
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-core --test event`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add Event type wrapping crossterm + KeyChord"
```

---

### Task 8: WidgetCtx

**Files:**
- Modify: `crates/sid-core/src/context.rs` (replace placeholder)
- Test: `crates/sid-core/tests/context.rs`

`WidgetCtx` carries channels widgets use to talk back to the app — emit actions, request a redraw, log, post a toast. We keep it small and trait-free so widgets can be tested without an entire App.

- [ ] **Step 1: Write the failing test**

Create `crates/sid-core/tests/context.rs`:

```rust
use std::sync::mpsc;

use sid_core::context::WidgetCtx;

#[test]
fn ctx_emit_action_pushes_to_channel() {
    let (tx, rx) = mpsc::channel();
    let mut ctx = WidgetCtx::new(tx);
    ctx.emit_action("quit");
    let id = rx.try_recv().unwrap();
    assert_eq!(id, "quit");
}

#[test]
fn ctx_redraw_flag_persists() {
    let (tx, _rx) = mpsc::channel();
    let mut ctx = WidgetCtx::new(tx);
    assert!(!ctx.needs_redraw());
    ctx.request_redraw();
    assert!(ctx.needs_redraw());
}
```

- [ ] **Step 2: Run — should fail to compile**

Run: `cargo test -p sid-core --test context`
Expected: compile error.

- [ ] **Step 3: Replace `crates/sid-core/src/context.rs`**

```rust
use std::sync::mpsc::Sender;

/// Context passed to a widget when it handles an event.
///
/// Lets widgets emit actions back to the app, request a redraw, or log.
pub struct WidgetCtx {
    action_tx: Sender<String>,
    redraw: bool,
}

impl WidgetCtx {
    pub fn new(action_tx: Sender<String>) -> Self {
        Self { action_tx, redraw: false }
    }

    /// Emit an action by ID. The App will dispatch it via its ActionRegistry.
    pub fn emit_action(&mut self, id: impl Into<String>) {
        let _ = self.action_tx.send(id.into());
    }

    /// Mark the screen as dirty; the next event-loop iteration redraws.
    pub fn request_redraw(&mut self) {
        self.redraw = true;
    }

    /// Consumed by the App after each event to decide whether to call `render`.
    pub fn needs_redraw(&self) -> bool {
        self.redraw
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-core --test context`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add WidgetCtx with action emission and redraw flag"
```

---

### Task 9: Tab + TabManager

**Files:**
- Create: `crates/sid-core/src/tab.rs`
- Modify: `crates/sid-core/src/lib.rs`
- Test: `crates/sid-core/tests/tab_manager.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-core/tests/tab_manager.rs`:

```rust
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::layout::Layout;
use sid_core::tab::{Tab, TabId, TabManager};
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

struct W(&'static str);
impl Widget for W {
    fn id(&self) -> WidgetId { WidgetId::new(self.0) }
    fn title(&self) -> &str { self.0 }
    fn render(&self, _: &mut dyn RenderTarget) {}
    fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
}

fn tab(id: &'static str, title: &'static str, w: &'static str) -> Tab {
    Tab {
        id: TabId::new(id),
        title: title.into(),
        layout: Layout::Single(Box::new(W(w))),
        hotkey: None,
    }
}

#[test]
fn tab_manager_starts_on_first_tab() {
    let tm = TabManager::new(vec![tab("a", "A", "wa"), tab("b", "B", "wb")]);
    assert_eq!(tm.active().id.as_str(), "a");
}

#[test]
fn next_and_prev_cycle() {
    let mut tm = TabManager::new(vec![tab("a", "A", "wa"), tab("b", "B", "wb"), tab("c", "C", "wc")]);
    tm.next();
    assert_eq!(tm.active().id.as_str(), "b");
    tm.next();
    assert_eq!(tm.active().id.as_str(), "c");
    tm.next();
    assert_eq!(tm.active().id.as_str(), "a");
    tm.prev();
    assert_eq!(tm.active().id.as_str(), "c");
}

#[test]
fn jump_by_index_clamps() {
    let mut tm = TabManager::new(vec![tab("a", "A", "wa"), tab("b", "B", "wb")]);
    tm.jump(1);
    assert_eq!(tm.active().id.as_str(), "b");
    tm.jump(99);
    assert_eq!(tm.active().id.as_str(), "b");
}

#[test]
fn switch_to_id_returns_true_when_found() {
    let mut tm = TabManager::new(vec![tab("a", "A", "wa"), tab("b", "B", "wb")]);
    assert!(tm.switch_to(&TabId::new("b")));
    assert_eq!(tm.active().id.as_str(), "b");
    assert!(!tm.switch_to(&TabId::new("nope")));
}
```

- [ ] **Step 2: Run — should fail to compile**

Run: `cargo test -p sid-core --test tab_manager`
Expected: compile error.

- [ ] **Step 3: Create `crates/sid-core/src/tab.rs`**

```rust
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::layout::Layout;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct TabId(String);

impl TabId {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
}

pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub layout: Layout,
    /// Optional one-keystroke hotkey (the char part — modifier is fixed to Ctrl).
    pub hotkey: Option<char>,
}

pub struct TabManager {
    tabs: Vec<Tab>,
    active_idx: usize,
}

impl TabManager {
    pub fn new(tabs: Vec<Tab>) -> Self {
        assert!(!tabs.is_empty(), "TabManager requires at least one tab");
        Self { tabs, active_idx: 0 }
    }

    pub fn active(&self) -> &Tab { &self.tabs[self.active_idx] }
    pub fn active_mut(&mut self) -> &mut Tab { &mut self.tabs[self.active_idx] }
    pub fn tabs(&self) -> &[Tab] { &self.tabs }
    pub fn active_index(&self) -> usize { self.active_idx }

    pub fn next(&mut self) {
        self.active_idx = (self.active_idx + 1) % self.tabs.len();
    }

    pub fn prev(&mut self) {
        self.active_idx = (self.active_idx + self.tabs.len() - 1) % self.tabs.len();
    }

    /// Jump to a tab by index. Out-of-range jumps clamp to last tab.
    pub fn jump(&mut self, idx: usize) {
        self.active_idx = idx.min(self.tabs.len() - 1);
    }

    /// Switch by ID. Returns true on success.
    pub fn switch_to(&mut self, id: &TabId) -> bool {
        if let Some(i) = self.tabs.iter().position(|t| &t.id == id) {
            self.active_idx = i;
            true
        } else {
            false
        }
    }
}
```

- [ ] **Step 4: Add to `lib.rs`**

Modify `crates/sid-core/src/lib.rs`:

```rust
pub mod context;
pub mod error;
pub mod event;
pub mod layout;
pub mod tab;
pub mod widget;

pub use error::{Result, SidError};
pub use layout::{Dir, Layout};
pub use tab::{Tab, TabId, TabManager};
pub use widget::{EventOutcome, RenderTarget, Widget, WidgetId};
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p sid-core --test tab_manager`
Expected: 4 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add Tab, TabId, TabManager with next/prev/jump/switch_to"
```

---

### Task 10: Action + ActionRegistry

**Files:**
- Create: `crates/sid-core/src/action.rs`
- Modify: `crates/sid-core/src/lib.rs`
- Test: `crates/sid-core/tests/action_registry.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-core/tests/action_registry.rs`:

```rust
use sid_core::action::{Action, ActionId, ActionRegistry, ActionScope};

#[test]
fn register_and_lookup_action() {
    let mut reg = ActionRegistry::new();
    reg.register(Action {
        id: ActionId::new("quit"),
        label: "Quit".into(),
        scope: ActionScope::Global,
        keybind_hint: Some("Ctrl+Q".into()),
    });
    let a = reg.get(&ActionId::new("quit")).expect("found");
    assert_eq!(a.label, "Quit");
}

#[test]
fn fuzzy_search_returns_matches_ordered_by_score() {
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("quit", "Quit"));
    reg.register(Action::new("ports.kill", "Kill port"));
    reg.register(Action::new("config.open", "Open config in kitty"));

    let results = reg.fuzzy("kil");
    let labels: Vec<&str> = results.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(labels.first(), Some(&"Kill port"));
}

#[test]
fn empty_query_returns_all_actions() {
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("a", "Alpha"));
    reg.register(Action::new("b", "Beta"));
    assert_eq!(reg.fuzzy("").len(), 2);
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-core --test action_registry`
Expected: compile error.

- [ ] **Step 3: Create `crates/sid-core/src/action.rs`**

```rust
use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ActionId(String);

impl ActionId {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl fmt::Display for ActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ActionScope {
    Global,
    Tab(String),
    Workspace,
    WorkspaceTree,
}

#[derive(Clone, Debug)]
pub struct Action {
    pub id: ActionId,
    pub label: String,
    pub scope: ActionScope,
    pub keybind_hint: Option<String>,
}

impl Action {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: ActionId::new(id),
            label: label.into(),
            scope: ActionScope::Global,
            keybind_hint: None,
        }
    }
}

#[derive(Default)]
pub struct ActionRegistry {
    by_id: BTreeMap<ActionId, Action>,
}

impl ActionRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn register(&mut self, a: Action) {
        self.by_id.insert(a.id.clone(), a);
    }

    pub fn get(&self, id: &ActionId) -> Option<&Action> {
        self.by_id.get(id)
    }

    pub fn all(&self) -> impl Iterator<Item = &Action> {
        self.by_id.values()
    }

    /// Tiny subsequence-match scorer for the command palette. Empty query returns all.
    /// Scoring: count of matched chars + bonus for label-start match. Higher is better.
    pub fn fuzzy(&self, query: &str) -> Vec<&Action> {
        if query.is_empty() {
            return self.all().collect();
        }
        let q = query.to_lowercase();
        let mut scored: Vec<(i32, &Action)> = self
            .by_id
            .values()
            .filter_map(|a| score_label(&q, &a.label).map(|s| (s, a)))
            .collect();
        scored.sort_by(|x, y| y.0.cmp(&x.0));
        scored.into_iter().map(|(_, a)| a).collect()
    }
}

fn score_label(query: &str, label: &str) -> Option<i32> {
    let label_l = label.to_lowercase();
    let mut q = query.chars();
    let mut cur = q.next()?;
    let mut score: i32 = 0;
    let mut last_pos: i32 = -2;
    let mut matched_anything = false;
    for (i, c) in label_l.chars().enumerate() {
        if c == cur {
            matched_anything = true;
            score += if i == 0 { 5 } else { 1 };
            if i as i32 == last_pos + 1 { score += 2; }
            last_pos = i as i32;
            cur = match q.next() {
                Some(c) => c,
                None => return Some(score),
            };
        }
    }
    if matched_anything && q.clone().next().is_none() {
        Some(score)
    } else {
        None
    }
}
```

- [ ] **Step 4: Add to `lib.rs`**

Modify `crates/sid-core/src/lib.rs` — add `pub mod action;` and `pub use action::{Action, ActionId, ActionRegistry, ActionScope};`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p sid-core --test action_registry`
Expected: 3 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add Action, ActionId, ActionRegistry with fuzzy filter"
```

---

### Task 11: KeyBinding + KeybindMap + defaults

**Files:**
- Create: `crates/sid-core/src/keybind.rs`
- Modify: `crates/sid-core/src/lib.rs`
- Test: `crates/sid-core/tests/keybind.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-core/tests/keybind.rs`:

```rust
use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::action::ActionId;
use sid_core::event::KeyChord;
use sid_core::keybind::{KeyBinding, KeybindMap};

#[test]
fn lookup_returns_action_for_bound_chord() {
    let mut m = KeybindMap::new();
    m.bind(KeyBinding {
        chord: KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
        action: ActionId::new("quit"),
    });
    let chord = KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert_eq!(m.lookup(&chord), Some(&ActionId::new("quit")));
}

#[test]
fn unbound_chord_returns_none() {
    let m = KeybindMap::new();
    let chord = KeyChord::new(KeyCode::Char('x'), KeyModifiers::NONE);
    assert_eq!(m.lookup(&chord), None);
}

#[test]
fn defaults_bind_quit_palette_settings_and_tab_nav() {
    let m = KeybindMap::cosmos_default();

    let q = KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert_eq!(m.lookup(&q), Some(&ActionId::new("app.quit")));

    let f = KeyChord::new(KeyCode::Char('f'), KeyModifiers::CONTROL);
    assert_eq!(m.lookup(&f), Some(&ActionId::new("palette.open")));

    let left = KeyChord::new(KeyCode::Left, KeyModifiers::CONTROL);
    assert_eq!(m.lookup(&left), Some(&ActionId::new("tabs.prev")));

    let right = KeyChord::new(KeyCode::Right, KeyModifiers::CONTROL);
    assert_eq!(m.lookup(&right), Some(&ActionId::new("tabs.next")));

    let n1 = KeyChord::new(KeyCode::Char('1'), KeyModifiers::CONTROL);
    assert_eq!(m.lookup(&n1), Some(&ActionId::new("tabs.jump.1")));

    let n6 = KeyChord::new(KeyCode::Char('6'), KeyModifiers::CONTROL);
    assert_eq!(m.lookup(&n6), Some(&ActionId::new("tabs.jump.6")));

    let comma = KeyChord::new(KeyCode::Char(','), KeyModifiers::CONTROL);
    assert_eq!(m.lookup(&comma), Some(&ActionId::new("app.open_settings")));
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-core --test keybind`
Expected: compile error.

- [ ] **Step 3: Create `crates/sid-core/src/keybind.rs`**

```rust
use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyModifiers};

use crate::action::ActionId;
use crate::event::KeyChord;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyBinding {
    pub chord: KeyChord,
    pub action: ActionId,
}

#[derive(Default)]
pub struct KeybindMap {
    by_chord: BTreeMap<ChordKey, ActionId>,
}

// We can't put KeyChord directly into a BTreeMap because KeyCode doesn't impl Ord.
// Use a string-keyed wrapper for ordering; equality semantics are preserved.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct ChordKey(String);

fn chord_key(c: &KeyChord) -> ChordKey {
    ChordKey(format!("{:?}|{:?}", c.code, c.mods.bits()))
}

impl KeybindMap {
    pub fn new() -> Self { Self::default() }

    pub fn bind(&mut self, b: KeyBinding) {
        self.by_chord.insert(chord_key(&b.chord), b.action);
    }

    pub fn lookup(&self, chord: &KeyChord) -> Option<&ActionId> {
        self.by_chord.get(&chord_key(chord))
    }

    /// Default keybinds for the cosmos profile.
    pub fn cosmos_default() -> Self {
        let mut m = Self::new();
        let bind = |m: &mut Self, code: KeyCode, mods: KeyModifiers, action: &str| {
            m.bind(KeyBinding { chord: KeyChord::new(code, mods), action: ActionId::new(action) });
        };
        // Tab nav
        bind(&mut m, KeyCode::Left, KeyModifiers::CONTROL, "tabs.prev");
        bind(&mut m, KeyCode::Right, KeyModifiers::CONTROL, "tabs.next");
        for i in 1..=6 {
            let c = char::from_digit(i, 10).unwrap();
            bind(&mut m, KeyCode::Char(c), KeyModifiers::CONTROL, &format!("tabs.jump.{i}"));
        }
        // Global
        bind(&mut m, KeyCode::Char('f'), KeyModifiers::CONTROL, "palette.open");
        bind(&mut m, KeyCode::Char('q'), KeyModifiers::CONTROL, "app.quit");
        bind(&mut m, KeyCode::Char(','), KeyModifiers::CONTROL, "app.open_settings");
        // Stubbed for plan 8 (detach), bound to a no-op action handler
        bind(&mut m, KeyCode::Char('d'), KeyModifiers::CONTROL, "tab.detach");
        bind(&mut m, KeyCode::Char('a'), KeyModifiers::CONTROL, "tab.attach");
        bind(&mut m, KeyCode::Char('r'), KeyModifiers::CONTROL, "tab.reload");
        m
    }
}
```

- [ ] **Step 4: Add to `lib.rs`**

Modify `crates/sid-core/src/lib.rs` — add `pub mod keybind;` and `pub use keybind::{KeyBinding, KeybindMap};`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p sid-core --test keybind`
Expected: 3 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add KeyBinding, KeybindMap, cosmos default profile"
```

---

### Task 12: CommandPalette

**Files:**
- Create: `crates/sid-core/src/palette.rs`
- Modify: `crates/sid-core/src/lib.rs`
- Test: `crates/sid-core/tests/palette.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-core/tests/palette.rs`:

```rust
use sid_core::action::{Action, ActionRegistry};
use sid_core::palette::CommandPalette;

#[test]
fn palette_opens_with_no_query_and_shows_all_actions() {
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("a", "Alpha"));
    reg.register(Action::new("b", "Beta"));
    let mut p = CommandPalette::new();
    p.open();
    assert!(p.is_open());
    let matches = p.matches(&reg);
    assert_eq!(matches.len(), 2);
}

#[test]
fn typing_filters_matches() {
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("quit", "Quit"));
    reg.register(Action::new("ports.kill", "Kill port"));
    let mut p = CommandPalette::new();
    p.open();
    p.input("kil");
    let matches = p.matches(&reg);
    assert_eq!(matches.first().map(|a| a.label.as_str()), Some("Kill port"));
}

#[test]
fn cursor_wraps_within_match_list() {
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("a", "Alpha"));
    reg.register(Action::new("b", "Beta"));
    let mut p = CommandPalette::new();
    p.open();
    assert_eq!(p.selected_index(), 0);
    p.cursor_down(&reg);
    assert_eq!(p.selected_index(), 1);
    p.cursor_down(&reg);
    assert_eq!(p.selected_index(), 0);
    p.cursor_up(&reg);
    assert_eq!(p.selected_index(), 1);
}

#[test]
fn close_clears_query_and_state() {
    let mut p = CommandPalette::new();
    p.open();
    p.input("kil");
    p.close();
    assert!(!p.is_open());
    assert_eq!(p.query(), "");
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-core --test palette`
Expected: compile error.

- [ ] **Step 3: Create `crates/sid-core/src/palette.rs`**

```rust
use crate::action::{Action, ActionRegistry};

pub struct CommandPalette {
    open: bool,
    query: String,
    selected: usize,
}

impl CommandPalette {
    pub fn new() -> Self {
        Self { open: false, query: String::new(), selected: 0 }
    }

    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected = 0;
    }

    pub fn is_open(&self) -> bool { self.open }
    pub fn query(&self) -> &str { &self.query }
    pub fn selected_index(&self) -> usize { self.selected }

    pub fn input(&mut self, s: &str) {
        self.query.push_str(s);
        self.selected = 0;
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    pub fn cursor_down(&mut self, reg: &ActionRegistry) {
        let len = self.matches(reg).len().max(1);
        self.selected = (self.selected + 1) % len;
    }

    pub fn cursor_up(&mut self, reg: &ActionRegistry) {
        let len = self.matches(reg).len().max(1);
        self.selected = (self.selected + len - 1) % len;
    }

    pub fn matches<'a>(&self, reg: &'a ActionRegistry) -> Vec<&'a Action> {
        reg.fuzzy(&self.query)
    }

    /// Currently selected action, if any.
    pub fn current<'a>(&self, reg: &'a ActionRegistry) -> Option<&'a Action> {
        let matches = self.matches(reg);
        matches.get(self.selected).copied()
    }
}

impl Default for CommandPalette {
    fn default() -> Self { Self::new() }
}
```

- [ ] **Step 4: Add to `lib.rs`**

Modify `crates/sid-core/src/lib.rs` — add `pub mod palette;` and `pub use palette::CommandPalette;`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p sid-core --test palette`
Expected: 4 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add CommandPalette with open/close/input/cursor/match selection"
```

---

## Phase C — Adapter trait shells

### Task 13: Adapter trait shells

**Files:**
- Create: `crates/sid-core/src/adapters/mod.rs`
- Create: `crates/sid-core/src/adapters/git.rs`
- Create: `crates/sid-core/src/adapters/ssh.rs`
- Create: `crates/sid-core/src/adapters/pty.rs`
- Create: `crates/sid-core/src/adapters/db_client.rs`
- Create: `crates/sid-core/src/adapters/sys.rs`
- Create: `crates/sid-core/src/adapters/notifier.rs`
- Create: `crates/sid-core/src/adapters/clipboard.rs`
- Modify: `crates/sid-core/src/lib.rs`

These are empty traits for now — filled in later plans. The point of creating them in Plan 1 is to establish the adapter pattern *in the code* before any concrete impls exist.

- [ ] **Step 1: Create `crates/sid-core/src/adapters/mod.rs`**

```rust
//! Adapter traits. Each external dependency that sid will eventually wrap
//! gets a trait here; concrete impls live in their own crates.

pub mod clipboard;
pub mod db_client;
pub mod git;
pub mod notifier;
pub mod pty;
pub mod ssh;
pub mod sys;
```

- [ ] **Step 2: Create the adapter trait files**

`crates/sid-core/src/adapters/git.rs`:

```rust
//! GitProvider — filled out in Plan 2 (Workspaces + git adapter).

pub trait GitProvider: Send + Sync {}
```

`crates/sid-core/src/adapters/ssh.rs`:

```rust
//! SshClient — filled out in Plan 3 (SSH + SFTP).

pub trait SshClient: Send + Sync {}
```

`crates/sid-core/src/adapters/pty.rs`:

```rust
//! PtyProvider — filled out in Plan 3 (PTY backbone).

pub trait PtyProvider: Send + Sync {}
```

`crates/sid-core/src/adapters/db_client.rs`:

```rust
//! DbClient — filled out in Plan 4 (Database tab).

pub trait DbClient: Send + Sync {}
```

`crates/sid-core/src/adapters/sys.rs`:

```rust
//! SysProvider — filled out in Plan 5 (Network tab).

pub trait SysProvider: Send + Sync {}
```

`crates/sid-core/src/adapters/notifier.rs`:

```rust
//! Notifier — the only adapter with a v1 implementation, because toast
//! notifications are part of the foundation.

#[derive(Clone, Debug)]
pub enum NotifyLevel {
    Info,
    Warn,
    Error,
}

pub trait Notifier: Send + Sync {
    fn notify(&self, level: NotifyLevel, message: &str);
}
```

`crates/sid-core/src/adapters/clipboard.rs`:

```rust
//! Clipboard — filled out in a later plan as needed.

pub trait Clipboard: Send + Sync {
    fn copy(&self, text: &str);
}
```

- [ ] **Step 3: Add module to `lib.rs`**

Modify `crates/sid-core/src/lib.rs` — add `pub mod adapters;`.

- [ ] **Step 4: Build the crate**

Run: `cargo build -p sid-core`
Expected: success.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): scaffold adapter trait modules for all external dependencies"
```

---

## Phase D — `sid-ui`

### Task 14: Theme struct

**Files:**
- Create: `crates/sid-ui/Cargo.toml`
- Create: `crates/sid-ui/src/lib.rs`
- Create: `crates/sid-ui/src/theme.rs`
- Test: `crates/sid-ui/tests/theme.rs`

- [ ] **Step 1: Create `crates/sid-ui/Cargo.toml`**

```toml
[package]
name = "sid-ui"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
sid-core.workspace = true
ratatui.workspace = true
serde.workspace = true
```

- [ ] **Step 2: Write the failing test**

Create `crates/sid-ui/tests/theme.rs`:

```rust
use sid_ui::theme::{Color, Theme};

#[test]
fn theme_holds_palette() {
    let t = Theme {
        name: "test".into(),
        background: Color::rgb(0x0b, 0x0b, 0x14),
        surface: Color::rgb(0x13, 0x13, 0x1f),
        foreground: Color::rgb(0xe6, 0xe6, 0xf0),
        muted: Color::rgb(0x4a, 0x4a, 0x60),
        accent_primary: Color::rgb(0xd4, 0x41, 0x41),
        accent_success: Color::rgb(0xa8, 0xd8, 0xe8),
        accent_warning: Color::rgb(0xe8, 0xb0, 0x4a),
        accent_error: Color::rgb(0xff, 0x55, 0x70),
        border: Color::rgb(0x1f, 0x1f, 0x2e),
        glyphs: Default::default(),
    };
    assert_eq!(t.name, "test");
    assert_eq!(t.background.r, 0x0b);
}

#[test]
fn color_to_ratatui_round_trips_rgb() {
    let c = Color::rgb(0x12, 0x34, 0x56);
    let rt: ratatui::style::Color = c.into();
    assert!(matches!(rt, ratatui::style::Color::Rgb(0x12, 0x34, 0x56)));
}
```

- [ ] **Step 3: Run — should fail**

Run: `cargo test -p sid-ui --test theme`
Expected: compile error.

- [ ] **Step 4: Create `crates/sid-ui/src/lib.rs`**

```rust
//! UI types and helpers. Ratatui-aware; widgets render via these helpers.

pub mod helpers;
pub mod theme;
pub mod themes;

pub use theme::{Color, GlyphSet, Theme};
```

(`helpers` and `themes` modules are added in Tasks 15 and 16; create empty placeholders now.)

Create `crates/sid-ui/src/themes.rs` (placeholder):

```rust
//! Placeholder — built-in themes added in Task 15.
```

Create `crates/sid-ui/src/helpers.rs` (placeholder):

```rust
//! Placeholder — themed widget helpers added in Task 16.
```

- [ ] **Step 5: Create `crates/sid-ui/src/theme.rs`**

```rust
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

impl From<Color> for ratatui::style::Color {
    fn from(c: Color) -> Self {
        ratatui::style::Color::Rgb(c.r, c.g, c.b)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GlyphSet {
    pub star: char,
    pub small_star: char,
    pub dot: char,
}

impl Default for GlyphSet {
    fn default() -> Self {
        Self { star: '★', small_star: '✦', dot: '·' }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub background: Color,
    pub surface: Color,
    pub foreground: Color,
    pub muted: Color,
    pub accent_primary: Color,
    pub accent_success: Color,
    pub accent_warning: Color,
    pub accent_error: Color,
    pub border: Color,
    pub glyphs: GlyphSet,
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p sid-ui --test theme`
Expected: 2 passed.

- [ ] **Step 7: Commit**

```bash
git add crates/sid-ui
git commit -m "feat(ui): add Theme struct, Color (RGB), GlyphSet"
```

---

### Task 15: Built-in theme consts

**Files:**
- Modify: `crates/sid-ui/src/themes.rs` (replace placeholder)
- Test: `crates/sid-ui/tests/themes.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-ui/tests/themes.rs`:

```rust
use sid_ui::themes::{cosmos, cosmos_light, dusk, void};

#[test]
fn cosmos_has_expected_background() {
    let t = cosmos();
    assert_eq!(t.name, "cosmos");
    assert_eq!(t.background.r, 0x0b);
    assert_eq!(t.background.g, 0x0b);
    assert_eq!(t.background.b, 0x14);
}

#[test]
fn all_themes_have_unique_names() {
    let names: Vec<_> = [cosmos(), void(), dusk(), cosmos_light()]
        .iter()
        .map(|t| t.name.clone())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len());
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-ui --test themes`
Expected: compile error.

- [ ] **Step 3: Replace `crates/sid-ui/src/themes.rs`**

```rust
use crate::theme::{Color, GlyphSet, Theme};

pub fn cosmos() -> Theme {
    Theme {
        name: "cosmos".into(),
        background: Color::rgb(0x0b, 0x0b, 0x14),
        surface: Color::rgb(0x13, 0x13, 0x1f),
        foreground: Color::rgb(0xe6, 0xe6, 0xf0),
        muted: Color::rgb(0x4a, 0x4a, 0x60),
        accent_primary: Color::rgb(0xd4, 0x41, 0x41),
        accent_success: Color::rgb(0xa8, 0xd8, 0xe8),
        accent_warning: Color::rgb(0xe8, 0xb0, 0x4a),
        accent_error: Color::rgb(0xff, 0x55, 0x70),
        border: Color::rgb(0x1f, 0x1f, 0x2e),
        glyphs: GlyphSet::default(),
    }
}

pub fn void() -> Theme {
    Theme {
        name: "void".into(),
        background: Color::rgb(0x00, 0x00, 0x00),
        surface: Color::rgb(0x0a, 0x0a, 0x0a),
        foreground: Color::rgb(0xee, 0xee, 0xee),
        muted: Color::rgb(0x55, 0x55, 0x55),
        accent_primary: Color::rgb(0xd4, 0x41, 0x41),
        accent_success: Color::rgb(0xc0, 0xc0, 0xc0),
        accent_warning: Color::rgb(0xe0, 0xa0, 0x40),
        accent_error: Color::rgb(0xff, 0x33, 0x33),
        border: Color::rgb(0x22, 0x22, 0x22),
        glyphs: GlyphSet::default(),
    }
}

pub fn dusk() -> Theme {
    Theme {
        name: "dusk".into(),
        background: Color::rgb(0x14, 0x10, 0x0c),
        surface: Color::rgb(0x1c, 0x18, 0x12),
        foreground: Color::rgb(0xf0, 0xe5, 0xd0),
        muted: Color::rgb(0x60, 0x55, 0x48),
        accent_primary: Color::rgb(0xe8, 0x70, 0x40),
        accent_success: Color::rgb(0xa8, 0xd8, 0x90),
        accent_warning: Color::rgb(0xe8, 0xb0, 0x4a),
        accent_error: Color::rgb(0xd0, 0x4a, 0x4a),
        border: Color::rgb(0x2a, 0x22, 0x1a),
        glyphs: GlyphSet::default(),
    }
}

pub fn cosmos_light() -> Theme {
    Theme {
        name: "cosmos-light".into(),
        background: Color::rgb(0xf4, 0xf4, 0xf8),
        surface: Color::rgb(0xea, 0xea, 0xf2),
        foreground: Color::rgb(0x18, 0x18, 0x24),
        muted: Color::rgb(0x70, 0x70, 0x82),
        accent_primary: Color::rgb(0xb0, 0x30, 0x30),
        accent_success: Color::rgb(0x40, 0x80, 0x90),
        accent_warning: Color::rgb(0xb0, 0x80, 0x30),
        accent_error: Color::rgb(0xc0, 0x30, 0x40),
        border: Color::rgb(0xd0, 0xd0, 0xdc),
        glyphs: GlyphSet::default(),
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-ui --test themes`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-ui
git commit -m "feat(ui): add built-in themes (cosmos, void, dusk, cosmos-light)"
```

---

### Task 16: Themed widget helpers

**Files:**
- Modify: `crates/sid-ui/src/helpers.rs` (replace placeholder)
- Test: `crates/sid-ui/tests/helpers.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-ui/tests/helpers.rs`:

```rust
use ratatui::style::{Color, Style};
use sid_ui::helpers::{styled_block, accent_text};
use sid_ui::themes::cosmos;

#[test]
fn styled_block_uses_border_color_from_theme() {
    let t = cosmos();
    let block = styled_block(&t, "title");
    // The block should be constructed; rendering would require a Buffer.
    // Here we sanity-check it does not panic and accepts the theme.
    let _ = block;
}

#[test]
fn accent_text_returns_span_with_primary_color() {
    let t = cosmos();
    let span = accent_text(&t, "alert");
    let expected: Color = t.accent_primary.into();
    assert_eq!(span.style, Style::default().fg(expected));
    assert_eq!(span.content, "alert");
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-ui --test helpers`
Expected: compile error.

- [ ] **Step 3: Replace `crates/sid-ui/src/helpers.rs`**

```rust
use ratatui::style::{Style, Stylize};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders};

use crate::theme::Theme;

pub fn styled_block<'a>(theme: &Theme, title: &'a str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border.into()))
        .title(format!(" {} {} ", theme.glyphs.small_star, title))
        .title_style(Style::default().fg(theme.foreground.into()).bold())
        .style(Style::default().bg(theme.background.into()).fg(theme.foreground.into()))
}

pub fn accent_text<'a>(theme: &Theme, text: &'a str) -> Span<'a> {
    Span::styled(text, Style::default().fg(theme.accent_primary.into()))
}

pub fn muted_text<'a>(theme: &Theme, text: &'a str) -> Span<'a> {
    Span::styled(text, Style::default().fg(theme.muted.into()))
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-ui --test helpers`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-ui
git commit -m "feat(ui): add styled_block, accent_text, muted_text helpers"
```

---

## Phase E — `sid-store`

### Task 17: Store trait + domain types

**Files:**
- Create: `crates/sid-store/Cargo.toml`
- Create: `crates/sid-store/src/lib.rs`

- [ ] **Step 1: Create `crates/sid-store/Cargo.toml`**

```toml
[package]
name = "sid-store"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
sid-core.workspace = true
redb.workspace = true
serde.workspace = true
postcard.workspace = true
thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 2: Create `crates/sid-store/src/lib.rs` with the trait + types**

```rust
//! Domain-shaped storage trait. `RedbStore` is the v1 implementation.
//! Domain types here; impl details in `redb_impl.rs`.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sid_core::tab::TabId;
use sid_core::widget::WidgetId;
use sid_core::SidError;

pub mod codec;
pub mod redb_impl;
pub mod schema;

pub use redb_impl::RedbStore;

/// Wall-clock instant as ns since UNIX epoch. Used for ordering.
pub type Epoch = u64;

pub fn now_epoch() -> Epoch {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettingValue(pub Vec<u8>);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub started_at: Epoch,
    pub last_active: Epoch,
    pub ended_at: Option<Epoch>,
    pub active_tab: Option<TabId>,
    pub open_tabs: Vec<TabId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WidgetState {
    pub tab_id: TabId,
    pub widget_id: WidgetId,
    pub blob: Vec<u8>,
}

pub trait Store: Send + Sync {
    fn get_setting(&self, key: &str) -> Result<Option<SettingValue>, SidError>;
    fn put_setting(&self, key: &str, val: &SettingValue) -> Result<(), SidError>;

    fn current_session(&self) -> Result<Option<SessionRecord>, SidError>;
    fn upsert_session(&self, s: &SessionRecord) -> Result<(), SidError>;
    fn end_session(&self, id: &str, ended_at: Epoch) -> Result<(), SidError>;
    fn list_sessions(&self) -> Result<Vec<SessionRecord>, SidError>;

    fn save_widget_state(&self, s: &WidgetState) -> Result<(), SidError>;
    fn load_widget_state(&self, tab: &TabId, widget: &WidgetId) -> Result<Option<Vec<u8>>, SidError>;
}

pub trait OpenStore {
    fn open(path: &Path) -> Result<Self, SidError>
    where
        Self: Sized;
}
```

- [ ] **Step 3: Create placeholders for `codec.rs`, `schema.rs`, `redb_impl.rs`**

`crates/sid-store/src/codec.rs`:

```rust
//! Placeholder — filled in Task 18.
```

`crates/sid-store/src/schema.rs`:

```rust
//! Placeholder — filled in Task 19.
```

`crates/sid-store/src/redb_impl.rs`:

```rust
//! Placeholder — filled in Tasks 19–22.

pub struct RedbStore;
```

- [ ] **Step 4: Build**

Run: `cargo build -p sid-store`
Expected: success (warnings about unused are fine for now).

- [ ] **Step 5: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): add Store trait, domain types, crate skeleton"
```

---

### Task 18: codec + versioned blob wrapper

**Files:**
- Modify: `crates/sid-store/src/codec.rs`
- Test: `crates/sid-store/tests/codec.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-store/tests/codec.rs`:

```rust
use serde::{Deserialize, Serialize};
use sid_store::codec::{decode_versioned, encode_versioned};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct ExampleV1 {
    a: u32,
    b: String,
}

#[test]
fn round_trip_postcard_with_version_prefix() {
    let v = ExampleV1 { a: 42, b: "hi".into() };
    let bytes = encode_versioned(1, &v).unwrap();
    assert_eq!(bytes[0], 1);
    let (version, decoded) = decode_versioned::<ExampleV1>(&bytes).unwrap();
    assert_eq!(version, 1);
    assert_eq!(decoded, v);
}

#[test]
fn unknown_version_returns_error() {
    // Encode with version 99 manually.
    let mut bytes = vec![99u8];
    bytes.extend_from_slice(b"junk");
    let r: Result<(u8, ExampleV1), _> = decode_versioned(&bytes);
    assert!(r.is_err());
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-store --test codec`
Expected: compile error.

- [ ] **Step 3: Replace `crates/sid-store/src/codec.rs`**

```rust
use serde::{Deserialize, Serialize};
use sid_core::SidError;

/// Wrap a struct in a 1-byte version prefix + postcard payload.
pub fn encode_versioned<T: Serialize>(version: u8, value: &T) -> Result<Vec<u8>, SidError> {
    let body = postcard::to_allocvec(value).map_err(|e| SidError::Storage(format!("postcard encode: {e}")))?;
    let mut out = Vec::with_capacity(1 + body.len());
    out.push(version);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode a versioned payload. Returns (version, value).
pub fn decode_versioned<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<(u8, T), SidError> {
    let (&v, rest) = bytes.split_first().ok_or_else(|| SidError::Storage("empty payload".into()))?;
    let value: T = postcard::from_bytes(rest).map_err(|e| SidError::Storage(format!("postcard decode: {e}")))?;
    Ok((v, value))
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-store --test codec`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): add versioned postcard codec for schema evolution"
```

---

### Task 19: RedbStore — open + schema

**Files:**
- Replace: `crates/sid-store/src/schema.rs`
- Replace: `crates/sid-store/src/redb_impl.rs`

- [ ] **Step 1: Replace `crates/sid-store/src/schema.rs`**

```rust
use redb::TableDefinition;

/// Per-table KV definitions. Keys are encoded as `&[u8]`; values are versioned-postcard blobs.

pub const SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("settings");
pub const SESSIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("sessions");

/// Sessions metadata: single-row table with the current session id.
pub const SESSION_META: TableDefinition<&str, &[u8]> = TableDefinition::new("session_meta");

/// Widget state: composite key serialized as "tab_id\0widget_id".
pub const WIDGET_STATE: TableDefinition<&str, &[u8]> = TableDefinition::new("widget_state");
```

- [ ] **Step 2: Replace `crates/sid-store/src/redb_impl.rs` with minimal `open` + sanity test**

```rust
use std::path::Path;

use redb::Database;
use sid_core::SidError;

use crate::schema::{SESSIONS, SESSION_META, SETTINGS, WIDGET_STATE};
use crate::OpenStore;

pub struct RedbStore {
    db: Database,
}

impl OpenStore for RedbStore {
    fn open(path: &Path) -> Result<Self, SidError> {
        let db = Database::create(path).map_err(|e| SidError::Storage(format!("redb open {path:?}: {e}")))?;
        // Ensure tables exist by creating a write txn that opens each.
        let txn = db.begin_write().map_err(|e| SidError::Storage(format!("begin_write: {e}")))?;
        {
            let _ = txn.open_table(SETTINGS).map_err(|e| SidError::Storage(format!("open settings: {e}")))?;
            let _ = txn.open_table(SESSIONS).map_err(|e| SidError::Storage(format!("open sessions: {e}")))?;
            let _ = txn.open_table(SESSION_META).map_err(|e| SidError::Storage(format!("open session_meta: {e}")))?;
            let _ = txn.open_table(WIDGET_STATE).map_err(|e| SidError::Storage(format!("open widget_state: {e}")))?;
        }
        txn.commit().map_err(|e| SidError::Storage(format!("commit: {e}")))?;
        Ok(Self { db })
    }
}

impl RedbStore {
    pub(crate) fn raw(&self) -> &Database { &self.db }
}
```

- [ ] **Step 3: Write the test**

Create `crates/sid-store/tests/open.rs`:

```rust
use sid_store::{OpenStore, RedbStore};
use tempfile::tempdir;

#[test]
fn open_creates_db_file_and_tables() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sid.redb");
    let _store = RedbStore::open(&path).unwrap();
    assert!(path.exists());
}

#[test]
fn reopen_existing_db_does_not_error() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sid.redb");
    {
        let _ = RedbStore::open(&path).unwrap();
    }
    let _ = RedbStore::open(&path).unwrap();
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-store --test open`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): add RedbStore open + schema (4 tables)"
```

---

### Task 20: RedbStore — settings get/put

**Files:**
- Modify: `crates/sid-store/src/redb_impl.rs`
- Test: `crates/sid-store/tests/settings.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-store/tests/settings.rs`:

```rust
use sid_store::{OpenStore, RedbStore, SettingValue, Store};
use tempfile::tempdir;

#[test]
fn put_then_get_round_trips() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.put_setting("theme.name", &SettingValue(b"cosmos".to_vec())).unwrap();
    let v = store.get_setting("theme.name").unwrap().unwrap();
    assert_eq!(v.0, b"cosmos".to_vec());
}

#[test]
fn get_unknown_returns_none() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    assert!(store.get_setting("missing").unwrap().is_none());
}

#[test]
fn put_overwrites_existing_value() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.put_setting("k", &SettingValue(b"v1".to_vec())).unwrap();
    store.put_setting("k", &SettingValue(b"v2".to_vec())).unwrap();
    let v = store.get_setting("k").unwrap().unwrap();
    assert_eq!(v.0, b"v2".to_vec());
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-store --test settings`
Expected: compile error (Store trait methods unimplemented on RedbStore).

- [ ] **Step 3: Modify `crates/sid-store/src/redb_impl.rs` to implement settings**

Append to the file:

```rust
use sid_core::tab::TabId;
use sid_core::widget::WidgetId;

use crate::{now_epoch, SessionRecord, SettingValue, Store, WidgetState};

impl Store for RedbStore {
    fn get_setting(&self, key: &str) -> Result<Option<SettingValue>, SidError> {
        let txn = self.db.begin_read().map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
        let tbl = txn.open_table(SETTINGS).map_err(|e| SidError::Storage(format!("open settings: {e}")))?;
        let got = tbl.get(key).map_err(|e| SidError::Storage(format!("get setting: {e}")))?;
        Ok(got.map(|v| SettingValue(v.value().to_vec())))
    }

    fn put_setting(&self, key: &str, val: &SettingValue) -> Result<(), SidError> {
        let txn = self.db.begin_write().map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
        {
            let mut tbl = txn.open_table(SETTINGS).map_err(|e| SidError::Storage(format!("open settings: {e}")))?;
            tbl.insert(key, &val.0[..]).map_err(|e| SidError::Storage(format!("insert setting: {e}")))?;
        }
        txn.commit().map_err(|e| SidError::Storage(format!("commit settings: {e}")))?;
        Ok(())
    }

    fn current_session(&self) -> Result<Option<SessionRecord>, SidError> {
        // Implemented in Task 21.
        let _ = self; Ok(None)
    }
    fn upsert_session(&self, _s: &SessionRecord) -> Result<(), SidError> { Ok(()) }
    fn end_session(&self, _id: &str, _ended_at: crate::Epoch) -> Result<(), SidError> { Ok(()) }
    fn list_sessions(&self) -> Result<Vec<SessionRecord>, SidError> { Ok(Vec::new()) }

    fn save_widget_state(&self, _s: &WidgetState) -> Result<(), SidError> {
        // Implemented in Task 22.
        Ok(())
    }
    fn load_widget_state(&self, _tab: &TabId, _widget: &WidgetId) -> Result<Option<Vec<u8>>, SidError> {
        Ok(None)
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-store --test settings`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): implement RedbStore settings get/put"
```

---

### Task 21: RedbStore — sessions + heartbeat

**Files:**
- Modify: `crates/sid-store/src/redb_impl.rs`
- Test: `crates/sid-store/tests/sessions.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-store/tests/sessions.rs`:

```rust
use sid_core::tab::TabId;
use sid_store::{now_epoch, OpenStore, RedbStore, SessionRecord, Store};
use tempfile::tempdir;

fn make_session(id: &str, active_tab: &str) -> SessionRecord {
    SessionRecord {
        id: id.into(),
        started_at: now_epoch(),
        last_active: now_epoch(),
        ended_at: None,
        active_tab: Some(TabId::new(active_tab)),
        open_tabs: vec![TabId::new(active_tab)],
    }
}

#[test]
fn upsert_and_current_session() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let s = make_session("abc", "workspaces");
    store.upsert_session(&s).unwrap();
    let got = store.current_session().unwrap().unwrap();
    assert_eq!(got.id, "abc");
    assert_eq!(got.active_tab.as_ref().unwrap().as_str(), "workspaces");
}

#[test]
fn list_sessions_returns_all() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_session(&make_session("a", "workspaces")).unwrap();
    store.upsert_session(&make_session("b", "ssh")).unwrap();
    let all = store.list_sessions().unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn end_session_marks_ended_at() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_session(&make_session("a", "workspaces")).unwrap();
    store.end_session("a", 12345).unwrap();
    let got = store.list_sessions().unwrap();
    assert_eq!(got[0].ended_at, Some(12345));
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-store --test sessions`
Expected: failures (stub returns None / empty).

- [ ] **Step 3: Replace the session stubs in `redb_impl.rs`**

In the `impl Store for RedbStore` block, replace the session methods:

```rust
    fn current_session(&self) -> Result<Option<SessionRecord>, SidError> {
        let txn = self.db.begin_read().map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
        let meta = txn.open_table(SESSION_META).map_err(|e| SidError::Storage(format!("open session_meta: {e}")))?;
        let Some(current_id_bytes) = meta.get("current").map_err(|e| SidError::Storage(format!("get current: {e}")))? else {
            return Ok(None);
        };
        let id_bytes = current_id_bytes.value().to_vec();
        let id_str = std::str::from_utf8(&id_bytes).map_err(|_| SidError::Storage("non-utf8 session id".into()))?;
        let tbl = txn.open_table(SESSIONS).map_err(|e| SidError::Storage(format!("open sessions: {e}")))?;
        let Some(blob) = tbl.get(id_str).map_err(|e| SidError::Storage(format!("get session: {e}")))? else {
            return Ok(None);
        };
        let (_version, rec) = crate::codec::decode_versioned::<SessionRecord>(blob.value())?;
        Ok(Some(rec))
    }

    fn upsert_session(&self, s: &SessionRecord) -> Result<(), SidError> {
        let bytes = crate::codec::encode_versioned(1, s)?;
        let txn = self.db.begin_write().map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
        {
            let mut sess = txn.open_table(SESSIONS).map_err(|e| SidError::Storage(format!("open sessions: {e}")))?;
            sess.insert(s.id.as_str(), &bytes[..]).map_err(|e| SidError::Storage(format!("insert session: {e}")))?;
            let mut meta = txn.open_table(SESSION_META).map_err(|e| SidError::Storage(format!("open session_meta: {e}")))?;
            meta.insert("current", s.id.as_bytes()).map_err(|e| SidError::Storage(format!("set current: {e}")))?;
        }
        txn.commit().map_err(|e| SidError::Storage(format!("commit session: {e}")))?;
        Ok(())
    }

    fn end_session(&self, id: &str, ended_at: crate::Epoch) -> Result<(), SidError> {
        let txn = self.db.begin_write().map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
        {
            let mut sess = txn.open_table(SESSIONS).map_err(|e| SidError::Storage(format!("open sessions: {e}")))?;
            let Some(existing) = sess.get(id).map_err(|e| SidError::Storage(format!("get session: {e}")))? else {
                return Ok(());
            };
            let (_v, mut rec) = crate::codec::decode_versioned::<SessionRecord>(existing.value())?;
            drop(existing);
            rec.ended_at = Some(ended_at);
            let bytes = crate::codec::encode_versioned(1, &rec)?;
            sess.insert(id, &bytes[..]).map_err(|e| SidError::Storage(format!("update session: {e}")))?;
        }
        txn.commit().map_err(|e| SidError::Storage(format!("commit end_session: {e}")))?;
        Ok(())
    }

    fn list_sessions(&self) -> Result<Vec<SessionRecord>, SidError> {
        let txn = self.db.begin_read().map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
        let tbl = txn.open_table(SESSIONS).map_err(|e| SidError::Storage(format!("open sessions: {e}")))?;
        let mut out = Vec::new();
        let iter = tbl.iter().map_err(|e| SidError::Storage(format!("iter sessions: {e}")))?;
        for entry in iter {
            let (_k, v) = entry.map_err(|e| SidError::Storage(format!("iter step: {e}")))?;
            let (_ver, rec) = crate::codec::decode_versioned::<SessionRecord>(v.value())?;
            out.push(rec);
        }
        Ok(out)
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-store --test sessions`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): implement RedbStore session persistence + current/list/end"
```

---

### Task 22: RedbStore — widget_state

**Files:**
- Modify: `crates/sid-store/src/redb_impl.rs`
- Test: `crates/sid-store/tests/widget_state.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-store/tests/widget_state.rs`:

```rust
use sid_core::tab::TabId;
use sid_core::widget::WidgetId;
use sid_store::{OpenStore, RedbStore, Store, WidgetState};
use tempfile::tempdir;

#[test]
fn round_trip_widget_state() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let tab = TabId::new("workspaces");
    let widget = WidgetId::new("workspaces.root");
    let state = WidgetState {
        tab_id: tab.clone(),
        widget_id: widget.clone(),
        blob: vec![1, 2, 3, 4],
    };
    store.save_widget_state(&state).unwrap();
    let got = store.load_widget_state(&tab, &widget).unwrap().unwrap();
    assert_eq!(got, vec![1, 2, 3, 4]);
}

#[test]
fn unknown_pair_returns_none() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let got = store.load_widget_state(&TabId::new("nope"), &WidgetId::new("nope")).unwrap();
    assert!(got.is_none());
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-store --test widget_state`
Expected: failure (stub returns Ok / None).

- [ ] **Step 3: Replace widget_state stubs in `redb_impl.rs`**

Replace the two methods in the `impl Store for RedbStore` block:

```rust
    fn save_widget_state(&self, s: &WidgetState) -> Result<(), SidError> {
        let key = format!("{}\0{}", s.tab_id.as_str(), s.widget_id.as_str());
        let txn = self.db.begin_write().map_err(|e| SidError::Storage(format!("write txn: {e}")))?;
        {
            let mut tbl = txn.open_table(WIDGET_STATE).map_err(|e| SidError::Storage(format!("open widget_state: {e}")))?;
            tbl.insert(key.as_str(), &s.blob[..]).map_err(|e| SidError::Storage(format!("insert widget_state: {e}")))?;
        }
        txn.commit().map_err(|e| SidError::Storage(format!("commit widget_state: {e}")))?;
        Ok(())
    }

    fn load_widget_state(&self, tab: &TabId, widget: &WidgetId) -> Result<Option<Vec<u8>>, SidError> {
        let key = format!("{}\0{}", tab.as_str(), widget.as_str());
        let txn = self.db.begin_read().map_err(|e| SidError::Storage(format!("read txn: {e}")))?;
        let tbl = txn.open_table(WIDGET_STATE).map_err(|e| SidError::Storage(format!("open widget_state: {e}")))?;
        let got = tbl.get(key.as_str()).map_err(|e| SidError::Storage(format!("get widget_state: {e}")))?;
        Ok(got.map(|v| v.value().to_vec()))
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-store --test widget_state`
Expected: 2 passed.

- [ ] **Step 5: Run the full store test suite**

Run: `cargo test -p sid-store`
Expected: all tests pass (codec + open + settings + sessions + widget_state).

- [ ] **Step 6: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): implement RedbStore widget_state save/load"
```

---

## Phase F — `sid-job`

### Task 23: JobQueue

**Files:**
- Create: `crates/sid-job/Cargo.toml`
- Create: `crates/sid-job/src/lib.rs`
- Test: `crates/sid-job/tests/job_queue.rs`

JobQueue accepts an async closure and gives back a `JobHandle`; the handle delivers a typed result over a channel. The App polls completed jobs in its event loop.

- [ ] **Step 1: Create `crates/sid-job/Cargo.toml`**

```toml
[package]
name = "sid-job"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
sid-core.workspace = true
tokio = { workspace = true, features = ["rt-multi-thread", "sync", "macros"] }
tracing.workspace = true
thiserror.workspace = true

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt", "rt-multi-thread", "time"] }
```

- [ ] **Step 2: Write the failing test**

Create `crates/sid-job/tests/job_queue.rs`:

```rust
use std::time::Duration;

use sid_job::JobQueue;
use tokio::time::sleep;

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
```

- [ ] **Step 3: Run — should fail**

Run: `cargo test -p sid-job`
Expected: compile error.

- [ ] **Step 4: Create `crates/sid-job/src/lib.rs`**

```rust
//! Tiny job queue: spawn async work; deliver typed results to the App via a channel.
//!
//! `T` is bounded `Send + Clone + 'static` so results can be both stored in the
//! completions vec (`drain_completed`) and sent through the oneshot
//! (`await_result`) without re-running the future.

use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[derive(Clone, Debug, thiserror::Error)]
pub enum JobError {
    #[error("job panicked")]
    Panic,
    #[error("job cancelled")]
    Cancelled,
}

pub struct JobQueue<T: Send + Clone + 'static> {
    completions: Arc<Mutex<Vec<Result<T, JobError>>>>,
}

impl<T: Send + Clone + 'static> Default for JobQueue<T> {
    fn default() -> Self { Self::new() }
}

impl<T: Send + Clone + 'static> JobQueue<T> {
    pub fn new() -> Self {
        Self { completions: Arc::new(Mutex::new(Vec::new())) }
    }

    /// Spawn a job. Returns a handle that resolves to the result when complete.
    /// The result is also pushed into the completions vec so the App can poll
    /// for finished work via `drain_completed()`.
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

    /// Drain results that have completed since the last call. Non-blocking.
    pub fn drain_completed(&self) -> Vec<Result<T, JobError>> {
        let mut g = self.completions.lock().unwrap();
        std::mem::take(&mut *g)
    }
}

pub struct JobHandle<T: Send + Clone + 'static> {
    rx: Option<oneshot::Receiver<Result<T, JobError>>>,
    _join: JoinHandle<()>,
}

impl<T: Send + Clone + 'static> JobHandle<T> {
    /// Await the job's result. Consumes the handle.
    pub async fn await_result(mut self) -> Result<T, JobError> {
        match self.rx.take() {
            Some(rx) => rx.await.unwrap_or(Err(JobError::Cancelled)),
            None => Err(JobError::Cancelled),
        }
    }
}
```

**Note on `JobError::Panic`.** v1 does not actively catch panics in spawned futures (tokio surfaces panics via the `JoinHandle` future, which we currently drop). If you need panic-as-error semantics later, capture `_join.await` in a separate poller and translate `JoinError::is_panic()` to `JobError::Panic`. For Plan 1 the variant exists so callers can pattern-match against future expansions without breaking changes.

- [ ] **Step 5: Run tests**

Run: `cargo test -p sid-job`
Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-job
git commit -m "feat(job): add JobQueue + JobHandle backed by tokio::spawn"
```

---

## Phase G — Widget stubs (`sid-widgets`)

### Task 24: shared ComingSoon stub body

**Files:**
- Create: `crates/sid-widgets/Cargo.toml`
- Create: `crates/sid-widgets/src/lib.rs`
- Create: `crates/sid-widgets/src/stub.rs`
- Test: `crates/sid-widgets/tests/stub.rs`

- [ ] **Step 1: Create `crates/sid-widgets/Cargo.toml`**

```toml
[package]
name = "sid-widgets"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
sid-core.workspace = true
sid-ui.workspace = true
ratatui.workspace = true
```

- [ ] **Step 2: Write the failing test**

Create `crates/sid-widgets/tests/stub.rs`:

```rust
use sid_widgets::stub::ComingSoonBody;

#[test]
fn body_returns_title_and_subtitle() {
    let b = ComingSoonBody::new("Workspaces", "git operations across registered repos");
    assert_eq!(b.title(), "Workspaces");
    assert!(b.subtitle().contains("git operations"));
}
```

- [ ] **Step 3: Run — should fail**

Run: `cargo test -p sid-widgets --test stub`
Expected: compile error.

- [ ] **Step 4: Create `crates/sid-widgets/src/lib.rs` and `stub.rs`**

`crates/sid-widgets/src/lib.rs`:

```rust
pub mod database;
pub mod network;
pub mod settings;
pub mod ssh;
pub mod stub;
pub mod system;
pub mod workspaces;

pub use database::DatabaseWidget;
pub use network::NetworkWidget;
pub use settings::SettingsWidget;
pub use ssh::SshWidget;
pub use system::SystemWidget;
pub use workspaces::WorkspacesWidget;
```

`crates/sid-widgets/src/stub.rs`:

```rust
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget};

pub struct ComingSoonBody {
    title: String,
    subtitle: String,
}

impl ComingSoonBody {
    pub fn new(title: impl Into<String>, subtitle: impl Into<String>) -> Self {
        Self { title: title.into(), subtitle: subtitle.into() }
    }
    pub fn title(&self) -> &str { &self.title }
    pub fn subtitle(&self) -> &str { &self.subtitle }

    pub fn render(&self, target: &mut dyn RenderTarget) {
        // Ratatui-aware rendering is performed by the App, which knows the Frame.
        // The stub body just exposes its content; rendering is wired through
        // the SidRenderTarget impl in sid-ui (added in the binary wiring).
        let _ = target;
    }

    pub fn handle_event(&mut self, _ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        EventOutcome::Bubble
    }
}
```

Also create the seven sibling files as empty placeholders (filled in Tasks 25–30):

```bash
touch crates/sid-widgets/src/workspaces.rs
touch crates/sid-widgets/src/ssh.rs
touch crates/sid-widgets/src/database.rs
touch crates/sid-widgets/src/network.rs
touch crates/sid-widgets/src/system.rs
touch crates/sid-widgets/src/settings.rs
```

Add a single line to each placeholder (so the crate compiles): `//! Filled in Tasks 25-30.` and `pub struct <name>;` (replace placeholders fully in their tasks). To keep `lib.rs` compiling, also add a tiny re-export to each file. Concretely:

`crates/sid-widgets/src/workspaces.rs`:

```rust
//! Stub placeholder — filled in Task 25.
pub struct WorkspacesWidget;
```

`crates/sid-widgets/src/ssh.rs`:

```rust
//! Stub placeholder — filled in Task 26.
pub struct SshWidget;
```

`crates/sid-widgets/src/database.rs`:

```rust
//! Stub placeholder — filled in Task 27.
pub struct DatabaseWidget;
```

`crates/sid-widgets/src/network.rs`:

```rust
//! Stub placeholder — filled in Task 28.
pub struct NetworkWidget;
```

`crates/sid-widgets/src/system.rs`:

```rust
//! Stub placeholder — filled in Task 29.
pub struct SystemWidget;
```

`crates/sid-widgets/src/settings.rs`:

```rust
//! Stub placeholder — filled in Task 30.
pub struct SettingsWidget;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p sid-widgets --test stub`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): add sid-widgets crate skeleton and ComingSoonBody stub"
```

---

### Task 25: WorkspacesWidget stub

**Files:**
- Replace: `crates/sid-widgets/src/workspaces.rs`
- Test: `crates/sid-widgets/tests/workspaces.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-widgets/tests/workspaces.rs`:

```rust
use sid_core::widget::Widget;
use sid_widgets::WorkspacesWidget;

#[test]
fn workspaces_widget_has_expected_id_and_title() {
    let w = WorkspacesWidget::new();
    assert_eq!(w.id().as_str(), "workspaces.root");
    assert_eq!(w.title(), "Workspaces");
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-widgets --test workspaces`
Expected: compile error.

- [ ] **Step 3: Replace `crates/sid-widgets/src/workspaces.rs`**

```rust
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

use crate::stub::ComingSoonBody;

pub struct WorkspacesWidget {
    body: ComingSoonBody,
}

impl WorkspacesWidget {
    pub fn new() -> Self {
        Self {
            body: ComingSoonBody::new(
                "Workspaces",
                "git operations across your registered code workspaces — coming in Plan 2",
            ),
        }
    }
}

impl Default for WorkspacesWidget {
    fn default() -> Self { Self::new() }
}

impl Widget for WorkspacesWidget {
    fn id(&self) -> WidgetId { WidgetId::new("workspaces.root") }
    fn title(&self) -> &str { self.body.title() }
    fn render(&self, target: &mut dyn RenderTarget) { self.body.render(target); }
    fn handle_event(&mut self, ev: &Event, ctx: &mut WidgetCtx) -> EventOutcome {
        self.body.handle_event(ev, ctx)
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-widgets --test workspaces`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): add WorkspacesWidget stub implementing Widget trait"
```

---

### Task 26: SshWidget stub

**Files:**
- Replace: `crates/sid-widgets/src/ssh.rs`
- Test: `crates/sid-widgets/tests/ssh.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-widgets/tests/ssh.rs`:

```rust
use sid_core::widget::Widget;
use sid_widgets::SshWidget;

#[test]
fn ssh_widget_has_expected_id_and_title() {
    let w = SshWidget::new();
    assert_eq!(w.id().as_str(), "ssh.root");
    assert_eq!(w.title(), "SSH");
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-widgets --test ssh`
Expected: compile error.

- [ ] **Step 3: Replace `crates/sid-widgets/src/ssh.rs`**

```rust
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

use crate::stub::ComingSoonBody;

pub struct SshWidget {
    body: ComingSoonBody,
}

impl SshWidget {
    pub fn new() -> Self {
        Self {
            body: ComingSoonBody::new(
                "SSH",
                "SSH host list + embedded terminal + SFTP — coming in Plan 3",
            ),
        }
    }
}

impl Default for SshWidget {
    fn default() -> Self { Self::new() }
}

impl Widget for SshWidget {
    fn id(&self) -> WidgetId { WidgetId::new("ssh.root") }
    fn title(&self) -> &str { self.body.title() }
    fn render(&self, target: &mut dyn RenderTarget) { self.body.render(target); }
    fn handle_event(&mut self, ev: &Event, ctx: &mut WidgetCtx) -> EventOutcome {
        self.body.handle_event(ev, ctx)
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-widgets --test ssh`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): add SshWidget stub implementing Widget trait"
```

---

### Task 27: DatabaseWidget stub

**Files:**
- Replace: `crates/sid-widgets/src/database.rs`
- Test: `crates/sid-widgets/tests/database.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-widgets/tests/database.rs`:

```rust
use sid_core::widget::Widget;
use sid_widgets::DatabaseWidget;

#[test]
fn database_widget_has_expected_id_and_title() {
    let w = DatabaseWidget::new();
    assert_eq!(w.id().as_str(), "database.root");
    assert_eq!(w.title(), "Database");
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-widgets --test database`
Expected: compile error.

- [ ] **Step 3: Replace `crates/sid-widgets/src/database.rs`**

```rust
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

use crate::stub::ComingSoonBody;

pub struct DatabaseWidget {
    body: ComingSoonBody,
}

impl DatabaseWidget {
    pub fn new() -> Self {
        Self {
            body: ComingSoonBody::new(
                "Database",
                "Postgres + SQLite query runner with paginated results — coming in Plan 4",
            ),
        }
    }
}

impl Default for DatabaseWidget {
    fn default() -> Self { Self::new() }
}

impl Widget for DatabaseWidget {
    fn id(&self) -> WidgetId { WidgetId::new("database.root") }
    fn title(&self) -> &str { self.body.title() }
    fn render(&self, target: &mut dyn RenderTarget) { self.body.render(target); }
    fn handle_event(&mut self, ev: &Event, ctx: &mut WidgetCtx) -> EventOutcome {
        self.body.handle_event(ev, ctx)
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-widgets --test database`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): add DatabaseWidget stub implementing Widget trait"
```

---

### Task 28: NetworkWidget stub

**Files:**
- Replace: `crates/sid-widgets/src/network.rs`
- Test: `crates/sid-widgets/tests/network.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-widgets/tests/network.rs`:

```rust
use sid_core::widget::Widget;
use sid_widgets::NetworkWidget;

#[test]
fn network_widget_has_expected_id_and_title() {
    let w = NetworkWidget::new();
    assert_eq!(w.id().as_str(), "network.root");
    assert_eq!(w.title(), "Network");
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-widgets --test network`
Expected: compile error.

- [ ] **Step 3: Replace `crates/sid-widgets/src/network.rs`**

```rust
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

use crate::stub::ComingSoonBody;

pub struct NetworkWidget {
    body: ComingSoonBody,
}

impl NetworkWidget {
    pub fn new() -> Self {
        Self {
            body: ComingSoonBody::new(
                "Network",
                "listening ports, processes, interfaces with kill-PID hotkeys — coming in Plan 5",
            ),
        }
    }
}

impl Default for NetworkWidget {
    fn default() -> Self { Self::new() }
}

impl Widget for NetworkWidget {
    fn id(&self) -> WidgetId { WidgetId::new("network.root") }
    fn title(&self) -> &str { self.body.title() }
    fn render(&self, target: &mut dyn RenderTarget) { self.body.render(target); }
    fn handle_event(&mut self, ev: &Event, ctx: &mut WidgetCtx) -> EventOutcome {
        self.body.handle_event(ev, ctx)
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-widgets --test network`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): add NetworkWidget stub implementing Widget trait"
```

---

### Task 29: SystemWidget stub

**Files:**
- Replace: `crates/sid-widgets/src/system.rs`
- Test: `crates/sid-widgets/tests/system.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-widgets/tests/system.rs`:

```rust
use sid_core::widget::Widget;
use sid_widgets::SystemWidget;

#[test]
fn system_widget_has_expected_id_and_title() {
    let w = SystemWidget::new();
    assert_eq!(w.id().as_str(), "system.root");
    assert_eq!(w.title(), "System");
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-widgets --test system`
Expected: compile error.

- [ ] **Step 3: Replace `crates/sid-widgets/src/system.rs`**

```rust
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

use crate::stub::ComingSoonBody;

pub struct SystemWidget {
    body: ComingSoonBody,
}

impl SystemWidget {
    pub fn new() -> Self {
        Self {
            body: ComingSoonBody::new(
                "System",
                "pinned configs, systemctl, custom quick-actions — coming in Plan 6",
            ),
        }
    }
}

impl Default for SystemWidget {
    fn default() -> Self { Self::new() }
}

impl Widget for SystemWidget {
    fn id(&self) -> WidgetId { WidgetId::new("system.root") }
    fn title(&self) -> &str { self.body.title() }
    fn render(&self, target: &mut dyn RenderTarget) { self.body.render(target); }
    fn handle_event(&mut self, ev: &Event, ctx: &mut WidgetCtx) -> EventOutcome {
        self.body.handle_event(ev, ctx)
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-widgets --test system`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): add SystemWidget stub implementing Widget trait"
```

---

### Task 30: SettingsWidget stub

**Files:**
- Replace: `crates/sid-widgets/src/settings.rs`
- Test: `crates/sid-widgets/tests/settings.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-widgets/tests/settings.rs`:

```rust
use sid_core::widget::Widget;
use sid_widgets::SettingsWidget;

#[test]
fn settings_widget_has_expected_id_and_title() {
    let w = SettingsWidget::new();
    assert_eq!(w.id().as_str(), "settings.root");
    assert_eq!(w.title(), "Settings");
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-widgets --test settings`
Expected: compile error.

- [ ] **Step 3: Replace `crates/sid-widgets/src/settings.rs`**

```rust
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

use crate::stub::ComingSoonBody;

pub struct SettingsWidget {
    body: ComingSoonBody,
}

impl SettingsWidget {
    pub fn new() -> Self {
        Self {
            body: ComingSoonBody::new(
                "Settings",
                "theme picker, keybind editor, behavior toggles — coming in Plan 7",
            ),
        }
    }
}

impl Default for SettingsWidget {
    fn default() -> Self { Self::new() }
}

impl Widget for SettingsWidget {
    fn id(&self) -> WidgetId { WidgetId::new("settings.root") }
    fn title(&self) -> &str { self.body.title() }
    fn render(&self, target: &mut dyn RenderTarget) { self.body.render(target); }
    fn handle_event(&mut self, ev: &Event, ctx: &mut WidgetCtx) -> EventOutcome {
        self.body.handle_event(ev, ctx)
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-widgets --test settings`
Expected: 1 passed.

- [ ] **Step 5: Run the full widgets test suite**

Run: `cargo test -p sid-widgets`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): add SettingsWidget stub implementing Widget trait"
```

---

## Phase H — App + event loop

### Task 31: App struct + initialization

**Files:**
- Create: `crates/sid-core/src/app.rs`
- Modify: `crates/sid-core/src/lib.rs`
- Test: `crates/sid-core/tests/app_init.rs`

The `App` owns the TabManager, KeybindMap, ActionRegistry, CommandPalette, and a borrow-ish handle on a `Store`. It does NOT own the runtime (Tokio) or the terminal backend (Ratatui); the binary crate wires those in.

- [ ] **Step 1: Write the failing test**

Create `crates/sid-core/tests/app_init.rs`:

```rust
use std::sync::mpsc;

use sid_core::action::{Action, ActionRegistry};
use sid_core::app::App;
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::keybind::KeybindMap;
use sid_core::layout::Layout;
use sid_core::tab::{Tab, TabId, TabManager};
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

struct W(&'static str);
impl Widget for W {
    fn id(&self) -> WidgetId { WidgetId::new(self.0) }
    fn title(&self) -> &str { self.0 }
    fn render(&self, _: &mut dyn RenderTarget) {}
    fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
}

fn tab(id: &'static str, title: &'static str, w: &'static str) -> Tab {
    Tab {
        id: TabId::new(id),
        title: title.into(),
        layout: Layout::Single(Box::new(W(w))),
        hotkey: None,
    }
}

#[test]
fn app_initializes_with_tabs_keybinds_and_registry() {
    let tabs = TabManager::new(vec![tab("a", "A", "wa")]);
    let kb = KeybindMap::cosmos_default();
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("app.quit", "Quit"));
    let app = App::new(tabs, kb, reg);
    assert_eq!(app.tabs().active().id.as_str(), "a");
    assert!(app.actions().get(&sid_core::action::ActionId::new("app.quit")).is_some());
    assert!(!app.is_quitting());
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-core --test app_init`
Expected: compile error.

- [ ] **Step 3: Create `crates/sid-core/src/app.rs`**

```rust
use std::sync::mpsc::{channel, Receiver, Sender};

use crate::action::{ActionId, ActionRegistry};
use crate::keybind::KeybindMap;
use crate::palette::CommandPalette;
use crate::tab::TabManager;

pub struct App {
    tabs: TabManager,
    keybinds: KeybindMap,
    actions: ActionRegistry,
    palette: CommandPalette,
    action_tx: Sender<String>,
    action_rx: Receiver<String>,
    quit: bool,
}

impl App {
    pub fn new(tabs: TabManager, keybinds: KeybindMap, actions: ActionRegistry) -> Self {
        let (action_tx, action_rx) = channel();
        Self {
            tabs,
            keybinds,
            actions,
            palette: CommandPalette::new(),
            action_tx,
            action_rx,
            quit: false,
        }
    }

    pub fn tabs(&self) -> &TabManager { &self.tabs }
    pub fn tabs_mut(&mut self) -> &mut TabManager { &mut self.tabs }
    pub fn keybinds(&self) -> &KeybindMap { &self.keybinds }
    pub fn actions(&self) -> &ActionRegistry { &self.actions }
    pub fn palette(&self) -> &CommandPalette { &self.palette }
    pub fn palette_mut(&mut self) -> &mut CommandPalette { &mut self.palette }
    pub fn is_quitting(&self) -> bool { self.quit }

    /// Sender clones can be handed to widgets via WidgetCtx::new.
    pub fn action_tx(&self) -> Sender<String> { self.action_tx.clone() }

    /// Drain queued action IDs emitted by widgets since the last call.
    pub fn drain_pending_actions(&mut self) -> Vec<ActionId> {
        let mut out = Vec::new();
        while let Ok(id) = self.action_rx.try_recv() {
            out.push(ActionId::new(id));
        }
        out
    }

    pub fn request_quit(&mut self) { self.quit = true; }
}
```

- [ ] **Step 4: Add to `lib.rs`**

Modify `crates/sid-core/src/lib.rs` — add `pub mod app;` and `pub use app::App;`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p sid-core --test app_init`
Expected: 1 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add App struct holding tabs/keybinds/actions/palette/quit-state"
```

---

### Task 32: StatePersister with debounce

**Files:**
- Create: `crates/sid-core/src/persister.rs`
- Modify: `crates/sid-core/src/lib.rs`
- Test: `crates/sid-core/tests/persister.rs`

A small in-memory marker that batches "I changed something" notifications; the actual writes happen in the binary which holds the `Store`. We keep this in `sid-core` so it doesn't depend on the storage backend.

- [ ] **Step 1: Write the failing test**

Create `crates/sid-core/tests/persister.rs`:

```rust
use std::time::Duration;

use sid_core::persister::StatePersister;

#[test]
fn mark_dirty_within_debounce_does_not_trigger() {
    let mut p = StatePersister::new(Duration::from_millis(50));
    p.mark_dirty();
    assert!(!p.should_flush());
}

#[test]
fn after_debounce_returns_true() {
    let mut p = StatePersister::new(Duration::from_millis(5));
    p.mark_dirty();
    std::thread::sleep(Duration::from_millis(15));
    assert!(p.should_flush());
}

#[test]
fn clean_means_no_flush_needed() {
    let mut p = StatePersister::new(Duration::from_millis(1));
    assert!(!p.should_flush());
    std::thread::sleep(Duration::from_millis(5));
    assert!(!p.should_flush());
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-core --test persister`
Expected: compile error.

- [ ] **Step 3: Create `crates/sid-core/src/persister.rs`**

```rust
use std::time::{Duration, Instant};

pub struct StatePersister {
    debounce: Duration,
    dirty_since: Option<Instant>,
}

impl StatePersister {
    pub fn new(debounce: Duration) -> Self {
        Self { debounce, dirty_since: None }
    }

    pub fn mark_dirty(&mut self) {
        if self.dirty_since.is_none() {
            self.dirty_since = Some(Instant::now());
        }
    }

    /// Returns true if a flush is due; consumes the dirty marker.
    pub fn should_flush(&mut self) -> bool {
        match self.dirty_since {
            Some(t) if t.elapsed() >= self.debounce => {
                self.dirty_since = None;
                true
            }
            _ => false,
        }
    }

    pub fn is_dirty(&self) -> bool { self.dirty_since.is_some() }
}
```

- [ ] **Step 4: Add to `lib.rs`**

Modify `crates/sid-core/src/lib.rs` — add `pub mod persister;` and `pub use persister::StatePersister;`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p sid-core --test persister`
Expected: 3 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add StatePersister with debounced dirty-flush gating"
```

---

### Task 33: App.handle_event — global dispatch

**Files:**
- Modify: `crates/sid-core/src/app.rs`
- Test: `crates/sid-core/tests/app_dispatch.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sid-core/tests/app_dispatch.rs`:

```rust
use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::action::{Action, ActionRegistry};
use sid_core::app::{App, Dispatch};
use sid_core::event::{Event, KeyChord};
use sid_core::keybind::KeybindMap;
use sid_core::layout::Layout;
use sid_core::tab::{Tab, TabId, TabManager};
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
use sid_core::context::WidgetCtx;

struct W(&'static str);
impl Widget for W {
    fn id(&self) -> WidgetId { WidgetId::new(self.0) }
    fn title(&self) -> &str { self.0 }
    fn render(&self, _: &mut dyn RenderTarget) {}
    fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
}
fn t(id: &'static str, title: &'static str) -> Tab {
    Tab { id: TabId::new(id), title: title.into(), layout: Layout::Single(Box::new(W(id))), hotkey: None }
}

#[test]
fn ctrl_right_advances_tab() {
    let tabs = TabManager::new(vec![t("a", "A"), t("b", "B")]);
    let mut app = App::new(tabs, KeybindMap::cosmos_default(), ActionRegistry::new());
    let chord = KeyChord::new(KeyCode::Right, KeyModifiers::CONTROL);
    let ev = Event::Key(chord);
    let _ = app.handle_event(&ev);
    assert_eq!(app.tabs().active().id.as_str(), "b");
}

#[test]
fn ctrl_q_sets_quit_flag() {
    let tabs = TabManager::new(vec![t("a", "A")]);
    let mut app = App::new(tabs, KeybindMap::cosmos_default(), ActionRegistry::new());
    let chord = KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    let _ = app.handle_event(&Event::Key(chord));
    assert!(app.is_quitting());
}

#[test]
fn ctrl_f_opens_palette() {
    let tabs = TabManager::new(vec![t("a", "A")]);
    let mut app = App::new(tabs, KeybindMap::cosmos_default(), ActionRegistry::new());
    assert!(!app.palette().is_open());
    let chord = KeyChord::new(KeyCode::Char('f'), KeyModifiers::CONTROL);
    let _ = app.handle_event(&Event::Key(chord));
    assert!(app.palette().is_open());
}

#[test]
fn ctrl_2_jumps_to_second_tab() {
    let tabs = TabManager::new(vec![t("a", "A"), t("b", "B"), t("c", "C")]);
    let mut app = App::new(tabs, KeybindMap::cosmos_default(), ActionRegistry::new());
    let chord = KeyChord::new(KeyCode::Char('2'), KeyModifiers::CONTROL);
    let _ = app.handle_event(&Event::Key(chord));
    assert_eq!(app.tabs().active().id.as_str(), "b");
}
```

- [ ] **Step 2: Run — should fail**

Run: `cargo test -p sid-core --test app_dispatch`
Expected: compile error (`handle_event` and `Dispatch` not yet present).

- [ ] **Step 3: Extend `crates/sid-core/src/app.rs`**

Add at the bottom of `app.rs`:

```rust
use crate::action::ActionId;
use crate::event::Event;

/// What the caller (the binary's runtime) should do next.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Dispatch {
    /// No further action; redraw if state changed.
    Continue,
    /// Quit the application cleanly.
    Quit,
}

impl App {
    /// Top-level dispatch. Handles palette input first if open, then global
    /// keybinds, then falls through to the active widget. Returns the next
    /// step for the runtime.
    pub fn handle_event(&mut self, ev: &Event) -> Dispatch {
        if self.palette.is_open() {
            if let Event::Key(chord) = ev {
                match chord.code {
                    crossterm::event::KeyCode::Esc => { self.palette.close(); return Dispatch::Continue; }
                    crossterm::event::KeyCode::Enter => {
                        if let Some(action) = self.palette.current(&self.actions).cloned() {
                            self.palette.close();
                            self.run_action(&action.id);
                        }
                        return Dispatch::Continue;
                    }
                    crossterm::event::KeyCode::Up => { self.palette.cursor_up(&self.actions); return Dispatch::Continue; }
                    crossterm::event::KeyCode::Down => { self.palette.cursor_down(&self.actions); return Dispatch::Continue; }
                    crossterm::event::KeyCode::Backspace => { self.palette.backspace(); return Dispatch::Continue; }
                    crossterm::event::KeyCode::Char(c) if chord.mods == crossterm::event::KeyModifiers::NONE
                        || chord.mods == crossterm::event::KeyModifiers::SHIFT => {
                        self.palette.input(&c.to_string());
                        return Dispatch::Continue;
                    }
                    _ => return Dispatch::Continue,
                }
            }
        }

        if let Event::Key(chord) = ev {
            if let Some(action) = self.keybinds.lookup(chord).cloned() {
                return self.run_action(&action);
            }
        }

        // Forward to the active widget (single-widget tab in v1).
        if let Some(widget) = self.tabs.active_mut().layout.iter_widgets_mut().next() {
            let tx = self.action_tx.clone();
            let mut ctx = crate::context::WidgetCtx::new(tx);
            let _ = widget.handle_event(ev, &mut ctx);
        }

        // Drain anything the widget emitted.
        for id in self.drain_pending_actions() {
            self.run_action(&id);
        }

        Dispatch::Continue
    }

    fn run_action(&mut self, id: &ActionId) -> Dispatch {
        match id.as_str() {
            "app.quit" => {
                self.quit = true;
                Dispatch::Quit
            }
            "palette.open" => {
                self.palette.open();
                Dispatch::Continue
            }
            "tabs.next" => {
                self.tabs.next();
                Dispatch::Continue
            }
            "tabs.prev" => {
                self.tabs.prev();
                Dispatch::Continue
            }
            s if s.starts_with("tabs.jump.") => {
                if let Some(num) = s.strip_prefix("tabs.jump.").and_then(|n| n.parse::<usize>().ok()) {
                    self.tabs.jump(num.saturating_sub(1));
                }
                Dispatch::Continue
            }
            "app.open_settings" => {
                self.tabs.switch_to(&crate::tab::TabId::new("settings"));
                Dispatch::Continue
            }
            // No-ops in this plan; handled in later plans.
            "tab.detach" | "tab.attach" | "tab.reload" => Dispatch::Continue,
            _ => {
                tracing::warn!(action = %id, "unknown action");
                Dispatch::Continue
            }
        }
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p sid-core --test app_dispatch`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): App.handle_event dispatches via palette/keybinds/widget + action runner"
```

---

### Task 34: tab nav handler – already covered by Task 33

This task was folded into Task 33's keybind dispatch (`tabs.next`, `tabs.prev`, `tabs.jump.N`). Verify by running all `sid-core` tests.

- [ ] **Step 1: Run `sid-core` tests**

Run: `cargo test -p sid-core`
Expected: all tests pass.

No commit needed for this task.

---

### Task 35: CommandPalette wiring – already covered by Task 33

`palette.open` / `Ctrl+F` is bound in cosmos_default and dispatched by `run_action`. Verify and move on.

- [ ] **Step 1: Run `sid-core` tests**

Run: `cargo test -p sid-core`
Expected: pass.

No commit needed.

---

### Task 36: Session restore prompt on launch

**Files:**
- Create: `crates/sid-core/src/restore.rs`
- Modify: `crates/sid-core/src/lib.rs`
- Test: `crates/sid-core/tests/restore.rs`

A pure decision function: given the previously-known session and the current epoch, return a `RestoreDecision` the binary acts on (no I/O, no UI).

- [ ] **Step 1: Write the failing test**

Create `crates/sid-core/tests/restore.rs`:

```rust
use sid_core::restore::{decide, RestoreDecision};

#[derive(Clone, Debug)]
struct Sess {
    last_active_secs_ago: i64,
    cleanly_ended: bool,
}

fn s(secs: i64, ended: bool) -> Sess {
    Sess { last_active_secs_ago: secs, cleanly_ended: ended }
}

#[test]
fn no_prior_session_means_new() {
    let d = decide(None::<Sess>, 60);
    assert_eq!(d, RestoreDecision::StartNew);
}

#[test]
fn clean_shutdown_means_new_session() {
    let d = decide(Some(s(10, true)), 60);
    assert_eq!(d, RestoreDecision::StartNew);
}

#[test]
fn fresh_dirty_session_offers_resume() {
    let d = decide(Some(s(30, false)), 60);
    assert_eq!(d, RestoreDecision::OfferResume);
}

#[test]
fn stale_dirty_session_is_treated_as_history() {
    let d = decide(Some(s(120, false)), 60);
    assert_eq!(d, RestoreDecision::StartNew);
}
```

Note: this test references a trait we haven't defined; replace `Sess` references with the concrete `decide` API below.

- [ ] **Step 2: Replace the test with the final form**

```rust
use sid_core::restore::{decide, RestoreDecision, SessionView};

fn s(last_active_secs_ago: u64, cleanly_ended: bool) -> SessionView {
    SessionView { last_active_secs_ago, cleanly_ended }
}

#[test]
fn no_prior_session_means_new() {
    let d = decide(None, 60);
    assert_eq!(d, RestoreDecision::StartNew);
}

#[test]
fn clean_shutdown_means_new_session() {
    let d = decide(Some(s(10, true)), 60);
    assert_eq!(d, RestoreDecision::StartNew);
}

#[test]
fn fresh_dirty_session_offers_resume() {
    let d = decide(Some(s(30, false)), 60);
    assert_eq!(d, RestoreDecision::OfferResume);
}

#[test]
fn stale_dirty_session_is_treated_as_history() {
    let d = decide(Some(s(120, false)), 60);
    assert_eq!(d, RestoreDecision::StartNew);
}
```

- [ ] **Step 3: Run — should fail**

Run: `cargo test -p sid-core --test restore`
Expected: compile error.

- [ ] **Step 4: Create `crates/sid-core/src/restore.rs`**

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreDecision {
    /// Start a fresh session. Default for clean shutdowns and no prior session.
    StartNew,
    /// Prompt the user; previous session is recent and was not cleanly ended.
    OfferResume,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionView {
    pub last_active_secs_ago: u64,
    pub cleanly_ended: bool,
}

/// Decide whether to offer to resume a previous session.
///
/// `fresh_threshold_secs` — if `last_active_secs_ago` exceeds this, the
/// previous session is treated as historical (start new).
pub fn decide(prev: Option<SessionView>, fresh_threshold_secs: u64) -> RestoreDecision {
    match prev {
        None => RestoreDecision::StartNew,
        Some(s) if s.cleanly_ended => RestoreDecision::StartNew,
        Some(s) if s.last_active_secs_ago > fresh_threshold_secs => RestoreDecision::StartNew,
        Some(_) => RestoreDecision::OfferResume,
    }
}
```

- [ ] **Step 5: Add to `lib.rs`**

Modify `crates/sid-core/src/lib.rs` — add `pub mod restore;` and `pub use restore::{decide, RestoreDecision, SessionView};`.

- [ ] **Step 6: Run tests**

Run: `cargo test -p sid-core --test restore`
Expected: 4 passed.

- [ ] **Step 7: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): add pure RestoreDecision helper for session-restore prompt"
```

---

## Phase I — `sid` binary

### Task 37: CLI args (clap)

**Files:**
- Create: `crates/sid/Cargo.toml`
- Create: `crates/sid/src/main.rs`
- Test: `crates/sid/tests/cli.rs`

- [ ] **Step 1: Create `crates/sid/Cargo.toml`**

```toml
[package]
name = "sid"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[[bin]]
name = "sid"
path = "src/main.rs"

[dependencies]
sid-core.workspace = true
sid-ui.workspace = true
sid-store.workspace = true
sid-job.workspace = true
sid-widgets.workspace = true

ratatui.workspace = true
crossterm.workspace = true
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "signal", "time"] }
clap.workspace = true

anyhow.workspace = true
color-eyre.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
tracing-appender.workspace = true
directories.workspace = true
```

- [ ] **Step 2: Write the failing test**

Create `crates/sid/tests/cli.rs`:

```rust
use std::process::Command;

#[test]
fn sid_help_runs() {
    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--help")
        .output()
        .expect("run sid --help");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("sid"));
    assert!(stdout.contains("--db"));
}

#[test]
fn sid_version_runs() {
    let out = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--version")
        .output()
        .expect("run sid --version");
    assert!(out.status.success());
}
```

- [ ] **Step 3: Run — should fail**

Run: `cargo test -p sid --test cli`
Expected: compile error (no `main.rs` yet).

- [ ] **Step 4: Create `crates/sid/src/main.rs` (CLI-only stub)**

```rust
use std::path::PathBuf;

use clap::Parser;

mod runtime;
mod wire;

#[derive(Parser, Debug)]
#[command(name = "sid", version, about = "a fast, focused TUI cockpit for developer workflow")]
struct Cli {
    /// Override the default redb file path.
    #[arg(long)]
    db: Option<PathBuf>,

    /// Start in this tab if present (id: workspaces, ssh, database, network, system, settings).
    #[arg(long)]
    start_tab: Option<String>,
}

fn main() -> anyhow::Result<()> {
    color_eyre::install().ok();
    let cli = Cli::parse();
    // For Task 37 we only verify CLI parsing works. Tasks 38–39 actually run the TUI.
    if cli.db.is_some() || cli.start_tab.is_some() {
        // exercised by tests; no-op here.
    }
    println!("sid {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
```

Create placeholders for the modules the file declares:

`crates/sid/src/runtime.rs`:

```rust
//! Placeholder — filled in Task 38.
```

`crates/sid/src/wire.rs`:

```rust
//! Placeholder — filled in Task 39.
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p sid --test cli`
Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/sid
git commit -m "feat(bin): scaffold sid binary with clap CLI (--db, --start-tab, --help, --version)"
```

---

### Task 38: Runtime — Tokio + crossterm event source

**Files:**
- Replace: `crates/sid/src/runtime.rs`

- [ ] **Step 1: Replace `crates/sid/src/runtime.rs`**

```rust
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event as CtEvent, EventStream};
use futures::StreamExt;
use sid_core::event::Event as SidEvent;
use tokio::sync::mpsc::{Receiver, Sender};

/// Spawn a task that translates crossterm events into SidEvents on a channel.
/// Also emits a `Tick` every `tick_rate`.
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

pub fn make_channel() -> (Sender<SidEvent>, Receiver<SidEvent>) {
    tokio::sync::mpsc::channel(64)
}

/// Convenience for tests / one-shot drivers.
pub async fn next_event(rx: &mut Receiver<SidEvent>) -> Result<SidEvent> {
    rx.recv().await.ok_or_else(|| anyhow::anyhow!("event stream closed"))
}
```

- [ ] **Step 2: Build**

Run: `cargo build -p sid`
Expected: success.

- [ ] **Step 3: Commit**

```bash
git add crates/sid/src/runtime.rs
git commit -m "feat(bin): add runtime — tokio event pump fed by crossterm EventStream + ticker"
```

---

### Task 39: Wire — build App with concrete impls + main loop

**Files:**
- Replace: `crates/sid/src/wire.rs`
- Replace: `crates/sid/src/main.rs`

- [ ] **Step 1: Replace `crates/sid/src/wire.rs`**

```rust
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use directories::ProjectDirs;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;
use ratatui::{Frame, Terminal};
use sid_core::action::{Action, ActionRegistry};
use sid_core::app::{App, Dispatch};
use sid_core::event::Event as SidEvent;
use sid_core::keybind::KeybindMap;
use sid_core::layout::Layout;
use sid_core::tab::{Tab, TabId, TabManager};
use sid_core::widget::Widget;
use sid_core::Result as SidResult;
use sid_store::{now_epoch, OpenStore, RedbStore, SessionRecord, Store};
use sid_ui::helpers::styled_block;
use sid_ui::themes::cosmos;
use sid_widgets::{DatabaseWidget, NetworkWidget, SettingsWidget, SshWidget, SystemWidget, WorkspacesWidget};
use tokio::sync::mpsc::Receiver;

pub struct SidApp {
    pub app: App,
    pub store: Arc<RedbStore>,
    pub session_id: String,
}

pub fn db_path(override_path: Option<PathBuf>) -> PathBuf {
    if let Some(p) = override_path { return p; }
    if let Some(dirs) = ProjectDirs::from("dev", "sid", "sid") {
        let data = dirs.data_local_dir().to_path_buf();
        std::fs::create_dir_all(&data).ok();
        return data.join("sid.redb");
    }
    PathBuf::from("./sid.redb")
}

pub fn build_app(start_tab: Option<&str>) -> App {
    let tabs = TabManager::new(vec![
        tab("workspaces", "Workspaces", Box::new(WorkspacesWidget::new()), Some('1')),
        tab("ssh", "SSH", Box::new(SshWidget::new()), Some('2')),
        tab("database", "Database", Box::new(DatabaseWidget::new()), Some('3')),
        tab("network", "Network", Box::new(NetworkWidget::new()), Some('4')),
        tab("system", "System", Box::new(SystemWidget::new()), Some('5')),
        tab("settings", "Settings", Box::new(SettingsWidget::new()), Some('6')),
    ]);
    let kb = KeybindMap::cosmos_default();
    let mut reg = ActionRegistry::new();
    for a in ["app.quit", "palette.open", "tabs.next", "tabs.prev", "app.open_settings", "tab.detach", "tab.attach", "tab.reload"] {
        reg.register(Action::new(a, pretty_label(a)));
    }
    for i in 1..=6 {
        reg.register(Action::new(format!("tabs.jump.{i}"), format!("Jump to tab {i}")));
    }
    let mut app = App::new(tabs, kb, reg);
    if let Some(id) = start_tab {
        let _ = app.tabs_mut().switch_to(&TabId::new(id));
    }
    app
}

fn tab(id: &str, title: &str, widget: Box<dyn Widget>, hotkey: Option<char>) -> Tab {
    Tab {
        id: TabId::new(id),
        title: title.to_string(),
        layout: Layout::Single(widget),
        hotkey,
    }
}

fn pretty_label(action_id: &str) -> String {
    match action_id {
        "app.quit" => "Quit".into(),
        "palette.open" => "Open command palette".into(),
        "tabs.next" => "Next tab".into(),
        "tabs.prev" => "Previous tab".into(),
        "app.open_settings" => "Open Settings".into(),
        "tab.detach" => "Detach tab (Plan 8)".into(),
        "tab.attach" => "Attach widget (Plan 8)".into(),
        "tab.reload" => "Reload tab data".into(),
        other => other.to_string(),
    }
}

pub fn save_active_tab(store: &dyn Store, session_id: &str, app: &App) -> SidResult<()> {
    let sess = SessionRecord {
        id: session_id.to_string(),
        started_at: now_epoch(),
        last_active: now_epoch(),
        ended_at: None,
        active_tab: Some(app.tabs().active().id.clone()),
        open_tabs: app.tabs().tabs().iter().map(|t| t.id.clone()).collect(),
    };
    store.upsert_session(&sess)
}

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let theme = cosmos();
    let size = frame.area();
    // Top bar with tab labels.
    let labels: String = app
        .tabs()
        .tabs()
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let marker = if i == app.tabs().active_index() { '●' } else { '·' };
            format!("{marker} {} ", t.title)
        })
        .collect();
    let bar = Paragraph::new(labels).block(styled_block(&theme, "sid"));
    let bar_rect = Rect { x: 0, y: 0, width: size.width, height: 3 };
    frame.render_widget(bar, bar_rect);

    // Active widget body — for stubs we render a centered placeholder.
    let body_rect = Rect { x: 0, y: 3, width: size.width, height: size.height.saturating_sub(3) };
    let title = app.tabs().active().title.clone();
    let body = Paragraph::new(format!("{title}\n\n(coming soon)")).block(styled_block(&theme, "panel"));
    frame.render_widget(body, body_rect);

    // Palette overlay if open.
    if app.palette().is_open() {
        let overlay_rect = centered(size, 60, 40);
        let mut lines = vec![format!("> {}", app.palette().query())];
        for (i, a) in app.palette().matches(app.actions()).into_iter().enumerate() {
            let prefix = if i == app.palette().selected_index() { ">" } else { " " };
            lines.push(format!("{prefix} {} ({})", a.label, a.id));
        }
        let p = Paragraph::new(lines.join("\n")).block(styled_block(&theme, "command palette"));
        frame.render_widget(p, overlay_rect);
    }
}

fn centered(area: Rect, pct_w: u16, pct_h: u16) -> Rect {
    let w = area.width * pct_w / 100;
    let h = area.height * pct_h / 100;
    let x = (area.width - w) / 2;
    let y = (area.height - h) / 2;
    Rect { x, y, width: w, height: h }
}

pub async fn run_event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    sid_app: &mut SidApp,
    rx: &mut Receiver<SidEvent>,
) -> Result<()> {
    let _ = save_active_tab(&*sid_app.store, &sid_app.session_id, &sid_app.app);
    loop {
        terminal.draw(|f| draw(f, &sid_app.app))?;
        let ev = match rx.recv().await {
            Some(e) => e,
            None => break,
        };
        let dispatch = sid_app.app.handle_event(&ev);
        let _ = save_active_tab(&*sid_app.store, &sid_app.session_id, &sid_app.app);
        if matches!(dispatch, Dispatch::Quit) {
            break;
        }
    }
    Ok(())
}
```

- [ ] **Step 2: Replace `crates/sid/src/main.rs`**

```rust
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use sid_store::{now_epoch, OpenStore, RedbStore, Store};
use tracing_subscriber::EnvFilter;

mod runtime;
mod wire;

#[derive(Parser, Debug)]
#[command(name = "sid", version, about = "a fast, focused TUI cockpit for developer workflow")]
struct Cli {
    /// Override the default redb file path.
    #[arg(long)]
    db: Option<PathBuf>,

    /// Start in this tab if present.
    #[arg(long)]
    start_tab: Option<String>,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    color_eyre::install().ok();
    install_tracing();
    let cli = Cli::parse();

    let path = wire::db_path(cli.db);
    let store = Arc::new(RedbStore::open(&path)?);

    // Start a new session record.
    let session_id = format!("sess-{}", now_epoch());
    let mut app = wire::build_app(cli.start_tab.as_deref());
    if let Some(prev) = store.current_session()? {
        let _ = app.tabs_mut().switch_to(&prev.active_tab.unwrap_or_else(|| sid_core::tab::TabId::new("workspaces")));
    }
    let mut sid_app = wire::SidApp { app, store: Arc::clone(&store), session_id: session_id.clone() };

    // Set up terminal.
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    // Event source.
    let (tx, mut rx) = runtime::make_channel();
    let pump = runtime::spawn_event_pump(tx, Duration::from_millis(250));

    // Run.
    let run_result = wire::run_event_loop(&mut terminal, &mut sid_app, &mut rx).await;
    pump.abort();

    // Restore terminal.
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Mark session ended.
    let _ = store.end_session(&session_id, now_epoch());

    run_result
}

fn install_tracing() {
    let filter = EnvFilter::try_from_env("SID_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
```

- [ ] **Step 3: Build**

Run: `cargo build -p sid`
Expected: success.

- [ ] **Step 4: Smoke test (manual; cannot fully run in CI)**

Run interactively: `cargo run -p sid` then press `Ctrl+→` `Ctrl+→` `Ctrl+1` `Ctrl+F` `Esc` `Ctrl+Q`.
Expected: TUI launches, tabs switch, palette opens & closes, app quits without panic. (Skip this step in non-interactive automation; the integration test in Task 40 covers the headless contract.)

- [ ] **Step 5: Commit**

```bash
git add crates/sid
git commit -m "feat(bin): wire RedbStore, App, widgets, event pump, and Ratatui render loop"
```

---

## Phase J — Integration tests + docs

### Task 40: Integration test — sid launches and quits cleanly

**Files:**
- Create: `crates/sid/tests/integration.rs`

We can't easily script a full TUI inside cargo-test, but we can verify the binary launches, ingests one Ctrl+Q via piped input, and exits 0. The test also confirms the redb file is created.

- [ ] **Step 1: Write the test**

Create `crates/sid/tests/integration.rs`:

```rust
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use tempfile::tempdir;

#[test]
fn sid_starts_and_exits_on_ctrl_q() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("sid.redb");

    let mut child = Command::new(env!("CARGO_BIN_EXE_sid"))
        .arg("--db").arg(&db)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn sid");

    // Give it a moment to set up the terminal.
    std::thread::sleep(Duration::from_millis(500));

    // Write Ctrl+Q.
    {
        let mut stdin = child.stdin.take().expect("stdin");
        // crossterm reads from raw stdin; on Linux Ctrl+Q is 0x11.
        stdin.write_all(&[0x11u8]).unwrap();
    }

    // Wait with a timeout.
    let start = std::time::Instant::now();
    loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => {
                assert!(status.success(), "sid exited with {status:?}");
                break;
            }
            None => {
                if start.elapsed() > Duration::from_secs(5) {
                    let _ = child.kill();
                    panic!("sid did not exit within 5s of Ctrl+Q");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    assert!(db.exists(), "redb file should have been created at {db:?}");
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p sid --test integration`
Expected: pass on Linux.

**Note for the engineer:** Some CI environments do not provide a tty; if `enable_raw_mode()` fails in such an environment, gate the integration test with `#[cfg(unix)]` and skip when `isatty(stdout)` is false (`if !atty::is(atty::Stream::Stdout) { return; }` — pulling in the `atty` crate via dev-dependencies if needed). For local dev this should "just work."

- [ ] **Step 3: Commit**

```bash
git add crates/sid
git commit -m "test(bin): add integration test — sid launches, accepts Ctrl+Q, exits 0"
```

---

### Task 41: README build instructions

**Files:**
- Modify: `README.md`

The README has a placeholder Quickstart section; replace it with the real build/test commands now that the foundation is wired.

- [ ] **Step 1: Find and replace the Quickstart section in `README.md`**

Replace:

```markdown
## Quickstart

> Not yet — building from the current commit does not produce a usable binary. See the foundation spec below for design and status.

Once shipped:

```sh
cargo install --path crates/sid
sid
```
```

with:

```markdown
## Quickstart

```sh
# Clone, build, run
git clone https://github.com/murphlmao/sid && cd sid
cargo build --release
./target/release/sid

# Or run from source
cargo run -p sid

# Tests
cargo test --workspace

# Override the DB location (otherwise XDG default applies)
sid --db /tmp/sid.redb
```

**Keybinds in this build:** `Ctrl+←/→` switch tabs · `Ctrl+1..6` jump · `Ctrl+F` command palette · `Ctrl+Q` quit · `Ctrl+,` open Settings.

> **What works in this build:** Foundation complete. Six tabs render as labelled stubs in the cosmos theme; navigation, command palette, theme, and active-tab persistence work. Real tab content arrives in subsequent plans.
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: replace Quickstart placeholder with real build instructions for Plan 1"
```

---

## Done criteria for Plan 1

- [ ] `cargo build --workspace` succeeds with no errors and no warnings.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- [ ] `cargo fmt --check` is clean.
- [ ] `cargo run -p sid` launches the TUI; you can switch all 6 tabs, open and close the palette, and quit with `Ctrl+Q`.
- [ ] Active tab persists across a restart.
- [ ] redb file is created at `~/.local/share/sid/sid.redb` (or `XDG_DATA_HOME` equivalent).
- [ ] README's Quickstart matches actual binary behavior.

---

## Self-review notes (run before requesting review)

1. **Spec coverage.** Plan 1 covers spec sections: tabs (stub), Widget trait + Layout, keybinds (defaults), command palette, theme system (cosmos + 3 alts), store (settings/sessions/widget_state), session persistence (heartbeat-less for now; full restore in Plan 7 alongside Settings UX), adapter trait shells. Items not yet covered: real tab content (Plans 2-6), full Settings (Plan 7), detach/IPC/multi-process (Plan 8), `query_history`/`themes`/`keybinds` tables (Plans 4/7).
2. **Type consistency.** Widget trait signatures match across `widget.rs`, `layout.rs`, `tab.rs`, `app.rs`, and all six widget impls in `sid-widgets`. `Action::new` matches `Action::new` in tests. `ActionId` / `TabId` / `WidgetId` all use the `new(impl Into<String>)` + `as_str()` API.
3. **No placeholders.** Every step has actual code/commands. The single forward reference is "Plan 8" / "Plan 2-7" in widget stub bodies — those are intentional and resolve later.
4. **Scope.** A focused-enough vertical slice that produces a runnable binary at the end. The engineer working through it will not need outside context beyond the spec doc.
