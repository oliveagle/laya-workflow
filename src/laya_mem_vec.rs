//! Phase-4 semantic retrieval for laya-mem: a lightweight vector index stored
//! alongside the SQLite memories table, with a Jev-Mem-style hybrid recall.
//!
//! Jev-Mem splits its retrieval into two layers — a keyword/BM25 path and a
//! dense-vector path — then fuses them with Reciprocal Rank Fusion
//! (`query_engine._rrf_fusion`, k=60). We mirror that with pure Rust and the
//! SQLite we already own:
//!
//! * encoder — [`encode_mock`] hashes every alphanumeric token into a fixed
//!   128-dim bag of float32 (the `MockEncoder` reference); [`encode_openai`]
//!   POSTs to any OpenAI-compatible `/embeddings` endpoint when the caller
//!   points `LAYA_MEM_EMBEDDING_URL` at one. The mock is deterministic, needs
//!   no network, and is the default.
//! * store — a `memory_vectors` SQLite table keyed by memory id, one BLOB per
//!   row. Vectors round-trip as hex through the sqlite3 CLI we already shell
//!   out to, so no driver dependency is added.
//! * search — brute-force cosine over every stored vector. Lay-mem's per-DB
//!   row count is tiny (agent conversations, not corpus search), so the
//!   O(n·dim) scan is faster than a FAISS index build and simpler to audit.
//! * fusion — [`rrf_fuse`] implements the same `Σ 1/(k + rank)` formula.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::Path;

use crate::laya_mem_util::call_db_for_db;

/// Every mock vector is 128 floats, matching Jev-Mem's `MockEncoder.dimension`.
pub const MOCK_DIM: usize = 128;
/// Jev-Mem uses k=60 as the RRF constant (`query_engine.py`, "empirically
/// optimal (Cormack et al., 2009)"); we keep the same default so a fused rank
/// can be compared against Jev-Mem's published retrieval numbers.
pub const RRF_K: f64 = 60.0;

// ─── encoding ─────────────────────────────────────────────────────────────

/// Hash one alphanumeric token to a slot in the bag. Mirrors `MockEncoder`:
/// `sha256(token)[:4] as big-endian % dimension`. We do not link a SHA-2 crate
/// for this; FNV-1a has equivalent slot-distribution for retrieval purposes
/// and is already used for `consolidation_key`.
fn token_slot(token: &str, dim: usize) -> usize {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in token.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h % dim as u64) as usize
}

/// Deterministic 128-dim hashed bag-of-words — the offline default encoder.
/// Same spirit as Jev-Mem's `MockEncoder`: good enough for lexical overlap,
/// never claimed to be semantic.
pub fn encode_mock(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0.0_f32; dim];
    let mut tok = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            tok.push(c);
        } else if !tok.is_empty() {
            let slot = token_slot(&tok, dim);
            v[slot] += 1.0;
            tok.clear();
        }
    }
    if !tok.is_empty() {
        let slot = token_slot(&tok, dim);
        v[slot] += 1.0;
    }
    v
}

/// Encode via an OpenAI-compatible `/embeddings` endpoint.
/// Returns `(vectors, dim)` for the input texts, one row each.
pub fn encode_openai(url: &str, api_key: &str, model: &str, texts: &[&str]) -> Result<(Vec<Vec<f32>>, usize)> {
    #[derive(serde::Deserialize)]
    struct EmbeddingResp {
        data: Vec<EmbeddingItem>,
    }
    #[derive(serde::Deserialize)]
    struct EmbeddingItem {
        embedding: Vec<f32>,
    }
    let body = json!({ "model": model, "input": texts });
    let mut req = ureq::post(url)
        .timeout(std::time::Duration::from_secs(30))
        .set("Content-Type", "application/json");
    if !api_key.is_empty() {
        req = req.set("Authorization", &format!("Bearer {api_key}"));
    }
    let resp = req
        .send_json(&body)
        .with_context(|| format!("POST {url}"))?;
    let parsed: EmbeddingResp = resp
        .into_json()
        .context("parsing /embeddings response")?;
    let dim = parsed
        .data
        .first()
        .map(|d| d.embedding.len())
        .ok_or_else(|| anyhow::anyhow!("empty embeddings response"))?;
    Ok((parsed.data.into_iter().map(|d| d.embedding).collect(), dim))
}

