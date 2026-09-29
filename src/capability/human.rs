//! Human-like input shaping for the CDP `chrome_cdp` capability.
//!
//! The engine drives a *person's* real Chrome, so its input should carry the
//! texture a person's input carries: the pointer travels along a curve and
//! eases into the target instead of teleporting onto it, the hand hovers a beat
//! before it commits, and keys land one at a time with the uneven rhythm of a
//! typist. Anti-bot middleware (Taobao's baxia `punish` wall, and most others)
//! keys on the *absence* of that texture: an instant move-and-click, or a whole
//! string delivered in one `Input.insertText`, is a signature no human produces,
//! and the site answers with a 滑块 / 验证码 challenge.
//!
//! This module is deliberately generic — it knows nothing about any one site.
//! It only turns "click here" and "type this" into the event stream a person
//! could have produced, and it stays fully deterministic-optional: the caller
//! gates it on the capability's `human` setting, so a probe that wants raw speed
//! (or a headless fixture) pays no cost.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A tiny xorshift PRNG. No `rand` dependency; seeded from the clock so the
/// jitter differs run to run while staying cheap and local.
pub struct Rng(u64);

impl Rng {
    pub fn from_clock() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x2545_F491_4F6C_DD1D);
        Rng((nanos ^ 0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    /// Uniform integer in [lo, hi]; `lo` when the range is empty.
    pub fn ms(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        lo + self.next_u64() % (hi - lo + 1)
    }
}

// ── cursor memory ───────────────────────────────────────────────────────────
// A mouse has a position, and a person's next move starts where the last one
// ended. Remembering it is what turns a teleport into a reach.

static CUR_X: AtomicU64 = AtomicU64::new(0);
static CUR_Y: AtomicU64 = AtomicU64::new(0);
static CUR_SET: AtomicU64 = AtomicU64::new(0);

pub fn set_cursor(x: f64, y: f64) {
    CUR_X.store(x.to_bits(), Ordering::Relaxed);
    CUR_Y.store(y.to_bits(), Ordering::Relaxed);
    CUR_SET.store(1, Ordering::Relaxed);
}

/// Where the pointer is, or `fallback` before the first move of the session.
pub fn last_cursor(fallback: (f64, f64)) -> (f64, f64) {
    if CUR_SET.load(Ordering::Relaxed) == 0 {
        return fallback;
    }
    (
        f64::from_bits(CUR_X.load(Ordering::Relaxed)),
        f64::from_bits(CUR_Y.load(Ordering::Relaxed)),
    )
}

// ── pointer ─────────────────────────────────────────────────────────────────

/// The points a hand would pass through reaching from `from` to `to`: a gently
/// bowed arc, sampled along an ease-in-out curve, so the pointer accelerates
/// off the start and decelerates onto the target.
pub fn mouse_track(from: (f64, f64), to: (f64, f64), rng: &mut Rng) -> Vec<(f64, f64)> {
    let (x0, y0) = from;
    let (x1, y1) = to;
    let dx = x1 - x0;
    let dy = y1 - y0;
    let dist = (dx * dx + dy * dy).sqrt();
    if dist < 3.0 {
        return vec![(x1, y1)];
    }
    // One control point, pushed off the straight line by a small perpendicular
    // bow, so the path arcs like a wrist rather than a ruler.
    let side = if rng.unit() < 0.5 { -1.0 } else { 1.0 };
    let bow = dist * rng.range(0.05, 0.16) * side;
    let mid_x = (x0 + x1) / 2.0 + (-dy / dist) * bow;
    let mid_y = (y0 + y1) / 2.0 + (dx / dist) * bow;

    let steps = (dist / 26.0).clamp(9.0, 24.0) as usize;
    let mut pts = Vec::with_capacity(steps);
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let e = t * t * (3.0 - 2.0 * t); // smoothstep: ease in, ease out
        let u = 1.0 - e;
        pts.push((
            u * u * x0 + 2.0 * u * e * mid_x + e * e * x1,
            u * u * y0 + 2.0 * u * e * mid_y + e * e * y1,
        ));
    }
    pts
}

/// Gap between two sampled points of the path, milliseconds.
pub fn move_gap_ms(rng: &mut Rng) -> u64 {
    rng.ms(6, 22)
}

/// Reaction time: notice the target, then commit. Milliseconds.
pub fn reaction_ms(rng: &mut Rng) -> u64 {
    rng.ms(140, 420)
}

/// Hover dwell: the hand settles on the target before pressing.
pub fn hover_ms(rng: &mut Rng) -> u64 {
    rng.ms(90, 260)
}

/// How long the button stays down.
pub fn hold_ms(rng: &mut Rng) -> u64 {
    rng.ms(55, 150)
}

/// Time between two wheel notches of a scroll.
pub fn wheel_gap_ms(rng: &mut Rng) -> u64 {
    rng.ms(35, 110)
}

// ── keyboard ────────────────────────────────────────────────────────────────

/// Per-character delay. Typists are uneven: brisk inside a word, a longer beat
/// at a space or punctuation, and the odd mid-thought hesitation.
pub fn key_delay_ms(rng: &mut Rng, ch: char, index: usize) -> u64 {
    let mut ms = if ch == ' ' {
        rng.ms(120, 300)
    } else if ch.is_ascii_punctuation() {
        rng.ms(140, 330)
    } else {
        rng.ms(65, 185)
    };
    if index > 0 && index % 7 == 0 && rng.unit() < 0.6 {
        ms += rng.ms(180, 520); // a pause to "think"
    }
    ms
}
