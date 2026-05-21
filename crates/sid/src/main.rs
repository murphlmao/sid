use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use clap::Parser;
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use sid_core::workspace_metadata::read_workspace_metadata;
use sid_store::{OpenStore, RedbStore, Store, Workspace, now_epoch};
use tracing_subscriber::EnvFilter;

mod runtime;
mod wire;

#[derive(Parser, Debug)]
#[command(
    name = "sid",
    version,
    about = "a fast, focused TUI cockpit for developer workflow"
)]
struct Cli {
    /// Override the default redb file path.
    #[arg(long)]
    db: Option<PathBuf>,

    /// Start in this tab if present (id: workspaces, ssh, database, network, system, settings).
    #[arg(long)]
    start_tab: Option<String>,

    /// Skip workspace discovery scan on startup (faster launch, useful in tests).
    #[arg(long)]
    skip_discovery: bool,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

/// Top-level subcommands for `sid`.
#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// Workspace registry operations.
    ///
    /// Workspaces are git repositories or umbrella directories registered in
    /// the sid store.  Use `add` / `remove` / `list` to manage them manually.
    /// Discovery (`~/vcs/` scan) runs automatically on each startup unless
    /// `--skip-discovery` is passed.
    Workspace {
        #[command(subcommand)]
        op: WorkspaceOp,
    },
}

/// Operations on the workspace registry.
#[derive(clap::Subcommand, Debug)]
enum WorkspaceOp {
    /// Register a workspace at the given path.
    ///
    /// The path is canonicalized and metadata is read from `.sid/_metadata.sid`
    /// if present; otherwise the directory name is used as the workspace name.
    Add {
        /// Filesystem path of the workspace to register.
        path: PathBuf,
    },
    /// Remove a registered workspace by its path.
    ///
    /// A workspace that has never been registered is a no-op (not an error).
    Remove {
        /// Filesystem path of the workspace to remove.
        path: PathBuf,
    },
    /// List all registered workspaces.
    List,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    color_eyre::install().ok();
    install_tracing();
    let cli = Cli::parse();

    let path = wire::db_path(cli.db);
    let store = Arc::new(RedbStore::open(&path)?);

    // Handle workspace subcommands (exit before launching TUI).
    if let Some(Cmd::Workspace { op }) = cli.cmd {
        return handle_workspace_cmd(&*store, op);
    }

    // Startup workspace discovery (scan ~/vcs/ by default).
    if !cli.skip_discovery {
        let roots = wire::default_discovery_roots();
        // Discovery errors are non-fatal: log and continue.
        if let Err(e) = wire::startup_discover(&*store, &roots) {
            tracing::warn!("workspace discovery failed: {e}");
        }
    }

    // Start a new session record.
    let session_id = format!("sess-{}", now_epoch());
    let workspaces = store.list_workspaces().unwrap_or_default();
    let mut app = wire::build_app(cli.start_tab.as_deref(), workspaces);

    // Restore last active tab from the previous session, if any.
    if let Ok(Some(prev)) = store.current_session() {
        if let Some(tab_id) = prev.active_tab {
            let _ = app.tabs_mut().switch_to(&tab_id);
        }
    }

    let mut sid_app = wire::SidApp {
        app,
        store: Arc::clone(&store),
        session_id: session_id.clone(),
    };

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

/// Dispatch a workspace registry subcommand and return.
fn handle_workspace_cmd(store: &dyn Store, op: WorkspaceOp) -> Result<()> {
    match op {
        WorkspaceOp::Add { path } => {
            let abs = std::fs::canonicalize(&path)
                .with_context(|| format!("canonicalize path {:?}", path))?;
            let meta = read_workspace_metadata(&abs).unwrap_or_else(|_| {
                sid_core::workspace_metadata::WorkspaceMetadata::from_basename(
                    &abs,
                    sid_core::workspace_metadata::WorkspaceKind::Repo,
                )
            });
            let w = Workspace {
                path: abs.clone(),
                name: meta.name,
                kind: meta.kind,
                manifest_hash: 0,
                last_seen: now_epoch(),
                parent: None,
            };
            store
                .upsert_workspace(&w)
                .map_err(|e| anyhow!("add workspace {:?}: {e}", abs))?;
            println!("added: {}", abs.display());
        }
        WorkspaceOp::Remove { path } => {
            // Best-effort canonicalize; if the path doesn't exist any more,
            // use it as-is (the store key is the original absolute path).
            let abs = std::fs::canonicalize(&path).unwrap_or(path.clone());
            store
                .remove_workspace(&abs)
                .map_err(|e| anyhow!("remove workspace {:?}: {e}", abs))?;
            println!("removed: {}", abs.display());
        }
        WorkspaceOp::List => {
            let workspaces = store
                .list_workspaces()
                .map_err(|e| anyhow!("list workspaces: {e}"))?;
            if workspaces.is_empty() {
                println!("(no workspaces registered)");
            } else {
                for w in &workspaces {
                    println!(
                        "{:<40} {:?}  {}",
                        w.name,
                        w.kind,
                        w.path.display()
                    );
                }
            }
        }
    }
    Ok(())
}
