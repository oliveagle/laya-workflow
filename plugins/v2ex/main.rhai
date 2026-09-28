// ─────────────────────────────────────────────────────────────────────────────
// V2EX reader — Laya workflow plugin (Chrome/CDP).
//
// Three jobs: a topic list (latest / hot / a tab), a node's topics, and one
// topic with its replies. Site logic only: which path a word means, how a list
// row is read, how a topic page becomes a post + reply list. The Rust base owns
// the tab, the CDP transport, the policy gate and the tab lifecycle.
// Rhai sharp edges honoured: constants live *inside* functions, borrowed slices
// are never returned, no `let mut`.
// ─────────────────────────────────────────────────────────────────────────────

fn is_space(c) { c == " " || c == "\n" || c == "\t" || c == "\r" }

fn trim_str(s) {
    let start = 0;
    let end = s.len();
    while start < end && is_space(s.sub_string(start, 1)) { start += 1; }
    while end > start && is_space(s.sub_string(end - 1, 1)) { end -= 1; }
    s.sub_string(start, end - start)
}

fn s_of(v) {
    if type_of(v) == "()" { return ""; }
    "" + v
}

fn map_get(m, k, dflt) {
    if type_of(m) == "map" && m.contains(k) && type_of(m[k]) != "()" { return m[k]; }
    dflt
}

fn parse_i(s, dflt) {
    let t = trim_str(s);
    if t == "" { return dflt; }
    let i = 0;
    let neg = false;
    if t.sub_string(0, 1) == "-" { neg = true; i = 1; }
    let val = 0;
    let digits = 0;
    while i < t.len() {
        let d = "0123456789".index_of(t.sub_string(i, 1));
        if d < 0 { break; }
        val = val * 10 + d;
        digits += 1;
        i += 1;
    }
    if digits == 0 { return dflt; }
    if neg { -val } else { val }
}

fn to_int(v, dflt) {
    let t = type_of(v);
    if t == "i64" { return v; }
    if t == "f64" { return parse_i("" + v, dflt); }
    if t == "string" { return parse_i(v, dflt); }
    dflt
}

fn sf(m, k) {
    let v = map_get(m, k, ());
    if type_of(v) == "()" { return ""; }
    "" + v
}

// ── intent → plan (pure) ────────────────────────────────────────────────────

// The `?tab=<x>` a tab word means; "" when the word is not a tab.
fn tab_of(word) {
    const TABS = #{
        latest: "", hot: "hot", tech: "tech", creative: "creative", play: "play",
        apple: "apple", jobs: "jobs", deals: "deals", city: "city", qna: "qna",
        all: "all", recent: "", "new": ""
    };
    if word == "latest" || word == "recent" || word == "new" { return "latest"; }
    if TABS.contains(word) { return word; }
    ""
}

// A topic id inside `/t/<id>#replyN`, a URL, or a bare number; "" when absent.
fn topic_id_of(text) {
    let t = trim_str(text);
    let at = t.index_of("/t/");
    let rest = t;
    if at >= 0 { rest = t.sub_string(at + 3, t.len() - (at + 3)); }
    let out = "";
    let i = 0;
    while i < rest.len() {
        let c = rest.sub_string(i, 1);
        if c >= "0" && c <= "9" { out += c; i += 1; } else { break; }
    }
    out
}

