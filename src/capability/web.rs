//! Web-research capabilities: `web_search` and `web_fetch`.
//!
//! These give a workflow the ability to look things up on the internet at
//! runtime — something none of the other kinds provide (the existing `http`
//! kind can reach an API you already know, but nothing turns a *query* into
//! results or a *page* into readable text).
//!
//! * `web_search` — send a query to a search endpoint, return structured
//!   results `[{title, url, snippet}]`. The endpoint is configurable so the
//!   same capability works against a local mock, a self-hosted SearxNG, or a
//!   hosted API. Nothing here is hard-wired to a public provider.
//! * `web_fetch`  — fetch a URL and return readable text (`format: text`),
//!   the raw body (`format: raw`) or a light markdown conversion
//!   (`format: markdown`), plus the response status and content type.
//!
//! Egress is governed by `policy.allow_hosts`, exactly like every other
//! network kind. Because these capabilities take URLs from workflow *data*
//! rather than only from the spec, they apply two extra guards:
//!
//! 1. only `http` / `https` URLs are accepted (no `file:` / `gopher:` etc.);
//! 2. redirects are never followed automatically — a redirect to a host that
//!    `allow_hosts` does not list must not be reachable by hopping through the
//!    first hop. A returned `3xx` is reported as-is so the workflow can decide.
//!
//! Bodies are capped by `max_bytes` (and by `policy.max_output`), and the
//! result says `truncated: true` when a cap was hit so nothing silently
//! pretends to be complete.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use super::{bounded_timeout, check_host, expand, stringify, truncate, Policy};

/// Default cap on how much of a response body we will read (256 KiB).
const DEFAULT_MAX_BYTES: usize = 256 << 10;

/// Default cap on the number of search results we will parse.
const DEFAULT_MAX_RESULTS: usize = 10;

// ── web_search ──────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct WebSearchCap {
    /// Search endpoint. Accepts either a full URL template containing `{query}`
    /// or a base URL to which `?q=` is appended.
    pub endpoint: String,
    /// HTTP method (GET puts the query in `?q=`, POST sends `{"q": ...}`).
    pub method: String,
    /// Extra request headers; values are expanded per call (use `${secret.…}`).
    pub headers: serde_json::Map<String, Value>,
    /// JSON pointer to the array of results in the response (e.g. `/results`).
    /// Empty means "try the usual suspects".
    pub results_path: String,
    /// Query-string key used when `endpoint` has no `{query}` placeholder and
    /// the method is GET. Providers differ: TinyFish takes `query`, most others
    /// take `q`.
    pub query_param: String,
    /// Field names to read out of each result object.
    pub title_field: String,
    pub url_field: String,
    pub snippet_field: String,
    pub timeout_ms: u64,
    pub max_results: usize,
    pub max_bytes: usize,
}

