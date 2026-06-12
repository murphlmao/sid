use crate::modal::Field;
use std::collections::BTreeMap;

/// Stable identifier for a form so the binary's submit handler can dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormId(pub String);

/// Whether a section's fields are user-editable or read-only facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    /// Framed input boxes; participates in Tab focus order.
    Editable,
    /// Muted key→value rows; skipped by focus, never editable.
    Info,
}

/// Declarative validators — data, not closures, so specs stay `Clone + Debug`
/// and validators are unit-testable in isolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validate {
    /// Value must be non-empty after trimming.
    NonEmpty,
    /// Value must parse as a u16 >= 1 (TCP port).
    Port,
    /// Value must parse as u64.
    Unsigned,
    /// Value must not exceed `n` characters (Unicode scalar values) after trimming.
    MaxLen(usize),
}

impl Validate {
    /// Check `value`; `None` means valid, `Some(msg)` is the error rendered
    /// under the field box.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::form::Validate;
    /// assert!(Validate::MaxLen(5).check("hello").is_none());
    /// assert!(Validate::MaxLen(4).check("hello").is_some());
    /// assert!(Validate::MaxLen(5).check("  hi  ").is_none()); // trimmed = 2 chars
    /// ```
    pub fn check(self, value: &str) -> Option<String> {
        match self {
            Validate::NonEmpty => {
                if value.trim().is_empty() {
                    Some("required".to_string())
                } else {
                    None
                }
            }
            Validate::Port => match value.trim().parse::<u16>() {
                Ok(p) if p >= 1 => None,
                _ => Some("must be a port (1-65535)".to_string()),
            },
            Validate::Unsigned => {
                if value.trim().parse::<u64>().is_ok() {
                    None
                } else {
                    Some("must be a whole number".to_string())
                }
            }
            Validate::MaxLen(n) => {
                if value.trim().chars().count() > n {
                    Some(format!("max {n} characters"))
                } else {
                    None
                }
            }
        }
    }
}

/// One keyed field inside a section.
#[derive(Debug, Clone)]
pub struct FormField {
    /// Stable key — survives reshapes, names the value in [`FormValues`].
    pub key: String,
    /// Visual + input payload (reuses the modal `Field` enum).
    pub field: Field,
    /// Validators run on every edit and on submit.
    pub validate: Vec<Validate>,
    /// Current validation error, if any (set by `FormPane`, rendered red).
    pub error: Option<String>,
}

impl FormField {
    /// Convenience constructor with no validators.
    pub fn new(key: impl Into<String>, field: Field) -> Self {
        Self {
            key: key.into(),
            field,
            validate: Vec::new(),
            error: None,
        }
    }

    /// Builder: attach validators.
    pub fn with_validate(mut self, v: Vec<Validate>) -> Self {
        self.validate = v;
        self
    }

    /// The field's current value as a string: Text/Password/Picker → the raw
    /// value, Choice → the selected option text, Toggle → "true"/"false",
    /// Display → its body.
    pub fn value_string(&self) -> String {
        match &self.field {
            Field::Text { value, .. }
            | Field::Password { value, .. }
            | Field::Picker { value, .. } => value.clone(),
            Field::Choice {
                options, selected, ..
            } => options.get(*selected).cloned().unwrap_or_default(),
            Field::Toggle { value, .. } => value.to_string(),
            Field::Display { body, .. } => body.clone(),
        }
    }
}

/// A titled group of fields.
#[derive(Debug, Clone)]
pub struct FormSection {
    /// Section heading (e.g. "Connection", "Derived").
    pub title: String,
    /// Editable vs Info.
    pub kind: SectionKind,
    /// Ordered fields.
    pub fields: Vec<FormField>,
}

/// Snapshot of all field values by key. Reshape hooks and submit handlers
/// consume this; it's a plain map so the binary crate never touches widget
/// internals.
pub type FormValues = BTreeMap<String, String>;

/// Rebuilds the section list when a watched field changes. A plain `fn`
/// pointer (not a boxed closure) keeps `FormSpec: Clone + Debug` and forces
/// reshape logic to be a pure, testable function of the values.
pub type ReshapeFn = fn(&FormValues) -> Vec<FormSection>;

/// A whole side-pane form.
#[derive(Debug, Clone)]
pub struct FormSpec {
    /// Dispatch identity (e.g. `database.connection.edit`).
    pub id: FormId,
    /// Pane title.
    pub title: String,
    /// Ordered sections.
    pub sections: Vec<FormSection>,
    /// Primary button label (default "Save").
    pub primary_label: String,
    /// Keys that trigger `reshape` when their value changes.
    pub watch: Vec<String>,
    /// Optional reshape hook.
    pub reshape: Option<ReshapeFn>,
}

