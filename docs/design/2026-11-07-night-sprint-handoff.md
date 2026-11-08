# Night sprint handoff — 2026-11-07

A parallel-agent sprint (≈ 20:15–23:12) that cleared most of the backlog, upgraded every
dependency, rolled out design system v2 across the app, and built the previously deferred
MCP server, `.claude` plugin, CI and Mac/Windows support. It ended on a hard stop, so the
last batch is **ungated** on purpose.

## Where things are

- **`main` @ `0fa6dbe` is gate-green**: fmt, clippy `-D warnings`, `cargo test --workspace`
  → 75 suites / 1924 tests / 0 failed (rustc 1.96, Linux).
- **`night/wrap-2026-11-07` = main + everything that finished after the last gate, merged
  without a combined gate.** Start the next session by gating this branch, fixing what
  breaks, and fast-forwarding main to it.
  - Merged into the wrap branch: `night/sec-binding` (H2 secret↔endpoint bindings, M1
    host-key fingerprint prompt, bounded SFTP view/download, config-editor exclusive temp
    files, sudo-password timeout; its own gate: 70 suites / 1960 tests + docker sshd live
    test), `night/perf` (unfocused System probe 5× slower, SSH drain 133 ms unfocused,
    perfcap fixes; `sid` tests only).
  - Still on their own branches at the stop (WIP commits allowed, ungated) — merge into the
    wrap branch next session: `night/ssh-session` (session v2, SFTP edit-in-place UI, Esc on
    modals + keep-server-copy backup in progress), `night/db-polish` (tree select-vs-activate
    fix for the arrow-key bug, table/column icons, sentence-case label test),
    `night/polish-settings-ws` (text rendering setting Auto/Subpixel/Grayscale, theme
    swatches, palette persistence, Workspaces polish), `night/ci-fix` (toolchain 1.99.1 for
    macOS, workspace clippy lints, Windows test fixes; re-merging main at the stop).

## What landed on main tonight (summary)

Renderer gpui-pre 0.3.11 + gpui-component 0.9 and all other deps (held: sysinfo 0.40 —
constructors now fallible; zbus 6 — keyring store still on 5) · design system v2 in
`sid-ui` (type 13/14/15/18/22, radius, soft shadows, motion, Tree/ContextMenu/Checkbox/
Switch/Skeleton/EmptyState v2; law in `.interface-design/system.md`, spec in
`docs/design/2026-11-07-friendly-ui-v2.md`) · per-tab rollouts: SSH home (import from
`~/.ssh/config`, host groups Tree, get-started surface), Database (schema Tree, empty
states, toasts), Network (Connections view, CPU/mem columns), System (sparklines, per-OS
pins), Workspaces v2 (stage/diff/commit/stash/branches/log, fetch/push via the git CLI),
chrome/nav (palette hub, `?` shortcut modal, Settings v6: reduce motion, notifications,
last tab) · security review fixes H1, M2, M3, M4, M5 + MCP/capture hardening + notification
escaping · `sid-mcp` (9 tools incl. `capture_ui`) · `.claude` plugin `sid-dev`, adapter hook,
per-crate CLAUDE.md, `docs/ARCHITECTURE.md`, `justfile` · CI on GitHub Actions
(linux/macos/windows) + cfg-gated platform adapters · headless UI interaction tests via
gpui-kit (`just ui-test`) · bugs #6 #7 #11 #12 #13 + pin-a-directory · GitHub issues #1–#4
closed.

## Next session, in order

1. Gate `night/wrap-2026-11-07`; merge the four pending branches into it one at a time
   (expect semantic conflicts — tonight's lesson: always type-check after a clean textual
   merge; git2 0.21 `Result` getters and duplicate fns bit twice); gate; fast-forward main.
2. Run the second adversarial security review that was planned but not run (focus:
   `crates/sid-git/src/cli.rs` — the git CLI honours repo config and runs hooks unlike
   libgit2: consider `-c core.hooksPath=/dev/null`, `--no-verify`, `protocol.ext.allow=never`,
   `core.sshCommand` override; the remote-edit UI; the import dialog; the committed hook).
3. sysinfo 0.40 (fallible constructors → graceful UI error) + "cached" memory in
   SystemOverview + a "collecting…" state for empty sparklines at launch.
4. QA sweep: every tab × 4 themes × 1920/1280, keyboard-only pass, live docker sshd/postgres.
5. Docs: rewrite `docs/HANDOFF.md` from the tree (still claims the focus-trap bug is open in
   its per-tab table; it is fixed), add `sid-os` / `sid-mcp` to ARCHITECTURE, correct
   `docs/design/SPIKE-FINDINGS.md`'s fractional-scaling note (gpui-pre 0.3.11 renders crisp
   at fractional scale; soft text is likely subpixel fringing → the new text-rendering setting).
6. Perf: the per-cell wrapper div in tables (~11% of a frame), narrower System
   `ProcessRefreshKind` — see `docs/design/2026-11-07-perf-pass.md`.

## Decisions waiting on Murphy

- **D5 accent**: cosmos's red accent makes every primary (each SSH card's Connect, Add
  connection) read as danger. A violet/indigo accent is a one-token change.
- Unfocused System tab now refreshes every 10 s (perf). Keep, or restore 2 s for
  dashboard-on-a-second-screen use?
- SFTP "Keep server copy" — being changed to keep a backup of local edits (was discard).
- Make the CI `gate` job a required status check (repo setting).
- Fetch/push go through the `git` CLI (decided tonight); revisit only if that's unwanted.

## Machine notes from tonight

Builds were throttled with a scratchpad wrapper (≤ 4 concurrent cargo runs, nice 19, idle
IO, 2 jobs, no incremental) and the Claude session + sccache server were reniced to 19.
Restart Claude Code and run `sccache --stop-server` to return to normal priority.
`scripts/sid-cap.sh` now honours `SID_CARGO=<wrapper>` and keeps user fonts in hermetic mode.
