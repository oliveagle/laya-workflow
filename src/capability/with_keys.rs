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
use serde_json::Value;
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
    /// The `fn` signature shape, matched against a single line (see [`fn_sigs`]).
    /// The whole-file `(?m)` form it replaced lives in the tests, which pin
    /// [`fn_sigs`] against it.
    fn_sig_one: Regex,
    kind_var: Regex,
    kind_lit: Regex,
    var_fn: Regex,
    str_lit: Regex,
    ident: Regex,
    as_ty: Regex,
    effective_op: Regex,
    str_field_lit: Regex,
    get_text_lit: Regex,
    clamp_args: Regex,
}

fn rx() -> &'static Rx {
    static R: OnceLock<Rx> = OnceLock::new();
    R.get_or_init(|| Rx {
        fn_sig_one: Regex::new(r"^\s*(?:pub\s+)?fn\s+(\w+)\s*[(<]").unwrap(),
        kind_var: Regex::new(
            r#"((?:"[a-z0-9_-]+"\s*\|\s*)*"[a-z0-9_-]+")\s*=>\s*Ok\(Capability::(\w+)"#,
        )
        .unwrap(),
        kind_lit: Regex::new(r#""([a-z0-9_-]+)""#).unwrap(),
        var_fn: Regex::new(
            r#"Capability::(\w+)\s*\(\s*c\s*\)\s*=>\s*(?:([a-z_]+)::)?(\w+)\s*\("#,
        )
        .unwrap(),
        str_lit: Regex::new(r#""([^"\\]*)""#).unwrap(),
        ident: Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap(),
        // `regex_lite` has no `\b`; a trailing `([^A-Za-z0-9_]|$)` is the whole-word
        // boundary for the accessor names below.
        as_ty: Regex::new(r"as_(u64|i64|f64|str|bool|array|object)([^A-Za-z0-9_]|$)").unwrap(),
        effective_op: Regex::new(r"effective_op\s*\(").unwrap(),
        str_field_lit: Regex::new(r#"str_field\s*\([^,]*,\s*&?with\s*,\s*[^,]*,\s*"([^"]+)""#)
            .unwrap(),
        get_text_lit: Regex::new(r#"get_text\s*\(\s*&?with\s*,\s*"([^"]+)""#).unwrap(),
        // P175: the two arguments of the `clamp` a read site applies. Either may
        // be a non-literal (`clamp(result_count as u64, 30)`), so the bounds are
        // parsed as literals and a non-literal side is simply not promised.
        clamp_args: Regex::new(r"\.clamp\(\s*([^()]*),([^()]*)\)").unwrap(),
    })
}

fn build() -> BTreeMap<String, BTreeSet<String>> {
    let s = sources();
    let (kind2var, var2fn) = dispatch();

    let mut funcs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (_, src) in &s.stripped {
        for (name, end) in fn_sigs(src) {
            if let Some(body) = body_after(src, end) {
                funcs.entry(name).or_default().push(body);
            }
        }
    }

    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (kind, var) in kind2var {
        let ck = canon_kind(kind);
        if OPEN.contains(&ck) {
            continue; // an open kind is absent from the map
        }
        // A variant with no dispatch fn of the `Capability::X(c) => f(c, ..)`
        // shape is not a set the sources can give us: the unit variant
        // `Passthrough` (kinds `passthrough` / `noop`) returns the whole expanded
        // `with`, so *every* key is read and any key is legal. Leaving it absent
        // from the map is the same as an open kind - an empty set would make the
        // guard refuse a key it cannot prove is unread.
        let Some((_, f)) = var2fn.get(var) else {
            continue;
        };
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let keys = reads_for(f, &funcs, &mut seen);
        out.entry(ck.to_string()).or_default().extend(keys);
    }
    for k in OPEN {
        out.remove(*k);
    }
    out
}

/// The capability sources, comment-blanked, plus the same text with every
/// whitespace character removed.
///
/// The P164 / P166 tables used to re-strip every source and re-scan all of them
/// with one regex each; `compact` lets a `with\s*\.\s*get\s*\(` scan be gated by a
/// plain substring search for `with.get(`, which is a *necessary* condition: it
/// can only ever skip a source the regex would not have matched. Same for the
/// other whole-source scans, which gate on a plain `contains` of their literal.
struct Sources {
    stripped: Vec<(&'static str, String)>,
    compact: Vec<(&'static str, String)>,
}

fn sources() -> &'static Sources {
    static S: OnceLock<Sources> = OnceLock::new();
    S.get_or_init(|| {
        let stripped: Vec<(&'static str, String)> =
            SOURCES.iter().map(|(n, s)| (*n, strip_comments(s))).collect();
        let compact: Vec<(&'static str, String)> =
            stripped.iter().map(|(n, s)| (*n, compact_ws(s))).collect();
        Sources { stripped, compact }
    })
}

impl Sources {
    fn src(&self, name: &str) -> Option<&str> {
        self.stripped.iter().find(|(n, _)| *n == name).map(|(_, s)| s.as_str())
    }

    /// The whitespace-stripped form of source `name`, or `""` for a name that is
    /// not a source (which makes the necessary-condition gates below skip, never
    /// run).
    fn compact_of(&self, name: &str) -> &str {
        self.compact.iter().find(|(n, _)| *n == name).map(|(_, s)| s.as_str()).unwrap_or("")
    }
}

fn compact_ws(src: &str) -> String {
    src.chars().filter(|c| !c.is_whitespace()).collect()
}

/// `regex_lite`'s `\s`: the ASCII whitespace class (it carries no Unicode
/// classes). Spelled out so the scanners below agree with the patterns they
/// replaced on every byte, not just on ` `.
#[inline]
fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && is_ws(b[i]) {
        i += 1;
    }
    i
}

/// Match `with\s*\.\s*get\s*\(` at byte `p`; return the offset just past the
/// `(`.
///
/// A hand-written scan rather than the `regex_lite` pattern it replaced. The
/// whole-source scan runs once per source per table (and once per reached body
/// per kind in [`reads_for`]), and `regex_lite` is a backtracking engine: the
/// pattern cost ~23 ms per pass over the ~780 KB of capability sources, where
/// this costs ~0.5 ms. [`scanners_match_the_regexes`] pins the equivalence on
/// every source.
fn match_with_get_prefix(b: &[u8], p: usize) -> Option<usize> {
    let mut i = p + 4; // past `with`
    i = skip_ws(b, i);
    if b.get(i) != Some(&b'.') {
        return None;
    }
    i = skip_ws(b, i + 1);
    if b.get(i..i + 3) != Some(b"get") {
        return None;
    }
    i = skip_ws(b, i + 3);
    if b.get(i) != Some(&b'(') {
        return None;
    }
    Some(i + 1)
}

/// Every literal read site: `(key, offset just past the closing quote)`.
/// Equivalent to `with\s*\.\s*get\s*\(\s*"([^"]+)"` - non-overlapping, left to
/// right, and the key is the run up to the first `"`.
fn scan_with_get(src: &str) -> Vec<(&str, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(rel) = src[i..].find("with") {
        let p = i + rel;
        let mut hit = None;
        if let Some(paren) = match_with_get_prefix(b, p) {
            let q = skip_ws(b, paren);
            if b.get(q) == Some(&b'"') {
                let start = q + 1;
                let mut j = start;
                while j < b.len() && b[j] != b'"' {
                    j += 1;
                }
                if j > start && j < b.len() {
                    hit = Some((start, j, j + 1));
                }
            }
        }
        match hit {
            Some((s, e, end)) => {
                out.push((&src[s..e], end));
                i = end;
            }
            None => i = p + 1,
        }
    }
    out
}

/// Does any read site take its key from a value rather than a literal - the
/// `with\s*\.\s*get\s*\(\s*[^"]` shape. `\s*` may land on the byte right after
/// `(`, so the pattern matches exactly when that byte is not the opening quote.
fn has_with_get_dyn(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0usize;
    while let Some(rel) = src[i..].find("with") {
        let p = i + rel;
        if let Some(paren) = match_with_get_prefix(b, p) {
            if b.get(paren).is_some_and(|c| *c != b'"') {
                return true;
            }
        }
        i = p + 1;
    }
    false
}

/// Every call site as `(callee, offset of the '(')`. Equivalent to
/// `([A-Za-z_][A-Za-z0-9_]*(?:::\w+)?)\s*\(` - the `::` group is greedy but does
/// not span a second `::`, so `a::b::c(` yields `b::c`.
fn scan_calls(src: &str) -> Vec<(&str, usize)> {
    let b = src.as_bytes();
    let ident0 = |c: u8| c.is_ascii_alphabetic() || c == b'_';
    let identn = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        if !ident0(b[i]) {
            i += 1;
            continue;
        }
        let s = i;
        let mut j = i + 1;
        while j < b.len() && identn(b[j]) {
            j += 1;
        }
        let mut name_end = j;
        if b.get(j) == Some(&b':') && b.get(j + 1) == Some(&b':') {
            let mut k = j + 2;
            while k < b.len() && identn(b[k]) {
                k += 1;
            }
            if k > j + 2 {
                name_end = k;
            }
        }
        let m = skip_ws(b, name_end);
        if b.get(m) == Some(&b'(') {
            out.push((&src[s..name_end], m));
            i = m + 1;
        } else {
            i = s + 1;
        }
    }
    out
}

/// Every `fn` definition in `src`, as `(name, offset just past the signature)`.
///
/// A line scan rather than a `(?m)` regex over the whole file: `fn` opens a
/// definition only where the line (after optional spaces and a `pub`) begins
/// with it, which is exactly where `(?m)^\s*(?:pub\s+)?fn` can anchor, and the
/// name and end offset come out identical either way. `fn_sigs_matches_the_regex`
/// pins that for every source.
fn fn_sigs(src: &str) -> Vec<(String, usize)> {
    let r = rx();
    let mut out = Vec::new();
    let mut off = 0usize;
    for line in src.split_inclusive('\n') {
        let t = line.trim_start();
        let body = match t.strip_prefix("pub") {
            Some(rest) if rest.starts_with(char::is_whitespace) => rest.trim_start(),
            _ => t,
        };
        if body.starts_with("fn") {
            if let Some(m) = r.fn_sig_one.captures(line) {
                out.push((m[1].to_string(), off + m.get(0).unwrap().end()));
            }
        }
        off += line.len();
    }
    out
}

/// `parse_kind`'s `"kind" => Ok(Capability::Variant)` table and the dispatch table
/// `Capability::Variant(c) => module::fn(c, ..)`. [`build`] needs the dispatched
/// function, [`kind_module`] needs its module, and both derive from these same two
/// scans - so they happen once, not once per table.
fn dispatch() -> &'static (BTreeMap<String, String>, BTreeMap<String, (String, String)>) {
    static D: OnceLock<(BTreeMap<String, String>, BTreeMap<String, (String, String)>)> =
        OnceLock::new();
    D.get_or_init(|| {
        let r = rx();
        let s = sources();
        let mut kind2var: BTreeMap<String, String> = BTreeMap::new();
        if let Some(p) = s.src("parse") {
            if p.contains("Ok(Capability::") {
                for m in r.kind_var.captures_iter(p) {
                    let var = m.get(2).unwrap().as_str().to_string();
                    for k in r.kind_lit.captures_iter(m.get(1).unwrap().as_str()) {
                        kind2var.entry(k[1].to_string()).or_insert_with(|| var.clone());
                    }
                }
            }
        }
        let mut var2fn: BTreeMap<String, (String, String)> = BTreeMap::new();
        if let Some(p) = s.src("mod") {
            if p.contains("Capability::") {
                for m in r.var_fn.captures_iter(p) {
                    let var = m[1].to_string();
                    let module = m.get(2).map(|x| x.as_str()).unwrap_or("mod").to_string();
                    let f = m[3].to_string();
                    var2fn.entry(var).or_insert((module, f));
                }
            }
        }
        (kind2var, var2fn)
    })
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
    let mut res = BTreeSet::new();
    if !seen.insert(name.to_string()) {
        return res;
    }
    let Some(bodies) = funcs.get(name) else {
        return res;
    };
    for body in bodies {
        for (key, _) in scan_with_get(body) {
            res.insert(key.to_string());
        }
        // A dynamic `with.get(<not-a-literal>)` means the key comes from a
        // captured value (a local closure): over-approximate with every
        // identifier literal the body passes to any call.
        if has_with_get_dyn(body) {
            for (_, open) in scan_calls(body) {
                let (a, z) = arg_span(body, open);
                res.extend(ident_literals(&body[a..z]));
            }
        }
        for (callee, open) in scan_calls(body) {
            let called = callee.rsplit("::").next().unwrap();
            let (a, z) = arg_span(body, open);
            let args = &body[a..z];
            if !has_word(args, "with") {
                continue;
            }
            if called == "expand" || called == "expand_str" {
                continue;
            }
            if called == "effective_op" {
                res.insert("op".to_string());
            }
            res.extend(ident_literals(args));
            res.extend(reads_for(called, funcs, seen));
        }
    }
    res
}

/// The byte range just inside the parens of the call whose `(` is at byte
/// `open` - `(start, end)`, sliced as `&src[start..end]`. A call whose parens do
/// not close yields `(0, 0)`, the same empty slice the owned form returned.
fn arg_span(src: &str, open: usize) -> (usize, usize) {
    let bytes = src.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return (0, 0);
    }
    let mut depth = 0i32;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return (open + 1, i);
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
    (0, 0)
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

// ── P166: the JSON type each kind's `with` read site wants ──────────────────

/// The JSON type a `with.<key>` read requires, when **every** read of that key
/// in the kind's module is typed and they agree. A key read anywhere with
/// `stringify` / `.cloned()` / `json!` accepts any JSON value, so it is absent
/// from the map - the compiler must not judge it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadTy {
    Str,
    U64,
    I64,
    F64,
    Bool,
    StrArray,
    Object,
}

impl ReadTy {
    /// What the read site wants, for the refusal message.
    fn name(self) -> &'static str {
        match self {
            ReadTy::Str => "a string",
            ReadTy::U64 => "a non-negative integer",
            ReadTy::I64 => "an integer",
            ReadTy::F64 => "a number",
            ReadTy::Bool => "a boolean",
            ReadTy::StrArray => "an array",
            ReadTy::Object => "a JSON object",
        }
    }

    fn accepts(self, v: &Value) -> bool {
        match self {
            ReadTy::Str => v.is_string(),
            ReadTy::U64 => v.as_u64().is_some(),
            ReadTy::I64 => v.as_i64().is_some(),
            ReadTy::F64 => v.as_f64().is_some(),
            ReadTy::Bool => v.is_boolean(),
            ReadTy::StrArray => v.is_array(),
            ReadTy::Object => v.is_object(),
        }
    }
}

/// A `call` step hands `with.<key>` a value whose JSON type the kind's read site
/// does not accept, so the engine takes its default and the value the author set
/// is silently discarded (P166). `None` when the compiler must not judge it: an
/// open kind, a key no read types, or a key read untyped somewhere.
pub fn with_value_rejected(kind: &str, key: &str, value: &Value) -> Option<&'static str> {
    let ty = with_types().get(canon_kind(kind))?.get(key)?;
    if ty.accepts(value) {
        return None;
    }
    Some(ty.name())
}

