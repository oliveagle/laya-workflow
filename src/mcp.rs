//! Generic MCP (Model Context Protocol) stdio server — the engine's
//! extension seam for tool groups.
//!
//! The Rust code in this module is the **transport**: JSON-RPC 2.0 framing,
//! the initialize / tools/list / tools/call / notifications handshake, and
//! a registry of tool sets. Domain logic (what each tool *does*) lives in
//! `McpToolSet` implementations — currently `laya-mem` (one set of four
//! memory-gate tools) and any future group built against the same trait.
//
//! Wire format: one JSON-RPC 2.0 object per line, terminated by `\n`.
//! Methods handled:
//
//! | method                       | reply?                |
//! |------------------------------|-----------------------|
//! | `initialize`                 | yes                   |
//! | `notifications/initialized`  | no (fire-and-forget)  |
//! | `tools/list`                 | yes                   |
//! | `tools/call`                 | yes                   |
//! | `ping`                       | yes                   |
//!
//! Errors use the JSON-RPC error codes (`-32601 method not found`,
//! `-32602 invalid params`, `-32603 internal`).
//!
//! ## Adding a new tool group
//!
//! ```rust,ignore
//! use laya_workflow::mcp::{McpTool, McpToolSet, serve_stdio};
//!
//! pub struct MyTools;
//! impl McpToolSet for MyTools {
//!     fn group_name(&self) -> &str { "my-tools" }
//!     fn tools(&self) -> Vec<Box<dyn McpTool>> { vec![...] }
//! }
//!
//! // then in the CLI:
//! serve_stdio(vec![Box::new(MyTools)]);
//! ```

use std::io::{self, BufRead, Write};
use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// One MCP tool exposed by a tool group. The handle is cheap (Arc) so the
/// server can serve many concurrent tool calls from the same registry
/// without cloning the implementation per request.
pub type DynTool = Arc<dyn McpTool>;

/// A single MCP tool. Implementors describe their name, schema, and
/// handler; the server handles the JSON-RPC framing.
pub trait McpTool: Send + Sync {
    /// Stable tool name (e.g. `"laya_mem_assess"`).
    fn name(&self) -> &str;
    /// One-paragraph human description shown to the agent.
    fn description(&self) -> &str;
    /// JSON Schema (inputSchema) describing the tool's arguments.
    fn input_schema(&self) -> Value;
    /// Execute the tool with the given arguments; return a JSON-serializable
    /// result. Errors are returned as `isError: true` tool results.
    fn call(&self, args: &Value) -> Result<Value>;
}

/// A *group* of related tools sharing a name prefix or domain. The server
/// concatenates `tools()` from every registered set when answering
/// `tools/list`.
pub trait McpToolSet: Send + Sync {
    /// Group identifier (e.g. `"laya-mem"`); informational only — the server
    /// exposes each tool by its own `McpTool::name`.
    fn group_name(&self) -> &str;

    /// All tools this set exposes.
    fn tools(&self) -> Vec<DynTool>;
}

// ─── JSON-RPC transport ────────────────────────────────────────────────────

fn write_message(v: &Value) {
    let s = serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string());
    let stdout = io::stdout();
    let mut h = stdout.lock();
    let _ = h.write_all(s.as_bytes());
    let _ = h.write_all(b"\n");
    let _ = h.flush();
}

fn send_response(id: Value, result: Value) {
    write_message(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
}

fn send_error(id: Value, code: i64, message: &str) {
    write_message(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    }));
}

fn send_tool_result(id: Value, structured: Value) {
    let text = serde_json::to_string_pretty(&structured).unwrap_or_else(|_| "{}".to_string());
    send_response(
        id,
        json!({
            "content": [{"type": "text", "text": text}],
            "structuredContent": structured,
            "isError": false,
        }),
    );
}

/// Tool-level error result: `isError: true` so MCP clients (and test
/// scripts) can distinguish a tool failure from a successful empty result.
fn send_tool_error(id: Value, msg: &str) {
    send_response(
        id,
        json!({
            "content": [{"type": "text", "text": msg}],
            "structuredContent": { "error": msg },
            "isError": true,
        }),
    );
}

fn handle_request(server_info: &ServerInfo, sets: &[Box<dyn McpToolSet>], msg: &Value) {
    let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let id = msg.get("id").cloned().unwrap_or(Value::Null);

    match method {
        "initialize" => {
            let requested = msg
                .get("params")
                .and_then(|p| p.get("protocolVersion"))
                .and_then(|v| v.as_str())
                .unwrap_or(PROTOCOL_VERSION);
            send_response(
                id,
                json!({
                    "protocolVersion": requested,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": server_info.name, "version": server_info.version },
                }),
            );
        }
        "ping" => send_response(id, json!({})),
        "tools/list" => {
            let mut tools: Vec<Value> = Vec::new();
            for set in sets {
                for t in set.tools() {
                    tools.push(json!({
                        "name": t.name(),
                        "description": t.description(),
                        "inputSchema": t.input_schema(),
                    }));
                }
            }
            send_response(id, json!({ "tools": tools }));
        }
        "tools/call" => {
            let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let mut handler: Option<DynTool> = None;
            for set in sets {
                for t in set.tools() {
                    if t.name() == name {
                        handler = Some(Arc::clone(&t));
                        break;
                    }
                }
                if handler.is_some() {
                    break;
                }
            }
            match handler {
                None => send_error(id, -32602, &format!("unknown tool: {name}")),
                Some(t) => match t.call(&args) {
                    Ok(v) => send_tool_result(id, v),
                    Err(e) => send_tool_error(id, &format!("{e:#}")),
                },
            }
        }
        "" => {} // notification without method — ignore silently
        other => send_error(id, -32601, &format!("method not found: {other}")),
    }
}

#[derive(Clone, Debug)]
pub struct ServerInfo {
    pub name: &'static str,
    pub version: &'static str,
}

/// Start the generic MCP stdio server. Reads JSON-RPC from stdin, writes
/// JSON-RPC responses to stdout. Blocks until stdin EOF.
///
/// `server_info` is reported to clients during the initialize handshake;
/// `sets` is the list of tool groups the server will expose. The order of
/// `sets` is preserved in `tools/list` output.
pub fn serve_stdio(server_info: ServerInfo, sets: Vec<Box<dyn McpToolSet>>) -> Result<()> {
    eprintln!(
        "[mcp] server: {}@{}  tool_sets: {}",
        server_info.name,
        server_info.version,
        sets.len()
    );
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[mcp] parse error: {e}");
                continue;
            }
        };
        // Notifications (no `id`) → fire-and-forget, no reply.
        if msg.get("id").is_none() && msg.get("method").is_some() {
            continue;
        }
        handle_request(&server_info, &sets, &msg);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoTool;
    impl McpTool for EchoTool {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "Echoes its input" }
        fn input_schema(&self) -> Value { json!({"type": "object"}) }
        fn call(&self, args: &Value) -> Result<Value> { Ok(args.clone()) }
    }

    struct EchoSet;
    impl McpToolSet for EchoSet {
        fn group_name(&self) -> &str { "echo" }
        fn tools(&self) -> Vec<DynTool> { vec![Arc::new(EchoTool)] }
    }

    #[test]
    fn tool_set_exposes_tools() {
        let s = EchoSet;
        let tools = s.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "echo");
    }
}
