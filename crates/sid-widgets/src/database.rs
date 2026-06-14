//! Database tab widget. Pure state in `DatabaseState`; rendering is the
//! binary's draw() responsibility.
//!
//! The widget mirrors the Plan-2 Workspaces pattern: testable in isolation
//! plus a thin render layer (a "coming soon" body for now — the real ratatui
//! drawing lands in the binary's wire layer along with the rest of the
//! Database tab UI).
//!
//! Right-pane sub-views are an enum: [`RightPane::Editor`] (default),
//! [`RightPane::Results`], [`RightPane::History`]. `Tab` cycles between them.

use std::{path::PathBuf, sync::Arc};

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row as TableRow, Table},
};
use sid_core::{
    adapters::db_client::{DbClient, DbKind, PageCursor, QueryPage},
    context::WidgetCtx,
    event::Event,
    widget::{EventOutcome, FooterHint, RenderTarget, Widget, WidgetId},
};
use sid_db_clients::lexer::{Token, tokenize};
use sid_store::{DbConnection, QueryRecord};
use sid_ui::{Theme, themes::cosmos};

use crate::{
    list_cursor::{CursorTarget, ListCursor},
    stub::ComingSoonBody,
};

/// Which right-pane sub-view is currently focused.
///
/// # Examples
///
/// ```
/// use sid_widgets::database::RightPane;
/// assert_ne!(RightPane::Editor, RightPane::Results);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RightPane {
    /// Multi-line SQL query editor.
    Editor,
    /// Paginated, sortable result table.
    Results,
    /// Per-connection query history.
    History,
}

/// Which pane in the Database tab currently owns keyboard input.
///
/// Distinct from [`RightPane`]: `RightPane` controls what the right side
/// **displays** (Editor / Results / History) while `DbFocus` controls
/// **where input goes** (the left connection list, or one of the three
/// right-pane sub-views).
///
/// `Tab` cycles 4-way: `Connections → Editor → Results → History → Connections`.
/// When focus advances to one of `Editor/Results/History` the right pane is
/// also switched to that display so the focused area is what the user sees.
///
/// # Examples
///
/// ```
/// use sid_widgets::database::DbFocus;
/// assert_eq!(DbFocus::default(), DbFocus::Connections);
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DbFocus {
    /// Left-hand connection list.
    #[default]
    Connections,
    /// Right-hand SQL editor.
    Editor,
    /// Right-hand results table.
    Results,
    /// Right-hand query history.
    History,
}

impl DbFocus {
    /// Cycle forward (Tab).
    pub fn next(self) -> Self {
        match self {
            DbFocus::Connections => DbFocus::Editor,
            DbFocus::Editor => DbFocus::Results,
            DbFocus::Results => DbFocus::History,
            DbFocus::History => DbFocus::Connections,
        }
    }
    /// Cycle backward (Shift+Tab).
    pub fn prev(self) -> Self {
        match self {
            DbFocus::Connections => DbFocus::History,
            DbFocus::Editor => DbFocus::Connections,
            DbFocus::Results => DbFocus::Editor,
            DbFocus::History => DbFocus::Results,
        }
    }
}

/// One-shot commands the widget asks the App to perform. The wire layer
/// translates these into JobQueue spawns and routes the result back via the
/// widget's `apply_*` setters.
///
/// # Examples
///
/// ```
/// use sid_widgets::database::DbCommand;
/// let _ = DbCommand::Disconnect;
/// ```
#[derive(Clone, Debug)]
pub enum DbCommand {
    /// Open a connection.
    Connect {
        /// Saved connection id.
        conn_id: String,
    },
    /// Close the active connection.
    Disconnect,
    /// Run a SQL statement against the active connection.
    RunQuery {
        /// Verbatim SQL.
        sql: String,
        /// Connection the query targets.
        conn_id: String,
    },
    /// Reload history for the active connection.
    LoadHistory {
        /// Connection id.
        conn_id: String,
    },
    /// Load the next page of an in-flight result set.
    LoadNextPage {
        /// Verbatim SQL of the original query.
        sql: String,
        /// Connection id.
        conn_id: String,
        /// Cursor returned by the previous page.
        cursor: PageCursor,
    },
    /// Copy a cell value to the system clipboard.
    CopyCell(String),
    /// Export the currently-loaded result set to a CSV file.
    ExportCsv {
        /// Filesystem destination.
        path: PathBuf,
    },
    /// Open the add/edit connection form. `prefill` is `Some` when editing an
    /// existing connection, `None` when creating a new one.
    OpenConnectionForm {
        /// Existing connection to edit, or `None` to create a new connection.
        prefill: Option<sid_store::DbConnection>,
    },
    /// Test the named connection through `DbClient::open` off-thread via the
    /// job queue. Result surfaces as a toast.
    TestConnection {
        /// Id of the connection to test.
        conn_id: String,
    },
}

/// Editor sub-view state. Multi-line; cursor in (line, column).
#[derive(Default)]
pub struct EditorState {
    /// Lines of source. Always has at least one element after
    /// [`EditorState::default_blank`].
    pub lines: Vec<String>,
    /// Cursor line index (0-based).
    pub cursor_line: usize,
    /// Cursor character column (0-based, character offset within the line).
    pub cursor_col: usize,
}

