# UX-v2 Branch 7 — Fix Dead Starfield Animations

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking. Read `2026-06-11-uxv2-master.md` first for the
> binding design decisions. Testing is TARGETED per task (decision 11 in the master plan):
> run only the tests named in each step, not the whole workspace.

**Goal:** The starfield animations are non-functional in two interlocking ways: (1) they tick
once per any event (key, mouse, timer), not once per timer tick — so keyboard activity makes
stars twinkle frantic while idle sessions tick at the wrong rate; (2) the pump's tick interval
is hardcoded to 250 ms regardless of `animation.fps`, so the user's FPS setting has no effect
on the actual render rate; (3) when the user changes `AnimationConfig` in the Settings tab and
presses `S`, `SidApp.animation` and `SidApp.fx_state` in `run_event_loop` are never updated —
the live animation state is permanently stale relative to the persisted config. Fix all three
without touching the form/focus substrate used by other UX-v2 branches.

**Architecture:** Three orthogonal bugs:

1. **Wrong tick gate (`crates/sid/src/wire.rs:1596`).** `fx.tick()` is called unconditionally
   at the top of `run_event_loop`'s `loop { ... }` body, before every draw, for every event
   (including key presses and mouse moves). The spec (`docs/superpowers/specs/2026-05-22-sid-
   ux-iteration.md:364`) says the pump wakes on a key event OR a 1/FPS timer tick — animation
   must advance only on `SidEvent::Tick`, not on every event. Fix: add a `received_tick: bool`
   flag that is `true` only when the current event is a `Tick`, and gate `fx.tick()` on that
   flag (move the tick call to after `rx.recv().await`, guarded by `if received_tick`).

2. **Hardcoded pump interval (`crates/sid/src/main.rs:602`).** The pump is spawned with
   `Duration::from_millis(250)` (4 fps). `AnimationConfig::fps` defaults to 8; the pump never
   consults it. Fix: compute `tick_ms = 1000 / animation.fps.max(1)` from the loaded
   `AnimationConfig` and pass `Duration::from_millis(tick_ms as u64)` to
   `runtime::spawn_event_pump`. The pump interval does not need to change at runtime (FPS
   changes take effect on next restart, same as `enabled`/`density`).

3. **Stale `SidApp.animation` after Settings save (`crates/sid/src/wire.rs:1716`).** When the
   user presses `S` in `AnimationView`, `flush_via_embedded_store` writes the new config to
   the store (`crates/sid-widgets/src/settings/animation.rs:272-278`) but emits no signal to
   wire.rs. `SidApp.animation` is never refreshed; `SidApp.fx_state` is never toggled to
   match `animation.enabled`. Fix: add a `PendingSettingsOutcome::AnimationChanged(AnimationConfig)`
   variant in `sid-widgets/src/settings/mod.rs`; emit it from `AnimationView::handle_event`
   on save (in addition to the existing store write); drain it in
   `apply_pending_settings_outcomes` to replace `SidApp.animation` and toggle
   `SidApp.fx_state`.

**Tech Stack:** Rust, ratatui (sid-widgets only), crossterm (sid-core event types), `tokio`
(runtime tick interval already in `sid/`), `insta` for snapshot regression.

---

### Task 1: Regression test — gate `fx.tick()` on `Tick` events only

This task establishes the failing regression test required by project convention (every bug
fix starts with a test that reproduces the bug before the fix is written). The test lives in
`crates/sid/src/wire.rs` under `#[cfg(test)]` and drives `run_event_loop` with a fake
`TestBackend` terminal and a hand-rolled event sequence.

**Files:**
- Modify: `crates/sid/src/wire.rs` — add test `animation_only_ticks_on_tick_event` inside
  the existing `#[cfg(test)] mod tests {` block (located at the bottom of the file, after
  `fn build_test_sid_app`).

- [x] Locate the existing `#[cfg(test)] mod tests` block in `crates/sid/src/wire.rs`. Find
  the `build_test_sid_app` helper function inside it and note its signature.

