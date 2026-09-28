// ─────────────────────────────────────────────────────────────────────────────
// alphaXiv paper downloader — Laya workflow plugin.
//
// This file holds *site* logic: how a natural-language query maps to a mode, how
// an `/abs/<id>` URL is localized, which endpoint the trending feed is paged
// from, how a listing page's cards become rows, and the per-paper retry.
//
// The Rust base underneath supplies the transport (CDP), the renderer
// (`host.save_article`), the policy gate and the tab lifecycle. The only way out
// of this script is the `host` handle, so every external effect still passes the
// engine's own checks.
// ─────────────────────────────────────────────────────────────────────────────

// ── small helpers ───────────────────────────────────────────────────────────

// Rhai's `trim()`/`replace()` hand back a borrowed slice that does not survive a
// return, so whitespace handling is done explicitly here (and only here).
fn is_space(c) {
    c == " " || c == "\t" || c == "\n" || c == "\r"
}

fn strip(s) {
    let a = 0;
    let n = s.len();
    while a < n && is_space(s.sub_string(a, 1)) { a += 1; }
    let b = n;
    while b > a && is_space(s.sub_string(b - 1, 1)) { b -= 1; }
    s.sub_string(a, b - a)
}

// Whitespace-separated tokens, without `trim()`/`replace()`.
fn words(s) {
    let out = [];
    let cur = "";
    let i = 0;
    while i < s.len() {
        let c = s.sub_string(i, 1);
        if is_space(c) {
            if cur != "" { out.push(cur); cur = ""; }
        } else {
            cur += c;
        }
        i += 1;
    }
    if cur != "" { out.push(cur); }
    out
}

// `with["key"]` as a trimmed string; "" when missing, null or the literal "null".
fn opt(args, key) {
    if !args.contains(key) { return ""; }
    let v = args[key];
    if type_of(v) == "()" { return ""; }
    let s = strip("" + v);
    if s == "null" { return ""; }
    s
}

fn opt_any(args, keys) {
    for k in keys {
        let v = opt(args, k);
        if v != "" { return v; }
    }
    ""
}

fn opt_int(args, key, dflt) {
    if !args.contains(key) { return dflt; }
    if type_of(args[key]) == "i64" { return args[key]; }
    dflt
}

fn trim_start_slashes(s) {
    let i = 0;
    while i < s.len() && s.sub_string(i, 1) == "/" { i += 1; }
    if i == 0 { return s; }
    s.sub_string(i, s.len() - i)
}

// ── classification: decide before acting ────────────────────────────────────

// True when a natural-language query points at a concrete paper page.
fn looks_like_paper_url(text) {
    let t = strip(text);
    let l = t.to_lower();
    if l.starts_with("http://") || l.starts_with("https://") { return true; }
    if l.contains("alphaxiv.org/abs/") { return true; }
    if l.contains("arxiv.org/abs/") { return true; }
    if l.contains("arxiv.org/pdf/") { return true; }
    false
}

// True for a BCP-47-ish locale path segment (`zh`, `en`, `zh-cn`, `pt-br`).
fn is_locale_segment(seg) {
    if seg.len() == 2 { return true; }
    if seg.len() == 5 && seg.sub_string(2, 1) == "-" { return true; }
    false
}

// True when the query asks for the trending/explore feed, not a keyword search.
fn is_trending(text) {
    const TRENDING_WORDS = ["热门", "趋势", "推荐", "最新"];
    let raw = strip(text);
    let l = raw.to_lower();
    if l == "" { return false; }
    for w in TRENDING_WORDS { if l.contains(w) { return true; } }
    if l.contains("/trending") { return true; }
    if l == "explore" || l == "feed" || l == "trending papers" { return true; }
    for t in words(l) { if t == "trending" { return true; } }
    false
}

// The feed time window, restricted to the API's allowed values (default 7 Days).
fn interval_of(args) {
    const INTERVALS = ["3 Days", "7 Days", "30 Days", "90 Days", "All time"];
    let raw = opt(args, "interval").to_lower();
    for a in INTERVALS { if a.to_lower() == raw { return a; } }
    "7 Days"
}

