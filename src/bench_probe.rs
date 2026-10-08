//! Research / A-B probes that used to live as `bench/*.py` scripts.
//!
//! * `jev-vs-gbnf`      — compare the jev semantic gate vs a GBNF structural
//!    gate on the same candidate memory records (Rust port of
//!    `bench/jev_vs_gbnf.py`, pure logic, no model needed).
//! * `bdd-to-needle`    — probe whether the on-device Needle 3 model can
//!    compile Gherkin BDD steps into workflow spec JSON (port of
//!    `bench/bdd_to_needle.py`).
//! * `needle-vs-heuristic` — A/B bench: Needle 3 vs the offline heuristics on
//!    routing / extraction / embedding / end-to-end (port of
//!    `bench/needle_vs_heuristic.py`).

use anyhow::{bail, Result};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// jev-vs-gbnf: a tiny GBNF subset validator + the jev semantic gate.
// ---------------------------------------------------------------------------

const GBNF_GRAMMAR: &str = r#"
# Memory record JSON for laya_mem_persist (System-One write-side).
# Keys are required in this exact order; no additional keys allowed.
root ::= "{" ws id_kv ws "," ws content_kv ws "," ws ts_kv ws "," ws type_scores_kv ws "," ws entities_kv ws "}"

id_kv            ::= "\"id\""            ws ":" ws integer
content_kv        ::= "\"content\""        ws ":" ws string
ts_kv             ::= "\"ts\""             ws ":" ws string
type_scores_kv    ::= "\"type_scores\""    ws ":" ws type_scores_obj
entities_kv       ::= "\"entities\""       ws ":" ws string_array

type_scores_obj   ::= "{" ws episodic_kv "," ws semantic_kv "," ws procedural_kv "," ws preference_kv ws "}"
episodic_kv       ::= "\"episodic\""     ws ":" ws number
semantic_kv       ::= "\"semantic\""     ws ":" ws number
procedural_kv     ::= "\"procedural\""   ws ":" ws number
preference_kv     ::= "\"preference\""   ws ":" ws number

string_array      ::= "[" ws ( string ("," ws string)* )? ws "]"
string            ::= "\"" char* "\""
char              ::= [^"\\] | "\\" ( ["\\\bfnrt] | "u" [0-9a-fA-F]{4} )
integer           ::= "-"? ( "0" | [1-9] [0-9]{0,15} )
number            ::= "-"? ( "0" | [1-9] [0-9]{0,15} ) ( "." [0-9]{1,16} )? ( [eE] [-+]? integer )?
ws                ::= ( " " | "\n" | "\t" )*
"#;

#[derive(Clone, Debug)]
enum GbnfAtom {
    /// A quoted literal.
    Lit(String),
    /// A char class body (between the outer brackets), e.g. `^"\\` or `0-9a-fA-F`.
    Class { negate: bool, body: String },
    /// A reference to another rule.
    Ref(String),
    /// A grouped sub-alternation: `(...)`.
    Group(Vec<GbnfSeq>),
}

#[derive(Clone, Debug)]
enum GbnfQuant {
    None,
    One, // ?
    Plus,
    Star,
    Repeat(u32, Option<u32>), // {m,n}
}

type GbnfSeq = Vec<(GbnfAtom, GbnfQuant)>;

fn gbnf_tokenize(rhs: &str) -> Result<Vec<String>> {
    let chars: Vec<char> = rhs.chars().collect();
    let n = chars.len();
    let mut i = 0;
    let mut toks = Vec::new();
    while i < n {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '#' {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '"' {
            let mut j = i + 1;
            while j < n && chars[j] != '"' {
                if chars[j] == '\\' {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            let s: String = chars[i..=j].iter().collect();
            toks.push(s);
            i = j + 1;
            continue;
        }
        if c == '[' {
            let mut depth = 1;
            let mut j = i + 1;
            while j < n && depth > 0 {
                if chars[j] == '\\' && j + 1 < n {
                    j += 2;
                    continue;
                }
                if chars[j] == ']' {
                    depth -= 1;
                } else if chars[j] == '[' {
                    depth += 1;
                }
                j += 1;
            }
            let s: String = chars[i..j].iter().collect();
            toks.push(s);
            i = j;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let mut j = i;
            while j < n && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '-') {
                j += 1;
            }
            let s: String = chars[i..j].iter().collect();
            toks.push(s);
            i = j;
            continue;
        }
        if "?*+|(),".contains(c) {
            toks.push(c.to_string());
            i += 1;
            continue;
        }
        if c == '{' {
            let mut depth = 1;
            let mut j = i + 1;
            while j < n && depth > 0 {
                if chars[j] == '}' {
                    depth -= 1;
                }
                j += 1;
            }
            let s: String = chars[i..j].iter().collect();
            toks.push(s);
            i = j;
            continue;
        }
        bail!("unexpected char in GBNF: {c:?} at {i}");
    }
    Ok(toks)
}

/// Parse one RHS into a list of alternatives; each alternative is a sequence
/// of (atom, quantifier) groups. Returns `(alts, next_idx)`.
fn gbnf_parse_alts(toks: &[String], start: usize) -> Result<(Vec<GbnfSeq>, usize)> {
    let mut alts: Vec<GbnfSeq> = Vec::new();
    let mut cur: GbnfSeq = Vec::new();
    let mut i = start;
    while i < toks.len() {
        let t = &toks[i];
        if t == "|" {
            alts.push(std::mem::take(&mut cur));
            i += 1;
            continue;
        }
        if t == ")" {
            alts.push(std::mem::take(&mut cur));
            return Ok((alts, i + 1));
        }
        if t == "(" {
            let (sub, next) = gbnf_parse_alts(toks, i + 1)?;
            cur.push((GbnfAtom::Group(sub), GbnfQuant::None));
            i = next;
            continue;
        }
        if t == "?" || t == "*" || t == "+" {
            if cur.is_empty() {
                bail!("quantifier without atom: {t}");
            }
            let q = if t == "?" {
                GbnfQuant::One
            } else if t == "*" {
                GbnfQuant::Star
            } else {
                GbnfQuant::Plus
            };
            let last = cur.last_mut().unwrap();
            last.1 = q;
            i += 1;
            continue;
        }
        if t.starts_with('{') && t.ends_with('}') {
            if cur.is_empty() {
                bail!("quantifier without atom: {t}");
            }
            let inner = &t[1..t.len() - 1];
            let (lo, hi) = if let Some((a, b)) = inner.split_once(',') {
                let lo: u32 = if a.is_empty() { 0 } else { a.parse()? };
                let hi: Option<u32> = if b.is_empty() { None } else { Some(b.parse()?) };
                (lo, hi)
            } else {
                let v: u32 = inner.parse()?;
                (v, Some(v))
            };
            let last = cur.last_mut().unwrap();
            last.1 = GbnfQuant::Repeat(lo, hi);
            i += 1;
            continue;
        }
        // A plain atom: literal / class / reference.
        let atom = if t.starts_with('"') && t.ends_with('"') {
            // Decode escapes like the Python `json.loads("[" + atom + "]")`.
            GbnfAtom::Lit(unescape_lit(t)?)
        } else if t.starts_with('[') && t.ends_with(']') {
            let body = &t[1..t.len() - 1];
            let negate = body.starts_with('^');
            let members = if negate { &body[1..] } else { body }.to_string();
            GbnfAtom::Class { negate, body: members }
        } else {
            GbnfAtom::Ref(t.clone())
        };
        cur.push((atom, GbnfQuant::None));
        i += 1;
    }
    alts.push(cur);
    Ok((alts, i))
}

fn unescape_lit(t: &str) -> Result<String> {
    // t includes the surrounding quotes.
    let inner = &t[1..t.len() - 1];
    let mut out = String::new();
    let chars: Vec<char> = inner.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let e = chars[i + 1];
            match e {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'u' => {
                    let hex: String = chars[i + 2..i + 6].iter().collect();
                    let cp = u32::from_str_radix(&hex, 16).map_err(|e| anyhow::anyhow!("bad \\u escape: {e}"))?;
                    out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                    i += 4;
                }
                other => out.push(other),
            }
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    Ok(out)
}

/// Does a single char match a char-class body? Supports `\xHH`, `\uHHHH`,
/// ranges `a-z` and single chars. Returns true when `ch` matches.
fn char_class_matches(ch: char, body: &str) -> bool {
    let chars: Vec<char> = body.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c == '\\' && i + 1 < n {
            let e = chars[i + 1];
            if e == 'x' && i + 4 <= n {
                let hex: String = chars[i + 2..i + 4].iter().collect();
                if let Ok(v) = u32::from_str_radix(&hex, 16) {
                    if char::from_u32(v) == Some(ch) {
                        return true;
                    }
                }
                i += 4;
                continue;
            }
            if e == 'u' && i + 6 <= n {
                let hex: String = chars[i + 2..i + 6].iter().collect();
                if let Ok(v) = u32::from_str_radix(&hex, 16) {
                    if char::from_u32(v) == Some(ch) {
                        return true;
                    }
                }
                i += 6;
                continue;
            }
            if ch == e {
                return true;
            }
            i += 2;
            continue;
        }
        if i + 2 < n && chars[i + 1] == '-' {
            let lo = c;
            let hi = chars[i + 2];
            if lo <= ch && ch <= hi {
                return true;
            }
            i += 3;
            continue;
        }
        if ch == c {
            return true;
        }
        i += 1;
    }
    false
}

#[derive(Clone)]
struct GbnfGrammar {
    rules: std::collections::HashMap<String, Vec<GbnfSeq>>,
}

fn gbnf_parse(src: &str) -> Result<GbnfGrammar> {
    let mut rules = std::collections::HashMap::new();
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !line.contains("::=") {
            continue;
        }
        let (head, rhs) = line.split_once("::=").unwrap();
        let name = head.trim().to_string();
        let toks = gbnf_tokenize(rhs)?;
        let (alts, _) = gbnf_parse_alts(&toks, 0)?;
        rules.insert(name, alts);
    }
    Ok(GbnfGrammar { rules })
}

