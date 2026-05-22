//! Animation sub-view for the Settings tab.
//!
//! Mutates a local copy of [`AnimationConfig`]; [`AnimationView::flush_dirty`]
//! writes it to the store as JSON under [`SETTING_ANIMATION_KEY`]. The binary's
//! render loop reads the persisted value at startup via
//! `wire::load_animation_config` and re-applies it.
//!
//! Six fields are exposed, one per row:
//!
//! - `Enabled`            — master on/off toggle.
//! - `Density`            — stars per 80x24 cells (`0..=100`, step 5).
//! - `Fps`                — animation frame rate (`1..=30`, step 1).
//! - `SupernovaIdleSecs`  — idle interval between spontaneous supernovae
//!   (`0..=3600`, step 15).
//! - `SupernovaOnEvent`   — whether widget events trigger celebratory bursts.
//! - `GlyphSet`           — palette cycled across `Cosmos -> Minimal -> Ascii`.
//!
//! Two construction shapes:
//!
//! - [`AnimationView::new`] — no embedded store; callers drive
//!   [`AnimationView::flush_dirty`] themselves. Used by tests.
//! - [`AnimationView::with_store`] — bound to an `Arc<dyn Store>`; the
//!   [`AnimationView::handle_event`] handler can persist the working
//!   config when the user presses `S` (or `Ctrl+S`).

use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use sid_core::SidError;
use sid_core::animation::{AnimationConfig, GlyphSet, SETTING_ANIMATION_KEY};
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::EventOutcome;
use sid_store::{SettingValue, Store};
use sid_ui::Theme;

/// Number of editable rows in the Animation sub-view.
const FIELD_COUNT: usize = 6;

/// Field positions in the rendered list — also the wrapping range for
/// [`AnimationView::focus_next`] / [`AnimationView::focus_prev`].
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::animation::AnimationField;
/// // The enum is non-exhaustive only in spirit — six concrete fields.
/// let _ = AnimationField::Enabled;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationField {
    /// Master on/off toggle for the background renderer.
    Enabled,
    /// Star density, clamped to `0..=100`.
    Density,
    /// Animation frame rate, clamped to `1..=30`.
    Fps,
    /// Seconds between idle supernovae, clamped to `0..=3600`.
    SupernovaIdleSecs,
    /// Whether widget events trigger supernovae.
    SupernovaOnEvent,
    /// Glyph palette cycled across [`GlyphSet`] variants.
    GlyphSet,
}

impl AnimationField {
    fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Enabled,
            1 => Self::Density,
            2 => Self::Fps,
            3 => Self::SupernovaIdleSecs,
            4 => Self::SupernovaOnEvent,
            _ => Self::GlyphSet,
        }
    }
}

/// Animation sub-view state.
///
/// Holds a working copy of [`AnimationConfig`] and a focus cursor. Mutations
/// flip the `dirty` flag; [`AnimationView::flush_dirty`] serialises the config
/// as JSON and persists it under [`SETTING_ANIMATION_KEY`].
///
/// `Arc<dyn Store>` does not implement `Debug`, so [`AnimationView`] has a
/// manual `Debug` impl that prints `<store bound>` / `<no store>` in place
/// of the handle.
///
/// # Examples
///
/// ```
/// use sid_core::animation::AnimationConfig;
/// use sid_widgets::settings::animation::AnimationView;
///
/// let v = AnimationView::new(AnimationConfig::default());
/// assert!(!v.is_dirty());
/// assert_eq!(v.config(), &AnimationConfig::default());
/// ```
pub struct AnimationView {
    cfg: AnimationConfig,
    focus: usize,
    dirty: bool,
    /// Optional store handle the view writes through when the user presses
    /// `S`. `None` for legacy / test callers that drive [`Self::flush_dirty`]
    /// directly; the binary's wire path constructs the view via
    /// [`AnimationView::with_store`] so the `S` route works end-to-end.
    store: Option<Arc<dyn Store>>,
}

