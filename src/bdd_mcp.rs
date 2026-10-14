//! BDD authoring MCP tools — let an agent author and validate `.feature`
//! workflows inside the loop, and audit the known-workflow catalog, without
//! shelling out to the CLI.
//!
//! `bdd_validate` is the authoring gate: feed it the `.feature` text (or a
//! repo-relative path) and it compiles + validates it, returning per-scenario
//! status, step coverage and line-numbered errors — the exact loop an agent
//! runs before committing a `.feature`.
//!
//! `bdd_catalog` is the completeness audit for "convert all known workflows to
//! BDD": every `dsl/<cat>/<name>.json` spec must have a `bdd/workflows/<cat>/<name>`
//! `@decision` source, and vice versa. Orphans on either side are reported so
//! a workflow that has not been converted (or a feature with no spec) cannot
//! hide.
//!
//! `bdd_import` and `bdd_decide` close the loop from the other two sides.
//! `bdd_import` lifts an existing JSON workflow spec back into a `@decision`
//! `.feature` that recompiles to the very same graph — so an agent can convert
//! *any* known workflow into BDD in-process, not just author a new one.
//! `bdd_decide` runs a finished `@decision` graph against a state and returns
//! the verdict + trace, so the graph is a decision an agent can *call*, not only
//! a product it writes. `bdd_list` is the discovery half: the callable inventory
//! of the corpus (each workflow's questions and the state fields to feed it), so
//! an agent can find the right graph before it calls it.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::bdd;
use crate::mcp::{DynTool, McpTool, McpToolSet};

pub struct BddTools;

impl McpToolSet for BddTools {
    fn group_name(&self) -> &str {
        "bdd"
    }

    fn tools(&self) -> Vec<DynTool> {
        vec![
            Arc::new(NewTool),
            Arc::new(TranspileTool),
            Arc::new(ValidateTool),
            Arc::new(CatalogTool),
            Arc::new(ImportTool),
            Arc::new(DecideTool),
            Arc::new(ListTool),
            Arc::new(VocabularyTool),
        ]
    }
}

/// A `@decision` feature compiles to one whole workflow; a plain feature to
/// one spec per Scenario (browser-step path). Both paths must satisfy the same
/// `validate` gate the engine runs.
fn compile_and_validate(text: &str, path: &str) -> Result<Value> {
    let feature = bdd::parse(text, path)?;
    // Guarantee scratch ${env.LAYA_*} dirs the way `bdd build` does, so
    // policy.allow_paths references resolve in validation.
    if let Ok(env_map) = crate::dsl_smoke::ensure_env_dirs() {
        for (k, v) in env_map {
            std::env::set_var(k, v);
        }
        let _ = crate::capability::secret::init(None);
    }
    let decision = feature
        .tags
        .iter()
        .any(|t| t == "decision" || t.starts_with("decision("));
    let cov = bdd::feature_coverage(&feature);
    let mut scenarios: Vec<Value> = Vec::new();
    let mut validate_ok = true;
    let mut validate_error: Option<String> = None;
    if decision {
        match bdd::compile_decision(&feature, &json!({})) {
            Ok(spec) => {
                scenarios.push(json!({"name": feature.name, "ok": true}));
                if let Err(e) = crate::spec::from_spec_with_dir(&spec, None, None) {
                    validate_ok = false;
                    let msg = format!("{e:#}");
                    // P35: pin the .feature line the semantic error refers to.
                    validate_error = Some(match bdd::locate_semantic_error_line(&feature, &msg) {
                        Some(l) => format!("line {l}: {msg}"),
                        None => msg,
                    });
                }
            }
            Err(e) => {
                scenarios.push(json!({"name": feature.name, "ok": false, "error": format!("{e:#}")}));
                validate_ok = false;
                validate_error = Some(format!("compile: {e:#}"));
            }
        }
    } else {
        for sc in &feature.scenarios {
            let name = sc.name.clone();
            match bdd::compile_scenario(&feature, sc, &json!({}), "chrome-headless.sh") {
                Ok(spec) => {
                    let ok = crate::spec::from_spec_with_dir(&spec, None, None).is_ok();
                    if !ok && validate_ok {
                        validate_ok = false;
                        validate_error = Some(format!("scenario {name:?} fails validate"));
                    }
                    scenarios.push(json!({"name": name, "ok": true}));
                }
                Err(e) => {
                    scenarios.push(json!({"name": name, "ok": false, "error": format!("{e:#}")}));
                    validate_ok = false;
                }
            }
        }
    }
    let scenarios_ok = scenarios.iter().all(|s| s["ok"].as_bool().unwrap_or(false));
    Ok(json!({
        "ok": scenarios_ok && validate_ok,
        "decision": decision,
        "feature": feature.name,
        "scenarios": scenarios,
        "coverage": {
            "total": cov.total,
            "matched": cov.matched,
            "pct": cov.coverage,
            "unmatched": cov.unmatched,
        },
        "validate": {
            "ok": validate_ok,
            "error": validate_error,
        },
    }))
}

struct ValidateTool;

impl McpTool for ValidateTool {
    fn name(&self) -> &str {
        "bdd_validate"
    }
    fn description(&self) -> &str {
        "Compile and validate a BDD workflow .feature (pass feature_text or a repo-relative feature_path). Returns per-scenario compile status, step coverage, and the engine validate verdict with line-numbered errors — the same gate `bdd build` runs, callable by an agent mid-authoring."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "feature_text": {"type": "string", "description": "The .feature source text to compile. Provide this or feature_path."},
                "feature_path": {"type": "string", "description": "Repo-relative path to an existing .feature, e.g. bdd/workflows/routing/support_ticket_router.feature. Provide this or feature_text."}
            }
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let text = match args.get("feature_text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => {
                let rel = args
                    .get("feature_path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow!("bdd_validate: provide feature_text or feature_path")
                    })?;
                let p = bdd::repo_root().join(rel);
                std::fs::read_to_string(&p)
                    .map_err(|e| anyhow!("bdd_validate: cannot read {rel}: {e}"))?
            }
        };
        compile_and_validate(&text, "<mcp>")
    }
}

struct CatalogTool;

