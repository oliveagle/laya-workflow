//! Structure & data capabilities — pure Rust, no side effects, no network.
//!
//! * `json`      — structural transforms (pick / merge / patch / path_set / flatten)
//! * `csv`       — CSV parse / generate
//! * `xml`       — tag-text extraction (regex-based, no parser dep)
//! * `markdown`  — heading / code-block / link extraction
//! * `diff`      — line and JSON diffs
//! * `validate`  — lightweight schema checks (required / type / enum / range)
//! * `math`      — expression eval + statistics
//! * `hash`      — sha256 / fnv1a64 / crc32 / md5-lite digests
//! * `graph`     — reachability / BFS / topological sort
//! * `tokenize`  — token counting & splitting for LLM budget estimates
//! * `cron`      — cron expression parse + next-fire computation

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Map, Value};

use super::{effective_op, expand, stringify};

// `math` lives in its own module (see `math.rs`); re-export so existing paths
// like `data::MathCap` / `data::call_math` keep resolving.
pub use super::math::{call_math, eval_expr, MathCap};

pub(super) fn get_text(with: &Value, key: &str) -> Result<String> {
    with.get(key)
        .map(stringify)
        .ok_or_else(|| anyhow!("capability needs {key:?} in 'with'"))
}

// ── json ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct JsonCap {
    /// `pick` | `merge` | `patch` | `path_set` | `flatten` | `sort_keys`
    pub op: String,
}

pub fn call_json(c: &JsonCap, with: &Value, _state: &Value) -> Result<Value> {
    let op = effective_op(&c.op, with, "pick");
    match op.as_str() {
        "pick" => {
            let src = with
                .get("value")
                .cloned()
                .ok_or_else(|| anyhow!("json.pick needs 'value'"))?;
            let keys: Vec<String> = with
                .get("keys")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .ok_or_else(|| anyhow!("json.pick needs 'keys'"))?;
            let mut out = Map::new();
            if let Some(o) = src.as_object() {
                for k in keys {
                    if let Some(v) = o.get(&k) {
                        out.insert(k, v.clone());
                    }
                }
            }
            Ok(json!({ "capability": "json", "op": "pick", "value": Value::Object(out) }))
        }
        "merge" => {
            let a = with.get("a").cloned().unwrap_or_else(|| json!({}));
            let b = with.get("b").cloned().unwrap_or_else(|| json!({}));
            let mut out = a.as_object().cloned().unwrap_or_default();
            if let Some(bo) = b.as_object() {
                for (k, v) in bo {
                    out.insert(k.clone(), v.clone());
                }
            }
            Ok(json!({ "capability": "json", "op": "merge", "value": Value::Object(out) }))
        }
        "patch" => {
            let mut base = with.get("value").cloned().unwrap_or_else(|| json!({}));
            // RFC-7386-ish: null deletes the key
            if let (Some(bo), Some(po)) = (
                base.as_object_mut(),
                with.get("patch").and_then(|p| p.as_object()),
            ) {
                for (k, v) in po {
                    if v.is_null() {
                        bo.remove(k);
                    } else {
                        bo.insert(k.clone(), v.clone());
                    }
                }
            }
            Ok(json!({ "capability": "json", "op": "patch", "value": base }))
        }
        "path_set" => {
            let path = get_text(with, "path")?;
            let mut base = with.get("value").cloned().unwrap_or_else(|| json!({}));
            let newv = with.get("new_value").cloned().unwrap_or(Value::Null);
            set_path(&mut base, &path, newv);
            Ok(json!({ "capability": "json", "op": "path_set", "path": path, "value": base }))
        }
        "flatten" => {
            let src = with.get("value").cloned().unwrap_or_else(|| json!({}));
            let mut out = Map::new();
            flatten_into(&src, "", &mut out);
            Ok(json!({ "capability": "json", "op": "flatten", "value": Value::Object(out) }))
        }
        "sort_keys" => {
            let src = with.get("value").cloned().unwrap_or_else(|| json!({}));
            Ok(json!({ "capability": "json", "op": "sort_keys", "text": stable_json(&src) }))
        }
        other => {
            bail!("json op {other:?} unsupported (pick|merge|patch|path_set|flatten|sort_keys)")
        }
    }
}

