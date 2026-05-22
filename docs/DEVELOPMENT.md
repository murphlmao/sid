# Development

This document covers how to work on `sid` — extending it, adding
adapters, adding tabs, adding themes, evolving the storage layer, and
the workflow rules that keep the code base shaped the way it is.

## Workflow philosophy

[`CLAUDE.md`] is the **binding** rules document. Read it before
contributing — it lays out the testing rigour, adapter pattern
enforcement, and adversarial-thinking requirements that this project
treats as non-negotiable. The TL;DR: tests land in the same commit as
the code they cover, every public item gets a doc test, every external
dep hides behind a trait, and `clippy --all-targets --all-features --
-D warnings` must be green before declaring done.

If something is hard to test, restructure it. Never skip a test because
"the shape made it awkward".

[`CLAUDE.md`]: ../CLAUDE.md

## Project layout

```
sid/
├── Cargo.toml                # workspace manifest, pinned deps
├── deny.toml                 # cargo-deny config (licenses, sources, advisories)
├── rustfmt.toml              # formatting (stable + a few nightly-only fields)
├── .editorconfig             # whitespace rules
├── crates/
│   ├── sid-core/             # Widget, Layout, App, traits — no Ratatui/Tokio/redb
│   ├── sid-ui/               # Theme, Ratatui helpers
│   ├── sid-store/            # Store trait, RedbStore, codec
│   ├── sid-job/              # JobQueue + loom tests
│   ├── sid-widgets/          # Six v1 tabs
│   ├── sid-git/              # Git2Provider (libgit2)
│   └── sid/                  # binary entry point + wire.rs
└── docs/
    ├── ARCHITECTURE.md       # how it's built (read this first)
    ├── INSTALLATION.md
    ├── DEVELOPMENT.md        # you are here
    ├── TESTING.md
    ├── CONTRIBUTING.md
    ├── TROUBLESHOOTING.md
    └── superpowers/          # specs and plans (source of truth for design)
```

For the detailed crate-by-crate breakdown see
[ARCHITECTURE.md](ARCHITECTURE.md).

## Adding a new widget

Each tab body is a `Widget` impl. To add a new one:

1. **Create the module.** Add a file in `crates/sid-widgets/src/`, e.g.
   `agents.rs`, with a public `AgentsWidget` struct.

2. **Implement `Widget`.**
   ```rust
   use sid_core::context::WidgetCtx;
   use sid_core::event::Event;
   use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

   pub struct AgentsWidget {
       // your state
   }

   impl AgentsWidget {
       pub fn new() -> Self { Self {} }
   }

   impl Widget for AgentsWidget {
       fn id(&self) -> WidgetId { WidgetId::new("agents") }
       fn title(&self) -> &str { "Agents" }
       fn render(&self, _target: &mut dyn RenderTarget) {
           // hand off to sid-ui helpers
       }
       fn handle_event(&mut self, _ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
           EventOutcome::Bubble
       }
       // save_state / load_state default to empty; implement if your widget has persistent state
   }
   ```

3. **Export it.** In `crates/sid-widgets/src/lib.rs`:
   ```rust
   pub mod agents;
   pub use agents::AgentsWidget;
   ```

4. **Tests.** Add unit tests in `agents.rs` under `#[cfg(test)] mod
   tests`, plus an integration test in `crates/sid-widgets/tests/` if
   the widget has multi-component flows. Doc tests are required on
   every `pub fn`/`pub struct`. See [TESTING.md](TESTING.md) for the
   full checklist.