impl McpTool for CatalogTool {
    fn name(&self) -> &str {
        "bdd_catalog"
    }
    fn description(&self) -> &str {
        "Audit the known-workflow catalog: every dsl/<cat>/<name>.json decision spec must have a bdd/workflows/<cat>/<name> @decision source and vice versa. Returns per-category counts plus any orphan specs (converted to JSON but never BDD'd) and any features without a spec — the completeness check behind 'convert all known workflows to BDD'."
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }
    fn call(&self, _args: &Value) -> Result<Value> {
        let root = bdd::repo_root();
        let wf_root = root.join("bdd").join("workflows");
        let mut features: Vec<(String, String)> = Vec::new(); // (cat, name)
        let mut cats: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        if wf_root.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&wf_root) {
                for e in rd.flatten() {
                    let cat = e.file_name().to_string_lossy().into_owned();
                    let cd = e.path();
                    if !cd.is_dir() {
                        continue;
                    }
                    if let Ok(sd) = std::fs::read_dir(&cd) {
                        for f in sd.flatten() {
                            let fp = f.path();
                            if fp.extension().map(|x| x == "feature").unwrap_or(false) {
                                if let Ok(txt) = std::fs::read_to_string(&fp) {
                                    let is_dec = txt.lines().any(|l| l.starts_with("@decision"));
                                    if is_dec {
                                        let name = fp
                                            .file_stem()
                                            .map(|s| s.to_string_lossy().into_owned())
                                            .unwrap_or_default();
                                        features.push((cat.clone(), name));
                                        *cats.entry(cat.clone()).or_insert(0) += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        features.sort();
        let feat_set: std::collections::BTreeSet<(String, String)> = features.iter().cloned().collect();

        let mut specs: Vec<(String, String)> = Vec::new();
        let mut spec_cats: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        let dsl_root = root.join("dsl");
        if let Ok(rd) = std::fs::read_dir(&dsl_root) {
            for e in rd.flatten() {
                let cat = e.file_name().to_string_lossy().into_owned();
                let cd = e.path();
                if !cd.is_dir() {
                    continue;
                }
                if let Ok(sd) = std::fs::read_dir(&cd) {
                    for f in sd.flatten() {
                        let fp = f.path();
                        if fp.extension().map(|x| x == "json").unwrap_or(false) {
                            // `<name>.config.json` sidecars are config, not specs:
                            // exclude so the catalog counts/audits real workflows.
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
                            *spec_cats.entry(cat.clone()).or_insert(0) += 1;
                        }
                    }
                }
            }
        }
        specs.sort();
        let spec_set: std::collections::BTreeSet<(String, String)> = specs.iter().cloned().collect();

        let orphans: Vec<Value> = (specs.iter().filter(|s| !feat_set.contains(*s)))
            .map(|(c, n)| json!({"cat": c, "name": n}))
            .collect();
        let no_spec: Vec<Value> = (features.iter().filter(|s| !spec_set.contains(*s)))
            .map(|(c, n)| json!({"cat": c, "name": n}))
            .collect();
        let cats_out: Value = cats
            .iter()
            .map(|(c, n)| (c.clone(), json!({"features": n, "specs": spec_cats.get(c).copied().unwrap_or(0)})))
            .collect();

        Ok(json!({
            "total_dsl_specs": specs.len(),
            "total_decision_features": features.len(),
            "complete": orphans.is_empty() && no_spec.is_empty(),
            "categories": cats_out,
            "orphan_specs": orphans,
            "features_without_spec": no_spec,
        }))
    }
}


/// `bdd_new`: open a greenfield decision workflow from an intent. Generates a
/// minimal valid `@decision` `.feature` skeleton (self-verified to pass
/// `bdd build`), returns the text for the agent to extend, and optionally
/// writes it to `bdd/workflows/<cat>/<name>.feature` (or a `root` override).
/// Together with `bdd_validate` and `bdd_catalog` this completes the authoring
/// loop entirely in-process: new → validate → extend → audit.
struct NewTool;

impl McpTool for NewTool {
    fn name(&self) -> &str {
        "bdd_new"
    }
    fn description(&self) -> &str {
        "Open a greenfield decision workflow: from an intent, generate a minimal valid @decision .feature skeleton that already passes bdd build — the cheapest valid start to the authoring loop. Returns the skeleton text (optionally writes it to bdd/workflows/<cat>/<name>.feature when write=true), the compile/validate verdict and next-step commands. Read the step grammar with bdd_vocabulary before editing steps, extend + validate with bdd_validate, and audit with bdd_catalog."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Workflow name (also the file stem and Feature name)."},
                "cat": {"type": "string", "description": "Category subdir under bdd/workflows/ (default: routing)."},
                "intent": {"type": "string", "description": "Natural-language intent; becomes the Feature description."},
                "primary_q": {"type": "string", "description": "Primary question name for the starter node (default: option)."},
                "write": {"type": "boolean", "description": "Write the skeleton to the workspace root bdd/workflows/<cat>/<name>.feature (default false = return text only)."},
                "root": {"type": "string", "description": "Workspace root to write under (default: the laya-workflow repo root)."},
                "single": {"type": "boolean", "description": "Emit a 1-node start instead of the 2-node canonical graph (default false)."},
                "nested": {"type": "boolean", "description": "Emit a nested-pipeline starter (prefilter -> sub-workflow -> audit with completion handoff) instead of the canonical 2-node graph (default false)."},
                "pattern": {"type": "string", "description": "Starter shape: two_node | single | nested | gate | loop (overrides single/nested)."}
            },
            "required": ["name"]
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("bdd_new: name is required"))?;
        if name.trim().is_empty() {
            return Err(anyhow!("bdd_new: name must not be empty"));
        }
        let cat = args.get("cat").and_then(|v| v.as_str()).unwrap_or("routing");
        let cat = if cat.trim().is_empty() { "routing" } else { cat.trim() };
        let primary_q = args.get("primary_q").and_then(|v| v.as_str()).unwrap_or("option");
        let primary_q = if primary_q.trim().is_empty() { "option" } else { primary_q.trim() };
        let intent = args.get("intent").and_then(|v| v.as_str()).map(|s| s.to_string());
        let write = args.get("write").and_then(|v| v.as_bool()).unwrap_or(false);
        let single = args.get("single").and_then(|v| v.as_bool()).unwrap_or(false);
        let nested = args.get("nested").and_then(|v| v.as_bool()).unwrap_or(false);
        let pattern = args.get("pattern").and_then(|v| v.as_str()).map(str::trim).filter(|p| !p.is_empty());
        let root = args
            .get("root")
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(bdd::repo_root);

        let desc = intent
            .clone()
            .unwrap_or_else(|| format!("The {name} decision workflow (scaffolded)."));
        let pattern = match pattern {
            Some(p) => p.to_string(),
            None if nested => "nested".to_string(),
            None if single => "single".to_string(),
            None => "two_node".to_string(),
        };
        let body = bdd::scaffold_body_pattern(name, cat, &primary_q, &desc, &pattern)?;
        // Self-verify exactly as `bdd build` would.
        let feature = bdd::parse(&body, "<bdd_new>")?;
        let spec = bdd::compile_decision(&feature, &json!({}))?;
        crate::spec::from_spec_with_dir(&spec, None, None)?;
        let nodes = spec["nodes"].as_array().map(|a| a.len()).unwrap_or(0);

        let mut written: Option<String> = None;
        let mut exists = false;
        if write {
            let dir = root.join("bdd").join("workflows").join(cat);
            std::fs::create_dir_all(&dir)?;
            let dest = dir.join(format!("{name}.feature"));
            if dest.exists() {
                exists = true;
            } else {
                std::fs::write(&dest, &body)?;
                written = Some(dest.to_string_lossy().into_owned());
            }
        }

        Ok(json!({
            "name": name,
            "cat": cat,
            "pattern": pattern,
            "valid": true,
            "nodes": nodes,
            "exists": exists,
            "written": written,
            "feature_text": body,
            "next": [
                "read the step grammar with bdd_vocabulary (or `bdd vocabulary`) before editing steps",
                "extend the graph, then validate with bdd_validate (or `bdd build`)",
                "emit the dsl spec with `bdd transpile <file> --out dsl/<cat>`",
                "prove .feature == dsl with `bdd round-trip`; audit with bdd_catalog"
            ]
        }))
    }
}


/// `bdd_transpile`: compile a `.feature` into its dsl spec(s) — the "emit the
/// dsl spec" step of the authoring loop, in-process. For an `@decision` feature
/// the result is the single decision-graph spec (dsl_version stripped, matching
/// the checked-in dsl convention); for a browser-step feature it is one spec per
/// Scenario. Optional write=true persists to the workspace root's dsl/<cat>/ tree
/// with an exists-guard. Together with `bdd_new`/`bdd_validate`/`bdd_catalog`
/// the whole loop (intent → skeleton → validate → emit spec → audit) runs over
/// MCP without shelling out.
struct TranspileTool;

impl McpTool for TranspileTool {
    fn name(&self) -> &str {
        "bdd_transpile"
    }
    fn description(&self) -> &str {
        "Compile a .feature into its dsl spec(s) — the 'emit the dsl spec' step of the authoring loop, in-process. For an @decision feature returns the single decision-graph spec (dsl_version stripped); for a browser-step feature one spec per Scenario. Pass feature_text or a repo-relative feature_path, plus cat and optional write=true (persists to the workspace root's dsl/<cat>/ tree with an exists-guard). Returns the spec JSON plus the validate verdict and written path."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "feature_text": {"type": "string", "description": "The .feature source text to compile. Provide this or feature_path."},
                "feature_path": {"type": "string", "description": "Repo-relative path to an existing .feature, e.g. bdd/workflows/routing/support_ticket_router.feature. Provide this or feature_text."},
                "cat": {"type": "string", "description": "Category subdir used for the written spec path and name (default: routing)."},
                "write": {"type": "boolean", "description": "Write the emitted spec to the workspace root dsl/<cat>/<name>.json (default false = return JSON only)."},
                "root": {"type": "string", "description": "Workspace root to write under (default: the laya-workflow repo root)."}
            }
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let (text, src_name) = match args.get("feature_text").and_then(|v| v.as_str()) {
            Some(t) => (t.to_string(), "<mcp>".to_string()),
            None => {
                let rel = args
                    .get("feature_path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("bdd_transpile: provide feature_text or feature_path"))?;
                let p = bdd::repo_root().join(rel);
                let name = std::path::Path::new(rel)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "spec".to_string());
                (
                    std::fs::read_to_string(&p)
                        .map_err(|e| anyhow!("bdd_transpile: cannot read {rel}: {e}"))?,
                    name,
                )
            }
        };
        let cat = args.get("cat").and_then(|v| v.as_str()).unwrap_or("routing");
        let cat = if cat.trim().is_empty() { "routing" } else { cat.trim() };
        let write = args.get("write").and_then(|v| v.as_bool()).unwrap_or(false);
        let root = args
            .get("root")
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(bdd::repo_root);

        let feature = bdd::parse(&text, &src_name)?;
        let decision = feature
            .tags
            .iter()
            .any(|t| t == "decision" || t.starts_with("decision("));

        let mut specs: Vec<Value> = Vec::new();
        if decision {
            let spec = bdd::compile_decision(&feature, &json!({}))?;
            let mut payload = spec.clone();
            if let Some(o) = payload.as_object_mut() {
                o.remove("dsl_version");
            }
            crate::spec::from_spec_with_dir(&payload, None, None)?;
            specs.push(payload);
        } else {
            for sc in &feature.scenarios {
                let spec = bdd::compile_scenario(&feature, sc, &json!({}), "chrome-headless.sh")?;
                let payload = bdd::public_spec(&spec);
                crate::spec::from_spec_with_dir(&payload, None, None)?;
                specs.push(payload);
            }
        }

        let name = if decision {
            specs[0]["name"].as_str().unwrap_or(&src_name).to_string()
        } else {
            feature.name.clone()
        };
        let mut written: Option<String> = None;
        let mut exists = false;
        if write {
            let dir = root.join("dsl").join(cat);
            std::fs::create_dir_all(&dir)?;
            let dest = dir.join(format!("{name}.json"));
            if dest.exists() {
                exists = true;
            } else {
                let body = if specs.len() == 1 {
                    format!("{}
", serde_json::to_string_pretty(&specs[0])?)
                } else {
                    format!("{}
", serde_json::to_string_pretty(&specs)?)
                };
                std::fs::write(&dest, &body)?;
                written = Some(dest.to_string_lossy().into_owned());
            }
        }

        Ok(json!({
            "name": name,
            "cat": cat,
            "decision": decision,
            "valid": true,
            "spec_count": specs.len(),
            "spec": if specs.len() == 1 { specs[0].clone() } else { Value::Null },
            "specs": if specs.len() > 1 { Value::Array(specs) } else { Value::Null },
            "exists": exists,
            "written": written,
            "next": [
                "prove .feature == dsl with `bdd round-trip`; audit with bdd_catalog"
            ]
        }))
    }
}


/// `bdd_import`: lift an existing workflow spec (a `.json`) into a `@decision`
/// `.feature` that compiles back to the very same graph — the reverse
/// transpiler (the inverse of `bdd_transpile`), in-process. The agent-facing
/// counterpart of `laya-workflow bdd import` / `bdd import --dir`: the tool
/// that lets an agent lift *any* known JSON workflow into BDD. Pass
/// `spec_path` (repo-relative or absolute) or `spec_json` (raw text).
struct ImportTool;

impl McpTool for ImportTool {
    fn name(&self) -> &str {
        "bdd_import"
    }
    fn description(&self) -> &str {
        "Import (lift) an existing workflow spec (a .json) into a @decision .feature that compiles back to the very same graph — the reverse transpiler, in-process; the agent-facing counterpart of `laya-workflow bdd import`. Pass spec_path (repo-relative or absolute) or spec_json (raw text). Returns the .feature text plus a round-trip proof (graph_matches), so an agent can lift any known JSON workflow into BDD."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "spec_path": {"type": "string", "description": "Path to an existing workflow spec .json, repo-relative or absolute. Provide this or spec_json."},
                "spec_json": {"type": "string", "description": "Raw workflow spec JSON text to import. Provide this or spec_path."}
            }
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let (spec, source) = match args.get("spec_json").and_then(Value::as_str) {
            Some(t) => (
                serde_json::from_str::<Value>(t)
                    .map_err(|e| anyhow!("bdd_import: spec_json is not valid JSON: {e}"))?,
                "<mcp>".to_string(),
            ),
            None => {
                let p = args
                    .get("spec_path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("bdd_import: provide spec_path or spec_json"))?;
                let path = if std::path::Path::new(p).is_absolute() {
                    std::path::PathBuf::from(p)
                } else {
                    bdd::repo_root().join(p)
                };
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| anyhow!("bdd_import: cannot read {p}: {e}"))?;
                (
                    serde_json::from_str::<Value>(&text)
                        .map_err(|e| anyhow!("bdd_import: {p} is not valid JSON: {e}"))?,
                    p.to_string(),
                )
            }
        };
        let name = spec
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("<spec>")
            .to_string();
        let feature_text = bdd::import_spec(&spec)?;
        // Prove the emitted feature recompiles to the same graph — the same
        // check `bdd import --dir` runs before it writes anything.
        let graph_matches = match bdd::parse(&feature_text, "<import>") {
            Ok(pf) => match bdd::compile_decision(&pf, &json!({})) {
                Ok(rb) => bdd::same_graph(&spec, &rb),
                Err(_) => false,
            },
            Err(_) => false,
        };
        Ok(json!({
            "ok": graph_matches,
            "name": name,
            "source": source,
            "feature_text": feature_text,
            "graph_matches": graph_matches,
            "next": [
                "file the .feature under bdd/workflows/<cat>/ so `bdd round-trip` and `bdd import-check` both pass"
            ]
        }))
    }
}

/// `bdd_decide`: run a `@decision` workflow against a state and return the
/// verdict — the "use the workflow" step that makes a BDD graph a callable
/// decision core for an agent, not just something it authors. Compiles the
/// feature on the fly (or loads a checked-in spec) and executes it with the
/// offline heuristic backend, the same deterministic engine `dsl smoke` and
/// `bdd score --run` drive. Pass `feature_text`/`feature_path` or `spec_path`,
/// plus `state`.
struct DecideTool;

/// The ledger key (`<cat>/<stem>`, no `.json`) for a resolved dsl spec path, so
/// `bdd_decide` can look the spec up in the same `live_only` map `bdd_list` and
/// `dsl smoke` use. `None` when the path is not under `dsl/` (e.g. a feature
/// compiled in-memory, which is source, not a live-only artifact).
fn spec_rel_key(path: &std::path::Path) -> Option<String> {
    let dsl = bdd::repo_root().join("dsl");
    let rel = path.strip_prefix(&dsl).ok()?;
    let s = rel.to_string_lossy().replace('\\', "/");
    s.strip_suffix(".json").map(str::to_string)
}

/// The per-node question contract of a spec's node — the *same* shape `bdd_list`
/// exposes at `nodes[].questions`: a list of `{id, type, answers, from_state}`.
/// Empty list when the node is unknown. A step-wise caller gets the next node's
/// *contract* (not the raw deduped `questions` map, which also carries prose and
/// merges shared ids across nodes), so one `bdd_decide {answers: []}` reply is
/// enough to answer the next question — no separate `bdd_list` call to learn the
/// accepted answers.
fn spec_node_contract(spec: &Value, node: &str) -> Value {
    spec.get("nodes")
        .and_then(Value::as_array)
        .and_then(|ns| {
            ns.iter()
                .find(|n| n.get("name").and_then(Value::as_str) == Some(node))
        })
        .map(node_question_contract)
        .unwrap_or_else(|| Value::Array(Vec::new()))
}

impl McpTool for DecideTool {
    fn name(&self) -> &str {
        "bdd_decide"
    }
    fn description(&self) -> &str {
        "Run a @decision workflow against a state and return the decision — the callable core of a BDD graph. Compiles the feature (or loads a checked-in spec_path) and executes it with the offline heuristic backend, the same deterministic engine `dsl smoke` and `bdd score --run` drive. Pass feature_text or feature_path, or spec_path, plus state (a JSON object). Returns the final label, the full trace (per-step node/action/answer), the result object, and the decision contract: `inputs` (the declared offline state fields the graph reads — heuristic source fields plus `${state.*}` interpolations plus kind-implicit reads) and `missing_inputs` (declared but absent from `state` — when non-empty the label may come from defaults; only when `inputs` is empty does the graph read no state at all and every call return the same default walk, so drive that with a real backend). `label_source` says how to read the returned label: `decision` (a declared output — the enumerable decision), `terminal` (the run's terminal verb, e.g. stop/escalate — no decision on that path), or `capability` (an undeclared label a `call` node's payload supplied; `outputs_complete` is then false, so `bdd_list`'s `outputs` is not exhaustive for this graph). `offline` is true when `dsl smoke` behaviour-verified this graph offline (an offline call is then trustworthy); when false the graph was only verified live (`verified_by` names the live gate, `why_live` why it cannot go offline) and the offline label is a defaults-only walk. Pass `answers` — an array of per-node answer objects, one per node the walk visits — to drive the graph with the caller's own answers instead of the offline heuristic: the graph stays the deterministic decision core and the caller supplies the perceptions, and the response then reports `backend: answers`. With `answers: []` the call instead returns {ok:false, needs_answer:true, at_node, ask, steps_so_far}: `at_node` is where the walk is waiting and `ask` is that node's own question contract — the same per-node `{id, type, answers, from_state}` shape `bdd_list` exposes at `nodes[].questions`, each entry listing the answers that question accepts — so a caller can walk the graph one question at a time straight from the reply, appending one answer object per needs_answer reply until it returns ok:true (no separate `bdd_list` call needed)."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["state"],
            "properties": {
                "feature_text": {"type": "string", "description": "The .feature source text to compile and run. Provide this, feature_path, or spec_path."},
                "feature_path": {"type": "string", "description": "Repo-relative path to an existing .feature decision workflow, e.g. bdd/workflows/guards/agent_permission_gate.feature."},
                "spec_path": {"type": "string", "description": "Repo-relative path to a checked-in dsl spec .json to run directly, e.g. dsl/guards/agent_permission_gate.json."},
                "name": {"type": "string", "description": "Port name of a decision workflow to resolve across the repo dsl/ tree (as returned by bdd_list), e.g. agent_permission_gate. Use instead of feature_text/feature_path/spec_path."},
                "state": {"type": "object", "description": "The run state, e.g. {\"text\": \"Payments API returns 500\"} or the ev_* answer fields a heuristic-driven workflow reads."},
                "answers": {"type": "array", "description": "Drive the graph with your own answers instead of the offline heuristic. One object per node the walk visits, each {<qid>: {type: choice|score|noul, choice|score|noul, probabilities, confidence}} in the same shape laya-tch returns. Pass [] to start a step-wise walk: the reply then names the start node (`at_node`) and its `ask` contract (ok:false, needs_answer:true, at_node, ask, steps_so_far) — `ask` is the node's own questions, each with the answers it accepts (`{id,type,answers,from_state}`, the same shape `bdd_list` reports) — and you append one answer per reply until ok:true."}
            }
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let state = args
            .get("state")
            .cloned()
            .ok_or_else(|| anyhow!("bdd_decide: state is required"))?;
        if !state.is_object() {
            return Err(anyhow!("bdd_decide: state must be a JSON object"));
        }
        // Seed the scratch ${env.LAYA_*} dirs the way `bdd build` / `dsl smoke`
        // do, so policy.allow_paths references resolve in the offline run.
        if let Ok(env_map) = crate::dsl_smoke::ensure_env_dirs() {
            for (k, v) in env_map {
                std::env::set_var(k, v);
            }
            let _ = crate::capability::secret::init(None);
        }

        let mut rel_key: Option<String> = None;
        let (wf, spec_value) = if let Some(rel) = args.get("spec_path").and_then(Value::as_str) {
            let path = if std::path::Path::new(rel).is_absolute() {
                std::path::PathBuf::from(rel)
            } else {
                bdd::repo_root().join(rel)
            };
            rel_key = spec_rel_key(&path);
            let txt = std::fs::read_to_string(&path)
                .map_err(|e| anyhow!("bdd_decide: cannot read spec {rel}: {e:#}"))?;
            let sv = serde_json::from_str::<Value>(&txt)
                .map_err(|e| anyhow!("bdd_decide: spec {rel} is not valid json: {e:#}"))?;
            (
                crate::spec::load_file(&path.to_string_lossy())
                    .map_err(|e| anyhow!("bdd_decide: cannot load spec {rel}: {e:#}"))?,
                sv,
            )
        } else if let Some(name) = args.get("name").and_then(Value::as_str) {
            // Resolve a workflow by its port name across the repo dsl tree, so
            // `bdd_list` -> `bdd_decide {name}` composes without a path.
            let dsl = bdd::repo_root().join("dsl");
            let hits: Vec<std::path::PathBuf> = crate::spec::discover(&dsl.to_string_lossy())
                .map_err(|e| anyhow!("bdd_decide: scanning dsl/: {e:#}"))?
                .into_iter()
                .filter(|(stem, _)| stem == name)
                .map(|(_, path)| path)
                .collect();
            match hits.len() {
                0 => return Err(anyhow!("bdd_decide: no decision spec named {name:?} under dsl/")),
                1 => {
                    rel_key = spec_rel_key(&hits[0]);
                    let txt = std::fs::read_to_string(&hits[0])
                        .map_err(|e| anyhow!("bdd_decide: cannot read {:?}: {e:#}", hits[0]))?;
                    let sv = serde_json::from_str::<Value>(&txt)
                        .map_err(|e| anyhow!("bdd_decide: {:?} is not valid json: {e:#}", hits[0]))?;
                    (
                        crate::spec::load_file(&hits[0].to_string_lossy())
                            .map_err(|e| anyhow!("bdd_decide: cannot load {:?}: {e:#}", hits[0]))?,
                        sv,
                    )
                }
                n => {
                    return Err(anyhow!(
                        "bdd_decide: {name:?} is ambiguous ({n} specs share that name); pass spec_path instead"
                    ))
                }
            }
        } else {
            let (text, src_name) = match args.get("feature_text").and_then(Value::as_str) {
                Some(t) => (t.to_string(), "<mcp>".to_string()),
                None => {
                    let rel = args
                        .get("feature_path")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            anyhow!("bdd_decide: provide feature_text, feature_path, or spec_path")
                        })?;
                    let p = bdd::repo_root().join(rel);
                    (
                        std::fs::read_to_string(&p)
                            .map_err(|e| anyhow!("bdd_decide: cannot read {rel}: {e}"))?,
                        rel.to_string(),
                    )
                }
            };
            let feature = bdd::parse(&text, &src_name)?;
            let decision = feature
                .tags
                .iter()
                .any(|t| t == "decision" || t.starts_with("decision("));
            if !decision {
                return Err(anyhow!(
                    "bdd_decide: only @decision workflows are runnable here; {src_name} is a browser-step feature (use `bdd run` for those)"
                ));
            }
            let mut spec = bdd::compile_decision(&feature, &json!({}))?;
            if let Some(o) = spec.as_object_mut() {
                o.remove("dsl_version");
            }
            let sv = spec.clone();
            (
                crate::spec::from_spec_with_dir(&spec, None, None)
                    .map_err(|e| anyhow!("bdd_decide: {e:#}"))?,
                sv,
            )
        };

        // The state fields the graph reads (its questions' heuristic sources, its
        // `${state.*}` interpolations, kind-implicit reads) and which of them the
        // caller did not supply. A missing
        // field is not an error — only some paths read some fields — but it is
        // the honest signal that a returned label may come from defaults rather
        // than from the caller's state: an empty `state` on a graph with declared
        // inputs yields the all-defaults walk, not a data-driven call.
        let declared = spec_inputs(&spec_value);
        let provided: std::collections::BTreeSet<String> = state
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        let inputs: Vec<String> = declared.iter().cloned().collect();
        let missing_inputs: Vec<String> = declared.difference(&provided).cloned().collect();

        // Same source of truth as `bdd_list` / `dsl smoke`: a spec the corpus
        // could only verify live (chrome/network/manual) is NOT offline-trustworthy,
        // so an offline `bdd_decide` on it is a defaults-only walk.
        let live_only = crate::dsl_smoke::coverage_ledger_live_only().unwrap_or_default();
        let live = rel_key.as_deref().and_then(|k| live_only.get(k));
        let offline = live.is_none();
        let verified_by = live.and_then(|e| e.get("covered_by")).cloned().unwrap_or(Value::Null);
        let why_live = live.and_then(|e| e.get("reason")).cloned().unwrap_or(Value::Null);

        // Backend: the caller may supply their own answers (an agent / a real
        // model drives the graph) instead of the offline heuristic. The graph
        // stays the deterministic decision core; only the perceptions come from
        // outside. With too few answers, the call reports the next question
        // instead of failing, so a caller can walk one step at a time.
        let answers = args.get("answers");
        let (out, backend_mode) = match answers {
            Some(ans) => {
                if !ans.is_array() {
                    return Err(anyhow!(
                        "bdd_decide: `answers` must be an array of per-node answer objects (one per node the walk visits; [] starts a step-wise walk)"
                    ));
                }
                let be = crate::backend::AnswersBackend::from_json(ans)
                    .map_err(|e| anyhow!("bdd_decide: {e:#}"))?;
                let mut steps: Vec<Value> = Vec::new();
                let mut at_node = wf.start.clone();
                let res = wf.run_with_progress(&be, &state, &mut |r: &crate::workflow::NodeResult| {
                    steps.push(json!({
                        "node": r.node_name,
                        "action": r.action.as_str(),
                        "answer": r.edge_answer,
                        "confidence": (r.confidence * 10000.0).round() / 10000.0,
                    }));
                    at_node = r.next_node.clone().unwrap_or_default();
                });
                match res {
                    Ok(o) => (o, "answers"),
                    Err(e) if be.exhausted() => {
                        return Ok(json!({
                            "ok": false,
                            "backend": "answers",
                            "needs_answer": true,
                            "at_node": at_node,
                            "ask": spec_node_contract(&spec_value, &at_node),
                            "steps_so_far": steps,
                            "message": format!("{e:#}"),
                        }));
                    }
                    Err(e) => return Err(anyhow!("bdd_decide: {e:#}")),
                }
            }
            None => {
                let o = wf.run(&crate::backend::HeuristicBackend, &state).map_err(|e| {
                    // A live-only graph (chrome/network/manual) cannot be walked
                    // offline; name the live gate in the failure so the caller
                    // routes to it rather than reading a bare runtime error.
                    if offline {
                        anyhow!("bdd_decide: run failed: {e:#}")
                    } else {
                        let by = verified_by.as_str().unwrap_or("live");
                        let why = why_live.as_str().unwrap_or("");
                        anyhow!("bdd_decide: run failed offline: {e:#}\n(this graph is not offline-verifiable; verified by: {by}; {why})")
                    }
                })?;
                (o, "heuristic")
            }
        };
        let j = out.to_json();
        // The decision label is the node's decisive projection, in order:
        // `result.label` (a threshold ladder), `gate_action` (a gate), `route` (a
        // combine: the choice answer or its review_label), `status` (a rubric:
        // scored / human_review). Only then fall back to the terminal verb
        // (`trace.final_action`, typically "stop"), which carries no decision.
        let label = decision_label(j.get("result").unwrap_or(&Value::Null), out.final_action());
        Ok(json!({
            "ok": true,
            "backend": backend_mode,
            "label": label,
            "label_source": label_source(&spec_value, j.get("result").unwrap_or(&Value::Null), out.final_action()),
            "outputs_complete": !has_opaque_outputs(&spec_value),
            "final_action": out.final_action(),
            "trace": j.get("trace").cloned().unwrap_or(Value::Null),
            "result": j.get("result").cloned().unwrap_or(Value::Null),
            "iterations": j.get("iterations").cloned().unwrap_or(Value::Null),
            "inputs": inputs,
            "missing_inputs": missing_inputs,
            "offline": offline,
            "verified_by": verified_by,
            "why_live": why_live,
        }))
    }
}


/// `bdd_list`: the callable inventory of the BDD decision corpus — every
/// `@decision` workflow with the questions it asks (and the state field each
/// heuristic answer reads) and the nodes it walks, so an agent can pick the
/// right workflow and know what to feed `bdd_decide`. This is the discovery
/// half of "use a BDD graph as a decision core": you cannot call what you
/// cannot find.
/// The decision labels a node's action can emit, in first-appearance order — the
/// same projection `bdd_decide` reports as `label`, so this is exactly the set of
/// answers a caller can get back:
///
///   * `threshold` — the ladder's rule labels (ladder order = priority);
///   * `gate` — BLOCK / ALLOW / CONFIRM;
///   * `combine` — the `route` values: the `choice` question's criteria keys minus
///     the `fallback` (a fallback answer routes to `review_label` instead), plus
///     `review_label` itself;
///   * `rubric` — the `status` values scored / human_review (the rubric withholds a
///     composite score as `human_review` when a dimension is uncertain).
///
/// Other action kinds (`copy_keys`/`call`/`none`) project no label, so they
/// contribute nothing. Deduped, order preserved (a ladder's order is priority, so
/// it is more meaningful than sort).
fn action_outputs(action: &Value, questions: Option<&Value>) -> Vec<String> {
    fn push(v: &mut Vec<String>, s: &str) {
        if !v.iter().any(|x| x == s) {
            v.push(s.to_string());
        }
    }
    let mut out: Vec<String> = Vec::new();
    match action.get("kind").and_then(Value::as_str) {
        Some("threshold") => {
            if let Some(rules) = action.get("rules").and_then(Value::as_array) {
                for r in rules {
                    if let Some(l) = r.get("label").and_then(Value::as_str) {
                        push(&mut out, l);
                    }
                }
            }
        }
        Some("gate") => {
            push(&mut out, "BLOCK");
            push(&mut out, "ALLOW");
            push(&mut out, "CONFIRM");
        }
        Some("combine") => {
            let fallback = action
                .get("fallback")
                .and_then(Value::as_str)
                .unwrap_or("other");
            let review = action
                .get("review_label")
                .and_then(Value::as_str)
                .unwrap_or("human_review");
            if let Some(cq) = action.get("choice").and_then(Value::as_str) {
                if let Some(keys) = questions
                    .and_then(|q| q.get(cq))
                    .and_then(|q| q.get("criteria"))
                    .and_then(Value::as_object)
                {
                    for k in keys.keys() {
                        if k != fallback {
                            push(&mut out, k);
                        }
                    }
                }
            }
            push(&mut out, review);
        }
        Some("rubric") => {
            push(&mut out, "scored");
            push(&mut out, "human_review");
        }
        _ => {}
    }
    out
}

/// The declared decision labels a graph can emit: the deduped union of every
/// node's [`action_outputs`], first-appearance order preserved (a threshold
/// ladder keeps its priority order). The single source of truth for
/// `bdd_list`'s `outputs` and the output-coverage gate, so the inventory and the
/// gate can never disagree about what a graph claims to return.
///
/// This is the *declared* set (for a single-node graph, identical to the set a
/// call returns; a multi-node graph returns only the last-emitted label, so some
/// members may be transit-only — see each node's `routes`). A label here that no
/// run can produce is a dead
/// rule or an undriven branch; `bdd_list`'s `outputs` is only trustworthy when
/// the registered offline corpus actually produces every member (the gate that
/// holds it to that lives in `test_bdd_output_coverage`).
pub fn declared_outputs(spec: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(ns) = spec.get("nodes").and_then(Value::as_array) {
        for n in ns {
            let node_outs = action_outputs(n.get("action").unwrap_or(&Value::Null), n.get("questions"));
            for o in node_outs {
                if !out.iter().any(|x| x == &o) {
                    out.push(o);
                }
            }
        }
    }
    out
}

/// The routing a node exposes: its `edge.condition` map (answer → destination)
/// and its `edge.default` (destination when no condition matches, or null). A
/// pure projection of the spec's edge, so `bdd_list` can show the decision
/// *topology* — an agent sees which node a label routes to instead of reading a
/// multi-node graph as a bag of nodes with no structure. Together with `outputs`
/// (the labels a node can emit) this makes the return semantics legible: a node's
/// label is returned to the caller only when emitting it does not route onward
/// (no `routes` entry for the answer, and the walk then stops).
fn node_routes(node: &Value) -> (Value, Value) {
    let edge = node
        .get("edge")
        .or_else(|| node.get("edges"))
        .unwrap_or(&Value::Null);
    let condition = edge
        .get("condition")
        .cloned()
        .filter(|c| c.is_object())
        .unwrap_or_else(|| json!({}));
    let default = edge.get("default").cloned().unwrap_or(Value::Null);
    (condition, default)
}

/// The answer contract a caller needs to drive THIS node over `bdd_decide`'s
/// `answers` argument: the node's own questions, each with the answer shape
/// (`type`) and the answers it accepts. Distinct from the workflow-level
/// `questions`, which is a deduped union across nodes — a shared id can be asked
/// by several nodes with different criteria, and a dynamic question
/// (`criteria.from_state`) materialises its options from the state at decide
/// time, so only the per-node list is the contract for feeding one step.
fn node_question_contract(node: &Value) -> Value {
    let mut out: Vec<Value> = Vec::new();
    if let Some(qs) = node.get("questions").and_then(Value::as_object) {
        for (qid, q) in qs {
            let qtype = q.get("type").and_then(Value::as_str).unwrap_or("choice");
            let from_state = q
                .pointer("/criteria/from_state")
                .and_then(Value::as_str)
                .map(|s| json!(s))
                .unwrap_or(Value::Null);
            let answers: Vec<String> = if from_state.is_null() {
                let mut v: Vec<String> = q
                    .get("criteria")
                    .and_then(Value::as_object)
                    .map(|o| o.keys().cloned().collect())
                    .unwrap_or_default();
                v.sort();
                v
            } else {
                Vec::new()
            };
            out.push(json!({
                "id": qid,
                "type": qtype,
                "answers": answers,
                "from_state": from_state,
            }));
        }
    }
    out.sort_by(|a, b| a["id"].as_str().unwrap_or("").cmp(b["id"].as_str().unwrap_or("")));
    Value::Array(out)
}

/// The decision label a run's `result` projects, in order: `label` (a threshold
/// ladder), `gate_action` (a gate), `route` (a combine: the choice answer or its
/// review label), then `status` (a rubric: `scored` / `human_review`). Only when
/// the graph projects no decision at all does it fall back to `final_action`
/// (the terminal verb, typically `stop`, which carries no decision). Shared by
/// `bdd_decide` and the output-coverage gate so both read a trace the same way.
pub fn decision_label(result: &Value, final_action: &str) -> String {
    result
        .pointer("/label")
        .and_then(Value::as_str)
        .or_else(|| result.pointer("/gate_action").and_then(Value::as_str))
        .or_else(|| result.pointer("/route").and_then(Value::as_str))
        .or_else(|| result.pointer("/status").and_then(Value::as_str))
        .unwrap_or(final_action)
        .to_string()
}

/// Whether a graph's output set is *opaque*: at least one node's action is a
/// `call` (a capability/plugin), whose runtime payload can set any state key —
/// including one `bdd_decide` projects as the decision label (`label` /
/// `gate_action` / `route` / `status`). `bdd_list` therefore cannot enumerate such
/// a graph's complete output set from the spec alone; it marks the graph
/// `outputs_complete: false` so a caller does not read `outputs` as exhaustive.
pub fn has_opaque_outputs(spec: &Value) -> bool {
    spec.get("nodes")
        .and_then(Value::as_array)
        .map(|ns| {
            ns.iter().any(|n| {
                n.get("action").and_then(|a| a.get("kind")).and_then(Value::as_str) == Some("call")
            })
        })
        .unwrap_or(false)
}

/// Where a run's returned `label` came from, so a caller knows how to read it:
///
///   * `"decision"` — the label is one of the graph's declared outputs (a
///     threshold ladder's label, a gate verb, a combine `route`, a rubric
///     `status`) — the intended, enumerable decision;
///   * `"terminal"` — the label is the run's terminal verb (`final_action`, e.g.
///     `stop` / `escalate`), the graph projecting no decision on the path taken;
///   * `"capability"` — the label is neither declared nor the terminal verb, and
///     the graph has an opaque `call` node whose payload supplied it (a label the
///     inventory cannot enumerate from the spec);
///   * `"unknown"` — undeclared, non-terminal, on a graph with no opaque node: a
///     hole the `bdd-label-accounted` gate fails on.
pub fn label_source(spec: &Value, result: &Value, final_action: &str) -> &'static str {
    let label = decision_label(result, final_action);
    if declared_outputs(spec).iter().any(|o| o == &label) {
        "decision"
    } else if label == final_action {
        "terminal"
    } else if has_opaque_outputs(spec) {
        "capability"
    } else {
        "unknown"
    }
}

/// Recursively collect `(cat, path)` for every decision spec under `root` (the
/// `dsl/` tree). `cat` is the spec's directory relative to `root` — `routing`,
/// `apps`, or a nested `routing/sub` — so a nested hierarchy stays discoverable.
/// This walks the tree exactly like `spec::discover`, so `bdd_list` can never
/// hide a spec that `bdd_decide {name}` would still resolve.
pub fn scan_dsl_specs(root: &std::path::Path) -> Vec<(String, std::path::PathBuf)> {
    let mut out: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(d) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
                continue;
            }
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let fname = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if fname.ends_with(".config.json") {
                continue;
            }
            let cat = p
                .parent()
                .and_then(|d| d.strip_prefix(root).ok())
                .map(|d| d.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            out.push((cat, p));
        }
    }
    out.sort();
    out
}

