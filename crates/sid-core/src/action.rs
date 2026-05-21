//! Actions and the registry that indexes them.
//!
//! An [`Action`] represents a named, user-visible operation (e.g., "Open palette",
//! "Quit"). The [`ActionRegistry`] stores actions by id and provides fuzzy-search.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable, unique identifier for an action.
///
/// # Examples
///
/// ```
/// use sid_core::action::ActionId;
///
/// let id = ActionId::new("app.quit");
/// assert_eq!(id.as_str(), "app.quit");
/// assert_eq!(id.to_string(), "app.quit");
/// ```
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ActionId(String);

impl ActionId {
    /// Create a new `ActionId` from any string-like value.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::ActionId;
    ///
    /// let id = ActionId::new("tabs.next");
    /// assert_eq!(id.as_str(), "tabs.next");
    /// ```
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Return the inner string slice.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::ActionId;
    ///
    /// let id = ActionId::new("palette.open");
    /// assert_eq!(id.as_str(), "palette.open");
    /// ```
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The scope in which an action is active.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ActionScope {
    /// Available regardless of focus.
    Global,
    /// Only active when the named tab is focused.
    Tab(String),
    /// Active within workspace context.
    Workspace,
    /// Active within the workspace file tree.
    WorkspaceTree,
}

/// A named user-visible operation.
///
/// # Examples
///
/// ```
/// use sid_core::action::{Action, ActionScope};
///
/// let a = Action::new("app.quit", "Quit");
/// assert_eq!(a.id.as_str(), "app.quit");
/// assert_eq!(a.label, "Quit");
/// assert_eq!(a.scope, ActionScope::Global);
/// assert!(a.keybind_hint.is_none());
/// ```
#[derive(Clone, Debug)]
pub struct Action {
    pub id: ActionId,
    pub label: String,
    pub scope: ActionScope,
    pub keybind_hint: Option<String>,
}

impl Action {
    /// Create a new `Action` with `Global` scope and no keybind hint.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::{Action, ActionScope};
    ///
    /// let a = Action::new("tabs.next", "Next Tab");
    /// assert_eq!(a.label, "Next Tab");
    /// assert_eq!(a.scope, ActionScope::Global);
    /// ```
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: ActionId::new(id),
            label: label.into(),
            scope: ActionScope::Global,
            keybind_hint: None,
        }
    }
}

/// A registry of actions indexed by [`ActionId`], with fuzzy search.
///
/// # Examples
///
/// ```
/// use sid_core::action::{Action, ActionRegistry};
///
/// let mut reg = ActionRegistry::new();
/// reg.register(Action::new("app.quit", "Quit"));
/// reg.register(Action::new("palette.open", "Open Palette"));
///
/// assert!(reg.get(&"app.quit".into()).is_some());
/// ```
#[derive(Default)]
pub struct ActionRegistry {
    by_id: BTreeMap<ActionId, Action>,
}

impl ActionRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an action. If an action with the same id exists, it is replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::{Action, ActionRegistry};
    ///
    /// let mut reg = ActionRegistry::new();
    /// reg.register(Action::new("app.quit", "Quit"));
    /// assert!(reg.get(&"app.quit".into()).is_some());
    /// ```
    pub fn register(&mut self, a: Action) {
        self.by_id.insert(a.id.clone(), a);
    }

    /// Remove an action by id. Returns the removed action if present.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::{Action, ActionRegistry};
    ///
    /// let mut reg = ActionRegistry::new();
    /// reg.register(Action::new("foo", "Foo"));
    /// assert!(reg.unregister(&"foo".into()).is_some());
    /// assert!(reg.unregister(&"foo".into()).is_none());
    /// ```
    pub fn unregister(&mut self, id: &ActionId) -> Option<Action> {
        self.by_id.remove(id)
    }

    /// Remove all actions whose id begins with `prefix`. Returns the count
    /// removed. Used by Plan 6 to drop globally-scoped quick-actions before
    /// re-hydrating from the store.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::{Action, ActionRegistry};
    ///
    /// let mut reg = ActionRegistry::new();
    /// reg.register(Action::new("qa-1", "X"));
    /// reg.register(Action::new("qa-2", "Y"));
    /// reg.register(Action::new("app.quit", "Quit"));
    /// assert_eq!(reg.unregister_with_prefix("qa-"), 2);
    /// assert!(reg.get(&"app.quit".into()).is_some());
    /// ```
    pub fn unregister_with_prefix(&mut self, prefix: &str) -> usize {
        let to_drop: Vec<ActionId> = self
            .by_id
            .keys()
            .filter(|k| k.as_str().starts_with(prefix))
            .cloned()
            .collect();
        let n = to_drop.len();
        for k in to_drop {
            self.by_id.remove(&k);
        }
        n
    }

    /// Look up an action by id.
    ///
    /// Returns `None` if no action with that id has been registered.
    pub fn get(&self, id: &ActionId) -> Option<&Action> {
        self.by_id.get(id)
    }

    /// Iterate over all registered actions in id-sorted order.
    pub fn all(&self) -> impl Iterator<Item = &Action> {
        self.by_id.values()
    }

    /// Return actions matching `query` using a fuzzy label filter, ranked by
    /// score. An empty query returns all actions.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::action::{Action, ActionRegistry};
    ///
    /// let mut reg = ActionRegistry::new();
    /// reg.register(Action::new("app.quit", "Quit"));
    /// reg.register(Action::new("palette.open", "Open Palette"));
    /// reg.register(Action::new("tabs.next", "Next Tab"));
    ///
    /// // Empty query returns all
    /// assert_eq!(reg.fuzzy("").len(), 3);
    ///
    /// // Prefix match
    /// let hits = reg.fuzzy("quit");
    /// assert_eq!(hits.len(), 1);
    /// assert_eq!(hits[0].id.as_str(), "app.quit");
    /// ```
    pub fn fuzzy(&self, query: &str) -> Vec<&Action> {
        if query.is_empty() {
            return self.all().collect();
        }
        let q = query.to_lowercase();
        let mut scored: Vec<(i32, &Action)> = self
            .by_id
            .values()
            .filter_map(|a| score_label(&q, &a.label).map(|s| (s, a)))
            .collect();
        scored.sort_by(|x, y| y.0.cmp(&x.0));
        scored.into_iter().map(|(_, a)| a).collect()
    }
}

/// Fuzzy-match `query` against `label`. Returns `Some(score)` if all query
/// characters appear in order within `label`, `None` otherwise.
///
/// Scoring:
/// - +5 if the first matched character is at position 0.
/// - +1 per matched character.
/// - +2 bonus for each consecutive run (the matched char immediately follows
///   the previously matched char).
fn score_label(query: &str, label: &str) -> Option<i32> {
    let label_l = label.to_lowercase();
    let mut q = query.chars();
    let mut cur = q.next()?;
    let mut score: i32 = 0;
    let mut last_pos: i32 = -2;
    let mut matched_anything = false;
    for (i, c) in label_l.chars().enumerate() {
        if c == cur {
            matched_anything = true;
            score += if i == 0 { 5 } else { 1 };
            if i as i32 == last_pos + 1 {
                score += 2;
            }
            last_pos = i as i32;
            cur = match q.next() {
                Some(c) => c,
                None => return Some(score),
            };
        }
    }
    if matched_anything && q.clone().next().is_none() {
        Some(score)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// From<&str> / From<String> convenience for ActionId
// ---------------------------------------------------------------------------

impl From<&str> for ActionId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for ActionId {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}
