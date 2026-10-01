//! Phase 5: Nemori-style episode segmentation for laya-mem.
//!
//! Jev-Mem groups consecutive conversation turns into semantic episodes using
//! an LLM boundary detector (`memory/episode_segmenter.py`). We implement the
//! same state machine with deterministic offline heuristics — no LLM needed
//! for the boundary decision, and the episode summary is assembled from the
//! buffered turns instead of being generated.
//!
//! A boundary is detected when any of these hold against the *last buffered*
//! turn:
//! 1. calendar-day change between `ts` fields (Jev-Mem `_check_explicit_signals`)
//! 2. an explicit topic marker ("by the way", "anyway", …) in the new turn
//! 3. zero token overlap between the new turn and the previous turn (a proxy
//!    for "semantically unrelated"; real embeddings via
//!    `LAYA_MEM_EMBEDDING_URL` make this genuinely semantic)
//! 4. the buffer hit `max_turns` (forced boundary, Jev-Mem `is_full`)
//!
//! When a boundary fires and the buffer holds at least `min_turns` turns, an
//! EPISODE memory row is written through the caller-provided hook. The
//! original turns are never deleted — Jev-Mem's non-destructive invariant.

use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;

use crate::laya_mem_vec::{cosine, encode_mock, upsert_vector, MOCK_DIM};

/// Default buffer ceiling; mirrors Jev-Mem `MessageBuffer(max_buffer_size=10)`.
pub const DEFAULT_MAX_TURNS: usize = 10;
/// Minimum turns required before an episode is created.
pub const DEFAULT_MIN_TURNS: usize = 2;

/// Explicit topic markers lifted from Jev-Mem's `_check_explicit_signals`.
const TOPIC_MARKERS: [&str; 6] = [
    "by the way",
    "anyway",
    "changing the subject",
    "moving on",
    "on another note",
    "different topic",
];

/// One buffered conversation turn (what [`EpisodeSegmenter::process_turn`]
/// receives).
#[derive(Debug, Clone)]
pub struct Turn {
    pub memory_id: String,
    pub content: String,
    pub ts: String,
}

/// An episode that was just flushed out of the buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct Episode {
    pub title: String,
    pub content: String,
    pub participant_hint: Option<String>,
    pub start_ts: String,
    pub end_ts: String,
    pub turn_ids: Vec<String>,
    pub boundary_reason: String,
}

/// Calendar-day comparison on RFC3339/ISO8601 timestamps: `2026-10-01T...`
/// is compared by its leading 10 chars. Malformed timestamps never trigger.
fn same_calendar_day(a: &str, b: &str) -> Option<bool> {
    let da = a.get(0..10)?;
    let db = b.get(0..10)?;
    Some(da == db)
}

/// True when any topic marker appears in `text` (case-insensitive).
fn has_topic_marker(text: &str) -> bool {
    let lower = text.to_lowercase();
    TOPIC_MARKERS.iter().any(|m| lower.contains(m))
}

/// Cheap token overlap for the offline "semantically unrelated" heuristic.
fn token_set(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(|t| t.to_lowercase())
        .collect()
}

/// Jaccard overlap of two token sets.
fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    inter / union
}

// ─── segmenter ─────────────────────────────────────────────────────────────

/// The stateful segmenter. Kept in memory behind a `Mutex` inside
/// [`crate::laya_mem::LayaMemTools`]; an MCP server keeps it warm across many
/// `laya_mem_persist` calls. Each new `laya-workflow laya-mem persist`
/// invocation starts a fresh buffer — the trade-off is documented and
/// non-destructive (EPISODE rows persist in SQLite even if the buffer resets).
#[derive(Debug)]
pub struct EpisodeSegmenter {
    pub max_turns: usize,
    pub min_turns: usize,
    buffer: Vec<Turn>,
}

impl Default for EpisodeSegmenter {
    fn default() -> Self {
        Self {
            max_turns: DEFAULT_MAX_TURNS,
            min_turns: DEFAULT_MIN_TURNS,
            buffer: Vec::new(),
        }
    }
}

