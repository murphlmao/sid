//! The Access-style "Relationships" diagram — a pop-out OS window opened from the
//! Database tab's "⧉ diagram" button ([`crate::ui::db_tab::AppState::open_diagram_window`]).
//!
//! [`DiagramView`] is handed an owned snapshot of the active connection's cached
//! [`SchemaInfo`] + [`SchemaGraph`] at open time (see that method's doc comment for why
//! a snapshot is fine for v1) and is otherwise fully self-contained: table boxes laid
//! out on a scrollable canvas, FK lines drawn under them, `1`/`∞` endpoint labels, and
//! drag-to-reposition — all pure sync render-from-state, no runtime/async of its own.

use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, AnyWindowHandle, Bounds, ClickEvent, Context, FontWeight, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Path, Pixels, Point, Render, SharedString,
    WeakEntity, Window, canvas, div, point, prelude::*, px, rgb, size,
};
use sid_core::db::{SchemaGraph, SchemaInfo};

use crate::app::AppState;
use crate::ui::db_tab::table_display_name;
use crate::ui::theme;

// ---- layout geometry ------------------------------------------------------------------------

const BOX_WIDTH: f32 = 220.0;
const HEADER_HEIGHT: f32 = 26.0;
const ROW_HEIGHT: f32 = 20.0;
const MAX_VISIBLE_COLUMNS: usize = 14;
const GAP_X: f32 = 60.0;
const GAP_Y: f32 = 40.0;
const BOXES_PER_COLUMN: usize = 4;
const MARGIN: f32 = 40.0;
const LINE_THICKNESS: f32 = 2.0;
/// Task 2's click-vs-drag threshold: total mouse-down→mouse-up pointer movement, in px,
/// under which a header interaction counts as a click rather than a drag. Small enough
/// to catch real drags unambiguously, generous enough to absorb natural pointer jitter
/// on an intended click.
const CLICK_DRAG_THRESHOLD_PX: f32 = 4.0;

/// One table box's rendering state — a pure projection of [`SchemaInfo`]/[`SchemaGraph`]
/// keyed by [`table_display_name`] (the same qualification rule
/// [`sid_core::db::ForeignKey`] edges use, so boxes and edges join by plain string
/// equality).
struct DiagramTable {
    /// The qualified key (`"schema.name"` or bare `"name"`) — also the header's display
    /// text. One field, not two: for every engine sid supports, the display name *is*
    /// the join key (see `table_display_name`'s doc comment), so a separate "display
    /// name" field would only ever hold a duplicate of this one.
    key: String,
    columns: Vec<String>,
    pk: HashSet<String>,
    fk_columns: HashSet<String>,
}

/// One FK edge, filtered to tables present in the snapshot (see [`DiagramView::new`]).
struct DiagramEdge {
    from_table: String,
    to_table: String,
    /// `from_table == to_table` — a self-referencing FK. Drawn as a small "↺ N" count
    /// badge on the table box instead of a line (see [`DiagramView::self_ref_count`]'s
    /// doc comment for why a loop-stub line was skipped for v1).
    self_ref: bool,
}

/// In-flight header interaction state: which table's header the mouse went down on, the
/// fixed offset between the mouse position and that table's top-left at that moment
/// (used to compute the table's new position once dragging starts — see
/// [`DiagramView::on_mouse_move`]), and the mouse-down position itself, kept to
/// distinguish a click from a drag (see [`is_click`]).
struct DragState {
    table_key: String,
    grab_offset: Point<Pixels>,
    mouse_down_pos: Point<Pixels>,
    /// Flips to `true` once cumulative movement crosses [`CLICK_DRAG_THRESHOLD_PX`] —
    /// until then, `on_mouse_move` leaves the table's position untouched, so a clean
    /// click never nudges the box even by a sub-threshold amount.
    dragging: bool,
}

