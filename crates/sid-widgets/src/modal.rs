//! Modal / dialog substrate for the sid TUI.
//!
//! A modal is a centered overlay containing a titled form with mixed field
//! types (text, password, toggle, choice, picker). The caller owns the
//! [`ModalSpec`] state and pumps key events into it via the small mutator
//! API exposed here. The binary's draw layer calls [`render_modal`] to paint
//! the modal on top of the existing frame.
//!
//! The substrate intentionally avoids the [`sid_core::widget::Widget`] trait:
//! a modal is short-lived form state, not a long-lived tab. Routing of key
//! events from `App` to the active modal is the binary's responsibility; this
//! module only provides the form model and the renderer.
//!
//! # Field types
//!
//! - [`Field::Text`] — free-form text with optional placeholder.
//! - [`Field::Password`] — same character pump as [`Field::Text`] but the
//!   renderer substitutes bullets so the value is never echoed.
//! - [`Field::Toggle`] — checkbox-style boolean.
//! - [`Field::Choice`] — radio-button list, one option selected at a time.
//! - [`Field::Picker`] — text input plus a `[browse]` hint, used for path
//!   pickers and other "type-or-browse" inputs.
//! - [`Field::Display`] — read-only multi-line text block. Each `\n`-separated
//!   line renders on its own row. Used for help drawers and other surfaces
//!   that need to show static prose inside the modal substrate.
//!
//! # Example
//!
//! ```
//! use sid_widgets::modal::{Field, ModalSpec};
//!
//! let mut m = ModalSpec::new(
//!     "ssh.add_host",
//!     "Add Host",
//!     vec![
//!         Field::Text { label: "alias".into(), value: String::new(), placeholder: None },
//!         Field::Password { label: "passphrase".into(), value: String::new() },
//!     ],
//! );
//! m.type_char('p');
//! m.type_char('i');
//! // First field is Text — characters land there.
//! match &m.fields[0] {
//!     Field::Text { value, .. } => assert_eq!(value, "pi"),
//!     _ => panic!("unexpected field type"),
//! }
//! ```

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use sid_ui::Theme;

/// Identity for a modal — used so the caller (a widget or the binary) can
/// recognise its own modal when the user submits.
///
/// # Examples
///
/// ```
/// use sid_widgets::modal::ModalId;
///
/// let id = ModalId("ssh.add_host".into());
/// assert_eq!(id.0, "ssh.add_host");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModalId(pub String);

/// What a single field in a modal form holds.
///
/// # Examples
///
/// ```
/// use sid_widgets::modal::Field;
///
/// let f = Field::Text {
///     label: "alias".into(),
///     value: "prod".into(),
///     placeholder: None,
/// };
/// match f {
///     Field::Text { value, .. } => assert_eq!(value, "prod"),
///     _ => unreachable!(),
/// }
/// ```
#[derive(Debug, Clone)]
pub enum Field {
    /// Free-form text input.
    Text {
        /// Field label rendered above the value row.
        label: String,
        /// Current value.
        value: String,
        /// Greyed-out hint shown when `value` is empty.
        placeholder: Option<String>,
    },
    /// Password field; renders bullets, never echoes characters.
    Password {
        /// Field label.
        label: String,
        /// Raw value; never rendered.
        value: String,
    },
    /// Toggle (checkbox).
    Toggle {
        /// Field label.
        label: String,
        /// Current boolean.
        value: bool,
    },
    /// Single choice from a list (rendered as `( ) opt ( ) opt (●) opt`).
    Choice {
        /// Field label.
        label: String,
        /// Available options. Must be non-empty for [`ModalSpec::space_or_enter_on_field`]
        /// to advance selection.
        options: Vec<String>,
        /// Index of the currently-selected option.
        selected: usize,
    },
    /// Path picker — text input + optional `[browse]` hint.
    Picker {
        /// Field label.
        label: String,
        /// Current path / value.
        value: String,
        /// Hint shown to the right of the value (e.g. `[browse ~/.ssh]`).
        hint: String,
    },
    /// Read-only multi-line text. Renders each `\n`-separated line on its
    /// own row inside the modal body. The renderer width-clips per line
    /// (no wrapping for now). All field mutators (`type_char`, `backspace`,
    /// `space_or_enter_on_field`) are no-ops on a `Display` field.
    ///
    /// Use this for help drawers, summary panels, and other "show me a
    /// block of text" surfaces that still benefit from the modal substrate.
    Display {
        /// Field label rendered above the body block.
        label: String,
        /// Multi-line text body. `\n` characters split the body into rows;
        /// the renderer prints each row on its own line.
        body: String,
    },
}

/// A whole modal form: title, ordered fields, primary/secondary button
/// labels, focus state, and an optional help hint line.
///
/// # Examples
///
/// ```
/// use sid_widgets::modal::{Field, ModalSpec};
///
/// let m = ModalSpec::new(
///     "demo",
///     "Demo",
///     vec![Field::Text {
///         label: "name".into(),
///         value: String::new(),
///         placeholder: None,
///     }],
/// );
/// assert_eq!(m.id.0, "demo");
/// assert_eq!(m.title, "Demo");
/// assert_eq!(m.primary_label, "Save");
/// assert_eq!(m.secondary_label, "Cancel");
/// assert_eq!(m.focus, 0);
/// ```
#[derive(Debug, Clone)]
pub struct ModalSpec {
    /// Stable identifier so the submit handler can dispatch on it.
    pub id: ModalId,
    /// Title rendered in the modal border.
    pub title: String,
    /// Ordered list of fields.
    pub fields: Vec<Field>,
    /// Label for the primary action (usually "Save").
    pub primary_label: String,
    /// Label for the secondary action (usually "Cancel").
    pub secondary_label: String,
    /// Optional dim hint rendered above the buttons.
    pub help_hint: Option<String>,
    /// Index of focused field; cycles with Tab / Shift+Tab.
    pub focus: usize,
}

