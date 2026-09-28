// ─────────────────────────────────────────────────────────────────────────────
// goofish (闲鱼) plugin — search, browse, collect, and watch prices.
//
// This file holds the *site policy*: how a natural-language query maps to a
// mode, which URL a listing or an item is, what the durable watch store looks
// like, and how a fresh price is folded into the price history.
//
// The Rust base underneath supplies the transport (CDP), the renderer
// (`host.save_article`), the policy gate and the tab lifecycle, so every
// external effect still passes through the engine's own checks.
//
// Three facts about goofish that shape the design, all verified against the
// live site:
//
//   * `/search` ignores `?page=`, `?sort=`, `?priceMin=` — every value returns
//     the same 30 ids. Paging is in-page React state, so `page/search.js`
//     clicks the site's own page boxes.
//   * The detail page is a SPA that renders the site footer immediately, so
//     "did the item load" cannot be keyed on body text length.
//   * Prices are plain DOM numbers, and a sold item renders no item block at
//     all rather than a page saying "sold" — so `status` is explicit.
//
// Price watching is durable: every observation is appended to
// `<out_dir>/watch.json`, which is what makes a later `watch` run able to say
// what changed since the last time — not merely what the price is now.
// ─────────────────────────────────────────────────────────────────────────────

// ── small helpers ───────────────────────────────────────────────────────────

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

// Numeric options arrive three ways depending on how the caller passed them: as
// a JSON integer, as a JSON float, or as a string ("15000") once a spec has
// interpolated `${state.price_min}`. Both readers take all three.
//
// The integer case is the one that bit: opt_float used to accept f64 only, so
// `--state '{"price_min":15000}'` silently became 0.0 and the price filter
// dropped nothing at all.
fn numeric_text(s) {
    let t = strip("" + s);
    if t == "" || t == "null" { return false; }
    let c = t.sub_string(0, 1);
    if c == "-" || c == "+" { t = t.sub_string(1, t.len() - 1); }
    if t.len() == 0 { return false; }
    "0123456789".index_of(t.sub_string(0, 1)) >= 0
}

fn opt_int(args, key, dflt) {
    if !args.contains(key) { return dflt; }
    let v = args[key];
    if type_of(v) == "()" { return dflt; }
    if type_of(v) == "i64" { return v; }
    if type_of(v) == "f64" { return int_of(v); }
    if numeric_text(v) { return int_of(onum(v)); }
    dflt
}

fn opt_float(args, key, dflt) {
    if !args.contains(key) { return dflt; }
    let v = args[key];
    if type_of(v) == "()" { return dflt; }
    // Multiplying by a float promotes; Rhai has no cast operator.
    if type_of(v) == "f64" { return v; }
    if type_of(v) == "i64" { return v * 1.0; }
    if numeric_text(v) { return onum(v); }
    dflt
}

// int_of is a truncating cast. Rhai's own `int()` is not one in this engine
// build (it returns a float, which is how a price formatter once produced
// "1.0.98.0"), so the fraction is subtracted instead.
fn int_of(v) {
    let neg = v < 0.0;
    let a = if neg { -v } else { v };
    let n = int_of_str("" + a);
    if neg { -n } else { n }
}

fn int_of_str(s) {
    // s is the already-rendered magnitude of a non-negative float.
    let dot = s.index_of(".");
    let head = if dot < 0 { s } else { s.sub_string(0, dot) };
    let acc = 0;
    let i = 0;
    while i < head.len() {
        let d = "0123456789".index_of(head.sub_string(i, 1));
        if d < 0 { break; }
        acc = acc * 10 + d;
        i += 1;
    }
    acc
}

fn contains(hay, needle) {
    needle == "" || hay.contains(needle)
}

fn omap(v) {
    type_of(v) == "map"
}

// Rhai has no ternary operator, and reading a map field that may be missing is
// the single most common thing this script does — so the three shapes live here.
fn pick(m, key) {
    if omap(m) && m.contains(key) { return m[key]; }
    ()
}

fn pstr(m, key) {
    ostr(pick(m, key))
}

fn pstr2(m, key, dflt) {
    if omap(m) && m.contains(key) && type_of(m[key]) != "()" { return ostr(m[key]); }
    dflt
}

fn ostr(v) {
    if type_of(v) == "()" { return ""; }
    strip("" + v)
}

fn oint(v, dflt) {
    if type_of(v) == "i64" { return v; }
    dflt
}

// Decimal value of a single character, or -1 when it is not a digit. Rhai has
// no portable "char to int", and the price parser below has to walk the string
// itself, so the ten cases are spelled out.
fn digit(c) {
    const D = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    let i = 0;
    while i < 10 {
        if c == D[i] { return i; }
        i += 1;
    }
    -1
}

fn pow10(k) {
    let r = 1.0;
    let i = 0;
    while i < k { r = r * 10.0; i += 1; }
    r
}

// A price is stored as a number, but the watch file is JSON a human may hand
// edit, so the same field also arrives as a string ("948" / "948.00"). Read
// either shape into an f64; 0.0 means "no price to speak of".
fn onum(v) {
    if type_of(v) == "f64" { return v; }
    // Rhai has no cast operator; multiplying by a float promotes.
    if type_of(v) == "i64" { return v * 1.0; }
    if type_of(v) == "()" { return 0.0; }
    let s = strip("" + v);
    let neg = false;
    let i = 0;
    if s.len() > 0 && (s.sub_string(0, 1) == "-" || s.sub_string(0, 1) == "+") {
        neg = s.sub_string(0, 1) == "-";
        i = 1;
    }
    let whole = 0.0;
    let frac = 0.0;
    let seen = false;
    // Two counters, not one: `digits` was both the "already saw a dot" flag and
    // the fraction-digit count, and it started at 1 on the dot — so "20500.55"
    // divided 55 by 10³ and came back as 20500.055. Every two-decimal price in a
    // hand-edited watch file was read at a tenth of its value.
    let dot = false;
    let frac_digits = 0;
    while i < s.len() {
        let d = digit(s.sub_string(i, 1));
        if d >= 0 {
            whole = whole * 10.0 + d;
            seen = true;
            i += 1;
        } else if s.sub_string(i, 1) == "." && !dot {
            dot = true;
            i += 1;
            while i < s.len() {
                let f = digit(s.sub_string(i, 1));
                if f < 0 { break; }
                frac = frac * 10.0 + f;
                frac_digits += 1;
                seen = true;
                i += 1;
            }
            break;
        } else {
            break;
        }
    }
    if !seen { return 0.0; }
    let out = whole + frac / pow10(frac_digits);
    if neg { -out } else { out }
}

