// jev-planner — one Jev step over a real Chrome page.
//
// The Rust engine owns the browser, the policy gates and the tab lifecycle; the
// `chrome_cdp` capability owns the *execution* of a decision (real CDP input).
// This plugin owns the *decision*: wait for the shared tab to settle, observe
// it, and return
//
//   { operation, element | selector, goal_probability, stuck_probability, ... }
//
// which an `agent_step` node then executes. The page-side policy lives in
// `page/plan.js`; this file is only plumbing + the deterministic wait.
//
// ops (pick one with `with.op`):
//   visit    navigate the shared tab to `with.url` and wait for the commit
//   step    observe the tab and return the next Jev decision            (page/plan.js)
//   observe  numbered snapshot: url, title, N elements, first screen, prices (page/observe.js)
//   dismiss  close a full-viewport modal overlay so later clicks land        (page/dismiss.js)
//   find     turn an intent into ONE numbered decision                  (page/find.js)
//   sweep    open N product cards one by one, read each detail, rank them
//            (page/cards.js + page/item.js) and return the best one
//   guard    wait out an anti-bot wall (滑块 / 验证码 / 访问太频繁) before acting
//            (page/wall.js)
//   reset   clear this origin's round counter and "already followed" memory
//
// `observe` / `find` are the *directed* half: where `step` lets the page-side
// policy roam (scroll a page through, then follow a content link), `find` says
// exactly what to act on — a role + label + optional nth — and returns the same
// Jev decision shape, so an `agent_step` node executes it unchanged. `find`
// matches against the laya-browser extension's numbered snapshot, so the number
// in the decision is the number the extension drew on the page.

fn run(host, ctx) {
    throw "jev-planner: pick an op (visit | step | observe | find | dismiss | sweep | pause | guard | reset)";
}

fn _arg(ctx, key) {
    let w = ctx["with"];
    if type_of(w) == "map" && w.contains(key) {
        let v = w[key];
        if v == () { return ""; }
        return v;
    }
    ""
}

fn _origin(u) {
    let parts = u.split("://");
    if parts.len < 2 { return ""; }
    let host = parts[1].split("/");
    parts[0] + "://" + host[0]
}

// Poll the target until it is really loaded *and* stable: the engine's own
// wait_ready skips the URL check and can return on the pre-navigation
// about:blank target, racing the navigation commit. `prefix` (when given)
// pins the wait to the origin we asked for; `quiet` is how long the URL must
// stop changing before we trust the page.
fn _settle(host, target, prefix, quiet_ms) {
    let probe = `({ url: location.href, ready: document.readyState, len: (document.body ? document.body.innerText.length : 0) })`;
    let last = "";
    let last_info = #{};
    let stable = 0;
    let ok = 0;
    let bad = 0;
    let last_err = "";
    let tries = 0;
    while tries < 100 {
        tries += 1;
        try {
            let info = host.browser_evaluate(target, probe, false);
            ok += 1;
            last_info = info;
            let url = info["url"];
            let wanted = prefix == "" || url.starts_with(prefix);
            // "complete" is the clean signal. Some sites (Taobao's home and search
            // pages) hold a long-lived resource open, so readyState sits at
            // "interactive" for tens of seconds even though the page is painted and
            // usable. After a short grace, trust a rendered body the way a person
            // reads a page that has visibly drawn.
            let drawn = info["ready"] == "complete"
                || (tries > 25 && info["ready"] != "loading" && _num(info["len"]) > 0);
            if wanted && url != "about:blank" && drawn {
                if url == last {
                    stable += 200;
                    if stable >= quiet_ms { return info; }
                } else {
                    stable = 0;
                }
            } else {
                stable = 0;
            }
            last = url;
        } catch (e) {
            bad += 1;
            last_err = "" + e;
            stable = 0;
        }
        sleep(0.2);
    }
    // The tab answered but never went quiet: a slow load, a spinner, or a SPA
    // that keeps changing its URL. Take the last good observation rather than
    // failing a long run over it — the planner re-observes next round anyway.
    if ok > 0 && last != "" {
        let why = "";
        if last_err != "" { why = ", last_err=" + last_err; }
        host.log("jev-planner: settle gave up after " + tries + " tries (ok=" + ok + ", err=" + bad
            + ", url=" + last + why + ")");
        return last_info;
    }
    throw "jev-planner: target never settled (tries=" + tries + ", ok=" + ok + ", err=" + bad
        + ", last=" + last + ", last_err=" + last_err + ")";
}

