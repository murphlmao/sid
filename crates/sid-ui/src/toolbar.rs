//! The toolbar — the filter · count · actions row.
//!
//! System, Network and Workspaces each grew their own version of this row, and each one
//! is worse than the last: a hairline rectangle for the filter, a bare accent word for
//! refresh, and no count at all. `systems_tab.rs:469-483` is the canonical example —
//! eight chained style calls typed inline for what should be one constructor.
//!
//! The shape is fixed here so those three screens stop disagreeing: filter on the left
//! and growing, the count as muted metadata, actions right-aligned, a hairline under the
//! whole row separating it from the data below.

use gpui::{
    AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div,
    prelude::FluentBuilder as _, rgb,
};

use crate::styled::{StyledExt as _, h_flex};
use crate::theme;

/// `n` of something, pluralised by the regular English rules.
///
/// Every noun these rows count is regular — process, port, workspace, connection,
/// interface, row, query, match — but "regular" is not "always `+s`": a sibilant ending
/// takes `-es` (**process**es, not "processs") and a consonant + `y` takes `-ies`.
/// Irregular nouns should go through [`Toolbar::count_label`] pre-formatted.
pub fn count_label(n: usize, singular: &str) -> String {
    if n == 1 {
        return format!("1 {singular}");
    }
    format!("{n} {}", pluralize(singular))
}

/// The plural of a regular noun.
fn pluralize(singular: &str) -> String {
    const SIBILANTS: [&str; 5] = ["s", "x", "z", "ch", "sh"];
    if SIBILANTS.iter().any(|end| singular.ends_with(end)) {
        return format!("{singular}es");
    }
    // Consonant + y -> -ies ("query" -> "queries"), but vowel + y keeps the y
    // ("key" -> "keys").
    if let Some(stem) = singular.strip_suffix('y')
        && !stem.ends_with(['a', 'e', 'i', 'o', 'u'])
    {
        return format!("{stem}ies");
    }
    format!("{singular}s")
}

/// The filter · count · actions row.
///
/// ```ignore
/// Toolbar::new()
///     .filter(search_input)
///     .count(processes.len(), "process")
///     .action(Button::new("refresh", "refresh").small().icon(Icon::Refresh))
/// ```
#[derive(IntoElement)]
pub struct Toolbar {
    filter: Option<AnyElement>,
    count: Option<SharedString>,
    actions: Vec<AnyElement>,
    children: Vec<AnyElement>,
}

impl Toolbar {
    /// An empty toolbar.
    pub fn new() -> Self {
        Self {
            filter: None,
            count: None,
            actions: Vec::new(),
            children: Vec::new(),
        }
    }

    /// The filter control. Takes the row's free width, so a wide window widens the
    /// field instead of stranding it at the left edge.
    pub fn filter(mut self, filter: impl IntoElement) -> Self {
        self.filter = Some(filter.into_any_element());
        self
    }

    /// `n` of `singular`, pluralised — see [`count_label`].
    pub fn count(mut self, n: usize, singular: &str) -> Self {
        self.count = Some(count_label(n, singular).into());
        self
    }

    /// A pre-formatted count, for irregular nouns or a richer summary
    /// (`"12 of 340 rows"`).
    pub fn count_label(mut self, label: impl Into<SharedString>) -> Self {
        self.count = Some(label.into());
        self
    }

    /// A right-aligned action. Repeatable; they render in the order added.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }
}

impl Default for Toolbar {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for Toolbar {
    /// Extra controls, placed between the filter and the count.
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Toolbar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let has_filter = self.filter.is_some();
        h_flex()
            .w_full()
            .gap_2()
            .px_3()
            .py_2()
            .hairline_b(&theme)
            .when_some(self.filter, |this, filter| {
                this.child(div().flex_1().min_w_0().child(filter))
            })
            // Without a filter the count and actions still belong on the right edge.
            .when(!has_filter, |this| this.child(div().flex_1()))
            .children(self.children)
            .when_some(self.count, |this, count| {
                this.child(div().flex_none().hint_text(&theme).child(count))
            })
            .when(!self.actions.is_empty(), |this| {
                this.child(h_flex().flex_none().gap_1().children(self.actions))
            })
            .text_color(rgb(theme.fg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_is_singular_and_everything_else_is_not() {
        // Including zero: "0 process" is the kind of detail that makes a UI feel
        // hand-made in the wrong way.
        assert_eq!(count_label(1, "process"), "1 process");
        assert_eq!(count_label(0, "process"), "0 processes");
        assert_eq!(count_label(2, "process"), "2 processes");
        assert_eq!(count_label(340, "row"), "340 rows");
    }

    #[test]
    fn a_sibilant_ending_takes_es() {
        // The first thing a naive `+s` gets wrong, on the very first screen that
        // needs it: the System tab counts *processes*.
        assert_eq!(pluralize("process"), "processes");
        assert_eq!(pluralize("match"), "matches");
        assert_eq!(pluralize("branch"), "branches");
        assert_eq!(pluralize("index"), "indexes");
    }

    #[test]
    fn a_consonant_y_takes_ies_and_a_vowel_y_does_not() {
        assert_eq!(pluralize("query"), "queries");
        assert_eq!(pluralize("directory"), "directories");
        assert_eq!(pluralize("key"), "keys");
    }

    #[test]
    fn the_ordinary_nouns_just_take_s() {
        for (singular, plural) in [
            ("port", "ports"),
            ("host", "hosts"),
            ("workspace", "workspaces"),
            ("connection", "connections"),
            ("interface", "interfaces"),
            ("row", "rows"),
        ] {
            assert_eq!(pluralize(singular), plural);
        }
    }

    #[test]
    fn the_count_setters_agree() {
        let count_of = |toolbar: Toolbar| toolbar.count.map(|c| c.to_string());
        assert_eq!(
            count_of(Toolbar::new().count(3, "port")).as_deref(),
            Some("3 ports")
        );
        assert_eq!(
            count_of(Toolbar::new().count_label("12 of 340 rows")).as_deref(),
            Some("12 of 340 rows")
        );
        // The later call wins, so a screen cannot accidentally render two counts.
        assert_eq!(
            count_of(Toolbar::new().count(3, "port").count_label("all ports")).as_deref(),
            Some("all ports")
        );
    }
}
