# Troubleshooting

Common problems you might hit running or developing `sid`, with the
fixes that have worked.

## Won't compile

### Missing `cargo` features

A `cargo build` that fails with "no method named X" or "unresolved
import" often means a workspace dependency is missing a feature flag.
The pinned versions in the root `Cargo.toml` already enable everything
needed for the default build. If you've added a new crate to the
workspace and inherited deps via `workspace = true`, double-check
you didn't drop a required feature in your local override.

### MSRV mismatch

`sid` requires Rust 1.85 (edition 2024). Older toolchains will fail with
`error: edition 2024 is unstable` or similar.

```sh
rustup toolchain install stable
rustup default stable
rustc --version
```

If a `rust-toolchain.toml` is added later, `rustup` will pin
automatically.

### Vendored libgit2 build failures

The `git2` crate is configured with `vendored-libgit2`, which builds
libgit2 from source statically. The build needs `cmake` and a C
compiler.

- "could not find cmake" → install `cmake` via your package manager
  (`pacman -S cmake`, `apt install cmake`, `brew install cmake`)
- "cc not found" → install `gcc` or `clang`
- "linker failed" on Linux → install `build-essential` (Debian/Ubuntu)
  or the `base-devel` group (Arch)

Once the build succeeds once, the artifact is cached in `target/` and
subsequent builds reuse it.

## `cargo test --doc` errors after dep changes

A symptom that's easy to misread: a doc test fails or rust-analyzer
flags imports red after you bump a dependency, but `cargo test` (the
non-doc variant) is fine.

Cause: stale incremental artefacts for the doc-test runner.

Fix:

```sh
cargo clean -p <crate>
cargo test --doc -p <crate>
```

If the problem is workspace-wide, `cargo clean` (no `-p`) and run
again. The first rebuild will be slow.

## redb file corrupted

If `sid` panics on launch with a redb error ("invalid magic",
"checksum mismatch", "page corruption"), the DB file is unreadable.
This is rare — redb is ACID and atomic — but possible if the disk lied
about a write completing (some SSDs do under power loss).

Steps:

1. **Back the file up.** Even a corrupted DB might be partially
   recoverable later.
   ```sh
   cp ~/.local/share/sid/sid.redb ~/.local/share/sid/sid.redb.broken-$(date +%s)
   ```
2. **Delete the live DB.**
   ```sh
   rm ~/.local/share/sid/sid.redb
   ```
3. **Restart `sid`.** It will create a fresh DB. Workspaces, themes, and
   sessions reset. SSH `known_hosts` and `~/.ssh/config` (read from
   disk) are unaffected — `sid` never modifies them.
4. **File an issue** with the backup attached if you have one. redb
   has a debug tool that can sometimes salvage the page tree.

If you want to test the fresh-DB path without losing state, use the
`--db` flag:

```sh
sid --db /tmp/sid-fresh.redb
```

## Terminal renders garbled

Symptoms: missing colors, broken borders, glyphs showing as `?` or
mojibake.

- **Truecolor unsupported.** Check `echo $COLORTERM`. You want
  `truecolor` or `24bit`. If empty, your terminal isn't advertising it;
  most modern terminals do. In tmux, set
  `tmux set -g terminal-overrides ',xterm-256color:RGB'` in your
  config and restart tmux.
- **Locale wrong.** Glyphs like `✦` are UTF-8; if `LANG`/`LC_ALL` is set
  to a non-UTF-8 locale they'll render as `?`. `export LC_ALL=en_US.UTF-8`
  (or your locale of choice) and relaunch.
- **Terminal font lacks the glyph.** The cosmos theme uses `✦ · ★` —
  most monospaced fonts include them, but if yours doesn't, picking a
  Nerd Font variant or a font like "JetBrains Mono" or "Iosevka" fixes
  it.

## `sid workspace add` doesn't find a repo

(This applies once Plan 2 lands; the subcommand isn't on `main` yet.)

If `sid workspace add ./some-dir` doesn't see the workspace, the most
common cause is a relative path that doesn't resolve from the directory
`sid` runs in. Try the canonical absolute path:

```sh
sid workspace add "$(realpath ./some-dir)"
```

`sid` canonicalizes internally, so the path you pass and the path stored
will be equivalent — but if the original path doesn't exist or has a
broken symlink, `realpath` will tell you immediately.

## rust-analyzer says modules don't exist but `cargo test` passes

After adding a crate to the workspace, adding a module, or bumping a
dep that changes module layout, rust-analyzer can cache stale
information. The compiler is happy; the editor is wrong.

- VS Code: Command Palette → **Rust Analyzer: Reload Workspace**.
- Neovim with `nvim-lspconfig`: `:LspRestart`.
- Helix: restart the editor or `:lsp-restart`.

If that doesn't help, kill the rust-analyzer process and let the editor
respawn it.

## Plugin discovery (`/mc-dc-audit` not found)

The `sid-testing` plugin's `/mc-dc-audit` skill is meant to surface in
Claude Code's slash-command list but doesn't appear in the default
discovery scan. Known issue. The current workaround is to point the
client at the marketplace.json directly.

Until the discovery path is fixed, run the skill via its plugin path:

```
/plugin sid-testing:mc-dc-audit crates/sid-store
```

A proper fix lives in the open follow-up tracking discovery for
self-hosted plugin marketplaces. Until then, the workaround works.

## Panics → no log file

`sid` installs a `std::panic::set_hook` that writes backtraces to
`~/.local/state/sid/crash-<timestamp>.log` and restores the terminal
before exiting with code 1.

If you panicked but no log file appeared, the most likely cause is
the panic happening before the hook was installed (very early startup,
before `main` returns). Re-run with
`RUST_BACKTRACE=full RUST_LOG=trace sid --db /tmp/repro.redb 2> sid.log`
and you'll get the backtrace on stderr.

## CI passes but local tests fail (or vice versa)

CI runs `cargo test --workspace --all-features` on a clean checkout.
Local runs reuse `target/`. If they disagree, the local artefacts are
stale:

```sh
cargo clean
cargo test --workspace --all-features
```

If that still disagrees with CI, the difference is environmental
(timezone, locale, presence of network). Re-read the failing test for
environmental dependencies — none of `sid`'s tests should require
network, and tests touching the clock should be using an injected
clock. If they aren't, that's a bug worth fixing.

---

If your problem isn't here, check
[INSTALLATION.md → Troubleshooting](INSTALLATION.md#troubleshooting)
for install-time issues, or open an issue per
[CONTRIBUTING.md → Filing an issue](CONTRIBUTING.md#filing-an-issue).
