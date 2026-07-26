# sid — bug hunt + observation-gate pass, 2026-07-26

**Built and tested against `e893c3b`** ("feat(sid-ui): crate skeleton + tokens + theme bridge").
Read-only pass: no repo file was modified (`git status` clean throughout).

> **Commit drift note:** partway through this session the worktree was renamed
> (`agent-a25556a7c5ca3a3b4` → `agent-g3tablefill`) and its HEAD advanced to
> `5120298` *"feat(sid-ui): Table fill-width column model (Fixed | Min | Grow)"*.
> All evidence below is from the `e893c3b` binary. That newer commit's title
> suggests it may already address the column-width findings (#1, #8) — re-verify
> those two against current `main` before scheduling work.

Evidence PNGs live in
`/tmp/claude-1000/-home-murphy-vcs-sid/f354a0cd-6c84-4e2f-a63f-dfdaec88f10c/scratchpad/shots/`
(abbreviated `SHOTS/` below).

---

## CONFIRMED BUGS

### 1. HIGH — SFTP file browser: the file-name column collapses to ~3 characters

Distinct files become indistinguishable. In the remote home dir, `.bashrc`,
`.bash_logout` and `.profile` render as `.ba`, `.ba`, `.prc` — two different
files show identical text, with no ellipsis.

- **Repro:** connect to any host → look at the FILES sidebar at its default width.
  Directory rows (`.ssh`, `sftp-fixture`) show full names; *file* rows do not.
- **Evidence:** `SHOTS/02-crop-sidebar.png`, `SHOTS/12-crop.png`
- **Root cause:** `entry_row`, `crates/sid/src/ui/session.rs:1281-1313`. The row is
  `px_3()` + `gap_2()` with glyph `w(px(14.))`, size `w(px(60.))`, mtime
  `w(px(84.))`, plus three action buttons (`view`, `↓`, `⧉`) — all fixed width.
  The name is the only `flex_1` child, so it absorbs the entire shortfall inside
  `SIDEBAR_WIDTH = px(320.)` (`session.rs:50`). File rows carry two more buttons
  than directory rows, which is exactly why only they collapse.
- **Lowest-layer test:** a row-layout test asserting the name column retains a
  minimum sensible width at `SIDEBAR_WIDTH` given the max action-button set.

### 2. MED — SFTP breadcrumb overflows its row and paints over the path field

At any path depth ≥ 2 the breadcrumb wraps and spills onto the row below,
overlapping the go-to-path input and the `Go` button.

- **Repro:** connect (lands at `/home/sid_test`). Toolbar renders `/` on row 1,
  then `ome` and `d_test` painted across row 2. Navigate to `/` (one segment) and
  the toolbar renders cleanly as three separate rows.
- **Evidence:** `SHOTS/02-crop-sidebar.png` (depth 2, broken) vs
  `SHOTS/11-crop.png` (root, clean)
- **Root cause:** `breadcrumb()`, `session.rs:1152-1160`, uses `.flex_wrap()`; its
  parent row (`session.rs:1081`, `div().flex_1()`) keeps a single-line height, so
  wrapped lines overflow instead of growing the toolbar. Confirmed empirically:
  row 3 ("N entries") sits at the *same* y in both the 1-segment and 3-segment
  cases — the toolbar height never grows.
- **Lowest-layer test:** assert toolbar height grows with breadcrumb line count.

### 3. MED — the go-to-path field renders as a ~20 px stub; clicking the visible field does nothing

The field is functional but effectively invisible and nearly unclickable.

- **Repro:**
  1. Connect, navigate to `/`.
  2. Click at (150, 213) — inside the *apparent* field — type `/etc`, press Return.
     → Nothing. No text, no navigation, and the terminal is unaffected (the
     keystrokes are silently dropped, not misrouted).
  3. Click at (15, 213) — on the ~20 px stub — type `/etc`. → Text enters, but only
     a sliver of `c` is visible. Click **Go** → navigates correctly to `/etc`
     (83 entries).
- **Evidence:** `SHOTS/13-crop.png` (x=150 → nothing), `SHOTS/15-crop.png`
  (x=15, text invisible), `SHOTS/16-crop.png` (Go → `/etc` works)
- **Root cause:** `TextInput::render`, `crates/sid/src/ui/text_input.rs:781`,
  returns `div().flex()` with **no width on the outer element** (only the inner
  child is `w_full()`), so the widget is content-sized. The wrapper at
  `session.rs:1099-1105` (`flex_1().min_w(px(0.)).overflow_hidden()`) does not
  stretch it. The placeholder `"/path/to/go"` (`session.rs:322`) never renders.
- **Useful A/B for the fixer:** the *same* widget renders full-width in the
  Add-host modal (`SHOTS/20-save-no-scope.png`) and the System filter
  (`SHOTS/24-crop.png`) — so the defect is in this call site's parent chain, not
  the widget alone.

### 4. MED — Enter does not submit the go-to-path field (only the `Go` button works)

- **Repro:** click the stub at (15, 213), type `/etc`, press Return → still at `/`
  (20 entries). Identical input but clicking `Go` → `/etc` (83 entries).
- **Evidence:** `SHOTS/17-crop.png` (Return, no nav) vs `SHOTS/16-crop.png` (Go, nav)
- **Root cause:** `goto_submit` (`session.rs:630`) is wired only to the `Go`
  button's `on_click` (`session.rs:1053`); there is no `on_action` / key binding
  for Enter in the file-panel key context.
- **Corroboration (independent):** the Settings/config-editor agent hit the same
  "Enter does not submit" on the System tab's *"pin a file…"* input, whose own doc
  comment at `crates/sid/src/ui/systems_tab.rs:112-114` claims *"Submits on Enter"*.
  **This is not a harness artifact** — `--key Return` demonstrably works for
  quick-connect (`SHOTS/02-quickconnect.png`) and the Add-host modal. Two
  independent submit-on-Enter regressions; worth auditing every `TextInput` site.

### 5. MED — no cancel for an in-flight connect, and no app-level dial timeout

- **Repro:** quick-connect `root@192.0.2.1:22` (TEST-NET-1, blackholed). The tab
  shows "Connecting…" with no cancel/abort affordance and no elapsed indicator for
  ~127 s, then fails with `connect failed: Connection timed out (os error 110)` —
  the kernel's TCP SYN-retry budget, not the app's.
- **Evidence:** `SHOTS/03-unreachable.png` (still "Connecting…" at 20 s),
  `SHOTS/05-unreachable-150s.png` (failed at ~127 s)
- **Root cause:** `RusshClient::connect`, `crates/sid-ssh/src/client.rs:209`, calls
  `russh::client::connect(..)` with no timeout wrapper. `Config.inactivity_timeout:
  Some(Duration::from_secs(300))` (`client.rs:196`) is a *post-handshake* setting,
  not a dial timeout.
- **Lowest-layer test:** `sid-ssh` test asserting a connect to a blackholed address
  returns `ConnectFailed` within a bounded deadline.

### 6. LOW — a failed session tab is a dead end

No retry / edit-credentials / try-password affordance; only "close tab". The error
text itself is good: *"Connection failed: authentication failed: all agent
identities rejected"*. Evidence: `SHOTS/04-authfail.png`.

### 7. LOW — the terminal grid has no inset padding

With the file panel docked right, terminal text is flush against the window's left
edge (x=0). Docked left it merely looks fine because the sidebar occupies that
space; the pane itself has ~0 px inset either way.
Evidence: `SHOTS/09-dock-right.png`, `SHOTS/09-leftedge.png` vs `SHOTS/02-leftedge.png`.
Root cause: `render_split`, `session.rs:879-883` — terminal is
`div().flex_1().size_full()` with no padding.

### 8. MED — table sort fires only on a ~6×8 px icon; clicking the header label selects the column instead

Affects **every** table in the app (shared component).

- **Repro (System tab):** click the PID header *label* (x≈300, y≈292) → the column
  highlights maroon, row order is unchanged (still CPU%-desc). Click precisely on
  the PID sort *chevron* (x≈240, y≈292) → sorts correctly and **numerically**
  descending (1315249, 1315248, 1315247 …).
- **Evidence:** `SHOTS/22-crop.png`, `SHOTS/23-crop.png` (label clicks, no reorder)
  vs `SHOTS/25-crop.png` (chevron, correct numeric sort)
- **Independently reproduced** on the Workspaces fleet table by a parallel agent,
  which located the exact hit box (~6×8 px) and the root cause in the vendored
  component: `gpui-component-0.5.1/src/table/state.rs:327-341`
  (`on_col_head_click` sets column *selection* only) vs `:744-785`
  (`render_sort_icon` is the only element wired to `perform_sort`); `render_th`
  `:791-820` attaches the header cell's `on_click` to the selection path.
- **The comparators are correct** (numeric, chronological, 3-state toggle). Only
  the click target is wrong. Fix is a bigger hit area, not a sort rewrite.

### 9. MED — the System tab's two-click "kill" confirm is racy; kill did not fire in 2/2 attempts

- **Repro:**
  1. `cp /bin/yes ./zqxburn && setsid ./zqxburn &` (≈100 % CPU, so it is
     deterministically row 1 under the default CPU%-desc sort).
  2. System tab → filter `zqxburn` → click the row's `kill` button twice (~0.4 s
     apart). → Process survives (`pgrep -x zqxburn` still alive).
  3. Single click, capture ~1.4 s later → the button still reads `kill` in muted,
     never `kill?` in danger.