/// Fold a node's declared questions into the set of state fields a
/// deterministic (offline) call reads. A heuristic names its source field in one
/// of three ways, all of which are real state reads:
///
///   * `answer_field` — the field's value *is* the answer;
///   * `field` — the field's text is matched (`match_rules` / `match_any` /
///     `match_regex` / `match_mid_any`), e.g. `{"field":"message"}`;
///   * `extract_numeric` — the field's number is compared.
///
/// A dotted path contributes its root state key.
fn inputs_from_questions(qs: Option<&Value>, into: &mut std::collections::BTreeSet<String>) {
    if let Some(o) = qs.and_then(Value::as_object) {
        for q in o.values() {
            let Some(h) = q.get("heuristic") else { continue };
            for k in ["answer_field", "field", "extract_numeric"] {
                if let Some(f) = h.get(k).and_then(Value::as_str) {
                    let root = f.split('.').next().unwrap_or(f);
                    if !root.is_empty() {
                        into.insert(root.to_string());
                    }
                }
            }
        }
    }
}

/// Every state field the graph reads for a deterministic (offline) call:
///
///   * each question's heuristic source field (`inputs_from_questions`);
///   * any `${state.<key>}` a node action or capability payload interpolates;
///   * the fields an action kind reads directly — `ui_proposal` reads `surface`
///     and `snapshot_hash` out of the state.
///
/// This is exactly what a caller must put in `state` for `bdd_decide`; anything
/// absent is reported back as `missing_inputs`, so the two tools cannot disagree
/// about the contract.
fn spec_inputs(spec: &Value) -> std::collections::BTreeSet<String> {
    let mut out: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Some(ns) = spec.get("nodes").and_then(Value::as_array) {
        for n in ns {
            inputs_from_questions(n.get("questions"), &mut out);
            if n.pointer("/action/kind").and_then(Value::as_str) == Some("ui_proposal") {
                out.insert("surface".to_string());
                out.insert("snapshot_hash".to_string());
            }
        }
    }
    collect_state_templates(spec, &mut out);
    out
}

