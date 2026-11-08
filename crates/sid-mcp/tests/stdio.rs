//! Spawns the real `sid-mcp` binary and speaks newline-delimited JSON-RPC over stdio:
//! `initialize` -> `tools/list` -> `tools/call crate_map`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

const EXPECTED_TOOLS: &[&str] = &[
    "adapter_check",
    "backlog",
    "capture_ui",
    "crate_map",
    "docs_index",
    "find_symbol",
    "gate",
    "recent_commits",
    "store_inspect",
];

#[test]
fn initialize_list_and_call_over_stdio() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_sid-mcp"))
        .env("SID_ROOT", &root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn sid-mcp");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    // Reader thread -> channel, so every wait has a timeout.
    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                let _ = tx.send(v);
            }
        }
    });
    let mut send = |v: Value| {
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    };
    let recv_id = |id: u64| -> Value {
        loop {
            let v = rx
                .recv_timeout(Duration::from_secs(60))
                .expect("response within 60s");
            if v["id"] == json!(id) {
                return v;
            }
        }
    };

    send(
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},
        "clientInfo":{"name":"test","version":"0"}}}),
    );
    let init = recv_id(1);
    assert_eq!(init["result"]["serverInfo"]["name"], "sid-mcp", "{init}");
    assert!(init["result"]["capabilities"]["tools"].is_object());

    send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let list = recv_id(2);
    let mut names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, EXPECTED_TOOLS);
    for t in list["result"]["tools"].as_array().unwrap() {
        assert!(t["inputSchema"].is_object(), "{t}");
        assert!(!t["description"].as_str().unwrap_or("").is_empty());
    }

    send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"crate_map","arguments":{}}}));
    let call = recv_id(3);
    let body = call["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    assert!(body.contains("sid-core"), "{body}");
    assert!(body.contains("sid-mcp"), "{body}");
    assert_ne!(call["result"]["isError"], json!(true));

    // A tool failure is a tool result with isError, not a protocol error.
    send(json!({"jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"find_symbol","arguments":{"query":"("}}}));
    let bad = recv_id(4);
    assert_eq!(bad["result"]["isError"], json!(true), "{bad}");

    let _ = child.kill();
    let _ = child.wait();
}
