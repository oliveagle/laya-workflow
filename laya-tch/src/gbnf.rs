//! A small GBNF (GGML BNF) subset for validating JSON-shaped request bodies
//! before they hit `laya-tch`'s decision model.
//!
//! Subset supported:
//!   * `rule ::= rhs ( sequence ) rhs`
//!   * terminals: `"…"` (with JSON escape decoding) and `[a-z]`, `[^` …]\]`
//!     (with `\xNN` / `\uNNNN` / `a-z` ranges)
//!   * quantifiers: `?`, `*`, `+`, `{m,n}` / `{m,}` / `{m}`
//!   * alternation: `|` at the same parens depth (top-level rule only —
//!     nested groups are flattened to a single multi-alt atom)
//!   * comments: `#…\n`
//!
//! Matches are character-based (no token masking — `laya-tch` is
//! encoder-only; this is a request/response *gate*, not a sampler).
//!
//! Origin / motivation: llaya.cpp's GBNF constrains autoregressive token
//! sampling. `laya-tch` doesn't sample — it produces a single decision per
//! question from a softmax over a small head — so a direct port is the wrong
//! fit. We instead use the same *grammar-driven* idea as a pre-flight gate
//! and (in `predict_impl`) an in-inference answer-set mask. See
//! `src/main.rs::system_one` and `predict_impl` for the wiring.

use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct GbnfError {
    pub message: String,
    pub pos: usize,
    pub rule: String,
}

impl std::fmt::Display for GbnfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GBNF rejected (rule={}, pos={}): {}",
            self.rule, self.pos, self.message
        )
    }
}

impl std::error::Error for GbnfError {}

