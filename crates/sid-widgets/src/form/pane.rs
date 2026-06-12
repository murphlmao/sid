use super::spec::{FormSpec, FormValues, SectionKind};
use crate::modal::Field;
use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::event::KeyChord;

/// Where focus sits inside the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFocusState {
    /// A field, by flat index over editable sections only.
    Field(usize),
    /// The primary (Save) button.
    Primary,
}

/// What the host (wire layer) must do after a key event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormEvent {
    /// Keep showing the pane; redraw.
    Continue,
    /// User confirmed submit and validation passed — values snapshot attached.
    Submit(FormValues),
    /// User left the pane (Esc / ← on a clean form, or confirmed discard).
    Cancel,
    /// Form is dirty and user pressed Esc/← — host must show the standard
    /// "discard changes?" confirm modal; on confirm, host closes the pane.
    RequestDiscardConfirm,
}

/// Live form pane: spec + focus + dirty flag.
#[derive(Debug, Clone)]
pub struct FormPane {
    /// The declarative form.
    pub spec: FormSpec,
    /// Current focus.
    pub focus: PaneFocusState,
    /// Set on first successful edit; drives the discard confirm.
    pub dirty: bool,
    /// Values at open time, for dirty comparison.
    baseline: FormValues,
}

impl FormPane {
    /// Open a pane focused on the first editable field.
    pub fn new(spec: FormSpec) -> Self {
        let baseline = spec.values();
        Self {
            spec,
            focus: PaneFocusState::Field(0),
            dirty: false,
            baseline,
        }
    }

