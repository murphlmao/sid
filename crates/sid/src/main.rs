use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use clap::Parser;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
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
mod toast;
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

    /// Skip workspace discovery scan on startup.
    ///
    /// **No-op in this release.** sid no longer scans `~/vcs/` at startup;
    /// the flag is preserved for one release cycle so muscle memory doesn't
    /// trigger a CLI error. Use the `workspaces.scan_now` command-palette
    /// action (when implemented) to scan on demand.
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
    /// Network info and actions.
    ///
    /// Provides non-interactive access to the same data the Network tab
    /// renders: listening ports, processes, interfaces. Also exposes the
    /// kill flow for scripting (`sid net kill <pid>` / `port:<n>`).
    Net {
        #[command(subcommand)]
        op: NetOp,
    },
    /// Read/write `sid` settings without launching the TUI.
    ///
    /// Settings are stored in the redb `settings` table; the keys are the
    /// canonical names listed in `sid_store::settings_keys`.
    Settings {
        #[command(subcommand)]
        op: SettingsOp,
    },
    /// System tab operations (pin configs, list services, manage quick actions).
    ///
    /// Allows scripting the pinned-configs registry and the global quick-action
    /// table from outside the TUI. `services` requires a systemd host.
    System {
        #[command(subcommand)]
        op: SystemOp,
    },
    /// Database connections — add / remove / list / run a query.
    ///
    /// Stores Postgres + SQLite connections in the redb `db_connections` table.
    /// `query` runs a SQL statement against the named connection and prints
    /// CSV on stdout (SELECT/WITH) or a `rows affected` summary (otherwise).
    Db {
        #[command(subcommand)]
        op: DbOp,
    },
    /// SSH host registry operations.
    ///
    /// Stores manually-added SSH hosts in the redb `ssh_hosts` table. The TUI
    /// merges these with entries from `~/.ssh/config` on the SSH tab.
    Ssh {
        #[command(subcommand)]
        op: SshOp,
    },
    /// Run the sid MCP server over stdio.
    ///
    /// Speaks JSON-RPC 2.0 per the Model Context Protocol spec and exposes
    /// the sid-mcp tool surface (crate_info, find_pub_item, coverage_summary,
    /// gate_status, plan_status, recent_commits, criterion_compare,
    /// pub_items_without_doc_tests, tool_manifest). Used by Claude Code and
    /// other MCP-aware clients. Reads + serves until the client disconnects.
    Mcp,
}

/// Operations on the SSH host registry.
#[derive(Clone, clap::Subcommand, Debug)]
enum SshOp {
    /// Add an SSH host.
    Add {
        /// Alias used to refer to the host within sid.
        alias: String,
        /// Hostname or IP address.
        host: String,
        /// SSH user.
        #[arg(long, default_value = "root")]
        user: String,
        /// SSH port.
        #[arg(long, default_value_t = 22)]
        port: u16,
        /// Optional identity file path.
        #[arg(long)]
        identity_file: Option<String>,
    },
    /// Remove an SSH host by alias.
    Remove {
        /// Alias passed to `add`.
        alias: String,
    },
    /// List registered SSH hosts (manual + ssh-config).
    List,
    /// Connect to an alias (launches the TUI pre-pointed at the host).
    Connect {
        /// Alias to connect to.
        alias: String,
    },
}

/// Operations on the saved DB connections registry (Plan 4).
#[derive(clap::Subcommand, Debug)]
enum DbOp {
    /// Add a saved DB connection.
    Add {
        /// Stable id (used by `sid db query <id>`).
        id: String,
        /// `postgres` or `sqlite`.
        #[arg(long)]
        kind: String,
        /// User-facing label.
        #[arg(long)]
        name: String,
        /// DSN (Postgres) or filesystem path / `:memory:` (SQLite).
        #[arg(long)]
        dsn: String,
        /// Optional password (Postgres). Stored in the secrets table.
        #[arg(long)]
        password: Option<String>,
    },
    /// Remove a connection by id.
    Remove {
        /// Stable id passed to `add`.
        id: String,
    },
    /// List saved connections.
    List,
    /// Run a SQL statement and print the result as CSV on stdout.
    Query {
        /// Connection id.
        id: String,
        /// SQL to run.
        sql: String,
    },
}

