// ─────────────────────────────────────────────────────────────────────────────
// cua — the decision layer over Cua Driver (https://github.com/trycua/cua).
//
// Cua Driver drives the *native* desktop: it walks the macOS accessibility
// tree, takes real screenshots, and posts real input events. It ships 58 tools
// behind one CLI verb:
//
//     cua-driver call <tool> --args '<json>'
//
// That verb is an `exec` capability, and laya-workflow's Rhai host has no exec
// — the only way out of a plugin is the table in `docs/plugins.md`. So this file
// cannot call Cua. It does the half that actually needs judgement:
//
//   * decide WHICH tool an intent means, and with what arguments;
//   * resolve a target to the exact handle Cua itself handed out, so a click
//     lands on a bound element instead of a guessed pixel;
//   * refuse the calls that can lose a user's work or read a user's clipboard;
//   * compress Cua's very large results (base64 PNGs, whole AX trees) into
//     something that fits a run state.
//
// An `exec` capability then runs the one call this plugin returns. That is the
// same split `plugins/jev-planner` uses with CDP, and for the same reason: the
// decision is policy, the effect is the engine's.
//
// ── why addressing is the hard part ─────────────────────────────────────────
//
// Cua never wants a screen coordinate typed by a caller. It wants one of:
//
//     { window_id }                        from list_windows
//     { pid, window_id }                   from list_windows + list_apps
//     { pid, element_token }               from get_window_state's elements[]
//
// `element_token` is an opaque per-snapshot handle — it is only valid for the
// snapshot that issued it, which is why `bind` will not hand back a token it
// found in a *different* observation than the one it was asked about. Clicking
// a remembered token silently hits whatever now occupies that slot; the whole
// safety story of the driver rests on callers re-binding every turn.
//
// So `bind` scores a target description against the AX elements in the
// *current* observation and returns the handle, and `plan` refuses to emit a
// click whose target it could not bind. An unbound click is not returned with
// zero coordinates — it is an error, because zero-zero is a real place on a
// desktop and a silent click there is worse than no click.
//
// ── ops (pick one with `with.op`) ───────────────────────────────────────────
//
//   tools      the 58-tool taxonomy with each tool's risk level
//   plan       one intent -> the NEXT single Cua call (the loop's unit of work)
//   sequence   one intent -> the whole ordered call plan
//   bind       a target description -> pid / window_id / element_token
//   inspect    a raw Cua result -> compact JSON, images stripped
//
// ── the risk table ──────────────────────────────────────────────────────────
//
//   observe  read-only. Changes nothing; the desktop looks the same after.
//   act      moves the UI, reversibly. A click, a keystroke, a scroll.
//   danger   loses work, reads private state, or installs software.
//
// `danger` is refused unless `allow_danger` is set, because the failure mode
// without it is not a crash — it is a *successful* `kill_app` on the user's
// editor, or a clipboard read that leaves a password in a run log. A refusal
// returns a reason the caller can show, never a silent no-op.
//
// The 58 names below are the live `cua-driver list-tools` output of 0.31.0,
// partitioned by hand. A tool the driver adds later is not in the table, and
// `risk_of` calls that "unknown" rather than "safe": a missing entry must fail
// closed, because the whole gate is the reason a caller can trust `act`.
// ─────────────────────────────────────────────────────────────────────────────

fn run(host, ctx) {
    throw "cua: pick an op (tools | plan | sequence | bind | inspect)";
}

// ── value helpers ───────────────────────────────────────────────────────────
//
// Specs interpolate `${state.x}`, so a number arrives as a string half the time
// and a missing key arrives as `()`. Every reader takes all three shapes
// rather than trusting one.

fn missing(v) {
    type_of(v) == "()"
}

// Note: no json_stringify here. That host fn takes the Host as its first
// parameter, so calling it bare would mean threading `host` through every
// reader in the file just to render a value that is a string nearly
// everywhere. The two places a non-string genuinely must become JSON — the
// argv payload, and the driver's own stdout — go through `jstr` / `as_json`,
// which do take a host. A map reaching `ostr` yields "" rather than a
// fabricated rendering.
fn ostr(v) {
    if missing(v) { return ""; }
    if type_of(v) == "string" { return v; }
    if type_of(v) == "bool" { if v { return "true"; } return "false"; }
    if type_of(v) == "i64" || type_of(v) == "f64" { return "" + v; }
    return "";
}

// The one place a value must become real JSON text: the argv payload.
fn jstr(host, v) {
    host.json_stringify(v)
}

// Trim without `trim()`: the borrowed-slice methods do not survive a `return`
// in this Rhai build (docs/plugins.md), so the edges are cut with sub_string.
fn trimmed(s) {
    let t = "" + s;
    let n = t.len();
    let a = 0;
    while a < n {
        let c = t.sub_string(a, 1);
        if c == " " || c == "\t" || c == "\n" || c == "\r" { a += 1; } else { break; }
    }
    let b = n;
    while b > a {
        let c = t.sub_string(b - 1, 1);
        if c == " " || c == "\t" || c == "\n" || c == "\r" { b -= 1; } else { break; }
    }
    t.sub_string(a, b - a)
}

fn norm(s) {
    trimmed("" + s).to_lower()
}

// Coerce to a number. Unparseable is 0, so callers that care check `num_ok`
// first: a target pid of 0 is never a real process, and letting "absent" read as
// "zero" turns a missing pid into a click aimed at pid 0.
fn num(v) {
    if missing(v) { return 0.0; }
    if type_of(v) == "i64" { return v * 1.0; }
    if type_of(v) == "f64" { return v; }
    if type_of(v) == "bool" { return 0.0; }
    let t = trimmed("" + v);
    if t == "" { return 0.0; }
    let i = 0;
    let dots = 0;
    let digits = 0;
    while i < t.len() {
        let c = t.sub_string(i, 1);
        if c == "-" || c == "+" {
            if i > 0 { return 0.0; }
        } else if c == "." {
            dots += 1;
            if dots > 1 { return 0.0; }
        } else if "0123456789".index_of(c) >= 0 {
            digits += 1;
        } else {
            return 0.0;
        }
        i += 1;
    }
    if digits == 0 { return 0.0; }
    t.to_float()
}

fn num_ok(v) {
    if missing(v) { return false; }
    if type_of(v) == "i64" || type_of(v) == "f64" { return true; }
    num(v) != 0.0
}

// Truncating cast. Rhai's own int() is not one in this engine build, so the
// fraction is subtracted rather than converted.
fn int_of(v) {
    let f = num(v);
    if f < 0.0 { return -int_of(-f); }
    int_of_str("" + f)
}