- [x] Add the following test immediately after the last existing `#[test]` in that block:

```rust
/// `fx.tick()` must advance `tick_count` exactly once per `SidEvent::Tick`
/// and must NOT advance it on key presses or mouse events.
///
/// Before the fix this test fails because `fx.tick()` fires once per loop
/// iteration regardless of event kind, so two key-press events advance
/// `tick_count` by 2 instead of 0.
#[tokio::test]
async fn animation_only_ticks_on_tick_event() {
    use ratatui::backend::TestBackend;
    use tokio::sync::mpsc;

    let backend = TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();

    let mut sid_app = build_test_sid_app(Some("workspaces"));
    // Replace the default None fx_state with a seeded one so tick_count is
    // observable. (build_test_sid_app sets fx_state = None; override it.)
    sid_app.fx_state = Some(sid_fx::FxState::with_seed(42));
    sid_app.animation = sid_core::animation::AnimationConfig::default();

    let (tx, mut rx) = mpsc::channel::<sid_core::event::Event>(16);

    // Send two key-press events then a Tick, then close the channel so the
    // loop exits after draining them.
    tx.send(sid_core::event::Event::Key(
        sid_core::event::KeyChord::new(
            crossterm::event::KeyCode::Char('j'),
            crossterm::event::KeyModifiers::NONE,
        ),
    ))
    .await
    .unwrap();
    tx.send(sid_core::event::Event::Key(
        sid_core::event::KeyChord::new(
            crossterm::event::KeyCode::Char('k'),
            crossterm::event::KeyModifiers::NONE,
        ),
    ))
    .await
    .unwrap();
    tx.send(sid_core::event::Event::Tick).await.unwrap();
    drop(tx); // close channel → loop exits after Tick

    run_event_loop(&mut terminal, &mut sid_app, &mut rx)
        .await
        .unwrap();

    let tick_count = sid_app
        .fx_state
        .as_ref()
        .expect("fx_state must remain Some")
        .tick_count;

    // BEFORE the fix: tick_count == 3 (once per loop iteration).
    // AFTER the fix:  tick_count == 1 (only on the Tick event).
    assert_eq!(
        tick_count, 1,
        "tick_count should be 1 (only the Tick event should advance it), got {tick_count}"
    );
}
```

- [x] Run the test to confirm it **fails** before any code change:
  ```
  cargo test -p sid animation_only_ticks_on_tick_event 2>&1 | tail -20
  ```
  Expected: `FAILED` with `tick_count` equal to 3 (or 2), not 1.

---

### Task 2: Fix — gate `fx.tick()` on `SidEvent::Tick`

**Files:**
- Modify: `crates/sid/src/wire.rs` — `run_event_loop` function, lines 1562–1710.

- [x] In `run_event_loop`, move the `fx.tick(...)` block (currently at lines 1596–1612,
  before `terminal.draw(...)`) so that it executes AFTER `rx.recv().await` and ONLY when
  the received event is `SidEvent::Tick`.

  The current structure is:
  ```rust
  loop {
      drain_pending_submits(sid_app);
      // ... other drains ...
      // Advance starfield phase on each frame before drawing.
      if let Some(fx) = sid_app.fx_state.as_mut() {
          let area = terminal.size()...
          fx.tick(area, &sid_app.animation);
      }
      terminal.draw(|f| draw(f, sid_app))?;
      let ev = match rx.recv().await { ... };
      // ... event routing ...
  }
  ```

  Replace with:
  ```rust
  loop {
      drain_pending_submits(sid_app);
      // ... other drains (unchanged) ...
      terminal.draw(|f| draw(f, sid_app))?;
      let ev = match rx.recv().await {
          Some(e) => e,
          None => break,
      };

      // Advance starfield phase on timer ticks only — not on key/mouse events.
      // This ensures the visual twinkle rate matches `animation.fps` (the pump
      // interval is set from fps in main.rs) rather than keyboard activity.
      // Per spec (docs/superpowers/specs/2026-05-22-sid-ux-iteration.md:364):
      // "the tokio event pump wakes on either a key event OR a 1/FPS tick".
      if ev == sid_core::event::Event::Tick {
          if let Some(fx) = sid_app.fx_state.as_mut() {
              let area = terminal
                  .size()
                  .map(|s| Rect { x: 0, y: 0, width: s.width, height: s.height })
                  .unwrap_or(Rect { x: 0, y: 0, width: 80, height: 24 });
              // Skip ticking while a modal is open (spec line 366).
              if sid_app.modal_stack.is_empty() {
                  fx.tick(area, &sid_app.animation);
              }
          }
      }

      // ... rest of event routing unchanged ...
  }
  ```

  Note: this also fixes the comment-vs-code drift at line 1232 (`wire.rs`) where the comment
  claimed "we don't tick stars while a modal is open" but the code did not enforce that guard.
  The `sid_app.modal_stack.is_empty()` check is now real.