fn with_types() -> &'static BTreeMap<String, BTreeMap<String, ReadTy>> {
    static T: OnceLock<BTreeMap<String, BTreeMap<String, ReadTy>>> = OnceLock::new();
    T.get_or_init(build_types)
}

/// `kind` -> the module stem its dispatch arm lives in (the file whose reads
/// define the kind's `with` contract).
fn kind_module() -> &'static BTreeMap<String, String> {
    static M: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    M.get_or_init(|| {
        let (kind2var, var2fn) = dispatch();
        let mut out: BTreeMap<String, String> = BTreeMap::new();
        for (kind, var) in kind2var {
            if let Some((module, _)) = var2fn.get(var) {
                out.insert(canon_kind(kind).to_string(), module.clone());
            }
        }
        out
    })
}

/// A read either wants a type or accepts anything. Two reads that disagree mean
/// the module does not speak with one voice, so the key is not judgeable.
fn note_ty(m: &mut BTreeMap<String, Option<ReadTy>>, key: &str, ty: Option<ReadTy>) {
    match m.get_mut(key) {
        Some(cur) => {
            if *cur != ty {
                *cur = None;
            }
        }
        None => {
            m.insert(key.to_string(), ty);
        }
    }
}

fn build_types() -> BTreeMap<String, BTreeMap<String, ReadTy>> {
    let r = rx();
    let s = sources();
    let mut per_mod: BTreeMap<String, BTreeMap<String, Option<ReadTy>>> = BTreeMap::new();
    for (name, full) in &s.stripped {
        // Only the non-test half speaks for the engine: a typed `with.get` inside
        // a `#[cfg(test)]` module would otherwise pin a key nothing read types.
        let cut = full.find("#[cfg(test)]").unwrap_or(full.len());
        let src = &full[..cut];
        let m = per_mod.entry((*name).to_string()).or_default();
        // A literal `with.get("` can only survive where `with.get(` does once
        // whitespace is gone - a necessary condition, so this gate can only skip a
        // source the scan would not have matched. (`sources()` already holds the
        // stripped form; the gate used to recompute it here, per table.)
        if s.compact_of(name).contains("with.get(\"") {
            for (key, from) in scan_with_get(src) {
                let seg = read_segment(src, from);
                note_ty(m, key, ty_in(&seg));
            }
        }
        // `effective_op(op, with, ...)` reads `with.op` as a string, and only a
        // non-empty string overrides the definition (P: `queue` op override).
        if src.contains("effective_op") {
            for _ in r.effective_op.captures_iter(src) {
                note_ty(m, "op", Some(ReadTy::Str));
            }
        }
        if src.contains("str_field") {
            for c in r.str_field_lit.captures_iter(src) {
                note_ty(m, &c[1].to_string(), Some(ReadTy::Str));
            }
        }
        // `get_text` runs the value through `stringify`, so any JSON type is
        // accepted - the key must not be judged.
        if src.contains("get_text") {
            for c in r.get_text_lit.captures_iter(src) {
                note_ty(m, &c[1].to_string(), None);
            }
        }
    }
    let km = kind_module();
    let mut out: BTreeMap<String, BTreeMap<String, ReadTy>> = BTreeMap::new();
    for (kind, module) in km {
        if OPEN.contains(&kind.as_str()) {
            continue;
        }
        let Some(m) = per_mod.get(module) else { continue };
        let typed: BTreeMap<String, ReadTy> =
            m.iter().filter_map(|(k, v)| v.map(|t| (k.clone(), t))).collect();
        if !typed.is_empty() {
            out.insert(kind.clone(), typed);
        }
    }
    out
}