impl EditorState {
    /// Construct an editor with one empty line and cursor at origin.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::database::EditorState;
    /// let e = EditorState::default_blank();
    /// assert_eq!(e.lines, vec![String::new()]);
    /// ```
    pub fn default_blank() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
        }
    }

    /// Insert one character at the cursor and advance.
    pub fn insert_char(&mut self, c: char) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        let line = &mut self.lines[self.cursor_line];
        let byte_idx = char_byte_offset(line, self.cursor_col);
        line.insert(byte_idx, c);
        self.cursor_col += 1;
    }

    /// Insert a line break at the cursor.
    pub fn insert_newline(&mut self) {
        let line = self.lines.remove(self.cursor_line);
        let byte_idx = char_byte_offset(&line, self.cursor_col);
        let (a, b) = line.split_at(byte_idx);
        self.lines.insert(self.cursor_line, a.to_string());
        self.lines.insert(self.cursor_line + 1, b.to_string());
        self.cursor_line += 1;
        self.cursor_col = 0;
    }

    /// Delete the character before the cursor; join lines if at column 0.
    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let line = &mut self.lines[self.cursor_line];
            let byte_idx = char_byte_offset(line, self.cursor_col);
            let prev = line[..byte_idx]
                .chars()
                .next_back()
                .map(|c| c.len_utf8())
                .unwrap_or(1);
            line.replace_range(byte_idx - prev..byte_idx, "");
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            let removed = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].chars().count();
            self.lines[self.cursor_line].push_str(&removed);
        }
    }

    /// Reposition the cursor, clamping to valid coordinates.
    pub fn move_cursor_to(&mut self, line: usize, col: usize) {
        self.cursor_line = line.min(self.lines.len().saturating_sub(1));
        let line_chars = self.lines[self.cursor_line].chars().count();
        self.cursor_col = col.min(line_chars);
    }

    /// Concatenate all lines with `\n`.
    pub fn full_source(&self) -> String {
        self.lines.join("\n")
    }

    /// Tokenise the current source for syntax highlighting.
    pub fn tokens(&self) -> Vec<Token> {
        tokenize(&self.full_source())
    }
}

/// Results sub-view state.
#[derive(Default)]
pub struct ResultsState {
    /// Currently displayed result page (`None` if no query has run).
    pub page: Option<QueryPage>,
    /// Selected row index within `page.rows`.
    pub selected_row: usize,
    /// Selected column index within `page.columns`.
    pub selected_col: usize,
    /// Currently-sorted column, if any.
    pub sort_col: Option<usize>,
    /// Sort direction (true = ascending).
    pub sort_asc: bool,
}

impl ResultsState {
    /// Replace the page and reset selection.
    pub fn set_page(&mut self, p: QueryPage) {
        self.page = Some(p);
        self.selected_row = 0;
        self.selected_col = 0;
    }

    /// Append rows from a follow-on page (preserving the cursor).
    pub fn append_page(&mut self, p: QueryPage) {
        if let Some(existing) = self.page.as_mut() {
            existing.rows.extend(p.rows);
            existing.next_cursor = p.next_cursor;
        } else {
            self.set_page(p);
        }
    }

    /// Move selection to the next row (wraps).
    pub fn select_next_row(&mut self) {
        if let Some(p) = &self.page
            && !p.rows.is_empty()
        {
            self.selected_row = (self.selected_row + 1) % p.rows.len();
        }
    }

    /// Move selection to the previous row (wraps).
    pub fn select_prev_row(&mut self) {
        if let Some(p) = &self.page
            && !p.rows.is_empty()
        {
            self.selected_row = (self.selected_row + p.rows.len() - 1) % p.rows.len();
        }
    }

    /// Move selection to the next column (wraps).
    pub fn select_next_col(&mut self) {
        if let Some(p) = &self.page
            && !p.columns.is_empty()
        {
            self.selected_col = (self.selected_col + 1) % p.columns.len();
        }
    }

    /// Move selection to the previous column (wraps).
    pub fn select_prev_col(&mut self) {
        if let Some(p) = &self.page
            && !p.columns.is_empty()
        {
            self.selected_col = (self.selected_col + p.columns.len() - 1) % p.columns.len();
        }
    }

    /// Borrow the currently-selected cell value, if any.
    pub fn selected_cell(&self) -> Option<&str> {
        let p = self.page.as_ref()?;
        p.rows
            .get(self.selected_row)?
            .values
            .get(self.selected_col)
            .map(|s| s.as_str())
    }

    /// Sort the loaded page by `col`. Toggles direction if already sorted on
    /// the same column.
    pub fn toggle_sort(&mut self, col: usize) {
        if self.sort_col == Some(col) {
            self.sort_asc = !self.sort_asc;
        } else {
            self.sort_col = Some(col);
            self.sort_asc = true;
        }
        if let Some(p) = self.page.as_mut() {
            p.rows.sort_by(|a, b| {
                let sa = a.values.get(col).map(String::as_str).unwrap_or("");
                let sb = b.values.get(col).map(String::as_str).unwrap_or("");
                match (sa.parse::<f64>(), sb.parse::<f64>()) {
                    (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                    _ => sa.cmp(sb),
                }
            });
            if !self.sort_asc {
                p.rows.reverse();
            }
        }
    }
}

/// History sub-view state.
#[derive(Default)]
pub struct HistoryState {
    /// Per-connection query records, newest first.
    pub records: Vec<QueryRecord>,
    /// Selected row index.
    pub selected: usize,
}

impl HistoryState {
    /// Replace the records and reset selection.
    pub fn set_records(&mut self, records: Vec<QueryRecord>) {
        self.records = records;
        self.selected = 0;
    }

    /// Borrow the currently-selected record.
    pub fn selected_record(&self) -> Option<&QueryRecord> {
        self.records.get(self.selected)
    }

    /// Advance selection (wraps).
    pub fn select_next(&mut self) {
        if self.records.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.records.len();
    }

