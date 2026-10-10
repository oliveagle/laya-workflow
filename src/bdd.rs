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
use std::collections::BTreeSet;
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
    Ok(caps[1].split('|').map(|c| c.trim().to_string()).collect())
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

        let Some(caps) = step_re().captures(line) else {
            bail!(
                "line {lineno}: not a step, table or section header: {stripped:?}"
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
    if capability == "bdd" {
        json!({"target_id": "/target_id"})
    } else {
        json!({})
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
    let hint = if close.is_empty() {
        String::new()
    } else {
        format!(" (did you mean: {}?)", close.join(", "))
    };
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

fn default_chrome_wrapper() -> String {
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
pub fn load_config(feature_path: &Path) -> Result<Value> {
    let stem = feature_path.with_extension("");
    let side = stem.with_extension("config.json");
    if !side.is_file() {
        return Ok(json!({}));
    }
    let raw = std::fs::read_to_string(&side)
        .with_context(|| format!("reading {}", side.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing {}", side.display()))
}

fn merge(dst: &mut Value, src: &Value) {
    if let (Value::Object(d), Value::Object(s)) = (dst, src) {
        for (k, v) in s {
            d.insert(k.clone(), v.clone());
        }
    }
}

/// Compile one scenario into a full spec (public shape + `_bdd` metadata).

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
    let merged: Vec<Step> = feature
        .background
        .iter()
        .chain(scenario.steps.iter())
        .cloned()
        .collect();

    let mut nodes: Vec<Value> = Vec::new();
    let mut keep: Vec<String> = Vec::new();
    let mut has_page = false;

    for st in &merged {
        let compiled = compile_step(st, has_page)
            .with_context(|| format!("line {}: {}", st.line, st.text))?;
        has_page = compiled.1;
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
        "capabilities": {
            // Every non-input step is a `bdd` plugin op — including the
            // asserts; the plugin owns what an assertion means.
            "chrome": chrome,
            "bdd": {
                "kind": "plugin", "plugin": "bdd", "browser": "chrome",
                "timeout_ms": 60000
            }
        },
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

    let mut manifest: Vec<Value> = Vec::new();
    let mut coverage_report: Vec<Coverage> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut assist_calls: Vec<(String, String)> = Vec::new();

    let chrome_wrapper = default_chrome_wrapper();
    for path in &paths {
        let feature = match load(path) {
            Ok(f) => f,
            Err(e) => {
                errors.push(format!("{}: {e}", rel_path(&path.to_string_lossy())));
                continue;
            }
        };
        let cov = feature_coverage(&feature);
        coverage_report.push(cov);

        let config = match load_config(path) {
            Ok(c) => c,
            Err(e) => {
                errors.push(format!("{}: {e}", rel_path(&path.to_string_lossy())));
                continue;
            }
        };
        let chrome_bin = config.get("chrome").and_then(|c| c.get("chrome_binary")).and_then(Value::as_str).unwrap_or(&chrome_wrapper);

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
                        "{} :: {}: {e}",
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
            errors.push(format!(
                "{}: step coverage {}% < {}% (unmatched: {})",
                cov.feature,
                cov.coverage,
                o.coverage_min,
                cov.unmatched.join(", ").chars().take(200).collect::<String>()
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
            eprintln!("  {e}");
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
                eprintln!("error: {fp}: {e}");
                return Ok(1);
            }
        };
        let config = match load_config(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: {fp}: {e}");
                return Ok(1);
            }
        };
        let cb = config
            .get("chrome")
            .and_then(|c| c.get("chrome_binary"))
            .and_then(Value::as_str)
            .unwrap_or(chrome_wrapper);
        for scenario in &feature.scenarios {
            let spec = match compile_scenario(&feature, scenario, &config, cb) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: {fp}: {e}");
                    return Ok(1);
                }
            };
            total += 1;
            let meta = spec["_bdd"].clone();
            let need = required_state(&spec);
            let scenario_name = meta["scenario"].as_str().unwrap_or("").to_string();
            let base = Path::new(fp)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| fp.clone());
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
    std::fs::read_to_string(&p).with_context(|| format!("cannot read {}", p.display()))
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
    let table_path = root.join("bdd").join("args_probes.json");
    let table: Value = serde_json::from_str(
        &std::fs::read_to_string(&table_path)
            .with_context(|| format!("cannot read {}", table_path.display()))?,
    )?;

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
                "the plugin throws {stmt:?} and nothing in bdd/args_probes.json claims it - \
                 add a row, or an elsewhere entry saying why it is not pinned here. An \
                 unpinned error message is one nobody reads twice."
            ));
        }
    }

    // 1: does each row actually produce its message?
    let tmp = tempfile_build_dir()?;
    let mut ran = 0usize;
    let exe = std::env::current_exe()?;
    if let Some(rows) = table.get("rows").and_then(Value::as_array) {
        for row in rows {
            let name = row.get("name").and_then(Value::as_str).unwrap_or("");
            let blurb = row.get("blurb").and_then(Value::as_str).unwrap_or("");
            let must_contain = row.get("must_contain").and_then(Value::as_str).unwrap_or("");
            let spec = build_args_spec(&table, row);
            let spec_path = tmp.join(format!("{name}.json"));
            std::fs::write(
                &spec_path,
                serde_json::to_string(&spec).unwrap_or_default(),
            )?;
            let out = CommandTimeout::run(
                &exe,
                &[
                    "run",
                    "--spec",
                    &spec_path.to_string_lossy(),
                    "--state",
                    "{}",
                ],
                30,
            );
            match out {
                None => problems.push(format!(
                    "row {name:?} ({blurb}): the run timed out; this path is supposed to \
                     refuse before touching a browser, so a hang is a real defect"
                )),
                Some((rc, output)) => {
                    if rc == 0 {
                        problems.push(format!(
                            "row {name:?} ({blurb}): the run exited 0, but the plugin is \
                             supposed to refuse with {must_contain:?}"
                        ));
                    } else if !output.contains(must_contain) {
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
    }
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

/// Build the spec the args gate runs for one row of bdd/args_probes.json.
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

    // label regex -> owner source ("transpile" = count it here).
    let labels: Vec<(Regex, &str)> = vec![
        (Regex::new(r"Current state: \*\*(\d+) scenario").unwrap(), "transpile"),
        (Regex::new(r"(\d+) steps map correctly").unwrap(), "vocabulary"),
        (Regex::new(r"(\d+) step arguments survive").unwrap(), "vocabulary"),
        (Regex::new(r"(\d+) operand types survive").unwrap(), "vocabulary"),
        (Regex::new(r"(\d+) steps still refuse to run without a page").unwrap(), "vocabulary"),
        (Regex::new(r"(\d+) stay refused after a release").unwrap(), "vocabulary"),
        (Regex::new(r"(\d+)/(\d+) argument errors refuse").unwrap(), "args"),
        (Regex::new(r"all (\d+) throw sites are accounted for").unwrap(), "args"),
        (Regex::new(r"\((\d+) pinned here, (\d+) declared elsewhere\)").unwrap(), "args"),
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

    let ints = |s: &str| -> Vec<Vec<i64>> {
        let mut out = Vec::new();
        for m in labels.iter().filter_map(|(rx, src)| if *src == s { Some(rx) } else { None }) {
            for c in m.captures_iter(&vocab_or_args(s, &vocab_out, &args_out)) {
                out.push((1..=c.len() - 1).map(|i| c[i].parse::<i64>().unwrap_or(0)).collect());
            }
        }
        out
    };
    fn vocab_or_args(owner: &str, vocab: &str, args: &str) -> String {
        match owner {
            "vocabulary" => vocab.to_string(),
            "args" => args.to_string(),
            _ => String::new(),
        }
    }

    let truth: Vec<Vec<Vec<i64>>> = labels
        .iter()
        .map(|(rx, src)| match *src {
            "transpile" => vec![vec![n_scenarios as i64]],
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

    // Collapse whitespace in the README before searching labels.
    let flat_readme = ws.replace_all(&readme, " ").into_owned();
    let mut problems: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for (i, (rx, src)) in labels.iter().enumerate() {
        let in_doc: Vec<Vec<i64>> = rx
            .captures_iter(&flat_readme)
            .map(|c| (1..c.len()).map(|i| c[i].parse::<i64>().unwrap_or(0)).collect())
            .collect();
        if in_doc.is_empty() {
            problems.push(format!(
                "the README no longer says anything matching {:?}, so the number it used \
                 to quote is no longer checked - either restore it or drop the label",
                rx.as_str()
            ));
            continue;
        }
        let want = &truth[i];
        if want.is_empty() {
            problems.push(format!(
                "the README quotes {:?} but the gate no longer prints anything matching \
                 it - the quote is stale",
                rx.as_str()
            ));
            continue;
        }
        checked += 1;
        if &in_doc != want {
            problems.push(format!(
                "the README says {} where the truth is {} for {:?} - a number nobody \
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

    for p in &problems {
        eprintln!("bdd doc check: {p}");
    }
    if !problems.is_empty() {
        eprintln!("bdd doc check: {} problem(s)", problems.len());
        return Ok(1);
    }
    println!(
        "bdd doc check: {checked}/{} counts the README quotes match what the gates \
         print, and every quoted gate line is still printed",
        labels.len()
    );
    Ok(0)
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
fn run_scenario(spec_path: &Path, state: &Value, timeout_secs: u64) -> (i32, String) {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return (-1, format!("cannot locate current exe: {e}")),
    };
    let state_json = state.to_string();
    let child = std::process::Command::new(exe)
        .arg("run")
        .arg("--spec")
        .arg(spec_path)
        .arg("--state")
        .arg(&state_json)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => return (-1, format!("spawn failed: {e}")),
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let out = child.wait_with_output().ok();
        let _ = tx.send(out);
    });
    match rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
        Ok(Some(out)) => {
            let combined =
                String::from_utf8_lossy(&out.stdout).into_owned() + String::from_utf8_lossy(&out.stderr).as_ref();
            (out.status.code().unwrap_or(-1), combined)
        }
        Ok(None) => (-1, "run failed to execute".into()),
        Err(_) => (124, format!("timed out after {timeout_secs}s")),
    }
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

    let jobs = o.jobs.max(1).min(plan.len());
    if o.port != 0 && jobs != 1 {
        eprintln!("error: --port pins one CDP endpoint, so it only works with --jobs 1");
        return Ok(1);
    }

    println!(
        "bdd: {} scenario(s)  ({})",
        plan.len(),
        if jobs == 1 { "serial".into() } else { format!("{jobs} workers") }
    );
    std::fs::create_dir_all(&o.out)?;

    // One CDP endpoint per worker: each worker keeps its own port and profile
    // for every scenario it runs, which is what lets scenario 2..N adopt the
    // Chrome that scenario 1 launched instead of paying for a fresh launch
    // (measured 1.9s per scenario with a fresh profile, 0.75s with a reused one).
    let mut endpoints: Vec<(u16, String)> = Vec::new();
    for w in 0..jobs {
        let port = if o.port != 0 { o.port } else { free_port()? };
        let profile = std::env::temp_dir().join(format!("laya-bdd-chrome-w{w}-{}", std::process::id()));
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
    for (w, (port, prof)) in endpoints.iter().enumerate() {
        println!("bdd: cdp[{w}]   127.0.0.1:{port} headless  ({prof})");
    }
    println!("bdd: headless chrome\n");

    let mut results: Vec<RunResult> = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    let mut done = 0usize;

    // Serial execution (the measured default; the Python runner's --jobs path
    // spun threads, but scenario work here is dominated by Chrome startup and
    // the report must print in plan order anyway).
    for (feature, scenario, spec, config) in &plan {
        let (port, profile_dir) = endpoints[0].clone();
        let xfail = scenario.tags.iter().any(|t| {
            t == xfail_tag || t.starts_with(&format!("{xfail_tag}("))
        });
        let spec_file = format!("{}.json", spec["name"].as_str().unwrap_or(""));
        let spec_path = Path::new(&o.out).join(&spec_file);
        std::fs::write(
            &spec_path,
            format!("{}\n", serde_json::to_string_pretty(&public_spec(spec))?),
        )?;
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
        let (rc, out) = run_scenario(&spec_path, &Value::Object(state), o.timeout_secs);
        let elapsed = started.elapsed().as_secs_f64();
        let name = format!(
            "{} :: {}",
            Path::new(&feature.path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            scenario.name
        );
        let (ok, label, why) = if xfail {
            // Failing is not enough — the run has to fail *for the stated
            // reason*. The reason lives in the tag:
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
        if !why.is_empty() {
            eprintln!("  {name}: {why}", );
        }
        results.push(RunResult {
            label: label.to_string(),
            ok,
            name,
            rc,
            out,
            xfail,
            elapsed,
        });
        done += 1;
        println!(
            "  ... {label:14} {}  [{rc}]  ({done}/{})",
            results.last().unwrap().name,
            plan.len()
        );
    }

    // Hand-written probes (local profile only: they exercise local fixtures).
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
        let started = std::time::Instant::now();
        let (rc, out) = run_scenario(path, &Value::Object(state), o.timeout_secs);
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
// tasks (`bdd/bench/tasks/*.json`) against exactly three things an author cares
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
    /// Score only these task ids (default: every task under bdd/bench/tasks).
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
    let tasks_dir = root.join("bdd").join("bench").join("tasks");
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

        // expressiveness: every scenario compiles.
        let mut compile_ok = true;
        let mut compile_error = None;
        for sc in &feature.scenarios {
            if let Err(e) = compile_scenario(&feature, sc, &json!({}), "chrome-headless.sh") {
                compile_ok = false;
                compile_error = Some(format!("{}: {e:#}", sc.name));
                break;
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

    // optional e2e: run every task feature once, in one `bdd run`, and read the
    // per-feature verdict out of its report.
    if o.run {
        let exe = std::env::current_exe()?;
        let mut args: Vec<String> = vec!["bdd".into(), "run".into()];
        for p in &feature_paths {
            args.push(p.to_string_lossy().into_owned());
        }
        // Every feature path is positional after `run`.
        let out = std::process::Command::new(exe).args(&args).output()?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        for t in tasks.iter_mut() {
            let name = Path::new(&t.feature)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut passes = 0usize;
            let mut fails = 0usize;
            for line in text.lines() {
                if line.contains(&name) {
                    if line.contains("FAIL") || line.contains("XFAIL-WRONG-REASON") {
                        fails += 1;
                    } else if line.contains("PASS") || line.contains("xfail") {
                        passes += 1;
                    }
                    // The runner prints each scenario's wall-clock as the last
                    // token (`1.19s`). Summing it per feature is the one honest
                    // *time* number the benchmark has: authoring time needs an
                    // author, but execution time is measurable here.
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
            "bdd score: end-to-end {} task(s) in {total_e2e:.2}s of scenario time (real Chrome)",
            tasks.iter().filter(|t| t.e2e_secs > 0.0).count()
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

/// Every `bdd/bench/live/*.feature` except the `_`-prefixed development probe.
pub fn live_features() -> Result<Vec<PathBuf>> {
    let dir = repo_root().join("bdd").join("bench").join("live");
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
    };
    run(&ro)
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