- [x] Run the regression test to confirm it now **passes**:
  ```
  cargo test -p sid animation_only_ticks_on_tick_event 2>&1 | tail -10
  ```
  Expected: `test animation_only_ticks_on_tick_event ... ok`.

- [x] Run the full `sid` test suite to check no regressions:
  ```
  cargo test -p sid 2>&1 | tail -20
  ```
  Expected: all tests pass.

- [x] Commit:
  ```
  fix(sid): gate fx.tick() on SidEvent::Tick only, skip during modals
  ```

---

### Task 3: Fix — derive pump tick interval from `animation.fps`

**Files:**
- Modify: `crates/sid/src/main.rs` — line 602 (`spawn_event_pump` call).

- [x] In `crates/sid/src/main.rs`, change the hardcoded `Duration::from_millis(250)` at
  line 602 to be derived from the already-loaded `animation` config. The `animation` local
  is created at line 551 (`let animation = wire::load_animation_config(&*store);`) and is
  still in scope at line 602.

  Current code:
  ```rust
  let pump = runtime::spawn_event_pump(tx, Duration::from_millis(250));
  ```

  Replace with:
  ```rust
  // Derive the pump tick interval from the animation FPS so `SidEvent::Tick`
  // fires at the configured rate. `fps.max(1)` prevents division by zero;
  // `min(30)` clamps the max to a sane value (matching AnimationConfig range).
  let tick_ms = 1000u64 / (animation.fps.max(1).min(30) as u64);
  let pump = runtime::spawn_event_pump(tx, Duration::from_millis(tick_ms));
  ```

  At default `fps = 8`, `tick_ms = 125` — matching the spec's "once every 125ms".

- [x] Add a regression test to `crates/sid/src/wire.rs` tests confirming the tick interval
  derivation formula is correct (pure arithmetic — no runtime needed):

  ```rust
  #[test]
  fn tick_interval_derives_from_fps() {
      // At fps=8 (default), tick interval must be 125ms (1000/8).
      assert_eq!(1000u64 / 8u64, 125);
      // At fps=1 (min), tick interval is 1000ms.
      assert_eq!(1000u64 / 1u64, 1000);
      // At fps=30 (max), tick interval is 33ms.
      assert_eq!(1000u64 / 30u64, 33);
      // fps=0 is prevented by .max(1); verify the guard.
      assert_eq!(1000u64 / (0u8.max(1).min(30) as u64), 1000);
  }
  ```

  Run:
  ```
  cargo test -p sid tick_interval_derives_from_fps 2>&1 | tail -5
  ```
  Expected: `ok`.

- [x] Run the full `sid` test suite:
  ```
  cargo test -p sid 2>&1 | tail -20
  ```
  Expected: all tests pass.

- [x] Commit:
  ```
  fix(sid): derive event pump tick interval from animation.fps (was hardcoded 250ms)
  ```

---

### Task 4: Regression test — Settings save propagates `AnimationConfig` to `SidApp`