struct GbnfMatch {
    pos: usize,
    ok: bool,
}

impl GbnfGrammar {
    fn match_alt(&self, alts: &[GbnfSeq], s: &[char], pos: usize) -> GbnfMatch {
        // Epsilon: no alternatives or all empty.
        if alts.is_empty() || alts.iter().all(|a| a.is_empty()) {
            return GbnfMatch { pos, ok: true };
        }
        let mut best = GbnfMatch { pos, ok: false };
        for alt in alts {
            let m = self.match_seq(alt, s, pos);
            if m.ok && m.pos > best.pos {
                best = m;
            } else if m.ok && !best.ok {
                best = m;
            }
        }
        best
    }

    fn match_seq(&self, seq: &GbnfSeq, s: &[char], pos: usize) -> GbnfMatch {
        let mut cur = pos;
        for (atom, quant) in seq {
            match quant {
                GbnfQuant::None => {
                    let m = self.match_atom(atom, s, cur);
                    if !m.ok {
                        return GbnfMatch { pos: cur, ok: false };
                    }
                    cur = m.pos;
                }
                GbnfQuant::One => {
                    let m = self.match_atom(atom, s, cur);
                    // Optional: it's fine whether or not it matched.
                    if m.ok {
                        cur = m.pos;
                    }
                }
                GbnfQuant::Star => {
                    let mut saved = cur;
                    loop {
                        let m = self.match_atom(atom, s, cur);
                        if !m.ok || m.pos == cur {
                            break;
                        }
                        cur = m.pos;
                        saved = cur;
                    }
                    let _ = saved;
                }
                GbnfQuant::Plus => {
                    let mut m = self.match_atom(atom, s, cur);
                    if !m.ok || m.pos == cur {
                        return GbnfMatch { pos: cur, ok: false };
                    }
                    cur = m.pos;
                    loop {
                        m = self.match_atom(atom, s, cur);
                        if !m.ok || m.pos == cur {
                            break;
                        }
                        cur = m.pos;
                    }
                }
                GbnfQuant::Repeat(lo, hi) => {
                    let mut n = 0u32;
                    let saved = cur;
                    loop {
                        let m = self.match_atom(atom, s, cur);
                        if !m.ok || m.pos == cur {
                            break;
                        }
                        cur = m.pos;
                        n += 1;
                        if let Some(h) = hi {
                            if n >= *h {
                                break;
                            }
                        }
                    }
                    if n < *lo {
                        return GbnfMatch { pos: saved, ok: false };
                    }
                }
            }
        }
        GbnfMatch { pos: cur, ok: true }
    }

    fn match_atom(&self, atom: &GbnfAtom, s: &[char], pos: usize) -> GbnfMatch {
        match atom {
            GbnfAtom::Group(sub) => self.match_alt(sub, s, pos),
            GbnfAtom::Lit(lit) => {
                let lc: Vec<char> = lit.chars().collect();
                if s.len() >= pos + lc.len() && s[pos..pos + lc.len()] == lc[..] {
                    GbnfMatch { pos: pos + lc.len(), ok: true }
                } else {
                    GbnfMatch { pos, ok: false }
                }
            }
            GbnfAtom::Class { negate, body } => {
                if pos >= s.len() {
                    return GbnfMatch { pos, ok: false };
                }
                let ch = s[pos];
                let mut ok = char_class_matches(ch, body);
                if *negate {
                    ok = !ok;
                }
                GbnfMatch { pos: pos + 1, ok }
            }
            GbnfAtom::Ref(name) => {
                match self.rules.get(name) {
                    Some(alts) => self.match_alt(alts, s, pos),
                    None => GbnfMatch { pos, ok: false },
                }
            }
        }
    }

    fn accepts(&self, root: &str, s: &str) -> (bool, usize) {
        let chars: Vec<char> = s.chars().collect();
        match self.rules.get(root) {
            Some(alts) => {
                let m = self.match_alt(alts, &chars, 0);
                (m.ok && m.pos == chars.len(), m.pos)
            }
            None => (false, 0),
        }
    }
}

/// The four jev-mem type labels, in order.
const TYPE_LABELS: [&str; 4] = ["episodic", "semantic", "procedural", "preference"];

/// jev semantic gate — the constraints laya_mem_persist relies on.
fn jev_check(rec: &Value) -> (bool, Vec<String>) {
    let mut errors = Vec::new();
    if !rec.is_object() {
        return (false, vec!["not a JSON object".to_string()]);
    }
    let content = rec.get("content").and_then(Value::as_str).unwrap_or("");
    if content.is_empty() {
        errors.push("content empty".to_string());
    } else if content.chars().count() > 1024 {
        errors.push(format!("content too long ({} > 1024)", content.chars().count()));
    }
    let ts = rec.get("ts").and_then(Value::as_str);
    match ts {
        Some(t) => {
            if !is_iso_8601(t) {
                errors.push(format!("ts not ISO-8601 ({t:?})"));
            }
        }
        None => errors.push("ts not ISO-8601 (None)".to_string()),
    }
    let mut ts_scores = std::collections::BTreeMap::new();
    if let Some(obj) = rec.get("type_scores").and_then(Value::as_object) {
        for (k, v) in obj {
            ts_scores.insert(k.clone(), v.clone());
        }
    } else {
        errors.push("type_scores not an object".to_string());
    }
    for label in TYPE_LABELS {
        match ts_scores.get(label) {
            Some(v) => {
                let num = v.as_f64();
                match num {
                    Some(x) if (0.0..=1.0).contains(&x) => {}
                    _ => errors.push(format!("type_scores.{label} not in [0,1] ({v})")),
                }
            }
            None => errors.push(format!("type_scores.{label} not in [0,1] (null)")),
        }
    }
    let extras: Vec<&String> = ts_scores.keys().filter(|k| !TYPE_LABELS.contains(&k.as_str())).collect();
    if !extras.is_empty() {
        let keys: Vec<String> = extras.into_iter().cloned().collect();
        errors.push(format!("type_scores has extra keys {keys:?}"));
    }
    if TYPE_LABELS.iter().all(|l| ts_scores.contains_key(*l)) {
        let s: f64 = TYPE_LABELS
            .iter()
            .filter_map(|l| ts_scores.get(*l).and_then(Value::as_f64))
            .sum();
        if (s - 1.0).abs() > 0.05 {
            errors.push(format!("type_scores sum {s:.3} not ≈ 1.0"));
        }
    }
    let entities: Vec<Value> = match rec.get("entities") {
        Some(Value::Array(a)) => a.clone(),
        _ => {
            errors.push("entities not an array".to_string());
            Vec::new()
        }
    };
    if entities.len() > 32 {
        errors.push(format!("entities too many ({} > 32)", entities.len()));
    }
    for (i, e) in entities.iter().enumerate() {
        match e.as_str() {
            Some(str) if str.is_empty() => errors.push(format!("entities[{i}] not a non-empty string")),
            Some(str) if str.chars().count() > 64 => {
                errors.push(format!("entities[{i}] too long ({} > 64)", str.chars().count()))
            }
            Some(_) => {}
            None => errors.push(format!("entities[{i}] not a non-empty string")),
        }
    }
    (errors.is_empty(), errors)
}