/// The relationships diagram's whole state. Renders from `tables`/`edges`/`positions`
/// alone — dragging just mutates `positions` and calls `cx.notify()`; the lines follow
/// because line endpoints are recomputed from `positions` on every render.
pub struct DiagramView {
    tables: Vec<DiagramTable>,
    edges: Vec<DiagramEdge>,
    positions: HashMap<String, Point<Pixels>>,
    drag: Option<DragState>,
    selected: Option<String>,
    /// Task 2's click-through: a weak handle back to the main window's [`AppState`], and
    /// that window itself — see [`crate::ui::db_tab::AppState::open_diagram_window`]'s
    /// doc comment for why both are needed (the entity is app-global, but the SQL
    /// editor's mutators need the *main* window's real `Window`, not this one's).
    app: WeakEntity<AppState>,
    main_window: AnyWindowHandle,
}

impl DiagramView {
    /// Build the diagram from a snapshot of the active connection's schema + graph.
    /// Edges naming a table absent from `schema` are dropped silently (defensive — the
    /// backend contract doesn't guarantee it, and a dangling edge would panic the
    /// anchor-picking geometry). Self-referencing edges are kept (as box badges, not
    /// lines) rather than dropped.
    ///
    /// `app`/`main_window` back Task 2's click-through (a table-name or column-row click
    /// jumping to the main window's SQL editor) — see
    /// [`crate::ui::db_tab::AppState::open_diagram_window`]'s doc comment for where
    /// they come from and why both are needed.
    pub fn new(
        schema: SchemaInfo,
        graph: SchemaGraph,
        app: WeakEntity<AppState>,
        main_window: AnyWindowHandle,
    ) -> Self {
        let tables: Vec<DiagramTable> = schema
            .tables
            .iter()
            .map(|t| {
                let key = table_display_name(t);
                let pk: HashSet<String> = graph
                    .primary_keys
                    .get(&key)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                let mut fk_columns = HashSet::new();
                for fk in &graph.foreign_keys {
                    if fk.from_table == key {
                        fk_columns.extend(fk.from_columns.iter().cloned());
                    }
                    if fk.to_table == key {
                        fk_columns.extend(fk.to_columns.iter().cloned());
                    }
                }
                DiagramTable {
                    key,
                    columns: t.columns.iter().map(|c| c.name.clone()).collect(),
                    pk,
                    fk_columns,
                }
            })
            .collect();

        let known: HashSet<&str> = tables.iter().map(|t| t.key.as_str()).collect();
        let edges: Vec<DiagramEdge> = graph
            .foreign_keys
            .iter()
            .filter(|fk| {
                known.contains(fk.from_table.as_str()) && known.contains(fk.to_table.as_str())
            })
            .map(|fk| DiagramEdge {
                from_table: fk.from_table.clone(),
                to_table: fk.to_table.clone(),
                self_ref: fk.from_table == fk.to_table,
            })
            .collect();

        let layout_inputs: Vec<LayoutInput> = tables
            .iter()
            .map(|t| {
                let degree = edges
                    .iter()
                    .filter(|e| e.from_table == t.key || e.to_table == t.key)
                    .count();
                LayoutInput {
                    key: t.key.clone(),
                    degree,
                    column_count: t.columns.len(),
                }
            })
            .collect();
        let positions = compute_initial_layout(&layout_inputs);

        Self {
            tables,
            edges,
            positions,
            drag: None,
            selected: None,
            app,
            main_window,
        }
    }

    /// How many self-referencing FKs `key` has — rendered as a "↺ N" badge on the box
    /// header rather than a loop-stub line (a real loop needs a curved path bulging out
    /// from and back into the same edge, which is a lot of geometry for a case that's
    /// rare in practice; a badge says "this table references itself" just as clearly).
    fn self_ref_count(&self, key: &str) -> usize {
        self.edges
            .iter()
            .filter(|e| e.self_ref && e.from_table == key)
            .count()
    }

    /// Each table's content-space bounding box (position + size), used both to paint
    /// the FK lines (via [`edge_anchors`]) and to place the `1`/`∞` labels.
    fn table_bounds(&self) -> HashMap<String, Bounds<Pixels>> {
        self.tables
            .iter()
            .filter_map(|t| {
                let pos = *self.positions.get(&t.key)?;
                let height = box_height(t.columns.len());
                Some((
                    t.key.clone(),
                    Bounds {
                        origin: pos,
                        size: size(px(BOX_WIDTH), px(height)),
                    },
                ))
            })
            .collect()
    }

