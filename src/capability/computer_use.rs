//! Bounded UI decision cycle (port of awesome-jev `examples/computer-use`).
//!
//! One cycle, four separated stages — this module implements the *executor*
//! half (the `prepare` half is workflow policy in the spec):
//!
//! 1. **prepare** (spec action `kind: "ui_proposal"`) — turn validated answers
//!    into a bounded proposal; anything below `min_confidence` or a
//!    `wait`/`blocked` operation never becomes an action.
//! 2. **fingerprint** — sha256 of the *canonical* (keys sorted) observation
//!    snapshot, captured before inference.
//! 3. **step** — the executor. Re-checks, in order: the proposal is `ready`,
//!    the surface is in the caller's allow-list, the **fresh** snapshot still
//!    hashes to the proposal's captured fingerprint (stale observations stop
//!    here — this is the TOCTOU guard), the target field is in the caller's
//!    permission scope, and the target is an enabled textbox. At most **one**
//!    field changes; no navigation, submit, script execution or text
//!    generation is representable. Nothing ever submits: `submitted` stays
//!    false and there is no op that could set it.
//! 4. **verify** — an *independent oracle*. The expected IDs/values live in the
//!    capability config and are never part of the workflow state the model
//!    sees; a wrong-but-plausible selection still fails the checks.
//!
//! The fixture is in-memory JSON (no browser/OS is driven here); a real driver
//! replaces `step`'s application but must keep the same recheck order, resolve
//! a fresh ref, dispatch once, and read the value back.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use super::{effective_op, expand, net::sha256_hex, Policy};

#[derive(Clone, Debug, Default)]
pub struct ComputerUseCap {
    /// `fingerprint` | `step` | `verify` (overridable per call via `with.op`)
    pub op: String,
    /// The only surface the caller permits mutations on.
    pub allowed_surface: String,
    /// Field ids the caller permits a `fill` on.
    pub allowed_fields: Vec<String>,
    /// Independent oracle expectations: `billing_field`, `delivery_field`,
    /// `delivery_value`, `amount_source`, `amount_value`.
    pub expected: Value,
    /// Confidence floor mirrored from the prepare stage (default 0.8).
    pub min_confidence: f64,
}