/// The text right after `with.get("…")`, up to the end of the read expression:
/// the end of the statement, the next read, or a line-break comma. What follows
/// the read is what types it.
fn read_segment(src: &str, from: usize) -> String {
    let end = src.len().min(from + 160);
    let tail = &src[from..end];
    let mut cut = tail.find(';').unwrap_or(tail.len());
    if let Some(i) = tail.find(",\n") {
        cut = cut.min(i);
    }
    if let Some(i) = tail.find("with.get") {
        cut = cut.min(i);
    }
    tail[..cut].to_string()
}

/// The JSON type every `as_*` accessor in a read segment wants, or `None` when
/// there is none (the read accepts anything) or they disagree.
fn ty_in(seg: &str) -> Option<ReadTy> {
    let r = rx();
    let mut found: Option<ReadTy> = None;
    for c in r.as_ty.captures_iter(seg) {
        let t = match &c[1] {
            "u64" => ReadTy::U64,
            "i64" => ReadTy::I64,
            "f64" => ReadTy::F64,
            "str" => ReadTy::Str,
            "bool" => ReadTy::Bool,
            "array" => ReadTy::StrArray,
            _ => ReadTy::Object,
        };
        match found {
            None => found = Some(t),
            Some(prev) if prev != t => return None,
            _ => {}
        }
    }
    found
}