impl FormSpec {
    /// Standard form with a "Save" primary button and no reshape.
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        sections: Vec<FormSection>,
    ) -> Self {
        Self {
            id: FormId(id.into()),
            title: title.into(),
            sections,
            primary_label: "Save".to_string(),
            watch: Vec::new(),
            reshape: None,
        }
    }

    /// Builder: watch `keys` and rebuild sections via `f` when one changes.
    pub fn with_reshape(mut self, keys: Vec<String>, f: ReshapeFn) -> Self {
        self.watch = keys;
        self.reshape = Some(f);
        self
    }

    /// Current values of every field, keyed.
    pub fn values(&self) -> FormValues {
        self.sections
            .iter()
            .flat_map(|s| s.fields.iter())
            .map(|f| (f.key.clone(), f.value_string()))
            .collect()
    }

    /// Apply a reshape: rebuild sections from `f`, then copy back the values
    /// of every surviving editable key so user input is never lost.
    pub fn run_reshape(&mut self) {
        let Some(f) = self.reshape else { return };
        let old = self.values();
        let mut next = f(&old);
        for section in &mut next {
            if section.kind != SectionKind::Editable {
                continue;
            }
            for field in &mut section.fields {
                if let Some(prev) = old.get(&field.key) {
                    restore_value(&mut field.field, prev);
                }
            }
        }
        self.sections = next;
    }

    /// First validation error across all fields, if any (submit gate).
    pub fn first_error(&self) -> Option<(String, String)> {
        self.sections
            .iter()
            .flat_map(|s| s.fields.iter())
            .find_map(|f| {
                f.validate
                    .iter()
                    .find_map(|v| v.check(&f.value_string()))
                    .map(|e| (f.key.clone(), e))
            })
    }
}

/// Write `prev` back into a rebuilt field, shape-aware: free-text fields take
/// the string verbatim; a Choice re-selects a matching option (else keeps the
/// reshape's default); a Toggle parses "true"/"false".
fn restore_value(field: &mut Field, prev: &str) {
    match field {
        Field::Text { value, .. } | Field::Password { value, .. } | Field::Picker { value, .. } => {
            *value = prev.to_string();
        }
        Field::Choice {
            options, selected, ..
        } => {
            if let Some(idx) = options.iter().position(|o| o == prev) {
                *selected = idx;
            }
        }
        Field::Toggle { value, .. } => {
            if let Ok(b) = prev.parse::<bool>() {
                *value = b;
            }
        }
        Field::Display { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modal::Field;

    fn text(key: &str, val: &str) -> FormField {
        FormField::new(
            key,
            Field::Text {
                label: key.to_string(),
                value: val.to_string(),
                placeholder: None,
            },
        )
    }

    fn pg_or_sqlite(values: &FormValues) -> Vec<FormSection> {
        let kind = values.get("kind").map(String::as_str).unwrap_or("Postgres");
        let mut fields = vec![FormField::new(
            "kind",
            Field::Choice {
                label: "kind".into(),
                options: vec!["Postgres".into(), "SQLite".into()],
                selected: if kind == "SQLite" { 1 } else { 0 },
            },
        )];
        if kind == "SQLite" {
            fields.push(text("path", ""));
        } else {
            fields.push(text("host", ""));
            fields.push(text("port", "5432").with_validate(vec![Validate::Port]));
        }
        vec![FormSection {
            title: "Connection".into(),
            kind: SectionKind::Editable,
            fields,
        }]
    }

    #[test]
    fn validate_port_rejects_junk_and_zero() {
        assert!(Validate::Port.check("5432").is_none());
        assert!(Validate::Port.check("0").is_some());
        assert!(Validate::Port.check("notaport").is_some());
        assert!(Validate::Port.check("70000").is_some());
    }

    #[test]
    fn values_snapshot_covers_all_field_shapes() {
        let spec = FormSpec::new(
            "t",
            "T",
            vec![FormSection {
                title: "s".into(),
                kind: SectionKind::Editable,
                fields: vec![
                    text("name", "prod"),
                    FormField::new(
                        "kind",
                        Field::Choice {
                            label: "kind".into(),
                            options: vec!["A".into(), "B".into()],
                            selected: 1,
                        },
                    ),
                    FormField::new(
                        "on",
                        Field::Toggle {
                            label: "on".into(),
                            value: true,
                        },
                    ),
                ],
            }],
        );
        let v = spec.values();
        assert_eq!(v["name"], "prod");
        assert_eq!(v["kind"], "B");
        assert_eq!(v["on"], "true");
    }

    #[test]
    fn reshape_preserves_surviving_keys_and_drops_dead_ones() {
        let mut spec = FormSpec::new("t", "T", pg_or_sqlite(&FormValues::new()))
            .with_reshape(vec!["kind".into()], pg_or_sqlite);
        // user types a host, then flips kind to SQLite
        if let Field::Text { value, .. } = &mut spec.sections[0].fields[1].field {
            *value = "10.0.0.5".into();
        }
        if let Field::Choice { selected, .. } = &mut spec.sections[0].fields[0].field {
            *selected = 1;
        }
        spec.run_reshape();
        let v = spec.values();
        assert_eq!(v["kind"], "SQLite");
        assert!(v.contains_key("path"));
        assert!(!v.contains_key("host")); // dead key dropped
        // flip back: port default restored, host is empty again (dead keys are not resurrected)
        if let Field::Choice { selected, .. } = &mut spec.sections[0].fields[0].field {
            *selected = 0;
        }
        spec.run_reshape();
        assert_eq!(spec.values()["port"], "5432");
    }

    #[test]
    fn reshape_is_idempotent_when_nothing_changed() {
        let mut spec = FormSpec::new("t", "T", pg_or_sqlite(&FormValues::new()))
            .with_reshape(vec!["kind".into()], pg_or_sqlite);
        spec.run_reshape();
        let once = spec.values();
        spec.run_reshape();
        assert_eq!(once, spec.values());
    }

    #[test]
    fn first_error_finds_invalid_port() {
        let mut spec = FormSpec::new("t", "T", pg_or_sqlite(&FormValues::new()));
        if let Field::Text { value, .. } = &mut spec.sections[0].fields[2].field {
            *value = "nope".into();
        }
        let (key, _msg) = spec.first_error().expect("port error");
        assert_eq!(key, "port");
    }
}