This is the pre-fix failing test for bug 3: `SidApp.animation` never updates after the user
changes settings and presses `S`.

**Files:**
- Modify: `crates/sid-widgets/src/settings/mod.rs` — add `AnimationChanged` variant to
  `PendingSettingsOutcome`.
- Modify: `crates/sid/src/wire.rs` — add test, then (Task 5) add drain handler.

- [x] Open `crates/sid-widgets/src/settings/mod.rs`. Find the `PendingSettingsOutcome` enum.
  It currently has one variant: `BehaviorToggled { key: String, value: ToggleValue }`.

  Add:
  ```rust
  /// Emitted by [`crate::settings::animation::AnimationView`] when the user
  /// presses `S` and the flush succeeds. The binary drains this variant and
  /// replaces `SidApp.animation` in place, then toggles `SidApp.fx_state`
  /// to match `config.enabled`.
  AnimationChanged(sid_core::animation::AnimationConfig),
  ```

- [x] Open `crates/sid-widgets/src/settings/animation.rs`. In `handle_event` at line 319,
  find the `S`-key match arms (lines 327–333). After `self.try_save()` succeeds, we need to
  emit the new config as a `PendingSettingsOutcome`. However, `handle_event` currently has no
  access to the `SettingsWidget`'s pending-outcome queue.

  The correct plumbing is to emit via `WidgetCtx` (the `_ctx` parameter). Check how
  `BehaviorTogglesView` emits outcomes by inspecting `crates/sid-widgets/src/settings/
  behavior_toggles.rs` — look for where it pushes to the outcome queue.

- [x] Read `crates/sid-widgets/src/settings/behavior_toggles.rs` to understand how outcomes
  are enqueued. The pattern is: each sub-view returns an outcome type to the parent
  `SettingsWidget`, which aggregates into `pending_outcomes`. Follow the same pattern for
  `AnimationView`.

- [x] In `crates/sid-widgets/src/settings/animation.rs`, change `handle_event` to return
  an `Option<AnimationConfig>` (the newly saved config) on a successful S-save, instead of
  always returning `EventOutcome::Consumed`. Specifically, change `try_save` to return
  `Option<AnimationConfig>` — `Some(self.cfg.clone())` on `Ok(true)`, `None` otherwise.

  Updated `try_save` signature and body:
  ```rust
  fn try_save(&mut self) -> Option<AnimationConfig> {
      match self.flush_via_embedded_store() {
          Ok(true) => Some(self.cfg.clone()),
          Ok(false) => {
              eprintln!(
                  "AnimationView: S pressed but no store bound; \
                   use AnimationView::with_store(...) to enable saving"
              );
              None
          }
          Err(e) => {
              eprintln!("AnimationView: flush_dirty failed: {e}");
              None
          }
      }
  }
  ```

  The `handle_event` return type stays `EventOutcome`; the newly saved config is returned
  via the `WidgetCtx` action bus. Look at how `BehaviorTogglesView` uses `ctx` to emit
  its outcome and mirror the pattern: call `ctx.emit_action(...)` with a well-known action
  key `"settings.animation.saved"` and encode the config as JSON in the action payload.

  Then in `SettingsWidget::handle_event` (`crates/sid-widgets/src/settings.rs`), intercept
  the `"settings.animation.saved"` action from the context's action log and push a
  `PendingSettingsOutcome::AnimationChanged(cfg)` to the widget's pending-outcomes queue.

  **NOTE:** If the `WidgetCtx` / action-bus mechanism is not how `BehaviorTogglesView` works
  (look at the actual code before implementing), adapt to the real pattern. Do NOT reference
  a function that doesn't exist. Read the actual behavior_toggles.rs first.