// ── P175: the numeric band a `with.<key>` read site clamps to ───────────────

/// The band a read site clamps a value into. Either bound is `None` when the
/// clamp's argument is not a literal - `clamp(result_count as u64, 30)` - so only
/// the literal side is something the reader can be held to.
#[derive(Clone, Copy)]
struct Band {
    lo: Option<f64>,
    hi: Option<f64>,
}

impl Band {
    fn empty(self) -> bool {
        self.lo.is_none() && self.hi.is_none()
    }

    fn exceeds(self, n: f64) -> bool {
        self.lo.is_some_and(|lo| n < lo) || self.hi.is_some_and(|hi| n > hi)
    }

    /// The band as the refusal message writes it.
    fn describe(self) -> String {
        match (self.lo, self.hi) {
            (Some(lo), Some(hi)) => format!("{}..={}", num(lo), num(hi)),
            (Some(lo), None) => format!("at least {}", num(lo)),
            (None, Some(hi)) => format!("at most {}", num(hi)),
            (None, None) => "no bound".to_string(),
        }
    }
}

/// A bound as the message writes it: `20` for `20.0`, `0.5` for `0.5`.
fn num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// A bound literal, or `None` when the argument is an expression.
fn num_lit(s: &str) -> Option<f64> {
    let t = s.trim().replace('_', "");
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    t.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// A `call` step hands `with.<key>` a number the read site clamps, so the engine
/// replaces it and the call runs with a different value (P175).
///
/// The read site is the truth: a key is judged only when the kind's module reads
/// it at **exactly one** place and that read clamps with a literal bound. One
/// read means the clamp is what every op sees, so the band is a promise - a
/// second read (with its own band, or none) means the compiler cannot know which
/// op will run and must stay silent. `None` also covers an open kind, a key
/// whose read has no literal clamp, and a value that is not a number (a string
/// is the *type* guard's business, P166).
pub fn with_range_rejected(kind: &str, key: &str, value: &Value) -> Option<String> {
    let band = *with_bands().get(canon_kind(kind))?.get(key)?;
    let n = value.as_f64()?;
    if !band.exceeds(n) {
        return None;
    }
    Some(band.describe())
}

/// The judged keys of `kind` with the band each is clamped to, as the guard
/// writes it (P175's drift guard pins this against the sources).
pub fn with_band_keys(kind: &str) -> Vec<(String, String)> {
    with_bands()
        .get(canon_kind(kind))
        .map(|m| m.iter().map(|(k, b)| (k.clone(), b.describe())).collect())
        .unwrap_or_default()
}

fn with_bands() -> &'static BTreeMap<String, BTreeMap<String, Band>> {
    static B: OnceLock<BTreeMap<String, BTreeMap<String, Band>>> = OnceLock::new();
    B.get_or_init(build_bands)
}