// ── classification: decide before acting ────────────────────────────────────

fn is_item_url(text) {
    let t = strip(text);
    let l = t.to_lower();
    if l.contains("goofish.com/item") || l.contains("2.taobao.com") || l.contains("item.htm") {
        return true;
    }
    // A bare 12-13 digit id is a goofish item id, not a search phrase.
    let d = words(strip(l));
    if d.len() == 1 {
        let w = d[0];
        if w.len() >= 12 && w.len() <= 13 {
            let all = true;
            let i = 0;
            while i < w.len() {
                if digit(w.sub_string(i, 1)) < 0 { all = false; break; }
                i += 1;
            }
            if all { return true; }
        }
    }
    false
}

// The id out of any of the URL spellings goofish hands out.
fn item_id_of(text) {
    let t = strip(text);
    let parts = split(t, '?');
    let q = if parts.len() > 1 { parts[1] } else { "" };
    for kv in split(q, '&') {
        let pair = split(kv, '=');
        if pair.len() == 2 && pair[0] == "id" {
            let v = pair[1];
            if v.len() > 0 { return v; }
        }
    }
    // …/item/<id>
    let segs = split(t, '/');
    let i = 0;
    while i < segs.len() {
        if segs[i] == "item" && i + 1 < segs.len() {
            let v = strip(segs[i + 1]);
            if v.len() > 0 && !v.contains("?") { return v; }
        }
        i += 1;
    }
    let d = words(t);
    if d.len() == 1 && d[0].len() >= 12 && d[0].len() <= 13 { return d[0]; }
    ""
}

// The item URL out of whatever the caller passed.
//
// The URL usually arrives inside a sentence — "添加监控 https://www.goofish.com/
// item?id=1071111102831" — so this scans the whitespace-separated words and
// returns the URL itself. Returning the whole string instead handed
// browser_open a line of prose, which is why `watch_add` found nothing.
fn item_url_of(text) {
    let t = strip(text);
    if t == "" { return ""; }
    for w in words(t) {
        let lw = w.to_lower();
        if lw.contains("goofish.com/item") || lw.contains("2.taobao.com") || lw.contains("item.htm") {
            return w;
        }
    }
    // A bare 12-13 digit id is an item too, and the id may be sitting in a
    // sentence as well ("盯这个 1071111102831").
    for w in words(t) {
        if w.len() >= 12 && w.len() <= 13 {
            let all = true;
            let i = 0;
            while i < w.len() {
                if digit(w.sub_string(i, 1)) < 0 { all = false; break; }
                i += 1;
            }
            if all { return "https://www.goofish.com/item?id=" + w; }
        }
    }
    ""
}

// True when the user is asking to look at tracked prices rather than to search.
fn is_watch_intent(text) {
    let l = strip(text).to_lower();
    if l == "" { return false; }
    const WORDS_CN = ["价格监控", "监控价格", "价格变化", "降价", "涨价", "盯一下", "关注价格", "价格走势"];
    for w in WORDS_CN { if l.contains(w) { return true; } }
    const WORDS_EN = ["price watch", "watch price", "price alert", "price change", "monitor price"];
    for w in WORDS_EN { if l.contains(w) { return true; } }
    false
}

fn is_watch_add(text) {
    let l = strip(text).to_lower();
    const WORDS_CN = ["添加监控", "加入监控", "关注这个", "加到监控", "盯这个", "监控这个", "订阅价格"];
    for w in WORDS_CN { if l.contains(w) { return true; } }
    const WORDS_EN = ["watch this", "add to watch", "track this", "monitor this"];
    for w in WORDS_EN { if l.contains(w) { return true; } }
    false
}

fn plan(args) {
    let query = opt_any(args, ["query", "q", "keyword", "keywords", "text"]);
    let raw_item = opt_any(args, ["item_url", "url", "item", "id"]);
    let explicit = opt(args, "mode").to_lower();

    let mode = explicit;
    if mode == "" {
        if is_watch_add(query) { mode = "watch_add"; }
        else if opt(args, "watch_file") != "" || opt(args, "watchlist") != "" { mode = "watch"; }
        else if is_watch_intent(query) && raw_item == "" { mode = "watch"; }
        else if raw_item != "" && query == "" { mode = "item"; }
        else if is_item_url(raw_item) { mode = "item"; }
        else { mode = "search"; }
    }

    // A search phrase that is really a URL/id still means "one item".
    let item = "";
    if mode == "item" || mode == "watch_add" {
        item = item_url_of(raw_item);
        if item == "" && query != "" {
            // The mode is already "one item", so take the URL or the id out of
            // the query wherever it sits: "添加监控 https://…" and "添加监控
            // 1087828137579" are the same request. is_item_url() is deliberately
            // not the gate here — it only matches a query that is *nothing but*
            // a URL or an id, and that strictness belongs to the inference
            // below, not to a mode the caller already chose.
            item = item_url_of(query);
        }
    } else if is_item_url(query) && opt(args, "count").is_empty() {
        // "https://…/item?id=…" asked for with no explicit mode: browse it.
        mode = "item";
        item = item_url_of(query);
        query = "";
    }

    // `search` on a URL the caller already localized is still a search, and its
    // `q=` term is already percent-encoded — do not encode it a second time.
    let preencoded = false;
    if mode == "search" && query.to_lower().contains("goofish.com/search") {
        query = search_term_from(query);
        preencoded = query != "";
    }

    let count = opt_int(args, "count", 0);
    if count <= 0 { count = if mode == "search" { 20 } else { 1 }; }

    #{
        mode: mode,
        query: query,
        preencoded: preencoded,
        item_url: item,
        item_id: if item == "" { "" } else { item_id_of(item) },
        count: count
    }
}