impl ModalSpec {
    /// Build a modal with the standard "Save" / "Cancel" buttons and the
    /// focus on the first field.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::modal::{Field, ModalSpec};
    ///
    /// let m = ModalSpec::new("id", "title", vec![]);
    /// assert_eq!(m.title, "title");
    /// assert!(m.fields.is_empty());
    /// assert_eq!(m.focus, 0);
    /// ```
    pub fn new(id: impl Into<String>, title: impl Into<String>, fields: Vec<Field>) -> Self {
        Self {
            id: ModalId(id.into()),
            title: title.into(),
            fields,
            primary_label: "Save".into(),
            secondary_label: "Cancel".into(),
            help_hint: None,
            focus: 0,
        }
    }

    /// Attach a dim help hint that renders above the buttons.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::modal::ModalSpec;
    ///
    /// let m = ModalSpec::new("id", "title", vec![]).with_help("Tab to move");
    /// assert_eq!(m.help_hint.as_deref(), Some("Tab to move"));
    /// ```
    pub fn with_help(mut self, hint: impl Into<String>) -> Self {
        self.help_hint = Some(hint.into());
        self
    }

    /// Advance focus by one (Tab). Wraps to 0 after the last field. No-op
    /// when there are no fields.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::modal::{Field, ModalSpec};
    ///
    /// let mut m = ModalSpec::new(
    ///     "id",
    ///     "t",
    ///     vec![
    ///         Field::Toggle { label: "a".into(), value: false },
    ///         Field::Toggle { label: "b".into(), value: false },
    ///     ],
    /// );
    /// m.cycle_focus_forward();
    /// assert_eq!(m.focus, 1);
    /// m.cycle_focus_forward();
    /// assert_eq!(m.focus, 0);
    /// ```
    pub fn cycle_focus_forward(&mut self) {
        if !self.fields.is_empty() {
            self.focus = (self.focus + 1) % self.fields.len();
        }
    }

    /// Step focus backward by one (Shift+Tab). Wraps to the last field after
    /// the first. No-op when there are no fields.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::modal::{Field, ModalSpec};
    ///
    /// let mut m = ModalSpec::new(
    ///     "id",
    ///     "t",
    ///     vec![
    ///         Field::Toggle { label: "a".into(), value: false },
    ///         Field::Toggle { label: "b".into(), value: false },
    ///     ],
    /// );
    /// m.cycle_focus_backward();
    /// assert_eq!(m.focus, 1);
    /// m.cycle_focus_backward();
    /// assert_eq!(m.focus, 0);
    /// ```
    pub fn cycle_focus_backward(&mut self) {
        if !self.fields.is_empty() {
            self.focus = if self.focus == 0 {
                self.fields.len() - 1
            } else {
                self.focus - 1
            };
        }
    }

    /// Pump a character into the focused text/password/picker field. No-op
    /// for non-text fields and when the modal has no fields.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::modal::{Field, FieldValue, ModalSpec};
    ///
    /// let mut m = ModalSpec::new(
    ///     "id",
    ///     "t",
    ///     vec![Field::Text {
    ///         label: "name".into(),
    ///         value: String::new(),
    ///         placeholder: None,
    ///     }],
    /// );
    /// m.type_char('s');
    /// m.type_char('i');
    /// m.type_char('d');
    /// match &m.collect_values()[0].1 {
    ///     FieldValue::Text(v) => assert_eq!(v, "sid"),
    ///     _ => unreachable!(),
    /// }
    /// ```
    pub fn type_char(&mut self, c: char) {
        let Some(field) = self.fields.get_mut(self.focus) else {
            return;
        };
        match field {
            Field::Text { value, .. }
            | Field::Password { value, .. }
            | Field::Picker { value, .. } => {
                value.push(c);
            }
            Field::Toggle { .. } | Field::Choice { .. } | Field::Display { .. } => {}
        }
    }

    /// Pop the last character of the focused text/password/picker field.
    /// No-op for non-text fields, empty values, and empty modals.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::modal::{Field, FieldValue, ModalSpec};
    ///
    /// let mut m = ModalSpec::new(
    ///     "id",
    ///     "t",
    ///     vec![Field::Text {
    ///         label: "name".into(),
    ///         value: "sid".into(),
    ///         placeholder: None,
    ///     }],
    /// );
    /// m.backspace();
    /// match &m.collect_values()[0].1 {
    ///     FieldValue::Text(v) => assert_eq!(v, "si"),
    ///     _ => unreachable!(),
    /// }
    /// ```
    pub fn backspace(&mut self) {
        let Some(field) = self.fields.get_mut(self.focus) else {
            return;
        };
        match field {
            Field::Text { value, .. }
            | Field::Password { value, .. }
            | Field::Picker { value, .. } => {
                value.pop();
            }
            Field::Toggle { .. } | Field::Choice { .. } | Field::Display { .. } => {}
        }
    }

    /// Toggle the focused [`Field::Toggle`], or advance the focused
    /// [`Field::Choice`] to the next option. No-op for text-y fields and
    /// empty modals.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::modal::{Field, FieldValue, ModalSpec};
    ///
    /// let mut m = ModalSpec::new(
    ///     "id",
    ///     "t",
    ///     vec![Field::Toggle { label: "on".into(), value: false }],
    /// );
    /// m.space_or_enter_on_field();
    /// match &m.collect_values()[0].1 {
    ///     FieldValue::Toggle(b) => assert!(*b),
    ///     _ => unreachable!(),
    /// }
    /// ```
    pub fn space_or_enter_on_field(&mut self) {
        let Some(field) = self.fields.get_mut(self.focus) else {
            return;
        };
        match field {
            Field::Toggle { value, .. } => {
                *value = !*value;
            }
            Field::Choice {
                options, selected, ..
            } => {
                if !options.is_empty() {
                    *selected = (*selected + 1) % options.len();
                }
            }
            Field::Text { .. }
            | Field::Password { .. }
            | Field::Picker { .. }
            | Field::Display { .. } => {}
        }
    }

