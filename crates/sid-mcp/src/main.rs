//! `sid-mcp` binary: serves the tools over stdio. Logs go to stderr only.

use rmcp::{ServiceExt, transport::stdio};

#[tokio::main]
async fn main() {
    let root = match sid_mcp::util::find_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("sid-mcp: {e}");
            std::process::exit(2);
        }
    };
    let server = sid_mcp::SidMcp::new(root);
    let service = match server.serve(stdio()).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sid-mcp: failed to start: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = service.waiting().await {
        eprintln!("sid-mcp: {e}");
        std::process::exit(1);
    }
}