fn set_path(v: &mut Value, path: &str, newv: Value) {
    let segs: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segs.is_empty() {
        *v = newv;
        return;
    }
    let mut cur = v;
    for seg in &segs[..segs.len() - 1] {
        if !cur.is_object() {
            *cur = json!({});
        }
        cur = cur
            .as_object_mut()
            .unwrap()
            .entry(seg.to_string())
            .or_insert_with(|| json!({}));
    }
    if !cur.is_object() {
        *cur = json!({});
    }
    cur.as_object_mut()
        .unwrap()
        .insert(segs[segs.len() - 1].to_string(), newv);
}

fn flatten_into(v: &Value, prefix: &str, out: &mut Map<String, Value>) {
    match v {
        Value::Object(o) => {
            for (k, val) in o {
                let p = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_into(val, &p, out);
            }
        }
        other => {
            out.insert(prefix.to_string(), other.clone());
        }
    }
}

fn stable_json(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

// ── csv ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct CsvCap {
    /// `parse` | `generate`
    pub op: String,
    pub delimiter: char,
    pub headers: bool,
}

pub fn call_csv(c: &CsvCap, with: &Value, _state: &Value) -> Result<Value> {
    let op = effective_op(&c.op, with, "pick");
    match op.as_str() {
        "parse" => {
            let text = get_text(with, "text")?;
            let delim = if c.delimiter == '\0' {
                ','
            } else {
                c.delimiter
            };
            let rows = parse_csv(&text, delim);
            let (headers, data) = if c.headers || !rows.is_empty() {
                (
                    rows.first().cloned().unwrap_or_default(),
                    rows.iter().skip(1).cloned().collect::<Vec<_>>(),
                )
            } else {
                (Vec::new(), rows.clone())
            };
            Ok(json!({
                "capability": "csv", "op": "parse",
                "headers": headers, "rows": data, "row_count": data.len(),
            }))
        }
        "generate" => {
            let rows = with
                .get("rows")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow!("csv.generate needs 'rows'"))?;
            let delim = if c.delimiter == '\0' {
                ','
            } else {
                c.delimiter
            };
            let mut out = String::new();
            if c.headers {
                if let Some(h) = with.get("headers").and_then(|v| v.as_array()) {
                    out.push_str(
                        &h.iter()
                            .map(|v| csv_escape(&stringify(v), delim))
                            .collect::<Vec<_>>()
                            .join(&delim.to_string()),
                    );
                    out.push('\n');
                }
            }
            for r in rows {
                let cells: Vec<String> = match r {
                    Value::Array(a) => a.iter().map(|v| csv_escape(&stringify(v), delim)).collect(),
                    other => vec![csv_escape(&stringify(other), delim)],
                };
                out.push_str(&cells.join(&delim.to_string()));
                out.push('\n');
            }
            Ok(
                json!({ "capability": "csv", "op": "generate", "text": out, "lines": out.lines().count() }),
            )
        }
        other => bail!("csv op {other:?} unsupported (parse | generate)"),
    }
}

