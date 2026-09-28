// ─────────────────────────────────────────────────────────────────────────────
// Hacker News reader — Laya workflow plugin (Chrome/CDP).
//
// Site logic only: which HN list a phrase means, how a front-page row plus its
// `.subtext` sibling becomes one record, and how a discussion page's comment
// rows become a list. The Rust base owns the tab, the CDP transport, the policy
// gate and the tab lifecycle, so this file decides *what* to read and nothing
// more: it never touches the network except through `host.*`.
//
// Rhai sharp edges honoured here: constants live *inside* functions, borrowed
// slices are never returned, and there is no `let mut`.
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

// A map field as a string ("" when missing/null).
fn sf(m, k) {
    let v = map_get(m, k, ());
    if type_of(v) == "()" { return ""; }
    "" + v
}

// A map field as an int (0 when missing/unparseable).
fn nf(m, k) { to_int(map_get(m, k, ()), 0) }

// A front-page list word → its path on news.ycombinator.com.
fn feed_path(word) {
    const PATHS = #{
        top: "/", front: "/", home: "/", best: "/best",
        "new": "/newest", newest: "/newest", ask: "/ask",
        show: "/show", jobs: "/jobs"
    };
    if PATHS.contains(word) { return PATHS[word]; }
    ""
}

// The story id inside `item?id=<id>`, a URL, or a bare id. "" when absent.
fn story_id_of(text) {
    let t = trim_str(text);
    if t == "" { return ""; }
    let at = t.index_of("item?id=");
    let skip = 8;
    if at < 0 { at = t.index_of("?id="); skip = 4; }
    if at < 0 { at = t.index_of("id="); skip = 3; }
    let rest = t;
    if at >= 0 { rest = t.sub_string(at + skip, t.len() - (at + skip)); }
    let out = "";
    let i = 0;
    while i < rest.len() {
        let c = rest.sub_string(i, 1);
        if c >= "0" && c <= "9" { out += c; i += 1; } else { break; }
    }
    out
}

// ── intent → plan (pure) ────────────────────────────────────────────────────

fn plan(args) {
    let query = trim_str(s_of(map_get(args, "query", "")));
    if query == "" { query = trim_str(s_of(map_get(args, "q", ""))); }
    let id = trim_str(s_of(map_get(args, "id", "")));
    if id == "" { id = trim_str(s_of(map_get(args, "item", ""))); }
    let explicit = trim_str(s_of(map_get(args, "mode", ""))).to_lower();

    let mode = "";
    if explicit != "" && explicit != "auto" {
        mode = explicit;
    } else if id != "" {
        mode = "item";
    } else if query != "" {
        let sid = story_id_of(query);
        if sid != "" {
            mode = "item";
            id = sid;
        } else {
            let w = query.to_lower();
            if feed_path(w) != "" { mode = w; }
            else if w == "hot" || w == "popular" || w == "trending" { mode = "best"; }
            else if w.contains("show hn") || w.contains("showhn") { mode = "show"; }
            else if w.contains("ask hn") || w.contains("askhn") { mode = "ask"; }
            else if w.contains("job") { mode = "jobs"; }
            else { mode = "search"; }
        }
    } else {
        mode = "top";
    }

    if mode == "front" || mode == "home" { mode = "top"; }
    if mode == "newest" { mode = "new"; }
    if mode == "comments" || mode == "discussion" { mode = "item"; }
    if mode == "query" || mode == "find" { mode = "search"; }
    if mode == "hot" || mode == "popular" || mode == "trending" { mode = "best"; }

    const ALL = ["top", "best", "new", "ask", "show", "jobs", "item", "search"];
    let known = false;
    for m in ALL { if m == mode { known = true; } }
    if !known {
        throw "hackernews: mode '" + mode + "' is not supported (top|best|new|ask|show|jobs|item|search)";
    }
    if mode == "item" {
        if id == "" { id = story_id_of(query); }
        if id == "" { throw "hackernews: item mode needs a story id or an item?id=<id> URL"; }
    }
    if mode == "search" && query == "" { throw "hackernews: search mode needs a query"; }

    let dflt = if mode == "item" { 20 } else { 30 };
    let limit = to_int(map_get(args, "count", ()), 0);
    if limit <= 0 { limit = to_int(map_get(args, "limit", ()), dflt); }
    if limit <= 0 { limit = dflt; }
    if limit > 100 { limit = 100; }

    #{ mode: mode, query: query, id: id, limit: limit }
}