/// Operations on the System tab — pinned configs, systemd services, quick actions.
#[derive(clap::Subcommand, Debug)]
enum SystemOp {
    /// Add a pinned config (creates or replaces).
    Pin {
        /// Path to pin (canonicalized).
        path: PathBuf,
        /// Display label. Defaults to the file name.
        #[arg(long)]
        label: Option<String>,
        /// Override the default opener command (shell command line).
        #[arg(long)]
        opener: Option<String>,
    },
    /// Remove a pinned config by path.
    Unpin {
        /// Path that was pinned.
        path: PathBuf,
    },
    /// List all pinned configs.
    Pins,
    /// List systemd services (requires `systemctl` on PATH).
    Services {
        /// Only user units. Mutually exclusive with `--system`; if neither set, both buses are queried.
        #[arg(long)]
        user: bool,
        /// Only system units. Mutually exclusive with `--user`.
        #[arg(long)]
        system: bool,
        /// Filter by `ActiveState` (e.g. `active`, `failed`, `inactive`).
        #[arg(long, value_name = "STATE")]
        state: Option<String>,
    },
    /// Quick-action CRUD + run.
    Action {
        #[command(subcommand)]
        op: ActionOp,
    },
}

/// Quick-action subcommands.
#[derive(clap::Subcommand, Debug)]
enum ActionOp {
    /// Add a global quick action.
    Add {
        /// Human-readable label.
        label: String,
        /// Shell command to run.
        cmd: String,
        /// Optional keybind chord string (matches `KeybindEntry::chord`).
        #[arg(long)]
        keybind: Option<String>,
    },
    /// List all quick actions.
    List,
    /// Remove a quick action by id.
    Remove {
        /// Action id (e.g. `qa-…`).
        id: String,
    },
    /// Run a quick action by id immediately (no TUI).
    Run {
        /// Action id (e.g. `qa-…`).
        id: String,
    },
}

/// Operations on the settings table.
#[derive(clap::Subcommand, Debug)]
enum SettingsOp {
    /// Print one setting value (UTF-8 bytes) to stdout.
    Get {
        /// Canonical setting key.
        key: String,
    },
    /// Set a setting to a raw UTF-8 string value.
    Set {
        /// Canonical setting key.
        key: String,
        /// New value (stored verbatim as UTF-8 bytes).
        value: String,
    },
    /// List every key currently set in the `settings` table.
    List,
    /// Delete a setting key.
    Delete {
        /// Canonical setting key.
        key: String,
    },
}