    /// The scrollable content area's size — the bounding box of every table box, plus
    /// margin on the trailing edges (the leading edges already have `MARGIN` baked into
    /// [`compute_initial_layout`]'s starting coordinates).
    fn content_size(&self) -> (f32, f32) {
        let mut max_x = 0f32;
        let mut max_y = 0f32;
        for t in &self.tables {
            let Some(&pos) = self.positions.get(&t.key) else {
                continue;
            };
            let height = box_height(t.columns.len());
            max_x = max_x.max(f32::from(pos.x) + BOX_WIDTH);
            max_y = max_y.max(f32::from(pos.y) + height);
        }
        (max_x + MARGIN, max_y + MARGIN)
    }

    /// Header mouse-down: arm a potential drag and select the table. Selection happens
    /// immediately either way (matches the pre-Task-2 behavior — there's no interaction
    /// where selecting without dragging/clicking matters); whether this interaction ends
    /// up moving the box or firing the table-name click-through is decided at mouse-up
    /// (see [`Self::on_mouse_up`]) once we know how far the pointer traveled.
    fn start_drag(&mut self, key: &str, mouse_pos: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(&table_pos) = self.positions.get(key) else {
            return;
        };
        self.drag = Some(DragState {
            table_key: key.to_string(),
            grab_offset: mouse_pos - table_pos,
            mouse_down_pos: mouse_pos,
            dragging: false,
        });
        self.selected = Some(key.to_string());
        cx.notify();
    }