// ── reads (browser + one API) ───────────────────────────────────────────────

fn list_url(mode, id) {
    const PATHS = #{ top: "/", best: "/best", "new": "/newest", ask: "/ask", show: "/show", jobs: "/jobs" };
    if mode == "item" { return "https://news.ycombinator.com/item?id=" + id; }
    "https://news.ycombinator.com" + PATHS[mode]
}

fn read_list(host, mode, limit) {
    let url = list_url(mode, "");
    let target = host.browser_open(url);
    let stories = [];
    let page_title = "";
    try {
        host.browser_wait_ready(target);
        let opts = #{ limit: limit };
        let js = "var __LAYA_OPTS__ = " + host.json_stringify(opts) + ";\n" + host.js("front.js");
        let value = host.browser_evaluate(target, js, true);
        if type_of(value) == "map" {
            let s = map_get(value, "stories", []);
            if type_of(s) == "array" { stories = s; }
            page_title = sf(value, "page_title");
        }
    } catch (e) {
        host.log("hackernews: list read failed at " + url + " (" + e + ")");
    }
    host.browser_release(target);
    #{ url: url, page_title: page_title, stories: stories }
}

fn read_item(host, id, limit) {
    let url = "https://news.ycombinator.com/item?id=" + id;
    let target = host.browser_open(url);
    let out = #{};
    try {
        host.browser_wait_ready(target);
        let opts = #{ limit: limit };
        let js = "var __LAYA_OPTS__ = " + host.json_stringify(opts) + ";\n" + host.js("story.js");
        let value = host.browser_evaluate(target, js, true);
        if type_of(value) == "map" { out = value; }
    } catch (e) {
        host.log("hackernews: item read failed at " + url + " (" + e + ")");
    }
    host.browser_release(target);
    out["url"] = url;
    out["id"] = id;
    out
}

// Full-text search over the public Algolia index (the engine's own HN search).
fn search_algolia(host, query, limit) {
    let url = "https://hn.algolia.com/api/v1/search?tags=story&hitsPerPage=" + limit
        + "&query=" + host.urlencode(query);
    let stories = [];
    try {
        let resp = host.http_get(url);
        let body = host.json_parse(sf(resp, "text"));
        let hits = map_get(body, "hits", []);
        if type_of(hits) == "array" {
            let i = 0;
            while i < hits.len() {
                let h = hits[i];
                let sid = sf(h, "objectID");
                stories.push(#{
                    rank: i + 1,
                    id: sid,
                    title: sf(h, "title"),
                    url: sf(h, "url"),
                    site: "",
                    points: nf(h, "points"),
                    comments: nf(h, "num_comments"),
                    user: sf(h, "author"),
                    age: sf(h, "created_at"),
                    hn_url: "https://news.ycombinator.com/item?id=" + sid,
                    comments_url: "https://news.ycombinator.com/item?id=" + sid
                });
                i += 1;
            }
        }
    } catch (e) {
        host.log("hackernews: search failed (" + e + ")");
    }
    #{ url: url, source: "algolia", stories: stories }
}

// ── entry point ─────────────────────────────────────────────────────────────

fn run(host, ctx) {
    let args = ctx["with"];
    let p = plan(args);
    let mode = p["mode"];
    let limit = p["limit"];
    let started = host.now();

    let out = #{};
    if mode == "item" {
        out = read_item(host, p["id"], limit);
    } else if mode == "search" {
        out = search_algolia(host, p["query"], limit);
    } else {
        out = read_list(host, mode, limit);
    }

    let stories = map_get(out, "stories", []);
    let count = 0;
    if type_of(stories) == "array" { count = stories.len(); }
    let finished = host.now();
    out["feed"] = mode;
    out["query"] = p["query"];
    out["limit"] = limit;
    out["count"] = count;
    out["fetched_at"] = started["rfc3339"];
    out["finished_at"] = finished["rfc3339"];
    out["tz"] = started["tz"];
    out
}