fn parse_csv(text: &str, delim: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    cell.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                cell.push(ch);
            }
        } else if ch == '"' {
            in_quotes = true;
        } else if ch == delim {
            row.push(std::mem::take(&mut cell));
        } else if ch == '\n' {
            row.push(std::mem::take(&mut cell));
            rows.push(std::mem::take(&mut row));
        } else if ch != '\r' {
            cell.push(ch);
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

fn csv_escape(s: &str, delim: char) -> String {
    if s.contains(delim) || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

// ── xml / markdown (regex-based text extraction) ────────────────────

#[derive(Clone, Debug, Default)]
pub struct XmlCap {
    pub tag: String,
    /// `tags` (list all tag names) | `text` (inner text of `tag`) | `attrs`
    pub op: String,
}

pub fn call_xml(c: &XmlCap, with: &Value, _state: &Value) -> Result<Value> {
    let text = get_text(with, "text")?;
    let op = effective_op(&c.op, with, "pick");
    match op.as_str() {
        "tags" => {
            let re =
                regex_lite::Regex::new(r"<\s*([A-Za-z_][\w:.-]*)").map_err(|e| anyhow!("{e}"))?;
            let mut names: Vec<String> = re
                .captures_iter(&text)
                .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
                .collect();
            names.sort();
            names.dedup();
            Ok(json!({ "capability": "xml", "op": "tags", "tags": names, "count": names.len() }))
        }
        "text" => {
            if c.tag.is_empty() {
                bail!("xml.text needs 'tag'");
            }
            let pat = format!(
                r"(?is)<\s*{}\b[^>]*>(.*?)<\s*/\s*{}\s*>",
                regex_escape(&c.tag),
                regex_escape(&c.tag)
            );
            let re = regex_lite::Regex::new(&pat).map_err(|e| anyhow!("{e}"))?;
            let found: Vec<String> = re
                .captures_iter(&text)
                .filter_map(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
                .collect();
            let count = found.len();
            Ok(
                json!({ "capability": "xml", "op": "text", "tag": c.tag, "texts": found, "count": count }),
            )
        }
        "attrs" => {
            if c.tag.is_empty() {
                bail!("xml.attrs needs 'tag'");
            }
            let pat = format!(r"(?is)<\s*{}\b([^>]*)>", regex_escape(&c.tag));
            let re = regex_lite::Regex::new(&pat).map_err(|e| anyhow!("{e}"))?;
            let attrs: Vec<String> = re
                .captures_iter(&text)
                .filter_map(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
                .collect();
            let count = attrs.len();
            Ok(
                json!({ "capability": "xml", "op": "attrs", "tag": c.tag, "attrs": attrs, "count": count }),
            )
        }
        other => bail!("xml op {other:?} unsupported (tags | text | attrs)"),
    }
}

fn regex_escape(s: &str) -> String {
    s.chars()
        .map(|c| {
            if "\\^$.|?*+()[]{}".contains(c) {
                format!("\\{c}")
            } else {
                c.to_string()
            }
        })
        .collect()
}

#[derive(Clone, Debug, Default)]
pub struct MarkdownCap {
    /// `headings` | `code_blocks` | `links` | `outline`
    pub op: String,
}

pub fn call_markdown(c: &MarkdownCap, with: &Value, _state: &Value) -> Result<Value> {
    let text = get_text(with, "text")?;
    let lines: Vec<&str> = text.lines().collect();
    let op = effective_op(&c.op, with, "pick");
    match op.as_str() {
        "headings" => {
            let mut hs = Vec::new();
            let mut in_fence = false;
            for l in &lines {
                if l.trim_start().starts_with("```") {
                    in_fence = !in_fence;
                    continue;
                }
                if in_fence {
                    continue;
                }
                let t = l.trim_start();
                if let Some(rest) = t.strip_prefix('#') {
                    let level = 1 + rest.chars().take_while(|c| *c == '#').count();
                    let title = rest.trim_start_matches('#').trim().to_string();
                    if !title.is_empty() {
                        hs.push(json!({ "level": level, "title": title }));
                    }
                }
            }
            let count = hs.len();
            Ok(
                json!({ "capability": "markdown", "op": "headings", "headings": hs, "count": count }),
            )
        }
        "code_blocks" => {
            let mut blocks = Vec::new();
            let mut cur: Option<(String, Vec<String>)> = None;
            for l in &lines {
                if let Some(rest) = l.trim_start().strip_prefix("```") {
                    match cur.take() {
                        None => cur = Some((rest.trim().to_string(), Vec::new())),
                        Some((lang, body)) => {
                            blocks.push(json!({ "lang": lang, "code": body.join("\n") }))
                        }
                    }
                } else if let Some((_, body)) = cur.as_mut() {
                    body.push(l.to_string());
                }
            }
            let count = blocks.len();
            Ok(
                json!({ "capability": "markdown", "op": "code_blocks", "blocks": blocks, "count": count }),
            )
        }
        "links" => {
            let re =
                regex_lite::Regex::new(r"\[([^\]]*)\]\(([^)]+)\)").map_err(|e| anyhow!("{e}"))?;
            let links: Vec<Value> = re
                .captures_iter(&text)
                .map(|c| {
                    json!({
                        "text": c.get(1).map(|m| m.as_str()).unwrap_or(""),
                        "url": c.get(2).map(|m| m.as_str()).unwrap_or(""),
                    })
                })
                .collect();
            let count = links.len();
            Ok(json!({ "capability": "markdown", "op": "links", "links": links, "count": count }))
        }
        "outline" => {
            let hs = call_markdown(
                &MarkdownCap {
                    op: "headings".into(),
                },
                with,
                _state,
            )?;
            let titles: Vec<String> = hs["headings"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|h| {
                            let lvl = h["level"].as_u64().unwrap_or(1) as usize;
                            format!(
                                "{}{}",
                                "  ".repeat(lvl - 1),
                                h["title"].as_str().unwrap_or("")
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(
                json!({ "capability": "markdown", "op": "outline", "outline": titles, "count": titles.len() }),
            )
        }
        other => {
            bail!("markdown op {other:?} unsupported (headings | code_blocks | links | outline)")
        }
    }
}

// ── diff ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct DiffCap {
    /// `lines` | `json`
    pub op: String,
}

pub fn call_diff(c: &DiffCap, with: &Value, _state: &Value) -> Result<Value> {
    let op = effective_op(&c.op, with, "pick");
    match op.as_str() {
        "lines" => {
            let a = get_text(with, "a")?;
            let b = get_text(with, "b")?;
            let al: Vec<&str> = a.lines().collect();
            let bl: Vec<&str> = b.lines().collect();
            let mut added = Vec::new();
            let mut removed = Vec::new();
            let mut changed = Vec::new();
            let n = al.len().max(bl.len());
            for i in 0..n {
                match (al.get(i), bl.get(i)) {
                    (Some(x), Some(y)) if x == y => {}
                    (Some(x), Some(y)) => {
                        changed.push(json!({ "line": i + 1, "from": x, "to": y }))
                    }
                    (Some(x), None) => removed.push(json!({ "line": i + 1, "text": x })),
                    (None, Some(y)) => added.push(json!({ "line": i + 1, "text": y })),
                    (None, None) => {}
                }
            }
            Ok(json!({
                "capability": "diff", "op": "lines",
                "added": added, "removed": removed, "changed": changed,
                "identical": added.is_empty() && removed.is_empty() && changed.is_empty(),
            }))
        }
        "json" => {
            let a = with.get("a").cloned().unwrap_or(Value::Null);
            let b = with.get("b").cloned().unwrap_or(Value::Null);
            let mut deltas = Vec::new();
            json_delta(&a, &b, "", &mut deltas);
            let count = deltas.len();
            Ok(
                json!({ "capability": "diff", "op": "json", "deltas": deltas, "count": count,
                       "identical": count == 0 }),
            )
        }
        other => bail!("diff op {other:?} unsupported (lines | json)"),
    }
}

fn json_delta(a: &Value, b: &Value, path: &str, out: &mut Vec<Value>) {
    match (a, b) {
        (Value::Object(ao), Value::Object(bo)) => {
            for (k, av) in ao {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match bo.get(k) {
                    Some(bv) => json_delta(av, bv, &p, out),
                    None => out.push(json!({ "path": p, "from": av, "to": null, "op": "remove" })),
                }
            }
            for (k, bv) in bo {
                if !ao.contains_key(k) {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    out.push(json!({ "path": p, "from": null, "to": bv, "op": "add" }));
                }
            }
        }
        _ if a == b => {}
        _ => out.push(json!({ "path": path, "from": a, "to": b, "op": "change" })),
    }
}

// ── validate (lightweight schema) ───────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct ValidateCap;

pub fn call_validate(_c: &ValidateCap, with: &Value, _state: &Value) -> Result<Value> {
    let value = with
        .get("value")
        .cloned()
        .ok_or_else(|| anyhow!("validate needs 'value'"))?;
    let schema = with
        .get("schema")
        .cloned()
        .ok_or_else(|| anyhow!("validate needs 'schema'"))?;
    let mut errors: Vec<String> = Vec::new();
    check_schema(&value, &schema, "", &mut errors);
    Ok(json!({
        "capability": "validate",
        "valid": errors.is_empty(),
        "errors": errors,
        "error_count": errors.len(),
    }))
}

fn check_schema(v: &Value, s: &Value, path: &str, errs: &mut Vec<String>) {
    let p = if path.is_empty() { "$" } else { path };
    if let Some(t) = s.get("type").and_then(|x| x.as_str()) {
        let ok = match t {
            "object" => v.is_object(),
            "array" => v.is_array(),
            "string" => v.is_string(),
            "number" => v.is_number(),
            "integer" => v.as_i64().is_some() || v.as_u64().is_some(),
            "boolean" => v.is_boolean(),
            "null" => v.is_null(),
            _ => true,
        };
        if !ok {
            errs.push(format!("{p}: expected {t}, got {}", kind_of(v)));
        }
    }
    if let Some(req) = s.get("required").and_then(|x| x.as_array()) {
        if let Some(o) = v.as_object() {
            for r in req.iter().filter_map(|x| x.as_str()) {
                if !o.contains_key(r) {
                    errs.push(format!("{p}: missing required key {r:?}"));
                }
            }
        }
    }
    if let Some(en) = s.get("enum").and_then(|x| x.as_array()) {
        if !en.iter().any(|e| e == v) {
            errs.push(format!("{p}: value {v} not in enum"));
        }
    }
    if let Some(mn) = s.get("minimum").and_then(|x| x.as_f64()) {
        if v.as_f64().map(|n| n < mn).unwrap_or(false) {
            errs.push(format!("{p}: {v} < minimum {mn}"));
        }
    }
    if let Some(mx) = s.get("maximum").and_then(|x| x.as_f64()) {
        if v.as_f64().map(|n| n > mx).unwrap_or(false) {
            errs.push(format!("{p}: {v} > maximum {mx}"));
        }
    }
    if let Some(min) = s.get("min_length").and_then(|x| x.as_u64()) {
        if v.as_str()
            .map(|t| (t.chars().count() as u64) < min)
            .unwrap_or(false)
        {
            errs.push(format!("{p}: shorter than min_length {min}"));
        }
    }
    if let Some(props) = s.get("properties").and_then(|x| x.as_object()) {
        if let Some(o) = v.as_object() {
            for (k, sub) in props {
                if let Some(sv) = o.get(k) {
                    let cp = format!("{p}.{k}");
                    check_schema(sv, sub, &cp, errs);
                }
            }
        }
    }
    if let Some(items) = s.get("items") {
        if let Some(a) = v.as_array() {
            for (i, it) in a.iter().enumerate() {
                let cp = format!("{p}[{i}]");
                check_schema(it, items, &cp, errs);
            }
        }
    }
}

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

// ── hash ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct HashCap {
    /// `sha256` | `sha256_hex` | `fnv1a64` | `crc32`
    pub op: String,
}

pub fn call_hash(c: &HashCap, with: &Value, _state: &Value) -> Result<Value> {
    let text = get_text(with, "text")?;
    let op = effective_op(&c.op, with, "pick");
    match op.as_str() {
        "sha256" | "sha256_hex" => Ok(json!({
            "capability": "hash", "op": "sha256",
            "hex": super::net::sha256_hex(text.as_bytes()),
            "bytes": text.len(),
        })),
        "fnv1a64" => Ok(json!({
            "capability": "hash", "op": "fnv1a64",
            "hex": format!("{:016x}", fnv1a64(text.as_bytes())),
        })),
        "crc32" => Ok(json!({
            "capability": "hash", "op": "crc32",
            "hex": format!("{:08x}", crc32(text.as_bytes())),
        })),
        other => bail!("hash op {other:?} unsupported (sha256 | fnv1a64 | crc32)"),
    }
}

fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

// ── graph ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct GraphCap {
    /// `reachable` | `bfs` | `toposort`
    pub op: String,
}

pub fn call_graph(c: &GraphCap, with: &Value, _state: &Value) -> Result<Value> {
    let edges = with
        .get("edges")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("graph capability needs 'edges' [[from,to], …]"))?;
    let mut adj: Map<String, Value> = Map::new();
    let mut nodes: Vec<String> = Vec::new();
    for e in edges {
        let pair = e
            .as_array()
            .ok_or_else(|| anyhow!("graph edge must be [from, to]"))?;
        let from = pair.first().map(stringify).unwrap_or_default();
        let to = pair.get(1).map(stringify).unwrap_or_default();
        for n in [&from, &to] {
            if !nodes.contains(n) {
                nodes.push(n.clone());
            }
        }
        let lst = adj.entry(from).or_insert_with(|| json!([]));
        if let Some(a) = lst.as_array_mut() {
            a.push(json!(to));
        }
    }
    let start = with.get("start").map(stringify).unwrap_or_default();
    let op = effective_op(&c.op, with, "pick");
    match op.as_str() {
        "reachable" | "bfs" => {
            if start.is_empty() {
                bail!("graph.{:?} needs 'start'", c.op);
            }
            let mut seen: Vec<String> = vec![start.clone()];
            let mut queue = vec![start.clone()];
            let mut order: Vec<String> = Vec::new();
            while let Some(n) = queue.pop() {
                order.push(n.clone());
                if let Some(Value::Array(nb)) = adj.get(&n) {
                    for t in nb.iter().map(stringify) {
                        if !seen.contains(&t) {
                            seen.push(t.clone());
                            queue.push(t);
                        }
                    }
                }
            }
            Ok(json!({
                "capability": "graph", "op": c.op, "start": start,
                "order": order, "reachable": seen, "count": seen.len(),
            }))
        }
        "toposort" => {
            // Kahn's algorithm; detects cycles
            let mut indeg: std::collections::HashMap<String, i64> =
                nodes.iter().map(|n| (n.clone(), 0)).collect();
            for e in edges {
                let p = e.as_array().unwrap();
                let to = p.get(1).map(stringify).unwrap_or_default();
                *indeg.entry(to).or_insert(0) += 1;
            }
            let mut ready: Vec<String> = indeg
                .iter()
                .filter(|(_, d)| **d == 0)
                .map(|(n, _)| n.clone())
                .collect();
            ready.sort();
            let mut out: Vec<String> = Vec::new();
            while let Some(n) = ready.pop() {
                out.push(n.clone());
                if let Some(Value::Array(nb)) = adj.get(&n) {
                    for t in nb.iter().map(stringify) {
                        let d = indeg.entry(t.clone()).or_insert(0);
                        *d -= 1;
                        if *d == 0 {
                            ready.push(t);
                            ready.sort();
                        }
                    }
                }
            }
            let cyclic = out.len() != nodes.len();
            Ok(json!({
                "capability": "graph", "op": "toposort", "order": out,
                "cyclic": cyclic, "node_count": nodes.len(),
            }))
        }
        other => bail!("graph op {other:?} unsupported (reachable | bfs | toposort)"),
    }
}

// ── tokenize ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct TokenizeCap {
    /// `count` (heuristic) | `split`
    pub op: String,
    /// Chars-per-token ratio used by the heuristic (default 4.0).
    pub chars_per_token: f64,
}

pub fn call_tokenize(c: &TokenizeCap, with: &Value, _state: &Value) -> Result<Value> {
    let text = get_text(with, "text")?;
    let cpt = if c.chars_per_token <= 0.0 {
        4.0
    } else {
        c.chars_per_token
    };
    let op = effective_op(&c.op, with, "pick");
    match op.as_str() {
        "count" => {
            let chars = text.chars().count() as f64;
            let words = text.split_whitespace().count();
            let est = (chars / cpt).round() as u64;
            Ok(json!({
                "capability": "tokenize", "op": "count",
                "chars": chars as u64, "words": words,
                "estimated_tokens": est,
                "method": format!("chars/{cpt}"),
            }))
        }
        "split" => {
            // whitespace+punctuation heuristic chunking, cap slice length
            let max = with
                .get("chunk_chars")
                .and_then(|v| v.as_u64())
                .unwrap_or(48) as usize;
            let mut chunks: Vec<String> = Vec::new();
            let mut cur = String::new();
            for ch in text.chars() {
                if ch.is_whitespace() {
                    if !cur.is_empty() {
                        chunks.push(std::mem::take(&mut cur));
                    }
                } else {
                    cur.push(ch);
                    if cur.chars().count() >= max {
                        chunks.push(std::mem::take(&mut cur));
                    }
                }
            }
            if !cur.is_empty() {
                chunks.push(cur);
            }
            let count = chunks.len();
            Ok(json!({ "capability": "tokenize", "op": "split", "chunks": chunks, "count": count }))
        }
        other => bail!("tokenize op {other:?} unsupported (count | split)"),
    }
}

// ── cron ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct CronCap {
    pub expr: String,
}

/// Supports the common 5-field form (`m h dom mon dow`) with `*`, `a`, `a,b`,
/// `a-b`, and `*/n`. Sufficient for "next fire time" scheduling checks.
pub fn call_cron(c: &CronCap, with: &Value, _state: &Value) -> Result<Value> {
    let expr = if c.expr.is_empty() {
        with.get("expr").map(stringify).unwrap_or_default()
    } else {
        stringify(&expand(&Value::String(c.expr.clone()), _state, with))
    };
    if expr.is_empty() {
        bail!("cron capability needs 'expr' (5 fields)");
    }
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        bail!(
            "cron expr must have 5 fields (got {}): {expr:?}",
            fields.len()
        );
    }
    let from = with
        .get("from_epoch")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });
    let next = next_cron(fields.as_slice(), from, 366 * 24 * 3600);
    Ok(json!({
        "capability": "cron", "expr": expr, "from_epoch": from,
        "next_epoch": next, "matched": next.is_some(),
    }))
}