// ─── store ────────────────────────────────────────────────────────────────

/// Create the `memory_vectors` table if missing. Kept separate from
/// [`crate::laya_mem_util::ensure_schema`] because vectors are optional
/// scaffolding: an old DB without them should not fail to open.
fn ensure_vector_schema(db_path: &Path) -> Result<()> {
    call_db_for_db(
        db_path,
        json!({
            "op": "exec",
            "statements": [
                "CREATE TABLE IF NOT EXISTS memory_vectors (memory_id TEXT PRIMARY KEY, dim INTEGER NOT NULL, vector BLOB NOT NULL)"
            ],
        }),
    )?;
    Ok(())
}

fn u32_to_hex(x: u32) -> String {
    // little-endian hex, 8 chars per float — matches `X'...'` literals.
    let bytes = x.to_le_bytes();
    let mut s = String::with_capacity(8);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_to_bytes(s: &str) -> Vec<u8> {
    let clean: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let mut out = Vec::with_capacity(clean.len() / 2);
    let b = clean.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        let hi = (b[i] as char).to_digit(16).unwrap_or(0) as u8;
        let lo = (b[i + 1] as char).to_digit(16).unwrap_or(0) as u8;
        out.push((hi << 4) | lo);
        i += 2;
    }
    out
}

fn bytes_to_f32s(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Persist one vector for a memory (upsert — re-persisting replaces).
pub fn upsert_vector(db_path: &Path, memory_id: &str, vector: &[f32]) -> Result<()> {
    ensure_vector_schema(db_path)?;
    let dim = vector.len();
    let mut hex = String::with_capacity(dim * 8);
    for f in vector {
        hex.push_str(&u32_to_hex(f.to_bits()));
    }
    let sql = format!(
        "INSERT INTO memory_vectors (memory_id, dim, vector) VALUES ('{}', {dim}, X'{hex}') \
         ON CONFLICT(memory_id) DO UPDATE SET dim = excluded.dim, vector = excluded.vector",
        crate::laya_mem_util::sql_escape(memory_id),
    );
    call_db_for_db(
        db_path,
        json!({ "op": "exec", "statements": [sql] }),
    )?;
    Ok(())
}

/// Fetch all (memory_id, dim, vector) triples. Small databases mean this is
/// fine; if it ever becomes a bottleneck the right fix is a page-in index,
/// not a new dependency.
pub fn load_vectors(db_path: &Path) -> Result<Vec<(String, usize, Vec<f32>)>> {
    ensure_vector_schema(db_path)?;
    let res = call_db_for_db(
        db_path,
        json!({ "op": "query", "sql": "SELECT memory_id, dim, hex(vector) AS v FROM memory_vectors" }),
    )?;
    let mut out = Vec::new();
    if let Some(rows) = res.get("rows").and_then(|v| v.as_array()) {
        for row in rows {
            let id = row
                .get("memory_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let dim = row
                .get("dim")
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as usize;
            let v = row
                .get("v")
                .and_then(|v| v.as_str())
                .map(hex_to_bytes)
                .unwrap_or_default();
            let f32s = bytes_to_f32s(&v);
            if f32s.len() == dim {
                out.push((id, dim, f32s));
            }
        }
    }
    Ok(out)
}

// ─── similarity + fusion ─────────────────────────────────────────────────

/// Cosine similarity; zero-norm vectors score 0.0 rather than NaN.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0_f32, 0.0_f32, 0.0_f32);
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom <= 1e-12 {
        0.0
    } else {
        dot / denom
    }
}

/// Brute-force top-k nearest by cosine. Ties broken by ascending memory_id so
/// the output order is deterministic even with equal scores.
pub fn search_vectors(
    db_path: &Path,
    query: &[f32],
    top_k: usize,
    exclude: Option<&str>,
) -> Result<Vec<(String, f32)>> {
    let all = load_vectors(db_path)?;
    let mut scored: Vec<(String, f32)> = all
        .iter()
        .filter(|(id, dim, _)| Some(id.as_str()) != exclude && *dim == query.len())
        .map(|(id, _, v)| (id.clone(), cosine(query, v)))
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scored.truncate(top_k.max(1));
    Ok(scored)
}

/// Reciprocal Rank Fusion, matching Jev-Mem `query_engine._rrf_fusion`.
/// `lists` is a set of pre-ranked candidate id lists, best-first. Returns
/// `(id, rrf_score)` sorted descending, ties broken by id.
pub fn rrf_fuse(lists: &[Vec<String>], k: f64) -> Vec<(String, f64)> {
    let mut scores: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    for list in lists {
        for (rank, id) in list.iter().enumerate() {
            let entry = scores.entry(id.clone()).or_insert(0.0);
            *entry += 1.0 / (k + rank as f64);
        }
    }
    let mut out: Vec<(String, f64)> = scores.into_iter().collect();
    out.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    out
}


/// Hybrid candidate discovery for consolidation: encode `query`, run the FTS5
/// BM25 query `fts_sql` (must return an `id` column), also scan the dense
/// vector index, fuse with RRF, and return the top-k ids (best first).
/// `exclude_id` is dropped from both streams before fusion.
pub fn hybrid_top_ids(db_path: &Path, fts_sql: &str, query: &str, exclude_id: &str, top_k: usize) -> Result<Vec<String>> {
    let dim = MOCK_DIM;
    let qvec = match std::env::var("LAYA_MEM_EMBEDDING_URL").ok().filter(|s| !s.is_empty()) {
        Some(url) => {
            let model = std::env::var("LAYA_MEM_EMBEDDING_MODEL")
                .unwrap_or_else(|_| "text-embedding-3-small".to_string());
            let key = std::env::var("LAYA_MEM_EMBEDDING_API_KEY").unwrap_or_default();
            match encode_openai(&url, &key, &model, &[query]) {
                Ok((mut vs, _)) if !vs.is_empty() => vs.remove(0),
                _ => encode_mock(query, dim),
            }
        }
        None => encode_mock(query, dim),
    };
    // BM25 stream
    let fts_res = call_db_for_db(db_path, json!({ "op": "query", "sql": fts_sql }))?;
    let fts_ids: Vec<String> = fts_res
        .get("rows")
        .and_then(|r| r.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    row.get("id").map(|v| match v {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        _ => String::new(),
                    }).filter(|s| !s.is_empty())
                })
                .filter(|s| s != exclude_id)
                .collect()
        })
        .unwrap_or_default();
    // Dense stream
    let dense = search_vectors(db_path, &qvec, top_k * 2, Some(exclude_id))?;
    let dense_ids: Vec<String> = dense.into_iter().map(|(id, _)| id).collect();
    let fused = rrf_fuse(&[fts_ids, dense_ids], RRF_K);
    Ok(fused.into_iter().take(top_k.max(1)).map(|(id, _)| id).collect())
}

