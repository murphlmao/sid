use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use sid_store::{now_epoch, OpenStore, RedbStore, Store};
use tracing_subscriber::EnvFilter;

mod runtime;
mod wire;

/// CLI arguments for the sid TUI cockpit.
///
/// # Examples
///
/// ```no_run
/// // Parsing is handled by clap at runtime; this type is constructed via
/// // `Cli::parse()` in main.  See integration tests in tests/cli.rs for
/// // end-to-end coverage.
/// ```
#[derive(Parser, Debug)]
#[command(name = "sid", version, about = "a fast, focused TUI cockpit for developer workflow")]
struct Cli {
    /// Override the default redb file path.
    #[arg(long)]
    db: Option<PathBuf>,

    /// Start in this tab if present (id: workspaces, ssh, database, network, system, settings).
    #[arg(long)]
    start_tab: Option<String>,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    color_eyre::install().ok();
    install_tracing();
    let cli = Cli::parse();

    let path = wire::db_path(cli.db);
    let store = Arc::new(RedbStore::open(&path)?);

    // Start a new session record.
    let session_id = format!("sess-{}", now_epoch());
    let mut app = wire::build_app(cli.start_tab.as_deref());

    // Restore last active tab from the previous session, if any.
    if let Ok(Some(prev)) = store.current_session() {
        if let Some(tab_id) = prev.active_tab {
            let _ = app.tabs_mut().switch_to(&tab_id);
        }
    }

    let mut sid_app = wire::SidApp { app, store: Arc::clone(&store), session_id: session_id.clone() };

    // Set up terminal.
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    // Event source.
    let (tx, mut rx) = runtime::make_channel();
    let pump = runtime::spawn_event_pump(tx, Duration::from_millis(250));

    // Run.
    let run_result = wire::run_event_loop(&mut terminal, &mut sid_app, &mut rx).await;
    pump.abort();

    // Restore terminal.
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Mark session ended.
    let _ = store.end_session(&session_id, now_epoch());

    run_result
}

fn install_tracing() {
    let filter = EnvFilter::try_from_env("SID_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
