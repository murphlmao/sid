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
    /// Network info and actions.
    ///
    /// Provides non-interactive access to the same data the Network tab
    /// renders: listening ports, processes, interfaces. Also exposes the
    /// kill flow for scripting (`sid net kill <pid>` / `port:<n>`).
    Net {
        #[command(subcommand)]
        op: NetOp,
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

    // Construct the SysProbe and spawn its polling loop so the Network tab
    // sees fresh snapshots while the TUI runs.
    let sys_probe = wire::build_sys_probe(Duration::from_secs(2));
    let probe_task = {
        let probe = Arc::clone(&sys_probe);
        // SysProbe::run consumes self; clone the inner Arc and re-wrap.
        tokio::spawn(async move {
            // Build a transient owned SysProbe sharing the provider handle
            // and interval. We bypass Arc::try_unwrap because the Arc may
            // have outstanding references in tests.
            let owned = sid_core::sys_probe::SysProbe::new(probe.provider(), probe.interval());
            owned.run().await;
        })
    };

    let mut sid_app = wire::SidApp {
        app,
        store: Arc::clone(&store),
        session_id: session_id.clone(),
        sys_probe: Some(Arc::clone(&sys_probe)),
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
    probe_task.abort();

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
            let provider_arc: std::sync::Arc<std::sync::Mutex<dyn sid_core::adapters::sys::SysProvider>> =
                std::sync::Arc::new(std::sync::Mutex::new(provider));
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