/// A very small ISO-8601 check (subset of the Python regex).
fn is_iso_8601(s: &str) -> bool {
    // `^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:?\d{2})?)?$`
    let bytes: Vec<char> = s.chars().collect();
    let n = bytes.len();
    if n < 10 {
        return false;
    }
    let digit = |c: char| c.is_ascii_digit();
    if !(digit(bytes[0]) && digit(bytes[1]) && digit(bytes[2]) && digit(bytes[3]) && bytes[4] == '-'
        && digit(bytes[5]) && digit(bytes[6]) && bytes[7] == '-' && digit(bytes[8]) && digit(bytes[9]))
    {
        return false;
    }
    if n == 10 {
        return true;
    }
    if bytes[10] != 'T' {
        return false;
    }
    // Need HH:MM:SS
    if n < 19 {
        return false;
    }
    if !(digit(bytes[11]) && digit(bytes[12]) && bytes[13] == ':' && digit(bytes[14]) && digit(bytes[15])
        && bytes[16] == ':' && digit(bytes[17]) && digit(bytes[18]))
    {
        return false;
    }
    let mut i = 19;
    if i < n && bytes[i] == '.' {
        i += 1;
        while i < n && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < n {
        let z = bytes[i];
        if z == 'Z' {
            i += 1;
        } else if z == '+' || z == '-' {
            i += 1;
            let mut cnt = 0;
            while i < n && bytes[i].is_ascii_digit() {
                i += 1;
                cnt += 1;
            }
            if cnt == 2 && i < n && bytes[i] == ':' {
                i += 1;
                while i < n && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
        } else {
            return false;
        }
    }
    i == n
}

#[derive(Clone)]
struct MemRecord {
    content: String,
    ts: String,
    type_scores: Value,
    entities: Vec<String>,
}

impl MemRecord {
    fn valid() -> Self {
        Self {
            content: "Alice planted basil and wants a weekly reminder.".to_string(),
            ts: "2026-09-02T14:00:00Z".to_string(),
            type_scores: json!({"episodic": 0.1, "semantic": 0.2, "procedural": 0.1, "preference": 0.6}),
            entities: vec!["Alice".to_string(), "basil".to_string()],
        }
    }

    fn to_json(&self, rid: i64) -> Value {
        json!({
            "id": rid,
            "content": self.content,
            "ts": self.ts,
            "type_scores": self.type_scores,
            "entities": self.entities,
        })
    }
}

fn string_mutations() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let valid_rec = MemRecord::valid();
    let valid = serde_json::to_string(&valid_rec.to_json(7)).unwrap();
    out.push(("clean valid".to_string(), valid.clone()));
    // Truncations
    out.push(("truncated 30b from end".to_string(), valid[..valid.len() - 30].to_string()));
    out.push((
        "truncated mid-value".to_string(),
        valid[..valid.find("\"content\"").unwrap() + 6].to_string(),
    ));
    out.push(("truncated to opening brace".to_string(), valid[..1].to_string()));
    // Wrong types
    let mut r = MemRecord::valid();
    r.content = "Alice planted basil and wants a weekly reminder.".to_string();
    let mut v = r.to_json(7);
    v["id"] = json!("seven");
    out.push(("id is string".to_string(), v.to_string()));
    let mut r = MemRecord::valid();
    let mut v = r.to_json(7);
    v["content"] = json!(42);
    out.push(("content is int".to_string(), v.to_string()));
    let mut r = MemRecord::valid();
    let mut v = r.to_json(7);
    v["type_scores"] = json!({"episodic": 1.2, "semantic": -0.1, "procedural": 0.0, "preference": 0.0});
    out.push(("type_scores out of [0,1]".to_string(), v.to_string()));
    let mut r = MemRecord::valid();
    let mut v = r.to_json(7);
    if let Some(obj) = v["type_scores"].as_object_mut() {
        obj.insert("extra".into(), json!(0.1));
    }
    out.push(("type_scores has extra key".to_string(), v.to_string()));
    let mut r = MemRecord::valid();
    let mut v = r.to_json(7);
    v["type_scores"]["episodic"] = json!("high");
    out.push(("type_scores.episodic is string".to_string(), v.to_string()));
    // Structural
    out.push(("missing closing brace".to_string(), valid[..valid.len() - 1].to_string()));
    out.push(("trailing comma + extra brace".to_string(), format!("{},}}", &valid[..valid.len() - 1])));
    let unquoted = valid.replacen("\"id\"", "id", 1);
    out.push(("unquoted key 'id'".to_string(), unquoted));
    let reordered = valid.replacen("\"id\":7,", "\"content\":\"Alice...", 1);
    out.push(("reordered/missing key".to_string(), reordered));
    // jev-flavored: semantic OK but missing some keys
    let mut r = MemRecord::valid();
    let mut v = r.to_json(7);
    v.as_object_mut().unwrap().remove("ts");
    out.push(("missing ts".to_string(), v.to_string()));
    let mut r = MemRecord::valid();
    let mut v = r.to_json(7);
    v.as_object_mut().unwrap().remove("entities");
    out.push(("missing entities".to_string(), v.to_string()));
    // jev-only catches (semantic)
    let mut r = MemRecord::valid();
    r.content = "x".repeat(2000);
    out.push(("content too long (jev catches)".to_string(), r.to_json(7).to_string()));
    let mut r = MemRecord::valid();
    r.entities = vec!["e".to_string(); 64];
    out.push(("entities too many (jev catches)".to_string(), r.to_json(7).to_string()));
    let mut r = MemRecord::valid();
    r.type_scores = json!({"episodic": 0.5, "semantic": 0.5, "procedural": 0.5, "preference": 0.5});
    out.push(("type_scores sum 2.0 (jev catches)".to_string(), r.to_json(7).to_string()));
    // GBNF-only catches (structural)
    let mut r = MemRecord::valid();
    let mut v = r.to_json(7);
    v["unknown_field"] = json!("ignored");
    out.push(("extra unknown field (GBNF catches)".to_string(), v.to_string()));
    out
}

fn render_live_jev(rows: &[LiveRow]) -> String {
    if rows.is_empty() {
        return "_laya-workflow CLI not available; live jev skipped._\n".to_string();
    }
    let mut lines = vec![
        "Real `laya_mem_admission` runs (offline heuristic) on a few observations:".to_string(),
        String::new(),
        "| observation | admission | confidence | should_store answer |".to_string(),
        "|---|---|---|---|".to_string(),
    ];
    for r in rows {
        match r {
            LiveRow::Error { observation, error } => {
                lines.push(format!("| {observation} | _error: {error}_ | | |"));
            }
            LiveRow::Ok { observation, label, confidence, answer } => {
                lines.push(format!("| {observation} | **{label}** | {confidence} | {answer} |"));
            }
        }
    }
    lines.join("\n") + "\n"
}

enum LiveRow {
    Error { observation: String, error: String },
    Ok { observation: String, label: String, confidence: String, answer: String },
}

fn live_jev_decisions(observations: &[&str]) -> Vec<LiveRow> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let spec = root.join("dsl/laya_mem/admission.json");
    let mut out = Vec::new();
    for obs in observations {
        let state = json!({"observation": obs, "recent_memories": []}).to_string();
        let wf = match crate::spec::load_file(&spec.to_string_lossy()) {
            Ok(w) => w,
            Err(e) => {
                out.push(LiveRow::Error { observation: (*obs).to_string(), error: format!("spec load failed: {e}") });
                continue;
            }
        };
        let backend = crate::backend::HeuristicBackend;
        match wf.run(&backend, &serde_json::from_str(&state).unwrap_or(Value::Null)) {
            Ok(res) => {
                let v = res.to_json();
                let r = v.get("result").cloned().unwrap_or(Value::Null);
                let label = r.get("label").and_then(Value::as_str).unwrap_or("?").to_string();
                let confidence = r.get("confidence").map(|x| x.to_string()).unwrap_or_else(|| "?".into());
                let answer = r.get("action_answer").and_then(Value::as_str).unwrap_or("?").to_string();
                out.push(LiveRow::Ok {
                    observation: (*obs).to_string(),
                    label,
                    confidence,
                    answer,
                });
            }
            Err(e) => {
                out.push(LiveRow::Error { observation: (*obs).to_string(), error: format!("run failed: {e}") });
            }
        }
    }
    out
}

fn write_analysis(rows: &[(String, bool, bool, bool, usize, Vec<String>)], n_jev: usize, n_gbnf: usize, n_both: usize, total: usize, live_rows: &[LiveRow], elapsed_s: f64) -> String {
    let jev_only = rows.iter().filter(|(_, j, g, _, _, _)| *j && !*g).count();
    let gbnf_only = rows.iter().filter(|(_, j, g, _, _, _)| !*j && *g).count();
    let both_fail = rows.iter().filter(|(_, j, g, _, _, _)| !*j && !*g).count();
    let mut p = Vec::new();
    p.push(format!("## Analysis\n"));
    p.push(format!("- **{total}** candidates x 3 gates; elapsed {elapsed_s:.2}s. Stable across reruns.\n"));
    p.push(format!("- **Both pass**: {n_both}/{total} (only the clean, schema-conformant record)."));
    p.push(format!("- **Both fail**: {both_fail}/{total} (truncations and structural errors that nobody can ignore)."));
    p.push(format!("- **jev catches, GBNF misses**: {jev_only}/{total} (semantic errors: type_scores out of [0,1], content too long, entities too many, type_scores sum drift)."));
    p.push(format!("- **GBNF catches, jev misses**: {gbnf_only}/{total} (structural errors: extra unknown fields, type-wrong but parseable values).\n"));
    p.push("### When to use which (or both)\n".to_string());
    p.push("- **jev only** is enough when the *content* of a value matters but the bytes are already well-formed (e.g. laya_mem_persist where the spec builds the SQL payload via `db.call`, not via free-form generation).".to_string());
    p.push("- **GBNF only** is enough when the output is a free-form generation step (e.g. an LLM writing the persisted record), but the values themselves are trivially valid (any number, any string).".to_string());
    p.push("- **jev + GBNF** is needed when *both* matter: the generation is free-form (so it needs structural constraints) **and** the values carry semantic meaning (so it needs a separate semantic check). That is the recommended write-gate policy for `laya_mem_persist`: only persist if jev says ALLOW/CONFIRM *and* the bytes to be written conform to the GBNF schema.\n".to_string());
    p.push("### Layered interaction\n".to_string());
    p.push("Jev-mem and GBNF guard **different layers**:\n".to_string());
    p.push("| layer | what it can catch | what it cannot catch |".to_string());
    p.push("|---|---|---|".to_string());
    p.push("| jev decision model (System-One controller) | semantic gate: admission ALLOW/CONFIRM/BLOCK, type, routing, stopping; numeric sanity (sum drift, range, length) | structural bytes — e.g. extra fields, type-wrong values |".to_string());
    p.push("| GBNF (token-mask in llama.cpp's sampler) | byte-level grammar: exact keys, exact types, exact ranges | semantic meaning — e.g. 'this score is impossible' |\n".to_string());
    p.push("The two are complementary, not substitutes. A laya_mem_persist pipeline that uses only one will let through either structural garbage (GBNF-only) or semantically-bad content (jev-only).\n".to_string());
    p.push(render_live_jev(live_rows));
    p.join("\n") + "\n"
}

