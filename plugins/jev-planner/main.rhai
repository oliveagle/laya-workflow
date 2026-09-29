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
//   reset   clear this origin's round counter and "already followed" memory
//
// `observe` / `find` are the *directed* half: where `step` lets the page-side
// policy roam (scroll a page through, then follow a content link), `find` says
// exactly what to act on — a role + label + optional nth — and returns the same
// Jev decision shape, so an `agent_step` node executes it unchanged. `find`
// matches against the laya-browser extension's numbered snapshot, so the number
// in the decision is the number the extension drew on the page.

fn run(host, ctx) {
    throw "jev-planner: pick an op (visit | step | observe | find | dismiss | reset)";
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
            if wanted && url != "about:blank" && info["ready"] == "complete" {
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

// ── op: reset ───────────────────────────────────────────────────────────────
fn reset(host, ctx) {
    let target = _arg(ctx, "target_id");
    if target == "" { throw "jev-planner.reset: with.target_id is required"; }
    host.browser_evaluate(target,
        `try { localStorage.removeItem('__laya_jev_round'); localStorage.removeItem('__laya_jev_seen'); } catch (e) {} true`,
        false)
}