// Pull the `q=` term back out of a full /search URL, so a copied search URL can
// be handed straight to the plugin.
//
// The term comes back still percent-encoded, and that is deliberate: it goes
// back into the URL unchanged (`search_url` skips `urlencode` for a term that
// came from a URL). The engine registers `urlencode` with no decoder beside it,
// so turning "%E7%B4%A2" back into 索 is not available — and re-encoding the
// already-encoded form would search for the literal text "%E7%B4%A2", which
// matches nothing. Passing it through is both shorter and exactly right.
fn search_term_from(url) {
    let parts = split(strip(url), '?');
    let q = if parts.len() > 1 { parts[1] } else { "" };
    for kv in split(q, '&') {
        let pair = split(kv, '=');
        if pair.len() == 2 && pair[0] == "q" { return pair[1]; }
    }
    ""
}

// `preencoded` is set when the term came out of a pasted /search URL and is
// already percent-encoded — see search_term_from.
fn search_url(host, query, preencoded) {
    if preencoded {
        return "https://www.goofish.com/search?q=" + query;
    }
    "https://www.goofish.com/search?q=" + host.urlencode(query)
}

// ── the watch store ─────────────────────────────────────────────────────────
//
// `<out_dir>/watch.json` is the whole of price monitoring: one entry per item
// with every price ever observed, so a later run can say *what changed since
// the last time* instead of only what the price is right now. It is written
// with host.write_file so the engine's own path policy still applies.

fn watch_path(out_dir) {
    out_dir + "/watch.json"
}

fn empty_watch(now) {
    #{ version: 1, updated_at: now["rfc3339"], updated_at_ms: now["unix_ms"], items: [] }
}

// Read the watch file. A missing file is the normal first-run case and is not
// an error; a corrupt one is downgraded to empty rather than failing the run,
// because losing the history is better than refusing to collect.
fn load_watch(host, path, now) {
    let read = host.read_file(path);
    let exists = false;
    if omap(read) && read.contains("exists") && read["exists"] { exists = true; }
    if !exists { return empty_watch(now); }
    let text = "";
    if omap(read) && read.contains("text") && type_of(read["text"]) == "string" {
        text = read["text"];
    }
    if strip(text) == "" { return empty_watch(now); }
    let parsed = ();
    try { parsed = host.json_parse(text); }
    catch (e) { return empty_watch(now); }
    if !omap(parsed) || !parsed.contains("items") || type_of(parsed["items"]) != "array" {
        return empty_watch(now);
    }
    parsed
}

fn save_watch(host, path, data) {
    host.write_file(path, host.json_stringify(data))
}

fn watch_items(wdata) {
    if omap(wdata) && wdata.contains("items") && type_of(wdata["items"]) == "array" {
        return wdata["items"];
    }
    []
}

// Find the index of `id` in the store; -1 when it is not tracked yet.
fn watch_index(items, id) {
    let i = 0;
    while i < items.len() {
        let e = items[i];
        if omap(e) && e.contains("id") && ostr(e["id"]) == id { return i; }
        i += 1;
    }
    -1
}

// One observation, as it is stored in the history.
fn history_point(rec, now, status) {
    let price = ();
    if rec.contains("price") && type_of(rec["price"]) != "()" { price = onum(rec["price"]); }
    let want = ();
    if rec.contains("want_count") && type_of(rec["want_count"]) != "()" {
        want = oint(rec["want_count"], 0);
    }
    let views = ();
    if rec.contains("views") && type_of(rec["views"]) != "()" { views = oint(rec["views"], 0); }
    #{
        at: now["rfc3339"],
        at_ms: now["unix_ms"],
        price: price,
        want_count: want,
        views: views,
        status: status
    }
}

// Fold a freshly read record into the store and report what moved.
//
// Returns `#{ data, change }`. The store comes back *with* the call rather than
// by reference because Rhai maps are value types: writing `wdata["items"]`
// inside this function would otherwise update a local copy and be lost on
// return, and a price monitor that quietly forgets every sighting is worse than
// no monitor at all.
//
// `change.direction` is "new" for a first sighting, then "up" / "down" / "same"
// once there is a previous price, and "unknown" when the page carried no number
// (a 价格面议 listing, or a sold item with no price block).
fn observe(host, wdata, rec, now, hist_max) {
    let id = "";
    if rec.contains("id") { id = ostr(rec["id"]); }
    let url = "";
    if rec.contains("url") { url = ostr(rec["url"]); }
    let title = "";
    if rec.contains("title") { title = ostr(rec["title"]); }
    let status = "on_sale";
    if rec.contains("status") { status = ostr(rec["status"]); }

    let has_price = rec.contains("price") && type_of(rec["price"]) != "()";
    let price = if has_price { onum(rec["price"]) } else { 0.0 };

    let items = watch_items(wdata);
    let idx = watch_index(items, id);
    let point = history_point(rec, now, status);

    if idx < 0 {
        let entry = #{
            id: id,
            url: url,
            title: title,
            price: if has_price { price } else { () },
            currency: "CNY",
            status: status,
            first_seen: now["rfc3339"],
            first_seen_ms: now["unix_ms"],
            last_seen: now["rfc3339"],
            last_seen_ms: now["unix_ms"],
            seen_count: 1,
            history: [point]
        };
        items.push(entry);
        wdata["items"] = items;
        // First sighting of something already delisted is "gone", not "new":
        // a monitor that reports "new" here keeps alerting about a dead id.
        let dir = "new";
        if status == "gone" { dir = "gone"; }
        return #{ data: wdata, change: #{
            id: id,
            url: url,
            title: title,
            direction: dir,
            old_price: (),
            new_price: if has_price { price } else { () },
            delta: (),
            delta_pct: (),
            at: now["rfc3339"]
        } };
    }

    let entry = items[idx];
    let old_price = ();
    // `old_price` stays unit() when the previous sighting had no number, which
    // is what separates "first priced sighting" from "unchanged".
    if entry.contains("price") && type_of(entry["price"]) != "()" {
        old_price = onum(entry["price"]);
    }
    // Copied rather than aliased: the trim below drops from the front, and
    // that must not reach back into the map it came from.
    let history = [];
    if entry.contains("history") && type_of(entry["history"]) == "array" {
        for p in entry["history"] { history.push(p); }
    }
    history.push(point);
    // The store is a monitoring file, not an archive: keep the tail, and drop
    // the oldest points so a long-lived watch cannot grow without bound.
    while history.len() > hist_max { history.remove(0); }

    entry["url"] = url;
    if title != "" { entry["title"] = title; }
    entry["status"] = status;
    entry["last_seen"] = now["rfc3339"];
    entry["last_seen_ms"] = now["unix_ms"];
    entry["seen_count"] = oint(entry["seen_count"], 0) + 1;
    entry["history"] = history;
    if has_price { entry["price"] = price; }

    let direction = "unknown";
    let delta = ();
    let delta_pct = ();
    // Gone beats any price comparison: a delisted page carries no price, so the
    // old-number-vs-new-number branch below would say "unknown" for the single
    // most important change there is.
    if status == "gone" {
        direction = "gone";
        entry["price"] = ();
    } else if has_price && type_of(old_price) != "()" {
        let d = price - onum(old_price);
        delta = d;
        if onum(old_price) != 0.0 { delta_pct = (d / onum(old_price) * 10000.0).round() / 100.0; }
        if d > 0.0 { direction = "up"; }
        else if d < 0.0 { direction = "down"; }
        else { direction = "same"; }
    }

    // `items[idx]` hands out a *copy* of the map, so every edit above landed on
    // a local and the store came back untouched. Without this line the watch
    // file kept reporting the first price it ever saw, `seen_count` stayed at 1
    // and the history never grew — while the change this function reported was
    // computed from that stale copy and still looked plausible. Verified against
    // a real watch.json: an item bought at 800 and re-read at 690 kept
    // `price: 800, seen_count: 1`. It is written back last, after the `gone`
    // branch above has had its say about the price.
    items[idx] = entry;
    wdata["items"] = items;

    #{ data: wdata, change: #{
        id: id,
        url: url,
        title: title,
        direction: direction,
        old_price: old_price,
        new_price: if has_price { price } else { () },
        delta: delta,
        delta_pct: delta_pct,
        at: now["rfc3339"]
    } }
}