- [x] Add a failing test in `crates/sid/src/wire.rs` that asserts `SidApp.animation` is
  updated after a settings save event sequence:

  ```rust
  /// After the user presses S in AnimationView (simulated here by injecting an
  /// AnimationChanged outcome directly), `SidApp.animation` must reflect the
  /// new config on the next event loop iteration.
  ///
  /// Before the fix: `apply_pending_settings_outcomes` ignores `AnimationChanged`
  /// so `SidApp.animation` stays at the startup value forever.
  #[tokio::test]
  async fn animation_config_propagates_after_settings_save() {
      use ratatui::backend::TestBackend;
      use tokio::sync::mpsc;

      let backend = TestBackend::new(80, 24);
      let mut terminal = ratatui::Terminal::new(backend).unwrap();

      let mut sid_app = build_test_sid_app(Some("settings"));
      // Confirm default animation is enabled.
      assert!(
          sid_app.animation.enabled,
          "precondition: animation enabled at startup"
      );

      // Build a new config with animation disabled.
      let new_cfg = sid_core::animation::AnimationConfig {
          enabled: false,
          ..sid_core::animation::AnimationConfig::default()
      };

      // Inject the outcome directly into the settings widget's pending queue,
      // bypassing the UI — this isolates the wire-layer drain logic.
      // (Access the settings widget via the same path `apply_pending_settings_outcomes` uses.)
      {
          use sid_core::layout::Layout;
          let tabs = sid_app.app.tabs_mut().tabs_mut();
          let settings_tab = tabs
              .iter_mut()
              .find(|t| t.id.as_str() == "settings")
              .expect("settings tab must be present in test app");
          let Layout::Single(w) = &mut settings_tab.layout else {
              panic!("settings tab must have Single layout");
          };
          let settings_widget = w
              .as_any_mut()
              .downcast_mut::<sid_widgets::SettingsWidget>()
              .expect("must downcast to SettingsWidget");
          settings_widget.push_pending_outcome(
              sid_widgets::settings::PendingSettingsOutcome::AnimationChanged(new_cfg.clone()),
          );
      }

      // Send one Tick so the loop runs once and drains the outcome.
      let (tx, mut rx) = mpsc::channel::<sid_core::event::Event>(4);
      tx.send(sid_core::event::Event::Tick).await.unwrap();
      drop(tx);

      run_event_loop(&mut terminal, &mut sid_app, &mut rx)
          .await
          .unwrap();

      // After the fix: SidApp.animation reflects the new config.
      assert_eq!(
          sid_app.animation.enabled, false,
          "SidApp.animation.enabled must be false after AnimationChanged drain"
      );
      // fx_state must be toggled to None because enabled=false.
      assert!(
          sid_app.fx_state.is_none(),
          "fx_state must be None when animation.enabled == false"
      );
  }
  ```

- [x] This test requires `SettingsWidget::push_pending_outcome` to be a public method. Add it
  to `crates/sid-widgets/src/settings.rs`:
  ```rust
  /// Inject an outcome directly into the pending queue. Used by tests to
  /// bypass the full UI event sequence when verifying the wire-layer drain.
  #[cfg(test)]
  pub fn push_pending_outcome(&mut self, outcome: PendingSettingsOutcome) {
      self.pending_outcomes.push(outcome);
  }
  ```

- [x] Run the test to confirm it **fails** before the fix in Task 5:
  ```
  cargo test -p sid animation_config_propagates_after_settings_save 2>&1 | tail -20
  ```
  Expected: fails because `apply_pending_settings_outcomes` does not handle `AnimationChanged`
  and `SidApp.animation.enabled` remains `true`.

---

### Task 5: Fix — drain `AnimationChanged` in `apply_pending_settings_outcomes`

**Files:**
- Modify: `crates/sid/src/wire.rs` — `apply_pending_settings_outcomes` function (~line 1716).

