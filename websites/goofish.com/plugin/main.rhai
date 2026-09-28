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

// True when the user is asking the plugin to *learn* rather than to look at a
// listing: to price an item by the tags it carries, or to fold today's
// vocabulary into the one it already knows.
//
// This sits above the watch intents on purpose. "价格进化" and "监控价格" are
// both about price, but one wants a model and the other wants a diff, and a
// request that names the model is asking for the model even if it also mentions
// monitoring.
fn is_evolve_intent(text) {
    let l = strip(text).to_lower();
    if l == "" { return false; }
    const WORDS_CN = ["价格进化", "标签进化", "价格模型", "标签库", "合理价格", "估价",
        "公平价", "学习标签", "沉淀标签", "进化一下", "价格学习"];
    for w in WORDS_CN { if l.contains(w) { return true; } }
    const WORDS_EN = ["evolve", "fair price", "price model", "learn tags", "price learning"];
    for w in WORDS_EN { if l.contains(w) { return true; } }
    false
}

fn plan(args) {
    let query = opt_any(args, ["query", "q", "keyword", "keywords", "text"]);
    let raw_item = opt_any(args, ["item_url", "url", "item", "id"]);
    let explicit = opt(args, "mode").to_lower();

    let mode = explicit;
    if mode == "" {
        if is_evolve_intent(query) { mode = "evolve"; }
        else if is_watch_add(query) { mode = "watch_add"; }
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
    // A learning run is only as good as its corpus, so "价格进化 索尼A7M4"
    // collects a wider sample than a plain search for the same phrase would;
    // a plain search keeps its own 20, and a listing is always just the one.
    if count <= 0 {
        count = if mode == "evolve" { 30 } else if mode == "search" { 20 } else { 1 };
    }

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
    // The last line of defence against a misread page. goofish abbreviates a
    // price with a 万/千 sitting beside it, and one stray character turns ¥9,180
    // into ¥91,800,000 - which then poisons the median, the high, the
    // vs_median_pct and every trend built on them, for good: a monitor is only
    // as trustworthy as the worst reading it ever accepted.
    // Once an item has a few real readings behind it, a price nowhere near its
    // own history is a parsing accident rather than a resale, so the sighting
    // is still recorded - without the number - and the change says why.
    let implausible = ();
    if has_price {
        let prior = [];
        for ph in history {
            let v = pick(ph, "price");
            if type_of(v) != "()" {
                let x = onum(v);
                if x > 0.0 { prior.push(x); }
            }
        }
        if prior.len() >= 3 {
            let med = median_of(prior);
            if med > 0.0 && (price > med * 5.0 || price < med / 5.0) { implausible = price; }
        }
    }
    let credible = has_price && implausible == ();
    if implausible != () {
        point["price"] = ();
        point["implausible_price"] = implausible;
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
    if credible { entry["price"] = price; }

    let direction = "unknown";
    let delta = ();
    let delta_pct = ();
    // Gone beats any price comparison: a delisted page carries no price, so the
    // old-number-vs-new-number branch below would say "unknown" for the single
    // most important change there is.
    if status == "gone" {
        direction = "gone";
        entry["price"] = ();
    } else if credible && type_of(old_price) != "()" {
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
        new_price: if credible { price } else { () },
        delta: delta,
        delta_pct: delta_pct,
        implausible_price: implausible,
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

// ══ self-evolution: prices and tags ═════════════════════════════════════════
//
// Everything above treats a listing as a fact to record. This section is the
// part that gets *better* at reading them: it mines the tags people actually
// write, folds the spellings that mean one thing into one vocabulary, and turns
// the price history into a fair price per tag so the next run can say "this is
// a good deal" instead of only "this one is ¥9800".
//
// Three files under `out_dir`, all of them derived from the first:
//   index.json       one row per item ever collected, with the tags mined from
//                    it — the corpus. Every statistic below is re-derived from
//                    it, so running `evolve` twice changes nothing.
//   tags.json        the vocabulary: canonical tag, spellings merged in, raw
//                    forms, co-occurrence, and the synonym pairs that justify
//                    the merges.
//   price_model.json per-tag price distribution (median ± a robust spread), the
//                    per-item price history summary, and the learned
//                    card-vs-detail correction factor.
//
// Only *accumulated* state grows across runs — spelling counts, synonym
// evidence, correction ratios, the run counter. Statistics are recomputed every
// time, because folding runs into statistics is how a counter ends up counting
// the same listing twice and pricing a camera by how often it was looked at.
// ── the evolution layer: a tag vocabulary and a price model that learn ───────
//
// "The tags and the prices evolve on their own" is three JSON stores under
// out_dir, so all of it works with no db capability, no extra process and no
// network:
//
//   index.json       one row per item ever collected, together with the tags
//                    mined from it. This is the corpus, and every statistic
//                    below is *derived* from it.
//   tags.json        the tag vocabulary: canonical tag -> how many items carry
//                    it, which spellings merge into it, what it co-occurs
//                    with, and which synonym pairs earlier runs confirmed.
//   price_model.json a per-tag price distribution (median + MAD, so one 999-yuan
//                    typo cannot move "the price of a Sony A7M4"), a per-item
//                    price summary, and the correction factor the plugin has
//                    learned for its own card-vs-detail price reads.
//
// The split between accumulated and derived is the load-bearing part. Only what
// cannot be re-derived grows: spelling counts, synonym evidence, correction
// ratios, the run counter. Every statistic is recomputed from index.json on
// each run. Folding each run's numbers into the statistics instead would
// double-count every item the second time `evolve` ran — which is the shape of
// "learning" that inflates a model until it means nothing.

fn tag_seps() {
    ["/", "|", ",", "，", "、", ";", "；", "+", "＋"]
}

// Split on any of a set of single characters. The engine registers no regex
// literal and no splitter taking a character class, and these are the
// separators goofish itself uses in tags and label values.
fn split_multi(s, seps) {
    let out = [];
    let cur = "";
    let i = 0;
    while i < s.len() {
        let c = s.sub_string(i, 1);
        if seps.contains(c) { out.push(cur); cur = ""; } else { cur += c; }
        i += 1;
    }
    out.push(cur);
    out
}

// Decoration around a tag rather than part of it: the 【】 the shop puts around
// "【免押】", brackets and quotes a seller types, bullets.
fn is_mark(c) {
    if c == "【" || c == "】" || c == "[" || c == "]" { return true; }
    if c == "(" || c == ")" || c == "（" || c == "）" { return true; }
    if c == "《" || c == "》" || c == "「" || c == "」" { return true; }
    if c == "<" || c == ">" || c == "*" || c == "~" || c == "^" || c == "#" { return true; }
    if c == "\"" || c == "'" || c == "“" || c == "”" || c == "‘" || c == "’" { return true; }
    false
}

fn ascii_ch(c) {
    const A = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 -_./+:()%|";
    A.contains(c)
}

// Latin script only. This is the whole of the script test below, and it is
// enough: the question it answers is never "which CJK block", only "is this
// written in the same alphabet as the other half of this pair".
fn is_latin(s) {
    let i = 0;
    while i < s.len() {
        if !ascii_ch(s.sub_string(i, 1)) { return false; }
        i += 1;
    }
    true
}

// Letters and digits only, so "iPhone 13" and "iphone-13" compare equal.
fn squash(s) {
    let out = "";
    let i = 0;
    while i < s.len() {
        let low = s.sub_string(i, 1).to_lower();
        if "abcdefghijklmnopqrstuvwxyz0123456789".contains(low) { out += low; }
        i += 1;
    }
    out
}

// Canonical spelling of one tag: decoration removed, spaces collapsed, case
// folded, length capped so one pathological label cannot become the vocabulary.
fn tag_norm(s) {
    let out = "";
    let i = 0;
    while i < s.len() {
        let c = s.sub_string(i, 1);
        if is_space(c) || is_mark(c) {
            if out != "" && !out.ends_with(" ") { out += " "; }
        } else {
            out += c;
        }
        i += 1;
    }
    let t = strip(out).to_lower();
    if t.len() > 24 { t = t.sub_string(0, 24); }
    t
}

// Reject the one shape that carries no meaning: a bare number. ("128GB" and
// "a7m4" survive, "11" and "97" do not.)
fn tag_ok(t) {
    if t == "" || t.len() > 24 { return false; }
    const NUM = "0123456789 .-+%/";
    let i = 0;
    while i < t.len() {
        if !NUM.contains(t.sub_string(i, 1)) { return true; }
        i += 1;
    }
    false
}

fn ns_of_key(k) {
    let p = k.index_of(":");
    if p <= 0 { "" } else { k.sub_string(0, p) }
}

fn tag_of_key(k) {
    let p = k.index_of(":");
    if p <= 0 { k } else { k.sub_string(p + 1) }
}

fn tag_key(ns, tag) {
    if ns == "" { tag } else { ns + ":" + tag }
}

// Unordered pair, one spelling. String ordering works in this engine build, so
// this is a real canonical form rather than a guess.
fn pair_key(a, b) {
    if a < b { a + "|" + b } else { b + "|" + a }
}

// Are these two spellings the same thing?
//
// This is the only heuristic in the vocabulary, and it is deliberately
// conservative: a wrong merge destroys information that cannot be recovered,
// while a missed merge only costs one extra row in the vocabulary.
//
//   * one spelling contains the other ("索尼" inside "索尼 a7m4")
//   * the halves are written in different scripts — the case that matters in
//     practice, because goofish writes the brand as "Sony/索尼" and without this
//     rule every brand would live in the vocabulary twice
//   * same letters once punctuation and spacing are ignored
//
// It deliberately does not merge on a shared prefix, which would have merged
// 全新 with 几乎全新 and A7M4 with A7M3.
fn syn_ok(a, b) {
    if a == b { return false; }
    if b.len() >= 2 && a.contains(b) { return true; }
    if a.len() >= 2 && b.contains(a) { return true; }
    if is_latin(a) != is_latin(b) { return true; }
    let ka = squash(a);
    ka != "" && ka == squash(b)
}

// ── deterministic ordering ──────────────────────────────────────────────────
//
// A map's key order is an implementation detail, and these files are read by
// humans and diffed, so every list that comes out of the learning pass is put
// into a fixed order: by key ascending, then by count descending.

fn has_k(rows, k) {
    for r in rows { if ("" + r["k"]) == ("" + k) { return true; } }
    false
}

fn order_by_key(rows) {
    let keys = [];
    for r in rows { keys.push("" + r["k"]); }
    keys.sort();
    let out = [];
    let i = 0;
    while i < keys.len() {
        for r in rows {
            if ("" + r["k"]) == keys[i] { out.push(r); break; }
        }
        i += 1;
    }
    out
}

// [{k, n}] -> the same rows, biggest n first, ties by ascending key.
// A sorted copy, always an array. `Array::sort` sorts in place and returns
// unit in this engine build, so `xs.sort()` in an expression is a null waiting
// to happen; this is the form that always yields the sorted array.
fn sorted_strings(xs) {
    xs.sort();
    xs
}

fn rank_top(rows, limit) {
    let base = order_by_key(rows);
    let out = [];
    while out.len() < limit && out.len() < base.len() {
        let bi = -1;
        let i = 0;
        while i < base.len() {
            if !has_k(out, base[i]["k"])
                && (bi < 0 || oint(base[i]["n"], 0) > oint(base[bi]["n"], 0)) { bi = i; }
            i += 1;
        }
        if bi < 0 { break; }
        out.push(base[bi]);
    }
    out
}

// Docs carry no "k", so they are ordered by one of their own string fields and
// then pulled back out in the order rank_top chose.
fn order_docs(docs, field) {
    let keys = [];
    for d in docs { keys.push("" + d[field]); }
    keys.sort();
    let out = [];
    let i = 0;
    while i < keys.len() {
        for d in docs {
            if ("" + d[field]) == keys[i] { out.push(d); break; }
        }
        i += 1;
    }
    out
}

fn pull_docs(ranked, docs, field) {
    let out = [];
    for r in ranked {
        for d in docs {
            if ("" + d[field]) == ("" + r["k"]) { out.push(d); break; }
        }
    }
    out
}

// ── small statistics ────────────────────────────────────────────────────────

fn sorted_copy(xs) {
    let o = [];
    for v in xs { o.push(onum(v)); }
    o.sort();
    o
}

fn median_of(xs) {
    if xs.len() == 0 { return (); }
    let s = sorted_copy(xs);
    let n = s.len();
    if n % 2 == 1 { return s[n / 2]; }
    (s[n / 2 - 1] + s[n / 2]) / 2.0
}

fn quantile_of(xs, q) {
    if xs.len() == 0 { return (); }
    let s = sorted_copy(xs);
    let pos = q * (s.len() - 1);
    let lo = int_of(pos);
    if lo >= s.len() - 1 { return s[s.len() - 1]; }
    s[lo] + (s[lo + 1] - s[lo]) * (pos - lo)
}

// Median absolute deviation: the spread one outlier cannot move, which is the
// whole reason the price model is not a mean.
fn mad_of(xs, med) {
    if xs.len() == 0 { return (); }
    let dev = [];
    for v in xs { dev.push((onum(v) - onum(med)).abs()); }
    median_of(dev)
}

fn round2(v) {
    (onum(v) * 100.0).round() / 100.0
}

// ── the three stores ────────────────────────────────────────────────────────

fn index_path(out_dir) { out_dir + "/index.json" }
fn tags_path(out_dir) { out_dir + "/tags.json" }
fn model_path(out_dir) { out_dir + "/price_model.json" }

// A missing file is the normal first-run case, and a corrupt one is downgraded
// to the default rather than failing the run: a rebuilt vocabulary is better
// than no collection at all. (Same reasoning as the watch store.)
fn read_json_or(host, path, dflt) {
    let read = host.read_file(path);
    let exists = false;
    if omap(read) && read.contains("exists") && read["exists"] { exists = true; }
    if !exists { return dflt; }
    let text = "";
    if omap(read) && read.contains("text") && type_of(read["text"]) == "string" { text = read["text"]; }
    if strip(text) == "" { return dflt; }
    let parsed = ();
    try { parsed = host.json_parse(text); }
    catch (e) { return dflt; }
    parsed
}

fn write_json(host, path, doc) {
    host.write_file(path, host.json_stringify(doc))
}

fn load_index(host, out_dir) {
    let doc = read_json_or(host, index_path(out_dir), #{});
    if omap(doc) && doc.contains("items") && type_of(doc["items"]) == "array" { doc }
    else { #{ version: 1, items: [] } }
}

fn index_rows(idx) {
    if omap(idx) && idx.contains("items") && type_of(idx["items"]) == "array" { idx["items"] }
    else { [] }
}

fn empty_learned(now) {
    #{
        version: 1,
        runs: 0,
        synonyms: [],
        corrections: #{},
        last_run_at: "",
        last_run_at_ms: now["unix_ms"]
    }
}

// The only state that accumulates between runs. Vocabulary counts and every
// price statistic are re-derived; this is the part that cannot be.
fn load_learned(host, out_dir, now) {
    let base = empty_learned(now);
    let tags_doc = read_json_or(host, tags_path(out_dir), #{});
    if omap(tags_doc) {
        if tags_doc.contains("synonyms") && type_of(tags_doc["synonyms"]) == "array" {
            base["synonyms"] = tags_doc["synonyms"];
        }
        if tags_doc.contains("runs") { base["runs"] = oint(tags_doc["runs"], 0); }
        if tags_doc.contains("last_run_at") { base["last_run_at"] = pstr2(tags_doc, "last_run_at", ""); }
        if tags_doc.contains("last_run_at_ms") { base["last_run_at_ms"] = oint(tags_doc["last_run_at_ms"], 0); }
    }
    let mdl = read_json_or(host, model_path(out_dir), #{});
    if omap(mdl) && mdl.contains("corrections") && omap(mdl["corrections"]) {
        base["corrections"] = mdl["corrections"];
    }
    base
}

// One corpus row: an item plus the tags mined from it.
fn index_row(entry, mined, now) {
    let tags = [];
    for t in mined["tags"] {
        tags.push(#{ ns: t["ns"], tag: t["tag"], raw: t["raw"] });
    }
    let syns = [];
    for s in mined["syns"] {
        syns.push(#{ key: s["key"], a: s["a"], b: s["b"], ns: s["ns"], raw: s["raw"] });
    }
    #{
        id: pstr(entry, "id"),
        title: pstr(entry, "title"),
        url: pstr(entry, "url"),
        status: pstr2(entry, "status", "on_sale"),
        location: pstr(entry, "location"),
        price: pick(entry, "price"),
        want_count: pick(entry, "want_count"),
        source_mode: pstr(entry, "source_mode"),
        collected_at: pstr2(entry, "collected_at", now["rfc3339"]),
        collected_at_ms: oint(pick(entry, "collected_at_ms"), 0),
        tags: tags,
        syns: syns
    }
}

// Fold this run's rows into the index: newest wins, oldest out once `index_max`
// is reached. The index is what a later run re-derives from, so it — rather
// than any accumulated number — is what keeps a second `evolve` honest.
fn merge_index(idx, fresh, index_max) {
    let seen = [];
    let rows = [];
    for r in fresh {
        let id = pstr(r, "id");
        if id == "" || seen.contains(id) { continue; }
        seen.push(id);
        rows.push(r);
    }
    let kept = [];
    let old = index_rows(idx);
    let i = 0;
    while i < old.len() {
        let r = old[i];
        let id = pstr(r, "id");
        if id != "" && !seen.contains(id) { kept.push(r); }
        i += 1;
    }
    let merged = [];
    for r in kept { merged.push(r); }
    for r in rows { merged.push(r); }
    while merged.len() > index_max { merged.remove(0); }
    merged
}

// The tags an item record carries, plus the synonym pairs its own label values
// declare. A value with several parts ("Sony/索尼" under 品牌) is the seller
// saying those parts name one thing, so each part becomes a tag and each pair
// becomes synonym evidence.
fn mine_tags(entry) {
    let tags = [];
    let syns = [];
    if !omap(entry) { return #{ tags: tags, syns: syns }; }
    let seps = tag_seps();

    if entry.contains("listing") {
        let l = entry["listing"];
        if omap(l) && l.contains("tags") && type_of(l["tags"]) == "array" {
            for raw in l["tags"] {
                let s = ostr(raw);
                if s == "" { continue; }
                for part in split_multi(s, seps) {
                    let t = tag_norm(part);
                    if tag_ok(t) { tags.push(#{ ns: "", tag: t, raw: strip(part) }); }
                }
            }
        }
    }

    if entry.contains("detail") {
        let d = entry["detail"];
        if omap(d) && d.contains("labels") && omap(d["labels"]) {
            let labels = d["labels"];
            let keys = labels.keys();
            keys.sort();
            for kk in keys {
                let k = ostr(kk);
                let ns = tag_norm(k);
                if ns == "" { continue; }
                let v = ostr(labels[k]);
                if v == "" { continue; }
                let parts = [];
                for part in split_multi(v, seps) {
                    let t = tag_norm(part);
                    if tag_ok(t) { parts.push(#{ ns: ns, tag: t, raw: strip(part) }); }
                }
                for p in parts { tags.push(p); }
                let i = 0;
                while i < parts.len() {
                    let j = i + 1;
                    while j < parts.len() {
                        if syn_ok(parts[i]["tag"], parts[j]["tag"]) {
                            syns.push(#{
                                key: pair_key(parts[i]["tag"], parts[j]["tag"]),
                                a: parts[i]["tag"],
                                b: parts[j]["tag"],
                                ns: ns,
                                raw: v
                            });
                        }
                        j += 1;
                    }
                    i += 1;
                }
            }
        }
    }
    // detail.seller.tags is deliberately not mined: those badges describe the
    // *seller* ("来闲鱼5年", "好评率100%"), so folding them into the item
    // vocabulary would end up pricing a camera by how long its owner has been
    // a member.
    #{ tags: tags, syns: syns }
}

// ── the correction factor ───────────────────────────────────────────────────
//
// The one thing the plugin learns about its own reading. goofish's search card
// and its detail page disagree on price often enough to matter — an
// abbreviated "1.38万", a 券后价 that the card shows and the detail does not —
// and when they disagree *consistently*, the ratio is a measurement error this
// plugin can correct on the next run instead of carrying forever.
//
// Four gates, all of which must pass, because a wrong factor would rewrite
// every price in the store:
//   * enough independent sightings (`feed_min_hits`)
//   * a tight distribution — the 25th and 75th percentile must be within
//     `feed_max_spread` of each other, so a mix of unrelated reasons to differ
//     never averages into one factor
//   * a plausible magnitude (0.5x .. 2x): beyond that it is a different price,
//     not a wrong unit
//   * and only ever applied to a card price that was never confirmed by a
//     detail page.
fn feed_factor_of(corrections, o) {
    if !omap(corrections) || !corrections.contains("feed_detail") { return 0.0; }
    let c = corrections["feed_detail"];
    if !omap(c) { return 0.0; }
    if oint(pick(c, "n"), 0) < oint(pick(o, "feed_min_hits"), 5) { return 0.0; }
    let spread = onum(pick(c, "spread"));
    if spread <= 0.0 || spread > onum(pick(o, "feed_max_spread")) { return 0.0; }
    let f = onum(pick(c, "factor"));
    if f < 0.5 || f > 2.0 { return 0.0; }
    // A factor within 2% of 1 is noise; applying it would only add a step
    // between the number on the page and the number in the store.
    if (f - 1.0).abs() < 0.02 { return 0.0; }
    f
}

// The whole learning pass: corpus in, documents out.
fn learn(host, out_dir, corpus, wdata, learned, fresh, feed_pairs, now, o) {
    let syn_min = oint(pick(o, "syn_min_hits"), 2);
    let min_samples = oint(pick(o, "min_samples"), 3);
    let sigma = onum(pick(o, "sigma"));
    let vocab_max = oint(pick(o, "vocab_max"), 300);
    let price_cap = oint(pick(o, "model_prices"), 120);

    // ── 1. how often each spelling is seen, and which pairs mean one thing ──
    let sightings = #{};   // tag key -> items carrying that spelling
    let raws = #{};        // tag key -> {raw spelling on the page: count}
    let syn_hits = #{};    // pair key -> {a, b, ns, n, first_seen, last_seen}
    let item_tags = [];    // per corpus row: the tag keys it carries

    for row in corpus {
        let keys = [];
        if omap(row) && row.contains("tags") && type_of(row["tags"]) == "array" {
            for t in row["tags"] {
                let tag = pstr(t, "tag");
                if !tag_ok(tag) { continue; }
                let k = tag_key(pstr2(t, "ns", ""), tag);
                if !keys.contains(k) { keys.push(k); }
                if !sightings.contains(k) { sightings[k] = 0; }
                sightings[k] = oint(sightings[k], 0) + 1;
                let rk = ostr(pick(t, "raw"));
                if rk != "" {
                    let bucket = if raws.contains(k) { raws[k] } else { #{} };
                    bucket[rk] = oint(bucket[rk], 0) + 1;
                    raws[k] = bucket;
                }
            }
        }
        if omap(row) && row.contains("syns") && type_of(row["syns"]) == "array" {
            for s in row["syns"] {
                let key = pstr(s, "key");
                if key == "" { continue; }
                let rec = if syn_hits.contains(key) { syn_hits[key] } else {
                    #{ a: pstr(s, "a"), b: pstr(s, "b"), ns: pstr2(s, "ns", ""), n: 0,
                       first_seen: now["rfc3339"], last_seen: now["rfc3339"] } };
                rec["n"] = oint(rec["n"], 0) + 1;
                rec["last_seen"] = now["rfc3339"];
                syn_hits[key] = rec;
            }
        }
        item_tags.push(#{ id: pstr(row, "id"), keys: keys });
    }

    // ── 2. the synonym graph: this corpus's declarations plus every pair an
    //       earlier run confirmed, unioned into one group per real thing ──
    let parent = #{};
    let syn_rows = [];
    let skeys = syn_hits.keys();
    skeys.sort();
    for kk in skeys {
        let key = ostr(kk);
        let rec = syn_hits[key];
        // A pair seen once is a coincidence: "128GB" and "256GB" in the same
        // listing say nothing. Two independent listings do.
        if oint(rec["n"], 0) < syn_min { continue; }
        let ns = pstr2(rec, "ns", "");
        uf_union(parent, tag_key(ns, pstr(rec, "a")), tag_key(ns, pstr(rec, "b")));
        syn_rows.push(#{ key: key, a: pstr(rec, "a"), b: pstr(rec, "b"), ns: ns,
                         n: oint(rec["n"], 0), first_seen: pstr(rec, "first_seen"),
                         last_seen: pstr(rec, "last_seen") });
    }
    syn_rows = order_docs(syn_rows, "key");

    // ── 3. one canonical name per group: the spelling the most items use ──
    let gns = #{};
    let gvars = #{};
    let canon_of = #{};
    let group_of = #{};
    let sk = sightings.keys();
    sk.sort();
    for kk in sk {
        let k = ostr(kk);
        let root = uf_find(parent, k);
        if !canon_of.contains(root) {
            let bucket = #{};   // spelling -> items
            let ns = ns_of_key(k);
            let tag = tag_of_key(k);
            bucket[tag] = 0;
            gvars[root] = bucket;
            gns[root] = ns;
        }
        let bucket = gvars[root];
        let tag = tag_of_key(k);
        bucket[tag] = oint(bucket[tag], 0) + oint(sightings[k], 0);
        gvars[root] = bucket;
    }
    for kk in sk {
        let k = ostr(kk);
        let root = uf_find(parent, k);
        if !canon_of.contains(root) {
            let names = [];
            let bucket = gvars[root];
            for nk in bucket.keys() { names.push(#{ k: ostr(nk), n: oint(bucket[nk], 0) }); }
            let ranked = rank_top(names, 1);
            let canon = if ranked.len() > 0 { ranked[0]["k"] } else { tag_of_key(k) };
            canon_of[root] = canon;
            gns[root] = ns_of_key(k);
        }
        group_of[k] = tag_key(pstr2(gns, root, ""), ostr(canon_of[root]));
    }

    // group -> the spellings that folded into it (co-occurrence and raw forms)
    let members = #{};
    for kk in sk {
        let k = ostr(kk);
        let g = ostr(group_of[k]);
        let list = if members.contains(g) { members[g] } else { [] };
        if !list.contains(k) { list.push(k); }
        members[g] = list;
    }

    // The priced vocabulary is bounded: the groups with the most items get
    // price statistics, the tail is still listed but never used to judge a
    // price. Everything stays linear in the corpus.
    let gcount = [];
    for kk in gns.keys() { gcount.push(#{ k: ostr(kk), n: 0 }); }
    let gkeys = [];
    for kk in group_of.keys() {
        let g = ostr(group_of[ostr(kk)]);
        let found = false;
        for e in gkeys { if e["k"] == g { found = true; break; } }
        if !found { gkeys.push(#{ k: g, n: oint(sightings[ostr(kk)], 0) }); }
    }
    let priced_groups = rank_top(gkeys, vocab_max);

    // ── 4. per-group counts, first/last seen, co-occurrence, sample titles ──
    let rows_by_id = #{};
    for row in corpus { rows_by_id[pstr(row, "id")] = row; }
    let item_count = #{};
    let price_of = #{};
    let first_seen = #{};
    let last_seen = #{};
    let samples = #{};
    let co = #{};
    for g in priced_groups {
        item_count[g["k"]] = 0;
        price_of[g["k"]] = [];
        co[g["k"]] = #{};
    }
    for ig in item_tags {
        let id = pstr(ig, "id");
        let row = if rows_by_id.contains(id) { rows_by_id[id] } else { #{} };
        let title = pstr(row, "title");
        let price = pick(row, "price");
        let at = oint(pick(row, "collected_at_ms"), 0);
        let keys = ig["keys"];
        for kk in keys {
            let g = if group_of.contains(ostr(kk)) { ostr(group_of[ostr(kk)]) } else { ostr(kk) };
            if !item_count.contains(g) { continue; }   // outside the priced vocabulary
            item_count[g] = oint(item_count[g], 0) + 1;
            if at > 0 {
                if !first_seen.contains(g) || at < oint(first_seen[g], 0) { first_seen[g] = at; }
                if !last_seen.contains(g) || at > oint(last_seen[g], 0) { last_seen[g] = at; }
            }
            if title != "" {
                let ss = if samples.contains(g) { samples[g] } else { [] };
                if ss.len() < 3 && !ss.contains(title) { ss.push(title); }
                samples[g] = ss;
            }
            if type_of(price) != "()" {
                let p = onum(price);
                if p > 0.0 {
                    let ps = price_of[g];
                    ps.push(p);
                    price_of[g] = ps;
                }
            }
            // Co-occurrence is counted once per item, so a tag repeated inside
            // one listing cannot inflate the pair.
            for k2 in keys {
                let g2 = if group_of.contains(ostr(k2)) { ostr(group_of[ostr(k2)]) } else { ostr(k2) };
                if g2 == g || !item_count.contains(g2) { continue; }
                let bucket = co[g];
                bucket[g2] = oint(bucket[g2], 0) + 1;
                co[g] = bucket;
            }
        }
    }

    // ── 5. the price model: one robust distribution per group ──
    let tag_stats = [];
    let stat_of = #{};
    for g in priced_groups {
        let key = g["k"];
        let ps = price_of[key];
        let n = ps.len();
        let stats = #{ n: 0, min: (), max: (), mean: (), median: (), mad: (),
                       fair: (), low: (), high: (), prices: [] };
        if n > 0 {
            let lo = 1e30; let hi = -1e30; let sum = 0.0;
            for p in ps {
                if p < lo { lo = p; }
                if p > hi { hi = p; }
                sum += p;
            }
            let med = median_of(ps);
            let m = mad_of(ps, med);
            stats["min"] = round2(lo);
            stats["max"] = round2(hi);
            stats["mean"] = round2(sum / n);
            stats["median"] = round2(med);
            stats["mad"] = round2(m);
            stats["n"] = n;
            if n >= min_samples {
                // median ± sigma · 1.4826 · MAD, the usual consistency factor
                // for a median/MAD pair. A MAD of zero (every listing the same
                // price) falls back to a 10% band so one different listing is
                // still visible.
                let spread = if m > 0.0 { sigma * 1.4826 * m } else { med * 0.1 };
                stats["fair"] = round2(med);
                stats["low"] = round2(med - spread);
                stats["high"] = round2(med + spread);
            }
            let keep = [];
            for p in sorted_copy(ps) { keep.push(round2(p)); }
            while keep.len() > price_cap { keep.remove(0); }
            stats["prices"] = keep;
        }
        stat_of[key] = stats;
        tag_stats.push(#{
            tag: key,
            ns: ns_of_key(key),
            items: oint(item_count[key], 0),
            n_priced: n,
            min: stats["min"], max: stats["max"], mean: stats["mean"],
            median: stats["median"], mad: stats["mad"],
            fair: stats["fair"], low: stats["low"], high: stats["high"],
            prices: stats["prices"]
        });
    }

    // ── 6. per-item price history, from the watch store ──
    let item_stats = [];
    let istat_of = #{};
    for w in watch_items(wdata) {
        let id = pstr(w, "id");
        let ps = [];
        let at = 0;
        let seen_at = -1;
        let seen_p = 0.0;
        let st = "on_sale";
        if omap(w) && w.contains("history") && type_of(w["history"]) == "array" {
            for h in w["history"] {
                let v = 0.0;
                let p = pick(h, "price");
                if type_of(p) != "()" { v = onum(p); }
                if v > 0.0 { ps.push(v); }
                let m = oint(pick(h, "at_ms"), 0);
                if m > at { at = m; }
                // The latest *sighting*, not the highest price. These were both
                // `sorted.last`, which only looked right while every price in
                // the history was identical: the moment one reading differed,
                // `last` became the peak of the whole series and `high` a copy
                // of it, so a monitor reported an item that had come down as
                // still trading at its high.
                if v > 0.0 && m >= seen_at { seen_at = m; seen_p = v; }
                st = pstr2(h, "status", st);
            }
        }
        let n = ps.len();
        let s = #{ n: n, last: (), low: (), high: (), median: (), mad: (), vs_median_pct: () };
        if n > 0 {
            let sorted = sorted_copy(ps);
            let med = median_of(ps);
            s["last"] = round2(seen_p);
            s["low"] = round2(sorted[0]);
            s["high"] = round2(sorted[sorted.len() - 1]);
            s["median"] = round2(med);
            s["mad"] = round2(mad_of(ps, med));
            if med > 0.0 && n >= 2 { s["vs_median_pct"] = round2((seen_p - med) / med * 100.0); }
        }
        istat_of[id] = s;
        item_stats.push(#{
            id: id, title: pstr(w, "title"), status: st,
            n: s["n"], last: s["last"], low: s["low"], high: s["high"],
            median: s["median"], mad: s["mad"], vs_median_pct: s["vs_median_pct"],
            last_seen_ms: at
        });
    }
    item_stats = order_docs(item_stats, "id");

    // ── 7. what this run's card/detail pairs say about our own readings ──
    let corrections = if omap(learned) && learned.contains("corrections") && omap(learned["corrections"])
                      { learned["corrections"] } else { #{} };
    let ckey = "feed_detail";
    let prev = if corrections.contains(ckey) && omap(corrections[ckey]) { corrections[ckey] } else {
        #{ key: ckey, n: 0, ratios: [], first_seen: now["rfc3339"], last_seen: now["rfc3339"] } };
    if !prev.contains("first_seen") || pstr2(prev, "first_seen", "") == "" {
        prev["first_seen"] = now["rfc3339"];
    }
    let ratios = if omap(prev) && prev.contains("ratios") && type_of(prev["ratios"]) == "array"
                 { prev["ratios"] } else { [] };
    let learned_now = 0;
    for fp in feed_pairs {
        let f = onum(pick(fp, "feed"));
        let d = onum(pick(fp, "detail"));
        // Only a magnitude-style discrepancy is learnable. A 券后价 or a
        // 价格面议 is a *different* price, and averaging the two kinds into one
        // factor is the exact mistake this is meant to prevent.
        if f <= 0.0 || d <= 0.0 { continue; }
        let r = d / f;
        if r < 0.5 || r > 2.0 { continue; }
        ratios.push(round2(r * 10000.0) / 10000.0);
        learned_now += 1;
    }
    while ratios.len() > price_cap { ratios.remove(0); }
    if learned_now > 0 { prev["last_seen"] = now["rfc3339"]; }
    prev["n"] = oint(prev["n"], 0) + learned_now;
    prev["ratios"] = ratios;
    if ratios.len() > 0 {
        let p25 = quantile_of(ratios, 0.25);
        let p75 = quantile_of(ratios, 0.75);
        prev["factor"] = round2(median_of(ratios));
        prev["p25"] = round2(p25);
        prev["p75"] = round2(p75);
        prev["spread"] = if p25 > 0.0 { round2(p75 / p25) } else { 0.0 };
    } else {
        prev["factor"] = ();
        prev["spread"] = 0.0;
    }
    corrections[ckey] = prev;

    // ── 8. anomalies: the whole corpus judged against what it now knows ──
    // The corpus, not this run's rows: `evolve` with no query browses nothing
    // and still has to answer "which of these is a deal?", which is a question
    // about everything collected so far rather than about the last fetch.
    let out_rows = [];
    let verdict_rows = [];
    for row in corpus {
        let price = pick(row, "price");
        if type_of(price) == "()" { continue; }
        let p = onum(price);
        if p <= 0.0 { continue; }
        // The best-supported group this item belongs to is the one that gets to
        // call it a bargain or a rip-off.
        let best = ();
        if omap(row) && row.contains("tags") && type_of(row["tags"]) == "array" {
            for t in row["tags"] {
                let k = tag_key(pstr2(t, "ns", ""), pstr(t, "tag"));
                if !group_of.contains(k) { continue; }
                let g = ostr(group_of[k]);
                if !stat_of.contains(g) { continue; }
                let st = stat_of[g];
                if type_of(pick(st, "fair")) == "()" { continue; }
                if best == () || oint(pick(st, "n"), 0) > oint(pick(best, "n"), 0) {
                    let copy = #{}; let f = st.keys();
                    for kk in f { copy[kk] = st[kk]; }
                    copy["tag"] = g;
                    best = copy;
                }
            }
        }
        if best == () { continue; }
        let fair = onum(pick(best, "fair"));
        if fair <= 0.0 { continue; }
        let pct = round2((p - fair) / fair * 100.0);
        let v = "fair";
        if p < onum(pick(best, "low")) { v = "below_fair"; }
        else if p > onum(pick(best, "high")) { v = "above_fair"; }
        let out_row = #{
            id: pstr(row, "id"),
            title: pstr(row, "title"),
            tag: pstr(best, "tag"),
            price: round2(p),
            fair: fair,
            low: pick(best, "low"),
            high: pick(best, "high"),
            n_priced: oint(pick(best, "n"), 0),
            pct_vs_fair: pct,
            verdict: v
        };
        verdict_rows.push(#{ k: pstr(row, "id"), n: pct.abs(), row: out_row });
        if v != "fair" { out_rows.push(#{ k: pstr(row, "id"), n: pct.abs(), row: out_row }); }
    }
    // Anything outside its tag's fair band — cheap *and* expensive alike. The
    // two are opposites for the buyer, so the caller splits them by
    // `row.verdict` rather than being handed one list called "deals" that
    // quietly contains the over-priced half.
    let outliers = pull_docs(rank_top(out_rows, 50), out_rows, "k");

    // ── 9. the vocabulary document ──
    let group_docs = [];
    for g in priced_groups {
        let key = g["k"];
        let spellings = [];
        let raws_out = [];
        let mk = if members.contains(key) { members[key] } else { [] };
        for m in mk {
            spellings.push(m);
            if raws.contains(m) {
                let bucket = raws[m];
                for rk in bucket.keys() { raws_out.push(#{ k: ostr(rk), n: oint(bucket[rk], 0) }); }
            }
        }
        let co_rows = [];
        let bucket = co[key];
        for ck in bucket.keys() { co_rows.push(#{ k: ostr(ck), n: oint(bucket[ck], 0) }); }
        let st = if stat_of.contains(key) { stat_of[key] } else { #{} };
        group_docs.push(#{
            tag: key,
            ns: ns_of_key(key),
            name: tag_of_key(key),
            items: oint(item_count[key], 0),
            // sort() sorts in place and hands back unit in this engine build,
            // so the sorted array is passed on in its own statement; reading its
            // return value used to store a null here.
            spellings: sorted_strings(spellings),
            raws: pull_docs(rank_top(raws_out, 8), raws_out, "k"),
            first_seen_ms: oint(first_seen[key], 0),
            last_seen_ms: oint(last_seen[key], 0),
            samples: if samples.contains(key) { samples[key] } else { [] },
            co: pull_docs(rank_top(co_rows, oint(pick(o, "co_top"), 6)), co_rows, "k"),
            n_priced: oint(pick(st, "n"), 0),
            fair: pick(st, "fair")
        });
    }
    let group_ranks = [];
    for g in group_docs { group_ranks.push(#{ k: g["tag"], n: g["items"] }); }
    let group_list = pull_docs(rank_top(group_ranks, vocab_max), group_docs, "tag");

    let fair_rows = [];
    for t in tag_stats {
        if type_of(pick(t, "fair")) == "()" { continue; }
        fair_rows.push(#{
            tag: t["tag"], n: t["n_priced"], items: t["items"],
            fair: t["fair"], low: t["low"], high: t["high"]
        });
    }
    let fair_ranks = [];
    for f in fair_rows { fair_ranks.push(#{ k: f["tag"], n: f["n"] }); }
    let fair_top = pull_docs(rank_top(fair_ranks, 12), fair_rows, "tag");

    // ── 10. what changed since the last run ──
    let prev_tags = [];
    if type_of(pick(o, "prev_tags")) == "array" { prev_tags = o["prev_tags"]; }
    let new_only = [];
    for g in group_list {
        if !prev_tags.contains(g["tag"]) {
            new_only.push(#{ tag: g["tag"], ns: g["ns"], items: g["items"], fair: g["fair"] });
        }
    }

    let runs = oint(pick(learned, "runs"), 0) + 1;
    let tags_doc = #{
        version: 1,
        updated_at: now["rfc3339"],
        updated_at_ms: now["unix_ms"],
        runs: runs,
        last_run_at: now["rfc3339"],
        last_run_at_ms: now["unix_ms"],
        group_count: group_list.len(),
        vocab_size: sk.len(),
        groups: group_list,
        synonyms: syn_rows
    };
    let model_doc = #{
        version: 1,
        updated_at: now["rfc3339"],
        updated_at_ms: now["unix_ms"],
        runs: runs,
        min_samples: min_samples,
        sigma: sigma,
        tags: tag_stats,
        items: item_stats,
        corrections: corrections,
        next_feed_factor: feed_factor_of(corrections, o),
        outliers: outliers
    };

    #{
        tags_doc: tags_doc,
        model_doc: model_doc,
        index_rows: corpus,
        new_tags: new_only,
        group_count: group_list.len(),
        vocab_size: sk.len(),
        priced_items: priced_groups.len(),
        fair: fair_top,
        outliers: outliers,
        verdicts: verdict_rows,
        group_of: group_of,
        corrections: corrections[ckey],
        next_feed_factor: feed_factor_of(corrections, o),
        runs: runs
    }
}

fn uf_find(parent, k) {
    let cur = k;
    let hops = 0;
    while omap(parent) && parent.contains(cur) && ostr(parent[cur]) != cur && hops < 64 {
        cur = ostr(parent[cur]);
        hops += 1;
    }
    cur
}

fn uf_union(parent, a, b) {
    let ra = uf_find(parent, a);
    let rb = uf_find(parent, b);
    if ra == rb { return ra; }
    // Deterministic shape, not a meaningful order: the group's *name* comes from
    // the frequency vote in learn(), never from which root happened to win.
    if ra.len() <= rb.len() { parent[rb] = ra; } else { parent[ra] = rb; }
    ra
}

// Every knob of the learning pass, with the defaults that make it safe to leave
// alone: the model only speaks about a tag once it has seen three priced
// listings carrying it, and a learned card-price factor needs five agreeing
// sightings inside a 10% band before it touches a price.
fn evolve_opts(args) {
    #{
        min_samples: opt_int(args, "min_samples", 3),
        sigma: opt_float(args, "sigma", 2.0),
        syn_min_hits: opt_int(args, "syn_min_hits", 2),
        vocab_max: opt_int(args, "vocab_max", 300),
        model_prices: opt_int(args, "model_prices", 120),
        index_max: opt_int(args, "index_max", 5000),
        co_top: opt_int(args, "co_top", 6),
        feed_min_hits: opt_int(args, "feed_min_hits", 5),
        feed_max_spread: opt_float(args, "feed_max_spread", 1.1),
        apply_feed_factor: opt_bool(args, "apply_feed_factor", true),
        emit_sql: opt_bool(args, "emit_sql", false),
        sql_max: opt_int(args, "sql_max", 800),
        sql_points: opt_int(args, "sql_points", 10)
    }
}
// ── SQLite hand-off ─────────────────────────────────────────────────────────
//
// The three stores are the plugin's own memory. They are deliberately not a
// database: a plugin has no way to reach one, and a JSON file rewritten once per
// run is the right shape for "what did I learn". When the caller does want SQL —
// SQLite for the rows, DuckDB for the aggregates, via the engine's `kind: "db"`
// capability — the plugin hands over finished statements instead of asking the
// engine to build them, so what lands in the database is exactly the numbers
// the plugin used for its own decisions.
//
// Nothing is executed here. `emit_sql: true` puts two arrays in the result,
// `sql_schema` (DDL) and `sql_statements` (rows), for the workflow to pass to a
// db capability. Every statement is idempotent, so collecting the same thing
// twice costs nothing but a re-read.
//
// The tables are prefixed `goofish_` because the database file is a shared
// resource: a user who keeps one SQLite file for prices and tags keeps their
// other tables in it too.

fn sql_q(s) {
    let out = "'";
    let i = 0;
    while i < s.len() {
        let c = s.sub_string(i, 1);
        if c == "'" { out += "''"; } else { out += c; }
        i += 1;
    }
    out + "'"
}

// A number, or NULL. A NaN or an absurd magnitude becomes NULL rather than a
// literal that would not parse — JSON has no NaN, but a median of zero in a
// ratio can still produce one.
fn sql_n(v) {
    if type_of(v) == "()" { return "NULL"; }
    let f = onum(v);
    if f != f { return "NULL"; }
    if f > 1e300 || f < -1e300 { return "NULL"; }
    fmt_num(f)
}

fn sql_i(v) {
    "" + oint(v, 0)
}

fn sql_join(xs) {
    let out = "";
    let i = 0;
    while i < xs.len() {
        if i > 0 { out += ","; }
        out += xs[i];
        i += 1;
    }
    out
}

fn sql_schema() {
    [
        "CREATE TABLE IF NOT EXISTS goofish_items(id TEXT PRIMARY KEY, title TEXT, url TEXT, status TEXT, location TEXT, want_count INTEGER, collected_at TEXT, collected_at_ms INTEGER, source_mode TEXT, tags TEXT, price REAL, last_price REAL, low_price REAL, high_price REAL, median_price REAL, vs_median_pct REAL)",
        // `implausible_price` carries a reading the monitor refused to believe
        // (see observe()): the sighting is kept with no price of its own, and
        // the number it would have had is kept beside it, so the SQLite copy
        // shows why the series has a hole in it instead of a 1000000% jump.
        "CREATE TABLE IF NOT EXISTS goofish_price_points(item_id TEXT, at_ms INTEGER, at TEXT, price REAL, want_count INTEGER, views INTEGER, status TEXT, implausible_price REAL, PRIMARY KEY (item_id, at_ms))",
        "CREATE TABLE IF NOT EXISTS goofish_tags(tag TEXT PRIMARY KEY, ns TEXT, name TEXT, items INTEGER, n_priced INTEGER, first_seen_ms INTEGER, last_seen_ms INTEGER, spellings TEXT, co_tags TEXT, fair REAL)",
        "CREATE TABLE IF NOT EXISTS goofish_item_tags(item_id TEXT, tag TEXT, raw TEXT, PRIMARY KEY (item_id, tag))",
        "CREATE TABLE IF NOT EXISTS goofish_tag_price_stats(tag TEXT PRIMARY KEY, ns TEXT, n INTEGER, items INTEGER, min REAL, median REAL, mean REAL, max REAL, mad REAL, fair REAL, low REAL, high REAL, prices TEXT)",
        "CREATE TABLE IF NOT EXISTS goofish_price_corrections(key TEXT PRIMARY KEY, n INTEGER, factor REAL, spread REAL, p25 REAL, p75 REAL, first_seen TEXT, last_seen TEXT)"
    ]
}

// `lv` is learn()'s return value, `wdata` the watch store.
//
// Small tables first, so a statement budget that runs out loses price points —
// the bulkiest and most reproducible table — rather than the vocabulary or the
// correction the rest of the model is keyed on.
fn sql_statements(lv, wdata, o) {
    let out = [];
    let max = oint(pick(o, "sql_max"), 800);
    let dropped = [0];
    let points_cap = oint(pick(o, "sql_points"), 10);
    let model = pick(lv, "model_doc");
    let tags_doc = pick(lv, "tags_doc");
    let corpus = pick(lv, "index_rows");
    let group_of = pick(lv, "group_of");

    // ── the learned correction, one row ──
    let corr = pick(model, "corrections");
    if omap(corr) {
        let ckeys = corr.keys();
        ckeys.sort();
        for kk in ckeys {
            let c = corr[ostr(kk)];
            out.push("INSERT OR REPLACE INTO goofish_price_corrections(key, n, factor, spread, p25, p75, first_seen, last_seen) VALUES("
                + sql_q(ostr(kk)) + ", " + sql_i(pick(c, "n")) + ", " + sql_n(pick(c, "factor"))
                + ", " + sql_n(pick(c, "spread")) + ", " + sql_n(pick(c, "p25")) + ", " + sql_n(pick(c, "p75"))
                + ", " + sql_q(pstr2(c, "first_seen", "")) + ", " + sql_q(pstr2(c, "last_seen", "")) + ")");
        }
    }

    // ── the vocabulary ──
    if omap(tags_doc) && tags_doc.contains("groups") && type_of(tags_doc["groups"]) == "array" {
        for g in tags_doc["groups"] {
            let spell = [];
            if g.contains("spellings") && type_of(g["spellings"]) == "array" {
                for s in g["spellings"] { spell.push(ostr(s)); }
            }
            let co = [];
            if g.contains("co") && type_of(g["co"]) == "array" {
                for c in g["co"] { co.push(ostr(pick(c, "k"))); }
            }
            out.push("INSERT OR REPLACE INTO goofish_tags(tag, ns, name, items, n_priced, first_seen_ms, last_seen_ms, spellings, co_tags, fair) VALUES("
                + sql_q(pstr(g, "tag")) + ", " + sql_q(pstr(g, "ns")) + ", " + sql_q(pstr(g, "name"))
                + ", " + sql_i(pick(g, "items")) + ", " + sql_i(pick(g, "n_priced"))
                + ", " + sql_i(pick(g, "first_seen_ms")) + ", " + sql_i(pick(g, "last_seen_ms"))
                + ", " + sql_q(sql_join(spell)) + ", " + sql_q(sql_join(co)) + ", " + sql_n(pick(g, "fair")) + ")");
        }
    }

    // ── per-tag price statistics ──
    if omap(model) && model.contains("tags") && type_of(model["tags"]) == "array" {
        for t in model["tags"] {
            let ps = [];
            if t.contains("prices") && type_of(t["prices"]) == "array" {
                for p in t["prices"] { ps.push(fmt_num(onum(p))); }
            }
            out.push("INSERT OR REPLACE INTO goofish_tag_price_stats(tag, ns, n, items, min, median, mean, max, mad, fair, low, high, prices) VALUES("
                + sql_q(pstr(t, "tag")) + ", " + sql_q(pstr(t, "ns")) + ", " + sql_i(pick(t, "n_priced"))
                + ", " + sql_i(pick(t, "items"))
                + ", " + sql_n(pick(t, "min")) + ", " + sql_n(pick(t, "median")) + ", " + sql_n(pick(t, "mean"))
                + ", " + sql_n(pick(t, "max")) + ", " + sql_n(pick(t, "mad")) + ", " + sql_n(pick(t, "fair"))
                + ", " + sql_n(pick(t, "low")) + ", " + sql_n(pick(t, "high")) + ", " + sql_q(sql_join(ps)) + ")");
        }
    }

    // ── per-item price history summary, keyed by the watch store's own ids ──
    let istat = #{};
    if omap(model) && model.contains("items") && type_of(model["items"]) == "array" {
        for it in model["items"] { istat[pstr(it, "id")] = it; }
    }
    let corpus_rows = if type_of(corpus) == "array" { corpus } else { [] };
    for row in corpus_rows {
        let id = pstr(row, "id");
        let tags = [];
        if omap(row) && row.contains("tags") && type_of(row["tags"]) == "array" {
            let keys = [];
            for t in row["tags"] {
                let k = tag_key(pstr2(t, "ns", ""), pstr(t, "tag"));
                if !keys.contains(k) { keys.push(k); }
            }
            keys.sort();
            for k in keys {
                tags.push(if group_of.contains(k) { ostr(group_of[k]) } else { k });
            }
        }
        let s = if istat.contains(id) { istat[id] } else { #{} };
        if out.len() >= max { dropped[0] += 1; continue; }
        out.push("INSERT OR REPLACE INTO goofish_items(id, title, url, status, location, want_count, collected_at, collected_at_ms, source_mode, tags, price, last_price, low_price, high_price, median_price, vs_median_pct) VALUES("
            + sql_q(id) + ", " + sql_q(pstr(row, "title")) + ", " + sql_q(pstr(row, "url"))
            + ", " + sql_q(pstr(row, "status")) + ", " + sql_q(pstr(row, "location"))
            + ", " + sql_i(pick(row, "want_count")) + ", " + sql_q(pstr(row, "collected_at"))
            + ", " + sql_i(pick(row, "collected_at_ms")) + ", " + sql_q(pstr(row, "source_mode"))
            + ", " + sql_q(sql_join(tags))
            // "price" is the price this row's tags were priced BY - the same
            // number the tag statistics were built from, which is what makes
            // "compare this listing against its tag's fair price" answerable in
            // SQL. "last_price" is the watch history's latest sighting, a
            // different (and possibly older) question, so both are kept.
            + ", " + sql_n(pick(row, "price"))
            + ", " + sql_n(pick(s, "last")) + ", " + sql_n(pick(s, "low")) + ", " + sql_n(pick(s, "high"))
            + ", " + sql_n(pick(s, "median")) + ", " + sql_n(pick(s, "vs_median_pct")) + ")");
        // item -> tag, so DuckDB can ask "what does this listing look like
        // against everything else carrying the same tag".
        let raw_of = #{};
        if omap(row) && row.contains("tags") && type_of(row["tags"]) == "array" {
            for t in row["tags"] { raw_of[tag_key(pstr2(t, "ns", ""), pstr(t, "tag"))] = ostr(pick(t, "raw")); }
        }
        let ki = 0;
        while ki < tags.len() {
            if out.len() >= max { dropped[0] += 1; ki += 1; continue; }
            let gk = tags[ki];
            out.push("INSERT OR REPLACE INTO goofish_item_tags(item_id, tag, raw) VALUES("
                + sql_q(id) + ", " + sql_q(gk) + ", " + sql_q(if raw_of.contains(gk) { ostr(raw_of[gk]) } else { gk }) + ")");
            ki += 1;
        }
    }

    // ── every price ever observed ──
    for w in watch_items(wdata) {
        let id = pstr(w, "id");
        if !omap(w) || !w.contains("history") || type_of(w["history"]) != "array" { continue; }
        let hist = [];
        for h in w["history"] { hist.push(h); }
        let start = 0;
        if hist.len() > points_cap { start = hist.len() - points_cap; }
        let i = start;
        while i < hist.len() {
            let h = hist[i];
            if out.len() >= max { dropped[0] += 1; i += 1; continue; }
            out.push("INSERT OR REPLACE INTO goofish_price_points(item_id, at_ms, at, price, want_count, views, status, implausible_price) VALUES("
                + sql_q(id) + ", " + sql_i(pick(h, "at_ms")) + ", " + sql_q(pstr2(h, "at", ""))
                + ", " + sql_n(pick(h, "price")) + ", " + sql_i(pick(h, "want_count"))
                + ", " + sql_i(pick(h, "views")) + ", " + sql_q(pstr2(h, "status", ""))
                + ", " + sql_n(pick(h, "implausible_price")) + ")");
            i += 1;
        }
    }

    #{ statements: out, dropped: dropped[0], count: out.len() }
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
    // A search and a learning run share the listing path; only the corpus size
    // and what finish() does afterwards differ.
    let wide = mode == "search" || mode == "evolve";
    if wide { source_url = search_url(host, query, p["preencoded"]); }

    let results = [];
    let failures = [];
    let changes = [];
    let wdata = load_watch(host, wpath, started);
    let initial_tracked = watch_items(wdata).len();
    let notes = [];

    // What the self-evolution pass gets to learn from this run: every entry
    // that was actually read, and the pairs where the same item was priced
    // twice — once on the card, once on the detail page. finish() turns them
    // into index.json / tags.json / price_model.json.
    let lr = #{ entries: [], feed_pairs: [], args: args, out_dir: out_dir };

    // A learned correction factor rewrites card prices that were never
    // confirmed by a detail page, so it has to be in hand *before* the search
    // loop reads any card. Loading it here costs one small file read and keeps
    // the correction out of the browsing path.
    let feed_factor = 0.0;
    {
        let mdoc = read_json_or(host, model_path(out_dir), #{});
        if omap(mdoc) && mdoc.contains("next_feed_factor") {
            feed_factor = onum(pick(mdoc, "next_feed_factor"));
        }
    }

    // "价格进化" with nothing to fetch is a legitimate request: re-derive the
    // model from everything already in index.json. It costs no browser time and
    // is the cheapest way to ask "what does the plugin think a Sony A7M4 costs?".
    if mode == "evolve" && p["item_url"] == "" && query == "" {
        notes.push("evolve with no query and no item: re-derived the price model from"
            + " index.json (" + index_rows(load_index(host, out_dir)).len() + " items on record)");
        return finish(host, started, mode, query, source_url, out_dir, wpath,
            wdata, initial_tracked, results, failures, changes, notes, 0, 0, 0, 0, lr);
    }

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
                    wdata, initial_tracked, results, failures, changes, notes, 0, 1, 0, 0, lr);
            }
            failures.push(#{ rank: 1, url: url, status: st, error: why });
            return finish(host, started, mode, query, source_url, out_dir, wpath,
                wdata, initial_tracked, results, failures, changes, notes, 0, 0, 0, 0, lr);
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
        lr["entries"].push(entry);
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
            wdata, initial_tracked, results, failures, changes, notes, 0, 0, 0, 0, lr);
    }

    // ── price monitoring: re-read every tracked item, report what moved ─────
    if mode == "watch" {
        let items = watch_items(wdata);
        if items.len() == 0 {
            notes.push("watch.json is empty — nothing tracked yet. Add an item with"
                + " `mode: item` (or pass watch_add:<url>) first.");
            return finish(host, started, mode, query, source_url, out_dir, wpath,
                wdata, initial_tracked, results, failures, changes, notes, 0, 0, 0, 0, lr);
        }

        let targets = [];
        let i = 0;
        while i < items.len() && targets.len() < watch_limit {
            let e = items[i];
            let keep = true;
            if watch_filter != "" {
                // The title or the id, case-insensitively. Sellers type the
                // model in every casing there is ("索尼a7m4", "索尼A7M4",
                // "Sony A7m4") and a watch list is a list of ids, so a literal
                // title-only contains() selects nothing unless the caller
                // happens to match the seller's own capitalisation or
                // reproduces a title from the site - and the run then reports
                // every counter as zero, which reads like "nothing moved"
                // rather than "nothing matched".
                let want = watch_filter.to_lower();
                let title = pstr(e, "title").to_lower();
                let id_l = pstr(e, "id").to_lower();
                keep = title.contains(want) || id_l.contains(want);
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
            lr["entries"].push(item_record(rec, j + 1, url, "watch", now));
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
            wdata, initial_tracked, results, failures, changes, notes, checked, gone, 0, 0, lr);
    }

    // ── search: list a page of cards, optionally open the first N in full ───
    if wide {
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
                wdata, initial_tracked, results, failures, changes, notes, 0, 0, 0, 0, lr);
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
        let corrected_cards = 0;
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

            // The one thing this plugin can learn about its own reading: the
            // card and the detail page price the same item, sometimes
            // differently. Record the pair when we happen to see both.
            let card_price = onum(pick(row, "price"));
            if opened {
                let detail_price = onum(pick(rec, "price"));
                if card_price > 0.0 && detail_price > 0.0 {
                    lr["feed_pairs"].push(#{ id: id, feed: card_price, detail: detail_price });
                }
            } else if feed_factor > 0.0 && card_price > 0.0 {
                // A card that was never opened is exactly the price the learned
                // factor exists to fix: the original is kept beside the
                // corrected one so the caller can see what was changed.
                rec["price"] = round2(card_price * feed_factor);
                rec["price_corrected"] = true;
                rec["price_uncorrected"] = card_price;
                corrected_cards += 1;
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
            lr["entries"].push(entry);
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
        if corrected_cards > 0 {
            notes.push("applied the learned card-price factor to " + corrected_cards
                + " unopened card(s); the raw price is kept as price_uncorrected");
        }
        let paged = 0;
        if omap(page) && page.contains("pages") && type_of(page["pages"]) == "array" {
            paged = page["pages"].len();
        }
        // checked_count means "re-read a tracked item and compared it", which
        // is what a watch run does; a listing just reads, so it reports 0.
        return finish(host, started, mode, query, source_url, out_dir, wpath,
            wdata, initial_tracked, results, failures, changes, notes, 0, 0,
            paged, dropped_by_price, lr);
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
          pages_visited, price_filtered, lr) {
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

    // ── self-evolution: fold what this run read into the model ───────────────
    //
    // Every mode feeds the pass, not just `evolve`: watching a price or
    // browsing an item is exactly the fresh evidence the model needs, and
    // making the caller opt in separately would mean the model only ever learns
    // when someone remembered to ask for it.
    let evo = ();
    let sql_schema_out = [];
    let sql_statements_out = [];
    let fresh = [];
    for e in lr["entries"] {
        let m = mine_tags(e);
        fresh.push(index_row(e, m, now));
    }
    let corpus = merge_index(load_index(host, out_dir), fresh,
        oint(pick(evolve_opts(lr["args"]), "index_max"), 5000));
    let o = evolve_opts(lr["args"]);
    let tags_doc = read_json_or(host, tags_path(out_dir), #{});
    let prev_tags = [];
    if omap(tags_doc) && tags_doc.contains("groups") && type_of(tags_doc["groups"]) == "array" {
        for g in tags_doc["groups"] { prev_tags.push(pstr(g, "tag")); }
    }
    o["prev_tags"] = prev_tags;
    let learned = load_learned(host, out_dir, now);
    let lv = learn(host, out_dir, corpus, wdata, learned, fresh, lr["feed_pairs"], now, o);

    let evo_error = "";
    try {
        // The index is the corpus, so it is written as a document around the
        // rows: the same shape load_index() reads back, which is what makes a
        // second `evolve` a re-derivation instead of a second opinion.
        write_json(host, index_path(out_dir), #{
            version: 1,
            updated_at: now["rfc3339"],
            updated_at_ms: now["unix_ms"],
            items: lv["index_rows"]
        });
        write_json(host, tags_path(out_dir), lv["tags_doc"]);
        write_json(host, model_path(out_dir), lv["model_doc"]);
    } catch (err) {
        // A failed write must not take the run's actual answer down with it: the
        // prices and tags above are still correct for this run.
        evo_error = "" + err;
        notes.push("could not save the price model: " + evo_error);
    }

    for nt in lv["new_tags"] {
        notes.push("learned tag " + nt["tag"] + " from " + nt["items"] + " item(s)");
    }
    for s in lv["tags_doc"]["synonyms"] {
        notes.push("tag synonyms " + s["a"] + " = " + s["b"] + " ("
            + s["n"] + " sightings, ns " + (if s["ns"] == "" { "-" } else { s["ns"] }) + ")");
    }
    let nf = lv["model_doc"]["next_feed_factor"];
    if nf > 0.0 {
        notes.push("learned card-price factor " + fmt_num(nf)
            + " — applied to card prices that no detail page confirmed");
    } else {
        notes.push("card-price factor not learned yet: not enough agreeing card/detail pairs");
    }
    // An outlier below the band is something to buy; one above it is something
    // to walk away from. They come out of the model in one list, because the
    // model only knows the band — the split is a question about intent, so it
    // belongs to the caller that knows.
    let cheap = [];
    let dear = [];
    for o in lv["outliers"] {
        let row = o["row"];
        if ostr(row["verdict"]) == "below_fair" { cheap.push(row); } else { dear.push(row); }
    }
    if cheap.len() > 0 {
        notes.push(cheap.len() + " item(s) priced below their tag's fair range"
            + " (best: " + ostr(cheap[0]["title"]) + " at " + fmt_num(onum(cheap[0]["price"]))
            + ", fair " + fmt_num(onum(cheap[0]["fair"])) + ")");
    }
    if dear.len() > 0 {
        notes.push(dear.len() + " item(s) priced above their tag's fair range");
    }
    evo = #{
        items_indexed: lv["index_rows"].len(),
        new_this_run: fresh.len(),
        tags: lv["tags_doc"]["groups"].len(),
        vocabulary: lv["tags_doc"]["vocab_size"],
        priced_items: lv["priced_items"],
        runs: lv["tags_doc"]["runs"],
        fair: lv["fair"],
        deals: cheap,
        overpriced: dear,
        verdicts: lv["verdicts"],
        next_feed_factor: nf,
        corrections: lv["model_doc"]["corrections"],
        index_file: index_path(out_dir),
        tags_file: tags_path(out_dir),
        model_file: model_path(out_dir),
        error: evo_error
    };

    if opt_bool(lr["args"], "emit_sql", false) {
        let sq = sql_statements(lv, wdata, o);
        sql_schema_out = sql_schema();
        sql_statements_out = sq["statements"];
        if sq["dropped"] > 0 {
            notes.push("sql: " + sq["count"] + " statements, " + sq["dropped"]
                + " price points dropped past sql_points");
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

    let out = #{
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
        failures: failures,
        evolve: evo
    };
    if sql_statements_out.len() > 0 {
        out["sql_schema"] = sql_schema_out;
        out["sql_statements"] = sql_statements_out;
    }
    out
}