// ── fetching ────────────────────────────────────────────────────────────────

// Open `url`, run `page/<script>` on it, and release the tab. One retry, because
// a freshly created background tab can be swapped out from under the first
// navigation — that is a transient CDP error, not a reason to lose the item.
fn read_page(host, url, script, opts, budget_ms) {
    let out = #{ ok: false, error: "" };
    let target = "";
    let opens = 0;
    loop {
        opens += 1;
        try {
            target = host.browser_open(url);
            host.browser_wait_ready(target);
            break;
        } catch (e) {
            out["error"] = "open: " + e;
            if target != "" { host.browser_release(target); target = ""; }
            if opens >= 2 { return out; }
        }
    }
    try {
        let js = "var __LAYA_OPTS__ = " + host.json_stringify(opts) + ";\n" + host.js(script);
        out = host.browser_evaluate(target, js, true);
    } catch (e) {
        out["error"] = "evaluate: " + e;
    }
    host.browser_release(target);
    out
}

fn read_search(host, url, limit, pages, budget_ms) {
    read_page(host, url, "search.js",
        #{ limit: limit, max_pages: pages, timeout_ms: budget_ms }, budget_ms)
}

fn read_item(host, url, budget_ms) {
    read_page(host, url, "item.js", #{ timeout_ms: budget_ms }, budget_ms)
}

// goofish has no "give me every item under X yuan" endpoint — the price inputs
// on /search are React state and `?priceMin=` is ignored — so a price band is
// applied to what came back. Better a short list than a silently wrong one.
fn price_ok(row, lo, hi) {
    if !row.contains("price") || type_of(row["price"]) == "()" { return true; }
    let p = onum(row["price"]);
    if p <= 0.0 { return true; }
    if lo > 0.0 && p < lo { return false; }
    if hi > 0.0 && p > hi { return false; }
    true
}

// The durable per-item record: everything read from the page, plus the item's
// own JSON file path, so a later `watch` run and a human both see the same
// thing.
fn item_record(rec, rank, source_url, mode, now) {
    // A listing card carries the region on `location`; a detail page carries it
    // on the seller block instead. Lift one into a top-level field so a consumer
    // never has to know which of the two sources a row came from.
    let seller = pick(rec, "seller");
    let location = pstr(rec, "location");
    if location == "" { location = pstr(seller, "location"); }
    // Normalize the numbers here, once, instead of in every reader.
    //
    // The page scripts produce JSON, and JSON has one number type: a 20500 yuan
    // camera arrives as an integer and lands in Rhai as i64, while the same
    // page's 1.38万 arrives as f64. Downstream `type_of(price) == "f64"` checks
    // silently take the "no usable price" branch for the integer half of the
    // world, which is how the product sheet ended up printing the raw
    // "¥20500包邮" caption instead of the number. onum/oint flatten both shapes.
    let want = ();
    if rec.contains("want_count") && type_of(rec["want_count"]) != "()" {
        want = oint(rec["want_count"], 0);
    }
    let views = ();
    if rec.contains("views") && type_of(rec["views"]) != "()" {
        views = oint(rec["views"], 0);
    }
    let original = ();
    if rec.contains("original_price") && type_of(rec["original_price"]) != "()" {
        original = onum(rec["original_price"]);
    }
    let price = ();
    if rec.contains("price") && type_of(rec["price"]) != "()" { price = onum(rec["price"]); }
    #{
        id: pstr(rec, "id"),
        url: pstr(rec, "url"),
        title: pstr(rec, "title"),
        price: price,
        price_text: pstr(rec, "price_text"),
        currency: "CNY",
        location: location,
        want_count: want,
        views: views,
        price_note: pstr(rec, "price_note"),
        discount_text: pstr(rec, "discount_text"),
        status: pstr2(rec, "status", "on_sale"),
        collected_at: now["rfc3339"],
        collected_at_ms: now["unix_ms"],
        source_url: source_url,
        source_mode: mode,
        rank: rank,
        listing: #{
            want_count: want,
            original_price: original,
            drop_pct: if rec.contains("drop_pct") { rec["drop_pct"] } else { () },
            tags: if rec.contains("tags") { rec["tags"] } else { [] },
            location: pstr(rec, "location"),
            seller_credit: pstr(rec, "seller_credit"),
            image: pstr(rec, "image"),
            avatar: pstr(rec, "avatar"),
            image_placeholder: omap(rec) && rec.contains("image_placeholder") && rec["image_placeholder"]
        },
        detail: #{
            original_price: original,
            want_count: want,
            views: views,
            shipping: pstr(rec, "shipping"),
            description: pstr(rec, "description"),
            labels: if rec.contains("labels") { rec["labels"] } else { #{} },
            images: if rec.contains("images") { rec["images"] } else { [] },
            seller: if rec.contains("seller") { rec["seller"] } else { #{} }
        }
    }
}

// `opt_bool(args, key, default)` — the switch that decides whether a missing
// value falls back to the default or is read as false.
fn opt_bool(args, key, dflt) {
    if !args.contains(key) { return dflt; }
    let v = args[key];
    if type_of(v) == "bool" { return v; }
    if type_of(v) == "()" { return dflt; }
    let s = strip("" + v).to_lower();
    if s == "" || s == "null" { return dflt; }
    if s == "true" || s == "1" || s == "yes" || s == "on" { return true; }
    if s == "false" || s == "0" || s == "no" || s == "off" { return false; }
    dflt
}

// ── entry point ─────────────────────────────────────────────────────────────

fn run(host, ctx) {
    let args = ctx["with"];
    let p = plan(args);
    let mode = p["mode"];
    let query = p["query"];

    let out_dir = opt(args, "out_dir");
    if out_dir == "" { out_dir = "~/tmp/goofish"; }
    let items_dir = out_dir + "/items";
    let wpath = watch_path(out_dir);

    let count = p["count"];
    let pages = opt_int(args, "pages", 1);
    if pages < 1 { pages = 1; }
    // One page is 30 cards; asking for more rows than a page can hold is a
    // silent no-op, so grow `pages` with `count` instead of truncating to 30.
    let want_rows = count;
    let need_pages = (want_rows + 29) / 30;
    if need_pages > pages { pages = need_pages; }

    let browse = opt_int(args, "browse", 0);
    let budget = opt_int(args, "page_budget_ms", 40000);
    let hist_max = opt_int(args, "history_max", 200);
    if hist_max < 2 { hist_max = 2; }
    let drop_pct = opt_float(args, "drop_pct", 0.0);
    let price_min = opt_float(args, "price_min", 0.0);
    let price_max = opt_float(args, "price_max", 0.0);
    let keep_open = opt_bool(args, "keep_open", false);
    let watch_filter = opt(args, "watch_query");
    let watch_limit = opt_int(args, "watch_limit", 20);
    if watch_limit < 1 { watch_limit = 1; }
    let save_listing = opt_bool(args, "save_listing", true);
    let sel = opt(args, "selector");
    // Markdown + figures is the expensive part (it re-renders the SPA), so it
    // is on where a human asked about *one* thing and off for a listing sweep.
    let markdown = opt_bool(args, "markdown", mode == "item" || mode == "watch_add");
    let track = opt_bool(args, "track", mode != "search");

    let started = host.now();
    let source_url = "";
    if mode == "search" { source_url = search_url(host, query, p["preencoded"]); }

    let results = [];
    let failures = [];
    let changes = [];
    let wdata = load_watch(host, wpath, started);
    let initial_tracked = watch_items(wdata).len();
    let notes = [];

    // ── one product: browse it, record it, and remember its price ───────────
    if mode == "item" || mode == "watch_add" {
        let url = p["item_url"];
        if url == "" {
            throw "goofish: mode '" + mode + "' needs an item URL or a goofish item id"
                + " (got query '" + query + "')";
        }
        let rec = read_item(host, url, budget);
        if !omap(rec) || !rec.contains("ok") || !rec["ok"] {
            let why = "";
            if omap(rec) && rec.contains("error") { why = rec["error"]; }
            let st = "unknown";
            if omap(rec) && rec.contains("status") { st = rec["status"]; }
            // A sold or deleted item is an answer, not a failure: the caller
            // asked about one thing and the answer is "it is gone". Reporting it
            // as a failure buried the only interesting thing about the run, and
            // it also let a monitor keep quoting a price for something that no
            // longer exists.
            if st == "gone" {
                let now = host.now();
                let title = "";
                if omap(rec) && rec.contains("title") { title = ostr(rec["title"]); }
                let gone_rec = #{ id: p["item_id"], url: url, title: title, status: "gone" };
                if track {
                    let obs = observe(host, wdata, gone_rec, now, hist_max);
                    wdata = obs["data"];
                    changes.push(obs["change"]);
                }
                results.push(#{
                    rank: 1,
                    id: p["item_id"],
                    url: url,
                    title: title,
                    price: (),
                    price_text: "",
                    status: "gone",
                    json: "",
                    markdown: (),
                    tracked: track
                });
                let tail = "";
                if track {
                    tail = " — watch.json updated so the monitor stops quoting a price";
                }
                notes.push("item " + p["item_id"] + " is gone (" + why + ")" + tail);
                return finish(host, started, mode, query, source_url, out_dir, wpath,
                    wdata, initial_tracked, results, failures, changes, notes, 0, 1, 0, 0);
            }
            failures.push(#{ rank: 1, url: url, status: st, error: why });
            return finish(host, started, mode, query, source_url, out_dir, wpath,
                wdata, initial_tracked, results, failures, changes, notes, 0, 0, 0, 0);
        }
        let now = host.now();
        let rank = 1;
        let entry = item_record(rec, rank, url, mode, now);
        entry["_selector"] = sel;
        let wrote = ();
        if save_listing {
            wrote = host.write_file(
                items_dir + "/" + p["item_id"] + ".json",
                host.json_stringify(entry)
            );
        }
        let md = ();
        if markdown {
            md = save_markdown(host, url, out_dir, p["item_id"], entry, keep_open);
        }
        if track {
            let obs = observe(host, wdata, rec, now, hist_max);
            wdata = obs["data"];
            changes.push(obs["change"]);
        }
        results.push(#{
            rank: rank,
            id: entry["id"],
            url: entry["url"],
            title: entry["title"],
            price: entry["price"],
            price_text: entry["price_text"],
            status: entry["status"],
            json: if omap(wrote) { wrote["path"] } else { "" },
            markdown: md,
            tracked: track
        });
        return finish(host, started, mode, query, source_url, out_dir, wpath,
            wdata, initial_tracked, results, failures, changes, notes, 0, 0, 0, 0);
    }

    // ── price monitoring: re-read every tracked item, report what moved ─────
    if mode == "watch" {
        let items = watch_items(wdata);
        if items.len() == 0 {
            notes.push("watch.json is empty — nothing tracked yet. Add an item with"
                + " `mode: item` (or pass watch_add:<url>) first.");
            return finish(host, started, mode, query, source_url, out_dir, wpath,
                wdata, initial_tracked, results, failures, changes, notes, 0, 0, 0, 0);
        }

        let targets = [];
        let i = 0;
        while i < items.len() && targets.len() < watch_limit {
            let e = items[i];
            let keep = true;
            if watch_filter != "" {
                let title = pstr(e, "title");
                keep = title.contains(watch_filter);
            }
            if keep { targets.push(e); }
            i += 1;
        }
        if targets.len() < items.len() {
            notes.push("watch_limit " + watch_limit + " / watch_query '" + watch_filter
                + "' selected " + targets.len() + " of " + items.len() + " tracked items");
        }

        let checked = 0;
        let gone = 0;
        let dropped = 0;
        let raised = 0;
        let same = 0;
        let j = 0;
        while j < targets.len() {
            let e = targets[j];
            let url = pstr(e, "url");
            let id = pstr(e, "id");
            let rec = read_item(host, url, budget);
            if !omap(rec) || !rec.contains("ok") || !rec["ok"] {
                let st = "unknown";
                if omap(rec) && rec.contains("status") { st = rec["status"]; }
                // A sold item is an answer, not a failure: record it so the
                // watch file stops reporting a price for something that is gone.
                if st == "gone" {
                    let now = host.now();
                    let obs = observe(host, wdata, #{ id: id, url: url, status: "gone",
                        title: pstr(e, "title") }, now, hist_max);
                    wdata = obs["data"];
                    changes.push(#{
                        id: id, url: url, title: pstr(e, "title"),
                        direction: "gone", old_price: (), new_price: (), delta: (),
                        delta_pct: (), at: now["rfc3339"]
                    });
                    gone += 1;
                } else {
                    let why = "";
                    if omap(rec) && rec.contains("error") { why = rec["error"]; }
                    failures.push(#{ rank: j + 1, id: id, url: url, status: st, error: why });
                }
                j += 1;
                continue;
            }
            let now = host.now();
            let obs = observe(host, wdata, rec, now, hist_max);
            wdata = obs["data"];
            let ch = obs["change"];
            checked += 1;
            let d = ostr(ch["direction"]);
            if d == "down" { dropped += 1; }
            else if d == "up" { raised += 1; }
            else if d == "same" { same += 1; }
            // Only a move worth reporting clears the threshold; the history
            // still records every sighting either way.
            let pct = 0.0;
            if ch.contains("delta_pct") && type_of(ch["delta_pct"]) != "()" {
                pct = onum(ch["delta_pct"]);
            }
            let abs_pct = if pct < 0.0 { -pct } else { pct };
            if d == "new" || d == "gone" || d == "unknown" || abs_pct >= drop_pct {
                changes.push(ch);
            }
            results.push(#{
                rank: j + 1,
                id: id,
                url: url,
                title: ostr(ch["title"]),
                price: pick(rec, "price"),
                old_price: ch["old_price"],
                delta: ch["delta"],
                delta_pct: ch["delta_pct"],
                status: pstr2(rec, "status", "on_sale"),
                direction: d
            });
            j += 1;
        }
        return finish(host, started, mode, query, source_url, out_dir, wpath,
            wdata, initial_tracked, results, failures, changes, notes, checked, gone, 0, 0);
    }

    // ── search: list a page of cards, optionally open the first N in full ───
    if mode == "search" {
        if query == "" {
            throw "goofish: a search needs a keyword — pass query/paper-style text, e.g."
                + " '搜索 索尼 A7M4' or --query 'iphone 13 pro'";
        }
        let page = read_search(host, source_url, want_rows, pages, budget);
        if !omap(page) || !page.contains("ok") || !page["ok"] {
            let why = "search returned no cards";
            if omap(page) && page.contains("error") && ostr(page["error"]) != "" {
                why = ostr(page["error"]);
            }
            failures.push(#{ rank: 0, url: source_url, status: "empty", error: why });
            return finish(host, started, mode, query, source_url, out_dir, wpath,
                wdata, initial_tracked, results, failures, changes, notes, 0, 0, 0, 0);
        }

        let rows = [];
        if page.contains("rows") && type_of(page["rows"]) == "array" {
            for row in page["rows"] {
                if price_ok(row, price_min, price_max) { rows.push(row); }
            }
        }
        let dropped_by_price = 0;
        if omap(page) && page.contains("rows") && type_of(page["rows"]) == "array" {
            dropped_by_price = page["rows"].len() - rows.len();
        }
        if dropped_by_price > 0 {
            notes.push("price filter dropped " + dropped_by_price + " of "
                + page["rows"].len() + " cards (goofish ignores ?priceMin=/?priceMax=, so this is"
                + " applied to the fetched page, not pushed into the query)");
        }

        let visited = [];
        let seen = [];
        let k = 0;
        while k < rows.len() {
            let row = rows[k];
            let id = pstr(row, "id");
            if id == "" || seen.contains(id) { k += 1; continue; }
            seen.push(id);
            let url = pstr(row, "url");
            let rank = results.len() + 1;

            let rec = row;
            let opened = false;
            if browse > visited.len() {
                // `browse` turns the first N cards into full detail records:
                // the listing card has no description, attributes or seller.
                let detail = read_item(host, url, budget);
                if omap(detail) && detail.contains("ok") && detail["ok"] {
                    rec = detail;
                    opened = true;
                    visited.push(id);
                }
            }

            let now = host.now();
            let entry = item_record(rec, rank, source_url, mode, now);
            entry["_selector"] = sel;
            let wrote = ();
            if save_listing {
                wrote = host.write_file(items_dir + "/" + id + ".json",
                    host.json_stringify(entry));
            }
            let md = ();
            if markdown && opened {
                md = save_markdown(host, url, out_dir, id, entry, keep_open);
            }
            if track {
                let obs = observe(host, wdata, rec, now, hist_max);
                wdata = obs["data"];
                changes.push(obs["change"]);
            }
            results.push(#{
                rank: rank,
                id: entry["id"],
                url: entry["url"],
                title: entry["title"],
                price: entry["price"],
                price_text: entry["price_text"],
                status: entry["status"],
                opened: opened,
                want_count: entry["want_count"],
                views: entry["views"],
                location: entry["location"],
                json: if omap(wrote) { wrote["path"] } else { "" },
                markdown: md,
                tracked: track
            });
            k += 1;
        }

        if browse > visited.len() {
            notes.push("browse " + browse + " requested but only " + visited.len()
                + " item pages were opened");
        }
        let paged = 0;
        if omap(page) && page.contains("pages") && type_of(page["pages"]) == "array" {
            paged = page["pages"].len();
        }
        // checked_count means "re-read a tracked item and compared it", which
        // is what a watch run does; a listing just reads, so it reports 0.
        return finish(host, started, mode, query, source_url, out_dir, wpath,
            wdata, initial_tracked, results, failures, changes, notes, 0, 0,
            paged, dropped_by_price);
    }

    throw "goofish resolved mode '" + mode + "' is not runnable";
}