/// Operations on the network / process surface.
#[derive(clap::Subcommand, Debug)]
enum NetOp {
    /// List TCP/UDP sockets in LISTEN state.
    Ports {
        /// Output format: `table` (default) or `json`.
        #[arg(long, default_value = "table")]
        format: String,
    },
    /// List visible processes.
    Procs {
        /// Output format: `table` (default) or `json`.
        #[arg(long, default_value = "table")]
        format: String,
        /// Sort key: `pid` (default), `cpu`, `rss`, `name`.
        #[arg(long, default_value = "pid")]
        sort: String,
        /// Maximum rows to print.
        #[arg(long, default_value_t = 50)]
        top: usize,
    },
    /// List network interfaces.
    Interfaces {
        /// Output format: `table` (default) or `json`.
        #[arg(long, default_value = "table")]
        format: String,
    },
    /// Send a kill signal to a target by PID or `port:<n>`.
    Kill {
        /// Either a numeric PID or `port:<n>` (e.g., `port:8080`).
        target: String,
        /// Skip the SIGTERM grace period and SIGKILL immediately.
        #[arg(long)]
        force: bool,
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

    // Handle network subcommands first: they don't need the redb store, so
    // we avoid opening sid.redb (and its single-process lock) for parallel
    // CLI invocations and one-shot scripts.
    if let Some(Cmd::Net { op }) = cli.cmd {
        return handle_net_cmd(op).await;
    }

    // The MCP server speaks JSON-RPC on stdio and never opens the redb
    // store. Dispatch before opening sid.redb so Claude Code can spawn
    // an MCP session in parallel with a running TUI.
    if let Some(Cmd::Mcp) = cli.cmd {
        let workspace_root =
            std::env::current_dir().context("cannot read current dir for sid mcp")?;
        sid_mcp::run_stdio(workspace_root)
            .await
            .map_err(|e| anyhow!("sid-mcp: {e}"))?;
        return Ok(());
    }

    let path = wire::db_path(cli.db);
    let store = Arc::new(RedbStore::open(&path)?);

    // Handle workspace subcommands (exit before launching TUI).
    if let Some(Cmd::Workspace { op }) = cli.cmd {
        return handle_workspace_cmd(&*store, op);
    }

    // Handle `sid settings` subcommands (exit before launching TUI).
    if let Some(Cmd::Settings { op }) = cli.cmd {
        return handle_settings_cmd(&*store, op);
    }

    // Handle `sid system` subcommands (exit before launching TUI).
    if let Some(Cmd::System { op }) = cli.cmd {
        return handle_system_cmd(&*store, op);
    }

    // Handle `sid db` subcommands (exit before launching TUI).
    if let Some(Cmd::Db { op }) = cli.cmd {
        return handle_db_cmd(store.clone(), op).await;
    }

    // Handle `sid ssh` subcommands; only `connect` falls through to TUI launch.
    let mut start_ssh_alias: Option<String> = None;
    if let Some(Cmd::Ssh { ref op }) = cli.cmd {
        match op {
            SshOp::Connect { alias } => {
                start_ssh_alias = Some(alias.clone());
            }
            _ => {
                return handle_ssh_cmd(&*store, op.clone());
            }
        }
    }

    // Startup workspace discovery is disabled. Workspaces are exclusively
    // user-registered (via `sid workspace add` or the in-TUI N modal).
    // `cli.skip_discovery` is preserved as a no-op stub for one release
    // cycle and is otherwise ignored.
    let _ = cli.skip_discovery;

    // Resolve the active theme + keybind profile from the store. The full
    // wiring of these into the running App is incremental — for now the call
    // validates that the on-disk state parses cleanly and seeds the cosmos
    // keybind profile on first run.
    let (_active_theme, _theme_registry) = wire::load_active_theme(&*store);
    let _active_keybinds = wire::load_active_keybinds(&*store);

    // Start a new session record.
    let session_id = format!("sess-{}", now_epoch());
    let workspaces = store.list_workspaces().unwrap_or_default();

    // Load SSH hosts from the store + entries from ~/.ssh/config.
    let ssh_hosts = store.list_ssh_hosts().unwrap_or_default();
    let cfg_path = directories::UserDirs::new()
        .map(|d| d.home_dir().join(".ssh/config"))
        .unwrap_or_else(|| std::path::PathBuf::from("~/.ssh/config"));
    let ssh_cfg_entries: Vec<sid_widgets::ssh::SshConfigEntryLite> =
        sid_ssh::read_ssh_config(&cfg_path)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !e.host.contains('*'))
            .map(|e| sid_widgets::ssh::SshConfigEntryLite {
                alias: e.host.clone(),
                host: e.hostname.unwrap_or(e.host),
                port: e.port.unwrap_or(22),
                user: e
                    .user
                    .unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "root".into())),
                identity_file: e.identity_file,
            })
            .collect();

    // Load every per-tab data set the binary can read from the store, so each
    // widget is constructed populated rather than empty.
    let db_connections = store.list_db_connections().unwrap_or_default();
    let pinned_configs = store.list_pinned_configs().unwrap_or_default();
    let quick_actions = store.list_quick_actions().unwrap_or_default();

    // Settings sub-views.
    let (active_theme, theme_registry) = wire::load_active_theme(&*store);
    let _ = active_theme; // theme is applied by the render layer, not the widget
    let active_theme_name = {
        use sid_store::TypedSettings;
        store
            .get_string(sid_store::settings_keys::THEME_NAME)
            .ok()
            .flatten()
            .unwrap_or_else(|| "cosmos".into())
    };
    let active_keybinds = wire::load_active_keybinds(&*store);
    let action_registry_for_keybinds = sid_core::action::ActionRegistry::new();
    let workspace_roots_paths: Vec<std::path::PathBuf> = wire::default_discovery_roots();
    let db_path_for_view = sid_widgets::settings::db_path::DbPathView::open(
        path.clone(),
        directories::ProjectDirs::from("dev", "murphlmao", "sid")
            .map(|d| d.config_dir().join("sid.toml"))
            .unwrap_or_else(|| std::path::PathBuf::from("sid.toml")),
    )
    .ok();
    let mut settings_categories: Vec<sid_widgets::SettingsCategory> = Vec::new();
    settings_categories.push(sid_widgets::SettingsCategory::Theme(
        sid_widgets::settings::theme_picker::ThemePickerView::new(
            &theme_registry,
            &active_theme_name,
        ),
    ));
    settings_categories.push(sid_widgets::SettingsCategory::Keybinds(
        sid_widgets::settings::keybind_editor::KeybindEditorView::new(
            &action_registry_for_keybinds,
            active_keybinds,
        ),
    ));
    {
        let mut behavior = sid_widgets::settings::behavior_toggles::BehaviorTogglesView::defaults();
        let _ = behavior.load_from_store(&*store);
        settings_categories.push(sid_widgets::SettingsCategory::Behavior(behavior));
    }
    {
        let animation_cfg = wire::load_animation_config(&*store);
        // `with_store` lets the view's own `S`-key handler flush through
        // the embedded handle without the wire layer needing to detect the
        // press and route a separate action.
        let store_handle: Arc<dyn Store> = Arc::clone(&store) as Arc<dyn Store>;
        settings_categories.push(sid_widgets::SettingsCategory::Animation(
            sid_widgets::settings::animation::AnimationView::with_store(
                animation_cfg,
                store_handle,
            ),
        ));
    }
    settings_categories.push(sid_widgets::SettingsCategory::WorkspaceRoots(
        sid_widgets::settings::workspace_roots::WorkspaceRootsView::new(workspace_roots_paths),
    ));
    settings_categories.push(sid_widgets::SettingsCategory::QuickActions(
        sid_widgets::settings::quick_actions::QuickActionsView::new(quick_actions.clone()),
    ));
    if let Some(v) = db_path_for_view {
        settings_categories.push(sid_widgets::SettingsCategory::DbPath(v));
    }
    settings_categories.push(sid_widgets::SettingsCategory::Reset(
        sid_widgets::settings::reset::ResetView::new(),
    ));

    let mut app = wire::build_app_hydrated(
        cli.start_tab.as_deref(),
        wire::BuildAppData {
            workspaces,
            ssh_hosts,
            ssh_config_entries: ssh_cfg_entries,
            start_ssh_alias,
            db_connections,
            pinned_configs,
            quick_actions,
            settings_categories,
        },
    );

    // Hydrate global quick-actions into the palette registry (Plan 6).
    if let Err(e) = wire::hydrate_quick_actions_into_registry(&*store, app.actions_mut()) {
        tracing::warn!("hydrate quick-actions failed: {e}");
    }

    // Restoration of the previous session's active tab is handled via a
    // user-facing modal pushed by `wire::maybe_push_resume_modal` below,
    // after the SidApp is fully constructed. The modal lets the user pick
    // between resuming the prior tab and starting fresh; the unconditional
    // auto-restore that used to live here would have made "Start fresh"
    // unreachable.

    // Construct the SysProbe and spawn its polling loop so the Network tab
    // sees fresh snapshots while the TUI runs. We subscribe BEFORE spawning
    // so the first snapshot is captured, and we hand the same Arc<SysProbe>
    // to the task — `SysProbe::run` takes `&self`, so the broadcast channel
    // observed by `sys_rx` is the same one the loop sends on.
    let sys_probe = wire::build_sys_probe(Duration::from_secs(2));
    let sys_rx = sys_probe.subscribe();
    let probe_task = {
        let probe = Arc::clone(&sys_probe);
        tokio::spawn(async move { probe.run().await })
    };

    let systemctl = wire::build_systemctl_client();
    let spawner = wire::build_terminal_spawner();
    let postgres = sid_db_clients::PostgresClient::factory();
    let sqlite = sid_db_clients::SqliteClient::factory();
    let (secrets, keyring_active) =
        wire::build_secret_store(&store, Arc::clone(&store) as Arc<dyn Store>);

    // Background animation: load persisted AnimationConfig if any, else default.
    let animation = wire::load_animation_config(&*store);
    let fx_state = if animation.enabled {
        Some(sid_fx::FxState::new())
    } else {
        None
    };

    let jobs: Arc<sid_job::JobQueue<wire::JobOutcome>> = Arc::new(sid_job::JobQueue::new());
    let (ssh_outcome_tx, ssh_outcome_rx) = tokio::sync::mpsc::unbounded_channel();

    let mut sid_app = wire::SidApp {
        app,
        store: Arc::clone(&store),
        session_id: session_id.clone(),
        sys_probe: Some(Arc::clone(&sys_probe)),
        sys_rx: Some(sys_rx),
        systemctl,
        spawner,
        postgres,
        sqlite,
        secrets,
        animation,
        fx_state,
        modal_stack: Vec::new(),
        form: None,
        form_origin_tab: None,
        pending_submits: Vec::new(),
        toasts: toast::ToastQueue::new(4),
        jobs,
        ssh_client_factory: wire::build_ssh_client_factory_fn(),
        ssh_outcome_tx,
        ssh_outcome_rx,
        ssh_byte_rx: None,
        ssh_last_pty_area: None,
        ssh_shutdown_tx: None,
    };

    // Push a toast if the user requested OS keyring but it was unavailable.
    {
        use sid_store::TypedSettings;
        let wanted = store
            .get_bool(sid_store::settings_keys::USE_OS_KEYRING)
            .unwrap_or(None)
            .unwrap_or(false);
        if wanted && !keyring_active {
            sid_app.toasts.push(crate::toast::Toast::error(
                "OS keyring unavailable — secrets stored as plaintext",
            ));
        }
    }

    // Offer the user a resume-or-start-fresh modal if the previous session
    // was recent enough and had a recorded active tab. No-op when there's no
    // prior session (e.g. first launch on a fresh store).
    wire::maybe_push_resume_modal(&mut sid_app);

    // Set up terminal. Mouse capture is enabled so the event pump receives
    // wheel scrolls and click events alongside keyboard input; the wire layer
    // routes them in `run_event_loop`.
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    // Enable the kitty keyboard protocol so Ctrl+Tab / Ctrl+Enter reach the
    // app in supporting terminals (kitty, wezterm, ghostty, foot).  Terminals
    // that do not support the protocol silently ignore the escape sequence, so
    // this is harmless on legacy terminals.
    let _ = crossterm::execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    // Event source.
    let (tx, mut rx) = runtime::make_channel();
    let pump = runtime::spawn_event_pump(tx, Duration::from_millis(250));

    // Run.
    let run_result = wire::run_event_loop(&mut terminal, &mut sid_app, &mut rx).await;
    pump.abort();
    probe_task.abort();

    // Restore terminal. Disable mouse capture in the same execute! call so a
    // crash before this point still releases the terminal cleanly on the
    // next process invocation (the Drop won't run, but the next process's
    // EnableMouseCapture supersedes any stale state).
    disable_raw_mode()?;
    let _ = execute!(
        std::io::stdout(),
        PopKeyboardEnhancementFlags,
        LeaveAlternateScreen,
        DisableMouseCapture
    );
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