/// Minimal JSON-shaped grammar for `laya-tch`'s `/system_one` payloads.
///
/// Validates that:
///   * the envelope has both `state` and `questions` keys,
///   * `questions` is a JSON object of question entries,
///   * each question is an object with `type` in {"choice", "score", "noul"}
///     and an `instructions` field of any JSON value,
///   * `criteria` (when present) holds any JSON value.
///
/// Used as a pre-flight gate when the server is started with
/// `--gbnf-strict`. Mirrors the spirit of llama.cpp's per-token logits
/// mask, but at the coarse-grained answer-set level (Choice/Score).
pub const SYSTEM_ONE_REQUEST_GBNF: &str = r#"
root        ::= "{" ws state_pair ws "," ws questions_pair ws "}"
state_pair  ::= "\"state\"" ws ":" ws value
questions_pair ::= "\"questions\"" ws ":" ws qmap
qmap        ::= "{" ws qentry ( ws "," ws qentry )* ws "}"
qentry      ::= string ws ":" ws qbody
qbody       ::= "{" ws qfields ws "}"
qfields     ::= qtype_pair ws "," ws instructions_pair ( ws "," ws criteria_pair )? ws
qtype_pair  ::= "\"type\"" ws ":" ws qtype
qtype       ::= ( "\"choice\"" | "\"score\"" | "\"noul\"" )
instructions_pair ::= "\"instructions\"" ws ":" ws value
criteria_pair ::= "\"criteria\"" ws ":" ws value
value       ::= ( string | number | "true" | "false" | "null" | object | array )
string      ::= "\"" schar* "\""
schar       ::= ( [^"\\] | "\\" esc )
esc         ::= [\\"/bfnrt]
number      ::= "-"? num_int num_frac? num_exp?
num_int     ::= ( "0" | [1-9] [0-9]* )
num_frac    ::= "." [0-9]+
num_exp     ::= ( "e" | "E" ) [+-]? [0-9]+
object      ::= "{" ws ( pair ( ws "," ws pair )* )? ws "}"
pair        ::= string ws ":" ws value
array       ::= "[" ws ( value ( ws "," ws value )* )? ws "]"
ws          ::= [ \t\n\r]*
"#;

type Rules = HashMap<String, Vec<Vec<Atom>>>;

/// One symbol in a rule's RHS. A quantified atom (`atom, q`) is encoded as
/// `Atom::Quant(Box<Atom>, QuantSpec)`. A parenthesised alternation list is
/// `Atom::Group(Vec<Vec<Atom>>)` and behaves as "match any one of these".
#[derive(Debug, Clone)]
pub(crate) enum Atom {
    Lit(String),
    Class {
        negate: bool,
        members: Vec<ClassMember>,
    },
    Ref(String),
    Quant(Box<Atom>, QuantSpec),
    Group(Vec<Vec<Atom>>),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum QuantSpec {
    ZeroOrOne,
    ZeroOrMore,
    OneOrMore,
    Repeat { lo: usize, hi: Option<usize> },
}

#[derive(Debug, Clone)]
pub(crate) enum ClassMember {
    Char(char),
    Range(char, char),
    Escaped(char),
}

/// Tokenize one rule's RHS into a flat list of symbols. Comments (`#…\n`)
/// and whitespace are skipped.
fn tokenize(rhs: &str) -> Vec<String> {
    let bytes = rhs.as_bytes();
    let n = bytes.len();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < n {
        let c = rhs[i..].chars().next().unwrap();
        if c == '#' {
            // line comment
            while i < n && rhs.as_bytes()[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        match c {
            '"' => {
                // string literal; decode JSON escapes only after we collect the raw
                let mut j = i + 1;
                while j < n && rhs.as_bytes()[j] != b'"' {
                    if rhs.as_bytes()[j] == b'\\' && j + 1 < n {
                        j += 2;
                    } else {
                        j += 1;
                    }
                }
                toks.push(rhs[i..=j].to_string());
                i = j + 1;
            }
            '[' => {
                let mut j = i + 1;
                let mut depth = 1;
                while j < n && depth > 0 {
                    match rhs.as_bytes()[j] {
                        b'\\' if j + 1 < n => j += 2,
                        b'[' => {
                            depth += 1;
                            j += 1;
                        }
                        b']' => {
                            depth -= 1;
                            j += 1;
                        }
                        _ => j += 1,
                    }
                }
                toks.push(rhs[i..j].to_string());
                i = j;
            }
            '(' | ')' | '?' | '*' | '+' | '|' | ':' | ',' => {
                toks.push(c.to_string());
                i += 1;
            }
            '{' => {
                let mut j = i;
                let mut depth = 1;
                while j < n && depth > 0 {
                    j += 1;
                    if j < n && rhs.as_bytes()[j] == b'}' {
                        depth -= 1;
                    }
                }
                toks.push(rhs[i..=j].to_string());
                i = j + 1;
            }
            _ if c.is_ascii_alphabetic() || c == '_' => {
                let mut j = i;
                while j < n {
                    let cc = rhs[j..].chars().next().unwrap();
                    if cc.is_ascii_alphanumeric() || cc == '_' || cc == '-' {
                        j += cc.len_utf8();
                    } else {
                        break;
                    }
                }
                toks.push(rhs[i..j].to_string());
                i = j;
            }
            _ => {
                // unknown char — skip silently (matches Python harness behaviour
                // for the whitespace/comments-only lines we strip ahead of time)
                i += c.len_utf8();
            }
        }
    }
    toks
}

fn parse_quant(t: &str) -> Option<QuantSpec> {
    match t {
        "?" => Some(QuantSpec::ZeroOrOne),
        "*" => Some(QuantSpec::ZeroOrMore),
        "+" => Some(QuantSpec::OneOrMore),
        _ => {
            if t.starts_with('{') && t.ends_with('}') {
                let inner = &t[1..t.len() - 1];
                if inner.contains(',') {
                    let mut parts = inner.splitn(2, ',');
                    let lo: usize = parts.next().unwrap().parse().ok()?;
                    let hi: Option<usize> = match parts.next() {
                        Some("") => None,
                        Some(s) => Some(s.parse().ok()?),
                        None => None,
                    };
                    Some(QuantSpec::Repeat { lo, hi })
                } else {
                    let n: usize = inner.parse().ok()?;
                    Some(QuantSpec::Repeat { lo: n, hi: Some(n) })
                }
            } else {
                None
            }
        }
    }
}

/// Parse a (possibly multi-line) grammar source into a name → alternatives map.
pub(crate) fn parse_grammar(src: &str) -> Result<Rules, GbnfError> {
    let mut rules: Rules = HashMap::new();
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, rhs) = match line.split_once("::=") {
            Some(pair) => pair,
            None => continue,
        };
        let name = name.trim().to_string();
        let toks = tokenize(rhs);
        let alts = parse_alts(&toks, 0).0;
        rules.insert(name, alts);
    }
    Ok(rules)
}

/// Parse one alternative (or a sequence), threading alternation at the
/// same parens depth. Returns (alts, end_index).
fn parse_alts(toks: &[String], mut i: usize) -> (Vec<Vec<Atom>>, usize) {
    let mut alts: Vec<Vec<Atom>> = vec![Vec::new()];
    while i < toks.len() {
        let t = &toks[i];
        if t == "|" {
            alts.push(Vec::new());
            i += 1;
            continue;
        }
        if t == ")" {
            return (alts, i + 1);
        }
        if t == "(" {
            let (sub, next) = parse_alts(toks, i + 1);
            alts.last_mut().unwrap().push(Atom::Group(sub));
            i = next;
            continue;
        }
        if let Some(q) = parse_quant(t) {
            if let Some(last) = alts.last_mut().unwrap().last_mut() {
                *last = Atom::Quant(Box::new(last.clone()), q);
            }
            i += 1;
            continue;
        }
        let atom = if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
            // The tokenizer keeps the delimiters; strip them and decode any
            // JSON escape sequences (\n, \", \uXXXX, ...) so the matcher
            // compares against the *content* of the literal - not the GBNF
            // source representation. Mirrors `bench/jev_vs_gbnf.py`'s
            // `json.loads("[" + atom + "]")[0]` trick.
            Atom::Lit(decode_json_string(&t[1..t.len() - 1]))
        } else if t.starts_with('[') {
            parse_class(&t[1..t.len() - 1])
        } else if t == ":" || t == "," {
            // Single-char structural literals (JSON separators). Adding them
            // to the tokenizer required also lowering them here, otherwise
            // parse_alts would treat them as undefined non-terminals.
            Atom::Lit(t.clone())
        } else {
            Atom::Ref(t.clone())
        };
        alts.last_mut().unwrap().push(atom);
        i += 1;
    }
    (alts, i)
}

fn parse_class(body: &str) -> Atom {
    let mut negate = false;
    let mut s = body;
    if let Some(rest) = s.strip_prefix('^') {
        negate = true;
        s = rest;
    }
    let mut members = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            members.push(ClassMember::Escaped(chars[i + 1]));
            i += 2;
            continue;
        }
        if i + 2 < chars.len() && chars[i + 1] == '-' {
            members.push(ClassMember::Range(c, chars[i + 2]));
            i += 3;
            continue;
        }
        members.push(ClassMember::Char(c));
        i += 1;
    }
    Atom::Class { negate, members }
}

fn decode_json_string(raw: &str) -> String {
    // Borrow serde_json::Value's escape handling by round-tripping through
    // a JSON parser. Cheaper than open-coding all the escapes.
    let wrapped = format!("\"{}\"", raw.replace('\n', "\\n"));
    match serde_json::from_str::<Value>(&wrapped) {
        Ok(Value::String(s)) => s,
        _ => raw.to_string(),
    }
}

fn class_matches(members: &[ClassMember], c: char) -> bool {
    members.iter().any(|m| match m {
        ClassMember::Char(x) => *x == c,
        ClassMember::Range(lo, hi) => *lo <= c && c <= *hi,
        ClassMember::Escaped('n') => c == '\n',
        ClassMember::Escaped('t') => c == '\t',
        ClassMember::Escaped('r') => c == '\r',
        ClassMember::Escaped(x) => *x == c,
    })
}

/// Match `an` starting at `pos`. Returns the longest advance position on
/// success, or `pos` and `false` on mismatch.
fn match_atom(an: &Atom, s: &str, pos: usize, rules: &Rules) -> (usize, bool) {
    match an {
        Atom::Lit(lit) => {
            if pos + lit.len() <= s.len() && &s.as_bytes()[pos..pos + lit.len()] == lit.as_bytes() {
                (pos + lit.len(), true)
            } else {
                (pos, false)
            }
        }
        Atom::Class { negate, members } => {
            if pos >= s.len() {
                return (pos, false);
            }
            let c = s[pos..].chars().next().unwrap();
            let hit = class_matches(members, c);
            if hit ^ *negate {
                (pos + c.len_utf8(), true)
            } else {
                (pos, false)
            }
        }
        Atom::Ref(name) => {
            let alts = match rules.get(name) {
                Some(a) => a,
                None => return (pos, false),
            };
            match_alt(alts, s, pos, rules)
        }
        Atom::Quant(inner, q) => {
            let (lo, hi) = match q {
                QuantSpec::ZeroOrOne => (0, Some(1)),
                QuantSpec::ZeroOrMore => (0, None),
                QuantSpec::OneOrMore => (1, None),
                QuantSpec::Repeat { lo, hi } => (*lo, *hi),
            };
            let mut cur = pos;
            let mut n = 0;
            loop {
                let (next, ok) = match_atom(inner, s, cur, rules);
                if !ok || next == cur {
                    break;
                }
                cur = next;
                n += 1;
                if let Some(h) = hi {
                    if n >= h {
                        break;
                    }
                }
            }
            if n >= lo {
                (cur, true)
            } else {
                (pos, false)
            }
        }
        Atom::Group(alts) => match_alt(alts, s, pos, rules),
    }
}

/// Pick the longest successful alternative (epsilon rule OK for empty).
fn match_alt(alts: &[Vec<Atom>], s: &str, pos: usize, rules: &Rules) -> (usize, bool) {
    let mut best = (pos, false);
    let mut deepest_fail = pos;
    for alt in alts {
        let (next, ok) = match_seq(alt, s, pos, rules);
        if ok && (!best.1 || next > best.0) {
            best = (next, true);
        } else if !ok && next > deepest_fail {
            deepest_fail = next;
        }
    }
    if best.1 {
        best
    } else {
        // No match accepted. Return the deepest position we tried before a
        // failure so the error message points at something useful.
        (deepest_fail, false)
    }
}

fn match_seq(seq: &[Atom], s: &str, pos: usize, rules: &Rules) -> (usize, bool) {
    let mut cur = pos;
    for an in seq {
        let (next, ok) = match_atom(an, s, cur, rules);
        if !ok {
            // Return the position where the failed atom was *tried*, not the
            // initial pos of this sequence. Otherwise a long sequence that
            // matches the first half and then fails at the end would mask the
            // real failure point.
            return (cur, false);
        }
        cur = next;
    }
    (cur, true)
}

/// Validate `input` against `grammar`'s `root` rule. Returns `Ok(())` when the
/// input is fully consumed (or fully matches a length-zero grammar); otherwise
/// returns a `GbnfError` with the first rejected position.
pub fn validate(grammar: &str, root: &str, input: &str) -> Result<(), GbnfError> {
    let rules = parse_grammar(grammar).map_err(|e: GbnfError| GbnfError {
        message: format!("grammar parse: {}", e.message),
        ..e
    })?;
    let alts = rules.get(root).ok_or_else(|| GbnfError {
        message: format!("root rule {:?} not found", root),
        pos: 0,
        rule: root.to_string(),
    })?;
    let (next, ok) = match_alt(alts, input, 0, &rules);
    if ok && next == input.len() {
        Ok(())
    } else {
        Err(GbnfError {
            message: format!("first rejected position; consumed={}, total={}", next, input.len()),
            pos: next,
            rule: root.to_string(),
        })
    }
}

/// Convenience wrapper that runs the gate against an already-parsed
/// `serde_json::Value` (re-serialised without whitespace).
pub fn validate_json(grammar: &str, root: &str, v: &Value) -> Result<(), GbnfError> {
    validate(grammar, root, &v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRAMMAR: &str = r#"
    // Realistic JSON-shaped grammar for the test corpus: literal key "a"
    // (with the surrounding quotes preserved by the tokenizer + decode_json_string)
    // so `rejects_unquoted_key` has a chance of failing. The `integer` rule
    // intentionally caps at 2 trailing digits so `char_class_works` can
    // assert both 12 and 1234.
        root    ::= "{" ws "\"a\"" ws ":" ws integer ws "}"
        integer ::= "-"? ( "0" | [1-9] [0-9]{0,2} )
        ws      ::= ( " " | "\n" )*
    "#;

    #[test]
    fn accepts_valid() {
        assert!(validate(GRAMMAR, "root", r#"{"a": 42}"#).is_ok());
        assert!(validate(GRAMMAR, "root", r#"{"a":-7}"#).is_ok());
        assert!(validate(GRAMMAR, "root", r#"{"a":0}"#).is_ok());
    }


    #[test]
    fn rejects_unterminated() {
        assert!(validate(GRAMMAR, "root", r#"{"a": 4"#).is_err());
    }

    #[test]
    fn rejects_unquoted_key() {
        assert!(validate(GRAMMAR, "root", r#"{a: 4}"#).is_err());
    }


    #[test]
    fn char_class_works() {
        assert!(validate(GRAMMAR, "root", r#"{"a": 12}"#).is_ok());
        assert!(validate(GRAMMAR, "root", r#"{"a": 1234}"#).is_err()); // 3 digits > {0,2}
    }

    #[test]
    fn validate_json_roundtrip() {
        let v: Value = serde_json::from_str(r#"{"a": 12}"#).unwrap();
        assert!(validate_json(GRAMMAR, "root", &v).is_ok());
    }

    #[test]
    fn literal_decodes_json_escapes() {
        // GBNF literal "\n" in source becomes the actual newline char
        // after we strip delimiters + JSON-decode. Matching against the
        // *real* newline byte is what callers want.
        let g = r#"root ::= "\n""#;
        assert!(validate(g, "root", "
").is_ok());
        // The literal '"hello"' in source becomes the 7-char string "hello"
        // (with quote delimiters).
        let g2 = r#"root ::= "\"hello\"""#;
        assert!(validate(g2, "root", "\"hello\"").is_ok());
    }
    #[test]
    fn char_class_with_negation_and_range() {
        // `[^a]` matches anything except `a`; `x` fails.
        let g = r#"root ::= [^a]+"#;
        assert!(validate(g, "root", "bcd").is_ok());
        assert!(validate(g, "root", "a").is_err());
    }

    #[test]
    fn alternation_with_parens() {
        let g = r#"root ::= ("foo" | "bar") "#;
        assert!(validate(g, "root", "foo").is_ok());
        assert!(validate(g, "root", "bar").is_ok());
        assert!(validate(g, "root", "baz").is_err());
    }

    #[test]
    fn repeat_count_bounds() {
        // {2,4} means 2..=4 repetitions
        let g = r#"root ::= "x"{2,4}"#;
        assert!(validate(g, "root", "xx").is_ok());
        assert!(validate(g, "root", "xxxx").is_ok());
        assert!(validate(g, "root", "xxxxx").is_err());
        assert!(validate(g, "root", "x").is_err());
    }

    #[test]
    fn json_shaped_payload_accepts_real_request() {
        // A realistic /system_one payload must validate against the production
        // grammar (covers: state string, two questions with different types,
        // one with criteria as an object, and a quoted-string value with `?`).
        let payload = serde_json::json!({
            "state": "The office will be closed on Monday for a public holiday.",
            "questions": {
                "is_action_required": {"type": "noul", "instructions": "Does this require any action?"},
                "category": {"type": "choice", "instructions": "What is this about?",
                             "criteria": {"holiday": "closures", "billing": "money", "security": "access"}}
            }
        });
        assert!(validate_json(crate::gbnf::SYSTEM_ONE_REQUEST_GBNF, "root", &payload).is_ok());
    }

    #[test]
    fn json_shaped_payload_rejects_missing_instructions() {
        // Missing `instructions` is invalid.
        let payload = serde_json::json!({
            "state": null,
            "questions": {
                "q": {"type": "choice"}
            }
        });
        assert!(validate_json(crate::gbnf::SYSTEM_ONE_REQUEST_GBNF, "root", &payload).is_err());
    }

    #[test]
    fn json_shaped_payload_rejects_unknown_type() {
        let payload = serde_json::json!({
            "state": null,
            "questions": {
                "q": {"type": "wibble", "instructions": "x"}
            }
        });
        assert!(validate_json(crate::gbnf::SYSTEM_ONE_REQUEST_GBNF, "root", &payload).is_err());
    }
}
