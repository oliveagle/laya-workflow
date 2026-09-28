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

// ── alphaXiv "AI Overview": ask the site for it when it is not there yet ────

// Booleans reach a plugin as a real bool, as a JSON string, or not at all.
fn opt_bool(args, key, dflt) {
    if !args.contains(key) { return dflt; }
    if type_of(args[key]) == "bool" { return args[key]; }
    if type_of(args[key]) == "()" { return dflt; }
    let s = opt(args, key).to_lower();
    if s == "" { return dflt; }
    s == "true" || s == "1" || s == "yes" || s == "on" || s == "y"
}

// One field of a `browser_evaluate` result, as text; "" when it is not there.
fn ov_str(m, key) {
    if type_of(m) == "map" && m.contains(key) && type_of(m[key]) != "()" {
        return "" + m[key];
    }
    ""
}

// Same, as an integer; `dflt` when absent or not a number.
fn ov_int(m, key, dflt) {
    if type_of(m) == "map" && m.contains(key) {
        if type_of(m[key]) == "i64" { return m[key]; }
        if type_of(m[key]) == "f64" { return m[key]; }
    }
    dflt
}

// Run `page/overview.js` against `target` and hand back whatever it resolved
// with. The script only ever resolves, so the sole failure mode here is the CDP
// call itself — a navigation destroying the execution context mid-poll.
fn overview_probe(host, target, action, budget_ms) {
    let opts = #{ action: action, timeout_ms: budget_ms, poll_ms: 2000 };
    let js = "var __LAYA_OPTS__ = " + host.json_stringify(opts) + ";\n" + host.js("overview.js");
    host.browser_evaluate(target, js, true)
}

// Make sure `url`'s AI Overview really exists, so the capture below cannot save
// the placeholder copy ("No overview yet. Generate one to research the paper…")
// and call the paper's Markdown complete.
//
// alphaXiv renders `#overview` as a skeleton first, then either the finished
// article or a "Generate overview" button — and the site says generation takes
// about five minutes, server-side, pushed back over its own live channel. So
// when the placeholder is up we click the site's own button and wait for the
// result, instead of polling an API the plugin has no business reproducing.
//
// Everything here is bounded, because it costs minutes per paper: one poll round
// per `step_ms` (kept under the per-call CDP budget), an overall `wait_cap_ms`
// deadline, and a couple of re-navigations for the sign-in round trip a click can
// trigger. It never throws — a generation problem downgrades to "saved without
// an overview" rather than losing the paper.
//
// Returns the outcome plus the `target_id` of the tab it waited on, so the caller
// can capture that very page instead of navigating to it a second time.
fn ov_now_ms(host) {
    let t = host.now();
    if type_of(t) == "map" && t.contains("unix_ms") && type_of(t["unix_ms"]) == "i64" {
        return t["unix_ms"];
    }
    0
}