/// Dispatch a `sid net …` subcommand.
async fn handle_net_cmd(op: NetOp) -> Result<()> {
    use std::time::Duration as StdDuration;

    use sid_core::adapters::sys::{Signal, SysProvider as _};
    use sid_core::sys_probe::kill_job::{KillOutcome, run_kill_job};

    let mut provider = sid_sysinfo::SysinfoProvider::new();
    match op {
        NetOp::Ports { format } => {
            let ports = provider
                .list_listening_ports()
                .map_err(|e| anyhow!("list_listening_ports: {e}"))?;
            match format.as_str() {
                "json" => println!("{}", serde_json::to_string_pretty(&ports)?),
                _ => {
                    println!(
                        "{:<6} {:<8} {:<5} {:<8} COMMAND",
                        "PORT", "PID", "PROTO", "STATE"
                    );
                    for p in ports {
                        let pid_s = p
                            .pid
                            .map(|p| p.as_u32().to_string())
                            .unwrap_or_else(|| "-".into());
                        println!(
                            "{:<6} {:<8} {:<5} {:<8} {}",
                            p.port,
                            pid_s,
                            format!("{:?}", p.protocol).to_lowercase(),
                            format!("{:?}", p.state).to_lowercase(),
                            p.command,
                        );
                    }
                }
            }
        }
        NetOp::Procs { format, sort, top } => {
            let mut procs = provider
                .list_processes()
                .map_err(|e| anyhow!("list_processes: {e}"))?;
            match sort.as_str() {
                "cpu" => procs.sort_by(|a, b| {
                    b.cpu_pct
                        .partial_cmp(&a.cpu_pct)
                        .unwrap_or(std::cmp::Ordering::Equal)
                }),
                "rss" => procs.sort_by(|a, b| b.rss_bytes.cmp(&a.rss_bytes)),
                "name" => procs.sort_by(|a, b| a.name.cmp(&b.name)),
                _ => procs.sort_by_key(|p| p.pid.as_u32()),
            }
            procs.truncate(top);
            match format.as_str() {
                "json" => println!("{}", serde_json::to_string_pretty(&procs)?),
                _ => {
                    println!(
                        "{:<8} {:<24} {:>6} {:>10} USER",
                        "PID", "NAME", "CPU%", "RSS"
                    );
                    for p in procs {
                        let user = p.user.unwrap_or_else(|| "-".into());
                        println!(
                            "{:<8} {:<24} {:>6.1} {:>10} {}",
                            p.pid.as_u32(),
                            truncate_to(&p.name, 24),
                            p.cpu_pct,
                            p.rss_bytes,
                            user,
                        );
                    }
                }
            }
        }
        NetOp::Interfaces { format } => {
            let ifs = provider
                .list_interfaces()
                .map_err(|e| anyhow!("list_interfaces: {e}"))?;
            match format.as_str() {
                "json" => println!("{}", serde_json::to_string_pretty(&ifs)?),
                _ => {
                    println!(
                        "{:<16} {:<6} {:>12} {:>12} ADDRS",
                        "NAME", "STATUS", "RX", "TX"
                    );
                    for i in ifs {
                        let status = if i.is_up { "up" } else { "down" };
                        println!(
                            "{:<16} {:<6} {:>12} {:>12} {}",
                            i.name,
                            status,
                            i.rx_bytes,
                            i.tx_bytes,
                            i.addrs.join(","),
                        );
                    }
                }
            }
        }
        NetOp::Kill { target, force } => {
            let pid = parse_kill_target(&target, &mut provider)?;
            if force {
                // Skip the SIGTERM grace period; SIGKILL directly.
                provider
                    .kill_process(pid, Signal::Kill)
                    .map_err(|e| anyhow!("kill: {e}"))?;
                println!("sent SIGKILL to PID {}", pid.as_u32());
                return Ok(());
            }
            let provider_arc: std::sync::Arc<
                std::sync::Mutex<dyn sid_core::adapters::sys::SysProvider>,
            > = std::sync::Arc::new(std::sync::Mutex::new(provider));
            let outcome = run_kill_job(provider_arc, pid, StdDuration::from_secs(5))
                .await
                .map_err(|e| anyhow!("kill: {e}"))?;
            match outcome {
                KillOutcome::Killed(p) => {
                    println!("killed PID {} (SIGTERM)", p.as_u32());
                }
                KillOutcome::EscalatedToSigkill(p) => {
                    println!("PID {} ignored SIGTERM; sent SIGKILL", p.as_u32());
                }
                KillOutcome::Failed(p, msg) => {
                    eprintln!("kill PID {} failed: {msg}", p.as_u32());
                    std::process::exit(2);
                }
            }
        }
    }
    Ok(())
}

