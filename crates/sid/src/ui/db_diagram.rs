//! The Access-style "Relationships" diagram — a pop-out OS window opened from the
//! Database tab's "⧉ diagram" button ([`crate::ui::db_tab::AppState::open_diagram_window`]).
//!
//! [`DiagramView`] is handed an owned snapshot of the active connection's cached
//! [`SchemaInfo`] + [`SchemaGraph`] at open time (see that method's doc comment for why
//! a snapshot is fine for v1) and is otherwise fully self-contained: table boxes laid
//! out on a scrollable canvas with FK lines drawn under them and `1`/`∞` endpoint labels
//! — all pure sync render-from-state, no runtime/async of its own. Drag-to-reposition
//! and selection highlighting land in a follow-up commit.

use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, Bounds, Context, FontWeight, IntoElement, Path, Pixels, Point, Render,
    SharedString, Window, canvas, div, point, prelude::*, px, rgb, size,
};
use sid_core::db::{SchemaGraph, SchemaInfo};

use crate::ui::db_tab::table_display_name;

// ---- palette (kept local — see `db_tab.rs`'s convention for why) --------------------------

const BG: u32 = 0x161618;
const BORDER: u32 = 0x2c2c30;
const FG: u32 = 0xdcdce0;
const FG_DIM: u32 = 0x8a8a90;
const BOX_BG: u32 = 0x1c1c20;
const HEADER_BG: u32 = 0x232327;
const BRAND: u32 = 0x5a9ad0;
const FK_TINT: u32 = 0x8bb0d0;
const EDGE_DIM: u32 = 0x3a3a40;

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

/// The relationships diagram's whole state. Renders from `tables`/`edges`/`positions`
/// alone — a follow-up commit adds dragging, which will just mutate `positions` and call
/// `cx.notify()`; the lines already follow because line endpoints are recomputed from
/// `positions` on every render.
pub struct DiagramView {
    tables: Vec<DiagramTable>,
    edges: Vec<DiagramEdge>,
    positions: HashMap<String, Point<Pixels>>,
}

impl DiagramView {
    /// Build the diagram from a snapshot of the active connection's schema + graph.
    /// Edges naming a table absent from `schema` are dropped silently (defensive — the
    /// backend contract doesn't guarantee it, and a dangling edge would panic the
    /// anchor-picking geometry). Self-referencing edges are kept (as box badges, not
    /// lines) rather than dropped.
    pub fn new(schema: SchemaInfo, graph: SchemaGraph) -> Self {
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

    /// The header bar: table/relationship counts, and — when the backend hasn't wired
    /// up `schema_graph` yet (or the engine genuinely has no FKs) — a subtle hint
    /// rather than a diagram that silently looks broken.
    fn header(&self) -> impl IntoElement + use<> {
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
            .border_color(rgb(BORDER))
            .child(div().text_sm().text_color(rgb(FG_DIM)).child(summary))
            .children(hint.map(|h| div().text_xs().text_color(rgb(FG_DIM)).child(h)))
    }

    /// The FK-lines layer. A `canvas()` sized to the scrollable content (see
    /// `session.rs`'s PTY grid for the same low-level-paint pattern) painted *before*
    /// the table boxes in child order, so lines sit under them. A follow-up commit adds
    /// a selection-driven bright pass on top of this dim one.
    fn edges_canvas(&self, content_size: (f32, f32)) -> impl IntoElement + use<> {
        let table_bounds = self.table_bounds();
        let edges: Vec<(String, String)> = self
            .edges
            .iter()
            .filter(|e| !e.self_ref)
            .map(|e| (e.from_table.clone(), e.to_table.clone()))
            .collect();

        canvas(
            move |_bounds, _window, _cx| {},
            move |bounds, (), window, _cx| {
                for (from_key, to_key) in &edges {
                    paint_edge(
                        bounds.origin,
                        &table_bounds,
                        from_key,
                        to_key,
                        EDGE_DIM,
                        window,
                    );
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
    fn edge_labels(&self) -> Vec<AnyElement> {
        let table_bounds = self.table_bounds();
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
                    edge_label(("diagram-edge-many", ix), from_anchor, "∞").into_any_element(),
                    edge_label(("diagram-edge-one", ix), to_anchor, "1").into_any_element(),
                ]
            })
            .collect()
    }

    /// One table's box: a header (optional self-ref badge; becomes the drag handle in a
    /// follow-up commit) over a capped column list (🔑 prefix on PK columns, tinted on
    /// FK columns, `+N more` once the list exceeds [`MAX_VISIBLE_COLUMNS`]).
    fn table_box(&self, ix: usize, table: &DiagramTable) -> impl IntoElement + use<> {
        let pos = self.positions.get(&table.key).copied().unwrap_or_default();
        let height = box_height(table.columns.len());
        let self_ref_count = self.self_ref_count(&table.key);

        let header = div()
            .id(("diagram-box-header", ix))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_1()
            .px_2()
            .py_1()
            .bg(rgb(HEADER_BG))
            .rounded_t_md()
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::BOLD)
                    .text_sm()
                    .text_color(rgb(FG))
                    .child(table.key.clone()),
            )
            .children((self_ref_count > 0).then(|| {
                div()
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child(format!("↺ {self_ref_count}"))
            }));

        let visible = table
            .columns
            .iter()
            .take(MAX_VISIBLE_COLUMNS)
            .enumerate()
            .map(|(cix, col)| {
                let is_pk = table.pk.contains(col);
                let is_fk = table.fk_columns.contains(col);
                let label = if is_pk {
                    format!("🔑 {col}")
                } else {
                    col.clone()
                };
                div()
                    .id(("diagram-box-col", ix * 1000 + cix))
                    .px_2()
                    .text_xs()
                    .text_color(rgb(if is_fk { FK_TINT } else { FG_DIM }))
                    .child(label)
                    .into_any_element()
            });
        let overflow = (table.columns.len() > MAX_VISIBLE_COLUMNS).then(|| {
            let more = table.columns.len() - MAX_VISIBLE_COLUMNS;
            div()
                .px_2()
                .text_xs()
                .text_color(rgb(FG_DIM))
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
            .bg(rgb(BOX_BG))
            .border_1()
            .border_color(rgb(BORDER))
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
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let content_size = self.content_size();
        let boxes = self
            .tables
            .iter()
            .enumerate()
            .map(|(ix, table)| self.table_box(ix, table));

        let content = div()
            .id("diagram-content")
            .relative()
            .w(px(content_size.0))
            .h(px(content_size.1))
            .child(self.edges_canvas(content_size))
            .children(boxes)
            .children(self.edge_labels());

        div()
            .id("diagram-root")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .child(self.header())
            .child(
                div()
                    .id("diagram-scroll")
                    .flex_1()
                    .overflow_scroll()
                    .child(content),
            )
    }
}

// ---- pure geometry helpers (unit-tested — no `Window`/`Context` needed) --------------------

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
) -> impl IntoElement + use<> {
    div()
        .id(id)
        .absolute()
        .left(px(f32::from(anchor.x) - 5.0))
        .top(px(f32::from(anchor.y) - 8.0))
        .text_xs()
        .text_color(rgb(BRAND))
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