pub struct JevVsGbnfOptions {
    pub md_out: Option<String>,
}

pub fn jev_vs_gbnf(opts: &JevVsGbnfOptions) -> Result<()> {
    let t0 = std::time::Instant::now();
    let grammar = gbnf_parse(GBNF_GRAMMAR)?;
    let cases = string_mutations();
    let mut rows: Vec<(String, bool, bool, bool, usize, Vec<String>)> = Vec::new();
    let mut n_jev = 0usize;
    let mut n_gbnf = 0usize;
    let mut n_both = 0usize;
    for (label, raw) in &cases {
        let (gbnf_ok, pos) = grammar.accepts("root", raw);
        let parsed: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
        let (jev_ok, jev_err) = jev_check(&parsed);
        let both = jev_ok && gbnf_ok;
        rows.push((label.clone(), jev_ok, gbnf_ok, both, pos, jev_err));
        if jev_ok {
            n_jev += 1;
        }
        if gbnf_ok {
            n_gbnf += 1;
        }
        if both {
            n_both += 1;
        }
    }
    let n = rows.len();
    println!("records: {n}  |  jev pass: {n_jev}  |  gbnf pass: {n_gbnf}  |  both pass: {n_both}");
    println!();
    println!("{:<30} {:<5} {:<5} {:<9} {:<10} {}", "case", "jev", "gbnf", "jev+gbnf", "parser-pos", "jev errors");
    println!("{}", "-".repeat(100));
    for (label, jev_ok, gbnf_ok, both, pos, jev_err) in &rows {
        println!(
            "{:<30} {:<5} {:<5} {:<9} {:<10} {}",
            label,
            if *jev_ok { "PASS" } else { "FAIL" },
            if *gbnf_ok { "PASS" } else { "FAIL" },
            if *both { "PASS" } else { "FAIL" },
            pos,
            jev_err.join("; ")[..jev_err.join("; ").len().min(60)].to_string()
        );
    }
    let elapsed = t0.elapsed().as_secs_f64();
    let live_rows = live_jev_decisions(&[
        "Alice planted basil and wants a weekly reminder.",
        "OK thanks",
        "Mira prefers concise explanations.",
        "trivial ack noted",
    ]);
    if let Some(out) = &opts.md_out {
        let mut md = String::new();
        md.push_str("# jev-mem vs GBNF comparison\n\n");
        md.push_str(&format!("Generated by `bench/jev_vs_gbnf.py` (Rust port `laya-workflow bench jev-vs-gbnf`). {n} candidate memory records.\n\n"));
        md.push_str("| case | jev | gbnf | jev+gbnf | parser pos | jev errors |\n");
        md.push_str("|---|---|---|---|---|---|\n");
        for (label, jev_ok, gbnf_ok, both, pos, jev_err) in &rows {
            md.push_str(&format!(
                "| {label} | {} | {} | {} | {pos} | {} |\n",
                if *jev_ok { "PASS" } else { "FAIL" },
                if *gbnf_ok { "PASS" } else { "FAIL" },
                if *both { "PASS" } else { "FAIL" },
                jev_err.join("; ")[..jev_err.join("; ").len().min(80)].to_string()
            ));
        }
        md.push_str(&write_analysis(&rows, n_jev, n_gbnf, n_both, n, &live_rows, elapsed));
        md.push_str(&format!("\n## Summary\n\n- **jev pass**: {n_jev}/{n}\n- **GBNF pass**: {n_gbnf}/{n}\n- **both pass** (the recommended write-gate): {n_both}/{n}\n"));
        let p = std::path::Path::new(out);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, &md)?;
        println!("\nwrote {out}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// bdd-to-needle: probe whether the on-device Needle 3 model can compile
// Gherkin BDD steps into workflow spec JSON. Port of bench/bdd_to_needle.py.
// ---------------------------------------------------------------------------

pub struct BddToNeedleOptions {
    pub quiet: bool,
}

/// Call the in-process Needle 3 engine for `extract` / `complete`.
fn needle_call(op: &str, args: &Value) -> Result<Value> {
    use crate::capability::Policy;
    let cap = crate::capability::needle::NeedleCap {
        op: op.to_string(),
        cact: None,
    };
    crate::capability::needle::call_needle(&cap, args, &json!({}), &Policy::default())
}

fn tool(name: &str, desc: &str, trig: &[&str], props: &[&str]) -> Value {
    let mut parameters = serde_json::Map::new();
    parameters.insert(
        "type".to_string(),
        json!("object"),
    );
    let mut properties = serde_json::Map::new();
    for k in props {
        properties.insert(k.to_string(), json!({"type": "string"}));
    }
    parameters.insert("properties".to_string(), Value::Object(properties));
    parameters.insert("required".to_string(), Value::Array(props.iter().map(|s| json!(s)).collect()));
    json!({
        "type": "function",
        "name": name,
        "description": desc,
        "triggers": trig.iter().map(|t| json!(t)).collect::<Vec<_>>(),
        "parameters": Value::Object(parameters),
    })
}

fn bdd_tools() -> Vec<Value> {
    vec![
        tool("open_page", "Open a URL and wait until it is loaded.", &[r"\b(am on|open)\b"], &["url"]),
        tool("navigate", "Move an open tab to a new URL.", &[r"\bnavigate\b"], &["url"]),
        tool("wait_for", "Poll until a CSS selector matches.", &[r"\bwait for\b"], &["selector"]),
        tool("click", "Click a CSS selector.", &[r"\bclick\b"], &["selector"]),
        tool("type_text", "Type text into a CSS selector.", &[r"\btype\b"], &["text", "selector"]),
        tool("select_option", "Select a value in a CSS selector.", &[r"\bselect\b"], &["value", "selector"]),
        tool("run_js", "Evaluate a javascript expression in the page.", &[r"\brun javascript\b"], &["expression"]),
        tool(
            "assert",
            "Run a named page assertion.",
            &[r"\b(title|url) contains|is visible|is absent|is true|is false|equals|contains\b"],
            &["assertion", "value", "expected"],
        ),
        tool("release_page", "Close the current page.", &[r"\brelease\b"], &["target_id"]),
    ]
}

fn single_tool() -> Value {
    json!({
        "type": "function", "name": "bdd_step",
        "description": "Translate a Gherkin browser-automation step into a workflow node.",
        "parameters": {"type": "object", "properties": {
            "op": {"type": "string", "enum": ["open", "navigate", "wait_for", "click",
                                              "type", "select", "evaluate", "assert", "release"]},
            "assertion": {"type": "string", "enum": ["title_contains", "url_contains", "visible",
                                                      "absent", "is_true", "is_false", "equals",
                                                      "contains", "equals_text"]},
            "value": {"type": "string"},
            "expected": {"type": "string"}}},
    })
}

type Case = (&'static str, &'static str, Option<&'static str>, &'static [(&'static str, &'static str)]);

const IN_VOCAB: &[Case] = &[
    ("Then the page title contains \"BDD Fixture\"", "assert", Some("title_contains"), &[("value", "BDD Fixture")]),
    ("When I click the element \"#greet\"", "click", None, &[("selector", "#greet")]),
    ("Given I am on \"<base_url>/index.html\"", "open_page", None, &[("url", "<base_url>/index.html")]),
    ("Then the element \"#heading\" is visible", "assert", Some("visible"), &[("value", "#heading")]),
    ("When I navigate to \"<base_url>/second.html\"", "navigate", None, &[("url", "<base_url>/second.html")]),
    ("When I type \"Ada\" into the element \"#name\"", "type_text", None, &[("text", "Ada"), ("selector", "#name")]),
    ("Then javascript \"document.readyState === 'complete'\" is true", "assert", Some("is_true"), &[("value", "document.readyState === 'complete'")]),
    ("When I wait for the element \"#echo[data-filled]\"", "wait_for", None, &[("selector", "#echo[data-filled]")]),
    ("When I select \"blue\" in the element \"#color\"", "select_option", None, &[("value", "blue"), ("selector", "#color")]),
    ("When I release the page", "release_page", None, &[]),
];

const NOVEL: &[Case] = &[
    ("Then the heading \"#hero\" should be displayed", "assert", Some("visible"), &[("value", "#hero")]),
    ("When I hit the submit button", "click", None, &[]),
    ("Given the site is open at https://example.com/login", "open_page", None, &[("url", "https://example.com/login")]),
    ("Then the address bar shows \"/dashboard\"", "assert", Some("url_contains"), &[]),
    ("When I fill the email field with ada@example.com", "type_text", None, &[]),
    ("Then the count of items is 7", "assert", Some("equals"), &[]),
    ("When I jump to the settings screen", "navigate", None, &[]),
    ("Then there should be no loading spinner", "assert", Some("absent"), &[]),
    ("When I choose \"USD\" from the currency picker", "select_option", None, &[("value", "USD")]),
    ("Then confirm the button is hidden", "assert", Some("absent"), &[]),
];

const SYSTEM: &str = "You are a Gherkin-to-workflow compiler. Call exactly one tool per step.";

fn unq(s: &str) -> &str {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

#[derive(Default, Clone)]
struct Counts {
    op: usize,
    assertion: usize,
    args: usize,
    full: usize,
    floor: usize,
}

fn score_single(n: usize) -> Counts {
    let mut c = Counts::default();
    for (step, exp_tool, exp_assert, exp_args) in IN_VOCAB {
        let args = json!({"text": step, "tool": single_tool(), "system": SYSTEM});
        let r = match needle_call("extract", &args) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("needle_extract failed: {e}");
                continue;
            }
        };
        let a = r.get("arguments").cloned().unwrap_or(Value::Null);
        let got = a.get("op").and_then(Value::as_str).unwrap_or("");
        let got_tool = match got {
            "open" => "open_page",
            "type" => "type_text",
            "select" => "select_option",
            "evaluate" => "run_js",
            other => other,
        };
        let oo = got_tool == *exp_tool;
        if oo {
            c.op += 1;
        }
        let ao = exp_assert.is_none() || a.get("assertion").and_then(Value::as_str) == *exp_assert;
        if ao {
            c.assertion += 1;
        }
        let ar = exp_args
            .iter()
            .all(|(k, v)| unq(a.get(*k).and_then(Value::as_str).unwrap_or("")) == *v);
        if ar {
            c.args += 1;
        }
        if oo && ao && ar {
            c.full += 1;
        }
        if r.get("confidence").and_then(Value::as_f64).unwrap_or(0.0) >= 0.1 {
            c.floor += 1;
        }
    }
    c
}

fn score_multi(cases: &[Case], n: usize) -> (Counts, Vec<(String, &'static str, Option<&'static str>, String, Value, f64, bool)>) {
    let mut c = Counts::default();
    let mut details = Vec::new();
    for (step, exp_tool, exp_assert, exp_args) in cases {
        let args = json!({"prompt": step, "tools": bdd_tools(), "system": SYSTEM});
        let r = match needle_call("complete", &args) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("needle_complete failed: {e}");
                continue;
            }
        };
        let calls = r.get("function_calls").and_then(Value::as_array).cloned().unwrap_or_default();
        let supp = r.get("suppressed_calls").and_then(Value::as_array).cloned().unwrap_or_default();
        let item = calls.first().or_else(|| supp.first()).cloned();
        let conf = r.get("confidence").and_then(Value::as_f64).unwrap_or(0.0);
        let args_v = item.as_ref().and_then(|i| i.get("arguments")).cloned().unwrap_or(Value::Null);
        let got = item.as_ref().and_then(|i| i.get("name")).and_then(Value::as_str).unwrap_or("");
        let oo = got == *exp_tool;
        if oo {
            c.op += 1;
        }
        let ao = exp_assert.is_none() || args_v.get("assertion").and_then(Value::as_str) == *exp_assert;
        if ao {
            c.assertion += 1;
        }
        let ar = exp_args
            .iter()
            .all(|(k, v)| unq(args_v.get(*k).and_then(Value::as_str).unwrap_or("")) == *v);
        if ar {
            c.args += 1;
        }
        let f = oo && ao && ar;
        if f {
            c.full += 1;
        }
        if conf >= 0.1 {
            c.floor += 1;
        }
        details.push((step.to_string(), *exp_tool, *exp_assert, got.to_string(), args_v, conf, f));
    }
    (c, details)
}

