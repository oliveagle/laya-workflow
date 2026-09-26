//! `math` capability: expression evaluation and descriptive statistics.
//!
//! Split out of `data.rs` to keep that module under the repo's 1000-line limit.
//! Non-finite results (overflow to `inf`, `0/0` to `NaN`) are rejected rather
//! than serialised: serde_json turns those into `null`, which would hand the
//! caller a silent wrong value instead of an error.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use super::data::get_text;
use super::effective_op;

// ── math ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct MathCap {
    /// `eval` | `stats`
    pub op: String,
}

pub fn call_math(c: &MathCap, with: &Value, _state: &Value) -> Result<Value> {
    let op = effective_op(&c.op, with, "eval");
        match op.as_str() {
        "eval" => {
            let expr = get_text(with, "expression")?;
            let v = eval_expr(&expr).ok_or_else(|| anyhow!("math.eval: cannot parse {expr:?}"))?;
            // A non-finite result (2^10000 -> inf, 0/0 -> NaN) used to serialise
            // to JSON `null`, so the caller got a silent wrong value instead of
            // an error. Reject it: an overflowed computation is not a number the
            // workflow can use.
            if !v.is_finite() {
                bail!("math.eval: {expr:?} is not a finite number (overflow or undefined)");
            }
            Ok(json!({ "capability": "math", "op": "eval", "expression": expr, "value": v }))
        }
        "stats" => {
            let nums: Vec<f64> = with
                .get("values")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow!("math.stats needs 'values'"))?
                .iter()
                .filter_map(|x| x.as_f64())
                .collect();
            if nums.is_empty() {
                bail!("math.stats: no numeric values");
            }
            let n = nums.len() as f64;
            let sum: f64 = nums.iter().sum();
            let mean = sum / n;
            if !mean.is_finite() {
                bail!("math.stats: sum/mean overflowed to a non-finite value");
            }
            let mut sorted = nums.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let pct = |p: f64| -> f64 {
                let idx = ((p / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
                sorted[idx.min(sorted.len() - 1)]
            };
            let var = nums.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
            let stddev = var.sqrt();
            // Non-finite floats serialise to JSON `null`, so a caller would read
            // a *silent* wrong value (observed: `stddev: null, mean: null` for
            // values that overflow). Error instead.
            if !stddev.is_finite() {
                bail!("math.stats: stddev is not a finite number (overflow)");
            }
            Ok(json!({
                "capability": "math", "op": "stats",
                "count": nums.len(), "sum": sum, "mean": mean,
                "min": sorted.first().copied(),
                "max": sorted.last().copied(),
                "stddev": stddev,
                "p50": pct(50.0), "p90": pct(90.0), "p99": pct(99.0),
            }))
        }
        other => bail!("math op {other:?} unsupported (eval | stats)"),
    }
}

/// Tiny recursive-descent calculator: + - * / % ^ parentheses, unary minus.
pub fn eval_expr(s: &str) -> Option<f64> {
    struct P<'a> {
        b: &'a [u8],
        i: usize,
    }
    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.b.len() && (self.b[self.i] as char).is_whitespace() {
                self.i += 1;
            }
        }
        fn expr(&mut self) -> Option<f64> {
            let mut v = self.term()?;
            loop {
                self.ws();
                match self.b.get(self.i) {
                    Some(b'+') => {
                        self.i += 1;
                        v += self.term()?;
                    }
                    Some(b'-') => {
                        self.i += 1;
                        v -= self.term()?;
                    }
                    _ => return Some(v),
                }
            }
        }
        fn term(&mut self) -> Option<f64> {
            let mut v = self.pow()?;
            loop {
                self.ws();
                match self.b.get(self.i) {
                    Some(b'*') => {
                        self.i += 1;
                        v *= self.pow()?;
                    }
                    Some(b'/') => {
                        self.i += 1;
                        let d = self.pow()?;
                        if d == 0.0 {
                            return None;
                        }
                        v /= d;
                    }
                    Some(b'%') => {
                        self.i += 1;
                        let d = self.pow()?;
                        if d == 0.0 {
                            return None;
                        }
                        v %= d;
                    }
                    _ => return Some(v),
                }
            }
        }
        fn pow(&mut self) -> Option<f64> {
            let base = self.unary()?;
            self.ws();
            if self.b.get(self.i) == Some(&b'^') {
                self.i += 1;
                let e = self.pow()?;
                return Some(base.powf(e));
            }
            Some(base)
        }
        fn unary(&mut self) -> Option<f64> {
            self.ws();
            if self.b.get(self.i) == Some(&b'-') {
                self.i += 1;
                return Some(-self.unary()?);
            }
            if self.b.get(self.i) == Some(&b'+') {
                self.i += 1;
                return self.unary();
            }
            self.atom()
        }
        fn atom(&mut self) -> Option<f64> {
            self.ws();
            if self.b.get(self.i) == Some(&b'(') {
                self.i += 1;
                let v = self.expr()?;
                self.ws();
                if self.b.get(self.i) == Some(&b')') {
                    self.i += 1;
                    return Some(v);
                }
                return None;
            }
            let start = self.i;
            while self.i < self.b.len()
                && ((self.b[self.i] as char).is_ascii_digit() || self.b[self.i] == b'.')
            {
                self.i += 1;
            }
            if start == self.i {
                return None;
            }
            std::str::from_utf8(&self.b[start..self.i]).ok()?.parse().ok()
        }
    }
    let mut p = P { b: s.as_bytes(), i: 0 };
    let v = p.expr()?;
    p.ws();
    if p.i == p.b.len() {
        Some(v)
    } else {
        None
    }
}