    /// Bound to the scroll container so it fires for the whole canvas, not just the
    /// dragged box (whose own bounds the mouse quickly leaves once dragging starts). The
    /// box only starts tracking the mouse once cumulative movement crosses
    /// [`CLICK_DRAG_THRESHOLD_PX`] (click-vs-drag disambiguation, Task 2) — before that,
    /// this is a no-op, so a clean click never nudges the box.
    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        if !drag.dragging {
            if is_click(drag.mouse_down_pos, event.position, CLICK_DRAG_THRESHOLD_PX) {
                return;
            }
            drag.dragging = true;
        }
        let new_pos = event.position - drag.grab_offset;
        let key = drag.table_key.clone();
        self.positions.insert(key, new_pos);
        cx.notify();
    }

    /// Mouse-up ends the header interaction started in [`Self::start_drag`]. If it never
    /// crossed the drag threshold (`!drag.dragging`), this was a clean click, not a
    /// drag — fire the table-name click-through (Task 2) instead of leaving the box
    /// merely selected/repositioned.
    fn on_mouse_up(&mut self, _event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if !drag.dragging {
            self.navigate_to_table(&drag.table_key, cx);
        }
        cx.notify();
    }

    /// A clean click (Task 2) on a table's header: jump the MAIN window's SQL editor to
    /// `SELECT * FROM <table>` and run it. Crosses OS windows via `main_window`/`app`
    /// (see [`DiagramView::new`]'s doc comment) — `AnyWindowHandle::update` hands back
    /// the main window's real `Window`, which `AppState::diagram_open_table` needs to
    /// touch the SQL `InputState` correctly. Best-effort: both `update` calls' results
    /// are dropped — if the main window has since closed or the app entity released,
    /// there's nothing to recover, and the diagram keeps working regardless.
    fn navigate_to_table(&self, table_key: &str, cx: &mut Context<Self>) {
        let app = self.app.clone();
        let table = table_key.to_string();
        let _ = self.main_window.update(cx, move |_root, window, cx| {
            window.activate_window();
            let _ = app.update(cx, |app, cx| {
                app.diagram_open_table(&table, window, cx);
            });
        });
    }

    /// A column-row click (Task 2): seed the MAIN window's SQL editor with a `WHERE`
    /// filter scaffold for `table`/`column` (not run). Same cross-window mechanism as
    /// [`Self::navigate_to_table`].
    fn navigate_to_column_filter(&self, table_key: &str, column: &str, cx: &mut Context<Self>) {
        let app = self.app.clone();
        let table = table_key.to_string();
        let column = column.to_string();
        let _ = self.main_window.update(cx, move |_root, window, cx| {
            window.activate_window();
            let _ = app.update(cx, |app, cx| {
                app.diagram_open_column_filter(&table, &column, window, cx);
            });
        });
    }

    /// The header bar: table/relationship counts, and — when the backend hasn't wired
    /// up `schema_graph` yet (or the engine genuinely has no FKs) — a subtle hint
    /// rather than a diagram that silently looks broken.
    fn header(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (border, muted) = (t.border, t.muted);
        let summary = format!(
            "{} tables · {} relationships",
            self.tables.len(),
            self.edges.len()
        );
        let hint: Option<SharedString> = self
            .edges
            .is_empty()
            .then(|| "no foreign keys detected — showing table layout only".into());

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(border))
            .child(div().text_sm().text_color(rgb(muted)).child(summary))
            .children(hint.map(|h| div().text_xs().text_color(rgb(muted)).child(h)))
    }

    /// The FK-lines layer. A `canvas()` sized to the scrollable content (see
    /// `session.rs`'s PTY grid for the same low-level-paint pattern) painted *before*
    /// the table boxes in child order, so lines sit under them. Two passes: every
    /// non-selected edge dim, then the selected table's edges again in bright — cheap
    /// re-draw over the dim pass rather than tracking per-edge "is this one bright" state.
    ///
    /// `table_bounds` is *moved* in rather than recomputed here — `render` builds it
    /// once per frame, hands the same map to [`Self::edge_labels`] by reference first,
    /// then moves ownership into this `'static` `canvas()` closure last (perf audit
    /// finding #2: was two separate `table_bounds()` builds per render).
    fn edges_canvas(
        &self,
        content_size: (f32, f32),
        table_bounds: HashMap<String, Bounds<Pixels>>,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        // Copied out (not borrowed) — the canvas closures below are `'static` and can't
        // hold a reference back into the active theme global.
        let t = theme::active(cx);
        let (edge_dim, edge_bright) = (t.border, t.accent);
        let edges: Vec<(String, String)> = self
            .edges
            .iter()
            .filter(|e| !e.self_ref)
            .map(|e| (e.from_table.clone(), e.to_table.clone()))
            .collect();
        let selected = self.selected.clone();

        canvas(
            move |_bounds, _window, _cx| {},
            move |bounds, (), window, _cx| {
                for (from_key, to_key) in &edges {
                    let is_selected = selected.as_deref() == Some(from_key.as_str())
                        || selected.as_deref() == Some(to_key.as_str());
                    if is_selected {
                        continue;
                    }
                    paint_edge(
                        bounds.origin,
                        &table_bounds,
                        from_key,
                        to_key,
                        edge_dim,
                        window,
                    );
                }
                if let Some(sel) = &selected {
                    for (from_key, to_key) in &edges {
                        if from_key == sel || to_key == sel {
                            paint_edge(
                                bounds.origin,
                                &table_bounds,
                                from_key,
                                to_key,
                                edge_bright,
                                window,
                            );
                        }
                    }
                }
            },
        )
        .w(px(content_size.0))
        .h(px(content_size.1))
    }

    /// The `∞`/`1` endpoint labels — plain text `div`s (not canvas text, per the plan:
    /// canvas is for the line geometry only), positioned at the same anchors the lines
    /// use, so they always sit right at the line's end regardless of drag. Returned
    /// flat (two elements per edge) rather than wrapped in a container div, so each
    /// label is a direct child of the same positioned container the table boxes are —
    /// no ambiguity about which ancestor `left`/`top` resolve against.
    ///
    /// Takes `table_bounds` by reference (borrowed from `render`'s single per-frame
    /// build — see [`Self::edges_canvas`]'s doc comment) rather than computing its own.
    fn edge_labels(
        &self,
        table_bounds: &HashMap<String, Bounds<Pixels>>,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let accent = theme::active(cx).accent;
        self.edges
            .iter()
            .filter(|e| !e.self_ref)
            .enumerate()
            .flat_map(|(ix, edge)| {
                let (Some(from), Some(to)) = (
                    table_bounds.get(&edge.from_table).copied(),
                    table_bounds.get(&edge.to_table).copied(),
                ) else {
                    return Vec::new();
                };
                let (from_anchor, to_anchor) = edge_anchors(from, to);
                vec![
                    edge_label(("diagram-edge-many", ix), from_anchor, "∞", accent)
                        .into_any_element(),
                    edge_label(("diagram-edge-one", ix), to_anchor, "1", accent).into_any_element(),
                ]
            })
            .collect()
    }

    /// One table's box: a header (drag handle + select trigger + optional self-ref
    /// badge) over a capped column list (`[pk]` prefix on PK columns, tinted on FK columns,
    /// `+N more` once the list exceeds [`MAX_VISIBLE_COLUMNS`]).
    fn table_box(
        &self,
        ix: usize,
        table: &DiagramTable,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (surface, selection_bg, border, fg, muted, accent, success) = (
            t.surface,
            t.selection,
            t.border,
            t.fg,
            t.muted,
            t.accent,
            t.success,
        );
        let pos = self.positions.get(&table.key).copied().unwrap_or_default();
        let height = box_height(table.columns.len());
        let selected = self.selected.as_deref() == Some(table.key.as_str());
        let self_ref_count = self.self_ref_count(&table.key);
        let header_key = table.key.clone();

        let header = div()
            .id(("diagram-box-header", ix))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_1()
            .px_2()
            .py_1()
            .bg(rgb(selection_bg))
            .rounded_t_md()
            .cursor_pointer()
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::BOLD)
                    .text_sm()
                    .text_color(rgb(fg))
                    .child(table.key.clone()),
            )
            .children((self_ref_count > 0).then(|| {
                div()
                    .text_xs()
                    .text_color(rgb(muted))
                    .child(format!("↺ {self_ref_count}"))
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                    this.start_drag(&header_key, ev.position, cx);
                }),
            );

        let table_key = table.key.clone();
        let visible = table
            .columns
            .iter()
            .take(MAX_VISIBLE_COLUMNS)
            .enumerate()
            .map(|(cix, col)| {
                let is_pk = table.pk.contains(col);
                let is_fk = table.fk_columns.contains(col);
                let label = if is_pk {
                    format!("[pk] {col}")
                } else {
                    col.clone()
                };
                let click_table = table_key.clone();
                let click_column = col.clone();
                div()
                    .id(("diagram-box-col", ix * 1000 + cix))
                    .px_2()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(if is_fk { success } else { muted }))
                    .hover(|s| s.bg(rgb(selection_bg)))
                    .child(label)
                    // Task 2: click a column to seed a `WHERE` filter scaffold in the
                    // main window's editor (Murphy: "i like the filter option when we
                    // click on a field"). Columns aren't drag handles, so no
                    // click-vs-drag threshold is needed here — GPUI's own `on_click`
                    // already only fires for a clean mouse-down+up on this element.
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                        this.navigate_to_column_filter(&click_table, &click_column, cx);
                    }))
                    .into_any_element()
            });
        let overflow = (table.columns.len() > MAX_VISIBLE_COLUMNS).then(|| {
            let more = table.columns.len() - MAX_VISIBLE_COLUMNS;
            div()
                .px_2()
                .text_xs()
                .text_color(rgb(muted))
                .child(format!("+{more} more"))
                .into_any_element()
        });

        div()
            .id(("diagram-box", ix))
            .absolute()
            .left(px(f32::from(pos.x)))
            .top(px(f32::from(pos.y)))
            .w(px(BOX_WIDTH))
            .h(px(height))
            .flex()
            .flex_col()
            .bg(rgb(surface))
            .border_1()
            .border_color(rgb(if selected { accent } else { border }))
            .rounded_md()
            .child(header)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .py_1()
                    .children(visible)
                    .children(overflow),
            )
    }
}

