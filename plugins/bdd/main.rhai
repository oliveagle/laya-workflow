// bdd — the Given/When/Then vocabulary, as deterministic ops.
//
// browser_base gives you a page and an `assert` over a `checks` map you build
// yourself. This plugin is the layer above it: the step *meanings* live here,
// not in whatever compiled the document. That split is the point - the
// assertion vocabulary is reusable from any spec, BDD-compiled or not, and
// there is exactly one definition of what "the page title contains X" means.
//
// Ops (pick one via the capability `op` or `with.op`):
//   open       open `with.url` and wait until it is *actually* loaded, then
//              return {target_id, url, title, ready}. USE ONLY INSIDE ONE CALL:
//              the engine closes every tab a plugin opened as soon as that
//              plugin call returns (src/capability/plugin.rs, "the engine
//              closes what the plugin opened"), so a multi-step scenario must
//              let the chrome_cdp capability own the page. `dsl/browser/
//              browser_base_probe.json` shows the supported shape.
//   navigate   `with.target_id` -> `with.url`, then wait for the load.
//   wait_for   poll until `with.selector` matches, or `with.timeout_ms` (default
//              15000) elapses. Reports what it waited for on failure.
//   evaluate   run `with.expression` on `with.target_id`.
//   assert     the Then family. `with.assertion` names the check and
//              `with.value` the operand; a mismatch throws.
//   release    close an owned target. Best effort, never masks a real error.
//
// The assertion names, and the `with` each one takes:
//   title_contains  value: substring   the page title contains it
//   url_contains    value: substring   location.href contains it
//   visible         value: selector   the element exists and is rendered
//   absent          value: selector   the element does not exist
//   is_true         value: expression the expression is truthy
//   is_false        value: expression the expression is falsy
//   equals          value: expression + `with.expected` (JSON literal)
//   contains        value: expression + `with.expected` (string)
//   equals_text     value: expression + `with.expected` (string, trimmed)
//
// Every assert evaluates an expression shaped `{ ok: <bool>, actual: <value> }`
// and then throws, on failure, with the actual value in the message. The
// parentheses around the object literal are load-bearing: the host evaluates
// `expression` as an expression, and a bare `{ ok: ... }` parses as a block
// statement with a label.

fn run(host, ctx) {
    let op = _arg(ctx, "op");
    if op == "" { op = _default_op(ctx); }
    if op == "open" { return _op_open(host, ctx); }
    if op == "navigate" { return _op_navigate(host, ctx); }
    if op == "wait_for" { return _op_wait_for(host, ctx); }
    if op == "evaluate" { return _op_evaluate(host, ctx); }
    if op == "assert" { return _op_assert(host, ctx); }
    if op == "release" { return _op_release(host, ctx); }
    throw "bdd: unknown op '" + op + "' (open | navigate | wait_for | evaluate | assert | release)";
}

// ── argument helpers ────────────────────────────────────────────────────────
fn _arg(ctx, key) {
    let w = ctx["with"];
    if type_of(w) == "map" && w.contains(key) { return w[key]; }
    ""
}

// Absent-vs-present matters for `expected`: `equals ... 0` must not read as
// "not supplied", so this returns () rather than "".
fn _arg_opt(ctx, key) {
    let w = ctx["with"];
    if type_of(w) == "map" && w.contains(key) { return w[key]; }
    ()
}

fn _flag(ctx, key) {
    let w = ctx["with"];
    if type_of(w) == "map" && w.contains(key) { return w[key] == true; }
    false
}

fn _num(ctx, key, dflt) {
    let v = _arg_opt(ctx, key);
    if type_of(v) == "int" { return v; }
    if type_of(v) == "float" { return v; }
    dflt
}

// A JS string literal for an arbitrary string, quotes and newlines included.
fn _js_str(host, s) {
    host.json_stringify("" + s)
}

fn _require_target(ctx, who) {
    let t = _arg(ctx, "target_id");
    if t == "" {
        throw "bdd." + who + ": with.target_id is required - an earlier step must open a page first";
    }
    t
}

fn _default_op(ctx) {
    // A capability may pin the op itself; `with.op` is the per-call override.
    let cap = ctx["capability"];
    if type_of(cap) == "map" && cap.contains("op") { return "" + cap["op"]; }
    "open"
}

