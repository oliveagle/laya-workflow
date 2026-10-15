//! P164 — which `with.<key>` keys each capability kind actually reads.
//!
//! A `call` step hands the engine a `with` object. Every kind reads a fixed set
//! of keys out of it and ignores the rest, so a mistyped key (`with.modes`) is a
//! silent no-op: the engine takes its default and the call runs to completion
//! with the wrong behaviour. The compiler cannot see this - the key is just data
//! in a JSON object - so `bdd build` stays green. This module is the contract
//! that lets the compiler refuse a key no kind reads.
//!
//! It is rebuilt by scanning the capability sources the same way `[P160]` scans
//! `parse.rs`: the read sites are the truth, and the scan re-derives them so a
//! new `with.get("k")` is picked up without a table to drift. Two directions are
//! unioned, because a kind reads `with` in only one of them:
//!
//! * a literal `with.get("k")` in the kind's dispatch arm or any function it
//!   reaches with `with` in hand (the transitive closure, so a helper that reads
//!   `with.get("k")` counts for its caller);
//! * the key arguments of a helper that takes `with` as an argument
//!   (`get_text(with, "text")`, `str_field(cap, with, state, "k")`) and, when a
//!   body reads `with` under a *dynamic* key (a captured closure),
//!   every identifier-shaped literal passed to any call in that body.
//!
//! Not every kind has a closed set. A `plugin` gets the whole `with` object its
//! script reads, and an `agent` forwards the whole object as the request body -
//! any key is legal there, so [`kind_reads`] returns `None` and the compiler
//! stays silent. Every other kind returns `Some(set)`.

use regex_lite::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// A kind whose `with` payload is not a closed set of literal keys.
const OPEN: &[&str] = &["plugin", "agent"];

/// Fold a kind alias to the canonical name the read sets are keyed by. Mirrors
/// `parse::canon_kind`; the compiler folds the same way before it looks the set
/// up, so a corpus that writes `chrome_cdp` resolves to `browser`'s reads.
pub fn canon_kind(k: &str) -> &str {
    match k {
        "md" => "markdown",
        "tokens" => "tokenize",
        "kv" => "keyvalue",
        "database" => "sql",
        "computer-use" => "computer_use",
        "notify_local" => "notify",
        "mail_send" => "smtp",
        "app_server" => "agent",
        "chrome_cdp" => "browser",
        "fetch_url" => "web_fetch",
        "search" => "web_search",
        "object_store" => "s3",
        "sysinfo" => "metrics",
        "agent_goal" => "goal_runner",
        other => other,
    }
}

/// The `with` keys kind `kind` reads, or `None` when the kind is open
/// (`plugin` / `agent`) and any key is legal.
pub fn kind_reads(kind: &str) -> Option<&'static BTreeSet<String>> {
    reads().get(canon_kind(kind))
}

/// `true` when `kind` has no closed read set, so the compiler must not judge it.
pub fn is_open(kind: &str) -> bool {
    reads().get(canon_kind(kind)).is_none()
}

fn reads() -> &'static BTreeMap<String, BTreeSet<String>> {
    static SETS: OnceLock<BTreeMap<String, BTreeSet<String>>> = OnceLock::new();
    SETS.get_or_init(build)
}

const SOURCES: &[(&str, &str)] = &[
    ("mod", include_str!("mod.rs")),
    ("parse", include_str!("parse.rs")),
    ("browser", include_str!("browser.rs")),
    ("computer_use", include_str!("computer_use.rs")),
    ("data", include_str!("data.rs")),
    ("db", include_str!("db.rs")),
    ("goal", include_str!("goal.rs")),
    ("human", include_str!("human.rs")),
    ("local", include_str!("local.rs")),
    ("math", include_str!("math.rs")),
    ("needle", include_str!("needle.rs")),
    ("net", include_str!("net.rs")),
    ("plugin", include_str!("plugin.rs")),
    ("proto", include_str!("proto.rs")),
    ("secret", include_str!("secret.rs")),
    ("service", include_str!("service.rs")),
    ("store", include_str!("store.rs")),
    ("sys", include_str!("sys.rs")),
    ("util", include_str!("util.rs")),
    ("web", include_str!("web.rs")),
];

