use std::path::PathBuf;

use clap::Parser;

mod runtime;
mod wire;

/// CLI arguments for the sid TUI cockpit.
///
/// # Examples
///
/// ```no_run
/// // Parsing is handled by clap at runtime; doc example shows construction.
/// use std::path::PathBuf;
/// // clap's derive macro generates the parser; not directly constructable
/// // without clap internals.  See integration tests in tests/cli.rs.
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

fn main() -> anyhow::Result<()> {
    color_eyre::install().ok();
    let cli = Cli::parse();
    // For Task 37 we only verify CLI parsing works. Tasks 38–39 actually run the TUI.
    if cli.db.is_some() || cli.start_tab.is_some() {
        // exercised by tests; no-op here.
    }
    println!("sid {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