fn _title(host, target) {
    let t = "";
    try { t = host.browser_evaluate(target, `document.title`, false); } catch (e) { t = ""; }
    t
}

// ── op: visit ────────────────────────────────────────────────────────────────
fn visit(host, ctx) {
    // `--query "https://…"` is the friendly spelling; `--state url` the explicit one.
    let url = _arg(ctx, "url");
    if url == "" { url = _arg(ctx, "query"); }
    if url == "" { throw "jev-planner.visit: with.url (or with.query) is required"; }
    let target = _arg(ctx, "target_id");
    let owned = false;
    if target == "" {
        target = host.browser_open(url);
        owned = true;
    } else {
        host.browser_navigate(target, url);
    }
    let info = #{ url: url, target_id: target };
    try {
        info = _settle(host, target, _origin(url), 400);
    } catch (e) {
        if owned { host.browser_release(target); }
        throw "jev-planner.visit: " + e;
    }
    // A run starts here: forget the previous run's round counter / followed set
    // so round numbers are per run, not cumulative in this profile.
    try {
        host.browser_evaluate(target,
            `try { localStorage.removeItem('__laya_jev_round'); localStorage.removeItem('__laya_jev_seen'); } catch (e) {} true`,
            false);
    } catch (e) { }
    #{
        target_id: target,
        url: info["url"],
        title: _title(host, target),
        ready: info["ready"]
    }
}

// ── op: step ────────────────────────────────────────────────────────────────
fn step(host, ctx) {
    let target = _arg(ctx, "target_id");
    if target == "" { throw "jev-planner.step: with.target_id is required"; }
    // A click issued by the previous node may still be navigating the tab; wait
    // until the URL has stopped changing so the observation is of one page.
    _settle(host, target, "", 500);
    let opts = #{ goal: _arg(ctx, "goal") };
    let js = "var __LAYA__ = " + host.json_stringify(opts) + ";\n" + host.js("plan.js");
    host.browser_evaluate(target, js, false)
}

// ── op: observe ─────────────────────────────────────────────────────────────
fn observe(host, ctx) {
    let target = _arg(ctx, "target_id");
    if target == "" { throw "jev-planner.observe: with.target_id is required"; }
    // A pager click navigates *asynchronously*: the tab can still show the old
    // page (same URL, same DOM) for a beat after the click. `with.prev_url` makes
    // this round wait for the URL to actually change before observing, which is
    // what fixes "observe the page you were just on" lag on a multi-page walk.
    let prev = _arg(ctx, "prev_url");
    if prev != "" && prev != "null" {
        let n = 0;
        while n < 80 {
            let u = "";
            try { u = host.browser_evaluate(target, "location.href", false); } catch (e) { u = ""; }
            if u != "" && u != prev { break; }
            sleep(0.25);
            n += 1;
        }
    }
    // A prior click may still be navigating; wait for one settled page first.
    _settle(host, target, "", 400);
    let js = "var __LAYA__ = {};\n" + host.js("observe.js");
    host.browser_evaluate(target, js, false)
}

