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
        "Open a greenfield decision workflow: from an intent, generate a minimal valid @decision .feature skeleton that already passes bdd build — the cheapest valid start to the authoring loop. Returns the skeleton text (optionally writes it to bdd/workflows/<cat>/<name>.feature when write=true), the compile/validate verdict and next-step commands. Then extend + validate with bdd_validate and audit with bdd_catalog."
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

impl McpTool for DecideTool {
    fn name(&self) -> &str {
        "bdd_decide"
    }
    fn description(&self) -> &str {
        "Run a @decision workflow against a state and return the decision — the callable core of a BDD graph. Compiles the feature (or loads a checked-in spec_path) and executes it with the offline heuristic backend, the same deterministic engine `dsl smoke` and `bdd score --run` drive. Pass feature_text or feature_path, or spec_path, plus state (a JSON object). Returns the final label, the full trace (per-step node/action/answer) and the result object."
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
                "state": {"type": "object", "description": "The run state, e.g. {\"text\": \"Payments API returns 500\"} or the ev_* answer fields a heuristic-driven workflow reads."}
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

        let wf = if let Some(rel) = args.get("spec_path").and_then(Value::as_str) {
            let path = if std::path::Path::new(rel).is_absolute() {
                std::path::PathBuf::from(rel)
            } else {
                bdd::repo_root().join(rel)
            };
            crate::spec::load_file(&path.to_string_lossy())
                .map_err(|e| anyhow!("bdd_decide: cannot load spec {rel}: {e:#}"))?
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
                1 => crate::spec::load_file(&hits[0].to_string_lossy())
                    .map_err(|e| anyhow!("bdd_decide: cannot load {:?}: {e:#}", hits[0]))?,
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
            crate::spec::from_spec_with_dir(&spec, None, None)
                .map_err(|e| anyhow!("bdd_decide: {e:#}"))?
        };

        let out = wf
            .run(&crate::backend::HeuristicBackend, &state)
            .map_err(|e| anyhow!("bdd_decide: run failed: {e:#}"))?;
        let j = out.to_json();
        // The decision label is the projected `result.label` (or `gate_action`
        // for a gate node) — the same field `check_expect` pins; `trace.final_action`
        // is only the terminal verb ("stop").
        let label = j
            .pointer("/result/label")
            .and_then(Value::as_str)
            .or_else(|| j.pointer("/result/gate_action").and_then(Value::as_str))
            .unwrap_or_else(|| out.final_action())
            .to_string();
        Ok(json!({
            "ok": true,
            "label": label,
            "final_action": out.final_action(),
            "trace": j.get("trace").cloned().unwrap_or(Value::Null),
            "result": j.get("result").cloned().unwrap_or(Value::Null),
            "iterations": j.get("iterations").cloned().unwrap_or(Value::Null),
        }))
    }
}


/// `bdd_list`: the callable inventory of the BDD decision corpus — every
/// `@decision` workflow with the questions it asks (and the state field each
/// heuristic answer reads) and the nodes it walks, so an agent can pick the
/// right workflow and know what to feed `bdd_decide`. This is the discovery
/// half of "use a BDD graph as a decision core": you cannot call what you
/// cannot find.
struct ListTool;

impl McpTool for ListTool {
    fn name(&self) -> &str {
        "bdd_list"
    }
    fn description(&self) -> &str {
        "List the callable BDD decision corpus: every @decision workflow as {cat, name, description, start, nodes, questions, inputs}. `questions` gives each question's id and type (with its `answer_field` when a heuristic drives it); `inputs` is the sorted set of state fields a deterministic (offline) call reads — exactly what to put in `state` for `bdd_decide`. Filter with cat and/or name. The inventory behind 'use a BDD graph as a decision core'."
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
        let mut workflows: Vec<Value> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dsl) {
            let mut cats: Vec<_> = rd.flatten().collect();
            cats.sort_by_key(|e| e.file_name());
            for e in cats {
                if !e.path().is_dir() {
                    continue;
                }
                let cat = e.file_name().to_string_lossy().into_owned();
                if cat_filter.map(|f| f != cat).unwrap_or(false) {
                    continue;
                }
                let mut files: Vec<_> = std::fs::read_dir(e.path())
                    .map(|r| r.flatten().collect())
                    .unwrap_or_default();
                files.sort_by_key(|x| x.file_name());
                for fe in files {
                    let p = fe.path();
                    if p.extension().and_then(|x| x.to_str()) != Some("json") {
                        continue;
                    }
                    let fname = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    if fname.ends_with(".config.json") {
                        continue;
                    }
                    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    if name_filter.map(|f| f != stem).unwrap_or(false) {
                        continue;
                    }
                    let Ok(txt) = std::fs::read_to_string(&p) else { continue };
                    let Ok(spec) = serde_json::from_str::<Value>(&txt) else { continue };
                    if spec.get("nodes").and_then(Value::as_array).is_none() || spec.get("start").is_none() {
                        continue;
                    }
                    let nodes: Vec<Value> = spec["nodes"]
                        .as_array()
                        .map(|ns| {
                            ns.iter()
                                .filter_map(|n| {
                                    let nm = n.get("name")?.as_str()?;
                                    Some(json!({
                                        "name": nm,
                                        "primary_q": n.get("primary_q").cloned().unwrap_or(Value::Null),
                                    }))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    // Questions deduped by id (first wins, since a shared id
                    // means the same question), plus the state fields a
                    // deterministic call reads.
                    let mut qmap: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
                    let mut inputs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
                    if let Some(ns) = spec["nodes"].as_array() {
                        for n in ns {
                            if let Some(qs) = n.get("questions").and_then(Value::as_object) {
                                for (qid, q) in qs {
                                    let af = q.pointer("/heuristic/answer_field").and_then(Value::as_str);
                                    if let Some(a) = af {
                                        inputs.insert(a.to_string());
                                    }
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
                    }));
                }
            }
        }
        Ok(json!({
            "count": workflows.len(),
            "workflows": workflows,
        }))
    }
}