// Rewrite an alphaXiv `/abs/<id>` URL to `/<lang>/abs/<id>`, dropping any locale
// already present. Root listing pages have no localized variant, so only paper
// paths change; other hosts and an `en`/`off`/empty lang are returned untouched.
fn localize(url, lang) {
    const LOCALES_OFF = ["en", "auto", "none", "off"];
    let raw = strip(lang);
    let l = trim_start_slashes(raw.to_lower());
    if l == "" { return url; }
    for off in LOCALES_OFF { if l == off { return url; } }

    let sep = url.to_lower().index_of("://");
    if sep < 0 { return url; }
    let rest = url.sub_string(sep + 3, url.len() - (sep + 3));
    let slash = rest.index_of("/");
    if slash < 0 { return url; }
    let authority = rest.sub_string(0, slash);
    let ahost = authority.to_lower();
    if ahost.starts_with("www.") { ahost = ahost.sub_string(4, ahost.len() - 4); }
    if ahost != "alphaxiv.org" { return url; }

    let path = rest.sub_string(slash, rest.len() - slash);
    let q = path.index_of("?");
    if q >= 0 { path = path.sub_string(0, q); }
    let h = path.index_of("#");
    if h >= 0 { path = path.sub_string(0, h); }

    let segs = trim_start_slashes(path).split("/");
    let segs = segs;
    if segs.len() >= 2 && is_locale_segment(segs[0]) { segs.remove(0); }
    if segs.len() == 0 || segs[0] != "abs" { return url; }

    let id = "";
    let i = 1;
    while i < segs.len() {
        if i > 1 { id += "/"; }
        id += segs[i];
        i += 1;
    }
    if id == "" { return url; }
    url.sub_string(0, sep) + "://" + authority + "/" + l + "/abs/" + id
}

// ── intent → plan ───────────────────────────────────────────────────────────

// Resolve a call's intent into `#{mode, query, target_url, count}`. `mode` may be
// forced; `auto` infers it: a paper URL → `url`, a trending keyword → `trending`,
// anything else → `search`. This is what lets the CLI accept a bare
// `--query "llm memory"` or `--query "trending"` with no JSON state.
fn plan(args) {
    const MAX_COUNT = 500;
    let query = opt_any(args, ["query", "q", "topic"]);
    let paper_url = opt_any(args, ["paper_url", "url"]);
    let explicit = opt(args, "mode").to_lower();
    let mode = "";

    if explicit == "" || explicit == "auto" {
        if paper_url != "" { mode = "url"; }
        else if query == "" { mode = ""; }
        else if looks_like_paper_url(query) { mode = "url"; }
        else if is_trending(query) { mode = "trending"; }
        else { mode = "search"; }
    } else if explicit == "url" || explicit == "paper" || explicit == "direct" || explicit == "abs" {
        mode = "url";
    } else if explicit == "trending" || explicit == "feed" || explicit == "explore"
        || explicit == "hot" || explicit == "popular" {
        mode = "trending";
    } else if explicit == "search" || explicit == "query" || explicit == "find" {
        mode = "search";
    } else {
        throw "alphaxiv mode '" + explicit + "' is not supported (auto|search|trending|url)";
    }

    if mode == "" {
        throw "alphaxiv needs 'query' (search text, a paper URL, or 'trending') or 'paper_url'";
    }

    let target_url = "";
    if mode == "url" {
        if paper_url != "" { target_url = paper_url; }
        else if looks_like_paper_url(query) { target_url = query; }
        else { throw "alphaxiv url mode needs a paper URL in 'paper_url' or 'query'"; }
    }

    // Trending defaults to a full page of ten and can page through the feed API,
    // so its count reaches MAX_COUNT; search is one rendered page (~10 cards).
    let trending = mode == "trending";
    let dflt = if trending { 10 } else { 1 };
    let mx = if trending { MAX_COUNT } else { 10 };
    let count = opt_int(args, "count", dflt);
    if count < 1 { count = 1; }
    if count > mx { count = mx; }

    #{ mode: mode, query: query, target_url: target_url, count: count }
}

// ── discovery ───────────────────────────────────────────────────────────────

// Read the rendered `/abs/` cards from a listing page (search or trending).
fn collect_links(host, page_url, limit) {
    let target = host.browser_open(page_url);
    let links = [];
    try {
        host.browser_wait_ready(target);
        let poll = host.timeout_ms() - 4000;
        if poll < 3000 { poll = 3000; }
        if poll > 12000 { poll = 12000; }
        let opts = #{ limit: limit, timeout_ms: poll };
        let js = "var __LAYA_OPTS__ = " + host.json_stringify(opts) + ";\n" + host.js("links.js");
        let value = host.browser_evaluate(target, js, true);
        if type_of(value) == "map" && value.contains("links") && type_of(value["links"]) == "array" {
            for row in value["links"] { links.push(row); }
        }
    } catch (e) {
        host.log("collect_links failed at " + page_url + ": " + e);
    }
    host.browser_release(target);
    links
}

// Page the public feed API from inside the page (the first-party origin the API
// expects), so trending can exceed the handful of cards the homepage renders.
fn collect_feed(host, count, interval) {
    let target = host.browser_open("https://www.alphaxiv.org/");
    let links = [];
    try {
        host.browser_wait_ready(target);
        let opts = #{ count: count, sort: "Hot", interval: interval };
        let js = "var __LAYA_OPTS__ = " + host.json_stringify(opts) + ";\n" + host.js("feed.js");
        let value = host.browser_evaluate(target, js, true);
        if type_of(value) == "map" && value.contains("links") && type_of(value["links"]) == "array" {
            for row in value["links"] { links.push(row); }
        }
    } catch (e) {
        host.log("collect_feed failed, falling back to the DOM: " + e);
    }
    host.browser_release(target);
    links
}

// ── entry point ─────────────────────────────────────────────────────────────

