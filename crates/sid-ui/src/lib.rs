//! `sid-ui` — sid's component crate: the design system, compiled.
//!
//! # Why this crate exists
//!
//! Before it, sid had 113 interactive sites across 17 files and **zero** shared
//! button/badge/card constructors: every affordance was an ad-hoc `div()` style chain
//! retyped inline, so quality was whatever each call site remembered to type and the
//! floor was "text with a hover fill". `.interface-design/system.md` said what things
//! should look like; nothing made that true. This crate is the enforcement mechanism —
//! the design system as an API, built so the wrong thing is hard to type.
//!
//! # Layering
//!
//! `sid-ui` is a **frontend** crate, so naming `gpui` and `gpui-component` here is
//! allowed and correct (CLAUDE.md rule 1). It is the adapter for the rendering surface:
//!
//! - It owns the semantic tokens ([`theme`]) — the single source of colour.
//! - It owns the cosmos -> `gpui_component::ThemeColor` [`bridge`], so widgets borrowed
//!   from the library render in the active sid palette instead of stock shadcn gray.
//! - It wraps library widgets where the library is good, and hand-rolls the rest.
//! - It depends on **no** domain crate: not `sid-core`, not `sid-store`. It knows
//!   colours, spacing and elements, and nothing about hosts, queries or sockets.
//!
//! The `sid` binary's tab modules import from here and, over the migration, stop naming
//! `gpui_component` at all — which left the gpui-component 0.6 / `gpui-pre` 0.3
//! move with exactly one blast radius.
//!
//! # House rules this crate enforces
//!
//! - **Semantic tokens are the only colour source.** The one exemption is the
//!   theme-agnostic modal scrim, [`bridge::SCRIM`], which lives here so no call site
//!   needs a hex literal. `tests/hygiene.rs` scans for the rest.
//! - **No emoji, anywhere.** Glyphs come from [`Icon`], a named registry over the
//!   bundled Lucide monochrome SVGs. Also scanned by `tests/hygiene.rs`.
//! - **Depth is borders and surface shifts, never shadows** — see [`Elevation`]. The
//!   bridge also switches the library's own `shadow` flag off.

pub mod action_cell;
pub mod badge;
pub mod bridge;
pub mod button;
pub mod card;
pub mod elevation;
pub mod empty_state;
pub mod focus;
pub mod gallery;
pub mod grid;
pub mod icon;
pub mod input;
pub mod kbd;
pub mod list;
pub mod meter;
pub mod modal;
pub mod notice;
pub mod radio;
pub mod scale;
pub mod scope_chip;
pub mod segmented;
pub mod status_bar;
pub mod status_dot;
pub mod styled;
pub mod table;
pub mod theme;
pub mod toast;
pub mod toolbar;
pub mod tooltip;
pub mod typography;

pub use action_cell::{ActionCell, Confirm, ConfirmArm, ConfirmButton};
pub use badge::{Badge, BadgeFill, BadgeTone};
pub use button::{Button, ButtonSize, ButtonState, ButtonVariant, IconButton};
pub use card::{Card, PANEL_FILTER_FLOOR};
pub use elevation::Elevation;
pub use empty_state::EmptyState;
pub use grid::{CardGrid, CardPaint, GridCard};
pub use icon::Icon;
pub use input::{
    FIELD_MIN_W, FieldWidth, InputState, SearchInput, TextInput, is_field_submit, on_submit,
};
pub use kbd::Kbd;
pub use list::{List, Row, RowPaint};
pub use meter::{Meter, MeterTone, StatCluster};
pub use modal::{Modal, PanelGeometry};
pub use notice::{InlineNotice, NoticeTone, caveat_line, error_line};
pub use radio::{Radio, RadioPaint, radio_paint};
pub use scale::{UiScale, scaled};
pub use scope_chip::{ScopeChip, ScopeOrigin};
pub use segmented::{Segment, SegmentSelect, SegmentedControl};
pub use status_bar::{StatusBar, StatusItem};
pub use status_dot::{ConnectionState, StatusDot, StatusLegend};
pub use styled::{StyledExt, focus_ring_color, h_flex, v_flex};
// `TABLE_CHROME` is re-exported at the root alongside the rest of the table surface:
// every call site that needs it is doing column arithmetic and already imports
// `ColumnWidth` from here, and having exactly one of the pair live a module deeper was
// a papercut with no rule behind it.
pub use table::{
    ColumnWidth, FillColumns, FillTable, FillTableDelegate, TABLE_CHROME, sortable_th,
};
pub use theme::Theme;
pub use toast::{Toast, ToastPaint, ToastTone};
pub use toolbar::Toolbar;
pub use tooltip::Tipped;
pub use typography::{ALL_TYPE_ROLES, TypeRole, TypeSpec, Typography, UI_MONO};
