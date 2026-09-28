// ─────────────────────────────────────────────────────────────────────────────
// Wikipedia reader — Laya workflow plugin (Chrome/CDP).
//
// Two jobs: turn a phrase into search-result rows, and render one article's
// body to Markdown through the engine's generic renderer (`host.save_article`).
// Site logic only: which language host, how a search row is read, which article
// a phrase or URL means, and where the Markdown lands. The Rust base owns the
// tab, the CDP transport, the policy gate and the tab lifecycle.
//
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

fn nf(m, k) { to_int(map_get(m, k, ()), 0) }

// A filesystem-safe leaf for an article path (no directory separators).
fn fs_safe(s) {
    let out = "";
    let i = 0;
    while i < s.len() {
        let c = s.sub_string(i, 1);
        if c == "/" || c == "\\" { out += "_"; } else { out += c; }
        i += 1;
    }
    out
}

// ── intent → plan (pure) ────────────────────────────────────────────────────

// Space-separated words into an article path (`Transformer (deep learning)` →
// `Transformer_(deep_learning)`); other characters are left to Wikipedia.
fn to_path(title) {
    let out = "";
    let i = 0;
    while i < title.len() {
        let c = title.sub_string(i, 1);
        if c == " " { out += "_"; } else { out += c; }
        i += 1;
    }
    out
}

// The article path inside a wikipedia.org URL, or "" when it is not one.
fn title_of_url(text) {
    let at = text.index_of("/wiki/");
    if at < 0 { return ""; }
    let rest = text.sub_string(at + 6, text.len() - (at + 6));
    let cut = rest.len();
    let i = 0;
    while i < rest.len() {
        let c = rest.sub_string(i, 1);
        if c == "#" || c == "?" { cut = i; break; }
        i += 1;
    }
    trim_str(rest.sub_string(0, cut))
}

// A short language code (`en`, `zh`, ...) or "" — guards against junk input.
fn lang_code(s) {
    let t = trim_str(s).to_lower();
    if t.len() < 2 || t.len() > 5 { return ""; }
    let i = 0;
    while i < t.len() {
        let c = t.sub_string(i, 1);
        let ok = (c >= "a" && c <= "z") || c == "-";
        if !ok { return ""; }
        i += 1;
    }
    if t.sub_string(0, 1) == "-" { return ""; }
    t
}

// The subdomain of a wikipedia.org URL (`zh` in `zh.wikipedia.org`), or "".
fn lang_of_url(text) {
    let at = text.index_of("//");
    let rest = if at >= 0 { text.sub_string(at + 2, text.len() - (at + 2)) } else { text };
    let dot = rest.index_of(".wikipedia.org");
    if dot < 0 { return ""; }
    lang_code(rest.sub_string(0, dot))
}

fn plan(args) {
    let query = trim_str(s_of(map_get(args, "query", "")));
    if query == "" { query = trim_str(s_of(map_get(args, "q", ""))); }
    let title = trim_str(s_of(map_get(args, "title", "")));
    if title == "" { title = trim_str(s_of(map_get(args, "page", ""))); }
    let explicit = trim_str(s_of(map_get(args, "mode", ""))).to_lower();

    let lang = lang_code(s_of(map_get(args, "lang", "")));
    if lang == "" { lang = lang_of_url(query); }
    if lang == "" { lang = lang_of_url(title); }
    if lang == "" { lang = "en"; }

    let mode = explicit;
    if mode == "" || mode == "auto" {
        if title != "" { mode = "page"; }
        else if query != "" && title_of_url(query) != "" { mode = "page"; title = title_of_url(query); }
        else { mode = "search"; }
    }
    if mode == "read" || mode == "article" || mode == "page" { mode = "page"; }
    if mode == "find" || mode == "query" || mode == "list" { mode = "search"; }

    if mode == "page" {
        if title == "" { title = title_of_url(query); }
        if title == "" {
            if query == "" { throw "wikipedia: page mode needs a title, a phrase, or a wikipedia.org/wiki/ URL"; }
            title = query;
        }
        title = to_path(title);
    } else if mode == "search" {
        if query == "" { throw "wikipedia: needs a 'query' to search"; }
    } else {
        throw "wikipedia: mode '" + mode + "' is not supported (auto|search|page)";
    }

    let dflt = if mode == "search" { 10 } else { 1 };
    let limit = to_int(map_get(args, "count", ()), 0);
    if limit <= 0 { limit = to_int(map_get(args, "limit", ()), dflt); }
    if limit <= 0 { limit = dflt; }
    if limit > 50 { limit = 50; }

    #{ mode: mode, query: query, title: title, lang: lang, limit: limit }
}

// ── reads ───────────────────────────────────────────────────────────────────

fn search(host, query, lang, limit) {
    let url = "https://" + lang + ".wikipedia.org/w/index.php?title=Special:Search&fulltext=1&search="
        + host.urlencode(query);
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
        host.log("wikipedia: search failed (" + e + ")");
    }
    host.browser_release(target);
    #{ url: url, source: "search", page_title: page_title, results: results }
}

fn read_page(host, title, lang, out_dir) {
    let url = "https://" + lang + ".wikipedia.org/wiki/" + title;
    let target = host.browser_open(url);
    let page_title = "";
    let written = #{};
    try {
        host.browser_wait_ready(target);
        // Drop the site chrome first, then let the generic renderer read the
        // article body (#mw-content-text) into Markdown + figures.
        host.browser_evaluate(target, host.js("clean.js"), true);
        let leaf = #{ target_id: target, selector: "#mw-content-text", slug: fs_safe(title) };
        if out_dir != "" { leaf["out_dir"] = out_dir; }
        let saved = host.save_article(leaf);
        page_title = sf(saved, "title");
        written = map_get(saved, "written", ());
    } catch (e) {
        host.log("wikipedia: page read failed at " + url + " (" + e + ")");
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
    if out_dir == "" { out_dir = "~/tmp/wikipedia"; }

    let out = #{};
    if mode == "page" {
        out = read_page(host, p["title"], p["lang"], out_dir);
    } else {
        out = search(host, p["query"], p["lang"], p["limit"]);
    }

    let results = map_get(out, "results", []);
    let count = 0;
    if type_of(results) == "array" { count = results.len(); }
    let written = map_get(out, "written", ());
    if mode == "page" && type_of(written) != "()" { count = 1; }
    let finished = host.now();
    out["mode"] = mode;
    out["query"] = p["query"];
    out["title"] = p["title"];
    out["lang"] = p["lang"];
    out["count"] = count;
    out["out_dir"] = out_dir;
    out["fetched_at"] = started["rfc3339"];
    out["finished_at"] = finished["rfc3339"];
    out
}