    /// Reverse selection (wraps).
    pub fn select_prev(&mut self) {
        if self.records.is_empty() {
            return;
        }
        self.selected = (self.selected + self.records.len() - 1) % self.records.len();
    }
}

/// Pure-state portion of [`DatabaseWidget`]. Testable without ratatui.
pub struct DatabaseState {
    connections: Vec<DbConnection>,
    /// Cursor over the connections list including the optional +add new row.
    pub cursor: ListCursor,
    active_client: Option<Arc<dyn DbClient>>,
    active_conn_id: Option<String>,
    right_pane: RightPane,
    /// Multi-line SQL editor state.
    pub editor: EditorState,
    /// Paginated results state.
    pub results: ResultsState,
    /// Query history state.
    pub history: HistoryState,
    pending: Vec<DbCommand>,
}

impl DatabaseState {
    /// Construct a new state with the given connection list. First connection
    /// is selected by default. The `+add new` row is shown by default.
    pub fn new(connections: Vec<DbConnection>) -> Self {
        Self::new_with_add_new(connections, true)
    }

    /// Create state. `add_new` mirrors the `show_add_new_row` setting.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::database::DatabaseState;
    /// let state = DatabaseState::new_with_add_new(vec![], true);
    /// assert!(state.cursor.add_new);
    /// ```
    pub fn new_with_add_new(connections: Vec<DbConnection>, add_new: bool) -> Self {
        let len = connections.len();
        Self {
            cursor: ListCursor::new(len, add_new, 0),
            connections,
            active_client: None,
            active_conn_id: None,
            right_pane: RightPane::Editor,
            editor: EditorState::default_blank(),
            results: ResultsState::default(),
            history: HistoryState::default(),
            pending: Vec::new(),
        }
    }

    /// Borrow the saved connection list.
    pub fn connections(&self) -> &[DbConnection] {
        &self.connections
    }

    /// Currently-selected connection (`None` if cursor is on +add new or list is empty).
    pub fn selected_connection(&self) -> Option<&DbConnection> {
        match self.cursor.target() {
            CursorTarget::Item(i) => self.connections.get(i),
            CursorTarget::AddNew | CursorTarget::Nothing => None,
        }
    }

    /// True when the cursor sits on the synthetic +add new row.
    pub fn is_add_new_selected(&self) -> bool {
        matches!(self.cursor.target(), CursorTarget::AddNew)
    }

    /// Replace the connection list (e.g., after a CLI add/remove), preserving
    /// cursor position where valid.
    pub fn set_connections(&mut self, c: Vec<DbConnection>, add_new: bool) {
        let new_len = c.len();
        self.connections = c;
        let old_pos = self.cursor.pos;
        self.cursor = ListCursor::new(new_len, add_new, old_pos);
    }

    /// Which right-pane sub-view is focused.
    pub fn right_pane(&self) -> RightPane {
        self.right_pane
    }

    /// Set the right-pane sub-view.
    pub fn set_right_pane(&mut self, p: RightPane) {
        self.right_pane = p;
    }

    /// Cycle Editor → Results → History → Editor.
    pub fn cycle_right_pane(&mut self) {
        self.right_pane = match self.right_pane {
            RightPane::Editor => RightPane::Results,
            RightPane::Results => RightPane::History,
            RightPane::History => RightPane::Editor,
        };
    }

    /// Id of the active connection, if any.
    pub fn active_conn_id(&self) -> Option<&str> {
        self.active_conn_id.as_deref()
    }

    /// Active client, if any.
    pub fn active_client(&self) -> Option<&Arc<dyn DbClient>> {
        self.active_client.as_ref()
    }

    /// Bind a freshly-opened client.
    pub fn apply_connect_result(&mut self, conn_id: String, client: Arc<dyn DbClient>) {
        self.active_conn_id = Some(conn_id);
        self.active_client = Some(client);
    }

    /// Mark a connection as active without binding a client. Used by tests
    /// (and CLI smoke flows) that need to drive renderer state without
    /// spinning up a real driver.
    pub fn set_active_conn_id_for_tests(&mut self, conn_id: String) {
        self.active_conn_id = Some(conn_id);
    }

    /// Clear the active client (after a disconnect).
    pub fn clear_active(&mut self) {
        self.active_conn_id = None;
        self.active_client = None;
    }

    /// Apply the result of a RunQuery command.
    pub fn apply_query_result(&mut self, page: QueryPage, record: Option<QueryRecord>) {
        self.results.set_page(page);
        self.right_pane = RightPane::Results;
        if let Some(r) = record {
            self.history.records.insert(0, r);
        }
    }

    /// Apply a follow-on page from a LoadNextPage command.
    pub fn apply_next_page(&mut self, page: QueryPage) {
        self.results.append_page(page);
    }

    /// Replace the loaded history records.
    pub fn apply_history(&mut self, records: Vec<QueryRecord>) {
        self.history.set_records(records);
    }

    /// Advance the connection-list selection (wraps from last item back to top).
    pub fn select_next(&mut self) {
        let total = self.cursor.total();
        if total == 0 {
            return;
        }
        self.cursor.pos = (self.cursor.pos + 1) % total;
    }

    /// Reverse the connection-list selection (wraps from top back to last item).
    pub fn select_prev(&mut self) {
        let total = self.cursor.total();
        if total == 0 {
            return;
        }
        self.cursor.pos = (self.cursor.pos + total - 1) % total;
    }

    /// Drain pending commands the renderer should hand to the App.
    pub fn drain_commands(&mut self) -> Vec<DbCommand> {
        std::mem::take(&mut self.pending)
    }

    /// Push a command (used by the widget's event handler).
    pub fn push_command(&mut self, c: DbCommand) {
        self.pending.push(c);
    }
}

/// Public widget wrapper. Pure-state portion is in [`DatabaseState`].
pub struct DatabaseWidget {
    state: DatabaseState,
    id: WidgetId,
    body: ComingSoonBody,
    focused_pane: DbFocus,
}

impl DatabaseWidget {
    /// Construct a new widget with the given connection list.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::DatabaseWidget;
    /// let w = DatabaseWidget::new(vec![]);
    /// assert_eq!(sid_core::widget::Widget::id(&w).as_str(), "database.root");
    /// ```
    pub fn new(connections: Vec<DbConnection>) -> Self {
        Self::new_with_add_new(connections, true)
    }