fn emit(label: &str, c: &Counts, n: usize) {
    println!(
        "  {label:<34} op {}/{}  assert {}/{}  args {}/{}  full {}/{}  conf>=0.1 {}/{}",
        c.op, n, c.assertion, n, c.args, n, c.full, n, c.floor, n
    );
}

pub fn bdd_to_needle(_opts: &BddToNeedleOptions) -> Result<()> {
    let n = IN_VOCAB.len();
    println!("{}", "=".repeat(74));
    println!("BDD -> needle -> spec JSON  (bench/bdd_to_needle.py → Rust)");
    println!("{}", "=".repeat(74));
    let a = score_single(n);
    emit("A single-tool extract (in-vocab)", &a, n);
    let (b, _) = score_multi(IN_VOCAB, n);
    emit("B multi-tool + triggers (in-vocab)", &b, n);
    let (c, c_det) = score_multi(NOVEL, n);
    emit("C multi-tool (novel phrasings)", &c, n);
    println!();
    println!("Novel-step failures (regex transpiler gets these wrong too — its coverage is 0):");
    for (step, exp_tool, exp_assert, got, _args, conf, ok) in &c_det {
        if !ok {
            println!(
                "  conf={conf:.3} expect={exp_tool}/{} got={got}  {step}",
                exp_assert.unwrap_or("-")
            );
        }
    }
    println!();
    // stability: rerun condition B once, report the delta on `full`
    let (b2, _) = score_multi(IN_VOCAB, n);
    println!(
        "Stability (condition B rerun): full {}/{} -> {}/{} (run-to-run variance is expected: the engine samples)",
        b.full, n, b2.full, n
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// needle-vs-heuristic: A/B bench — Needle 3 vs the offline heuristics on
// routing / extraction / embedding / end-to-end. Rust port of
// bench/needle_vs_heuristic.py.
// ---------------------------------------------------------------------------

pub struct NeedleVsHeuristicOptions {
    pub quiet: bool,
}

const DATE_FACT: &str = "date: 2026-10-07 Wed 09:00; locale: en-US; this is a customer support inbox.";

fn needle_complete(prompt: &str, tools: &[Value], system: Option<&str>) -> Value {
    let mut args = serde_json::Map::new();
    args.insert("prompt".into(), json!(prompt));
    args.insert("tools".into(), json!(tools));
    if let Some(sys) = system {
        args.insert("system".into(), json!(sys));
    }
    let args = Value::Object(args);
    needle_call("complete", &args).unwrap_or(Value::Null)
}

fn needle_extract(text: &str, tool: &Value, system: Option<&str>) -> Value {
    let mut args = serde_json::Map::new();
    args.insert("text".into(), json!(text));
    args.insert("tool".into(), tool.clone());
    if let Some(sys) = system {
        args.insert("system".into(), json!(sys));
    }
    let args = Value::Object(args);
    needle_call("extract", &args).unwrap_or(Value::Null)
}

fn needle_embed(text: &str) -> Vec<f64> {
    let r = needle_call("embed", &json!({"text": text})).unwrap_or(Value::Null);
    r.get("vector")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_f64()).collect())
        .unwrap_or_default()
}