impl EpisodeSegmenter {
    pub fn new(max_turns: usize, min_turns: usize) -> Self {
        Self {
            max_turns: max_turns.max(2),
            min_turns: min_turns.max(1),
            buffer: Vec::new(),
        }
    }

    /// Buffer size.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Detect a boundary against `prev` (None when the buffer is empty).
    /// Returns the reason string, or `None` when there is no boundary.
    pub fn boundary_reason(prev: Option<&Turn>, new: &Turn, max_turns: usize, current_len: usize) -> Option<String> {
        if current_len >= max_turns {
            return Some(format!("max buffer size ({max_turns}) reached"));
        }
        let prev = prev?;
        // 1. calendar-day change
        if let Some(false) = same_calendar_day(&prev.ts, &new.ts) {
            return Some("calendar-day change".to_string());
        }
        // 2. explicit topic marker
        if has_topic_marker(&new.content) {
            return Some("explicit topic marker".to_string());
        }
        // 3. zero token overlap (semantic gap)
        let prev_tokens = token_set(&prev.content);
        let new_tokens = token_set(&new.content);
        if !prev_tokens.is_empty() && !new_tokens.is_empty() {
            if jaccard(&prev_tokens, &new_tokens) == 0.0 {
                // Also check the dense side, which catches cross-language /
                // morphological variants that share a hashed slot.
                let a = encode_mock(&prev.content, MOCK_DIM);
                let b = encode_mock(&new.content, MOCK_DIM);
                if cosine(&a, &b) < 0.02 {
                    return Some("no token or dense overlap with previous turn".to_string());
                }
            }
        }
        None
    }

    /// Append a turn and return an [`Episode`] if the buffer flushed.
    pub fn process_turn(&mut self, turn: Turn) -> Option<Episode> {
        let reason = {
            let prev = self.buffer.last();
            let len = self.buffer.len();
            Self::boundary_reason(prev, &turn, self.max_turns, len)
        };
        if reason.is_some() && self.buffer.len() >= self.min_turns {
            let ep = self.flush_locked(reason.unwrap_or_default());
            self.buffer.push(turn);
            return Some(ep);
        }
        self.buffer.push(turn);
        None
    }

    /// Flush whatever is left in the buffer (call at end-of-conversation).
    pub fn flush_remaining(&mut self, reason: &str) -> Option<Episode> {
        if self.buffer.len() >= self.min_turns {
            Some(self.flush_locked(reason.to_string()))
        } else {
            None
        }
    }

    /// Reset without emitting an episode (used after an unrecoverable error).
    pub fn reset(&mut self) {
        self.buffer.clear();
    }

    fn flush_locked(&mut self, boundary_reason: String) -> Episode {
        let turns: Vec<Turn> = std::mem::take(&mut self.buffer);
        // The turn that triggered the flush is pushed by the caller after the
        // episode is emitted, so `turns` holds the *previous* turns only.
        let start_ts = turns.first().map(|t| t.ts.clone()).unwrap_or_default();
        let end_ts = turns.last().map(|t| t.ts.clone()).unwrap_or_default();
        let turn_ids: Vec<String> = turns.iter().map(|t| t.memory_id.clone()).collect();
        let (title, content, participant_hint) = summarize_episode(&turns);
        Episode {
            title,
            content,
            participant_hint,
            start_ts,
            end_ts,
            turn_ids,
            boundary_reason,
        }
    }
}

/// Deterministic 3-part summary — no LLM needed:
/// * title: first sentence (trimmed to 60 chars) or "Conversation episode"
/// * content: `Episode covering N turns: <first turn> … <last turn>`
/// * participant hint: first mention of an `@name` or capitalized name, if any
fn summarize_episode(turns: &[Turn]) -> (String, String, Option<String>) {
    let first = turns.first().map(|t| t.content.as_str()).unwrap_or("");
    let last = turns.last().map(|t| t.content.as_str()).unwrap_or("");
    let title_sentence: String = first
        .split(|c: char| c == '.' || c == '!' || c == '?')
        .next()
        .unwrap_or(first)
        .trim()
        .chars()
        .take(60)
        .collect();
    let title = if title_sentence.is_empty() {
        "Conversation episode".to_string()
    } else {
        title_sentence
    };
    let content = format!(
        "Episode covering {} turns: {} … {}",
        turns.len(),
        first.chars().take(120).collect::<String>(),
        last.chars().take(120).collect::<String>(),
    );
    // A very cheap participant hint: first `@word` in the combined text.
    let combined: String = turns.iter().map(|t| t.content.as_str()).collect::<Vec<_>>().join(" ");
    let participant_hint = combined
        .split_whitespace()
        .find(|w| w.starts_with('@') && w.len() > 1)
        .map(|w| w.trim_start_matches('@').to_string());
    (title, content, participant_hint)
}