    /// Construct a new widget with the given connection list and explicit
    /// `add_new` flag governing whether the `+add new` synthetic row appears.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::database::DatabaseWidget;
    /// let w = DatabaseWidget::new_with_add_new(vec![], true);
    /// assert!(w.state().cursor.add_new);
    /// ```
    pub fn new_with_add_new(connections: Vec<DbConnection>, add_new: bool) -> Self {
        Self {
            state: DatabaseState::new_with_add_new(connections, add_new),
            id: WidgetId::new("database.root"),
            body: ComingSoonBody::new(
                "Database",
                "Postgres + SQLite query runner — Plan 4 wires the editor + results table in a follow-up.",
            ),
            focused_pane: DbFocus::default(),
        }
    }

    /// Currently-focused pane.
    pub fn focused_pane(&self) -> DbFocus {
        self.focused_pane
    }

    /// Stable string label for the focused pane.
    pub fn focused_pane_label(&self) -> &'static str {
        match self.focused_pane {
            DbFocus::Connections => "Connections",
            DbFocus::Editor => "Editor",
            DbFocus::Results => "Results",
            DbFocus::History => "History",
        }
    }

    /// Cycle focus forward. Synchronises [`RightPane`] when focus lands on
    /// one of the right-pane sub-views.
    pub fn focus_next(&mut self) {
        self.focused_pane = self.focused_pane.next();
        self.sync_right_pane_with_focus();
    }

    /// Cycle focus backward. Synchronises [`RightPane`] when focus lands on
    /// one of the right-pane sub-views.
    pub fn focus_prev(&mut self) {
        self.focused_pane = self.focused_pane.prev();
        self.sync_right_pane_with_focus();
    }