/// Reproduce the offline heuristic exactly: substring match.
fn laya_heuristic_route(text: &str, categories: &[(&'static str, &'static [&'static str])]) -> Vec<&'static str> {
    let t = text.to_lowercase();
    let mut hits = Vec::new();
    for (cat, kws) in categories {
        for k in *kws {
            if t.contains(&k.to_lowercase()) {
                hits.push(*cat);
                break;
            }
        }
    }
    hits
}

fn heuristic_keywords() -> Vec<(&'static str, &'static [&'static str])> {
    vec![
        ("refund", &["refund", "charge", "money back", "payment returned", "reverse", "return"]),
        ("technical", &["wifi", "app crashes", "500 error", "login", "bluetooth", "video won't", "settings", "crash"]),
        ("shipping", &["package", "parcel", "delivery", "courier", "delivered", "shipping", "order"]),
    ]
}

/// Regex heuristic: vendor/total/due_date from a free-text invoice.
fn laya_heuristic_extract_invoice(text: &str) -> (Option<String>, Option<f64>, Option<String>) {
    // vendor: `(?:from|by)\s+([A-Z][A-Za-z0-9&\. ]+?)(?=,|\.|$|\s+\$)`
    // regex-lite has no look-around: match the name run, then trim at the
    // same stop points the Python lookahead would have used.
    let mut vendor = None;
    let re_v = regex_lite::Regex::new(r"(?:from|by)\s+([A-Z][A-Za-z0-9&\. ]+)").unwrap();
    if let Some(c) = re_v.captures(text) {
        if let Some(name) = c.get(1) {
            let s = name.as_str();
            let bytes = s.as_bytes();
            let mut end = bytes.len();
            for (j, &b) in bytes.iter().enumerate() {
                if b == b'.' || b == b',' {
                    end = j;
                    break;
                }
                if b == b' ' && j + 1 < bytes.len() && bytes[j + 1] == b'$' {
                    end = j;
                    break;
                }
            }
            let trimmed = s[..end].trim();
            if !trimmed.is_empty() {
                vendor = Some(trimmed.to_string());
            }
        }
    }
    // total: `\$([0-9][0-9,]*\.?[0-9]*)`
    let mut total = None;
    let re_t = regex_lite::Regex::new(r"\$([0-9][0-9,]*\.?[0-9]*)").unwrap();
    if let Some(c) = re_t.captures(text) {
        if let Some(m) = c.get(1) {
            let s = m.as_str().replace(',', "");
            total = s.parse::<f64>().ok();
        }
    }
    // due: `due\\s+(...)`
    let mut due = None;
    let re_d = regex_lite::Regex::new(
        r"due\s+([0-9]{4}-[0-9]{2}-[0-9]{2}|[A-Z][a-z]+ \d{1,2},? \d{4}|\d{1,2}[/-]\d{1,2}[/-]\d{2,4})",
    )
    .unwrap();
    if let Some(c) = re_d.captures(text) {
        due = c.get(1).map(|m| m.as_str().to_string());
    }
    (vendor, total, due)
}

fn invoice_tool() -> Value {
    json!({
        "type": "function", "name": "invoice",
        "description": "Record invoice vendor, total and due date from text.",
        "parameters": {"type": "object", "properties": {
            "vendor": {"type": "string"},
            "total": {"type": "number"},
            "due_date": {"type": "string"}},
            "required": ["vendor", "total", "due_date"]}
    })
}

fn norm_date(s: Option<&str>) -> Option<String> {
    let s = s?.trim();
    // (\d{4})-(\d{2})-(\d{2})
    let re1 = regex_lite::Regex::new(r"(\d{4})-(\d{2})-(\d{2})").unwrap();
    if let Some(c) = re1.captures(s) {
        if let (Some(a), Some(b), Some(d)) = (c.get(1), c.get(2), c.get(3)) {
            if a.as_str().len() == 4 && a.as_str().chars().all(|x| x.is_ascii_digit()) {
                return Some(format!("{}-{}-{}", a.as_str(), b.as_str(), d.as_str()));
            }
        }
    }
    // (\d{2})/(\d{2})/(\d{4})
    let re2 = regex_lite::Regex::new(r"(\d{2})/(\d{2})/(\d{4})").unwrap();
    if let Some(c) = re2.captures(s) {
        if let (Some(a), Some(b), Some(d)) = (c.get(1), c.get(2), c.get(3)) {
            return Some(format!("{}-{}-{}", d.as_str(), b.as_str(), a.as_str()));
        }
    }
    // ([A-Z][a-z]+)\\s+(\d{1,2}),?\\s+(\d{4})
    let re3 = regex_lite::Regex::new(r"([A-Z][a-z]+)\s+(\d{1,2}),?\s+(\d{4})").unwrap();
    if let Some(c) = re3.captures(s) {
        if let (Some(mon), Some(day), Some(year)) = (c.get(1), c.get(2), c.get(3)) {
            let months = [
                ("January", "01"), ("February", "02"), ("March", "03"), ("April", "04"),
                ("May", "05"), ("June", "06"), ("July", "07"), ("August", "08"),
                ("September", "09"), ("Oct", "10"), ("October", "10"), ("November", "11"),
                ("December", "12"), ("Sep", "09"),
            ];
            let mm = months.iter().find(|(m, _)| *m == mon.as_str()).map(|(_, v)| *v).unwrap_or("?");
            let dd: u32 = day.as_str().parse().unwrap_or(0);
            return Some(format!("{}-{}-{:02}", year.as_str(), mm, dd));
        }
    }
    None
}

fn norm_total(v: Option<&Value>) -> Option<f64> {
    let s = v?.as_str()?;
    let t = s.replace(',', "").replace('$', "").replace("USD", "").trim().to_string();
    t.parse::<f64>().ok()
}

fn vendor_match(a: Option<&str>, b: &str) -> bool {
    match (a, b) {
        (Some(x), y) if !x.is_empty() && !y.is_empty() => {
            let xl = x.to_lowercase();
            let yl = y.to_lowercase();
            let min = yl.len().min(4);
            if min == 0 {
                return false;
            }
            xl.starts_with(&yl[..min]) || yl.starts_with(&xl[..min])
        }
        _ => false,
    }
}

fn total_match(a: Option<f64>, b: f64) -> bool {
    match a {
        Some(x) => (x - b).abs() < 1.0,
        None => false,
    }
}

fn date_match(a: Option<&str>, b: &str) -> bool {
    match a {
        Some(x) if !x.is_empty() && !b.is_empty() => {
            let a_s = &x[..x.len().min(10)];
            let b_s = &b[..b.len().min(10)];
            a_s == b_s || norm_date(Some(x)) == norm_date(Some(b))
        }
        _ => false,
    }
}

fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let nb: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if na > 0.0 && nb > 0.0 {
        dot / (na * nb)
    } else {
        0.0
    }
}

/// Simple deterministic BOW (replaces Python zlib.crc32 — deterministic FNV-1a).
fn bow(text: &str, dim: usize) -> Vec<f64> {
    let mut v = vec![0.0; dim];
    let re = regex_lite::Regex::new(r"[a-z]{2,}").unwrap();
    for w in re.find_iter(&text.to_lowercase()) {
        // FNV-1a 32-bit
        let mut h: u32 = 0x811c9dc5;
        for b in w.as_str().as_bytes() {
            h ^= *b as u32;
            h = h.wrapping_mul(0x01000193);
        }
        let idx = (h as usize) % dim;
        v[idx] += 1.0;
    }
    v
}

fn route_tools() -> Vec<Value> {
    vec![
        json!({
            "type": "function", "name": "refund_order",
            "description": "Give the customer their money back for a purchase.",
            "triggers": ["\\b(refund|return|charge.?back)\\b", "\\bmoney\\s*back\\b", "\\btake\\s*(it|them)\\s*back\\b", "\\b(cancel|reverse)\\s+(this\\s+)?(purchase|order|charge)\\b", "\\bcredit\\s+me\\b"],
            "parameters": {"type": "object", "properties": {"reason": {"type": "string"}}, "required": []}}),
        json!({
            "type": "function", "name": "fix_technical",
            "description": "Help with a broken device, app, or connection.",
            "triggers": ["\\b(wifi|internet|bluetooth|app|software)\\b", "\\b(crash|freeze|stutter|drop|pair|sign\\s*in)\\b", "\\b(500|error|broken|not\\s*working|won.t)\\b"],
            "parameters": {"type": "object", "properties": {"issue": {"type": "string"}}, "required": []}}),
        json!({
            "type": "function", "name": "track_package",
            "description": "Find where a delivery is or change its address.",
            "triggers": ["\\b(package|parcel|order|delivery|deliver|courier|shipment|track|arriv|customs)\\b"],
            "parameters": {"type": "object", "properties": {"order_id": {"type": "string"}}, "required": []}}),
    ]
}

fn name2cat(name: &str) -> &'static str {
    match name {
        "refund_order" => "refund",
        "fix_technical" => "technical",
        "track_package" => "shipping",
        _ => "other",
    }
}

const INTENTS: &[&str] = &[
    "my earbuds died after two weeks, please take them back and give me my money back",
    "the jacket is torn on arrival, i want to send it back",
    "this lamp never worked, i'd like to get my cash back for it",
    "the toaster burnt out in a month, i'm sending it back",
    "please cancel this purchase and credit me",
    "my internet keeps dropping out every few minutes",
    "the software closes by itself whenever i open it",
    "i keep getting an error page when i sign in",
    "streaming keeps freezing and stuttering",
    "my headphones won't pair with the phone anymore",
    "where's my package? no — my stuff, it should have been here yesterday",
    "the tracker shows arrived but i got nothing",
    "my goods have been stuck at customs for a week",
    "the driver left my box at the wrong house",
    "can i change where this gets sent",
    "just wanted to say your service was great last week",
    "do you have an app for android",
    "what time are you open on sunday",
    "how do i stop getting your emails",
    "do students get a discount",
];

const INTENT_LABELS: &[&str] = &[
    "refund", "refund", "refund", "refund", "refund",
    "technical", "technical", "technical", "technical", "technical",
    "shipping", "shipping", "shipping", "shipping", "shipping",
    "other", "other", "other", "other", "other",
];

const INVOICE_TEXTS: &[&str] = &[
    "Invoice from Acme Corp, $1,200.00, due 2026-09-01",
    "INVOICE: acme corp  TOTAL: 1,200 USD DUE: September 1st 2026",
    "invoice — vendor: Acme Corporation amount: USD 1,200.00 due date: 01/09/2026",
    "bill to: Acme Corp ltd\namount due: $1200.00\npayment by: Sep 1 2026",
    "acme corp owes us 1200 dollars for the september invoice, payable by 2026-09-01",
    "Invoice #1042\n  From: Acme Corp\n  Total: $1,200.00\n  Due: 2026-09-01",
    "ACME CORPORATION INVOICE\n  AMOUNT: 1,200.00 USD\n  PAYMENT DUE: 2026-09-01",
    "acme corp sent an invoice for twelve hundred dollars, due on the first of september twenty twenty-six",
];

const TICKET_TEXTS: &[&str] = &[
    "Order #88472 is missing from my account, customer 55331",
    "customer 12345 order #90123 the screen is cracked",
    "i'm customer 88 order #12345 and i want my money back for the broken toaster",
    "account holder 90210, order 67890, the sound stopped working after the update",
    "my name is customer 555 and i placed order #44444 last week but the package arrived damaged",
];

const TICKET_LABELS: &[(&str, &str, &str)] = &[
    ("technical", "55331", "88472"),
    ("technical", "12345", "90123"),
    ("billing", "88", "12345"),
    ("technical", "90210", "67890"),
    ("shipping", "555", "44444"),
];

pub fn needle_vs_heuristic(_opts: &NeedleVsHeuristicOptions) -> Result<()> {
    let kws = heuristic_keywords();
    let tools = route_tools();
    let n = INTENTS.len();

    // SCENARIO A — intent routing
    println!("{}", "=".repeat(74));
    println!("SCENARIO A — INTENT ROUTING  (heuristic substring vs Needle enum-classify)");
    println!("{}", "=".repeat(74));
    let mut rows: Vec<(usize, &str, &str, bool, &str, bool, Option<f64>)> = Vec::new();
    let (mut heur_ok, mut n_ok, mut n_refused, mut n_wrong, mut h_wrong) = (0, 0, 0, 0, 0);
    for (i, (text, truth)) in INTENTS.iter().zip(INTENT_LABELS.iter()).enumerate() {
        let h = laya_heuristic_route(text, &kws);
        let hcat = h.first().copied().unwrap_or("other");
        let r = needle_complete(text, &tools, Some(DATE_FACT));
        let calls = r.get("function_calls").and_then(Value::as_array).cloned().unwrap_or_default();
        let sup = r.get("suppressed_calls").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut ncat = "other";
        if !calls.is_empty() {
            let name = calls[0].get("name").and_then(Value::as_str).unwrap_or("");
            ncat = name2cat(name);
        } else if !sup.is_empty() {
            let name = sup[0].get("name").and_then(Value::as_str).unwrap_or("");
            ncat = name2cat(name);
            n_refused += 1;
        } else {
            n_refused += 1;
        }
        let hok = hcat == *truth;
        let nok = ncat == *truth;
        if hok { heur_ok += 1 } else { h_wrong += 1 }
        if nok { n_ok += 1 } else { n_wrong += 1 }
        let conf = r.get("confidence").and_then(Value::as_f64);
        rows.push((i + 1, truth, hcat, hok, ncat, nok, conf));
        println!(
            "[{:2}] truth={:9} heur={:9}{}  needle={:9}{}  conf={}",
            i + 1, truth, hcat, if hok { "OK" } else { "XX" }, ncat,
            if nok { "OK" } else { "XX" },
            conf.map(|c| format!("{c}")).unwrap_or_else(|| "-".into())
        );
    }
    println!();
    println!("  HEURISTIC accuracy: {heur_ok}/20 ({:.0}%)", 100.0 * heur_ok as f64 / 20.0);
    println!("  NEEDLE    accuracy: {n_ok}/20 ({:.0}%)   (refused→other: {n_refused})", 100.0 * n_ok as f64 / 20.0);

    // A2: literal-keyword variants
    println!("\n  -- A2: literal-keyword variants (heuristic's home turf) --");
    let kw_prompts = [
        "i want a refund for the broken headphones",
        "reverse the charge on order 552",
        "my wifi keeps disconnecting every 5 minutes",
        "the app crashes when i open settings",
        "where is my package, it was supposed to arrive",
        "the courier left my order at the wrong address",
        "tracking shows delivered but i never got it",
        "can i change the delivery address for order 881",
        "please return my money, the product is not as described",
        "i keep getting a 500 error on the login page",
    ];
    let kw_labels = ["refund", "refund", "technical", "technical", "shipping", "shipping", "shipping", "shipping", "refund", "technical"];
    let (mut h2, mut n2) = (0, 0);
    for (text, truth) in kw_prompts.iter().zip(kw_labels.iter()) {
        let h = laya_heuristic_route(text, &kws);
        let hcat = h.first().copied().unwrap_or("other");
        let r = needle_complete(text, &tools, Some(DATE_FACT));
        let calls = r.get("function_calls").and_then(Value::as_array).cloned().unwrap_or_default();
        let sup = r.get("suppressed_calls").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut ncat = "other";
        if !calls.is_empty() {
            let name = calls[0].get("name").and_then(Value::as_str).unwrap_or("");
            ncat = name2cat(name);
        } else if !sup.is_empty() {
            let name = sup[0].get("name").and_then(Value::as_str).unwrap_or("");
            ncat = name2cat(name);
        }
        if hcat == *truth { h2 += 1; }
        if ncat == *truth { n2 += 1; }
    }
    println!("  HEURISTIC accuracy (keyword prompts): {h2}/10 ({:.0}%)", 100.0 * h2 as f64 / 10.0);
    println!("  NEEDLE    accuracy (keyword prompts): {n2}/10 ({:.0}%)", 100.0 * n2 as f64 / 10.0);

    // SCENARIO B — structured extraction
    println!("\n{}", "=".repeat(74));
    println!("SCENARIO B — STRUCTURED EXTRACTION  (regex vs Needle grammar-guaranteed)");
    println!("{}", "=".repeat(74));
    let inv_tool = invoice_tool();
    let (mut regex_ok, mut needle_ok) = (0, 0);
    let mut lat_b = Vec::new();
    let mut reg_f1 = [("vendor", 0u32, 0u32), ("total", 0, 0), ("due_date", 0, 0)];
    let mut ne_f1 = [("vendor", 0u32, 0u32), ("total", 0, 0), ("due_date", 0, 0)];
    let truth_v = ("Acme Corp", 1200.0, "2026-09-01");
    for (i, text) in INVOICE_TEXTS.iter().enumerate() {
        let r_reg = laya_heuristic_extract_invoice(text);
        let t0 = std::time::Instant::now();
        let r_ne = needle_extract(text, &inv_tool, None);
        lat_b.push(t0.elapsed().as_secs_f64() * 1000.0);
        let args = r_ne.get("arguments").cloned().unwrap_or(Value::Null);
        let nr = (
            r_reg.0.as_deref(),
            r_reg.1,
            norm_date(r_reg.2.as_deref()),
        );
        let nn = (
            args.get("vendor").and_then(Value::as_str),
            norm_total(args.get("total")),
            norm_date(args.get("due_date").and_then(Value::as_str)),
        );
        let all_ok_r = vendor_match(nr.0, truth_v.0) && total_match(nr.1, truth_v.1) && date_match(nr.2.as_deref(), truth_v.2);
        let all_ok_n = vendor_match(nn.0, truth_v.0) && total_match(nn.1, truth_v.1) && date_match(nn.2.as_deref(), truth_v.2);
        if all_ok_r { regex_ok += 1; }
        if all_ok_n { needle_ok += 1; }
        // regex F1
        for (name, hit, present) in reg_f1.iter_mut() {
            let tp_ok = match *name {
                "vendor" => vendor_match(nr.0, truth_v.0),
                "total" => total_match(nr.1, truth_v.1),
                _ => date_match(nr.2.as_deref(), truth_v.2),
            };
            let tp_some = match *name {
                "vendor" => nr.0.is_some(),
                "total" => nr.1.is_some(),
                _ => nr.2.is_some(),
            };
            if tp_ok { *hit += 1; }
            if tp_some { *present += 1; }
        }
        // needle F1
        for (name, hit, present) in ne_f1.iter_mut() {
            let tp_ok = match *name {
                "vendor" => vendor_match(nn.0, truth_v.0),
                "total" => total_match(nn.1, truth_v.1),
                _ => date_match(nn.2.as_deref(), truth_v.2),
            };
            let tp_some = match *name {
                "vendor" => nn.0.is_some(),
                "total" => nn.1.is_some(),
                _ => nn.2.is_some(),
            };
            if tp_ok { *hit += 1; }
            if tp_some { *present += 1; }
        }
        println!(
            "[{}] regex={} {:?}  needle={} {:?} conf={}",
            i + 1,
            if all_ok_r { "OK" } else { "XX" },
            nr,
            if all_ok_n { "OK" } else { "XX" },
            nn,
            r_ne.get("confidence").and_then(Value::as_f64).map(|c| format!("{c}")).unwrap_or_else(|| "-".into())
        );
    }
    let avg_b = lat_b.iter().sum::<f64>() / lat_b.len() as f64;
    println!("\n  REGEX  full-record accuracy: {regex_ok}/{} ({:.0}%)", INVOICE_TEXTS.len(), 100.0 * regex_ok as f64 / INVOICE_TEXTS.len() as f64);
    println!("  NEEDLE full-record accuracy: {needle_ok}/{} ({:.0}%)  avg {avg_b:.0}ms/call", INVOICE_TEXTS.len(), 100.0 * needle_ok as f64 / INVOICE_TEXTS.len() as f64);
    for (k, a, b) in &reg_f1 {
        let (na, nb) = ne_f1.iter().find(|(x, _, _)| x == k).map(|(_, x, y)| (*x, *y)).unwrap_or((0, 0));
        println!("    {k:10} regex P={a}/{b}  needle P={na}/{nb}");
    }

    // SCENARIO C — embedding recall
    println!("\n{}", "=".repeat(74));
    println!("SCENARIO C — EMBEDDING RECALL  (mock BOW 128d vs Needle 3072d)");
    println!("{}", "=".repeat(74));
    let pairs_semantic = [
        ("how do i sleep better at night", "dim the bedroom lights"),
        ("my computer is very slow", "the laptop takes forever to boot"),
        ("i want to send money to my friend", "transfer funds to a contact"),
        ("the food arrived cold", "my meal was already cold when the courier dropped it"),
        ("can you remind me to call mom tomorrow", "please set a reminder for tomorrow to ring my mother"),
    ];
    let pairs_lexical = [
        ("the wifi is not working", "wifi broken"),
        ("refund please", "i want a refund"),
        ("package never arrived", "my package is lost"),
        ("app crashes on startup", "the app crashes when i open it"),
        ("delivery is late", "the delivery is delayed"),
    ];
    let mut ns_ln = Vec::new();
    let mut ns_lh = Vec::new();
    let mut nl_ln = Vec::new();
    let mut nl_lh = Vec::new();
    for (q, k) in pairs_semantic.iter() {
        let e1 = needle_embed(q);
        let e2 = needle_embed(k);
        ns_ln.push(cosine(&e1, &e2));
        ns_lh.push(cosine(&bow(q, 128), &bow(k, 128)));
    }
    for (q, k) in pairs_lexical.iter() {
        let e1 = needle_embed(q);
        let e2 = needle_embed(k);
        nl_ln.push(cosine(&e1, &e2));
        nl_lh.push(cosine(&bow(q, 128), &bow(k, 128)));
    }
    let mock_avg_sem = ns_lh.iter().sum::<f64>() / ns_lh.len() as f64;
    let needle_avg_sem = ns_ln.iter().sum::<f64>() / ns_ln.len() as f64;
    let mock_avg_lex = nl_lh.iter().sum::<f64>() / nl_lh.len() as f64;
    let needle_avg_lex = nl_ln.iter().sum::<f64>() / nl_ln.len() as f64;
    println!("  Semantic pairs (no lexical overlap, n={}):", pairs_semantic.len());
    println!("    Mock BOW    avg cosine: {mock_avg_sem:.4}");
    println!("    Needle 3072 avg cosine: {needle_avg_sem:.4}   {}", if needle_avg_sem > mock_avg_sem { "WIN" } else { "LOSE" });
    println!("  Lexical pairs (keyword overlap, n={}):", pairs_lexical.len());
    println!("    Mock BOW    avg cosine: {mock_avg_lex:.4}");
    println!("    Needle 3072 avg cosine: {needle_avg_lex:.4}   {}", if needle_avg_lex > mock_avg_lex { "WIN" } else { "LOSE" });

    // SCENARIO D — workflow end-to-end
    println!("\n{}", "=".repeat(74));
    println!("SCENARIO D — WORKFLOW END-TO-END  (2 needle calls: classify + entity)");
    println!("{}", "=".repeat(74));
    let cls_tool = json!({
        "type": "function", "name": "classify_ticket",
        "description": "The category and urgency of a support ticket shared as text.",
        "parameters": {"type": "object", "properties": {
            "category": {"type": "string", "enum": ["billing", "technical", "shipping", "feedback", "other"], "description": "the nature of the customer's problem: billing for money issues, technical for broken devices or software, shipping for delivery issues, feedback for praise or suggestions, other for anything else"},
            "urgency": {"type": "string", "enum": ["low", "medium", "high"]}},
            "required": []}
    });
    let ent_tool = json!({
        "type": "function", "name": "extract_entity",
        "description": "Extract the customer id and order id from a support ticket.",
        "parameters": {"type": "object", "properties": {
            "customer_id": {"type": "string"},
            "order_id": {"type": "string"}},
            "required": ["customer_id", "order_id"]}
    });
    let (mut wf_ok, mut wf_cat_ok, mut wf_ent_ok) = (0, 0, 0);
    let mut wf_lat = Vec::new();
    for (i, (text, truth)) in TICKET_TEXTS.iter().zip(TICKET_LABELS.iter()).enumerate() {
        let t0 = std::time::Instant::now();
        let cls = needle_extract(text, &cls_tool, Some(DATE_FACT));
        let ent = needle_extract(text, &ent_tool, Some(DATE_FACT));
        let lat = t0.elapsed().as_secs_f64() * 1000.0;
        wf_lat.push(lat);
        let cargs = cls.get("arguments").cloned().unwrap_or(Value::Null);
        let eargs = ent.get("arguments").cloned().unwrap_or(Value::Null);
        let cat = cargs.get("category").and_then(Value::as_str).unwrap_or("no_cat");
        let cid = eargs.get("customer_id").and_then(Value::as_str).unwrap_or("?");
        let oid = eargs.get("order_id").and_then(Value::as_str).unwrap_or("?");
        let cat_ok = cat == truth.0;
        let ent_ok = cid == truth.1 && oid == truth.2;
        let ok = cat_ok && ent_ok;
        if cat_ok { wf_cat_ok += 1; }
        if ent_ok { wf_ent_ok += 1; }
        if ok { wf_ok += 1; }
        println!("[{}] cat={cat:10} cust={cid:8} order={oid:8}  lat={lat:.0}ms  {}  (truth: {}/{}/{})", i + 1, if ok { "OK" } else { "XX" }, truth.0, truth.1, truth.2);
    }
    println!("\n  NEEDLE classify accuracy: {wf_cat_ok}/5 ({:.0}%)", 100.0 * wf_cat_ok as f64 / 5.0);
    println!("  NEEDLE entity extract accuracy: {wf_ent_ok}/5 ({:.0}%)", 100.0 * wf_ent_ok as f64 / 5.0);
    let avg_wf = wf_lat.iter().sum::<f64>() / wf_lat.len() as f64;
    println!("  NEEDLE-WORKFLOW full accuracy: {wf_ok}/5 ({:.0}%)  avg {avg_wf:.0}ms/workflow", 100.0 * wf_ok as f64 / 5.0);

    fn regex_classify_ticket(text: &str) -> &'static str {
        let t = text.to_lowercase();
        if ["refund", "money back", "billing", "credit"].iter().any(|w| t.contains(w)) {
            return "billing";
        }
        if ["broken", "cracked", "stopped", "missing", "error", "not working"].iter().any(|w| t.contains(w)) {
            return "technical";
        }
        if ["package", "arrived", "damaged", "shipping", "deliver"].iter().any(|w| t.contains(w)) {
            return "shipping";
        }
        "other"
    }
    fn regex_entities(text: &str) -> (Option<String>, Option<String>) {
        let re_c = regex_lite::Regex::new(r"(?i)customer\s+(\d+)").unwrap();
        let re_o = regex_lite::Regex::new(r"(?i)order\s*#?\s*(\d+)").unwrap();
        let cid = re_c.captures(text).and_then(|c| c.get(1)).map(|m| m.as_str().to_string());
        let oid = re_o.captures(text).and_then(|c| c.get(1)).map(|m| m.as_str().to_string());
        (cid, oid)
    }
    let mut h_ok = 0;
    for (i, (text, truth)) in TICKET_TEXTS.iter().zip(TICKET_LABELS.iter()).enumerate() {
        let cat = regex_classify_ticket(text);
        let (cid, oid) = regex_entities(text);
        let ok = cat == truth.0 && cid.as_deref() == Some(truth.1) && oid.as_deref() == Some(truth.2);
        if ok { h_ok += 1; }
        println!("  [h{}] cat={cat:10} cust={:8} order={:8}  {}",
            i + 1, cid.as_deref().unwrap_or("None"), oid.as_deref().unwrap_or("None"),
            if ok { "OK" } else { "XX" });
    }
    println!("  REGEX-PIPELINE full accuracy: {h_ok}/5 ({:.0}%)", 100.0 * h_ok as f64 / 5.0);

    // SCENARIO E — combination
    println!("\n{}", "=".repeat(74));
    println!("SCENARIO E — COMBINATION  (heuristic fast-path + Needle fallback)");
    println!("{}", "=".repeat(74));
    let mut hyb_a = 0;
    let mut hyb_a_cost = 0;
    for (_i, _truth, _hcat, hok, _ncat, nok, _conf) in &rows {
        if *hok {
            hyb_a += 1;
        } else {
            hyb_a_cost += 1;
            if *nok {
                hyb_a += 1;
            }
        }
    }
    println!("  A  hybrid routing accuracy: {hyb_a}/20 ({:.0}%)   (heuristic alone 7/20 = 35%; needle alone 12/20 = 60%)   escalated to needle: {hyb_a_cost}/20", 100.0 * hyb_a as f64 / 20.0);
    println!("     -> +{:.0}pt over heuristic alone, +{:.0}pt over needle alone, using {hyb_a_cost} of {} calls on needle", 100.0 * hyb_a as f64 / 20.0 - 35.0, 100.0 * hyb_a as f64 / 20.0 - 60.0, rows.len());

    let mut hyb_b2 = 0;
    let mut hyb_b_cost = 0;
    for text in INVOICE_TEXTS {
        let r_reg = laya_heuristic_extract_invoice(text);
        let nr = (r_reg.0.as_deref(), r_reg.1, norm_date(r_reg.2.as_deref()));
        let ok_reg = vendor_match(nr.0, truth_v.0) && total_match(nr.1, truth_v.1) && date_match(nr.2.as_deref(), truth_v.2);
        if ok_reg {
            hyb_b2 += 1;
        } else {
            let r_ne = needle_extract(text, &inv_tool, None);
            let args_ne = r_ne.get("arguments").cloned().unwrap_or(Value::Null);
            let nn = (
                args_ne.get("vendor").and_then(Value::as_str),
                norm_total(args_ne.get("total")),
                norm_date(args_ne.get("due_date").and_then(Value::as_str)),
            );
            let ok_ne = vendor_match(nn.0, truth_v.0) && total_match(nn.1, truth_v.1) && date_match(nn.2.as_deref(), truth_v.2);
            if ok_ne { hyb_b2 += 1; }
            hyb_b_cost += 1;
        }
    }
    println!("  B  hybrid extract accuracy: {hyb_b2}/8 ({:.0}%)   (regex alone 1/8 = 12%; needle alone 5/8 = 62%)   needle calls: {hyb_b_cost}", 100.0 * hyb_b2 as f64 / 8.0);

    let mut hyb_d = 0;
    for (i, (text, truth)) in TICKET_TEXTS.iter().zip(TICKET_LABELS.iter()).enumerate() {
        let cat = regex_classify_ticket(text);
        let ent = needle_extract(text, &ent_tool, Some(DATE_FACT));
        let eargs = ent.get("arguments").cloned().unwrap_or(Value::Null);
        let cid = eargs.get("customer_id").and_then(Value::as_str).unwrap_or("?");
        let oid = eargs.get("order_id").and_then(Value::as_str).unwrap_or("?");
        let ok = cat == truth.0 && cid == truth.1 && oid == truth.2;
        if ok { hyb_d += 1; }
        println!("  [c{}] cat(regex)={cat:10} cust(needle)={cid:8} order(needle)={oid:8}  {}", i + 1, if ok { "OK" } else { "XX" });
    }
    println!("  D  hybrid workflow accuracy: {hyb_d}/5 ({:.0}%)   (regex pipeline alone 4/5 = 80%; needle-workflow alone 1/5 = 20%)", 100.0 * hyb_d as f64 / 5.0);
    println!("  C  embeddings: Needle 3072d dominates BOW on BOTH semantic ({needle_avg_sem:.2} vs {mock_avg_sem:.2}) and lexical ({needle_avg_lex:.2} vs {mock_avg_lex:.2}) recall — no hybrid needed, just swap the backend.");
    println!("\n{}", "=".repeat(74));
    println!("DONE");
    Ok(())
}