    /// Cycle the value of the focused [`Field::Choice`] or flip the focused
    /// [`Field::Toggle`]. `dir > 0` advances forward, `dir < 0` goes
    /// backward, `dir == 0` is a no-op. Non-value fields (Text / Password /
    /// Picker / Display) and an empty modal are also no-ops.
    ///
    /// Routed to from [`route_key_to_modal`] on Left/Right arrow keys.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::modal::{Field, ModalSpec};
    ///
    /// let mut m = ModalSpec::new("id", "t", vec![Field::Choice {
    ///     label: "k".into(),
    ///     options: vec!["a".into(), "b".into()],
    ///     selected: 0,
    /// }]);
    /// m.cycle_focused_value(1);
    /// if let Field::Choice { selected, .. } = &m.fields[0] {
    ///     assert_eq!(*selected, 1);
    /// }
    /// ```
    pub fn cycle_focused_value(&mut self, dir: i8) {
        if dir == 0 {
            return;
        }
        let Some(field) = self.fields.get_mut(self.focus) else {
            return;
        };
        match field {
            Field::Choice {
                options, selected, ..
            } => {
                if options.is_empty() {
                    return;
                }
                let n = options.len();
                let s = *selected;
                *selected = if dir > 0 {
                    (s + 1) % n
                } else {
                    (s + n - 1) % n
                };
            }
            Field::Toggle { value, .. } => {
                *value = !*value;
            }
            Field::Text { .. }
            | Field::Password { .. }
            | Field::Picker { .. }
            | Field::Display { .. } => {}
        }
    }

    /// Collect all field values into a `(label, FieldValue)` Vec for the
    /// submit handler. Order matches the field declaration order.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::modal::{Field, FieldValue, ModalSpec};
    ///
    /// let m = ModalSpec::new(
    ///     "id",
    ///     "t",
    ///     vec![
    ///         Field::Text { label: "name".into(), value: "sid".into(), placeholder: None },
    ///         Field::Toggle { label: "on".into(), value: true },
    ///     ],
    /// );
    /// let vs = m.collect_values();
    /// assert_eq!(vs.len(), 2);
    /// assert_eq!(vs[0].0, "name");
    /// assert!(matches!(&vs[0].1, FieldValue::Text(s) if s == "sid"));
    /// assert!(matches!(&vs[1].1, FieldValue::Toggle(true)));
    /// ```
    pub fn collect_values(&self) -> Vec<(String, FieldValue)> {
        self.fields
            .iter()
            .map(|f| match f {
                Field::Text { label, value, .. } => {
                    (label.clone(), FieldValue::Text(value.clone()))
                }
                Field::Password { label, value } => {
                    (label.clone(), FieldValue::Password(value.clone()))
                }
                Field::Toggle { label, value } => (label.clone(), FieldValue::Toggle(*value)),
                Field::Choice {
                    label,
                    options,
                    selected,
                } => {
                    let picked = options.get(*selected).cloned().unwrap_or_default();
                    (label.clone(), FieldValue::Choice(picked))
                }
                Field::Picker { label, value, .. } => {
                    (label.clone(), FieldValue::Picker(value.clone()))
                }
                Field::Display { label, body } => {
                    (label.clone(), FieldValue::Display(body.clone()))
                }
            })
            .collect()
    }
}

/// Submit-time projection of a single [`Field`]. The submit handler matches
/// on this rather than the source [`Field`] enum so it cannot, for example,
/// accidentally read a password back out of a text field's display value.
///
/// # Examples
///
/// ```
/// use sid_widgets::modal::FieldValue;
///
/// let v = FieldValue::Text("alice".into());
/// match v {
///     FieldValue::Text(s) => assert_eq!(s, "alice"),
///     _ => unreachable!(),
/// }
/// ```
#[derive(Debug, Clone)]
pub enum FieldValue {
    /// Snapshot of a [`Field::Text`] value.
    Text(String),
    /// Snapshot of a [`Field::Password`] value.
    Password(String),
    /// Snapshot of a [`Field::Toggle`] value.
    Toggle(bool),
    /// Selected option text from a [`Field::Choice`].
    Choice(String),
    /// Snapshot of a [`Field::Picker`] value.
    Picker(String),
    /// Snapshot of a [`Field::Display`] body. Carried separately from
    /// [`FieldValue::Text`] so submit handlers can distinguish read-only
    /// content from user-supplied text. Most submit handlers will simply
    /// ignore `Display` entries — they exist for completeness so
    /// [`ModalSpec::collect_values`] returns the full set of fields.
    Display(String),
}

/// Bullet character used to mask passwords. U+2022 BULLET.
const PASSWORD_BULLET: char = '\u{2022}';

/// Outcome of routing a single key event through an open modal.
///
/// The binary's event loop interprets:
/// - `Consumed`  — keep the modal open, redraw on next frame.
/// - `Submit`    — pop the modal, hand `collect_values()` to the submit handler.
/// - `Cancel`    — pop the modal without invoking any handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalKeyOutcome {
    Consumed,
    Submit,
    Cancel,
}

