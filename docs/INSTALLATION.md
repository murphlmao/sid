# Installation

`sid` is a single Rust binary. There's no daemon, no system service,
no package to install — you build it once and run it.

## TLDR — spin it up locally in 60 seconds

If you have Rust 1.85+ and cmake already:

```sh
git clone https://github.com/murphlmao/sid && cd sid
cargo run -p sid
```

That's it. First build takes a few minutes (vendored libgit2 + Tokio/Ratatui).
Subsequent runs are sub-second.

**Inside the TUI:**

| Key | What it does |
|:---|:---|
| `Ctrl+Q` | **Quit** (also shown in the help bar at the bottom of every frame) |
| `Ctrl+F` | Open the command palette (fuzzy-search every action) |
| `Ctrl+←` / `Ctrl+→` | Previous / next tab |
| `Ctrl+1..6` | Jump directly to tab 1–6 |
| `Ctrl+,` | Jump to the Settings tab |
| `j / k` | Down / up in any list (Workspaces tree, etc.) |
| `Enter` | Drill in / expand an umbrella workspace |

**What you'll see on first launch:**

- Top bar: six tab labels (`● Workspaces · SSH · Database · Network · System · Settings`)
- Body: the active tab's content. Workspaces shows "no workspaces registered yet" with hints; the other five tabs honestly say "Plan 3/4/5/6/7 — not yet implemented."
- Bottom bar: a one-line keybind hint, always visible. **This is your "how do I exit?" cheat sheet.**

**Registering a workspace** (e.g., for the Workspaces tab to show something useful):

```sh
# In another terminal — these are CLI subcommands that exit immediately, no TUI:
./target/release/sid workspace add ~/code/some-repo
./target/release/sid workspace list
./target/release/sid workspace remove ~/code/some-repo

# Or: put your repos under ~/vcs/ and they auto-discover on every launch.
```

Now relaunch `sid` and the Workspaces tab will show your registered repos as a tree.

---

## Prerequisites