fn truncate_to(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

/// Parse a `target` argument from `sid net kill`:
/// - `port:N` → resolves to the PID of the LISTEN socket on N, if any.
/// - digits-only → treats as a raw PID.
/// - anything else → exits with code 2.
fn parse_kill_target(
    target: &str,
    provider: &mut sid_sysinfo::SysinfoProvider,
) -> Result<sid_core::adapters::sys::Pid> {
    use sid_core::adapters::sys::{Pid, SysProvider as _};

    if let Some(num) = target.strip_prefix("port:") {
        let port: u16 = num
            .parse()
            .map_err(|e| anyhow!("invalid port number {num:?}: {e}"))?;
        let ports = provider
            .list_listening_ports()
            .map_err(|e| anyhow!("list_listening_ports: {e}"))?;
        let owner = ports
            .into_iter()
            .find(|p| p.port == port)
            .ok_or_else(|| anyhow!("no listening socket on port {port}"))?;
        let pid = owner
            .pid
            .ok_or_else(|| anyhow!("port {port} has no attributable PID"))?;
        return Ok(pid);
    }
    // Plain digits → PID.
    if target.chars().all(|c| c.is_ascii_digit()) && !target.is_empty() {
        let pid: u32 = target
            .parse()
            .map_err(|e| anyhow!("invalid PID {target:?}: {e}"))?;
        return Ok(Pid::from_u32(pid));
    }
    Err(anyhow!(
        "invalid kill target {target:?}: expected `port:<n>` or a numeric PID"
    ))
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
                    println!("{:<40} {:?}  {}", w.name, w.kind, w.path.display());
                }
            }
        }
    }
    Ok(())
}

