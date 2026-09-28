// ─────────────────────────────────────────────────────────────────────────────
// Bing web search — Laya workflow plugin (Chrome/CDP).
//
// One job: turn a phrase into organic result rows from a real Chrome tab.
// Site logic only: the market, the result URL, and how a `li.b_algo` row is
// read. The Rust base owns the tab, the CDP transport, the policy gate and the
// tab lifecycle. Rhai sharp edges honoured: constants live *inside* functions,
// borrowed slices are never returned, no `let mut`.
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

// A market like `en-US` / `zh-CN`; "" when the text is not one.
fn market_code(s) {
    let t = trim_str(s);
    if t.len() < 2 || t.len() > 6 { return ""; }
    let i = 0;
    while i < t.len() {
        let c = t.sub_string(i, 1);
        let alpha = (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") || c == "-";
        if !alpha { return ""; }
        i += 1;
    }
    t
}

fn plan(args) {
    let query = trim_str(s_of(map_get(args, "query", "")));
    if query == "" { query = trim_str(s_of(map_get(args, "q", ""))); }
    let explicit = trim_str(s_of(map_get(args, "mode", ""))).to_lower();
    if explicit != "" && explicit != "auto" && explicit != "search" && explicit != "web" && explicit != "find" {
        throw "bing: mode '" + explicit + "' is not supported (auto|search)";
    }
    if query == "" { throw "bing: needs a 'query' to search"; }

    let mkt = market_code(s_of(map_get(args, "mkt", "")));
    if mkt == "" { mkt = market_code(s_of(map_get(args, "lang", ""))); }

    let limit = to_int(map_get(args, "count", ()), 0);
    if limit <= 0 { limit = to_int(map_get(args, "limit", ()), 10); }
    if limit <= 0 { limit = 10; }
    if limit > 30 { limit = 30; }

    #{ mode: "search", query: query, mkt: mkt, limit: limit }
}

// ── read ────────────────────────────────────────────────────────────────────

fn search(host, query, mkt, limit) {
    let url = "https://www.bing.com/search?q=" + host.urlencode(query) + "&count=" + limit;
    if mkt != "" { url += "&mkt=" + mkt; }
    let target = host.browser_open(url);
    let results = [];
    let page_title = "";
    try {
        host.browser_wait_ready(target);
        let opts = #{ limit: limit };
        let js = "var __LAYA_OPTS__ = " + host.json_stringify(opts) + ";\n" + host.js("search.js");
        let value = host.browser_evaluate(target, js, true);
        if type_of(value) == "map" {
            let r = map_get(value, "results", []);
            if type_of(r) == "array" { results = r; }
            page_title = sf(value, "page_title");
        }
    } catch (e) {
        host.log("bing: search failed (" + e + ")");
    }
    host.browser_release(target);
    #{ url: url, source: "search", page_title: page_title, results: results }
}

// ── entry point ─────────────────────────────────────────────────────────────

fn run(host, ctx) {
    let args = ctx["with"];
    let p = plan(args);
    let started = host.now();

    let out = search(host, p["query"], p["mkt"], p["limit"]);
    let results = map_get(out, "results", []);
    let count = 0;
    if type_of(results) == "array" { count = results.len(); }
    let finished = host.now();
    out["mode"] = p["mode"];
    out["query"] = p["query"];
    out["mkt"] = p["mkt"];
    out["count"] = count;
    out["fetched_at"] = started["rfc3339"];
    out["finished_at"] = finished["rfc3339"];
    out
}