fn build_bands() -> BTreeMap<String, BTreeMap<String, Band>> {
    let s = sources();
    // module -> key -> (how many read sites, the band of the first one)
    let mut per_mod: BTreeMap<String, BTreeMap<String, (usize, Option<Band>)>> =
        BTreeMap::new();
    for (name, full) in &s.stripped {
        let cut = full.find("#[cfg(test)]").unwrap_or(full.len());
        let src = &full[..cut];
        let m = per_mod.entry((*name).to_string()).or_default();
        // Same necessary-condition gate as `build_types`, off the cached
        // whitespace-stripped copy rather than a fresh one per table.
        if s.compact_of(name).contains("with.get(\"") {
            for (key, from) in scan_with_get(src) {
                let seg = band_segment(src, from);
                let band = band_in(&seg);
                let e = m.entry(key.to_string()).or_insert((0, None));
                if e.0 == 0 {
                    e.1 = band;
                }
                e.0 += 1;
            }
        }
    }
    let km = kind_module();
    let mut out: BTreeMap<String, BTreeMap<String, Band>> = BTreeMap::new();
    for (kind, module) in km {
        if OPEN.contains(&kind.as_str()) {
            continue;
        }
        let Some(m) = per_mod.get(module) else { continue };
        let judged: BTreeMap<String, Band> = m
            .iter()
            .filter_map(|(k, (sites, band))| match band {
                Some(b) if *sites == 1 && !b.empty() => Some((k.clone(), *b)),
                _ => None,
            })
            .collect();
        if !judged.is_empty() {
            out.insert(kind.clone(), judged);
        }
    }
    out
}