/// Dispatch a `sid ssh …` subcommand (excluding `connect`).
fn handle_ssh_cmd(store: &dyn Store, op: SshOp) -> Result<()> {
    use sid_store::{SshHost, SshHostSource};
    match op {
        SshOp::Add {
            alias,
            host,
            user,
            port,
            identity_file,
        } => {
            let h = SshHost {
                alias: alias.clone(),
                host,
                port,
                user,
                identity_file,
                source: SshHostSource::Manual,
                last_connected: 0,
                command_history: Vec::new(),
                last_sftp_path: None,
                auth_kind: sid_store::SshAuthKind::Agent,
            };
            store
                .upsert_ssh_host(&h)
                .with_context(|| "upsert ssh host")?;
            println!("added ssh host: {alias}");
        }
        SshOp::Remove { alias } => {
            store
                .remove_ssh_host(&alias)
                .with_context(|| "remove ssh host")?;
            println!("removed ssh host: {alias}");
        }
        SshOp::List => {
            for h in store.list_ssh_hosts().unwrap_or_default() {
                println!("{:<20} {}@{}:{} [Manual]", h.alias, h.user, h.host, h.port);
            }
            let cfg_path = directories::UserDirs::new()
                .map(|d| d.home_dir().join(".ssh/config"))
                .unwrap_or_else(|| std::path::PathBuf::from("~/.ssh/config"));
            for e in sid_ssh::read_ssh_config(&cfg_path).unwrap_or_default() {
                let user = e.user.unwrap_or_else(|| "?".to_string());
                let hostname = e.hostname.unwrap_or_else(|| e.host.clone());
                println!(
                    "{:<20} {}@{}:{} [SshConfig]",
                    e.host,
                    user,
                    hostname,
                    e.port.unwrap_or(22)
                );
            }
        }
        SshOp::Connect { .. } => {
            // Handled in main: falls through to TUI launch.
        }
    }
    Ok(())
}

