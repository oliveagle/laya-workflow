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
                    validate_error = Some(format!("{e:#}"));
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
                "single": {"type": "boolean", "description": "Emit a 1-node start instead of the 2-node canonical graph (default false)."}
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
        let root = args
            .get("root")
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(bdd::repo_root);

        let desc = intent
            .clone()
            .unwrap_or_else(|| format!("The {name} decision workflow (scaffolded)."));
        let body = bdd::scaffold_body(name, cat, &primary_q, &desc, if single { 1 } else { 2 });
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