impl Render for DiagramView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bg = theme::active(cx).bg;
        let content_size = self.content_size();
        // Perf audit finding #2: build once per render, not once per call site.
        // `edge_labels` only ever needs to *read* it, so that call comes first
        // (borrowing); `edges_canvas`'s `canvas()` closure is `'static`, so it takes
        // ownership last, once nothing else needs the map.
        let table_bounds = self.table_bounds();
        let edge_labels = self.edge_labels(&table_bounds, cx);
        // Indices, not an iterator over `&self.tables` — `table_box` re-borrows
        // `self.tables[ix]` itself, since it also needs `&mut self` (via `cx.listener`)
        // in the same call, and an active `&self.tables` iterator would conflict with that.
        let boxes: Vec<_> = (0..self.tables.len())
            .map(|ix| {
                let table = &self.tables[ix];
                self.table_box(ix, table, cx)
            })
            .collect();

        let content = div()
            .id("diagram-content")
            .relative()
            .w(px(content_size.0))
            .h(px(content_size.1))
            .child(self.edges_canvas(content_size, table_bounds, cx))
            .children(boxes)
            .children(edge_labels);

        div()
            .id("diagram-root")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(bg))
            .child(self.header(cx))
            .child(
                div()
                    .id("diagram-scroll")
                    .flex_1()
                    .overflow_scroll()
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .child(content),
            )
    }
}