/// Dispatch a `sid settings …` subcommand.
fn handle_settings_cmd(store: &dyn Store, op: SettingsOp) -> Result<()> {
    use sid_store::SettingValue;
    match op {
        SettingsOp::Get { key } => match store.get_setting(&key)? {
            None => Err(anyhow!("setting not set: {key}")),
            Some(v) => {
                match std::str::from_utf8(&v.0) {
                    Ok(s) => println!("{s}"),
                    Err(_) => {
                        // Fall back to a length-prefixed hex line for non-UTF8
                        // values so scripts can still consume the output.
                        println!("0x{}", hex_string(&v.0));
                    }
                }
                Ok(())
            }
        },
        SettingsOp::Set { key, value } => {
            store.put_setting(&key, &SettingValue(value.into_bytes()))?;
            Ok(())
        }
        SettingsOp::Delete { key } => {
            if store.delete_setting(&key)? {
                println!("deleted: {key}");
            } else {
                println!("not set: {key}");
            }
            Ok(())
        }
        SettingsOp::List => {
            for key in store.list_setting_keys()? {
                match store.get_setting(&key)? {
                    Some(v) => match std::str::from_utf8(&v.0) {
                        Ok(s) => println!("{key} = {s}"),
                        Err(_) => println!("{key} = 0x{}", hex_string(&v.0)),
                    },
                    None => println!("{key} = <missing>"),
                }
            }
            Ok(())
        }
    }
}

/// Dispatch `sid system …` subcommands (pin/unpin/pins/services/action).
fn handle_system_cmd(store: &dyn Store, op: SystemOp) -> Result<()> {
    use sid_core::adapters::systemctl::{SystemctlClient as _, UnitBus, UnitFilter};
    use sid_store::{PinnedConfig, QuickAction, QuickActionScope};

    match op {
        SystemOp::Pin {
            path,
            label,
            opener,
        } => {
            // canonicalize so identical-but-different-form paths collapse.
            let abs = std::fs::canonicalize(&path).unwrap_or(path);
            let display_label = label.unwrap_or_else(|| {
                abs.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("(unnamed)")
                    .to_string()
            });
            let pc = PinnedConfig {
                path: abs.clone(),
                label: display_label,
                opener_cmd: opener,
                created_at: now_epoch(),
            };
            store.upsert_pinned_config(&pc)?;
            println!("pinned: {}", abs.display());
            Ok(())
        }
        SystemOp::Unpin { path } => {
            let abs = std::fs::canonicalize(&path).unwrap_or(path);
            store.remove_pinned_config(&abs)?;
            println!("unpinned: {}", abs.display());
            Ok(())
        }
        SystemOp::Pins => {
            for p in store.list_pinned_configs()? {
                println!("{:<40} {}", p.label, p.path.display());
            }
            Ok(())
        }
        SystemOp::Services {
            user,
            system,
            state,
        } => {
            let bus_both = (user && system) || (!user && !system);
            let bus = if system {
                UnitBus::System
            } else {
                UnitBus::User
            };
            let state_filter = state.as_deref().map(sid_system::parse::parse_unit_state);
            let client = sid_system::SystemctlCmdClient::new()
                .map_err(|e| anyhow!("systemctl unavailable: {e}"))?;
            let units = client
                .list_units(UnitFilter {
                    name_substring: None,
                    state: state_filter,
                    bus,
                    bus_both,
                })
                .map_err(|e| anyhow!("list_units: {e}"))?;
            for u in units {
                println!(
                    "{:<40} {:<12} {:<10} {}",
                    u.name,
                    format!("{:?}", u.state),
                    u.sub_state,
                    u.description
                );
            }
            Ok(())
        }
        SystemOp::Action { op } => match op {
            ActionOp::Add {
                label,
                cmd,
                keybind,
            } => {
                let a = QuickAction {
                    id: QuickAction::new_id(),
                    label,
                    scope: QuickActionScope::Global,
                    cmd,
                    keybind,
                };
                store.upsert_quick_action(&a)?;
                println!("added action: {} ({})", a.label, a.id);
                Ok(())
            }
            ActionOp::List => {
                for a in store.list_quick_actions()? {
                    println!("{:<24} {:<40} {}", a.id, a.label, a.cmd);
                }
                Ok(())
            }
            ActionOp::Remove { id } => {
                store.remove_quick_action(&id)?;
                println!("removed: {id}");
                Ok(())
            }
            ActionOp::Run { id } => {
                let a = store
                    .get_quick_action(&id)?
                    .ok_or_else(|| anyhow!("no such action: {id}"))?;
                let parts = shell_words::split(&a.cmd).map_err(|e| anyhow!("shell-words: {e}"))?;
                let (bin, args) = parts.split_first().ok_or_else(|| anyhow!("empty cmd"))?;
                let status = std::process::Command::new(bin).args(args).status()?;
                std::process::exit(status.code().unwrap_or(1));
            }
        },
    }
}