// ─── process-level registry ────────────────────────────────────────────────

/// One segmenter per SQLite store, shared by every `laya_mem_persist` call in
/// this process. Keyed by the *string* form of the db path so an MCP server
/// keeps one warm buffer per store (Jev-Mem keeps a single in-memory buffer;
/// our MCP tools accept a per-call `db_path`, so we widen it to a map).
///
/// `OnceLock` is used rather than `static` because `HashMap::new()` is not
/// const (and we want zero allocations when the feature is off).
type SegMap = std::collections::HashMap<String, std::sync::Arc<std::sync::Mutex<EpisodeSegmenter>>>;
static REGISTRY: std::sync::OnceLock<std::sync::Mutex<SegMap>> = std::sync::OnceLock::new();

fn registry() -> &'static std::sync::Mutex<SegMap> {
    REGISTRY.get_or_init(|| std::sync::Mutex::new(SegMap::new()))
}

/// Enable segmentation? Default is **on**: a full memory system segments
/// episodes without being asked, exactly like Jev-Mem's `EpisodeSegmenter`.
/// `LAYA_MEM_EPISODES=0` turns it off (e.g. for a store that only wants
/// raw observations).
pub fn episodes_enabled() -> bool {
    match std::env::var("LAYA_MEM_EPISODES") {
        Ok(v) => v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("on"),
        Err(_) => true,
    }
}

fn env_usize(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .filter(|v| !v.is_empty())
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

/// Feed one freshly-persisted memory into the segmenter for `db_path`.
/// If that flushes an episode, the EPISODE row is written (and vector-indexed)
/// here. Returns the EPISODE memory id when one was created, `None` otherwise.
/// Never fails: episode bookkeeping must not break a successful persist.
pub fn feed(db_path: &Path, memory_id: i64, content: &str, ts: &str) -> Option<i64> {
    if !episodes_enabled() {
        return None;
    }
    let key = db_path.display().to_string();
    let seg = {
        let mut map = registry().lock().unwrap_or_else(|e| e.into_inner());
        map.entry(key.clone())
            .or_insert_with(|| {
                std::sync::Arc::new(std::sync::Mutex::new(EpisodeSegmenter::new(
                    env_usize("LAYA_MEM_EPISODE_MAX_TURNS", DEFAULT_MAX_TURNS),
                    env_usize("LAYA_MEM_EPISODE_MIN_TURNS", DEFAULT_MIN_TURNS),
                )))
            })
            .clone()
    };
    let episode = {
        let mut s = seg.lock().unwrap_or_else(|e| e.into_inner());
        s.process_turn(Turn {
            memory_id: memory_id.to_string(),
            content: content.to_string(),
            ts: ts.to_string(),
        })
    };
    let ep = episode?;
    match persist_episode_row(db_path, &ep) {
        Ok(id) if id > 0 => Some(id),
        _ => None,
    }
}

/// Return the current buffer size for `db_path` (0 when unknown / off).
pub fn buffered_turns(db_path: &Path) -> usize {
    let key = db_path.display().to_string();
    let map = registry().lock().unwrap_or_else(|e| e.into_inner());
    map.get(&key)
        .map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).len())
        .unwrap_or(0)
}

// ─── persistence hook ───────────────────────────────────────────────────────

