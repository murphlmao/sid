# Settings Tab Iteration — Implementation Plan

Spec: `docs/superpowers/specs/2026-06-13-settings-tab-iteration-design.md`
Branch: `feat/settings-tab-iteration`

## Execution model
- **Foundation (opus/orchestrator):** `MotionStyle` in `sid-core` — gates W3.
- **Parallel grunt (2× sonnet, disjoint crates, no worktrees):** agent FX owns `sid-fx`; agent WIDGETS owns `sid-widgets`. Neither touches the binary or the other's crate.
- **Integration (opus/orchestrator):** all binary wiring (`sid/src/wire.rs`, `main.rs`, `settings_undo.rs`).
- **Review (cheap) + one `/sid-gate`.**

## API contracts the subagents MUST honor (so the binary can consume them)

### sid-fx (agent FX)
- Keep public fns `render_starfield(buf, area, &FxState, &AnimationConfig, &Theme)` and `render_supernovae(...)` signature-compatible.
- Add `render_shooting_stars(buf, area, &FxState, &AnimationConfig, &Theme)` OR fold shooting-star rendering into `render_supernovae`/`render_starfield` — but expose ONE additional public render fn if a new layer is added, documented. If folded in, no new call site needed.
- `FxState::tick(area, &AnimationConfig)` unchanged signature; internally branches on `cfg.motion`.
- Determinism preserved: `with_seed` reproducible.

### sid-widgets (agent WIDGETS)
- `pub enum LogLevel { Info, Success, Error }` (Copy, Eq).
- `pub struct LogEntry { pub epoch: u64, pub level: LogLevel, pub message: String }` + `LogEntry::new(epoch, level, message)`.
- `pub struct LogsView` with: `new()`, `push(LogEntry)` (caps at `LOG_RING_CAP`, evicts oldest), `entries() -> &VecDeque<LogEntry>`, scroll handling in `handle_event`.
- `SettingsWidget`: `pub fn record_log(&mut self, entry: LogEntry)` that routes into the `Logs` category's `LogsView` (no-op-safe if category absent). Bottom tail strip rendered by `SettingsWidget` itself (reserve bottom rows in `render_into_frame`).
- `SettingsCategory::Logs(LogsView)` variant + label "Logs".
- `MotionStyle` picker in `AnimationView`: cycles `cfg.motion`, emits the existing animation outcome so the binary persists it via `AnimationChanged`.

## Tasks

### Foundation
- [ ] F1. `sid-core`: add `MotionStyle { Twinkle, Drift, Cosmos }` (default `Cosmos`) + `#[serde(default)] pub motion` on `AnimationConfig`. Tests: default-is-cosmos, round-trip all styles, legacy-JSON-without-motion → Cosmos + other fields preserved. `cargo test -p sid-core animation`

### Agent FX (sonnet, crate `sid-fx`)
- [ ] FX1. `Star` fixed-point position (`xq`,`yq` 1/256 cell) + velocity; `spawn_star` sets them.
- [ ] FX2. `tick()` branches on `cfg.motion` (Twinkle/Drift/Cosmos); Drift integrates+wraps; brighter=faster.
- [ ] FX3. Boosted twinkle amplitude (visible) used by Twinkle + Cosmos.
- [ ] FX4. Shooting-star transient entity + spawn cadence in Cosmos + render.
- [ ] FX5. Tests: drift preserves count & stays in-bounds (proptest), determinism w/ seed, Twinkle positions fixed, Cosmos spawns shooting stars, render no-panic on tiny/huge areas. `cargo test -p sid-fx`

### Agent WIDGETS (sonnet, crate `sid-widgets`)
- [ ] WG1. `settings/logs.rs`: `LogLevel`, `LogEntry`, `LogsView` (+ `LOG_RING_CAP`), doc tests, unit tests (cap/evict, scroll bounds, time format).
- [ ] WG2. `SettingsCategory::Logs` + label; `SettingsWidget::record_log`; bottom tail strip in `render_into_frame` (newest N, always visible). Tests.
- [ ] WG3. Arrow-nav in `handle_event`: Right/`l` enter; Esc universal back; Left back except Behavior/DbPath. Tests for each category.
- [ ] WG4. `AnimationView` motion-style picker (cycles `cfg.motion`, emits outcome). Tests.
- [ ] WG5. Remove `auto_scan_workspaces` toggle + `reset.rs` FACTORY_KEYS entry; fix tests (count 6→5, index comments). `cargo test -p sid-widgets settings`

### Integration (opus/orchestrator, crate `sid`)
- [ ] I1. `SidApp`: add `active_theme: Theme`, `persister: StatePersister`, `last_heartbeat: Instant`. Update all constructors/tests.
- [ ] I2. W1 theme: populate `active_theme` in `main.rs`; `draw()` uses it; `ThemeApplied` updates live.
- [ ] I3. W4 logs: `record()` helper; route `persist_outcome` + direct toast sites through it; forward entries into settings widget each loop; render tail+category use widget API.
- [ ] I4. W5 toasts: `const TOASTS_ENABLED=false;` gate on `render_toasts` call.
- [ ] I5. W6 auto-restore: branch `main.rs` on `AUTO_RESTORE_SESSION`.
- [ ] I6. W6 default-tab: fallback in `main.rs` when no CLI arg.
- [ ] I7. W6 persist-debounce: wire `StatePersister`; gate `save_active_tab`; flush on quit.
- [ ] I8. W6 heartbeat: `last_heartbeat` + Tick interval `upsert_session`.
- [ ] I9. Fix binary `settings_undo.rs` proptest keys (drop auto_scan).
- [ ] I10. Per-area scoped tests (`cargo test -p sid <name>`), then `/sid-gate`.

## Scoped test commands (never run full workspace mid-flight)
- F1: `cargo test -p sid-core animation`
- FX: `cargo test -p sid-fx`
- WIDGETS: `cargo test -p sid-widgets settings`
- Integration: `cargo test -p sid <test_name>` per area; final `/sid-gate`.