pub fn call_web_search(
    c: &WebSearchCap,
    with: &Value,
    state: &Value,
    policy: &Policy,
) -> Result<Value> {
    let query = with
        .get("query")
        .map(stringify)
        .map(|q| stringify(&expand(&Value::String(q), state, with)))
        .unwrap_or_default();
    if query.trim().is_empty() {
        bail!("web_search needs 'query'");
    }
    let endpoint = stringify(&expand(&Value::String(c.endpoint.clone()), state, with));
    if endpoint.is_empty() {
        bail!("web_search needs 'endpoint'");
    }

    let method = if c.method.is_empty() {
        "GET"
    } else {
        c.method.as_str()
    };
    let qkey = if c.query_param.is_empty() {
        "q"
    } else {
        c.query_param.as_str()
    };
    let url = if endpoint.contains("{query}") {
        endpoint.replace("{query}", &urlencode(&query))
    } else if method.eq_ignore_ascii_case("GET") {
        let sep = if endpoint.contains('?') { '&' } else { '?' };
        format!("{endpoint}{sep}{qkey}={}", urlencode(&query))
    } else {
        endpoint.clone()
    };
    check_url(&url, policy)?;

    let max_results = if c.max_results == 0 {
        DEFAULT_MAX_RESULTS
    } else {
        c.max_results
    };
    let max_bytes = byte_cap(c.max_bytes, policy);
    let agent = ureq::AgentBuilder::new()
        .timeout(bounded_timeout(
            if c.timeout_ms == 0 {
                15_000
            } else {
                c.timeout_ms
            },
            policy,
        ))
        // Search endpoints are trusted config, but a redirect still moves the
        // request to another host — surface it instead of following blindly.
        .redirects(0)
        .build();

    let mut req = if method.eq_ignore_ascii_case("POST") {
        agent.post(&url).set("content-type", "application/json")
    } else {
        agent.get(&url)
    };
    for (k, v) in &c.headers {
        req = req.set(k, &stringify(&expand(v, state, with)));
    }
    let resp = if method.eq_ignore_ascii_case("POST") {
        let mut payload = json!({ qkey: query.clone() });
        payload["query"] = json!(query.clone());
        req.send_string(&payload.to_string())
    } else {
        req.call()
    };

    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            // A non-2xx is still a real answer; report it rather than throwing
            // away the status/body the workflow may want to branch on.
            let body = read_body(r.into_reader(), max_bytes)?;
            return Ok(json!({
                "capability": "web_search", "query": query, "status": code as i64,
                "results": [], "count": 0,
                "body": truncate(body.0, policy.max_output), "truncated": body.1,
            }));
        }
        Err(e) => return Err(anyhow!("web_search request failed: {e}")),
    };

    let status = resp.status() as i64;
    let content_type = resp.header("content-type").unwrap_or("").to_string();
    let (body, truncated) = read_body(resp.into_reader(), max_bytes)?;

    let results = parse_results(&body, c, max_results);
    Ok(json!({
        "capability": "web_search",
        "query": query,
        "status": status,
        "content_type": content_type,
        "results": results,
        "count": results.len(),
        "truncated": truncated,
    }))
}

/// Pull `[{title, url, snippet}]` out of a search response.
///
/// Handles the shapes we can expect without a dependency: a top-level array, a
/// configured JSON pointer, or the common envelope keys.
fn parse_results(body: &str, c: &WebSearchCap, max: usize) -> Vec<Value> {
    let parsed: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        // Not JSON: a plain-text result list is still useful (one URL per line).
        Err(_) => return parse_plain_results(body, max),
    };

    let arr: Vec<Value> = if !c.results_path.is_empty() {
        parsed
            .pointer(&c.results_path)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    } else if let Some(a) = parsed.as_array() {
        a.clone()
    } else {
        ["results", "items", "data", "hits", "organic_results"]
            .iter()
            .find_map(|k| parsed.get(*k).and_then(|v| v.as_array()).cloned())
            .unwrap_or_default()
    };

    arr.iter()
        .take(max)
        .map(|item| {
            let title = pick(item, &c.title_field, &["title", "name", "heading"]);
            let url = pick(item, &c.url_field, &["url", "link", "href"]);
            let snippet = pick(
                item,
                &c.snippet_field,
                &["snippet", "description", "summary", "content", "text"],
            );
            json!({ "title": title, "url": url, "snippet": snippet })
        })
        .filter(|r| {
            !r["title"].as_str().unwrap_or("").is_empty()
                || !r["url"].as_str().unwrap_or("").is_empty()
        })
        .collect()
}

/// Configured field first, then the first non-empty of `defaults`.
fn pick(item: &Value, configured: &str, defaults: &[&str]) -> String {
    if !configured.is_empty() {
        return item.get(configured).map(stringify).unwrap_or_default();
    }
    for k in defaults {
        if let Some(v) = item.get(*k) {
            let s = stringify(v);
            if !s.is_empty() {
                return s;
            }
        }
    }
    String::new()
}

/// Fallback for a text/plain result list: lines that look like `Title <url>`.
fn parse_plain_results(body: &str, max: usize) -> Vec<Value> {
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Split on whitespace and treat the first http(s) token as the URL.
        let mut url = String::new();
        let mut title_parts = Vec::new();
        for tok in line.split_whitespace() {
            let t = tok.trim_matches(|ch| ch == '<' || ch == '>' || ch == '(' || ch == ')');
            if url.is_empty() && (t.starts_with("http://") || t.starts_with("https://")) {
                url = t.to_string();
            } else {
                title_parts.push(tok);
            }
        }
        if url.is_empty() && title_parts.is_empty() {
            continue;
        }
        out.push(json!({ "title": title_parts.join(" "), "url": url, "snippet": "" }));
        if out.len() >= max {
            break;
        }
    }
    out
}