// ---- pure geometry helpers (unit-tested — no `Window`/`Context` needed) --------------------

/// Task 2's click-vs-drag decision: whether a mouse-down at `down` followed by a
/// mouse-up at `up` should count as a click (`true`) rather than a drag (`false`) —
/// total pointer movement at or under `threshold_px`. Pure — no `Window`/`Context`
/// needed — so it's the unit-testable core of the disambiguation.
fn is_click(down: Point<Pixels>, up: Point<Pixels>, threshold_px: f32) -> bool {
    let dx = f32::from(up.x) - f32::from(down.x);
    let dy = f32::from(up.y) - f32::from(down.y);
    (dx * dx + dy * dy).sqrt() <= threshold_px
}

/// One table's layout inputs — everything [`compute_initial_layout`] needs to place it,
/// stripped of the rest of [`DiagramTable`] so the layout function stays pure and
/// trivially testable.
struct LayoutInput {
    key: String,
    /// Count of FK edges touching this table (either side) — higher-degree tables (the
    /// "hubs" of the schema) sort first, so they land in the first column near the top.
    degree: usize,
    column_count: usize,
}

/// A table box's height for `column_count` columns: header + one row per column, capped
/// at [`MAX_VISIBLE_COLUMNS`] (plus one more row for the "+N more" line beyond the cap).
fn box_height(column_count: usize) -> f32 {
    let visible_rows = if column_count > MAX_VISIBLE_COLUMNS {
        MAX_VISIBLE_COLUMNS + 1
    } else {
        column_count.max(1)
    };
    HEADER_HEIGHT + visible_rows as f32 * ROW_HEIGHT
}

/// Deterministic initial layout: sort tables by FK-degree descending (ties broken by
/// key, for a stable order independent of `SchemaInfo`'s original table order), then
/// place them in a grid of [`BOXES_PER_COLUMN`]-tall columns, stacking each table below
/// the previous one in its column by that table's actual (column-count-derived) height.
fn compute_initial_layout(tables: &[LayoutInput]) -> HashMap<String, Point<Pixels>> {
    let mut order: Vec<&LayoutInput> = tables.iter().collect();
    order.sort_by(|a, b| b.degree.cmp(&a.degree).then_with(|| a.key.cmp(&b.key)));

    let num_columns = order.len().div_ceil(BOXES_PER_COLUMN).max(1);
    let mut col_y = vec![MARGIN; num_columns];
    let mut positions = HashMap::with_capacity(order.len());

    for (ix, table) in order.into_iter().enumerate() {
        let col = ix / BOXES_PER_COLUMN;
        let x = MARGIN + col as f32 * (BOX_WIDTH + GAP_X);
        let y = col_y[col];
        positions.insert(table.key.clone(), point(px(x), px(y)));
        col_y[col] = y + box_height(table.column_count) + GAP_Y;
    }
    positions
}