    /// Focus the pane that contains the given coordinate. No-op when the
    /// coordinate falls outside `area`.
    ///
    /// Layout mirrors [`Self::render_into_frame`]: a 30/70 horizontal split.
    /// The left 30% focuses [`DbFocus::Connections`]. On the right, the top
    /// 30% of the vertical span focuses [`DbFocus::Editor`]; the remainder
    /// (excluding the bottom status row) focuses whichever of
    /// [`DbFocus::Results`] / [`DbFocus::History`] mirrors the currently
    /// active [`RightPane`]. The status row at the very bottom is a no-op
    /// click target.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::layout::Rect;
    /// use sid_widgets::DatabaseWidget;
    /// use sid_widgets::database::DbFocus;
    /// let mut w = DatabaseWidget::new(vec![]);
    /// let area = Rect { x: 0, y: 0, width: 100, height: 40 };
    /// // Left column → Connections.
    /// w.focus_at(area, 5, 5);
    /// assert_eq!(w.focused_pane(), DbFocus::Connections);
    /// // Top-right → Editor.
    /// w.focus_at(area, 80, 2);
    /// assert_eq!(w.focused_pane(), DbFocus::Editor);
    /// ```
    pub fn focus_at(&mut self, area: Rect, col: u16, row: u16) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        if col < area.x || col >= area.x.saturating_add(area.width) {
            return;
        }
        if row < area.y || row >= area.y.saturating_add(area.height) {
            return;
        }
        let split_col = area.x.saturating_add(area.width.saturating_mul(30) / 100);
        if col < split_col {
            self.focused_pane = DbFocus::Connections;
            return;
        }
        // Right side: top 30% is editor, then results/history, then a 1-row
        // status. The 1-row status at the very bottom is a no-op click.
        let editor_h = area.height.saturating_mul(30) / 100;
        let editor_end_row = area.y.saturating_add(editor_h);
        let status_row = area.y.saturating_add(area.height).saturating_sub(1);
        if row < editor_end_row {
            self.focused_pane = DbFocus::Editor;
        } else if row >= status_row {
            // Status row: ignore (return without mutation).
            return;
        } else {
            // Middle pane: mirror the currently-visible RightPane so the
            // focus reflects what the user actually clicked on.
            self.focused_pane = match self.state.right_pane() {
                RightPane::History => DbFocus::History,
                _ => DbFocus::Results,
            };
        }
        self.sync_right_pane_with_focus();
    }

    fn sync_right_pane_with_focus(&mut self) {
        match self.focused_pane {
            DbFocus::Editor => self.state.set_right_pane(RightPane::Editor),
            DbFocus::Results => self.state.set_right_pane(RightPane::Results),
            DbFocus::History => self.state.set_right_pane(RightPane::History),
            DbFocus::Connections => {}
        }
    }

    /// Borrow the inner state.
    pub fn state(&self) -> &DatabaseState {
        &self.state
    }

    /// Borrow the inner state mutably.
    pub fn state_mut(&mut self) -> &mut DatabaseState {
        &mut self.state
    }

    /// Replace the loaded result page wholesale. Test-only helper used to
    /// drive snapshot tests without going through the JobQueue.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::adapters::db_client::QueryPage;
    /// use sid_widgets::DatabaseWidget;
    /// let mut w = DatabaseWidget::new(vec![]);
    /// w.set_results_for_tests(QueryPage {
    ///     columns: vec![],
    ///     rows: vec![],
    ///     next_cursor: None,
    ///     duration_ms: 0,
    /// });
    /// assert!(w.state().results.page.is_some());
    /// ```
    pub fn set_results_for_tests(&mut self, page: QueryPage) {
        self.state.results.set_page(page);
    }

    /// Render the widget into a ratatui [`Frame`]. Mirrors the structure of
    /// `NetworkWidget::render_into_frame`; used by the binary's wire layer
    /// and by the insta snapshot tests in `tests/database_render.rs`.
    ///
    /// Layout:
    ///
    /// ```text
    /// ┌─────────────┬────────────────────────────────┐
    /// │ Connections │ Editor                         │
    /// │             ├────────────────────────────────┤
    /// │             │ Results / History              │
    /// │             ├────────────────────────────────┤
    /// │             │ status                         │
    /// └─────────────┴────────────────────────────────┘
    /// ```
    pub fn render_into_frame(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let outer = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Min(0)])
            .split(area);
        let left = outer[0];
        let right_split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(30),
                Constraint::Min(0),
                Constraint::Length(1),
            ])
            .split(outer[1]);
        let editor_rect = right_split[0];
        let middle_rect = right_split[1];
        let status_rect = right_split[2];

        self.render_connection_list(frame, left, theme);
        self.render_editor(frame, editor_rect, theme);
        match self.state.right_pane {
            RightPane::History => self.render_history(frame, middle_rect, theme),
            RightPane::Editor | RightPane::Results => {
                self.render_results(frame, middle_rect, theme);
            }
        }
        self.render_status_bar(frame, status_rect, theme);
    }

    fn render_connection_list(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let focused = self.focused_pane == DbFocus::Connections;
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let mut title_style = Style::default().fg(theme.foreground.into());
        if focused {
            title_style = title_style.add_modifier(Modifier::BOLD);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(" Connections ")
            .title_style(title_style);

        let active = self.state.active_conn_id.as_deref();
        let mut lines: Vec<Line<'_>> = Vec::new();

        // +add new synthetic row
        if self.state.cursor.add_new {
            let add_new_selected = matches!(self.state.cursor.target(), CursorTarget::AddNew);
            let style = if add_new_selected {
                Style::default()
                    .fg(theme.accent_primary.into())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.accent_success.into())
            };
            let marker = if add_new_selected { '>' } else { ' ' };
            lines.push(Line::from(Span::styled(
                format!("{marker} + add new"),
                style,
            )));
        }

        if self.state.connections.is_empty() && !self.state.cursor.add_new {
            let msg = "(no connections — `sid db add` to register)";
            lines.push(Line::from(Span::styled(
                msg,
                Style::default().fg(theme.muted.into()),
            )));
        }

        for (i, c) in self.state.connections.iter().enumerate() {
            let selected = matches!(self.state.cursor.target(), CursorTarget::Item(j) if j == i);
            let dot = if Some(c.id.as_str()) == active {
                '*'
            } else {
                'o'
            };
            let marker = if selected { '>' } else { ' ' };
            let kind = match c.kind {
                DbKind::Postgres => "postgres",
                DbKind::Sqlite => "sqlite",
            };
            let text = format!("{marker} {dot} {} ({kind})", c.name);
            let style = if selected {
                Style::default()
                    .fg(theme.background.into())
                    .bg(theme.accent_primary.into())
            } else {
                Style::default().fg(theme.foreground.into())
            };
            lines.push(Line::from(Span::styled(text, style)));
        }
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }

    fn render_editor(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let focused = self.focused_pane == DbFocus::Editor;
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let mut title_style = Style::default().fg(theme.foreground.into());
        if focused {
            title_style = title_style.add_modifier(Modifier::BOLD);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(" SQL ")
            .title_style(title_style);

        let cursor_line = self.state.editor.cursor_line;
        let cursor_col = self.state.editor.cursor_col;
        let lines: Vec<Line<'_>> = self
            .state
            .editor
            .lines
            .iter()
            .enumerate()
            .map(|(li, line)| {
                if li == cursor_line {
                    let chars: Vec<char> = line.chars().collect();
                    let col = cursor_col.min(chars.len());
                    let before: String = chars[..col].iter().collect();
                    let at: String = if col < chars.len() {
                        chars[col].to_string()
                    } else {
                        " ".into()
                    };
                    let after: String = if col < chars.len() {
                        chars[col + 1..].iter().collect()
                    } else {
                        String::new()
                    };
                    let cursor_style = Style::default()
                        .fg(theme.background.into())
                        .bg(theme.accent_primary.into())
                        .add_modifier(Modifier::REVERSED);
                    Line::from(vec![
                        Span::styled(before, Style::default().fg(theme.foreground.into())),
                        Span::styled(at, cursor_style),
                        Span::styled(after, Style::default().fg(theme.foreground.into())),
                    ])
                } else {
                    Line::from(Span::styled(
                        line.clone(),
                        Style::default().fg(theme.foreground.into()),
                    ))
                }
            })
            .collect();
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }

    fn render_results(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let focused = self.focused_pane == DbFocus::Results;
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };

        let (rows_len, page_title) = match self.state.results.page.as_ref() {
            Some(p) => {
                let total = p.rows.len();
                let next = p
                    .next_cursor
                    .map(|c| format!(" · next-offset {}", c.offset))
                    .unwrap_or_default();
                (total, format!(" rows {total}{next} "))
            }
            None => (0, " no results yet ".to_string()),
        };
        let title = format!(" Results ·{page_title}");
        let mut title_style = Style::default().fg(theme.foreground.into());
        if focused {
            title_style = title_style.add_modifier(Modifier::BOLD);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(title)
            .title_style(title_style);

        let Some(page) = self.state.results.page.as_ref() else {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "(no query has run yet — Ctrl+R to execute)",
                    Style::default().fg(theme.muted.into()),
                )))
                .block(block),
                rect,
            );
            return;
        };

        let header_cells: Vec<String> = page.columns.iter().map(|c| c.name.clone()).collect();
        let header = TableRow::new(header_cells).style(
            Style::default()
                .fg(theme.muted.into())
                .add_modifier(Modifier::BOLD),
        );
        let selected_row = self.state.results.selected_row;
        let body: Vec<TableRow<'_>> = page
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let style = if i == selected_row && focused {
                    Style::default()
                        .fg(theme.background.into())
                        .bg(theme.accent_primary.into())
                } else {
                    Style::default().fg(theme.foreground.into())
                };
                TableRow::new(r.values.clone()).style(style)
            })
            .collect();
        // Even widths; ratatui handles overflow.
        let n_cols = page.columns.len().max(1);
        let constraints: Vec<Constraint> =
            std::iter::repeat_n(Constraint::Percentage((100 / n_cols).max(1) as u16), n_cols)
                .collect();
        let _ = rows_len;
        let table = Table::new(body, constraints).header(header).block(block);
        frame.render_widget(table, rect);
    }

    fn render_history(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let focused = self.focused_pane == DbFocus::History;
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let mut title_style = Style::default().fg(theme.foreground.into());
        if focused {
            title_style = title_style.add_modifier(Modifier::BOLD);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(" Query history ")
            .title_style(title_style);

        if self.state.history.records.is_empty() {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "(no history yet)",
                    Style::default().fg(theme.muted.into()),
                )))
                .block(block),
                rect,
            );
            return;
        }

        let selected = self.state.history.selected;
        let lines: Vec<Line<'_>> = self
            .state
            .history
            .records
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let one_line: String = r.sql.replace('\n', " ");
                let style = if i == selected && focused {
                    Style::default()
                        .fg(theme.background.into())
                        .bg(theme.accent_primary.into())
                } else {
                    Style::default().fg(theme.foreground.into())
                };
                Line::from(Span::styled(one_line, style))
            })
            .collect();
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }

    fn render_status_bar(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
        let pane = match self.state.right_pane {
            RightPane::Editor => "Editor",
            RightPane::Results => "Results",
            RightPane::History => "History",
        };
        let label = format!(
            "Tab cycles right pane · Ctrl+R run query · Ctrl+E export CSV · current pane: {pane}"
        );
        let para = Paragraph::new(Line::from(Span::styled(
            label,
            Style::default().fg(theme.muted.into()),
        )));
        frame.render_widget(para, rect);
    }
}