// ── deterministic navigation wait ───────────────────────────────────────────
// Poll short evaluates until the target's URL matches `prefix` and readyState
// is complete. `browser_wait_ready` skips the URL check and can return on the
// pre-navigation about:blank target, so a long evaluate issued right after
// races the real navigation commit.
fn _wait_loaded(host, target, prefix) {
    let tries = 0;
    while tries < 100 {
        tries += 1;
        try {
            let info = host.browser_evaluate(target, "({ url: location.href, ready: document.readyState, title: document.title })", false);
            if info["url"].starts_with(prefix) && info["ready"] == "complete" { return info; }
        } catch (e) {
            // navigation commit in progress - keep polling
        }
        sleep(0.15);
    }
    throw "bdd: target never reached " + prefix + " (tries=" + tries + ")";
}

fn _safe_release(host, target) {
    if target == "" { return; }
    try {
        host.browser_release(target);
    } catch (e) {
        host.log("bdd: release best-effort: " + e);
    }
}

// ── op: open ────────────────────────────────────────────────────────────────
fn _op_open(host, ctx) {
    let url = _arg(ctx, "url");
    if url == "" { throw "bdd.open: with.url is required"; }
    let prefix = _arg(ctx, "prefix");
    if prefix == "" { prefix = url; }
    let target = host.browser_open(url);
    let info = #{};
    try {
        info = _wait_loaded(host, target, prefix);
    } catch (e) {
        _safe_release(host, target);
        throw "bdd.open: " + e;
    }
    // A page that reports itself hidden will silently drop synthetic input, so
    // say so here rather than letting a later click fail with no explanation.
    #{
        target_id: target, url: info["url"], title: info["title"], ready: info["ready"]
    }
}

// ── op: navigate ────────────────────────────────────────────────────────────
fn _op_navigate(host, ctx) {
    let target = _require_target(ctx, "navigate");
    let url = _arg(ctx, "url");
    if url == "" { throw "bdd.navigate: with.url is required"; }
    host.browser_navigate(target, url);
    let info = _wait_loaded(host, target, url);
    #{ target_id: target, url: info["url"], title: info["title"], ready: info["ready"] }
}

// ── op: wait_for ────────────────────────────────────────────────────────────
fn _op_wait_for(host, ctx) {
    let target = _require_target(ctx, "wait_for");
    let selector = _arg(ctx, "selector");
    if selector == "" { throw "bdd.wait_for: with.selector is required"; }
    // Milliseconds on both sides. The first version divided the budget into
    // seconds but counted `waited` in milliseconds, so `100 < 15.0` was already
    // false and the loop gave up after a single poll while claiming a 15s
    // timeout. A wait that lies about how long it waited is worse than no wait.
    let budget = _num(ctx, "timeout_ms", 15000);
    let js = "!!document.querySelector(" + _js_str(host, selector) + ")";
    let waited = 0;
    // Keep the last probe error. "timed out" alone is a dead end: the usual
    // cause is not a slow page but an unreachable one (a closed tab, a dead
    // target), and that is visible only in the error the probe threw.
    let last = "";
    let polls = 0;
    while waited < budget {
        polls += 1;
        let found = false;
        try {
            found = host.browser_evaluate(target, js, false) == true;
        } catch (e) {
            last = "" + e;
        }
        if found {
            return #{ target_id: target, ready: true, selector: selector, waited_ms: waited, polls: polls };
        }
        sleep(0.1);
        waited += 100;
    }
    let why = if last == "" { "" } else { "; last probe error: " + last };
    throw "bdd.wait_for: timed out after " + budget + "ms (" + polls + " polls) waiting for " + selector + why;
}

// ── op: evaluate ────────────────────────────────────────────────────────────
fn _op_evaluate(host, ctx) {
    let target = _require_target(ctx, "evaluate");
    let expression = _arg(ctx, "expression");
    if expression == "" { throw "bdd.evaluate: with.expression is required"; }
    let out = host.browser_evaluate(target, expression, true);
    #{ target_id: target, value: out }
}

// ── op: release ─────────────────────────────────────────────────────────────
fn _op_release(host, ctx) {
    let target = _arg(ctx, "target_id");
    _safe_release(host, target);
    #{ released: true, target_id: target }
}