    /// Flat list of `(section_idx, field_idx)` for editable-section fields,
    /// in render order — the Tab traversal order.
    fn editable_slots(&self) -> Vec<(usize, usize)> {
        self.spec
            .sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.kind == SectionKind::Editable)
            .flat_map(|(si, s)| (0..s.fields.len()).map(move |fi| (si, fi)))
            .collect()
    }

    fn focused_slot(&self) -> Option<(usize, usize)> {
        match self.focus {
            PaneFocusState::Field(i) => self.editable_slots().get(i).copied(),
            PaneFocusState::Primary => None,
        }
    }

    /// Route one key chord. Returns what the host must do.
    pub fn handle_key(&mut self, chord: KeyChord) -> FormEvent {
        let slots = self.editable_slots().len();
        match (chord.code, chord.mods) {
            (KeyCode::Tab, m) if m.is_empty() => {
                self.focus = match self.focus {
                    PaneFocusState::Field(i) if i + 1 < slots => PaneFocusState::Field(i + 1),
                    PaneFocusState::Field(_) => PaneFocusState::Primary,
                    PaneFocusState::Primary => PaneFocusState::Field(0),
                };
                FormEvent::Continue
            }
            // NB: KeyModifiers is a bitflags struct — it cannot appear as a
            // match *pattern*; classify via guards.
            (KeyCode::BackTab, _) => self.focus_prev(slots),
            (KeyCode::Tab, m) if m.contains(KeyModifiers::SHIFT) => self.focus_prev(slots),
            (KeyCode::Esc, _) => self.leave(),
            (KeyCode::Left, _) if !self.focused_field_is_text() => self.leave(),
            (KeyCode::Enter, _) => match self.focus {
                PaneFocusState::Primary => self.try_submit(),
                PaneFocusState::Field(_) => {
                    // Enter on a field = advance (form-filling muscle memory);
                    // Enter on the last field falls onto Save.
                    self.handle_key(KeyChord {
                        code: KeyCode::Tab,
                        mods: KeyModifiers::empty(),
                    })
                }
            },
            _ => {
                self.edit_focused(chord);
                FormEvent::Continue
            }
        }
    }

    /// Shift+Tab / BackTab: previous field, wrapping list ↔ Save button.
    fn focus_prev(&mut self, slots: usize) -> FormEvent {
        self.focus = match self.focus {
            PaneFocusState::Field(0) => PaneFocusState::Primary,
            PaneFocusState::Field(i) => PaneFocusState::Field(i - 1),
            PaneFocusState::Primary if slots == 0 => PaneFocusState::Primary,
            PaneFocusState::Primary => PaneFocusState::Field(slots - 1),
        };
        FormEvent::Continue
    }

    fn leave(&mut self) -> FormEvent {
        if self.dirty && self.spec.values() != self.baseline {
            FormEvent::RequestDiscardConfirm
        } else {
            FormEvent::Cancel
        }
    }

    fn try_submit(&mut self) -> FormEvent {
        self.revalidate_all();
        if self.spec.first_error().is_some() {
            // Jump focus to the first offending field.
            if let Some(idx) = self.first_error_slot() {
                self.focus = PaneFocusState::Field(idx);
            }
            return FormEvent::Continue;
        }
        FormEvent::Submit(self.spec.values())
    }

    fn first_error_slot(&self) -> Option<usize> {
        let slots = self.editable_slots();
        slots
            .iter()
            .position(|&(si, fi)| self.spec.sections[si].fields[fi].error.is_some())
    }

    fn revalidate_all(&mut self) {
        for section in &mut self.spec.sections {
            for field in &mut section.fields {
                field.error = field
                    .validate
                    .iter()
                    .find_map(|v| v.check(&field.value_string()));
            }
        }
    }

    fn focused_field_is_text(&self) -> bool {
        self.focused_slot().is_some_and(|(si, fi)| {
            matches!(
                self.spec.sections[si].fields[fi].field,
                Field::Text { .. } | Field::Password { .. } | Field::Picker { .. }
            )
        })
    }

    /// Apply a printable/backspace/arrow edit to the focused field; runs the
    /// field's validators, marks dirty, and fires reshape on watched keys.
    fn edit_focused(&mut self, chord: KeyChord) {
        let Some((si, fi)) = self.focused_slot() else {
            return;
        };
        let key = self.spec.sections[si].fields[fi].key.clone();
        let changed = {
            let f = &mut self.spec.sections[si].fields[fi];
            let changed = match (&mut f.field, chord.code) {
                (
                    Field::Text { value, .. }
                    | Field::Password { value, .. }
                    | Field::Picker { value, .. },
                    KeyCode::Char(c),
                ) => {
                    value.push(c);
                    true
                }
                (
                    Field::Text { value, .. }
                    | Field::Password { value, .. }
                    | Field::Picker { value, .. },
                    KeyCode::Backspace,
                ) => value.pop().is_some(),
                (
                    Field::Choice {
                        options, selected, ..
                    },
                    KeyCode::Right | KeyCode::Char(' '),
                ) => {
                    *selected = (*selected + 1) % options.len().max(1);
                    true
                }
                // No Left arms for Choice/Toggle: ← on a non-text field LEAVES
                // the pane (handled before edit_focused is reached). Choice
                // cycles forward-only via Right/Space, wrapping.
                (Field::Toggle { value, .. }, KeyCode::Char(' ') | KeyCode::Right) => {
                    *value = !*value;
                    true
                }
                _ => false,
            };
            if changed {
                f.error = f.validate.iter().find_map(|v| v.check(&f.value_string()));
            }
            changed
        };
        if changed {
            self.dirty = true;
            if self.spec.watch.contains(&key) {
                self.spec.run_reshape();
                // Reshape may shrink the slot list; clamp focus.
                let slots = self.editable_slots().len();
                if let PaneFocusState::Field(i) = self.focus {
                    if i >= slots && slots > 0 {
                        self.focus = PaneFocusState::Field(slots - 1);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::spec::{FormField, FormSection, FormSpec, SectionKind, Validate};
    use super::*;
    use crate::modal::Field;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn chord(code: KeyCode) -> KeyChord {
        KeyChord {
            code,
            mods: KeyModifiers::empty(),
        }
    }

    fn two_field_form() -> FormPane {
        FormPane::new(FormSpec::new(
            "t",
            "T",
            vec![FormSection {
                title: "s".into(),
                kind: SectionKind::Editable,
                fields: vec![
                    FormField::new(
                        "name",
                        Field::Text {
                            label: "name".into(),
                            value: String::new(),
                            placeholder: None,
                        },
                    )
                    .with_validate(vec![Validate::NonEmpty]),
                    FormField::new(
                        "port",
                        Field::Text {
                            label: "port".into(),
                            value: "5432".into(),
                            placeholder: None,
                        },
                    )
                    .with_validate(vec![Validate::Port]),
                ],
            }],
        ))
    }

    #[test]
    fn tab_cycles_fields_then_save_then_wraps() {
        let mut p = two_field_form();
        assert_eq!(p.focus, PaneFocusState::Field(0));
        p.handle_key(chord(KeyCode::Tab));
        assert_eq!(p.focus, PaneFocusState::Field(1));
        p.handle_key(chord(KeyCode::Tab));
        assert_eq!(p.focus, PaneFocusState::Primary);
        p.handle_key(chord(KeyCode::Tab));
        assert_eq!(p.focus, PaneFocusState::Field(0));
    }

    #[test]
    fn esc_on_clean_form_cancels_but_dirty_requests_confirm() {
        let mut p = two_field_form();
        assert_eq!(p.handle_key(chord(KeyCode::Esc)), FormEvent::Cancel);
        p.handle_key(chord(KeyCode::Char('x')));
        assert_eq!(
            p.handle_key(chord(KeyCode::Esc)),
            FormEvent::RequestDiscardConfirm
        );
    }

    #[test]
    fn typing_then_backspace_to_baseline_is_clean_again() {
        let mut p = two_field_form();
        p.handle_key(chord(KeyCode::Char('x')));
        p.handle_key(chord(KeyCode::Backspace));
        // dirty flag is sticky but leave() compares values to baseline
        assert_eq!(p.handle_key(chord(KeyCode::Esc)), FormEvent::Cancel);
    }

    #[test]
    fn submit_blocked_on_invalid_field_and_focus_jumps_there() {
        let mut p = two_field_form();
        // empty name violates NonEmpty
        p.focus = PaneFocusState::Primary;
        assert_eq!(p.handle_key(chord(KeyCode::Enter)), FormEvent::Continue);
        assert_eq!(p.focus, PaneFocusState::Field(0));
    }

    #[test]
    fn valid_form_submits_values() {
        let mut p = two_field_form();
        for c in "prod".chars() {
            p.handle_key(chord(KeyCode::Char(c)));
        }
        p.focus = PaneFocusState::Primary;
        match p.handle_key(chord(KeyCode::Enter)) {
            FormEvent::Submit(v) => {
                assert_eq!(v["name"], "prod");
                assert_eq!(v["port"], "5432");
            }
            other => panic!("expected Submit, got {other:?}"),
        }
    }

    #[test]
    fn left_arrow_leaves_pane_only_on_non_text_fields() {
        let mut p = two_field_form();
        // focused field is Text — Left must NOT leave (it's a no-op edit here)
        assert_eq!(p.handle_key(chord(KeyCode::Left)), FormEvent::Continue);
        p.focus = PaneFocusState::Primary;
        assert_eq!(p.handle_key(chord(KeyCode::Left)), FormEvent::Cancel);
    }

    #[test]
    fn enter_on_field_advances_instead_of_submitting() {
        let mut p = two_field_form();
        assert_eq!(p.handle_key(chord(KeyCode::Enter)), FormEvent::Continue);
        assert_eq!(p.focus, PaneFocusState::Field(1));
    }

    use proptest::prelude::*;

    fn arbitrary_chord() -> impl Strategy<Value = KeyChord> {
        prop_oneof![
            Just(chord(KeyCode::Tab)),
            Just(KeyChord {
                code: KeyCode::Tab,
                mods: KeyModifiers::SHIFT
            }),
            Just(chord(KeyCode::BackTab)),
            Just(chord(KeyCode::Enter)),
            Just(chord(KeyCode::Esc)),
            Just(chord(KeyCode::Left)),
            Just(chord(KeyCode::Right)),
            Just(chord(KeyCode::Backspace)),
            any::<char>()
                .prop_filter("printable", |c| c.is_ascii_graphic())
                .prop_map(|c| chord(KeyCode::Char(c))),
        ]
    }

    proptest! {
        #[test]
        fn focus_never_strands(keys in prop::collection::vec(arbitrary_chord(), 0..64)) {
            let mut p = two_field_form();
            for k in keys {
                let _ = p.handle_key(k);
                // invariant: focus always points at a real slot or Primary
                match p.focus {
                    PaneFocusState::Field(i) => prop_assert!(i < 2),
                    PaneFocusState::Primary => {}
                }
            }
        }
    }
}