/// Render the widget into a fresh test buffer of `(width, height)` using
/// the cosmos theme. Mirrors [`crate::network::render_to_string`].
///
/// # Examples
///
/// ```
/// use sid_widgets::DatabaseWidget;
/// use sid_widgets::database::render_to_string;
/// let w = DatabaseWidget::new(vec![]);
/// let s = render_to_string(&w, 80, 24);
/// assert!(s.contains("Connections"));
/// ```
pub fn render_to_string(widget: &DatabaseWidget, width: u16, height: u16) -> String {
    use ratatui::{Terminal, backend::TestBackend};
    let backend = TestBackend::new(width, height);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    term.draw(|f| widget.render_into_frame(f, f.area(), &theme))
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

impl Default for DatabaseWidget {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl Widget for DatabaseWidget {
    fn id(&self) -> &WidgetId {
        &self.id
    }

    fn title(&self) -> &str {
        "Database"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn footer_hint(&self) -> Vec<FooterHint> {
        match self.focused_pane {
            DbFocus::Connections => vec![
                FooterHint::new("Enter", "add/edit"),
                FooterHint::new("N", "new"),
                FooterHint::new("D", "delete"),
                FooterHint::new("Ctrl+T", "test"),
            ],
            DbFocus::Editor => vec![
                FooterHint::new("Ctrl+R", "run"),
                FooterHint::new("Ctrl+D", "disconnect"),
                FooterHint::new("Tab", "results"),
            ],
            DbFocus::Results => vec![
                FooterHint::new("j/k", "row"),
                FooterHint::new("h/l", "col"),
                FooterHint::new("c", "copy"),
                FooterHint::new("Tab", "history"),
            ],
            DbFocus::History => vec![
                FooterHint::new("j/k", "select"),
                FooterHint::new("Enter", "load"),
                FooterHint::new("Tab", "editor"),
            ],
        }
    }

    fn render(&self, target: &mut dyn RenderTarget) {
        // Full ratatui-driven layout (left pane + editor/results/history) is a
        // follow-up wire-layer task. For now use the shared "coming soon"
        // body so the tab still draws cleanly when selected.
        self.body.render(target);
    }

    fn handle_event(&mut self, ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        if let Event::Key(chord) = ev {
            // Tab / Shift+Tab cycle the focused pane FIRST.
            match (chord.code, chord.mods) {
                (KeyCode::Tab, KeyModifiers::NONE) => {
                    self.focus_next();
                    return EventOutcome::Consumed;
                }
                (KeyCode::Tab, m) if m.contains(KeyModifiers::SHIFT) => {
                    self.focus_prev();
                    return EventOutcome::Consumed;
                }
                (KeyCode::BackTab, _) => {
                    self.focus_prev();
                    return EventOutcome::Consumed;
                }
                _ => {}
            }
            // Alt+<key> is reserved for future cross-pane actions.
            if chord.mods.contains(KeyModifiers::ALT) {
                // TODO: cross-pane actions on Alt+<key>
                return EventOutcome::Bubble;
            }
            // Widget-global keybinds (independent of focused pane). These
            // are command bindings, not navigation; existing CRUD modal
            // triggers fire here regardless of which pane is focused.
            match (chord.code, chord.mods) {
                (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                    if let Some(id) = self.state.active_conn_id().map(|s| s.to_string()) {
                        let cmd = DbCommand::RunQuery {
                            sql: self.state.editor.full_source(),
                            conn_id: id,
                        };
                        self.state.push_command(cmd);
                    }
                    return EventOutcome::Consumed;
                }
                (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                    self.state.push_command(DbCommand::Disconnect);
                    return EventOutcome::Consumed;
                }
                _ => {}
            }
            // Pane-gated routing.
            match self.focused_pane {
                DbFocus::Connections => match (chord.code, chord.mods) {
                    (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                        self.state.select_next();
                        return EventOutcome::Consumed;
                    }
                    (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                        self.state.select_prev();
                        return EventOutcome::Consumed;
                    }
                    // Enter: open add form (on +add new) or edit form (on existing connection).
                    (KeyCode::Enter, KeyModifiers::NONE) => {
                        let prefill = if self.state.is_add_new_selected() {
                            None
                        } else {
                            self.state.selected_connection().cloned()
                        };
                        self.state
                            .push_command(DbCommand::OpenConnectionForm { prefill });
                        return EventOutcome::Consumed;
                    }
                    // N — convenience alias for "new", always opens add form.
                    (KeyCode::Char('N') | KeyCode::Char('n'), KeyModifiers::NONE) => {
                        self.state
                            .push_command(DbCommand::OpenConnectionForm { prefill: None });
                        return EventOutcome::Consumed;
                    }
                    // D / Delete — remove selected connection (keep existing delete modal flow).
                    (
                        KeyCode::Delete | KeyCode::Char('D') | KeyCode::Char('d'),
                        KeyModifiers::NONE,
                    ) => {
                        // wire.rs still handles this via database_modal_for_key on Delete/D;
                        // bubble so the existing modal path fires.
                        return EventOutcome::Bubble;
                    }
                    // Ctrl+T — test the highlighted connection; fall back to active.
                    (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
                        let conn_id = self
                            .state
                            .selected_connection()
                            .map(|c| c.id.as_str())
                            .or_else(|| self.state.active_conn_id())
                            .map(|s| s.to_string());
                        if let Some(id) = conn_id {
                            self.state
                                .push_command(DbCommand::TestConnection { conn_id: id });
                        }
                        return EventOutcome::Consumed;
                    }
                    _ => {}
                },
                DbFocus::Editor => {
                    // Editor pane: only "Editor-local" navigation is bound
                    // here. Character typing is the binary's wire-layer
                    // responsibility (it owns the EditorState mutators).
                }
                DbFocus::Results => match (chord.code, chord.mods) {
                    (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                        self.state.results.select_next_row();
                        return EventOutcome::Consumed;
                    }
                    (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                        self.state.results.select_prev_row();
                        return EventOutcome::Consumed;
                    }
                    (KeyCode::Char('c'), KeyModifiers::NONE) => {
                        if let Some(cell) = self.state.results.selected_cell() {
                            let cmd = DbCommand::CopyCell(cell.to_string());
                            self.state.push_command(cmd);
                        }
                        return EventOutcome::Consumed;
                    }
                    _ => {}
                },
                DbFocus::History => match (chord.code, chord.mods) {
                    (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                        self.state.history.select_next();
                        return EventOutcome::Consumed;
                    }
                    (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                        self.state.history.select_prev();
                        return EventOutcome::Consumed;
                    }
                    _ => {}
                },
            }
        }
        EventOutcome::Bubble
    }
}

fn char_byte_offset(s: &str, char_col: usize) -> usize {
    s.char_indices()
        .nth(char_col)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use sid_core::{context::WidgetCtx, event::Event, widget::Widget};

    use super::*;

    // Helper — minimal DbConnection for tests.
    fn stub_conn(id: &str) -> sid_store::DbConnection {
        use sid_store::now_epoch;
        sid_store::DbConnection {
            id: id.to_string(),
            kind: DbKind::Postgres,
            name: id.to_string(),
            dsn: format!("postgres://localhost/{id}"),
            secret_ref: None,
            created_at: now_epoch(),
        }
    }

    // Helper — minimal WidgetCtx for tests.
    fn stub_ctx() -> WidgetCtx {
        let (tx, _rx) = mpsc::channel();
        WidgetCtx::new(tx)
    }

    #[test]
    fn id_and_title_correct() {
        let w = DatabaseWidget::default();
        assert_eq!(w.id().as_str(), "database.root");
        assert_eq!(w.title(), "Database");
    }

    // ── Task 1: ListCursor integration ──────────────────────────────────────

    #[test]
    fn cursor_add_new_row_at_top_when_enabled() {
        let conns = vec![stub_conn("a"), stub_conn("b")];
        let mut state = DatabaseState::new_with_add_new(conns, true);
        // initial position is 0 — the synthetic +add new row
        assert!(matches!(
            state.cursor.target(),
            crate::list_cursor::CursorTarget::AddNew
        ));
        state.select_next();
        assert!(matches!(
            state.cursor.target(),
            crate::list_cursor::CursorTarget::Item(0)
        ));
        state.select_prev();
        assert!(matches!(
            state.cursor.target(),
            crate::list_cursor::CursorTarget::AddNew
        ));
    }

    #[test]
    fn cursor_wraps_to_add_new_from_last_item() {
        let conns = vec![stub_conn("a"), stub_conn("b")];
        let mut state = DatabaseState::new_with_add_new(conns, true);
        // drive to last item
        state.select_next(); // Item(0)
        state.select_next(); // Item(1)
        state.select_next(); // wraps back to AddNew
        assert!(matches!(
            state.cursor.target(),
            crate::list_cursor::CursorTarget::AddNew
        ));
    }

    #[test]
    fn cursor_no_add_new_row_when_disabled() {
        let conns = vec![stub_conn("a")];
        let state = DatabaseState::new_with_add_new(conns, false);
        assert!(matches!(
            state.cursor.target(),
            crate::list_cursor::CursorTarget::Item(0)
        ));
    }

    // ── Task 2: DbCommand::OpenConnectionForm + TestConnection ──────────────

    #[test]
    fn enter_on_connection_emits_open_form_with_prefill() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let conns = vec![stub_conn("pg")];
        let mut w = DatabaseWidget::new_with_add_new(conns, false);
        // cursor is on Item(0) since add_new=false
        let ev = Event::Key(KeyChord {
            code: KeyCode::Enter,
            mods: KeyModifiers::NONE,
        });
        let mut ctx = stub_ctx();
        w.handle_event(&ev, &mut ctx);
        let cmds = w.state.drain_commands();
        assert!(
            cmds.iter()
                .any(|c| matches!(c, DbCommand::OpenConnectionForm { prefill: Some(_) })),
            "expected OpenConnectionForm with prefill, got: {:?}",
            cmds
        );
    }

    #[test]
    fn enter_on_add_new_row_emits_open_form_no_prefill() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let conns = vec![stub_conn("pg")];
        let mut w = DatabaseWidget::new_with_add_new(conns, true);
        // cursor starts at AddNew
        let ev = Event::Key(KeyChord {
            code: KeyCode::Enter,
            mods: KeyModifiers::NONE,
        });
        let mut ctx = stub_ctx();
        w.handle_event(&ev, &mut ctx);
        let cmds = w.state.drain_commands();
        assert!(
            cmds.iter()
                .any(|c| matches!(c, DbCommand::OpenConnectionForm { prefill: None })),
            "expected OpenConnectionForm with no prefill, got: {:?}",
            cmds
        );
    }

    #[test]
    fn n_key_emits_open_form_no_prefill() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let mut w = DatabaseWidget::default();
        let ev = Event::Key(KeyChord {
            code: KeyCode::Char('N'),
            mods: KeyModifiers::NONE,
        });
        let mut ctx = stub_ctx();
        w.handle_event(&ev, &mut ctx);
        let cmds = w.state.drain_commands();
        assert!(
            cmds.iter()
                .any(|c| matches!(c, DbCommand::OpenConnectionForm { prefill: None })),
            "expected OpenConnectionForm, got: {:?}",
            cmds
        );
    }

    #[test]
    fn ctrl_t_emits_test_connection() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let conns = vec![stub_conn("pg")];
        let mut w = DatabaseWidget::new_with_add_new(conns, false);
        // set active conn id so Ctrl+T picks it up
        w.state.set_active_conn_id_for_tests("pg".to_string());
        let ev = Event::Key(KeyChord {
            code: KeyCode::Char('t'),
            mods: KeyModifiers::CONTROL,
        });
        let mut ctx = stub_ctx();
        w.handle_event(&ev, &mut ctx);
        let cmds = w.state.drain_commands();
        assert!(
            cmds.iter()
                .any(|c| matches!(c, DbCommand::TestConnection { conn_id } if conn_id == "pg")),
            "expected TestConnection, got: {:?}",
            cmds
        );
    }

    // ── Snapshot tests ───────────────────────────────────────────────────────

    #[test]
    fn snapshot_connection_list_empty_with_add_new() {
        let w = DatabaseWidget::new_with_add_new(vec![], true);
        let s = render_to_string(&w, 80, 24);
        insta::assert_snapshot!("connection_list_empty_add_new", s);
    }

    #[test]
    fn snapshot_connection_list_two_items_cursor_on_add_new() {
        // cursor at pos 0 → +add new is highlighted
        let w = DatabaseWidget::new_with_add_new(vec![stub_conn("pg"), stub_conn("staging")], true);
        let s = render_to_string(&w, 80, 24);
        insta::assert_snapshot!("connection_list_add_new_selected", s);
    }

    #[test]
    fn snapshot_connection_list_two_items_cursor_on_first_item() {
        let mut w =
            DatabaseWidget::new_with_add_new(vec![stub_conn("pg"), stub_conn("staging")], true);
        w.state.select_next(); // moves to Item(0)
        let s = render_to_string(&w, 80, 24);
        insta::assert_snapshot!("connection_list_first_item_selected", s);
    }

    #[test]
    fn snapshot_connection_list_no_add_new_row() {
        let w = DatabaseWidget::new_with_add_new(vec![stub_conn("pg")], false);
        let s = render_to_string(&w, 80, 24);
        insta::assert_snapshot!("connection_list_no_add_new", s);
    }

    /// Fix 3: Ctrl+T prefers the highlighted (selected) connection over the
    /// active one when both exist.
    #[test]
    fn ctrl_t_prefers_selected_over_active() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let conns = vec![stub_conn("pg"), stub_conn("staging")];
        let mut w = DatabaseWidget::new_with_add_new(conns, false);
        // pg is active, staging is highlighted (cursor on Item(1))
        w.state.set_active_conn_id_for_tests("pg".to_string());
        w.state.select_next(); // Item(0) → Item(1) = staging
        let ev = Event::Key(KeyChord {
            code: KeyCode::Char('t'),
            mods: KeyModifiers::CONTROL,
        });
        let mut ctx = stub_ctx();
        w.handle_event(&ev, &mut ctx);
        let cmds = w.state.drain_commands();
        assert!(
            cmds.iter().any(
                |c| matches!(c, DbCommand::TestConnection { conn_id } if conn_id == "staging")
            ),
            "Ctrl+T should test highlighted conn 'staging', not active 'pg'; got: {:?}",
            cmds
        );
    }

    /// Fix 3: Ctrl+T falls back to active when cursor is on +add new.
    #[test]
    fn ctrl_t_falls_back_to_active_on_add_new_row() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use sid_core::event::KeyChord;
        let conns = vec![stub_conn("pg")];
        let mut w = DatabaseWidget::new_with_add_new(conns, true);
        // Cursor starts on +add new (default with add_new=true); pg is active.
        w.state.set_active_conn_id_for_tests("pg".to_string());
        assert!(
            w.state.is_add_new_selected(),
            "cursor should be on +add new"
        );
        let ev = Event::Key(KeyChord {
            code: KeyCode::Char('t'),
            mods: KeyModifiers::CONTROL,
        });
        let mut ctx = stub_ctx();
        w.handle_event(&ev, &mut ctx);
        let cmds = w.state.drain_commands();
        assert!(
            cmds.iter()
                .any(|c| matches!(c, DbCommand::TestConnection { conn_id } if conn_id == "pg")),
            "Ctrl+T should fall back to active 'pg' when on +add new; got: {:?}",
            cmds
        );
    }
}