// Render the item's own Markdown sheet.
//
// Why the plugin writes this instead of handing the page to the generic
// article renderer: goofish's detail page is built almost entirely out of
// <span>s inside <div>s, and the renderer drops any div without block-level
// children. Verified on a live capture with the item container as the selector —
// 432 bytes of Markdown: ten pictures and the "为你推荐" heading, and not one
// character of the title, price, description or attributes.
//
// `capture` is whatever host.save_article returned. That call still runs first,
// because it is the thing that actually downloads the pictures; its `images`
// list maps each CDN url to the local file name, which is what lets the sheet
// reference the downloaded copies and fall back to the CDN only for the ones
// that failed.
// Two decimals, but without the ".0" that float formatting appends to a whole
// number: a ¥20500 camera must read "¥20500", not "¥20500.0".
//
// Built by trimming the rendered float rather than by splitting it into integer
// and fraction parts, because Rhai's `int()` is not a truncating cast in this
// engine build (it hands back a float, so the split produced "1.0.98.0").
fn fmt_num(v) {
    let r = (v * 100.0).round() / 100.0;
    let s = "" + r;
    if s.ends_with(".0") { s.sub_string(0, s.len() - 2) } else { s }
}

// The alicdn object key of an image url: everything between the last "/" and
// the first "_", e.g. ".../bao/uploaded/i2/2222253586975/O1CN01mids..._.webp"
// -> "O1CN01mids...".
//
// Matching has to be on this rather than on the whole url. The same picture is
// spelled two different ways on the two paths that meet here: the carousel
// carries the site's own rendition ("..._790x10000Q90.jpg_.webp") while
// save_article records the thumbnail it actually downloaded
// ("..._!!4611686018427386399-0-xy_item.jpg_Q90.jpg_.webp"). The suffix differs,
// the object key does not, so an exact url comparison matched one image out of
// seven and the other six fell back to remote links.
fn alicdn_key(url) {
    let s = "" + url;
    let start = 0;
    let i = 0;
    while i < s.len() {
        if s.sub_string(i, 1) == "/" { start = i + 1; }
        i += 1;
    }
    let head = s.sub_string(start, s.len() - start);
    let at = head.index_of("_");
    if at < 0 { head } else { head.sub_string(0, at) }
}

