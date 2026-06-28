# sid GPUI Bootstrap & De-risk — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the fresh `sid` repo and prove GPUI renders the three primitives sid needs (scrollable list, text input, monospace grid) on Murphy's Wayland — before any feature porting begins.

**Architecture:** Fresh-repo pivot. The TUI repo is archived (GitHub `murphlmao/sid-poc`); the new `murphlmao/sid` is a clean slate. This plan does *only* the bootstrap (repo logistics + frame + buildable workspace) and the *de-risk spike*. It deliberately stops at a go/no-go gate on GPUI-on-Wayland. No store, no adapters, no feature work — those are later plans, written only after the spike's outcome is known.

**Tech Stack:** Rust (edition 2024), GPUI (Zed's GPU-accelerated UI framework, pinned via git), Wayland.

## Global Constraints

- Rust edition `2024`, follow the workspace `rust-version` once set (start at the current stable).
- **Adapter pattern is retained from day one:** OS-integration points (keyring, PTY, editor-launch, clipboard, notifications) go behind traits; only the Linux/Wayland impl exists now. GPUI is the rendering surface and may be named directly only in frontend crates.
- **Layered scope is the core data invariant** (global base + per-workspace `.sid/` overlay; workspace shadows global; secrets never committed). Not built in this plan, but every later design must honor it.
- **Deferred per Murphy — do NOT rebuild in this plan:** the MCP server, the `.claude` plugin (skills/agents/hooks), and heavy CI. Carry only a lean CLAUDE.md.
- Testing: pragmatic mode — targeted tests per feature, one gate review at the end of a slice (not per-commit rigor). A rendering spike is gated by manual observation, not unit tests; that is correct, not a shortcut.

## Roadmap (this is plan 1 of N — later plans written after the spike)

1. **Bootstrap & De-risk** ← *this plan*. Output: fresh repo that builds + a documented GPUI-on-Wayland go/no-go.
2. **Layered store foundation** (headless, fully testable): global redb + per-workspace `.sid/` text + composition + secret refs. The genuinely-new work; medium-agnostic.
3. **SSH/SFTP vertical slice in GPUI** (the spearhead): salvage Tier-1 `sid-ssh` adapter, build host list + embedded terminal + SFTP on the layered store, to daily-use quality.
4+. Database slice, Network slice, then — only once a slice is load-bearing in daily use — cross-tab integration. MCP/plugin/CI rebuild slots in when the surface stabilizes.

---

## Task 1: Archive the local repo and clone the fresh one

**Files:** none created; filesystem + git remote surgery only.

**Interfaces:**
- Produces: `~/vcs/sid-poc` (the archived TUI, origin → `sid-poc.git`) and a fresh empty-ish `~/vcs/sid` (origin → `sid.git`).

- [ ] **Step 1: Verify the remote topology before touching anything**

Run:
```bash
cd ~/vcs/sid
git remote get-url origin                                   # expect: .../murphlmao/sid.git
git ls-remote --heads origin | head                          # the NEW repo: expect only the init commit on main
git ls-remote --heads https://github.com/murphlmao/sid-poc.git | head   # the ARCHIVE: expect the old feat/* branches
```
Expected: `origin` = `sid.git` (new), which has a *different* main than local (`d89f76c…` vs local `bf67f40`); `sid-poc.git` carries the old history. If the archive does **not** show the old branches, STOP — the archive is incomplete and must be fixed before proceeding.

- [ ] **Step 2: Rename the local repo to the archive name**

Run:
```bash
cd ~/vcs
mv sid sid-poc
```

- [ ] **Step 3: Repoint the archived repo's origin (prevents pushing TUI history into the new repo)**

Run:
```bash
cd ~/vcs/sid-poc
git remote set-url origin https://github.com/murphlmao/sid-poc.git
git remote get-url origin     # confirm: .../sid-poc.git
git status                    # confirm: clean, on main, HEAD bf67f40
```
Expected: origin now `sid-poc.git`. No push needed — the archive remote already has this history.

- [ ] **Step 4: Clone the fresh repo into ~/vcs/sid**

Run:
```bash
cd ~/vcs
git clone https://github.com/murphlmao/sid.git sid
cd ~/vcs/sid
git remote get-url origin     # confirm: .../sid.git
git log --oneline -1          # confirm: the d89f76c init commit (NOT bf67f40)
```

- [ ] **Step 5: Gate (no commit)**

Verify all true: two dirs (`~/vcs/sid-poc`, `~/vcs/sid`); `sid-poc` origin = `sid-poc.git` with TUI history; `sid` origin = `sid.git` with only the init commit. The TUI POC is reachable for cribbing; the new repo is uncontaminated.

---

## Task 2: Establish the frame and the North-Star spec in the new repo

**Files:**
- Create: `~/vcs/sid/.gitignore` (Rust)
- Create: `~/vcs/sid/README.md` (adapted)
- Create: `~/vcs/sid/CLAUDE.md` (lean)
- Create: `~/vcs/sid/docs/mockups/sid-mockup.html` (the approved layout wireframe)
- Create: `~/vcs/sid/docs/design/2026-06-27-gpui-rebuild-design.md` (the design spec)

**Interfaces:**
- Produces: the repo's design source-of-truth that all later plans cite.

- [ ] **Step 1: Add a Rust `.gitignore`**

Create `~/vcs/sid/.gitignore`:
```gitignore
/target
**/*.rs.bk
*.pdb
.DS_Store
```

- [ ] **Step 2: Adapt the README**

Create `~/vcs/sid/README.md` — keep the identity (named after the dog, ops-cockpit, minimal, galaxy aesthetic) but state the new reality: native **GPUI desktop app**, git-centric **layered workspace scope**, **status: rebuilding from the TUI proof-of-concept (archived at `murphlmao/sid-poc`).** One-paragraph "what it is", the core-tabs list (SSH/SFTP, Database, Network; Workspaces/System secondary), and the scope-model one-liner.

- [ ] **Step 3: Write a lean CLAUDE.md**

Create `~/vcs/sid/CLAUDE.md` with only: (a) the adapter-pattern rule (GPUI named only in frontend crates; OS-integration behind traits), (b) the layered-scope invariants (global base + `.sid/` overlay, workspace shadows global, secrets in keyring never committed, single-process scope-switch), (c) vertical-slice discipline (finish one tab to daily-use before the next), (d) pragmatic testing mode, (e) an explicit note that the MCP server + `.claude` plugin + heavy CI are **deferred, to be rebuilt later**. No mention of the retired TUI automation.

- [ ] **Step 4: Carry the approved layout mockup**

Copy the wireframe built during design into `~/vcs/sid/docs/mockups/sid-mockup.html` (source: the session scratchpad `sid-mockup.html`). It is the reference for layout: top tab strip (function axis) + titlebar scope switcher (context axis), full-width content.

- [ ] **Step 5: Write the design spec (North Star)**

Create `~/vcs/sid/docs/design/2026-06-27-gpui-rebuild-design.md` with these sections, each filled from the brainstorming decisions (source: memory `sid-gpui-pivot`):
  1. **Reframe** — sid is an integrated developer ops-cockpit; vertical-slice not horizontal.
  2. **Medium** — native GPUI desktop GUI; rationale (tools-it-replaces-are-GUIs, visual taste, lightweight≠TUI, almost-always-local workflow); honest GPUI/Wayland caveat.
  3. **Scope model** — layered global+workspace; `.sid/` text committed; redb global; secrets via keyring refs; single-process scope-switch (supersedes detach); workspace shadows global; `default_scope` ask/ws/global.
  4. **Layout** — top tabs + titlebar scope switcher; full-width content; per-tab pickers go top (DB connection dropdown), lists collapse.
  5. **Code disposition** — Tier 1 adapters salvage, Tier 2 store/domain re-found, Tier 3 TUI retired.
  6. **Cross-platform** — Wayland now; GNOME/Win/Mac via adapter seams, empty slots now.
  7. **Out of scope now** — MCP/plugin/CI rebuild; other tabs until SSH is load-bearing.

- [ ] **Step 6: Commit**

```bash
cd ~/vcs/sid
git add .gitignore README.md CLAUDE.md docs/
git commit -m "docs(sid): frame, lean CLAUDE.md, layout mockup, GPUI rebuild design spec"
```

---

## Task 3: Buildable Cargo workspace with a `sid` binary (toolchain check, no GPUI yet)

**Files:**
- Create: `~/vcs/sid/Cargo.toml` (workspace)
- Create: `~/vcs/sid/crates/sid/Cargo.toml`
- Create: `~/vcs/sid/crates/sid/src/main.rs`

**Interfaces:**
- Produces: a workspace that `cargo build` and `cargo run -p sid` succeed on. Isolates "is my Rust toolchain fine" from "does GPUI work" — so a Task 4 failure is unambiguously a GPUI/Wayland issue.

- [ ] **Step 1: Workspace manifest**

Create `~/vcs/sid/Cargo.toml`:
```toml
[workspace]
resolver = "2"
members = ["crates/sid"]

[workspace.package]
edition = "2024"
license = "GPL-3.0-only"
authors = ["Murphy Malcolm"]
repository = "https://github.com/murphlmao/sid"
```

- [ ] **Step 2: Binary crate manifest**

Create `~/vcs/sid/crates/sid/Cargo.toml`:
```toml
[package]
name = "sid"
version = "0.0.1"
edition.workspace = true
license.workspace = true

[dependencies]
```

- [ ] **Step 3: Minimal main**

Create `~/vcs/sid/crates/sid/src/main.rs`:
```rust
fn main() {
    println!("sid {} — GPUI rebuild bootstrap", env!("CARGO_PKG_VERSION"));
}
```

- [ ] **Step 4: Build and run**

Run:
```bash
cd ~/vcs/sid
cargo run -p sid
```
Expected: prints `sid 0.0.1 — GPUI rebuild bootstrap`. Toolchain confirmed.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/
git commit -m "chore(sid): cargo workspace skeleton + placeholder binary"
```

---

## Task 4: Open a GPUI window on Wayland (the core spike — riskiest step)

**Files:**
- Modify: `~/vcs/sid/crates/sid/Cargo.toml` (add gpui)
- Modify: `~/vcs/sid/crates/sid/src/main.rs`

**Interfaces:**
- Produces: a process that opens a native window on Wayland showing the text "sid". This is the medium's first proof of life.

> **API note (not a placeholder — a deliberate instruction):** GPUI's public API is unstable and thinly documented; it is developed in-tree at Zed. Do NOT trust hardcoded signatures from memory. **Pin the dependency, then copy the *current* `hello_world` example from that exact pinned source** (`crates/gpui/examples/hello_world.rs` in `zed-industries/zed`). Adapt names to whatever that revision exposes.

- [ ] **Step 1: Add and pin the GPUI dependency**

In `~/vcs/sid/crates/sid/Cargo.toml`, add gpui pinned to an explicit revision (prefer a git rev so the API is reproducible; record *why* this rev, per the keyring-v4 "pin load-bearing choices" lesson):
```toml
[dependencies]
# Pinned to a known-good Zed revision — GPUI has no stable release; bump deliberately.
gpui = { git = "https://github.com/zed-industries/zed", rev = "<PICK_A_RECENT_COMMIT>" }
```
Then resolve: `cargo fetch` and confirm gpui + its (large) dependency tree build: `cargo build -p sid` (first build is slow — GPUI pulls a lot).

- [ ] **Step 2: Adapt the current hello_world example**

Replace `main.rs` with the *current* pinned example's window-open pattern (shape shown; reconcile to the pinned API):
```rust
use gpui::*;

struct Sid;

impl Render for Sid {
    fn render(&mut self, _cx: &mut ViewContext<Self>) -> impl IntoElement {
        div().flex().size_full().justify_center().items_center().child("sid")
    }
}

fn main() {
    App::new().run(|cx: &mut AppContext| {
        cx.open_window(WindowOptions::default(), |cx| cx.new_view(|_| Sid))
            .unwrap();
    });
}
```

- [ ] **Step 3: Run on Wayland — the gate**

Run (force Wayland to test the real target, not XWayland):
```bash
cd ~/vcs/sid
WAYLAND_DISPLAY="$WAYLAND_DISPLAY" cargo run -p sid
```
Expected: a native window opens on your compositor showing "sid". **This step needs your physical display — run it yourself / interactively.**

**DECISION POINT.** If the window opens cleanly → continue. If it crashes, renders black, or won't use Wayland → STOP and capture the exact error in Task 6's findings. This is the single most important signal in the whole pivot; a failure here is cheaper now than after the store is built.

- [ ] **Step 4: Commit**

```bash
git add crates/sid/Cargo.toml crates/sid/src/main.rs Cargo.lock
git commit -m "spike(sid): GPUI window opens on Wayland"
```

---

## Task 5: Render the three primitives every sid view needs

**Files:**
- Modify: `~/vcs/sid/crates/sid/src/main.rs`

**Interfaces:**
- Produces: a window proving a scrollable list, a focusable text input that accepts typing, and an aligned monospace grid all render — the minimal vocabulary of the host list, the command/query input, and the terminal.

- [ ] **Step 1: Assemble the validation surface from the current examples**

Extend `render` to lay out three regions side by side / stacked, cribbing each from the pinned gpui examples:
  - **scrollable list** — from `uniform_list` / `list` example; ~30 fake host rows.
  - **text input** — from the `input` / `text_input` example; must show a caret and accept typed characters (proves focus + IME/keyboard on Wayland).
  - **monospace grid** — a `div` with a monospace font rendering ~10 lines of fixed-width text (proto-terminal); verify columns align (proves font metrics — critical, since the embedded terminal is the SSH spearhead's hard piece).

(Shape only — reconcile element/builder names to the pinned API.)

- [ ] **Step 2: Run and observe — the gate**

Run: `cargo run -p sid` (on your Wayland display).
Expected, verify all three by eye + keyboard:
  - list scrolls smoothly with wheel/keys;
  - the input takes focus and accepts typed text with a visible caret;
  - the monospace block is column-aligned.
These three are the rendering foundation of every tab. If any is broken or janky, note it in Task 6 — it directly predicts the cost of the SSH slice.

- [ ] **Step 3: Commit**

```bash
git add crates/sid/src/main.rs
git commit -m "spike(sid): list + text input + monospace grid render on Wayland"
```

---

## Task 6: Record spike findings and the go/no-go gate

**Files:**
- Create: `~/vcs/sid/docs/design/SPIKE-FINDINGS.md`

**Interfaces:**
- Produces: a documented, evidence-based decision on the medium, and the first push to `sid.git`.

- [ ] **Step 1: Write the findings**

Create `~/vcs/sid/docs/design/SPIKE-FINDINGS.md` covering: Wayland window — clean / issues; list scroll quality; text input + caret + keyboard/IME behavior; monospace alignment; GPUI build time + binary size; API friction encountered; outlook for an embedded terminal (reference: Zed's terminal on `alacritty_terminal`); and a verdict:
  - **GREEN** → proceed to the layered-store-foundation plan.
  - **YELLOW** → proceed but list the caveats/spikes to resolve early.
  - **RED** → reconsider the medium (evaluate `iced`/`egui`, which have more mature Linux/Wayland stories) before investing further. The scope-model and store plans are medium-agnostic and survive either way.

- [ ] **Step 2: Commit and push (first push to the new repo)**

```bash
cd ~/vcs/sid
git add docs/design/SPIKE-FINDINGS.md
git commit -m "docs(sid): GPUI-on-Wayland spike findings + go/no-go"
git push -u origin main
```
Expected: pushes onto the new repo's main (fast-forward over the init commit, or reconcile if the init commit conflicts — if so, `git rebase` local work onto `origin/main` first; the init commit is just a README/gitignore).

- [ ] **Step 3: Gate**

The new repo builds, opens a GPUI window on your Wayland with the three primitives working, and carries a written verdict. STOP here and review before starting the store-foundation plan.

---

## Self-Review

- **Scope coverage:** repo logistics (T1), frame + spec (T2), buildable workspace (T3), window spike (T4), primitives spike (T5), findings + gate (T6). The stated goal (fresh repo + GPUI-on-Wayland go/no-go) is fully covered. Store/adapters/features are explicitly out of scope and roadmapped.
- **No fabricated APIs:** GPUI code is given as shape + an explicit "adapt from the pinned current example" instruction, because hardcoding an unstable API would be the *real* placeholder. This is deliberate and correct for a spike whose purpose is to discover that API.
- **Hazard handled:** the origin-mispointing trap is fixed in T1 (rename + set-url) before any push (T6).
- **Deferred items honored:** no MCP/plugin/CI work appears in any task.