impl std::fmt::Debug for AnimationView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnimationView")
            .field("cfg", &self.cfg)
            .field("focus", &self.focus)
            .field("dirty", &self.dirty)
            .field(
                "store",
                if self.store.is_some() {
                    &"<store bound>"
                } else {
                    &"<no store>"
                },
            )
            .finish()
    }
}

impl AnimationView {
    /// Build a view around `cfg` without a backing store. Focus starts at
    /// `Enabled`; dirty starts `false`. Use this for tests / scenarios that
    /// drive [`Self::flush_dirty`] directly; the production binary uses
    /// [`Self::with_store`] so `S` can persist via the embedded handle.
    pub fn new(cfg: AnimationConfig) -> Self {
        Self {
            cfg,
            focus: 0,
            dirty: false,
            store: None,
        }
    }

    /// Build a view bound to `store`. Pressing `S` (uppercase) or `Ctrl+S`
    /// via [`Self::handle_event`] persists the working config through the
    /// embedded handle without the caller needing access to a store.
    pub fn with_store(cfg: AnimationConfig, store: Arc<dyn Store>) -> Self {
        Self {
            cfg,
            focus: 0,
            dirty: false,
            store: Some(store),
        }
    }

    /// True if this view holds a backing store reference. Used by tests +
    /// the wire layer to assert which constructor was selected.
    pub fn has_store(&self) -> bool {
        self.store.is_some()
    }

    /// Borrow the working [`AnimationConfig`].
    pub fn config(&self) -> &AnimationConfig {
        &self.cfg
    }

    /// Currently focused field.
    pub fn focused_field(&self) -> AnimationField {
        AnimationField::from_index(self.focus)
    }

    /// Move focus down by one row (wraps).
    pub fn focus_next(&mut self) {
        self.focus = (self.focus + 1) % FIELD_COUNT;
    }

    /// Move focus up by one row (wraps).
    pub fn focus_prev(&mut self) {
        self.focus = if self.focus == 0 {
            FIELD_COUNT - 1
        } else {
            self.focus - 1
        };
    }

    /// Adjust the focused field.
    ///
    /// - `dir > 0` increases the value or advances to the next option.
    /// - `dir < 0` decreases the value or moves to the previous option.
    /// - `dir == 0` toggles booleans (no-op on non-booleans? See below).
    ///
    /// Field-specific semantics:
    ///
    /// - `Enabled`           — toggles on any `dir`.
    /// - `Density`           — `+/- 5`, clamped to `0..=100`.
    /// - `Fps`               — `+/- 1`, clamped to `1..=30`.
    /// - `SupernovaIdleSecs` — `+/- 15`, clamped to `0..=3600`.
    /// - `SupernovaOnEvent`  — toggles on any `dir`.
    /// - `GlyphSet`          — cycles `Cosmos -> Minimal -> Ascii -> Cosmos`
    ///   (or in reverse for `dir < 0`). On `dir == 0` cycles forward.
    ///
    /// Any actual change flips the dirty flag.
    pub fn adjust_focused(&mut self, dir: i32) {
        let field = self.focused_field();
        let before = self.cfg.clone();
        match field {
            AnimationField::Enabled => {
                self.cfg.enabled = !self.cfg.enabled;
            }
            AnimationField::Density => {
                self.cfg.density = step_u8(self.cfg.density, dir, 5, 0, 100);
            }
            AnimationField::Fps => {
                self.cfg.fps = step_u8(self.cfg.fps, dir, 1, 1, 30);
            }
            AnimationField::SupernovaIdleSecs => {
                self.cfg.supernova_idle_secs =
                    step_u32(self.cfg.supernova_idle_secs, dir, 15, 0, 3600);
            }
            AnimationField::SupernovaOnEvent => {
                self.cfg.supernova_on_event = !self.cfg.supernova_on_event;
            }
            AnimationField::GlyphSet => {
                self.cfg.glyph_set = cycle_glyph_set(self.cfg.glyph_set, dir);
            }
        }
        if self.cfg != before {
            self.dirty = true;
        }
    }

