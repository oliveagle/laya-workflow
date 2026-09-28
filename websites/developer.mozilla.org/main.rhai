// ─────────────────────────────────────────────────────────────────────────────
// MDN Web Docs reader — Laya workflow plugin.
//
// Search goes through MDN's public search API (`host.http_get`), so it is fast
// and needs no tab. Reading one doc opens the page in a background Chrome tab
// and hands it to the engine's generic renderer (`host.save_article`). Site
// logic only: which locale, which doc path a phrase means, how a hit is read.
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

// ── paths & locale (pure) ───────────────────────────────────────────────────

// A locale like `en-US` / `zh-CN` / `fr`; "" when the text is not one.
fn locale_code(s) {
    let t = trim_str(s);
    if t.len() < 2 || t.len() > 6 { return ""; }
    let i = 0;
    let dashes = 0;
    while i < t.len() {
        let c = t.sub_string(i, 1);
        let alpha = (c >= "a" && c <= "z") || (c >= "A" && c <= "Z");
        if c == "-" { dashes += 1; }
        else if !alpha { return ""; }
        i += 1;
    }
    if dashes > 1 { return ""; }
    t
}

// The `/locale/docs/...` path inside an MDN URL or phrase, or "".
fn doc_path_of(text) {
    let t = trim_str(text);
    let at = t.index_of("/docs/");
    if at < 0 { return ""; }
    let head = t.sub_string(0, at);
    // Keep the last "/" of the head so `.../en-US` (or a bare `/en-US`) is kept.
    let cut = 0;
    let i = 0;
    while i < head.len() {
        if head.sub_string(i, 1) == "/" { cut = i; }
        i += 1;
    }
    if cut == 0 && head != "" && head.sub_string(0, 1) != "/" { cut = head.len(); }
    let rest = t.sub_string(cut, t.len() - cut);
    let stop = rest.len();
    let j = 0;
    while j < rest.len() {
        let c = rest.sub_string(j, 1);
        if c == "#" || c == "?" { stop = j; break; }
        j += 1;
    }
    trim_str(rest.sub_string(0, stop))
}

// `/locale/docs/...` → an absolute MDN URL, filling in the locale when absent.
fn doc_url(path, locale) {
    let p = trim_str(path);
    if p.len() == 0 || p.sub_string(0, 1) != "/" { p = "/" + p; }
    if p.starts_with("/docs/") { p = "/" + locale + p; }
    "https://developer.mozilla.org" + p
}

fn plan(args) {
    let query = trim_str(s_of(map_get(args, "query", "")));
    if query == "" { query = trim_str(s_of(map_get(args, "q", ""))); }
    let path = trim_str(s_of(map_get(args, "path", "")));
    if path == "" { path = trim_str(s_of(map_get(args, "url", ""))); }
    let explicit = trim_str(s_of(map_get(args, "mode", ""))).to_lower();

    let locale = locale_code(s_of(map_get(args, "lang", "")));
    if locale == "" { locale = locale_code(s_of(map_get(args, "locale", ""))); }
    if locale == "" { locale = "en-US"; }

    let mode = explicit;
    if mode == "" || mode == "auto" {
        if path != "" { mode = "page"; }
        else if doc_path_of(query) != "" { mode = "page"; path = doc_path_of(query); }
        else { mode = "search"; }
    }
    if mode == "read" || mode == "docs" || mode == "doc" { mode = "page"; }
    if mode == "find" || mode == "query" || mode == "list" { mode = "search"; }

    if mode == "page" {
        if path == "" { path = doc_path_of(query); }
        if path == "" { throw "mdn: page mode needs an MDN doc path (/en-US/docs/...) or an MDN URL"; }
    } else if mode == "search" {
        if query == "" { throw "mdn: needs a 'query' to search"; }
    } else {
        throw "mdn: mode '" + mode + "' is not supported (auto|search|page)";
    }

    let dflt = if mode == "search" { 10 } else { 1 };
    let limit = to_int(map_get(args, "count", ()), 0);
    if limit <= 0 { limit = to_int(map_get(args, "limit", ()), dflt); }
    if limit <= 0 { limit = dflt; }
    if limit > 30 { limit = 30; }

    #{ mode: mode, query: query, path: path, locale: locale, limit: limit }
}

// ── reads ───────────────────────────────────────────────────────────────────

fn search(host, query, locale, limit) {
    let url = "https://developer.mozilla.org/api/v1/search?q=" + host.urlencode(query)
        + "&locale=" + locale;
    let results = [];
    try {
        let resp = host.http_get(url);
        let body = host.json_parse(sf(resp, "text"));
        let docs = map_get(body, "documents", []);
        if type_of(docs) == "array" {
            let i = 0;
            while i < docs.len() && i < limit {
                let d = docs[i];
                results.push(#{
                    rank: i + 1,
                    title: sf(d, "title"),
                    url: "https://developer.mozilla.org" + sf(d, "mdn_url"),
                    path: sf(d, "mdn_url"),
                    summary: sf(d, "summary")
                });
                i += 1;
            }
        }
    } catch (e) {
        host.log("mdn: search failed (" + e + ")");
    }
    #{ url: url, source: "search", results: results }
}

fn read_page(host, path, locale, out_dir) {
    let url = doc_url(path, locale);
    let target = host.browser_open(url);
    let page_title = "";
    let written = #{};
    try {
        host.browser_wait_ready(target);
        let leaf = #{ target_id: target, selector: "#content" };
        if out_dir != "" { leaf["out_dir"] = out_dir; }
        let saved = host.save_article(leaf);
        page_title = sf(saved, "title");
        written = map_get(saved, "written", ());
    } catch (e) {
        host.log("mdn: page read failed at " + url + " (" + e + ")");
    }
    host.browser_release(target);
    #{ url: url, source: "page", page_title: page_title, written: written, results: [] }
}

// ── entry point ─────────────────────────────────────────────────────────────

fn run(host, ctx) {
    let args = ctx["with"];
    let p = plan(args);
    let mode = p["mode"];
    let started = host.now();
    let out_dir = trim_str(s_of(map_get(args, "out_dir", "")));
    if out_dir == "" { out_dir = "~/tmp/mdn"; }

    let out = #{};
    if mode == "page" {
        out = read_page(host, p["path"], p["locale"], out_dir);
    } else {
        out = search(host, p["query"], p["locale"], p["limit"]);
    }

    let results = map_get(out, "results", []);
    let count = 0;
    if type_of(results) == "array" { count = results.len(); }
    let written = map_get(out, "written", ());
    if mode == "page" && type_of(written) != "()" { count = 1; }
    let finished = host.now();
    out["mode"] = mode;
    out["query"] = p["query"];
    out["path"] = p["path"];
    out["locale"] = p["locale"];
    out["count"] = count;
    out["out_dir"] = out_dir;
    out["fetched_at"] = started["rfc3339"];
    out["finished_at"] = finished["rfc3339"];
    out
}