/// The literal clamp a read segment applies, if any. Only the first `clamp` of
/// the read counts; a bound that is not a literal stays `None`.
fn band_in(seg: &str) -> Option<Band> {
    let r = rx();
    let c = r.clamp_args.captures(seg)?;
    Some(Band { lo: num_lit(&c[1]), hi: num_lit(&c[2]) })
}

/// Like [`read_segment`] but for the *band*: it does not stop at a following
/// `with.get` when that one is an alias in the same read
/// (`with.get("tab_concurrency").or_else(|| with.get("concurrency"))`) - the
/// clamp comes after the alias and applies to both names.
fn band_segment(src: &str, from: usize) -> String {
    let end = src.len().min(from + 240);
    let tail = &src[from..end];
    let mut cut = tail.find(';').unwrap_or(tail.len());
    if let Some(i) = tail.find(",\n") {
        cut = cut.min(i);
    }
    let mut at = 0usize;
    while let Some(p) = tail[at..].find("with.get") {
        let i = at + p;
        if is_alias_read(&tail[..i]) {
            at = i + "with".len();
            continue;
        }
        cut = cut.min(i);
        break;
    }
    tail[..cut].to_string()
}

/// `… .or_else(|| ` right before a `with.get` - the same read under a second
/// name, not a second read.
fn is_alias_read(before: &str) -> bool {
    let t: String = before.chars().filter(|c| !c.is_whitespace()).collect();
    t.ends_with("or_else(||")
}

// ───────────────────────────── P177 ─────────────────────────────
// The one decision both guards share. P164/P166/P175 first lived only on the BDD
// compile path (`bdd.rs`), so a hand-written or side-loaded JSON spec reached
// `validate` with none of them: a `with.<key>` no call of the kind reads, or a
// value of the wrong type / outside the read site's band, passed
// `laya-workflow validate --spec` with exit 0. Rather than keep two copies - which
// drift the day a read site changes - the decision lives here once and both
// callers ([`with_step_issue`] + [`with_issue_body`]) format its verdict.

/// Every `X` a value names with a `${with.X}` reference, at any depth. A key that
/// *is* referenced (from another `with` value, or from the capability's own
/// definition - `${with.text}` inside a `body`) counts as read even when no
/// `with.get` literal names it.
pub fn with_refs_in(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::String(s) => {
            let mut rest = s.as_str();
            while let Some(i) = rest.find("${with.") {
                let after = &rest[i + "${with.".len()..];
                let name: String = after
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if name.is_empty() {
                    if after.is_empty() {
                        break;
                    }
                    rest = &after[1..];
                } else {
                    rest = &after[name.len()..];
                    out.insert(name);
                }
            }
        }
        Value::Array(a) => {
            for x in a {
                with_refs_in(x, out);
            }
        }
        Value::Object(o) => {
            for x in o.values() {
                with_refs_in(x, out);
            }
        }
        _ => {}
    }
}

/// `true` when `a` and `b` are one edit apart (a transposition counts, so
/// `moed`/`mode` is caught too).
pub fn one_apart(a: &str, b: &str) -> bool {
    if a == b {
        return false;
    }
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let (n, m) = (a.len(), b.len());
    if n.abs_diff(m) > 1 {
        return false;
    }
    if n == m {
        let diff: Vec<usize> = (0..n).filter(|&i| a[i] != b[i]).collect();
        return diff.len() == 1
            || (diff.len() == 2
                && diff[1] == diff[0] + 1
                && a[diff[0]] == b[diff[1]]
                && a[diff[1]] == b[diff[0]]);
    }
    let (s, l) = if n < m { (a, b) } else { (b, a) };
    let (mut i, mut j) = (0, 0);
    let mut skipped = false;
    while i < s.len() && j < l.len() {
        if s[i] == l[j] {
            i += 1;
            j += 1;
        } else if skipped {
            return false;
        } else {
            skipped = true;
            j += 1;
        }
    }
    true
}