// ── web_fetch ───────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct WebFetchCap {
    /// `text` (default) | `markdown` | `raw`
    pub format: String,
    /// Extra request headers (e.g. a User-Agent); expanded per call.
    pub headers: serde_json::Map<String, Value>,
    pub timeout_ms: u64,
    pub max_bytes: usize,
    /// Optional API mode: when set, POST the fetched URL to this endpoint
    /// instead of GET-ing the page itself.
    ///
    /// Hosted extractors (TinyFish's `/v1/fetch`, Jina Reader, …) do not take
    /// the target URL in the request line — they take a JSON body. `body_field`
    /// names the key that receives the URL (default `url`), and when
    /// `body_urls_array` is set the URL is sent as a one-element array under
    /// that key (`{"urls": ["…"]}`), which is what those APIs expect.
    pub endpoint: String,
    pub method: String,
    pub body_field: String,
    pub body_urls_array: bool,
    /// Optional extra JSON merged into the API-mode request body.
    pub body: Value,
    /// JSON pointer to the extracted document inside the API response
    /// (TinyFish returns `{"results":[{"text":…,"title":…}]}` → `/results/0`).
    pub result_path: String,
    pub text_field: String,
    pub title_field: String,
}

pub fn call_web_fetch(
    c: &WebFetchCap,
    with: &Value,
    state: &Value,
    policy: &Policy,
) -> Result<Value> {
    let raw_url = with
        .get("url")
        .map(stringify)
        .ok_or_else(|| anyhow!("web_fetch needs 'url'"))?;
    let url = stringify(&expand(&Value::String(raw_url), state, with));
    if url.trim().is_empty() {
        bail!("web_fetch needs 'url'");
    }
    // In API mode the *endpoint* is what policy must authorise; the target URL
    // travels in the body. Both are checked so neither hop escapes the allowlist.
    let endpoint = stringify(&expand(&Value::String(c.endpoint.clone()), state, with));
    if !endpoint.is_empty() {
        check_url(&endpoint, policy)?;
    }
    check_url(&url, policy)?;

    let max_bytes = byte_cap(c.max_bytes, policy);
    let agent = ureq::AgentBuilder::new()
        .timeout(bounded_timeout(
            if c.timeout_ms == 0 {
                20_000
            } else {
                c.timeout_ms
            },
            policy,
        ))
        // Do not chase redirects: a 302 to an un-listed host must not be
        // reachable through the first hop.
        .redirects(0)
        .build();

    if !endpoint.is_empty() {
        return call_web_fetch_api(c, &endpoint, &url, &agent, max_bytes, state, with, policy);
    }

    let mut req = agent.get(&url);
    for (k, v) in &c.headers {
        req = req.set(k, &stringify(&expand(v, state, with)));
    }

    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let (body, truncated) = read_body(r.into_reader(), max_bytes)?;
            return Ok(render_fetch(
                &url,
                code as i64,
                "",
                &body,
                truncated,
                c,
                policy,
            ));
        }
        Err(e) => return Err(anyhow!("web_fetch request failed: {e}")),
    };

    let status = resp.status() as i64;
    let content_type = resp.header("content-type").unwrap_or("").to_string();
    // Report where a redirect points without following it.
    let location = resp.header("location").unwrap_or("").to_string();
    let (body, truncated) = read_body(resp.into_reader(), max_bytes)?;
    let mut out = render_fetch(&url, status, &content_type, &body, truncated, c, policy);
    if !location.is_empty() {
        out["location"] = json!(location);
    }
    Ok(out)
}

fn render_fetch(
    url: &str,
    status: i64,
    content_type: &str,
    body: &str,
    truncated: bool,
    c: &WebFetchCap,
    policy: &Policy,
) -> Value {
    let format = if c.format.is_empty() {
        "text"
    } else {
        c.format.as_str()
    };
    let is_html = content_type.contains("html") || body.trim_start().starts_with('<');
    let (text, title) = match format {
        "raw" => (body.to_string(), String::new()),
        "markdown" => {
            let (t, title) = html_to_markdown(body);
            (truncate(t, policy.max_output), title)
        }
        // "text" and anything unrecognised: strip tags when it looks like HTML.
        _ => {
            if is_html {
                let (t, title) = html_to_text(body);
                (truncate(t, policy.max_output), title)
            } else {
                (truncate(body.to_string(), policy.max_output), String::new())
            }
        }
    };
    let mut out = json!({
        "capability": "web_fetch",
        "url": url,
        "status": status,
        "content_type": content_type,
        "format": format,
        "chars": text.chars().count(),
        "truncated": truncated,
        "text": text,
    });
    if !title.is_empty() {
        out["title"] = json!(title);
    }
    out
}