fn run(host, ctx) {
    let args = ctx["with"];
    let p = plan(args);
    let mode = p["mode"];
    let query = p["query"];
    let count = p["count"];

    let lang = opt(args, "lang");
    if lang == "" { lang = opt(args, "language"); }
    if lang == "" { lang = "zh"; }

    let out_dir = opt(args, "out_dir");
    if out_dir == "" { out_dir = "~/tmp/alphaxiv"; }
    let selector = opt(args, "selector");
    let image_base = opt(args, "image_dir");
    if image_base == "" { image_base = "images"; }
    let stable_ms = opt_int(args, "stable_ms", 1500);
    let max_wait_ms = opt_int(args, "max_wait_ms", 45000);
    let keep_open = args.contains("keep_open")
        && type_of(args["keep_open"]) == "bool"
        && args["keep_open"];
    // Discover a few extra cards so a stray first link cannot starve the count.
    let collect_limit = count;
    if collect_limit < 5 { collect_limit = 5; }
    if collect_limit > 10 { collect_limit = 10; }
    let feed_interval = interval_of(args);
    let started = host.now();

    let source_url = "";
    let discovered = [];
    let discovery = "";
    if mode == "url" {
        let url = localize(p["target_url"], lang);
        source_url = url;
        discovered = [#{ url: url, title: "" }];
        discovery = "url";
    } else if mode == "search" {
        let home = "https://www.alphaxiv.org/";
        let url = home + "?query=" + host.urlencode(query);
        source_url = url;
        discovered = collect_links(host, url, collect_limit);
        discovery = "dom";
    } else if mode == "trending" {
        source_url = "https://www.alphaxiv.org/";
        let found = collect_feed(host, count, feed_interval);
        if found.len() > 0 {
            discovered = found;
            discovery = "feed_api";
        } else {
            discovered = collect_links(host, source_url, collect_limit);
            discovery = "dom";
        }
    } else {
        throw "alphaxiv resolved mode '" + mode + "' is not runnable";
    }

    let selected = [];
    let i = 0;
    while i < discovered.len() && selected.len() < count {
        selected.push(discovered[i]);
        i += 1;
    }
    if selected.len() == 0 {
        throw "alphaxiv " + mode + " found no papers at '" + source_url + "' (query '" + query
            + "'); the listing may still have been loading";
    }

    let results = [];
    let failures = [];
    let index = 0;
    for link in selected {
        let raw = "";
        if type_of(link) == "map" && link.contains("url") { raw = "" + link["url"]; }
        let url = localize(raw, lang);
        let title_hint = "";
        if type_of(link) == "map" && link.contains("title") { title_hint = "" + link["title"]; }
        let slug = host.slug(url, title_hint);

        let leaf = #{
            url: url,
            slug: slug,
            out_dir: out_dir,
            image_dir: image_base + "/" + slug,
            stable_ms: stable_ms,
            max_wait_ms: max_wait_ms,
            keep_open: keep_open
        };
        if selector != "" { leaf["selector"] = selector; }
        // Provenance for each `<slug>_meta.json`, so a trending run is
        // self-describing (and timestamped) for later monitoring.
        let meta = #{ source: source_url, source_mode: mode, rank: index + 1, query: query };
        if mode == "trending" { meta["interval"] = feed_interval; }
        leaf["meta_extra"] = meta;

        // One retry: over a long page-through a single transient CDP error
        // (e.g. a navigation timeout) should not drop the paper.
        let saved = ();
        let last_error = "";
        let attempts = 0;
        loop {
            attempts += 1;
            try {
                saved = host.save_article(leaf);
                break;
            } catch (e) {
                last_error = "" + e;
                if attempts >= 2 { break; }
            }
        }

        if type_of(saved) == "()" {
            failures.push(#{
                rank: index + 1,
                url: url,
                title_hint: title_hint,
                attempts: attempts,
                error: last_error
            });
        } else {
            let written = ();
            if saved.contains("written") { written = saved["written"]; }
            let downloaded_at = ();
            if type_of(written) == "map" && written.contains("downloaded_at") {
                downloaded_at = written["downloaded_at"];
            }
            results.push(#{
                rank: index + 1,
                url: url,
                title_hint: title_hint,
                attempts: attempts,
                title: saved["title"],
                markdown_chars: saved["markdown_chars"],
                rendered: saved["rendered"],
                images: saved["images"],
                written: written,
                downloaded_at: downloaded_at
            });
        }
        index += 1;
    }

    let finished = host.now();
    #{
        mode: mode,
        query: query,
        lang: lang,
        discovery: discovery,
        source_url: source_url,
        interval: if mode == "trending" { feed_interval } else { () },
        started_at: started["rfc3339"],
        started_at_unix_ms: started["unix_ms"],
        finished_at: finished["rfc3339"],
        out_dir: out_dir,
        requested_count: count,
        selected_count: selected.len(),
        saved_count: results.len(),
        failed_count: failures.len(),
        results: results,
        failures: failures
    }
}