    /// `true` if any field has been changed since last [`Self::flush_dirty`]
    /// or [`Self::load_from_store`].
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Serialise the working config as JSON and persist it under
    /// [`SETTING_ANIMATION_KEY`]. Clears the dirty flag on success.
    pub fn flush_dirty(&mut self, store: &dyn Store) -> Result<(), SidError> {
        let bytes = serde_json::to_vec(&self.cfg)
            .map_err(|e| SidError::Storage(format!("animation serialize: {e}")))?;
        store.put_setting(SETTING_ANIMATION_KEY, &SettingValue(bytes))?;
        self.dirty = false;
        Ok(())
    }

    /// Load the persisted [`AnimationConfig`] from `store`, replacing the
    /// working copy. If the key is absent or the stored bytes fail to
    /// deserialise, the working copy is left unchanged. Clears the dirty
    /// flag.
    pub fn load_from_store(&mut self, store: &dyn Store) -> Result<(), SidError> {
        if let Some(v) = store.get_setting(SETTING_ANIMATION_KEY)?
            && let Ok(cfg) = serde_json::from_slice::<AnimationConfig>(&v.0)
        {
            self.cfg = cfg;
        }
        self.dirty = false;
        Ok(())
    }

    /// Flush the working config through the embedded store, if any. Returns
    /// `Ok(true)` when a write was attempted and succeeded, `Ok(false)` when
    /// no store is wired (the view was built via [`Self::new`]), and any
    /// underlying [`SidError`] from [`Self::flush_dirty`].
    ///
    /// This is the path the `S`-key handler takes; it lets the binary
    /// surface a "saved" toast on `Ok(true)` and a warning on `Ok(false)`
    /// without needing to thread a store reference through the widget tree.
    pub fn flush_via_embedded_store(&mut self) -> Result<bool, SidError> {
        if let Some(store) = self.store.clone() {
            self.flush_dirty(&*store)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Route a single [`Event`] into this view. Translates key chords into
    /// the existing mutator methods and, on `S` (uppercase) or `Ctrl+S`,
    /// persists the working config via [`Self::flush_via_embedded_store`].
    ///
    /// Returns:
    /// - [`EventOutcome::Consumed`] for any handled key (including failed
    ///   saves — the error is logged via `eprintln!` so the event isn't
    ///   silently re-dispatched).
    /// - [`EventOutcome::Bubble`] for events the view doesn't recognise.
    ///
    /// `_ctx` is accepted for symmetry with the
    /// [`sid_core::widget::Widget::handle_event`] signature so the composer
    /// can forward without reshaping arguments; the view does not use it
    /// today.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::sync::mpsc;
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::animation::AnimationConfig;
    /// use sid_core::context::WidgetCtx;
    /// use sid_core::event::{Event, KeyChord};
    /// use sid_core::widget::EventOutcome;
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use sid_widgets::settings::animation::AnimationView;
    /// use tempfile::tempdir;
    ///
    /// let d = tempdir().unwrap();
    /// let store: Arc<dyn Store> =
    ///     Arc::new(RedbStore::open(&d.path().join("anim.redb")).unwrap());
    /// let mut v = AnimationView::with_store(AnimationConfig::default(), store);
    /// let (tx, _rx) = mpsc::channel();
    /// let mut ctx = WidgetCtx::new(tx);
    /// let ev = Event::Key(KeyChord::new(KeyCode::Down, KeyModifiers::NONE));
    /// assert_eq!(v.handle_event(&ev, &mut ctx), EventOutcome::Consumed);
    /// ```
    pub fn handle_event(&mut self, ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        let Event::Key(k) = ev else {
            return EventOutcome::Bubble;
        };
        match (k.code, k.mods) {
            // Uppercase `S` (any non-Ctrl modifiers) or `Ctrl+S` — persist
            // via the embedded store.
            (KeyCode::Char('S'), m) if !m.contains(KeyModifiers::CONTROL) => {
                self.try_save();
                EventOutcome::Consumed
            }
            (KeyCode::Char('s'), m) if m.contains(KeyModifiers::CONTROL) => {
                self.try_save();
                EventOutcome::Consumed
            }
            (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                self.focus_next();
                EventOutcome::Consumed
            }
            (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                self.focus_prev();
                EventOutcome::Consumed
            }
            (KeyCode::Right, _) | (KeyCode::Char('l'), KeyModifiers::NONE) => {
                self.adjust_focused(1);
                EventOutcome::Consumed
            }
            (KeyCode::Left, _) | (KeyCode::Char('h'), KeyModifiers::NONE) => {
                self.adjust_focused(-1);
                EventOutcome::Consumed
            }
            (KeyCode::Char(' ') | KeyCode::Enter, _) => {
                self.adjust_focused(0);
                EventOutcome::Consumed
            }
            _ => EventOutcome::Bubble,
        }
    }

    /// Save helper used by the `S` handler. Logs (but does not panic) when
    /// no store is bound or the write fails — the key event has been
    /// consumed and there's nowhere good to surface a `Result` from
    /// [`Self::handle_event`].
    ///
    /// `sid-widgets` does not depend on `tracing`, so we route via stderr.
    /// The TUI captures stderr through its parent tracing layer when
    /// running under the production binary.
    fn try_save(&mut self) {
        match self.flush_via_embedded_store() {
            Ok(true) => {}
            Ok(false) => {
                eprintln!(
                    "AnimationView: S pressed but no store bound; \
                     use AnimationView::with_store(...) to enable saving"
                );
            }
            Err(e) => {
                eprintln!("AnimationView: flush_dirty failed: {e}");
            }
        }
    }

    /// Render the Animation sub-view into `area`.
    ///
    /// The view owns its outer bordered block; `focused` controls the border
    /// color (accent vs muted) and the title-bar bold modifier so the Settings
    /// composer can signal which pane currently owns keyboard input without
    /// overlaying its own block on top.
    pub fn render_into_frame(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        theme: &Theme,
        focused: bool,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let mut title_style = Style::default().fg(theme.foreground.into());
        if focused {
            title_style = title_style.add_modifier(Modifier::BOLD);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(" Animation ")
            .title_style(title_style);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let rows: Vec<Line> = self.build_lines(theme);
        let para = Paragraph::new(rows).style(Style::default().fg(theme.foreground.into()));
        frame.render_widget(para, inner);
    }

    fn build_lines(&self, theme: &Theme) -> Vec<Line<'static>> {
        let mut rows: Vec<Line<'static>> = Vec::with_capacity(FIELD_COUNT + 4);
        // The outer Block already renders " Animation " in its title bar.
        // The body just lists fields — no duplicate heading.
        let _ = theme;
        let field_rows = [
            (
                AnimationField::Enabled,
                "Enabled",
                bool_value(self.cfg.enabled),
            ),
            (
                AnimationField::Density,
                "Density",
                format!("{} / 100", self.cfg.density),
            ),
            (AnimationField::Fps, "FPS", format!("{} / 30", self.cfg.fps)),
            (
                AnimationField::SupernovaIdleSecs,
                "Supernova idle (secs)",
                self.cfg.supernova_idle_secs.to_string(),
            ),
            (
                AnimationField::SupernovaOnEvent,
                "Supernova on event",
                bool_value(self.cfg.supernova_on_event),
            ),
            (
                AnimationField::GlyphSet,
                "Glyph set",
                glyph_label(self.cfg.glyph_set).to_string(),
            ),
        ];

        for (i, (_, label, value)) in field_rows.iter().enumerate() {
            let focused = i == self.focus;
            let cursor = if focused { '>' } else { ' ' };
            let marker = if focused { '*' } else { 'o' };
            let text = format!("{cursor} {marker} {label:<24} {value}");
            let line = Line::from(text);
            let line = if focused {
                line.style(
                    Style::default()
                        .fg(theme.accent_primary.into())
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                line.style(Style::default().fg(theme.foreground.into()))
            };
            rows.push(line);
        }

        rows.push(Line::from(String::new()));
        rows.push(
            Line::from("[ Space/Enter: toggle/cycle ]".to_string())
                .style(Style::default().fg(theme.muted.into())),
        );
        rows.push(
            Line::from("[ Left/Right: decrement/increment ]".to_string())
                .style(Style::default().fg(theme.muted.into())),
        );
        rows.push(
            Line::from("[ S: save ]".to_string()).style(Style::default().fg(theme.muted.into())),
        );

        rows
    }
}

fn bool_value(b: bool) -> String {
    if b {
        "[on]".to_string()
    } else {
        "[off]".to_string()
    }
}

fn glyph_label(g: GlyphSet) -> &'static str {
    match g {
        GlyphSet::Cosmos => "Cosmos",
        GlyphSet::Minimal => "Minimal",
        GlyphSet::Ascii => "Ascii",
    }
}

fn cycle_glyph_set(current: GlyphSet, dir: i32) -> GlyphSet {
    let order = [GlyphSet::Cosmos, GlyphSet::Minimal, GlyphSet::Ascii];
    let idx = order.iter().position(|g| *g == current).unwrap_or(0) as i32;
    let len = order.len() as i32;
    let step = if dir < 0 { -1 } else { 1 };
    let new = (idx + step).rem_euclid(len) as usize;
    order[new]
}

fn step_u8(current: u8, dir: i32, step: u8, min: u8, max: u8) -> u8 {
    let delta: i32 = if dir < 0 {
        -(step as i32)
    } else if dir > 0 {
        step as i32
    } else {
        // dir == 0 on a numeric field is a no-op (booleans handle their own
        // case at the call site).
        0
    };
    let raw = (current as i32).saturating_add(delta);
    let clamped = raw.clamp(min as i32, max as i32);
    clamped as u8
}

fn step_u32(current: u32, dir: i32, step: u32, min: u32, max: u32) -> u32 {
    let delta: i64 = if dir < 0 {
        -(step as i64)
    } else if dir > 0 {
        step as i64
    } else {
        0
    };
    let raw = (current as i64).saturating_add(delta);
    let clamped = raw.clamp(min as i64, max as i64);
    clamped as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_u8_clamps_high() {
        assert_eq!(step_u8(98, 1, 5, 0, 100), 100);
    }

    #[test]
    fn step_u8_clamps_low() {
        assert_eq!(step_u8(2, -1, 5, 0, 100), 0);
    }

    #[test]
    fn step_u32_clamps_high() {
        assert_eq!(step_u32(3590, 1, 15, 0, 3600), 3600);
    }

    #[test]
    fn step_u32_clamps_low() {
        assert_eq!(step_u32(10, -1, 15, 0, 3600), 0);
    }

    #[test]
    fn cycle_glyph_forward_wraps() {
        assert_eq!(cycle_glyph_set(GlyphSet::Cosmos, 1), GlyphSet::Minimal);
        assert_eq!(cycle_glyph_set(GlyphSet::Minimal, 1), GlyphSet::Ascii);
        assert_eq!(cycle_glyph_set(GlyphSet::Ascii, 1), GlyphSet::Cosmos);
    }

    #[test]
    fn cycle_glyph_reverse_wraps() {
        assert_eq!(cycle_glyph_set(GlyphSet::Cosmos, -1), GlyphSet::Ascii);
        assert_eq!(cycle_glyph_set(GlyphSet::Ascii, -1), GlyphSet::Minimal);
    }

    #[test]
    fn focused_field_starts_at_enabled() {
        let v = AnimationView::new(AnimationConfig::default());
        assert_eq!(v.focused_field(), AnimationField::Enabled);
    }

    #[test]
    fn new_without_store_reports_no_store() {
        let v = AnimationView::new(AnimationConfig::default());
        assert!(!v.has_store());
    }

    #[test]
    fn with_store_reports_store_bound() {
        use sid_store::{OpenStore, RedbStore};
        use tempfile::tempdir;
        let d = tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(RedbStore::open(&d.path().join("anim.redb")).unwrap());
        let v = AnimationView::with_store(AnimationConfig::default(), store);
        assert!(v.has_store());
    }

    #[test]
    fn flush_via_embedded_store_no_store_returns_false() {
        let mut v = AnimationView::new(AnimationConfig::default());
        let out = v.flush_via_embedded_store().unwrap();
        assert!(!out, "flush with no embedded store must return Ok(false)");
    }

    #[test]
    fn flush_via_embedded_store_with_store_returns_true() {
        use sid_store::{OpenStore, RedbStore};
        use tempfile::tempdir;
        let d = tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(RedbStore::open(&d.path().join("anim.redb")).unwrap());
        let mut v = AnimationView::with_store(AnimationConfig::default(), Arc::clone(&store));
        let out = v.flush_via_embedded_store().unwrap();
        assert!(out, "flush with embedded store must return Ok(true)");
        // The setting key should now be present.
        let got = store.get_setting(SETTING_ANIMATION_KEY).unwrap();
        assert!(got.is_some(), "animation setting key was not written");
    }

    #[test]
    fn handle_event_uppercase_s_with_store_flushes() {
        use std::sync::mpsc;

        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::{Event, KeyChord};
        use sid_store::{OpenStore, RedbStore};
        use tempfile::tempdir;
        let d = tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(RedbStore::open(&d.path().join("anim.redb")).unwrap());
        let mut v = AnimationView::with_store(AnimationConfig::default(), Arc::clone(&store));
        // Mutate something first so dirty != false.
        v.focus_next(); // Density
        v.adjust_focused(1); // bump
        assert!(v.is_dirty());

        let (tx, _rx) = mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        let ev = Event::Key(KeyChord::new(KeyCode::Char('S'), KeyModifiers::NONE));
        let out = v.handle_event(&ev, &mut ctx);
        assert_eq!(out, EventOutcome::Consumed);
        assert!(!v.is_dirty(), "S press should clear the dirty flag");
        let got = store.get_setting(SETTING_ANIMATION_KEY).unwrap();
        assert!(got.is_some(), "S press should have written the setting key");
    }

    #[test]
    fn handle_event_ctrl_s_with_store_flushes() {
        use std::sync::mpsc;

        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::{Event, KeyChord};
        use sid_store::{OpenStore, RedbStore};
        use tempfile::tempdir;
        let d = tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(RedbStore::open(&d.path().join("anim.redb")).unwrap());
        let mut v = AnimationView::with_store(AnimationConfig::default(), Arc::clone(&store));
        let (tx, _rx) = mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        let ev = Event::Key(KeyChord::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        let out = v.handle_event(&ev, &mut ctx);
        assert_eq!(out, EventOutcome::Consumed);
        let got = store.get_setting(SETTING_ANIMATION_KEY).unwrap();
        assert!(got.is_some(), "Ctrl+S press should have written the key");
    }

    #[test]
    fn handle_event_j_focus_next() {
        use std::sync::mpsc;

        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::{Event, KeyChord};
        let mut v = AnimationView::new(AnimationConfig::default());
        let (tx, _rx) = mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        let ev = Event::Key(KeyChord::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(v.handle_event(&ev, &mut ctx), EventOutcome::Consumed);
        assert_eq!(v.focused_field(), AnimationField::Density);
    }

    #[test]
    fn handle_event_k_focus_prev() {
        use std::sync::mpsc;

        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::{Event, KeyChord};
        let mut v = AnimationView::new(AnimationConfig::default());
        // Move down once so we have somewhere to step back from.
        v.focus_next();
        let (tx, _rx) = mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        let ev = Event::Key(KeyChord::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(v.handle_event(&ev, &mut ctx), EventOutcome::Consumed);
        assert_eq!(v.focused_field(), AnimationField::Enabled);
    }

    #[test]
    fn handle_event_unknown_key_bubbles() {
        use std::sync::mpsc;

        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::{Event, KeyChord};
        let mut v = AnimationView::new(AnimationConfig::default());
        let (tx, _rx) = mpsc::channel();
        let mut ctx = WidgetCtx::new(tx);
        let ev = Event::Key(KeyChord::new(KeyCode::Char('z'), KeyModifiers::NONE));
        assert_eq!(v.handle_event(&ev, &mut ctx), EventOutcome::Bubble);
    }
}
