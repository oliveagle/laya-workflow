//! Cross-implementation parity: the Rust MLX runtime vs a frozen golden produced
//! by the upstream Python `laya-mlx` package (`mizorewww/laya-mlx` 0.2.0).
//!
//! Skips (passes) when no checkpoint is available, so CPU-only CI is unaffected.

use std::path::PathBuf;

use serde_json::Value;

fn fixtures() -> Option<(Value, Value)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../laya-tch/mlx/tests");
    let cases = std::fs::read_to_string(dir.join("parity_cases.json")).ok()?;
    let expected = std::fs::read_to_string(dir.join("parity_expected.json")).ok()?;
    Some((serde_json::from_str(&cases).ok()?, serde_json::from_str(&expected).ok()?))
}

#[test]
fn matches_python_golden() {
    let dir = match laya_mlx::runtime::resolve_model_dir(None) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("skipping parity test: {e}");
            return;
        }
    };
    let Some((cases, expected)) = fixtures() else {
        eprintln!("skipping parity test: fixtures missing");
        return;
    };
    let agent = laya_mlx::runtime::Agent::load(&dir).expect("load agent");

    let mut prob_max = 0.0f64;
    for (name, req) in cases.as_object().unwrap() {
        let state = req.get("state").unwrap();
        let questions = req.get("questions").unwrap();
        let got = agent.system_one(state, questions).expect("inference");
        let want = &expected[name]["answers"];
        for (qid, w) in want.as_object().unwrap() {
            let g = &got["answers"][qid];
            if let Some(c) = w.get("choice") {
                assert_eq!(g["choice"], *c, "choice mismatch {name}/{qid}");
            }
            for k in ["noul", "score"] {
                if let Some(v) = w.get(k) {
                    let d = (g[k].as_f64().unwrap() - v.as_f64().unwrap()).abs();
                    assert!(d < 5e-3, "{name}/{qid} {k}: rust={} py={} d={d}", g[k], v);
                }
            }
            if let Some(probs) = w.get("probabilities").and_then(|p| p.as_object()) {
                for (lab, pv) in probs {
                    let d = (g["probabilities"][lab].as_f64().unwrap() - pv.as_f64().unwrap()).abs();
                    prob_max = prob_max.max(d);
                    assert!(d < 3e-3, "{name}/{qid} p[{lab}]: rust={} py={} d={d}", g["probabilities"][lab], pv);
                }
            }
        }
    }
    eprintln!("parity ok, max probability diff = {prob_max:.5}");
}
