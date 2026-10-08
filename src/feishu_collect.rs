//! `feishu-collect`: collect unread Feishu messages across every chat, for the
//! unread digest. Rust replacement for the inline `python3 -c` in
//! `websites/feishu.com/plugin/collect.sh` — same contract, no Python.

use anyhow::Result;
use serde_json::{json, Value};
use std::io::Read;
use std::process::Command;

fn lark_cli(args: &[&str]) -> Value {
    let out = Command::new("lark-cli")
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    serde_json::from_str(&out).unwrap_or(Value::Null)
}

fn chat_messages(cid: &str, page: usize) -> Vec<Value> {
    let m = lark_cli(&[
        "im", "+chat-messages-list", "--chat-id", cid, "--page-size", &page.to_string(),
        "--order", "desc", "--no-reactions", "--format", "json",
    ]);
    m.get("data")
        .and_then(|d| d.get("messages"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|x| x.get("deleted").and_then(Value::as_bool) != Some(true))
        .collect()
}

fn read_status(ids: &[String]) -> std::collections::HashSet<String> {
    let st = lark_cli(&[
        "im", "+messages-read-status", "--as", "user", "--message-ids",
        &ids.join(","), "--format", "json",
    ]);
    st.get("data")
        .and_then(|d| d.get("items"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| i.get("is_read").and_then(Value::as_bool) != Some(true))
        .filter_map(|i| i.get("message_id").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn resolve_knobs(args: &[String]) -> (usize, usize) {
    let s = |i: usize| args.get(i).map(|s| s.as_str()).unwrap_or("");
    let chat = match s(0) { "" | "null" => 15, v => v.parse().unwrap_or(15) };
    let page = match s(1) { "" | "null" => 20, v => v.parse().unwrap_or(20) };
    (chat, page)
}

pub fn run(args: &[String]) -> Result<()> {
    if args.first().map(|s| s.as_str()) == Some("--print-knobs") {
        let (c, p) = resolve_knobs(&args[1..].to_vec());
        println!("{c}/{p}");
        return Ok(());
    }
    let (chats_limit, page) = resolve_knobs(args);

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let d: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    if d.get("ok").and_then(Value::as_bool) != Some(true) {
        let err = d
            .get("error")
            .map(|e| e.to_string())
            .unwrap_or_else(|| "no error".into());
        println!("{}", json!({"ok": false, "err": err}));
        std::process::exit(1);
    }
    let chats = d
        .get("data")
        .and_then(|x| x.get("chats"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for c in chats.into_iter().take(chats_limit) {
        let cid = c.get("chat_id").and_then(Value::as_str).unwrap_or("").to_string();
        if cid.is_empty() {
            continue;
        }
        let mode = c.get("chat_mode").and_then(Value::as_str).unwrap_or("").to_string();
        let name = c.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let msgs = chat_messages(&cid, page);
        if msgs.is_empty() {
            continue;
        }
        let ids: Vec<String> = msgs
            .iter()
            .take(50)
            .filter_map(|m| m.get("message_id").and_then(Value::as_str).map(str::to_string))
            .collect();
        let unread_ids = read_status(&ids);
        let unread: Vec<Value> = msgs
            .into_iter()
            .filter(|m| {
                m.get("message_id")
                    .and_then(Value::as_str)
                    .map(|s| unread_ids.contains(s))
                    .unwrap_or(false)
            })
            .map(|mut x| {
                x["chat_name"] = json!(name.clone());
                x["chat_id"] = json!(cid.clone());
                x["chat_type"] = json!(mode.clone());
                x["app_link"] = x.get("message_app_link").cloned().unwrap_or(Value::Null);
                x
            })
            .collect();
        if !unread.is_empty() {
            out.push(json!({
                "chat_id": cid, "chat_name": name, "chat_type": mode, "unread": unread
            }));
        }
    }
    println!("{}", json!({"ok": true, "chats": out}));
    Ok(())
}
