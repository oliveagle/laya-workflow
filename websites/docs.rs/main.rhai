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

fn limit_of(args, dflt, max) {
    let n = to_int(map_get(args, "count", ()), 0);
    if n <= 0 { n = to_int(map_get(args, "limit", ()), dflt); }
    if n <= 0 { n = dflt; }
    if n > max { n = max; }
    n
}

fn text_field(m, k) {
    let v = map_get(m, k, ());
    if type_of(v) == "()" { return ""; }
    "" + v
}

fn int_field(m, k) { to_int(map_get(m, k, ()), 0) }

// ── intent → plan (pure) ────────────────────────────────────────────────────

// The crate name of a docs.rs page (`/serde/latest/serde/`, `/crate/serde/1.0.0`).
fn crate_of(text) {
    let t = trim_str(text);
    if t == "" { return ""; }
    let at = t.index_of("docs.rs/");
    let rest = if at >= 0 { t.sub_string(at + 8, t.len() - (at + 8)) } else { t };
    let segs = rest.split("/");
    let i = 0;
    while i < segs.len() {
        let seg = segs[i];
        if seg != "" && seg != "crate" && seg != "latest" && !seg.contains(".") && seg != "docs" {
            return seg;
        }
        i += 1;
    }
    ""
}

fn crate_is_valid(s) {
    if s == "" { return false; }
    let i = 0;
    while i < s.len() {
        let c = s.sub_string(i, 1);
        let ok = (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") || (c >= "0" && c <= "9")
            || c == "-" || c == "_";
        if !ok { return false; }
        i += 1;
    }
    true
}

fn plan(args) {
    let query = trim_str(s_of(map_get(args, "query", "")));
    if query == "" { query = trim_str(s_of(map_get(args, "q", ""))); }
    let krate = trim_str(s_of(map_get(args, "crate", "")));
    if krate == "" { krate = trim_str(s_of(map_get(args, "name", ""))); }
    let explicit = trim_str(s_of(map_get(args, "mode", ""))).to_lower();

    let mode = explicit;
    if mode == "" || mode == "auto" {
        if krate != "" { mode = "crate"; }
        else if query != "" && query.contains("docs.rs/") { mode = "crate"; krate = crate_of(query); }
        else { mode = "search"; }
    }
    if mode == "read" || mode == "crate" || mode == "docs" || mode == "api" { mode = "crate"; }
    else if mode == "find" || mode == "search" || mode == "query" { mode = "search"; }
    else { throw "docsrs: mode '" + mode + "' is not supported (auto|search|crate)"; }

    if mode == "crate" {
        if krate == "" { krate = crate_of(query); }
        if !crate_is_valid(krate) { throw "docsrs: crate mode needs a crate name (or a docs.rs URL)"; }
    } else if query == "" {
        throw "docsrs: needs a 'query' to search";
    }

    let dflt = if mode == "search" { 10 } else { 1 };
    #{ mode: mode, query: query, crate: krate, limit: limit_of(args, dflt, 50) }
}

fn docs_url(krate) {
    "https://docs.rs/" + krate + "/latest/" + krate + "/"
}

// ── reads ───────────────────────────────────────────────────────────────────

fn search(host, query, limit) {
    let url = "https://docs.rs/releases/search?query=" + host.urlencode(query);
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
            page_title = text_field(value, "page_title");
        }
    } catch (e) {
        host.log("docsrs: search failed (" + e + ")");
    }
    host.browser_release(target);
    #{ url: url, source: "search", page_title: page_title, results: results }
}

fn read_docs(host, krate, out_dir) {
    let url = docs_url(krate);
    let target = host.browser_open(url);
    let page_title = "";
    let written = #{};
    try {
        host.browser_wait_ready(target);
        let leaf = #{ target_id: target, selector: "main", slug: krate };
        if out_dir != "" { leaf["out_dir"] = out_dir; }
        let saved = host.save_article(leaf);
        page_title = text_field(saved, "title");
        written = map_get(saved, "written", ());
    } catch (e) {
        host.log("docsrs: docs read failed at " + url + " (" + e + ")");
    }
    host.browser_release(target);
    #{ url: url, source: "crate", docs_url: url, page_title: page_title, written: written, results: [] }
}

// ── entry point ─────────────────────────────────────────────────────────────

fn run(host, ctx) {
    let args = ctx["with"];
    let p = plan(args);
    let mode = p["mode"];
    let started = host.now();
    let out_dir = trim_str(s_of(map_get(args, "out_dir", "")));
    if out_dir == "" { out_dir = "~/tmp/docsrs"; }

    let out = #{};
    if mode == "crate" {
        out = read_docs(host, p["crate"], out_dir);
    } else {
        out = search(host, p["query"], p["limit"]);
    }

    let results = map_get(out, "results", []);
    let count = 0;
    if type_of(results) == "array" { count = results.len(); }
    let written = map_get(out, "written", ());
    if mode == "crate" && type_of(written) != "()" { count = 1; }
    let finished = host.now();
    out["mode"] = mode;
    out["query"] = p["query"];
    out["crate"] = p["crate"];
    out["count"] = count;
    out["out_dir"] = out_dir;
    out["fetched_at"] = started["rfc3339"];
    out["finished_at"] = finished["rfc3339"];
    out
}