/// Route a single crossterm `KeyEvent` into `modal` and return what the caller
/// should do next.
///
/// - `Esc`                                → Cancel
/// - `Up` / `Shift+Tab` / `BackTab`       → cycle focus backward, Consumed
/// - `Down` / `Tab` (no Shift)            → cycle focus forward, Consumed
/// - `Left`                               → cycle focused Choice/Toggle backward, Consumed
/// - `Right`                              → cycle focused Choice/Toggle forward, Consumed
/// - `Backspace`                          → backspace on focused text/password/picker, Consumed
/// - `Enter`                              → Submit (always — buttons stay decorative)
/// - `Space` on Toggle / Choice           → flip/cycle the field, Consumed (legacy convenience)
/// - `Char(c)` (no Ctrl/Alt)              → type_char, Consumed
/// - any other key                        → Consumed (modal swallows it)
///
/// `Enter` no longer cycles a focused Choice — that's now Left/Right. This
/// makes Enter unambiguously "submit the form" so users can confirm without
/// accidentally changing the selection on the way out.
///
/// # Examples
///
/// ```
/// use crossterm::event::{KeyCode, KeyModifiers};
/// use sid_core::event::KeyChord;
/// use sid_widgets::modal::{Field, ModalKeyOutcome, ModalSpec, route_key_to_modal};
///
/// let mut m = ModalSpec::new("id", "t",
///     vec![Field::Text { label: "n".into(), value: String::new(), placeholder: None }]);
///
/// // Esc -> Cancel
/// let esc = KeyChord { code: KeyCode::Esc, mods: KeyModifiers::NONE };
/// assert_eq!(route_key_to_modal(&mut m, esc), ModalKeyOutcome::Cancel);
///
/// // Enter on a text field -> Submit
/// let enter = KeyChord { code: KeyCode::Enter, mods: KeyModifiers::NONE };
/// assert_eq!(route_key_to_modal(&mut m, enter), ModalKeyOutcome::Submit);
/// ```
pub fn route_key_to_modal(
    modal: &mut ModalSpec,
    key: sid_core::event::KeyChord,
) -> ModalKeyOutcome {
    use crossterm::event::{KeyCode, KeyModifiers};
    match (key.code, key.mods) {
        (KeyCode::Esc, _) => ModalKeyOutcome::Cancel,
        (KeyCode::Up, _) | (KeyCode::BackTab, _) => {
            modal.cycle_focus_backward();
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Down, _) => {
            modal.cycle_focus_forward();
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Tab, m) if !m.contains(KeyModifiers::SHIFT) => {
            modal.cycle_focus_forward();
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Tab, _) => {
            modal.cycle_focus_backward();
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Left, _) => {
            modal.cycle_focused_value(-1);
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Right, _) => {
            modal.cycle_focused_value(1);
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Backspace, _) => {
            modal.backspace();
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Enter, _) => ModalKeyOutcome::Submit,
        (KeyCode::Char(' '), _)
            if matches!(
                modal.fields.get(modal.focus),
                Some(Field::Toggle { .. } | Field::Choice { .. })
            ) =>
        {
            modal.space_or_enter_on_field();
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Char(c), m)
            if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
        {
            modal.type_char(c);
            ModalKeyOutcome::Consumed
        }
        _ => ModalKeyOutcome::Consumed,
    }
}

/// Number of body lines a *standard* field occupies: one for the label, one
/// for the value/control. [`Field::Display`] varies per-field — see
/// [`field_body_lines`].
const LINES_PER_FIELD: u16 = 2;

/// Number of rows a field occupies inside the body area. Standard fields use
/// [`LINES_PER_FIELD`]; [`Field::Display`] uses `1` for the label plus one
/// row per `\n`-separated body line (minimum `1`, in case the body is empty).
fn field_body_lines(field: &Field) -> u16 {
    match field {
        Field::Display { body, .. } => {
            // `lines()` returns zero entries for an empty string; clamp to 1
            // so the field still occupies a row for its label.
            let body_rows = body.lines().count().max(1) as u16;
            // +1 for the label row.
            body_rows.saturating_add(1)
        }
        _ => LINES_PER_FIELD,
    }
}

/// Number of chrome lines around the field block: top/bottom border (2),
/// optional help hint (1 when present), and the button row (1) plus a
/// trailing blank row (1) for breathing room. We add `2` borders + `1`
/// button row + `1` blank, and the help hint adds its own line on top.
const CHROME_LINES_NO_HELP: u16 = 4;
const CHROME_LINES_WITH_HELP: u16 = 5;