// The directed-intent ops read their arguments from `with`; keep the page-side
// names explicit instead of forwarding whatever the spec happened to pass.
fn _opts(ctx, keys) {
    let w = ctx["with"];
    if type_of(w) != "map" { w = #{}; }
    let o = #{};
    for k in keys {
        if w.contains(k) {
            let v = w[k];
            if v != () { o[k] = v; }
        }
    }
    o
}

// ── op: dismiss ─────────────────────────────────────────────────────────────
fn dismiss(host, ctx) {
    let target = _arg(ctx, "target_id");
    if target == "" { throw "jev-planner.dismiss: with.target_id is required"; }
    host.browser_evaluate(target, host.js("dismiss.js"), false)
}

// ── op: find ────────────────────────────────────────────────────────────────
fn find(host, ctx) {
    let target = _arg(ctx, "target_id");
    if target == "" { throw "jev-planner.find: with.target_id is required"; }
    _settle(host, target, "", 300);
    // Whitelist the page-side knobs find.js reads, so a typo in the spec shows
    // up as "no element matched" with the candidate list, not as a silent
    // default action on the wrong element.
    let opts = _opts(ctx, [
        "operation", "roles", "role", "match", "href", "nth", "exact", "selector", "text",
        "note", "stay_in_tab", "goal_probability", "stuck_probability"
    ]);
    let js = "var __LAYA__ = " + host.json_stringify(opts) + ";\n" + host.js("find.js");
    // A list that is still fetching renders a skeleton pager, so the first scan
    // can legitimately come back empty for a control that appears a moment
    // later. `with.retry_ms` re-scans before giving up, which is what a
    // pagination step needs; without it, one attempt and fail (the error carries
    // the candidate list, so a real typo is still obvious).
    let retry_ms = _arg(ctx, "retry_ms");
    let tries = 0;
    if type_of(retry_ms) == "int" || type_of(retry_ms) == "i64" { tries = retry_ms / 400; }
    let n = 0;
    while true {
        try {
            return host.browser_evaluate(target, js, false);
        } catch (e) {
            n += 1;
            if n > tries { throw e; }
            sleep(0.4);
        }
    }
}

// ── op: sweep ───────────────────────────────────────────────────────────────
// The comparison pass: read the product cards on the current search-results
// page (page/cards.js), then open them ONE AT A TIME — navigate the owned tab to
// each card's detail page, settle, read it (page/item.js) — for up to `with.n`
// products. Every product is journalled as one line and the op returns the whole
// table plus the single best row, so the workflow can end by navigating to that
// winner and observing it. Nothing here clicks: the tab is the same one the run
// owns, only its URL changes.
//
// Taobao rate-limits a burst of navigations with a *punish* wall (a slider, a
// click-image, a drag-drop, or a bare "验证码拦截"/deny page). So the sweep paces
// itself like a person (1.6–3.0 s between page opens) and, when a read-back is a
// wall, cools down and re-opens the same URL a few times before writing it off.
fn _int(ctx, key, dflt) {
    let v = _arg(ctx, key);
    if type_of(v) == "int" || type_of(v) == "i64" { return v; }
    if v == "" { return dflt; }
    let s = "" + v;
    try { return parse_int(s); } catch (e) { return dflt; }
}

fn _num(v) {
    if type_of(v) == "int" || type_of(v) == "i64" { return v.to_float(); }
    if type_of(v) == "f64" || type_of(v) == "float" { return v; }
    if v == () { return -1.0e9; }
    if v == "" { return -1.0e9; }
    try { return parse_float("" + v); } catch (e) { return -1.0e9; }
}

fn _abs(x) {
    if x < 0.0 { return -x; }
    x
}

// A wall, however it announced itself. `wall.js` and `item.js` already fold the
// baxia shell and the in-iframe `punish` frame into `wall`; the flags are repeated
// here so a page script that only reports them is still understood.
fn _wall(d) {
    if d["wall"] == true { return true; }
    if d["shell"] == true { return true; }
    if d["punished"] == true { return true; }
    let t = "" + d["title"];
    let u = "" + d["url"];
    if t.contains("验证码") || t.contains("拦截") || t.contains("访问过") || t.contains("访问太频繁") { return true; }
    if u.contains("punish") || u.contains("captcha") || u.contains("x5sec") { return true; }
    false
}

// ── op: pause ───────────────────────────────────────────────────────────────
// A human-scale delay the workflow can put between two clicks (`with.ms`), so a
// directed tour does not machine-gun a site into its anti-bot wall.
fn pause(host, ctx) {
    let ms = _int(ctx, "ms", 1200);
    sleep(ms.to_float() / 1000.0);
    #{ paused_ms: ms }
}

// ── op: guard ───────────────────────────────────────────────────────────────
// Human patience in front of an anti-bot wall. Taobao answers a too-fast client
// with a 滑块 / 验证码 overlay ("访问太频繁"); the fix is not to solve it but to
// stop and wait the way a person would, then try the page again. Loops until the
// page is clean or `with.wait_ms` (default 90s) runs out and returns whether a
// wall is still up, so the workflow can keep waiting or give up out loud.
fn guard(host, ctx) {
    let target = _arg(ctx, "target_id");
    if target == "" { throw "jev-planner.guard: with.target_id is required"; }
    let budget_ms = _int(ctx, "wait_ms", 90000);
    let url = _arg(ctx, "url");
    // Try to close a challenge we *can* close (the baxia dialog's X) before
    // spending the wall budget on waiting: on Taobao the X clears the box and the
    // page is usable again immediately, and only an un-dismissable challenge is
    // worth backing off from.
    let pre = #{};
    try { pre = host.browser_evaluate(target, host.js("dismiss.js"), false); } catch (e) { pre = #{}; }
    let attempts = 0;
    let waited = 0;
    let wall = false;
    loop {
        let d = host.browser_evaluate(target, host.js("wall.js"), false);
        wall = d["wall"] == true;
        if !wall { break; }
        if url == "" { url = "" + d["url"]; }
        if waited >= budget_ms { break; }
        // each retry waits longer, the way a person who keeps seeing the wall
        // backs further off instead of hammering it
        let cool = 6000 + attempts * 6000;
        host.log("guard: wall up (attempt " + (attempts + 1) + "), cooling down " + cool + "ms");
        sleep(cool.to_float() / 1000.0);
        waited += cool;
        attempts += 1;
        let stuck_url = url.contains("punish") || url.contains("x5sec") || url.contains("_____tmd_____");
        if url != "" && !stuck_url {
            host.browser_navigate(target, url);
            _settle(host, target, _origin(url), 800);
        }
    }
    #{ wall: wall, attempts: attempts, waited_ms: waited, url: url,
       pre_dismissed: pre["closed"], pre_blocking: pre["still_blocking"] }
}

fn sweep(host, ctx) {
    let target = _arg(ctx, "target_id");
    if target == "" { throw "jev-planner.sweep: with.target_id is required"; }
    let list_url = _arg(ctx, "list_url");
    if list_url == "" { list_url = _arg(ctx, "url"); }
    let n = _int(ctx, "n", 20);
    let results = _arg(ctx, "results");
    if results == "" { results = "sweep.jsonl"; }

    // Read the list first; if Taobao answers with a wall (no cards), cool down
    // and re-open the list a few times rather than sweeping an empty page.
    let list = [];
    let total = 0;
    let la = 0;
    while la < 4 && total == 0 {
        if list_url != "" {
            host.browser_navigate(target, list_url);
            _settle(host, target, _origin(list_url), 600);
        }
        let cards = host.browser_evaluate(target, host.js("cards.js"), false);
        list = cards["cards"];
        if type_of(list) != "array" { list = []; }
        total = list.len();
        if total == 0 {
            host.log("sweep: 0 cards (wall?), cooldown " + (la + 1));
            sleep(6.0 + la * 2.5);
            la += 1;
        }
    }
    let want = if n < total { n } else { total };
    host.log("sweep: " + total + " cards on the list, opening " + want);

    let items = [];
    let lines = "";
    let i = 0;
    while i < want {
        let card = list[i];
        let href = card["href"];
        let rec = #{
            i: i,
            href: href,
            list_title: card["title"],
            list_price: card["price"],
            title: card["title"],
            price: card["price"],
            server: false,
            ecc_reg: false,
            single: false,
            stick: false,
            kit: false,
            capacity_gb: 0,
            capacity_min_gb: 0,
            freq_mhz: 0,
            multi_sku: false,
            score: -100,
            wall: false
        };
        let d = #{};
        let got = false;
        let attempt = 0;
        while attempt < 4 && !got {
            try {
                // Human-ish pace: 2.2 s .. 4.5 s between page opens. A person
                // comparing 20 listings does not machine-gun them, and the longer
                // gap is what keeps "访问太频繁" from showing up mid-sweep.
                sleep(2.2 + ((i + attempt) % 6) * 0.45);
                host.browser_navigate(target, href);
                _settle(host, target, _origin(href), 600);
                d = host.browser_evaluate(target, host.js("item.js"), false);
                if _wall(d) {
                    host.log("sweep: wall on item " + (i + 1) + ", cooldown " + (attempt + 1));
                    sleep(6.0 + attempt * 2.5);
                    attempt += 1;
                } else {
                    got = true;
                }
            } catch (e) {
                rec["error"] = "" + e;
                attempt += 1;
                sleep(2.0);
            }
        }
        if got {
            rec["title"] = d["title"];
            rec["price"] = d["price"];
            rec["server"] = d["server"];
            rec["ecc_reg"] = d["ecc_reg"];
            rec["single"] = d["single"];
            rec["stick"] = d["stick"];
            rec["kit"] = d["kit"];
            rec["capacity_gb"] = d["capacity_gb"];
            rec["capacity_min_gb"] = d["capacity_min_gb"];
            rec["multi_sku"] = d["multi_sku"];
            rec["freq_mhz"] = d["freq_mhz"];
            rec["score"] = d["score"];
            rec["spec"] = d["spec_excerpt"];
            rec["detail_url"] = d["url"];
        } else {
            rec["wall"] = true;
            rec["score"] = -100;
        }
        items.push(rec);
        lines += host.json_stringify(rec) + "\n";
        host.log("sweep[" + (i + 1) + "/" + want + "] score=" + rec["score"]
            + " server=" + rec["server"] + " stick=" + rec["stick"]
            + " multi_sku=" + rec["multi_sku"]
            + " " + rec["title"]);
        i += 1;
    }
    if lines != "" { try { host.write_file(results, lines); } catch (e) { host.log("sweep: journal write failed: " + e); } }

    // Rank: highest score wins; ties prefer the capacity closest to a 32 GB
    // single DIMM, then the lower price. A walled/errored row is -100, so it can
    // never win.
    let best_i = 0;
    let k = 1;
    while k < items.len() {
        let s = _num(items[k]["score"]);
        let bs = _num(items[best_i]["score"]);
        let better = s > bs;
        if s == bs {
            let dk = _abs(_num(items[k]["capacity_gb"]) - 32.0);
            let db = _abs(_num(items[best_i]["capacity_gb"]) - 32.0);
            if dk < db { better = true; }
            else if dk == db {
                let p = _num(items[k]["price"]);
                let bp = _num(items[best_i]["price"]);
                if p >= 0.0 && bp >= 0.0 && p < bp { better = true; }
            }
        }
        if better { best_i = k; }
        k += 1;
    }
    let winner = if items.len() > 0 { items[best_i] } else { #{} };
    host.log("sweep: best = " + winner["title"] + " (score=" + winner["score"] + ")");
    #{
        total_cards: total,
        opened: items.len(),
        best_index: best_i,
        best_href: winner["href"],
        best_title: winner["title"],
        best_price: winner["price"],
        best_score: winner["score"],
        best_spec: winner["spec"],
        items: items
    }
}

// ── op: reset ───────────────────────────────────────────────────────────────
fn reset(host, ctx) {
    let target = _arg(ctx, "target_id");
    if target == "" { throw "jev-planner.reset: with.target_id is required"; }
    host.browser_evaluate(target,
        `try { localStorage.removeItem('__laya_jev_round'); localStorage.removeItem('__laya_jev_seen'); } catch (e) {} true`,
        false)
}
