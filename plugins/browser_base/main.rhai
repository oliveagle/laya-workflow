// browser_base — generic single-page browser primitives for Laya workflows.
//
// Embedded in the Laya binary (`builtin()`), so any checkout can drive a real
// Chrome page via a `chrome_cdp` capability without shipping any site logic.
//
// Ops (pick one via the capability `op` or `with.op`):
//   open       open `with.url`, wait until it is *actually* loaded, report
//              {url, title, ready, sample}.  Self-contained by default (page
//              closed before returning); pass `with.keep_open=true` to keep the
//              target usable by later nodes (target_id is returned).
//   evaluate   run `with.expression` (JS) on a page and return its result.
//              Self-contained: opens `with.url` + waits, unless `with.target_id`
//              names a page already kept open by a previous node.
//   wait_htmx  open a page and wait until `window.htmx` is defined; report
//              {htmx_loaded, url, title, text}.
//   assert     run `with.expression` and require every `with.checks` key to
//              equal its expected value in the result; throws a deterministic
//              FAIL otherwise (a node that throws fails the workflow, so a
//              spec can branch on PASS/FAIL without an LLM).
//
// Robustness layers (the reason this exists as a shared base):
//   1. `_wait_loaded` polls short evaluates until the target URL matches and
//      readyState is complete.  `BrowserHost::wait_ready` skips the URL check
//      and can return on the pre-navigation `about:blank` target, so a long
//      evaluate issued right after races the real navigation commit (CDP
//      `-32000 Inspected target navigated or closed`).  Polling absorbs that
//      window deterministically.
//   2. `_retry` wraps each self-contained op (up to 3 attempts, fresh page per
//      attempt) as a secondary net for any other transient CDP failure.
//
// No site-specific assertions live here: consumers pass url / expression /
// checks via `with` and keep their own expectations.

fn run(host, ctx) {
    throw "browser_base: pick an op (open | evaluate | wait_htmx | assert)";
}

// ── argument helpers ────────────────────────────────────────────────────────
fn _arg(ctx, key) {
    let w = ctx["with"];
    if type_of(w) == "map" && w.contains(key) { return w[key]; }
    ""
}

fn _flag(ctx, key) {
    let w = ctx["with"];
    if type_of(w) == "map" && w.contains(key) { return w[key] == true; }
    false
}

// JS helper embedded in htmx waits: resolves true once window.htmx is defined.
fn wait_htmx_helper() {
    "const waitHtmx = async () => {"
    + "    const dl = Date.now() + 15000;"
    + "    while (Date.now() < dl) {"
    + "        if (typeof window.htmx === \"object\") return true;"
    + "        await new Promise(r => setTimeout(r, 100));"
    + "    }"
    + "    return false;"
    + "};"
}

// Best-effort close of an owned page; never masks the primary error.
fn _safe_release(host, target) {
    try {
        host.browser_release(target);
    } catch (e) {
        host.log("browser_base: release best-effort: " + e);
    }
}

// Deterministic navigation wait: poll short evaluates until the target's URL
// matches `prefix` and readyState is complete.  See the module doc for why this
// cannot be `browser_wait_ready` alone.  100 * 0.15s = 15s worst case.
fn _wait_loaded(host, target, prefix) {
    let tries = 0;
    while tries < 100 {
        tries += 1;
        try {
            let info = host.browser_evaluate(target, `({ url: location.href, ready: document.readyState, title: document.title })`, false);
            let url = info["url"];
            if url.starts_with(prefix) && info["ready"] == "complete" {
                return info;
            }
        } catch (e) {
            // navigation commit in progress — keep polling
        }
        sleep(0.15);
    }
    throw "wait_loaded: target never reached " + prefix + " (tries=" + tries + ")";
}

// ── retry dispatcher ────────────────────────────────────────────────────────
fn _retry(host, ctx, opname) {
    // Returns the op result map; each self-contained attempt is a fresh page,
    // absorbing the engine's transient `Inspected target navigated or closed`
    // race.  `target_id` reuse is not retried on a new page (same target), so
    // a stale target fails fast instead of burning three attempts.
    let attempt = 0;
    let last_err = "";
    while attempt < 3 {
        attempt += 1;
        try {
            return _attempt(host, ctx, opname);
        } catch (e) {
            last_err = "" + e;
            host.log("browser_base " + opname + ": attempt " + attempt + " failed: " + last_err);
            if attempt >= 3 { break; }
            sleep(0.4);   // let a transient Chrome/CDP condition settle
        }
    }
    throw last_err;
}

fn _attempt(host, ctx, opname) {
    if opname == "open" { return _attempt_open(host, ctx); }
    if opname == "evaluate" { return _attempt_evaluate(host, ctx); }
    if opname == "wait_htmx" { return _attempt_wait_htmx(host, ctx); }
    if opname == "assert" { return _attempt_assert(host, ctx); }
    throw "browser_base: unknown op " + opname;
}