async fn handle_db_cmd(store: Arc<RedbStore>, op: DbOp) -> Result<()> {
    use sid_core::adapters::db_client::{DbClient, DbKind, OpenParams};
    use sid_core::adapters::secrets::{SecretId, SecretStore};
    use sid_db_clients::{PostgresClient, SqliteClient};
    use sid_secrets::PlainStore;
    use sid_store::{DbConnection, QueryRecord};

    match op {
        DbOp::Add {
            id,
            kind,
            name,
            dsn,
            password,
        } => {
            let kind = match kind.as_str() {
                "postgres" => DbKind::Postgres,
                "sqlite" => DbKind::Sqlite,
                other => anyhow::bail!("unknown kind '{other}' (use 'postgres' or 'sqlite')"),
            };
            let secret_ref = if let Some(pw) = password {
                let r = SecretId::new(format!("db.{id}.password"));
                let plain = PlainStore::new(store.clone() as Arc<dyn Store>);
                plain
                    .put(&r, pw.as_bytes())
                    .map_err(|e| anyhow!("put secret: {e}"))?;
                Some(r)
            } else {
                None
            };
            let conn = DbConnection {
                id: id.clone(),
                kind,
                name,
                dsn,
                secret_ref,
                created_at: now_epoch(),
            };
            store.upsert_db_connection(&conn)?;
            println!("added connection: {id}");
            Ok(())
        }
        DbOp::Remove { id } => {
            if let Some(c) = store.get_db_connection(&id)? {
                if let Some(r) = c.secret_ref {
                    let plain = PlainStore::new(store.clone() as Arc<dyn Store>);
                    let _ = plain.delete(&r);
                }
            }
            store.remove_db_connection(&id)?;
            println!("removed connection: {id}");
            Ok(())
        }
        DbOp::List => {
            for c in store.list_db_connections()? {
                println!("{:<24} {:?}  {}  ({})", c.id, c.kind, c.name, c.dsn);
            }
            Ok(())
        }
        DbOp::Query { id, sql } => {
            let conn = store
                .get_db_connection(&id)?
                .ok_or_else(|| anyhow!("no such connection: {id}"))?;
            let password = if let Some(r) = conn.secret_ref.as_ref() {
                let plain = PlainStore::new(store.clone() as Arc<dyn Store>);
                plain
                    .get(r)
                    .map_err(|e| anyhow!("get secret: {e}"))?
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            } else {
                None
            };
            let factory: Arc<dyn DbClient> = match conn.kind {
                DbKind::Postgres => PostgresClient::factory(),
                DbKind::Sqlite => SqliteClient::factory(),
            };
            let client = factory
                .open(OpenParams {
                    kind: conn.kind,
                    dsn: conn.dsn.clone(),
                    password,
                })
                .await
                .map_err(|e| anyhow!("open: {e}"))?;
            let trimmed = sql.trim_start().to_ascii_uppercase();
            let is_query = trimmed.starts_with("SELECT") || trimmed.starts_with("WITH");
            if is_query {
                let mut cursor = None;
                let mut wrote_header = false;
                let stdout = std::io::stdout();
                let mut lock = stdout.lock();
                loop {
                    let page = client
                        .query_paged(&sql, cursor, 500)
                        .await
                        .map_err(|e| anyhow!("query: {e}"))?;
                    if !wrote_header {
                        let header_page = sid_core::adapters::db_client::QueryPage {
                            columns: page.columns.clone(),
                            rows: vec![],
                            next_cursor: None,
                            duration_ms: 0,
                        };
                        sid_widgets::csv_export::write_page_csv(&header_page, &mut lock)?;
                        wrote_header = true;
                    }
                    {
                        let mut w = csv::Writer::from_writer(&mut lock);
                        for r in &page.rows {
                            w.write_record(r.values.iter().map(|s| s.as_str()))
                                .map_err(std::io::Error::other)?;
                        }
                        w.flush()?;
                    }
                    cursor = page.next_cursor;
                    if cursor.is_none() {
                        break;
                    }
                }
            } else {
                let r = client
                    .execute(&sql)
                    .await
                    .map_err(|e| anyhow!("execute: {e}"))?;
                println!("{} rows affected ({}ms)", r.rows_affected, r.duration_ms);
            }
            let _ = store.append_query_record(&QueryRecord {
                conn_id: id.clone(),
                sql: sql.clone(),
                duration_ms: 0,
                row_count: 0,
                ts_ns: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            });
            Ok(())
        }
    }
}

fn hex_string(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