fn ensure_overview(host, url, wait_cap_ms, step_ms) {
    // A CDP call is capped by the capability timeout, so a poll round has to
    // finish inside it or the page script is killed while it waits.
    const MAX_STEP = 20000;
    const MIN_STEP = 4000;
    const RETRY_CLICK_MS = 30000;
    const MAX_CLICKS = 2;
    const MAX_REOPENS = 3;
    const FAILS_PER_REOPEN = 3;
    const MAX_REENTERS = 20;
    const REENTER_MS = 45000;
    // Signed out, alphaXiv renders no generate button at all, so there is
    // nothing to click and no result to wait for. Give up after this many
    // re-entries without ever having seen one, instead of sitting out the whole
    // budget on every paper.
    const MAX_SIGHTLESS_REENTERS = 3;
    const FINAL_LOOK_MS = 20000;

    let step = step_ms;
    if step > MAX_STEP { step = MAX_STEP; }
    if step < MIN_STEP { step = MIN_STEP; }

    let out = #{
        attempted: true,
        clicked: false,
        ready: false,
        state: "unknown",
        initial_state: "unknown",
        waited_ms: 0,
        polls: 0,
        clicks: 0,
        renavigations: 0,
        reopens: 0,
        url: url
    };

    // Wall clock, not the sum of the poll budgets: a tab that dies makes a poll
    // return instantly, and charging it its full slice would spend the whole
    // waiting allowance in a couple of seconds.
    let started_ms = ov_now_ms(host);

    // One retry on opening: a freshly created background tab can be swapped out
    // from under the first navigation, which is a transient CDP error and not a
    // reason to give up on the paper.
    let target = "";
    let opens = 0;
    loop {
        opens += 1;
        try {
            target = host.browser_open(url);
            host.browser_wait_ready(target);
            break;
        } catch (e) {
            out["last_error"] = "open: " + e;
            if target != "" { host.browser_release(target); target = ""; }
            if opens >= 2 { out["error"] = out["last_error"]; return out; }
        }
    }
    out["target_id"] = target;

    let clicks = 0;
    let last_click_ms = -999999;
    let last_enter_ms = started_ms;
    let fails = 0;
    let off_page = 0;
    let sightless = 0;
    loop {
        let now_ms = ov_now_ms(host);
        let elapsed = now_ms - started_ms;
        out["waited_ms"] = elapsed;

        // Every pass re-probes: the click and the sign-in detour both come back
        // through here, and only the live DOM says where the tab actually is.
        let seen = #{};
        let probe_ok = true;
        try { seen = overview_probe(host, target, "probe", 4000); }
        catch (e) { probe_ok = false; out["last_error"] = "probe: " + e; }

        if !probe_ok {
            // The tab itself is gone (Chrome discards a background page nobody is
            // driving). Keep probing it and the run just spins, so replace it.
            fails += 1;
            if fails >= FAILS_PER_REOPEN {
                // Once there is nothing left to replace it with, stop: continuing
                // would spin on a dead target for the rest of the budget and burn
                // the engine's operation ceiling instead of making progress.
                if ov_int(out, "reopens", 0) >= MAX_REOPENS {
                    out["error"] = "the browser tab stopped responding: " + ov_str(out, "last_error");
                    break;
                }
                fails = 0;
                host.browser_release(target);
                try {
                    target = host.browser_open(url);
                    host.browser_wait_ready(target);
                    out["reopens"] = ov_int(out, "reopens", 0) + 1;
                    out["target_id"] = target;
                    last_enter_ms = ov_now_ms(host);
                } catch (e) {
                    out["error"] = "reopen: " + e;
                    break;
                }
            }
            if elapsed >= wait_cap_ms { break; }
            continue;
        }
        fails = 0;

        let state = ov_str(seen, "state");
        if state == "" { state = "unknown"; }
        if state == "placeholder" { sightless = 0; }
        if ov_str(out, "initial_state") == "unknown" { out["initial_state"] = state; }
        out["state"] = state;

        if state == "ready" { out["ready"] = true; break; }

        // Click once the placeholder is up. The site also offers "Try again"
        // after a run that died, so one well-spaced retry is allowed.
        if state == "placeholder" && clicks < MAX_CLICKS
            && elapsed - last_click_ms > RETRY_CLICK_MS {
            let hit = #{};
            let clicked = false;
            try {
                hit = overview_probe(host, target, "click", 4000);
                clicked = ov_str(hit, "clicked") == "true";
            } catch (e) { out["last_error"] = "click: " + e; }
            if clicked {
                clicks += 1;
                out["clicks"] = clicks;
                out["clicked"] = true;
                last_click_ms = elapsed;
                host.log("alphaxiv: asked alphaXiv to generate the overview for " + url);
                continue;
            }
        }

        if elapsed >= wait_cap_ms { break; }

        // One poll round inside the remaining budget.
        let remaining = wait_cap_ms - elapsed;
        let slice = step;
        if slice > remaining { slice = remaining; }
        if slice < MIN_STEP { break; }
        let chunk = #{};
        let ok = true;
        try { chunk = overview_probe(host, target, "wait", slice); }
        catch (e) { ok = false; out["last_error"] = "wait: " + e; }
        out["polls"] = ov_int(out, "polls", 0) + 1;
        // Deliberately not counted as a failure: Chrome throttles timers in a
        // background tab, so a poll round can overrun the capability's per-call
        // timeout while the page is perfectly healthy. The probe above is what
        // decides whether the tab itself is gone.
        if !ok { continue; }

        // Re-entering the paper is the only way to learn the result. The
        // generation itself is server-side and survives a reload, but the page's
        // live channel is not: a click that bounces through alphaXiv's sign-in
        // flow re-mounts the section, and from then on that page instance sits
        // empty and never hears about a run it did not watch. A fresh load both
        // re-subscribes and re-reads what the server already has — which is how
        // a finished overview shows up, minutes later, on a page loaded then.
        let href = ov_str(chunk, "href");
        let reenter = false;
        if !href.contains("/abs/") {
            // Off the paper entirely: let a sign-in round trip finish on its own
            // before dragging the tab back.
            off_page += 1;
            reenter = off_page >= 2;
        } else {
            off_page = 0;
            reenter = ov_str(chunk, "state") == "unknown"
                && ov_now_ms(host) - last_enter_ms >= REENTER_MS;
        }
        // An empty section that never grows into the placeholder, across several
        // fresh loads, means this browser is not signed in: alphaXiv only offers
        // generation to a signed-in reader. Waiting longer cannot change that.
        if reenter && clicks == 0 && ov_str(chunk, "state") == "unknown" {
            sightless += 1;
            if sightless >= MAX_SIGHTLESS_REENTERS {
                out["error"] = "no Generate overview button appeared (signed out?)";
                break;
            }
        }
        if reenter && ov_int(out, "renavigations", 0) < MAX_REENTERS {
            try {
                host.browser_navigate(target, url);
                host.browser_wait_ready(target);
                out["renavigations"] = ov_int(out, "renavigations", 0) + 1;
                last_enter_ms = ov_now_ms(host);
                off_page = 0;
            } catch (e) { out["last_error"] = "reenter: " + e; }
        }
    }

    // The budget ran out (or generation was never requested): one last look, so a
    // run that finished moments ago is still saved with its overview.
    if !out["ready"] && ov_int(out, "clicks", 0) > 0 {
        try {
            host.browser_navigate(target, url);
            host.browser_wait_ready(target);
            let last = overview_probe(host, target, "wait", FINAL_LOOK_MS);
            out["state"] = ov_str(last, "state");
            out["ready"] = out["state"] == "ready";
        } catch (e) { out["last_error"] = "final: " + e; }
    }
    out["waited_ms"] = ov_now_ms(host) - started_ms;

    out
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

    // A missing AI Overview is worth generating: alphaXiv answers the request in
    // about five minutes, server-side. It is opt-out, and default-on only where
    // the user named one paper — a listing run would otherwise turn into hours
    // of generation, so there it stays off unless asked for, and then only for
    // the first `overview_max_papers`.
    let want_overview = opt_bool(args, "generate_overview", mode == "url");
    let overview_wait_ms = opt_int(args, "overview_wait_ms", 420000);
    let overview_step_ms = opt_int(args, "overview_step_ms", 20000);
    let overview_max_papers = opt_int(args, "overview_max_papers",
        if mode == "url" { 1 } else { 3 });
    if overview_max_papers < 0 { overview_max_papers = 0; }

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
        // An ungenerated overview makes the capture worthless: the section reads
        // "No overview yet…", so the Markdown looks finished while carrying no
        // overview at all. Ask for it first, then save the very tab we waited on
        // instead of navigating to the paper a second time.
        let ov = #{
            attempted: false,
            clicked: false,
            ready: false,
            state: "skipped",
            initial_state: "skipped",
            waited_ms: 0,
            polls: 0,
            clicks: 0,
            renavigations: 0,
            url: url
        };
        let target = "";
        if want_overview && index < overview_max_papers {
            ov = ensure_overview(host, url, overview_wait_ms, overview_step_ms);
            target = ov_str(ov, "target_id");
        } else if want_overview {
            ov["state"] = "over_paper_budget";
        }
        if target != "" { leaf.remove("url"); leaf["target_id"] = target; }

        // Provenance for each `<slug>_meta.json`, so a run is self-describing
        // (and timestamped) for later monitoring — including whether the AI
        // Overview was already there, was generated here, or could not be.
        let meta = #{ source: source_url, source_mode: mode, rank: index + 1, query: query };
        if mode == "trending" { meta["interval"] = feed_interval; }
        meta["overview_attempted"] = ov["attempted"];
        meta["overview_state"] = ov["state"];
        meta["overview_ready"] = ov["ready"];
        meta["overview_clicked"] = ov["clicked"];
        meta["overview_waited_ms"] = ov["waited_ms"];
        if ov.contains("error") { meta["overview_error"] = ov["error"]; }
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
                // A tab that died mid-generation cannot be captured; spend the
                // last try on a fresh navigation to the paper.
                if target != "" && attempts < 2 {
                    leaf.remove("target_id");
                    leaf["url"] = url;
                }
                if attempts >= 2 { break; }
            }
        }
        if target != "" { host.browser_release(target); }

        if type_of(saved) == "()" {
            failures.push(#{
                rank: index + 1,
                url: url,
                title_hint: title_hint,
                attempts: attempts,
                overview_state: ov["state"],
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
                overview_state: ov["state"],
                overview_ready: ov["ready"],
                overview_clicked: ov["clicked"],
                overview_waited_ms: ov["waited_ms"],
                overview_error: if ov.contains("last_error") { ov["last_error"] } else { "" },
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

    let generated = 0;
    let overview_waited = 0;
    for r in results {
        if ov_str(r, "overview_ready") == "true" { generated += 1; }
        overview_waited += ov_int(r, "overview_waited_ms", 0);
    }

    let finished = host.now();
    #{
        mode: mode,
        overview: #{
            generate: want_overview,
            max_papers: overview_max_papers,
            ready_count: generated,
            waited_ms: overview_waited
        },
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