/// Persist an EPISODE memory row via SQL and index its vector. Returns the
/// new memory id. Uses the same `sql_escape` plumbing as the rest of laya-mem.
pub fn persist_episode_row(
    db_path: &Path,
    episode: &Episode,
) -> anyhow::Result<i64> {
    let entities = match &episode.participant_hint {
        Some(p) => serde_json::to_string(&json!([p]))?,
        None => "[]".to_string(),
    };
    let sources = serde_json::to_string(&episode.turn_ids)?;
    let esc = crate::laya_mem_util::sql_escape;
    let ts_esc = esc(&now_or(&episode.end_ts));
    let sql = format!(
        "INSERT INTO memories (content, ts, entities, type_scores, node_type, source_memory_ids) VALUES ('{}', '{}', '{}', '{{}}', 'EPISODE', '{}')",
        esc(&episode.content),
        ts_esc,
        esc(&entities),
        esc(&sources),
    );
    let res = crate::laya_mem_util::call_db_for_db(
        db_path,
        json!({ "op": "exec", "statements": [sql, "SELECT last_insert_rowid() AS id"] }),
    )?;
    let id = res
        .get("rows")
        .and_then(|r| r.as_array())
        .and_then(|a| a.first())
        .and_then(|row| row.get("id"))
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    if id > 0 {
        upsert_vector(db_path, &id.to_string(), &encode_mock(&episode.content, MOCK_DIM))?;
    }
    Ok(id)
}

fn now_or(preferred: &str) -> String {
    if preferred.is_empty() {
        crate::laya_mem_util::now_iso()
    } else {
        preferred.to_string()
    }
}