/// API mode: POST the target URL to an extractor endpoint and read the text out
/// of its JSON envelope.
///
/// The response shape is configurable because hosted extractors differ: a
/// `result_path` JSON pointer locates the document (TinyFish:
/// `/results/0`, Jina: `/data`), and `text_field`/`title_field` name the keys
/// inside it. With no `result_path` the body is searched for the first object
/// carrying a plausible text field.
fn call_web_fetch_api(
    c: &WebFetchCap,
    endpoint: &str,
    url: &str,
    agent: &ureq::Agent,
    max_bytes: usize,
    state: &Value,
    with: &Value,
    policy: &Policy,
) -> Result<Value> {
    let field = if c.body_field.is_empty() {
        "url"
    } else {
        c.body_field.as_str()
    };
    let mut body = if c.body.is_object() {
        c.body.clone()
    } else {
        json!({})
    };
    body[field] = if c.body_urls_array {
        json!([url])
    } else {
        json!(url)
    };
    let method = if c.method.is_empty() {
        "POST"
    } else {
        c.method.as_str()
    };

    let mut req = if method.eq_ignore_ascii_case("POST") {
        agent.post(endpoint).set("content-type", "application/json")
    } else {
        agent
            .request(method, endpoint)
            .set("content-type", "application/json")
    };
    for (k, v) in &c.headers {
        req = req.set(k, &stringify(&expand(v, state, with)));
    }
    let resp = match req.send_string(&body.to_string()) {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let (text, truncated) = read_body(r.into_reader(), max_bytes)?;
            return Ok(json!({
                "capability": "web_fetch", "url": url, "endpoint": endpoint,
                "status": code as i64, "format": c.format,
                "chars": 0, "truncated": truncated,
                "text": truncate(text.clone(), policy.max_output),
                "error": truncate(text, policy.max_output),
            }));
        }
        Err(e) => return Err(anyhow!("web_fetch api request failed: {e}")),
    };

    let status = resp.status() as i64;
    let content_type = resp.header("content-type").unwrap_or("").to_string();
    let (raw, truncated) = read_body(resp.into_reader(), max_bytes)?;

    let parsed: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    let doc = locate_document(&parsed, c);
    let text_key = if c.text_field.is_empty() {
        "text"
    } else {
        c.text_field.as_str()
    };
    let title_key = if c.title_field.is_empty() {
        "title"
    } else {
        c.title_field.as_str()
    };
    let mut text = doc
        .and_then(|d| d.get(text_key))
        .map(stringify)
        .unwrap_or_default();
    let mut title = doc
        .and_then(|d| d.get(title_key))
        .map(stringify)
        .unwrap_or_default();

    // A plain (non-JSON) body is still usable as the content itself.
    let mut note = Value::Null;
    if doc.is_none() {
        if parsed.is_null() {
            text = raw.clone();
        } else {
            note = json!("response JSON did not contain an extractable document; see /raw");
        }
    }
    if title.is_empty() {
        if let Some(t) = parsed
            .pointer("/results/0/title")
            .map(stringify)
            .filter(|s| !s.is_empty())
        {
            title = t;
        }
    }

    let out = json!({
        "capability": "web_fetch",
        "url": url,
        "endpoint": endpoint,
        "status": status,
        "content_type": content_type,
        "format": if c.format.is_empty() { "text" } else { c.format.as_str() },
        "chars": text.chars().count(),
        "truncated": truncated,
        "text": truncate(text, policy.max_output),
        "title": title,
    });
    let mut out = out;
    if !note.is_null() {
        out["note"] = note;
        out["raw"] = json!(truncate(raw, policy.max_output));
    }
    // Surface per-URL errors some extractors report alongside partial success.
    if let Some(errs) = parsed.get("errors").and_then(|v| v.as_array()) {
        if !errs.is_empty() {
            out["errors"] = json!(errs);
        }
    }
    Ok(out)
}