/// Render `modal` centered over `full_area`. The non-modal cells of
/// `full_area` are dimmed by writing a translucent layer of the theme's
/// background colour; the modal block itself is drawn on top of a [`Clear`]
/// so the underlying tab content does not bleed through the modal body.
///
/// The renderer is deliberately stateless: the caller owns the [`ModalSpec`]
/// and decides when to draw it. See module-level docs for the routing
/// contract.
///
/// # Layout
///
/// ```text
/// ┌─ <title> ──────────────────────────────────┐
/// │                                            │
/// │  <label_1>                                 │
/// │  <value/control_1>                         │
/// │                                            │
/// │  <label_2>                                 │
/// │  <value/control_2>                         │
/// │                                            │
/// │  <help_hint?>                              │
/// │                                            │
/// │  [ Enter: <primary> ]  [ Esc: <secondary> ]│
/// └────────────────────────────────────────────┘
/// ```
pub fn render_modal(frame: &mut Frame<'_>, full_area: Rect, theme: &Theme, modal: &ModalSpec) {
    // Dim layer: a Block with only a bg style covers the entire frame area.
    // We use the theme's background colour, which already represents the
    // darkest layer in the palette — putting it back over the existing
    // content effectively "fades" everything behind the modal. The Block
    // has no borders so it never leaks chrome onto the underlying widget.
    let dim = Block::default().style(Style::default().bg(theme.background.into()));
    frame.render_widget(dim, full_area);

    let modal_area = compute_modal_rect(full_area, modal);

    // Clear so the modal body fully replaces whatever was rendered beneath
    // it (the dim layer above does not erase the symbols, just restyles).
    frame.render_widget(Clear, modal_area);

    let border_style = Style::default()
        .fg(theme.accent_primary.into())
        .add_modifier(Modifier::BOLD);
    let title = format!(" {} {} ", theme.glyphs.small_star, modal.title);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title)
        .title_style(
            Style::default()
                .fg(theme.foreground.into())
                .add_modifier(Modifier::BOLD),
        )
        .style(
            Style::default()
                .bg(theme.surface.into())
                .fg(theme.foreground.into()),
        );

    // Render the outer block to claim the area; subsequent paragraphs draw
    // inside the inner rect.
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // Compute the slice of lines we have for body + chrome. The chrome
    // (buttons row + optional help hint + trailing blank) takes the bottom
    // few rows; everything above is the field block.
    let chrome = if modal.help_hint.is_some() {
        CHROME_INNER_WITH_HELP
    } else {
        CHROME_INNER_NO_HELP
    };
    let body_height = inner.height.saturating_sub(chrome);
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(body_height), Constraint::Min(chrome)])
        .split(inner);

    render_fields(frame, layout[0], theme, modal);
    render_chrome(frame, layout[1], theme, modal);
}

/// Inner chrome rows when no help hint is set: blank padding + button row +
/// trailing blank. Three rows is enough breathing room around the buttons.
const CHROME_INNER_NO_HELP: u16 = 3;

/// Inner chrome rows with a help hint: blank padding + help line + button
/// row + trailing blank.
const CHROME_INNER_WITH_HELP: u16 = 4;

/// Compute the rectangle the modal occupies. ~60% of the frame width by
/// default, height fitting the fields plus chrome. Clamped to never exceed
/// the frame size.
fn compute_modal_rect(full_area: Rect, modal: &ModalSpec) -> Rect {
    // Sum per-field heights instead of `n * LINES_PER_FIELD` so a
    // [`Field::Display`] with N body lines pushes the modal taller as
    // needed, while standard fields keep their two-row footprint.
    let fields_lines: u16 = modal
        .fields
        .iter()
        .map(field_body_lines)
        .fold(0u16, |acc, n| acc.saturating_add(n));
    let chrome = if modal.help_hint.is_some() {
        CHROME_LINES_WITH_HELP
    } else {
        CHROME_LINES_NO_HELP
    };
    // +1 for the blank between title and first field, ensures the first
    // label has breathing room beneath the title row.
    let desired_h = fields_lines.saturating_add(chrome).saturating_add(1);
    let h = desired_h.min(full_area.height).max(3);

    let w = (full_area.width * 6 / 10).max(28).min(full_area.width);
    let x = full_area.x + (full_area.width.saturating_sub(w)) / 2;
    let y = full_area.y + (full_area.height.saturating_sub(h)) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

/// Render the field block (labels + value rows) inside `area`.
fn render_fields(frame: &mut Frame<'_>, area: Rect, theme: &Theme, modal: &ModalSpec) {
    if area.height == 0 || modal.fields.is_empty() {
        return;
    }
    // Standard fields are 2 rows; Display fields are 1 + body.lines().count().
    // Render each field independently so the focused field can carry its
    // own border style without touching the others.
    let mut y = area.y;
    for (i, field) in modal.fields.iter().enumerate() {
        if y >= area.y + area.height {
            break;
        }
        let needed = field_body_lines(field);
        let field_area = Rect {
            x: area.x,
            y,
            width: area.width,
            height: needed.min((area.y + area.height).saturating_sub(y)),
        };
        render_field(frame, field_area, theme, field, i == modal.focus);
        y = y.saturating_add(needed);
    }
}

/// Render a single field into a 2-row rect: row 0 is the label, row 1 is
/// the value or control. The focused field draws its value row inside a
/// 1-cell accent border so it stands out from siblings.
fn render_field(frame: &mut Frame<'_>, area: Rect, theme: &Theme, field: &Field, focused: bool) {
    if area.height == 0 {
        return;
    }
    let label_text = field_label(field);
    let label_style = Style::default()
        .fg(theme.muted.into())
        .add_modifier(if focused {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
    let label = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(label_text, label_style),
    ]));
    let label_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    frame.render_widget(label, label_area);

    if area.height < 2 {
        return;
    }

    if let Field::Display { body, .. } = field {
        // Multi-line read-only body: one row per `\n`-separated line, all
        // styled with `theme.muted`. The leading "  " prefix mirrors the
        // single-line body layout so labels and bodies share their column.
        let body_style = Style::default().fg(theme.muted.into());
        let available = area.height.saturating_sub(1);
        let mut y = area.y + 1;
        for (i, line) in body.lines().enumerate() {
            if i as u16 >= available {
                break;
            }
            let row_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            };
            let para = Paragraph::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(line.to_string(), body_style),
            ]));
            frame.render_widget(para, row_area);
            y = y.saturating_add(1);
        }
        return;
    }

    let value_area = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: 1,
    };
    let value_line = render_field_value(theme, field, focused);
    let mut value_para = Paragraph::new(value_line);
    if focused {
        // Subtle accent prefix so screenshots can see which row is focused
        // even when colour is unavailable (insta strips styles).
        value_para = value_para.style(Style::default().fg(theme.accent_primary.into()));
    }
    frame.render_widget(value_para, value_area);
}