// A node name inside `/go/<name>`, `go/<name>` or `node:<name>`; "" when absent.
fn node_of(text) {
    let t = trim_str(text);
    let at = t.index_of("/go/");
    let rest = "";
    if at >= 0 { rest = t.sub_string(at + 4, t.len() - (at + 4)); }
    else if t.starts_with("go/") { rest = t.sub_string(3, t.len() - 3); }
    else if t.starts_with("node:") { rest = t.sub_string(5, t.len() - 5); }
    if rest == "" { return ""; }
    let out = "";
    let i = 0;
    while i < rest.len() {
        let c = rest.sub_string(i, 1);
        let ok = (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") || (c >= "0" && c <= "9") || c == "-" || c == "_";
        if !ok { break; }
        out += c;
        i += 1;
    }
    out
}

fn plan(args) {
    let query = trim_str(s_of(map_get(args, "query", "")));
    if query == "" { query = trim_str(s_of(map_get(args, "q", ""))); }
    let id = trim_str(s_of(map_get(args, "id", "")));
    if id == "" { id = topic_id_of(query); }
    let node = trim_str(s_of(map_get(args, "node", "")));
    if node == "" { node = node_of(query); }
    let explicit = trim_str(s_of(map_get(args, "mode", ""))).to_lower();

    let mode = explicit;
    if mode == "" || mode == "auto" {
        if id != "" { mode = "topic"; }
        else if node != "" { mode = "node"; }
        else if query != "" && tab_of(query.to_lower()) != "" { mode = query.to_lower(); }
        else if query == "" { mode = "hot"; }
        else { mode = "list"; }
    }
    if mode == "topic" || mode == "post" { mode = "topic"; }
    else if mode == "node" || mode == "go" { mode = "node"; }
    else if mode == "front" { mode = "hot"; }
    else { mode = tab_of(mode); }
    if mode == "" && explicit != "" { throw "v2ex: mode '" + explicit + "' is not supported"; }

    if mode == "topic" {
        if id == "" { throw "v2ex: topic mode needs a topic id or a /t/<id> URL"; }
    } else if mode == "node" {
        if node == "" { throw "v2ex: node mode needs a node name (/go/<name> or node:<name>)"; }
    } else if mode == "" || mode == "list" {
        throw "v2ex: needs a tab (latest|hot|tech|creative|play|apple|jobs|deals|city|qna|all), a node (/go/<name>) or a topic (/t/<id>)";
    }

    let dflt = if mode == "topic" { 30 } else { 30 };
    let limit = to_int(map_get(args, "count", ()), 0);
    if limit <= 0 { limit = to_int(map_get(args, "limit", ()), dflt); }
    if limit <= 0 { limit = dflt; }
    if limit > 100 { limit = 100; }

    #{ mode: mode, query: query, id: id, node: node, limit: limit }
}

// ── reads ───────────────────────────────────────────────────────────────────

fn list_url(mode, node) {
    if mode == "node" { return "https://www.v2ex.com/go/" + node; }
    if mode == "latest" { return "https://www.v2ex.com/"; }
    "https://www.v2ex.com/?tab=" + mode
}

fn read_list(host, mode, node, limit) {
    let url = list_url(mode, node);
    let target = host.browser_open(url);
    let results = [];
    let page_title = "";
    try {
        host.browser_wait_ready(target);
        let opts = #{ limit: limit };
        let js = "var __LAYA_OPTS__ = " + host.json_stringify(opts) + ";\n" + host.js("list.js");
        let value = host.browser_evaluate(target, js, true);
        if type_of(value) == "map" {
            let r = map_get(value, "results", []);
            if type_of(r) == "array" { results = r; }
            page_title = sf(value, "page_title");
        }
    } catch (e) {
        host.log("v2ex: list read failed at " + url + " (" + e + ")");
    }
    host.browser_release(target);
    #{ url: url, source: "list", page_title: page_title, results: results }
}

fn read_topic(host, id, limit) {
    let url = "https://www.v2ex.com/t/" + id;
    let target = host.browser_open(url);
    let out = #{};
    try {
        host.browser_wait_ready(target);
        let opts = #{ limit: limit };
        let js = "var __LAYA_OPTS__ = " + host.json_stringify(opts) + ";\n" + host.js("topic.js");
        let value = host.browser_evaluate(target, js, true);
        if type_of(value) == "map" { out = value; }
    } catch (e) {
        host.log("v2ex: topic read failed at " + url + " (" + e + ")");
    }
    host.browser_release(target);
    out["url"] = url;
    out["id"] = id;
    out["results"] = [];
    out
}

// ── entry point ─────────────────────────────────────────────────────────────

fn run(host, ctx) {
    let args = ctx["with"];
    let p = plan(args);
    let mode = p["mode"];
    let started = host.now();

    let out = #{};
    if mode == "topic" {
        out = read_topic(host, p["id"], p["limit"]);
    } else {
        out = read_list(host, mode, p["node"], p["limit"]);
    }

    let results = map_get(out, "results", []);
    let count = 0;
    if type_of(results) == "array" { count = results.len(); }
    if mode == "topic" { count = to_int(map_get(out, "count", ()), 0); }
    let finished = host.now();
    out["mode"] = mode;
    out["query"] = p["query"];
    out["node"] = p["node"];
    out["feed"] = mode;
    out["limit"] = p["limit"];
    out["count"] = count;
    out["fetched_at"] = started["rfc3339"];
    out["finished_at"] = finished["rfc3339"];
    out
}