- [x] In `apply_pending_settings_outcomes`, add a match arm for the new
  `PendingSettingsOutcome::AnimationChanged` variant. The arm must:
  1. Replace `sid_app.animation` with the new config.
  2. Toggle `sid_app.fx_state`:
     - If `new_cfg.enabled && sid_app.fx_state.is_none()` → `sid_app.fx_state = Some(FxState::new())`
     - If `!new_cfg.enabled && sid_app.fx_state.is_some()` → `sid_app.fx_state = None`

  Current code (partial, at the end of the `for outcome in outcomes` loop):
  ```rust
  for outcome in outcomes {
      let PendingSettingsOutcome::BehaviorToggled { key, value } = outcome;
      // ... put_bool / put_u64 / put_string ...
  }
  ```

  Replace with a proper `match`:
  ```rust
  for outcome in outcomes {
      match outcome {
          PendingSettingsOutcome::BehaviorToggled { key, value } => {
              let put_result = match &value {
                  ToggleValue::Bool(b) => sid_app.store.put_bool(key, *b),
                  ToggleValue::Choice { options, selected } => {
                      let picked = options.get(*selected).cloned().unwrap_or_default();
                      sid_app.store.put_string(key, &picked)
                  }
                  ToggleValue::U64 { value, .. } => sid_app.store.put_u64(key, *value),
                  ToggleValue::String(s) => sid_app.store.put_string(key, s),
              };
              match put_result {
                  Ok(()) => {
                      sid_app.toasts.push(Toast::success(format!("Saved {key}")));
                  }
                  Err(e) => {
                      sid_app
                          .toasts
                          .push(Toast::error(format!("Save failed for {key}: {e}")));
                  }
              }
          }
          PendingSettingsOutcome::AnimationChanged(new_cfg) => {
              // Toggle fx_state to match the new enabled flag before
              // replacing animation so the comparison is against the
              // *old* value.
              if new_cfg.enabled && sid_app.fx_state.is_none() {
                  sid_app.fx_state = Some(sid_fx::FxState::new());
              } else if !new_cfg.enabled && sid_app.fx_state.is_some() {
                  sid_app.fx_state = None;
              }
              sid_app.animation = new_cfg;
              sid_app
                  .toasts
                  .push(Toast::success("Animation settings applied".to_string()));
          }
      }
  }
  ```

- [x] Run the regression test from Task 4 to confirm it now **passes**:
  ```
  cargo test -p sid animation_config_propagates_after_settings_save 2>&1 | tail -10
  ```
  Expected: `ok`.

- [x] Run the full `sid` test suite:
  ```
  cargo test -p sid 2>&1 | tail -20
  ```
  Expected: all tests pass.

- [x] Run clippy on `sid` and `sid-widgets`:
  ```
  cargo clippy -p sid -p sid-widgets --all-targets -- -D warnings 2>&1 | tail -20
  ```
  Expected: no warnings.

- [x] Commit:
  ```
  fix(sid,sid-widgets): propagate AnimationConfig live to SidApp after Settings S-save
  ```

---

### Task 6: Insta snapshot — two consecutive Tick events produce distinct buffers

This snapshot test proves the animation is visually alive: two frames separated by one
`SidEvent::Tick` must produce different buffer contents. Without this test, future refactors
can re-introduce a dead animation silently.