- **Evidence:** `SHOTS/29-right.png`, `SHOTS/30-right.png`
- **Suspected root cause:** `set_processes`, `systems_tab.rs:248-252`,
  unconditionally does `self.armed_kill = None` on **every** 2 s refresh tick, so
  the arm is cleared out from under the user between the two clicks. Arming itself
  is correctly PID-bound (`armed_kill: Option<Pid>` `:193`; render `:401-406`;
  click handler `:417-423`).
- **Lowest-layer test:** arm a pid on the delegate, call `set_processes` with the
  same row set, assert the arm survives (or that a deliberate grace window exists).
- **Safety note — this is "kill doesn't work", not "kills the wrong thing".** The
  design is fail-safe: one click can never kill, and the second click only kills a
  *matching PID*, so a reordering table cannot redirect a confirm onto a bystander.

### 10. MED — Network → Ports: the "Process" column is `—` on every row

Including the rows that *do* resolve a PID.

- **Repro:** Network tab → Ports. 40 listening ports; Process is `—` for all 40,
  and still `—` for the four rows with a known PID (15090, 755075, 994652, 995811).
- **Evidence:** `SHOTS/32-network.png`
- **Root cause** (confirmed by a parallel agent before it died):
  `crates/sid/src/ui/network_tab.rs:542-545` renders `—` exactly when
  `port.command.is_empty()` — the command string is never populated even when the
  pid is known.