// Local file name for `url` out of a save_article image list, or "" when the
// download did not happen.
fn local_image(capture, url) {
    if !omap(capture) || !capture.contains("images") { return ""; }
    if type_of(capture["images"]) != "array" { return ""; }
    let key = alicdn_key(url);
    for img in capture["images"] {
        if omap(img) && img.contains("name") && pstr(img, "name") != "" {
            if pstr(img, "url") == url { return pstr(img, "name"); }
            if key != "" && alicdn_key(pstr(img, "url")) == key { return pstr(img, "name"); }
        }
    }
    ""
}

fn render_markdown(host, out_dir, id, entry, capture) {
    let images_dir = "images/" + id;
    let body = "";
    body += "# " + pstr(entry, "title") + "\n";
    body += "" + "\n";

    // --- the facts a buyer acts on -----------------------------------------
    let price = pick(entry, "price");
    let price_text = "";
    if type_of(price) == "f64" {
        let note = pstr(entry, "price_note");
        price_text = if note == "" { "¥" + fmt_num(price) } else { note + " ¥" + fmt_num(price) };
    } else {
        price_text = if pstr(entry, "price_text") != "" { pstr(entry, "price_text") } else { "价格待议" };
    }
    body += "| 项目 | 内容 |" + "\n";
    body += "| --- | --- |" + "\n";
    body += "| 价格 | " + price_text + " |" + "\n";
    let original = pick(entry["detail"], "original_price");
    if type_of(original) == "f64" {
        body += "| 原价 | ¥" + fmt_num(original) + " |" + "\n";
    }
    let discount = pstr(entry, "discount_text");
    if discount != "" { body += "| 优惠 | " + discount + " |" + "\n"; }
    let shipping = pstr(entry["detail"], "shipping");
    if shipping != "" { body += "| 运费 | " + shipping + " |" + "\n"; }
    body += "| 商品ID | " + id + " |" + "\n";
    body += "| 链接 | " + pstr(entry, "url") + " |" + "\n";
    let want = pick(entry, "want_count");
    if type_of(want) != "()" { body += "| 想要人数 | " + want + " |" + "\n"; }
    let views = pick(entry, "views");
    if type_of(views) != "()" { body += "| 浏览量 | " + views + " |" + "\n"; }
    if pstr(entry, "location") != "" { body += "| 所在地 | " + pstr(entry, "location") + " |" + "\n"; }
    body += "| 采集时间 | " + pstr(entry, "collected_at") + " |" + "\n";
    body += "" + "\n";

    // --- pictures ------------------------------------------------------------
    let imgs = pick(entry["detail"], "images");
    if type_of(imgs) == "array" && imgs.len() > 0 {
        body += "## 图片" + "\n";
        body += "" + "\n";
        for url in imgs {
            let name = local_image(capture, "" + url);
            if name != "" {
                body += "![" + id + "](" + images_dir + "/" + name + ")" + "\n";
            } else {
                body += "![" + id + "](" + url + ")" + "\n";
            }
        }
        body += "" + "\n";
    }

    // --- attributes ----------------------------------------------------------
    let labels = pick(entry["detail"], "labels");
    if omap(labels) && labels.len() > 0 {
        body += "## 商品属性" + "\n";
        body += "" + "\n";
        for key in labels.keys() {
            body += "- **" + key + "**: " + labels[key] + "\n";
        }
        body += "" + "\n";
    }

    // --- description ---------------------------------------------------------
    let description = pstr(entry["detail"], "description");
    if description != "" {
        body += "## 描述" + "\n";
        body += "" + "\n";
        body += description + "\n";
        body += "" + "\n";
    }

    // --- seller --------------------------------------------------------------
    let seller = pick(entry["detail"], "seller");
    if omap(seller) {
        let nick = pstr(seller, "nick");
        if nick != "" {
            body += "## 卖家" + "\n";
            body += "" + "\n";
            body += "- 昵称: " + nick + "\n";
            let loc = pstr(seller, "location");
            if loc != "" { body += "- 所在地: " + loc + "\n"; }
            let years = pick(seller, "joined_years");
            if type_of(years) != "()" { body += "- 闲鱼年限: " + years + " 年" + "\n"; }
            let sold = pick(seller, "sold_count");
            if type_of(sold) != "()" { body += "- 卖出宝贝: " + sold + " 件" + "\n"; }
            let credit = pick(seller, "credit_rate");
            if type_of(credit) != "()" { body += "- 好评率: " + fmt_num(credit) + "%" + "\n"; }
            let tags = pick(seller, "tags");
            if type_of(tags) == "array" && tags.len() > 0 {
                let flat = "";
                for t in tags {
                    if flat != "" { flat = flat + " / "; }
                    flat = flat + ("" + t);
                }
                body += "- 标签: " + flat + "\n";
            }
            body += "" + "\n";
        }
    }

    // --- search context ------------------------------------------------------
    let tags = pick(entry["listing"], "tags");
    if type_of(tags) == "array" && tags.len() > 0 {
        body += "## 搜索卡片标签" + "\n";
        body += "" + "\n";
        let flat = "";
        for t in tags {
            if flat != "" { flat = flat + " / "; }
            flat = flat + ("" + t);
        }
        body += flat + "\n";
        body += "" + "\n";
    }

    let wr = host.write_file(out_dir + "/item-" + id + ".md", body);
    if omap(wr) && wr.contains("path") { return "" + wr["path"]; }
    ""
}

