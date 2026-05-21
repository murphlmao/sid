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

use std::path::PathBuf;
use std::sync::Arc;

use sid_core::adapters::db_client::{DbClient, PageCursor, QueryPage};
use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
use sid_db_clients::lexer::{Token, tokenize};
use sid_store::{DbConnection, QueryRecord};

use crate::stub::ComingSoonBody;

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
    selected_idx: usize,
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
    /// is selected by default.
    pub fn new(connections: Vec<DbConnection>) -> Self {
        Self {
            connections,
            selected_idx: 0,
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

    /// Currently-selected connection (`None` if list is empty).
    pub fn selected_connection(&self) -> Option<&DbConnection> {
        self.connections.get(self.selected_idx)
    }

    /// Replace the connection list (e.g., after a CLI add/remove).
    pub fn set_connections(&mut self, c: Vec<DbConnection>) {
        self.connections = c;
        if self.selected_idx >= self.connections.len() {
            self.selected_idx = 0;
        }
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

    /// Advance the connection-list selection.
    pub fn select_next(&mut self) {
        let n = self.connections.len();
        if n == 0 {
            return;
        }
        self.selected_idx = (self.selected_idx + 1) % n;
    }

    /// Reverse the connection-list selection.
    pub fn select_prev(&mut self) {
        let n = self.connections.len();
        if n == 0 {
            return;
        }
        self.selected_idx = (self.selected_idx + n - 1) % n;
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
        Self {
            state: DatabaseState::new(connections),
            id: WidgetId::new("database.root"),
            body: ComingSoonBody::new(
                "Database",
                "Postgres + SQLite query runner — Plan 4 wires the editor + results table in a follow-up.",
            ),
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

    fn render(&self, target: &mut dyn RenderTarget) {
        // Full ratatui-driven layout (left pane + editor/results/history) is a
        // follow-up wire-layer task. For now use the shared "coming soon"
        // body so the tab still draws cleanly when selected.
        self.body.render(target);
    }

    fn handle_event(&mut self, ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        if let Event::Key(chord) = ev {
            match (chord.code, chord.mods) {
                (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                    match self.state.right_pane {
                        RightPane::Results => self.state.results.select_next_row(),
                        RightPane::History => self.state.history.select_next(),
                        RightPane::Editor => self.state.select_next(),
                    }
                    return EventOutcome::Consumed;
                }
                (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                    match self.state.right_pane {
                        RightPane::Results => self.state.results.select_prev_row(),
                        RightPane::History => self.state.history.select_prev(),
                        RightPane::Editor => self.state.select_prev(),
                    }
                    return EventOutcome::Consumed;
                }
                (KeyCode::Tab, KeyModifiers::NONE) => {
                    self.state.cycle_right_pane();
                    return EventOutcome::Consumed;
                }
                (KeyCode::Enter, KeyModifiers::NONE) => {
                    if let Some(c) = self.state.selected_connection() {
                        let cmd = DbCommand::Connect {
                            conn_id: c.id.clone(),
                        };
                        self.state.push_command(cmd);
                    }
                    return EventOutcome::Consumed;
                }
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
                (KeyCode::Char('c'), KeyModifiers::NONE)
                    if self.state.right_pane == RightPane::Results =>
                {
                    if let Some(cell) = self.state.results.selected_cell() {
                        let cmd = DbCommand::CopyCell(cell.to_string());
                        self.state.push_command(cmd);
                    }
                    return EventOutcome::Consumed;
                }
                _ => {}
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
    use sid_core::widget::Widget;

    use super::*;

    #[test]
    fn id_and_title_correct() {
        let w = DatabaseWidget::default();
        assert_eq!(w.id().as_str(), "database.root");
        assert_eq!(w.title(), "Database");
    }
}