/// The anchor point on `bounds`'s boundary nearest `toward`: the right-edge midpoint if
/// `toward` is at or past the box's horizontal center, else the left-edge midpoint.
/// Boxes only ever move horizontally relative to each other in the initial grid layout,
/// but this stays correct after a drag moves a box anywhere.
fn box_anchor(bounds: &Bounds<Pixels>, toward: Point<Pixels>) -> Point<Pixels> {
    let center = bounds.center();
    if toward.x >= center.x {
        point(bounds.right(), center.y)
    } else {
        point(bounds.left(), center.y)
    }
}

/// The pair of anchor points an FK line between `from` and `to` should connect — each
/// box's edge nearest the *other* box's center.
fn edge_anchors(from: Bounds<Pixels>, to: Bounds<Pixels>) -> (Point<Pixels>, Point<Pixels>) {
    let from_anchor = box_anchor(&from, to.center());
    let to_anchor = box_anchor(&to, from.center());
    (from_anchor, to_anchor)
}

/// The 4 corners (in fan-triangulation order — see [`gpui::Path::line_to`]'s doc
/// comment) of a thin quad running from `a` to `b`, `thickness` wide. gpui has no
/// line-stroke primitive reachable from a `canvas()`'s low-level paint API — only path
/// *fill* — so a "line" is really a filled rectangle: offset `a`/`b` by the half-width
/// perpendicular to the `a -> b` vector, both directions.
fn thin_quad(a: Point<Pixels>, b: Point<Pixels>, thickness: Pixels) -> [Point<Pixels>; 4] {
    let dx = f32::from(b.x) - f32::from(a.x);
    let dy = f32::from(b.y) - f32::from(a.y);
    let len = (dx * dx + dy * dy).sqrt().max(0.01);
    let half = f32::from(thickness) / 2.0;
    let nx = -dy / len * half;
    let ny = dx / len * half;
    [
        point(a.x + px(nx), a.y + px(ny)),
        point(b.x + px(nx), b.y + px(ny)),
        point(b.x - px(nx), b.y - px(ny)),
        point(a.x - px(nx), a.y - px(ny)),
    ]
}

/// Paint one FK line (as a filled thin quad — see [`thin_quad`]) from `from_key`'s box to
/// `to_key`'s, anchored at their nearest edges. `canvas_origin` translates the anchors
/// (computed in content-local space, matching where the table `div`s are positioned)
/// into window space, which is what `Window::paint_path` expects — the same
/// local-origin-plus-offset pattern `session.rs`'s PTY canvas uses for its glyph rows.
/// Silently no-ops if either table is missing its bounds (shouldn't happen — both keys
/// come from edges already filtered to known tables in [`DiagramView::new`] — but this
/// stays defensive rather than indexing and panicking).
fn paint_edge(
    canvas_origin: Point<Pixels>,
    table_bounds: &HashMap<String, Bounds<Pixels>>,
    from_key: &str,
    to_key: &str,
    color: u32,
    window: &mut Window,
) {
    let (Some(from), Some(to)) = (table_bounds.get(from_key), table_bounds.get(to_key)) else {
        return;
    };
    let (from_anchor, to_anchor) = edge_anchors(*from, *to);
    let a = canvas_origin + from_anchor;
    let b = canvas_origin + to_anchor;
    let corners = thin_quad(a, b, px(LINE_THICKNESS));

    let mut path = Path::new(corners[0]);
    path.line_to(corners[1]);
    path.line_to(corners[2]);
    path.line_to(corners[3]);
    window.paint_path(path, rgb(color));
}

