# Is `gpui-kit` a good fit for sid?

**Date:** 2026-09-16
**Status:** research, no code changed. Repo untouched; the compile evidence was
produced in a throwaway crate at `~/.cache/sid-gpui-kit-eval/scratch` (deleted after
this was written).

## Verdict

**Not as a runtime dependency. Adopt it as a dev-dependency, for exactly one thing:
the UI test harness.** `gpui-kit` is a facade crate — its entire published source is
`lib.rs` + `test.rs`, it contains no widgets, no app shell, no theme system and no
window chrome, and its five direct dependencies (`gpui-pre`, `gpui-pre-platform`,
`gpui-base`, `gpui-component`, `gpui-kit-assets`) are *exactly* the five sid already
lists in `[workspace.dependencies]`. Swapping sid's imports onto `gpui_kit::component::`
would buy literally nothing and cost a rename across `sid-ui` plus the rule-1 hygiene
ratchet. But `gpui_kit::test` (new in 0.6.1, 2026-09-09) is the only place upstream
publishes `TestWindowExt` — `find` / `click` / `press` / `input` / `hover` / `drag_to` /
`within` over a real headless window — and sid has no UI interaction tests at all today,
only `sid-ui/tests/hygiene.rs` (a source-text scanner) and sway/`grim` pixel captures.
The deciding reason: sid's cohesion loop currently verifies behaviour by *looking at PNGs*
("25 Tabs stay inside the host form", §3 of the 2026-09-09 loop doc), and this turns that
class of claim into an assertion for one dev-dependency and **+1 package** on a
1069-package lock.

## 1. What `gpui-kit` actually is

**The repo was renamed.** `GET api.github.com/repos/longbridge/gpui-component` now
returns `full_name: longbridge/gpui-kit` (same repo id, created 2024-06-13, 14,416 stars,
101 open issues, last push 2026-09-16T11:02Z). The v0.6.0 release note (2026-09-03) says
it outright: *"The repository and ecosystem are now named **GPUI Kit** … GPUI Component
remains the styled component layer within the toolkit."* (PR #2927). Docs moved to
`gpui-kit.com`. Raw-GitHub URLs under `/longbridge/gpui-component/` still resolve by
redirect.

**The crate is a facade, not a successor.** Published `gpui-kit-0.6.1` contains
`src/lib.rs` and `src/test.rs` and nothing else. `lib.rs` is:

- `pub use ::gpui::*;` — GPUI re-exported at the Kit root
- `pub use ::gpui_base as base;` / `::gpui_component as component` (feature `component`,
  on by default) / `::gpui_kit_assets as assets` (feature `assets`, on) /
  `::gpui_platform as platform`
- `pub use ::gpui_platform::application;`
- `pub fn init(cx)` → `gpui_component::init(cx)` (which sid already calls)
- an `actions!` macro that re-spells `gpui::Action` so consumers need not name `gpui`
- `pub use gpui_base::TestSupportExt;`

That is the whole crate. The workspace members resolve as: `crates/kit` = this facade;
`crates/component` = the styled widgets sid already uses; `crates/base` = the unstyled
behaviour layer sid already uses for `active_focus_trap`; `crates/assets` = the Lucide
catalogue sid already uses; `crates/shell` + `crates/component-shell` = `publish = false`,
a JavaScript scriptable runtime, irrelevant.

**Which crate does sid already depend on transitively?** None — sid names
`gpui-component`, `gpui-base` and `gpui-kit-assets` *directly* in
`[workspace.dependencies]`. `gpui-kit` itself appears nowhere in `Cargo.lock`.

**The one original thing.** `crates/kit/src/test.rs` (431 lines) adds
`TestWindowExt` (`find` / `try_find` / `within` / `click` / `click_at` / `right_click` /
`double_click` / `hover` / `scroll` / `drag_to` / `press` / `input`), `ScopedWindow`, and
`TestAppContextExt`. Everything it builds on — `ElementSnapshot`, `TestSupportExt`,
`find`, `snapshots`, `has_observed_focus`, `scope`, `registered_paths` — lives in
`gpui-base 0.6.1` (`src/test_support.rs`, `src/observe.rs`), which sid **already has**.
`gpui-kit-0.6.0/src/` is `lib.rs` alone: the harness is one week old.