// ─── unit tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::laya_mem_util::sql_escape;
    use std::path::PathBuf;

    fn tmpdb(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "laya-mem-vec-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::create_dir_all(&d);
        let p = d.join(format!("{tag}.sqlite"));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn mock_encoder_is_deterministic_and_slot_hashed() {
        let a = encode_mock("business administration degree", MOCK_DIM);
        let b = encode_mock("business administration degree", MOCK_DIM);
        assert_eq!(a.len(), MOCK_DIM);
        assert_eq!(a, b, "same text must encode identically");
        // a token always lands in the same slot
        let one = encode_mock("zebra", MOCK_DIM);
        assert_eq!(one.iter().sum::<f32>(), 1.0, "one token = one hit");
        assert!(one.iter().any(|&x| x == 1.0));
    }

    #[test]
    fn cosine_zero_vector_is_zero_not_nan() {
        let z = vec![0.0_f32; 4];
        assert_eq!(cosine(&z, &[1.0, 0.0, 0.0, 0.0]), 0.0);
        assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0]), 1.0);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
    }

    #[test]
    fn upsert_then_search_round_trips_through_sqlite() {
        let db = tmpdb("upsert-search");
        crate::laya_mem_util::ensure_schema(&db).unwrap();
        // Seed two memories so foreign-key-style queries still return rows.
        let sql = format!(
            "INSERT INTO memories (content, ts, entities, type_scores, node_type) VALUES ('{}', '', '[]', '{{}}', 'OBSERVATION')",
            sql_escape("alpha document")
        );
        call_db_for_db(&db, json!({ "op": "exec", "statements": [sql] })).unwrap();
        let sql2 = format!(
            "INSERT INTO memories (content, ts, entities, type_scores, node_type) VALUES ('{}', '', '[]', '{{}}', 'OBSERVATION')",
            sql_escape("beta document")
        );
        call_db_for_db(&db, json!({ "op": "exec", "statements": [sql2] })).unwrap();

        let va = encode_mock("alpha document", MOCK_DIM);
        let vb = encode_mock("beta document", MOCK_DIM);
        let vq = encode_mock("alpha document", MOCK_DIM);
        upsert_vector(&db, "1", &va).unwrap();
        upsert_vector(&db, "2", &vb).unwrap();

        let hits = search_vectors(&db, &vq, 2, None).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "1", "nearest to 'alpha document' should be id 1");
        assert!(hits[0].1 > 0.99);

        // Upsert replaces, does not duplicate.
        let vq2 = encode_mock("beta document", MOCK_DIM);
        upsert_vector(&db, "1", &vq2).unwrap();
        let hits2 = search_vectors(&db, &vq2, 2, None).unwrap();
        assert_eq!(hits2[0].0, "1");
        let (one, _) = hits2
            .iter()
            .find(|(id, _)| id == "1")
            .cloned()
            .unwrap();
        let _ = one;
    }

    #[test]
    fn rrf_fusion_matches_jev_mem_formula() {
        // Jev-Mem example from the docstring: two ranked lists.
        let l1 = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let l2 = vec!["b".to_string(), "a".to_string()];
        let fused = rrf_fuse(&[l1, l2], 60.0);
        // a: 1/61 + 1/62; b: 1/62 + 1/61 → tie → id asc → "a" first
        let expected = 1.0 / (60.0 + 0.0) + 1.0 / (60.0 + 1.0);
        assert!((fused[0].1 - expected).abs() < 1e-9, "got {}", fused[0].1);
        assert!((fused[1].1 - expected).abs() < 1e-9);
        // c only appears in list 1 at rank 2: 1/63 — strictly lower
        let c = fused.iter().find(|(id, _)| id == "c").unwrap();
        assert!(c.1 < expected);
    }

    #[test]
    fn exclude_id_is_honoured_in_search() {
        let db = tmpdb("exclude");
        crate::laya_mem_util::ensure_schema(&db).unwrap();
        let v = encode_mock("same text", MOCK_DIM);
        upsert_vector(&db, "1", &v).unwrap();
        upsert_vector(&db, "2", &v).unwrap();
        let hits = search_vectors(&db, &v, 2, Some("1")).unwrap();
        assert_eq!(hits.len(), 1, "excluding id=1 must drop it");
        assert_eq!(hits[0].0, "2");
    }

    #[test]
    fn hex_round_trip() {
        let b = vec![0xde, 0xad, 0xbe, 0xef];
        let h = b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(hex_to_bytes(&h), b);
        let f = 3.14159_f32;
        let fs = u32_to_hex(f.to_bits());
        let recovered = f32::from_bits(u32::from_le_bytes(
            hex_to_bytes(&fs).try_into().unwrap(),
        ));
        assert_eq!(f, recovered);
    }
}