/// Find the extracted document inside an extractor's response envelope.
fn locate_document<'a>(parsed: &'a Value, c: &WebFetchCap) -> Option<&'a Value> {
    if parsed.is_null() {
        return None;
    }
    // Explicit pointer wins.
    if !c.result_path.is_empty() {
        return parsed.pointer(&c.result_path);
    }
    // `{"results":[{...}]}` (TinyFish) / `{"data":{...}}` (Jina) / bare object.
    for ptr in ["/results/0", "/data", "/document", "/result"] {
        if let Some(v) = parsed.pointer(ptr) {
            if v.is_object() {
                return Some(v);
            }
        }
    }
    if parsed.is_object() {
        return Some(parsed);
    }
    None
}

// ── HTML helpers ────────────────────────────────────────────────────

/// Very small HTML → text conversion: drop `script`/`style`/`head` bodies,
/// turn block-level tags into newlines, strip the remaining tags, and decode
/// the handful of entities that actually show up in text.
fn html_to_text(html: &str) -> (String, String) {
    let title = extract_title(html);
    let stripped = strip_raw_elements(html);
    let mut out = String::with_capacity(stripped.len());
    let mut in_tag = false;
    let mut tag = String::new();
    for ch in stripped.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let t = tag.trim().to_ascii_lowercase();
                // Block-ish tags become line breaks; <br> is a single break.
                let name = t
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                if matches!(
                    name,
                    "p" | "div"
                        | "br"
                        | "li"
                        | "tr"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "section"
                        | "article"
                        | "header"
                        | "footer"
                        | "blockquote"
                        | "pre"
                ) {
                    out.push('\n');
                } else if matches!(name, "td" | "th") {
                    out.push('\t');
                }
                tag.clear();
            }
            _ if in_tag => tag.push(ch),
            _ => out.push(ch),
        }
    }
    let collapsed = collapse_blank_lines(&out);
    let decoded = decode_entities(&collapsed);
    (decoded.trim().to_string(), title)
}

/// Light markdown: keep headings, links, and list items recognisable instead of
/// flattening everything to plain text.
fn html_to_markdown(html: &str) -> (String, String) {
    let title = extract_title(html);
    let stripped = strip_raw_elements(html);
    let mut out = String::with_capacity(stripped.len());
    let mut in_tag = false;
    let mut tag = String::new();
    let mut href: Option<String> = None;
    let mut link_text = String::new();
    for ch in stripped.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let t = tag.trim().to_ascii_lowercase();
                let name = t
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                match name {
                    "a" => {
                        if t.starts_with("/a") {
                            // Close the link, using its text as the label.
                            let label = link_text.trim();
                            match href.take() {
                                Some(h) if !label.is_empty() => {
                                    out.push_str(&format!("[{label}]({h})"))
                                }
                                Some(h) => out.push_str(&h),
                                None => out.push_str(label),
                            }
                            link_text.clear();
                        } else {
                            href = attr_value(&t, "href");
                        }
                    }
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        if !t.starts_with('/') {
                            let level: usize = name[1..].parse().unwrap_or(1);
                            out.push('\n');
                            out.push_str(&"#".repeat(level.clamp(1, 6)));
                            out.push(' ');
                        } else {
                            out.push('\n');
                        }
                    }
                    "li" => {
                        if t.starts_with('/') {
                            out.push('\n');
                        } else {
                            out.push_str("\n- ");
                        }
                    }
                    "p" | "div" | "br" | "tr" | "section" | "article" | "blockquote" | "pre" => {
                        out.push('\n')
                    }
                    _ => {}
                }
                tag.clear();
            }
            _ if in_tag => tag.push(ch),
            _ => {
                if href.is_some() {
                    link_text.push(ch);
                } else {
                    out.push(ch);
                }
            }
        }
    }
    let collapsed = collapse_blank_lines(&out);
    (decode_entities(&collapsed).trim().to_string(), title)
}