- Port sort is correct and numeric by default (22, 53, 68, 1716, 2222, 5353 … 9500).

### 11. LOW/MED — relationships diagram: connectors collapse into one line; every box clips its last column

- **Repro:** Database tab → select "demo sqlite" → click `diagram`.
  Header reads "5 tables · 5 relationships", but only the `products → order_items`
  connector is legibly drawn; the other four collapse into near-invisible vertical
  segments hugging x≈1220 because the default layout stacks all related tables in
  one column. Separately, every box slices its final column mid-row:
  `orders`/freight, `employees`/reports_to, `order_items`/discount,
  `customers`/city, `products`/in_stock.
- **Evidence:** `SHOTS/35-diagram-window.png`

### 12. LOW — Workspaces rename field does not select-all on focus

Renaming "fleet" and typing "my-fleet" without clearing yields `fleetmy-fleet`.
(Parallel agent; `crates/sid/src/ui/workspaces_tab.rs:1109-1125`.)

### 13. LOW — System process table's "User" column shows a numeric UID (`1000`), not a username

Evidence: `SHOTS/28-right.png`. `sysinfo` is already configured with the `user`
feature, so the name should be resolvable.

---

## GATES CLOSED

| Gate | Verdict |
|:--|:--|
| **B5 live-sshd smoke** (open since Plan 3B) | **PASS** — `docker_sshd_key_auth_exec_and_sftp_round_trip ... ok` against the dockerized sshd (key auth + exec + SFTP list/put/get/create/remove round-trip). Fixture torn down. |
| Plan 3C connect flow + terminal render | **PASS** — quick-connect Enter connects; MOTD + prompt render with correct colors (`SHOTS/02-quickconnect.png`). |
| Multiple concurrent sessions | **PASS** — two live sessions, both green (`SHOTS/07-two-sessions.png`). |
| `Ctrl+PgUp` session cycling | **PASS** (`SHOTS/08-ctrl-pageup.png`). |
| Disconnect / close-tab → no stale UI | **PASS** — returns to a clean home surface (`SHOTS/10-closed-tab.png`). |
| Terminal resize / reflow | **PASS** at 1024×700 — cols reduce, MOTD rewraps (`SHOTS/14-small-window.png`). |
| Dock left/right + sidebar collapse | **PASS** apart from bug #7 (`SHOTS/09`, `SHOTS/18`). |
| Connect error handling — auth refused, bad DNS | **PASS** — specific, actionable messages; "failed" status + red dot. |
| Add-connection flow | **PASS** incl. missing-scope validation ("choose where to save: workspace or global"). Note neither scope is preselected despite `default_scope`. |
| **Theme live-switch + persistence across restart** | **PASS** — two launches against fixed XDG dirs; cosmos-light survived restart. |
| Settings toggles persistence | **PASS** — `default_scope`, `file_browser_side`, keyring all persisted; keyring-off correctly surfaces "secrets: in-memory (no persistence)". |
| **Workspaces checkout refusal on a dirty tree** (data-loss check) | **PASS** — refused inline, branch unchanged on disk, modification preserved. Clean-tree checkout succeeds; untracked-only files correctly do not block. |
| Workspaces add / rename / unregister | **PASS** — idempotent re-add, graceful error on nonexistent path, trailing-slash + `~` canonicalization, two-click armed unregister, rename updates row *and* scope chip. |
| Workspaces fleet table + repo sub-views | **PASS** — every value cross-checked against real `git` CLI over 4 constructed repos. |
| System filter | **PASS** — filters correctly, count consistent (`SHOTS/24-crop.png`). |
| System CPU/mem/swap cards | **PASS** — match `free -m` and `nproc` exactly (31.1 GB / 10.8 GB, swap 34.2 / 3.0 GB). |
| Relationships diagram opens on an FK-bearing DB | **PASS** (`SHOTS/35-diagram-window.png`). |
| Diagram **self-referential FK stubs** (polish item) | **ALREADY DONE** — `employees.reports_to → employees.id` (`crates/sid-db/src/demo.rs:69`) renders as a `↺ 1` badge in the box header. Not broken; close this queue item. |