/// One `∞`/`1` endpoint label, centered on `anchor` (a small fixed offset — these are
/// 1-character glyphs, not measured text, so a nudge reads close enough without a text
/// layout pass just to place a label).
fn edge_label(
    id: (&'static str, usize),
    anchor: Point<Pixels>,
    glyph: &'static str,
    color: u32,
) -> impl IntoElement + use<> {
    div()
        .id(id)
        .absolute()
        .left(px(f32::from(anchor.x) - 5.0))
        .top(px(f32::from(anchor.y) - 8.0))
        .text_xs()
        .text_color(rgb(color))
        .child(glyph)
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn input(key: &str, degree: usize, column_count: usize) -> LayoutInput {
        LayoutInput {
            key: key.to_string(),
            degree,
            column_count,
        }
    }

    #[test]
    fn higher_degree_tables_sort_first_and_land_at_the_top() {
        let inputs = vec![input("low", 0, 3), input("high", 5, 3)];
        let positions = compute_initial_layout(&inputs);
        assert_eq!(
            positions[&"high".to_string()],
            point(px(MARGIN), px(MARGIN))
        );
        assert!(positions[&"low".to_string()].y > positions[&"high".to_string()].y);
        // Same column (degree-sorted stack), not side by side.
        assert_eq!(
            positions[&"low".to_string()].x,
            positions[&"high".to_string()].x
        );
    }

    #[test]
    fn equal_degree_ties_break_by_key_ascending() {
        let inputs = vec![input("zebra", 1, 2), input("alpha", 1, 2)];
        let positions = compute_initial_layout(&inputs);
        assert!(positions[&"alpha".to_string()].y < positions[&"zebra".to_string()].y);
    }

    #[test]
    fn wraps_to_a_new_column_after_boxes_per_column() {
        let inputs: Vec<LayoutInput> = (0..=BOXES_PER_COLUMN)
            .map(|i| input(&format!("t{i}"), 0, 1))
            .collect();
        let positions = compute_initial_layout(&inputs);
        let wrapped = format!("t{BOXES_PER_COLUMN}");
        assert_eq!(positions[&wrapped].y, px(MARGIN));
        assert!(positions[&wrapped].x > positions[&"t0".to_string()].x);
    }
}

#[cfg(test)]
mod anchor_tests {
    use super::*;

    fn bounds_at(x: f32, y: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(BOX_WIDTH), px(60.0)),
        }
    }

    #[test]
    fn anchors_pick_the_facing_edges_when_boxes_sit_side_by_side() {
        let left_box = bounds_at(0.0, 0.0);
        let right_box = bounds_at(400.0, 0.0);
        let (from_anchor, to_anchor) = edge_anchors(left_box, right_box);
        assert_eq!(from_anchor.x, left_box.right());
        assert_eq!(to_anchor.x, right_box.left());
    }

    #[test]
    fn anchors_flip_when_the_order_of_the_boxes_flips() {
        let left_box = bounds_at(0.0, 0.0);
        let right_box = bounds_at(400.0, 0.0);
        // Same two boxes, called with `to` first this time — each box's anchor should
        // still be the edge nearest the *other* box, not a fixed "first arg" side.
        let (from_anchor, to_anchor) = edge_anchors(right_box, left_box);
        assert_eq!(from_anchor.x, right_box.left());
        assert_eq!(to_anchor.x, left_box.right());
    }

    #[test]
    fn anchor_y_is_the_boxs_vertical_center() {
        let a = bounds_at(0.0, 100.0);
        let b = bounds_at(400.0, 100.0);
        let (from_anchor, _) = edge_anchors(a, b);
        assert_eq!(from_anchor.y, a.center().y);
    }
}

#[cfg(test)]
mod click_vs_drag_tests {
    use super::*;

    /// Task 2's TDD target: movement at or under the threshold is a click, anything
    /// past it is a drag — including right at the boundary (an exact threshold-distance
    /// move should still count as a click per `is_click`'s `<=`).
    #[test]
    fn movement_under_threshold_is_a_click() {
        let down = point(px(100.0), px(100.0));
        let up = point(px(102.0), px(101.0));
        assert!(is_click(down, up, CLICK_DRAG_THRESHOLD_PX));
    }

    #[test]
    fn movement_over_threshold_is_a_drag() {
        let down = point(px(100.0), px(100.0));
        let up = point(px(110.0), px(100.0));
        assert!(!is_click(down, up, CLICK_DRAG_THRESHOLD_PX));
    }

    #[test]
    fn zero_movement_is_a_click() {
        let p = point(px(50.0), px(50.0));
        assert!(is_click(p, p, CLICK_DRAG_THRESHOLD_PX));
    }
}
