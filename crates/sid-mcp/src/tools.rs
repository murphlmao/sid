//! MCP tool implementations.
//!
//! Each submodule implements one tool: takes typed params (or a path
//! string), reads from the workspace, returns a typed result.
//!
//! Tools are deliberately thin — they read filesystem state, parse it,
//! and return a struct. Workflow logic lives in skills/agents that
//! compose multiple tool calls.

pub mod coverage;
pub mod crate_info;
pub mod criterion;
pub mod doc_test_gaps;
pub mod find_pub_item;
pub mod gate_status;
pub mod plan_status;
pub mod recent_commits;