/// The `with.<key>` names `kind` reads, unioned with every name the capability
/// `def` names as `${with.X}` and every name the step names (`step_refs`).
/// `None` when the kind is not judged at all (open, or no read set).
pub fn read_with_keys(
    kind: &str,
    def: &Value,
    step_refs: &BTreeSet<String>,
) -> Option<BTreeSet<String>> {
    if is_open(kind) {
        return None;
    }
    let set = kind_reads(kind)?;
    let mut used: BTreeSet<String> = set.iter().cloned().collect();
    with_refs_in(def, &mut used);
    used.extend(step_refs.iter().cloned());
    Some(used)
}

/// Name a JSON value's type the way the guard messages do.
pub fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(b) => {
            if *b {
                "`true`"
            } else {
                "`false`"
            }
        }
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// The first `with` problem a `call` step has, with everything its message
/// needs. [`with_issue_body`] renders it; the caller prefixes `line N:` (BDD) or
/// `node <name>:` (`validate`).
pub enum WithIssue {
    /// A key no call of the kind reads (P164).
    Unread {
        cap: String,
        kind: String,
        key: String,
        reads: Vec<String>,
        hint: Option<String>,
    },
    /// A value whose JSON type the read site does not accept (P166).
    WrongType {
        cap: String,
        kind: String,
        key: String,
        want: &'static str,
        got: &'static str,
    },
    /// A number the read site clamps to a band (P175).
    OutOfBand {
        cap: String,
        kind: String,
        key: String,
        value: String,
        band: String,
    },
}