- **Rust 1.85 or newer** (edition 2024). Install via [rustup](https://rustup.rs):
  ```sh
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```
- **Linux or macOS.** Windows builds; expect rough edges around PTY
  behaviour until Plan 3 lands. There is no Windows support promise in
  v1.
- **A terminal that supports truecolor.** Modern Linux terminals
  (kitty, foot, alacritty, wezterm, xterm-256color), macOS Terminal.app
  or iTerm2, and Windows Terminal all qualify. Check with
  `echo $COLORTERM` — you want `truecolor` or `24bit`.
- **cmake** (for the vendored libgit2 build). Install via your package
  manager: `pacman -S cmake`, `apt install cmake`, `brew install cmake`.
- **No system libgit2 install required.** The `git2` crate is configured
  with `vendored-libgit2`, so libgit2 builds and links statically from
  source. This costs a one-time compile but means the produced binary
  doesn't need any libgit2 on the host.

Nightly Rust is **not** required for normal use. Some `rustfmt` options
in `rustfmt.toml` are nightly-only and silently ignored on stable; that
is fine. Miri (`cargo +nightly miri test`) is only needed if you ever add
`unsafe` blocks — Plan 1 contains none.

## Build from source

```sh
git clone https://github.com/murphlmao/sid
cd sid
cargo build --release
```

The first build will take several minutes — most of it is libgit2 and
the Tokio/Ratatui ecosystem. Subsequent builds are fast.

The release binary lands at `./target/release/sid`. With `strip = true`
and `lto = "thin"` in the release profile, it's a single self-contained
executable (~10-15 MB depending on platform).

For development iteration use `cargo run -p sid` — slower at runtime,
but the warm rebuild is sub-second after the first build.

## First run

```sh
./target/release/sid
```

On first launch, `sid` creates its data directory under the XDG default
and opens the redb file:

| Path | Platform | Purpose |
|:---|:---|:---|
| `$XDG_DATA_HOME/sid/sid.redb` | Linux (default `~/.local/share/sid/sid.redb`) | the database |
| `$XDG_STATE_HOME/sid/` | Linux (default `~/.local/state/sid/`) | crash logs, future detach socket |
| `~/Library/Application Support/sid/sid.redb` | macOS | the database |

When you quit cleanly (`Ctrl+Q`), the active tab + session timestamps are
written back. The next launch picks up where you left off, after a
"Resume session?" prompt if the prior session ended within the last
60 minutes.

You should land on the **Workspaces** tab. The six tabs render as
labelled blocks in the cosmos theme. Navigate with `Ctrl+←/→` or
`Ctrl+1..6`; open the command palette with `Ctrl+F`; quit with `Ctrl+Q`.

## CLI flags

```
sid [OPTIONS] [SUBCOMMAND]
```

| Flag | Description |
|:---|:---|
| `--db <PATH>` | Override the default redb file path. Useful for testing on a throwaway DB. |
| `--start-tab <ID>` | Start in the given tab if found. IDs: `workspaces`, `ssh`, `database`, `network`, `system`, `settings`. |

The following subcommands manage the workspace registry without launching the TUI — they mutate the redb store and exit:

| Subcommand | Description |
|:---|:---|
| `sid workspace add <PATH>` | Register a workspace at the given path. Reads `<path>/.sid/_metadata.sid` if present; otherwise uses the directory name. |
| `sid workspace remove <PATH>` | Unregister a workspace. Idempotent — removing a non-registered path is a no-op. |
| `sid workspace list` | List registered workspaces with kind + path. |
| `sid --skip-discovery` | Skip the startup auto-scan of `~/vcs/`. Useful in tests, CI, or for fast launches. |

These subcommands operate against the same redb file. They do not start
the TUI — they print to stdout and exit. This lets you script workspace
management without spinning up an interactive session.

## System dependencies

`sid` is designed to install via `cargo` with no further system setup,
but a few transitive dependencies pull in system tools at build time:

- **libgit2** — vendored via the `vendored-libgit2` feature. Built from
  source, statically linked. Requires `cmake` and a C compiler at
  build time only; the resulting binary does not need libgit2 on the
  host.
- **libpq** — only needed when the Database tab (Plan 4) lands and you
  want Postgres support. SQLite uses bundled `rusqlite` so no system
  install is needed for that.

If you see a build failure complaining about a missing C library, that's
almost always either cmake or pkg-config missing. Install the relevant
package and rerun.

## Troubleshooting

### "not a TTY" / "stdin is not a terminal"

Some environments (CI runners, certain IDE terminals, Docker
containers without `-t`) don't expose a real TTY. `sid` is a TUI
application; without a TTY it can't enter raw mode or read keys.

- The smoke tests in `crates/sid/tests/` detect this and skip themselves
  cleanly.
- The binary itself prints a helpful error and exits 1. The terminal is
  restored before exit.
- If you're in VS Code's integrated terminal and it claims no TTY,
  switch to "External Terminal" or run from an OS-native terminal.

### "permission denied" on `~/.local/share/sid/`

If the XDG data home is unwritable (read-only filesystem, root-owned
parent), open the DB elsewhere with `--db`:

```sh
sid --db /tmp/sid.redb
```

`sid` creates the directory and the file on first run; if the parent
doesn't exist, it creates that too.

### rust-analyzer staleness after dependency changes

Adding a new crate to the workspace or a new external dep can leave
rust-analyzer caching stale module info. The symptom is "unresolved
module" or "unresolved import" errors in your editor for code that
`cargo test -p <crate>` accepts.

- VS Code: Command Palette → **Rust Analyzer: Reload Workspace** (or
  reload the window).
- Neovim with `nvim-lspconfig`: `:LspRestart`.
- If that doesn't fix it, `cargo clean -p <crate>` and rerun
  `cargo check` once.

### Crashed mid-session, can't relaunch

Run from a different terminal and the previous run's terminal state
will be cleaned up by the kernel on process exit. If the redb file got
into an inconsistent state, see
[TROUBLESHOOTING.md → redb file corrupted](TROUBLESHOOTING.md#redb-file-corrupted).

---

For more, see:

- [DEVELOPMENT.md](DEVELOPMENT.md) — how to work on `sid`
- [TROUBLESHOOTING.md](TROUBLESHOOTING.md) — runtime problems and fixes
- [ARCHITECTURE.md](ARCHITECTURE.md) — what you're actually running