/// Resolve the label string from a [`Field`].
fn field_label(field: &Field) -> &str {
    match field {
        Field::Text { label, .. }
        | Field::Password { label, .. }
        | Field::Toggle { label, .. }
        | Field::Choice { label, .. }
        | Field::Picker { label, .. }
        | Field::Display { label, .. } => label,
    }
}

/// Build the value line for a single field. The focus prefix (`>`) is added
/// to the front of focused rows so the snapshot tests can spot the cursor
/// without depending on colour styles.
fn render_field_value<'a>(theme: &'a Theme, field: &'a Field, focused: bool) -> Line<'a> {
    let prefix = if focused { "> " } else { "  " };
    let mut spans = vec![Span::raw(prefix)];
    match field {
        Field::Text {
            value, placeholder, ..
        } => {
            if value.is_empty() {
                if let Some(ph) = placeholder {
                    spans.push(Span::styled(
                        ph.clone(),
                        Style::default().fg(theme.muted.into()),
                    ));
                }
            } else {
                spans.push(Span::raw(value.clone()));
            }
        }
        Field::Password { value, .. } => {
            let bullets: String =
                std::iter::repeat_n(PASSWORD_BULLET, value.chars().count()).collect();
            spans.push(Span::raw(bullets));
        }
        Field::Toggle { value, .. } => {
            let mark = if *value { "[x]" } else { "[ ]" };
            spans.push(Span::styled(
                mark.to_string(),
                Style::default().fg(theme.accent_primary.into()),
            ));
            spans.push(Span::raw(" "));
            spans.push(Span::raw(if *value { "on" } else { "off" }));
            if focused {
                spans.push(Span::raw("   "));
                spans.push(Span::styled(
                    "‹ ›".to_string(),
                    Style::default().fg(theme.muted.into()),
                ));
            }
        }
        Field::Choice {
            options, selected, ..
        } => {
            for (i, opt) in options.iter().enumerate() {
                let glyph = if i == *selected { "(●)" } else { "( )" };
                spans.push(Span::raw(glyph.to_string()));
                spans.push(Span::raw(" "));
                spans.push(Span::raw(opt.clone()));
                if i + 1 < options.len() {
                    spans.push(Span::raw("  "));
                }
            }
            if focused {
                spans.push(Span::raw("   "));
                spans.push(Span::styled(
                    "‹ ›".to_string(),
                    Style::default().fg(theme.muted.into()),
                ));
            }
        }
        Field::Picker { value, hint, .. } => {
            if value.is_empty() {
                spans.push(Span::styled(
                    "(no path)".to_string(),
                    Style::default().fg(theme.muted.into()),
                ));
            } else {
                spans.push(Span::raw(value.clone()));
            }
            if !hint.is_empty() {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    format!("[{hint}]"),
                    Style::default().fg(theme.muted.into()),
                ));
            }
        }
        // `Display` fields are rendered specially by [`render_field`] across
        // multiple rows; this single-line builder is only reached for the
        // first line of a Display body. Return the first line styled with
        // `theme.muted` to match the rest of the body.
        Field::Display { body, .. } => {
            let first = body.lines().next().unwrap_or("");
            spans.push(Span::styled(
                first.to_string(),
                Style::default().fg(theme.muted.into()),
            ));
        }
    }
    Line::from(spans)
}

/// Render the bottom chrome: optional help hint above the button row.
fn render_chrome(frame: &mut Frame<'_>, area: Rect, theme: &Theme, modal: &ModalSpec) {
    if area.height == 0 {
        return;
    }
    let mut y = area.y;
    // Skip a blank row for breathing room.
    y = y.saturating_add(1);

    if let Some(hint) = &modal.help_hint {
        if y < area.y + area.height {
            let hint_para = Paragraph::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(hint.clone(), Style::default().fg(theme.muted.into())),
            ]));
            let hint_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            };
            frame.render_widget(hint_para, hint_area);
            y = y.saturating_add(1);
        }
    }

    if y >= area.y + area.height {
        return;
    }

    let primary = format!("[ Enter: {} ]", modal.primary_label);
    let secondary = format!("[ Esc: {} ]", modal.secondary_label);
    let buttons = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            primary,
            Style::default()
                .fg(theme.accent_primary.into())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled(secondary, Style::default().fg(theme.muted.into())),
    ]));
    let buttons_area = Rect {
        x: area.x,
        y,
        width: area.width,
        height: 1,
    };
    frame.render_widget(buttons, buttons_area);
}

// ---------------------------------------------------------------------------
// Test helpers — pub(crate) usage from integration tests via re-export.
// ---------------------------------------------------------------------------