/// Remove `<script>`, `<style>`, `<noscript>` and `<head>` element *contents*
/// (they are never page text, and script bodies would otherwise leak into the
/// extracted text as garbage).
fn strip_raw_elements(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let lower = html.to_ascii_lowercase();
    let mut i = 0usize;
    while i < html.len() {
        let mut skipped = false;
        for tag in ["script", "style", "noscript", "head", "svg"] {
            let open = format!("<{tag}");
            if lower[i..].starts_with(&open) {
                let close = format!("</{tag}");
                match lower[i..].find(&close) {
                    Some(off) => {
                        i += off + close.len();
                        // step past the closing '>'
                        if let Some(gt) = html[i..].find('>') {
                            i += gt + 1;
                        }
                        skipped = true;
                    }
                    None => {
                        i = html.len();
                        skipped = true;
                    }
                }
                break;
            }
        }
        if skipped {
            continue;
        }
        // Advance one UTF-8 character at a time.
        let step = html[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        out.push_str(&html[i..i + step]);
        i += step;
    }
    out
}

fn extract_title(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let start = match lower.find("<title") {
        Some(s) => s,
        None => return String::new(),
    };
    let after = match lower[start..].find('>') {
        Some(o) => start + o + 1,
        None => return String::new(),
    };
    let end = match lower[after..].find("</title") {
        Some(e) => after + e,
        None => return String::new(),
    };
    decode_entities(html[after..end].trim()).trim().to_string()
}

/// Pull `name="value"` / `name='value'` out of a lowercased tag body.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let key = format!("{name}=");
    let pos = tag.find(&key)? + key.len();
    let rest = &tag[pos..];
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let body = &rest[1..];
        let end = body.find(quote)?;
        Some(body[..end].to_string())
    } else {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

/// Squeeze runs of blank lines and trailing spaces so extracted text reads well.
fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blanks = 0usize;
    for line in s.lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
            out.push('\n');
        } else {
            blanks = 0;
            out.push_str(t);
            out.push('\n');
        }
    }
    out
}

/// Decode the entities that realistically appear in page text.
fn decode_entities(s: &str) -> String {
    let mut out = s
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&hellip;", "…");
    // Numeric entities: &#123; and &#x1F600;
    while let Some(pos) = out.find("&#") {
        let rest = &out[pos + 2..];
        let end = match rest.find(';') {
            Some(e) if e <= 8 => e,
            _ => {
                // Not a decodable entity; leave it and move on.
                let mut idx = pos + 2;
                while out[idx..].starts_with("&#") {
                    idx += 2;
                }
                if idx <= out.len() {
                    out.replace_range(pos..pos + 1, "\u{0}");
                    out = out.replace('\u{0}', "&");
                }
                break;
            }
        };
        let digits = &rest[..end];
        let code = if let Some(hex) = digits
            .strip_prefix('x')
            .or_else(|| digits.strip_prefix('X'))
        {
            u32::from_str_radix(hex, 16).ok()
        } else {
            digits.parse::<u32>().ok()
        };
        match code.and_then(char::from_u32) {
            Some(ch) => {
                let mut rep = String::new();
                rep.push(ch);
                out.replace_range(pos..pos + 2 + end + 1, &rep);
            }
            None => out.replace_range(pos..pos + 1, "\u{0}"),
        }
        out = out.replace('\u{0}', "&");
    }
    out
}

// ── shared plumbing ─────────────────────────────────────────────────

/// Egress guard plus a scheme check.
///
/// The scheme check matters here because these capabilities accept a URL from
/// workflow *data*: without it a spec could be steered into `file:` or another
/// non-HTTP scheme.
fn check_url(url: &str, policy: &Policy) -> Result<()> {
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        bail!("only http/https URLs are allowed, got {url:?}");
    }
    check_host(url, policy)
}

/// Effective byte cap: never above what the policy allows for output.
fn byte_cap(requested: usize, policy: &Policy) -> usize {
    let want = if requested == 0 {
        DEFAULT_MAX_BYTES
    } else {
        requested
    };
    want.min(policy.max_output.max(1))
}

/// Read at most `max` bytes; the flag reports whether the body was clipped.
fn read_body(reader: impl std::io::Read, max: usize) -> Result<(String, bool)> {
    use std::io::Read as _;
    let mut buf = Vec::with_capacity(8192);
    // Read one byte past the cap so we can tell "exactly max" from "more".
    let mut limited = reader.take((max as u64).saturating_add(1));
    limited
        .read_to_end(&mut buf)
        .map_err(|e| anyhow!("reading response body failed: {e}"))?;
    let truncated = buf.len() > max;
    if truncated {
        buf.truncate(max);
    }
    Ok((String::from_utf8_lossy(&buf).to_string(), truncated))
}

/// Percent-encode a query string (unreserved characters kept as-is).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