// ─── unit tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(id: &str, content: &str, ts: &str) -> Turn {
        Turn { memory_id: id.to_string(), content: content.to_string(), ts: ts.to_string() }
    }

    #[test]
    fn topic_marker_triggers_boundary() {
        let mut seg = EpisodeSegmenter::new(10, 2);
        // Two related turns fill the buffer; the third carries the marker.
        assert!(seg.process_turn(turn("1", "I went to the store", "2026-10-01T10:00:00")).is_none());
        assert!(seg.process_turn(turn("2", "They had a big sale", "2026-10-01T10:03:00")).is_none());
        let ep = seg.process_turn(turn("3", "By the way, my car broke down", "2026-10-01T10:05:00"));
        assert!(ep.is_some(), "topic marker 'by the way' must flush");
        let ep = ep.unwrap();
        assert_eq!(ep.boundary_reason, "explicit topic marker");
        assert_eq!(ep.turn_ids, vec!["1", "2"], "episode holds the buffered turns");
        assert_eq!(seg.len(), 1, "the triggering turn stays in the buffer");
    }

    #[test]
    fn calendar_day_change_triggers_boundary() {
        let mut seg = EpisodeSegmenter::new(10, 2);
        assert!(seg.process_turn(turn("1", "Morning coffee ritual", "2026-10-01T08:00:00")).is_none());
        assert!(seg.process_turn(turn("2", "Then a long meeting", "2026-10-01T09:00:00")).is_none());
        let ep = seg.process_turn(turn("3", "Evening walk around the block", "2026-10-02T20:00:00"));
        assert!(ep.is_some(), "day change must flush");
        assert_eq!(ep.unwrap().boundary_reason, "calendar-day change");
    }

    #[test]
    fn zero_overlap_triggers_boundary() {
        let mut seg = EpisodeSegmenter::new(10, 2);
        assert!(seg.process_turn(turn("1", "quantum error correction research", "2026-10-01T10:00:00")).is_none());
        assert!(seg.process_turn(turn("2", "qubit fidelity benchmarks improved", "2026-10-01T10:01:00")).is_none());
        let ep = seg.process_turn(turn("3", "buying groceries at the supermarket", "2026-10-01T10:02:00"));
        assert!(ep.is_some(), "no token overlap must flush");
        assert_eq!(ep.unwrap().boundary_reason, "no token or dense overlap with previous turn");
    }

    #[test]
    fn related_turns_do_not_trigger_boundary() {
        let mut seg = EpisodeSegmenter::new(10, 2);
        assert!(seg.process_turn(turn("1", "User bought a new car last week", "2026-10-01T10:00:00")).is_none());
        // Continues the topic, no marker → no boundary.
        assert!(seg.process_turn(turn("2", "User purchased a new vehicle recently", "2026-10-01T10:01:00")).is_none());
        assert_eq!(seg.len(), 2, "no boundary: related turns stay buffered");
    }

    #[test]
    fn max_turns_forces_boundary() {
        let mut seg = EpisodeSegmenter::new(3, 2);
        for i in 1..=3 {
            let _ = seg.process_turn(turn(&i.to_string(), "same topic continues here", "2026-10-01T10:00:00"));
        }
        // 4th turn should trigger a max-buffer flush of turns 1-3.
        let ep = seg.process_turn(turn("4", "same topic continues here", "2026-10-01T10:01:00"));
        assert!(ep.is_some());
        assert_eq!(ep.unwrap().boundary_reason, "max buffer size (3) reached");
    }

    #[test]
    fn below_min_turns_does_not_flush() {
        let mut seg = EpisodeSegmenter::new(10, 3);
        assert!(seg.process_turn(turn("1", "Morning coffee", "2026-10-01T08:00:00")).is_none());
        // Day change, but only 1 buffered turn (< min_turns=3) → no episode.
        assert!(seg.process_turn(turn("2", "Evening walk", "2026-10-02T20:00:00")).is_none());
        assert_eq!(seg.len(), 2, "below min_turns: buffer accumulates");
    }

    #[test]
    fn flush_remaining_emits_and_empties() {
        let mut seg = EpisodeSegmenter::new(10, 2);
        let _ = seg.process_turn(turn("1", "first", "2026-10-01T10:00:00"));
        let _ = seg.process_turn(turn("2", "second", "2026-10-01T10:01:00"));
        let ep = seg.flush_remaining("end of conversation");
        assert!(ep.is_some());
        let ep = ep.unwrap();
        assert_eq!(ep.turn_ids, vec!["1", "2"]);
        assert_eq!(ep.boundary_reason, "end of conversation");
        assert!(seg.is_empty());
    }

    #[test]
    fn episode_summary_contains_both_ends() {
        let turns = vec![
            turn("1", "Started with quantum error correction research.", "2026-10-01T10:00:00"),
            turn("2", "Ended with buying groceries.", "2026-10-01T10:01:00"),
        ];
        let (title, content, hint) = summarize_episode(&turns);
        assert_eq!(title, "Started with quantum error correction research");
        assert!(content.contains("2 turns"));
        assert!(content.contains("Started with"));
        assert!(content.contains("Ended with"));
        assert!(hint.is_none());
    }

    #[test]
    fn persist_episode_row_round_trips() {
        let d = std::env::temp_dir().join(format!("laya-mem-ep-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let db = d.join("ep.sqlite");
        let _ = std::fs::remove_file(&db);
        crate::laya_mem_util::ensure_schema(&db).unwrap();
        // Seed a source memory so source_memory_ids refers to a real row.
        let sql = format!(
            "INSERT INTO memories (content, ts, entities, type_scores, node_type) VALUES ('{}', '', '[]', '{{}}', 'OBSERVATION')",
            crate::laya_mem_util::sql_escape("source observation")
        );
        crate::laya_mem_util::call_db_for_db(&db, json!({"op":"exec","statements":[sql]})).unwrap();
        let ep = Episode {
            title: "Test".to_string(),
            content: "Episode covering 2 turns: first … second".to_string(),
            participant_hint: Some("alice".to_string()),
            start_ts: "2026-10-01T10:00:00".to_string(),
            end_ts: "2026-10-01T10:01:00".to_string(),
            turn_ids: vec!["1".to_string()],
            boundary_reason: "test".to_string(),
        };
        let id = persist_episode_row(&db, &ep).unwrap();
        assert!(id > 0);
        let row = crate::laya_mem_util::fetch_memory_row(&db, &id.to_string()).unwrap();
        assert_eq!(row.get("node_type").and_then(|v| v.as_str()), Some("EPISODE"));
        // Vector was also indexed.
        let vecs = crate::laya_mem_vec::load_vectors(&db).unwrap();
        assert!(vecs.iter().any(|(vid, _, _)| vid == &id.to_string()));
    }
}
