# Night sprint handoff — 2026-11-07

> **Closed 2026-11-08.** Everything below is integrated: `night/wrap-2026-11-07`,
> `night/ssh-session`, `night/polish-settings-ws` and `night/ci-fix` merged through
> `night/integrate`, sysinfo 0.40 landed, and `main` is `56ca357` (76 suites / 2054 tests /
> 0 failed; fmt and clippy `-D warnings` clean). Read the sections below as history; the
> current state is `docs/HANDOFF.md`.
>
> **Security review #2** (run on the integrated tree) is fixed:
> - M-1, the git CLI path honoured repo config and hooks: env scrubbed, `GIT_ALLOW_PROTOCOL=ssh:https`,
>   hooks/fsmonitor/askPass neutralised via `-c`, `ext::`/http/git remotes refused (`a35aec2`).
> - M-4, discard treated a path as a glob: now literal; `remove_untracked` spares nested repos (`7c5baca`).
> - M-2 and M-3 are the remote-edit and secret-binding findings (mapped here by commit subject, not
>   from the review text): edit-in-place never loses edits, shell-inert local names, SHA-256
>   conflict check (`11134b1`, `f226209`); tmp-rename sets mode at open and swaps only safe
>   targets, and is opt-in (`93e7689`); a binding's layer is derived from the key (`1a468ed`),
>   promote/demote mints a fresh ref (`520058e`), a moved record never keeps the old ref (`97b90c8`).
> - LOWs: deceptive bidi/zero-width characters stripped from notifications and toasts
>   (`a8c5f62`), bounded regular-file config and identity reads (`e83b096`), config-editor saves
>   through a symlink (`52f1905`), ssh_config import parsed off the UI thread (`2af7817`),
>   stash ops keyed by oid and hostile text neutralised (`620a0ee`), ConfirmArm 300 ms guard
>   (`413d39f`), palette recents keyed by origin plus clipboard sanitising (`aa438db`),
>   adapter-violation hook covers MultiEdit (`527cf9b`), CI least-privilege and SHA pins (`3d6653f`).
>
> **Also done since:** violet accent (D5), Settings v8 background refresh (Off by default),
> QA-sweep fixes across every tab (see `2026-11-08-qa-sweep.md`).
>
> **What remains:** QA findings #11 (part) and #16-#19; an async keyring probe (needs a design
> decision); zbus 6; narrower `ProcessRefreshKind`; making the CI `gate` job a required check;
> a full re-sweep including live SFTP edit-in-place.

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
  - Also merged into the wrap branch: `night/db-polish` (tree select-vs-activate fix for
    the arrow-key bug, table/column icons, sentence-case label test, folder headers).
  - Still on their own branches (pushed, ungated) because they CONFLICT with the wrap
    branch — resolve and merge next session:
    - `night/ssh-session` @ b907100 (wip — DOES NOT COMPILE: `esc_wrap` in
      `crates/sid/src/ui/session/edit.rs` ~L548 needs `use<M>` (or return `AnyElement`) and a
      `gpui::InteractiveElement` import for `.key_context`; `18175d4` is the last gate-green
      commit on that branch): session v2, SFTP edit-in-place UI,
      keep-server-copy backup, Esc on modals. Conflict: `crates/sid/src/ui/session.rs`
      vs sec-binding's host-key prompt / bounded view+download (their logic wins, its
      presentation wins).
    - `night/polish-settings-ws` @ 2cf62f4: text rendering setting (Auto/Subpixel/
      Grayscale), Settings v7, theme swatch cards, palette recents + sub-view entries,
      Workspaces log badges. Conflict: `crates/sid-store/src/{global.rs,lib.rs}` — its
      Settings v7 vs sec-binding's `secret_bindings` table (keep both; re-check the v6→v7
      migration test).
    - `night/ci-fix` @ 637f8c0: toolchain 1.99.1, workspace clippy lints, Windows fixes,
      macOS mint_secret_id clock fix. Conflict: `crates/sid-ui/Cargo.toml` (its `[lints]`
      vs ui-tests' gpui-kit dev-dependency — keep both). Its last CI run (42445111268) was
      unfinished at the stop.

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