/// Canonical JSON: object keys sorted at every depth, so two snapshots that
/// differ only in key insertion order hash identically (awesome-jev uses
/// `json.dumps(..., sort_keys=True, allow_nan=False)`; JSON has no NaN).
fn canonical(v: &Value) -> String {
    match v {
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| format!("{}:{}", Value::String((*k).clone()), canonical(&o[*k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        other => other.to_string(),
    }
}

fn fingerprint(snapshot: &Value) -> String {
    sha256_hex(canonical(snapshot).as_bytes())
}

fn surface_of(snapshot: &Value) -> Result<String> {
    snapshot
        .get("surface")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("snapshot missing 'surface'"))
}

pub fn call_computer_use(
    c: &ComputerUseCap,
    with: &Value,
    state: &Value,
    policy: &Policy,
) -> Result<Value> {
    let _ = policy;
    let with = expand(with, state, with);
    let op = effective_op(&c.op, &with, "fingerprint");
    match op.as_str() {
        "fingerprint" => {
            let snapshot = with
                .get("snapshot")
                .cloned()
                .ok_or_else(|| anyhow!("computer_use.fingerprint needs 'snapshot'"))?;
            Ok(json!({
                "capability": "computer_use",
                "op": "fingerprint",
                "snapshot_hash": fingerprint(&snapshot),
                "surface": surface_of(&snapshot)?,
                "revision": snapshot.get("revision").cloned().unwrap_or(Value::Null),
            }))
        }
        "step" => {
            let proposal = with
                .get("proposal")
                .cloned()
                .ok_or_else(|| anyhow!("computer_use.step needs 'proposal'"))?;
            let current = with
                .get("snapshot")
                .cloned()
                .ok_or_else(|| anyhow!("computer_use.step needs 'snapshot'"))?;
            let values = with.get("values").cloned().unwrap_or_else(|| json!({}));

            // ── executor rechecks, in the same order as awesome-jev ──
            if proposal.get("status").and_then(|v| v.as_str()) != Some("ready") {
                bail!(
                    "only a ready proposal can reach the executor (got {:?})",
                    proposal.get("status")
                );
            }
            let allowed_surface = if c.allowed_surface.is_empty() {
                with.get("allowed_surface").and_then(|v| v.as_str()).unwrap_or("")
            } else {
                &c.allowed_surface
            };
            if allowed_surface.is_empty() {
                bail!("computer_use: no allowed_surface configured (fail closed)");
            }
            let prop_surface = proposal
                .get("surface")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if surface_of(&current)? != allowed_surface || prop_surface != allowed_surface {
                bail!("surface is outside the caller's allow-list");
            }
            if fingerprint(&current) != proposal.get("snapshot_hash").and_then(|v| v.as_str()).unwrap_or("") {
                bail!("stale observation; choose again using fresh state");
            }
            let mut after = current.clone(); // deep clone via serde Value
            let action = proposal.get("action").cloned().unwrap_or(Value::Null);
            let applied = if action.is_null() {
                // a `done` answer still has to pass the oracle below
                Value::Null
            } else {
                let kind = action.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                if kind != "fill" {
                    bail!("action {kind:?} is outside the caller's permission scope (fill only)");
                }
                let target_id = action
                    .get("target_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("fill action needs 'target_id'"))?;
                let allowed_fields: Vec<String> = if c.allowed_fields.is_empty() {
                    with.get("allowed_fields")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                        .unwrap_or_default()
                } else {
                    c.allowed_fields.clone()
                };
                if !allowed_fields.iter().any(|f| f == target_id) {
                    bail!("target {target_id:?} is outside the caller's permission scope");
                }
                let value_key = action
                    .get("value_key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("fill action needs 'value_key'"))?;
                let value = values.get(value_key).ok_or_else(|| {
                    anyhow!("supplied value {value_key:?} missing (values are local, not model-visible)")
                })?;
                let mut applied = false;
                if let Some(obj) = after.as_object_mut() {
                    if let Some(elements) = obj.get_mut("elements").and_then(|v| v.as_array_mut()) {
                        let mut seen = None;
                        for (i, el) in elements.iter().enumerate() {
                            if el.get("id").and_then(|v| v.as_str()) == Some(target_id) {
                                seen = Some(i);
                                break;
                            }
                        }
                        let i = seen.ok_or_else(|| anyhow!("target {target_id:?} is missing"))?;
                        let role = elements[i].get("role").and_then(|v| v.as_str()).unwrap_or("");
                        let enabled = elements[i].get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                        if role != "textbox" || !enabled {
                            bail!("target {target_id:?} is disabled or not editable");
                        }
                        elements[i]["value"] = value.clone();
                        applied = true;
                    }
                }
                // at most one field changed; `submitted` is never touched
                if !applied {
                    bail!("snapshot has no editable elements");
                }
                if let Some(obj) = after.as_object_mut() {
                    let rev = obj.get("revision").and_then(|v| v.as_i64()).unwrap_or(0);
                    obj["revision"] = json!(rev + 1);
                }
                action
            };

            // Freshness guard *after* mutation: this pair must not drift apart
            // in a real driver either (read-back is the driver's job).
            Ok(json!({
                "capability": "computer_use",
                "op": "step",
                "status": if applied.is_null() { "unchanged" } else { "applied" },
                "snapshot": after,
                "applied_action": applied,
                "one_field_only": true,
                "submitted": false,
            }))
        }
        "verify" => {
            let after = with
                .get("after")
                .cloned()
                .ok_or_else(|| anyhow!("computer_use.verify needs 'after'"))?;
            let proposal = with
                .get("proposal")
                .cloned()
                .ok_or_else(|| anyhow!("computer_use.verify needs 'proposal'"))?;
            let expected = if c.expected.is_null() {
                with.get("expected").cloned().unwrap_or(Value::Null)
            } else {
                c.expected.clone()
            };
            let expected = expected
                .as_object()
                .ok_or_else(|| anyhow!("computer_use.verify: no oracle 'expected' configured"))?;
            let get_el = |id: &str| -> Option<&Value> {
                after
                    .get("elements")
                    .and_then(|v| v.as_array())?
                    .iter()
                    .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id))
            };
            let mut checks = serde_json::Map::new();
            // billing field holds the supplied value
            let bf = expected.get("billing_field").and_then(|v| v.as_str()).unwrap_or("");
            let bv = expected.get("billing_value").cloned().unwrap_or(Value::Null);
            checks.insert(
                "billing_value".into(),
                json!(get_el(bf).and_then(|e| e.get("value")) == Some(&bv)),
            );
            // delivery contact preserved
            let df = expected.get("delivery_field").and_then(|v| v.as_str()).unwrap_or("");
            let dv = expected.get("delivery_value").cloned().unwrap_or(Value::Null);
            checks.insert(
                "delivery_unchanged".into(),
                json!(get_el(df).and_then(|e| e.get("value")) == Some(&dv)),
            );
            // nothing was submitted
            checks.insert(
                "not_submitted".into(),
                json!(after.get("submitted").and_then(|v| v.as_bool()) == Some(false)),
            );
            // extraction came from the right source
            let asrc = expected.get("amount_source").and_then(|v| v.as_str()).unwrap_or("");
            checks.insert(
                "amount_source".into(),
                json!(proposal.pointer("/extracted/source_id").and_then(|v| v.as_str()) == Some(asrc)),
            );
            let aval = expected.get("amount_value").and_then(|v| v.as_str()).unwrap_or("");
            checks.insert(
                "amount_value".into(),
                json!(proposal.pointer("/extracted/value").and_then(|v| v.as_str()) == Some(aval)),
            );
            let all_ok = checks.values().all(|v| v == &json!(true));
            Ok(json!({
                "capability": "computer_use",
                "op": "verify",
                "status": if all_ok { "verified" } else { "failed" },
                "checks": checks,
            }))
        }
        other => bail!("computer_use op {other:?} unsupported (fingerprint | step | verify)"),
    }
}