// ── the Then family ─────────────────────────────────────────────────────────
// Build the page-side expression for an assertion name. Each returns an object
// literal with `ok` and `actual`; `_op_assert` is the only thing that judges it.
fn _expr_for(host, ctx, name) {
    let value = _arg(ctx, "value");
    if name == "title_contains" {
        return "(function(){ var t = document.title || '';"
             + " return { ok: t.indexOf(" + _js_str(host, value) + ") !== -1, actual: t }; })()";
    }
    if name == "url_contains" {
        return "(function(){ var u = location.href || '';"
             + " return { ok: u.indexOf(" + _js_str(host, value) + ") !== -1, actual: u }; })()";
    }
    if name == "visible" {
        return "(function(){ var e = document.querySelector(" + _js_str(host, value) + ");"
             + " if (!e) return { ok: false, actual: 'no such element' };"
             + " var r = e.getBoundingClientRect();"
             + " var vis = (r.width > 0 || r.height > 0)"
             + "   && getComputedStyle(e).visibility !== 'hidden'"
             + "   && getComputedStyle(e).display !== 'none';"
             + " return { ok: vis, actual: e.tagName.toLowerCase()"
             + "   + (vis ? ' visible' : ' not visible') }; })()";
    }
    if name == "absent" {
        return "(function(){ var e = document.querySelector(" + _js_str(host, value) + ");"
             + " return { ok: e === null, actual: e ? e.tagName.toLowerCase() : null }; })()";
    }
    if name == "is_true" {
        return "({ ok: !!(eval(" + _js_str(host, value) + ")), actual: eval(" + _js_str(host, value) + ") })";
    }
    if name == "is_false" {
        return "({ ok: !(eval(" + _js_str(host, value) + ")), actual: eval(" + _js_str(host, value) + ") })";
    }
    if name == "equals" {
        let expected = _arg_opt(ctx, "expected");
        if type_of(expected) == "()" {
            throw "bdd.assert: assertion 'equals' needs with.expected (a JSON literal)";
        }
        return "({ ok: (eval(" + _js_str(host, value) + ")) === " + host.json_stringify(expected)
             + ", actual: eval(" + _js_str(host, value) + ") })";
    }
    if name == "contains" || name == "equals_text" {
        let expected = _arg_opt(ctx, "expected");
        if type_of(expected) == "()" {
            throw "bdd.assert: assertion '" + name + "' needs with.expected (a string)";
        }
        if name == "equals_text" {
            return "({ ok: String(eval(" + _js_str(host, value) + ")).trim() === " + host.json_stringify(expected)
                 + ", actual: String(eval(" + _js_str(host, value) + ")).trim() })";
        }
        return "({ ok: String(eval(" + _js_str(host, value) + ")).indexOf(" + host.json_stringify(expected)
             + ") !== -1, actual: String(eval(" + _js_str(host, value) + ")) })";
    }
    throw "bdd.assert: unknown assertion '" + name
        + "' (title_contains | url_contains | visible | absent | is_true | is_false | equals | contains | equals_text)";
}

fn _op_assert(host, ctx) {
    let target = _require_target(ctx, "assert");
    let name = _arg(ctx, "assertion");
    if name == "" { throw "bdd.assert: with.assertion is required"; }
    let value = _arg(ctx, "value");
    if value == "" {
        throw "bdd.assert: with.value is required for assertion '" + name + "'";
    }
    let js = _expr_for(host, ctx, name);

    // One attempt by default. A deterministic assertion that is going to fail
    // should say so quickly; the way to absorb a page that is still settling
    // is an explicit `wait_for` before it, which is visible in the document.
    // Opt in to retries when a check is genuinely racy.
    let attempts = _num(ctx, "attempts", 1);
    let last = "";
    let n = 0;
    while n < attempts {
        n += 1;
        let res = ();
        try {
            res = host.browser_evaluate(target, js, false);
        } catch (e) {
            last = "bdd.assert: " + name + ": evaluation failed: " + e;
            if n >= attempts { break; }
            sleep(0.3);
            continue;
        }
        let actual = if type_of(res) == "map" && res.contains("actual") { res["actual"] } else { () };
        let ok = type_of(res) == "map" && res.contains("ok") && res["ok"] == true;
        if ok {
            return #{ target_id: target, pass: true, assertion: name, actual: actual };
        }
        last = "bdd.assert: FAIL " + name + " with value " + host.json_stringify(value)
             + " | actual: " + host.json_stringify(actual);
        if n >= attempts { break; }
        sleep(0.3);
    }
    throw last;
}