// Capture the page (which downloads the pictures) and then write the sheet.
fn save_markdown(host, url, out_dir, id, entry, keep_open) {
    let leaf = #{
        url: url,
        // Without an explicit slug the engine derives one from the URL, which
        // for goofish is the whole "item?id=…&categoryId=…" query — stable, but
        // unreadable in a directory listing. "item-<id>" is short and sorts.
        slug: "item-" + id,
        out_dir: out_dir,
        image_dir: "images/" + id,
        stable_ms: 1200,
        max_wait_ms: 45000,
        keep_open: keep_open,
        // Scoping the renderer to the item container is what keeps the capture
        // from pulling in the header and the whole recommendation rail.
        selector: "[class^='item-container--'], [class^='item-main-container--']"
    };
    leaf["meta_extra"] = #{
        source: url,
        source_mode: "item",
        id: id,
        title: pstr(entry, "title"),
        price: pick(entry, "price"),
        collected_at: pstr(entry, "collected_at")
    };

    // One retry: a save that dies on a transient CDP error should not lose an
    // item the plugin has already read.
    let capture = #{};
    let attempts = 0;
    loop {
        attempts += 1;
        try {
            capture = host.save_article(leaf);
            break;
        } catch (e) {
            capture = #{};
            if attempts >= 2 { break; }
        }
    }
    render_markdown(host, out_dir, id, entry, capture);
}