// ── op: open — open a URL and wait until it is actually loaded ──────────────
fn open(host, ctx) {
    _retry(host, ctx, "open")
}

fn _attempt_open(host, ctx) {
    let url = _arg(ctx, "url");
    if url == "" { throw "browser_base.open: with.url is required"; }
    let prefix = _arg(ctx, "prefix");
    if prefix == "" { prefix = url; }
    let keep = _flag(ctx, "keep_open");
    let target = host.browser_open(url);
    let info = #{};
    try {
        info = _wait_loaded(host, target, prefix);
    } catch (e) {
        _safe_release(host, target);
        throw "browser_base.open: " + e;
    }
    let sample = "";
    try {
        sample = host.browser_evaluate(target, `document.body ? document.body.innerText.slice(0, 300) : ""`, false);
    } catch (e) {
        sample = "";
    }
    if keep == true {
        return #{
            target_id: target,
            url: info["url"], title: info["title"], ready: info["ready"],
            sample: sample
        };
    }
    _safe_release(host, target);
    #{
        url: info["url"], title: info["title"], ready: info["ready"],
        sample: sample
    }
}

// ── op: evaluate — run arbitrary JS on a page ───────────────────────────────
fn evaluate(host, ctx) {
    _retry(host, ctx, "evaluate")
}

fn _attempt_evaluate(host, ctx) {
    let expression = _arg(ctx, "expression");
    if expression == "" { throw "browser_base.evaluate: with.expression is required"; }
    let target_id = _arg(ctx, "target_id");
    let owned = false;
    let target = target_id;
    if target_id == "" {
        let url = _arg(ctx, "url");
        if url == "" { throw "browser_base.evaluate: with.url (or with.target_id) is required"; }
        let prefix = _arg(ctx, "prefix");
        if prefix == "" { prefix = url; }
        target = host.browser_open(url);
        owned = true;
        try {
            _wait_loaded(host, target, prefix);
        } catch (e) {
            _safe_release(host, target);
            throw "browser_base.evaluate: " + e;
        }
    }
    let await_promise = true;
    let w = ctx["with"];
    if type_of(w) == "map" && w.contains("await_promise") { await_promise = w["await_promise"] == true; }
    let out = #{};
    try {
        out = host.browser_evaluate(target, expression, await_promise);
    } catch (e) {
        if owned { _safe_release(host, target); }
        throw "browser_base.evaluate: " + e;
    }
    if owned { _safe_release(host, target); }
    out
}

// ── op: wait_htmx — open a page and wait until window.htmx is defined ───────
fn wait_htmx(host, ctx) {
    _retry(host, ctx, "wait_htmx")
}

fn _attempt_wait_htmx(host, ctx) {
    let target_id = _arg(ctx, "target_id");
    let owned = false;
    let target = target_id;
    if target_id == "" {
        let url = _arg(ctx, "url");
        if url == "" { throw "browser_base.wait_htmx: with.url (or with.target_id) is required"; }
        let prefix = _arg(ctx, "prefix");
        if prefix == "" { prefix = url; }
        target = host.browser_open(url);
        owned = true;
        try {
            _wait_loaded(host, target, prefix);
        } catch (e) {
            _safe_release(host, target);
            throw "browser_base.wait_htmx: " + e;
        }
    }
    let js = `(async () => {
        ${wait_htmx_helper()}
        const htmx_loaded = await waitHtmx();
        return {
            htmx_loaded: htmx_loaded,
            url: location.href,
            title: document.title,
            text: document.body ? document.body.innerText.slice(0, 400) : ""
        };
    })()`;
    let out = #{};
    try {
        out = host.browser_evaluate(target, js, true);
    } catch (e) {
        if owned { _safe_release(host, target); }
        throw "browser_base.wait_htmx: " + e;
    }
    if owned { _safe_release(host, target); }
    out
}

// ── op: assert — evaluate and require every check to hold ───────────────────
fn assert(host, ctx) {
    _retry(host, ctx, "assert")
}

fn _attempt_assert(host, ctx) {
    let res = _attempt_evaluate(host, ctx);
    let checks = ctx["with"]["checks"];
    if type_of(checks) != "map" {
        throw "browser_base.assert: with.checks must be a map of key -> expected value";
    }
    let failed = [];
    for key in checks.keys() {
        let expected = checks[key];
        let actual = if type_of(res) == "map" && res.contains(key) { res[key] } else { () };
        if actual != expected {
            failed.push(key);
        }
    }
    if failed.len() > 0 {
        throw "browser_base.assert: FAIL — " + host.json_stringify(checks)
            + " (missing/mismatched: " + host.json_stringify(failed) + ")";
    }
    if type_of(res) == "map" {
        res["pass"] = true;
    }
    res
}