/// The compiled patterns, built once. `regex_lite` has no `\b`, so whole-word
/// `with` is matched by hand in [`has_word`].
struct Rx {
    fn_sig: Regex,
    kind_var: Regex,
    kind_lit: Regex,
    var_fn: Regex,
    with_get_lit: Regex,
    with_get_dyn: Regex,
    call: Regex,
    str_lit: Regex,
    ident: Regex,
}

fn rx() -> &'static Rx {
    static R: OnceLock<Rx> = OnceLock::new();
    R.get_or_init(|| Rx {
        fn_sig: Regex::new(r"(?m)^\s*(?:pub\s+)?fn\s+(\w+)\s*[(<]").unwrap(),
        kind_var: Regex::new(
            r#"((?:"[a-z0-9_-]+"\s*\|\s*)*"[a-z0-9_-]+")\s*=>\s*Ok\(Capability::(\w+)"#,
        )
        .unwrap(),
        kind_lit: Regex::new(r#""([a-z0-9_-]+)""#).unwrap(),
        var_fn: Regex::new(
            r#"Capability::(\w+)\s*\(\s*c\s*\)\s*=>\s*(?:([a-z_]+)::)?(\w+)\s*\("#,
        )
        .unwrap(),
        with_get_lit: Regex::new(r#"with\s*\.\s*get\s*\(\s*"([^"]+)""#).unwrap(),
        with_get_dyn: Regex::new(r#"with\s*\.\s*get\s*\(\s*[^"]"#).unwrap(),
        call: Regex::new(r"([A-Za-z_][A-Za-z0-9_]*(?:::\w+)?)\s*\(").unwrap(),
        str_lit: Regex::new(r#""([^"\\]*)""#).unwrap(),
        ident: Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap(),
    })
}

fn build() -> BTreeMap<String, BTreeSet<String>> {
    let r = rx();
    // Comment-free, string-intact sources. Comments are blanked so a `with.get`
    // in a doc comment cannot contribute; strings are left so a literal key can.
    let stripped: Vec<(&str, String)> =
        SOURCES.iter().map(|(n, s)| (*n, strip_comments(s))).collect();
    let src_of = |name: &str| stripped.iter().find(|(n, _)| *n == name).map(|(_, s)| s.as_str());

    let mut funcs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (_, src) in &stripped {
        for m in r.fn_sig.captures_iter(src) {
            let whole = m.get(0).unwrap();
            let name = m.get(1).unwrap().as_str().to_string();
            if let Some(body) = body_after(src, whole.end()) {
                funcs.entry(name).or_default().push(body);
            }
        }
    }

    // kind string -> Capability variant (parse_kind), variant -> module::fn (dispatch)
    let mut kind2var: BTreeMap<String, String> = BTreeMap::new();
    if let Some(p) = src_of("parse") {
        for m in r.kind_var.captures_iter(p) {
            let var = m.get(2).unwrap().as_str().to_string();
            for k in r.kind_lit.captures_iter(m.get(1).unwrap().as_str()) {
                kind2var.entry(k[1].to_string()).or_insert_with(|| var.clone());
            }
        }
    }
    let mut var2fn: BTreeMap<String, (String, String)> = BTreeMap::new();
    if let Some(p) = src_of("mod") {
        for m in r.var_fn.captures_iter(p) {
            let var = m[1].to_string();
            let module = m.get(2).map(|x| x.as_str()).unwrap_or("mod").to_string();
            let f = m[3].to_string();
            var2fn.entry(var).or_insert((module, f));
        }
    }

    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (kind, var) in &kind2var {
        let ck = canon_kind(kind);
        if OPEN.contains(&ck) {
            continue; // an open kind is absent from the map
        }
        let keys = match var2fn.get(var) {
            Some((_, f)) => {
                let mut seen: BTreeSet<String> = BTreeSet::new();
                reads_for(f, &funcs, &mut seen)
            }
            None => BTreeSet::new(),
        };
        out.entry(ck.to_string()).or_default().extend(keys);
    }
    for k in OPEN {
        out.remove(*k);
    }
    out
}

/// The body of a function, from the `{` that opens it to its matching `}`.
fn body_after(src: &str, from: usize) -> Option<String> {
    let b = src[from..].find('{')? + from;
    let bytes = src.as_bytes();
    let mut depth = 0i32;
    let mut i = b;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(src[b..=i].to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The literal keys a function reads out of `with`, plus those of everything it
/// reaches with `with` in hand.
fn reads_for(
    name: &str,
    funcs: &BTreeMap<String, Vec<String>>,
    seen: &mut BTreeSet<String>,
) -> BTreeSet<String> {
    let r = rx();
    let mut res = BTreeSet::new();
    if !seen.insert(name.to_string()) {
        return res;
    }
    let Some(bodies) = funcs.get(name) else {
        return res;
    };
    for body in bodies {
        for c in r.with_get_lit.captures_iter(body) {
            res.insert(c[1].to_string());
        }
        // A dynamic `with.get(<not-a-literal>)` means the key comes from a
        // captured value (a local closure): over-approximate with every
        // identifier literal the body passes to any call.
        if r.with_get_dyn.is_match(body) {
            for c in r.call.captures_iter(body) {
                let args = arg_span(body, c.get(0).unwrap().end() - 1);
                res.extend(ident_literals(&args));
            }
        }
        for c in r.call.captures_iter(body) {
            let called = c[1].rsplit("::").next().unwrap().to_string();
            let args = arg_span(body, c.get(0).unwrap().end() - 1);
            if !has_word(&args, "with") {
                continue;
            }
            if called == "expand" || called == "expand_str" {
                continue;
            }
            if called == "effective_op" {
                res.insert("op".to_string());
            }
            res.extend(ident_literals(&args));
            res.extend(reads_for(&called, funcs, seen));
        }
    }
    res
}

/// The argument list of the call whose `(` is at byte `open`.
fn arg_span(src: &str, open: usize) -> String {
    let bytes = src.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return String::new();
    }
    let mut depth = 0i32;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return src[open + 1..i].to_string();
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'"' {
                        break;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    String::new()
}

/// Every `"…"` literal in `s` that is a bare identifier (`text`, `op`, `id_key`).
fn ident_literals(s: &str) -> Vec<String> {
    let r = rx();
    r.str_lit
        .captures_iter(s)
        .map(|c| c[1].to_string())
        .filter(|k| r.ident.is_match(k))
        .collect()
}

/// `needle` as a whole word in `hay` (an identifier boundary on both sides).
fn has_word(hay: &str, needle: &str) -> bool {
    let b = hay.as_bytes();
    let n = needle.len();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut i = 0;
    while let Some(p) = hay[i..].find(needle) {
        let at = i + p;
        let left_ok = at == 0 || !ident(b[at - 1]);
        let right = at + n;
        let right_ok = right >= b.len() || !ident(b[right]);
        if left_ok && right_ok {
            return true;
        }
        i = at + n;
    }
    false
}

/// Blank `//` and `/* … */` comments so a `with.get` inside a comment cannot
/// contribute, while leaving string literals intact (a literal key lives there).
fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                out.push(b' ');
                i += 1;
            }
        } else if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            out.push(b' ');
            out.push(b' ');
            i += 2;
            while i < b.len() && !(b[i] == b'*' && i + 1 < b.len() && b[i + 1] == b'/') {
                out.push(if b[i] == b'\n' { b'\n' } else { b' ' });
                i += 1;
            }
            if i < b.len() {
                out.push(b' ');
                out.push(b' ');
                i += 2;
            }
        } else if b[i] == b'"' {
            out.push(b'"');
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' && i + 1 < b.len() {
                    out.push(b[i]);
                    out.push(b[i + 1]);
                    i += 2;
                    continue;
                }
                out.push(b[i]);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| src.to_string())
}