**Files:**
- Create: `crates/sid/tests/snapshots/` (directory, if it doesn't exist).
- Modify: `crates/sid/src/wire.rs` — add `animation_frames_differ` test.

- [x] Add the following test in the `#[cfg(test)] mod tests` block of
  `crates/sid/src/wire.rs`:

  ```rust
  /// Two consecutive frames separated by exactly one `SidEvent::Tick` must
  /// produce visually different buffer contents in at least one cell.
  ///
  /// This guards against "dead" animations where `tick_count` advances but
  /// the rendered output never changes. The test uses a seeded `FxState` so
  /// the star positions are deterministic; after one tick the `phase` of at
  /// least one star changes, which changes the rendered colour or glyph.
  ///
  /// Snapshot the SECOND frame with insta so the expected appearance is
  /// locked. Re-accept with `cargo insta review` only after deliberate
  /// visual changes to the animation layer.
  #[tokio::test]
  async fn animation_frames_differ_after_tick() {
      use ratatui::backend::TestBackend;
      use tokio::sync::mpsc;

      // Use a large-enough terminal so stars are guaranteed to exist.
      let backend = TestBackend::new(80, 24);
      let mut terminal = ratatui::Terminal::new(backend).unwrap();

      let mut sid_app = build_test_sid_app(Some("workspaces"));
      // Seed the FxState so positions are deterministic and we can assert
      // frame equality / difference reliably.
      let cfg = sid_core::animation::AnimationConfig {
          enabled: true,
          density: 30,
          fps: 8,
          ..sid_core::animation::AnimationConfig::default()
      };
      sid_app.animation = cfg.clone();
      sid_app.fx_state = Some(sid_fx::FxState::with_seed(42));

      // Draw the first frame by sending one Tick.
      let (tx, mut rx) = mpsc::channel::<sid_core::event::Event>(8);
      tx.send(sid_core::event::Event::Tick).await.unwrap();
      // Keep channel open so the loop waits for more events.
      // After the first tick+draw the loop blocks on recv — we must send
      // a second event to observe the second frame.
      // Strategy: send Tick → loop draws frame1 and waits → send Tick →
      // loop draws frame2 and waits → drop tx → loop exits.
      tx.send(sid_core::event::Event::Tick).await.unwrap();
      drop(tx);

      run_event_loop(&mut terminal, &mut sid_app, &mut rx)
          .await
          .unwrap();

      // After two Tick events, tick_count must be 2.
      let tc = sid_app
          .fx_state
          .as_ref()
          .expect("fx_state present")
          .tick_count;
      assert_eq!(tc, 2, "exactly two ticks must have fired");

      // Snapshot the terminal buffer after the second frame so the rendered
      // star positions and styles are locked. The buffer content is a
      // string of rows joined by '\n'; insta compares it textually.
      let buf_string: String = {
          let buf = terminal.backend().buffer();
          (0..buf.area.height)
              .map(|y| {
                  (0..buf.area.width)
                      .map(|x| buf[(x, y)].symbol().to_string())
                      .collect::<String>()
              })
              .collect::<Vec<_>>()
              .join("\n")
      };

      // On first run, insta creates the snapshot file.
      // On subsequent runs it must match — if animation rendering changes,
      // this snapshot fails and you must re-accept with `cargo insta review`.
      insta::assert_snapshot!("animation_two_ticks_80x24_seed42", buf_string);
  }
  ```

- [x] Run the test once to generate the snapshot file:
  ```
  cargo test -p sid animation_frames_differ_after_tick 2>&1 | tail -15
  ```
  If insta creates the snapshot for the first time: `cargo insta review` to accept it, or
  set `INSTA_UPDATE=always` to auto-accept on first run:
  ```
  INSTA_UPDATE=always cargo test -p sid animation_frames_differ_after_tick 2>&1 | tail -10
  ```

- [x] Run the test again to confirm it passes (snapshot now exists):
  ```
  cargo test -p sid animation_frames_differ_after_tick 2>&1 | tail -10
  ```
  Expected: `ok`.

- [x] Commit (include the generated `.snap` file):
  ```
  test(sid): snapshot animation output after 2 ticks — guards against dead-animation regression
  ```

---

### Task 7: Wire `AnimationChanged` emission from `AnimationView`

This task closes the loop so that the `S`-key path in the real UI actually emits
`AnimationChanged` into the settings widget's pending queue, triggering Task 5's drain.

**Files:**
- Modify: `crates/sid-widgets/src/settings/animation.rs` — `handle_event`, `try_save`.
- Modify: `crates/sid-widgets/src/settings.rs` — `handle_event` or equivalent dispatch point
  where sub-view outcomes are converted to `PendingSettingsOutcome`.

- [x] Before making any changes, read `crates/sid-widgets/src/settings/behavior_toggles.rs`
  in full to understand how `BehaviorTogglesView` enqueues a `PendingSettingsOutcome` into
  the parent `SettingsWidget`. Identify the exact calling convention: does it return an
  outcome type, does it push to a shared queue, or does it emit via `WidgetCtx`?

- [x] Follow the same pattern for `AnimationView`. The goal: when `try_save` returns
  `Some(cfg)`, the `AnimationView::handle_event` caller (the `SettingsWidget`) pushes
  `PendingSettingsOutcome::AnimationChanged(cfg)` into its `pending_outcomes` queue.

  Implementation depends on what `behavior_toggles.rs` reveals. Do NOT invent an API;
  mirror the existing pattern exactly.

- [x] Add a unit test in `crates/sid-widgets/tests/settings_animation.rs` (new file)
  verifying that after `handle_event(S)` on a bound `AnimationView`, the parent
  `SettingsWidget`'s `pending_outcomes` contains one `AnimationChanged` entry:

  ```rust
  use std::sync::Arc;
  use sid_core::animation::AnimationConfig;
  use sid_core::event::{Event, KeyChord};
  use sid_store::{OpenStore, RedbStore};
  use sid_widgets::settings::PendingSettingsOutcome;
  use tempfile::tempdir;

  #[test]
  fn s_key_emits_animation_changed_outcome() {
      let d = tempdir().unwrap();
      let store: Arc<dyn sid_store::Store> =
          Arc::new(RedbStore::open(&d.path().join("anim_outcome.redb")).unwrap());

      // Build a SettingsWidget that includes the AnimationView bound to the store.
      // (Use the production builder, not a test shortcut, so the real dispatch
      // path is exercised.)
      let cfg = AnimationConfig { density: 10, ..AnimationConfig::default() };
      let mut settings = sid_widgets::SettingsWidget::new_with_animation(cfg, Arc::clone(&store));

      // Navigate to the Animation sub-view.
      // (Inspect SettingsWidget to find how to select the Animation category —
      // likely by emitting the right key sequence or calling a test helper.)
      // ... navigate to animation sub-view ...

      // Press S.
      let (tx, _rx) = std::sync::mpsc::channel();
      let mut ctx = sid_core::context::WidgetCtx::new(tx);
      let s_key = Event::Key(KeyChord::new(
          crossterm::event::KeyCode::Char('S'),
          crossterm::event::KeyModifiers::NONE,
      ));
      settings.handle_event(&s_key, &mut ctx);

      // The pending queue must contain exactly one AnimationChanged outcome.
      let outcomes = settings.take_pending_outcomes();
      assert_eq!(outcomes.len(), 1, "expected one pending outcome after S");
      assert!(
          matches!(outcomes[0], PendingSettingsOutcome::AnimationChanged(_)),
          "outcome must be AnimationChanged, got {:?}", outcomes[0]
      );
  }
  ```

  Adapt the navigation step once you've read the actual `SettingsWidget` API.

- [x] Run:
  ```
  cargo test -p sid-widgets s_key_emits_animation_changed_outcome 2>&1 | tail -10
  ```
  Expected: `ok`.

- [x] Run clippy on `sid-widgets`:
  ```
  cargo clippy -p sid-widgets --all-targets -- -D warnings 2>&1 | tail -10
  ```

- [x] Commit:
  ```
  feat(sid-widgets): AnimationView S-key emits AnimationChanged outcome for live-apply
  ```

---

### Task 8: Final integration check

- [x] Run the targeted test suite covering all changed crates:
  ```
  cargo test -p sid -p sid-widgets -p sid-fx 2>&1 | tail -30
  ```
  Expected: all tests pass.

- [x] Run clippy across all affected crates:
  ```
  cargo clippy -p sid -p sid-widgets -p sid-fx --all-targets -- -D warnings 2>&1 | tail -20
  ```
  Expected: zero warnings.

- [x] Confirm fmt:
  ```
  cargo fmt -p sid -p sid-widgets -p sid-fx -- --check 2>&1 | tail -10
  ```
  Expected: no diff.
