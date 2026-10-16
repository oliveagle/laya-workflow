//! BDD layer: Gherkin `.feature` → laya-workflow specs, with accuracy as the
//! hard gate.
//!
//! This is the Rust port of the former `scripts/bdd/*.py` toolchain
//! (`gherkin.py`, `steps.py`, `transpile.py`, `build.py`, `needle_assist.py`).
//! One `.feature` file is the single source of truth; it drives the workflow
//! spec, the local test manifest and the production integration-test plan.
//!
//! Accuracy is the gate, not a report:
//!   * a step the deterministic vocabulary cannot compile is a HARD ERROR
//!   * every generated spec must pass the engine's structural check
//!   * per-feature step coverage must reach `--coverage-min` (default 100%)
//!   * out-of-vocabulary steps get a needle *suggestion* (`--assist`), never
//!     an automatic compile
//!
//! The parsing/compiler logic is a line-faithful port of the Python originals;
//! their doc-comments (the "why" behind every deliberate quirk) are preserved
//! where they still hold.

use anyhow::{anyhow, bail, Context, Result};
use regex_lite::Regex;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// One `Given` / `When` / `Then` line after keyword resolution.
#[derive(Debug, Clone)]
pub struct Step {
    /// The literal keyword as written.
    pub keyword: String,
    /// Resolved: `given` | `when` | `then` (And/But/* inherit the previous).
    pub kind: String,
    /// Step text with `<placeholders>` already substituted.
    pub text: String,
    pub line: usize,
    /// Data-table rows attached to this step (`| a | b |` immediately after the
    /// step line, outside an Examples block). Used by the decision vocabulary
    /// to carry option/level tables. Empty for every browser step.
    pub table: Vec<Vec<String>>,
    /// The source line of each `table` row, in the same order, so a rejected row
    /// can be pointed at exactly (P161). Always the same length as `table` when
    /// the text came from `parse`; a hand-built `Step` (a test) may leave it empty.
    pub row_lines: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct Scenario {
    pub name: String,
    pub steps: Vec<Step>,
    pub tags: Vec<String>,
    pub line: usize,
    pub outline: bool,
    /// `(line, row)` for each Examples row; empty for a plain scenario.
    pub examples: Vec<(usize, Vec<(String, String)>)>,
}

impl Scenario {
    pub fn slug(&self) -> String {
        slugify(&self.name)
    }
}

#[derive(Debug, Clone)]
pub struct Feature {
    pub name: String,
    pub description: String,
    pub background: Vec<Step>,
    pub scenarios: Vec<Scenario>,
    pub tags: Vec<String>,
    pub path: String,
}

impl Feature {
    pub fn slug(&self) -> String {
        slugify(&self.name)
    }
}

/// `re.sub(r"[^a-z0-9]+", "_", name.lower()).strip("_")`, Python-faithful.
fn slugify(name: &str) -> String {
    let mut out = String::new();
    for ch in name.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let s = out.trim_matches('_').to_string();
    if s.is_empty() {
        "scenario".to_string()
    } else {
        s
    }
}

// ── shared regexes ─────────────────────────────────────────────────────────

/// Given / When / Then / And / But. And/But inherit the previous keyword.
fn step_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\s*(Given|When|Then|And|But|\*)\s+(.*\S)\s*$").expect("step_re")
    })
}

/// A line that failed [`step_re`] is usually a step whose *keyword* is mangled
/// (`Givn`/`Thenn`/`Wen`). Point at the closest keyword so the author fixes the
/// typo instead of re-reading "not a step" (P89 usability axis). Returns `""`
/// when nothing is close enough to guess safely — never a false lead.
fn keyword_typo_hint(line: &str) -> String {
    const KEYWORDS: [&str; 5] = ["Given", "When", "Then", "And", "But"];
    let trimmed = line.trim_start();
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    // A step needs a keyword AND a body; a single bare word is not a typo'd step.
    if words.len() < 2 {
        return String::new();
    }
    let first = words[0].to_ascii_lowercase();
    if KEYWORDS.iter().any(|k| k.to_ascii_lowercase() == first) {
        return String::new();
    }
    match crate::spec::closest_match(&first, &KEYWORDS) {
        Some(k) => format!(" (did you mean the `{k}` keyword?)"),
        None => String::new(),
    }
}

/// A tag with an optional parenthetical reason: `@expected_failure(reason)`.
fn tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*@(?P<name>[^\s(]+)(?:\((?P<reason>[^)]*)\))?\s*$").expect("tag_re"))
}

fn row_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*\|(.*)\|\s*$").expect("row_re"))
}

/// `include: <path>` inside a Background. Deliberately the *only* reuse
/// mechanism: a general `$ref` with parameters and nesting would be a larger
/// language to keep honest than the duplication it removes.
fn include_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^include:\s*(?P<path>[^\s]+)\s*$").expect("include_re"))
}

/// `<name>` in a step becomes `${state.name}`. The identifier shape is
/// deliberate: `i < 10` and `a<b` in a JavaScript expression are unaffected.
fn param_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<([A-Za-z_][A-Za-z0-9_]*)>").expect("param_re"))
}

fn placeholder_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\$\{state\.([A-Za-z_][A-Za-z0-9_]*)\}").expect("placeholder_re")
    })
}

// ── Gherkin parser ─────────────────────────────────────────────────────────

fn step_kind_for(keyword: &str) -> Result<String> {
    match keyword {
        "Given" => Ok("given".into()),
        "When" => Ok("when".into()),
        "Then" => Ok("then".into()),
        _ => bail!("internal: unresolved keyword {keyword}"),
    }
}

fn cells(line: &str, lineno: usize) -> Result<Vec<String>> {
    let caps = row_re()
        .captures(line)
        .ok_or_else(|| anyhow!("line {lineno}: expected a `| a | b |` table row, got: {}", line.trim()))?;
    // Split on `|`, honouring the standard Gherkin `\|` escape so a cell can
    // carry a literal pipe (e.g. a shell `df ... | tail -1` inside a JSON
    // capability config). Everything else passes through untouched — the
    // downstream parsers (quoted_tokens_escaped etc.) still see the raw text.
    let inner = &caps[1];
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'|') {
            cur.push('|');
            chars.next();
        } else if c == '|' {
            out.push(cur.trim().to_string());
            cur = String::new();
        } else {
            cur.push(c);
        }
    }
    out.push(cur.trim().to_string());
    Ok(out)
}

/// Replace `<placeholder>` with the example row's value.
fn substitute(text: &str, row: &[(String, String)], lineno: usize) -> Result<String> {
    let mut out = String::new();
    let mut rest = text;
    // manual scan so we can error on a missing column exactly like Python
    while let Some(start) = rest.find('<') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('>') else {
            out.push_str(rest);
            return Ok(out);
        };
        let key = &after[..end];
        out.push_str(&rest[..start]);
        if !key.chars().any(|c| !c.is_alphanumeric() && c != '_') {
            if let Some((_, v)) = row.iter().find(|(k, _)| k == key) {
                out.push_str(v);
            } else {
                bail!(
                    "line {lineno}: Examples table has no column {key:?} (used by {text:?})"
                );
            }
        } else {
            out.push_str(&text[start..start + 1 + end + 1]);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Steps from an `include:`d file. Every failure mode is loud: a silently
/// empty include would surface much later as a confusing "needs a page".
fn read_include(rel: &str, including: &str, lineno: usize) -> Result<Vec<Step>> {
    if Path::new(rel).is_absolute() {
        bail!(
            "line {lineno}: `include {rel}` must be a path relative to the including \
             file, not an absolute path"
        );
    }
    let base = if including == "<string>" {
        std::env::current_dir().unwrap_or_default()
    } else {
        Path::new(including)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default()
    };
    let target = base.join(rel);
    let norm = target.canonicalize().unwrap_or(target);
    if !norm.is_file() {
        bail!(
            "line {lineno}: `include {rel}` in {} does not exist (looked for {})",
            Path::new(including)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| including.to_string()),
            norm.display()
        );
    }
    let body = std::fs::read_to_string(&norm)
        .with_context(|| format!("reading include {rel}"))?;
    if body.lines().any(|l| include_re().is_match(l.trim())) {
        bail!(
            "line {lineno}: {rel} itself contains an `include`. Includes are one \
             level deep on purpose - a cycle is a hang, not an error"
        );
    }
    let mut steps = Vec::new();
    let mut last_kind: Option<String> = None;
    for (n, raw) in body.lines().enumerate() {
        let text = raw.trim();
        if text.is_empty() || text.starts_with('#') {
            continue;
        }
        let Some(caps) = step_re().captures(raw) else {
            bail!(
                "{rel} line {}: not a step: {text:?} - an included file is a flat \
                 list of Given/When/Then, not another feature",
                n + 1
            );
        };
        let keyword = caps[1].to_string();
        let body = caps[2].to_string();
        let kind = if keyword == "*" {
            last_kind
                .clone()
                .ok_or_else(|| anyhow!("{rel} line {}: `*` step with no preceding Given/When/Then", n + 1))?
        } else if keyword == "And" || keyword == "But" {
            last_kind
                .clone()
                .ok_or_else(|| anyhow!("{rel} line {}: {keyword} step with no preceding Given/When/Then", n + 1))?
        } else {
            step_kind_for(&keyword)?
        };
        last_kind = Some(kind.clone());
        steps.push(Step {
            keyword,
            kind,
            text: body,
            line: n + 1,
            table: Vec::new(),
            row_lines: Vec::new(),
        });
    }
    if steps.is_empty() {
        bail!(
            "line {lineno}: `include {rel}` contributed no steps. An include that \
             silently adds nothing is worse than no include"
        );
    }
    Ok(steps)
}

/// `@expected_failure(bdd.assert: FAIL equals)` → `Some("bdd.assert: FAIL equals")`.
/// Returns `None` for an absent *or bare* tag, so a bare `@expected_failure` is
/// an error rather than "no reason needed" — that is the point of requiring one.
pub fn tag_reason(scenario: &Scenario, tag: &str) -> Option<String> {
    let prefix = format!("{tag}(");
    for t in &scenario.tags {
        if t.starts_with(&prefix) && t.ends_with(')') {
            let inner = t[prefix.len()..t.len() - 1].trim();
            if !inner.is_empty() {
                return Some(inner.to_string());
            }
        }
    }
    None
}

/// Parse a Gherkin document. Rejects what it does not understand instead of
/// guessing: a step that silently parses wrong is worse than one that fails.
pub fn parse(text: &str, path: &str) -> Result<Feature> {
    let mut feature: Option<Feature> = None;
    let mut scenario: Option<Scenario> = None;
    let mut pending_tags: Vec<String> = Vec::new();
    let mut last_kind: Option<String> = None;
    let mut in_background = false;
    let mut in_examples = false;
    let mut examples_header: Option<Vec<String>> = None;
    let mut section: Option<String> = None;
    let mut desc: Vec<String> = Vec::new();

    for (idx, raw) in text.lines().enumerate() {
        let lineno = idx + 1;
        let line = raw.trim_end();
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }

        if let Some(caps) = tag_re().captures(line) {
            let name = caps["name"].to_string();
            let reason = caps
                .name("reason")
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            if reason.is_empty() {
                pending_tags.push(name);
            } else {
                pending_tags.push(format!("{name}({reason})"));
            }
            continue;
        }

        // A line that starts with '@' but that the tag grammar above did not accept
        // (an unclosed `@outputs(k`, a space before the `(`, a space inside the name)
        // used to fall through to the *description* text: the tag never reached the
        // compiler, so the declaration the author wrote disappeared and every gate
        // stayed green. Refuse it by name instead - the same stance as the rest of
        // this parser, which would rather fail than guess (P169).
        if stripped.starts_with('@') {
            bail!(
                "line {lineno}: {stripped:?} starts with '@' but is not a tag this parser \
                 reads - a tag is `@name` or `@name(...)`, with no space between the name \
                 and the '(' and no text after the ')'. It would be taken for description \
                 text, so the declaration it was meant to make would be lost. Fix the tag, \
                 or start the line with '#' if it is prose."
            );
        }

        let head = stripped
            .split_once(':')
            .map(|(h, _)| h.trim().to_lowercase())
            .unwrap_or_default();
        if matches!(
            head.as_str(),
            "feature" | "background" | "scenario" | "scenario outline" | "examples" | "rule"
        ) {
            let (label, rest) = stripped
                .split_once(':')
                .map(|(l, r)| (l, r))
                .unwrap_or(("", ""));
            let sec = if head == "rule" {
                "scenario".to_string()
            } else {
                head.clone()
            };
            section = Some(sec.clone());

            if sec == "feature" {
                feature = Some(Feature {
                    name: rest.trim().to_string(),
                    description: String::new(),
                    background: Vec::new(),
                    scenarios: Vec::new(),
                    tags: std::mem::take(&mut pending_tags),
                    path: path.to_string(),
                });
                desc.clear();
                in_background = false;
                in_examples = false;
                continue;
            }

            let f = feature.as_mut().ok_or_else(|| {
                anyhow!("line {lineno}: {}: before any Feature:", label.trim())
            })?;

            if sec == "background" {
                in_background = true;
                in_examples = false;
                scenario = None;
                continue;
            }
            if sec == "examples" {
                in_background = false;
                in_examples = true;
                if scenario.is_none() {
                    bail!("line {lineno}: Examples: outside of a Scenario");
                }
                examples_header = None;
                continue;
            }

            // scenario / scenario outline / rule
            in_background = false;
            in_examples = false;
            if let Some(s) = &scenario {
                if s.outline && s.examples.is_empty() {
                    bail!(
                        "line {lineno}: Scenario Outline {:?} has no Examples table",
                        s.name
                    );
                }
            }
            scenario = Some(Scenario {
                name: if rest.trim().is_empty() {
                    format!("scenario@{lineno}")
                } else {
                    rest.trim().to_string()
                },
                steps: Vec::new(),
                tags: std::mem::take(&mut pending_tags),
                line: lineno,
                outline: sec == "scenario outline",
                examples: Vec::new(),
            });
            last_kind = None;
            f.scenarios.push(scenario.clone().unwrap());
            continue;
        }

        let f = feature.as_mut().ok_or_else(|| {
            anyhow!("line {lineno}: text before any Feature: header: {stripped:?}")
        })?;

        if in_examples {
            let sc = scenario.as_mut().expect("checked above");
            let c = cells(stripped, lineno)?;
            if examples_header.is_none() {
                let mut seen = BTreeSet::new();
                for h in &c {
                    if !seen.insert(h.clone()) {
                        bail!("line {lineno}: duplicate column in Examples header {c:?}");
                    }
                }
                examples_header = Some(c);
            } else {
                let hdr = examples_header.as_ref().unwrap();
                if c.len() != hdr.len() {
                    bail!(
                        "line {lineno}: Examples row has {} cells, header has {}: {}",
                        c.len(),
                        hdr.len(),
                        stripped
                    );
                }
                sc.examples.push((
                    lineno,
                    hdr.iter().cloned().zip(c).collect(),
                ));
            }
            // keep the fresh scenario in sync (we cloned into feature.scenarios)
            if let Some(last) = f.scenarios.last_mut() {
                *last = sc.clone();
            }
            continue;
        }

        if section.as_deref() == Some("feature")
            && f.scenarios.is_empty()
            && !["given", "when", "then", "and", "but"]
                .contains(&&*stripped.to_lowercase())
        {
            desc.push(stripped.to_string());
            f.description = desc.join("\n");
            continue;
        }

        if let Some(caps) = include_re().captures(stripped) {
            if !in_background {
                bail!(
                    "line {lineno}: `include` is only allowed inside a Background, \
                     not in a {:?} block",
                    section.as_deref().unwrap_or("")
                );
            }
            let inc = read_include(&caps["path"], path, lineno)?;
            f.background.extend(inc);
            last_kind = None;
            continue;
        }

        // A `| a | b |` data-table row attached to the previous step. Used by
        // the decision vocabulary to carry option/level tables. Rows inside an
        // Examples block are handled above; anywhere else a row must belong to
        // the step that immediately precedes it.
        if row_re().captures(stripped).is_some() {
            let row = cells(stripped, lineno)?;
            if in_background {
                match f.background.last_mut() {
                    Some(st) => {
                        st.table.push(row);
                        st.row_lines.push(lineno);
                    }
                    None => bail!("line {lineno}: table row without a preceding step"),
                }
            } else {
                let Some(sc) = scenario.as_mut() else {
                    bail!("line {lineno}: table row outside a Background/Scenario");
                };
                let Some(st) = sc.steps.last_mut() else {
                    bail!("line {lineno}: table row without a preceding step");
                };
                st.table.push(row);
                st.row_lines.push(lineno);
                if let Some(last) = f.scenarios.last_mut() {
                    *last = sc.clone();
                }
            }
            continue;
        }

        let Some(caps) = step_re().captures(line) else {
            bail!(
                "line {lineno}: not a step, table or section header: {stripped:?}{}",
                keyword_typo_hint(stripped)
            );
        };
        let keyword = caps[1].to_string();
        let body = caps[2].to_string();
        let kind = if keyword == "*" {
            last_kind
                .clone()
                .ok_or_else(|| anyhow!("line {lineno}: `*` step with no preceding Given/When/Then"))?
        } else if keyword == "And" || keyword == "But" {
            last_kind
                .clone()
                .ok_or_else(|| anyhow!("line {lineno}: {keyword} step with no preceding Given/When/Then"))?
        } else {
            step_kind_for(&keyword)?
        };
        last_kind = Some(kind.clone());
        let st = Step {
            keyword,
            kind,
            text: body,
            line: lineno,
            table: Vec::new(),
            row_lines: Vec::new(),
        };
        if in_background {
            f.background.push(st);
        } else if scenario.is_some() {
            let sc = scenario.as_mut().unwrap();
            sc.steps.push(st);
            if let Some(last) = f.scenarios.last_mut() {
                *last = sc.clone();
            }
        } else {
            bail!("line {lineno}: step outside Background/Scenario: {stripped:?}");
        }
    }

    let mut feature = feature.ok_or_else(|| anyhow!("no Feature: header found"))?;
    if feature.scenarios.is_empty() {
        bail!("feature has no Scenario");
    }
    for s in &feature.scenarios {
        if s.outline && s.examples.is_empty() {
            bail!("Scenario Outline {:?} has no Examples table", s.name);
        }
    }
    expand_outlines(&mut feature);
    Ok(feature)
}

/// Turn every Examples row into a concrete scenario, so downstream never has
/// to think about outlines again.
fn expand_outlines(feature: &mut Feature) {
    let mut out = Vec::new();
    for s in std::mem::take(&mut feature.scenarios) {
        if !s.outline {
            out.push(s);
            continue;
        }
        let nrows = s.examples.len();
        for (i, (lineno, row)) in s.examples.iter().enumerate() {
            // `substitute` only errors on a column the Examples table lacks;
            // those were already validated at parse time, so unwrap is safe.
            let suffix = if nrows > 1 {
                format!(" (row {})", i + 1)
            } else {
                String::new()
            };
            let mut name = substitute(&s.name, row, *lineno).unwrap_or_else(|_| s.name.clone());
            name.push_str(&suffix);
            let steps = s
                .steps
                .iter()
                .map(|st| Step {
                    text: substitute(&st.text, row, st.line).unwrap_or_else(|_| st.text.clone()),
                    ..st.clone()
                })
                .collect();
            out.push(Scenario {
                name,
                steps,
                tags: s.tags.clone(),
                line: *lineno,
                outline: false,
                examples: Vec::new(),
            });
        }
    }
    feature.scenarios = out;
}

pub fn load(path: &Path) -> Result<Feature> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&text, &path.to_string_lossy())
}

// ── vocabulary: step text → op / assertion / arguments ─────────────────────

fn unquote(tok: &str) -> String {
    let t = tok.trim();
    if t.len() >= 2 {
        let b: Vec<char> = t.chars().collect();
        if (b[0] == '"' || b[0] == '\'') && b[0] == b[b.len() - 1] {
            return t[1..t.len() - 1].to_string();
        }
    }
    t.to_string()
}

/// A Gherkin operand as the JSON value it was *written* as. Quoted means
/// string, full stop. Unquoted: try number/bool, else bare string. This is what
/// keeps `equals 3` (int) distinct from `equals "3"` (string) — the type the
/// author wrote is the whole signal, and `json.loads` on its own would get
/// `"3"` backwards.
fn typed_operand(raw: &str) -> Value {
    let tok = raw.trim();
    if tok.len() >= 2 {
        let b: Vec<char> = tok.chars().collect();
        if (b[0] == '"' || b[0] == '\'') && b[0] == b[b.len() - 1] {
            return json!(unquote(tok));
        }
    }
    serde_json::from_str::<Value>(tok).unwrap_or_else(|_| json!(tok))
}

/// What a node publishes back into state.
fn project(capability: &str) -> Value {
    match capability {
        "bdd" => json!({"target_id": "/target_id"}),
        // An exec step files its shell result into state so later assertions
        // can read it — the same keys the DSL specs' exec capability uses.
        "exec" => json!({
            "exit_code": "/exit_code",
            "stdout": "/stdout",
            "stderr": "/stderr"
        }),
        _ => json!({}),
    }
}

#[derive(Debug, Clone)]
struct CompiledNode {
    instruction: String,
    action: Option<Value>,
}

/// The vocabulary tables. `(regex, op, capability)` / `(regex, op, assertion)`.
fn given_table() -> &'static Vec<(Regex, &'static str, &'static str)> {
    static T: OnceLock<Vec<(Regex, &'static str, &'static str)>> = OnceLock::new();
    T.get_or_init(|| {
        vec![
            (Regex::new(r"^the browser is ready$").unwrap(), "open", "bdd"),
            (Regex::new(r"^I am on (?P<url>.+)$").unwrap(), "open", "bdd"),
        ]
    })
}

fn when_table() -> &'static Vec<(Regex, &'static str, &'static str)> {
    static T: OnceLock<Vec<(Regex, &'static str, &'static str)>> = OnceLock::new();
    T.get_or_init(|| {
        vec![
            (Regex::new(r"^I open (?P<url>.+)$").unwrap(), "open", "bdd"),
            (
                Regex::new(r"^I navigate to (?P<url>.+)$").unwrap(),
                "navigate",
                "bdd",
            ),
            (
                Regex::new(r"^I wait for the element (?P<sel>.+)$").unwrap(),
                "wait_for",
                "bdd",
            ),
            (
                Regex::new(r"^I click the element (?P<sel>.+?) if it is present$").unwrap(),
                "click_if_present",
                "bdd",
            ),
            (
                Regex::new(r"^I click the element (?P<sel>.+)$").unwrap(),
                "click",
                "chrome",
            ),
            (
                Regex::new(r"^I type (?P<text>.+?) into the element (?P<sel>.+)$").unwrap(),
                "type",
                "chrome",
            ),
            (
                Regex::new(r"^I select (?P<value>.+?) in the element (?P<sel>.+)$").unwrap(),
                "select",
                "chrome",
            ),
            (
                Regex::new(r"^I run javascript (?P<expr>.+)$").unwrap(),
                "evaluate",
                "bdd",
            ),
            (
                Regex::new(r"^I run the command (?P<cmd>.+)$").unwrap(),
                "exec",
                "exec",
            ),
            (
                Regex::new(r"^I release the page$").unwrap(),
                "release",
                "bdd",
            ),
            // ── workflow vocabulary: data extraction & conditional & key ──
            (
                Regex::new(r"^I extract the text of the element (?P<sel>.+?) into (?P<var>.+)$").unwrap(),
                "extract_text",
                "bdd",
            ),
            (
                Regex::new(r"^I extract the attribute (?P<attr>.+?) of the element (?P<sel>.+?) into (?P<var>.+)$").unwrap(),
                "extract_attribute",
                "bdd",
            ),
            (
                Regex::new(r"^I extract the page url into (?P<var>.+)$").unwrap(),
                "extract_url",
                "bdd",
            ),
            (
                Regex::new(r"^I extract the page title into (?P<var>.+)$").unwrap(),
                "extract_title",
                "bdd",
            ),
            (
                Regex::new(r"^I press the key (?P<key>.+)$").unwrap(),
                "key",
                "chrome",
            ),
            (
                Regex::new(r"^I wait until the element (?P<sel>.+?) becomes (?P<state>.+)$").unwrap(),
                "wait_until",
                "bdd",
            ),
        ]
    })
}

/// `(regex, op, assertion-name)`. **Order is load-bearing**: `equals text "x"`
/// also matches the loose `equals` rule, so the specific pattern must be tried
/// first. With the loose one first the step compiled *silently* to `equals`
/// against the literal string `text "x"` — a quiet wrong assertion, worse than
/// a hard error.
fn then_table() -> &'static Vec<(Regex, &'static str, &'static str)> {
    static T: OnceLock<Vec<(Regex, &'static str, &'static str)>> = OnceLock::new();
    T.get_or_init(|| {
        vec![
            (
                Regex::new(r"^the page title contains (?P<v>.+)$").unwrap(),
                "assert",
                "title_contains",
            ),
            (
                Regex::new(r"^the page url contains (?P<v>.+)$").unwrap(),
                "assert",
                "url_contains",
            ),
            (
                Regex::new(r"^the saved value (?P<v>.+?) equals text (?P<v2>.+)$").unwrap(),
                "assert",
                "state_equals",
            ),
            (
                Regex::new(r"^the saved value (?P<v>.+?) contains text (?P<v2>.+)$").unwrap(),
                "assert",
                "state_contains",
            ),
            // ── exec-result assertions: compare values an `I run the command`
            //    step projected into state (exit_code / stdout / stderr). They
            //    never touch the page, so `needs_page` stays false. ──
            (
                Regex::new(r"^the exit code is (?P<v>.+)$").unwrap(),
                "assert",
                "exit_code_equals",
            ),
            (
                Regex::new(r"^stdout contains (?P<v>.+)$").unwrap(),
                "assert",
                "stdout_contains",
            ),
            (
                Regex::new(r"^stderr contains (?P<v>.+)$").unwrap(),
                "assert",
                "stderr_contains",
            ),
            (
                Regex::new(r"^the element (?P<v>.+?) is visible$").unwrap(),
                "assert",
                "visible",
            ),
            (
                Regex::new(r"^the element (?P<v>.+?) is absent$").unwrap(),
                "assert",
                "absent",
            ),
            (
                Regex::new(r"^javascript (?P<v>.+?) is true$").unwrap(),
                "assert",
                "is_true",
            ),
            (
                Regex::new(r"^javascript (?P<v>.+?) is false$").unwrap(),
                "assert",
                "is_false",
            ),
            (
                Regex::new(r"^javascript (?P<v>.+?) equals[ _]text (?P<v2>.+)$").unwrap(),
                "assert",
                "equals_text",
            ),
            (
                Regex::new(r"^javascript (?P<v>.+?) equals (?P<v2>.+)$").unwrap(),
                "assert",
                "equals",
            ),
            (
                Regex::new(r"^javascript (?P<v>.+?) contains (?P<v2>.+)$").unwrap(),
                "assert",
                "contains",
            ),
        ]
    })
}

// ── decision-workflow vocabulary ────────────────────────────────────────────
//
// A `@decision` feature expresses a decision-graph workflow (the agentic core):
// one Scenario per node, questions defined with Given, edge routing with When,
// and ordered threshold rules / copy_keys with Then. Steps are recognised by
// `dec_match`; they never go through the browser `compile_step` path.
//
// `(regex, tag)` — tag selects the handler in `compile_decision_node`.
fn dec_given_table() -> &'static Vec<(Regex, &'static str)> {
    static T: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    T.get_or_init(|| {
        vec![
            (
                Regex::new(r#"^a choice question "(?P<q>[^"]+)" asks "(?P<text>.*)"$"#).unwrap(),
                "choice",
            ),
            (
                Regex::new(r#"^a score question "(?P<q>[^"]+)" asks "(?P<text>.*)"$"#).unwrap(),
                "score",
            ),
            (
                Regex::new(r#"^a noul question "(?P<q>[^"]+)" asks "(?P<text>.*)"$"#).unwrap(),
                "noul",
            ),
            (
                Regex::new(r#"^the sub workflow "(?P<w>.+)"$"#).unwrap(),
                "sub_workflow",
            ),
            (
                Regex::new(r#"^when the sub workflow completes continue at "(?P<w>[^"]+)"$"#).unwrap(),
                "sub_workflow_continue",
            ),
            (
                Regex::new(r"^the workflow runs at most (?P<n>\d+) iterations$").unwrap(),
                "max_iterations",
            ),
            (
                Regex::new(r"^the policy$").unwrap(),
                "policy",
            ),
            (
                Regex::new(r"^the capabilities$").unwrap(),
                "capabilities",
            ),
            // ── heuristics (attach right after their question; they configure
            //    the *most recent* question in the same Scenario) ──
            (
                Regex::new(
                    r#"^answer "(?P<ans>[^"]+)" when the field "(?P<field>[^"]+)" contains (?P<needles>.+) with hit (?P<hit>\d+(?:\.\d+)?)(?: and miss (?P<miss>\d+(?:\.\d+)?))?$"#,
                )
                .unwrap(),
                "heur_field_any",
            ),
            (
                Regex::new(
                    r#"^answer "(?P<ans>[^"]+)" when the text contains (?P<needles>.+) with hit (?P<hit>\d+(?:\.\d+)?)(?: and miss (?P<miss>\d+(?:\.\d+)?))?$"#,
                )
                .unwrap(),
                "heur_any",
            ),
            (
                Regex::new(
                    r#"^score "(?P<ans>[^"]+)" from the numeric field "(?P<field>[^"]+)"(?: with miss (?P<miss>\d+(?:\.\d+)?))?$"#,
                )
                .unwrap(),
                "heur_numeric",
            ),
            (
                Regex::new(
                    r#"^answer "(?P<ans>[^"]+)" when the field "(?P<field>[^"]+)" matches (?P<patterns>.+) with hit (?P<hit>\d+(?:\.\d+)?)(?: and miss (?P<miss>\d+(?:\.\d+)?))?$"#,
                )
                .unwrap(),
                "heur_regex_field",
            ),
            (
                Regex::new(
                    r#"^answer "(?P<ans>[^"]+)" when the text matches (?P<patterns>.+) with hit (?P<hit>\d+(?:\.\d+)?)(?: and miss (?P<miss>\d+(?:\.\d+)?))?$"#,
                )
                .unwrap(),
                "heur_regex",
            ),
            (
                Regex::new(
                    r#"^answer "(?P<ans>[^"]+)" with mid (?P<mid>\d+(?:\.\d+)?) when the field "(?P<field>[^"]+)" contains (?P<needles>.+)$"#,
                )
                .unwrap(),
                "heur_mid",
            ),
            (
                Regex::new(
                    r#"^answer by rules on the field "(?P<field>[^"]+)" with hit (?P<hit>\d+(?:\.\d+)?)(?: and fallback "(?P<fallback>[^"]+)")?$"#,
                )
                .unwrap(),
                "heur_match_rules",
            ),
            (
                Regex::new(
                    r#"^score "(?P<ans>[^"]+)" when the field "(?P<field>[^"]+)" contains "(?P<high>[^"]+)" scoring (?P<hit>\d+(?:\.\d+)?) and "(?P<mid>[^"]+)" scoring (?P<midv>\d+(?:\.\d+)?) miss (?P<miss>\d+(?:\.\d+)?)$"#,
                )
                .unwrap(),
                "heur_score_bands",
            ),
            (
                Regex::new(r#"^the answer is always "(?P<a>.+)"$"#).unwrap(),
                "heur_answer",
            ),
            (
                Regex::new(r#"^the answer comes from the field "(?P<f>.+)"$"#).unwrap(),
                "heur_answer_field",
            ),
        ]
    })
}

fn dec_when_table() -> &'static Vec<(Regex, &'static str)> {
    static T: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    T.get_or_init(|| {
        vec![
            (
                Regex::new(r#"^the answer "(?P<a>[^"]+)" goes to "(?P<node>.+)"$"#).unwrap(),
                "edge",
            ),
            (
                Regex::new(r#"^any other answer goes to "(?P<node>.+)"$"#).unwrap(),
                "edge_default",
            ),
        ]
    })
}

fn dec_then_table() -> &'static Vec<(Regex, &'static str)> {
    static T: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    T.get_or_init(|| {
        vec![
            (
                // A conjunction of leaf conditions joined by ` and `. Listed
                // first so a genuine conjunction (which may itself contain a
                // quoted `"` or the word `and`) is not swallowed by the looser
                // single-condition rules below. `parse_conjunction_leaves`
                // splits on the ` and ` that starts another condition, and the
                // marker `(?:"|the answer)` keeps an ` and ` inside a quoted
                // option from matching.
                Regex::new(
                    r#"^label "(?P<label>[^"]+)" when (?P<rest>.+ and (?:"|the answer).+)$"#,
                )
                .unwrap(),
                "label_all",
            ),
            (
                Regex::new(
                    r#"^label "(?P<label>[^"]+)" when the answer to "(?P<q>[^"]+)" is "(?P<a>.+)"$"#,
                )
                .unwrap(),
                "label_eq",
            ),
            (
                Regex::new(
                    r#"^label "(?P<label>[^"]+)" when the answer to "(?P<q>[^"]+)" is one of (?P<list>.+)$"#,
                )
                .unwrap(),
                "label_eq_in",
            ),
            (
                Regex::new(
                    r#"^label "(?P<label>[^"]+)" when the answer is "(?P<a>.+)"$"#,
                )
                .unwrap(),
                "label_eq_implicit",
            ),
            (
                Regex::new(
                    r#"^label "(?P<label>[^"]+)" when the answer to "(?P<q>[^"]+)" is "(?P<a>.+)" with at least (?P<c>\d+(?:\.\d+)?) confidence$"#,
                )
                .unwrap(),
                "label_opt_gte",
            ),
            (
                Regex::new(
                    r#"^label "(?P<label>[^"]+)" when the answer to "(?P<q>[^"]+)" is "(?P<a>.+)" with at most (?P<c>\d+(?:\.\d+)?) confidence$"#,
                )
                .unwrap(),
                "label_opt_lt",
            ),
            (
                Regex::new(
                    r#"^label "(?P<label>[^"]+)" when the answer is "(?P<a>.+)" with at least (?P<c>\d+(?:\.\d+)?) confidence$"#,
                )
                .unwrap(),
                "label_opt_gte_implicit",
            ),
            (
                Regex::new(
                    r#"^label "(?P<label>[^"]+)" when "(?P<q>[^"]+)" is at least (?P<n>\d+(?:\.\d+)?)$"#,
                )
                .unwrap(),
                "label_gte",
            ),
            (
                Regex::new(
                    r#"^label "(?P<label>[^"]+)" when "(?P<q>[^"]+)" is below (?P<n>\d+(?:\.\d+)?)$"#,
                )
                .unwrap(),
                "label_lt",
            ),
            (
                Regex::new(r#"^label "(?P<label>.+)" otherwise$"#).unwrap(),
                "label_always",
            ),
            (
                Regex::new(r#"^keep the answer to "(?P<q>.+)"$"#).unwrap(),
                "copy_keys",
            ),
            (
                Regex::new(r#"^keep the state key "(?P<k>.+)"$"#).unwrap(),
                "keep_state",
            ),
            (
                // P143: `state.rename` (project_state's `{out: in}` map) had no
                // feature-language spelling, so `import` could not reproduce it.
                Regex::new(r#"^map the state key "(?P<out>[^"]+)" from "(?P<in>[^"]+)"$"#)
                    .unwrap(),
                "rename_state",
            ),
            (
                Regex::new(r"^require confidence at least (?P<n>\d+(?:\.\d+)?)$").unwrap(),
                "min_conf",
            ),
            (
                Regex::new(r"^allow up to (?P<n>\d+) retries$").unwrap(),
                "max_retries",
            ),
            (
                Regex::new(r#"^the primary question is "(?P<q>.+)"$"#).unwrap(),
                "primary_q",
            ),
            (
                Regex::new(r#"^the action reads the question "(?P<q>.+)"$"#).unwrap(),
                "action_q",
            ),
            (
                Regex::new(r#"^call the capability "(?P<cap>.+)"$"#).unwrap(),
                "call",
            ),
            (
                Regex::new(
                    r#"^gate "(?P<q>.+)" on option "(?P<opt>.+)" with block (?P<block>\d+(?:\.\d+)?) allow (?P<allow>\d+(?:\.\d+)?)$"#,
                )
                .unwrap(),
                "gate",
            ),
            (
                Regex::new(r#"^merge open probabilities on "(?P<q>.+)"$"#).unwrap(),
                "merge_open_probs",
            ),
            (
                Regex::new(
                    r#"^combine "(?P<choice>.+)" with signal "(?P<signal>.+)" fallback "(?P<fallback>.+)" review "(?P<review>.+)" requiring (?P<min>\d+(?:\.\d+)?) high (?P<high>\d+(?:\.\d+)?) low (?P<low>\d+(?:\.\d+)?)$"#,
                )
                .unwrap(),
                "combine",
            ),
            (
                Regex::new(
                    r#"^rubric "(?P<q1>.+)" at (?P<l1>\d+) levels weight (?P<w1>\d+(?:\.\d+)?) and "(?P<q2>.+)" at (?P<l2>\d+) levels weight (?P<w2>\d+(?:\.\d+)?) requiring (?P<min>\d+(?:\.\d+)?)$"#,
                )
                .unwrap(),
                "rubric",
            ),
            (
                Regex::new(
                    r#"^prepare a UI proposal from operation "(?P<op>[^"]+)" field "(?P<field>[^"]+)" amount "(?P<amt>[^"]+)" surface "(?P<surface>[^"]+)" hash "(?P<hash>[^"]+)" requiring (?P<min>\d+(?:\.\d+)?)(?: value "(?P<value_key>[^"]+)")?$"#,
                )
                .unwrap(),
                "ui_proposal",
            ),
        ]
    })
}

/// The leaf conditions a conjunctive `label` rule may join with ` and `. Each
/// mirrors one single-condition step, minus the `label "..." when ` prefix, so a
/// conjunct reads exactly like the standalone step it generalises.
fn leaf_when_table() -> &'static Vec<(Regex, &'static str)> {
    static T: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    T.get_or_init(|| {
        vec![
            (
                Regex::new(r#"^the answer to "(?P<q>[^"]+)" is "(?P<a>[^"]+)" with at least (?P<c>\d+(?:\.\d+)?) confidence$"#).unwrap(),
                "opt_gte",
            ),
            (
                Regex::new(r#"^the answer to "(?P<q>[^"]+)" is "(?P<a>[^"]+)" with at most (?P<c>\d+(?:\.\d+)?) confidence$"#).unwrap(),
                "opt_lt",
            ),
            (
                Regex::new(r#"^the answer to "(?P<q>[^"]+)" is one of (?P<list>.+)$"#).unwrap(),
                "ans_in",
            ),
            (
                Regex::new(r#"^the answer to "(?P<q>[^"]+)" is "(?P<a>[^"]+)"$"#).unwrap(),
                "ans",
            ),
            (
                Regex::new(r#"^the answer is "(?P<a>[^"]+)" with at least (?P<c>\d+(?:\.\d+)?) confidence$"#).unwrap(),
                "opt_gte_impl",
            ),
            (
                Regex::new(r#"^the answer is "(?P<a>[^"]+)"$"#).unwrap(),
                "ans_impl",
            ),
            (
                Regex::new(r#"^"(?P<q>[^"]+)" is at least (?P<n>\d+(?:\.\d+)?)$"#).unwrap(),
                "v_gte",
            ),
            (
                Regex::new(r#"^"(?P<q>[^"]+)" is below (?P<n>\d+(?:\.\d+)?)$"#).unwrap(),
                "v_lt",
            ),
        ]
    })
}

fn parse_num(s: &str) -> Option<f64> {
    s.parse::<f64>().ok()
}

/// Parse one leaf condition of a conjunctive rule into its `when` object.
fn parse_leaf_when(text: &str) -> Option<Value> {
    let t = text.trim();
    for (re, tag) in leaf_when_table().iter() {
        let Some(c) = re.captures(t) else { continue };
        return match *tag {
            "opt_gte" => Some(json!({"question": c["q"].to_string(), "option": c["a"].to_string(), "gte": parse_num(&c["c"])?})),
            "opt_lt" => Some(json!({"question": c["q"].to_string(), "option": c["a"].to_string(), "lt": parse_num(&c["c"])?})),
            "ans_in" => {
                let toks = quoted_tokens(&c["list"]);
                if toks.is_empty() {
                    None
                } else {
                    Some(json!({"question": c["q"].to_string(), "answer_in": toks}))
                }
            }
            "ans" => Some(json!({"question": c["q"].to_string(), "answer": c["a"].to_string()})),
            "opt_gte_impl" => Some(json!({"option": c["a"].to_string(), "gte": parse_num(&c["c"])?})),
            "ans_impl" => Some(json!({"answer": c["a"].to_string()})),
            "v_gte" => Some(json!({"question": c["q"].to_string(), "value_gte": parse_num(&c["n"])?})),
            "v_lt" => Some(json!({"question": c["q"].to_string(), "value_lt": parse_num(&c["n"])?})),
            _ => None,
        };
    }
    None
}

/// Split a conjunctive rule body into leaf `when` objects. Parses the whole
/// body first (so an ` and ` inside a quoted option does not split a leaf),
/// then walks every ` and ` boundary, taking the first split whose left side is
/// a leaf and whose right side recurses. Fails loudly on a body that is not a
/// conjunction of known leaves — the compiler never silently drops a condition.
fn parse_conjunction_leaves(rest: &str) -> Result<Vec<Value>> {
    if let Some(w) = parse_leaf_when(rest) {
        return Ok(vec![w]);
    }
    let mut from = 0usize;
    while let Some(pos) = rest[from..].find(" and ") {
        let i = from + pos;
        let (l, r) = (&rest[..i], &rest[i + 5..]);
        if let Some(lw) = parse_leaf_when(l) {
            if let Ok(mut rw) = parse_conjunction_leaves(r) {
                let mut v = vec![lw];
                v.append(&mut rw);
                return Ok(v);
            }
        }
        from = i + 5;
    }
    bail!(
        "cannot parse `{rest}` as conditions joined by ` and ` (each must be a `the answer to \"…\" is …` or `\"question\" is at least|below …` condition)"
    )
}

/// Match a decision step and return `(captures, tag)`. Order is load-bearing:
/// the specific `when the answer to ... is ...` rule is tried before the loose
/// `when "Q" is at least N` rule, and both before `otherwise`.
fn dec_match<'a>(
    kind: &str,
    text: &'a str,
) -> Result<(regex_lite::Captures<'a>, &'static str)> {
    let table = match kind {
        "given" => dec_given_table(),
        "when" => dec_when_table(),
        "then" => dec_then_table(),
        _ => bail!("unknown decision step kind {kind:?}"),
    };
    for (re, tag) in table.iter() {
        if let Some(caps) = re.captures(text) {
            return Ok((caps, tag));
        }
    }
    bail!("unknown decision {kind} step: {text:?}")
}

/// Every step name / op the vocabulary covers (for `--list`-style output and
/// "did you mean" hints).
pub fn vocabulary() -> Vec<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    for t in [given_table(), when_table(), then_table()] {
        for (_, op, extra) in t {
            if *op == "assert" {
                set.insert(extra.to_string());
            } else {
                set.insert(op.to_string());
            }
        }
    }
    set.into_iter().collect()
}

/// The step *patterns* (as regex sources) — used by the doc/vocab gates.
pub fn vocabulary_patterns() -> Vec<&'static str> {
    let mut v = Vec::new();
    for t in [given_table(), when_table(), then_table()] {
        for (re, _, _) in t {
            v.push(re.as_str());
        }
    }
    v
}

/// Human-readable templates for the decision-workflow vocabulary, derived from
/// the `dec_given/when/then_table` regexes (capture groups → `<name>`). Used to
/// turn an unmatched step in an @decision feature into an actionable
/// "did you mean <template>?" suggestion (P28 usability axis).
pub fn decision_templates() -> &'static [(&'static str, &'static str)] {
    &[
        ("given", "a choice question \"<q>\" asks \"<text>\" (table: | answer | meaning |)"),
        ("given", "a score question \"<q>\" asks \"<text>\" (table: | level |)"),
        ("given", "a noul question \"<q>\" asks \"<text>\" (table: | truthy/falsy | text |)"),
        ("given", "the sub workflow \"<name>\""),
        ("given", "when the sub workflow completes continue at \"<node>\""),
        ("given", "the workflow runs at most <n> iterations"),
        ("given", "the policy (table: | key | value |)"),
        ("given", "the capabilities (table: | name | config |)"),
        ("given", "answer \"<ans>\" when the field \"<field>\" contains <needles> with hit <hit>"),
        ("given", "answer \"<ans>\" when the text contains <needles> with hit <hit>"),
        ("given", "score \"<ans>\" from the numeric field \"<field>\""),
        ("given", "answer \"<ans>\" when the field \"<field>\" matches <patterns> with hit <hit>"),
        ("given", "answer \"<ans>\" when the text matches <patterns> with hit <hit>"),
        ("given", "answer \"<ans>\" with mid <mid> when the field \"<field>\" contains <needles>"),
        ("given", "answer by rules on the field \"<field>\" with hit <hit>"),
        ("given", "score \"<ans>\" when the field \"<field>\" contains \"<high>\" scoring <hit> and \"<mid>\" scoring <midv> miss <miss>"),
        ("given", "the answer is always \"<a>\""),
        ("given", "the answer comes from the field \"<f>\""),
        ("when", "the answer \"<a>\" goes to \"<node>\""),
        ("when", "any other answer goes to \"<node>\""),
        ("then", "label \"<label>\" when the answer to \"<q>\" is \"<a>\""),
        ("then", "label \"<label>\" when the answer to \"<q>\" is \"<a>\" with at least <c> confidence"),
        ("then", "label \"<label>\" when the answer to \"<q>\" is \"<a>\" with at most <c> confidence"),
        ("then", "label \"<label>\" when the answer is \"<a>\" with at least <c> confidence"),
        ("then", "label \"<label>\" when the answer to \"<q>\" is one of <list>"),
        ("then", "label \"<label>\" when the answer is \"<a>\""),
        ("then", "label \"<label>\" when <cond> and <cond>"),
        ("then", "label \"<label>\" when \"<q>\" is at least <n>"),
        ("then", "label \"<label>\" when \"<q>\" is below <n>"),
        ("then", "label \"<label>\" otherwise"),
        ("then", "keep the answer to \"<q>\""),
        ("then", "keep the state key \"<k>\""),
        ("then", "map the state key \"<out>\" from \"<in>\""),
        ("then", "require confidence at least <n>"),
        ("then", "allow up to <n> retries"),
        ("then", "the primary question is \"<q>\""),
        ("then", "the action reads the question \"<q>\""),
        ("then", "call the capability \"<cap>\""),
        ("then", "gate \"<q>\" on option \"<opt>\" with block <block> allow <allow>"),
        ("then", "merge open probabilities on \"<q>\""),
        ("then", "combine \"<choice>\" with signal \"<signal>\" fallback \"<fallback>\" review \"<review>\" requiring <min> high <high> low <low>"),
        ("then", "rubric \"<q1>\" at <l1> levels weight <w1> and \"<q2>\" at <l2> levels weight <w2> requiring <min>"),
        ("then", "prepare a UI proposal from operation \"<op>\" field \"<field>\" amount \"<amt>\" surface \"<surface>\" hash \"<hash>\" requiring <min>"),
    ]
}

/// The three step kinds a `@decision` feature uses, in authoring order. Kept
/// next to [`decision_templates`] so the vocabulary listing and the compiler's
/// suggestion index cannot drift apart.
pub const VOCABULARY_KINDS: [&str; 3] = ["given", "when", "then"];

/// The decision authoring vocabulary as JSON: the curated step templates the
/// `@decision` compiler is built around, grouped by keyword kind. This is the
/// same list behind the `did you mean the decision ... step` hints, exposed so
/// an agent can read the grammar *before* it writes a step instead of only
/// after a red build. `note` states plainly what it is and what it is not.
pub fn vocabulary_json() -> Value {
    let templates = decision_templates();
    let steps: Vec<Value> = templates
        .iter()
        .map(|(kind, tmpl)| json!({"kind": kind, "template": tmpl}))
        .collect();
    let mut by_kind = serde_json::Map::new();
    for kind in VOCABULARY_KINDS {
        let list: Vec<&str> = templates
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, t)| *t)
            .collect();
        by_kind.insert(kind.to_string(), json!(list));
    }
    json!({
        "count": steps.len(),
        "kinds": VOCABULARY_KINDS,
        "steps": steps,
        "by_kind": Value::Object(by_kind),
        "dialects": vocabulary_dialects(),
        "note": "Curated authoring templates for a @decision feature, grouped by \
keyword kind (given/when/then). Write a step by copying a template and \
replacing <placeholders>; the compiler accepts these phrasings and suggests \
the closest one on a typo. Deliberately not an exhaustive grammar - these are \
the phrasings the did-you-mean index knows. Validate the compiled graph with \
bdd_validate (or `bdd build`) before committing."
    })
}

/// Table dialects the compiler accepts inside a step's table, beyond the plain
/// `| answer | meaning |` option list - phrasings an author cannot guess from a
/// one-line template. Exposed by `bdd vocabulary` (CLI `--json` and MCP
/// `bdd_vocabulary`), so an agent discovers the dynamic-candidate grammar
/// *before* writing it, not from a red build. Single source for both surfaces.
pub fn vocabulary_dialects() -> Value {
    json!([
        {
            "step": "a choice question \"<q>\" asks \"<text>\"",
            "dialect": "dynamic criteria from a state array (code extracts the candidates, the model only selects one id - the value is never generated)",
            "table": [
                "| answer | meaning |",
                "| from_state | <state array key, e.g. candidates> |",
                "| id_key | <id field, default id> |",
                "| value_key | <value field, default value> |",
                "| none | <text for the no-candidate option> |"
            ],
            "note": "with `from_state`, the sibling keys must be exactly from_state/id_key/value_key/none; the engine materialises one option per candidate id from `state[<state array key>]` at decide time and always adds `none` (the fail-closed no-candidate answer). See `bdd scaffold --pattern dynamic`."
        },
        {
            "step": "call the capability \"<cap>\"",
            "dialect": "parameterised capability call + state projection + chained sub-calls (same step, special table rows)",
            "table": [
                "| with.<key> | <value> |",
                "| project.<state key> | <dotted pointer into the call result> |",
                "| chain.<as>.capability | <capability name> |",
                "| chain.<as>.with.<key> | <value> |",
                "| chain.<as>.project.<state key> | <dotted pointer> |"
            ],
            "note": "every row must start with `with.`, `project.` or `chain.`; a row that starts with anything else is refused by name. `with.<key>` passes an argument (a JSON literal, a plain string, or a `${state.x}` / `${env.X}` template); `project.<key>` stores a dotted pointer from the call result (e.g. `/body/html`) under a state key; `chain.<as>.…` adds a sub-call the engine runs after the primary one - each distinct `<as>` is one nested call, in the order its rows appear. Compiles to `{kind: call, capability, with, project, chain: [{capability, with, project}]}`."
        }
    ])
}

/// Options for `laya-workflow bdd vocabulary`.
pub struct VocabularyOptions {
    /// Emit machine-readable JSON instead of the human-readable list.
    pub json: bool,
}

/// `laya-workflow bdd vocabulary`: print the decision authoring vocabulary - the
/// templates behind the compiler's `did you mean` hints - so an agent (or a
/// human) can read the grammar before writing a step. `--json` returns the same
/// list for a tool to consume.
pub fn vocabulary_cli(o: &VocabularyOptions) -> Result<i32> {
    if o.json {
        println!("{}", serde_json::to_string_pretty(&vocabulary_json())?);
        return Ok(0);
    }
    let all = decision_templates();
    println!(
        "decision vocabulary: {} template(s) across {} keyword kind(s)",
        all.len(),
        VOCABULARY_KINDS.len()
    );
    for kind in VOCABULARY_KINDS {
        let list: Vec<&str> = all.iter().filter(|(k, _)| *k == kind).map(|(_, t)| *t).collect();
        println!("\n{kind} ({})", list.len());
        for t in list {
            println!("  {t}");
        }
    }
    if let Some(ds) = vocabulary_dialects().as_array() {
        println!("\ntable dialects (same step, special table rows):");
        for d in ds {
            let step = d.get("step").and_then(Value::as_str).unwrap_or("");
            let di = d.get("dialect").and_then(Value::as_str).unwrap_or("");
            println!("  {step}  ->  {di}");
            if let Some(rows) = d.get("table").and_then(Value::as_array) {
                for r in rows {
                    if let Some(r) = r.as_str() {
                        println!("      {r}");
                    }
                }
            }
        }
    }
    println!(
        "\ncopy a template and replace <placeholders>; then validate with `bdd build <file>` (or MCP bdd_validate)."
    );
    Ok(0)
}

/// "did you mean" suggestion from the decision vocabulary for an unmatched step.
/// Tokenises the step text and each template's literal words (capture placeholders
/// `<name>` ignored), prefers the same keyword kind, and returns the closest
/// template when >=2 literal words overlap.
pub fn decision_suggest(kind: &str, text: &str) -> Option<String> {
    // Stopwords that carry no signal when matching literal step words.
    const STOP: [&str; 4] = ["the", "and", "with", "any"];
    let words: std::collections::HashSet<&str> = text
        .split(|c: char| !c.is_ascii_lowercase() && c != '_')
        .filter(|w| w.len() >= 3 && !STOP.contains(w))
        .collect();
    if words.is_empty() {
        return None;
    }
    // Distinct literal words of a template (capture placeholders `<name>` and
    // stopwords ignored) that also appear in the step text.
    let overlap = |tmpl: &str| {
        let tw: std::collections::HashSet<&str> = tmpl
            .split(|c: char| !c.is_ascii_lowercase() && c != '_')
            .filter(|w| w.len() >= 3 && !w.starts_with('<') && !STOP.contains(w))
            .collect();
        tw.iter().filter(|w| words.contains(*w)).count()
    };
    let mut best: Option<(usize, &'static str)> = None;
    // Pass 1: same keyword kind, >=2 shared literal words.
    for &(tkind, tmpl) in decision_templates() {
        if tkind != kind {
            continue;
        }
        let shared = overlap(tmpl);
        if shared >= 2 && best.map(|(b, _)| shared > b).unwrap_or(true) {
            best = Some((shared, tmpl));
        }
    }
    // Pass 2: no same-kind hit → accept a cross-kind template (the author put
    // a `Then` step under `Given`/`And`, or vice versa). Only fills the gap.
    if best.is_none() {
        for &(_, tmpl) in decision_templates() {
            let shared = overlap(tmpl);
            if shared >= 2 && best.map(|(b, _)| shared > b).unwrap_or(true) {
                best = Some((shared, tmpl));
            }
        }
    }
    best.map(|(_, tmpl)| tmpl.to_string())
}

// ── P131: decision-side vocabulary-drift gate ───────────────────────────────
//
// `decision_templates` is the single source behind `bdd vocabulary` / MCP
// `bdd_vocabulary` and the "did you mean" hints. It is hand-maintained next to
// the compiler's own tables (`dec_given/when/then_table`), so the two can drift
// silently: a template the index advertises but the compiler no longer accepts
// (an author copies it and gets a red build), or a step shape the compiler
// accepts but the index omits (a hint can never suggest it). This gate closes
// both directions on the live tables.

/// A sample token to stand in for one `<placeholder>` when a template is turned
/// back into a concrete step. `quoted` is true when the placeholder already sits
/// between the quotes in the template (e.g. `"<field>"`), so the same name can be
/// a string in one template and a number in another (`"<high>"` vs `high <high>`).
fn dec_sample(name: &str, quoted: bool) -> String {
    if quoted {
        return "x".to_string();
    }
    match name {
        // list-ish placeholders render as a one-element quoted list.
        "needles" | "patterns" | "list" => "\"x\"".to_string(),
        // a full leaf condition for the conjunctive label rule.
        "cond" => "the answer is \"x\"".to_string(),
        // numeric placeholders.
        "n" | "c" | "hit" | "miss" | "mid" | "midv" | "min" | "block" | "allow" | "w1" | "w2"
        | "l1" | "l2" | "low" | "high" => "1".to_string(),
        // everything else is a bare word where a quoted string is expected.
        _ => "x".to_string(),
    }
}

/// Turn one advertised template into a concrete step: drop a trailing
/// `(table: …)` annotation, then replace every `<name>` with a sample.
pub fn instantiate_decision_template(tmpl: &str) -> String {
    let body = match tmpl.find(" (table:") {
        Some(i) => &tmpl[..i],
        None => tmpl,
    };
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after = &rest[lt + 1..];
        match after.find('>') {
            Some(gt) => {
                let name = &after[..gt];
                let quoted = out.ends_with('"') && after[gt + 1..].starts_with('"');
                out.push_str(&dec_sample(name, quoted));
                rest = &after[gt + 1..];
            }
            None => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Does a template compile? Instantiate it and feed it to the decision matcher
/// for the same keyword kind. `false` = the index advertises a phrasing the
/// compiler rejects (the dangerous direction of the drift).
pub fn decision_template_compiles(kind: &str, tmpl: &str) -> bool {
    dec_match(kind, &instantiate_decision_template(tmpl)).is_ok()
}

/// The compiler's live step shapes: keyword kind → the set of tags its table can
/// produce. One tag = one accepted phrasing (one regex).
pub fn decision_shape_tags(
) -> std::collections::BTreeMap<&'static str, std::collections::BTreeSet<&'static str>> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut m: BTreeMap<&'static str, BTreeSet<&'static str>> = BTreeMap::new();
    for (kind, table) in [
        ("given", dec_given_table()),
        ("when", dec_when_table()),
        ("then", dec_then_table()),
    ] {
        m.insert(kind, table.iter().map(|(_, tag)| *tag).collect());
    }
    m
}

/// Every way the advertised vocabulary and the compiler's shapes can disagree,
/// for the given template list. Empty means "the index covers every shape and
/// nothing it advertises is dead". Pure over its inputs, so a regression can pass
/// a truncated list and watch it fire.
pub fn decision_vocabulary_gaps(templates: &[(&str, &str)]) -> Vec<String> {
    use std::collections::{BTreeMap, BTreeSet};
    let shapes = decision_shape_tags();
    let mut problems: Vec<String> = Vec::new();
    let mut produced: BTreeMap<String, BTreeSet<&'static str>> = BTreeMap::new();
    for (kind, tmpl) in templates {
        if !VOCABULARY_KINDS.iter().any(|k| k == kind) {
            problems.push(format!(
                "template advertised under unknown kind {kind:?}: {tmpl:?}"
            ));
            continue;
        }
        let text = instantiate_decision_template(tmpl);
        match dec_match(kind, &text) {
            Ok((_, tag)) => {
                produced.entry(kind.to_string()).or_default().insert(tag);
            }
            Err(_) => problems.push(format!(
                "advertised {kind} template does not compile: {tmpl:?} (instantiated: {text:?})"
            )),
        }
    }
    for (kind, tags) in &shapes {
        let seen = produced.get(*kind);
        for tag in tags {
            if !seen.map(|s| s.contains(*tag)).unwrap_or(false) {
                problems.push(format!(
                    "{kind} step shape {tag:?} is accepted by the compiler but no advertised \
                     template covers it (a did-you-mean hint can never suggest it)"
                ));
            }
        }
    }
    problems
}

pub fn decision_vocab_check() -> Result<i32> {
    let (rc, _) = decision_vocab_check_impl(false)?;
    Ok(rc)
}

fn decision_vocab_check_impl(silent: bool) -> Result<(i32, Option<String>)> {
    let templates = decision_templates();
    let problems = decision_vocabulary_gaps(templates);
    if !problems.is_empty() {
        eprintln!(
            "bdd decision-vocab-check: {} drift problem(s) between the advertised \
             vocabulary and the compiler:",
            problems.len()
        );
        for p in &problems {
            eprintln!("  {p}");
        }
        return Ok((1, None));
    }
    let shapes = decision_shape_tags();
    let per_kind: Vec<String> = VOCABULARY_KINDS
        .iter()
        .map(|k| format!("{k}={}", shapes.get(k).map(|s| s.len()).unwrap_or(0)))
        .collect();
    let total: usize = shapes.values().map(|s| s.len()).sum();
    let line = format!(
        "decision vocabulary: {} advertised template(s) cover all {} compiler step shape(s) \
         across {} keyword kind(s) ({})",
        templates.len(),
        total,
        VOCABULARY_KINDS.len(),
        per_kind.join(", ")
    );
    if !silent {
        println!("{line}");
    }
    Ok((0, Some(line)))
}

/// P35: a semantic-validation error names the offending node (a Scenario slug)
/// and quotes the identifier(s) it cannot resolve. `validate_graph_semantics`
/// runs in `spec.rs` on the compiled JSON, far from the `.feature` text, so the
/// error carries no line. Map it back here: pull the offending identifier out of
/// the error grammar (the token next to `unknown ...`, else the token just
/// before `is not`), then report the first step whose text contains it. Falls
/// back to the named scenario's first step (e.g. an unreachable-node error,
/// which quotes only the dead node).
pub fn locate_semantic_error_line(feature: &Feature, msg: &str) -> Option<usize> {
    // Every `"..."` literal in the error, with its byte offset.
    let mut tokens: Vec<(usize, &str)> = Vec::new();
    let b = msg.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            if let Some(rel) = msg[i + 1..].find('"') {
                let tok = &msg[i + 1..i + 1 + rel];
                if !tok.is_empty() {
                    tokens.push((i + 1, tok));
                }
                i += rel + 2;
                continue;
            }
        }
        i += 1;
    }
    // Offending identifier by error grammar: the token right after the first
    // `unknown` marker (`unknown node "X"` / `references unknown question "Q"`),
    // else the token immediately before `is not` (`primary_q "X" is not ...` /
    // `edge condition key "X" is not an answer of primary question "PQ"` — where
    // the trailing `PQ` is a valid reference and must not win).
    let offending = if let Some(idx) = msg.find("unknown") {
        tokens.iter().find(|(start, _)| *start > idx).map(|(_, t)| *t)
    } else if let Some(idx) = msg.find("is not") {
        tokens
            .iter()
            .rev()
            .find(|(start, _)| *start < idx)
            .map(|(_, t)| *t)
    } else if let Some(idx) = msg.find("capability") {
        // P39: `action calls undeclared capability "X"` — the quoted name is the
        // offending token; point the author at the exact `call the capability`
        // step instead of the Scenario's first line.
        tokens.iter().find(|(start, _)| *start > idx).map(|(_, t)| *t)
    } else {
        None
    };
    if let Some(tok) = offending {
        if let Some(line) = feature
            .scenarios
            .iter()
            .flat_map(|s| s.steps.iter())
            .find(|st| st.text.contains(tok))
            .map(|st| st.line)
        {
            return Some(line);
        }
    }
    // Fallback: the named scenario's first step line.
    let rest = msg.strip_prefix("node ")?;
    let end = rest.find([':', ' ']).unwrap_or(rest.len());
    let node = &rest[..end];
    feature
        .scenarios
        .iter()
        .find(|s| s.slug() == node)
        .and_then(|s| s.steps.first().map(|st| st.line))
}

fn match_step<'a>(kind: &str, text: &'a str) -> Result<(regex_lite::Captures<'a>, &'static str, &'static str)> {
    let table = match kind {
        "given" => given_table(),
        "when" => when_table(),
        "then" => then_table(),
        _ => bail!("unknown step kind {kind:?}"),
    };
    for (re, op, extra) in table.iter() {
        if let Some(caps) = re.captures(text) {
            return Ok((caps, op, extra));
        }
    }
    // Decision-workflow vocabulary fallback (only valid inside @decision
    // features; compile_step rejects these, compile_decision handles them).
    let dtable = match kind {
        "given" => dec_given_table(),
        "when" => dec_when_table(),
        "then" => dec_then_table(),
        _ => bail!("unknown step kind {kind:?}"),
    };
    for (re, tag) in dtable.iter() {
        if let Some(caps) = re.captures(text) {
            return Ok((caps, tag, "dec"));
        }
    }
    let known = vocabulary();
    let words: Vec<&str> = text
        .split(|c: char| !c.is_ascii_lowercase() && c != '_')
        .collect();
    let close: Vec<String> = known
        .iter()
        .filter(|n| {
            n.split(|c: char| !c.is_ascii_lowercase() && c != '_')
                .any(|w| w.len() > 3 && text.contains(w) && words.contains(&w))
        })
        .cloned()
        .collect();
    let mut hint = if close.is_empty() {
        String::new()
    } else {
        format!(" (did you mean: {}?)", close.join(", "))
    };
    // @decision features have their own vocabulary; point the author at the
    // closest decision step so a typo is fixable without --assist (P28).
    if let Some(t) = decision_suggest(kind, text) {
        let extra = format!("did you mean the decision {kind} step: {t}?");
        hint = if hint.is_empty() {
            format!(" ({extra})")
        } else {
            format!("{hint} ({extra})")
        };
    }
    bail!(
        "unknown {kind} step: {text:?}{hint}\n  supported: {}",
        known.join(", ")
    )
}

/// Return `(with-args, human instruction, needs-a-page)`.
fn build_args(
    op: &str,
    extra: &str,
    caps: &regex_lite::Captures<'_>,
) -> Result<(Map<String, Value>, String, bool, Option<Value>)> {
    let get = |k: &str| caps.name(k).map(|m| m.as_str().to_string());
    let v = unquote(&get("v").unwrap_or_default());

    if op == "open" {
        let Some(url) = get("url") else {
            // `the browser is ready` — a declaration, not an action.
            return Ok((
                Map::new(),
                "Is the CDP browser connected and ready to accept steps?".into(),
                false,
                None,
            ));
        };
        let url = unquote(&url);
        return Ok((
            {
                let mut m = Map::new();
                m.insert("url".into(), json!(url.clone()));
                m
            },
            format!("Did Chrome open {url} and reach a loaded page?"),
            false,
            None,
        ));
    }
    if op == "navigate" {
        let url = unquote(&get("url").unwrap_or_default());
        return Ok((
            {
                let mut m = Map::new();
                m.insert("url".into(), json!(url.clone()));
                m
            },
            format!("Did the page navigate to {url}?"),
            true,
            None,
        ));
    }
    if op == "release" {
        return Ok((Map::new(), "Was the page closed?".into(), true, None));
    }
    if op == "wait_for" {
        let sel = unquote(&get("sel").unwrap_or_default());
        return Ok((
            {
                let mut m = Map::new();
                m.insert("selector".into(), json!(sel.clone()));
                m
            },
            format!("Is {sel} present on the page?"),
            true,
            None,
        ));
    }
    if op == "click" {
        let sel = unquote(&get("sel").unwrap_or_default());
        return Ok((
            {
                let mut m = Map::new();
                m.insert("selector".into(), json!(sel.clone()));
                m
            },
            format!("Was {sel} clicked?"),
            true,
            None,
        ));
    }
    if op == "type" {
        let text = unquote(&get("text").unwrap_or_default());
        let sel = unquote(&get("sel").unwrap_or_default());
        return Ok((
            {
                let mut m = Map::new();
                m.insert("selector".into(), json!(sel.clone()));
                m.insert("text".into(), json!(text.clone()));
                m
            },
            format!("Was {text:?} typed into {sel}?"),
            true,
            None,
        ));
    }
    if op == "select" {
        let value = unquote(&get("value").unwrap_or_default());
        let sel = unquote(&get("sel").unwrap_or_default());
        return Ok((
            {
                let mut m = Map::new();
                m.insert("value".into(), json!(value.clone()));
                m.insert("selector".into(), json!(sel.clone()));
                m
            },
            format!("Was {value:?} selected in {sel}?"),
            true,
            None,
        ));
    }
    if op == "evaluate" {
        // The capture group is `expr`, not `v`. Reading `v` handed the plugin
        // an empty expression — a call that could only ever throw — for a whole
        // release cycle because no scenario used the step.
        let expr = unquote(&get("expr").unwrap_or_default());
        return Ok((
            {
                let mut m = Map::new();
                m.insert("expression".into(), json!(expr.clone()));
                m
            },
            format!("Did the script run on the page: {expr:?}?"),
            true,
            None,
        ));
    }
    if op == "exec" {
        // `I run the command "…"` compiles to the engine's native `exec`
        // capability. The argv is a single `/bin/sh -c` of the whole command,
        // and `cmd` is passed per-call so one capability serves every exec step
        // in the scenario. The exec capability projects exit_code/stdout/stderr
        // into state (see `project("exec")`); nothing here needs a page.
        let cmd = unquote(&get("cmd").unwrap_or_default());
        return Ok((
            {
                let mut m = Map::new();
                m.insert("cmd".into(), json!(cmd.clone()));
                m
            },
            format!("Did the command exit 0: {cmd:?}?"),
            false,
            None,
        ));
    }
    if op == "extract_text" {
        let sel = unquote(&get("sel").unwrap_or_default());
        let var = get("var").unwrap_or_default();
        return Ok((
            {
                let mut m = Map::new();
                m.insert("expression".into(), json!(format!("document.querySelector('{}').textContent", sel)));
                m
            },
            format!("Extracted text of {sel} into state.{var}"),
            true,
            Some(json!({ var.clone(): "/value" })),
        ));
    }
    if op == "extract_attribute" {
        let sel = unquote(&get("sel").unwrap_or_default());
        let attr = unquote(&get("attr").unwrap_or_default());
        let var = get("var").unwrap_or_default();
        return Ok((
            {
                let mut m = Map::new();
                m.insert("expression".into(), json!(format!("document.querySelector('{}').getAttribute('{}')", sel, attr)));
                m
            },
            format!("Extracted attribute {attr} of {sel} into state.{var}"),
            true,
            Some(json!({ var.clone(): "/value" })),
        ));
    }
    if op == "extract_url" {
        let var = get("var").unwrap_or_default();
        return Ok((
            {
                let mut m = Map::new();
                m.insert("expression".into(), json!("location.href"));
                m
            },
            format!("Extracted page url into state.{var}"),
            true,
            Some(json!({ var.clone(): "/value" })),
        ));
    }
    if op == "extract_title" {
        let var = get("var").unwrap_or_default();
        return Ok((
            {
                let mut m = Map::new();
                m.insert("expression".into(), json!("document.title"));
                m
            },
            format!("Extracted page title into state.{var}"),
            true,
            Some(json!({ var.clone(): "/value" })),
        ));
    }
    if op == "key" {
        let key = unquote(&get("key").unwrap_or_default());
        return Ok((
            {
                let mut m = Map::new();
                m.insert("key".into(), json!(key.clone()));
                m
            },
            format!("Pressed key {key:?}"),
            true,
            None,
        ));
    }
    if op == "wait_until" {
        let sel = unquote(&get("sel").unwrap_or_default());
        let state = unquote(&get("state").unwrap_or_default());
        // Each state is a guarded boolean, so a page where the element is not
        // there yet is "not yet satisfied" rather than a TypeError the poll
        // would report as an error. `visible` uses the same notion of visible
        // the `visible` assertion does (a box on screen, not merely present);
        // `absent` is the one that means "not in the DOM at all".
        let q = format!("document.querySelector('{sel}')");
        let js = match state.as_str() {
            "visible" => format!(
                "(function(){{var e={q}; if(!e) return false; var r=e.getBoundingClientRect(); \
                 return (r.width>0||r.height>0) && getComputedStyle(e).visibility!=='hidden';}})()"
            ),
            "invisible" => format!(
                "(function(){{var e={q}; if(!e) return true; var r=e.getBoundingClientRect(); \
                 return !((r.width>0||r.height>0) && getComputedStyle(e).visibility!=='hidden');}})()"
            ),
            "absent" => format!("!{q}"),
            "enabled" => format!("(function(){{var e={q}; return !!e && !e.disabled;}})()"),
            "disabled" => format!("(function(){{var e={q}; return !!e && !!e.disabled;}})()"),
            _ => bail!(
                "wait_until: unsupported state {state:?} (visible | invisible | absent | enabled | disabled)"
            ),
        };
        return Ok((
            {
                let mut m = Map::new();
                m.insert("expression".into(), json!(js.clone()));
                m
            },
            format!("Waited for {sel} to become {state}"),
            true,
            None,
        ));
    }
    if op == "click_if_present" {
        let sel = unquote(&get("sel").unwrap_or_default());
        return Ok((
            {
                let mut m = Map::new();
                m.insert("selector".into(), json!(sel.clone()));
                m.insert("if_present".into(), json!(true));
                m
            },
            format!("Clicked {sel} if it was present"),
            true,
            None,
        ));
    }
    if op == "assert" {
        let mut m = Map::new();
        m.insert("assertion".into(), json!(extra));
        if matches!(extra, "exit_code_equals" | "stdout_contains" | "stderr_contains") {
            // `I run the command` projected exit_code/stdout/stderr into state;
            // compare those against the document's literal. Like the state_*
            // family, no page is involved, so `needs_page` stays false.
            let key = match extra {
                "exit_code_equals" => "exit_code",
                "stdout_contains" => "stdout",
                _ => "stderr",
            };
            let raw = get("v").unwrap_or_default();
            let expected = if extra == "exit_code_equals" {
                typed_operand(raw.trim())
            } else {
                json!(unquote(&raw))
            };
            m.insert("value".into(), json!(format!("${{state.{key}}}")));
            m.insert("expected".into(), expected);
            let what = if extra == "exit_code_equals" {
                format!("exit code {}", raw.trim())
            } else {
                format!("{key} to contain {:?}", unquote(&raw))
            };
            return Ok((
                m,
                format!("Did the command's {what}?"),
                false,
                None,
            ));
        }
        if matches!(extra, "state_equals" | "state_contains") {
            // `v` names a state key (a variable an earlier extract step set),
            // not a page expression; the operand is a literal string. Compare
            // it to `expected` directly. No page is needed to check collected
            // workflow outputs, so `needs_page` is false.
            let key = unquote(&get("v").unwrap_or_default());
            let text = unquote(&get("v2").unwrap_or_default());
            m.insert("value".into(), json!(format!("${{state.{key}}}")));
            m.insert("expected".into(), json!(text.clone()));
            let verb = if extra == "state_equals" { "equal" } else { "contain" };
            return Ok((
                m,
                format!(
                    "Does state.{key} {verb} {:?}?",
                    text
                ),
                false,
                None,
            ));
        }
        m.insert("value".into(), json!(v.clone()));
        let desc = if matches!(extra, "equals" | "equals_text" | "contains") {
            let raw = get("v2").unwrap_or_default();
            let text = unquote(&raw);
            if extra == "equals" {
                m.insert("expected".into(), typed_operand(raw.trim()));
                format!(
                    "({v}) === {}",
                    serde_json::to_string(m.get("expected").unwrap()).unwrap_or_default()
                )
            } else {
                m.insert("expected".into(), json!(text.clone()));
                format!(
                    "String({v}) {} {}",
                    if extra == "equals_text" { "==" } else { "contains" },
                    serde_json::to_string(&text).unwrap_or_default()
                )
            }
        } else {
            format!("{extra}({})", serde_json::to_string(&v).unwrap_or_default())
        };
        return Ok((
            m,
            format!("Did the assertion hold: {desc}?"),
            true,
            None,
        ));
    }
    bail!("no builder for op {op:?}")
}

/// Compile one step. Returns `(node, page-exists-after)`.
fn compile_step(step: &Step, has_page: bool) -> Result<(CompiledNode, bool)> {
    // A Scenario Outline column, or any hand-written `<name>`, becomes a state
    // key interpolated at run time.
    let text = param_re()
        .replace_all(&step.text, |caps: &regex_lite::Captures| {
            format!("${{state.{}}}", &caps[1])
        })
        .into_owned();
    let (caps, op, extra) = match_step(&step.kind, &text)?;
    if extra == "dec" {
        bail!(
            "line {}: decision step {text:?} is only valid in a @decision feature \
             (bdd build compiles it into a decision-graph workflow, not a browser call chain)",
            step.line
        );
    }
    let (mut args, instruction, needs_page, project_override) = build_args(op, extra, &caps)?;

    if needs_page && !has_page {
        bail!(
            "line {}: step {text:?} needs a page, but no earlier step opened one. \
             Add `Given I am on \"<url>\"` (or `When I open \"<url>\"`) before it.",
            step.line
        );
    }

    let capability = if matches!(op, "click" | "type" | "select" | "key" | "click_if_present") {
        "chrome"
    } else if op == "exec" {
        // `I run the command` uses the engine's native exec capability, not the
        // bdd plugin — the plugin owns page ops, the engine owns processes.
        "exec"
    } else {
        "bdd"
    };
    // The step's op name is not always the capability op it runs: the extract
    // family reuse the plugin's `evaluate` op (they differ only in what they
    // project out of its result), and `click_if_present` runs the chrome
    // capability's `click` with an `if_present` flag. Keeping one op per step
    // name in the vocabulary, but dispatching to the reused implementation,
    // means the vocabulary stays readable while the capability surface stays
    // small and already tested.
    let cap_op = match op {
        "extract_text" | "extract_attribute" | "extract_url" | "extract_title" => "evaluate",
        "click_if_present" => "click",
        other => other,
    };
    if has_page {
        args.insert("target_id".into(), json!("${state.target_id}"));
    }

    // Build the action's `with` dict in Python-faithful order: `op` first,
    // then the args the op carries, then `target_id` if a page is open. The
    // existing `args` Map was already filled in `op, …` order by `build_args`
    // (or `op, url` for `open`); we rebuild here so `op` is always key #0.
    let mut with: Map<String, Value> = Map::new();
    with.insert("op".into(), json!(cap_op));
    for (k, v) in args.into_iter() {
        if k != "op" {
            with.insert(k, v);
        }
    }

    // Compute the per-call project map. `target_id` is always projected for
    // page-bound steps; extract* ops merge extra state keys on top.
    let mut call_project: Map<String, Value> = project(capability)
        .as_object()
        .cloned()
        .unwrap_or_default();
    if let Some(p) = &project_override {
        if let Some(obj) = p.as_object() {
            for (k, v) in obj {
                call_project.insert(k.clone(), v.clone());
            }
        }
    }

    let mut now_has_page = has_page;
    let action = if op == "open" && with.contains_key("url") {
        // The page is opened by the chrome_cdp *capability*, not by the plugin:
        // the engine closes every tab a plugin opened before the next step.
        now_has_page = true;
        let mut p = Map::new();
        p.insert("target_id".into(), json!("/target_id"));
        if let Some(po) = project_override {
            if let Some(obj) = po.as_object() {
                for (k, v) in obj {
                    p.insert(k.clone(), v.clone());
                }
            }
        }
        Some(json!({
            "kind": "call", "capability": "chrome",
            "with": with,
            "project": p
        }))
    } else if op != "open" {
        Some(json!({
            "kind": "call", "capability": capability,
            "with": with,
            "project": call_project
        }))
    } else {
        // `the browser is ready` (op="open" with no url): a declaration, not
        // an action — the node carries no call.
        None
    };
    if op == "release" {
        now_has_page = false;
    }
    Ok((
        CompiledNode {
            instruction,
            action,
        },
        now_has_page,
    ))
}

// ── transpiler ─────────────────────────────────────────────────────────────

/// Repo root, derived from this source file's location (`src/bdd.rs`).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// The `chrome_binary` a transpiled scenario gets when its config pins none.
///
/// On unix this is `scripts/bdd/chrome-headless.sh`, which `exec`s a real
/// Chrome with `--headless=new` prepended — a headful window nobody is looking
/// at reports `visibilityState: "hidden"` and silently drops synthetic input,
/// so a scripted click would test nothing.
///
/// On Windows there is no POSIX wrapper (and the release tarball ships only
/// `laya-workflow.exe`), so the empty string is returned: the browser
/// capability treats that as "use `default_chrome_binary()`", which resolves
/// the platform Chrome (`chrome.exe`). The launch path adds the headless flags
/// itself on a display-less host, and a Windows box running these tests has a
/// desktop session, so a windowed Chrome is correct there.
fn default_chrome_wrapper() -> String {
    if cfg!(windows) {
        return String::new();
    }
    repo_root()
        .join("scripts")
        .join("bdd")
        .join("chrome-headless.sh")
        .to_string_lossy()
        .into_owned()
}

fn node(name: &str, instruction: &str, action: Option<Value>, keep: &[String], nxt: &str) -> Value {
    let mut n = json!({
        "name": name,
        "primary_q": "ok",
        "questions": {
            "ok": {
                "type": "choice",
                "instructions": instruction,
                "criteria": {"A": "yes", "B": "no"}
            }
        },
        // The offline heuristic answers a choice question with its first key,
        // so a healthy scenario walks A -> A -> ... -> done.
        "edge": {"condition": {"A": nxt}, "default": "STOP"},
        "state": {"keep": keep}
    });
    if let Some(a) = action {
        n["action"] = a;
    }
    n
}

fn render(feature: &Feature, scenario: &Scenario) -> String {
    let mut lines = vec![format!("Feature: {}", feature.name)];
    if !feature.description.is_empty() {
        lines.push(String::new());
        lines.push(feature.description.clone());
    }
    for s in &feature.background {
        let mut k = s.kind.clone();
        k[..1].make_ascii_uppercase();
        lines.push(format!("    {k} {}", s.text));
    }
    lines.push(String::new());
    lines.push(format!("  Scenario: {}", scenario.name));
    for s in &scenario.steps {
        let mut k = s.kind.clone();
        k[..1].make_ascii_uppercase();
        lines.push(format!("    {k} {}", s.text));
    }
    lines.join("\n")
}

fn default_policy() -> Value {
    json!({
        "allow_exec": true,
        "allow_hosts": ["127.0.0.1", "localhost"],
        "max_timeout_ms": 120000,
        "max_output": 1048576,
        "retries": 0
    })
}

fn default_chrome(chrome_wrapper: &str) -> Value {
    // `endpoint` / `profile_dir` are state-interpolated so the runner can hand
    // out a genuinely free port and a private profile dir per run; a hard-coded
    // port is a lie (another chrome may already own it). `chrome_binary` is
    // literal: it is spawned as a filename, so `${state.x}` would be a path.
    json!({
        "kind": "chrome_cdp",
        "endpoint": "http://127.0.0.1:${state.cdp_port}",
        "profile_dir": "${state.cdp_profile}",
        "launch": true,
        "startup_timeout_ms": 30000,
        "timeout_ms": 15000,
        "max_text": 8000,
        "max_owned_pages": 32,
        "owned_idle_ms": 300000,
        "chrome_binary": chrome_wrapper,
        // Human-shaped input is right for driving someone's browser and wrong
        // for a test: it makes every click a different gesture. Off by default;
        // a feature can turn it on with {"chrome": {"human": true}}.
        "human": false
    })
}

/// Optional `<feature>.config.json` sidecar (policy / chrome overrides and
/// Scenario Outline `initial_state` defaults).
///
/// Sidecars live in the json tree, mirrored from the feature source:
/// `bdd/workflows/<cat>/<name>.feature` -> `dsl/<cat>/<name>.config.json`. This
/// keeps `bdd/` a pure BDD-source tree (zero `.json` files) and `dsl/` the
/// single home of every JSON artifact. A config left next to the feature is
/// still honoured as a fallback, so a scratch feature can stay self-contained.
pub fn load_config(feature_path: &Path) -> Result<Value> {
    let side = config_sidecar_path(feature_path);
    if !side.is_file() {
        return Ok(json!({}));
    }
    let raw = std::fs::read_to_string(&side)
        .with_context(|| format!("reading {}", side.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing {}", side.display()))
}

/// Where a feature's `<name>.config.json` sidecar lives: the json-tree mirror
/// first (`dsl/<cat>/<name>.config.json`), falling back to the old sibling
/// location (`<feature>.config.json`) so a scratch feature can carry its own.
fn config_sidecar_path(feature_path: &Path) -> PathBuf {
    let root = repo_root();
    if let Ok(rel) = feature_path.strip_prefix(root.join("bdd").join("workflows")) {
        let mirror = root.join("dsl").join(rel).with_extension("config.json");
        if mirror.is_file() {
            return mirror;
        }
    }
    feature_path.with_extension("config.json")
}

fn merge(dst: &mut Value, src: &Value) {
    if let (Value::Object(d), Value::Object(s)) = (dst, src) {
        for (k, v) in s {
            d.insert(k.clone(), v.clone());
        }
    }
}

/// Compile one scenario into a full spec (public shape + `_bdd` metadata).

/// The tag names the toolchain reads a **payload** from: the tags that carry a
/// declaration in parentheses rather than a bare flag. Everything in this set
/// is a *contract* the compiler looks up by exact name, so a payload tag written
/// almost right is a declaration nothing reads and the build stays green (P169).
///
/// The set is pinned by a drift test (`bdd-payload-tags`) that scans `bdd.rs` for
/// the readers themselves — a literal `strip_prefix("<name>(")` and every
/// `tag_reason(scenario, <name>)` call, with `<name>` resolved through its
/// `const`/`let` binding — and requires each name it finds to be covered here. A
/// new payload reader therefore cannot be added without landing in this table.
pub const PAYLOAD_TAGS: [&str; 2] = ["outputs", "expected_failure"];

/// The bare-flag tags the toolchain reads by exact name whose near-miss is
/// *silently* inert - here, the profile selector. `bdd build --profile
/// production` (and `bdd run --profile production`) pick up only a scenario
/// tagged exactly `@production`, so a near-miss is skipped without a word: the
/// run goes green while the integration test never runs. Verified end to end - a
/// `@prodution` feature compiles locally at 100% coverage yet contributes zero
/// production-IT to the manifest.
///
/// `decision` is a bare-flag selector too (read by the same `has_tag`), but it is
/// deliberately *not* listed here: a `@decison` feature already fails loudly - its
/// decision steps are refused as browser steps - so the real case is covered, and
/// a near-miss guard would risk refusing ordinary words (`decide`, `decisions`).
/// `bdd-selector-tags` pins that exclusion by name.
pub const SELECTOR_TAGS: [&str; 1] = ["production"];

/// True when `a` and `b` are within two edits — a substitution, an insertion or a
/// deletion — of each other, which is what a keying slip looks like. Unlike a
/// similarity score this catches a transposition (`@outptus` scores only 0.54 by
/// dice, but is two substitutions away) while *not* refusing a name that is
/// merely similar (`@expected_thing` shares seven bigrams with
/// `expected_failure`, but is five edits away). A wrong refusal is worse than a
/// missed typo, so the test is spelled out as an edit count rather than tuned
/// with a threshold.
fn within_two_edits(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().abs_diff(b.len()) > 2 {
        return false;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()] <= 2
}

/// A plausible mis-spelling of one of [`PAYLOAD_TAGS`], or `None`.
fn near_payload_tag(name: &str) -> Option<&'static str> {
    PAYLOAD_TAGS
        .iter()
        .copied()
        .find(|k| *k != name && within_two_edits(name, k))
}

/// A plausible mis-spelling of one of [`SELECTOR_TAGS`], or `None`. Same
/// two-edit test as [`near_payload_tag`]: a wrong refusal is worse than a missed
/// typo, so a merely similar name is left alone.
fn near_selector_tag(name: &str) -> Option<&'static str> {
    SELECTOR_TAGS
        .iter()
        .copied()
        .find(|k| *k != name && within_two_edits(name, k))
}

/// Refuse a `@production` placed where its reader never looks. The profile
/// selector reads **Scenario** tags only - `bdd build --profile production` and
/// `bdd run --profile production` both filter on `scenario.tags` - and a
/// `@decision` feature is a decision-graph workflow that never runs as a browser
/// integration test at all. So a `@production` on the Feature line, or anywhere on
/// a `@decision` feature, selects nothing. Verified end to end: a Feature-level
/// `@production` compiles locally and contributes zero production-IT to a
/// production build that still exits 0.
pub fn selector_placement_problems(
    feature_tags: &[String],
    all_tags: &[String],
    decision: bool,
) -> Vec<String> {
    const PROD: &str = "production";
    if decision {
        for t in all_tags {
            if t == PROD || t.starts_with(&format!("{PROD}(")) {
                return vec![format!(
                    "@{t} selects nothing on a @decision feature: a decision workflow is a \
                     decision-graph spec, not a browser integration test, so `bdd build --profile \
                     production` / `bdd run --profile production` never reach it. Drop the tag."
                )];
            }
        }
        return Vec::new();
    }
    if has_tag(feature_tags, PROD) {
        return vec![
            "@production on the Feature line selects nothing: the production profile filters on \
             *Scenario* tags, so a Feature-level @production is silently inert and the \
             integration test never runs. Put @production on each Scenario you want run (see \
             bdd/workflows/examples/production_it.feature), or drop it."
                .to_string(),
        ];
    }
    Vec::new()
}

/// Refuse a payload tag written with nothing in it. The parser drops an empty
/// parenthetical, so `@outputs`, `@outputs()`, `@outputs( )` and
/// `@expected_failure` all arrive here as a bare name — and each of them used to
/// mean "declare nothing" while the build stayed green.
fn empty_payload_message(name: &str, tag: &str) -> String {
    match name {
        "outputs" => format!(
            "@{tag} declares no outputs - the @outputs(...) contract gate then has nothing to \
             check, so the promise the tag was there to make disappears without a word. Write \
             @outputs(<state key>, ...) with at least one state key (a bare @outputs, \
             @outputs() and @outputs(,) are all empty)."
        ),
        _ => format!(
            "@{tag} declares no reason. Without one, any non-zero exit counts - including a \
             typo'd Given, a Chrome that would not start, or a 404 from the fixture - so the \
             xfail would pass on a run that failed for the wrong reason. Write \
             @expected_failure(<text the failure must contain>)."
        ),
    }
}

/// Refuse a payload tag in a mode whose reader never runs: a `@decision` feature
/// compiles to a spec that `bdd run` never executes (and `bdd run` is the only
/// reader of an xfail), and only a browser Scenario turns `@outputs(...)` into a
/// checked contract. Such a tag is a declaration the toolchain silently drops,
/// in the same sense the empty ones above are.
fn decision_payload_message(name: &str, tag: &str) -> String {
    match name {
        "outputs" => format!(
            "@{tag} is not compiled for a @decision feature: only a browser Scenario turns \
             @outputs(...) into a checked contract, and a decision workflow's observable \
             result is its final label. Drop the tag, or declare the outputs on the browser \
             Scenario that produces the keys."
        ),
        _ => format!(
            "@{tag} is not compiled for a @decision feature: only `bdd run` (the browser \
             corpus) honours an xfail, and a decision feature compiles to a spec that runner \
             never executes. Drop the tag."
        ),
    }
}

/// Refuse a bare-flag selector written almost right. The toolchain reads it by
/// exact name; any other spelling selects nothing, so the document drops out of
/// the gate it meant to enter without a word.
fn selector_tag_message(target: &str, tag: &str) -> String {
    match target {
        "production" => format!(
            "@{tag} is not a tag the toolchain reads, so nothing selects on it - did you mean \
             @production? Only a scenario tagged exactly @production is picked up by \
             `bdd build --profile production` / `bdd run --profile production`, so a near-miss \
             is silently skipped and the run goes green while the integration test never runs."
        ),
        _ => format!(
            "@{tag} is not a tag the toolchain reads, so nothing selects on it - did you mean \
             @{target}?"
        ),
    }
}

/// Problems with a feature's *payload* tags — the tags that carry a declaration
/// in parentheses rather than a bare flag (P169, P170).
///
/// `@outputs(...)` is a contract: the compiler looks it up by the literal prefix
/// `outputs(` and turns it into the `_bdd.outputs` list the `@outputs` gate then
/// checks against what the workflow really produces. So a payload tag written
/// *almost* right is not a harmless unknown tag — it is a declaration nothing
/// reads, and the build stays green:
///
///   * a bare `@outputs` — which is also what `@outputs()` and `@outputs( )`
///     parse into, because the parser drops an empty parenthetical — and a list
///     with no items (`@outputs(,)`) both yield *zero* declared outputs, so the
///     gate has nothing to check and the promise disappears; for
///     `@expected_failure` the same shape means an xfail that passes on the
///     wrong reason;
///   * `@outptus(title)` / `@output(...)` are stored as ordinary tags, so
///     `bdd build` succeeds and the caller discovers at run time that the state
///     key it was promised is not there;
///   * in a `@decision` feature every `@outputs(...)` / `@expected_failure(...)`
///     is dropped on the floor: only a browser Scenario compiles the one, and
///     only `bdd run` reads the other.
///
/// `decision` says which mode the caller compiles in. Returned as a list so the
/// guard can be exercised on tag vectors directly (the same shape as
/// `decision_vocabulary_gaps`); the compilers refuse the first problem.
pub fn tag_shape_problems(tags: &[String], decision: bool) -> Vec<String> {
    let mut problems = Vec::new();
    for t in tags {
        let (name, payload) = match t.find('(') {
            Some(i) if t.ends_with(')') => (&t[..i], Some(&t[i + 1..t.len() - 1])),
            _ => (t.as_str(), None),
        };
        if let Some(k) = PAYLOAD_TAGS.iter().copied().find(|k| *k == name) {
            if decision {
                problems.push(decision_payload_message(k, t));
            } else if payload
                .map(|p| p.split(',').all(|x| x.trim().is_empty()))
                .unwrap_or(true)
            {
                problems.push(empty_payload_message(k, t));
            }
            continue;
        }
        if let Some(k) = near_payload_tag(name) {
            let what = match k {
                "outputs" => "the output contract",
                _ => "the xfail",
            };
            problems.push(format!(
                "@{t} is not a tag the compiler reads, so it declares nothing - did you mean \
                 @{k}(...)? Write @{k}(...) if you meant {what}, or drop the tag."
            ));
        } else if let Some(k) = near_selector_tag(name) {
            problems.push(selector_tag_message(k, t));
        }
    }
    problems
}

/// `@outputs(a, b)` on a Scenario declares the state keys the workflow is
/// *contractually* supposed to produce, so a caller (an agent invoking this as
/// a tool) knows what it can read back. Returns [] when the tag is absent.
pub fn scenario_outputs(scenario: &Scenario) -> Vec<String> {
    for t in &scenario.tags {
        if let Some(rest) = t.strip_prefix("outputs(") {
            if let Some(inner) = rest.strip_suffix(')') {
                return inner
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }
    }
    Vec::new()
}

/// State keys the compiled nodes actually *produce* (via a `project` map), plus
/// whatever `initial_state` seeds. This is the ground truth an `@outputs`
/// declaration is checked against: a declared output nobody produces is a lie
/// a caller would only discover at run time, so it is a compile error instead.
fn produced_state_keys(nodes: &[Value], initial_state: &Value) -> BTreeSet<String> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    if let Some(o) = initial_state.as_object() {
        for k in o.keys() {
            keys.insert(k.clone());
        }
    }
    for n in nodes {
        if let Some(proj) = n.get("action").and_then(|a| a.get("project")).and_then(|p| p.as_object()) {
            for k in proj.keys() {
                keys.insert(k.clone());
            }
        }
    }
    keys
}

pub fn compile_scenario(
    feature: &Feature,
    scenario: &Scenario,
    config: &Value,
    chrome_wrapper: &str,
) -> Result<Value> {
    // P169: a payload tag the reader cannot honour is a declaration that would be
    // silently dropped (`@outputs` with no keys, a near-miss name, or any
    // `@outputs(...)` in a @decision feature). Refuse it while it is in hand.
    let mut tags: Vec<String> = feature.tags.clone();
    tags.extend(scenario.tags.iter().cloned());
    if let Some(p) = tag_shape_problems(&tags, false).first() {
        bail!("{p}");
    }
    if let Some(p) = selector_placement_problems(&feature.tags, &tags, false).first() {
        bail!("{p}");
    }
    let merged: Vec<Step> = feature
        .background
        .iter()
        .chain(scenario.steps.iter())
        .cloned()
        .collect();

    let mut nodes: Vec<Value> = Vec::new();
    let mut keep: Vec<String> = Vec::new();
    // `has_page` tracks *now* (a release clears it); `had_page` tracks *ever*.
    // The browser handle is bound on `had_page`: a scenario that opens a page,
    // asserts on it, then releases it still needed the browser for that assert,
    // so `has_page` going false at the release must not strip the handle.
    let mut has_page = false;
    let mut had_page = false;

    for st in &merged {
        let compiled = compile_step(st, has_page)
            .with_context(|| format!("line {}: {}", st.line, st.text))?;
        has_page = compiled.1;
        had_page |= has_page;
        let idx = nodes.len();
        // Nodes are named by position so edge targets are predictable:
        // s0 -> s1 -> ... -> sN(done) -> STOP.
        let action = compiled.0.action.clone();
        nodes.push(node(
            &format!("s{idx}"),
            &compiled.0.instruction,
            action.clone(),
            &keep,
            &format!("s{}", idx + 1),
        ));
        // A state key must survive node to node if any later step reads it.
        if let Some(blob) = &action {
            let s = serde_json::to_string(blob).unwrap_or_default();
            for caps in placeholder_re().captures_iter(&s) {
                let key = caps[1].to_string();
                if !keep.contains(&key) {
                    keep.push(key);
                }
            }
            if blob.get("capability").and_then(Value::as_str) == Some("chrome") {
                if let Some(proj) = blob.get("project").and_then(Value::as_object) {
                    for key in proj.keys() {
                        if !keep.contains(key) {
                            keep.push(key.clone());
                        }
                    }
                }
            }
        }
    }

    // keep is only complete now, so stamp it onto every node.
    let keep_val: Value = json!(keep);
    for n in &mut nodes {
        n["state"]["keep"] = keep_val.clone();
    }

    let n = nodes.len();
    let done = node(
        &format!("s{n}"),
        &format!(
            "Scenario {:?} completed every step without a failed assertion?",
            scenario.name
        ),
        None,
        &keep,
        "STOP",
    );

    let mut policy = default_policy();
    merge(&mut policy, config.get("policy").unwrap_or(&json!({})));
    let mut chrome = default_chrome(chrome_wrapper);
    merge(&mut chrome, config.get("chrome").unwrap_or(&json!({})));

    let mut all = nodes;
    all.push(done);

    // A scenario that never opens a page (pure `I run the command` + state/exec
    // assertions) must not drag a browser along: the plugin host brings up
    // `browser` *before* any op, so even a page-free assert would launch (or
    // worse, fail to find) Chrome. Bind the browser when a page is opened at
    // *any* point (`had_page`) - a scenario that releases its page still used
    // the browser on the steps before the release.
    let mut capabilities = Map::new();
    capabilities.insert("chrome".into(), chrome);
    let mut bdd_cap = Map::new();
    bdd_cap.insert("kind".into(), json!("plugin"));
    bdd_cap.insert("plugin".into(), json!("bdd"));
    bdd_cap.insert("timeout_ms".into(), json!(60000));
    if had_page {
        bdd_cap.insert("browser".into(), json!("chrome"));
    }
    capabilities.insert("bdd".into(), Value::Object(bdd_cap));
    // `I run the command` steps call the engine's native exec capability. The
    // argv wraps the whole command in the platform shell — `/bin/sh -c` on
    // unix, `%COMSPEC% /c` (cmd.exe) on Windows — so a spec transpiled on the
    // machine that runs it works there; baking in `/bin/sh` made every exec
    // step fail on Windows with "not found". `cmd` arrives per-call via
    // `${with.cmd}`. Unused in browser-only scenarios, so always declared.
    let exec_argv = crate::capability::util::shell_argv("${with.cmd}");
    capabilities.insert(
        "exec".into(),
        json!({
            "kind": "exec",
            "argv": exec_argv,
            "timeout_ms": 120000,
            "max_output": 1048576
        }),
    );

    let mut spec = json!({
        "name": format!("bdd_{}_{}", feature.slug(), scenario.slug()),
        "dsl_version": 2,
        "description": format!(
            "Generated from {} by laya-workflow bdd. Do not edit: change the \
             .feature and recompile.\n\n{}",
            Path::new(&feature.path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| feature.path.clone()),
            render(feature, scenario)
        ),
        "start": "s0",
        "max_iterations": n + 2,
        "convergence_window": 3,
        "convergence_eps": 0.001,
        "policy": policy,
        "capabilities": Value::Object(capabilities),
        "nodes": all
    });
    // `@outputs(...)` contract gate: every declared output must be produced by
    // a step (a `project` key) or seeded in `initial_state`. Declaring an
    // output nobody produces turns a silent run-time surprise into a build
    // failure — the same accuracy-first stance as the step vocabulary.
    let declared = scenario_outputs(scenario);
    if !declared.is_empty() {
        let seeded = config
            .get("initial_state")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let produced = produced_state_keys(&all, &seeded);
        let missing: Vec<String> = declared
            .iter()
            .filter(|k| !produced.contains(*k))
            .cloned()
            .collect();
        if !missing.is_empty() {
            bail!(
                "Scenario {:?} declares @outputs({}) but nothing produces: {}. \
                 Add a step that extracts it (e.g. `I extract ... into <{}>`), \
                 or seed it in initial_state. A declared output nobody produces \
                 is a promise the run cannot keep.",
                scenario.name,
                declared.join(", "),
                missing.join(", "),
                missing[0]
            );
        }
    }

    let mut meta = Map::new();
    meta.insert("feature".into(), json!(feature.name));
    meta.insert("feature_path".into(), json!(rel_path(&feature.path)));
    meta.insert("scenario".into(), json!(scenario.name));
    meta.insert("tags".into(), json!(scenario.tags));
    let init = config.get("initial_state").cloned().unwrap_or_else(|| json!({}));
    meta.insert("initial_state".into(), init);
    meta.insert("outputs".into(), json!(declared));
    spec["_bdd"] = Value::Object(meta);
    Ok(spec)
}

// ── decision-workflow compiler (`@decision` features) ───────────────────────
//
// A `@decision` feature is one decision-graph workflow: each Scenario is one
// node, all nodes merge into a single spec. This is the layer that lets BDD
// express the agentic core (routing / gate / policy workflows) that a browser
// step chain cannot.
//
//   * Background  — workflow-level settings (`the workflow runs at most N
//                   iterations`).
//   * Scenario    — one node. Given defines questions, When defines edge
//                   routing, Then defines the action (ordered threshold rules
//                   or copy_keys) and node metadata (min_confidence /
//                   max_retries / state.keep).

/// Extract every `"..."` token from a step fragment (heuristic needle list).
fn quoted_tokens(text: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#""([^"]+)""#).unwrap());
    re.captures_iter(text).map(|c| c[1].to_string()).collect()
}

/// P173: a `policy` integer the runtime will not keep as written. Two ways the
/// engine quietly overrides an author's value: it clamps it to a hard ceiling at
/// load (`capability::POLICY_MAX_*`), and it reads the key with `as_u64()`, so a
/// negative number is not read at all and the default applies. Both mean the
/// budget that runs is not the one written; refuse them where the raw cell is
/// still in hand, naming the ceiling the clamp actually uses.
fn policy_int_error(key: &str, n: i64, line: usize) -> Option<String> {
    let (ceil, what) = match key {
        "max_timeout_ms" => (
            crate::capability::POLICY_MAX_TIMEOUT_MS as i64,
            "the hard ceiling for any capability timeout",
        ),
        "max_output" => (
            crate::capability::POLICY_MAX_OUTPUT as i64,
            "the hard ceiling for captured output bytes",
        ),
        "retries" => (
            crate::capability::POLICY_MAX_RETRIES as i64,
            "the hard ceiling for transport retries",
        ),
        _ => return None,
    };
    if n < 0 {
        return Some(format!(
            "line {line}: policy key {key:?} is {n} — the engine reads it with `as_u64()`, so a negative value is not read at all and its default applies; write 0 or more, up to {ceil} ({what})"
        ));
    }
    if n > ceil {
        return Some(format!(
            "line {line}: policy key {key:?} is {n}, but the runtime clamps it to {ceil} ({what}) — a value above the ceiling is silently reduced at load, so the budget that runs is not the one written"
        ));
    }
    None
}

/// The part of a policy list cell that the quoted-token splitter *drops*:
/// everything outside a `"…"` span, with `\"` / `\\` honoured inside a span
/// exactly as [`quoted_tokens_escaped`] reads it. A caller refuses the row when
/// this still holds anything but commas and whitespace, because the dropped
/// text is a value the author wrote that the engine will never see (P168).
fn policy_list_residual(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' {
            out.push(c);
            continue;
        }
        while let Some(c2) = chars.next() {
            match c2 {
                '\\' => {
                    if matches!(chars.peek(), Some('"') | Some('\\')) {
                        chars.next();
                    }
                }
                '"' => break,
                _ => {}
            }
        }
    }
    out
}

/// Like `quoted_tokens` but honours `\"` escapes inside the quoted spans, so a
/// regex pattern that contains a literal double-quote (e.g. `['"]`) can be
/// written as `['\"]` inside the outer `"..."` delimiters. Backslashes that do
/// not precede a quote or another backslash are preserved verbatim, so `\d`,
/// `\s`, `\.` etc. survive round-trips.
fn quoted_tokens_escaped(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut tok = String::new();
        let mut closed = false;
        while let Some((_, c2)) = chars.next() {
            match c2 {
                '\\' => match chars.peek() {
                    Some(&(_, '"')) => {
                        tok.push('"');
                        chars.next();
                    }
                    Some(&(_, '\\')) => {
                        tok.push('\\');
                        chars.next();
                    }
                    _ => tok.push('\\'),
                },
                '"' => {
                    closed = true;
                    break;
                }
                _ => tok.push(c2),
            }
        }
        if closed {
            out.push(tok);
        }
    }
    out
}

fn compile_decision_node(feature: &Feature, scenario: &Scenario) -> Result<Value> {
    let name = scenario.slug();
    let mut questions: Map<String, Value> = Map::new();
    let mut primary_q: Option<String> = None;
    let mut first_q_line: Option<usize> = None;
    let mut edge_condition: Map<String, Value> = Map::new();
    let mut edge_default: Option<String> = None;
    let mut rules: Vec<Value> = Vec::new();
    let mut copy_keys: Vec<String> = Vec::new();
    let mut keep: Vec<String> = Vec::new();
    let mut rename: Map<String, Value> = Map::new();
    let mut min_confidence: Option<f64> = None;
    let mut max_retries: Option<usize> = None;
    let mut last_question: Option<String> = None;
    let mut explicit_primary: Option<String> = None;
    let mut action_q: Option<String> = None;
    let mut call_action: Option<(String, Map<String, Value>, Map<String, Value>)> = None;
    let mut call_chain: Vec<Value> = Vec::new();
    let mut gate_action: Option<(String, String, f64, f64)> = None;
    let mut merge_action: Option<String> = None;
    let mut combine_action: Option<Map<String, Value>> = None;
    let mut rubric_action: Option<Map<String, Value>> = None;
    let mut ui_proposal_action: Option<Map<String, Value>> = None;
    let mut sub_workflow: Option<String> = None;
    let mut sub_continue: Option<String> = None;
    // P158: every action-carrying `Then` step seen in this Scenario, in source
    // order, as `(kind, line)`. Used below to refuse a Scenario that declares
    // more than one action (see the note at the assembly).
    let mut action_steps: Vec<(String, usize)> = Vec::new();
    // P162: what this node declares and the line it was declared on, so a second
    // declaration of a one-per-node thing is refused instead of silently winning.
    let mut declared: Vec<(String, usize)> = Vec::new();

    for st in &scenario.steps {
        let (caps, tag) = dec_match(&st.kind, &st.text)
            .with_context(|| format!("line {}: {}", st.line, st.text))?;
        // P158: which action does this step declare, if any? `threshold` is the
        // only kind that legitimately spans several steps (one rule per step);
        // `copy_keys` accumulates; the rest hold a single action per node.
        let action_kind = match tag {
            "gate" => Some("gate"),
            "merge_open_probs" => Some("merge_open_probs"),
            "combine" => Some("combine"),
            "rubric" => Some("rubric"),
            "ui_proposal" => Some("ui_proposal"),
            "copy_keys" => Some("copy_keys"),
            "call" => Some("call"),
            "label_all" | "label_eq" | "label_eq_in" | "label_eq_implicit" | "label_opt_gte"
            | "label_opt_lt" | "label_opt_gte_implicit" | "label_gte" | "label_lt"
            | "label_always" => Some("threshold"),
            _ => None,
        };
        if let Some(k) = action_kind {
            action_steps.push((k.to_string(), st.line));
        }
        match tag {
            "choice" | "score" | "noul" => {
                let q = caps["q"].to_string();
                let text = caps["text"].to_string();
                let mut qv = Map::new();
                qv.insert("type".into(), json!(tag));
                qv.insert("instructions".into(), json!(text));
                match tag {
                    "choice" => {
                        // Gherkin step-table convention: the first row is the
                        // column header (`| answer | meaning |`), the rest are
                        // data rows.
                        let mut criteria = Map::new();
                        let mut seen: Vec<(String, usize)> = Vec::new();
                        for (i, row) in st.table.iter().enumerate().skip(1) {
                            if row.len() < 2 || row[0].is_empty() {
                                bail!(
                                    "line {}: choice question {q:?} table rows need | answer | meaning |",
                                    st.line
                                );
                            }
                            table_key_once(
                                &mut seen,
                                row[0].as_str(),
                                row_line(st, i),
                                &format!("choice question {q:?} table"),
                            )?;
                            criteria.insert(row[0].clone(), json!(row[1]));
                        }
                        if criteria.is_empty() {
                            bail!(
                                "line {}: choice question {q:?} needs at least one option row",
                                st.line
                            );
                        }
                        qv.insert("criteria".into(), Value::Object(criteria));
                    }
                    "score" => {
                        // First row is the `| level |` header.
                        let mut levels: Vec<Value> = Vec::new();
                        for row in st.table.iter().skip(1) {
                            if !row.is_empty() && !row[0].is_empty() {
                                levels.push(json!(row[0]));
                            }
                        }
                        if levels.is_empty() {
                            bail!(
                                "line {}: score question {q:?} needs at least one level row",
                                st.line
                            );
                        }
                        qv.insert("criteria".into(), Value::Array(levels));
                    }
                    "noul" => {
                        // Optional | key | text | rows carry the yes/no
                        // semantics annotations (`truthy` / `falsy`) that the
                        // source DSL stores on noul questions.
                        let mut seen: Vec<(String, usize)> = Vec::new();
                        for (i, row) in st.table.iter().enumerate() {
                            if row.len() < 2 || row[0].trim().is_empty() {
                                bail!(
                                    "line {}: noul question {q:?} table rows need | truthy | text | / | falsy | text |",
                                    st.line
                                );
                            }
                            let key = row[0].trim();
                            table_key_once(
                                &mut seen,
                                key,
                                row_line(st, i),
                                &format!("noul question {q:?} table"),
                            )?;
                            let val = row[1].trim();
                            match key {
                                "truthy" | "falsy" => {
                                    qv.insert(key.to_string(), json!(val));
                                }
                                _ => bail!(
                                    "line {}: noul question {q:?} table key {key:?} must be `truthy` or `falsy`",
                                    st.line
                                ),
                            }
                        }
                    }
                    _ => {} // unreachable: tag is choice|score|noul here
                }
                if primary_q.is_none() {
                    primary_q = Some(q.clone());
                    first_q_line = Some(st.line);
                }
                last_question = Some(q.clone());
                step_once(&mut declared, &format!("the question {q:?}"), st.line)?;
                questions.insert(q, Value::Object(qv));
            }
            // ── heuristics attach to the most recent question in this Scenario ──
            "heur_any" | "heur_field_any" => {
                let lq = last_question.clone().ok_or_else(|| {
                    anyhow!("line {}: heuristic step before any question", st.line)
                })?;
                let needles = quoted_tokens(&caps["needles"]);
                if needles.is_empty() {
                    bail!("line {}: heuristic needs at least one needle", st.line);
                }
                let hit: f64 = caps["hit"].to_string().parse()?;
                let q = questions
                    .get_mut(&lq)
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| anyhow!("line {}: question {lq:?} not found", st.line))?;
                let mut h = Map::new();
                if let Some(f) = caps.name("field") {
                    h.insert("field".into(), json!(f.as_str()));
                }
                h.insert("match_any".into(), json!(needles));
                h.insert("p_hit".into(), json!(hit));
                // explicit `and miss M` overrides the default 1-hit; 1 - 0.95
                // is 0.050000000000000044 in f64, so round the default.
                let miss: f64 = match caps.name("miss") {
                    Some(m) => m.as_str().parse()?,
                    None => ((1.0 - hit) * 1_000_000.0).round() / 1_000_000.0,
                };
                h.insert("p_miss".into(), json!(miss));
                set_heuristic(&mut declared, q, &lq, st.line, Value::Object(h))?;
            }
            "heur_numeric" => {
                let lq = last_question.clone().ok_or_else(|| {
                    anyhow!("line {}: heuristic step before any question", st.line)
                })?;
                let field = caps["field"].to_string();
                let q = questions
                    .get_mut(&lq)
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| anyhow!("line {}: question {lq:?} not found", st.line))?;
                let mut h = Map::new();
                let miss: f64 = match caps.name("miss") {
                    Some(m) => m.as_str().parse()?,
                    None => 0.0,
                };
                h.insert("extract_numeric".into(), json!(field));
                h.insert("score_miss".into(), json!(miss));
                set_heuristic(&mut declared, q, &lq, st.line, Value::Object(h))?;
            }
            "heur_mid" => {
                let lq = last_question.clone().ok_or_else(|| {
                    anyhow!("line {}: heuristic step before any question", st.line)
                })?;
                let q = questions
                    .get_mut(&lq)
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| anyhow!("line {}: question {lq:?} not found", st.line))?;
                let needles = quoted_tokens(&caps["needles"]);
                let mid: f64 = caps["mid"].to_string().parse()?;
                let h = q.entry("heuristic".to_string()).or_insert_with(|| json!({}));
                let ho = h.as_object_mut().ok_or_else(|| {
                    anyhow!("line {}: heuristic not an object", st.line)
                })?;
                ho.insert("match_mid_any".into(), json!(needles));
                ho.insert("p_mid".into(), json!(mid));
            }
            "heur_regex" | "heur_regex_field" => {
                let lq = last_question.clone().ok_or_else(|| {
                    anyhow!("line {}: heuristic step before any question", st.line)
                })?;
                let patterns = quoted_tokens_escaped(&caps["patterns"]);
                if patterns.is_empty() {
                    bail!("line {}: regex heuristic needs at least one pattern", st.line);
                }
                let hit: f64 = caps["hit"].to_string().parse()?;
                let q = questions
                    .get_mut(&lq)
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| anyhow!("line {}: question {lq:?} not found", st.line))?;
                let mut h = Map::new();
                if let Some(f) = caps.name("field") {
                    h.insert("field".into(), json!(f.as_str()));
                }
                h.insert("match_regex".into(), json!(patterns));
                h.insert("p_hit".into(), json!(hit));
                // explicit `and miss M` overrides the default 1-hit, rounded to
                // avoid 0.050000000000000044 in f64.
                let miss: f64 = match caps.name("miss") {
                    Some(m) => m.as_str().parse()?,
                    None => ((1.0 - hit) * 1_000_000.0).round() / 1_000_000.0,
                };
                h.insert("p_miss".into(), json!(miss));
                set_heuristic(&mut declared, q, &lq, st.line, Value::Object(h))?;
            }
            "heur_match_rules" => {
                let lq = last_question.clone().ok_or_else(|| {
                    anyhow!("line {}: heuristic step before any question", st.line)
                })?;
                let field = caps["field"].to_string();
                let hit: f64 = caps["hit"].to_string().parse()?;
                let fallback = caps.name("fallback").map(|m| m.as_str().to_string());
                let mut rules: Map<String, Value> = Map::new();
                // First row is the `| label | needles |` header; skip it.
                let mut seen: Vec<(String, usize)> = Vec::new();
                for (i, row) in st.table.iter().enumerate().skip(1) {
                    if row.len() < 2 || row[0].is_empty() {
                        bail!(
                            "line {}: match_rules step needs | label | needles | rows",
                            st.line
                        );
                    }
                    table_key_once(
                        &mut seen,
                        row[0].trim(),
                        row_line(st, i),
                        "match_rules step",
                    )?;
                    let needles = quoted_tokens_escaped(&row[1]);
                    if needles.is_empty() {
                        bail!("line {}: match_rules row {row:?} needs quoted needles", st.line);
                    }
                    rules.insert(row[0].trim().to_string(), json!(needles));
                }
                if rules.is_empty() {
                    bail!("line {}: match_rules needs at least one | label | needles | row", st.line);
                }
                let q = questions
                    .get_mut(&lq)
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| anyhow!("line {}: question {lq:?} not found", st.line))?;
                let mut h = Map::new();
                h.insert("field".into(), json!(field));
                h.insert("match_rules".into(), Value::Object(rules));
                h.insert("p_hit".into(), json!(hit));
                if let Some(fb) = fallback {
                    h.insert("fallback".into(), json!(fb));
                }
                set_heuristic(&mut declared, q, &lq, st.line, Value::Object(h))?;
            }
            "heur_score_bands" => {
                let lq = last_question.clone().ok_or_else(|| {
                    anyhow!("line {}: heuristic step before any question", st.line)
                })?;
                let field = caps["field"].to_string();
                let hit: f64 = caps["hit"].to_string().parse()?;
                let midv: f64 = caps["midv"].to_string().parse()?;
                let miss: f64 = caps["miss"].to_string().parse()?;
                let q = questions
                    .get_mut(&lq)
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| anyhow!("line {}: question {lq:?} not found", st.line))?;
                let mut h = Map::new();
                h.insert("field".into(), json!(field));
                h.insert("match_any".into(), json!([caps["high"].to_string()]));
                h.insert("match_mid_any".into(), json!([caps["mid"].to_string()]));
                h.insert("score_hit".into(), json!(hit));
                h.insert("score_mid".into(), json!(midv));
                h.insert("score_miss".into(), json!(miss));
                set_heuristic(&mut declared, q, &lq, st.line, Value::Object(h))?;
            }
            "heur_answer" => {
                let lq = last_question.clone().ok_or_else(|| {
                    anyhow!("line {}: heuristic step before any question", st.line)
                })?;
                let q = questions
                    .get_mut(&lq)
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| anyhow!("line {}: question {lq:?} not found", st.line))?;
                set_heuristic(
                    &mut declared,
                    q,
                    &lq,
                    st.line,
                    json!({"answer": caps["a"].to_string()}),
                )?;
            }
            "heur_answer_field" => {
                let lq = last_question.clone().ok_or_else(|| {
                    anyhow!("line {}: heuristic step before any question", st.line)
                })?;
                let q = questions
                    .get_mut(&lq)
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| anyhow!("line {}: question {lq:?} not found", st.line))?;
                set_heuristic(
                    &mut declared,
                    q,
                    &lq,
                    st.line,
                    json!({"answer_field": caps["f"].to_string()}),
                )?;
            }
            "gate" => {
                let q = caps["q"].to_string();
                let opt = caps["opt"].to_string();
                let block: f64 = caps["block"].to_string().parse()?;
                let allow: f64 = caps["allow"].to_string().parse()?;
                gate_action = Some((q, opt, block, allow));
            }
            "merge_open_probs" => {
                merge_action = Some(caps["q"].to_string());
            }
            "combine" => {
                let min: f64 = caps["min"].to_string().parse()?;
                let high: f64 = caps["high"].to_string().parse()?;
                let low: f64 = caps["low"].to_string().parse()?;
                let mut m = Map::new();
                m.insert("choice".into(), json!(caps["choice"].to_string()));
                m.insert("signal".into(), json!(caps["signal"].to_string()));
                m.insert("fallback".into(), json!(caps["fallback"].to_string()));
                m.insert("review_label".into(), json!(caps["review"].to_string()));
                m.insert("min_confidence".into(), json!(min));
                m.insert("high_threshold".into(), json!(high));
                m.insert("low_threshold".into(), json!(low));
                combine_action = Some(m);
            }
            "rubric" => {
                let min: f64 = caps["min"].to_string().parse()?;
                let l1: usize = caps["l1"].to_string().parse()?;
                let l2: usize = caps["l2"].to_string().parse()?;
                let w1: f64 = caps["w1"].to_string().parse()?;
                let w2: f64 = caps["w2"].to_string().parse()?;
                let mut m = Map::new();
                m.insert("min_confidence".into(), json!(min));
                m.insert(
                    "dimensions".into(),
                    json!([
                        {"question": caps["q1"].to_string(), "levels": l1, "weight": w1},
                        {"question": caps["q2"].to_string(), "levels": l2, "weight": w2},
                    ]),
                );
                rubric_action = Some(m);
            }
            "ui_proposal" => {
                let min: f64 = caps["min"].to_string().parse()?;
                let mut m = Map::new();
                m.insert("operation".into(), json!(caps["op"].to_string()));
                m.insert("field".into(), json!(caps["field"].to_string()));
                m.insert("amount".into(), json!(caps["amt"].to_string()));
                m.insert("surface".into(), json!(caps["surface"].to_string()));
                m.insert("snapshot_hash".into(), json!(caps["hash"].to_string()));
                m.insert("min_confidence".into(), json!(min));
                if let Some(vk) = caps.name("value_key") {
                    m.insert("value_key".into(), json!(vk.as_str()));
                }
                ui_proposal_action = Some(m);
            }
            "edge" => {
                let a = caps["a"].to_string();
                let node = caps["node"].to_string();
                step_once(&mut declared, &format!("the route for answer {a:?}"), st.line)?;
                edge_condition.insert(a, json!(node));
            }
            "edge_default" => {
                step_once(
                    &mut declared,
                    "a catch-all route (`any other answer goes to …`)",
                    st.line,
                )?;
                edge_default = Some(caps["node"].to_string());
            }
            "label_all" => {
                // `caps["rest"]` spans only up to the matched ` and `; recover the
                // whole conjunction body from the full match instead, splitting at
                // the quoted label (`" when `), so every conjunct is kept.
                let whole = caps.get(0).map(|m| m.as_str()).unwrap_or("");
                let rest = whole
                    .split_once("\" when ")
                    .map(|(_, r)| r.to_string())
                    .unwrap_or_else(|| whole.to_string());
                let leaves = parse_conjunction_leaves(&rest)
                    .map_err(|e| anyhow!("line {}: {e:#}", st.line))?;
                if leaves.len() < 2 {
                    bail!(
                        "line {}: a conjunctive `label` rule needs at least two conditions joined by ` and `",
                        st.line
                    );
                }
                rules.push(json!({
                    "when": {"all": leaves},
                    "label": caps["label"].to_string()
                }));
            }
            "label_eq" => {
                rules.push(json!({
                    "when": {"question": caps["q"].to_string(), "answer": caps["a"].to_string()},
                    "label": caps["label"].to_string()
                }));
            }
            "label_eq_in" => {
                rules.push(json!({
                    "when": {"question": caps["q"].to_string(), "answer_in": quoted_tokens(&caps["list"])},
                    "label": caps["label"].to_string()
                }));
            }
            "label_eq_implicit" => {
                rules.push(json!({
                    "when": {"answer": caps["a"].to_string()},
                    "label": caps["label"].to_string()
                }));
            }
            "label_opt_gte" => {
                let c: f64 = caps["c"].to_string().parse()?;
                rules.push(json!({
                    "when": {"question": caps["q"].to_string(), "option": caps["a"].to_string(), "gte": c},
                    "label": caps["label"].to_string()
                }));
            }
            "label_opt_lt" => {
                let c: f64 = caps["c"].to_string().parse()?;
                rules.push(json!({
                    "when": {"question": caps["q"].to_string(), "option": caps["a"].to_string(), "lt": c},
                    "label": caps["label"].to_string()
                }));
            }
            "label_opt_gte_implicit" => {
                let c: f64 = caps["c"].to_string().parse()?;
                rules.push(json!({
                    "when": {"option": caps["a"].to_string(), "gte": c},
                    "label": caps["label"].to_string()
                }));
            }
            "label_gte" => {
                let n: f64 = caps["n"].to_string().parse()?;
                rules.push(json!({
                    "when": {"question": caps["q"].to_string(), "value_gte": n},
                    "label": caps["label"].to_string()
                }));
            }
            "label_lt" => {
                let n: f64 = caps["n"].to_string().parse()?;
                rules.push(json!({
                    "when": {"question": caps["q"].to_string(), "value_lt": n},
                    "label": caps["label"].to_string()
                }));
            }
            "label_always" => {
                rules.push(json!({
                    "when": {"always": true},
                    "label": caps["label"].to_string()
                }));
            }
            "copy_keys" => copy_keys.push(caps["q"].to_string()),
            "keep_state" => keep.push(caps["k"].to_string()),
            "rename_state" => {
                let out = caps["out"].to_string();
                step_once(&mut declared, &format!("the state rename for {out:?}"), st.line)?;
                rename.insert(out, json!(caps["in"].to_string()));
            }
            "min_conf" => {
                step_once(&mut declared, "`require confidence at least N`", st.line)?;
                min_confidence = Some(caps["n"].to_string().parse()?);
            }
            "max_retries" => {
                step_once(&mut declared, "`allow up to N retries`", st.line)?;
                max_retries = Some(caps["n"].to_string().parse()?);
            }
            "sub_workflow" => {
                step_once(&mut declared, "the sub workflow reference", st.line)?;
                sub_workflow = Some(caps["w"].to_string());
            }
            "sub_workflow_continue" => {
                step_once(&mut declared, "the sub workflow continue target", st.line)?;
                sub_continue = Some(caps["w"].to_string());
            }
            "primary_q" => {
                step_once(&mut declared, "the primary question", st.line)?;
                explicit_primary = Some(caps["q"].to_string());
            }
            "action_q" => {
                step_once(&mut declared, "the action question", st.line)?;
                action_q = Some(caps["q"].to_string());
            }
            "call" => {
                let cap = caps["cap"].to_string();
                let mut with: Map<String, Value> = Map::new();
                let mut project: Map<String, Value> = Map::new();
                // `chain.<as>.<capability|with.<k>|project.<k>>` rows build the
                // nested sub-call list in encounter order. Each distinct `<as>`
                // becomes one `{capability, as, with?, project?}` entry.
                let mut chain: Vec<(String, String, Map<String, Value>, Map<String, Value>)> = Vec::new();
                let chain_idx = |chain: &Vec<(String, String, Map<String, Value>, Map<String, Value>)>, as_name: &str| {
                    chain.iter().position(|(a, _, _, _)| a == as_name)
                };
                let mut seen: Vec<(String, usize)> = Vec::new();
                for (i, row) in st.table.iter().enumerate() {
                    if row.len() < 2 || row[0].is_empty() {
                        bail!(
                            "line {}: call step table rows need | with.<key> | value |, | project.<key> | ptr | or | chain.<as>.<key> | value |",
                            st.line
                        );
                    }
                    table_key_once(&mut seen, row[0].as_str(), row_line(st, i), "call step table")?;
                    let cell = row[1].trim();
                    let val: Value = serde_json::from_str(cell).unwrap_or_else(|_| json!(cell));
                    if let Some(rest) = row[0].strip_prefix("chain.") {
                        let (as_name, sub) = rest
                            .split_once('.')
                            .ok_or_else(|| anyhow!(
                                "line {}: chain row {rest:?} must be chain.<as>.<capability|with.<k>|project.<k>>",
                                st.line
                            ))?;
                        let as_name = as_name.to_string();
                        let idx = chain_idx(&chain, &as_name);
                        let idx = match idx {
                            Some(i) => i,
                            None => {
                                chain.push((as_name.clone(), String::new(), Map::new(), Map::new()));
                                chain.len() - 1
                            }
                        };
                        if sub == "capability" {
                            let capv: String = if let Ok(v2) = serde_json::from_str::<serde_json::Value>(cell) {
                                v2.as_str().unwrap_or(cell).to_string()
                            } else {
                                cell.to_string()
                            };
                            chain[idx].1 = capv;
                        } else if let Some(k) = sub.strip_prefix("with.") {
                            chain[idx].2.insert(k.to_string(), val);
                        } else if let Some(k) = sub.strip_prefix("project.") {
                            chain[idx].3.insert(k.to_string(), val);
                        } else {
                            bail!(
                                "line {}: chain row {sub:?} must be capability / with.<k> / project.<k>",
                                st.line
                            );
                        }
                        continue;
                    }
                    let (kind, key) = match row[0].split_once('.') {
                        Some((k, rest)) => (k, rest.to_string()),
                        None => ("with", row[0].clone()),
                    };
                    match kind {
                        "with" => { with.insert(key, val); }
                        "project" => { project.insert(key, val); }
                        other => bail!(
                            "line {}: call table row {other:?} must start with `with.`, `project.` or `chain.`",
                            st.line
                        ),
                    }
                }
                for (as_name, capv, cw, cp) in chain {
                    if capv.is_empty() {
                        bail!("line {}: chain entry {as_name:?} needs a `chain.{as_name}.capability` row", st.line);
                    }
                    let mut e = Map::new();
                    e.insert("capability".into(), json!(capv));
                    e.insert("as".into(), json!(as_name));
                    if !cw.is_empty() { e.insert("with".into(), Value::Object(cw)); }
                    if !cp.is_empty() { e.insert("project".into(), Value::Object(cp)); }
                    call_chain.push(Value::Object(e));
                }
                guard_call_with(feature, &cap, &with, &call_chain, st.line)?;
                guard_call_project(&project, &call_chain, st.line)?;
                call_action = Some((cap, with, project));
            }
            other => bail!("line {}: unhandled decision tag {other:?}", st.line),
        }
    }

    // P136: a node whose edge routes on a question must name its primary when it
    // declares more than one question. `Edge::resolve` matches the *primary* answer
    // against the condition keys, and with several questions and no explicit
    // `the primary question is "<q>"` the compiler silently takes the
    // first-declared one — so reordering the questions (or a hand edit) would
    // silently change the routed question, with nothing failing. Make the routed
    // question explicit in the source instead of inferred from declaration order.
    if !edge_condition.is_empty() && questions.len() >= 2 && explicit_primary.is_none() {
        let declared: Vec<&str> = questions.keys().map(String::as_str).collect();
        bail!(
            "line {}: Scenario {:?} routes on a question (edge condition keys {:?}) but declares {} questions and no primary — add `Then the primary question is \"{}\"` (the routed question is otherwise inferred from declaration order; declared: {:?})",
            first_q_line.unwrap_or(scenario.line),
            scenario.name,
            edge_condition.keys().collect::<Vec<_>>(),
            questions.len(),
            primary_q.clone().unwrap_or_default(),
            declared,
        );
    }

    // A sub-workflow reference node composes by reference: the whole node is
    // `{"name": ..., "workflow": <ref>}` with no questions / edge / action of
    // its own (the sub-graph owns all of those). Any other decision content in
    // the same Scenario would be silently dropped, so refuse it instead.
    if let Some(wf) = sub_workflow {
        if !questions.is_empty()
            || !edge_condition.is_empty()
            || edge_default.is_some()
            || !rules.is_empty()
            || !copy_keys.is_empty()
            || !keep.is_empty()
            || min_confidence.is_some()
            || max_retries.is_some()
            || call_action.is_some()
            || gate_action.is_some()
            || merge_action.is_some()
            || combine_action.is_some()
            || rubric_action.is_some()
            || ui_proposal_action.is_some()
            || primary_q.is_some()
            || last_question.is_some()
        {
            bail!(
                "line {}: Scenario {:?} mixes `the sub workflow` with node content (questions/edges/actions) - a workflow-reference node carries only the reference",
                scenario.line,
                scenario.name
            );
        }
        let mut node = json!({
            "name": name,
            "workflow": wf,
        });
        if let Some(ct) = sub_continue {
            // completion handoff: when the sub-workflow terminates, continue at
            // the caller's node. Emitted as the reference node's own edge so the
            // engine's ref-expansion can redirect the sub's internal STOPs here.
            node["edge"] = json!({"condition": {}, "default": ct});
        }
        return Ok(node);
    }

    // A decision node may have zero questions (a pure pass-through/stop node
    // that only carries an edge), so no hard requirement here. Most nodes carry
    // at least one question.
    // explicit `the primary question is "Q"` overrides the first-question default;
    // a threshold action can additionally read a different question via
    // `the action reads the question "Q"`.
    let primary_q = explicit_primary
        .clone()
        .or(primary_q)
        .unwrap_or_else(|| "ok".to_string());

    // P158: a Scenario carries at most one action.
    //
    // Every action-carrying `Then` step compiles to a different action kind, and
    // the assembly below is a priority chain (`if !rules.is_empty() { … } else if …`),
    // so a second action used to be dropped in silence: `bdd build` on a feature
    // with both a `Then label …` and a `Then call the capability …` exited 0 with
    // the `call` simply absent from the emitted spec. Refuse the mix by name, and
    // refuse a repeated singleton action (a second `gate` / `call` / … overwrites
    // the first) rather than losing it. `threshold`, `copy_keys` accumulate, so
    // several `label …` / `keep the answer to …` steps stay legal.
    let mut seen_kinds: Vec<(&str, usize)> = Vec::new();
    let mut repeated: Option<(&str, usize, usize)> = None;
    for (k, line) in &action_steps {
        match seen_kinds.iter().find(|(s, _)| s == k) {
            None => seen_kinds.push((k.as_str(), *line)),
            Some((_, first)) => {
                let singleton = matches!(
                    k.as_str(),
                    "gate" | "merge_open_probs" | "combine" | "rubric" | "ui_proposal" | "call"
                );
                if singleton && repeated.is_none() {
                    repeated = Some((k.as_str(), *first, *line));
                }
            }
        }
    }
    if seen_kinds.len() > 1 {
        bail!(
            "line {}: this scenario declares {} actions ({}) - a decision node carries at most one action, and the engine assembles them in a fixed priority order, so all but the first would be silently dropped; keep one action-carrying `Then` step and move the others to their own scenario",
            seen_kinds[1].1,
            seen_kinds.len(),
            seen_kinds
                .iter()
                .map(|(k, l)| format!("{k} (line {l})"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if let Some((k, first, second)) = repeated {
        bail!(
            "line {second}: this scenario declares a second `{k}` action (the first is at line {first}) - a decision node keeps one `{k}` action, so the earlier one would be silently dropped; keep one `{k}` step and move the other to its own scenario"
        );
    }

    let action = if !rules.is_empty() {
        Some(json!({
            "kind": "threshold",
            "question": action_q.clone().unwrap_or_else(|| primary_q.clone()),
            "rules": rules
        }))
    } else if let Some((q, opt, block, allow)) = gate_action {
        Some(json!({
            "kind": "gate",
            "question": q,
            "option": opt,
            "block_at": block,
            "allow_at": allow,
        }))
    } else if let Some(q) = merge_action {
        Some(json!({
            "kind": "merge_open_probs",
            "question": q,
        }))
    } else if let Some(mut m) = combine_action {
        m.insert("kind".into(), json!("combine"));
        Some(Value::Object(m))
    } else if let Some(mut m) = rubric_action {
        m.insert("kind".into(), json!("rubric"));
        Some(Value::Object(m))
    } else if let Some(mut m) = ui_proposal_action {
        m.insert("kind".into(), json!("ui_proposal"));
        Some(Value::Object(m))
    } else if !copy_keys.is_empty() {
        Some(json!({
            "kind": "copy_keys",
            "keys": copy_keys
        }))
    } else if let Some((cap, with, project)) = call_action {
        let mut a = Map::new();
        a.insert("kind".into(), json!("call"));
        a.insert("capability".into(), json!(cap));
        if !call_chain.is_empty() {
            a.insert("chain".into(), Value::Array(call_chain));
        }
        if !with.is_empty() {
            a.insert("with".into(), Value::Object(with));
        }
        if !project.is_empty() {
            a.insert("project".into(), Value::Object(project));
        }
        Some(Value::Object(a))
    } else {
        None
    };

    let edge = json!({
        "condition": edge_condition,
        "default": edge_default.unwrap_or_else(|| "STOP".to_string()),
    });

    let mut n = json!({
        "name": name,
        "primary_q": primary_q,
        "questions": questions,
        "edge": edge,
        "min_confidence": min_confidence.unwrap_or(0.0),
    });
    if let Some(mr) = max_retries {
        n["max_retries"] = json!(mr);
    }
    if let Some(a) = action {
        n["action"] = a;
    }
    if !keep.is_empty() || !rename.is_empty() {
        let mut st = Map::new();
        if !keep.is_empty() {
            st.insert("keep".into(), json!(keep));
        }
        if !rename.is_empty() {
            st.insert("rename".into(), Value::Object(rename));
        }
        n["state"] = Value::Object(st);
    }
    Ok(n)
}

/// The source line of row `i` of a step table. A `Step` built by hand (a test)
/// has no `row_lines`, so fall back to the step's own line (P161).
fn row_line(st: &Step, i: usize) -> usize {
    st.row_lines.get(i).copied().unwrap_or(st.line)
}

/// Refuse a step table that names the same row key twice.
///
/// Every decision-vocabulary table that carries `| key | value |` rows compiles
/// into a single JSON object (a choice question's `criteria`, a `call` step's
/// `with`/`project`/`chain`, the `policy`, the `capabilities`, a `match_rules`
/// heuristic), so a repeated key silently overwrites the row above it: the
/// declaration the author wrote first is thrown away and `bdd build` still
/// succeeds. That is P128's "a declared option nobody reads" one level up, in
/// the authoring layer — so refuse it, naming both lines. `seen` is per step (a
/// table is one map), and `key` is the raw row key exactly as the compiler reads
/// it (P161).
fn table_key_once(seen: &mut Vec<(String, usize)>, key: &str, line: usize, what: &str) -> Result<()> {
    if let Some((_, first)) = seen.iter().find(|(k, _)| k.as_str() == key) {
        bail!(
            "line {line}: this {what} declares {key:?} twice (the first is at line {first}) - the rows compile into one JSON object, so the earlier row would be silently dropped; keep one row per key"
        );
    }
    seen.push((key.to_string(), line));
    Ok(())
}

/// The step-layer twin of [`table_key_once`] (P162): a `Given` / `When` / `Then`
/// step that sets something the compiler keeps only one of.
///
/// A second `When the answer "a" goes to …`, a second `any other answer goes to
/// …`, a second heuristic on the same question, a second `require confidence at
/// least N` / `allow up to N retries` / `the primary question is "q"` / `map the
/// state key "out" from "in"` - each overwrites the one before it, so the step the
/// author wrote first is thrown away while `bdd build` still exits 0. That is
/// P161's silent loss one layer up: there the dropped declaration was a row of one
/// table, here it is a whole step (the node's routing, or a node-wide setting), so
/// the behaviour of the node quietly changes. `what` names the declaration, and
/// both lines are reported.
///
/// The `keep the …` steps are the deliberate exception: their target is a *list*
/// (`state.keep`), where a repeat is kept rather than dropped. (`state.rename` is a
/// map keyed by the output key instead, so a repeated target *is* refused, and the
/// Background's `Given the workflow runs at most N iterations` / `Given the policy`
/// / `Given the capabilities` are guarded by the same helper one function over.)
fn step_once(declared: &mut Vec<(String, usize)>, what: &str, line: usize) -> Result<()> {
    if let Some((_, first)) = declared.iter().find(|(k, _)| k == what) {
        bail!(
            "line {line}: this node declares {what} twice (the first is at line {first}) - the later step overwrites the earlier one, so the one you wrote first would be silently dropped; keep one per node"
        );
    }
    declared.push((what.to_string(), line));
    Ok(())
}


/// The capability definitions a feature declares (`Given the capabilities`),
/// keyed by name and left exactly as written (P164). The compiler folds each
/// `kind` the same way the engine does before it looks a read set up.
fn declared_capabilities(feature: &Feature) -> Map<String, Value> {
    let mut out: Map<String, Value> = Map::new();
    for st in &feature.background {
        let Ok((_, tag)) = dec_match(&st.kind, &st.text) else {
            continue;
        };
        if tag != "capabilities" {
            continue;
        }
        for row in &st.table {
            if row.len() < 2 || row[0].trim().is_empty() {
                continue;
            }
            if let Ok(def) = serde_json::from_str::<Value>(row[1].trim()) {
                if def.get("kind").and_then(Value::as_str).is_some() {
                    out.insert(row[0].trim().to_string(), def);
                }
            }
        }
    }
    out
}

/// Refuse a `call` step whose `with` table the engine can only silently drop:
/// a key no call of the kind reads (P164), a value of the wrong type (P166), or
/// a number outside the read site's band (P175).
///
/// The engine reads a fixed set of keys out of `with` and ignores the rest, so a
/// mistyped key (`with.modes` for `mode`, or a key copied from another kind) is a
/// silent no-op; a value of the wrong JSON type leaves the read site on its
/// default; and a number outside the band the read site clamps to runs as the
/// clamped value, not the one written. None is visible to `validate` because the
/// key is just data in a JSON object - so the decision lives once, in
/// [`crate::capability::with_keys::with_step_issue`], shared with the `spec.rs`
/// guard so a hand-written JSON spec and a `@decision` feature are held to the
/// same contract (and cannot drift apart).
fn guard_call_with(
    feature: &Feature,
    cap: &str,
    with: &Map<String, Value>,
    chain: &[Value],
    line: usize,
) -> Result<()> {
    let caps = declared_capabilities(feature);
    if let Some(issue) = crate::capability::with_keys::with_step_issue(&caps, cap, with, chain) {
        bail!(
            "line {line}: {}",
            crate::capability::with_keys::with_issue_body(&issue)
        );
    }
    Ok(())
}

/// Refuse a `call` step's `project` row the engine can only silently discard (P165).
///
/// `project.<state key> | <pointer>` is applied at run time by reading the
/// pointer with `as_str()` and resolving it against the call result, so a
/// pointer of any other JSON type (`| project.ok | true |`, `| project.count |
/// 3 |`) is skipped and `state.<key>` is **never written** - and because the
/// compiler counts the row as a producer, an `@outputs` naming it stays green
/// while the run cannot keep the promise. A projection key that is empty writes
/// a state key the `${state.<key>}` grammar (one character or more) can never
/// name, and a `chain` row keyed by an empty alias files the sub-call's result
/// under a name no `chain.<alias>.*` row can reach. None of the three can do
/// what it says, so each is refused rather than shipped.
///
/// The decision lives once, in
/// [`crate::capability::call_project::project_step_issue`], shared with the
/// `spec.rs` guard so a hand-written JSON spec and a `@decision` feature are
/// held to the same contract (and cannot drift apart) - P165 on the BDD path,
/// P178 on the spec path.
fn guard_call_project(project: &Map<String, Value>, chain: &[Value], line: usize) -> Result<()> {
    if let Some(issue) = crate::capability::call_project::project_step_issue(project, chain) {
        bail!(
            "line {line}: {}",
            crate::capability::call_project::project_issue_body(&issue)
        );
    }
    Ok(())
}
/// Attach a heuristic to question `lq`, refusing a *second* heuristic on the same
/// question (P162). A question carries one `heuristic` object, so a second
/// `Given answer …` / `score …` step replaces the first and the question silently
/// loses the rules the author wrote first. Heuristics on *different* questions are
/// the normal shape (one per question), which is why the key is the question id.
fn set_heuristic(
    declared: &mut Vec<(String, usize)>,
    q: &mut Map<String, Value>,
    lq: &str,
    line: usize,
    h: Value,
) -> Result<()> {
    step_once(declared, &format!("a heuristic on question {lq:?}"), line)?;
    q.insert("heuristic".into(), h);
    Ok(())
}

/// Compile a `@decision` feature into a single decision-graph workflow spec.
/// Compile a `@decision` feature into a single decision-graph workflow spec.
pub fn compile_decision(feature: &Feature, config: &Value) -> Result<Value> {
    // P169: every `@outputs(...)` in a @decision feature is dropped on the floor -
    // refuse it here rather than let the author believe a caller contract exists.
    let mut tags: Vec<String> = feature.tags.clone();
    for sc in &feature.scenarios {
        tags.extend(sc.tags.iter().cloned());
    }
    if let Some(p) = tag_shape_problems(&tags, true).first() {
        bail!("{p}");
    }
    if let Some(p) = selector_placement_problems(&feature.tags, &tags, true).first() {
        bail!("{p}");
    }
    let mut nodes: Vec<Value> = Vec::new();
    let mut max_iterations: Option<usize> = None;

    let mut policy: Option<Value> = None;
    let mut capabilities: Option<Value> = None;
    // P163: the Background's own one-per-feature steps. `Given the workflow runs
    // at most N iterations` / `Given the policy` / `Given the capabilities` each
    // set a single field of the spec, so a second one silently replaces the first
    // - the same loss `step_once` refuses inside a Scenario.
    let mut declared: Vec<(String, usize)> = Vec::new();
    for st in &feature.background {
        let (caps, tag) = dec_match(&st.kind, &st.text)
            .with_context(|| format!("line {}: {}", st.line, st.text))?;
        match tag {
            "max_iterations" => {
                step_once(&mut declared, "`the workflow runs at most N iterations`", st.line)?;
                max_iterations = Some(caps["n"].to_string().parse()?);
            }
            "policy" => {
                step_once(&mut declared, "`Given the policy`", st.line)?;
                if st.table.is_empty() {
                    bail!(
                        "line {}: `Given the policy` needs a | key | value | table ",
                        st.line
                    );
                }
                let mut p: Map<String, Value> = Map::new();
                let mut seen: Vec<(String, usize)> = Vec::new();
                for (i, row) in st.table.iter().enumerate() {
                    if row.len() < 2 || row[0].trim().is_empty() {
                        bail!(
                            "line {}: policy table rows need | key | value |",
                            st.line
                        );
                    }
                    let key = row[0].trim();
                    table_key_once(&mut seen, key, row_line(st, i), "policy table")?;
                    let raw = row[1].trim();
                    let val: Value = match key {
                        // P168: the engine keeps only the exact spellings `true`
                        // / `false` (any other word — `yes`, `True`, `1` — is
                        // silently read as *false*, which is the opposite of what
                        // a `yes` author meant), and reads the list keys as the
                        // *quoted* spans of the cell (bare text is dropped, and an
                        // emptied list means *no* restriction). Both are the same
                        // "a declared value the engine throws away" class as
                        // P152/P154/P164; refuse them here, where the raw text is
                        // still in hand.
                        "allow_exec" => match raw {
                            "true" => json!(true),
                            "false" => json!(false),
                            other => bail!(
                                "line {}: policy key \"allow_exec\" must be exactly `true` or `false`, got {other:?} — the engine reads only that spelling, so any other word is silently taken as false (not the default), and your policy differs from what you wrote",
                                st.line
                            ),
                        },
                        "allow_hosts" | "allow_paths" => {
                            if let Some(bad) = policy_list_residual(raw)
                                .split(',')
                                .map(str::trim)
                                .find(|t| !t.is_empty())
                            {
                                bail!(
                                    "line {}: policy key {key:?} takes a comma-separated list of quoted strings, but {bad:?} is not inside quotes — the engine keeps only the quoted spans, so {bad:?} is silently dropped, and an emptied list means *no* restriction at all",
                                    st.line
                                );
                            }
                            json!(quoted_tokens_escaped(raw))
                        }
                        "max_timeout_ms" | "max_output" | "retries" => {
                            let n = raw.parse::<i64>().map_err(|e| {
                                anyhow!(
                                    "line {}: policy key {key:?} expects an integer, got {raw:?}: {e}",
                                    st.line
                                )
                            })?;
                            // P173: the integer is well-typed here, but the
                            // runtime still clamps it to a hard ceiling at load
                            // (and never reads a negative one), so a written value
                            // can be one the engine quietly ignores. Refuse it in
                            // the raw text, naming the same ceiling constant the
                            // clamp reads.
                            if let Some(p) = policy_int_error(key, n, st.line) {
                                bail!("{p}");
                            }
                            json!(n)
                        }
                        _ => json!(raw),
                    };
                    p.insert(key.to_string(), val);
                }
                policy = Some(Value::Object(p));
            }
            "capabilities" => {
                step_once(&mut declared, "`Given the capabilities`", st.line)?;
                if st.table.is_empty() {
                    bail!(
                        "line {}: `Given the capabilities` needs a | name | config | table ",
                        st.line
                    );
                }
                let mut caps: Map<String, Value> = Map::new();
                let mut seen: Vec<(String, usize)> = Vec::new();
                for (i, row) in st.table.iter().enumerate() {
                    if row.len() < 2 || row[0].trim().is_empty() {
                        bail!(
                            "line {}: capabilities table rows need | name | config |",
                            st.line
                        );
                    }
                    let name = row[0].trim();
                    table_key_once(&mut seen, name, row_line(st, i), "capabilities table")?;
                    let raw = row[1].trim();
                    let val: Value = serde_json::from_str(raw).map_err(|e| {
                        anyhow!(
                            "line {}: capabilities {name:?} expects a JSON config object, got {raw:?}: {e}",
                            st.line
                        )
                    })?;
                    caps.insert(name.to_string(), val);
                }
                if caps.is_empty() {
                    bail!("line {}: capabilities table is empty", st.line);
                }
                capabilities = Some(Value::Object(caps));
            }
            other => bail!(
                "line {}: {other:?} is not a workflow-level setting; the Background of a @decision feature only accepts `Given the workflow runs at most N iterations` / `Given the policy` / `Given the capabilities`",
                st.line
            ),
        }
    }

    // Each Scenario is one node, so its name is the node id. Catch duplicates
    // HERE (where the Scenario's own line is known) rather than only later in
    // `spec::validate`, so the author gets `line N:` in `bdd build` /
    // `bdd_validate` instead of a bare "duplicate node name" (P89 usability axis).
    let mut seen_nodes: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for scenario in &feature.scenarios {
        let node = compile_decision_node(feature, scenario)?;
        let node_name = node["name"].as_str().unwrap_or("").to_string();
        if !seen_nodes.insert(node_name.clone()) {
            bail!(
                "line {}: duplicate node name {node_name:?} — every Scenario is one node, so scenario names must be unique",
                scenario.line
            );
        }
        nodes.push(node);
    }
    if nodes.is_empty() {
        bail!("feature {:?} has no Scenario (a @decision feature needs at least one node)", feature.name);
    }
    let start = nodes[0]["name"].as_str().unwrap_or("s0").to_string();
    let n_nodes = nodes.len();

    let mut spec = json!({
        "name": feature.slug(),
        "dsl_version": 2,
        "description": if feature.description.is_empty() {
            format!("BDD decision workflow: {}", feature.name)
        } else {
            feature.description.clone()
        },
        "start": start,
        "max_iterations": max_iterations.unwrap_or(10 + n_nodes * 2),
        "convergence_window": 5,
        "convergence_eps": 0.0001,
        "nodes": nodes,
    });
    // Policy is emitted verbatim (Gherkin background step merged over an
    // optional config-sidecar policy), *without* default-filling: the source
    // DSL stores exactly the keys the author wrote, so default-filling would
    // break the round-trip. Only emit the block when the author pinned one
    // down at all.
    if config.get("policy").is_some() || policy.is_some() {
        let mut merged_policy = config.get("policy").cloned().unwrap_or_else(|| json!({}));
        if let Some(bp) = &policy {
            merge(&mut merged_policy, bp);
        }
        spec["policy"] = merged_policy;
    }
    if let Some(c) = capabilities {
        spec["capabilities"] = c;
    } else if let Some(c) = config.get("capabilities") {
        spec["capabilities"] = c.clone();
    }
    spec["_bdd"] = json!({
        "feature": feature.name,
        "feature_path": rel_path(&feature.path),
        "mode": "decision",
    });
    Ok(spec)
}

/// Path relative to the repo root (matching Python's `os.path.relpath(..., ROOT)`).
fn rel_path(p: &str) -> String {
    let root = repo_root();
    let abs = if Path::new(p).is_absolute() {
        PathBuf::from(p)
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    abs.strip_prefix(&root)
        .map(|r| r.to_string_lossy().into_owned())
        .unwrap_or_else(|_| p.to_string())
}

/// State keys the spec interpolates but does not produce itself. Scans the
/// whole spec, not just the nodes: `endpoint` and `profile_dir` interpolate too.
pub fn required_state(spec: &Value) -> Vec<String> {
    let mut produced: BTreeSet<String> = BTreeSet::new();
    if let Some(nodes) = spec.get("nodes").and_then(Value::as_array) {
        for n in nodes {
            if let Some(act) = n.get("action") {
                if let Some(proj) = act.get("project").and_then(Value::as_object) {
                    produced.extend(proj.keys().cloned());
                }
            }
        }
    }
    let mut blob = String::new();
    if let Some(nodes) = spec.get("nodes") {
        blob.push_str(&serde_json::to_string(nodes).unwrap_or_default());
    }
    if let Some(caps) = spec.get("capabilities") {
        blob.push_str(&serde_json::to_string(caps).unwrap_or_default());
    }
    let mut need: BTreeSet<String> = BTreeSet::new();
    for caps in placeholder_re().captures_iter(&blob) {
        let k = caps[1].to_string();
        if !produced.contains(&k) {
            need.insert(k);
        }
    }
    need.into_iter().collect()
}

/// Strip the `_`-prefixed metadata keys (they are build bookkeeping, not spec).
pub fn public_spec(spec: &Value) -> Value {
    let mut out = Map::new();
    if let Value::Object(o) = spec {
        for (k, v) in o {
            if !k.starts_with('_') {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(out)
}

// ── coverage ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
pub struct Coverage {
    pub feature: String,
    pub total: usize,
    pub matched: usize,
    pub coverage: f64,
    pub unmatched: Vec<String>,
}

/// Count vocabulary-matched vs total steps (background included), using the
/// same matcher the compiler uses — so "covered" means "the deterministic
/// compiler knows this step", which is the accuracy-first bar.
pub fn feature_coverage(feature: &Feature) -> Coverage {
    let steps = feature
        .background
        .iter()
        .chain(
            feature
                .scenarios
                .iter()
                .flat_map(|s| s.steps.iter()),
        );
    let mut matched = 0usize;
    let mut total = 0usize;
    let mut unmatched = Vec::new();
    for st in steps {
        total += 1;
        if match_step(&st.kind, &st.text).is_ok() {
            matched += 1;
        } else {
            unmatched.push(format!("{} {}", st.kind, st.text));
        }
    }
    Coverage {
        feature: rel_path(&feature.path),
        total,
        matched,
        coverage: if total == 0 {
            100.0
        } else {
            (matched as f64 * 100.0 / total as f64 * 10.0).round() / 10.0
        },
        unmatched,
    }
}

// ── build gate (port of laya-workflow bdd build) ─────────────────────────────

pub struct BuildOptions {
    pub features: Vec<String>,
    pub out: String,
    pub profile: String,
    pub filter: Option<String>,
    pub all: bool,
    pub coverage_min: f64,
    pub no_validate: bool,
    pub base_url: Option<String>,
    pub emit: Option<String>,
    pub assist: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            features: Vec::new(),
            out: repo_root()
                .join("target")
                .join("bdd-build")
                .to_string_lossy()
                .into_owned(),
            profile: "local".into(),
            filter: None,
            all: false,
            coverage_min: 100.0,
            no_validate: false,
            base_url: None,
            emit: Some("all".into()),
            assist: false,
        }
    }
}

fn has_tag(tags: &[String], name: &str) -> bool {
    tags.iter()
        .any(|t| t == name || t.starts_with(&format!("{name}(")))
}

/// Validate a generated spec against the engine's structural gate.
fn validate_spec(spec_path: &str) -> Result<(), String> {
    match crate::spec::load_file(spec_path) {
        Ok(_) => Ok(()),
        Err(e) => {
            let s = format!("{e:#}");
            let tail: String = s.chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect();
            Err(tail)
        }
    }
}

/// Build every artifact a BDD document drives, with accuracy as the hard gate
/// (port of `laya-workflow bdd build main()`).
pub fn build(o: &BuildOptions) -> Result<i32> {
    let root = repo_root();
    let feature_dir = root.join("bdd").join("features");
    let prod_tag = "production";

    // Feature selection: explicit args, or all of bdd/features/*.feature
    // (non-recursive on purpose — bdd/features/setup/ holds step lists).
    let mut paths: Vec<PathBuf> = Vec::new();
    if o.features.is_empty() {
        if feature_dir.is_dir() {
            let mut names: Vec<String> = Vec::new();
            for e in std::fs::read_dir(&feature_dir)? {
                let e = e?;
                let name = e.file_name().to_string_lossy().into_owned();
                if name.ends_with(".feature") && e.path().is_file() {
                    names.push(name);
                }
            }
            names.sort();
            for n in names {
                paths.push(feature_dir.join(n));
            }
        }
    } else {
        for f in &o.features {
            paths.push(PathBuf::from(f));
        }
    }
    if paths.is_empty() {
        eprintln!("build: no feature files selected");
        return Ok(1);
    }

    let profile = o.profile.as_str();
    let base_url = if profile == "production" {
        o.base_url.clone().or_else(|| std::env::var("BDD_BASE_URL").ok())
    } else {
        None
    };
    if profile == "production" && base_url.is_none() {
        eprintln!(
            "build: --profile production needs --base-url (or $BDD_BASE_URL) — \
             production integration tests run against a real target"
        );
        return Ok(1);
    }

    let out_dir = Path::new(&o.out);
    let spec_dir = out_dir.join("spec");
    std::fs::create_dir_all(&spec_dir)?;

    // Decision specs may reference the scratch dirs (${env.LAYA_STORE_DIR} &
    // friends) in `policy.allow_paths`. Guarantee them the same way `dsl smoke`
    // does, then re-snapshot the secrets store so `validate` can resolve the
    // references in CI, where no such vars exist. Same treatment as dsl_smoke.rs.
    if let Ok(env_map) = crate::dsl_smoke::ensure_env_dirs() {
        for (k, v) in env_map {
            std::env::set_var(k, v);
        }
        let _ = crate::capability::secret::init(None);
    }

    let mut manifest: Vec<Value> = Vec::new();
    let mut coverage_report: Vec<Coverage> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut assist_calls: Vec<(String, String)> = Vec::new();

    let chrome_wrapper = default_chrome_wrapper();
    for path in &paths {
        let feature = match load(path) {
            Ok(f) => f,
            Err(e) => {
                errors.push(format!("{}: {e:#}", rel_path(&path.to_string_lossy())));
                continue;
            }
        };
        let cov = feature_coverage(&feature);
        coverage_report.push(cov);

        let config = match load_config(path) {
            Ok(c) => c,
            Err(e) => {
                errors.push(format!("{}: {e:#}", rel_path(&path.to_string_lossy())));
                continue;
            }
        };
        let chrome_bin = config.get("chrome").and_then(|c| c.get("chrome_binary")).and_then(Value::as_str).unwrap_or(&chrome_wrapper);

        // `@decision` features compile through the decision-graph path: the whole
        // feature is ONE workflow (each Scenario = one node). Browser steps do
        // not exist here, so coverage/validate run on the decision compiler's
        // output instead of the per-scenario chain.
        if has_tag(&feature.tags, "decision") {
            let spec = match compile_decision(&feature, &config) {
                Ok(s) => s,
                Err(e) => {
                    errors.push(format!(
                        "{}: {e:#}",
                        rel_path(&path.to_string_lossy())
                    ));
                    continue;
                }
            };
            let spec_name = spec["name"].as_str().unwrap_or("").to_string();
            let spec_file = format!("{spec_name}.json");
            let spec_path = spec_dir.join(&spec_file);
            let public = public_spec(&spec);
            std::fs::write(
                &spec_path,
                format!("{}
", serde_json::to_string_pretty(&public)?),
            )?;
            let (validated, vmsg) = if o.no_validate {
                (true, String::new())
            } else {
                match validate_spec(&spec_path.to_string_lossy()) {
                    Ok(()) => (true, String::new()),
                    Err(msg) => (false, msg),
                }
            };
            if !validated {
                // P35: pin the .feature line the semantic error refers to, so
                // the author fixes the right step instead of hunting.
                let located = match locate_semantic_error_line(&feature, &vmsg) {
                    Some(l) => format!("line {l}: {vmsg}"),
                    None => vmsg,
                };
                errors.push(format!(
                    "{}: `validate` rejected the generated decision spec: {located}",
                    rel_path(&path.to_string_lossy())
                ));
            }
            manifest.push(json!({
                "feature": rel_path(&path.to_string_lossy()),
                "scenario": feature.name,
                "spec": format!("spec/{spec_file}"),
                "tags": feature.tags,
                "profile": profile,
                "base_url": base_url,
                "nodes": spec["nodes"].as_array().map(|a| a.len()).unwrap_or(0),
                "validated": validated,
                "mode": "decision",
            }));
            continue;
        }

        // P172: a selector tag on the Feature line selects nothing - the profile
        // filter reads *Scenario* tags, so the misplaced `@production` would be
        // skipped before `compile_scenario` could refuse it, and a production
        // build would ship one fewer integration test while still reporting OK.
        // Refuse it here, for both profiles, so the placement cannot hide behind
        // the very filter that makes it inert.
        if let Some(p) = selector_placement_problems(&feature.tags, &feature.tags, false).first() {
            errors.push(format!("{}: {p}", rel_path(&path.to_string_lossy())));
            continue;
        }

        for scenario in &feature.scenarios {
            let tags = &scenario.tags;
            if profile == "production" && !o.all && !has_tag(tags, prod_tag) {
                continue;
            }
            if let Some(f) = &o.filter {
                let fname = f.strip_prefix('@').unwrap_or(f);
                if !has_tag(tags, fname) {
                    continue;
                }
            }
            let spec = match compile_scenario(&feature, scenario, &config, chrome_bin) {
                Ok(s) => s,
                Err(e) => {
                    errors.push(format!(
                        "{} :: {}: {e:#}",
                        rel_path(&path.to_string_lossy()),
                        scenario.name
                    ));
                    continue;
                }
            };
            let spec_name = spec["name"].as_str().unwrap_or("").to_string();
            let spec_file = format!("{spec_name}.json");
            let spec_path = spec_dir.join(&spec_file);
            let public = public_spec(&spec);
            std::fs::write(
                &spec_path,
                format!("{}\n", serde_json::to_string_pretty(&public)?),
            )?;

            let (validated, vmsg) = if o.no_validate {
                (true, String::new())
            } else {
                match validate_spec(&spec_path.to_string_lossy()) {
                    Ok(()) => (true, String::new()),
                    Err(msg) => (false, msg),
                }
            };
            if !validated {
                errors.push(format!(
                    "{} :: {}: `validate` rejected the generated spec: {vmsg}",
                    rel_path(&path.to_string_lossy()),
                    scenario.name
                ));
            }
            manifest.push(json!({
                "feature": rel_path(&path.to_string_lossy()),
                "scenario": scenario.name,
                "spec": format!("spec/{spec_file}"),
                "tags": tags,
                "profile": profile,
                "base_url": base_url,
                "nodes": spec["nodes"].as_array().map(|a| a.len()).unwrap_or(0),
                "validated": validated,
            }));
        }
    }

    // Per-feature coverage gate (strict = 100% by default).
    for cov in &coverage_report {
        if cov.coverage < o.coverage_min {
            // For an @decision feature, point at the closest decision-vocabulary
            // step so the author can fix the typo without digging (P28). Each
            // unmatched entry is "<kind> <text>".
            let mut annotated: Vec<String> = Vec::new();
            for u in &cov.unmatched {
                if let Some((kind, text)) = u.split_once(' ') {
                    let mut entry = u.clone();
                    if let Some(t) = decision_suggest(kind, text) {
                        entry = format!("{u} (did you mean the decision {kind} step: {t}?)");
                    }
                    annotated.push(entry);
                } else {
                    annotated.push(u.clone());
                }
            }
            errors.push(format!(
                "{}: step coverage {}% < {}% (unmatched: {})",
                cov.feature,
                cov.coverage,
                o.coverage_min,
                annotated.join(", ").chars().take(400).collect::<String>()
            ));
        }
    }

    // Needle assist for out-of-vocabulary steps — suggestions only.
    if o.assist {
        for cov in &coverage_report {
            for step in &cov.unmatched {
                assist_calls.push((cov.feature.clone(), step.clone()));
            }
        }
        for (feature, step) in &assist_calls {
            match needle_suggest(step) {
                None => eprintln!("  assist: {feature}: {step:?} -> (needle unavailable)"),
                Some(s) => {
                    let conf = s.get("confidence").and_then(Value::as_f64).unwrap_or(0.0);
                    let label = if conf >= ACCEPT_FLOOR { "LIKELY" } else { "guess" };
                    let mut core = Map::new();
                    for k in ["op", "assertion", "value", "expected"] {
                        if let Some(v) = s.get(k) {
                            if !v.is_null() {
                                core.insert(k.into(), v.clone());
                            }
                        }
                    }
                    println!(
                        "  assist [{label} conf={conf:.2}] {feature}: {step:?}"
                    );
                    println!(
                        "        -> {}  (suggestion only — add a rule to the vocabulary to compile it)",
                        serde_json::to_string(&Value::Object(core)).unwrap_or_default()
                    );
                }
            }
        }
    }

    // Emit the manifests.
    let emit = o.emit.as_deref().unwrap_or("all");
    if matches!(emit, "all" | "manifest") {
        let m = json!({"profile": profile, "base_url": base_url, "scenarios": manifest});
        std::fs::write(out_dir.join("manifest.json"), serde_json::to_string_pretty(&m)?)?;
    }
    if matches!(emit, "all" | "coverage") {
        let c: Vec<Value> = coverage_report
            .iter()
            .map(|cov| json!({
                "feature": cov.feature,
                "total": cov.total,
                "matched": cov.matched,
                "coverage": cov.coverage,
                "unmatched": cov.unmatched
            }))
            .collect();
        std::fs::write(out_dir.join("coverage.json"), serde_json::to_string_pretty(&c)?)?;
    }
    if profile == "production" {
        let m = json!({"profile": "production", "base_url": base_url, "scenarios": manifest});
        std::fs::write(out_dir.join("it.manifest.json"), serde_json::to_string_pretty(&m)?)?;
    }

    // An empty selection is an error, not a success.
    if manifest.is_empty() && errors.is_empty() {
        if profile == "production" && !o.all {
            errors.push(format!(
                "no @{prod_tag} scenarios selected — tag some `Scenario:` blocks with \
                 @{prod_tag}, or pass --all to override"
            ));
        } else if let Some(f) = &o.filter {
            errors.push(format!("--filter {f}: no scenario carries that tag"));
        } else if !o.features.is_empty() {
            errors.push("the selected feature file(s) contain no scenario".to_string());
        } else {
            errors.push("no scenario was selected at all".to_string());
        }
    }

    // Report.
    let n_spec = manifest.len();
    let n_val = manifest
        .iter()
        .filter(|m| m.get("validated").and_then(Value::as_bool).unwrap_or(false))
        .count();
    let n_prod = manifest
        .iter()
        .filter(|m| m.get("profile").and_then(Value::as_str) == Some("production"))
        .count();
    for cov in &coverage_report {
        let tag = if cov.coverage >= o.coverage_min { "ok" } else { "LOW" };
        let mut line = format!(
            "  coverage {:5.1}%  {}/{}  {:<3}  {}",
            cov.coverage, cov.matched, cov.total, tag, cov.feature
        );
        if !cov.unmatched.is_empty() {
            line.push_str(&format!(
                "   unmatched: {}",
                cov.unmatched.join("; ").chars().take(120).collect::<String>()
            ));
        }
        println!("{line}");
    }
    println!(
        "  profile {profile}  base_url {}",
        base_url.clone().unwrap_or_else(|| "(local fixtures)".into())
    );
    if profile == "production" {
        println!("  specs {n_spec}  validated {n_val}  production-IT {n_prod}  assist-suggestions {}", assist_calls.len());
    } else {
        println!("  specs {n_spec}  validated {n_val}  assist-suggestions {}", assist_calls.len());
    }
    if !errors.is_empty() {
        eprintln!("\nerrors:");
        for e in &errors {
            eprintln!("  {e:#}");
        }
        return Ok(1);
    }
    println!("  OK — every scenario compiled, every spec validated, coverage at the bar");
    Ok(0)
}

// ── transpile CLI (port of laya-workflow bdd transpile main) ─────────────────

pub fn transpile_cli(
    features: &[String],
    out: Option<&str>,
    list: bool,
    check: bool,
    chrome_bin: Option<&str>,
) -> Result<i32> {
    let default_wrapper = default_chrome_wrapper();
    let chrome_wrapper = chrome_bin.unwrap_or(&default_wrapper);
    let mut total = 0usize;
    for fp in features {
        let path = Path::new(fp);
        let feature = match load(path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("error: {fp}: {e:#}");
                return Ok(1);
            }
        };
        let config = match load_config(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: {fp}: {e:#}");
                return Ok(1);
            }
        };
        let cb = config
            .get("chrome")
            .and_then(|c| c.get("chrome_binary"))
            .and_then(Value::as_str)
            .unwrap_or(chrome_wrapper);
        let base = Path::new(fp)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| fp.clone());
        // An `@decision` feature is one whole decision graph (each Scenario one
        // node), so it transpiles to a single spec — the exact file an author
        // drops at dsl/<cat>/<name>.json. This closes the greenfield loop: write
        // the .feature, transpile to the dsl spec, and `bdd round-trip` proves
        // the two agree (the same semantic canonicalisation round-trip applies).
        if has_tag(&feature.tags, "decision") {
            total += 1;
            let spec = match compile_decision(&feature, &config) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: {fp}: {e:#}");
                    return Ok(1);
                }
            };
            if list {
                let n = spec["nodes"].as_array().map(|a| a.len()).unwrap_or(0);
                println!("{base} :: {}  (@decision graph, {n} nodes)", spec["name"].as_str().unwrap_or(""));
                continue;
            }
            if let Err(e) = crate::spec::from_spec_with_dir(&spec, None, None) {
                let msg = format!("{e:#}");
                let located = match locate_semantic_error_line(&feature, &msg) {
                    Some(l) => format!("line {l}: {msg}"),
                    None => msg,
                };
                eprintln!("error: {base}: generated decision spec fails validate: {located}");
                return Ok(1);
            }
            // Match the checked-in dsl convention (round-trip normalises
            // dsl_version away, so either spelling round-trips; dsl specs omit it).
            let mut payload = spec.clone();
            if let Some(o) = payload.as_object_mut() {
                o.remove("dsl_version");
            }
            let body = format!("{}\n", serde_json::to_string_pretty(&payload)?);
            if check {
                println!("ok: {base} :: {} ({} nodes)", spec["name"].as_str().unwrap_or(""), spec["nodes"].as_array().map(|a| a.len()).unwrap_or(0));
                continue;
            }
            match out {
                None => print!("{body}"),
                Some(out) => {
                    std::fs::create_dir_all(out)?;
                    let dest = Path::new(out).join(format!("{}.json", spec["name"].as_str().unwrap_or("")));
                    std::fs::write(&dest, body)?;
                    println!("wrote {}", dest.display());
                }
            }
            continue;
        }
        for scenario in &feature.scenarios {
            let spec = match compile_scenario(&feature, scenario, &config, cb) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: {fp}: {e:#}");
                    return Ok(1);
                }
            };
            total += 1;
            let meta = spec["_bdd"].clone();
            let need = required_state(&spec);
            let scenario_name = meta["scenario"].as_str().unwrap_or("").to_string();
            if list {
                let mut line = format!("{base} :: {scenario_name}");
                if !need.is_empty() {
                    line.push_str(&format!("   [state: {}]", need.join(", ")));
                }
                println!("{line}");
                continue;
            }
            let payload = public_spec(&spec);
            let body = format!("{}\n", serde_json::to_string_pretty(&payload)?);
            if check {
                if !spec.get("nodes").and_then(Value::as_array).map(|a| !a.is_empty()).unwrap_or(false)
                    || payload.get("start").is_none()
                {
                    eprintln!("error: {scenario_name}: empty spec");
                    return Ok(1);
                }
                let mut line = format!("ok: {base} :: {scenario_name} ({} nodes", spec["nodes"].as_array().map(|a| a.len()).unwrap_or(0));
                if !need.is_empty() {
                    line.push_str(&format!(", state: {}", need.join(", ")));
                }
                println!("{line})");
                continue;
            }
            match out {
                None => print!("{body}"),
                Some(out) => {
                    std::fs::create_dir_all(out)?;
                    let dest = Path::new(out).join(format!("{}.json", spec["name"].as_str().unwrap_or("")));
                    std::fs::write(&dest, body)?;
                    println!("wrote {}", dest.display());
                }
            }
        }
    }
    if total == 0 {
        eprintln!("no scenarios found");
        return Ok(1);
    }
    Ok(0)
}

// ── scaffold CLI: open a greenfield decision workflow ─────────────────────────

/// Options for `bdd scaffold`.
pub struct ScaffoldOptions {
    pub name: String,
    pub cat: String,
    pub intent: Option<String>,
    pub stdout: bool,
    pub force: bool,
    pub primary_q: String,
    pub single: bool,
    pub nested: bool,
    /// Starter shape: `single` | `two_node` | `nested` | `gate` | `loop`
    /// (None → derived from `single`/`nested`, default `two_node`).
    pub pattern: Option<String>,
}

/// The scaffolded `.feature` text. Kept as a pure function so tests can assert
/// on the skeleton without touching the filesystem.
pub fn scaffold_body(
    name: &str,
    cat: &str,
    primary_q: &str,
    desc: &str,
    nodes: usize,
    nested: bool,
) -> String {
    // Back-compat shim for the original two knobs → a named pattern.
    let pattern = if nested {
        "nested"
    } else if nodes <= 1 {
        "single"
    } else {
        "two_node"
    };
    scaffold_body_pattern(name, cat, primary_q, desc, pattern).unwrap_or_default()
}

/// The set of starter shapes `bdd scaffold --pattern` understands, with a
/// one-line description each. Kept next to the builder so help text and builder
/// cannot drift.
pub const SCAFFOLD_PATTERNS: &[(&str, &str)] = &[
    ("two_node", "choice → score → STOP (default; threshold labels + keep)"),
    ("single", "one choice node that keeps its answer"),
    ("nested", "prefilter → sub-workflow → audit (pipeline)"),
    ("gate", "one choice node with a gate action (ALLOW/CONFIRM/BLOCK)"),
    ("loop", "refine loop: a choice node that routes back to itself"),
    ("dynamic", "code extracts candidates into a state array; a choice question selects one id (criteria.from_state)"),
];

/// Build a starter `.feature` for a named pattern. Errors on an unknown pattern
/// (so the CLI/MCP can reject a typo instead of silently emitting the default).
pub fn scaffold_body_pattern(
    name: &str,
    cat: &str,
    primary_q: &str,
    desc: &str,
    pattern: &str,
) -> Result<String> {
    match pattern {
        "single" | "two_node" | "nested" | "gate" | "loop" | "dynamic" => {}
        other => {
            let names: Vec<&str> = SCAFFOLD_PATTERNS.iter().map(|(n, _)| *n).collect();
            bail!(
                "unknown scaffold pattern {other:?} — choose one of: {}",
                names.join(", ")
            );
        }
    }
    if pattern == "gate" {
        return Ok(scaffold_gate(name, cat, desc, primary_q));
    }
    if pattern == "loop" {
        return Ok(scaffold_loop(name, cat, desc));
    }
    if pattern == "dynamic" {
        return Ok(scaffold_dynamic(name, cat, desc, primary_q));
    }
    let nodes = if pattern == "single" { 1 } else { 2 };
    Ok(scaffold_body_classic(
        name,
        cat,
        primary_q,
        desc,
        nodes,
        pattern == "nested",
    ))
}

/// One choice node whose outcome is a calibrated gate action
/// (`ALLOW` / `CONFIRM` / `BLOCK`), the shape of `triage_then_moderate`'s
/// `safety_gate` / `billing_gate` nodes.
fn scaffold_gate(name: &str, cat: &str, desc: &str, primary_q: &str) -> String {
    let mut b = scaffold_header(name, cat, desc, "gate", "one choice node with a gate action");
    b.push_str("@decision
");
    b.push_str(&format!("Feature: {name}
"));
    b.push_str(&format!("  {desc}\n\n"));
    b.push_str(&format!("  Scenario: {name}\n"));
    b.push_str(&format!("    Given a choice question \"{primary_q}\" asks \"Should this proceed?\"\n"));
    b.push_str("      | answer | meaning                              |\n");
    b.push_str("      | A      | no, it is safe to proceed as-is     |\n");
    b.push_str("      | B      | yes, it is risky and needs a human  |\n");
    b.push_str("    When any other answer goes to \"STOP\"\n");
    b.push_str(&format!("    Then gate \"{primary_q}\" on option \"B\" with block 0.6 allow 0.7\n"));
    b
}

/// Refine loop: a single node whose `revise` answer routes back to itself
/// (bounded by `max_retries`), the shape of `iterative_refine_loop`.
fn scaffold_loop(name: &str, cat: &str, desc: &str) -> String {
    let mut b = scaffold_header(name, cat, desc, "loop", "a choice node that routes back to itself");
    b.push_str("@decision\n");
    b.push_str(&format!("Feature: {name}\n"));
    b.push_str(&format!("  {desc}\n\n"));
    b.push_str("  Background:\n");
    bpush_str_bg(&mut b);
    b.push_str(&format!("  Scenario: {name}\n"));
    b.push_str("    Given a choice question \"verdict\" asks \"Accept the current draft, revise once more, or escalate to a human?\"\n");
    b.push_str("      | answer   | meaning                          |\n");
    b.push_str("      | accept   | good enough to ship as-is         |\n");
    b.push_str("      | revise   | one more pass will improve it     |\n");
    b.push_str("      | escalate | needs a human editor's judgement  |\n");
    b.push_str("    When the answer \"accept\" goes to \"STOP\"\n");
    b.push_str(&format!("    And the answer \"revise\" goes to \"{name}\"\n"));
    b.push_str("    And the answer \"escalate\" goes to \"STOP\"\n");
    b.push_str("    Then allow up to 3 retries\n");
    b
}

/// Dynamic-candidate pattern: a capability extracts candidates into a state
/// array, then a choice question selects exactly one of them by id
/// (`criteria.from_state`) - the "the model never generates the value, it only
/// picks an id" shape of awesome-jev span-selection. The `select` node's table
/// carries the dynamic dialect of a choice question (`| from_state | ... |` +
/// `| id_key | ... |` + `| value_key | ... |` + `| none | ... |`) instead of a
/// static option list.
fn scaffold_dynamic(name: &str, cat: &str, desc: &str, primary_q: &str) -> String {
    let q = if primary_q.is_empty() { "candidate" } else { primary_q };
    let mut b = scaffold_header(
        name,
        cat,
        desc,
        "dynamic",
        "a capability extracts candidates into a state array; a choice question selects one id",
    );
    b.push_str("@decision\n");
    b.push_str(&format!("Feature: {name}\n"));
    b.push_str(&format!("  {desc}\n\n"));
    b.push_str("  Background:\n");
    b.push_str("    Given the workflow runs at most 6 iterations\n");
    b.push_str("    Given the capabilities\n");
    b.push_str(
        "      | spans | {\"kind\":\"text\",\"op\":\"spans\",\"pattern\":\"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\"} |\n",
    );
    b.push_str("\n");
    b.push_str(&format!("  Scenario: {name}\n"));
    b.push_str("    Given a choice question \"has_text\" asks \"Is there source text to scan?\"\n");
    b.push_str("      | answer | meaning |\n");
    b.push_str("      | A      | no      |\n");
    b.push_str("      | B      | yes     |\n");
    b.push_str("    And answer \"B\" when the field \"text\" contains \"@\" with hit 0.9\n");
    b.push_str("    When the answer \"B\" goes to \"select\"\n");
    b.push_str("    And any other answer goes to \"STOP\"\n");
    b.push_str("    Then call the capability \"spans\"\n");
    b.push_str("      | with.text | ${state.text} |\n");
    b.push_str("      | project.candidates | /candidates |\n");
    b.push_str("    Then allow up to 1 retries\n");
    b.push_str("\n");
    b.push_str("  Scenario: select\n");
    b.push_str(&format!(
        "    Given a choice question \"{q}\" asks \"Which candidate does the text identify? Select none when it does not establish exactly one.\"\n"
    ));
    b.push_str("      | answer | meaning |\n");
    b.push_str("      | from_state | candidates |\n");
    b.push_str("      | id_key | id |\n");
    b.push_str("      | value_key | value |\n");
    b.push_str("      | none | No candidate is clearly identified. |\n");
    b.push_str("    And the answer comes from the field \"selected_id\"\n");
    b.push_str("    When the answer \"none\" goes to \"STOP\"\n");
    b.push_str("    And any other answer goes to \"STOP\"\n");
    b.push_str(&format!("    Then keep the answer to \"{q}\"\n"));
    b.push_str("    Then allow up to 1 retries\n");
    b
}

fn bpush_str_bg(b: &mut String) {
    b.push_str("    Given the workflow runs at most 8 iterations\n");
}

fn scaffold_header(name: &str, cat: &str, desc: &str, pattern: &str, blurb: &str) -> String {
    let mut body = String::new();
    body.push_str("# Scaffold for \"");
    body.push_str(name);
    body.push_str("\" (category: ");
    body.push_str(cat);
    body.push_str(")\n");
    body.push_str("# Intent: ");
    body.push_str(desc);
    body.push_str("\n");
    body.push_str(&format!("# Pattern: {pattern} — {blurb}\n"));
    body.push_str("# Generated by `laya-workflow bdd scaffold`. Extend this decision graph:\n");
    body.push_str("#   questions:    `Given a choice|score|noul question \"<q>\" asks \"...\"` + answer table\n");
    body.push_str("#   routing:      `When the answer \"<a>\" goes to \"<node>\"` / `And any other answer goes to \"STOP\"`\n");
    body.push_str("#   outcomes:     `Then label \"<L>\" when the answer to \"<q>\" is ...` / `And keep the answer to \"<q>\"`\n");
    body.push_str("#   capabilities: `Given the policy | key | value |` (background) / `Then run the \"<cap>\" capability ...`\n");
    body.push_str("#   nesting:      `And when the sub workflow completes continue at \"<node>\"` hands control back\n");
    body.push_str("# Keep this file the single source of truth, then:\n");
    body.push_str("#   laya-workflow bdd build <file>                     # compile + validate + 100% coverage\n");
    body.push_str("#   laya-workflow bdd transpile <file> --out dsl/<cat> # emit the dsl spec\n");
    body.push_str("#   laya-workflow bdd round-trip                       # prove .feature == dsl spec\n");
    body
}

fn scaffold_body_classic(
    name: &str,
    cat: &str,
    primary_q: &str,
    desc: &str,
    nodes: usize,
    nested: bool,
) -> String {
    let mut body = String::new();
    body.push_str("# Scaffold for \"");
    body.push_str(name);
    body.push_str("\" (category: ");
    body.push_str(cat);
    body.push_str(")\n");
    body.push_str("# Intent: ");
    body.push_str(desc);
    body.push_str("\n");
    body.push_str("# Generated by `laya-workflow bdd scaffold`. Extend this decision graph:\n");
    body.push_str("#   questions:    `Given a choice|score|noul question \"<q>\" asks \"...\"` + answer table\n");
    body.push_str("#   routing:      `When the answer \"<a>\" goes to \"<node>\"` / `And any other answer goes to \"STOP\"`\n");
    body.push_str("#   outcomes:     `Then label \"<L>\" when the answer to \"<q>\" is ...` / `And keep the answer to \"<q>\"`\n");
    body.push_str("#   capabilities: `Given the policy | key | value |` (background) / `Then run the \"<cap>\" capability ...`\n");
    body.push_str("#   nesting:      `And when the sub workflow completes continue at \"<node>\"` hands control back\n");
    if nested {
        body.push_str("# Starter: nested-pipeline graph (prefilter -> sub-workflow -> audit). The sub\n");
        body.push_str("# hands control back to the caller with the continuation step; swap in any\n");
        body.push_str("# sub-workflow by name. Use --single for a 1-node start. Keep this file the\n");
        body.push_str("# single source of truth, then:\n");
    } else {
        body.push_str("# Starter: 2-node canonical graph (choice -> score -> STOP). Clone a\n");
        body.push_str("# Scenario and re-wire its edges to grow the graph; use --single for a\n");
        body.push_str("# 1-node start. Keep this file the single source of truth, then:\n");
    }
    body.push_str("#   laya-workflow bdd build <file>                     # compile + validate + 100% coverage\n");
    body.push_str("#   laya-workflow bdd transpile <file> --out dsl/<cat> # emit the dsl spec\n");
    body.push_str("#   laya-workflow bdd round-trip                       # prove .feature == dsl spec\n");
    body.push_str("@decision\n");
    body.push_str(&format!("Feature: {name}\n"));
    body.push_str(&format!("  {desc}\n\n"));
    if nested {
        // Nested-pipeline starter: prefilter (noise drop) -> sub-workflow ->
        // downstream audit. The sub-workflow hands control back to the caller
        // with the continuation step, so the pipeline runs sub-graph THEN a
        // local downstream step. Referencing an existing workflow keeps the
        // skeleton valid with no further setup; swap in any sub-workflow.
        body.push_str("  Scenario: prefilter\n");
        body.push_str("    Given a choice question \"is_noise\" asks \"Is this a real request that needs routing?\"\n");
        body.push_str("      | answer | meaning                                      |\n");
        body.push_str("      | A      | no, it is a real request that needs routing  |\n");
        body.push_str("      | B      | yes, it is automated noise or a duplicate    |\n");
        body.push_str("    And answer by rules on the field \"text\" with hit 0.9 and fallback \"A\"\n");
        body.push_str("      | label | needles |\n");
        body.push_str("      | B | \"digest\", \"newsletter\", \"archived\", \"automated\", \"unsubscribe\" |\n");
        body.push_str("    When the answer \"A\" goes to \"intake\"\n");
        body.push_str("    And any other answer goes to \"STOP\"\n");
        body.push_str("    Then allow up to 1 retries\n");
        body.push_str("\n");
        body.push_str("  Scenario: intake\n");
        body.push_str("    Given the sub workflow \"support_ticket_router\"\n");
        body.push_str("    And when the sub workflow completes continue at \"audit\"\n");
        body.push_str("\n");
        body.push_str("  Scenario: audit\n");
        body.push_str("    Given a choice question \"needs_human\" asks \"Does a human need to double-check the routing outcome?\"\n");
        body.push_str("      | answer | meaning                                  |\n");
        body.push_str("      | A      | no, the automated routing is trustworthy  |\n");
        body.push_str("      | B      | yes, a human should verify before acting   |\n");
        body.push_str("    When any other answer goes to \"STOP\"\n");
        body.push_str("    Then label \"AUDIT_REQUIRED\" when the answer to \"needs_human\" is \"B\"\n");
        body.push_str("    And label \"AUDIT_SKIPPED\" otherwise\n");
        body.push_str("\n");
        return body;
    }
    body.push_str(&format!("  Scenario: {name}\n"));
    body.push_str(&format!("    Given a choice question \"{primary_q}\" asks \"Choose an option\"\n"));
    body.push_str("      | answer   | meaning       |\n");
    body.push_str("      | option_a | first choice  |\n");
    body.push_str("      | option_b | second choice |\n");
    if nodes >= 2 {
        body.push_str(&format!("    When the answer \"option_a\" goes to \"severity_probe\"\n"));
        body.push_str("    And any other answer goes to \"STOP\"\n");
        body.push_str(&format!("    Then keep the answer to \"{primary_q}\"\n"));
        body.push_str("\n");
        body.push_str("  Scenario: severity_probe\n");
        body.push_str("    Given a score question \"severity\" asks \"How severe is the impact?\"\n");
        body.push_str("      | level    |\n");
        body.push_str("      | minor    |\n");
        body.push_str("      | outage   |\n");
        body.push_str("    When any other answer goes to \"STOP\"\n");
        body.push_str("    Then label \"PAGE_ONCALL\" when \"severity\" is at least 1.5\n");
        body.push_str("    And label \"BACKLOG\" otherwise\n");
    } else {
        body.push_str("    When the answer \"option_a\" goes to \"STOP\"\n");
        body.push_str("    And any other answer goes to \"STOP\"\n");
        body.push_str(&format!("    Then keep the answer to \"{primary_q}\"\n"));
    }
    body
}

/// Generate a minimal valid `@decision` feature skeleton from an intent. The
/// skeleton is chosen so it already passes `bdd build` (compile + validate +
/// 100% coverage) — the cheapest valid start to the authoring loop:
/// `scaffold → validate/extend → transpile → round-trip`. Self-verifies
/// in-process by compiling the generated text before writing/printing.
pub fn scaffold(o: &ScaffoldOptions) -> Result<i32> {
    let name = o.name.trim();
    if name.is_empty() {
        eprintln!("error: --name is required");
        return Ok(1);
    }
    let cat = if o.cat.trim().is_empty() {
        "routing".to_string()
    } else {
        o.cat.trim().to_string()
    };
    let primary_q = if o.primary_q.trim().is_empty() {
        "option".to_string()
    } else {
        o.primary_q.trim().to_string()
    };
    let intent = o.intent.as_deref().unwrap_or("").trim();
    let desc = if intent.is_empty() {
        format!("The {name} decision workflow (scaffolded).")
    } else {
        intent.to_string()
    };
    // Resolve the starter shape: an explicit --pattern wins; else the legacy
    // --single / --nested flags; else the canonical 2-node graph.
    let pattern = match o.pattern.as_deref().map(str::trim) {
        Some(p) if !p.is_empty() => p.to_string(),
        _ if o.nested => "nested".to_string(),
        _ if o.single => "single".to_string(),
        _ => "two_node".to_string(),
    };
    let body = match scaffold_body_pattern(name, &cat, &primary_q, &desc, &pattern) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: {e:#}");
            return Ok(1);
        }
    };

    // Self-verify: the skeleton must compile to a valid decision graph.
    let feature = match parse(&body, "<scaffold>") {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: generated skeleton fails to parse: {e:#}");
            return Ok(1);
        }
    };
    let spec = match compile_decision(&feature, &json!({})) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: generated skeleton fails to compile: {e:#}");
            return Ok(1);
        }
    };
    if let Err(e) = crate::spec::from_spec_with_dir(&spec, None, None) {
        eprintln!("error: generated skeleton fails to validate: {e:#}");
        return Ok(1);
    }

    let nodes = spec["nodes"].as_array().map(|a| a.len()).unwrap_or(0);
    if o.stdout {
        print!("{body}");
        println!("ok: {name} (@decision graph, {nodes} nodes) — valid skeleton");
        return Ok(0);
    }

    let dir = Path::new("bdd").join("workflows").join(&cat);
    let dest = dir.join(format!("{name}.feature"));
    if dest.exists() && !o.force {
        eprintln!("error: {} already exists (use --force to overwrite)", dest.display());
        return Ok(1);
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&dest, &body)?;
    println!("wrote {}", dest.display());
    println!("ok: {name} compiles and validates as a @decision graph ({nodes} nodes)");
    println!("next: extend the graph, then `laya-workflow bdd build {}` and `bdd transpile {} --out dsl/{}`", dest.display(), dest.display(), cat);
    Ok(0)
}

// ── needle assist (port of laya-workflow bdd build --assist) ──────────────────

/// The bench's measured model: below 0.5 even the "right" routing is too often
/// a guess. 0.5 is the floor for "likely"; anything below prints as "guess".
pub const ACCEPT_FLOOR: f64 = 0.5;

const SYSTEM: &str = "You are a Gherkin-to-workflow compiler. Call exactly one tool per step.";

const TOOLS: &str = "[
 {\"type\":\"function\",\"name\":\"open_page\",\"description\":\"Open a URL and wait until it is loaded.\",\"triggers\":[\"\\b(am on|open)\\b\"],\"parameters\":{\"type\":\"object\",\"properties\":{\"url\":{\"type\":\"string\"}},\"required\":[\"url\"]}},
 {\"type\":\"function\",\"name\":\"navigate\",\"description\":\"Move an open tab to a new URL.\",\"triggers\":[\"\\bnavigate\\b\"],\"parameters\":{\"type\":\"object\",\"properties\":{\"url\":{\"type\":\"string\"}},\"required\":[\"url\"]}},
 {\"type\":\"function\",\"name\":\"wait_for\",\"description\":\"Poll until a CSS selector matches.\",\"triggers\":[\"\\bwait for\\b\"],\"parameters\":{\"type\":\"object\",\"properties\":{\"selector\":{\"type\":\"string\"}},\"required\":[\"selector\"]}},
 {\"type\":\"function\",\"name\":\"click\",\"description\":\"Click a CSS selector.\",\"triggers\":[\"\\bclick\\b\"],\"parameters\":{\"type\":\"object\",\"properties\":{\"selector\":{\"type\":\"string\"}},\"required\":[\"selector\"]}},
 {\"type\":\"function\",\"name\":\"type_text\",\"description\":\"Type text into a CSS selector.\",\"triggers\":[\"\\btype\\b\"],\"parameters\":{\"type\":\"object\",\"properties\":{\"text\":{\"type\":\"string\"},\"selector\":{\"type\":\"string\"}},\"required\":[\"text\",\"selector\"]}},
 {\"type\":\"function\",\"name\":\"select_option\",\"description\":\"Select a value in a CSS selector.\",\"triggers\":[\"\\bselect\\b\"],\"parameters\":{\"type\":\"object\",\"properties\":{\"value\":{\"type\":\"string\"},\"selector\":{\"type\":\"string\"}},\"required\":[\"value\",\"selector\"]}},
 {\"type\":\"function\",\"name\":\"run_js\",\"description\":\"Evaluate a javascript expression in the page.\",\"triggers\":[\"\\brun javascript\\b\"],\"parameters\":{\"type\":\"object\",\"properties\":{\"expression\":{\"type\":\"string\"}},\"required\":[\"expression\"]}},
 {\"type\":\"function\",\"name\":\"assert\",\"description\":\"Run a named page assertion: title_contains, url_contains, visible, absent, is_true, is_false, equals, contains, equals_text.\",\"triggers\":[\"\\b(title|url) contains|is visible|is absent|is true|is false|equals|contains\\b\"],\"parameters\":{\"type\":\"object\",\"properties\":{\"assertion\":{\"type\":\"string\"},\"value\":{\"type\":\"string\"},\"expected\":{\"type\":\"string\"}},\"required\":[\"assertion\"]}},
 {\"type\":\"function\",\"name\":\"release_page\",\"description\":\"Close the current page.\",\"triggers\":[\"\\brelease\\b\"],\"parameters\":{\"type\":\"object\",\"properties\":{\"target_id\":{\"type\":\"string\"}},\"required\":[]}}
]";

const TOOL_TO_OP: &[(&str, &str)] = &[
    ("open_page", "open"),
    ("navigate", "navigate"),
    ("wait_for", "wait_for"),
    ("click", "click"),
    ("type_text", "type"),
    ("select_option", "select"),
    ("run_js", "evaluate"),
    ("assert", "assert"),
    ("release_page", "release"),
];

/// Ask Needle 3 (through this binary's own `mcp serve`) what a step probably
/// means. Returns `None` when the engine/weights are absent or nothing was
/// extracted — suggestions only, never a compile.
fn needle_suggest(step: &str) -> Option<Value> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let req = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "needle_complete", "arguments": {
            "prompt": step,
            "tools": serde_json::from_str::<Value>(TOOLS).ok()?,
            "system": SYSTEM
        }}
    });
    let exe = std::env::current_exe().ok()?;
    let mut child = Command::new(exe)
        .arg("mcp")
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    {
        let stdin = child.stdin.as_mut()?;
        let body = serde_json::to_string(&req).ok()?;
        let _ = stdin.write_all(body.as_bytes());
        let _ = stdin.write_all(b"\n");
    }
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    let d: Value = serde_json::from_slice(&out.stdout).ok()?;
    let text = d
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)?;
    let r: Value = serde_json::from_str(text).ok()?;
    let calls = r.get("function_calls").and_then(Value::as_array).cloned().unwrap_or_default();
    let supp = r.get("suppressed_calls").and_then(Value::as_array).cloned().unwrap_or_default();
    let item = calls.first().cloned().or_else(|| supp.first().cloned())?;
    let args = item.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
    let name = item.get("name").and_then(Value::as_str)?;
    let op = TOOL_TO_OP.iter().find(|(n, _)| *n == name).map(|(_, o)| *o)?;
    let mut out_v = Map::new();
    out_v.insert("op".into(), json!(op));
    out_v.insert("confidence".into(), r.get("confidence").cloned().unwrap_or(json!(0.0)));
    for k in ["assertion", "value", "expected"] {
        if let Some(v) = args.get(k) {
            if !v.is_null() {
                out_v.insert(k.into(), v.clone());
            }
        }
    }
    out_v.insert("matched_tool".into(), json!(name));
    Some(Value::Object(out_v))
}

// ── check gates (ports of scripts/bdd/{vocabulary,probe,args_probe,doc}_check.py) ──

fn plugin_src() -> Result<String> {
    let p = repo_root().join("plugins").join("bdd").join("main.rhai");
    match std::fs::read_to_string(&p) {
        Ok(s) => Ok(s),
        Err(_) => Ok(include_str!("../plugins/bdd/main.rhai").to_string()),
    }
}

/// Port of `vocabulary_check.py check_plugin_messages`.
fn check_plugin_messages() -> Vec<String> {
    let mut problems = Vec::new();
    let src = match plugin_src() {
        Ok(s) => s,
        Err(e) => return vec![e.to_string()],
    };
    let op_dispatch = Regex::new(r#"if op == "([a-z_]+)" \{ return _op_"#).unwrap();
    let assertion = Regex::new(r#"if name == "([a-z_]+)""#).unwrap();
    let enum_re = Regex::new(r"\(([a-z_ |]+)\)").unwrap();
    let ops: BTreeSet<String> = op_dispatch
        .captures_iter(&src)
        .map(|c| c[1].to_string())
        .collect();
    let assertions: BTreeSet<String> = assertion
        .captures_iter(&src)
        .map(|c| c[1].to_string())
        .collect();
    if ops.is_empty() || assertions.is_empty() {
        return vec![
            "plugin dispatch not found - the patterns in the bdd vocabulary \
             gate no longer match plugins/bdd/main.rhai"
                .into(),
        ];
    }
    for (label, want, needle) in [
        ("unknown op", &ops, "bdd: unknown op"),
        ("unknown assertion", &assertions, "bdd.assert: unknown assertion"),
    ] {
        let Some(at) = src.find(needle) else {
            problems.push(format!("the {label} message is gone from plugins/bdd/main.rhai"));
            continue;
        };
        let before = &src[at.saturating_sub(40)..at];
        if !before.contains("throw") {
            problems.push(format!("the {label} message is gone from plugins/bdd/main.rhai"));
            continue;
        }
        let end = src[at..].find(';').map(|e| at + e).unwrap_or(at + 400);
        let statement = &src[at..end.min(src.len())];
        let Some(m) = enum_re.captures(statement) else {
            problems.push(format!(
                "the {label} message no longer lists the vocabulary: {}",
                statement.trim().chars().take(70).collect::<String>()
            ));
            continue;
        };
        let listed: BTreeSet<String> = m[1]
            .split('|')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        for missing in want.difference(&listed) {
            problems.push(format!(
                "the {label} message does not mention {missing:?}, which the \
                 plugin handles - someone hitting that error gets told it does not exist"
            ));
        }
        for extra in listed.difference(want) {
            problems.push(format!(
                "the {label} message advertises {extra:?}, which the plugin does \
                 not handle - someone will try it and get no such op"
            ));
        }
    }
    problems
}

/// Port of `vocabulary_check.py check_plugin_header`.
fn check_plugin_header() -> Vec<String> {
    let mut problems = Vec::new();
    let src = match plugin_src() {
        Ok(s) => s,
        Err(e) => return vec![e.to_string()],
    };
    fn header_block<'a>(src: &'a str, start: &str, end: &str) -> &'a str {
        let Some(a) = src.find(start) else { return "" };
        let Some(rel_b) = src[a + 1..].find(end) else { return "" };
        &src[a..a + 1 + rel_b]
    }
    let ops_block = header_block(&src, "// Ops (pick one via", "// The assertion names");
    let assert_block = header_block(&src, "// The assertion names", "// Every assert");
    if ops_block.is_empty() || assert_block.is_empty() {
        return vec![
            "the plugin's header tables are gone from plugins/bdd/main.rhai - the \
             patterns in the bdd vocabulary gate no longer match"
                .into(),
        ];
    }
    let op_dispatch = Regex::new(r#"if op == "([a-z_]+)" \{ return _op_"#).unwrap();
    let assertion = Regex::new(r#"if name == "([a-z_]+)""#).unwrap();
    let header_ops = Regex::new(r"(?m)^//   ([a-z_]+)\s{2,}").unwrap();
    let header_assert = Regex::new(r"(?m)^//   ([a-z_]+)\s{2,}value:").unwrap();
    let num_default = Regex::new(r#"_num\(ctx, "([a-z_]+)", (\d+)\)"#).unwrap();
    let ops: BTreeSet<String> = op_dispatch
        .captures_iter(&src)
        .map(|c| c[1].to_string())
        .collect();
    let assertions: BTreeSet<String> = assertion
        .captures_iter(&src)
        .map(|c| c[1].to_string())
        .collect();
    let listed_ops: BTreeSet<String> = header_ops
        .captures_iter(ops_block)
        .map(|c| c[1].to_string())
        .collect();
    for missing in ops.difference(&listed_ops) {
        problems.push(format!(
            "the plugin handles op {missing:?} but its header does not document it"
        ));
    }
    for extra in listed_ops.difference(&ops) {
        problems.push(format!(
            "the header documents op {extra:?}, which the dispatcher does not handle"
        ));
    }
    let listed_assertions: BTreeSet<String> = header_assert
        .captures_iter(assert_block)
        .map(|c| c[1].to_string())
        .collect();
    for missing in assertions.difference(&listed_assertions) {
        problems.push(format!(
            "the plugin implements assertion {missing:?} but its header does not \
             document what `with` it takes"
        ));
    }
    for extra in listed_assertions.difference(&assertions) {
        problems.push(format!(
            "the header documents assertion {extra:?}, which the plugin does not implement"
        ));
    }
    // A default the header quotes and the code does not use is a number someone
    // will budget against.
    let flat = ops_block
        .lines()
        .map(|ln| {
            ln.trim_start()
                .strip_prefix("//")
                .map(|s| s.to_string())
                .unwrap_or_default()
                .trim()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join(" ");
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    let doc_re = Regex::new(r"with\.([a-z_]+)`? \(default (\d+)\)").unwrap();
    let documented: std::collections::HashMap<String, i64> = doc_re
        .captures_iter(&flat)
        .map(|c| (c[1].to_string(), c[2].parse::<i64>().unwrap_or(0)))
        .collect();
    let actual: std::collections::HashMap<String, i64> = num_default
        .captures_iter(&src)
        .map(|c| (c[1].to_string(), c[2].parse::<i64>().unwrap_or(0)))
        .collect();
    let mut keys: Vec<&String> = actual.keys().collect();
    keys.sort();
    for key in keys {
        let want = actual[key];
        match documented.get(key) {
            None => problems.push(format!(
                "the plugin has a default for with.{key} ({want}) that the header does not quote"
            )),
            Some(got) if *got != want => problems.push(format!(
                "the header says with.{key} defaults to {got}, the code uses {want}"
            )),
            _ => {}
        }
    }
    problems
}

/// Port of `vocabulary_check.py check_xfail_reasons`.
fn check_xfail_reasons() -> Vec<String> {
    const XFAIL: &str = "expected_failure";
    let mut problems = Vec::new();
    let feature_dir = repo_root().join("bdd").join("features");
    if !feature_dir.is_dir() {
        // No repo corpus next to a release binary: this gate has nothing to check.
        return Vec::new();
    }
    let mut names: Vec<String> = Vec::new();
    if feature_dir.is_dir() {
        if let Ok(rd) = std::fs::read_dir(&feature_dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.ends_with(".feature") {
                    names.push(name);
                }
            }
        }
    }
    names.sort();
    if names.is_empty() {
        return vec![format!("{}: no .feature files found", feature_dir.display())];
    }
    for name in names {
        let path = feature_dir.join(&name);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                problems.push(format!("{name}: {e}"));
                continue;
            }
        };
        let feature = match parse(&text, &path.to_string_lossy()) {
            Ok(f) => f,
            Err(e) => {
                problems.push(format!("{name}: {e}"));
                continue;
            }
        };
        for scenario in &feature.scenarios {
            let tagged = scenario
                .tags
                .iter()
                .any(|t| t == XFAIL || t.starts_with(&format!("{XFAIL}(")));
            if !tagged {
                continue;
            }
            if tag_reason(scenario, XFAIL).is_none() {
                problems.push(format!(
                    "{name} :: {}: @{XFAIL} declares no reason. Without one, any \
                     non-zero exit counts - including a typo'd Given, a Chrome that \
                     would not start, or a 404 from the fixture. Write @{XFAIL}(<text \
                     the failure must contain>).",
                    scenario.name
                ));
            }
        }
    }
    problems
}

/// Port of `vocabulary_check.py main`.
pub fn check_vocabulary() -> Result<i32> {
    let (rc, _) = check_vocabulary_impl(false)?;
    Ok(rc)
}
fn check_vocabulary_impl(silent: bool) -> Result<(i32, Option<String>)> {
    let mut failures: Vec<String> = Vec::new();
    let mk = |kind: &str, text: &str| Step {
        keyword: format!("{} ", kind),
        kind: kind.to_string(),
        text: text.to_string(),
        line: 1,
        table: Vec::new(),
        row_lines: Vec::new(),
    };

    // (kind, text, want_op, want_assertion, has_page)
    let cases: Vec<(&str, &str, Option<&str>, Option<&str>, bool)> = vec![
        ("given", "the browser is ready", None, None, false),
        ("given", "I am on \"http://x/\"", Some("open"), None, false),
        ("when", "I open \"http://x/\"", Some("open"), None, true),
        ("when", "I navigate to \"http://x/\"", Some("navigate"), None, true),
        ("when", r##"I wait for the element "#a""##, Some("wait_for"), None, true),
        ("when", r##"I click the element "#a""##, Some("click"), None, true),
        ("when", r##"I type "hi" into the element "#a""##, Some("type"), None, true),
        ("when", r##"I select "blue" in the element "#a""##, Some("select"), None, true),
        ("when", "I run javascript \"1+1\"", Some("evaluate"), None, true),
        ("when", "I release the page", Some("release"), None, true),
        ("then", "the page title contains \"x\"", Some("assert"), Some("title_contains"), true),
        ("then", "the page url contains \"x\"", Some("assert"), Some("url_contains"), true),
        ("then", r##"the element "#a" is visible"##, Some("assert"), Some("visible"), true),
        ("then", r##"the element "#a" is absent"##, Some("assert"), Some("absent"), true),
        ("then", "javascript \"a\" is true", Some("assert"), Some("is_true"), true),
        ("then", "javascript \"a\" is false", Some("assert"), Some("is_false"), true),
        ("then", "javascript \"a\" equals 3", Some("assert"), Some("equals"), true),
        ("then", "javascript \"a\" contains \"x\"", Some("assert"), Some("contains"), true),
        ("then", "javascript \"a\" equals text \"x\"", Some("assert"), Some("equals_text"), true),
        ("then", "javascript \"a\" equals_text \"x\"", Some("assert"), Some("equals_text"), true),
        // ── workflow vocabulary (not testing): extraction, conditional, key,
        //    and assertions on collected state ──
        ("when", r##"I extract the text of the element "#a" into t"##, Some("evaluate"), None, true),
        ("when", r##"I extract the attribute "href" of the element "#a" into link"##, Some("evaluate"), None, true),
        ("when", "I extract the page url into u", Some("evaluate"), None, true),
        ("when", "I extract the page title into t", Some("evaluate"), None, true),
        ("when", "I press the key \"Enter\"", Some("key"), None, true),
        ("when", r##"I wait until the element "#a" becomes visible"##, Some("wait_until"), None, true),
        ("when", r##"I click the element "#a" if it is present"##, Some("click"), None, true),
        // state assertions read collected state, so they need no page: has_page=false.
        ("then", "the saved value \"first_row\" equals text \"ACME\"", Some("assert"), Some("state_equals"), false),
        ("then", "the saved value \"first_row\" contains text \"ACME\"", Some("assert"), Some("state_contains"), false),
        // ── exec vocabulary: a CLI command + assertions on its projected state ──
        ("when", "I run the command \"arkcli version\"", Some("exec"), None, false),
        ("then", "the exit code is 0", Some("assert"), Some("exit_code_equals"), false),
        ("then", "stdout contains \"ok\"", Some("assert"), Some("stdout_contains"), false),
        ("then", "stderr contains \"err\"", Some("assert"), Some("stderr_contains"), false),
        ("when", "I release the page", Some("release"), None, true),
    ];
    for (kind, text, want_op, want_assertion, has_page) in &cases {
        match compile_step(&mk(kind, text), *has_page) {
            Err(e) => failures.push(format!("{kind} {text:?}: {e}")),
            Ok((node, _)) => {
                let action = node.action.clone().unwrap_or_else(|| json!({}));
                let got_op = action
                    .get("with")
                    .and_then(|w| w.get("op"))
                    .and_then(Value::as_str);
                if got_op != *want_op {
                    failures.push(format!(
                        "{kind} {text:?}: op {got_op:?}, want {want_op:?}"
                    ));
                    continue;
                }
                let got_assert = action
                    .get("with")
                    .and_then(|w| w.get("assertion"))
                    .and_then(Value::as_str);
                if got_assert != *want_assertion {
                    failures.push(format!(
                        "{kind} {text:?}: assertion {got_assert:?}, want {want_assertion:?}"
                    ));
                }
            }
        }
    }

    // (kind, text, want args)
    let arg_cases: Vec<(&str, &str, Vec<(&str, Value)>)> = vec![
        ("given", "I am on \"http://x/\"", vec![("url", json!("http://x/"))]),
        ("when", "I navigate to \"http://y/\"", vec![("url", json!("http://y/"))]),
        ("when", r##"I wait for the element "#a""##, vec![("selector", json!("#a"))]),
        ("when", r##"I click the element "#a""##, vec![("selector", json!("#a"))]),
        ("when", r##"I type "hi" into the element "#a""##, vec![("selector", json!("#a")), ("text", json!("hi"))]),
        ("when", r##"I select "blue" in the element "#a""##, vec![("selector", json!("#a")), ("value", json!("blue"))]),
        ("when", "I run javascript \"1+1\"", vec![("expression", json!("1+1"))]),
        ("when", "I run javascript \"document.title = 1\"", vec![("expression", json!("document.title = 1"))]),
        ("then", "the page title contains \"x\"", vec![("assertion", json!("title_contains")), ("value", json!("x"))]),
        ("then", "the page url contains \"x\"", vec![("assertion", json!("url_contains")), ("value", json!("x"))]),
        ("then", r##"the element "#a" is visible"##, vec![("assertion", json!("visible")), ("value", json!("#a"))]),
        ("then", r##"the element "#a" is absent"##, vec![("assertion", json!("absent")), ("value", json!("#a"))]),
        ("then", "javascript \"a\" is true", vec![("assertion", json!("is_true")), ("value", json!("a"))]),
        ("then", "javascript \"a\" is false", vec![("assertion", json!("is_false")), ("value", json!("a"))]),
        ("then", "javascript \"a\" equals 3", vec![("assertion", json!("equals")), ("value", json!("a")), ("expected", json!(3))]),
        ("then", "javascript \"a\" contains \"x\"", vec![("assertion", json!("contains")), ("expected", json!("x"))]),
        ("then", "javascript \"a\" equals text \"x\"", vec![("assertion", json!("equals_text")), ("expected", json!("x"))]),
        // ── workflow vocabulary arguments ──
        ("when", r##"I extract the text of the element "#a" into t"##, vec![("expression", json!("document.querySelector('#a').textContent"))]),
        ("when", r##"I extract the attribute "href" of the element "#a" into link"##, vec![("expression", json!("document.querySelector('#a').getAttribute('href')"))]),
        ("when", "I extract the page url into u", vec![("expression", json!("location.href"))]),
        ("when", "I extract the page title into t", vec![("expression", json!("document.title"))]),
        ("when", "I press the key \"Enter\"", vec![("key", json!("Enter"))]),
        ("when", r##"I wait until the element "#a" becomes enabled"##, vec![("expression", json!("(function(){var e=document.querySelector('#a'); return !!e && !e.disabled;})()"))]),
        ("when", r##"I click the element "#a" if it is present"##, vec![("selector", json!("#a")), ("if_present", json!(true))]),
        // ── exec vocabulary arguments ──
        ("when", "I run the command \"arkcli version\"", vec![("cmd", json!("arkcli version"))]),
        ("when", "I run the command \"echo hi >&2; exit 3\"", vec![("cmd", json!("echo hi >&2; exit 3"))]),
        ("then", "the exit code is 0", vec![("assertion", json!("exit_code_equals")), ("value", json!("${state.exit_code}")), ("expected", json!(0))]),
        ("then", "the exit code is \"0\"", vec![("assertion", json!("exit_code_equals")), ("value", json!("${state.exit_code}")), ("expected", json!("0"))]),
        ("then", "stdout contains \"ok\"", vec![("assertion", json!("stdout_contains")), ("value", json!("${state.stdout}")), ("expected", json!("ok"))]),
        ("then", "stderr contains \"err\"", vec![("assertion", json!("stderr_contains")), ("value", json!("${state.stderr}")), ("expected", json!("err"))]),
    ];
    for (kind, text, wants) in &arg_cases {
        match compile_step(&mk(kind, text), true) {
            Err(e) => failures.push(format!("{kind} {text:?}: {e}")),
            Ok((node, _)) => {
                let with = node.action.clone().unwrap_or_else(|| json!({}));
                let with = with.get("with").cloned().unwrap_or_else(|| json!({}));
                for (key, want) in wants {
                    let got = with.get(*key);
                    if got != Some(want) {
                        failures.push(format!(
                            "{kind} {text:?}: with.{key} is {got:?}, want {want:?}"
                        ));
                    }
                }
                for key in ["url", "selector", "text", "value", "expression", "assertion"] {
                    if wants.iter().any(|(k, _)| *k == key) {
                        let v = with.get(key);
                        if v.is_none() || v.and_then(Value::as_str).map(|s| s.is_empty()).unwrap_or(true) {
                            failures.push(format!("{kind} {text:?}: with.{key} compiled empty"));
                        }
                    }
                }
            }
        }
    }

    // Operand types: the type is the author's signal.
    let operand_cases: Vec<(&str, Value)> = vec![
        ("equals 3", json!(3)),
        ("equals \"3\"", json!("3")),
        ("equals true", json!(true)),
        ("equals \"true\"", json!("true")),
        ("equals hello", json!("hello")),
        ("equals \"hello\"", json!("hello")),
        ("equals -1.5", json!(-1.5)),
        ("equals null", Value::Null),
    ];
    for (text, want) in &operand_cases {
        match compile_step(&mk("then", &format!("javascript \"a\" {text}")), true) {
            Err(e) => failures.push(format!("then javascript \"a\" {text}: {e}")),
            Ok((node, _)) => {
                let with = node.action.clone().unwrap_or_else(|| json!({}));
                let with = with.get("with").cloned().unwrap_or_else(|| json!({}));
                let got = with.get("expected");
                let type_matches = match (got, want) {
                    (Some(Value::Number(g)), Value::Number(w)) => {
                        g.as_f64() == w.as_f64()
                    }
                    (Some(Value::String(g)), Value::String(w)) => g == w,
                    (Some(Value::Bool(g)), Value::Bool(w)) => g == w,
                    (Some(Value::Null), Value::Null) => true,
                    _ => false,
                };
                if !type_matches {
                    failures.push(format!(
                        "then javascript \"a\" {text}: expected {want:?}, got {got:?}"
                    ));
                }
            }
        }
    }

    // Workflow extraction must *project* the collected value into the named
    // state key. An extract step that runs the expression but files the result
    // nowhere is the silent-failure shape this whole vocabulary exists to stop:
    // the workflow "succeeds" and the caller reads an empty output. Pinning the
    // project pointer is what makes `@outputs` a real contract rather than a
    // comment.
    let extract_cases: Vec<(&str, &str, &str)> = vec![
        (r##"I extract the text of the element "#a" into first_row"##, "first_row", "/value"),
        (r##"I extract the attribute "href" of the element "#a" into link"##, "link", "/value"),
        ("I extract the page url into page_url", "page_url", "/value"),
        ("I extract the page title into page_title", "page_title", "/value"),
    ];
    for (text, key, ptr) in &extract_cases {
        match compile_step(&mk("when", text), true) {
            Err(e) => failures.push(format!("when {text:?}: {e}")),
            Ok((node, _)) => {
                let got = node
                    .action
                    .as_ref()
                    .and_then(|a| a.get("project"))
                    .and_then(|p| p.get(*key));
                if got != Some(&json!(ptr)) {
                    failures.push(format!(
                        "when {text:?}: project[{key}] is {got:?}, want {ptr:?} - an \
                         extracted value the state never keeps is a caller-visible empty output"
                    ));
                }
            }
        }
    }

    // state_equals / state_contains read collected state, so they must compile
    // with no page open (a workflow checks its outputs after the tab is gone).
    for text in [
        "the saved value \"first_row\" equals text \"ACME\"",
        "the saved value \"first_row\" contains text \"ACME\"",
    ] {
        if let Err(e) = compile_step(&mk("then", text), false) {
            failures.push(format!(
                "then {text:?}: {e} - a state assertion must not require a page"
            ));
        }
    }

    // `release` must leave the compiler in the has-no-page state.
    match compile_step(&mk("when", "I release the page"), true) {
        Err(e) => failures.push(format!("when \"I release the page\": {e}")),
        Ok((_, still_has_page)) => {
            if still_has_page {
                failures.push(
                    "when \"I release the page\": the compiler still believes a page is \
                     open, so any later step would run against a closed target"
                        .into(),
                );
            }
        }
    }

    // Post-release: a step that needs a page must be a compile error.
    let post_release: Vec<(&str, &str)> = vec![
        ("when", "I release the page"),
        ("then", r##"the element "#a" is visible"##),
    ];
    for (kind, text) in &post_release {
        match compile_step(&mk(kind, text), false) {
            Err(e) => {
                // Python: StepError is a pass; UnknownStep is a failure.
                let msg = format!("{e:#}");
                if !msg.contains("needs a page") && !msg.contains("unknown") {
                    // Not a StepError and not an UnknownStep — treat as UnknownStep failure
                    failures.push(format!("post-release {kind} {text:?}: {e}"));
                }
            }
            Ok(_) => failures.push(format!(
                "post-release {kind} {text:?}: compiled with no page, want a StepError"
            )),
        }
    }

    failures.extend(check_plugin_messages());
    failures.extend(check_xfail_reasons());

    // Needs-a-page cases: must refuse without a page.
    let needs_page: Vec<(&str, &str)> = vec![
        ("when", r##"I click the element "#a""##),
        ("when", r##"I type "hi" into the element "#a""##),
        ("when", r##"I select "blue" in the element "#a""##),
        ("when", r##"I wait for the element "#a""##),
        ("when", "I navigate to \"http://x/\""),
        ("when", "I run javascript \"1+1\""),
        ("then", r##"the element "#a" is visible"##),
        ("then", "javascript \"a\" is true"),
    ];
    for (kind, text) in &needs_page {
        match compile_step(&mk(kind, text), false) {
            Err(e) => {
                let msg = format!("{e:#}");
                if msg.contains("needs a page") {
                    continue;
                }
                failures.push(format!("needs-page {text:?}: {e}"));
            }
            Ok(_) => failures.push(format!(
                "needs-page {text:?}: compiled without a page, want a StepError"
            )),
        }
    }

    failures.extend(check_plugin_header());

    if !failures.is_empty() {
        eprintln!("bdd vocabulary: {} problem(s):", failures.len());
        for f in &failures {
            eprintln!("  {f}");
        }
        return Ok((1, None));
    }
    let line = format!(
        "bdd vocabulary: {} steps map correctly, {} step arguments survive, {} operand \
         types survive, {} steps still refuse to run without a page, {} stay refused \
         after a release, every @expected_failure says what it disproves, the plugin's \
         own vocabulary messages are in sync, and its header documents every op, \
         assertion and default it has",
        cases.len(),
        arg_cases.len(),
        operand_cases.len(),
        needs_page.len(),
        post_release.len()
    );
    if !silent { println!("{line}"); }
    Ok((0, Some(line)))
}

/// Port of `probe_check.py` — check hand-written probe specs without Chrome.
pub fn check_probes() -> Result<i32> {
    const TERMINAL: &str = "STOP";
    let root = repo_root();
    let runner_state: BTreeSet<String> = ["url", "cdp_port", "cdp_profile"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let paths: Vec<PathBuf> = [
        "dsl/browser/bdd_assert_probe.json",
        "dsl/browser/bdd_release_probe.json",
        "dsl/browser/bdd_wait_probe.json",
        "dsl/browser/bdd_wait_until_probe.json",
        "dsl/browser/bdd_retry_probe.json",
        "dsl/browser/browser_base_probe.json",
    ]
    .iter()
    .map(|p| root.join(p))
    .filter(|p| p.is_file())
    .collect();
    if paths.is_empty() {
        println!("bdd probes: none found - skipped");
        return Ok(0);
    }

    let state_ref = placeholder_re();
    let mut problems: Vec<String> = Vec::new();
    for path in &paths {
        let rel = rel_path(&path.to_string_lossy());
        let spec: Value = match serde_json::from_str(
            &std::fs::read_to_string(path).with_context(|| format!("reading {rel}"))?,
        ) {
            Ok(v) => v,
            Err(e) => {
                problems.push(format!("{rel}: {e}"));
                continue;
            }
        };
        let nodes = spec.get("nodes").and_then(Value::as_array).cloned().unwrap_or_default();
        if nodes.is_empty() {
            problems.push(format!("{rel}: no nodes"));
            continue;
        }
        let mut names: Vec<String> = Vec::new();
        for n in &nodes {
            let name = n.get("name").and_then(Value::as_str);
            match name {
                None => problems.push(format!("{rel}: a node has no name")),
                Some(name) if names.contains(&name.to_string()) => {
                    problems.push(format!(
                        "{rel}: duplicate node name {name:?} - an edge naming it is ambiguous"
                    ));
                }
                Some(name) => names.push(name.to_string()),
            }
        }
        let start = spec.get("start").and_then(Value::as_str).unwrap_or("");
        if !names.iter().any(|n| n == start) {
            problems.push(format!(
                "{rel}: start {start:?} is not a node; the run would have nothing to do"
            ));
            continue;
        }
        let capabilities: BTreeSet<String> = spec
            .get("capabilities")
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();

        // (1) edge targets must exist
        let mut adjacency: std::collections::HashMap<String, BTreeSet<String>> = Default::default();
        for n in &nodes {
            let name = n.get("name").and_then(Value::as_str).unwrap_or("");
            let mut targets: BTreeSet<String> = BTreeSet::new();
            let edge = n.get("edge").cloned().unwrap_or_else(|| json!({}));
            if let Some(cond) = edge.get("condition").and_then(Value::as_object) {
                for (key, dest) in cond {
                    if let Some(dest) = dest.as_str() {
                        if dest != TERMINAL {
                            targets.insert(dest.to_string());
                            if !names.iter().any(|n| n == dest) {
                                problems.push(format!(
                                    "{rel}: node {name:?} routes {key} -> {dest:?}, which is \
                                     not a node. `validate` accepts this and the run stops early \
                                     with error_node_missing"
                                ));
                            }
                        }
                    }
                }
            }
            if let Some(dest) = edge.get("default").and_then(Value::as_str) {
                if dest != TERMINAL {
                    targets.insert(dest.to_string());
                    if !names.iter().any(|n| n == dest) {
                        problems.push(format!("{rel}: node {name:?} default -> {dest:?}, which is not a node"));
                    }
                }
            }
            if !name.is_empty() {
                adjacency.insert(name.to_string(), targets);
            }
            if let Some(act) = n.get("action") {
                if let Some(cap) = act.get("capability").and_then(Value::as_str) {
                    if !capabilities.contains(cap) {
                        let list: Vec<&String> = capabilities.iter().collect();
                        problems.push(format!(
                            "{rel}: node {name:?} calls capability {cap:?}, which is not declared (have: {})",
                            if list.is_empty() { "none".to_string() } else { list.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ") }
                        ));
                    }
                }
            }
        }

        // (2) reachability
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut stack = vec![start.to_string()];
        while let Some(cur) = stack.pop() {
            if seen.contains(&cur) {
                continue;
            }
            if let Some(targets) = adjacency.get(&cur) {
                seen.insert(cur);
                for t in targets {
                    stack.push(t.clone());
                }
            }
        }
        for name in &names {
            if !seen.contains(name) {
                problems.push(format!(
                    "{rel}: node {name:?} is unreachable from {start:?}, so whatever it \
                     checks is never checked"
                ));
            }
        }

        // (3) state keys
        let mut kept = runner_state.clone();
        for n in &nodes {
            if let Some(keep) = n.pointer("/state/keep").and_then(Value::as_array) {
                for k in keep {
                    if let Some(k) = k.as_str() {
                        kept.insert(k.to_string());
                    }
                }
            }
        }
        for n in &nodes {
            let name = n.get("name").and_then(Value::as_str).unwrap_or("");
            let mut without_state = n.clone();
            if let Some(o) = without_state.as_object_mut() {
                o.remove("state");
            }
            let blob = serde_json::to_string(&without_state).unwrap_or_default();
            let mut keys: BTreeSet<String> = BTreeSet::new();
            for c in state_ref.captures_iter(&blob) {
                keys.insert(c[1].to_string());
            }
            for key in &keys {
                if !kept.contains(key) {
                    problems.push(format!(
                        "{rel}: node {name:?} reads ${{state.{key}}}, which no node keeps \
                         and the runner does not supply"
                    ));
                }
            }
        }
    }
    if !problems.is_empty() {
        eprintln!("bdd probes: {} problem(s):", problems.len());
        for p in &problems {
            eprintln!("  {p}");
        }
        return Ok(1);
    }
    println!(
        "bdd probes: {} hand-written spec(s) are connected, reachable and fully wired",
        paths.len()
    );
    Ok(0)
}

/// Port of `args_probe_check.py` — run the plugin's argument-validation errors.
pub fn check_args_probes() -> Result<i32> {
    let (rc, _) = check_args_probes_impl(false)?;
    Ok(rc)
}
fn check_args_probes_impl(silent: bool) -> Result<(i32, Option<String>)> {
    let root = repo_root();
    let plugin_path = root.join("plugins").join("bdd").join("main.rhai");
    let table_path = root.join("bench").join("bdd").join("args_probes.json");
    // A release binary has no repo checkout next to it — `repo_root()` is the
    // *build* machine's path (e.g. `D:\a\laya-workflow\laya-workflow` on the
    // Windows CI), which does not exist where the exe is unpacked. Same
    // disk-first-else-embedded pattern as `plugin_src()`: the table ships
    // inside the binary, so the gate always has its ground truth.
    let table: Value = serde_json::from_str(&match std::fs::read_to_string(&table_path) {
        Ok(s) => s,
        Err(_) => include_str!("../bench/bdd/args_probes.json").to_string(),
    })
    .with_context(|| format!("cannot read args probes table ({})", table_path.display()))?;

    // Every `throw` in the plugin, one line each, comments stripped.
    let src = plugin_src().map_err(|e| {
        eprintln!(
            "bdd args probes: no `throw` found in {} - the patterns in the args gate \
             no longer match the plugin",
            plugin_path.display()
        );
        e
    })?;
    let line_comment = Regex::new(r"//[^\n]*").unwrap();
    let code = line_comment.replace_all(&src, "").into_owned();
    let throw_re = Regex::new(r"\bthrow\b").unwrap();
    let whitespace = Regex::new(r"\s+").unwrap();
    let mut stmts: Vec<String> = Vec::new();
    for m in throw_re.find_iter(&code) {
        let end = code[m.start()..].find(';').map(|e| m.start() + e).unwrap_or(m.start() + 200);
        let stmt = &code[m.start()..end.min(code.len())];
        stmts.push(whitespace.replace_all(stmt, " ").trim().to_string());
    }
    if stmts.is_empty() {
        eprintln!(
            "bdd args probes: no `throw` found in {} - the patterns in the args gate \
             no longer match the plugin",
            plugin_path.display()
        );
        return Ok((1, None));
    }

    // Is the table still a complete, accurate map of the throws?
    let mut problems: Vec<String> = Vec::new();
    let mut claimed: BTreeSet<String> = BTreeSet::new();
    if let Some(rows) = table.get("rows").and_then(Value::as_array) {
        for row in rows {
            let name = row.get("name").and_then(Value::as_str).unwrap_or("");
            let source = row.get("source").and_then(Value::as_str).unwrap_or("");
            let hits: Vec<&String> = stmts.iter().filter(|s| s.contains(source)).collect();
            if hits.len() != 1 {
                problems.push(format!(
                    "row {name:?}: its source {source:?} matches {} of the plugin's {} \
                     throw sites, expected 1 - the message was reworded, or the anchor is \
                     now ambiguous",
                    hits.len(),
                    stmts.len()
                ));
                continue;
            }
            claimed.insert(hits[0].clone());
        }
    }
    if let Some(els) = table.get("elsewhere").and_then(Value::as_array) {
        for entry in els {
            let source = entry.get("source").and_then(Value::as_str).unwrap_or("");
            let hits: Vec<&String> = stmts.iter().filter(|s| s.contains(source)).collect();
            if hits.len() != 1 {
                problems.push(format!(
                    "elsewhere entry {source:?} matches {} throw sites, expected 1 - it no \
                     longer describes a real one",
                    hits.len()
                ));
                continue;
            }
            claimed.insert(hits[0].clone());
            if entry.get("why").and_then(Value::as_str).unwrap_or("").is_empty() {
                problems.push(format!(
                    "elsewhere entry {source:?} has no reason; a throw the table skips has \
                     to say why it is not skipped here"
                ));
            }
        }
    }
    for stmt in &stmts {
        if !claimed.contains(stmt) {
            problems.push(format!(
                "the plugin throws {stmt:?} and nothing in bench/bdd/args_probes.json claims it - \
                 add a row, or an elsewhere entry saying why it is not pinned here. An \
                 unpinned error message is one nobody reads twice."
            ));
        }
    }

    // 1: does each row actually produce its message?
    //
    // Each row is one `run --spec` that must refuse before touching a browser.
    // A single run costs ~225 ms of process start-up (the 14 MB binary plus the
    // Rhai plugin), and the table holds 16 rows, so running them one after
    // another paid ~3.6 s for a gate whose whole point is a handful of cold
    // refusals. The runs are independent and read-only - each refuses before it
    // touches anything - so they run in parallel and the verdicts are reported
    // in row order, byte for byte as before. The spec files are all written up
    // front on this thread, so the children never race on the scratch dir.
    let tmp = tempfile_build_dir()?;
    let exe = std::env::current_exe()?;
    let jobs: Vec<(String, String, String, PathBuf)> =
        if let Some(rows) = table.get("rows").and_then(Value::as_array) {
            let mut jobs = Vec::with_capacity(rows.len());
            for row in rows {
                let name = row.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let blurb = row.get("blurb").and_then(Value::as_str).unwrap_or("").to_string();
                let must_contain = row
                    .get("must_contain")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let spec = build_args_spec(&table, row);
                let spec_path = tmp.join(format!("{name}.json"));
                std::fs::write(&spec_path, serde_json::to_string(&spec).unwrap_or_default())?;
                jobs.push((name, blurb, must_contain, spec_path));
            }
            jobs
        } else {
            Vec::new()
        };
    let outcomes: Vec<Option<(i32, String)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .iter()
            .map(|(_, _, _, spec_path)| {
                let exe = exe.clone();
                let spec_arg = spec_path.to_string_lossy().into_owned();
                scope.spawn(move || {
                    CommandTimeout::run(&exe, &["run", "--spec", &spec_arg, "--state", "{}"], 30)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or(None))
            .collect()
    });
    let (ran, row_problems) = args_probe_verdicts(&jobs, &outcomes);
    problems.extend(row_problems);
    let _ = std::fs::remove_dir_all(&tmp);

    let rows = table.get("rows").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
    let elsewhere = table.get("elsewhere").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
    for p in &problems {
        eprintln!("bdd args probes: {p}");
    }
    if !problems.is_empty() {
        eprintln!("bdd args probes: {} problem(s)", problems.len());
        return Ok((1, None));
    }
    let line = format!(
        "bdd args probes: {ran}/{rows} argument errors refuse with the message they \
         claim, and all {} throw sites are accounted for ({rows} pinned here, {elsewhere} \
         declared elsewhere)",
        stmts.len()
    );
    if !silent { println!("{line}"); }
    Ok((0, Some(line)))
}

/// Turn the per-row run outcomes into `(ran, problems)`, reported in row order.
///
/// Split out of [`check_args_probes_impl`] so a regression can hand in synthetic
/// outcomes and assert every failure mode is named - the real runs spawn the
/// CLI, which the tests binary cannot do for itself. Each outcome is the exit
/// code and combined output of one `run --spec`, or `None` for a timeout.
pub fn args_probe_verdicts(
    jobs: &[(String, String, String, PathBuf)],
    outcomes: &[Option<(i32, String)>],
) -> (usize, Vec<String>) {
    let mut ran = 0usize;
    let mut problems: Vec<String> = Vec::new();
    for ((name, blurb, must_contain, _), out) in jobs.iter().zip(outcomes) {
        match out {
            None => problems.push(format!(
                "row {name:?} ({blurb}): the run timed out; this path is supposed to \
                 refuse before touching a browser, so a hang is a real defect"
            )),
            Some((rc, output)) => {
                if *rc == 0 {
                    problems.push(format!(
                        "row {name:?} ({blurb}): the run exited 0, but the plugin is \
                         supposed to refuse with {must_contain:?}"
                    ));
                } else if !output.contains(must_contain.as_str()) {
                    let first = output
                        .lines()
                        .find(|l| l.starts_with("Error:"))
                        .unwrap_or("")
                        .to_string();
                    problems.push(format!(
                        "row {name:?} ({blurb}): the run failed without saying \
                         {must_contain:?} - got: {}",
                        first.chars().take(120).collect::<String>()
                    ));
                }
                ran += 1;
            }
        }
    }
    (ran, problems)
}

/// Build the spec the args gate runs for one row of bench/bdd/args_probes.json.
fn build_args_spec(table: &Value, row: &Value) -> Value {
    let d = table.get("spec_defaults").cloned().unwrap_or_else(|| json!({}));
    let name = row.get("name").and_then(Value::as_str).unwrap_or("");
    let blurb = row.get("blurb").and_then(Value::as_str).unwrap_or("");
    let start = d.get("start").and_then(Value::as_str).unwrap_or("act");
    json!({
        "name": format!("bdd_args_{name}"),
        "dsl_version": d.get("dsl_version").and_then(Value::as_i64).unwrap_or(2),
        "description": format!("bdd must refuse this: {blurb}"),
        "start": start,
        "max_iterations": d.get("max_iterations").and_then(Value::as_i64).unwrap_or(3),
        "convergence_window": d.get("convergence_window").and_then(Value::as_i64).unwrap_or(2),
        "convergence_eps": d.get("convergence_eps").and_then(Value::as_f64).unwrap_or(0.001),
        "policy": d.get("policy").cloned().unwrap_or_else(|| json!({})),
        "capabilities": table.get("capabilities").cloned().unwrap_or_else(|| json!({})),
        "nodes": [{
            "name": start,
            "primary_q": "ok",
            "questions": {"ok": {
                "type": "choice",
                "instructions": "Did the plugin refuse this with the documented message?",
                "criteria": {"A": "yes", "B": "no"}
            }},
            "edge": {"condition": {"A": "STOP"}, "default": "STOP"},
            "state": {"keep": []},
            "action": {"kind": "call", "capability": "bdd", "with": row.get("with").cloned().unwrap_or_else(|| json!({}))}
        }]
    })
}

fn tempfile_build_dir() -> Result<PathBuf> {
    let base = std::env::temp_dir();
    let mut p;
    let mut i = 0;
    loop {
        p = base.join(format!("laya-bdd-args-{}-{}", std::process::id(), i));
        if !p.exists() {
            std::fs::create_dir_all(&p)?;
            break;
        }
        i += 1;
        if i > 100 {
            bail!("could not create a temp dir");
        }
    }
    Ok(p)
}

/// `subprocess.run(..., capture_output=True, timeout=N)` equivalent: exit code
/// + combined stdout/stderr, or `None` on timeout.
struct CommandTimeout;
impl CommandTimeout {
    fn run(exe: &Path, args: &[&str], timeout_secs: u64) -> Option<(i32, String)> {
        use std::process::{Command, Stdio};
        let child = Command::new(exe)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
        let handle = child;
        // `std::process::Child` has no timeout API, so poll on a thread.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let out = handle.wait_with_output().ok()?;
            let combined = String::from_utf8_lossy(&out.stdout).into_owned()
                + String::from_utf8_lossy(&out.stderr).as_ref();
            let _ = tx.send((out.status.code().unwrap_or(-1), combined));
            Some(())
        });
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(remaining) {
            Ok((code, out)) => Some((code, out)),
            Err(_) => None,
        }
    }
}

/// Port of `doc_check.py` — keep the numbers bdd/README.md quotes honest.
pub fn check_doc() -> Result<i32> {
    let root = repo_root();
    let readme = std::fs::read_to_string(root.join("bdd").join("README.md"))
        .with_context(|| "reading bdd/README.md")?;
    let agent = std::fs::read_to_string(root.join("bdd").join("AGENT.md"))
        .with_context(|| "reading bdd/AGENT.md")?;
    let (checked, total, n_paths, problems) = doc_check_impl(&readme, &agent)?;
    for p in &problems {
        eprintln!("bdd doc check: {p}");
    }
    if !problems.is_empty() {
        eprintln!("bdd doc check: {} problem(s)", problems.len());
        return Ok(1);
    }
    println!(
        "bdd doc check: {checked}/{total} labelled counts across bdd/README.md and bdd/AGENT.md \
         match the gates that own them, every quoted gate line is still printed, and all \
         {n_paths} quoted bdd/bench/dsl paths exist on disk"
    );
    Ok(0)
}

/// The doc gate as a pure function of the two documents. `check_doc` reads the
/// files, prints, and turns the count into an exit code; a regression hands in a
/// mutated document and watches this same code report it. The README is the
/// user-facing page, AGENT.md is the maintenance guide an agent reads before it
/// edits anything - a guide that lies about the corpus is worse than no guide.
pub fn doc_check_impl(readme: &str, agent: &str) -> Result<(usize, usize, usize, Vec<String>)> {
    let root = repo_root();
    let ws = Regex::new(r"\s+").unwrap();

    // Run the two sub-checkers once; the doc check then searches *their* output
    // for every label they own (never their own source), which is what keeps a
    // stale count from comparing against itself.
    let mut vocab_out = String::new();
    let _ = check_vocabulary_raw(&mut vocab_out);
    let mut args_out = String::new();
    let _ = check_args_probes_raw(&mut args_out);

    // How many specs bdd/features/ actually compiles to (counted by compiling,
    // the way check.sh counts — a Scenario Outline with two examples is two).
    let n_scenarios = {
        let feature_dir = root.join("bdd").join("features");
        let mut names: Vec<String> = Vec::new();
        if feature_dir.is_dir() {
            for e in std::fs::read_dir(&feature_dir)? {
                let e = e?;
                let name = e.file_name().to_string_lossy().into_owned();
                if name.ends_with(".feature") && e.path().is_file() {
                    names.push(name);
                }
            }
        }
        names.sort();
        let mut n = 0usize;
        for name in names {
            let path = feature_dir.join(name);
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(f) = parse(&text, &path.to_string_lossy()) {
                    n += f.scenarios.len();
                }
            }
        }
        n
    };

    // The coverage-ledger counts bdd/README.md quotes. They went stale once
    // (P113 hand-fixed `127/162` -> `128/163` in this very file), so derive them
    // from the same authority `dsl smoke` prints: the states table, the expect
    // table, and the coverage-ledger audit. A number nobody recomputes rots on
    // the next edit; a label that reads the live gate cannot.
    let expect_map: std::collections::BTreeMap<String, Value> =
        std::fs::read_to_string(root.join("bench").join("dsl_smoke_expect.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.as_object().cloned())
            .map(|o| o.into_iter().collect())
            .unwrap_or_default();
    let expect_keys: std::collections::BTreeSet<String> =
        expect_map.keys().cloned().collect();
    let n_expect = expect_keys.len();
    let states_map: std::collections::BTreeMap<String, Value> =
        std::fs::read_to_string(root.join("bench").join("dsl_smoke_states.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.as_object().cloned())
            .map(|o| o.into_iter().collect())
            .unwrap_or_default();
    // A <stem, {state: ...}> table: one spec per stem, one state per inner key.
    // Both halves are quoted in bdd/AGENT.md and both have drifted, so both are
    // recomputed here rather than counted by eye.
    let table_counts = |m: &std::collections::BTreeMap<String, Value>| -> (i64, i64) {
        let specs = m.len() as i64;
        let states: i64 = m
            .values()
            .map(|v| v.as_object().map(|o| o.len() as i64).unwrap_or(0))
            .sum();
        (specs, states)
    };
    let (n_expect_specs, n_expect_states) = table_counts(&expect_map);
    let (n_states_specs, n_states_states) = table_counts(&states_map);
    let dsl_root = root.join("dsl");
    let specs = crate::dsl_smoke::discover_specs(&dsl_root).unwrap_or_default();
    let n_specs = specs.len();
    let (n_offline, n_live, _) =
        crate::dsl_smoke::coverage_ledger_audit(&specs, &dsl_root, &states_map)
            .unwrap_or((0, 0, Vec::new()));

    // The sentence "Every one of the N checked-in dsl/<cat>/<name>.json specs is the
    // compiled output of a @decision feature ... proven equivalent in both
    // directions by `laya-workflow bdd round-trip`" quotes the round-trip gate, so
    // run that gate and quote *its* line: N is its total, and the "proven" half is
    // its rc. Same rule as the ledger counts above - a number nobody recomputes
    // rots on the next conversion (this one said 99 while the corpus held 163).
    let (rt_rc, rt_line) = round_trip_impl(true, &RoundTripOptions::default())?;
    let rt_line = rt_line.unwrap_or_default();
    let (rt_matched, rt_total) = Regex::new(r"bdd roundtrip: (\d+)/(\d+)")
        .unwrap()
        .captures(&rt_line)
        .map(|c| {
            (
                c[1].parse::<i64>().unwrap_or(0),
                c[2].parse::<i64>().unwrap_or(0),
            )
        })
        .unwrap_or((0, 0));

    // Collapse whitespace in both documents before searching labels.
    let flat_readme = ws.replace_all(&readme, " ").into_owned();
    let flat_agent = ws.replace_all(&agent, " ").into_owned();

    // The README also enumerates the pinned stems themselves ("+N pinned in the
    // decision workflows: a/b/c/..."). The count must equal the length of the list
    // it introduces (they drifted apart: "+67" over a 73-name list), and every name
    // must be a real pin, so a rename cannot leave a phantom behind.
    let pinned_rx = Regex::new(r"\+(\d+) pinned in the decision workflows: ([^;]+)").unwrap();
    let pinned_lists: Vec<Vec<String>> = pinned_rx
        .captures_iter(&flat_readme)
        .map(|c| {
            c[2].split('/')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .collect();
    let pinned_truth: Vec<Vec<i64>> = pinned_lists
        .iter()
        .map(|l| vec![l.len() as i64])
        .collect();

    // label regex -> (owning gate, document that quotes it). The README is the
    // user-facing page; AGENT.md is the maintenance guide, and its "current
    // state" paragraph quotes the same gates one document further from them - so
    // it drifts one edit sooner.
    const README: &str = "bdd/README.md";
    const AGENT: &str = "bdd/AGENT.md";
    let labels: Vec<(Regex, &str, &str)> = vec![
        (Regex::new(r"Current state: \*\*(\d+) scenario").unwrap(), "transpile", README),
        (Regex::new(r"(\d+) steps map correctly").unwrap(), "vocabulary", README),
        (Regex::new(r"(\d+) step arguments survive").unwrap(), "vocabulary", README),
        (Regex::new(r"(\d+) operand types survive").unwrap(), "vocabulary", README),
        (Regex::new(r"(\d+) steps still refuse to run without a page").unwrap(), "vocabulary", README),
        (Regex::new(r"(\d+) stay refused after a release").unwrap(), "vocabulary", README),
        (Regex::new(r"(\d+)/(\d+) argument errors refuse").unwrap(), "args", README),
        (Regex::new(r"all (\d+) throw sites are accounted for").unwrap(), "args", README),
        (Regex::new(r"\((\d+) pinned here, (\d+) declared elsewhere\)").unwrap(), "args", README),
        // The corpus counts, read live from the ledger authority above.
        (Regex::new(r"(\d+) of (\d+) specs carry behavioural states").unwrap(), "ledger", README),
        (Regex::new(r"(\d+) labelled expectation sets").unwrap(), "ledger", README),
        (Regex::new(r"(\d+) offline-verified \+ (\d+) live-declared = (\d+)").unwrap(), "ledger", README),
        // The round-trip authority and its enumerated pin list.
        (Regex::new(r"Every one of the (\d+) checked-in").unwrap(), "roundtrip", README),
        (
            Regex::new(r"\+(\d+) pinned in the decision workflows").unwrap(),
            "pinned",
            README,
        ),
        // AGENT.md's current-state paragraph: the two sample tables, the ledger,
        // and the two gate lines it claims are green.
        (Regex::new(r"覆盖 \*\*(\d+)\*\* 个 spec / \*\*(\d+)\*\* 个 state").unwrap(), "expect_table", AGENT),
        (Regex::new(r"注册 \*\*(\d+)\*\* 个 spec / \*\*(\d+)\*\* 个 state").unwrap(), "states_table", AGENT),
        (Regex::new(r"覆盖台账\s*`(\d+) offline-verified \+ (\d+) live-declared = (\d+) specs`").unwrap(), "ledger", AGENT),
        (Regex::new(r"`dsl smoke (\d+)/(\d+)`").unwrap(), "smoke_total", AGENT),
        (Regex::new(r"`bdd round-trip (\d+)/(\d+)`").unwrap(), "rt_total", AGENT),
    ];
    let quoted_blocks: Vec<(Regex, &str)> = vec![
        (
            Regex::new(r"(?s)```\n(bdd vocabulary:.*?)```").unwrap(),
            "vocabulary",
        ),
        (
            Regex::new(r"(?s)```\n\$ laya-workflow bdd args-probe-check\n(bdd args probes:.*?)```")
                .unwrap(),
            "args",
        ),
    ];

    let ledger_truth = |pat: &str| -> Vec<Vec<i64>> {
        match pat {
            _ if pat.starts_with("覆盖台账") => {
                vec![vec![n_offline as i64, n_live as i64, n_specs as i64]]
            }
            r"(\d+) of (\d+) specs carry behavioural states" => {
                vec![vec![n_offline as i64, n_specs as i64]]
            }
            r"(\d+) labelled expectation sets" => vec![vec![n_expect as i64]],
            r"(\d+) offline-verified \+ (\d+) live-declared = (\d+)" => {
                vec![vec![n_offline as i64, n_live as i64, n_specs as i64]]
            }
            _ => vec![],
        }
    };
    let truth: Vec<Vec<Vec<i64>>> = labels
        .iter()
        .map(|(rx, src, _)| match *src {
            "transpile" => vec![vec![n_scenarios as i64]],
            "ledger" if rx.as_str().starts_with("覆盖台账") => {
                vec![vec![n_offline as i64, n_live as i64, n_specs as i64]]
            }
            "ledger" => ledger_truth(rx.as_str()),
            "expect_table" => vec![vec![n_expect_specs, n_expect_states]],
            "states_table" => vec![vec![n_states_specs, n_states_states]],
            "smoke_total" | "rt_total" => vec![vec![n_specs as i64, n_specs as i64]],
            "roundtrip" => vec![vec![rt_total]],
            "pinned" => pinned_truth.clone(),
            "vocabulary" => rx
                .captures_iter(&vocab_out)
                .map(|c| (1..c.len()).map(|i| c[i].parse::<i64>().unwrap_or(0)).collect())
                .collect(),
            "args" => rx
                .captures_iter(&args_out)
                .map(|c| (1..c.len()).map(|i| c[i].parse::<i64>().unwrap_or(0)).collect())
                .collect(),
            _ => vec![],
        })
        .collect();

    let mut problems: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for (i, (rx, _src, doc)) in labels.iter().enumerate() {
        let text = if *doc == README { &flat_readme } else { &flat_agent };
        let in_doc: Vec<Vec<i64>> = rx
            .captures_iter(text)
            .map(|c| (1..c.len()).map(|i| c[i].parse::<i64>().unwrap_or(0)).collect())
            .collect();
        if in_doc.is_empty() {
            problems.push(format!(
                "{doc} no longer says anything matching {:?}, so the number it used \
                 to quote is no longer checked - either restore it or drop the label",
                rx.as_str()
            ));
            continue;
        }
        let want = &truth[i];
        if want.is_empty() {
            problems.push(format!(
                "{doc} quotes {:?} but the gate no longer prints anything matching \
                 it - the quote is stale",
                rx.as_str()
            ));
            continue;
        }
        checked += 1;
        if &in_doc != want {
            problems.push(format!(
                "{doc} says {} where the truth is {} for {:?} - a number nobody \
                 recomputes goes stale on the next edit",
                if in_doc.len() == 1 {
                    format!("{:?}", in_doc[0])
                } else {
                    format!("{in_doc:?}")
                },
                if want.len() == 1 {
                    format!("{:?}", want[0])
                } else {
                    format!("{want:?}")
                },
                rx.as_str()
            ));
        }
    }

    // The README's "the two are proven equivalent in both directions" is the
    // round-trip gate's claim to keep: a drift (or a dsl spec with no BDD source)
    // makes it false, and a false claim about a gate is worse than no claim.
    if rt_rc != 0 || rt_matched != rt_total || rt_total == 0 {
        problems.push(format!(
            "the README says round-trip proves the dsl tree is the compiled output of the \
             bdd tree in both directions, but the gate reports {rt_matched}/{rt_total} \
             (rc={rt_rc}) - fix the drift or drop the claim"
        ));
    }
    // A name in the pinned list that is not a real pin is a phantom: the rename
    // that orphaned it also quieted the count above it.
    for list in &pinned_lists {
        for name in list {
            if !expect_keys.contains(name) {
                problems.push(format!(
                    "the README lists {name:?} as a pinned stem, but it is not a key of \
                     bench/dsl_smoke_expect.json - a rename left a phantom behind"
                ));
            }
        }
    }

    for (block, src) in &quoted_blocks {
        let Some(m) = block.captures(&readme) else {
            problems.push(format!(
                "the README's quoted block for {:?} is gone - it was the evidence that \
                 this label is checked",
                block.as_str()
            ));
            continue;
        };
        let squash = |t: &str| ws.replace_all(t, " ").trim().to_string();
        let quoted = squash(&m[1]);
        let shown_is_full_line = match *src {
            "vocabulary" => vocab_out.lines().any(|ln| squash(ln) == quoted),
            "args" => args_out.lines().any(|ln| squash(ln) == quoted),
            _ => false,
        };
        if !shown_is_full_line {
            problems.push(format!(
                "the README quotes gate output that is not exactly what the gate prints: \
                 {:?} - stale or truncated",
                quoted.chars().take(70).collect::<String>()
            ));
        }
    }

    // P123/P125: every concrete `bdd/…` / `bench/…` / `dsl/…` path a checked-in
    // doc quotes must exist on disk. A corpus move once left
    // `bench/bdd/live/*.feature` in a report after the sources had moved to
    // `bdd/workflows/browser/`, and nothing recomputed the doc that quoted it.
    // P125 widened the scope twice: the `dsl/` tree is a spec root too (a moved
    // `dsl/…/x.json` quoted by a doc went unchecked), and the scan used to read
    // only `bench/bdd/`, so the two documents a user actually reads first -
    // `docs/bdd_quickstart.md` and `docs/*.md` - were never checked. It now reads
    // every checked-in `*.md` (skipping `.git`/`target`).
    // Globs (`bdd/*.feature`, `dsl/codegraph/*.json`) and `<placeholders>` are
    // skipped — only a literal path with a known extension is a claim about a
    // file that has to be there, so a proposed file must be spelled as a
    // placeholder. The README and AGENT.md are scanned from the strings handed in
    // (so a regression can inject a phantom) *and* from disk; the dedup key is
    // (doc, path), so scanning them twice does not double the count.
    // The `regex` crate has no look-around, so the left boundary is matched as a
    // (non-captured) consuming group; group 1 is the path.
    let path_rx = Regex::new(
        r"(?:^|[^A-Za-z0-9_./<>-])((?:bdd|bench|dsl)/[A-Za-z0-9_./-]+\.(?:feature|json|md|sh|rs|rhai|html))",
    )
    .unwrap();
    let mut path_docs: Vec<(String, String)> = vec![
        (README.to_string(), readme.to_string()),
        (AGENT.to_string(), agent.to_string()),
    ];
    {
        // Every checked-in `*.md`, not just the BDD toolchain's own. `.git` and
        // `target` are the only trees skipped (a build artifact is not a doc).
        const SKIP_DIRS: [&str; 2] = [".git", "target"];
        let mut stack = vec![root.clone()];
        let mut mds: Vec<PathBuf> = Vec::new();
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    let name = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
                    if !SKIP_DIRS.contains(&name) {
                        stack.push(p);
                    }
                } else if p.extension().and_then(|x| x.to_str()) == Some("md") {
                    mds.push(p);
                }
            }
        }
        mds.sort();
        for p in mds {
            if let Ok(t) = std::fs::read_to_string(&p) {
                let rel = p
                    .strip_prefix(&root)
                    .map(|r| r.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| p.to_string_lossy().into_owned());
                path_docs.push((rel, t));
            }
        }
    }
    let mut n_paths = 0usize;
    let mut seen_paths: std::collections::BTreeSet<(String, String)> =
        std::collections::BTreeSet::new();
    for (doc, text) in &path_docs {
        for c in path_rx.captures_iter(text) {
            let p = c[1].trim_end_matches(['.', ',', ';', ')', ':']);
            if !seen_paths.insert((doc.clone(), p.to_string())) {
                continue;
            }
            n_paths += 1;
            if !root.join(p).exists() {
                problems.push(format!(
                    "{doc} quotes {p}, which does not exist on disk - a moved file left a \
                     phantom path behind"
                ));
            }
        }
    }

    Ok((checked, labels.len(), n_paths, problems))
}

/// Run vocabulary_check but write output into `out` instead of stdout, so the
/// doc gate can diff the README's quotes against real gate output.
fn check_vocabulary_raw(out: &mut String) -> Result<i32> {
    // Run the vocabulary gate silently; its real success line is what the doc
    // gate compares the README quotes against, so capture it rather than
    // re-derive it (the impl returns it in the silent mode for exactly this).
    let (rc, line) = check_vocabulary_impl(true)?;
    if rc == 0 {
        if let Some(line) = line {
            out.push_str(&line);
            out.push('\n');
        }
    }
    Ok(rc)
}

fn check_args_probes_raw(out: &mut String) -> Result<i32> {
    let (rc, line) = check_args_probes_impl(true)?;
    if rc == 0 {
        if let Some(line) = line {
            out.push_str(&line);
            out.push('\n');
        }
    }
    Ok(rc)
}

// ── runner (port of laya-workflow bdd run) ───────────────────────────────────

/// Run the BDD documents in bdd/features/ against real Chrome over CDP.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub features: Vec<String>,
    /// Only features whose path contains this substring.
    pub filter: Option<String>,
    /// Only scenarios carrying this tag (e.g. @production).
    pub tags: Option<String>,
    pub profile: String,
    pub base_url: Option<String>,
    pub port: u16,
    pub jobs: usize,
    pub timeout_secs: u64,
    pub keep: bool,
    pub out: String,
    /// Live-fire mode: the features target real, external sites (their sidecar
    /// `policy.allow_hosts` names the hosts). No local fixture server, and the
    /// hand-written fixture probes are skipped — this is the real web, not the
    /// hermetic suite.
    pub live: bool,
    /// Write a per-scenario JSON report here (exit code, seconds, verdict).
    pub report: Option<String>,
    /// Make a missing browser an error instead of a clean skip. With it unset
    /// `bdd run` on a browserless box exits 0 with a "skipped" line (the same
    /// skip-not-fail stance as the plugin fold gate); with it set, the absence
    /// is a failure, so a gate that promises browser coverage cannot pass by
    /// doing nothing.
    pub require_browser: bool,
    /// Attach to an existing CDP endpoint instead of launching Chrome, e.g.
    /// `http://127.0.0.1:9222`. Every compiled scenario is pointed at it with
    /// `launch=false, attach=true`, so a long-lived browser another process (or
    /// a human) started is driven as-is. Implies serial execution, skips the
    /// "is a local Chrome installed?" gate (the target is remote), and requires
    /// the endpoint to be reachable — attach never spawns a browser.
    pub cdp: Option<String>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            features: Vec::new(),
            filter: None,
            tags: None,
            profile: "local".into(),
            base_url: None,
            port: 0,
            jobs: 1,
            timeout_secs: 180,
            keep: false,
            out: repo_root()
                .join("target")
                .join("bdd-specs")
                .to_string_lossy()
                .into_owned(),
            live: false,
            report: None,
            require_browser: false,
            cdp: None,
        }
    }
}

/// A socket bound to 127.0.0.1:0, dropped immediately — used to discover a
/// free port without racing a second bind on the same port.
fn free_port() -> Result<u16> {
    use std::net::TcpListener;
    let l = TcpListener::bind("127.0.0.1:0")?;
    let port = l.local_addr()?.port();
    drop(l);
    Ok(port)
}

/// A CDP port no earlier worker holds.
///
/// `free_port()` binds and releases, so two calls can hand back the same port
/// before either is bound. Serially that never mattered (there is one endpoint);
/// with `--jobs N` two workers would race for one Chrome and their scenarios would
/// fight over one endpoint. So draw again until the port is unused, and let a failing
/// draw propagate - a worker is never handed a port another worker holds. `free` is a
/// parameter so the draw can be driven from a test without opening sockets.
pub fn unique_worker_port(taken: &[u16], free: impl FnMut() -> Result<u16>) -> Result<u16> {
    let mut free = free;
    loop {
        let port = free()?;
        if !taken.contains(&port) {
            return Ok(port);
        }
    }
}

/// The cap on `--jobs 0` (auto).
///
/// Past this, more workers stop buying wall-clock: the browser stage measured
/// 11.9s / 12.5s / 10.5s / 11.5s / 11.5s / 11.8s at 4 / 4 / 6 / 8 / 12 / 16
/// workers on a 96-core box - flat within run-to-run noise - while a
/// 16-worker run flaked once in four (a probe that must refuse reported
/// success). Eight keeps the whole measured gain and stays clear of that.
/// Each extra worker cold-starts one more Chrome, so past the knee the
/// concurrency is cold starts and contention, not speed.
pub const BDD_AUTO_JOBS_CAP: usize = 8;

/// The worker count for `--jobs 0`: one per available core, capped at
/// [`BDD_AUTO_JOBS_CAP`] and at the plan (a worker with no scenario to run is
/// pure overhead). `cores` is a parameter so the rule is unit-testable without
/// depending on the host it runs on.
pub fn auto_jobs(cores: usize, plan_len: usize) -> usize {
    cores.max(1).min(BDD_AUTO_JOBS_CAP).min(plan_len.max(1))
}

/// The port of a `http[s]://host:port[/...]` CDP endpoint, if it names one.
pub fn cdp_url_port(url: &str) -> Option<u16> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let hostport = rest.split('/').next().unwrap_or("");
    match hostport.rsplit_once(':') {
        Some((_, p)) => p.parse().ok(),
        None => Some(if url.starts_with("https://") { 443 } else { 80 }),
    }
}

/// Pre-flight for `--cdp`: is the endpoint actually accepting TCP? Attach never
/// launches, so "not running" must be a clear up-front error rather than a
/// confusing failure inside the first scenario's browser step.
pub fn cdp_endpoint_reachable(url: &str) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let hostport = rest.split('/').next().unwrap_or("");
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(p) => (h, p),
            Err(_) => return false,
        },
        None => (
            hostport,
            if url.starts_with("https://") { 443 } else { 80 },
        ),
    };
    let Ok(mut addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    match addrs.next() {
        Some(addr) => TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(1500))
            .is_ok(),
        None => false,
    }
}

/// Point a spec's browser capabilities at an external CDP endpoint instead of
/// launching Chrome: `endpoint=<url>`, `launch=false`, `attach=true`.
///
/// Applied to every `chrome_cdp`/`browser` capability, because both the
/// transpiled `chrome` capability and the hand-written probe specs carry their
/// own. Pure, so the rewrite is testable without a browser.
pub fn attach_spec(spec: &mut Value, url: &str) {
    let Some(caps) = spec.get_mut("capabilities").and_then(Value::as_object_mut) else {
        return;
    };
    for def in caps.values_mut() {
        let Some(obj) = def.as_object_mut() else {
            continue;
        };
        let kind = obj.get("kind").and_then(Value::as_str).unwrap_or("");
        if kind == "chrome_cdp" || kind == "browser" {
            obj.insert("endpoint".into(), json!(url));
            obj.insert("launch".into(), json!(false));
            obj.insert("attach".into(), json!(true));
            // Every compiled browser cap also carries the launch path's own
            // options (`profile_dir`, `chrome_binary`, `startup_timeout_ms`).
            // `attach: true` returns from the runtime before it reads any of
            // them, and the capability layer refuses the dead pair on purpose
            // (P176). Leaving them here made `bdd run --cdp <url>` build a spec
            // the registry rejected, so attach mode failed *every* scenario -
            // 0 passed / 8 failed, on the P190 binary as well. Drop them, so the
            // rewritten cap is exactly what the attach branch reads.
            for dead in ["profile_dir", "chrome_binary", "startup_timeout_ms"] {
                obj.remove(dead);
            }
        }
    }
}

/// Serve `directory` over HTTP on a free localhost port for the duration of
/// the block (port of Python's `http.server` fixture server in run.py). The
/// server runs on a background thread; the returned handle stops it when
/// dropped. Handles only GET; everything else 404s. Path traversal is refused.
fn spawn_fixture_server(directory: &Path) -> Result<(String, FixtureServer)> {
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let base = format!("http://127.0.0.1:{port}");
    let root = directory.to_path_buf();

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = stop.clone();
    let handle = std::thread::spawn(move || {
        for stream in listener.incoming() {
            if stop2.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let Ok(mut stream) = stream else { continue };
            let root = root.clone();
            std::thread::spawn(move || {
                let _ = serve_one(&mut stream, &root);
            });
        }
    });

    Ok((base, FixtureServer { stop, port, handle: Some(handle) }))
}

fn serve_one(stream: &mut TcpStream, root: &Path) -> std::io::Result<()> {
    use std::io::{BufRead, BufReader, Write};
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    // Read the request line only (a HEAD/GET carries no meaningful body).
    reader.read_line(&mut request)?;
    let line = request.trim();
    let mut parts = line.split_whitespace();
    let _method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let (body, status, ctype) = route(root, target);
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut out = stream;
    out.write_all(resp.as_bytes())?;
    out.write_all(&body)?;
    out.flush()?;
    Ok(())
}

fn route(root: &Path, target: &str) -> (Vec<u8>, &'static str, &'static str) {
    let path = target.split('?').next().unwrap_or("/");
    let rel = path.trim_start_matches('/');
    let mut p = root.to_path_buf();
    for comp in rel.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            // Refuse traversal: the fixture server is hermetic and local.
            return (b"forbidden".to_vec(), "403 Forbidden", "text/plain");
        }
        p.push(comp);
    }
    if p.is_dir() {
        p.push("index.html");
    }
    match std::fs::read(&p) {
        Ok(bytes) => {
            let ctype = match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
                "html" => "text/html",
                "js" => "application/javascript",
                "css" => "text/css",
                "json" => "application/json",
                _ => "application/octet-stream",
            };
            (bytes, "200 OK", ctype)
        }
        Err(_) => (b"not found".to_vec(), "404 Not Found", "text/plain"),
    }
}

/// Stop the fixture server when dropped.
struct FixtureServer {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    port: u16,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        // Connect once so `listener.incoming()` wakes up and the thread exits.
        let _ = std::net::TcpStream::connect(format!("127.0.0.1:{}", self.port));
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Kill Chrome processes still holding one of *our* profile dirs.
/// Matching on the profile dir is precise rather than broad: these are temp
/// dirs this process just created, so nothing else can be named by them. No
/// `pkill chrome`, which would take the developer's own browser with it.
fn reap_chrome(profiles: &[String]) -> usize {
    use std::process::{Command, Stdio};
    let mut killed = 0usize;
    for profile in profiles {
        let found = Command::new("pgrep")
            .arg("-f")
            .arg(profile)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        if let Ok(out) = found {
            for pid in String::from_utf8_lossy(&out.stdout).split_whitespace() {
                // SIGTERM via `kill`, not pkill: matching on the profile dir is
                // precise, and `kill` cannot touch anything outside those PIDs.
                let _ = Command::new("kill")
                    .arg("-TERM")
                    .arg(pid)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                killed += 1;
            }
        }
    }
    killed
}

/// Remove the profile dirs, retrying while Chrome is still letting go.
/// SIGTERM is asynchronous, and a directory Chrome still has open will fail to
/// be removed. `rmtree(ignore_errors)` alone leaves the dir behind and says
/// nothing, which is how 47 of them accumulated unnoticed.
fn dispose_profiles(profiles: &[String]) -> usize {
    let mut left = 0usize;
    for profile in profiles {
        let mut remaining = 5;
        loop {
            if !Path::new(profile).is_dir() {
                break;
            }
            let _ = std::fs::remove_dir_all(profile);
            if !Path::new(profile).is_dir() {
                break;
            }
            remaining -= 1;
            if remaining == 0 {
                eprintln!("bdd: warning: could not remove {profile}");
                left += 1;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
    left
}

/// Run one scenario: `<exe> run --spec <spec> --state <json>`, with a timeout.
/// Read a child pipe to EOF, so a spec whose output exceeds the ~64KiB pipe
/// buffer cannot deadlock the child in `write()`.
fn drain_pipe<R: std::io::Read>(r: Option<R>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut r) = r {
        let _ = r.read_to_end(&mut buf);
    }
    buf
}

/// Kill the child **and everything it spawned**, then reap it.
///
/// `Child::kill` signals only the direct child. The `run --spec` child launches
/// its own Chrome, which would survive as an orphan holding the CDP port — the
/// very leak the runner's own Chrome teardown exists to avoid. The child is
/// spawned into a fresh process group precisely so the whole tree can be taken
/// down here (unix: negative pid; Windows: `taskkill /T`).
fn kill_process_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // The child leads its own group (set at spawn), so -pid reaches Chrome too.
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn run_scenario(spec_path: &Path, state: &Value, timeout_secs: u64) -> (i32, String) {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return (-1, format!("cannot locate current exe: {e}")),
    };
    let state_json = state.to_string();
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("run")
        .arg("--spec")
        .arg(spec_path)
        .arg("--state")
        .arg(&state_json)
        // One line per completed node on stderr, so a timeout can name the last
        // node the spec reached instead of only reporting that it timed out.
        .arg("--progress")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        // Own process group: `kill_process_tree` takes Chrome down with it.
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (-1, format!("spawn failed: {e}")),
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let child_pid = child.id();
    let out_h = std::thread::spawn(move || drain_pipe(stdout));
    let err_h = std::thread::spawn(move || drain_pipe(stderr));

    // A hard deadline that *reaps*: the old shape moved `Child` into a waiter
    // thread and only gave up on the receiver, so the child (and its Chrome)
    // kept running after the runner had moved on — a hung probe then burned its
    // whole budget while the machine accumulated orphans.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut status: Option<std::process::ExitStatus> = None;
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                status = Some(st);
                break;
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    timed_out = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                kill_process_tree(&mut child);
                return (-1, format!("wait failed: {e}"));
            }
        }
    }
    if timed_out {
        kill_process_tree(&mut child);
    }
    let out = out_h.join().unwrap_or_default();
    let err = err_h.join().unwrap_or_default();
    if timed_out {
        let last = String::from_utf8_lossy(&err)
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty() && l.starts_with("[laya-progress]"))
            .map(str::to_string)
            .unwrap_or_else(|| "<no node completed>".to_string());
        return (
            124,
            format!(
                "timed out after {timeout_secs}s (child pid {child_pid} and its tree killed); \
                 last node: {last}"
            ),
        );
    }
    let combined =
        String::from_utf8_lossy(&out).into_owned() + String::from_utf8_lossy(&err).as_ref();
    (status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1), combined)
}

/// The deterministic assert/cdp error lines, without the run's noise.
fn extract_failure(output: &str) -> String {
    let mut keep = Vec::new();
    for line in output.lines() {
        let low = line.to_lowercase();
        if low.contains("fail")
            || low.contains("error")
            || low.contains("not found")
            || low.contains("mismatch")
            || low.contains("timed out")
        {
            keep.push(line.trim().to_string());
        }
        if keep.len() >= 6 {
            break;
        }
    }
    keep.join("\n")
}

/// One row of the final verdict table.
#[derive(Clone)]
struct RunResult {
    label: String,
    ok: bool,
    name: String,
    rc: i32,
    out: String,
    xfail: bool,
    elapsed: f64,
}

/// The well-known Chrome/Chromium install locations `bdd run` falls back to
/// when nothing is on `$PATH` - the same list `scripts/bdd/chrome-headless.sh`
/// probes, so the preflight probe and the launcher agree on what "available"
/// means.
fn platform_chrome_defaults() -> Vec<&'static str> {
    if cfg!(target_os = "macos") {
        vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ]
    } else if cfg!(target_os = "windows") {
        vec![r"C:\Program Files\Google\Chrome\Application\chrome.exe"]
    } else {
        vec![
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
        ]
    }
}

/// Pure half of [`resolve_run_chrome`]: an explicit override wins outright (the
/// wrapper `exec`s it and fails loudly if the path is wrong), else the first
/// well-known name that resolves to a regular file under `dirs`. Split out so
/// the policy is unit-testable - and deliberately *without* the platform
/// defaults, whose presence would make the "nothing found" case depend on the
/// machine the test runs on.
pub fn resolve_run_chrome_in(over: Option<&str>, dirs: &[std::path::PathBuf]) -> Option<String> {
    if let Some(v) = over {
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    crate::capability::browser::find_chrome_binary_in(dirs)
}

/// The browser `bdd run` will actually drive. `$LAYA_BDD_REAL_CHROME` /
/// `$CHROME_BIN` (what `scripts/bdd/chrome-headless.sh` honours) beat a real
/// binary on `$PATH`, which beats the platform default install locations.
fn resolve_run_chrome() -> Option<String> {
    let over = ["LAYA_BDD_REAL_CHROME", "CHROME_BIN"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()));
    let dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(found) = resolve_run_chrome_in(over.as_deref(), &dirs) {
        return Some(found);
    }
    platform_chrome_defaults()
        .into_iter()
        .find(|d| std::path::Path::new(d).is_file())
        .map(str::to_string)
}

/// What `bdd run` does once the browser probe has finished. Pure, so the
/// skip-versus-fail contract is pinned in the offline suite rather than only on
/// a machine that happens to have no browser.
#[derive(Debug, PartialEq, Eq)]
pub enum RunBrowserGate {
    /// A browser is available: run.
    Proceed,
    /// No browser and the caller did not demand one: skip cleanly (exit 0).
    Skip,
    /// No browser but the caller demanded one (`--require-browser`): exit 1.
    Fail,
}

pub fn run_browser_gate(have_browser: bool, require_browser: bool) -> RunBrowserGate {
    match (have_browser, require_browser) {
        (true, _) => RunBrowserGate::Proceed,
        (false, true) => RunBrowserGate::Fail,
        (false, false) => RunBrowserGate::Skip,
    }
}

/// Run one planned scenario on one CDP endpoint and turn the child's exit code
/// into a verdict.
///
/// Shared by both execution paths: `--jobs 1` calls it in plan order (the
/// original loop, byte-identical output), `--jobs N` calls it from N workers,
/// one endpoint each.
#[allow(clippy::too_many_arguments)]
fn run_plan_item(
    feature: &Feature,
    scenario: &Scenario,
    spec: &Value,
    config: &Value,
    endpoint: &(u16, String),
    base_url: &str,
    out_dir: &Path,
    timeout_secs: u64,
    xfail_tag: &str,
) -> (RunResult, String) {
    let (port, profile_dir) = endpoint.clone();
    let xfail = scenario
        .tags
        .iter()
        .any(|t| t == xfail_tag || t.starts_with(&format!("{xfail_tag}(")));
    let name = format!(
        "{} :: {}",
        Path::new(&feature.path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        scenario.name
    );
    let spec_file = format!("{}.json", spec["name"].as_str().unwrap_or(""));
    let spec_path = out_dir.join(&spec_file);
    let wrote = serde_json::to_string_pretty(&public_spec(spec))
        .map_err(|e| e.to_string())
        .and_then(|t| std::fs::write(&spec_path, format!("{t}\n")).map_err(|e| e.to_string()));
    if let Err(e) = wrote {
        // The compiled spec could not be written, so the scenario can never run.
        // Report it as this scenario's failure instead of aborting the gate, so a
        // full disk names the one spec that could not be staged.
        return (
            RunResult {
                label: "FAIL".into(),
                ok: false,
                name,
                rc: -1,
                out: format!("could not stage the compiled spec {}: {e}", spec_path.display()),
                xfail,
                elapsed: 0.0,
            },
            String::new(),
        );
    }
    let mut state = Map::new();
    state.insert("base_url".into(), json!(base_url));
    state.insert("cdp_port".into(), json!(port));
    state.insert("cdp_profile".into(), json!(profile_dir));
    if let Some(init) = config.get("initial_state").and_then(Value::as_object) {
        for (k, v) in init {
            state.insert(k.clone(), v.clone());
        }
    }
    let started = std::time::Instant::now();
    let (rc, out) = run_scenario(&spec_path, &Value::Object(state), timeout_secs);
    let elapsed = started.elapsed().as_secs_f64();
    let (ok, label, why) = if xfail {
        // Failing is not enough - the run has to fail *for the stated reason*.
        // The reason lives in the tag:
        //   @expected_failure(bdd.assert: FAIL equals)
        let want = tag_reason(scenario, xfail_tag);
        if rc == 0 {
            (
                false,
                "XFAIL-WRONG-REASON",
                "the scenario passed, so the check it was written to disprove no \
                 longer fails"
                    .to_string(),
            )
        } else if want.is_none() {
            (
                false,
                "XFAIL-WRONG-REASON",
                "the tag declares no reason, so any failure at all would count".into(),
            )
        } else if !out.contains(want.as_deref().unwrap_or("")) {
            (
                false,
                "XFAIL-WRONG-REASON",
                format!(
                    "it failed, but never said {:?}, so it failed for some other reason",
                    want.unwrap()
                ),
            )
        } else {
            (true, "xfail", String::new())
        }
    } else {
        (rc == 0, if rc == 0 { "PASS" } else { "FAIL" }, String::new())
    };
    (
        RunResult {
            label: label.to_string(),
            ok,
            name,
            rc,
            out,
            xfail,
            elapsed,
        },
        why,
    )
}

pub fn run(o: &RunOptions) -> Result<i32> {
    let root = repo_root();
    let feature_dir = root.join("bdd").join("features");
    let fixture_dir = root.join("bdd").join("fixtures");
    let xfail_tag = "expected_failure";

    // Hand-written specs the runner executes alongside the compiled ones.
    let browser_base_probe = root.join("dsl/browser/browser_base_probe.json");
    let handwritten_spec = root.join("dsl/browser/bdd_assert_probe.json");
    let release_probe = root.join("dsl/browser/bdd_release_probe.json");
    let wait_probe = root.join("dsl/browser/bdd_wait_probe.json");
    let wait_until_probe = root.join("dsl/browser/bdd_wait_until_probe.json");
    let retry_probe = root.join("dsl/browser/bdd_retry_probe.json");
    const RELEASE_MUST_CONTAIN: &str = "no open target";
    const WAIT_PROBE_BUDGET_MS: i64 = 600;
    const RETRY_PROBE_ATTEMPTS: i64 = 8;
    let _ = RETRY_PROBE_ATTEMPTS;

    // Feature selection (non-recursive: bdd/features/setup/ holds `include:`
    // step lists with no `Feature:` header).
    let mut paths: Vec<PathBuf> = Vec::new();
    if o.features.is_empty() {
        if feature_dir.is_dir() {
            let mut names: Vec<String> = Vec::new();
            for ent in std::fs::read_dir(&feature_dir)?.flatten() {
                let name = ent.file_name().to_string_lossy().into_owned();
                if name.ends_with(".feature") && ent.path().is_file() {
                    names.push(name);
                }
            }
            names.sort();
            for n in names {
                paths.push(feature_dir.join(n));
            }
        }
    } else {
        paths.extend(o.features.iter().map(PathBuf::from));
    }
    if let Some(f) = &o.filter {
        paths.retain(|p| p.to_string_lossy().contains(f.as_str()));
    }
    if paths.is_empty() {
        eprintln!("no feature files selected");
        return Ok(1);
    }

    let profile = o.profile.as_str();
    let base_url = if o.live {
        Some(String::new())
    } else if profile == "production" {
        o.base_url.clone().or_else(|| std::env::var("BDD_BASE_URL").ok())
    } else {
        None
    };
    if profile == "production" && !o.live && base_url.is_none() {
        eprintln!(
            "bdd: --profile production needs --base-url (or $BDD_BASE_URL) — production \
             integration tests run against a real target, never the local fixture server"
        );
        return Ok(1);
    }

    // Attach mode: drive a CDP endpoint that already exists (a long-lived
    // browser another process or a human started) instead of launching Chrome.
    // It supersedes the "is a browser installed?" gate — the target may be
    // remote or already running — but the endpoint must be live, because attach
    // never spawns.
    let attach: Option<String> = o
        .cdp
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(url) = &attach {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            eprintln!("bdd: --cdp must be a http(s):// CDP endpoint, got {url:?}");
            return Ok(1);
        }
        if !cdp_endpoint_reachable(url) {
            eprintln!(
                "bdd: --cdp {url} is not reachable — attach drives an existing browser and \
                 never launches one; start it (e.g. `chrome --remote-debugging-port=<port>`) \
                 or drop --cdp to let Laya launch Chrome"
            );
            return Ok(1);
        }
    }

    // Is there a browser to drive at all? The suite's whole claim is that a real
    // page loaded / a real assertion held; without a browser that claim cannot be
    // made, so a browserless box skips (exit 0) rather than failing on the spawn -
    // the same skip-not-fail stance as the plugin fold gate. A box that *wants*
    // the guarantee asks for it with `--require-browser` (which turns the skip
    // into an error), so a gate that promises browser behaviour cannot pass by
    // doing nothing. Attaching counts as "have a browser": the reachability check
    // above already proved the target is up.
    match run_browser_gate(attach.is_some() || resolve_run_chrome().is_some(), o.require_browser) {
        RunBrowserGate::Proceed => {}
        RunBrowserGate::Skip => {
            println!(
                "bdd: skipped - no Chrome/Chromium found (set LAYA_BDD_REAL_CHROME or \
                 CHROME_BIN, or install one); re-run with --require-browser to make this an error"
            );
            return Ok(0);
        }
        RunBrowserGate::Fail => {
            eprintln!(
                "bdd: no Chrome/Chromium found; set LAYA_BDD_REAL_CHROME or CHROME_BIN, \
                 or install one (required by --require-browser)"
            );
            return Ok(1);
        }
    }
    let tag_filter = o
        .tags
        .as_deref()
        .map(|t| t.strip_prefix('@').unwrap_or(t).to_string());

    // Compile everything first: a broken document should cost zero browser time.
    // plan entries: (feature, scenario, spec, config)
    let chrome_wrapper = default_chrome_wrapper();
    let mut plan: Vec<(Feature, Scenario, Value, Value)> = Vec::new();
    for path in &paths {
        let feature = match load(path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("error: {}: {e}", path.display());
                return Ok(1);
            }
        };
        let config = load_config(path)?;
        let cb = config
            .get("chrome")
            .and_then(|c| c.get("chrome_binary"))
            .and_then(Value::as_str)
            .unwrap_or(&chrome_wrapper)
            .to_string();
        // P172: same guard as `build` - the production filter drops a Feature-level
        // `@production` (and every `@production` on a `@decision` feature) before
        // `compile_scenario` runs, so the misplacement would otherwise pass in
        // silence. A misplaced selector tag is a declaration nothing reads.
        if let Some(p) = selector_placement_problems(
            &feature.tags,
            &feature.tags,
            has_tag(&feature.tags, "decision"),
        )
        .first()
        {
            eprintln!(
                "error: {}: {p}",
                path.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            );
            return Ok(1);
        }
        for scenario in &feature.scenarios {
            let tags = &scenario.tags;
            if profile == "production" && !has_tag(tags, "production") {
                continue;
            }
            if let Some(tf) = &tag_filter {
                if !has_tag(tags, tf) {
                    continue;
                }
            }
            let spec = match compile_scenario(&feature, scenario, &config, &cb) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!(
                        "error: {} :: {}: {e}",
                        path.file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        scenario.name
                    );
                    return Ok(1);
                }
            };
            let mut spec = spec;
            if let Some(url) = &attach {
                attach_spec(&mut spec, url);
            }
            plan.push((feature.clone(), scenario.clone(), spec, config.clone()));
        }
    }

    if plan.is_empty() {
        let why = if profile == "production" {
            "no scenario tagged @production".to_string()
        } else if let Some(tf) = &tag_filter {
            format!("no scenario tagged @{tf}")
        } else {
            "no scenarios found".to_string()
        };
        eprintln!("error: {why} — an empty plan must not report success");
        return Ok(1);
    }

    // The worker count the gate will actually use, and a note when it is not
    // what the flags literally said. Attach is serial by construction (one
    // shared external browser); `--port` pins one endpoint; `--jobs 0` means
    // auto - one worker per core, capped (see `auto_jobs`), so the gate stops
    // being pinned to whatever number a shell default happened to hard-code.
    // Every reduction is *named* on the header line rather than applied in
    // silence: `--jobs 8` on a 3-scenario plan running under 3 workers, or an
    // auto run that silently chose 1, is exactly the gap a reader must not have
    // to infer (P191 - "it ran with N workers" and "I asked for N" differ).
    let (jobs, note) = if attach.is_some() {
        (1usize, Some("--cdp drives one external browser".to_string()))
    } else if o.port != 0 {
        if o.jobs > 1 {
            eprintln!("error: --port pins one CDP endpoint, so it only works with --jobs 1");
            return Ok(1);
        }
        (1, Some("--port pins one CDP endpoint".to_string()))
    } else if o.jobs == 0 {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        (
            auto_jobs(cores, plan.len()),
            Some(format!(
                "--jobs auto (one per core, capped at {BDD_AUTO_JOBS_CAP})"
            )),
        )
    } else if o.jobs > plan.len() {
        (
            plan.len(),
            Some(format!("--jobs {} clamped to the plan size", o.jobs)),
        )
    } else {
        (o.jobs.max(1), None)
    };

    let mut desc = if jobs == 1 {
        "serial".to_string()
    } else {
        format!("{jobs} workers")
    };
    if let Some(n) = &note {
        desc.push_str(", ");
        desc.push_str(n);
    }
    println!("bdd: {} scenario(s)  ({desc})", plan.len());
    std::fs::create_dir_all(&o.out)?;

    // One CDP endpoint per worker: each worker keeps its own port and profile
    // for every scenario it runs, which is what lets scenario 2..N adopt the
    // Chrome that scenario 1 launched instead of paying for a fresh launch
    // (measured 1.9s per scenario with a fresh profile, 0.75s with a reused one).
    //
    // In attach mode there is nothing to launch and one shared target, so the
    // single endpoint is the external URL's port and the "profile" is only a
    // Laya bookkeeping dir (never the target's own profile, which we do not own).
    let mut endpoints: Vec<(u16, String)> = Vec::new();
    for w in 0..jobs {
        let (mut port, profile) = match &attach {
            Some(url) => (
                cdp_url_port(url).unwrap_or(0),
                std::env::temp_dir().join(format!("laya-bdd-attach-w{w}-{}", std::process::id())),
            ),
            None => (
                if o.port != 0 { o.port } else { free_port()? },
                std::env::temp_dir().join(format!("laya-bdd-chrome-w{w}-{}", std::process::id())),
            ),
        };
        // `free_port()` binds and releases, so two calls can hand back the same
        // port before either is bound. Serially that never mattered; with
        // `--jobs N` racing, two workers on one port would fight over one Chrome.
        // An attach endpoint is the same port for every worker by construction,
        // so only a Laya-launched endpoint is redrawn.
        if attach.is_none() {
            let taken: Vec<u16> = endpoints.iter().map(|(p, _)| *p).collect();
            port = unique_worker_port(&taken, free_port)?;
        }
        std::fs::create_dir_all(&profile)?;
        endpoints.push((port, profile.to_string_lossy().into_owned()));
    }

    // No `browser ensure` here on purpose: each `laya-workflow run` launches
    // the headless Chrome its spec asks for, so the runner never opens a window
    // on the developer's screen.
    let mut fixture_child: Option<FixtureServer> = None;
    let base_url = if profile == "production" || o.live {
        base_url.unwrap_or_default()
    } else {
        let (base, child) = spawn_fixture_server(&fixture_dir)?;
        fixture_child = Some(child);
        println!(
            "bdd: fixtures {base}  ({})",
            rel_path(&fixture_dir.to_string_lossy())
        );
        base
    };
    if o.live {
        println!("bdd: live-fire  (real external sites; no fixtures, no probes)");
    }
    if profile == "production" {
        println!("bdd: profile production  base_url {base_url}  (no local fixtures)");
    }
    match &attach {
        Some(url) => println!("bdd: cdp attach {url}  (launch=false, attach=true; serial)\n"),
        None => {
            for (w, (port, prof)) in endpoints.iter().enumerate() {
                println!("bdd: cdp[{w}]   127.0.0.1:{port} headless  ({prof})");
            }
            println!("bdd: headless chrome\n");
        }
    }
    let mut results: Vec<RunResult> = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    let out_dir = PathBuf::from(&o.out);

    if jobs == 1 {
        // Serial execution: the original loop, same output line for line. Attach
        // mode and `--port` always land here - both pin a single endpoint.
        let mut done = 0usize;
        for (feature, scenario, spec, config) in &plan {
            let (r, why) = run_plan_item(
                feature,
                scenario,
                spec,
                config,
                &endpoints[0],
                &base_url,
                &out_dir,
                o.timeout_secs,
                xfail_tag,
            );
            done += 1;
            if !why.is_empty() {
                eprintln!("  {}: {why}", r.name);
            }
            let (label, rc) = (r.label.clone(), r.rc);
            results.push(r);
            println!(
                "  ... {label:14} {}  [{rc}]  ({done}/{})",
                results.last().unwrap().name,
                plan.len()
            );
        }
    } else {
        // `--jobs N`: N scenarios at once, each on its own CDP endpoint (worker
        // `index % N`). Every worker keeps its own port and profile, so each pays
        // one Chrome launch at most and then adopts the instance it started -
        // exactly what the serial path does, only overlapped. The verdicts are
        // stitched back in plan order, so the report is identical to a serial run
        // and only the wall clock changes.
        let slots: Vec<std::sync::Mutex<Option<(RunResult, String)>>> =
            (0..plan.len()).map(|_| std::sync::Mutex::new(None)).collect();
        let next = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for w in 0..jobs {
                let slots = &slots;
                let next = &next;
                let plans = plan.as_slice();
                let endpoints = &endpoints;
                let base_url = base_url.as_str();
                let out_dir = &out_dir;
                scope.spawn(move || loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    match plans.get(i) {
                        Some((feature, scenario, spec, config)) => {
                            let item = run_plan_item(
                                feature,
                                scenario,
                                spec,
                                config,
                                &endpoints[w],
                                base_url,
                                out_dir,
                                o.timeout_secs,
                                xfail_tag,
                            );
                            *slots[i].lock().unwrap() = Some(item);
                        }
                        None => break,
                    }
                });
            }
        });
        let mut done = 0usize;
        for (i, slot) in slots.into_iter().enumerate() {
            done += 1;
            let (r, why) = match slot.into_inner() {
                Ok(Some(v)) => v,
                _ => (
                    RunResult {
                        label: "FAIL".into(),
                        ok: false,
                        name: format!("plan item {i}: the worker that owned it did not report"),
                        rc: -1,
                        out: String::new(),
                        xfail: false,
                        elapsed: 0.0,
                    },
                    String::new(),
                ),
            };
            if !why.is_empty() {
                eprintln!("  {}: {why}", r.name);
            }
            let (label, rc) = (r.label.clone(), r.rc);
            results.push(r);
            println!(
                "  ... {label:14} {}  [{rc}]  ({done}/{})",
                results.last().unwrap().name,
                plan.len()
            );
        }
    }

    // Hand-written probes (local profile only: they exercise local fixtures).
    // In attach mode the probe spec's own `chrome` capability is rewritten to
    // the attach target (a temp copy), so the probe drives the borrowed browser
    // rather than insisting on a Laya-owned one.
    let probe_attach_dir = if attach.is_some() {
        Some(tempfile_build_dir()?)
    } else {
        None
    };
    let mut add_probe = |path: &Path,
                         state_extra: Value,
                         expect: &str,
                         must_contain: &[&str],
                         name_label: &str,
                         must_fail: bool,
                         results: &mut Vec<RunResult>|
     -> Option<String> {
        if profile == "production" || !path.is_file() {
            return None;
        }
        let (port, profile_dir) = endpoints[0].clone();
        let mut state = Map::new();
        state.insert("base_url".into(), json!(base_url));
        state.insert("cdp_port".into(), json!(port));
        state.insert("cdp_profile".into(), json!(profile_dir));
        if let Some(o) = state_extra.as_object() {
            for (k, v) in o {
                state.insert(k.clone(), v.clone());
            }
        }
        let run_path: PathBuf = match (&attach, &probe_attach_dir) {
            (Some(url), Some(dir)) => match std::fs::read_to_string(path)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            {
                Some(mut spec) => {
                    attach_spec(&mut spec, url);
                    let stem = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "probe".into());
                    let p = dir.join(format!("{stem}.attach.json"));
                    let _ = std::fs::write(&p, serde_json::to_string(&spec).unwrap_or_default());
                    p
                }
                None => path.to_path_buf(),
            },
            _ => path.to_path_buf(),
        };
        let started = std::time::Instant::now();
        let (rc, out) = run_scenario(&run_path, &Value::Object(state), o.timeout_secs);
        let elapsed = started.elapsed().as_secs_f64();
        let mut problems: Vec<String> = Vec::new();
        if !must_fail && rc != 0 {
            problems.push(format!(
                "the spec failed, but a working {} must succeed",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
        if must_fail && rc == 0 {
            problems.push(format!(
                "the spec succeeded, but it exists because {expect} is supposed to refuse"
            ));
        }
        for needle in must_contain {
            if !out.contains(needle) {
                problems.push(format!(
                    "the run failed without saying {needle:?} - a reader cannot tell a \
                     mistyped selector from a budget that was never applied"
                ));
            }
        }
        for p in &problems {
            eprintln!(
                "  {}: {p}",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            for line in extract_failure(&out).lines() {
                eprintln!("                 {line}");
            }
        }
        let ok = problems.is_empty();
        results.push(RunResult {
            label: if ok { "PASS" } else { "FAIL" }.into(),
            ok,
            name: format!("hand-written {name_label}"),
            rc,
            out,
            xfail: !ok,
            elapsed,
        });
        None
    };

    if !o.live {
    let empty_state = json!({});
    add_probe(
        &browser_base_probe,
        json!({"url": format!("{base_url}/htmx.html")}),
        "multi-step browser_base",
        &[],
        &rel_path(&browser_base_probe.to_string_lossy()),
        false,
        &mut results,
    );
    add_probe(
        &handwritten_spec,
        json!({"url": format!("{base_url}/index.html")}),
        "no transpiler",
        &[],
        &rel_path(&handwritten_spec.to_string_lossy()),
        false,
        &mut results,
    );
    add_probe(
        &release_probe,
        json!({"url": format!("{base_url}/index.html")}),
        "must refuse to release twice",
        &[RELEASE_MUST_CONTAIN],
        &rel_path(&release_probe.to_string_lossy()),
        true,
        &mut results,
    );
    let wait_needle = format!("timed out after {WAIT_PROBE_BUDGET_MS}ms");
    add_probe(
        &wait_probe,
        json!({"url": format!("{base_url}/index.html")}),
        "must time out and say what it waited for",
        &[&wait_needle, "#never-going-to-appear", "ms elapsed"],
        &rel_path(&wait_probe.to_string_lossy()),
        true,
        &mut results,
    );
    let wait_until_needle = format!("bdd.wait_until: timed out after {WAIT_PROBE_BUDGET_MS}ms");
    add_probe(
        &wait_until_probe,
        json!({"url": format!("{base_url}/index.html")}),
        "workflow wait_until must time out and say what it polled",
        &[&wait_until_needle, "!!document.querySelector('#search').disabled", "ms elapsed"],
        &rel_path(&wait_until_probe.to_string_lossy()),
        true,
        &mut results,
    );
    add_probe(
        &retry_probe,
        json!({"url": format!("{base_url}/index.html")}),
        "must win a race it cannot win in one attempt",
        &[],
        &rel_path(&retry_probe.to_string_lossy()),
        false,
        &mut results,
    );
    let _ = empty_state;
    }

    results.sort_by(|a, b| a.name.cmp(&b.name));
    let passed = results.iter().filter(|r| r.ok).count();
    for r in &results {
        println!("  {:14} {}  [{}]  {:.2}s", r.label, r.name, r.rc, r.elapsed);
        if !r.ok || r.xfail {
            let detail = extract_failure(&r.out);
            if !detail.is_empty() {
                for line in detail.lines() {
                    println!("                 {line}");
                }
            }
        }
        if !r.ok {
            failures.push((r.name.clone(), r.out.clone()));
        }
    }
    let failed = failures.len();
    let mut slow = results.clone();
    slow.sort_by(|a, b| b.elapsed.partial_cmp(&a.elapsed).unwrap_or(std::cmp::Ordering::Equal));
    println!("\nbdd: slowest scenarios (Chrome/CDP, not page work):");
    for r in slow.iter().take(3) {
        println!("  {:5.2}s  {}", r.elapsed, r.name);
    }
    println!("\nbdd: {passed} passed, {failed} failed");

    if let Some(rp) = &o.report {
        let rows: Vec<Value> = results
            .iter()
            .map(|r| json!({
                "name": r.name, "pass": r.ok, "rc": r.rc,
                "xfail": r.xfail, "seconds": (r.elapsed * 100.0).round() / 100.0
            }))
            .collect();
        let doc = json!({
            "mode": if o.live { "live" } else { "hermetic" },
            "total": results.len(),
            "passed": passed,
            "failed": failed,
            "scenarios": rows,
        });
        if let Err(e) = std::fs::write(rp, serde_json::to_string_pretty(&doc)?) {
            eprintln!("bdd: could not write report {rp}: {e}");
        } else {
            println!("bdd: report {rp}");
        }
    }

    // Whatever happens — a failing scenario, a Ctrl-C, a crash — kill the Chrome
    // processes holding our profile dirs and remove them. The runner used to
    // leak them: nine survivors were found holding renderers, and the machine's
    // load average was 58 with them and 31 without.
    let profiles: Vec<String> = endpoints.iter().map(|(_, p)| p.clone()).collect();
    reap_chrome(&profiles);
    dispose_profiles(&profiles);
    if let Some(child) = fixture_child {
        drop(child);  // FixtureServer::drop stops the thread
    }

    if !failures.is_empty() {
        println!("\n--- first failure, full output ---");
        let (name, out) = &failures[0];
        println!("{name}\n{out}");
    }
    if !o.keep {
        let _ = std::fs::remove_dir_all(&o.out);
    }
    println!("\n{}", if failed == 0 { "all green" } else { "RED" });
    Ok(if failed == 0 { 0 } else { 1 })
}

// ── authoring benchmark (`laya-workflow bdd score`) ─────────────────────────
//
// The workflow half of the vocabulary is only worth having if an agent can
// *author* a workflow from an intent quickly and correctly. That is a claim
// about a loop, so it needs numbers, and the numbers have to come from the same
// gates the loop actually hits. This subcommand scores a corpus of authoring
// tasks (`bench/bdd/tasks/*.json`) against exactly three things an author cares
// about, all measured, none asserted:
//
//   * expressiveness — does the reference workflow compile, at 100% coverage?
//   * vocabulary recall/precision — do the phrasings an author would *want* to
//     write compile, and do the ones that are not in the vocabulary get caught
//     with a named error instead of being silently mis-compiled?
//   * the `@outputs` contract — is declaring an output nobody produces really a
//     compile error, on a real feature, not just in a unit test?
//
// `--check` turns the hard ones into a gate (all 100%); `--run` adds an end to
// end pass over a real Chrome, so the benchmark also measures whether the
// workflows it scores actually run.

/// Does the deterministic vocabulary know this step? The same matcher the
/// compiler uses, so "true" means "an agent writing this gets a workflow".
pub fn vocab_matches(kind: &str, text: &str) -> bool {
    match_step(kind, text).is_ok()
}

/// One `(kind, text)` step probe out of a task's JSON.
fn parse_step_probe(v: &Value) -> (String, String) {
    (
        v.get("kind").and_then(Value::as_str).unwrap_or("when").to_string(),
        v.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
    )
}

pub struct ScoreOptions {
    /// Turn the hard targets (compile, coverage, recall, precision, contract)
    /// into a gate: any shortfall exits non-zero.
    pub check: bool,
    /// Also run every task's reference workflow against real Chrome, so the
    /// benchmark covers execution and not only compilation.
    pub run: bool,
    /// Write the metrics as JSON here.
    pub json: Option<String>,
    /// Score only these task ids (default: every task under bench/bdd/tasks).
    pub tasks: Vec<String>,
}

impl Default for ScoreOptions {
    fn default() -> Self {
        Self { check: false, run: false, json: None, tasks: Vec::new() }
    }
}

struct TaskScore {
    id: String,
    intent: String,
    feature: String,
    compile_ok: bool,
    compile_error: Option<String>,
    coverage: f64,
    declared_outputs: usize,
    known_ok: usize,
    known_total: usize,
    unknown_ok: usize,
    unknown_total: usize,
    contract_ok: bool,
    reference_steps: usize,
    unique_steps: usize,
    e2e: Option<bool>,
    /// Wall-clock seconds the reference workflow took end to end (`--run`).
    e2e_secs: f64,
}

pub fn score(o: &ScoreOptions) -> Result<i32> {
    let root = repo_root();
    let tasks_dir = root.join("bench").join("bdd").join("tasks");
    if !tasks_dir.is_dir() {
        bail!("no authoring corpus at {} - nothing to score", tasks_dir.display());
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&tasks_dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
        .collect();
    files.sort();

    let mut tasks: Vec<TaskScore> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    let mut feature_paths: Vec<PathBuf> = Vec::new();

    for path in &files {
        let meta: Value = serde_json::from_str(&std::fs::read_to_string(path)?)
            .with_context(|| format!("parsing {}", path.display()))?;
        let id = meta.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        if !o.tasks.is_empty() && !o.tasks.iter().any(|t| t == &id) {
            continue;
        }
        let intent = meta.get("intent").and_then(Value::as_str).unwrap_or("").to_string();
        let feature_rel = meta.get("feature").and_then(Value::as_str).unwrap_or("").to_string();
        let feature_path = root.join(&feature_rel);
        let feature_text = std::fs::read_to_string(&feature_path)
            .with_context(|| format!("reading {feature_rel}"))?;
        let feature = parse(&feature_text, &feature_path.to_string_lossy())?;

        // expressiveness: every scenario compiles. A `@decision` feature is one
        // decision-graph workflow (each Scenario = one node), so it compiles
        // through `compile_decision`; the browser-step path (`compile_scenario`)
        // rejects decision steps by design, so scoring a decision task through
        // it would report every reference workflow as broken. The generated
        // decision spec must also pass the same `validate` gate `bdd build`
        // runs, or a task whose graph never validates would pass this half.
        let mut compile_ok = true;
        let mut compile_error = None;
        if has_tag(&feature.tags, "decision") {
            match compile_decision(&feature, &json!({})) {
                Ok(spec) => {
                    if let Err(e) = crate::spec::from_spec_with_dir(&spec, None, None) {
                        compile_ok = false;
                        compile_error = Some(format!("validate: {e:#}"));
                    }
                }
                Err(e) => {
                    compile_ok = false;
                    compile_error = Some(format!("{}: {e:#}", feature.name));
                }
            }
        } else {
            for sc in &feature.scenarios {
                if let Err(e) = compile_scenario(&feature, sc, &json!({}), "chrome-headless.sh") {
                    compile_ok = false;
                    compile_error = Some(format!("{}: {e:#}", sc.name));
                    break;
                }
            }
        }
        let cov = feature_coverage(&feature);
        let declared: usize = feature.scenarios.iter().map(|s| scenario_outputs(s).len()).sum();

        let known = meta.get("known_steps").and_then(Value::as_array).cloned().unwrap_or_default();
        let unknown = meta.get("unknown_steps").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut known_ok = 0usize;
        for k in &known {
            let (kind, text) = parse_step_probe(k);
            if vocab_matches(&kind, &text) {
                known_ok += 1;
            } else {
                problems.push(format!("{id}: a step the corpus says is supported does not compile: {kind} {text:?}"));
            }
        }
        let mut unknown_ok = 0usize;
        for k in &unknown {
            let (kind, text) = parse_step_probe(k);
            match match_step(&kind, &text) {
                Ok(_) => problems.push(format!(
                    "{id}: a step the corpus says is out of vocabulary compiled silently: {kind} {text:?} - the worst drift shape"
                )),
                Err(e) => {
                    let msg = format!("{e:#}");
                    if msg.contains("unknown") {
                        unknown_ok += 1;
                    } else {
                        problems.push(format!("{id}: out-of-vocabulary step failed for the wrong reason: {msg}"));
                    }
                }
            }
        }

        // @outputs contract: dropping the step that produces a declared output
        // must turn into a compile error, on the real feature text.
        let mut contract_ok = true;
        if let Some(drop) = meta.get("drop_output_step").and_then(Value::as_str) {
            let mutated: String = feature_text
                .lines()
                .filter(|l| !l.contains(drop))
                .collect::<Vec<_>>()
                .join("\n");
            if mutated == feature_text {
                contract_ok = false;
                problems.push(format!("{id}: drop_output_step {drop:?} matched no line - the mutation is stale"));
            } else {
                let mf = parse(&mutated, &feature_path.to_string_lossy())?;
                let caught = mf
                    .scenarios
                    .iter()
                    .any(|sc| compile_scenario(&mf, sc, &json!({}), "chrome-headless.sh").is_err());
                if !caught {
                    contract_ok = false;
                    problems.push(format!(
                        "{id}: removing the step that produces a declared @outputs key still compiled - the contract is not enforced"
                    ));
                }
            }
        }

        let all_steps: Vec<&Step> = feature
            .background
            .iter()
            .chain(feature.scenarios.iter().flat_map(|s| s.steps.iter()))
            .collect();
        let unique: BTreeSet<String> = all_steps
            .iter()
            .map(|s| format!("{} {}", s.kind, s.text))
            .collect();

        feature_paths.push(feature_path.clone());
        tasks.push(TaskScore {
            id,
            intent,
            feature: feature_rel,
            compile_ok,
            compile_error,
            coverage: cov.coverage,
            declared_outputs: declared,
            known_ok,
            known_total: known.len(),
            unknown_ok,
            unknown_total: unknown.len(),
            contract_ok,
            reference_steps: all_steps.len(),
            unique_steps: unique.len(),
            e2e: None,
            e2e_secs: 0.0,
        });
    }

    if tasks.is_empty() {
        bail!("no authoring tasks selected");
    }

    // optional e2e: run every task feature once. Two halves, because `bdd run`'s
    // browser compiler rejects @decision steps by design (sending them there
    // reported every decision workflow as "failed end to end"):
    //   * browser tasks run against real Chrome via one `bdd run`,
    //   * @decision tasks run offline through the heuristic engine on the
    //     states registered in bench/dsl_smoke_states.json for the matching
    //     dsl stem. The round-trip gate already proves the compiled spec == the
    //     checked-in dsl spec, so this is the authored feature actually running
    //     through the engine, not a stale artifact.
    if o.run {
        let states: Value = {
            let p = repo_root().join("bench").join("dsl_smoke_states.json");
            serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_default()).unwrap_or(Value::Null)
        };
        let expect: Value = {
            let p = repo_root().join("bench").join("dsl_smoke_expect.json");
            serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_default()).unwrap_or(Value::Null)
        };
        let backend = crate::backend::HeuristicBackend;

        // ── browser half ──
        let browser_paths: Vec<PathBuf> = feature_paths
            .iter()
            .filter(|p| {
                std::fs::read_to_string(p)
                    .map(|t| !t.lines().any(|l| l.starts_with("@decision")))
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        let mut chrome_text = String::new();
        if !browser_paths.is_empty() {
            let exe = std::env::current_exe()?;
            let mut args: Vec<String> = vec!["bdd".into(), "run".into()];
            for p in &browser_paths {
                args.push(p.to_string_lossy().into_owned());
            }
            // Every feature path is positional after `run`.
            let out = std::process::Command::new(exe).args(&args).output()?;
            chrome_text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }

        for t in tasks.iter_mut() {
            let feat_rel = t.feature.clone();
            let feat_path = repo_root().join(&feat_rel);
            let name = Path::new(&feat_rel)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let src = std::fs::read_to_string(&feat_path).unwrap_or_default();
            let is_decision = src.lines().any(|l| l.starts_with("@decision"));
            if is_decision {
                // ── decision half: compile the authored feature, run the
                // compiled spec offline on every registered state ──
                let mut runnable = 0usize;
                let mut fails = 0usize;
                let mut secs = 0.0;
                match parse(&src, &feat_rel).and_then(|f| {
                    compile_decision(&f, &json!({}))
                }) {
                    Ok(spec) => {
                        if let Ok(wf) = crate::spec::from_spec_with_dir(&spec, None, None) {
                            let stem = Path::new(&feat_rel)
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            if let Some(smap) = states.get(&stem).and_then(|v| v.as_object()) {
                                for (label, st) in smap {
                                    runnable += 1;
                                    match wf.run(&backend, st) {
                                        Ok(out) => {
                                            secs += out.trace.total_latency_ms / 1000.0;
                                            // P30: assert the authored feature's
                                            // decision outputs against the same
                                            // pinned expect as dsl smoke — an
                                            // authored feature that routes to the
                                            // wrong node but does not error now
                                            // fails e2e instead of passing.
                                            if let Some(emap) = expect
                                                .get(&stem)
                                                .and_then(|v| v.as_object())
                                            {
                                                if let Some(want) = emap.get(label) {
                                                    for msg in crate::dsl_smoke::check_expect(
                                                        &out, want,
                                                    ) {
                                                        fails += 1;
                                                        problems.push(format!(
                                                            "{}: state {label:?} decided wrong: {msg}",
                                                            t.id
                                                        ));
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            // Offline detection: a state that
                                            // errors only because it needs a
                                            // network endpoint that is down
                                            // (local mocks absent / no network)
                                            // is skipped, not a regression — the
                                            // structural half is the offline
                                            // proof, and the integration job
                                            // runs it for real.
                                            if crate::dsl_smoke::network_bound_offline(&spec) {
                                                eprintln!(
                                                    "  {id}: state {label:?} skipped offline (network endpoint(s) unreachable), structural proof only",
                                                    id = t.id,
                                                    label = label
                                                );
                                                continue;
                                            }
                                            fails += 1;
                                            problems.push(format!(
                                                "{}: state {label:?} failed offline: {e:#}",
                                                t.id
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        problems.push(format!("{}: reference workflow failed to compile: {e:#}", t.id));
                        fails += 1;
                    }
                }
                if runnable == 0 {
                    // No states registered for this stem: the offline structural
                    // half (compile + validate + 100% coverage) is the proof.
                    t.e2e = Some(t.compile_ok && t.coverage >= 100.0);
                } else {
                    t.e2e = Some(fails == 0);
                }
                t.e2e_secs = secs;
            } else {
                // ── browser half: read the Chrome report for this feature ──
                let mut passes = 0usize;
                let mut fails = 0usize;
                for line in chrome_text.lines() {
                    if line.contains(&name) {
                        if line.contains("FAIL") || line.contains("XFAIL-WRONG-REASON") {
                            fails += 1;
                        } else if line.contains("PASS") || line.contains("xfail") {
                            passes += 1;
                        }
                        // The runner prints each scenario's wall-clock as the
                        // last token (`1.19s`). Summing it per feature is the
                        // one honest *time* number the benchmark has: authoring
                        // time needs an author, but execution time is measured.
                        if let Some(tok) = line.split_whitespace().last() {
                            if let Some(v) = tok.strip_suffix('s').and_then(|n| n.parse::<f64>().ok()) {
                                t.e2e_secs += v;
                            }
                        }
                    }
                }
                t.e2e = Some(fails == 0 && passes > 0);
                if fails > 0 {
                    problems.push(format!("{}: the reference workflow failed end to end", t.id));
                }
            }
        }
    }

    // ── aggregate ───────────────────────────────────────────────────────────
    let n = tasks.len();
    let compiled = tasks.iter().filter(|t| t.compile_ok).count();
    let min_cov = tasks.iter().map(|t| t.coverage).fold(f64::INFINITY, f64::min);
    let contracts = tasks.iter().filter(|t| t.contract_ok).count();
    let known_ok: usize = tasks.iter().map(|t| t.known_ok).sum();
    let known_total: usize = tasks.iter().map(|t| t.known_total).sum();
    let unknown_ok: usize = tasks.iter().map(|t| t.unknown_ok).sum();
    let unknown_total: usize = tasks.iter().map(|t| t.unknown_total).sum();
    let steps: usize = tasks.iter().map(|t| t.reference_steps).sum();
    let unique: usize = tasks.iter().map(|t| t.unique_steps).sum();
    let draft_fixes: usize = tasks.iter().map(|t| t.unknown_total).sum();
    let avg_steps = steps as f64 / n as f64;

    println!("bdd score: {n} authoring task(s)");
    for t in &tasks {
        let e2e = match t.e2e {
            Some(true) => format!(" e2e=PASS {:.2}s", t.e2e_secs),
            Some(false) => " e2e=FAIL".to_string(),
            None => String::new(),
        };
        println!(
            "  {:<22} compile={} coverage={:5.1}% outputs={} known={}/{} unknown={}/{} contract={}{}",
            t.id,
            if t.compile_ok { "ok" } else { "FAIL" },
            t.coverage,
            t.declared_outputs,
            t.known_ok,
            t.known_total,
            t.unknown_ok,
            t.unknown_total,
            if t.contract_ok { "ok" } else { "FAIL" },
            e2e
        );
        if let Some(e) = &t.compile_error {
            println!("      compile error: {e}");
        }
    }
    println!(
        "bdd score: expressiveness {compiled}/{n} compile at min coverage {min_cov:.1}%, \
         @outputs contract {contracts}/{n} mutations caught"
    );
    println!(
        "bdd score: vocabulary recall {known_ok}/{known_total}, precision {unknown_ok}/{unknown_total} \
         (every out-of-vocabulary step refused by name, never mis-compiled)"
    );
    println!(
        "bdd score: authoring cost {steps} steps ({unique} unique), avg {avg_steps:.1} steps/workflow, \
         {draft_fixes} naive-draft fix(es)"
    );
    let total_e2e: f64 = tasks.iter().map(|t| t.e2e_secs).sum();
    if o.run {
        println!(
            "bdd score: end-to-end {} task(s) in {total_e2e:.2}s of scenario time (decision offline heuristic + browser real Chrome)",
            tasks.iter().filter(|t| t.e2e == Some(true)).count()
        );
    }

    if let Some(path) = &o.json {
        let report = json!({
            "tasks": tasks.iter().map(|t| json!({
                "id": t.id, "intent": t.intent, "feature": t.feature,
                "compile_ok": t.compile_ok, "coverage": t.coverage,
                "declared_outputs": t.declared_outputs,
                "known_ok": t.known_ok, "known_total": t.known_total,
                "unknown_ok": t.unknown_ok, "unknown_total": t.unknown_total,
                "contract_ok": t.contract_ok, "reference_steps": t.reference_steps,
                "unique_steps": t.unique_steps, "e2e": t.e2e, "e2e_secs": t.e2e_secs
            })).collect::<Vec<_>>(),
            "aggregate": {
                "tasks": n, "compiled": compiled, "min_coverage": min_cov,
                "contracts_caught": contracts,
                "vocabulary_recall": [known_ok, known_total],
                "vocabulary_precision": [unknown_ok, unknown_total],
                "steps": steps, "unique_steps": unique,
                "avg_steps_per_workflow": avg_steps,
                "naive_draft_fixes": draft_fixes,
                "e2e_secs_total": total_e2e
            }
        });
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("bdd score: wrote {path}");
    }

    let hard_ok = compiled == n
        && min_cov >= 100.0
        && contracts == n
        && known_ok == known_total
        && unknown_ok == unknown_total;
    if !problems.is_empty() {
        for p in &problems {
            eprintln!("bdd score: {p}");
        }
    }
    if o.check && !hard_ok {
        eprintln!("bdd score: FAIL - an authoring target is below the bar");
        return Ok(1);
    }
    if !o.check {
        println!("bdd score: (informational - pass --check to gate)");
    }
    Ok(if problems.is_empty() { 0 } else { 1 })
}

// ── round-trip gate (`laya-workflow bdd roundtrip`) ──────────────────────────
//
// Every `@decision` feature in `bdd/workflows/` must compile back to the same
// decision-graph spec as its checked-in `dsl/<cat>/<name>.json` counterpart.
// The dsl files are authoritative (the features were converted from them), so a
// feature that compiles at 100% coverage can still *drift* — a `max_iterations`
// line forgotten, a `truthy`/`falsy` annotation dropped, a question reworded.
// The compile gate (`bdd build`) cannot see that; this gate can, and it is what
// makes "bdd == dsl" a checked invariant rather than a claim.
//
// Comparison is semantic, not byte-equal: fields the engine defaults are
// normalised before the diff so a dsl that omits an engine-defaulted key is
// not flagged against a compiler that spells the default out:
//   * `with: {}` on a `call` action            == absent `with`
//   * `has_action`                             == derived from `action`
//   * `min_confidence: 0.0`                    == absent
//   * empty `instructions` on a question       == absent
//   * edge `condition` entries whose target is the `default` == folded into
//     the default (both spellings route identically)
//   * threshold rule `when.question` equal to the action's `question` == implied
//   * heuristic `p_hit`/`p_miss`/`p_mid`/`p_fallback` that equal the engine
//     default (0.9/0.1/0.5/0.6)                == absent
//   * `description` / `convergence_*`          == not compared (prose / tuning)


// ── bdd import: a spec → a `.feature` that compiles back to it ──────────────
//
// `bdd transpile` compiles a `.feature` into a spec and `bdd round-trip` proves
// a checked-in pair agrees. `bdd import` is the other direction: given a spec -
// from this repo or from another (a legacy workflow that was never written in
// BDD) - emit a `@decision` `.feature` that compiles back to the very same
// graph. `bdd import-check` holds that exactly for every checked-in spec, which
// proves the BDD surface is at least as expressive as the JSON DSL, and is the
// tool that lifts any known workflow into BDD mode.

/// A Gherkin table cell must not hold a raw `|` (it splits the row); escape it
/// the way `cells()` un-escapes it.
fn imp_cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// `"..."` with exactly the escapes `quoted_tokens_escaped` reads back.
fn imp_q(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Every imported fragment must land on one Gherkin line.
fn imp_line(what: &str, s: &str) -> Result<()> {
    if s.contains('\n') || s.contains('\r') {
        bail!("cannot import: {what} contains a newline, which has no Gherkin spelling: {s:?}");
    }
    Ok(())
}

/// A value that goes inside `"..."` where the step grammar uses `[^"]+`.
fn imp_tok(what: &str, s: &str) -> Result<()> {
    imp_line(what, s)?;
    if s.contains('"') {
        bail!("cannot import: {what} contains a double quote: {s:?}");
    }
    Ok(())
}

fn imp_str<'a>(what: &str, v: Option<&'a Value>) -> Result<&'a str> {
    match v.and_then(Value::as_str) {
        Some(s) => {
            imp_line(what, s)?;
            Ok(s)
        }
        None => bail!(
            "cannot import: {what} must be a string, got {}",
            v.map(Value::to_string).unwrap_or_else(|| "nothing".to_string())
        ),
    }
}

/// Render a float so the step regexes (`\d+(?:\.\d+)?`) read it back.
fn imp_num(v: f64) -> Result<String> {
    let s = format!("{v}");
    if s.contains('e') || s.contains('E') {
        bail!("cannot import: {v} needs scientific notation, which the step grammar does not accept");
    }
    Ok(s)
}

fn imp_f64(what: &str, v: Option<&Value>) -> Result<f64> {
    v.and_then(Value::as_f64).ok_or_else(|| {
        anyhow!(
            "cannot import: {what} must be a number, got {}",
            v.map(Value::to_string).unwrap_or_else(|| "nothing".to_string())
        )
    })
}

/// A quoted token list, `"a" "b"`, exactly as `quoted_tokens` reads it back.
/// `quoted_tokens` cannot carry a `"` inside a token, so refuse one.
fn imp_tokens(what: &str, xs: &[Value]) -> Result<String> {
    let mut toks = Vec::new();
    for x in xs {
        let s = imp_str(what, Some(x))?;
        imp_tok(what, s)?;
        toks.push(imp_q(s));
    }
    Ok(toks.join(" "))
}

/// Like [`imp_tokens`] but for the places the compiler reads back with
/// `quoted_tokens_escaped`, where a token *may* contain a `"` (written `\"`).
fn imp_tokens_esc(what: &str, xs: &[Value]) -> Result<String> {
    let mut toks = Vec::new();
    for x in xs {
        let s = imp_str(what, Some(x))?;
        imp_line(what, s)?;
        toks.push(imp_q(s));
    }
    Ok(toks.join(" "))
}

/// The steps of one Scenario, grouped so the emitter can spell the first step
/// of each group with its real keyword and every later one with `And` (which
/// the parser makes inherit the group's kind). A step is its body line plus any
/// table rows that attach to it.
#[derive(Default)]
struct ImpSteps {
    given: Vec<(String, Vec<String>)>,
    when: Vec<(String, Vec<String>)>,
    then: Vec<(String, Vec<String>)>,
}

impl ImpSteps {
    fn push(&mut self, kind: &str, body: String, table: Vec<String>) {
        match kind {
            "given" => self.given.push((body, table)),
            "when" => self.when.push((body, table)),
            _ => self.then.push((body, table)),
        }
    }

    fn render(&self) -> String {
        let mut out = String::new();
        for (kw, group) in [("Given", &self.given), ("When", &self.when), ("Then", &self.then)] {
            for (i, (body, table)) in group.iter().enumerate() {
                let keyword = if i == 0 { kw } else { "And" };
                out.push_str(&format!("    {keyword} {body}\n"));
                for row in table {
                    out.push_str(&format!("      {row}\n"));
                }
            }
        }
        out
    }
}

/// One `a <type> question ...` plus the heuristic step that configures it.
fn imp_question(st: &mut ImpSteps, qname: &str, q: &Map<String, Value>) -> Result<()> {
    imp_tok("question name", qname)?;
    let qtype = q.get("type").and_then(Value::as_str).unwrap_or("choice");
    let instr = q.get("instructions").and_then(Value::as_str).unwrap_or("");
    imp_line("question instructions", instr)?;
    match qtype {
        "choice" => {
            let criteria = q.get("criteria").and_then(Value::as_object).ok_or_else(|| {
                anyhow!("cannot import: choice question {qname:?} has no criteria object")
            })?;
            let mut table = vec!["| answer | meaning |".to_string()];
            for (k, v) in criteria {
                imp_line("choice option", k)?;
                let meaning = imp_str("choice meaning", Some(v))?;
                table.push(format!("| {} | {} |", imp_cell(k), imp_cell(meaning)));
            }
            st.push("given", format!("a choice question \"{qname}\" asks \"{instr}\""), table);
        }
        "score" => {
            let levels = q.get("criteria").and_then(Value::as_array).ok_or_else(|| {
                anyhow!("cannot import: score question {qname:?} has no criteria array")
            })?;
            let mut table = vec!["| level |".to_string()];
            for lv in levels {
                let s = imp_str("score level", Some(lv))?;
                table.push(format!("| {} |", imp_cell(s)));
            }
            st.push("given", format!("a score question \"{qname}\" asks \"{instr}\""), table);
        }
        "noul" => {
            let mut table = Vec::new();
            for key in ["truthy", "falsy"] {
                if let Some(v) = q.get(key) {
                    let v = imp_str("noul text", Some(v))?;
                    table.push(format!("| {key} | {} |", imp_cell(v)));
                }
            }
            st.push("given", format!("a noul question \"{qname}\" asks \"{instr}\""), table);
        }
        other => bail!("cannot import: question {qname:?} has unsupported type {other:?}"),
    }
    if let Some(h) = q.get("heuristic") {
        if !h.is_object() {
            bail!("cannot import: question {qname:?} heuristic is not an object");
        }
        // Fill the defaults the engine applies when a key is absent (the same
        // normalisation the round-trip comparison uses), so the emitted step
        // spells out exactly the probabilities the compiled spec will carry.
        let h = rt_norm_heuristic(h);
        let h = h.as_object().expect("rt_norm_heuristic always returns an object");
        imp_heuristic(st, qname, h)?;
    }
    Ok(())
}

/// The heuristic step that attaches to the question just emitted.
fn imp_heuristic(st: &mut ImpSteps, qname: &str, h: &Map<String, Value>) -> Result<()> {
    let s = |k: &str| h.get(k).and_then(Value::as_str);
    let has = |k: &str| h.contains_key(k);
    if let Some(f) = s("answer_field") {
        imp_tok("heuristic.answer_field", f)?;
        st.push("given", format!("the answer comes from the field \"{f}\""), vec![]);
        return Ok(());
    }
    if let Some(a) = s("answer") {
        imp_tok("heuristic.answer", a)?;
        st.push("given", format!("the answer is always \"{a}\""), vec![]);
        return Ok(());
    }
    if let Some(field) = s("extract_numeric") {
        imp_tok("heuristic.extract_numeric", field)?;
        let miss = imp_f64("heuristic.score_miss", h.get("score_miss"))?;
        st.push(
            "given",
            format!(
                "score \"{qname}\" from the numeric field \"{field}\" with miss {}",
                imp_num(miss)?
            ),
            vec![],
        );
        return Ok(());
    }
    if has("match_rules") {
        let field = s("field")
            .ok_or_else(|| anyhow!("cannot import: match_rules heuristic needs a field"))?;
        imp_tok("heuristic.field", field)?;
        let hit = imp_f64("heuristic.p_hit", h.get("p_hit"))?;
        let rules = h
            .get("match_rules")
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow!("cannot import: match_rules is not an object"))?;
        let body = match s("fallback") {
            Some(fb) => format!(
                "answer by rules on the field \"{field}\" with hit {} and fallback \"{fb}\"",
                imp_num(hit)?
            ),
            None => format!(
                "answer by rules on the field \"{field}\" with hit {}",
                imp_num(hit)?
            ),
        };
        let mut table = vec!["| label | needles |".to_string()];
        for (label, needles) in rules {
            imp_line("match_rules label", label)?;
            let needles = needles
                .as_array()
                .ok_or_else(|| anyhow!("cannot import: match_rules[{label}] is not an array"))?;
            let toks = imp_tokens_esc("match_rules needle", needles)?;
            table.push(format!("| {} | {} |", imp_cell(label), imp_cell(&toks)));
        }
        st.push("given", body, table);
        return Ok(());
    }
    let any = h.get("match_any").and_then(Value::as_array);
    let mid_any = h.get("match_mid_any").and_then(Value::as_array);
    let field = s("field");
    if has("score_hit") && has("score_mid") && has("score_miss") {
        let (any, mid_any) = match (any, mid_any) {
            (Some(a), Some(m)) => (a, m),
            _ => bail!("cannot import: score-bands heuristic needs match_any and match_mid_any"),
        };
        if any.len() != 1 || mid_any.len() != 1 {
            bail!("cannot import: score-bands heuristic needs exactly one high and one mid needle");
        }
        let field = field.ok_or_else(|| anyhow!("cannot import: score-bands heuristic needs a field"))?;
        imp_tok("heuristic.field", field)?;
        let high = imp_str("heuristic.match_any[0]", Some(&any[0]))?;
        let mid = imp_str("heuristic.match_mid_any[0]", Some(&mid_any[0]))?;
        imp_tok("heuristic.match_any[0]", high)?;
        imp_tok("heuristic.match_mid_any[0]", mid)?;
        let hit = imp_f64("heuristic.score_hit", h.get("score_hit"))?;
        let midv = imp_f64("heuristic.score_mid", h.get("score_mid"))?;
        let miss = imp_f64("heuristic.score_miss", h.get("score_miss"))?;
        st.push(
            "given",
            format!(
                "score \"{qname}\" when the field \"{field}\" contains \"{high}\" scoring {} and \"{mid}\" scoring {} miss {}",
                imp_num(hit)?,
                imp_num(midv)?,
                imp_num(miss)?
            ),
            vec![],
        );
        return Ok(());
    }
    if let Some(any) = any {
        let hit = imp_f64("heuristic.p_hit", h.get("p_hit"))?;
        let miss = imp_f64("heuristic.p_miss", h.get("p_miss"))?;
        let needles = imp_tokens("heuristic.match_any[]", any)?;
        if needles.is_empty() {
            bail!("cannot import: match_any heuristic has no needles");
        }
        let body = match field {
            Some(f) => {
                imp_tok("heuristic.field", f)?;
                format!(
                    "answer \"{qname}\" when the field \"{f}\" contains {needles} with hit {} and miss {}",
                    imp_num(hit)?,
                    imp_num(miss)?
                )
            }
            None => format!(
                "answer \"{qname}\" when the text contains {needles} with hit {} and miss {}",
                imp_num(hit)?,
                imp_num(miss)?
            ),
        };
        st.push("given", body, vec![]);
        if let Some(mid_any) = mid_any {
            let f = field
                .ok_or_else(|| anyhow!("cannot import: match_mid_any heuristic needs a field"))?;
            let mid = imp_f64("heuristic.p_mid", h.get("p_mid"))?;
            let needles = imp_tokens("heuristic.match_mid_any[]", mid_any)?;
            st.push(
                "given",
                format!(
                    "answer \"{qname}\" with mid {} when the field \"{f}\" contains {needles}",
                    imp_num(mid)?
                ),
                vec![],
            );
        }
        return Ok(());
    }
    if let Some(patterns) = h.get("match_regex").and_then(Value::as_array) {
        let hit = imp_f64("heuristic.p_hit", h.get("p_hit"))?;
        let miss = imp_f64("heuristic.p_miss", h.get("p_miss"))?;
        let pats = imp_tokens_esc("heuristic.match_regex[]", patterns)?;
        if pats.is_empty() {
            bail!("cannot import: match_regex heuristic has no patterns");
        }
        let body = match field {
            Some(f) => {
                imp_tok("heuristic.field", f)?;
                format!(
                    "answer \"{qname}\" when the field \"{f}\" matches {pats} with hit {} and miss {}",
                    imp_num(hit)?,
                    imp_num(miss)?
                )
            }
            None => format!(
                "answer \"{qname}\" when the text matches {pats} with hit {} and miss {}",
                imp_num(hit)?,
                imp_num(miss)?
            ),
        };
        st.push("given", body, vec![]);
        return Ok(());
    }
    bail!(
        "cannot import: unsupported heuristic keys {:?} on question {qname:?}",
        h.keys().collect::<Vec<_>>()
    )
}

/// One `label ...` rule of a threshold action.
/// Render one leaf threshold condition (the part after `when `) back to its
/// step text. The single-condition steps and the conjuncts of a conjunction
/// share this, so a leaf round-trips exactly the same way in both.
fn imp_when_leaf(when: &serde_json::Map<String, Value>, q_fallback: &str) -> Result<String> {
    let q = when.get("question").and_then(Value::as_str);
    if let Some(list) = when.get("answer_in").and_then(Value::as_array) {
        let qq = q.ok_or_else(|| anyhow!("cannot import: `answer_in` rule needs a question"))?;
        imp_tok("answer_in question", qq)?;
        let toks = imp_tokens("answer_in", list)?;
        return Ok(format!("the answer to \"{qq}\" is one of {toks}"));
    }
    if let Some(a) = when.get("answer").and_then(Value::as_str) {
        imp_tok("answer", a)?;
        if let Some(c) = when.get("value_gte") {
            let c = imp_num(imp_f64("value_gte", Some(c))?)?;
            return Ok(match q {
                Some(qq) => {
                    imp_tok("answer question", qq)?;
                    format!("the answer to \"{qq}\" is \"{a}\" with at least {c} confidence")
                }
                None => format!("the answer is \"{a}\" with at least {c} confidence"),
            });
        }
        return Ok(match q {
            Some(qq) => {
                imp_tok("answer question", qq)?;
                format!("the answer to \"{qq}\" is \"{a}\"")
            }
            None => format!("the answer is \"{a}\""),
        });
    }
    if let Some(opt) = when.get("option").and_then(Value::as_str) {
        imp_tok("option", opt)?;
        // `option` + `gte` (the "at least N confidence" step) is the common
        // shape; `option` + `lt` (the "at most N confidence" step) is the
        // mirror. Exactly one must be present, and the mirror needs an
        // explicit question to round-trip (there is no implicit spelling).
        let (c, cmp) = match (when.get("gte"), when.get("lt")) {
            (Some(g), None) => (imp_num(imp_f64("gte", Some(g))?)?, "at least"),
            (None, Some(l)) => (imp_num(imp_f64("lt", Some(l))?)?, "at most"),
            _ => bail!(
                "cannot import: an `option` rule needs exactly one of `gte`/`lt` (have gte={:?} lt={:?})",
                when.get("gte"), when.get("lt")
            ),
        };
        return Ok(match q {
            Some(qq) => {
                imp_tok("option question", qq)?;
                format!("the answer to \"{qq}\" is \"{opt}\" with {cmp} {c} confidence")
            }
            None if cmp == "at least" => {
                format!("the answer is \"{opt}\" with at least {c} confidence")
            }
            None => bail!("cannot import: an `option` + `lt` rule needs an explicit `question`"),
        });
    }
    if let Some(v) = when.get("value_gte") {
        // A rule may omit `question`; the compiler fills the action's own
        // question in, which the round-trip normaliser folds away again - so
        // spell it out with the action's question to recover the same shape.
        let qq = q.unwrap_or(q_fallback);
        imp_tok("value_gte question", qq)?;
        let v = imp_num(imp_f64("value_gte", Some(v))?)?;
        return Ok(format!("\"{qq}\" is at least {v}"));
    }
    if let Some(v) = when.get("value_lt") {
        let qq = q.unwrap_or(q_fallback);
        imp_tok("value_lt question", qq)?;
        let v = imp_num(imp_f64("value_lt", Some(v))?)?;
        return Ok(format!("\"{qq}\" is below {v}"));
    }
    bail!("cannot import: threshold rule with unsupported `when`: {when:?}")
}

/// One `label ...` rule of a threshold action.
fn imp_rule(st: &mut ImpSteps, q_fallback: &str, rule: &Value) -> Result<()> {
    let label = imp_str("threshold rule label", rule.get("label"))?;
    imp_tok("threshold rule label", label)?;
    let when = rule
        .get("when")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("cannot import: threshold rule has no `when` object"))?;
    if when.get("always").and_then(Value::as_bool) == Some(true) {
        st.push("then", format!("label \"{label}\" otherwise"), vec![]);
        return Ok(());
    }
    // A conjunction: render each conjunct as a leaf and join with ` and `, the
    // same spelling `parse_conjunction_leaves` splits back apart.
    if let Some(all) = when.get("all").and_then(Value::as_array) {
        let mut parts: Vec<String> = Vec::new();
        for sub in all {
            let so = sub
                .as_object()
                .ok_or_else(|| anyhow!("cannot import: `when.all` conditions must be objects"))?;
            parts.push(imp_when_leaf(so, q_fallback)?);
        }
        if parts.len() < 2 {
            bail!("cannot import: a conjunctive rule needs at least two conditions");
        }
        st.push(
            "then",
            format!("label \"{label}\" when {}", parts.join(" and ")),
            vec![],
        );
        return Ok(());
    }
    let body = imp_when_leaf(when, q_fallback)?;
    st.push("then", format!("label \"{label}\" when {body}"), vec![]);
    Ok(())
}

/// The `Then` steps that rebuild a node's action.
fn imp_action(st: &mut ImpSteps, primary_q: &str, a: &Value) -> Result<()> {
    let o = a
        .as_object()
        .ok_or_else(|| anyhow!("cannot import: node action is not an object"))?;
    let kind = o
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("cannot import: node action has no kind"))?;
    match kind {
        "threshold" => {
            let aq = o.get("question").and_then(Value::as_str).unwrap_or(primary_q);
            if aq != primary_q {
                imp_tok("action question", aq)?;
                st.push("then", format!("the action reads the question \"{aq}\""), vec![]);
            }
            let rules = o
                .get("rules")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("cannot import: threshold action has no rules array"))?;
            for r in rules {
                imp_rule(st, aq, r)?;
            }
        }
        "gate" => {
            let q = imp_str("gate.question", o.get("question"))?;
            let opt = imp_str("gate.option", o.get("option"))?;
            imp_tok("gate.question", q)?;
            imp_tok("gate.option", opt)?;
            let b = imp_num(imp_f64("gate.block_at", o.get("block_at"))?)?;
            let al = imp_num(imp_f64("gate.allow_at", o.get("allow_at"))?)?;
            st.push("then", format!("gate \"{q}\" on option \"{opt}\" with block {b} allow {al}"), vec![]);
        }
        "merge_open_probs" => {
            let q = imp_str("merge_open_probs.question", o.get("question"))?;
            imp_tok("merge_open_probs.question", q)?;
            st.push("then", format!("merge open probabilities on \"{q}\""), vec![]);
        }
        "combine" => {
            let choice = imp_str("combine.choice", o.get("choice"))?;
            let signal = imp_str("combine.signal", o.get("signal"))?;
            let fallback = imp_str("combine.fallback", o.get("fallback"))?;
            let review = imp_str("combine.review_label", o.get("review_label"))?;
            for (w, v) in [
                ("combine.choice", choice),
                ("combine.signal", signal),
                ("combine.fallback", fallback),
                ("combine.review_label", review),
            ] {
                imp_tok(w, v)?;
            }
            let min = imp_num(imp_f64("combine.min_confidence", o.get("min_confidence"))?)?;
            let high = imp_num(imp_f64("combine.high_threshold", o.get("high_threshold"))?)?;
            let low = imp_num(imp_f64("combine.low_threshold", o.get("low_threshold"))?)?;
            st.push(
                "then",
                format!("combine \"{choice}\" with signal \"{signal}\" fallback \"{fallback}\" review \"{review}\" requiring {min} high {high} low {low}"),
                vec![],
            );
        }
        "rubric" => {
            let dims = o.get("dimensions").and_then(Value::as_array).ok_or_else(|| {
                anyhow!("cannot import: rubric action has no dimensions array")
            })?;
            if dims.len() != 2 {
                bail!("cannot import: rubric action needs exactly two dimensions, got {}", dims.len());
            }
            let d0 = dims[0].as_object().ok_or_else(|| anyhow!("cannot import: rubric dimension 0 is not an object"))?;
            let d1 = dims[1].as_object().ok_or_else(|| anyhow!("cannot import: rubric dimension 1 is not an object"))?;
            let q1 = imp_str("rubric.dimensions[0].question", d0.get("question"))?;
            let q2 = imp_str("rubric.dimensions[1].question", d1.get("question"))?;
            imp_tok("rubric.dimensions[0].question", q1)?;
            imp_tok("rubric.dimensions[1].question", q2)?;
            let l1 = imp_num(imp_f64("rubric.dimensions[0].levels", d0.get("levels"))?)?;
            let l2 = imp_num(imp_f64("rubric.dimensions[1].levels", d1.get("levels"))?)?;
            let w1 = imp_num(imp_f64("rubric.dimensions[0].weight", d0.get("weight"))?)?;
            let w2 = imp_num(imp_f64("rubric.dimensions[1].weight", d1.get("weight"))?)?;
            let min = imp_num(imp_f64("rubric.min_confidence", o.get("min_confidence"))?)?;
            st.push(
                "then",
                format!("rubric \"{q1}\" at {l1} levels weight {w1} and \"{q2}\" at {l2} levels weight {w2} requiring {min}"),
                vec![],
            );
        }
        "ui_proposal" => {
            let op = imp_str("ui_proposal.operation", o.get("operation"))?;
            let f = imp_str("ui_proposal.field", o.get("field"))?;
            let amt = imp_str("ui_proposal.amount", o.get("amount"))?;
            let surface = imp_str("ui_proposal.surface", o.get("surface"))?;
            let hash = imp_str("ui_proposal.snapshot_hash", o.get("snapshot_hash"))?;
            for (w, v) in [
                ("ui_proposal.operation", op),
                ("ui_proposal.field", f),
                ("ui_proposal.amount", amt),
                ("ui_proposal.surface", surface),
                ("ui_proposal.snapshot_hash", hash),
            ] {
                imp_tok(w, v)?;
            }
            let min = imp_num(imp_f64(
                "ui_proposal.min_confidence",
                o.get("min_confidence"),
            )?)?;
            let body = match o.get("value_key").and_then(Value::as_str) {
                Some(vk) => {
                    imp_tok("ui_proposal.value_key", vk)?;
                    format!("prepare a UI proposal from operation \"{op}\" field \"{f}\" amount \"{amt}\" surface \"{surface}\" hash \"{hash}\" requiring {min} value \"{vk}\"")
                }
                None => format!("prepare a UI proposal from operation \"{op}\" field \"{f}\" amount \"{amt}\" surface \"{surface}\" hash \"{hash}\" requiring {min}"),
            };
            st.push("then", body, vec![]);
        }
        "copy_keys" => {
            let keys = o
                .get("keys")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("cannot import: copy_keys action has no keys array"))?;
            for k in keys {
                let k = imp_str("copy_keys[]", Some(k))?;
                st.push("then", format!("keep the answer to \"{k}\""), vec![]);
            }
        }
        "call" => {
            let cap = imp_str("call.capability", o.get("capability"))?;
            imp_tok("call.capability", cap)?;
            let mut rows: Vec<(String, String)> = Vec::new();
            if let Some(with) = o.get("with").and_then(Value::as_object) {
                for (k, v) in with {
                    rows.push((format!("with.{k}"), serde_json::to_string(v)?));
                }
            }
            if let Some(project) = o.get("project").and_then(Value::as_object) {
                for (k, v) in project {
                    rows.push((format!("project.{k}"), serde_json::to_string(v)?));
                }
            }
            if let Some(chain) = o.get("chain").and_then(Value::as_array) {
                for e in chain {
                    let e = e
                        .as_object()
                        .ok_or_else(|| anyhow!("cannot import: call chain entry is not an object"))?;
                    let asname = imp_str("call.chain[].as", e.get("as"))?;
                    imp_line("call.chain[].as", asname)?;
                    if asname.contains('.') {
                        bail!("cannot import: call chain alias {asname:?} must not contain a dot");
                    }
                    let subcap = imp_str("call.chain[].capability", e.get("capability"))?;
                    rows.push((format!("chain.{asname}.capability"), serde_json::to_string(&json!(subcap))?));
                    if let Some(with) = e.get("with").and_then(Value::as_object) {
                        for (k, v) in with {
                            rows.push((format!("chain.{asname}.with.{k}"), serde_json::to_string(v)?));
                        }
                    }
                    if let Some(project) = e.get("project").and_then(Value::as_object) {
                        for (k, v) in project {
                            rows.push((format!("chain.{asname}.project.{k}"), serde_json::to_string(v)?));
                        }
                    }
                }
            }
            let table = rows
                .into_iter()
                .map(|(k, v)| format!("| {} | {} |", imp_cell(&k), imp_cell(&v)))
                .collect();
            st.push("then", format!("call the capability \"{cap}\""), table);
        }
        other => bail!("cannot import: node action kind {other:?} has no BDD spelling"),
    }
    Ok(())
}

/// One `Scenario:` - a decision node. The primary-question override is emitted
/// only when the first question's name would not already be it.
fn imp_node(out: &mut String, node: &Value) -> Result<()> {
    let o = node
        .as_object()
        .ok_or_else(|| anyhow!("cannot import: a node is not an object"))?;
    let name = imp_str("node name", o.get("name"))?;
    imp_tok("node name", name)?;
    if slugify(name) != name {
        bail!("cannot import: node name {name:?} is not a slug (Scenario names are slugified)");
    }
    out.push_str(&format!("\n  Scenario: {name}\n"));

    if let Some(wf) = o.get("workflow") {
        let wf = imp_str("node.workflow", Some(wf))?;
        imp_tok("node.workflow", wf)?;
        out.push_str(&format!("    Given the sub workflow \"{wf}\"\n"));
        // Emit the handoff step exactly when the node carries an edge default.
        // `STOP` is *not* a reason to omit it: the compiler records an explicit
        // `edge.default: "STOP"` for `continue at "STOP"`, but *no edge at all*
        // for a reference node with no handoff step, so dropping the `STOP` step
        // would recompile to a different graph and `same_graph` would fail.
        if let Some(default) = o
            .get("edge")
            .and_then(|e| e.get("default"))
            .and_then(Value::as_str)
        {
            imp_tok("node.edge.default", default)?;
            // The step body literally starts with `when` (it is a Given-kind
            // step); the vocabulary spells it that way.
            out.push_str(&format!(
                "    And when the sub workflow completes continue at \"{default}\"\n"
            ));
        }
        return Ok(());
    }

    let questions = o.get("questions").and_then(Value::as_object);
    let primary_q = o.get("primary_q").and_then(Value::as_str).unwrap_or("ok");
    let first_q = questions.and_then(|q| q.keys().next()).map(|s| s.as_str());

    let mut st = ImpSteps::default();
    if let Some(questions) = questions {
        for (qname, q) in questions {
            let q = q
                .as_object()
                .ok_or_else(|| anyhow!("cannot import: question {qname:?} is not an object"))?;
            imp_question(&mut st, qname, q)?;
        }
    }

    // Edges. A condition entry that already routes to the default is folded
    // away by the round-trip normaliser, so it is not emitted.
    let edge = o.get("edge");
    let default = edge
        .and_then(|e| e.get("default"))
        .and_then(Value::as_str)
        .unwrap_or("STOP");
    let mut routed = false;
    if let Some(cond) = edge.and_then(|e| e.get("condition")).and_then(Value::as_object) {
        for (a, tgt) in cond {
            let tgt = imp_str("edge.condition target", Some(tgt))?;
            imp_tok("edge.condition answer", a)?;
            imp_tok("edge.condition target", tgt)?;
            if tgt == default {
                continue;
            }
            routed = true;
            st.push("when", format!("the answer \"{a}\" goes to \"{tgt}\""), vec![]);
        }
    }
    if default != "STOP" || !st.when.is_empty() {
        imp_tok("edge.default", default)?;
        st.push("when", format!("any other answer goes to \"{default}\""), vec![]);
    }

    // Emit the primary explicitly when it is not the first question, or whenever
    // the node routes on a question while declaring more than one (P136: the
    // compiler now requires that, so the routed question is never inferred from
    // declaration order).
    let n_q = questions.map(|q| q.len()).unwrap_or(0);
    if first_q != Some(primary_q) || (n_q >= 2 && routed) {
        imp_tok("primary_q", primary_q)?;
        st.push("then", format!("the primary question is \"{primary_q}\""), vec![]);
    }
    if let Some(a) = o.get("action") {
        imp_action(&mut st, primary_q, a)?;
    }
    if let Some(state) = o.get("state").and_then(Value::as_object) {
        // P143: both halves of the projection round-trip now. `keep` first, then
        // `rename`; serde_json keys are sorted, so the emitted feature is stable.
        if let Some(keep) = state.get("keep").and_then(Value::as_array) {
            for k in keep {
                let k = imp_str("state.keep[]", Some(k))?;
                imp_line("state.keep[]", k)?;
                st.push("then", format!("keep the state key \"{k}\""), vec![]);
            }
        }
        if let Some(rename) = state.get("rename").and_then(Value::as_object) {
            for (to, from) in rename {
                imp_tok("state.rename[] out", to)?;
                let from = imp_str("state.rename[] in", Some(from))?;
                imp_line("state.rename[] in", from)?;
                st.push(
                    "then",
                    format!("map the state key \"{to}\" from \"{from}\""),
                    vec![],
                );
            }
        }
    }
    if let Some(mc) = o.get("min_confidence").and_then(Value::as_f64) {
        if mc != 0.0 {
            st.push(
                "then",
                format!("require confidence at least {}", imp_num(mc)?),
                vec![],
            );
        }
    }
    if let Some(n) = o.get("max_retries").and_then(Value::as_u64) {
        st.push("then", format!("allow up to {n} retries"), vec![]);
    }

    out.push_str(&st.render());
    Ok(())
}

/// A spec → the `.feature` text that compiles back to it. The inverse of
/// [`compile_decision`]; errors (rather than guessing) on anything the BDD
/// grammar cannot spell.
pub fn import_spec(spec: &Value) -> Result<String> {
    let o = spec
        .as_object()
        .ok_or_else(|| anyhow!("cannot import: the spec is not a JSON object"))?;
    let nodes = o
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("cannot import: the spec has no `nodes` array"))?;
    if nodes.is_empty() {
        bail!("cannot import: the spec has no nodes");
    }
    if o.contains_key("workflow") || o.contains_key("sub_workflow") {
        bail!("cannot import: a spec that is itself a sub-workflow reference has no standalone spelling");
    }
    let name = imp_str("spec name", o.get("name"))?;
    imp_line("spec name", name)?;
    let start = imp_str("spec start", o.get("start"))?;
    imp_line("spec start", start)?;
    if !nodes.iter().any(|n| n.get("name").and_then(Value::as_str) == Some(start)) {
        bail!("cannot import: spec start {start:?} is not one of the node names");
    }

    let mut out = String::from("@decision\n");
    out.push_str(&format!("Feature: {name}\n"));

    // The description is free prose between `Feature:` and `Background:`. Only
    // reproduce it when every line is safe there (a line spelled like a Gherkin
    // keyword would be read as a step); otherwise fall back to a generated one.
    let desc = o.get("description").and_then(Value::as_str).unwrap_or("");
    let safe_desc = !desc.is_empty()
        && desc.lines().all(|l| {
            let t = l.trim();
            let low = t.to_lowercase();
            !t.is_empty()
                && !matches!(low.as_str(), "given" | "when" | "then" | "and" | "but")
                && !matches!(
                    t.split_once(':').map(|(h, _)| h.trim().to_lowercase()).as_deref(),
                    Some("feature" | "background" | "scenario" | "scenario outline" | "examples" | "rule")
                )
        });
    if safe_desc {
        for l in desc.lines() {
            out.push_str(&format!("\n  {}\n", l.trim()));
        }
    } else {
        out.push_str(&format!("\n  Imported from the {name} workflow spec.\n"));
    }

    let mut background: Vec<(String, Vec<String>)> = Vec::new();
    if let Some(n) = o.get("max_iterations").and_then(Value::as_u64) {
        background.push((format!("the workflow runs at most {n} iterations"), vec![]));
    }
    if let Some(policy) = o.get("policy").and_then(Value::as_object) {
        if !policy.is_empty() {
            let mut rows: Vec<String> = Vec::new();
            for (k, v) in policy {
                let cell = match k.as_str() {
                    "allow_exec" => match v.as_bool() {
                        Some(b) => b.to_string(),
                        None => bail!("cannot import: policy.allow_exec must be a bool, got {v}"),
                    },
                    "allow_hosts" | "allow_paths" => {
                        let xs = v
                            .as_array()
                            .ok_or_else(|| anyhow!("cannot import: policy.{k} must be an array"))?;
                        imp_tokens_esc(&format!("policy.{k}[]"), xs)?
                    }
                    "max_timeout_ms" | "max_output" | "retries" => match v.as_i64() {
                        Some(n) => n.to_string(),
                        None => bail!("cannot import: policy.{k} must be an integer, got {v}"),
                    },
                    _ => {
                        let sval = v
                            .as_str()
                            .ok_or_else(|| anyhow!("cannot import: policy.{k} must be a string, got {v}"))?;
                        imp_line(&format!("policy.{k}"), sval)?;
                        imp_cell(sval)
                    }
                };
                rows.push(format!("| {k} | {cell} |"));
            }
            background.push(("the policy".to_string(), rows));
        }
    }
    if let Some(caps) = o.get("capabilities").and_then(Value::as_object) {
        if !caps.is_empty() {
            let mut rows: Vec<String> = Vec::new();
            for (k, v) in caps {
                imp_line("capability name", k)?;
                let cfg = serde_json::to_string(v)?;
                rows.push(format!("| {} | {} |", imp_cell(k), imp_cell(&cfg)));
            }
            background.push(("the capabilities".to_string(), rows));
        }
    }
    if !background.is_empty() {
        out.push_str("\n  Background:\n");
        for (i, (line, rows)) in background.iter().enumerate() {
            let kw = if i == 0 { "Given" } else { "And" };
            out.push_str(&format!("    {kw} {line}\n"));
            for row in rows {
                out.push_str(&format!("      {row}\n"));
            }
        }
    }

    // The `start` node must be the first Scenario (that is how the compiler
    // recovers `start`).
    let mut order: Vec<&Value> = Vec::with_capacity(nodes.len());
    for n in nodes {
        if n.get("name").and_then(Value::as_str) == Some(start) {
            order.push(n);
        }
    }
    for n in nodes {
        if n.get("name").and_then(Value::as_str) != Some(start) {
            order.push(n);
        }
    }
    for n in order {
        imp_node(&mut out, n)?;
    }
    Ok(out)
}

/// A spec discovered under `bdd import --dir`: its file stem (the port name),
/// its path (for messages) and the parsed JSON.
pub struct FoundSpec {
    pub name: String,
    pub path: PathBuf,
    pub spec: Value,
}

/// Where `--dir` should send each imported `.feature` (and, under `--into`, its
/// spec). The single-spec path writes to stdout / `--out <FILE>` inline.
enum ImportDest {
    /// `--dir --out <DIR>`: write `<DIR>/<name>.feature` for every spec.
    Dir(PathBuf),
    /// `--into <cat>`: file the feature under `bdd/workflows/<cat>/` and copy the
    /// spec to `dsl/<cat>/`, so `round-trip` and `import-check` cover the pair.
    RepoCat(String),
    /// `--dir` with no destination: report what would be imported, write nothing.
    DryRun,
}

/// `bdd import`: lift a known workflow into BDD — the inverse of `transpile`.
///
/// * `<SPEC>` — one `.json` → one `.feature` (to stdout, `--out <FILE>`, or
///   `--into <cat>` to file it into this repo's trees).
/// * `--dir <DIR>` — every decision spec under `DIR` → one `.feature` each
///   (`--out <DIR>`, or `--into <cat>` to port them all; neither = a dry run
///   that only reports). `*.config.json` sidecars are skipped by name, and any
///   json without a non-empty `nodes` is skipped as "not a decision spec".
///
/// A feature is verified *before* it is written: it must re-parse, and the graph
/// it compiles back to must equal the source spec under `same_graph`. A spec BDD
/// cannot spell (or cannot round-trip) is reported, never written half-way.
pub fn import(o: &ImportOptions) -> Result<i32> {
    if o.spec.is_some() && o.dir.is_some() {
        bail!("pass either <SPEC> or --dir <DIR>, not both");
    }
    if o.out.is_some() && o.into.is_some() {
        bail!("pass either --out or --into, not both");
    }
    match (&o.spec, &o.dir) {
        (Some(spec), None) => import_one(spec, o),
        (None, Some(dir)) => import_dir(dir, o),
        (None, None) => {
            bail!("pass a <SPEC> path to import one spec, or --dir <DIR> to import a whole tree")
        }
        (Some(_), Some(_)) => {
            unreachable!("guarded above: <SPEC> and --dir are mutually exclusive")
        }
    }
}

/// One `.json` → one `.feature`.
fn import_one(spec_path: &str, o: &ImportOptions) -> Result<i32> {
    let text =
        std::fs::read_to_string(spec_path).with_context(|| format!("reading {spec_path}"))?;
    let spec: Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {spec_path} as JSON"))?;
    let stem = Path::new(spec_path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workflow".into());
    let mut feature = import_spec(&spec)?;

    if let Some(cat) = &o.into {
        let (feat, spec_out) = repo_cat_paths(&repo_root(), cat, &stem);
        if (feat.exists() || spec_out.exists()) && !o.force {
            bail!(
                "{} already holds `{stem}` (--force to overwrite)",
                feat.parent().unwrap_or(Path::new("")).display()
            );
        }
        feature = import_spec(&normalise_name(&spec, &stem))?;
        write_file(&feat, &feature)?;
        write_file(
            &spec_out,
            &format!(
                "{}\n",
                serde_json::to_string_pretty(&normalise_name(&spec, &stem))?
            ),
        )?;
        eprintln!(
            "bdd import: wrote {} (+ spec {})",
            feat.display(),
            spec_out.display()
        );
        return Ok(0);
    }
    match &o.out {
        Some(p) => {
            write_file(Path::new(p), &feature)?;
            eprintln!("bdd import: wrote {p}");
        }
        None => print!("{feature}"),
    }
    Ok(0)
}

/// The port name `--into` files a workflow under: the source file stem, which
/// must already be a slug because `compile` slugs the `Feature:` line and the
/// round-trip gate compares that against the spec's `name`.
pub fn repo_cat_paths(root: &Path, cat: &str, stem: &str) -> (PathBuf, PathBuf) {
    (
        root.join("bdd")
            .join("workflows")
            .join(cat)
            .join(format!("{stem}.feature")),
        root.join("dsl").join(cat).join(format!("{stem}.json")),
    )
}

/// Copy a spec for filing under `--into`, pinning its `name` to the port stem so
/// `import` emits `Feature: <stem>` and the recompiled graph carries the same
/// name — that is what makes the filed pair pass both `round-trip` and
/// `import-check`.
pub fn normalise_name(spec: &Value, stem: &str) -> Value {
    let mut s = spec.clone();
    if let Some(obj) = s.as_object_mut() {
        obj.insert("name".to_string(), json!(stem));
    }
    s
}

/// Write `text` to `path`, creating the parent directory first.
fn write_file(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

/// Recursively collect every decision spec under `dir`: a `.json` that parses to
/// an object with a non-empty `nodes` array. `*.config.json` sidecars are
/// config, not workflows. Returns the specs, plus a reason line for each json
/// that was skipped because it is *not* a workflow.
pub fn collect_spec_dir(dir: &Path) -> Result<(Vec<FoundSpec>, Vec<String>)> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd =
            std::fs::read_dir(&d).with_context(|| format!("reading spec dir {}", d.display()))?;
        for e in rd.flatten() {
            let p = e.path();
            let fname = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if !fname.starts_with('.') {
                    stack.push(p);
                }
            } else if p.is_file() && fname.ends_with(".json") {
                files.push(p);
            }
        }
    }
    files.sort();

    let mut found = Vec::new();
    let mut skipped = Vec::new();
    for p in files {
        let rel = p.strip_prefix(dir).unwrap_or(&p).display().to_string();
        let fname = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if fname.ends_with(".config.json") {
            continue; // a config sidecar, never a workflow
        }
        let text =
            std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        let spec: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                skipped.push(format!("{rel}: not JSON ({e})"));
                continue;
            }
        };
        let is_spec = spec
            .get("nodes")
            .and_then(Value::as_array)
            .map(|n| !n.is_empty())
            .unwrap_or(false);
        if !is_spec {
            skipped.push(format!("{rel}: not a decision spec (no non-empty `nodes`)"));
            continue;
        }
        found.push(FoundSpec {
            name: fname.trim_end_matches(".json").to_string(),
            path: p,
            spec,
        });
    }
    Ok((found, skipped))
}

/// `--dir`: every decision spec under `dir` → one `.feature` each. Each is
/// verified (re-parse + recompile + `same_graph`) before it is written.
fn import_dir(dir: &str, o: &ImportOptions) -> Result<i32> {
    let dir = Path::new(dir);
    if !dir.is_dir() {
        bail!("--dir {} is not a directory", dir.display());
    }
    let root = repo_root();
    let dest = if let Some(cat) = &o.into {
        ImportDest::RepoCat(cat.clone())
    } else if let Some(out) = &o.out {
        ImportDest::Dir(PathBuf::from(out))
    } else {
        ImportDest::DryRun
    };

    let (found, skipped) = collect_spec_dir(dir)?;
    if found.is_empty() {
        bail!(
            "no decision spec (*.json with a non-empty `nodes`) found under {}",
            dir.display()
        );
    }

    let mut report: Vec<Value> = Vec::new();
    let mut written = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for f in &found {
        let rel = f
            .path
            .strip_prefix(dir)
            .unwrap_or(&f.path)
            .display()
            .to_string();
        let feature = match import_spec(&f.spec) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{rel}: {e:#}"));
                report.push(json!({"name": f.name, "status": "unspellable"}));
                continue;
            }
        };
        let verified = match parse(&feature, "<import>") {
            Ok(pf) => match compile_decision(&pf, &json!({})) {
                Ok(rb) => same_graph(&f.spec, &rb),
                Err(_) => false,
            },
            Err(_) => false,
        };
        if !verified {
            failed.push(format!(
                "{rel}: the imported feature does not compile back to the same graph"
            ));
            report.push(json!({"name": f.name, "status": "not-round-trippable"}));
            continue;
        }
        match &dest {
            ImportDest::DryRun => {
                report.push(json!({"name": f.name, "status": "importable"}));
            }
            ImportDest::Dir(out_dir) => {
                let path = out_dir.join(format!("{}.feature", f.name));
                if path.exists() && !o.force {
                    failed.push(format!(
                        "{rel}: {} exists (--force to overwrite)",
                        path.display()
                    ));
                    report.push(json!({"name": f.name, "status": "collision"}));
                    continue;
                }
                write_file(&path, &feature)?;
                written += 1;
                report.push(
                    json!({"name": f.name, "status": "written", "out": path.to_string_lossy()}),
                );
            }
            ImportDest::RepoCat(cat) => {
                let (feat, spec_out) = repo_cat_paths(&root, cat, &f.name);
                if (feat.exists() || spec_out.exists()) && !o.force {
                    failed.push(format!(
                        "{rel}: `{}` is already in the tree (--force to overwrite)",
                        f.name
                    ));
                    report.push(json!({"name": f.name, "status": "collision"}));
                    continue;
                }
                let spec = normalise_name(&f.spec, &f.name);
                let feature = import_spec(&spec)?;
                write_file(&feat, &feature)?;
                write_file(
                    &spec_out,
                    &format!("{}\n", serde_json::to_string_pretty(&spec)?),
                )?;
                written += 1;
                report.push(json!({
                    "name": f.name,
                    "status": "ported",
                    "feature": feat.to_string_lossy(),
                    "spec": spec_out.to_string_lossy()
                }));
            }
            ImportDest::RepoCat(_) | ImportDest::Dir(_) | ImportDest::DryRun => {
                unreachable!("handled above")
            }
        }
    }

    if let Some(path) = &o.json {
        let out = json!({
            "dir": dir.display().to_string(),
            "counts": {"found": found.len(), "written": written, "skipped": skipped.len(), "failed": failed.len()},
            "specs": report,
            "skipped": skipped
        });
        write_file(
            Path::new(path),
            &format!("{}\n", serde_json::to_string_pretty(&out)?),
        )?;
    }

    println!(
        "bdd import: {} spec(s) found under {} - {} {}, {} skipped, {} failed",
        found.len(),
        dir.display(),
        written,
        match dest {
            ImportDest::DryRun => "importable (dry run; pass --out or --into to write)",
            _ => "written",
        },
        skipped.len(),
        failed.len()
    );
    for s in &skipped {
        eprintln!("bdd import: skipped {s}");
    }
    for f in &failed {
        eprintln!("bdd import: cannot import {f}");
    }
    if failed.is_empty() {
        Ok(0)
    } else {
        Ok(1)
    }
}

/// Options for `bdd import` (one spec, or a whole directory).
pub struct ImportOptions {
    /// One spec `.json` to import (omit with `--dir`).
    pub spec: Option<String>,
    /// Import every decision spec under this directory.
    pub dir: Option<String>,
    /// Write the `.feature` here — a file for one spec, a directory for `--dir`.
    pub out: Option<String>,
    /// File the port under `bdd/workflows/<cat>/` and copy the spec to `dsl/<cat>/`.
    pub into: Option<String>,
    /// Write the report as JSON here.
    pub json: Option<String>,
    /// Overwrite an existing target instead of reporting a collision.
    pub force: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            spec: None,
            dir: None,
            out: None,
            into: None,
            json: None,
            force: false,
        }
    }
}

/// Options for `bdd import-check`.
pub struct ImportCheckOptions {
    /// Write the per-spec verdicts as JSON here.
    pub json: Option<String>,
}

impl Default for ImportCheckOptions {
    fn default() -> Self {
        Self { json: None }
    }
}

/// The import gate: for every checked-in `@decision` feature, `import` its dsl
/// spec and require the re-compiled graph to equal that spec. This is the
/// *reverse* of `bdd round-trip`, and together they prove the two spellings are
/// interchangeable - a spec is not just `transpile`-able from its feature, it is
/// *recoverable* from the spec alone. A spec that cannot be imported is a
/// workflow BDD cannot yet express.
pub fn import_check(o: &ImportCheckOptions) -> Result<i32> {
    let root = repo_root();
    let wf_root = root.join("bdd").join("workflows");
    let mut features: Vec<PathBuf> = Vec::new();
    if wf_root.is_dir() {
        walk_features(&wf_root, &mut features);
    }
    features.sort();

    // Guarantee the scratch dirs the specs reference (`${env.LAYA_*}`) resolve,
    // exactly as `bdd build` and `bdd round-trip` do.
    if let Ok(env_map) = crate::dsl_smoke::ensure_env_dirs() {
        for (k, v) in env_map {
            std::env::set_var(k, v);
        }
        let _ = crate::capability::secret::init(None);
    }

    let mut total = 0usize;
    let mut matched = 0usize;
    let mut problems: Vec<String> = Vec::new();
    let mut verdicts: Vec<Value> = Vec::new();
    for f in &features {
        let text =
            std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?;
        let feature = parse(&text, &f.to_string_lossy())?;
        if !has_tag(&feature.tags, "decision") {
            continue;
        }
        total += 1;
        let rel = f
            .strip_prefix(&wf_root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let rel = rel.trim_start_matches('/').to_string();
        let (cat, fname) = match rel.rsplit_once('/') {
            Some((c, n)) => (c.to_string(), n.to_string()),
            None => (String::new(), rel.clone()),
        };
        let name = fname.strip_suffix(".feature").unwrap_or(&fname).to_string();
        let dsl_path = root.join("dsl").join(&cat).join(format!("{name}.json"));
        let dsl_text = match std::fs::read_to_string(&dsl_path) {
            Ok(t) => t,
            Err(_) => {
                problems.push(format!(
                    "{rel}: no dsl counterpart at {}",
                    dsl_path.display()
                ));
                verdicts.push(json!({"feature": rel, "ok": false, "reason": "no dsl counterpart"}));
                continue;
            }
        };
        let dsl: Value = serde_json::from_str(&dsl_text)
            .with_context(|| format!("parsing {}", dsl_path.display()))?;

        let back_text = match import_spec(&dsl) {
            Ok(t) => t,
            Err(e) => {
                problems.push(format!("{rel}: import failed: {e:#}"));
                verdicts
                    .push(json!({"feature": rel, "ok": false, "reason": format!("import: {e:#}")}));
                continue;
            }
        };
        let back = match parse(&back_text, "<import>") {
            Ok(f) => f,
            Err(e) => {
                problems.push(format!("{rel}: the imported feature does not parse: {e:#}"));
                verdicts.push(
                    json!({"feature": rel, "ok": false, "reason": format!("reparse: {e:#}")}),
                );
                continue;
            }
        };
        let spec = match compile_decision(&back, &json!({})) {
            Ok(s) => s,
            Err(e) => {
                problems.push(format!(
                    "{rel}: the imported feature does not compile: {e:#}"
                ));
                verdicts.push(
                    json!({"feature": rel, "ok": false, "reason": format!("recompile: {e:#}")}),
                );
                continue;
            }
        };
        if let Err(e) = crate::spec::from_spec_with_dir(&spec, None, None) {
            problems.push(format!("{rel}: the imported feature fails validate: {e:#}"));
            verdicts.push(json!({"feature": rel, "ok": false, "reason": format!("validate: {e:#}")}));
            continue;
        }
        let nb = rt_norm(&spec);
        let nd = rt_norm(&dsl);
        if nb == nd {
            matched += 1;
            verdicts.push(json!({"feature": rel, "ok": true}));
        } else {
            let diff = rt_first_diff(&nb, &nd, "spec").unwrap_or_else(|| "non-equal".to_string());
            problems.push(format!("{rel}: {diff}"));
            verdicts.push(json!({"feature": rel, "ok": false, "reason": diff}));
        }
    }

    if let Some(p) = &o.json {
        let out = root.join(p);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out, serde_json::to_string_pretty(&verdicts)?)?;
    }

    println!("bdd import: {matched}/{total} checked-in specs recompile from their imported feature");
    for p in &problems {
        println!("  {p}");
    }
    if !problems.is_empty() {
        eprintln!("bdd import: FAIL - {} of {} specs could not be re-imported", problems.len(), total);
        return Ok(1);
    }
    Ok(0)
}

/// Options for `bdd features-check`.
pub struct FeaturesCheckOptions {
    /// Write the per-feature verdicts as JSON here.
    pub json: Option<String>,
}

impl Default for FeaturesCheckOptions {
    fn default() -> Self {
        Self { json: None }
    }
}

/// The corpus-completeness gate: **every checked-in `.feature` compiles, and
/// every one is covered by a named gate bucket.**
///
/// The big gates each compile their own tree - `check.sh` builds
/// `bdd/features/*.feature` and every `@decision` feature, `bdd compare` the
/// `bdd/workflows/browser/` corpus, `bdd score` the authoring tasks - and until
/// this gate existed *nothing* compiled `bdd/workflows/examples/`, the two
/// features `docs/bdd_quickstart.md` tells a new author to copy. A vocabulary
/// change could break a documented example while every gate stayed green. So
/// this walks the whole `bdd/` tree (minus `bdd/features/setup/`, the
/// `include:`-ed step lists with no `Feature:` header), compiles each with the
/// same compiler `bdd build` uses, and refuses a feature that is in no known
/// bucket - a `.feature` dropped in a directory no gate reads is *named* here
/// rather than silently shipping unverified.
pub fn features_check(o: &FeaturesCheckOptions) -> Result<i32> {
    let root = repo_root();
    let (total, compiled, buckets, verdicts, problems) = features_check_impl(&root)?;
    if let Some(p) = &o.json {
        let out = root.join(p);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out, serde_json::to_string_pretty(&verdicts)?)?;
    }
    let bucket_str = buckets
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ");
    println!("bdd features-check: {compiled}/{total} checked-in features compile ({bucket_str})");
    for p in &problems {
        println!("  {p}");
    }
    if !problems.is_empty() {
        eprintln!(
            "bdd features-check: FAIL - {} problem(s) over {} feature(s)",
            problems.len(),
            total
        );
        return Ok(1);
    }
    Ok(0)
}

/// The gate tree a checked-in `.feature` belongs to, or `None` if no gate
/// compiles it. Path first (an `@decision` feature under `workflows/browser/`
/// is a browser case), tag last.
pub fn feature_bucket(rel: &str, feature: &Feature) -> Option<&'static str> {
    if rel.starts_with("bdd/features/") {
        Some("features")
    } else if rel.starts_with("bdd/workflows/browser/") {
        Some("browser")
    } else if rel.starts_with("bdd/workflows/tasks/") {
        Some("tasks")
    } else if rel.starts_with("bdd/workflows/examples/") {
        Some("examples")
    } else if has_tag(&feature.tags, "decision") {
        Some("decision")
    } else {
        None
    }
}

/// The testable core of [`features_check`]: `(total, compiled, buckets,
/// verdicts, problems)`. Split out so a regression can assert on the counts and
/// the bucket map without parsing the printed line.
pub fn features_check_impl(
    root: &std::path::Path,
) -> Result<(
    usize,
    usize,
    std::collections::BTreeMap<&'static str, usize>,
    Vec<Value>,
    Vec<String>,
)> {
    let bdd = root.join("bdd");
    // Guarantee the scratch dirs the specs reference (`${env.LAYA_*}`) resolve,
    // exactly as `bdd build` / `round-trip` do.
    if let Ok(env_map) = crate::dsl_smoke::ensure_env_dirs() {
        for (k, v) in env_map {
            std::env::set_var(k, v);
        }
        let _ = crate::capability::secret::init(None);
    }
    let wrapper = default_chrome_wrapper();

    let mut all: Vec<PathBuf> = Vec::new();
    if bdd.is_dir() {
        walk_features(&bdd, &mut all);
    }
    all.sort();

    let mut total = 0usize;
    let mut compiled = 0usize;
    let mut buckets: std::collections::BTreeMap<&'static str, usize> = std::collections::BTreeMap::new();
    let mut problems: Vec<String> = Vec::new();
    let mut verdicts: Vec<Value> = Vec::new();
    for f in &all {
        let rel = f.strip_prefix(root).unwrap_or(f).to_string_lossy().into_owned();
        let rel = rel.trim_start_matches('/').to_string();
        // `bdd/features/setup/*.feature` are the step lists pulled in with
        // `include:` - a flat list with no `Feature:` header, not a feature.
        if rel.starts_with("bdd/features/setup/") {
            continue;
        }
        total += 1;
        let text =
            std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?;
        let feature = match parse(&text, &f.to_string_lossy()) {
            Ok(fe) => fe,
            Err(e) => {
                problems.push(format!("{rel}: parse failed: {e:#}"));
                verdicts.push(json!({"feature": rel, "ok": false, "reason": format!("parse: {e:#}")}));
                continue;
            }
        };
        match feature_bucket(&rel, &feature) {
            Some(b) => *buckets.entry(b).or_insert(0) += 1,
            None => problems.push(format!(
                "{rel}: not covered by any gate bucket (add it to a gated tree, or to feature_bucket)"
            )),
        }
        let config = load_config(f).unwrap_or_else(|_| json!({}));
        let reason = if has_tag(&feature.tags, "decision") {
            match compile_decision(&feature, &config) {
                Ok(spec) => crate::spec::from_spec_with_dir(&spec, None, None)
                    .err()
                    .map(|e| format!("generated spec fails validate: {e:#}")),
                Err(e) => Some(format!("compile failed: {e:#}")),
            }
        } else {
            let mut reason = None;
            for sc in &feature.scenarios {
                match compile_scenario(&feature, sc, &config, &wrapper) {
                    Ok(spec) => {
                        let payload = public_spec(&spec);
                        let empty_nodes = spec
                            .get("nodes")
                            .and_then(Value::as_array)
                            .map(|a| a.is_empty())
                            .unwrap_or(true);
                        if empty_nodes || payload.get("start").is_none() {
                            reason = Some(format!("scenario {:?} compiled to an empty spec", sc.name));
                            break;
                        }
                    }
                    Err(e) => {
                        reason = Some(format!("scenario {:?} compile failed: {e:#}", sc.name));
                        break;
                    }
                }
            }
            reason
        };
        if let Some(r) = reason {
            problems.push(format!("{rel}: {r}"));
            verdicts.push(json!({"feature": rel, "ok": false, "reason": r}));
        } else {
            compiled += 1;
            verdicts.push(json!({"feature": rel, "ok": true}));
        }
    }
    Ok((total, compiled, buckets, verdicts, problems))
}

pub struct RoundTripOptions {
    /// Write the per-feature verdicts as JSON here.
    pub json: Option<String>,
}

impl Default for RoundTripOptions {
    fn default() -> Self {
        Self { json: None }
    }
}

/// Fill the heuristic defaults the engine applies when a key is absent, so a
/// dsl that omits them compares equal to a compiler that spells them out.
fn rt_norm_heuristic(h: &Value) -> Value {
    let mut m = h.as_object().cloned().unwrap_or_default();
    if m.contains_key("match_rules") {
        m.entry("p_hit".to_string()).or_insert(json!(0.9));
        m.entry("p_miss".to_string()).or_insert(json!(0.02));
        m.entry("p_fallback".to_string()).or_insert(json!(0.6));
    } else if m.contains_key("match_any")
        || m.contains_key("match_regex")
        || m.contains_key("extract_numeric")
        || m.contains_key("match_mid_any")
    {
        m.entry("p_hit".to_string()).or_insert(json!(0.9));
        m.entry("p_miss".to_string()).or_insert(json!(0.1));
        if m.contains_key("match_mid_any") {
            m.entry("p_mid".to_string()).or_insert(json!(0.5));
        }
    }
    Value::Object(m)
}

/// Canonicalise a JSON number so `1` and `1.0` compare equal.
fn rt_canon(v: Value) -> Value {
    match v {
        Value::Number(n) => Value::from(n.as_f64().unwrap_or(0.0)),
        Value::Array(a) => Value::Array(a.into_iter().map(rt_canon).collect()),
        Value::Object(o) => Value::Object(
            o.into_iter().map(|(k, v)| (k, rt_canon(v))).collect(),
        ),
        other => other,
    }
}

/// Normalise one side of the round-trip comparison (see the module comment).
fn rt_norm(v: &Value) -> Value {
    let mut obj = v.as_object().cloned().unwrap_or_default();
    // top-level: drop compiler metadata, prose and tuning we do not compare
    for k in [
        "_bdd",
        "description",
        "convergence_window",
        "convergence_eps",
        "dsl_version",
    ] {
        obj.remove(k);
    }
    // nodes: compare by name (array order is not load-bearing; `start` + edge
    // references are), after normalising each node.
    if let Some(nodes) = obj.get("nodes").cloned() {
        let mut by_name = Map::new();
        if let Some(arr) = nodes.as_array() {
            for n in arr {
                let name = n.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                by_name.insert(name, rt_norm_node(n));
            }
        }
        obj.insert("nodes".to_string(), Value::Object(by_name));
    }
    rt_canon(Value::Object(obj))
}

/// Normalise one decision node for comparison.
fn rt_norm_node(n: &Value) -> Value {
    let mut no = n.as_object().cloned().unwrap_or_default();
    no.remove("has_action");
    if let Some(mc) = no.get("min_confidence") {
        if mc.as_f64() == Some(0.0) {
            no.remove("min_confidence");
        }
    }
    // edge: fold condition entries that route to the default
    if let Some(edge) = no.get_mut("edge").and_then(Value::as_object_mut) {
        let default = edge.get("default").cloned();
        if let Some(cond) = edge.get_mut("condition").and_then(Value::as_object_mut) {
            if let Some(d) = &default {
                cond.retain(|_, tgt| tgt != d);
            }
        }
    }
    // questions: empty instructions == absent; fill heuristic defaults
    if let Some(qs) = no.get_mut("questions").and_then(Value::as_object_mut) {
        for q in qs.values_mut() {
            if let Some(go) = q.as_object_mut() {
                if go.get("instructions").map(|i| i.as_str().unwrap_or("") == "") == Some(true) {
                    go.remove("instructions");
                }
                if let Some(h) = go.get("heuristic").cloned() {
                    go.insert("heuristic".to_string(), rt_norm_heuristic(&h));
                }
            }
        }
    }
    // action: `with: {}` == absent; threshold rule `when.question` that
    // repeats the action's own question == implied
    if let Some(a) = no.get_mut("action").and_then(Value::as_object_mut) {
        if a.get("with").map(|w| w.as_object().map(|m| m.is_empty()).unwrap_or(false))
            == Some(true)
        {
            a.remove("with");
        }
        if a.get("kind").and_then(Value::as_str) == Some("threshold") {
            let aq = a.get("question").cloned();
            if let Some(rules) = a.get_mut("rules").and_then(Value::as_array_mut) {
                for r in rules.iter_mut() {
                    if let Some(w) = r.get_mut("when").and_then(Value::as_object_mut) {
                        if let Some(q) = &aq {
                            if w.get("question") == Some(q) {
                                w.remove("question");
                            }
                        }
                    }
                }
            }
        }
    }
    rt_canon(Value::Object(no))
}

/// Two specs describe the same decision graph under the round-trip
/// normalisation: compiler metadata and prose are ignored, nodes are compared
/// by name, a condition entry that routes to the default is folded away, and
/// absent heuristic probabilities are filled with the engine's defaults.
/// Public so the offline tests can assert equivalence directly.
pub fn same_graph(a: &Value, b: &Value) -> bool {
    rt_norm(a) == rt_norm(b)
}

/// First differing path between two JSON values (for a readable problem line).
fn rt_first_diff<'a>(a: &'a Value, b: &'a Value, path: &str) -> Option<String> {
    if a == b {
        return None;
    }
    match (a, b) {
        (Value::Object(am), Value::Object(bm)) => {
            for k in am.keys().chain(bm.keys()) {
                let av = am.get(k);
                let bv = bm.get(k);
                match (av, bv) {
                    (Some(x), Some(y)) => {
                        if let Some(d) = rt_first_diff(x, y, &format!("{path}.{k}")) {
                            return Some(d);
                        }
                    }
                    (Some(x), None) => {
                        return Some(format!("{path}.{k}: present={x} dsl=absent"));
                    }
                    (None, Some(y)) => {
                        return Some(format!("{path}.{k}: built=absent dsl={y}"));
                    }
                    (None, None) => {}
                }
            }
            None
        }
        (Value::Array(xs), Value::Array(ys)) => {
            if xs.len() != ys.len() {
                return Some(format!("{path}: built.len={} dsl.len={}", xs.len(), ys.len()));
            }
            for (i, (x, y)) in xs.iter().zip(ys.iter()).enumerate() {
                if let Some(d) = rt_first_diff(x, y, &format!("{path}[{i}]")) {
                    return Some(d);
                }
            }
            None
        }
        _ => Some(format!("{path}: built={a} dsl={b}")),
    }
}

/// The round-trip gate: every `@decision` feature reproduces its dsl spec.
/// A cross-repo conversion diff. `sources` are the upstream scenario names (for
/// example the subdirectories of an ole-eval `app_scenarios/` directory);
/// `features` and `specs` are the BDD port names in *this* repo. A closed
/// conversion has every source name on both port sides and no port without a
/// source. Pure, so the symmetric-difference rules are unit-testable without a
/// second checkout.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PortDiff {
    /// A source with no `bdd/workflows/<cat>/<name>.feature`.
    pub sources_no_feature: Vec<String>,
    /// A source with no `dsl/<cat>/<name>.json`.
    pub sources_no_spec: Vec<String>,
    /// A feature with no upstream source.
    pub features_no_source: Vec<String>,
    /// A checked-in spec with no upstream source.
    pub specs_no_source: Vec<String>,
}

impl PortDiff {
    pub fn is_empty(&self) -> bool {
        self.sources_no_feature.is_empty()
            && self.sources_no_spec.is_empty()
            && self.features_no_source.is_empty()
            && self.specs_no_source.is_empty()
    }
    pub fn total(&self) -> usize {
        self.sources_no_feature.len()
            + self.sources_no_spec.len()
            + self.features_no_source.len()
            + self.specs_no_source.len()
    }
}

fn names_missing_from(set: &BTreeSet<String>, have: &BTreeSet<String>) -> Vec<String> {
    set.difference(have).cloned().collect()
}

pub fn diff_port_sets(
    sources: &BTreeSet<String>,
    features: &BTreeSet<String>,
    specs: &BTreeSet<String>,
) -> PortDiff {
    PortDiff {
        sources_no_feature: names_missing_from(sources, features),
        sources_no_spec: names_missing_from(sources, specs),
        features_no_source: names_missing_from(features, sources),
        specs_no_source: names_missing_from(specs, sources),
    }
}

pub struct PortCheckOptions {
    /// Upstream scenario source directory (one subdirectory per scenario).
    pub sources: PathBuf,
    /// Port category: `bdd/workflows/<cat>/` and `dsl/<cat>/`.
    pub cat: String,
    /// Write the diff as JSON here.
    pub json: Option<String>,
    /// The **sealed** upstream source set, checked in under `bench/bdd/`. Without
    /// a second checkout the closure can only be proven against this seal, and
    /// with one it is re-verified — see `port_check`.
    pub pin: Option<PathBuf>,
    /// Seal the live `sources` directory into this manifest (re-seal after a
    /// review, never on a hunch).
    pub write_pin: Option<PathBuf>,
}

impl Default for PortCheckOptions {
    fn default() -> Self {
        Self {
            sources: PathBuf::new(),
            cat: "ole_eval".into(),
            json: None,
            pin: None,
            write_pin: None,
        }
    }
}

/// File stems of extension `ext` directly under `dir`. `<name>.config.json`
/// sidecars are config, not specs, so they never count as a port.
fn read_stem_set(dir: &Path, ext: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let n = e.file_name().to_string_lossy().into_owned();
            if ext == "json" && n.ends_with(".config.json") {
                continue;
            }
            if let Some(stem) = n.strip_suffix(&format!(".{ext}")) {
                if !stem.is_empty() {
                    out.insert(stem.to_string());
                }
            }
        }
    }
    out
}

/// FNV-1a 64 of `bytes`, lowercase hex.
///
/// A *fingerprint*, not a digest: it exists so that a scenario whose source
/// changed since it was sealed stops matching the seal. Nothing here is a
/// security boundary and the crate carries no hash dependency for it.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A scenario's *source* fingerprint: the top-level `.py` files, sorted by name,
/// each folded in as `name` + NUL + bytes. `results/` and `__pycache__/` are
/// outputs, not source, so a benchmark run does not move the seal.
fn scenario_fingerprint(dir: &Path) -> Result<String> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for e in std::fs::read_dir(dir)
        .with_context(|| format!("reading scenario dir {}", dir.display()))?
        .flatten()
    {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let n = e.file_name().to_string_lossy().into_owned();
        if n.ends_with(".py") {
            files.push((n, p));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (n, p) in &files {
        let body = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
        for chunk in [n.as_bytes(), &[0u8], body.as_slice()] {
            h ^= fnv1a64(chunk);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    Ok(format!("{h:016x}"))
}

/// Every scenario under `dir`: name -> source fingerprint, name-sorted.
fn read_sources_sealed(dir: &Path) -> Result<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    for e in std::fs::read_dir(dir)
        .with_context(|| format!("reading source dir {}", dir.display()))?
        .flatten()
    {
        if !e.path().is_dir() {
            continue;
        }
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with('.') || n.starts_with("__") {
            continue;
        }
        let fp = scenario_fingerprint(&e.path())?;
        out.push((n, fp));
    }
    out.sort();
    Ok(out)
}

/// The sealed manifest (`bench/bdd/port_pins/<cat>.json`), as name -> fingerprint.
fn read_pin(path: &Path) -> Result<Vec<(String, String)>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading the seal {}", path.display()))?;
    let v: Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing the seal {}", path.display()))?;
    let arr = v
        .get("sources")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("{}: no `sources` array", path.display()))?;
    let mut out: Vec<(String, String)> = Vec::new();
    for it in arr {
        let name = it
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("{}: a `sources` entry has no `name`", path.display()))?;
        let fp = it
            .get("fingerprint")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        out.push((name.to_string(), fp));
    }
    out.sort();
    Ok(out)
}

/// `laya-workflow bdd port-check`: prove a cross-repo conversion is *closed*.
/// Every upstream scenario must have both a `@decision` feature and a checked-in
/// spec, and no port may exist without a source. The within-repo reverse
/// direction (a spec with no feature) is already proven by `bdd round-trip`;
/// this adds the one thing that needs the second checkout.
pub fn port_check(o: &PortCheckOptions) -> Result<i32> {
    let live: Option<Vec<(String, String)>> = if o.sources.as_os_str().is_empty() {
        None
    } else {
        if !o.sources.is_dir() {
            bail!("--sources {} is not a directory", o.sources.display());
        }
        Some(read_sources_sealed(&o.sources)?)
    };

    // Sealing is a deliberate act, never a gate: it writes the manifest from a
    // checkout a reviewer just compared by hand.
    if let Some(path) = &o.write_pin {
        let Some(live) = &live else {
            bail!("--write-pin needs --sources <DIR>: there is nothing to seal");
        };
        let arr: Vec<Value> = live
            .iter()
            .map(|(n, fp)| json!({"name": n, "fingerprint": fp}))
            .collect();
        let doc = json!({
            "cat": o.cat,
            "what": "The sealed upstream source set that the BDD port under bdd/workflows/<cat>/ \
                     and dsl/<cat>/ was converted from, one entry per scenario (fingerprint = \
                     FNV-1a 64 over its top-level .py files). `bdd port-check --pin <this>` \
                     proves the port is still exactly this set without a second checkout; adding \
                     `--sources <checkout>` re-verifies the seal itself (same names, same source \
                     fingerprints), so a change upstream cannot pass unnoticed.",
            "sources": arr,
        });
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        std::fs::write(path, format!("{}\n", serde_json::to_string_pretty(&doc)?))?;
        println!(
            "bdd port-check: sealed {} source(s) into {}",
            live.len(),
            path.display()
        );
        // Sealing is an act, not a verdict: it says nothing about the port, so it
        // does not pretend to. The gate that *does* judge is `--pin`.
        return Ok(0);
    }

    // Where the source *names* come from. With a live checkout that is the
    // authority; without one, the seal stands in for it. Never the port set
    // itself - that would be comparing the port against itself.
    let names_from = if live.is_some() { "live checkout" } else { "sealed manifest" };
    let source_names: BTreeSet<String> = match (&live, &o.pin) {
        (Some(live), _) => live.iter().map(|(n, _)| n.clone()).collect(),
        (None, Some(pin)) => read_pin(pin)?.iter().map(|(n, _)| n.clone()).collect(),
        (None, None) => bail!(
            "port-check needs --sources <DIR> (a live upstream checkout) or --pin <FILE> \
             (the sealed source set under bench/bdd/port_pins/)"
        ),
    };

    // Given both, the seal itself is on trial: same names, same fingerprints.
    let mut seal_problems: Vec<String> = Vec::new();
    if let (Some(live), Some(pin)) = (&live, &o.pin) {
        let sealed = read_pin(pin)?;
        let live_names: BTreeSet<String> = live.iter().map(|(n, _)| n.clone()).collect();
        let sealed_names: BTreeSet<String> = sealed.iter().map(|(n, _)| n.clone()).collect();
        for n in live_names.difference(&sealed_names) {
            seal_problems.push(format!(
                "the upstream source {n:?} is not in the seal - port it, then re-seal with \
                 --write-pin"
            ));
        }
        for n in sealed_names.difference(&live_names) {
            seal_problems.push(format!(
                "the seal lists {n:?} but the upstream checkout has no such scenario - the seal \
                 outlived its source"
            ));
        }
        for (n, fp) in live {
            if let Some((_, sfp)) = sealed.iter().find(|(s, _)| s == n) {
                // An empty fingerprint is a *name-only* seal: the source set was
                // sealed, its contents were not (the checked-in copy of a corpus
                // that lives in another repo seals names, so no bytes of that
                // corpus are quoted here). Only a seal that carries a
                // fingerprint is compared against the live source.
                if !sfp.is_empty() && sfp != fp {
                    seal_problems.push(format!(
                        "the upstream source {n:?} changed since it was sealed ({sfp} -> {fp}) - \
                         re-check the port, then re-seal"
                    ));
                }
            }
        }
    }

    let root = repo_root();
    let sources = source_names;
    let features = read_stem_set(&root.join("bdd").join("workflows").join(&o.cat), "feature");
    let specs = read_stem_set(&root.join("dsl").join(&o.cat), "json");
    let diff = diff_port_sets(&sources, &features, &specs);

    if let Some(path) = &o.json {
        let report = json!({
            "sources": o.sources.to_string_lossy(),
            "names_from": names_from,
            "seal_problems": seal_problems.clone(),
            "cat": o.cat,
            "counts": {
                "sources": sources.len(),
                "features": features.len(),
                "specs": specs.len()
            },
            "diff": {
                "sources_no_feature": diff.sources_no_feature,
                "sources_no_spec": diff.sources_no_spec,
                "features_no_source": diff.features_no_source,
                "specs_no_source": diff.specs_no_source
            }
        });
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        std::fs::write(path, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
    }

    let label = |v: &[String]| -> String {
        if v.is_empty() {
            "-".into()
        } else {
            v.join(", ")
        }
    };
    println!(
        "bdd port-check: {} source(s) / {} feature(s) / {} spec(s)  (cat {})",
        sources.len(),
        features.len(),
        specs.len(),
        o.cat
    );
    for p in &seal_problems {
        eprintln!("bdd port-check: {p}");
    }
    if diff.is_empty() && seal_problems.is_empty() {
        println!(
            "bdd port-check: closed against the {names_from} ({} source(s) / {} feature(s) / {} \
             spec(s)) - every source has a feature and a spec, every port has a source, and the \
             seal is intact",
            sources.len(),
            features.len(),
            specs.len()
        );
        return Ok(0);
    }
    eprintln!("bdd port-check: {} drift(s):", diff.total());
    if !diff.sources_no_feature.is_empty() {
        eprintln!(
            "  source with no feature: {}",
            label(&diff.sources_no_feature)
        );
    }
    if !diff.sources_no_spec.is_empty() {
        eprintln!("  source with no spec:    {}", label(&diff.sources_no_spec));
    }
    if !diff.features_no_source.is_empty() {
        eprintln!(
            "  feature with no source: {}",
            label(&diff.features_no_source)
        );
    }
    if !diff.specs_no_source.is_empty() {
        eprintln!("  spec with no source:    {}", label(&diff.specs_no_source));
    }
    Ok(1)
}

// ── scope census: "convert all known workflows" as a bounded, checkable claim ──

/// How many upstream workflows a scope census accounts for.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScopeCounts {
    pub universes: usize,
    /// Upstream names with an explicit BDD port (`ported`).
    pub ported: usize,
    /// Upstream names deliberately not ported, each with a reason (`search_space`).
    pub search_space: usize,
    /// Universes declared `ported_all` (a full conversion proven by `bdd port-check`).
    pub ported_all: usize,
    /// Universes declared `enumerate: false` - a private upstream `--sources` cannot
    /// list; their names are pinned in the census and every ported feature is still
    /// existence-checked.
    pub non_enumerable: usize,
    /// Live upstream workflow files enumerated (only with `--sources`).
    pub sources: usize,
}

pub struct ScopeCheckOptions {
    /// An ole-eval (or similar) checkout root; the census `upstream` paths are
    /// resolved under it. Optional: offline the census is judged on its own, and
    /// with it every listed name must match the live `.py` set.
    pub sources: Option<String>,
    /// Write the report as JSON here.
    pub json: Option<String>,
}

impl Default for ScopeCheckOptions {
    fn default() -> Self {
        Self { sources: None, json: None }
    }
}

/// The census gate as a pure function of the census text (plus an optional live
/// checkout), so a regression can hand in a mutated census and watch this same
/// code report it. A *universe* is an upstream directory of workflows; every
/// workflow in it is either `ported` (with the BDD feature that is its
/// decision-graph port) or `search_space` (deliberately not ported, with the
/// reason). A name that is neither, once `--sources` is given, is an upstream
/// workflow nobody accounted for - exactly the failure "convert all known
/// workflows" must not have.
pub fn scope_check_impl(
    root: &Path,
    census: &str,
    sources: Option<&Path>,
) -> Result<(ScopeCounts, Vec<String>)> {
    let v: Value = serde_json::from_str(census).with_context(|| "parsing the scope census")?;
    let universes = v
        .get("universes")
        .and_then(Value::as_array)
        .with_context(|| "scope census: `universes` must be an array")?;
    let mut counts = ScopeCounts::default();
    let mut problems: Vec<String> = Vec::new();
    for u in universes {
        let id = u.get("id").and_then(Value::as_str).unwrap_or("<unnamed>");
        let Some(upstream) = u.get("upstream").and_then(Value::as_str) else {
            problems.push(format!(
                "{id}: `upstream` is required (the repo-relative directory of workflows)"
            ));
            continue;
        };
        counts.universes += 1;
        let ported = u.get("ported").and_then(Value::as_object);
        let search = u.get("search_space").and_then(Value::as_object);
        let ported_all = u.get("ported_all").and_then(Value::as_bool).unwrap_or(false);
        if ported_all {
            counts.ported_all += 1;
        } else if ported.is_none() && search.is_none() {
            problems.push(format!(
                "{id}: a universe must carry `ported`, `search_space`, or `ported_all` - \
                 silence is not a disposition"
            ));
            continue;
        }

        if let Some(p) = ported {
            for (name, feat) in p {
                if let Some(feat) = feat.as_str() {
                    counts.ported += 1;
                    match std::fs::read_to_string(root.join(feat)) {
                        Err(_) => problems.push(format!(
                            "{id}: {name} is ported to {feat}, but that file does not exist"
                        )),
                        Ok(txt) if !txt.lines().any(|l| l.starts_with("@decision")) => {
                            problems.push(format!(
                                "{id}: {name} is ported to {feat}, but that feature is not a \
                                 @decision workflow"
                            ))
                        }
                        Ok(_) => {}
                    }
                } else {
                    problems
                        .push(format!("{id}: ported[{name:?}] must be a feature path string"));
                }
                if search.map_or(false, |s| s.contains_key(name)) {
                    problems.push(format!(
                        "{id}: {name} is listed both as ported and as a search-space - pick one"
                    ));
                }
            }
        }
        if let Some(s) = search {
            for (name, reason) in s {
                counts.search_space += 1;
                if reason.as_str().map(|r| r.trim().is_empty()).unwrap_or(true) {
                    problems.push(format!(
                        "{id}: search_space[{name:?}] needs a non-empty reason - a workflow is \
                         never quietly skipped"
                    ));
                }
            }
        }

        // A universe may declare `enumerate: false` when its upstream is not publicly
        // enumerable (a private repo, e.g. a set of ops scripts). Its names are pinned
        // here and every `ported` feature was existence-checked above; `--sources` has
        // no directory to list, so it is skipped rather than reported as a missing dir.
        let enumerable = u.get("enumerate").and_then(Value::as_bool).unwrap_or(true);
        if !enumerable {
            counts.non_enumerable += 1;
        }
        if let Some(src) = sources {
            if !enumerable {
                // pinned in-repo; nothing to enumerate live
            } else {
                let dir = src.join(upstream);
                if !dir.is_dir() {
                    problems.push(format!(
                        "{id}: --sources has no {upstream} under {}",
                        src.display()
                    ));
                } else {
                    let mut live: BTreeSet<String> = BTreeSet::new();
                    for e in std::fs::read_dir(&dir)?.flatten() {
                        let n = e.file_name().to_string_lossy().into_owned();
                        if let Some(stem) = n.strip_suffix(".py") {
                            if !stem.is_empty() && stem != "__init__" {
                                live.insert(stem.to_string());
                            }
                        }
                    }
                    if ported_all {
                        // A fully-ported universe is checked name-by-name by
                        // `bdd port-check`; here it only contributes to the count.
                        counts.sources += live.len();
                    } else {
                        let mut listed: BTreeSet<String> = BTreeSet::new();
                        if let Some(p) = ported {
                            listed.extend(p.keys().cloned());
                        }
                        if let Some(s) = search {
                            listed.extend(s.keys().cloned());
                        }
                        counts.sources += listed.len();
                        for n in live.difference(&listed) {
                            problems.push(format!(
                                "{id}: the upstream {upstream}/{n}.py is not in the census - port it, \
                                 or list it (with a reason) as a search-space"
                            ));
                        }
                        for n in listed.difference(&live) {
                            problems.push(format!(
                                "{id}: the census lists {n} but {upstream} has no {n}.py - the census \
                                 outlived its source"
                            ));
                        }
                    }
                }
            }
        }
    }
    problems.sort();
    problems.dedup();
    Ok((counts, problems))
}

pub fn scope_check(o: &ScopeCheckOptions) -> Result<i32> {
    let root = repo_root();
    let census = std::fs::read_to_string(root.join("bench").join("bdd").join("scope_census.json"))
        .with_context(|| "reading bench/bdd/scope_census.json")?;
    let sources: Option<PathBuf> = o.sources.as_ref().map(PathBuf::from);
    if let Some(s) = &sources {
        if !s.is_dir() {
            bail!("--sources {} is not a directory", s.display());
        }
    }
    let (counts, problems) = scope_check_impl(&root, &census, sources.as_deref())?;
    if let Some(path) = &o.json {
        let report = json!({
            "sources_root": o.sources.clone().unwrap_or_default(),
            "counts": {
                "universes": counts.universes,
                "ported": counts.ported,
                "search_space": counts.search_space,
                "ported_all": counts.ported_all,
                "non_enumerable": counts.non_enumerable,
                "sources": counts.sources
            },
            "problems": problems.clone(),
        });
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        std::fs::write(path, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
    }
    for p in &problems {
        eprintln!("bdd scope-check: {p}");
    }
    if !problems.is_empty() {
        eprintln!("bdd scope-check: {} problem(s)", problems.len());
        return Ok(1);
    }
    println!(
        "bdd scope-check: {} upstream universe(s) accounted for - {} ported in full (proven by \
         `bdd port-check`), {} workflow(s) named ({} ported + {} search-space), {} non-enumerable{}",
        counts.universes,
        counts.ported_all,
        counts.ported + counts.search_space,
        counts.ported,
        counts.search_space,
        counts.non_enumerable,
        if sources.is_some() {
            " - the census matches the live checkout"
        } else {
            " (offline; pass --sources <ole-eval root> to match the live checkout)"
        }
    );
    Ok(0)
}

/// Round-trip gate: the README cites its line, so the doc gate needs it without
/// the printing — the same `(rc, Some(success line))` shape `check_vocabulary_impl`
/// uses, so a gate's number is never compared against a copy of itself.
fn round_trip_impl(silent: bool, o: &RoundTripOptions) -> Result<(i32, Option<String>)> {
    let root = repo_root();
    let wf_root = root.join("bdd").join("workflows");
    let mut features: Vec<PathBuf> = Vec::new();
    if wf_root.is_dir() {
        walk_features(&wf_root, &mut features);
    }
    features.sort();

    let mut total = 0usize;
    let mut matched = 0usize;
    let mut problems: Vec<String> = Vec::new();
    let mut verdicts: Vec<Value> = Vec::new();
    // (cat, name) of every @decision feature, so the dsl/ walk below can prove
    // the conversion is closed: no dsl spec may exist that has no BDD source.
    let mut feature_names: BTreeSet<(String, String)> = BTreeSet::new();

    for f in &features {
        let text = std::fs::read_to_string(f)
            .with_context(|| format!("reading {}", f.display()))?;
        let feature = parse(&text, &f.to_string_lossy())?;
        if !has_tag(&feature.tags, "decision") {
            continue;
        }
        total += 1;
        let rel = f.strip_prefix(&wf_root).unwrap().to_string_lossy().into_owned();
        let rel = rel.trim_start_matches('/');
        let (cat, fname) = match rel.rsplit_once('/') {
            Some((c, n)) => (c, n),
            None => ("", rel),
        };
        let name = fname.strip_suffix(".feature").unwrap_or(fname);
        feature_names.insert((cat.to_string(), name.to_string()));
        let dsl_path = root.join("dsl").join(cat).join(format!("{name}.json"));

        let spec = match compile_decision(&feature, &json!({})) {
            Ok(s) => s,
            Err(e) => {
                problems.push(format!("{rel}: compile failed: {e:#}"));
                verdicts.push(json!({"feature": rel, "ok": false, "reason": format!("compile: {e:#}")}));
                continue;
            }
        };
        // Decision specs may reference scratch dirs (${env.LAYA_*}) in
        // policy.allow_paths; guarantee them the same way `bdd build` does so
        // validate resolves them in CI, where no such vars exist.
        if let Ok(env_map) = crate::dsl_smoke::ensure_env_dirs() {
            for (k, v) in env_map {
                std::env::set_var(k, v);
            }
            let _ = crate::capability::secret::init(None);
        }
        if let Err(e) = crate::spec::from_spec_with_dir(&spec, None, None) {
            problems.push(format!("{rel}: generated spec fails validate: {e:#}"));
            verdicts.push(json!({"feature": rel, "ok": false, "reason": format!("validate: {e:#}")}));
            continue;
        }
        let dsl_text = match std::fs::read_to_string(&dsl_path) {
            Ok(t) => t,
            Err(_) => {
                problems.push(format!("{rel}: no dsl counterpart at {}", dsl_path.display()));
                verdicts.push(json!({"feature": rel, "ok": false, "reason": "no dsl counterpart"}));
                continue;
            }
        };
        let dsl: Value = serde_json::from_str(&dsl_text)
            .with_context(|| format!("parsing {}", dsl_path.display()))?;

        let nb = rt_norm(&spec);
        let nd = rt_norm(&dsl);
        if nb == nd {
            matched += 1;
            verdicts.push(json!({"feature": rel, "ok": true}));
        } else {
            let diff = rt_first_diff(&nb, &nd, "spec").unwrap_or_else(|| "non-equal".to_string());
            problems.push(format!("{rel}: {diff}"));
            verdicts.push(json!({"feature": rel, "ok": false, "reason": diff}));
        }
    }

    // Completeness, the reverse direction: every checked-in dsl spec must have
    // a BDD source. A spec with no `.feature` is a known workflow that has not
    // been converted — the round-trip loop above cannot see it (it walks the
    // features), so it is proven here instead.
    let dsl_root = root.join("dsl");
    if dsl_root.is_dir() {
        let mut specs: Vec<(String, String)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dsl_root) {
            for e in rd.flatten() {
                let p = e.path();
                if !p.is_dir() {
                    continue;
                }
                let cat = e.file_name().to_string_lossy().into_owned();
                if let Ok(sd) = std::fs::read_dir(&p) {
                    for f in sd.flatten() {
                        let fp = f.path();
                        if fp.extension().map(|x| x == "json").unwrap_or(false) {
                            // `<name>.config.json` sidecars are config, not specs:
                            // they have no BDD source and must not count as orphans.
                            if fp
                                .file_name()
                                .map(|n| n.to_string_lossy().ends_with(".config.json"))
                                .unwrap_or(false)
                            {
                                continue;
                            }
                            let name = fp
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            specs.push((cat.clone(), name));
                        }
                    }
                }
            }
        }
        specs.sort();
        for (cat, name) in specs {
            if !feature_names.contains(&(cat.clone(), name.clone())) {
                problems.push(format!(
                    "dsl/{cat}/{name}.json has no @decision BDD source at bdd/workflows/{cat}/{name}.feature - convert it"
                ));
            }
        }
    }

    if o.json.is_some() {
        let out = root.join(o.json.as_ref().unwrap());
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out, serde_json::to_string_pretty(&verdicts)?)?;
    }

    let line = format!(
        "bdd roundtrip: {matched}/{total} decision workflows reproduce their dsl spec"
    );
    if !silent {
        println!("{line}");
        for p in &problems {
            println!("  {p}");
        }
        if !problems.is_empty() {
            eprintln!("bdd roundtrip: FAIL - {}/{} drifted", problems.len(), total);
        }
    }
    let rc = if problems.is_empty() { 0 } else { 1 };
    Ok((rc, Some(line)))
}

pub fn round_trip(o: &RoundTripOptions) -> Result<i32> {
    let (rc, _) = round_trip_impl(false, o)?;
    Ok(rc)
}

/// The verdict of an app-parity walk: how many reference cases exist, how many
/// reproduced the app's expected label through their BDD port, and every case
/// that did not.
#[derive(Debug, Clone, Default)]
pub struct AppParityReport {
    pub total: usize,
    pub matched: usize,
    pub problems: Vec<String>,
    pub verdicts: Vec<Value>,
}

pub struct AppParityOptions {
    /// Write the per-case verdicts as JSON here.
    pub json: Option<String>,
}

impl Default for AppParityOptions {
    fn default() -> Self {
        Self { json: None }
    }
}

/// Replay every built-in app's reference case (`crate::apps::all_reference_cases`,
/// the same cases `laya-workflow apps` scores) end to end through the app's
/// checked-in BDD port spec, and require the label the engine projects to match
/// the app's expectation.
///
/// The port is the app's routing re-expressed as *data*; the app is *code*. This
/// holds the two together case-for-case, offline (no Chrome, no model), so a port
/// that silently drifts from the app it claims to reproduce fails here rather than
/// passing unnoticed. `root` anchors the port paths (`dsl/...`) so a test can point
/// it at a mutated copy and prove the gate actually reads the spec.
/// Run one checked-in port spec on `state` and return the decision label the
/// engine projects (the same projection `bdd_decide` uses).
fn port_decision_label(root: &std::path::Path, spec_rel: &str, state: &Value) -> Result<String> {
    let backend = crate::backend::HeuristicBackend;
    let wf = crate::spec::load_file(&root.join(spec_rel).to_string_lossy())?;
    let out = wf.run(&backend, state)?;
    let j = out.to_json();
    Ok(crate::bdd_mcp::decision_label(
        j.get("result").unwrap_or(&Value::Null),
        out.final_action(),
    ))
}

pub fn app_parity_impl(root: &std::path::Path) -> Result<AppParityReport> {
    let mut report = AppParityReport::default();

    // Primitive apps: the port must reproduce the app's published reference label.
    for c in crate::apps::all_reference_cases() {
        report.total += 1;
        let label = match port_decision_label(root, c.bdd_spec, &c.state) {
            Ok(l) => l,
            Err(e) => {
                report
                    .problems
                    .push(format!("{}: port {} failed on {}: {e:#}", c.app, c.bdd_spec, c.state));
                report.verdicts.push(json!({
                    "app": c.app, "port": c.bdd_spec, "input": c.state,
                    "expected": c.expected, "ok": false, "reason": format!("{e:#}"),
                }));
                continue;
            }
        };
        let ok = c.expected.split('|').any(|e| e.trim() == label);
        if ok {
            report.matched += 1;
        } else {
            report.problems.push(format!(
                "{}: port {} produced `{label}`, expected `{}` for state {}",
                c.app, c.bdd_spec, c.expected, c.state
            ));
        }
        report.verdicts.push(json!({
            "app": c.app, "port": c.bdd_spec, "input": c.state,
            "label": label, "expected": c.expected, "ok": ok,
        }));
    }

    // Composed app: the built-in gateway (gate -> triage). Both the APP and its
    // port are driven and both must land in the expected label set — the app
    // proves the expectation is its real behaviour, the port proves it reproduces
    // it. (The app writes its verdict under `action`, the port under `label`.)
    for (name, state, expected) in crate::apps::gateway_reference_cases() {
        report.total += 1;
        let want = |label: &str| expected.split('|').any(|e| e.trim() == label);

        let app_label = match crate::apps::gateway_workflow()
            .run(&crate::backend::HeuristicBackend, &state)
        {
            Ok(out) => out
                .state
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string(),
            Err(e) => {
                report.problems.push(format!("gateway/{name}: app run failed: {e:#}"));
                report.verdicts.push(json!({
                    "app": "gateway", "case": name, "input": state,
                    "expected": expected, "ok": false, "reason": format!("app: {e:#}"),
                }));
                continue;
            }
        };
        let port_label = match port_decision_label(root, crate::apps::GATEWAY_BDD_SPEC, &state) {
            Ok(l) => l,
            Err(e) => {
                report.problems.push(format!(
                    "gateway/{name}: port {} failed: {e:#}",
                    crate::apps::GATEWAY_BDD_SPEC
                ));
                report.verdicts.push(json!({
                    "app": "gateway", "case": name, "input": state,
                    "expected": expected, "ok": false, "reason": format!("port: {e:#}"),
                }));
                continue;
            }
        };
        let ok = want(&app_label) && want(&port_label);
        if ok {
            report.matched += 1;
        } else if !want(&app_label) {
            report
                .problems
                .push(format!("gateway/{name}: app produced `{app_label}`, expected `{expected}`"));
        } else {
            report.problems.push(format!(
                "gateway/{name}: port {} produced `{port_label}`, expected `{expected}` (app said `{app_label}`)",
                crate::apps::GATEWAY_BDD_SPEC
            ));
        }
        report.verdicts.push(json!({
            "app": "gateway", "case": name, "input": state,
            "app_label": app_label, "port_label": port_label,
            "expected": expected, "ok": ok,
        }));
    }

    Ok(report)
}

/// CLI gate over [`app_parity_impl`]: exits non-zero if any reference case
/// diverges. The cases live in `crate::apps`; the port paths are checked in
/// under `dsl/`.
pub fn app_parity_check(o: &AppParityOptions) -> Result<i32> {
    let root = repo_root();
    let report = app_parity_impl(&root)?;
    if let Some(p) = &o.json {
        let out = root.join(p);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out, serde_json::to_string_pretty(&report.verdicts)?)?;
    }
    println!(
        "bdd app-parity: {}/{} reference cases reproduce the app's label through their BDD port spec",
        report.matched, report.total
    );
    for p in &report.problems {
        println!("  {p}");
    }
    if !report.problems.is_empty() {
        eprintln!(
            "bdd app-parity: FAIL - {} of {} cases diverged",
            report.problems.len(),
            report.total
        );
    }
    Ok(if report.problems.is_empty() { 0 } else { 1 })
}

/// Recursively collect `*.feature` files under a directory.
fn walk_features(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk_features(&p, out);
            } else if p.extension().map(|x| x == "feature").unwrap_or(false) {
                out.push(p);
            }
        }
    }
}

// ── live-fire corpus (`laya-workflow bdd live`) ─────────────────────────────
//
// The hermetic suite runs against fixtures; this runs the same generic workflow
// vocabulary against *real* external sites, one feature per case, each with a
// sidecar `<name>.config.json` naming the hosts its steps navigate to. It is the
// same `run()` engine, told not to start fixtures and not to add the fixture
// probes — so a case is real Chrome against the real web or it is nothing.

pub struct LiveOptions {
    pub features: Vec<String>,
    pub filter: Option<String>,
    pub out: String,
    pub timeout_secs: u64,
    pub json: Option<String>,
}

impl Default for LiveOptions {
    fn default() -> Self {
        Self {
            features: Vec::new(),
            filter: None,
            out: repo_root().join("target").join("bdd-live").to_string_lossy().into_owned(),
            timeout_secs: 180,
            json: None,
        }
    }
}

/// Every `bdd/workflows/browser/*.feature` except the `_`-prefixed development probe.
pub fn live_features() -> Result<Vec<PathBuf>> {
    let dir = repo_root().join("bdd").join("workflows").join("browser");
    if !dir.is_dir() {
        bail!("no live corpus at {} - nothing to run", dir.display());
    }
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().map(|x| x == "feature").unwrap_or(false)
                && !p.file_name().map(|n| n.to_string_lossy().starts_with('_')).unwrap_or(true)
        })
        // @decision features (one decision workflow per feature) are compiled by
        // the decision-graph path and gated by the decision loop in check.sh,
        // not by the browser live corpus. Excluding them keeps `bdd compare` /
        // `bdd live` on the browser-step features only.
        .filter(|p| {
            let text = std::fs::read_to_string(p).unwrap_or_default();
            !text.lines().any(|l| l.trim_start().starts_with("@decision"))
        })
        .collect();
    out.sort();
    Ok(out)
}

pub fn live(o: &LiveOptions) -> Result<i32> {
    let mut features = if o.features.is_empty() {
        live_features()?.iter().map(|p| p.to_string_lossy().into_owned()).collect()
    } else {
        o.features.clone()
    };
    if let Some(f) = &o.filter {
        features.retain(|p| p.contains(f.as_str()));
    }
    if features.is_empty() {
        bail!("no live features selected");
    }
    let ro = RunOptions {
        features,
        filter: None,
        tags: None,
        profile: "live".into(),
        base_url: None,
        port: 0,
        jobs: 1,
        timeout_secs: o.timeout_secs,
        keep: false,
        out: o.out.clone(),
        live: true,
        report: o.json.clone(),
        // `bdd live` exists only to drive a real browser against real sites; a
        // missing browser there is a failure, not something to skip quietly.
        require_browser: true,
        // Live-fire launches its own browser; attaching is a `bdd run --cdp` knob.
        cdp: None,
    };
    run(&ro)
}

// ── Decision-scoped live gate (`laya-workflow bdd live-decisions`) ──────────
//
// `dsl smoke --base-url <url>` is the only way to drive the whole decision
// corpus through a live System-1 backend, but it *also* gates on the offline
// EXPECT table — and a live model's labels are mostly *meant* to differ from
// the heuristic-shaped offline expectations (exact-arithmetic inputs, a fixed
// role order, …). So a live `dsl smoke` reports noise, not signal.
//
// This is the decision-scoped live variant: drive every `@decision` workflow's
// registered sample states through the live backend and ask the **structural**
// question alone — did every state terminate, in-contract, with no backend
// error? Label agreement with the offline expectation is measured and printed,
// but it is *information*, never a failure: the gate target is "the whole
// decision corpus walks cleanly through the deployed System-1 model", not "the
// model reproduces offline labels".

pub struct LiveDecisionsOptions {
    /// Live laya-tch / soft-decider base URL (default $BDD_BASE_URL, else
    /// http://127.0.0.1:18405 — the deployed decision service).
    pub base_url: Option<String>,
    /// Only walk features whose path or stem contains this substring.
    pub filter: Option<String>,
    /// Write the per-spec verdicts as JSON here.
    pub json: Option<String>,
}

impl Default for LiveDecisionsOptions {
    fn default() -> Self {
        Self { base_url: None, filter: None, json: None }
    }
}

/// Per-spec live verdict. `structural_error` is `None` when the whole spec
/// walked cleanly (compiles, validates, AND every state **terminated** — the
/// engine returns `Ok` even when a walk hits its safety valve, see
/// [`walk_outcome_clean`]); a label that disagreed with the offline expectation
/// is *not* an error. `states_nonterminating` counts walks that ended at the
/// safety valve (a structural failure); `states_looped` counts walks the engine's
/// revisit guard observed (informational — some graphs loop on purpose).
#[derive(Clone, Debug, Default)]
pub struct SpecLiveResult {
    pub rel: String,
    pub structural_error: Option<String>,
    /// P199: the live backend failed (transport / HTTP status / malformed
    /// response) while walking this spec. Distinct from `structural_error`: the
    /// graph did not regress, the System-1 backend could not answer.
    pub backend_error: Option<String>,
    pub states_run: u32,
    pub states_agreed: u32,
    pub states_skipped: u32,
    pub states_nonterminating: u32,
    pub states_looped: u32,
}

/// Did a completed walk actually *terminate*? `ResilientWorkflow::run` returns
/// `Ok` in every case — including when the walk hits the engine's safety valve —
/// so a structural gate cannot read `Ok` as "clean". A `final_action` of
/// `max_iterations` (the iteration cap was reached) or `error_node_missing`
/// (a routed node was not in the graph) means the walk never reached a real
/// terminal (`stop` / `escalate`) or a convergence stop (`converged`), so it is
/// **not** a clean walk. Pure, so the regression can pin the rule.
pub fn walk_outcome_clean(final_action: &str) -> bool {
    final_action != "max_iterations" && final_action != "error_node_missing"
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiveSummary {
    pub specs_total: u32,
    pub specs_clean: u32,
    /// P199: specs that regressed (non-terminating / engine error).
    pub specs_structural_failure: u32,
    /// P199: specs the live backend could not answer (inconclusive, not a
    /// corpus regression).
    pub specs_backend_error: u32,
    pub states_run: u32,
    pub states_agreed: u32,
    pub states_skipped: u32,
    pub states_nonterminating: u32,
    pub states_looped: u32,
}

/// Fold per-spec live verdicts into the gate's numbers. Pure, so the regression
/// can pin the "a label mismatch is not a failure" rule without a live backend:
/// `specs_clean` counts specs with **no** structural error — a clean spec whose
/// labels all disagreed is still clean.
pub fn summarize_live(per_spec: &[SpecLiveResult]) -> LiveSummary {
    let mut s = LiveSummary { specs_total: per_spec.len() as u32, ..Default::default() };
    for r in per_spec {
        let structural = r.structural_error.is_some() || r.states_nonterminating > 0;
        if structural {
            s.specs_structural_failure += 1;
        }
        if r.backend_error.is_some() {
            s.specs_backend_error += 1;
        }
        if !structural && r.backend_error.is_none() {
            s.specs_clean += 1;
        }
        s.states_run += r.states_run;
        s.states_agreed += r.states_agreed;
        s.states_skipped += r.states_skipped;
        s.states_nonterminating += r.states_nonterminating;
        s.states_looped += r.states_looped;
    }
    s
}

/// The environment gap a live decision walk must skip instead of failing: a
/// capability gated to another platform, an optional on-device asset that is not
/// installed (the needle engine/weights), or a `shell` command whose fixed
/// program is not on `$PATH`. Pure, so the regression can pin the rule without a
/// live backend.
///
/// P198: `bdd live-decisions` is a gate over the **decision corpus**, not over
/// the host. The three families are exactly the ones `dsl smoke` already skips
/// (`platform_bound_here` / `offline_asset_bound` / `binary_bound`); a host that
/// cannot exercise a state must report it *skipped*, never as a model
/// regression. The run-time sibling of the third family — a bare not-found
/// spawn error from an `exec` / `db` / plugin-spawned program — is classified by
/// `dsl_smoke::missing_program` at the call site.
pub fn live_env_gap(spec: &Value) -> Option<String> {
    if crate::dsl_smoke::platform_bound_here(spec) {
        return Some("platform-bound capability (not available on this OS)".to_string());
    }
    if let Some(why) = crate::dsl_smoke::offline_asset_bound(spec) {
        return Some(why);
    }
    crate::dsl_smoke::binary_bound(spec)
}

fn load_bench_table(name: &str) -> Result<BTreeMap<String, Value>> {
    let path = repo_root().join("bench").join(name);
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let parsed: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parse {}", path.display()))?;
    match parsed {
        Value::Object(m) => Ok(m.into_iter().collect()),
        _ => bail!("{} must be a JSON object", path.display()),
    }
}

pub fn live_decisions(o: &LiveDecisionsOptions) -> Result<i32> {
    let root = repo_root();
    let wf_root = root.join("bdd").join("workflows");
    let mut features: Vec<PathBuf> = Vec::new();
    if wf_root.is_dir() {
        walk_features(&wf_root, &mut features);
    }
    features.sort();

    let states = load_bench_table("dsl_smoke_states.json")?;
    let expect = load_bench_table("dsl_smoke_expect.json")?;
    // Guarantee the scratch env vars a decision spec may reference resolve, so
    // `from_spec_with_dir` validates in a bare shell (same as `bdd round-trip`).
    if let Ok(env_map) = crate::dsl_smoke::ensure_env_dirs() {
        for (k, v) in env_map {
            std::env::set_var(k, v);
        }
        let _ = crate::capability::secret::init(None);
    }

    let base_url = o
        .base_url
        .clone()
        .or_else(|| std::env::var("BDD_BASE_URL").ok())
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:18405".to_string());
    let backend = crate::backend::LayaBackend::new(&base_url);

    let mut per_spec: Vec<SpecLiveResult> = Vec::new();
    for f in &features {
        let text = std::fs::read_to_string(f)
            .with_context(|| format!("reading {}", f.display()))?;
        let feature = parse(&text, &f.to_string_lossy())?;
        if !has_tag(&feature.tags, "decision") {
            continue;
        }
        let rel = f
            .strip_prefix(&wf_root)
            .map(|p| p.to_string_lossy().trim_start_matches('/').to_string())
            .unwrap_or_else(|_| f.to_string_lossy().into_owned());
        let name = f
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if let Some(filt) = &o.filter {
            if !rel.contains(filt.as_str()) && !name.contains(filt.as_str()) {
                continue;
            }
        }

        let spec = match compile_decision(&feature, &json!({})) {
            Ok(s) => s,
            Err(e) => {
                per_spec.push(SpecLiveResult {
                    rel,
                    structural_error: Some(format!("compile: {e:#}")),
                    ..Default::default()
                });
                continue;
            }
        };
        let wf = match crate::spec::from_spec_with_dir(&spec, None, None) {
            Ok(w) => w,
            Err(e) => {
                per_spec.push(SpecLiveResult {
                    rel,
                    structural_error: Some(format!("validate: {e:#}")),
                    ..Default::default()
                });
                continue;
            }
        };
        let Some(state_map) = states.get(&name).and_then(Value::as_object) else {
            // validate-only is a clean walk: there was nothing to drive.
            per_spec.push(SpecLiveResult { rel, ..Default::default() });
            continue;
        };

        // P198: `live-decisions` gates the *decision corpus*, not the host — the
        // same three "environment gap" families `dsl smoke` skips mean this host
        // cannot exercise the spec, not that the deployed model regressed.
        // Evaluated once per spec: they depend only on the spec and the machine.
        let env_gap = live_env_gap(&spec);
        let mut res = SpecLiveResult { rel, ..Default::default() };
        for (label, st) in state_map {
            if let Some(why) = &env_gap {
                eprintln!(
                    "       {label}: {why} — skipped, validate only (this host cannot exercise it)"
                );
                res.states_skipped += 1;
                continue;
            }
            match wf.run(&backend, st) {
                Ok(out) => {
                    // `run` returns Ok even at the safety valve (see
                    // `walk_outcome_clean`), so "returned" is not "terminated":
                    // a walk that ended at `max_iterations` / `error_node_missing`
                    // is a structural failure, not a clean walk.
                    let fa = out.trace.final_action.clone();
                    if !walk_outcome_clean(&fa) {
                        res.states_run += 1;
                        res.states_nonterminating += 1;
                        res.structural_error =
                            Some(format!("{label}: the walk did not terminate (final_action={fa})"));
                        break;
                    }
                    res.states_run += 1;
                    if out.trace.loop_detected {
                        res.states_looped += 1;
                    }
                    let want = expect
                        .get(&name)
                        .and_then(Value::as_object)
                        .and_then(|m| m.get(label))
                        .cloned();
                    if let Some(want) = &want {
                        if crate::dsl_smoke::check_expect(&out, want).is_empty() {
                            res.states_agreed += 1;
                        }
                    }
                }
                Err(e) => {
                    // P198: an `exec` / `db` (or plugin-spawned) program that is
                    // not installed surfaces as this spawn error; a bare name the
                    // OS cannot find is the environment lacking a tool, not a
                    // defect in the spec — skip validate-only, exactly like
                    // `dsl smoke`, instead of failing the spec.
                    if crate::dsl_smoke::missing_program(&e.to_string()).is_some() {
                        res.states_skipped += 1;
                        continue;
                    }
                    // A spec whose declared *local* endpoints are unreachable is
                    // an environment gap, not a model regression — skip it the
                    // way `dsl smoke` does.
                    if crate::dsl_smoke::network_bound_offline(&spec) {
                        res.states_skipped += 1;
                        continue;
                    }
                    // P199: a live-backend failure (transport / HTTP status /
                    // malformed response) is NOT a spec regression — the graph
                    // never got to run. Record it in its own bucket, named
                    // `[BACKEND]` on stderr, so the gate never blames the corpus
                    // for a broken / absent System-1 model.
                    if crate::backend::is_backend_error(&e) {
                        res.states_run += 1;
                        res.backend_error = Some(format!("{label}: {e}"));
                        break;
                    }
                    res.states_run += 1;
                    res.structural_error = Some(format!("{label}: {e}"));
                    break;
                }
            }
        }
        per_spec.push(res);
    }

    let s = summarize_live(&per_spec);
    for r in &per_spec {
        if let Some(err) = &r.structural_error {
            eprintln!("[FAIL] {}: {err}", r.rel);
        }
    }
    // P199: the backend-failure bucket is reported on its own line — a broken /
    // unreachable live backend must never look like a spec regression.
    for r in &per_spec {
        if let Some(err) = &r.backend_error {
            eprintln!("[BACKEND] {}: {err}", r.rel);
        }
    }
    // Only widen the summary line when the new bucket is non-empty, so the
    // all-clean output stays byte-identical to the pre-P199 gate.
    let mut buckets: Vec<String> = Vec::new();
    if s.specs_structural_failure > 0 {
        buckets.push(format!("{} structural-failure", s.specs_structural_failure));
    }
    if s.specs_backend_error > 0 {
        buckets.push(format!("{} backend-error", s.specs_backend_error));
    }
    let flags = if buckets.is_empty() {
        String::new()
    } else {
        format!(" [{}]", buckets.join(", "))
    };
    println!(
        "live decisions: {}/{} specs walked structurally clean{flags}; agreement {}/{} states ({} skipped, {} loop-guard, {} non-terminating); backend={}",
        s.specs_clean, s.specs_total, s.states_agreed, s.states_run, s.states_skipped,
        s.states_looped, s.states_nonterminating, base_url
    );

    if let Some(path) = &o.json {
        let report = json!({
            "base_url": base_url,
            "summary": {
                "specs_total": s.specs_total,
                "specs_clean": s.specs_clean,
                "specs_structural_failure": s.specs_structural_failure,
                "specs_backend_error": s.specs_backend_error,
                "states_run": s.states_run,
                "states_agreed": s.states_agreed,
                "states_skipped": s.states_skipped,
                "states_nonterminating": s.states_nonterminating,
                "states_looped": s.states_looped
            },
            "specs": per_spec.iter().map(|r| json!({
                "feature": r.rel,
                "clean": r.structural_error.is_none() && r.backend_error.is_none(),
                "error": r.structural_error,
                "backend_error": r.backend_error,
                "states_run": r.states_run,
                "states_agreed": r.states_agreed,
                "states_skipped": r.states_skipped,
                "states_nonterminating": r.states_nonterminating,
                "states_looped": r.states_looped
            })).collect::<Vec<_>>()
        });
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        std::fs::write(path, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
    }

    if s.specs_structural_failure > 0 {
        return Ok(1);
    }
    if s.specs_backend_error > 0 {
        // Inconclusive: the decision corpus could not be walked because the
        // live backend failed. Distinct from a structural regression (rc=1).
        return Ok(2);
    }
    Ok(0)
}

// ── BDD vs pure JSON DSL (`laya-workflow bdd compare`) ──────────────────────
//
// The point of a Gherkin step vocabulary is that it is a *cheaper* authoring and
// debugging surface than the JSON spec it compiles to. This measures that on the
// live corpus, per case and in aggregate:
//
//   * volume  — non-comment .feature lines and bytes vs the compiled spec's
//               pretty JSON lines and bytes, and steps vs nodes (1:1);
//   * gate    — seed one fault in each representation and ask the gate the
//               author actually runs ("bdd build" here, "validate" there) whether
//               it is caught, offline, before a browser is ever opened.

pub struct CompareOptions {
    pub json: Option<String>,
    pub only: Vec<String>,
}

impl Default for CompareOptions {
    fn default() -> Self {
        Self { json: None, only: Vec::new() }
    }
}

struct CaseCompare {
    id: String,
    steps: usize,
    bdd_lines: usize,
    bdd_bytes: usize,
    json_lines: usize,
    json_bytes: usize,
    json_nodes: usize,
    bdd_fault_caught: bool,
    json_cap_fault_caught: bool,
    json_edge_fault_caught: bool,
    json_sem_fault_caught: bool,
    json_struct_fault_caught: bool,
}

fn noncomment_lines(text: &str) -> usize {
    text.lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#')
        })
        .count()
}

pub fn compare(o: &CompareOptions) -> Result<i32> {
    let feats = live_features()?;
    let tmp = tempfile_build_dir()?;
    let mut cases: Vec<CaseCompare> = Vec::new();
    let mut problems: Vec<String> = Vec::new();

    for path in &feats {
        let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if !o.only.is_empty() && !o.only.iter().any(|x| x == &id) {
            continue;
        }
        let rel = rel_path(&path.to_string_lossy());
        let text = std::fs::read_to_string(path)?;
        let feature = parse(&text, &path.to_string_lossy())?;
        let config = load_config(path)?;
        // Compile every scenario; the union of nodes is the JSON spec an author
        // would otherwise have to write by hand.
        let mut spec_nodes = 0usize;
        let mut first_spec: Option<Value> = None;
        let mut compile_ok = true;
        for sc in &feature.scenarios {
            match compile_scenario(&feature, sc, &config, "scripts/bdd/chrome-headless.sh") {
                Ok(spec) => {
                    spec_nodes += spec.get("nodes").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
                    if first_spec.is_none() {
                        first_spec = Some(public_spec(&spec));
                    }
                }
                Err(e) => {
                    compile_ok = false;
                    problems.push(format!("{rel}: does not compile: {e:#}"));
                }
            }
        }
        let Some(spec) = first_spec else { continue };

        let steps: usize = noncomment_lines(&text)
            .min(feature.background.len() + feature.scenarios.iter().map(|s| s.steps.len()).sum::<usize>());
        let bdd_steps = feature.background.len()
            + feature.scenarios.iter().map(|s| s.steps.len()).sum::<usize>();
        let bdd_lines = noncomment_lines(&text);
        let bdd_bytes = text.len();
        let spec_str = serde_json::to_string_pretty(&spec)?;
        let json_lines = spec_str.lines().count();
        let json_bytes = spec_str.len();

        // ── gate: one seeded fault per representation ──
        // BDD: an out-of-vocabulary verb. The deterministic compiler refuses it.
        let mutated_bdd = text.replacen("I click the element", "I tap the element", 1);
        let bdd_fault_caught = mutated_bdd != text
            && match parse(&mutated_bdd, &path.to_string_lossy()) {
                Ok(mf) => mf
                    .scenarios
                    .iter()
                    .any(|sc| compile_scenario(&mf, sc, &config, "scripts/bdd/chrome-headless.sh").is_err()),
                Err(_) => true,
            };

        // JSON: three *semantic* mistakes a JSON author makes — an unknown op,
        // an edge to a node that does not exist, an unknown assertion — each run
        // through the gate they actually run: `validate` (check_version +
        // load_file + registry + secret audit). Plus a structural control
        // (delete `start`) that `validate` is expected to catch, so a 0 on the
        // semantic faults is a real finding and not a broken probe.
        let load = |spec: &Value, name: &str| -> bool {
            let p = tmp.join(name);
            let _ = std::fs::write(&p, serde_json::to_string(&spec).unwrap_or_default());
            crate::spec::load_file(&p.to_string_lossy()).is_err()
        };
        // an unknown op on the first node that actually runs an effect.
        let mut m_a = spec.clone();
        if let Some(arr) = m_a.get_mut("nodes").and_then(Value::as_array_mut) {
            if let Some(n0) = arr.iter_mut().find(|n| n.pointer("/action/with").is_some()) {
                if let Some(w) = n0.pointer_mut("/action/with").and_then(Value::as_object_mut) {
                    w.insert("op".into(), json!("frobnicate"));
                }
            }
        }
        let json_cap_fault_caught = load(&m_a, &format!("{id}_badop.json"));

        // an edge to a node that is not in the graph.
        let mut m_b = spec.clone();
        if let Some(arr) = m_b.get_mut("nodes").and_then(Value::as_array_mut) {
            if let Some(n0) = arr.first_mut() {
                if let Some(e) = n0.get_mut("edge").and_then(|e| e.get_mut("condition")).and_then(Value::as_object_mut) {
                    e.insert("Z".into(), json!("no_such_node"));
                }
            }
        }
        let json_edge_fault_caught = load(&m_b, &format!("{id}_badedge.json"));

        // an unknown assertion — seed it on a real assert node when there is one.
        let mut m_c = spec.clone();
        if let Some(arr) = m_c.get_mut("nodes").and_then(Value::as_array_mut) {
            let idx = arr
                .iter()
                .position(|n| n.pointer("/action/with/op").and_then(Value::as_str) == Some("assert"))
                .or_else(|| arr.iter().position(|n| n.pointer("/action/with").is_some()));
            if let Some(i) = idx {
                if let Some(w) = arr[i].pointer_mut("/action/with").and_then(Value::as_object_mut) {
                    w.insert("assertion".into(), json!("frobnicate"));
                }
            }
        }
        let json_sem_fault_caught = load(&m_c, &format!("{id}_badsem.json"));

        // structural control: `validate` must reject a spec with no `start`.
        let mut m_d = spec.clone();
        if let Some(o) = m_d.as_object_mut() {
            o.remove("start");
        }
        let json_struct_fault_caught = load(&m_d, &format!("{id}_nostart.json"));

        let _ = (steps, compile_ok);
        cases.push(CaseCompare {
            id,
            steps: bdd_steps,
            bdd_lines,
            bdd_bytes,
            json_lines,
            json_bytes,
            json_nodes: spec_nodes,
            bdd_fault_caught,
            json_cap_fault_caught,
            json_edge_fault_caught,
            json_sem_fault_caught,
            json_struct_fault_caught,
        });
    }
    let _ = std::fs::remove_dir_all(&tmp);

    if cases.is_empty() {
        bail!("no live features to compare");
    }

    println!("bdd compare: BDD .feature vs the JSON spec it compiles to ({} case(s))", cases.len());
    println!(
        "  {:<34} {:>6} {:>6} {:>7} {:>7} {:>6}",
        "case", "steps", "bdd-ln", "json-ln", "ratio", "nodes"
    );
    let mut tot_steps = 0usize;
    let mut tot_bdd_bytes = 0usize;
    let mut tot_json_bytes = 0usize;
    let mut tot_bdd_lines = 0usize;
    let mut tot_json_lines = 0usize;
    let mut bdd_caught = 0usize;
    let mut json_caught = 0usize;
    let mut json_struct_caught = 0usize;
    for c in &cases {
        let ratio = c.json_bytes as f64 / c.bdd_bytes.max(1) as f64;
        println!(
            "  {:<34} {:>6} {:>6} {:>7} {:>6.1}x {:>6}",
            c.id, c.steps, c.bdd_lines, c.json_lines, ratio, c.json_nodes
        );
        tot_steps += c.steps;
        tot_bdd_bytes += c.bdd_bytes;
        tot_json_bytes += c.json_bytes;
        tot_bdd_lines += c.bdd_lines;
        tot_json_lines += c.json_lines;
        if c.bdd_fault_caught { bdd_caught += 1; }
        if c.json_cap_fault_caught { json_caught += 1; }
        if c.json_edge_fault_caught { json_caught += 1; }
        if c.json_sem_fault_caught { json_caught += 1; }
        if c.json_struct_fault_caught { json_struct_caught += 1; }
    }
    let n = cases.len();
    let byte_ratio = tot_json_bytes as f64 / tot_bdd_bytes.max(1) as f64;
    let line_ratio = tot_json_lines as f64 / tot_bdd_lines.max(1) as f64;
    println!();
    println!(
        "bdd compare: {tot_steps} steps, {tot_bdd_lines} .feature lines ({tot_bdd_bytes} B) vs \
         {tot_json_lines} JSON lines ({tot_json_bytes} B)"
    );
    println!(
        "bdd compare: authoring volume — JSON is {byte_ratio:.1}x the bytes and {line_ratio:.1}x the lines \
         of the same workflow in BDD"
    );
    println!(
        "bdd compare: authoring gate — BDD compiles a closed vocabulary, so an unknown step is a \
         compile error offline: caught {bdd_caught}/{n}."
    );
    println!(
        "bdd compare: authoring gate — the JSON author's `validate` is structural-only: it caught \
         {json_caught}/{} semantic faults (unknown op / dangling edge / unknown assertion), which \
         surface only when the workflow runs. Control: it caught {json_struct_caught}/{n} missing-`start` \
         faults, so the 0 is a real gap, not a broken probe.",
        n * 3
    );

    if let Some(path) = &o.json {
        let report = json!({
            "cases": cases.iter().map(|c| json!({
                "id": c.id, "steps": c.steps,
                "bdd_lines": c.bdd_lines, "bdd_bytes": c.bdd_bytes,
                "json_lines": c.json_lines, "json_bytes": c.json_bytes, "json_nodes": c.json_nodes,
                "bdd_fault_caught": c.bdd_fault_caught,
                "json_unknown_op_caught": c.json_cap_fault_caught,
                "json_edge_fault_caught": c.json_edge_fault_caught,
                "json_semantic_fault_caught": c.json_sem_fault_caught,
                "json_structural_fault_caught": c.json_struct_fault_caught,
            })).collect::<Vec<_>>(),
            "aggregate": {
                "cases": n, "steps": tot_steps,
                "bdd_bytes": tot_bdd_bytes, "json_bytes": tot_json_bytes,
                "bdd_lines": tot_bdd_lines, "json_lines": tot_json_lines,
                "byte_ratio": byte_ratio, "line_ratio": line_ratio,
                "bdd_fault_caught_offline": [bdd_caught, n],
                "json_semantic_fault_caught_offline": [json_caught, n * 3],
                "json_structural_fault_caught_offline": [json_struct_caught, n],
            }
        });
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("bdd compare: wrote {path}");
    }
    if !problems.is_empty() {
        for p in &problems {
            eprintln!("bdd compare: {p}");
        }
        return Ok(1);
    }
    Ok(0)
}