/// Render a [`ModalSpec`] into a fresh `width × height` test buffer using
/// the cosmos theme and return the concatenated cell symbols as a single
/// `\n`-terminated string. Intended for insta snapshots and ad-hoc visual
/// inspection.
///
/// # Examples
///
/// ```
/// use sid_widgets::modal::{ModalSpec, render_modal_to_string};
///
/// let m = ModalSpec::new("demo", "Demo", vec![]);
/// let s = render_modal_to_string(&m, 60, 12);
/// assert!(s.contains("Demo"));
/// ```
pub fn render_modal_to_string(modal: &ModalSpec, width: u16, height: u16) -> String {
    use ratatui::{Terminal, backend::TestBackend};
    use sid_ui::themes::cosmos;
    let backend = TestBackend::new(width, height);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    term.draw(|f| render_modal(f, f.area(), &theme, modal))
        .unwrap();
    let buf = term.backend().buffer();
    let mut s = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            s.push_str(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_field(label: &str, value: &str) -> Field {
        Field::Text {
            label: label.into(),
            value: value.into(),
            placeholder: None,
        }
    }

    #[test]
    fn new_sets_defaults() {
        let m = ModalSpec::new("id", "title", vec![text_field("a", "")]);
        assert_eq!(m.id, ModalId("id".into()));
        assert_eq!(m.title, "title");
        assert_eq!(m.primary_label, "Save");
        assert_eq!(m.secondary_label, "Cancel");
        assert!(m.help_hint.is_none());
        assert_eq!(m.focus, 0);
    }

    #[test]
    fn with_help_attaches_hint() {
        let m = ModalSpec::new("id", "t", vec![]).with_help("Tab to move");
        assert_eq!(m.help_hint.as_deref(), Some("Tab to move"));
    }

    #[test]
    fn cycle_focus_forward_wraps() {
        let mut m = ModalSpec::new(
            "id",
            "t",
            vec![
                text_field("a", ""),
                text_field("b", ""),
                text_field("c", ""),
            ],
        );
        m.cycle_focus_forward();
        assert_eq!(m.focus, 1);
        m.cycle_focus_forward();
        assert_eq!(m.focus, 2);
        m.cycle_focus_forward();
        assert_eq!(m.focus, 0);
    }

    #[test]
    fn cycle_focus_backward_wraps() {
        let mut m = ModalSpec::new(
            "id",
            "t",
            vec![
                text_field("a", ""),
                text_field("b", ""),
                text_field("c", ""),
            ],
        );
        m.cycle_focus_backward();
        assert_eq!(m.focus, 2);
        m.cycle_focus_backward();
        assert_eq!(m.focus, 1);
        m.cycle_focus_backward();
        assert_eq!(m.focus, 0);
    }

    #[test]
    fn cycle_focus_empty_modal_is_noop() {
        let mut m = ModalSpec::new("id", "t", vec![]);
        m.cycle_focus_forward();
        m.cycle_focus_backward();
        assert_eq!(m.focus, 0);
    }

    #[test]
    fn type_char_appends_to_focused_text() {
        let mut m = ModalSpec::new("id", "t", vec![text_field("a", "h")]);
        m.type_char('i');
        match &m.fields[0] {
            Field::Text { value, .. } => assert_eq!(value, "hi"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn type_char_appends_to_focused_password() {
        let mut m = ModalSpec::new(
            "id",
            "t",
            vec![Field::Password {
                label: "p".into(),
                value: "ab".into(),
            }],
        );
        m.type_char('c');
        match &m.fields[0] {
            Field::Password { value, .. } => assert_eq!(value, "abc"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn type_char_appends_to_focused_picker() {
        let mut m = ModalSpec::new(
            "id",
            "t",
            vec![Field::Picker {
                label: "p".into(),
                value: "/tmp".into(),
                hint: "browse".into(),
            }],
        );
        m.type_char('/');
        m.type_char('a');
        match &m.fields[0] {
            Field::Picker { value, .. } => assert_eq!(value, "/tmp/a"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn type_char_on_toggle_is_noop() {
        let mut m = ModalSpec::new(
            "id",
            "t",
            vec![Field::Toggle {
                label: "on".into(),
                value: false,
            }],
        );
        m.type_char('x');
        match &m.fields[0] {
            Field::Toggle { value, .. } => assert!(!*value),
            _ => unreachable!(),
        }
    }

    #[test]
    fn type_char_on_empty_modal_is_noop() {
        let mut m = ModalSpec::new("id", "t", vec![]);
        m.type_char('x');
        assert!(m.fields.is_empty());
    }

    #[test]
    fn backspace_pops_text() {
        let mut m = ModalSpec::new("id", "t", vec![text_field("a", "hi")]);
        m.backspace();
        match &m.fields[0] {
            Field::Text { value, .. } => assert_eq!(value, "h"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn backspace_on_empty_text_is_noop() {
        let mut m = ModalSpec::new("id", "t", vec![text_field("a", "")]);
        m.backspace();
        match &m.fields[0] {
            Field::Text { value, .. } => assert_eq!(value, ""),
            _ => unreachable!(),
        }
    }

    #[test]
    fn backspace_on_toggle_is_noop() {
        let mut m = ModalSpec::new(
            "id",
            "t",
            vec![Field::Toggle {
                label: "on".into(),
                value: true,
            }],
        );
        m.backspace();
        match &m.fields[0] {
            Field::Toggle { value, .. } => assert!(*value),
            _ => unreachable!(),
        }
    }

    #[test]
    fn space_or_enter_flips_toggle() {
        let mut m = ModalSpec::new(
            "id",
            "t",
            vec![Field::Toggle {
                label: "on".into(),
                value: false,
            }],
        );
        m.space_or_enter_on_field();
        match &m.fields[0] {
            Field::Toggle { value, .. } => assert!(*value),
            _ => unreachable!(),
        }
        m.space_or_enter_on_field();
        match &m.fields[0] {
            Field::Toggle { value, .. } => assert!(!*value),
            _ => unreachable!(),
        }
    }

    #[test]
    fn space_or_enter_advances_choice_with_wrap() {
        let mut m = ModalSpec::new(
            "id",
            "t",
            vec![Field::Choice {
                label: "k".into(),
                options: vec!["a".into(), "b".into(), "c".into()],
                selected: 0,
            }],
        );
        m.space_or_enter_on_field();
        match &m.fields[0] {
            Field::Choice { selected, .. } => assert_eq!(*selected, 1),
            _ => unreachable!(),
        }
        m.space_or_enter_on_field();
        m.space_or_enter_on_field();
        match &m.fields[0] {
            Field::Choice { selected, .. } => assert_eq!(*selected, 0),
            _ => unreachable!(),
        }
    }

    #[test]
    fn space_or_enter_on_empty_choice_is_noop() {
        let mut m = ModalSpec::new(
            "id",
            "t",
            vec![Field::Choice {
                label: "k".into(),
                options: vec![],
                selected: 0,
            }],
        );
        m.space_or_enter_on_field();
        match &m.fields[0] {
            Field::Choice { selected, .. } => assert_eq!(*selected, 0),
            _ => unreachable!(),
        }
    }

    #[test]
    fn space_or_enter_on_text_is_noop() {
        let mut m = ModalSpec::new("id", "t", vec![text_field("a", "x")]);
        m.space_or_enter_on_field();
        match &m.fields[0] {
            Field::Text { value, .. } => assert_eq!(value, "x"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn collect_values_orders_match_fields() {
        let m = ModalSpec::new(
            "id",
            "t",
            vec![
                text_field("a", "x"),
                Field::Password {
                    label: "p".into(),
                    value: "y".into(),
                },
                Field::Toggle {
                    label: "t".into(),
                    value: true,
                },
                Field::Choice {
                    label: "c".into(),
                    options: vec!["one".into(), "two".into()],
                    selected: 1,
                },
                Field::Picker {
                    label: "pk".into(),
                    value: "/etc".into(),
                    hint: String::new(),
                },
            ],
        );
        let v = m.collect_values();
        assert_eq!(v.len(), 5);
        assert!(matches!(&v[0].1, FieldValue::Text(s) if s == "x"));
        assert!(matches!(&v[1].1, FieldValue::Password(s) if s == "y"));
        assert!(matches!(&v[2].1, FieldValue::Toggle(true)));
        assert!(matches!(&v[3].1, FieldValue::Choice(s) if s == "two"));
        assert!(matches!(&v[4].1, FieldValue::Picker(s) if s == "/etc"));
    }

    #[test]
    fn collect_values_handles_empty_choice() {
        let m = ModalSpec::new(
            "id",
            "t",
            vec![Field::Choice {
                label: "c".into(),
                options: vec![],
                selected: 0,
            }],
        );
        let v = m.collect_values();
        assert!(matches!(&v[0].1, FieldValue::Choice(s) if s.is_empty()));
    }

    #[test]
    fn render_modal_does_not_panic_on_tiny_area() {
        let m = ModalSpec::new("id", "t", vec![text_field("a", ""), text_field("b", "")]);
        // 10x4 is way too small for the rendered modal; we just want to
        // confirm the renderer clamps gracefully.
        let s = render_modal_to_string(&m, 10, 4);
        assert!(!s.is_empty());
    }

    #[test]
    fn render_modal_shows_title() {
        let m = ModalSpec::new("id", "MyTitle", vec![]);
        let s = render_modal_to_string(&m, 60, 10);
        assert!(s.contains("MyTitle"));
    }

    #[test]
    fn field_body_lines_text_field_is_two() {
        let f = text_field("a", "");
        assert_eq!(field_body_lines(&f), LINES_PER_FIELD);
    }

    #[test]
    fn field_body_lines_display_counts_lines() {
        let f = Field::Display {
            label: "keys".into(),
            body: "line1\nline2\nline3".into(),
        };
        // 1 label row + 3 body rows.
        assert_eq!(field_body_lines(&f), 4);
    }

    #[test]
    fn field_body_lines_display_empty_body_is_two() {
        let f = Field::Display {
            label: "k".into(),
            body: String::new(),
        };
        // Empty body still claims one row (so the label has a sibling).
        assert_eq!(field_body_lines(&f), 2);
    }

    #[test]
    fn type_char_on_display_is_noop() {
        let mut m = ModalSpec::new(
            "id",
            "t",
            vec![Field::Display {
                label: "k".into(),
                body: "hello".into(),
            }],
        );
        m.type_char('x');
        match &m.fields[0] {
            Field::Display { body, .. } => assert_eq!(body, "hello"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn backspace_on_display_is_noop() {
        let mut m = ModalSpec::new(
            "id",
            "t",
            vec![Field::Display {
                label: "k".into(),
                body: "hello".into(),
            }],
        );
        m.backspace();
        match &m.fields[0] {
            Field::Display { body, .. } => assert_eq!(body, "hello"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn space_or_enter_on_display_is_noop() {
        let mut m = ModalSpec::new(
            "id",
            "t",
            vec![Field::Display {
                label: "k".into(),
                body: "before".into(),
            }],
        );
        m.space_or_enter_on_field();
        match &m.fields[0] {
            Field::Display { body, .. } => assert_eq!(body, "before"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn collect_values_returns_display_body() {
        let m = ModalSpec::new(
            "id",
            "t",
            vec![Field::Display {
                label: "help".into(),
                body: "line1\nline2".into(),
            }],
        );
        let v = m.collect_values();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "help");
        assert!(matches!(&v[0].1, FieldValue::Display(s) if s == "line1\nline2"));
    }

    #[test]
    fn render_modal_display_renders_one_row_per_newline() {
        // Five `\n` separators yields six body rows.
        let body = "row0\nrow1\nrow2\nrow3\nrow4\nrow5";
        let m = ModalSpec::new(
            "id",
            "MultiLine",
            vec![Field::Display {
                label: "help".into(),
                body: body.into(),
            }],
        );
        let s = render_modal_to_string(&m, 60, 24);
        // Every body line should be visible verbatim somewhere in the
        // rendered buffer. Critically, no `\n` literal should appear in
        // the painted cells — the renderer must have split the body
        // across physical rows.
        for row in ["row0", "row1", "row2", "row3", "row4", "row5"] {
            assert!(
                s.contains(row),
                "expected body row {row} to appear in rendered modal:\n{s}"
            );
        }
        // The literal backslash-n sequence must NOT appear (the modal
        // body would have leaked `\n` if the renderer treated it as a
        // single value row).
        assert!(
            !s.contains("\\n"),
            "literal `\\n` leaked into rendered modal:\n{s}"
        );
    }
}