Downloads: `gpui-kit` 11,103 total vs `gpui-component` 121,976. Nobody is on the facade yet.

## 2. Module overlap

`gpui-kit` contributes **zero** widgets, so the honest table lists the nearest
`gpui-component` / `gpui-base` equivalent and notes that sid can already reach every one
of them today without adding `gpui-kit`.

| sid-ui module | Upstream equivalent (reachable today) | Verdict |
|:--|:--|:--|
| `button.rs` (997 L) | `gpui_component::button` — already wrapped | **keep** (variant/ink rules are sid's) |
| `card.rs` `Card::panel` (474 L) | `group_box.rs`, `dock::Panel` | **keep** — panel header *is* the toolbar contract |
| `toolbar.rs` (217 L) | none (Dock tab bar is not this) | **keep** |
| `status_bar.rs` (266 L) | `gpui_component::status_bar` | **keep** — sid's 26 px strip is spec'd in `system.md` |
| `table/*` `FillTable` (1,340 L) | `DataTable` — already wrapped; `Column.width` still `Pixels`-only in 0.6.1 | **keep** (loop doc §1 re-tested this) |
| `modal.rs` (512 L) | `dialog`, `sheet` | **keep** |
| `focus.rs` (113 L) | `gpui_base::focus_trap` — already used | **keep** |
| `theme.rs` + `bridge.rs` (1,226 L) | `gpui_component::theme`, `gpui_base::theme_tokens` | **keep** — the bridge *is* the 13-token adapter |
| `typography.rs` / `scale.rs` (1,012 L) | none | **keep** |
| `icon.rs` (412 L) | `gpui_kit_assets::IconName` — already resolved through it | **keep** |
| `toast.rs` / `notice.rs` (650 L) | `notification.rs`, `alert.rs` | **keep** |
| `segmented.rs` (525 L) | `tab`, `toggle_group` | **keep** |
| `kbd.rs` (198 L) | `gpui_component::kbd` — deliberately demoted to a formatter 2026-09-09 | **keep** |
| `input/*` (927 L) | `gpui_component::input` — already wrapped | **keep** |
| `list` / `grid` / `meter` / `badge` / `action_cell` / `status_dot` / `scope_chip` / `empty_state` / `radio` / `elevation` / `styled` / `tooltip` / `gallery` | partial or none | **keep** |
| — | `gpui_kit::test::TestWindowExt` + `ScopedWindow` | **new — the only thing worth taking** |

Net: adopting `gpui-kit` as a runtime dep deletes **0 lines** of the ~13.9 k in `sid-ui`
and adds a second vocabulary (`gpui_kit::component::X` alongside `sid_ui::X`). As a
dev-dep it adds no vocabulary to shipping code at all.

## 3. Fit with sid's rules

- **Rule 1 (confinement): yes, trivially, as a dev-dep.** `sid-ui/tests/hygiene.rs`
  scans `sid-ui/src` and `sid/src` only — `tests/` trees are not scanned, and the ban is
  on the literal `gpui_component::`, which a dev-dep never introduces. *One caveat:*
  `.test_support()` must be called on the element inside **production** render code to
  register it for querying. It is a documented no-op in normal builds ("keeps production
  render chains intact and returns the original element"), but it means `sid/src` tab code
  would name `gpui_base::TestSupportExt`. Fix is one line: `pub use
  gpui_base::TestSupportExt;` from `sid_ui`, so call sites keep naming `sid_ui`.
- **Forces a theme system / app shell / window chrome? No.** `gpui_kit::init` *is*
  `gpui_component::init`, which sid already calls. No `Root`, no title bar, no dock, no
  decoration is imposed. Nothing fights "one top chrome bar", the 13 tokens, or "no
  shadows" — those constraints are sid's bridge's job and the bridge is untouched.
- **Dependency footprint: +1 package.** `cargo tree -e no-dev -p gpui-kit --depth 1`
  → `gpui-base 0.6.1`, `gpui-component 0.6.1`, `gpui-kit-assets 0.6.1`, `gpui-pre 0.3.5`,
  `gpui-pre-platform 0.3.5`. All five are already in sid's lock. The published manifest
  pins `gpui = "0.3.1"` (caret — unifies with sid's 0.3.4) and `gpui_platform` with
  `["font-kit", "x11", "wayland", "runtime_shaders"]`, the identical set sid names.
- **Compile time: real but bounded.** Cold graph in the scratch crate = 716 crates, 4.8 G
  debug target. `test-support` produces a second build variant for **83** crates
  (including `gpui-pre` and `gpui-component`). `cargo tree -f "{p} {f}"` confirms the
  resolver keeps them apart — a plain `cargo build` resolves `gpui-pre` as
  `default,font-kit,wayland,windows-manifest,x11`; only with dev-deps does it gain
  `test-support,proptest,backtrace,leak-detection`. **Release builds stay clean**; the
  cost is one large `cargo test` rebuild after adoption and a fatter `target/`.
- **API stability: good, for a one-week-old API.** `crates/kit/src/test.rs` is **byte-
  identical** between published 0.6.1 and `main` HEAD as of today. `lib.rs` drifted 39
  diff lines, all of it iOS/Android cfg gating. 0.6.0 → 0.6.1 was additive (multi-cursor,
  bracket pairing, headless testing); nothing in the facade broke. 101 open issues on a
  14.4 k-star repo.

## 4. Top three concrete wins

1. **Focus, tab order and modal traps become assertions instead of captures.** The loop
   doc's strongest existing evidence for the focus trap is an A/B screenshot run. With
   `w.press("tab", cx)` in a loop and `snap.focused()`, that becomes a test that fails in
   CI. Same for Enter/Esc routing and the twelve migrated input call sites.
2. **`FillTable` column arithmetic gets a real oracle.** `column_width.rs` (493 L) +
   `columns.rs` (348 L) currently prove themselves with `sid-cap.sh` at 700 px and 900 px.
   `snap.bounds()` returns real laid-out pixels in a headless window, so "the filter
   shrinks to `PANEL_FILTER_FLOOR` before the title truncates" is one assertion at any
   width, in any theme, in 0.01 s.
3. **Regressions like the 0.6 Button one get caught before the capture pass.** The
   `Custom`-variant 20 %-alpha/lost-border regression was found by eye during the upgrade
   spike. State and geometry regressions of that shape are exactly what
   `ElementSnapshot` reports. This directly de-risks the next `gpui-pre` bump.

Things that would get **thinner**: nothing in `sid-ui` shrinks. The win is additive
coverage, not deletion — which is why this is a dev-dep, not an adoption.

## 5. Top three risks

1. **The harness is seven days old** (`gpui-kit 0.6.0` has no `test.rs`; 0.6.1 shipped
   2026-09-09). Its own `TESTING.md` hedges heavily: missed-binding diagnostics are "best
   effort", snapshots "cannot infer an unobserved ancestor's opacity or inspect pixels",
   and this is explicitly "not exhaustive option coverage".
2. **No pixel capture on Linux — `sid-cap.sh` is not replaced.** The `rendering` target
   is `test = false`, macOS-only, and requires a real Metal device; TESTING.md: *"Other
   platforms explicitly skip this target until GPUI supplies a headless renderer."* The
   sway/`grim` harness stays the only way to check ink, contrast and the four themes. Kit
   tests *state and geometry*, not appearance.
3. **Same pre-release lock-in sid already accepted, one layer deeper.** `gpui-pre` is a
   snapshot line and the facade is one more crate that has to be bumped in lockstep on
   every `gpui-pre` / `gpui-component` move. Mitigated by keeping it out of shipping code
   entirely — a broken dev-dep blocks `cargo test`, never a release.

*Not a risk:* **no macOS chrome assumption.** The published manifest carries x11 + wayland
features; macOS-only code is `cfg(target_os = "macos")`-gated in `gpui-base`
(`macos_accessibility.rs`, `objc2-app-kit`). Verified by running the harness on this
machine.

## 6. Compile-and-run evidence (Linux, headless, no display server)

Scratch crate, `gpui-kit = "0.6.1"` + dev-dep `features = ["test-support"]`, rendering a
real `gpui_component::Button`:

```
PATHS: [View(EntityId(1v1)), Name("gpui_component::button::button::Button"),
        Name("gpui_base::button::Button"), Name("go")]
LABEL: Some("Go") BOUNDS: Bounds { origin: (0px, 0px), size: 1920px × 32px } VIS: true
HITS: 1
test clicks_a_real_button ... ok    (finished in 0.01s)
```

`w.find("go")` read the accessibility label and the real laid-out bounds;
`w.click("go", cx)` fired the genuine `on_click` listener. No compositor, no sway, no grim.

**One papercut worth writing down before anyone hits it.** `use gpui_kit::*;` in a test
file glob-imports GPUI's `test` macro, which shadows Rust's built-in `#[test]`, so
`#[gpui_kit::test]` expands into itself: `error: recursion limit reached while expanding
#[test]`. Following the compiler's own suggestion (`recursion_limit = "512"`) **segfaults
rustc** (`SIGSEGV` in `gpui_macros`). The fix is to import Kit types explicitly, which
`gpui-kit`'s `lib.rs` says in a comment but the diagnostic never mentions. Worth an
upstream issue.

## 7. Suggested first step (partial adoption)

Smallest thing that proves the value, systems-first, red before green:

1. `crates/sid-ui/Cargo.toml`, `[dev-dependencies]`:
   `gpui-kit = { version = "0.6.1", features = ["test-support"] }` — dev only, nothing
   else in the workspace changes.
2. `crates/sid-ui/src/lib.rs`: `pub use gpui_base::TestSupportExt;` so call sites name
   `sid_ui`, not `gpui_base` (rule 1, keeps the ratchet honest).
3. `.test_support()` on the identified element in `sid_ui::Button`, `TextInput` and
   `Modal` — **before** `.track_focus(&handle)`, per TESTING.md, or focus queries silently
   return `None`.
4. One new file `crates/sid-ui/tests/focus_trap.rs` that reproduces the existing A/B
   capture claim as an assertion: open a `Modal` with three fields, press Tab 25 times,
   assert focus never leaves the trap. Write it **red first** — delete the
   `tab_group()`/`focus_trap` call, watch it fail, restore it, watch it pass. That test is
   the whole deliverable; if it can't be made to fail for the right reason, stop and drop
   the dependency.
5. Only if step 4 lands green: a second test pinning `FillTable`'s
   `PANEL_FILTER_FLOOR` behaviour via `snap.bounds()` at 700 px.

Do **not** move any `use gpui_component::` to `use gpui_kit::component::`. There is no
benefit, and it would mean re-pointing `sid-ui/tests/hygiene.rs`'s
`the_rendering_library_is_named_only_in_sid_ui_and_the_composition_root` scanner at a new
literal — churn with no payoff.

## Sources

- `https://api.github.com/repos/longbridge/gpui-component` → redirects to
  `longbridge/gpui-kit` (14,416 ★, 101 open issues, pushed 2026-09-16T11:02Z)
- GitHub release v0.6.0 (2026-09-03), v0.6.1 (2026-09-09), longbridge/gpui-kit
- `https://raw.githubusercontent.com/longbridge/gpui-component/main/Cargo.toml`
  (workspace members, `gpui-pre` 0.3.5 line)
- `crates/kit/{Cargo.toml,README.md,TESTING.md,src/lib.rs,src/test.rs}` @ main, and the
  published `gpui-kit-0.6.0` / `gpui-kit-0.6.1` tarballs from `static.crates.io`
- `gpui-base-0.6.1/src/test_support.rs`, `src/observe.rs`, `src/lib.rs:132,215`
- `gpui-component-0.6.1/src/` module listing (dock, sidebar, title_bar, window_border,
  status_bar, setting, notification, command, resizable — all already reachable by sid)
- crates.io API: `gpui-kit` 0.1.0/0.6.0/0.6.1, 11,103 downloads; `gpui-component`
  0.6.1, 121,976 downloads
- sid: `Cargo.toml`, `Cargo.lock` (1069 packages, `gpui-pre` 0.3.4),
  `crates/sid-ui/Cargo.toml`, `crates/sid/Cargo.toml`, `crates/sid-ui/src/lib.rs`,
  `crates/sid-ui/tests/hygiene.rs:29-35,556-600`, `.interface-design/system.md:7-33`,
  `docs/design/2026-09-09-ui-cohesion-loop.md` §0/§1/§3/§4
- Local compile evidence: `~/.cache/sid-gpui-kit-eval/scratch` (716 crates, 4.8 G target,
  83 double-built crates under `test-support`), deleted after writing this
