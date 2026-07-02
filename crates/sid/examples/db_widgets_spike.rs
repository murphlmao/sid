//! Wave 2, W1 — rendering spike for `gpui-component` (observation-gated, no unit tests).
//!
//! Proves `gpui-component` 0.5 adopts cleanly into `crates/sid` and that the two widgets
//! the real Database tab needs — a SQL-highlighted code editor (`InputState::code_editor`)
//! and a results `Table` — actually render, before W2-W6 wire them to `sid_db`/`Store`.
//!
//! Deliberately a standalone example, not a change to `main.rs`/`app.rs`: the concurrent
//! SSH track (Plan 3C) is editing those files right now, and this is throwaway
//! observation scaffolding, not product code. No blocking work happens in `render` — the
//! rows below are static, exactly like the brief's sketch; a real connection/query is W5.
//!
//! Run with `cargo run --example db_widgets_spike -p sid` (needs a Wayland/X11 display).

use gpui::{
    App, Application, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
};
use gpui_component::{
    ActiveTheme, Theme, ThemeMode,
    input::{Input, InputState},
    table::{Column, Table, TableDelegate, TableState},
};

/// A handful of static rows — enough to prove the table paints a header, striped rows,
/// and cell content. `sid_db::QueryPage` rows replace this in W5.
struct SpikeDelegate {
    columns: Vec<Column>,
    rows: Vec<Vec<String>>,
}

impl SpikeDelegate {
    fn new() -> Self {
        Self {
            columns: vec![
                Column::new("id", "id").width(px(60.)).sortable(),
                Column::new("name", "name").width(px(160.)),
                Column::new("email", "email").width(px(240.)),
            ],
            rows: vec![
                vec![
                    "1".to_string(),
                    "ada".to_string(),
                    "ada@example.com".to_string(),
                ],
                vec![
                    "2".to_string(),
                    "grace".to_string(),
                    "grace@example.com".to_string(),
                ],
                vec![
                    "3".to_string(),
                    "linus".to_string(),
                    "linus@example.com".to_string(),
                ],
            ],
        }
    }
}

impl TableDelegate for SpikeDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div().px_2().child(self.rows[row_ix][col_ix].clone())
    }
}

/// The spike view: a SQL code editor (tree-sitter highlighting) above a static results
/// table — the pairing W5 wires to a real connection + `query_paged` call.
struct SpikeView {
    sql: Entity<InputState>,
    table: Entity<TableState<SpikeDelegate>>,
}

impl SpikeView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // NOTE (observed, not chased further — see task-1-report.md): a multi-line
        // `default_value`/`set_value` seeded before the first paint only ever paints its
        // first line here (`text_wrapper.len()`/the visible range look correct by
        // inspection of gpui-component 0.5.1's own source, so this looks like a
        // paint-path quirk specific to programmatically-seeded text, not a fundamental
        // multi-line limitation) — interactive typing goes through a different
        // (incremental) edit path and was not re-tested here (no input-injection tool in
        // this sandbox). Single-line text renders correctly, as does the highlighting.
        let sql = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("sql")
                .line_number(true)
                .rows(8)
                .default_value("select id, name, email\nfrom users\nwhere id = 1;\n")
        });
        let table = cx.new(|cx| TableState::new(SpikeDelegate::new(), window, cx));
        Self { sql, table }
    }
}

impl Render for SpikeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap_3()
            .p_4()
            .bg(rgb(0x161618))
            .text_color(rgb(0xdcdce0))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x8a8a90))
                    .child("gpui-component spike — SQL editor + results table (Wave 2, W1)"),
            )
            .child(
                // `Input` in multi-line/code-editor mode lays out at `height: relative(1.)`
                // (see gpui-component's `element.rs::request_layout`) — it fills whatever
                // height its parent gives it rather than sizing itself from `.rows(..)`, so
                // the wrapper needs an explicit height or the editor collapses to one line.
                div()
                    .h(px(180.))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x2c2c30))
                    .bg(cx.theme().background)
                    .child(Input::new(&self.sql)),
            )
            .child(
                div()
                    .h(px(220.))
                    .w_full()
                    .child(Table::new(&self.table).stripe(true)),
            )
    }
}

fn main() {
    Application::new()
        .with_assets(gpui_component_assets::Assets)
        .run(|cx| {
            gpui_component::init(cx);
            let bounds = Bounds::centered(None, size(px(720.), px(560.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    Theme::change(ThemeMode::Dark, Some(window), cx);
                    // `gpui-component`'s `Input`/`Table` reach for a `gpui_component::Root`
                    // ancestor at render time (confirmed empirically: without it, rendering
                    // panics at `root.rs`'s `window.root::<Root>().expect(..)`), so the
                    // window's first layer must be `Root`, not `SpikeView` directly.
                    let view = cx.new(|cx| SpikeView::new(window, cx));
                    cx.new(|cx| gpui_component::Root::new(view, window, cx))
                },
            )
            .unwrap();
            cx.activate(true);
        });
}