/// Add the root key of every `${state.<key>[.path]}` template found in `v`.
/// Question definitions are skipped — their heuristic source fields are already
/// collected (and they are declarations, not reads). Byte-wise scan so it is
/// safe on multi-byte text.
fn collect_state_templates(v: &Value, into: &mut std::collections::BTreeSet<String>) {
    match v {
        Value::Object(o) => {
            for (k, sub) in o {
                if k == "questions" {
                    continue;
                }
                collect_state_templates(sub, into);
            }
        }
        Value::Array(a) => {
            for sub in a {
                collect_state_templates(sub, into);
            }
        }
        Value::String(s) => {
            let bytes = s.as_bytes();
            let needle = b"${state.";
            let mut i = 0;
            while i + needle.len() <= bytes.len() {
                if &bytes[i..i + needle.len()] == needle {
                    let mut j = i + needle.len();
                    let start = j;
                    while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                        j += 1;
                    }
                    if j > start {
                        if let Ok(k) = std::str::from_utf8(&bytes[start..j]) {
                            into.insert(k.to_string());
                        }
                        i = j;
                        continue;
                    }
                }
                i += 1;
            }
        }
        _ => {}
    }
}

struct ListTool;

impl McpTool for ListTool {
    fn name(&self) -> &str {
        "bdd_list"
    }
    fn description(&self) -> &str {
        "List the callable BDD decision corpus: every @decision workflow as {cat, name, description, start, node_count, nodes, questions, inputs, outputs}. `questions` gives each question's id and type (with its `answer_field` when a heuristic drives it), and each node's own `questions` gives the questions *that node* asks — the exact contract for one `bdd_decide` `answers` step: {id, type, answers (the values it accepts), from_state (the state list a dynamic question's options are materialised from, else null)} (the node list is authoritative; the workflow-level `questions` is a deduped union and a shared id can be asked by several nodes); `inputs` is the sorted set of state fields the graph reads for a deterministic (offline) call — every question's heuristic source field (`answer_field` / `field` / `extract_numeric`) plus every `${state.<field>}` a node action interpolates and the fields a kind reads directly (`ui_proposal`'s surface / snapshot_hash) — exactly what to put in `state` for `bdd_decide`; `outputs` is the union of the labels any node can emit (each node's `outputs` = its threshold-ladder labels or its gate verbs BLOCK/ALLOW/CONFIRM) — for a single-node graph (no routing) that is exactly the set a call returns; a multi-node graph *returns* only the label of the node where the walk stops (the last emitted one, visible in the trace), so read each node's `routes` (the answer -> destination map) and `default` to see which of a node's labels continue onward versus stop there: emitting a label with a `routes` entry walks on, without one it stops and is what the caller gets back. Each entry also carries `offline` — true when `dsl smoke` behaviour-verified the graph offline (an offline `bdd_decide` on it is trustworthy) and false when it is only verified live (`verified_by` names the live gate, `why_live` why); the top-level `offline_count` / `live_count` summarise the returned set. Scans the dsl/ tree recursively (as `bdd_decide {name}` resolves), so nested categories are both discoverable and callable; filter with cat (a category, or a parent of nested ones) and/or name. Each graph also carries `outputs_complete` — false when a node's action is a `call` (a capability whose runtime payload can set a state key `bdd_decide` projects as the label), so `outputs` there is not a proven-complete set and a returned label may fall outside it. The inventory behind 'use a BDD graph as a decision core'."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cat": {"type": "string", "description": "Only list workflows in this category (e.g. guards, ole_eval, routing)."},
                "name": {"type": "string", "description": "Only list the workflow whose port name matches this exactly."}
            }
        })
    }
    fn call(&self, args: &Value) -> Result<Value> {
        let cat_filter = args.get("cat").and_then(Value::as_str);
        let name_filter = args.get("name").and_then(Value::as_str);
        let dsl = bdd::repo_root().join("dsl");
        // The specs the corpus cannot behaviour-verify offline (chrome/network/
        // manual), from the same ledger `dsl smoke` counts. An agent that only
        // calls over MCP needs to know which graphs it can actually trust.
        let live_only = crate::dsl_smoke::coverage_ledger_live_only().unwrap_or_default();
        let mut workflows: Vec<Value> = Vec::new();
        for (cat, p) in scan_dsl_specs(&dsl) {
            if cat_filter
                .map(|f| cat != f && !cat.starts_with(&format!("{f}/")))
                .unwrap_or(false)
            {
                continue;
            }
            let stem = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name_filter.map(|f| f != stem).unwrap_or(false) {
                continue;
            }
            let Ok(txt) = std::fs::read_to_string(&p) else { continue };
            let Ok(spec) = serde_json::from_str::<Value>(&txt) else { continue };
            if spec.get("nodes").and_then(Value::as_array).is_none() || spec.get("start").is_none() {
                continue;
            }
            // The closed output vocabulary of the graph: every label any node can
            // emit (for a single-node graph this is exactly its output set; a
            // multi-node graph's terminal label is the last node's, visible in the
            // `trace`). Deduped, first-appearance order so a node keeps its
            // ladder's priority order.
            let outputs: Vec<String> = declared_outputs(&spec);
            let nodes: Vec<Value> = spec["nodes"]
                .as_array()
                .map(|ns| {
                    ns.iter()
                        .filter_map(|n| {
                            let nm = n.get("name")?.as_str()?;
                            let node_outs =
                                        action_outputs(n.get("action").unwrap_or(&Value::Null), n.get("questions"));
                            let (routes, default) = node_routes(n);
                            Some(json!({
                                "name": nm,
                                "primary_q": n.get("primary_q").cloned().unwrap_or(Value::Null),
                                "outputs": node_outs,
                                "routes": routes,
                                "default": default,
                                "questions": node_question_contract(n),
                            }))
                        })
                        .collect()
                })
                .unwrap_or_default();
            // Questions deduped by id (first wins, since a shared id means the
            // same question), plus the state fields a deterministic call reads.
            let mut qmap: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
            let inputs = spec_inputs(&spec);
            if let Some(ns) = spec["nodes"].as_array() {
                for n in ns {
                    if let Some(qs) = n.get("questions").and_then(Value::as_object) {
                        for (qid, q) in qs {
                            let af = q.pointer("/heuristic/answer_field").and_then(Value::as_str);
                            qmap.entry(qid.clone()).or_insert_with(|| {
                                let mut e = serde_json::Map::new();
                                if let Some(t) = q.get("type").and_then(Value::as_str) {
                                    e.insert("type".into(), json!(t));
                                }
                                if let Some(a) = af {
                                    e.insert("answer_field".into(), json!(a));
                                }
                                Value::Object(e)
                            });
                        }
                    }
                }
            }
            let questions: Vec<Value> = qmap
                .into_iter()
                .map(|(id, v)| {
                    let mut o = serde_json::Map::new();
                    o.insert("id".into(), json!(id));
                    if let Value::Object(m) = v {
                        for (k, val) in m {
                            o.insert(k, val);
                        }
                    }
                    Value::Object(o)
                })
                .collect();
            let live = live_only.get(&format!("{cat}/{stem}"));
            workflows.push(json!({
                "cat": cat,
                "name": stem,
                "spec_name": spec.get("name").cloned().unwrap_or(Value::Null),
                "description": spec.get("description").cloned().unwrap_or(Value::Null),
                "start": spec.get("start").cloned().unwrap_or(Value::Null),
                "node_count": nodes.len(),
                "nodes": nodes,
                "questions": questions,
                "inputs": inputs.into_iter().collect::<Vec<_>>(),
                "outputs": outputs,
                // true: behaviour-verified offline by `dsl smoke`, so an offline
                // `bdd_decide` on it is trustworthy. false: only verified live
                // (`verified_by` says how, `why_live` why it cannot go offline), so
                // an offline call would be a defaults-only walk.
                "offline": live.is_none(),
                "verified_by": live.and_then(|e| e.get("covered_by")).cloned().unwrap_or(Value::Null),
                "why_live": live.and_then(|e| e.get("reason")).cloned().unwrap_or(Value::Null),
                // false when a `call` node's payload can emit a label the spec
                // cannot enumerate, so `outputs` is not a proven-complete set.
                "outputs_complete": !has_opaque_outputs(&spec),
            }));
        }
        let offline_count = workflows
            .iter()
            .filter(|w| w.get("offline").and_then(Value::as_bool).unwrap_or(false))
            .count();
        Ok(json!({
            "count": workflows.len(),
            "offline_count": offline_count,
            "live_count": workflows.len() - offline_count,
            "workflows": workflows,
        }))
    }
}


/// `bdd_vocabulary`: the decision authoring vocabulary - the curated step
/// templates a `@decision` feature is built from, grouped by keyword kind
/// (given/when/then). This is the same list behind the `did you mean the
/// decision ... step` hints, so an agent can read the grammar *before* writing
/// a step instead of only discovering it from a red build. `bdd_new` opens the
/// skeleton; this says what to write inside it.
struct VocabularyTool;

impl McpTool for VocabularyTool {
    fn name(&self) -> &str {
        "bdd_vocabulary"
    }
    fn description(&self) -> &str {
        "The decision authoring vocabulary: the curated step templates a @decision feature is built from, grouped by keyword kind (given/when/then). Same list behind the `did you mean the decision ... step` hints - read it to author a step correctly the first time, then validate with bdd_validate (or `bdd build`). Copy a template and replace its <placeholders>; not an exhaustive grammar, but the phrasings the compiler accepts and the did-you-mean index knows."
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "required": [] })
    }
    fn call(&self, _args: &Value) -> Result<Value> {
        Ok(bdd::vocabulary_json())
    }
}