// One exit for every mode, so the shape of a run's result never depends on
// which branch produced it.
fn finish(host, started, mode, query, source_url, out_dir, wpath, wdata,
          initial_tracked, results, failures, changes, notes, checked, gone,
          pages_visited, price_filtered) {
    let now = host.now();
    let items = watch_items(wdata);
    let tracked = items.len();
    let saved_watch = "";
    if tracked != initial_tracked || changes.len() > 0 || mode == "item" || mode == "watch_add" {
        let err = "";
        try {
            wdata["updated_at"] = now["rfc3339"];
            wdata["updated_at_ms"] = now["unix_ms"];
            let wr = save_watch(host, wpath, wdata);
            if omap(wr) && wr.contains("path") { saved_watch = "" + wr["path"]; }
        } catch (e) {
            saved_watch = "write failed: " + e;
        }
    }

    let drops = 0;
    let rises = 0;
    let news = 0;
    let onces = 0;
    let unknowns = 0;
    for c in changes {
        let d = pstr(c, "direction");
        if d == "down" { drops += 1; }
        else if d == "up" { rises += 1; }
        else if d == "new" { news += 1; }
        else if d == "same" { onces += 1; }
        else { unknowns += 1; }
    }

    #{
        mode: mode,
        query: query,
        source_url: source_url,
        started_at: started["rfc3339"],
        started_at_unix_ms: started["unix_ms"],
        finished_at: now["rfc3339"],
        out_dir: out_dir,
        watch_file: wpath,
        watch_saved: saved_watch,
        tracked_count: tracked,
        checked_count: checked,
        gone_count: gone,
        pages_visited: pages_visited,
        price_filtered_out: price_filtered,
        found_count: results.len(),
        failed_count: failures.len(),
        changes: changes,
        summary: #{
            added: news,
            down: drops,
            up: rises,
            same: onces,
            unknown: unknowns,
            gone: gone
        },
        notes: notes,
        results: results,
        failures: failures
    }
}