fn int_of_str(s) {
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

fn mget(m, k) {
    if type_of(m) != "map" { return (); }
    if !m.contains(k) { return (); }
    m[k]
}

fn mstr(m, k) {
    trimmed(ostr(mget(m, k)))
}

fn mnum(m, k) {
    num(mget(m, k))
}

fn flag(m, k, dflt) {
    let v = mget(m, k);
    if missing(v) { return dflt; }
    if type_of(v) == "bool" { return v; }
    let s = norm(v);
    if s == "true" || s == "1" || s == "yes" || s == "on" { return true; }
    if s == "false" || s == "0" || s == "no" || s == "off" { return false; }
    dflt
}

// "Is there a value here" — true for a non-empty string, and for a map or
// array whatever its size. An empty map still counts: it is a real answer from
// the driver (a window with no AX nodes), not a missing one.
fn present(v) {
    if missing(v) { return false; }
    if type_of(v) == "string" { return trimmed(v) != ""; }
    return true;
}

fn as_array(v) {
    if type_of(v) == "array" { return v; }
    []
}

// Cua's CLI prints one JSON document on stdout, and a driver-level refusal is
// plain text with a non-zero exit. Both arrive here as a string, so the reader
// accepts either and never raises: an unreadable result is a fact to report,
// not a reason to abort the run.
fn as_json(host, v) {
    if missing(v) { return #{}; }
    if type_of(v) == "map" || type_of(v) == "array" { return v; }
    let t = trimmed("" + v);
    if t == "" { return #{}; }
    try { host.json_parse(t) } catch { #{} }
}

fn with_of(ctx) {
    let w = ctx["with"];
    if type_of(w) == "map" { return w; }
    #{}
}

fn state_of(ctx) {
    let s = ctx["state"];
    if type_of(s) == "map" { return s; }
    #{}
}

// ── the tool taxonomy ───────────────────────────────────────────────────────

fn taxonomy() {
    let rows = [];
    // --- observe ---
    rows.push(#{ tool: "check_for_update", risk: "observe" });
    rows.push(#{ tool: "check_permissions", risk: "observe" });
    rows.push(#{ tool: "get_accessibility_tree", risk: "observe" });
    rows.push(#{ tool: "get_agent_cursor_state", risk: "observe" });
    rows.push(#{ tool: "get_browser_state", risk: "observe" });
    rows.push(#{ tool: "get_config", risk: "observe" });
    rows.push(#{ tool: "get_cursor_position", risk: "observe" });
    rows.push(#{ tool: "get_desktop_state", risk: "observe" });
    rows.push(#{ tool: "get_recording_state", risk: "observe" });
    rows.push(#{ tool: "get_screen_size", risk: "observe" });
    rows.push(#{ tool: "get_session", risk: "observe" });
    rows.push(#{ tool: "get_session_state", risk: "observe" });
    rows.push(#{ tool: "get_window_state", risk: "observe" });
    rows.push(#{ tool: "health_report", risk: "observe" });
    rows.push(#{ tool: "list_apps", risk: "observe" });
    rows.push(#{ tool: "list_sessions", risk: "observe" });
    rows.push(#{ tool: "list_windows", risk: "observe" });
    rows.push(#{ tool: "parse_visual_regions", risk: "observe" });
    rows.push(#{ tool: "verify_state", risk: "observe" });
    rows.push(#{ tool: "zoom", risk: "observe" });
    // --- act ---
    rows.push(#{ tool: "bring_to_front", risk: "act" });
    rows.push(#{ tool: "browser_click", risk: "act" });
    rows.push(#{ tool: "browser_dialog", risk: "act" });
    rows.push(#{ tool: "browser_download", risk: "act" });
    rows.push(#{ tool: "browser_navigate", risk: "act" });
    rows.push(#{ tool: "browser_pointer", risk: "act" });
    rows.push(#{ tool: "browser_prepare", risk: "act" });
    rows.push(#{ tool: "browser_set_input_files", risk: "act" });
    rows.push(#{ tool: "browser_type", risk: "act" });
    rows.push(#{ tool: "click", risk: "act" });
    rows.push(#{ tool: "double_click", risk: "act" });
    rows.push(#{ tool: "drag", risk: "act" });
    rows.push(#{ tool: "hotkey", risk: "act" });
    rows.push(#{ tool: "invoke_menu", risk: "act" });
    rows.push(#{ tool: "launch_app", risk: "act" });
    rows.push(#{ tool: "move_cursor", risk: "act" });
    rows.push(#{ tool: "page", risk: "act" });
    rows.push(#{ tool: "press_key", risk: "act" });
    rows.push(#{ tool: "right_click", risk: "act" });
    rows.push(#{ tool: "scroll", risk: "act" });
    rows.push(#{ tool: "set_agent_cursor_enabled", risk: "act" });
    rows.push(#{ tool: "set_agent_cursor_motion", risk: "act" });
    rows.push(#{ tool: "set_agent_cursor_theme", risk: "act" });
    rows.push(#{ tool: "set_value", risk: "act" });
    rows.push(#{ tool: "set_window_frame", risk: "act" });
    rows.push(#{ tool: "start_session", risk: "act" });
    rows.push(#{ tool: "type_text", risk: "act" });
    // --- danger ---
    rows.push(#{ tool: "clipboard_read", risk: "danger" });
    rows.push(#{ tool: "clipboard_write", risk: "danger" });
    rows.push(#{ tool: "end_session", risk: "danger" });
    rows.push(#{ tool: "escalate_session", risk: "danger" });
    rows.push(#{ tool: "install_extension", risk: "danger" });
    rows.push(#{ tool: "install_ffmpeg", risk: "danger" });
    rows.push(#{ tool: "kill_app", risk: "danger" });
    rows.push(#{ tool: "replay_trajectory", risk: "danger" });
    rows.push(#{ tool: "set_config", risk: "danger" });
    rows.push(#{ tool: "start_recording", risk: "danger" });
    rows.push(#{ tool: "stop_recording", risk: "danger" });
    return rows;
}

// "unknown" is a real answer, not a fallback to "safe": see the note above
// `run`. A caller that wants to act on an unrecognised tool has to say so.
fn risk_of(tool) {
    let t = norm(tool);
    for row in taxonomy() {
        if norm(row["tool"]) == t { return row["risk"]; }
    }
    "unknown"
}

fn known_tool(tool) {
    risk_of(tool) != "unknown"
}

// A brief reason per danger tool, so a refusal says *why* rather than just
// "no". The caller usually shows this text to a human deciding whether to
// re-run with allow_danger.
fn danger_reason(tool) {
    let t = norm(tool);
    if t == "kill_app" { return "force-terminates a process (kill -9); unsaved work is not recoverable"; }
    if t == "clipboard_read" { return "reads the user's clipboard, which is private by default"; }
    if t == "clipboard_write" { return "replaces the user's clipboard, destroying whatever was there"; }
    if t == "start_recording" { return "starts capturing the screen and input to disk"; }
    if t == "stop_recording" { return "ends a screen recording and writes it out"; }
    if t == "replay_trajectory" { return "re-runs recorded input against the live desktop"; }
    if t == "set_config" { return "mutates Cua Driver's own configuration"; }
    if t == "install_extension" { return "installs a Driver extension"; }
    if t == "install_ffmpeg" { return "installs a binary onto the machine"; }
    if t == "end_session" { return "tears down the session's cursor, recording and state"; }
    if t == "escalate_session" { return "deprecated capture-scope escalation; widens what is recorded"; }
    "listed as danger by the plugin's risk table"
}

// ── op: tools ───────────────────────────────────────────────────────────────
//
// The taxonomy as data, so an outer agent can read what the driver can do
// before it commits to an intent. `counts` is here so a caller can see the
// gate is total without counting 58 rows by hand.

fn tools(host, ctx) {
    let w = with_of(ctx);
    let rows = taxonomy();
    let counts = #{ observe: 0, act: 0, danger: 0, unknown: 0 };
    for r in rows {
        let k = r["risk"];
        counts[k] = counts[k] + 1;
    }
    let want = norm(mstr(w, "risk"));
    let out = [];
    for r in rows {
        if want == "" || r["risk"] == want { out.push(r); }
    }
    #{ total: rows.len(), counts: counts, filtered: out.len(), tools: out }
}

// ── binding: find the handle Cua handed out ─────────────────────────────────
//
// The walk is deliberately schema-agnostic. Cua's `get_window_state` returns
// `elements[]` today, but the plugin cannot verify that against a live driver
// from inside Rhai, and a binder hard-coded to one field path would return
// "no match" on any shape change — which reads exactly like "that button is
// not there", and sends the caller off to click something else. So instead the
// walk looks for *any* object carrying an `element_token`, wherever it sits.

// Collect every map under `v` that carries `key`.
//
// The accumulator is *returned*, not pushed into. Rhai clones an array when it
// is passed as a function argument, so the `collect_into(v, key, out, depth)`
// shape this replaced appended to a local copy that the caller never saw: the
// walk reported zero elements for every input, which reads exactly like "the
// snapshot contains no clickable elements" and sends the caller off to click
// something else. Returning the array is the only way the recursion composes.
fn collect_with(v, key, depth) {
    let out = [];
    if depth > 12 { return out; }
    if type_of(v) == "map" {
        if v.contains(key) { out.push(v); }
        for k in v.keys() {
            let sub = collect_with(v[k], key, depth + 1);
            let i = 0;
            while i < sub.len() { out.push(sub[i]); i += 1; }
        }
    } else if type_of(v) == "array" {
        let i = 0;
        while i < v.len() {
            let sub = collect_with(v[i], key, depth + 1);
            let j = 0;
            while j < sub.len() { out.push(sub[j]); j += 1; }
            i += 1;
        }
    }
    return out;
}

// Field names that carry a human-readable label in an accessibility element,
// most specific first. A value found under one of these is worth more than the
// same text found in some other string field.
fn label_keys() {
    ["label", "title", "ax_title", "name", "text", "value", "ax_value",
     "description", "ax_description", "help", "placeholder", "role", "ax_role",
     "subrole", "identifier", "url", "string_value"]
}

// A base64 PNG rides along with every screenshot; treating it as candidate text
// would both blow the budget and let a click bind to a blob.
fn usable_text(s) {
    let t = trimmed("" + s);
    if t.len() > 120 { return ""; }
    let low = t.to_lower();
    if low.contains("data:image") { return ""; }
    if low.contains("base64") { return ""; }
    if low.len() > 24 && !low.contains(" ") && low.contains("/") { return ""; }
    t
}

// The element's searchable text: preferred label fields first, then any other
// short string on the element. The fallback is what makes this survive a
// schema change — a new field the driver invents for its own reasons is still
// searchable text, and losing it would silently narrow every query.
fn element_text(el) {
    let parts = [];
    let keys = label_keys();
    for k in keys {
        if el.contains(k) {
            let t = usable_text(ostr(el[k]));
            if t != "" { parts.push(t); }
        }
    }
    for k in el.keys() {
        let known = false;
        for lk in keys { if lk == k { known = true; } }
        if known { continue; }
        let v = el[k];
        if type_of(v) != "string" { continue; }
        let t = usable_text(v);
        if t != "" { parts.push(t); }
    }
    return parts;
}

// How well one element's text answers the query. Ordered strongest-first: an
// exact hit on a label is what the caller meant, a substring is a guess that
// might also match a sibling, and token overlap is the fallback that keeps
// "the blue OK button in the footer" from failing when the element only
// records "OK".
fn score_element(parts, query) {
    let q = norm(query);
    if q == "" { return 0; }
    let best = 0;
    for raw in parts {
        let t = norm(raw);
        if t == "" { continue; }
        if t == q { if 100 > best { best = 100; } continue; }
        if t.starts_with(q) { if 70 > best { best = 70; } continue; }
        if t.contains(q) { if 50 > best { best = 50; } continue; }
        if q.contains(t) && t.len() >= 3 { if 30 > best { best = 30; } continue; }
        // Token overlap: how many of the query's words appear in this text.
        let hits = 0;
        let total = 0;
        for w in words(q) {
            total += 1;
            if t.contains(w) { hits += 1; }
        }
        if total > 0 && hits == total && 20 > best { best = 20; }
    }
    return best;
}

// Split on whitespace. Hand-rolled because the string package's `split` is
// avoided here to keep one definition of "a word" in the file.
fn words(s) {
    let out = [];
    let cur = "";
    let i = 0;
    let t = "" + s;
    while i < t.len() {
        let c = t.sub_string(i, 1);
        if c == " " || c == "\t" || c == "\n" || c == "\r" || c == "," || c == ";" {
            if cur != "" { out.push(cur); cur = ""; }
        } else {
            cur += c;
        }
        i += 1;
    }
    if cur != "" { out.push(cur); }
    return out;
}

// ── op: bind ────────────────────────────────────────────────────────────────
//
// A target description -> the exact handle to act on. `observation` is the raw
// payload of the get_window_state / get_accessibility_tree call the caller just
// made; passing the *current* one is the caller's job, because a token from an
// older snapshot is not a weaker answer, it is a wrong one.

fn bind(host, ctx) {
    let w = with_of(ctx);
    let target = norm(mstr(w, "target"));
    if target == "" { target = norm(mstr(w, "query")); }
    if target == "" {
        return #{ ok: false, error: "bind needs 'target' (or 'query')" };
    }
    let obs_raw = mget(w, "observation");
    if missing(obs_raw) {
        return #{
            ok: false, error: "bind needs 'observation'",
            hint: "pass the raw payload of the get_window_state call; tokens are per-snapshot"
        };
    }
    let obs = as_json(host, obs_raw);
    let elems = elements_of(obs);
    if elems.len() == 0 {
        return #{
            ok: false, error: "no elements with an element_token in this observation",
            scanned: obs, target: target,
            hint: "the observation must be a get_window_state / get_accessibility_tree result, not a list_windows one"
        };
    }
    return rank_elements(elems, target, obs, w);
}

// pid / window_id are addressed at three levels: on the element itself, on the
// observation that contains it, and in the caller's own run state. The first
// two keep a token self-describing; the third is what lets a turn reuse the
// window it is already working in without re-deriving it. A zero here is
// "absent" and is never emitted as a real id.
fn pick_num(el, obs, w, key) {
    let v = mget(el, key);
    if num_ok(v) { return int_of(v); }
    v = mget(obs, key);
    if num_ok(v) { return int_of(v); }
    v = mget(w, key);
    if num_ok(v) { return int_of(v); }
    return 0;
}

fn rank_elements(elems, target, obs, w) {
    let min_score = 20;
    let scored = [];
    for el in elems {
        let parts = element_text(el);
        let s = score_element(parts, target);
        scored.push(#{ el: el, score: s, text: parts });
    }
    // Descending by score. Insertion sort: the candidate set is one window's
    // elements, and a hand-rolled sort keeps the comparison in one place.
    let n = scored.len();
    let i = 1;
    while i < n {
        let cur = scored[i];
        let j = i - 1;
        while j >= 0 && scored[j]["score"] < cur["score"] {
            scored[j + 1] = scored[j];
            j -= 1;
        }
        scored[j + 1] = cur;
        i += 1;
    }

    let best = scored[0];
    if best["score"] < min_score {
        return #{
            ok: false,
            error: "no element matched '" + target + "'",
            target: target,
            candidates_scanned: n,
            // The near-misses are the useful part: "OK" against a dialog whose
            // buttons are "OKAY" and "Cancel" is a decision the caller can make,
            // and returning nothing would leave it guessing.
            closest: brief(scored, 5),
            hint: "no clickable target was bound; nothing will be clicked"
        };
    }

    let el = best["el"];
    let pid = pick_num(el, obs, w, "pid");
    let wid = pick_num(el, obs, w, "window_id");
    let token = trimmed(ostr(mget(el, "element_token")));
    return #{
        ok: true,
        target: target,
        score: best["score"],
        element_token: token,
        pid: pid,
        window_id: wid,
        // Cua addresses a click by element_token *or* by pid+window_id, never
        // both, and the token carries its own window. Sending both is at best
        // redundant and at worst a mismatch against a different window.
        address: #{
            element_token: token,
            pid: pid,
            window_id: wid
        },
        matched_text: brief_one(best["text"]),
        alternatives: brief_tail(scored, 1, 4)
    };
}

fn brief_one(parts) {
    let out = [];
    let i = 0;
    while i < parts.len() && out.len() < 4 {
        let t = trimmed(parts[i]);
        if t != "" { out.push(t); }
        i += 1;
    }
    return out;
}

fn brief(scored, limit) {
    let out = [];
    let i = 0;
    while i < scored.len() && out.len() < limit {
        out.push(#{
            score: scored[i]["score"],
            text: brief_one(scored[i]["text"])
        });
        i += 1;
    }
    return out;
}

fn brief_tail(scored, from, limit) {
    let out = [];
    let i = from;
    while i < scored.len() && out.len() < limit {
        if scored[i]["score"] > 0 {
            out.push(#{
                score: scored[i]["score"],
                text: brief_one(scored[i]["text"])
            });
        }
        i += 1;
    }
    return out;
}

// ── intent parsing ──────────────────────────────────────────────────────────
//
// A keyword grammar, not a language model, and it is deliberately shallow. The
// outer node's `primary_q` is where judgement belongs: this returns its best
// deterministic read *with the alternatives it considered*, and says plainly
// when it is not confident. A parser that guessed silently would hand the
// engine a real `click` built from a misread verb, and the whole reason this
// split exists is that the decision is reviewable before the effect lands.
//
// Order is significant — the first table is checked first, so the specific
// ("kill") is separated from the general ("close the window") before the
// general one can swallow it.

// Peel the words that describe *where* or *how* to click, leaving the label.
//
// The comparison string is rebuilt after every strip and the loop restarts.
// Computing it once per pass and mutating `cur` inside the inner loop is a trap
// that looks harmless and is not: a duplicated entry in `fillers` then matched
// the *stale* lowercase text twice, so "the OK button" lost its first four
// characters twice and arrived at the binder as "utton" — a target that matches
// nothing, reported as "no element matched" for a button plainly in the
// snapshot.
fn strip_fillers(s) {
    let fillers = ["the ", "a ", "an ", "on ", "at ", "in ", "to ", "please ",
                   "now", "then", "my ", "our ", "button ", "按钮"];
    let cur = trimmed("" + s);
    let again = true;
    while again {
        again = false;
        for f in fillers {
            if cur.to_lower().starts_with(f) && cur.len() > f.len() {
                cur = trimmed(cur.sub_string(f.len(), cur.len() - f.len()));
                again = true;
                break;
            }
        }
        if cur.len() > 1 && (cur.starts_with("\"") || cur.starts_with("'")) {
            cur = trimmed(cur.sub_string(1, cur.len() - 1));
            again = true;
        }
        if cur.len() > 1 && (cur.ends_with("\"") || cur.ends_with("'")) {
            cur = trimmed(cur.sub_string(0, cur.len() - 1));
            again = true;
        }
    }
    return cur;
}

// "<label> <role noun>" is how people name a control; the role noun is not part
// of the label the accessibility tree carries. "the OK button" has to become
// "OK", or the binder scores the snapshot's "OK" element against "OK button"
// and reports the button as absent.
fn strip_trailing_nouns(s) {
    let nouns = [" button", " link", " field", " text field", " tab", " item",
                 " row", " cell", " menu", " menu item", " icon", " checkbox",
                 " checkbox button", " radio button", " toolbar", " tool",
                 " dropdown", " combo box", " toggle", " switch", " slider",
                 " 按钮", "链接", "选项", "菜单", "输入框", "标签页"];
    let tails = [" in the", " on the", " at the", " of the", " in a", " on a",
                 " in this", " of this", " here", " please", " now"];
    let cur = trimmed("" + s);
    let again = true;
    while again {
        again = false;
        for f in nouns {
            if cur.to_lower().ends_with(f) && cur.len() > f.len() {
                cur = trimmed(cur.sub_string(0, cur.len() - f.len()));
                again = true;
                break;
            }
        }
        if again { continue; }
        for f in tails {
            if cur.to_lower().ends_with(f) && cur.len() > f.len() {
                cur = trimmed(cur.sub_string(0, cur.len() - f.len()));
                again = true;
                break;
            }
        }
    }
    return cur;
}

fn after_verb(s, verbs) {
    let low = norm(s);
    let at = -1;
    let hit = "";
    for v in verbs {
        let p = low.index_of(v);
        if p >= 0 && (at < 0 || p < at) { at = p; hit = v; }
    }
    if at < 0 { return ""; }
    let end = at + hit.len();
    if end >= s.len() { return ""; }
    return trimmed(s.sub_string(end, s.len() - end));
}

fn contains_any(s, words) {
    let low = norm(s);
    for w in words {
        if low.contains(w) { return true; }
    }
    return false;
}

// Parse an intent into the Cua call it means. Returns
//   #{ verb, tool, args, risk, confidence, why, target, needs_bind }
//
// `needs_bind` marks the calls whose arguments are not knowable from the
// intent alone: a click needs the element handle Cua issued for the *current*
// snapshot, so `plan` emits the observation first and defers the click.
fn parse_intent(intent, w) {
    let s = trimmed("" + intent);
    let low = norm(s);
    let sid = sid_of(w);

    // ── read-only observations ──
    if contains_any(s, ["kill ", "force quit", "force-quit", "杀掉", "结束进程"]) {
        return verb_of("quit_app", "kill_app",
            strip_fillers(after_verb(s, ["kill ", "force quit ", "force-quit ", "杀掉"])),
            sid, "force-terminates a process by pid");
    }
    if contains_any(s, ["clipboard", "剪贴板"]) {
        if contains_any(s, ["read", "get", "what", "读", "取", "看看"]) {
            return verb_of("read_clipboard", "clipboard_read", "", sid,
                "reads the system clipboard");
        }
        let payload = strip_quoted(s, ["copy ", "write ", "put ", "set ", "写", "复制"]);
        return #{ verb: "write_clipboard", tool: "clipboard_write",
                  args: #{ session: sid, text: payload },
                  risk: risk_of("clipboard_write"), confidence: "medium",
                  target: payload, needs_bind: false,
                  why: "asked to replace the clipboard" };
    }
    if contains_any(s, ["screenshot", "screen shot", "截屏", "截图", "what's on screen", "on the screen"]) {
        return #{ verb: "screenshot", tool: "get_desktop_state", args: obs_args(w, sid),
                  risk: "observe", confidence: "high", target: "",
                  needs_bind: false, why: "asked for what is on screen" };
    }
    if contains_any(s, ["screen size", "resolution", "how big is the screen", "分辨率", "屏幕大小"]) {
        return #{ verb: "screen_size", tool: "get_screen_size", args: #{ session: sid },
                  risk: "observe", confidence: "high", target: "",
                  needs_bind: false, why: "asked for the display geometry" };
    }
    if contains_any(s, ["window", "窗口"]) {
        return #{ verb: "windows", tool: "list_windows", args: #{ session: sid },
                  risk: "observe", confidence: "high", target: "",
                  needs_bind: false, why: "asked which windows exist" };
    }
    if contains_any(s, ["app", "application", "应用", "程序"]) && contains_any(s, ["list", "running", "which", "what", "哪些", "运行"]) {
        return #{ verb: "apps", tool: "list_apps", args: #{ session: sid },
                  risk: "observe", confidence: "high", target: "",
                  needs_bind: false, why: "asked which apps are running" };
    }
    if contains_any(s, ["cursor", "mouse position", "pointer", "光标", "鼠标位置"]) {
        return #{ verb: "cursor", tool: "get_cursor_position", args: #{ session: sid },
                  risk: "observe", confidence: "high", target: "",
                  needs_bind: false, why: "asked where the pointer is" };
    }
    if contains_any(s, ["permission", "accessibility", "screen recording", "权限", "辅助功能"]) {
        return #{ verb: "permissions", tool: "check_permissions", args: #{ session: sid },
                  risk: "observe", confidence: "high", target: "",
                  needs_bind: false, why: "asked about the macOS TCC gate" };
    }
    if contains_any(s, ["health", "doctor", "diagnos", "健康"]) {
        return #{ verb: "health", tool: "health_report", args: #{ session: sid },
                  risk: "observe", confidence: "high", target: "",
                  needs_bind: false, why: "asked for driver diagnostics" };
    }
    // ── app control ──
    let app = strip_fillers(after_verb(s, ["launch ", "open ", "start ", "打开", "启动", "运行"]));
    if app != "" {
        return #{ verb: "launch", tool: "launch_app", args: launch_args(app, sid),
                  risk: "act", confidence: "high", target: app,
                  needs_bind: false, why: "asked to start an app" };
    }
    let front = strip_fillers(after_verb(s, ["focus ", "foreground ", "switch to ", "activate ", "切换到", "激活"]));
    if front != "" {
        return #{ verb: "focus", tool: "bring_to_front", args: launch_args(front, sid),
                  risk: "act", confidence: "high", target: front,
                  needs_bind: false, why: "asked to raise an app" };
    }

    // ── pointer ──
    // The double- and right-click forms are matched before the bare one,
    // because "double click" contains "click" and would otherwise degrade into
    // two separate single clicks — which on a list is a different action
    // entirely, not a slower version of the same one.
    let dbl = after_verb(s, ["double click", "double-click", "doubleclick", "双击"]);
    if dbl != "" {
        return #{ verb: "double_click", tool: "double_click", args: #{ session: sid },
                  risk: "act", confidence: "high", target: strip_trailing_nouns(strip_fillers(dbl)),
                  needs_bind: true, why: "asked for a double click" };
    }
    let rgt = after_verb(s, ["right click", "right-click", "rightclick", "右键"]);
    if rgt != "" {
        return #{ verb: "right_click", tool: "right_click", args: #{ session: sid },
                  risk: "act", confidence: "high", target: strip_trailing_nouns(strip_fillers(rgt)),
                  needs_bind: true, why: "asked for a right click" };
    }
    if contains_any(s, ["scroll", "滚动"]) {
        return #{ verb: "scroll", tool: "scroll", args: scroll_args(s, sid),
                  risk: "act", confidence: "high", target: "",
                  needs_bind: false, why: "asked to scroll" };
    }
    let clk = after_verb(s, ["click on ", "click the ", "click ", "tap ", "点击", "点一下", "点 "]);
    if clk != "" {
        return #{ verb: "click", tool: "click", args: #{ session: sid },
                  risk: "act", confidence: "high", target: strip_trailing_nouns(strip_fillers(clk)),
                  needs_bind: true, why: "asked to click an element" };
    }

    // ── keyboard ──
    if contains_any(s, ["hotkey", "shortcut", "keyboard shortcut", "快捷键"]) || low.contains("cmd+") || low.contains("command+") {
        return #{ verb: "hotkey", tool: "hotkey", args: hotkey_args(s, sid),
                  risk: "act", confidence: "medium", target: "",
                  needs_bind: false, why: "asked for a key combination" };
    }
    let typed = after_verb(s, ["type ", "type in ", "enter the text ", "input ", "输入", "键入"]);
    let pressed = after_verb(s, ["press ", "hit ", "press the key ", "send the key ", "按键", "按一下"]);
    if typed != "" {
        return #{ verb: "type", tool: "type_text", args: #{ session: sid, text: strip_quoted(typed, []) },
                  risk: "act", confidence: "high", target: "",
                  needs_bind: false, why: "asked to type text" };
    }
    if pressed != "" {
        // "press the button" is a click and "press return" is a keystroke. The
        // remainder decides, because English uses one word for both.
        let named = strip_trailing_nouns(strip_fillers(pressed));
        if looks_like_key(norm(named)) {
            return #{ verb: "press", tool: "press_key", args: #{ session: sid, key: norm(named) },
                      risk: "act", confidence: "high", target: "",
                      needs_bind: false, why: "asked for a keystroke" };
        }
        return #{ verb: "click", tool: "click", args: #{ session: sid },
                  risk: "act", confidence: "medium", target: named,
                  needs_bind: true, why: "'press' with a non-key remainder reads as a click" };
    }

    // ── browser (Cua's own CDP-backed tools) ──
    let url = strip_fillers(after_verb(s, ["navigate to ", "go to ", "browse to ", "open url ", "visit ", "访问", "浏览"]));
    if url != "" {
        return #{ verb: "navigate", tool: "browser_navigate", args: #{ session: sid },
                  risk: "act", confidence: "medium", target: url,
                  needs_bind: false, why: "asked to navigate a bound tab" };
    }

    return #{ verb: "", tool: "", args: #{}, risk: "unknown", confidence: "low",
              target: "", needs_bind: false, why: "no verb in the intent matched" };
}

// The shape every parsed intent returns. Kept in one place because a field
// added here and forgotten in one branch is how a call ends up planned with no
// `risk` and therefore no gate on it.
fn verb_of(verb, tool, target, sid, why) {
    #{ verb: verb, tool: tool, args: #{ session: sid }, risk: risk_of(tool),
       confidence: "high", target: target, needs_bind: false, why: why }
}

// A dotted token is a bundle id; anything with a space is a display name. Cua
// prefers bundle_id, and picking wrong here is the difference between starting
// the app the user named and starting a different one that shares the word.
fn launch_args(app, sid) {
    let a = trimmed(app);
    if a == "" { return #{ session: sid }; }
    if a.contains(".") && !a.contains(" ") { return #{ session: sid, bundle_id: a }; }
    return #{ session: sid, name: a };
}

fn obs_args(w, sid) {
    let a = #{ session: sid };
    let d = mget(w, "max_image_dimension");
    if num_ok(d) { a["max_image_dimension"] = int_of(d); }
    let f = mstr(w, "screenshot_out_file");
    if f != "" { a["screenshot_out_file"] = f; }
    return a;
}

fn scroll_args(s, sid) {
    let low = norm(s);
    let dir = "down";
    if low.contains("up") || low.contains("上") { dir = "up"; }
    else if low.contains("left") || low.contains("左") { dir = "left"; }
    else if low.contains("right") || low.contains("右") { dir = "right"; }
    let a = #{ session: sid, direction: dir };
    // Cua's `amount` is wheel notches on the pixel path and key repeats on the
    // keystroke path; a larger default than 3 undershoots a page and makes the
    // caller loop for no reason.
    a["amount"] = 5;
    return a;
}

fn hotkey_args(s, sid) {
    let low = norm(s);
    // "cmd+shift+p" and "command+shift+p" are the same chord; normalising the
    // long names is what lets one entry in the key table match both.
    let t = low;
    let keys = [];
    let cur = "";
    let i = 0;
    while i < t.len() {
        let c = t.sub_string(i, 1);
        if c == "+" {
            if cur != "" { keys.push(cur); cur = ""; }
        } else {
            cur += c;
        }
        i += 1;
    }
    if cur != "" { keys.push(cur); }
    let out = [];
    for k in keys {
        let n = canonical_key(k);
        if n != "" { out.push(n); }
    }
    return #{ session: sid, keys: out };
}

fn canonical_key(k) {
    let n = norm(k);
    if n == "cmd" || n == "command" || n == "meta" { return "cmd"; }
    if n == "shift" { return "shift"; }
    if n == "opt" || n == "option" || n == "alt" { return "option"; }
    if n == "ctrl" || n == "control" { return "ctrl"; }
    if n == "fn" { return "fn"; }
    return n;
}

// "press the button" is a click; "press return" is a keystroke. English uses
// one verb for both, so the remainder is what decides — and guessing wrong
// sends a Return to a dialog that was asked to be clicked.
fn looks_like_key(k) {
    if k == "" { return false; }
    let names = ["return", "enter", "tab", "escape", "esc", "space", "up", "down",
                 "left", "right", "delete", "backspace", "home", "end", "pageup",
                 "pagedown", "cmd", "command", "shift", "option", "alt", "ctrl",
                 "control", "fn", "capslock", "help", "insert"];
    for n in names { if k == n { return true; } }
    // F1..F12 — anything else starting with f and ending in a digit.
    if k.len() >= 2 && k.len() <= 3 {
        let head = k.sub_string(0, 1);
        if head == "f" && "0123456789".index_of(k.sub_string(1, 1)) >= 0 { return true; }
    }
    return false;
}

// Strip the verb (if given) and then any quoting, because the text a user
// wants typed is very often quoted and the quotes are not part of it.
fn strip_quoted(s, verbs) {
    let t = trimmed("" + s);
    if verbs.len() > 0 {
        let a = after_verb(t, verbs);
        if a != "" { t = a; }
    }
    t = trimmed(t);
    let n = t.len();
    if n >= 2 {
        let f = t.sub_string(0, 1);
        let l = t.sub_string(n - 1, 1);
        if (f == "\"" && l == "\"") || (f == "'" && l == "'") {
            return t.sub_string(1, n - 2);
        }
    }
    return t;
}

// ── the gate ────────────────────────────────────────────────────────────────
//
// One chokepoint. Every call the plugin emits goes through `emit`, so there is
// no path by which a `danger` tool reaches the `exec` capability without
// passing the same check, and no way for a new op to forget it.
//
// The refusal is deliberately loud. Returning an empty plan for
// "quitting Safari" would look to a caller like a parse failure, and the
// natural next move would be to loosen something.

fn emit(host, tool, args, why, allow_danger) {
    let risk = risk_of(tool);
    let base = #{ tool: tool, args: args, risk: risk, why: why, blocked: false, error: "" };
    if risk == "unknown" {
        base["blocked"] = true;
        base["error"] = "unknown tool '" + tool + "': not in this plugin's risk table"
            + " (the driver may be newer than the plugin; re-check with op=tools)";
        return base;
    }
    if risk == "danger" && !allow_danger {
        base["blocked"] = true;
        base["error"] = "refused: " + tool + " " + danger_reason(tool)
            + " — pass allow_danger: true to run it anyway";
        return base;
    }
    // The exec capability interpolates ${state.args} into a single argv
    // element, so the arguments must already be a compact JSON *string* here.
    // An object would stringify into the argv slot as JSON too, but doing it
    // once, in the plugin, means the exact bytes that run are visible in the
    // plan the caller reviewed.
    base["args_json"] = wire(host, args);
    return base;
}

fn blocked_result(e, why) {
    #{ status: "blocked", blocked: true, error: e, why: why,
       tool: "", args: #{}, args_json: "", risk: "unknown" }
}

// ── op: plan ────────────────────────────────────────────────────────────────
//
// The loop's unit of work: an intent plus whatever the previous turn observed
// in, one Cua call out.
//
// It is stateless on purpose. Deriving the next call from (intent, observation,
// call_index) rather than from a cursor the plugin has to carry means a dropped
// or replayed turn cannot desynchronise the plan from the desktop — the state
// that matters (which element is bound *now*) is re-read from the observation
// every turn, which is the only place it is still true.
//
// A call that needs a target emits the observation first and returns
// `phase: "observe"`. The caller runs it, feeds the result back as
// `with.observation`, and the next call to `plan` binds and acts. A caller that
// wants the whole thing without a loop should use `sequence`.

// The turn counter rides on the log rather than on an increment. The DSL has
// no arithmetic in `project`, so a spec cannot write `turn = turn + 1`; and a
// counter the plugin owns separately could drift from the steps it actually
// emitted. The log is that record — its length is the number of calls made, so
// the budget and the plan are the same fact and cannot disagree.
fn plan(host, ctx) {
    let w = with_of(ctx);
    let st = state_of(ctx);
    let log = as_array(mget(st, "cua_log"));
    let r = plan_turn(host, w, log.len());
    // Only a step that will actually be executed is recorded. A blocked or
    // unparsed turn made no call, and counting it would spend budget on a step
    // the driver never saw.
    if mstr(r, "status") == "call" {
        log.push(#{ tool: mstr(r, "tool"), args_json: mstr(r, "args_json"),
                    risk: mstr(r, "risk"), phase: mstr(r, "phase") });
    }
    r["turn"] = log.len();
    r["cua_log"] = log;
    r["driver_status"] = driver_status_of(w);
    r["verdict"] = if mstr(r, "status") == "call" { "GO" } else { "HALT" };
    return r;
}

// What the driver said last time, as a single word. Reported on every plan —
// including the ones that go on to emit a call — because "the previous call
// failed and this one is a different tool" is exactly the situation a caller
// needs to see and cannot reconstruct from `cua_log` alone.
fn driver_status_of(w) {
    let t = trimmed(ostr(mget(w, "error_text")) + " " + ostr(mget(w, "observation")));
    if t == "" { return "none"; }
    return classify(t, 0);
}

fn plan_turn(host, w, log_len) {
    let intent = mstr(w, "intent");
    let allow_danger = flag(w, "allow_danger", false);
    let max_calls = int_of(mget(w, "max_calls"));
    if max_calls <= 0 { max_calls = 8; }

    // A caller driving `plan` by hand may pass `call_index` directly; the spec
    // loop lets the log supply it.
    let call_index = int_of(mget(w, "call_index"));
    if mget(w, "call_index") == () { call_index = log_len; }

    if intent == "" {
        return #{ status: "needs_clarification", blocked: false, error: "",
                 why: "plan needs 'intent'",
                 candidates: verb_catalogue() };
    }
    if call_index >= max_calls {
        return #{ status: "done", blocked: false, error: "", done: true,
                 tool: "", args: #{}, args_json: "", risk: "observe",
                 why: "call budget spent (" + max_calls + " calls) without the caller stopping the loop",
                 call_index: call_index };
    }

    let p = parse_intent(intent, w);
    if p["tool"] == "" {
        return #{ status: "needs_clarification", blocked: false, error: "",
                 intent: intent, call_index: call_index,
                 why: p["why"], candidates: verb_catalogue() };
    }

    // quit_app addresses a pid, and the pid is not knowable from "quit
    // Safari" — it has to come from list_apps, the same deferral a click needs.
    let needs_list = p["verb"] == "quit_app" || p["verb"] == "launch";
    let obs = mget(w, "observation");
    // Presence, not text: an observation legitimately arrives as a map, and
    // `ostr` renders a map as "" — testing it as a string made every snapshot
    // look absent, so the loop re-observed forever and never bound anything.
    let have_obs = present(obs);

    // A driver-level refusal (the macOS permission gate, an unknown tool, a
    // timeout) is not a step to recover from by trying the next intent — every
    // retry hits the same wall, and re-planning against an error string would
    // bind against nothing. Stop here and say what to fix, once.
    //
    // Both streams are read because the driver does not pick one: a successful
    // call prints its JSON on stdout, and a refusal such as
    // `permissions_pending: ...` arrives on *stderr* with exit 75. Classifying
    // stdout alone therefore classified every failure as "empty", which is how a
    // permission wall looks identical to a call that produced no output.
    let err_text = ostr(mget(w, "error_text"));
    // Only *raw* driver output is classified. A map observation is a document
    // the caller already parsed, so there is no text to classify — and running
    // classify() on it read "" and declared every good snapshot "empty", which
    // blocked the run before it could bind anything.
    let raw_obs = "";
    if type_of(obs) == "string" { raw_obs = trimmed(obs); }
    if raw_obs != "" || err_text != "" {
        let st = classify(raw_obs + " " + err_text, 0);
        if st != "ok" {
            return #{
                status: "blocked", blocked: true, intent: intent, verb: p["verb"],
                error: "the last Cua Driver call did not succeed (" + st + "): " + trimmed(raw_obs + " " + err_text),
                remedy: remedy(st), tool: "", args: #{}, args_json: "",
                risk: "observe", call_index: call_index, turn: call_index,
                why: "stopping: the driver refused the call, so the next one would too"
            };
        }
    }

    if p["needs_bind"] || needs_list {
        if !have_obs {
            return observe_step(host, p, w, sid_of(w), intent, call_index);
        }
        let o = as_json(host, obs);
        if p["needs_bind"] {
            return bind_and_act(host, p, o, w, intent, call_index);
        }
        return list_and_act(host, p, o, w, intent, call_index);
    }

    let e = emit(host, p["tool"], p["args"], p["why"], allow_danger);
    if e["blocked"] { return blocked_from(e, p, intent, call_index); }
    return #{
        status: "call", blocked: false, error: "", intent: intent,
        phase: "act", verb: p["verb"], tool: e["tool"], args: e["args"],
        args_json: e["args_json"], risk: e["risk"], why: p["why"],
        confidence: p["confidence"], call_index: call_index
    };
}

fn sid_of(w) {
    // Empty means "do not name a session". The driver only accepts a `session`
    // label that a `start_session` call actually created -- an unstarted label
    // is refused outright ("session has ended; ... Call start_session"). So a
    // made-up label is worse than none: with none the driver falls back to its
    // own implicit session and every tool works.
    let s = mstr(w, "session");
    if s == "laya" { return ""; }
    return s;
}

// Serialize the argv payload, dropping an empty session label. Every emitted
// call goes through here, so no path can hand the exec capability a `session`
// key the driver will reject.
fn wire(host, args) {
    if args.contains("session") && args["session"] == "" { args.remove("session"); }
    return jstr(host, args);
}

// A bind-requiring intent, with nothing to bind against yet.
//
// Which observation to take depends on what is already known. With a pid and a
// window in hand, get_window_state with a `query` is both narrower and cheaper
// than walking the whole desktop, and Cua does the filtering itself. Without
// them, the desktop-wide accessibility tree is the only observation that can
// name the app the user is talking about.
fn observe_step(host, p, w, sid, intent, ci) {
    let pid = int_of(mget(w, "pid"));
    let wid = int_of(mget(w, "window_id"));
    let args = #{ session: sid };
    let tool = "get_accessibility_tree";
    let why = "need an element handle before " + p["verb"];
    if pid > 0 && wid > 0 {
        tool = "get_window_state";
        args["pid"] = pid;
        args["window_id"] = wid;
        // The screenshot is not needed to resolve a handle and is by far the
        // largest thing in the response; the structured elements are.
        args["include_screenshot"] = false;
        if p["target"] != "" { args["query"] = p["target"]; }
    }
    return #{
        status: "call", blocked: false, error: "", intent: intent,
        phase: "observe", verb: p["verb"], tool: tool, args: args,
        args_json: wire(host, args), risk: risk_of(tool), why: why,
        confidence: p["confidence"], awaiting: p["verb"], target: p["target"],
        call_index: ci
    };
}

fn bind_and_act(host, p, obs, w, intent, ci) {
    let b = rank_elements(elements_of(obs), p["target"], obs, w);
    if !b["ok"] {
        return #{
            status: "blocked", blocked: true, intent: intent, verb: p["verb"],
            error: b["error"], hint: mstr(b, "hint"),
            candidates: mget(b, "closest"), scanned: mget(b, "candidates_scanned"),
            tool: "", args: #{}, args_json: "", risk: risk_of(p["tool"]),
            call_index: ci,
            // Explicit: a failed bind ends the turn rather than falling
            // through to a coordinate click. This is the whole point.
            why: "no element matched '" + p["target"] + "'; nothing was clicked"
        };
    }
    let a = p["args"];
    let addr = b["address"];
    a["element_token"] = addr["element_token"];
    if addr["pid"] > 0 { a["pid"] = addr["pid"]; }
    return #{
        status: "call", blocked: false, error: "", intent: intent,
        phase: "act", verb: p["verb"], tool: p["tool"], args: a,
        args_json: wire(host, a), risk: p["risk"],
        why: p["why"] + " — bound '" + p["target"] + "' to "
            + mstr(b, "element_token") + " (score " + b["score"] + ")",
        confidence: p["confidence"], address: addr,
        matched_text: mget(b, "matched_text"),
        alternatives: mget(b, "alternatives"), call_index: ci
    };
}

// `launch` and `quit_app` name an app; the pid comes from list_apps. Matching
// is on the app's own name, and a miss returns the candidate list rather than a
// pid of 0 — kill_app with pid 0 is a failed call, and launch_app with an
// empty name is a failed call, so neither is destructive by accident, but
// neither is useful either.
fn list_and_act(host, p, obs, w, intent, ci) {
    let want = norm(p["target"]);
    let apps = apps_of(obs);
    let hits = [];
    for a in apps {
        let label = norm(usable_text(ostr(mget(a, "name"))));
        if label == "" { label = norm(usable_text(ostr(mget(a, "app_name")))); }
        if label == "" { label = norm(usable_text(ostr(mget(a, "bundle_id")))); }
        if label == "" { continue; }
        let s = 0;
        if label == want { s = 100; }
        else if label.contains(want) || want.contains(label) { s = 50; }
        if s > 0 {
            let row = #{ label: label, score: s, pid: int_of(mget(a, "pid")) };
            if mget(a, "bundle_id") != () { row["bundle_id"] = ostr(mget(a, "bundle_id")); }
            if mget(a, "running") != () { row["running"] = mget(a, "running"); }
            hits.push(row);
        }
    }
    if hits.len() == 0 {
        return #{
            status: "blocked", blocked: true, intent: intent, verb: p["verb"],
            error: "no app matched '" + p["target"] + "'",
            candidates: brief_apps(apps, 6), tool: "", args: #{}, args_json: "",
            risk: p["risk"], call_index: ci,
            why: "the app name did not resolve to a pid; nothing was launched or killed"
        };
    }
    let best = hits[0];
    let a = p["args"];
    if p["verb"] == "quit_app" {
        a["pid"] = best["pid"];
        if best["pid"] <= 0 {
            return #{
                status: "blocked", blocked: true, intent: intent, verb: p["verb"],
                error: "'" + best["label"] + "' has no pid in list_apps",
                candidates: hits, tool: "", args: #{}, args_json: "",
                risk: p["risk"], call_index: ci,
                why: "kill_app needs an exact pid; refusing to guess one"
            };
        }
    }
    let e = emit(host, p["tool"], a, p["why"], flag(w, "allow_danger", false));
    if e["blocked"] { return blocked_from(e, p, intent, ci); }
    return #{
        status: "call", blocked: false, error: "", intent: intent,
        phase: "act", verb: p["verb"], tool: e["tool"], args: e["args"],
        args_json: e["args_json"], risk: e["risk"], why: p["why"],
        confidence: "high", resolved_app: best, call_index: ci
    };
}

fn collect_with(v, key, out, depth) {
    if depth > 12 { return; }
    if type_of(v) == "map" {
        if v.contains(key) { out.push(v); }
        for k in v.keys() { collect_with(v[k], key, out, depth + 1); }
    } else if type_of(v) == "array" {
        for x in v { collect_with(x, key, out, depth + 1); }
    }
}

fn elements_of(obs) {
    return collect_with(obs, "element_token", 0);
}

// App records, not every object that happens to carry a pid. A window record
// has a pid too, and matching "quit Safari" against a window titled "Safari"
// would produce a pid for a document window rather than for the app.
fn apps_of(obs) {
    let all = collect_with(obs, "pid", 0);
    let out = [];
    for a in all {
        let named = false;
        for k in ["name", "app_name", "bundle_id", "display_name"] {
            if mstr(a, k) != "" { named = true; }
        }
        if named { out.push(a); }
    }
    return out;
}

fn brief_apps(apps, limit) {
    let out = [];
    let i = 0;
    while i < apps.len() && out.len() < limit {
        let a = apps[i];
        let label = mstr(a, "name");
        if label == "" { label = mstr(a, "app_name"); }
        if label == "" { label = mstr(a, "bundle_id"); }
        let row = #{ label: label, pid: int_of(mget(a, "pid")) };
        if mget(a, "running") != () { row["running"] = mget(a, "running"); }
        out.push(row);
        i += 1;
    }
    return out;
}

fn blocked_from(e, p, intent, ci) {
    #{ status: "blocked", blocked: true, intent: intent, verb: p["verb"],
       tool: e["tool"], risk: e["risk"], error: e["error"], args: #{},
       args_json: "", call_index: ci, why: p["why"] }
}

// What this plugin understands, so a caller whose intent did not parse is told
// how to rephrase rather than left guessing at the grammar.
fn verb_catalogue() {
    [
        #{ say: "screenshot the screen", does: "get_desktop_state" },
        #{ say: "list the windows", does: "list_windows" },
        #{ say: "which apps are running", does: "list_apps" },
        #{ say: "what is the screen size", does: "get_screen_size" },
        #{ say: "where is the cursor", does: "get_cursor_position" },
        #{ say: "check permissions", does: "check_permissions" },
        #{ say: "run a health report", does: "health_report" },
        #{ say: "open Safari", does: "launch_app" },
        #{ say: "focus Safari", does: "bring_to_front" },
        #{ say: "click the OK button", does: "observe, then click" },
        #{ say: "double click Documents", does: "observe, then double_click" },
        #{ say: "right click the file", does: "observe, then right_click" },
        #{ say: "scroll down", does: "scroll" },
        #{ say: "type hello", does: "type_text" },
        #{ say: "press return", does: "press_key" },
        #{ say: "cmd+shift+p", does: "hotkey" },
        #{ say: "read the clipboard", does: "clipboard_read (danger)" },
        #{ say: "copy hello to the clipboard", does: "clipboard_write (danger)" },
        #{ say: "kill Safari", does: "kill_app (danger)" }
    ]
}

// ── op: sequence ────────────────────────────────────────────────────────────
//
// The whole plan for an intent, as data, for a caller that drives the calls
// itself (or wants to show them to a human before anything runs).
//
// It returns the same steps `plan` would emit, with the *shape* of the bound
// address left open — a token cannot be filled in here, because no observation
// has been taken yet. Claiming otherwise would be the one genuinely dishonest
// thing this plugin could do: an `element_token` invented from a plan is a
// token that points at nothing, and the failure mode is a click on whatever
// the driver resolves it to.

fn sequence(host, ctx) {
    let w = with_of(ctx);
    let intent = mstr(w, "intent");
    let allow_danger = flag(w, "allow_danger", false);
    if intent == "" {
        return #{ ok: false, error: "sequence needs 'intent'", steps: [] };
    }
    let p = parse_intent(intent, w);
    if p["tool"] == "" {
        return #{ ok: false, error: p["why"], intent: intent, steps: [],
                 candidates: verb_catalogue() };
    }
    let sid = sid_of(w);
    let steps = [];
    if p["needs_bind"] || p["verb"] == "quit_app" {
        let os = observe_step(host, p, w, sid, intent, 0);
        steps.push(#{ tool: os["tool"], args: os["args"],
                      args_json: wire(host, os["args"]), risk: os["risk"],
                      phase: "observe", why: os["why"] });
    }
    let e = emit(host, p["tool"], p["args"], p["why"], allow_danger);
    steps.push(#{ tool: e["tool"], args: e["args"], args_json: mstr(e, "args_json"),
                  risk: e["risk"], phase: "act", blocked: e["blocked"],
                  error: mstr(e, "error"), why: p["why"],
                  // Present, and deliberately null: filled in by `bind` against
                  // the real observation.
                  needs_bind: p["needs_bind"], target: p["target"] });
    let blocked = false;
    for st in steps { if st.contains("blocked") && st["blocked"] { blocked = true; } }
    return #{
        ok: !blocked, intent: intent, verb: p["verb"], risk: p["risk"],
        confidence: p["confidence"], blocked: blocked, steps: steps,
        notes: sequence_notes(p, steps)
    };
}

fn sequence_notes(p, steps) {
    let out = [];
    if p["needs_bind"] {
        out.push("step 2's element_token is empty on purpose: run step 1, then call op=bind with its result and this target, then re-issue the click");
    }
    if p["risk"] == "danger" {
        out.push("this plan includes a danger tool; it stays blocked until allow_danger is set");
    }
    if steps.len() == 2 {
        out.push("run the two steps in order — the second is only valid against the first's output");
    }
    return out;
}

// ── op: inspect ─────────────────────────────────────────────────────────────
//
// A raw Cua result -> something a run state can hold.
//
// Two problems, both about size. A desktop screenshot is a base64 PNG of a few
// hundred kilobytes, and `get_window_state` returns one *plus* the whole
// accessibility tree; putting that in state means every downstream node — and
// every log line — carries it. And a driver-level refusal arrives as plain text
// on stdout, which a caller reading it as JSON sees as a parse failure rather
// than as the permission gate it actually is.
//
// So: images are replaced by a descriptor, lists are capped, and the driver's
// own error strings are classified into a `status` the caller can branch on.

// A driver refusal is text, not JSON, and these are the ones worth naming.
// `permissions_pending` is by far the most common: on macOS every tool except
// check_permissions is behind the TCC gate, and the message says "retry after
// the permission gate completes", which invites exactly the retry loop that
// will not help until a human runs `cua-driver permissions grant`.
fn classify(raw, exit_code) {
    let low = norm(raw);
    if low == "" { return "empty"; }
    if low.contains("permissions_pending") || low.contains("permission is still pending") {
        return "permission_gate";
    }
    if low.contains("unknown tool") || low.contains("no such tool") || low.contains("unknown command") {
        return "unknown_tool";
    }
    if low.contains("timed out") || low.contains("timeout") { return "timeout"; }
    if low.contains("not found") || low.contains("no window") { return "not_found"; }
    if exit_code != 0 { return "driver_error"; }
    return "ok";
}

fn remedy(status) {
    if status == "permission_gate" {
        return "run `cua-driver permissions grant` and approve Accessibility + Screen Recording; retrying will not clear it";
    }
    if status == "unknown_tool" {
        return "the driver is newer than this plugin's risk table; check `cua-driver list-tools` against op=tools";
    }
    if status == "timeout" { return "raise the capability's timeout_ms, or narrow the observation"; }
    if status == "not_found" { return "the target window or app is gone; re-observe before acting"; }
    if status == "driver_error" { return "see raw for the driver's own message"; }
    if status == "empty" { return "the call produced no output; check the argv the plan emitted"; }
    return "";
}

fn is_image_payload(s) {
    let t = "" + s;
    if t.len() < 512 { return false; }
    let low = t.to_lower();
    return low.contains("data:image") || low.contains("base64,");
}

// Replace image payloads with a descriptor. The byte count is kept because
// "there was a screenshot, and it was 412 KB" is the useful half of the fact;
// the other half is what does not fit in a run state.
fn strip_images(v, depth) {
    if depth > 14 { return "…"; }
    if type_of(v) == "string" {
        if is_image_payload(v) {
            return #{ image: true, bytes: v.len(),
                     note: "base64 payload removed by op=inspect" };
        }
        if v.len() > 20000 { return v.sub_string(0, 20000) + "…[truncated]"; }
        return v;
    }
    if type_of(v) == "array" {
        let out = [];
        let i = 0;
        while i < v.len() {
            if out.len() >= 200 { out.push("…[" + (v.len() - 200) + " more]"); break; }
            out.push(strip_images(v[i], depth + 1));
            i += 1;
        }
        return out;
    }
    if type_of(v) == "map" {
        let out = #{};
        for k in v.keys() { out[k] = strip_images(v[k], depth + 1); }
        return out;
    }
    return v;
}

fn inspect(host, ctx) {
    let w = with_of(ctx);
    let tool = mstr(w, "tool");
    let given = mget(w, "result");
    if missing(given) { given = mget(w, "stdout"); }
    // A caller that already has the parsed document passes a map; going through
    // ostr first would stringify it to "" and classify a good result as empty.
    let parsed_in = type_of(given) == "map" || type_of(given) == "array";
    let raw = if parsed_in { "" } else { ostr(given) };
    // stderr carries the refusal; a success leaves it empty, so concatenating
    // it costs nothing and is the only way to see a permission gate at all.
    let err = ostr(mget(w, "stderr"));
    if err == "" { err = ostr(mget(w, "error")); }
    let exit_code = int_of(mget(w, "exit_code"));
    let status = if parsed_in { "ok" } else { classify(raw + " " + err, exit_code) };

    // A refusal is text. Returning the classification *and* the text is what
    // lets the caller show the user the driver's actual message instead of
    // inventing one. The length guard comes first: sub_string(0, 1) on an
    // empty stdout is a runtime error, and "empty" is exactly the status that
    // produces one.
    let body = trimmed(raw);
    let looks_json = body.len() > 0 && (body.sub_string(0, 1) == "{" || body.sub_string(0, 1) == "[");
    if status != "ok" && !looks_json {
        return #{
            status: status, ok: false, tool: tool, exit_code: exit_code,
            remedy: remedy(status), raw: trimmed(raw + " " + err),
            summary: #{}
        };
    }

    let doc = if parsed_in { given } else { as_json(host, raw) };
    let elems = elements_of(doc);
    let apps = apps_of(doc);
    let summary = #{};
    if tool == "list_apps" || tool == "list_windows" || tool == "get_accessibility_tree" {
        summary["records"] = apps.len();
        summary["sample"] = brief_apps(apps, 12);
    }
    if tool == "get_window_state" {
        summary["elements"] = elems.len();
        summary["sample"] = brief_elements(elems, 12);
        summary["has_tree_markdown"] = mget(doc, "tree_markdown") != ();
    }
    if tool == "get_desktop_state" {
        summary["bounds"] = ostr(mget(doc, "bounds"));
        summary["image"] = find_image_note(doc);
    }
    return #{
        status: status, ok: status == "ok", tool: tool, exit_code: exit_code,
        remedy: "", summary: summary, data: strip_images(doc, 0)
    };
}

fn brief_elements(elems, limit) {
    let out = [];
    let i = 0;
    while i < elems.len() && out.len() < limit {
        out.push(#{
            token: mstr(elems[i], "element_token"),
            text: brief_one(element_text(elems[i]))
        });
        i += 1;
    }
    return out;
}

fn find_image_note(v) {
    if type_of(v) == "map" {
        for k in v.keys() {
            if v[k] != () && type_of(v[k]) == "map" && v[k]["image"] == true {
                return v[k];
            }
            let deep = find_image_note(v[k]);
            if deep != () { return deep; }
        }
    } else if type_of(v) == "array" {
        for x in v {
            let deep = find_image_note(x);
            if deep != () { return deep; }
        }
    }
    return ();
}