---

## COULD NOT TEST

- **Diagram "refresh-in-window"** — no refresh affordance exists in the pop-out
  window's header (it shows only "5 tables · 5 relationships"); the `↻` control
  lives in the *main* window's SCHEMA panel. Reported as still-missing rather than
  broken.
- **Diagram "release-outside-window drag"** — the capture harness's pointer driver
  exposes discrete clicks, not press-move-release, so a drag could not be
  expressed. Needs a manual pass or a drag primitive added to `cap-input/vptr.py`.
- **Config-file editor** — ≤1 MiB gate, non-UTF-8 gate, read-only banner, atomic
  perms-preserving save, dirty marker, and directory/nonexistent pinning. The
  assigned agent died to the worktree rename before running them. Unit tests
  already cover the gates and the perms-preserving save
  (`gate_loaded_bytes_rejects_over_cap`, `save_preserving_permissions_keeps_mode_bits`,
  etc.), so a priori confidence is reasonable but unconfirmed in the UI.
  One code-only suspicion: `submit_pin` (`systems_tab.rs:695-722`) gates on
  `Path::exists()` with no `is_file()` check, so a directory would likely pin.
- **`live_sshd_agent_exec_shell_sftp`** — needs a real ssh-agent plus a trusted
  sshd on `localhost:22`; the harness deliberately provides neither. Failed only
  with `AuthFailed("SSH_AUTH_SOCK not set")`, i.e. a missing prereq, not a defect.
- **Network sub-tabs** (Services / Interfaces / Docker / Kubernetes), filter
  behavior, and cross-sub-tab stale-data guards — the assigned agent died before
  covering them.
- **Fleet ahead/behind and Branch column sorting** — the constructed test repos had
  no remotes (ahead/behind always `—`) and identical branch names, so those two
  comparators were never exercised with differentiated data.

---

## Cleanup performed

All scratch processes killed (`zqxburn`, `zqxscratch`, two `sleep 3000`), the
dedicated ssh-agent terminated and its socket + copied test key removed, orphaned
headless-sway compositors from the stalled subagents killed, subagent fixture dirs
removed (`/tmp/sid-bughunt-ws`, `/tmp/sid-bh-cfg`, `/tmp/sid-bh-sys`,
`/tmp/sid-bughunt-net`), and the dockerized sshd fixture torn down
(`docker compose ... down -v` — container and network removed). Residual check
clean. The real user XDG dirs were never touched: every run used sid-cap's
hermetic temp XDG (`--real` was never passed).