The widget is now ready to be wired into a tab. See
[Adding a new tab](#adding-a-new-tab) below.

## Adding a new adapter

An adapter is the trait-impl boundary between widget code and an
external library. The recipe:

1. **Define the trait in `sid-core::adapters`.** If you're adding a new
   surface (e.g., `BackupProvider`), create
   `crates/sid-core/src/adapters/backup.rs` and declare the trait. Use
   dyn-compatible signatures: take `&self` or `&mut self`, no generics
   in method positions, no `Self` in return types.

   ```rust
   // crates/sid-core/src/adapters/backup.rs
   use std::path::Path;
   use crate::SidError;

   pub trait BackupProvider: Send + Sync {
       fn snapshot(&self, target: &Path) -> Result<(), SidError>;
       fn list_snapshots(&self) -> Result<Vec<String>, SidError>;
   }
   ```

   Then wire it into `crates/sid-core/src/adapters/mod.rs`:
   ```rust
   pub mod backup;
   ```

2. **Create the impl crate.** Add `crates/sid-backup/` with a single
   external dep (e.g., `restic-rs`). Add it to the workspace `members`
   in the root `Cargo.toml` and to `[workspace.dependencies]`.

   ```toml
   # crates/sid-backup/Cargo.toml
   [package]
   name = "sid-backup"
   version.workspace = true
   edition.workspace = true

   [dependencies]
   sid-core.workspace = true
   restic-rs = "..."
   ```

3. **Implement the trait.** `crates/sid-backup/src/lib.rs`:
   ```rust
   use sid_core::adapters::backup::BackupProvider;
   use sid_core::SidError;
   use std::path::Path;

   pub struct ResticProvider { /* fields */ }

   impl BackupProvider for ResticProvider {
       fn snapshot(&self, target: &Path) -> Result<(), SidError> { todo!() }
       fn list_snapshots(&self) -> Result<Vec<String>, SidError> { todo!() }
   }
   ```

4. **Wire it in the binary.** Only `crates/sid/src/wire.rs` names
   concrete impls. Construct `ResticProvider` there and pass it into
   the widget that needs it (typically via `Arc<dyn BackupProvider>`).

5. **Tests.** Unit + integration tests in `crates/sid-backup/`,
   adversarial coverage for the external library's failure modes, and
   a contract test stub in `crates/sid-core/tests/` that asserts the
   trait shape stays dyn-compatible.

The rule that keeps the adapter pattern from rotting: **no other crate
adds `restic-rs` to its dependencies.** If you find yourself wanting to,
that's a design bug — the trait surface is missing something.

## Adding a new tab

A tab is a `Tab` value registered with the `TabManager` in
`crates/sid/src/wire.rs`. The pattern from the existing six:

```rust
// In wire::build_app
let tabs = TabManager::new(vec![
    tab("workspaces", "Workspaces", Box::new(WorkspacesWidget::new()), Some('1')),
    tab("ssh",        "SSH",        Box::new(SshWidget::new()),        Some('2')),
    // ...
    tab("agents",     "Agents",     Box::new(AgentsWidget::new()),     Some('7')),
]);
```

A few things to do alongside:

- **Update the default keybinds.** In
  `crates/sid-core/src/keybind.rs`'s `cosmos_default`, extend the
  `Ctrl+1..6` loop to cover your new tab number. The Settings tab
  keybind editor (Plan 7) will eventually surface this.
- **Action registry.** Register tab-jump actions
  (`tabs.jump.7`, etc.) in `crates/sid/src/wire.rs` so the command
  palette can find them.
- **Update the README's "What's inside" table.**

The new tab participates in `Ctrl+←/→` and `Ctrl+F` palette filtering
automatically.

## Theming

Themes live in `crates/sid-ui/src/themes.rs`. Each is a `Theme` value:

```rust
use sid_ui::theme::{Glyphs, Palette, Theme};

pub fn midnight() -> Theme {
    Theme {
        name: "midnight",
        palette: Palette {
            background: "#000010",
            surface:    "#0a0a1f",
            foreground: "#e0e0f0",
            muted:      "#404060",
            accent_primary: "#8b5cf6",
            accent_success: "#a8d8e8",
            accent_warning: "#e8b04a",
            accent_error:   "#ff5570",
            border:     "#1a1a30",
        },
        glyphs: Glyphs {
            star: '✦',
            dot:  '·',
            bullet: '★',
        },
    }
}
```

Export it from `crates/sid-ui/src/lib.rs` and add it to the built-in
registry. End users can also create custom themes in-app via the
Settings tab (once Plan 7 lands), stored in the `themes` redb table.

### Theme tests

Snapshot tests live in `crates/sid-ui/tests/`. After adding a theme:

```rust
#[test]
fn midnight_palette_snapshot() {
    insta::assert_yaml_snapshot!(midnight().palette);
}
```

Run with `cargo test -p sid-ui`. The first run creates the snapshot
under `crates/sid-ui/tests/snapshots/`; subsequent runs assert it stays
stable. Review pending snapshots with `cargo insta review`.

## Storage migrations

Every postcard blob written to redb is prefixed with a single `u8`
version byte. The codec helpers in `crates/sid-store/src/codec.rs`
enforce this:

```rust
use sid_store::codec::{encode_versioned, decode_versioned};

let bytes  = encode_versioned(1u8, &my_value)?;
let (ver, decoded): (u8, MyValue) = decode_versioned(&bytes)?;
```

The convention:

- **Bump the version byte** every time the on-disk shape of a value
  changes. Even adding a field counts if the postcard wire format changes.
- **Dispatch by version on read.** The `Store` impl pattern matches on
  the version byte and selects the right deserializer:
  ```rust
  match decode_versioned::<NewShape>(&raw) {
      Ok((1, v)) => v,
      Err(_) => {
          // try v0
          let (0, old): (u8, OldShape) = decode_versioned(&raw)?;
          old.migrate()
      }
      Ok((v, _)) => return Err(SidError::Storage(format!("unknown version {v}"))),
  }
  ```
- **Add a migration test** in `crates/sid-store/tests/` that writes the
  old shape and asserts the new code reads it correctly. These tests
  are the regression net for "did I forget to handle the old format?".
- **Forward compatibility** is required for anything a detached process
  might write (Plan 8). Backward compatibility is required for any
  table a long-running prior version may have populated.

When you change a schema in a way that requires migration, note it in
the commit body and reference the migration test by name.

## Subagent-driven development

Initial bring-up used the
[`superpowers:subagent-driven-development`] skill: one Claude Code
subagent per task from the plan documents in `docs/superpowers/plans/`,
running in parallel where the task graph allows. Each subagent has a
scoped budget and reports back into the main session for review and
commit. The pattern is overkill for small changes; it shines on
multi-week scaffolding work where the task graph is mostly independent.

If you continue that pattern, the plan docs are the source of truth for
task ordering; spec docs are the source of truth for design intent.
Don't silently diverge from either — update the doc in the same PR.

[`superpowers:subagent-driven-development`]: superpowers/

## Conventional commits

The commit prefix grammar:

| Prefix | Use |
|:---|:---|
| `feat(scope): ...` | New feature. `scope` is the crate name (`core`, `widgets`, `store`, etc.) |
| `fix(scope): ...` | Bug fix. Always paired with a regression test in the same commit. |
| `test(scope): ...` | Test-only changes (new test, expanded coverage). |
| `refactor(scope): ...` | No behaviour change; structural cleanup. |
| `perf(scope): ...` | Performance-motivated change. Include the benchmark delta in the body. |
| `docs: ...` | Documentation only. No scope. |
| `chore: ...` | Tooling, dependencies, configs. No scope. |
| `build: ...` | Build-system changes (Cargo features, profiles). |
| `ci: ...` | CI-only changes (`.github/workflows/`). |

A **good** commit body explains the *why* and lists the failure modes
considered. Example:

```
feat(store): add workspace registry with absolute-path canonicalization

Workspaces are keyed by their canonical absolute path so registering
the same dir twice via different relative paths is a no-op. Failure
modes tested: symlink loops, paths with trailing slashes, non-UTF-8
bytes in the path (Linux), and registering a path that doesn't exist
yet (allowed; useful for "I'm about to clone").
```

**Do not** add a `Co-Authored-By: Claude` trailer. Murphy explicitly
opted out — the author is the human running the session.

## Branch policy

This is a single-developer repo right now. The convention:

- **`main`** is the integration branch. CI runs on every push.
- **Feature branches are optional** — small fixes go straight to `main`
  once they're clean.
- **PRs run CI.** When you do open one, CI checks `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --workspace --all-features`, and `cargo deny check`.
- **No force-push to main.** Force-pushing a feature branch is fine.

If you're a contributor (not Murphy), open a PR. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the full rules.

## Common commands cheat sheet

```sh
# All tests across the workspace
cargo test --workspace --all-features

# Single crate
cargo test -p sid-core
cargo test -p sid-store --tests
cargo test -p sid-widgets --test workspaces

# Doc tests only
cargo test --doc -p sid-core

# A single test by name
cargo test -p sid-core fuzzy_search_returns_matches

# Lint + format gates (must pass before declaring done)
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check

# Build the release binary
cargo build --release

# Benchmarks (Criterion — see TESTING.md)
cargo bench -p sid-job --no-run             # compile only
cargo bench -p sid-job                      # run

# Loom tests (concurrency model checker)
RUSTFLAGS="--cfg loom" cargo test --test loom_concurrency -p sid-job

# Insta snapshots — review before accepting
cargo insta review

# Coverage (install once: cargo install cargo-llvm-cov)
cargo llvm-cov --workspace --branch --html
xdg-open target/llvm-cov/html/index.html

# Dependency audit
cargo deny check

# Update a single dependency (avoid `cargo update` without args)
cargo update -p tokio --precise 1.47.0
```

For the full testing playbook see [TESTING.md](TESTING.md).

## Profiling with dhat

`dhat` is a workspace dev-dependency in `Cargo.toml`. Use it to find
allocation hot spots in the render loop or any path that shows up
slower than its criterion budget. dhat measures allocation churn
(not just resident memory): the heap allocator's lock contention is
what causes UI jank, not memory pressure.

### One-shot profile

1. Enable the `dhat-heap` feature on the binary crate temporarily:

   ```toml
   # crates/sid/Cargo.toml
   [features]
   default = []
   dhat-heap = ["dhat"]
   ```

2. Add the dhat profiler initialization to `crates/sid/src/main.rs`
   inside a `#[cfg(feature = "dhat-heap")]` block at the top of `main`:

   ```rust
   #[cfg(feature = "dhat-heap")]
   let _profiler = dhat::Profiler::new_heap();
   ```

3. Run a typical session and exit cleanly:

   ```bash
   cargo run --features dhat-heap -- --skip-discovery
   # Interact for ~30 s: switch tabs, open a modal, close it,
   # then quit (Ctrl+Q or your bound quit chord).
   ```

4. dhat writes `dhat-heap.json` to the project root. Open it in the
   viewer (no installation required):

   - <https://nnethercote.github.io/dh_view/dh_view.html> — load
     `dhat-heap.json`.

5. Look for entries with high `total_blocks` × `bytes` — those are the
   per-frame allocators. Typical TUI offenders:
   - per-frame `String` allocations in render paths
   - `Vec<Row>` rebuilds in every `Table::new(...)`
   - `Vec<&Workspace>` materializations in `visible_workspaces()`

### Interpreting

- Allocations per frame should be near-constant. A spike when you switch
  tabs is the smoking gun for a hot-path regression.
- Total bytes is less important than allocation **churn**.
- If a function shows up high but only runs once per second, ignore it —
  the budget is per-frame, not per-call.

### Pair with criterion

The criterion benches in `crates/sid-core/benches/` and
`crates/sid-widgets/benches/` give you a per-function wall-clock
measurement; dhat tells you whether allocations dominate that
measurement. Use both: criterion to detect a regression, dhat to
diagnose it.