fn field_match(f: &str, v: u32, lo: u32, hi: u32) -> bool {
    if f == "*" {
        return true;
    }
    for part in f.split(',') {
        if let Some(step) = part.strip_prefix("*/") {
            if let Ok(n) = step.parse::<u32>() {
                if n > 0 && (v - lo) % n == 0 {
                    return true;
                }
                continue;
            }
        }
        if let Some((a, b)) = part.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
                if v >= a && v <= b {
                    return true;
                }
            }
            continue;
        }
        if let Ok(n) = part.parse::<u32>() {
            if n == v {
                return true;
            }
        }
        let _ = hi;
    }
    false
}

fn next_cron(fields: &[&str], from: i64, horizon: i64) -> Option<i64> {
    let (m_f, h_f, dom_f, mon_f, dow_f) = (fields[0], fields[1], fields[2], fields[3], fields[4]);
    let mut t = from + 60 - (from % 60); // next whole minute
    let end = from + horizon;
    while t <= end {
        let (_, mo, d, hh, mm, _) = crate::capability::local::civil_from_epoch_pub(t);
        let dow = weekday_from_days(t.div_euclid(86_400));
        if field_match(mon_f, mo, 1, 12)
            && field_match(dom_f, d, 1, 31)
            && field_match(dow_f, dow, 0, 6)
            && field_match(h_f, hh, 0, 23)
            && field_match(m_f, mm, 0, 59)
        {
            return Some(t);
        }
        t += 60;
    }
    None
}

/// 0 = Sunday … 6 = Saturday (1970-01-01 was a Thursday).
fn weekday_from_days(days: i64) -> u32 {
    (((days % 7) + 11) % 7) as u32
}
