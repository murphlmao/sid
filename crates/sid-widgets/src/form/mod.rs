//! Side-pane form substrate (UX-v2).
//!
//! A [`FormSpec`] is a declarative description of an add/edit form or a
//! read-only inspector: ordered sections, each `Editable` or `Info`, holding
//! keyed fields. [`FormPane`] (see `pane.rs`) owns a spec plus focus/dirty
//! state and turns key events into value edits; `render.rs` draws it as the
//! right side pane of a tab body. The binary crate constructs specs and
//! dispatches submits by [`FormId`] — exactly the pattern `ModalSpec` uses,
//! relocated from a centered popup to a framed side pane.

mod pane;
mod render;
mod spec;

// pub use pane::{FormEvent, FormPane, PaneFocusState};
// pub use render::render_form_pane;
pub use spec::{FormField, FormId, FormSection, FormSpec, FormValues, SectionKind, Validate};
