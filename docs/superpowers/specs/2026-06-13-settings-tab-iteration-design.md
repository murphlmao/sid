# Settings Tab Iteration — Design

**Date:** 2026-06-13
**Status:** Approved (owner waived per-section approval; proceed directly)
**Scope:** Five fixes/features on the Settings tab. Each is independently testable.

## Problem statement (owner report)

1. Cannot use `→` to enter a category's options panel; only `Tab` toggles panes.
2. Themes show no visual difference — suspected non-functional.
3. Setting-apply pop-ups/log lines cover the bottom of the screen and look bad; want a dedicated logs area in Settings.
4. Most behavior settings (resume / autoresume, etc.) do nothing.
5. Animations "don't work" — stars only shift on resize, never animate on their own.

## Root causes (verified, with evidence)

| Area | Root cause | Evidence |
|---|---|---|
| Themes | `draw()` hardcodes `let theme = cosmos();`; `SidApp` has no theme field; the loaded theme is discarded. | `wire.rs:1185`, `main.rs:445` (`let _ = active_theme;`) |
| Animation | Stars are **fixed-position by design**; `tick()` only advances a brightness phase. At `fps=8`, `speed 1..=8` the twinkle cycle is ~8 s and the brightness lerp on dark colors is imperceptible. Ticks DO fire and DO advance — there is no plumbing bug. | `sid-fx/src/lib.rs:51-55,286-288`; pump `runtime.rs`; loop `wire.rs:1890-1911` |
| Navigation | No `KeyCode::Right`/`Left` handling at `SettingsWidget` level; only `Tab`/`BackTab` toggle. | `settings.rs:698-911` |
| Logs/toasts | Toasts are fire-and-forget, bottom-right, 3 s, no backing store; emitted from `persist_outcome` + direct sites. | `wire.rs:2474-2497,2416,2430,2436,2452`, `toast.rs` |
| Dead settings | 5 of 6 behavior toggles are stored but never read; `maybe_push_resume_modal` ignores `AUTO_RESTORE_SESSION`; `StatePersister` exists in `sid-core` but is **not wired into the binary at all**. | `main.rs:707`, `wire.rs:1080-1125`, `sid-core/src/persister.rs` (unused) |

## Decisions (owner)

- **Animation:** ship all three as selectable styles — `Twinkle`, `Drift`, `Cosmos` — chosen in the Animation settings panel. Default `Cosmos`.
- **Logs:** Both — a bottom-strip tail (always visible in Settings) AND a full scrollable "Logs" category, fed by one shared ring buffer.
- **Toasts:** logs-only for now — stop rendering floating toasts; keep all toast code intact but disabled behind a flag (revisit later).
- **Settings wiring:** wire up all four (auto-restore, default tab, persist-debounce, heartbeat). Remove the `auto_scan_workspaces` toggle (feature no longer exists; keep the store const as legacy).

## Design by workstream

### W1 — Theme applies (bug fix; crate: `sid`)
- Add `active_theme: Theme` to `SidApp`; populate from `load_active_theme()` in `main.rs` (stop discarding).
- `draw()`: `let theme = &sid_app.active_theme;` (one line; widgets already take `&theme`).
- On `ThemeApplied`, mutate `sid_app.active_theme` live; drop "(takes effect on restart)".

### W2 — `→` enters a category (crate: `sid-widgets`)
- In `SettingsWidget::handle_event`: `Categories` + `Right`/`l` → `SubView`. `SubView` + `Esc` → `Categories` (universal). `SubView` + `Left` → `Categories` except for sub-views that bind Left to value-cycling (`Behavior`, `DbPath`), where Left keeps cycling. `Tab`/`BackTab` unchanged.

### W3 — Animation motion styles (crates: `sid-core`, `sid-fx`, `sid-widgets`)
- `MotionStyle { Twinkle, Drift, Cosmos }` on `AnimationConfig` (`#[serde(default)]`, default `Cosmos`). JSON-persisted → migration-safe.
- `sid-fx`: `Star` gains fixed-point sub-cell position (`xq`, `yq` in 1/256 cell units) + per-star velocity. `tick()` branches on `motion`:
  - `Twinkle`: positions fixed; brightness boosted (wider amplitude, visible).
  - `Drift`: integrate velocity each tick; wrap at edges; brighter stars faster (parallax).
  - `Cosmos`: Drift + boosted Twinkle + shooting-star streaks + tightened idle supernova cadence.
- Shooting star = lightweight transient entity (like `Supernova`): a short streak that moves a few cells over a few frames then expires.
- Picker added to `AnimationView` (`sid-widgets/settings/animation.rs`); persisted via existing `AnimationChanged` flow; live (config passed every frame).

### W4 — Logs panel (crates: `sid-widgets`, `sid`)
- `LogLevel { Info, Success, Error }`, `LogEntry { epoch: u64, level, message }`, `LogsView` (capped `VecDeque`, scroll) in `sid-widgets/settings/logs.rs`. Time formatted dep-free from epoch (HH:MM:SS).
- New `SettingsCategory::Logs(LogsView)`. Settings page reserves a bottom tail strip (newest N), always visible in Settings; the Logs category shows full scrollable history. Same buffer.
- Binary: `record(sid_app, level, msg)` helper appends to the log ring AND the (disabled) toast queue; the wire loop forwards new entries into the settings widget's `LogsView`.

### W5 — Toasts logs-only (crate: `sid`)
- Gate the `render_toasts()` call behind `const TOASTS_ENABLED: bool = false;`. Keep `ToastQueue`/`Toast`/push sites compiling and tested.

### W6 — Wire dead settings; remove auto-scan (crates: `sid`, `sid-widgets`)
- `AUTO_RESTORE_SESSION` (`yes`/`ask`/`no`): branch in `main.rs` before `maybe_push_resume_modal` — `yes` auto-restores silently, `ask` = current modal, `no` = skip.
- `DEFAULT_TAB`: `main.rs:518`, when `cli.start_tab.is_none()` fall back to the setting before `build_app_hydrated`.
- `PERSIST_DEBOUNCE_MS`: construct `StatePersister::new(Duration::from_millis(setting))`, store on `SidApp`, gate `save_active_tab` behind `should_flush()`, `mark_dirty()` on state change, **flush on quit**.
- `HEARTBEAT_INTERVAL_SECS`: `last_heartbeat: Instant` on `SidApp`; on `Tick`, if elapsed ≥ interval, `upsert_session` with fresh `last_active`.
- Remove `auto_scan_workspaces` toggle from `behavior_toggles.rs` + `reset.rs` FACTORY_KEYS; fix referencing tests (`behavior_toggles.rs:489,504,692`, binary `settings_undo.rs:233` proptest).

## Adapter compliance
`MotionStyle` ∈ `sid-core`; motion logic ∈ `sid-fx`; `LogsView`/`LogEntry` ∈ `sid-widgets` (no external crate names); theme/persister/heartbeat wiring ∈ `sid` binary. Respects the Edit-time adapter hook.

## Testing policy (owner constraint)
Per-workstream **scoped** tests only (`cargo test -p <crate> <module>`), never the full workspace mid-flight. One `/sid-gate` pass at the end.