/// The first `with` problem a call step has, in the order the guards report them:
/// an unread key (P164), then a wrong type (P166), then an out-of-band number
/// (P175). `caps` maps capability name to its definition; `with` / `chain` are
/// the step's tables. `None` when the step is clean or its kind is not judged.
pub fn with_step_issue(
    caps: &serde_json::Map<String, Value>,
    cap: &str,
    with: &serde_json::Map<String, Value>,
    chain: &[Value],
) -> Option<WithIssue> {
    let mut refs: BTreeSet<String> = BTreeSet::new();
    with_refs_in(&Value::Object(with.clone()), &mut refs);
    for e in chain {
        with_refs_in(e, &mut refs);
    }
    // The outer call first, then each chain entry - the order the messages use.
    let mut steps: Vec<(&str, &serde_json::Map<String, Value>)> = vec![(cap, with)];
    for e in chain {
        let Some(cn) = e.get("capability").and_then(Value::as_str) else {
            continue;
        };
        if let Some(m) = e.get("with").and_then(Value::as_object) {
            steps.push((cn, m));
        }
    }
    let kind_of = |capname: &str| -> String {
        caps.get(capname)
            .and_then(|d| d.get("kind"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    // 1. a key no call of the kind reads
    for (capname, kv) in &steps {
        let Some(def) = caps.get(*capname) else {
            continue;
        };
        let kind = kind_of(capname);
        let Some(used) = read_with_keys(&kind, def, &refs) else {
            continue;
        };
        for k in kv.keys() {
            if used.contains(k) {
                continue;
            }
            let reads: Vec<String> = kind_reads(&kind)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();
            let hint = reads
                .iter()
                .find(|cand| one_apart(cand, k))
                .map(|c| format!(" Did you mean `with.{c}`?"));
            return Some(WithIssue::Unread {
                cap: (*capname).to_string(),
                kind,
                key: k.clone(),
                reads,
                hint,
            });
        }
    }
    // 2. a value of the wrong type
    for (capname, kv) in &steps {
        let kind = kind_of(capname);
        for (k, v) in kv.iter() {
            if matches!(v.as_str(), Some(s) if s.contains("${")) {
                continue;
            }
            if let Some(want) = with_value_rejected(&kind, k, v) {
                return Some(WithIssue::WrongType {
                    cap: (*capname).to_string(),
                    kind,
                    key: k.clone(),
                    want,
                    got: json_type_name(v),
                });
            }
        }
    }
    // 3. a number the read site clamps
    for (capname, kv) in &steps {
        let kind = kind_of(capname);
        for (k, v) in kv.iter() {
            if matches!(v.as_str(), Some(s) if s.contains("${")) {
                continue;
            }
            if let Some(band) = with_range_rejected(&kind, k, v) {
                return Some(WithIssue::OutOfBand {
                    cap: (*capname).to_string(),
                    kind,
                    key: k.clone(),
                    value: v.to_string(),
                    band,
                });
            }
        }
    }
    None
}

/// Render a [`WithIssue`] as the message body the guards print (the caller adds
/// the `line N:` / `node X:` locator).
pub fn with_issue_body(issue: &WithIssue) -> String {
    match issue {
        WithIssue::Unread {
            cap,
            kind,
            key,
            reads,
            hint,
        } => {
            let reads = if reads.is_empty() {
                format!("A {kind:?} capability reads no `with` key by name; it only sees the keys its own definition names as `${{with.X}}`.")
            } else {
                let shown = if reads.len() <= 24 {
                    reads.join(", ")
                } else {
                    format!("{} and {} more", reads[..12].join(", "), reads.len() - 12)
                };
                format!("The keys a {kind:?} capability reads: {shown}.")
            };
            let hint = hint.as_deref().unwrap_or("");
            format!(
                "`call the capability {cap:?}` passes `with.{key}`, but no {kind:?} call reads that key - the engine ignores it and the call runs with the default, so the value you set is silently dropped.{hint} {reads}"
            )
        }
        WithIssue::WrongType {
            cap,
            kind,
            key,
            want,
            got,
        } => format!(
            "`call the capability {cap:?}` passes `with.{key}` as {got}, but a {kind:?} call reads it as {want} - the engine falls back to its default on any other JSON type, so the value you set is silently discarded."
        ),
        WithIssue::OutOfBand {
            cap,
            kind,
            key,
            value,
            band,
        } => format!(
            "`call the capability {cap:?}` passes `with.{key}` as {value}, but a {kind:?} call clamps it to {band} - a value outside that band is silently replaced at the read site, so the setting that runs is not the one written."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `regex_lite` patterns the hand-written scanners replaced, rebuilt here
    /// rather than in [`Rx`]: the production build then carries no pattern it does
    /// not use, and the equivalence is still pinned against the pattern itself.
    fn ref_fn_sig() -> &'static Regex {
        static R: OnceLock<Regex> = OnceLock::new();
        R.get_or_init(|| Regex::new(r"(?m)^\s*(?:pub\s+)?fn\s+(\w+)\s*[(<]").unwrap())
    }

    fn ref_with_get_lit() -> &'static Regex {
        static R: OnceLock<Regex> = OnceLock::new();
        R.get_or_init(|| Regex::new(r#"with\s*\.\s*get\s*\(\s*"([^"]+)""#).unwrap())
    }

    fn ref_with_get_dyn() -> &'static Regex {
        static R: OnceLock<Regex> = OnceLock::new();
        R.get_or_init(|| Regex::new(r#"with\s*\.\s*get\s*\(\s*[^"]"#).unwrap())
    }

    fn ref_call() -> &'static Regex {
        static R: OnceLock<Regex> = OnceLock::new();
        R.get_or_init(|| Regex::new(r"([A-Za-z_][A-Za-z0-9_]*(?:::\w+)?)\s*\(").unwrap())
    }

    /// [`fn_sigs`] must find exactly what the `(?m)` regex it replaced found -
    /// same names, same offsets, same order - on every capability source.
    #[test]
    fn fn_sigs_matches_the_regex() {
        let r = ref_fn_sig();
        for (name, src) in SOURCES {
            let want: Vec<(String, usize)> = r
                .captures_iter(src)
                .map(|m| (m[1].to_string(), m.get(0).unwrap().end()))
                .collect();
            assert!(!want.is_empty(), "source {name} defines no fn (sanity)");
            assert_eq!(fn_sigs(src), want, "source {name}");
        }
    }

    /// The scanners must return exactly what the patterns did - same values, same
    /// offsets, same order, same count - on every capability source. This is what
    /// lets the hot paths drop `regex_lite` without moving a single judgement: a
    /// `with` key set that differs by one entry would make `bdd build` accept or
    /// refuse a step the other way.
    #[test]
    fn scanners_match_the_regexes() {
        for (name, src) in SOURCES {
            let want: Vec<(String, usize)> = ref_with_get_lit()
                .captures_iter(src)
                .map(|c| (c[1].to_string(), c.get(0).unwrap().end()))
                .collect();
            let got: Vec<(String, usize)> = scan_with_get(src)
                .into_iter()
                .map(|(k, end)| (k.to_string(), end))
                .collect();
            assert_eq!(got, want, "scan_with_get on {name}");

            let want: Vec<(String, usize)> = ref_call()
                .captures_iter(src)
                .map(|c| (c[1].to_string(), c.get(0).unwrap().end() - 1))
                .collect();
            let got: Vec<(String, usize)> = scan_calls(src)
                .into_iter()
                .map(|(k, open)| (k.to_string(), open))
                .collect();
            assert_eq!(got, want, "scan_calls on {name}");

            assert_eq!(
                has_with_get_dyn(src),
                ref_with_get_dyn().is_match(src),
                "has_with_get_dyn on {name}"
            );
        }
    }
}
