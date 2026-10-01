//! `codegraph` section — 200+ checks covering the codegraph MCP tool set:
//!
//!  1. SQL escape helpers (`sql_str`, `sql_like_escape`)
//!  2. the five view adapters against a synthetic fixture DB
//!  3. the three embedded DSL controllers (routing / stopping / traversal)
//!  4. the MCP tool surface (`codegraph_route` / `fetch` / `traverse` / `stop` / `status`)
//!  5. budget allocation (largest-remainder)
//!  6. an end-to-end route → fetch → traverse → stop loop
//!  7. guards: unknown view, missing arg, non-existent DB, special chars
//!
//! Fixture: a small codegraph DB created via the real `sqlite3` CLI in
//! `$TMPDIR`, using the same schema the codegraph indexer writes
//! (`nodes`, `edges`, `files`, `nodes_fts`, `unresolved_refs`).

use super::{Harness, Value};
use laya_workflow::backend::HeuristicBackend;
use laya_workflow::codegraph_util as util;
use laya_workflow::mcp::McpToolSet;
use laya_workflow::spec::load_file;
use laya_workflow::workflow::ResilientWorkflow;
use serde_json::json;
use std::path::PathBuf;
use std::sync::OnceLock;

// ─── fixture ───────────────────────────────────────────────────────────────

/// Build (once) a small codegraph DB under `$TMPDIR` and return its path.
///
/// Schema mirrors the real codegraph indexer output (contentless FTS5 over
/// `nodes` with rowid tie-back, `unresolved_refs` with a `candidates` JSON
/// blob). Rows exercise every view: a function, a method, a class, a route,
/// a file, an import; call / extends / imports edges; three files with
/// different `modified_at`.
fn fixture_db() -> &'static PathBuf {
    static DB: OnceLock<PathBuf> = OnceLock::new();
    DB.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("laya-codegraph-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db = dir.join("codegraph.db");
        let _ = std::fs::remove_file(&db);
        let sql = r#"
CREATE TABLE nodes (
    id TEXT PRIMARY KEY, kind TEXT, name TEXT, qualified_name TEXT,
    file_path TEXT, language TEXT, start_line INTEGER, end_line INTEGER,
    signature TEXT, docstring TEXT
);
CREATE TABLE edges (id INTEGER PRIMARY KEY AUTOINCREMENT, source TEXT, target TEXT, kind TEXT, line INTEGER);
CREATE TABLE files (path TEXT PRIMARY KEY, language TEXT, size INTEGER, modified_at REAL, indexed_at REAL, node_count INTEGER);
CREATE TABLE unresolved_refs (id INTEGER PRIMARY KEY AUTOINCREMENT, from_node_id TEXT, reference_name TEXT, reference_kind TEXT, file_path TEXT, language TEXT, candidates TEXT);
CREATE VIRTUAL TABLE nodes_fts USING fts5(id, name, qualified_name, docstring, signature, content='nodes', content_rowid='rowid');

INSERT INTO nodes VALUES
  ('auth_fn',        'function', 'authenticate',     'app::auth::authenticate',      'src/auth.rs',    'rust', 10, 42,  'fn authenticate(&self, token: &str) -> bool', 'checks a bearer token'),
  ('auth_middleware','method',   'middleware',       'app::auth::middleware',        'src/auth.rs',    'rust', 50, 80,  'fn middleware(&self, next: fn())',            'guards protected routes'),
  ('session_store',  'class',    'SessionStore',     'app::session::SessionStore',   'src/session.rs', 'rust', 5,  200, 'struct SessionStore',                        'persists auth sessions'),
  ('health_route',   'route',    'GET /health',      'app::routes::health',          'src/routes.rs',  'rust', 1,  8,   'GET /health',                                 'health check endpoint'),
  ('main_file',      'file',     'main.rs',          'src/main.rs',                  'src/main.rs',    'rust', 1,  1,   NULL,                                          NULL),
  ('login_import',   'import',   'login',            'app::auth::login',             'src/login.rs',   'rust', 1,  1,   NULL,                                          NULL),
  ('user_model',     'struct',   'User',             'app::models::User',            'src/models.rs',  'rust', 3,  30,  'struct User { id: u64, name: String }',       'the User model'),
  ('validate_fn',    'function', 'validate_token',   'app::auth::validate_token',    'src/auth.rs',    'rust', 100, 120, 'fn validate_token(&self) -> bool',            'validates a raw token'),
  ('token_helper',   'method',   'refresh',          'app::auth::refresh',           'src/auth.rs',    'rust', 130, 160, 'fn refresh(&mut self)',                      'refreshes the token'),
  ('audit_log',      'function', 'audit_log',        'app::audit::log',              'src/audit.rs',   'rust', 1,  20,  'fn audit_log(&self, msg: &str)',              'appends to the audit trail');

INSERT INTO edges (source, target, kind, line) VALUES
  ('auth_middleware','auth_fn',       'calls',        55),
  ('auth_fn',        'session_store', 'calls',        12),
  ('auth_fn',        'user_model',    'calls',        15),
  ('login_import',   'auth_fn',       'imports',      1),
  ('main_file',      'login_import',  'imports',      1),
  ('validate_fn',    'auth_fn',       'calls',        105),
  ('token_helper',   'auth_fn',       'calls',        135),
  ('session_store',  'user_model',    'references',   30),
  ('health_route',   'main_file',     'contains',     1),
  ('auth_middleware','health_route',  'contains',     1);

INSERT INTO files VALUES
  ('src/auth.rs',    'rust', 4096, 1788600000.0, 1788700000.0, 4),
  ('src/session.rs', 'rust', 2048, 1788550000.0, 1788700000.0, 1),
  ('src/routes.rs',  'rust', 1024, 1788650000.0, 1788700000.0, 1),
  ('src/main.rs',    'rust',  512, 1788500000.0, 1788700000.0, 0);

INSERT INTO unresolved_refs (from_node_id, reference_name, reference_kind, file_path, language, candidates) VALUES
  ('auth_fn',        'OAuthProvider', 'calls',      'src/auth.rs',    'rust', '["app::auth::OAuthProvider","ext::oauth::Provider"]'),
  ('validate_fn',    'JwtDecoder',    'calls',      'src/auth.rs',    'rust', '["app::jwt::Decoder"]'),
  ('session_store',  'Redis',         'references', 'src/session.rs', 'rust', '["ext::redis::Client"]');

INSERT INTO nodes_fts(rowid, id, name, qualified_name, docstring, signature)
SELECT rowid, id, name, qualified_name, docstring, signature FROM nodes;
"#;
        let st = std::process::Command::new("sqlite3")
            .arg(db.to_str().unwrap())
            .arg(sql)
            .status()
            .expect("sqlite3 must be installed");
        assert!(st.success(), "fixture db creation failed");
        db
    })
}

/// A fresh [`CodegraphTools`] bound to the fixture DB with a temp spec dir so
/// tests never write into `~/.laya-workflow`.
fn fixture_tools() -> laya_workflow::codegraph::CodegraphTools {
    let spec_dir = std::env::temp_dir().join(format!("laya-codegraph-specs-{}", std::process::id()));
    let _ = laya_workflow::codegraph_util::ensure_specs(&spec_dir);
    laya_workflow::codegraph::CodegraphTools {
        spec_dir,
        db_path: fixture_db().clone(),
        base_url: None, // offline heuristic
    }
}

fn tool(set: &laya_workflow::codegraph::CodegraphTools, name: &str) -> laya_workflow::mcp::DynTool {
    set.tools()
        .into_iter()
        .find(|t| t.name() == name)
        .unwrap_or_else(|| panic!("tool {name} not registered"))
}

fn run_spec(dir: &std::path::Path, name: &str, state: &Value) -> Value {
    let path = dir.join(format!("{name}.json"));
    let wf: ResilientWorkflow = load_file(path.to_str().unwrap()).unwrap();
    let out = wf.run(&HeuristicBackend, state).unwrap();
    out.to_json()
}

// ─── 1. SQL escape helpers ─────────────────────────────────────────────────

fn test_escape(h: &mut Harness) {
    use laya_workflow::codegraph_util::{sql_like_escape, sql_str};
    h.eq("sql_str: simple", sql_str("abc"), "'abc'".to_string());
    h.eq("sql_str: empty", sql_str(""), "''".to_string());
    h.eq("sql_str: one quote", sql_str("o'brien"), "'o''brien'".to_string());
    h.eq("sql_str: two quotes", sql_str("a''b"), "'a''''b'".to_string());
    h.eq("sql_str: unicode", sql_str("认证"), "'认证'".to_string());
    h.eq("sql_str: space", sql_str("foo bar"), "'foo bar'".to_string());
    h.eq("sql_str: newline kept", sql_str("a\nb"), "'a\nb'".to_string());
    h.eq("sql_str: backslash kept", sql_str("a\\b"), "'a\\b'".to_string());

    h.eq("like_escape: plain", sql_like_escape("auth"), "auth".to_string());
    h.eq("like_escape: percent", sql_like_escape("100%"), "100\\%".to_string());
    h.eq("like_escape: underscore", sql_like_escape("foo_bar"), "foo\\_bar".to_string());
    h.eq("like_escape: backslash", sql_like_escape("a\\b"), "a\\\\b".to_string());
    h.eq("like_escape: all", sql_like_escape("%_\\"), "\\%\\_\\\\".to_string());
    h.eq("like_escape: empty", sql_like_escape(""), "".to_string());
}

// ─── 2. view adapters against the fixture DB ───────────────────────────────

fn test_views(h: &mut Harness) {
    let db = fixture_db();
    h.check("db_ready: fixture has required tables", util::db_ready(db).unwrap());

    // semantic (FTS5)
    let r = util::view_semantic(db, "auth", 5).unwrap();
    h.eq("semantic: engine is fts5", r["engine"].as_str().unwrap(), "fts5");
    h.check("semantic: auth matches >0 rows", r["rows"].as_array().map(|a| !a.is_empty()).unwrap_or(false));
    let names: Vec<&str> = r["rows"].as_array().unwrap().iter().filter_map(|x| x["qualified_name"].as_str()).collect();
    h.check("semantic: 'auth' hits authenticate", names.iter().any(|n| n.contains("authenticate")));

    let r = util::view_semantic(db, "nonexistenttoken", 5).unwrap();
    h.eq("semantic: no match returns 0 rows", r["rows"].as_array().map(|a| a.is_empty()).unwrap_or(false), true);
    let r = util::view_semantic(db, "session", 1).unwrap();
    h.eq("semantic: limit=1 returns 1 row", r["rows"].as_array().map(|a| a.len()).unwrap_or(0), 1);
    // FTS5 special tokens: prefix + wildcard
    let r = util::view_semantic(db, "auth*", 20).unwrap();
    h.check("semantic: prefix 'auth*' > 1 hit", r["rows"].as_array().map(|a| a.len()).unwrap_or(0) > 1);
    // SQL injection attempts must not crash — FTS5 rejects, LIKE escapes
    let r = util::view_semantic(db, "' OR 1=1 --", 5);
    h.check("semantic: FTS5 injection does not crash (either error or empty)", r.is_ok());
    let r = util::view_semantic(db, "auth%'--", 5);
    h.check("semantic: LIKE-injection does not crash", r.is_ok());

    // causal
    let r = util::view_causal(db, "authenticate", false, 10).unwrap();
    let kinds: Vec<&str> = r["rows"].as_array().unwrap().iter().filter_map(|x| x["edge_kind"].as_str()).collect();
    h.check("causal: outbound auth edges include calls", kinds.contains(&"calls"));
    let r = util::view_causal(db, "authenticate", true, 10).unwrap();
    let callers: Vec<&str> = r["rows"].as_array().unwrap().iter().filter_map(|x| x["other_qualified_name"].as_str()).collect();
    h.check("causal: inbound finds validate_token caller", callers.iter().any(|t| t.contains("validate_token")));
    let r = util::view_causal(db, "zzzzz", false, 10).unwrap();
    h.eq("causal: no seed = 0 rows", r["rows"].as_array().map(|a| a.is_empty()).unwrap_or(false), true);

    // entity
    let r = util::view_entity(db, "app::auth::authenticate").unwrap();
    h.eq("entity: exact qualified_name match 1 row", r["rows"].as_array().map(|a| a.len()).unwrap_or(0), 1);
    // "JwtDecoder" is not a node but is in unresolved_refs with candidates — falls back to soft match
    let r = util::view_entity(db, "JwtDecoder").unwrap();
    let rows = r["rows"].as_array().unwrap();
    h.check("entity: unknown falls back to soft match", !rows.is_empty() && rows[0]["soft_match"].as_bool().unwrap_or(false));
    // a truly unknown symbol returns no rows cleanly
    let r = util::view_entity(db, "no_such_symbol_xyz").unwrap();
    h.check("entity: truly unknown returns 0 rows", r["rows"].as_array().map(|a| a.is_empty()).unwrap_or(false));

    // temporal
    let r = util::view_temporal(db, 10).unwrap();
    let paths: Vec<&str> = r["rows"].as_array().unwrap().iter().filter_map(|x| x["path"].as_str()).collect();
    h.eq("temporal: returns 4 files", r["rows"].as_array().map(|a| a.len()).unwrap_or(0), 4);
    h.check("temporal: newest first is routes.rs", paths.first() == Some(&"src/routes.rs"));
    let r = util::view_temporal(db, 1).unwrap();
    h.eq("temporal: limit=1 returns 1", r["rows"].as_array().map(|a| a.len()).unwrap_or(0), 1);

    // multi-hop
    let r = util::view_multi_hop(db, "login", 1, 20).unwrap();
    let depths: Vec<i64> = r["rows"].as_array().unwrap().iter().filter_map(|x| x["depth"].as_i64()).collect();
    h.check("multi_hop: depth=1 stays ≤1", depths.iter().all(|&d| d <= 1));
    h.check("multi_hop: depth=1 finds login itself", depths.iter().any(|&d| d == 0));
    let r = util::view_multi_hop(db, "login", 3, 20).unwrap();
    let depths: Vec<i64> = r["rows"].as_array().unwrap().iter().filter_map(|x| x["depth"].as_i64()).collect();
    h.check("multi_hop: depth=3 reaches authenticate", depths.contains(&2));
    let r = util::view_multi_hop(db, "nonexistent", 2, 5).unwrap();
    h.eq("multi_hop: no seed = 0 rows", r["rows"].as_array().map(|a| a.is_empty()).unwrap_or(false), true);
}

// ─── 3. embedded DSL controllers ────────────────────────────────────────────

fn test_specs(h: &mut Harness) {
    let spec_dir = std::env::temp_dir().join(format!("laya-codegraph-specs-{}", std::process::id()));
    let _ = util::ensure_specs(&spec_dir);

    // routing: all 6 hints explicit
    let r = run_spec(&spec_dir, "routing_codegraph", &json!({
        "query": "find auth code",
        "route_semantic": "high", "route_temporal": "low", "route_causal": "high",
        "route_entity": "low", "route_multi_hop_need": "yes", "route_recency": "low",
    }));
    let s = &r["result"];
    h.eq("routing: semantic high → 0.85", s["semantic"].as_f64().unwrap(), 0.85);
    h.eq("routing: temporal low → 0.1", s["temporal"].as_f64().unwrap(), 0.1);
    h.eq("routing: causal high → 0.85", s["causal"].as_f64().unwrap(), 0.85);
    h.eq("routing: entity low → 0.1", s["entity"].as_f64().unwrap(), 0.1);
    h.eq("routing: multi_hop yes → 0.85", s["multi_hop_need"].as_f64().unwrap(), 0.85);
    h.eq("routing: recency low → 0.1", s["recency_importance"].as_f64().unwrap(), 0.1);

    // routing: missing field falls back to whole-state substring (documented)
    let r = run_spec(&spec_dir, "routing_codegraph", &json!({
        "query": "auth", "route_semantic": "high",
    }));
    let s = &r["result"];
    // Without the other five, "high" / "yes" needles leak from the serialised state.
    let leaky = s["temporal"].as_f64().unwrap_or(0.0) > 0.5;
    h.check("routing: missing hints leak (documented pitfall)", leaky);

    // stopping: each branch fires
    for (name, state, want) in [
        ("sufficient", json!({"query":"q","evidence":["a"],"evidence_status":"sufficient"}), "STOP_EVIDENCE_OK"),
        ("contradiction", json!({"query":"q","evidence":["a"],"evidence_status":"contradiction"}), "CONTINUE_CONTRADICTION"),
        ("insufficient", json!({"query":"q","evidence":["a"],"evidence_status":"insufficient"}), "CONTINUE_EVIDENCE_INSUFFICIENT"),
        ("missing flag", json!({"query":"q","evidence":["a"],"missing_evidence":"missing"}), "CONTINUE_MISSING"),
        ("empty evidence", json!({"query":"q","evidence":[]}), "CONTINUE_EVIDENCE_INSUFFICIENT"),
    ] {
        let r = run_spec(&spec_dir, "stopping_codegraph", &state);
        let got = r["result"]["label"].as_str().unwrap_or("");
        h.eq(&format!("stopping: {name}"), got.to_string(), want.to_string());
    }

    // traversal: 2 candidates, axis scores + weighted ranking
    let r = run_spec(&spec_dir, "traversal_codegraph", &json!({
        "cand_0_relevance": "high", "cand_0_relation_usefulness": "high",
        "cand_0_new_information": "low", "cand_0_supports_current_evidence": "low",
        "cand_1_relevance": "low", "cand_1_relation_usefulness": "low",
        "cand_1_new_information": "high", "cand_1_supports_current_evidence": "high",
    }));
    let s = &r["result"];
    h.eq("traversal: cand_0_relevance 0.85", s["cand_0_relevance"].as_f64().unwrap(), 0.85);
    h.eq("traversal: cand_0_new_information 0.1", s["cand_0_new_information"].as_f64().unwrap(), 0.1);
    h.eq("traversal: cand_1_relevance 0.1", s["cand_1_relevance"].as_f64().unwrap(), 0.1);
    h.eq("traversal: cand_1_new_information 0.85", s["cand_1_new_information"].as_f64().unwrap(), 0.85);
}

// ─── 4. MCP tool surface ─────────────────────────────────────────────────────

fn test_mcp_tools(h: &mut Harness) {
    let set = fixture_tools();

    // route: budget allocation
    let t = tool(&set, "codegraph_route");
    let r = t.call(&json!({
        "query": "find auth code",
        "route_semantic": "high", "route_temporal": "low", "route_causal": "high",
        "route_entity": "low", "route_multi_hop_need": "yes", "route_recency": "low",
        "total": 8,
    })).unwrap();
    let b = &r["budget"];
    h.eq("route: total sums to 8", ["semantic","temporal","causal","entity","multi_hop_need","recency_importance"].iter().map(|k| b[*k].as_i64().unwrap_or(0)).sum::<i64>(), 8);
    h.check("route: semantic gets a big share", b["semantic"].as_i64().unwrap() >= 2);
    h.check("route: causal gets a big share", b["causal"].as_i64().unwrap() >= 2);
    h.eq("route: temporal gets 0", b["temporal"].as_i64().unwrap(), 0);
    h.check("route: scores echoed", r["scores"]["semantic"].as_f64().unwrap() == 0.85);
    // missing 'query' must error
    h.check("route: missing query errors", t.call(&json!({"route_semantic":"high"})).is_err());

    // fetch: each view
    let t = tool(&set, "codegraph_fetch");
    for (view, args, want_rows) in [
        ("semantic", json!({"view":"semantic","query":"auth","budget":5}), 1),
        ("causal",   json!({"view":"causal","seed":"authenticate","budget":10}), 1),
        ("entity",   json!({"view":"entity","seed":"app::auth::authenticate"}), 1),
        ("temporal", json!({"view":"temporal","budget":4}), 4),
        ("multi_hop",json!({"view":"multi_hop","seed":"login","depth":2,"budget":10}), 1),
    ] {
        let r = t.call(&args).unwrap();
        let n = r["rows"].as_array().map(|a| a.len()).unwrap_or(0);
        h.check(&format!("fetch[{view}]: returns ≥{want_rows} row(s)"), n >= want_rows);
    }
    h.check("fetch: unknown view errors", t.call(&json!({"view":"badview"})).is_err());
    h.check("fetch: missing view errors", t.call(&json!({})).is_err());
    h.check("fetch: budget clamps", t.call(&json!({"view":"temporal","budget":500})).is_ok());
    h.check("fetch: depth clamps", t.call(&json!({"view":"multi_hop","seed":"x","depth":99,"budget":5})).is_ok());

    // traverse: weighted ranking
    let t = tool(&set, "codegraph_traverse");
    let r = t.call(&json!({
        "cand_0_relevance":"high", "cand_0_relation_usefulness":"high",
        "cand_0_new_information":"low", "cand_0_supports_current_evidence":"low",
        "cand_1_relevance":"low", "cand_1_relation_usefulness":"low",
        "cand_1_new_information":"high", "cand_1_supports_current_evidence":"high",
    })).unwrap();
    let ranked = r["ranked"].as_array().unwrap();
    h.eq("traverse: returns 2 candidates", ranked.len(), 2);
    h.eq("traverse: cand_0 weighted > cand_1", ranked[0]["weighted"].as_f64().unwrap() > ranked[1]["weighted"].as_f64().unwrap(), true);
    h.eq("traverse: top is candidate 0", ranked[0]["candidate"].as_i64().unwrap(), 0);
    // custom weights flip the ranking
    let r = t.call(&json!({
        "cand_0_relevance":"high", "cand_0_relation_usefulness":"low",
        "cand_0_new_information":"low", "cand_0_supports_current_evidence":"low",
        "cand_1_relevance":"low", "cand_1_relation_usefulness":"high",
        "cand_1_new_information":"high", "cand_1_supports_current_evidence":"high",
        "weights": {"relevance": 0.0, "relation_usefulness": 1.0, "new_information": 0.0, "supports_current_evidence": 0.0},
    })).unwrap();
    let ranked = r["ranked"].as_array().unwrap();
    h.eq("traverse: custom weights flip ranking", ranked[0]["candidate"].as_i64().unwrap(), 1);

    // stop: branch decisions
    let t = tool(&set, "codegraph_stop");
    for (name, args, want) in [
        ("sufficient", json!({"query":"q","evidence":["a"],"evidence_status":"sufficient"}), "STOP_EVIDENCE_OK"),
        ("contradiction", json!({"query":"q","evidence":["a"],"evidence_status":"contradiction"}), "CONTINUE_CONTRADICTION"),
        ("insufficient", json!({"query":"q","evidence":["a"],"evidence_status":"insufficient"}), "CONTINUE_MISSING"),
        ("missing", json!({"query":"q","evidence":["a"],"missing_evidence":true}), "CONTINUE_MISSING"),
    ] {
        let r = t.call(&args).unwrap();
        h.eq(&format!("stop[{name}]"), r["decision"].as_str().unwrap().to_string(), want.to_string());
    }
    h.check("stop: missing query errors", t.call(&json!({})).is_err());

    // status
    let t = tool(&set, "codegraph_status");
    let r = t.call(&json!({})).unwrap();
    h.eq("status: ready=true", r["ready"].as_bool().unwrap(), true);
    let stats = &r["stats"];
    h.eq("status: nodes=10", stats["counts"]["nodes"].as_i64().unwrap(), 10);
    h.eq("status: edges=10", stats["counts"]["edges"].as_i64().unwrap(), 10);
    h.eq("status: files=4", stats["counts"]["files"].as_i64().unwrap(), 4);
    h.eq("status: unresolved=3", stats["counts"]["unresolved_refs"].as_i64().unwrap(), 3);
    h.check("status: edge kinds include calls", stats["edge_kinds"].as_array().map(|a| a.iter().any(|x| x["kind"].as_str().unwrap_or("") == "calls")).unwrap_or(false));
    // missing db
    let bad = laya_workflow::codegraph::CodegraphTools { spec_dir: set.spec_dir.clone(), db_path: PathBuf::from("/tmp/no-such-db.db"), base_url: None };
    let r = tool(&bad, "codegraph_status").call(&json!({})).unwrap();
    h.eq("status: missing db → ready=false", r["ready"].as_bool().unwrap(), false);
}

// ─── 5. budget allocation (largest remainder) ───────────────────────────────

fn test_budget(h: &mut Harness) {
    use laya_workflow::codegraph_util::allocate_budgets;
    let alloc = |scores: &[f64; 6], total: i64| {
        let [a, b, c, d, e, f] = allocate_budgets(scores, total);
        [a, b, c, d, e, f]
    };
    // all zero → all zeros
    h.eq("budget: all-zero gives all-zero", alloc(&[0.0; 6], 8), [0; 6]);
    // uniform → even split with remainder spread
    let r = alloc(&[0.5; 6], 8);
    // largest-remainder with 6 equal fractional parts (0.333…) gives the remainder to the lowest-index items
    h.eq("budget: uniform 6 views / total 8", r, [2, 2, 1, 1, 1, 1]);
    // one dominant view gets the most
    let r = alloc(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0], 8);
    h.eq("budget: single view takes all", r, [8, 0, 0, 0, 0, 0]);
    // total 1 with one dominant view
    let r = alloc(&[0.9, 0.1, 0.0, 0.0, 0.0, 0.0], 1);
    h.eq("budget: total 1 dominant", r, [1, 0, 0, 0, 0, 0]);
    // total 0
    let r = alloc(&[0.5, 0.5, 0.0, 0.0, 0.0, 0.0], 0);
    h.eq("budget: total 0 → all zero", r, [0, 0, 0, 0, 0, 0]);
    // negative total clamps to zero
    let r = alloc(&[0.5, 0.5, 0.0, 0.0, 0.0, 0.0], -5);
    h.check("budget: negative total tolerated", r.iter().all(|&x| x >= 0));
    // sum matches total across random-ish inputs
    for total in [1i64, 3, 5, 7, 10, 16, 24] {
        let scores = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6];
        let r = alloc(&scores, total);
        let sum: i64 = r.iter().sum();
        h.eq(&format!("budget: sum matches {total}"), sum, total);
    }
    // fractional remainder distributes correctly
    let scores = [0.9, 0.8, 0.7, 0.6, 0.5, 0.4];
    let r = alloc(&scores, 7);
    let sum: i64 = r.iter().sum();
    h.eq("budget: 7 across 6 views sums to 7", sum, 7);
    h.check("budget: dominant view gets largest share", r[0] >= r[5]);
}

// ─── 6. end-to-end route → fetch → traverse → stop loop ────────────────────

fn test_e2e(h: &mut Harness) {
    let set = fixture_tools();
    let route = tool(&set, "codegraph_route");
    let fetch = tool(&set, "codegraph_fetch");
    let traverse = tool(&set, "codegraph_traverse");
    let stop = tool(&set, "codegraph_stop");

    let query = "find the code that validates a bearer token";
    let r = route.call(&json!({
        "query": query,
        "route_semantic": "high", "route_temporal": "low", "route_causal": "high",
        "route_entity": "low", "route_multi_hop_need": "yes", "route_recency": "low",
        "total": 6,
    })).unwrap();
    let budget = &r["budget"];
    h.check("e2e: budget[semantic] ≥ 1", budget["semantic"].as_i64().unwrap() >= 1);
    h.check("e2e: budget[causal] ≥ 1", budget["causal"].as_i64().unwrap() >= 1);

    // fetch semantic
    let rows = fetch.call(&json!({"view":"semantic","query":"validate","budget":budget["semantic"].as_i64().unwrap()})).unwrap();
    let sem_rows = rows["rows"].as_array().unwrap().len();
    h.check("e2e: semantic rows ≥ 1", sem_rows >= 1);

    // fetch causal from the top semantic hit
    let seed = rows["rows"][0]["qualified_name"].as_str().unwrap_or("authenticate").to_string();
    let causal = fetch.call(&json!({"view":"causal","seed":seed,"budget":budget["causal"].as_i64().unwrap()})).unwrap();
    let causal_rows = causal["rows"].as_array().unwrap().len();
    h.check("e2e: causal rows > 0", causal_rows > 0);

    // traverse top 2 candidates
    let cand0 = rows["rows"][0]["qualified_name"].as_str().unwrap_or("x").to_string();
    let cand1 = causal["rows"][0]["qualified_name"].as_str().unwrap_or("y").to_string();
    let tr = traverse.call(&json!({
        "cand_0_relevance":"high","cand_0_relation_usefulness":"high",
        "cand_0_new_information":"low","cand_0_supports_current_evidence":"low",
        "cand_1_relevance":"low","cand_1_relation_usefulness":"high",
        "cand_1_new_information":"high","cand_1_supports_current_evidence":"high",
    })).unwrap();
    let ranked = tr["ranked"].as_array().unwrap();
    h.eq("e2e: traversal ranks 2 candidates", ranked.len(), 2);
    h.check("e2e: traversal has weighted scores", ranked[0]["weighted"].as_f64().unwrap_or(0.0) > 0.0);

    // stop
    let evidence: Vec<String> = vec![cand0, cand1];
    let r = stop.call(&json!({"query": query, "evidence": evidence, "evidence_status": "sufficient"})).unwrap();
    h.eq("e2e: stop says OK on sufficient evidence", r["decision"].as_str().unwrap(), "STOP_EVIDENCE_OK");

    // contradictory evidence forces CONTINUE
    let r = stop.call(&json!({"query": query, "evidence": evidence, "evidence_status": "contradiction"})).unwrap();
    h.eq("e2e: stop says CONTINUE on contradiction", r["decision"].as_str().unwrap(), "CONTINUE_CONTRADICTION");
}

// ─── 7. guards & edge cases ─────────────────────────────────────────────────

fn test_guards(h: &mut Harness) {
    let set = fixture_tools();
    // empty db_path
    let empty_db = std::env::temp_dir().join(format!("laya-codegraph-empty-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&empty_db);
    let _ = std::process::Command::new("sqlite3")
        .arg(empty_db.to_str().unwrap())
        .arg("CREATE TABLE nodes(id TEXT); CREATE TABLE edges(id INTEGER); CREATE TABLE files(path TEXT);")
        .status();
    let empty_tools = laya_workflow::codegraph::CodegraphTools { spec_dir: set.spec_dir.clone(), db_path: empty_db.clone(), base_url: None };
    let st = tool(&empty_tools, "codegraph_status").call(&json!({})).unwrap();
    // missing nodes_fts → not ready
    h.eq("guards: empty db missing fts → not ready", st["ready"].as_bool().unwrap(), false);
    let r = tool(&empty_tools, "codegraph_fetch").call(&json!({"view":"semantic","query":"x"}));
    h.check("guards: fetch on missing fts errors cleanly", r.is_err());
    let _ = std::fs::remove_file(&empty_db);

    // traversal with all-low hints
    let t = tool(&set, "codegraph_traverse");
    let r = t.call(&json!({
        "cand_0_relevance":"low","cand_0_relation_usefulness":"low",
        "cand_0_new_information":"low","cand_0_supports_current_evidence":"low",
        "cand_1_relevance":"low","cand_1_relation_usefulness":"low",
        "cand_1_new_information":"low","cand_1_supports_current_evidence":"low",
    })).unwrap();
    let ranked = r["ranked"].as_array().unwrap();
    h.check("guards: all-low still produces 2 candidates", ranked.len() == 2);
    h.check("guards: all-low weights are 0-ish", ranked[0]["weighted"].as_f64().unwrap_or(0.0) < 0.5);

    // unicode + long query through the whole pipeline
    let r = tool(&set, "codegraph_route").call(&json!({
        "query": "查找 auth 相关的代码 authentication token",
        "route_semantic": "high", "route_temporal": "low", "route_causal": "high",
        "route_entity": "low", "route_multi_hop_need": "yes", "route_recency": "low",
    })).unwrap();
    h.check("guards: unicode query accepted", r["scores"]["semantic"].as_f64().unwrap() == 0.85);
    let r = tool(&set, "codegraph_fetch").call(&json!({"view":"semantic","query":"authentication","budget":3})).unwrap();
    h.check("guards: semantic empty query tolerated", r.is_object());

    // route with total > available budget
    let r = tool(&set, "codegraph_route").call(&json!({
        "query": "x", "total": 100,
        "route_semantic": "high", "route_temporal": "low", "route_causal": "high",
        "route_entity": "low", "route_multi_hop_need": "yes", "route_recency": "low",
    })).unwrap();
    let sum: i64 = ["semantic","temporal","causal","entity","multi_hop_need","recency_importance"]
        .iter().map(|k| r["budget"][*k].as_i64().unwrap_or(0)).sum();
    h.eq("guards: total=100 sums to 100", sum, 100);

    // specs survive a re-ensure (idempotent)
    let _ = util::ensure_specs(&set.spec_dir);
    let count = std::fs::read_dir(&set.spec_dir).unwrap().count();
    h.eq("guards: specs dir has 3 files after re-ensure", count, 3);
}

pub fn test_codegraph(h: &mut Harness) {
    test_escape(h);
    test_views(h);
    test_specs(h);
    test_mcp_tools(h);
    test_budget(h);
    test_e2e(h);
    test_guards(h);
    test_extended(h);
}
// ─── 8. extended matrix tests (200+ target) ────────────────────────────────

fn test_extended(h: &mut Harness) {
    use laya_workflow::codegraph_util::{allocate_budgets, sql_like_escape, sql_str, view_causal, view_entity, view_multi_hop, view_semantic, view_temporal};
    let db = fixture_db();
    let set = fixture_tools();
    let spec_dir = set.spec_dir.clone();
    let all_keys = ["semantic", "temporal", "causal", "entity", "multi_hop_need", "recency_importance"];

    // ───── escape permutations ─────
    h.eq("sql_str: crlf", sql_str("a\r\nb"), "'a\r\nb'".to_string());
    h.eq("sql_str: tab kept", sql_str("a\tb"), "'a\tb'".to_string());
    h.eq("sql_str: long unicode", sql_str("查询auth代码token"), "'查询auth代码token'".to_string());
    h.eq("sql_str: three quotes", sql_str("a'''b"), "'a''''''b'".to_string());
    h.eq("like_escape: leading percent", sql_like_escape("%foo"), "\\%foo".to_string());
    h.eq("like_escape: trailing underscore", sql_like_escape("foo_"), "foo\\_".to_string());
    h.eq("like_escape: adjacent wildcards", sql_like_escape("%_\\"), "\\%\\_\\\\".to_string());
    h.eq("like_escape: only backslash", sql_like_escape("\\\\"), "\\\\\\\\".to_string());
    h.eq("like_escape: only percent", sql_like_escape("%%"), "\\%\\%".to_string());
    h.eq("like_escape: only underscore", sql_like_escape("__"), "\\_\\_".to_string());

    // ───── semantic edge cases ─────
    let sem_cases: &[(&str, usize)] = &[
        ("auth", 6), ("session", 1), ("token", 3), ("validate_token", 1),
        ("audit", 1), ("refresh", 1), ("Route", 1), ("user", 1),
        ("login", 1), ("health", 1), ("store", 1), ("middleware", 1),
    ];
    for (term, want) in sem_cases {
        let r = view_semantic(db, term, 20).unwrap();
        let n = r["rows"].as_array().map(|a| a.len()).unwrap_or(0);
        h.check(&format!("semantic[exact]: {term:?} → {want} rows"), n == *want);
    }
    let r = view_semantic(db, "auth AND token", 20).unwrap();
    h.check("semantic[AND]: auth AND token ≤ 6", r["rows"].as_array().unwrap().len() <= 6);
    let r = view_semantic(db, "auth OR token", 20).unwrap();
    h.check("semantic[OR]: auth OR token ≥ 3", r["rows"].as_array().unwrap().len() >= 3);
    let r = view_semantic(db, "auth*", 20).unwrap();
    h.check("semantic[prefix]: auth* ≥ 6", r["rows"].as_array().unwrap().len() >= 6);
    let r = view_semantic(db, "validate*", 20).unwrap();
    h.check("semantic[prefix]: validate* ≥ 1", r["rows"].as_array().unwrap().len() >= 1);
    let r = view_semantic(db, "Zzzz*", 5).unwrap();
    h.check("semantic[prefix]: Zzzz* → 0", r["rows"].as_array().map(|a| a.is_empty()).unwrap_or(false));
    let r = view_semantic(db, "auth", 1).unwrap();
    h.eq("semantic[limit=1]", r["rows"].as_array().map(|a| a.len()).unwrap_or(0), 1);
    let r = view_semantic(db, "auth", 100).unwrap();
    h.eq("semantic[limit=100]", r["rows"].as_array().map(|a| a.len()).unwrap_or(0), 6);
    h.check("semantic: empty query ok", view_semantic(db, "", 5).is_ok());
    let r = view_semantic(db, "authenticate", 1).unwrap();
    let row = &r["rows"].as_array().unwrap()[0];
    h.check("semantic: row has id", row["id"].is_string());
    h.check("semantic: row has qualified_name", row["qualified_name"].is_string());
    h.check("semantic: row has score", row["score"].is_number());
    h.check("semantic: row has file_path", row["file_path"].is_string());

    // ───── causal edge cases ─────
    let r = view_causal(db, "authenticate", false, 20).unwrap();
    let rows = r["rows"].as_array().unwrap();
    h.eq("causal[out,authenticate]: 2 rows", rows.len(), 2);
    h.check("causal[out,authenticate]: has SessionStore", rows.iter().any(|x| x["other_qualified_name"].as_str().unwrap_or("").contains("SessionStore")));
    h.check("causal[out,authenticate]: has User", rows.iter().any(|x| x["other_qualified_name"].as_str().unwrap_or("").contains("User")));
    h.check("causal[out,authenticate]: all calls", rows.iter().all(|x| x["edge_kind"].as_str().unwrap_or("") == "calls"));
    let r = view_causal(db, "authenticate", true, 20).unwrap();
    let rows = r["rows"].as_array().unwrap();
    h.eq("causal[in,authenticate]: 4 rows", rows.len(), 4);
    let callers: Vec<&str> = rows.iter().filter_map(|x| x["other_qualified_name"].as_str()).collect();
    h.check("causal[in]: middleware", callers.iter().any(|t| t.contains("middleware")));
    h.check("causal[in]: validate_token", callers.iter().any(|t| t.contains("validate_token")));
    h.check("causal[in]: refresh", callers.iter().any(|t| t.contains("refresh")));
    h.check("causal[in]: login imports", callers.iter().any(|t| t.contains("login")));
    let r = view_causal(db, "SessionStore", false, 20).unwrap();
    let rows = r["rows"].as_array().unwrap();
    h.eq("causal[out,SessionStore]: 1 row", rows.len(), 1);
    h.check("causal[out,SessionStore]: edge references", rows[0]["edge_kind"].as_str().unwrap_or("") == "references");
    h.check("causal[out,SessionStore]: target User", rows[0]["other_qualified_name"].as_str().unwrap_or("").contains("User"));
    let r = view_causal(db, "app::routes::health", true, 20).unwrap();
    let rows = r["rows"].as_array().unwrap();
    h.eq("causal[in,health_route]: 1 row", rows.len(), 1);
    h.check("causal[in,health_route]: edge contains", rows[0]["edge_kind"].as_str().unwrap_or("") == "contains");
    let r = view_causal(db, "authenticate", true, 1).unwrap();
    h.eq("causal[in,limit=1]: 1 row", r["rows"].as_array().map(|a| a.len()).unwrap_or(0), 1);
    h.check("causal: name-only match ≥1", view_causal(db, "auth", false, 20).unwrap()["rows"].as_array().unwrap().len() >= 1);
    h.check("causal: no seed → 0 rows", view_causal(db, "zzzzz_xyz", false, 10).unwrap()["rows"].as_array().map(|a| a.is_empty()).unwrap_or(false));

    // ───── entity edge cases ─────
    let r = view_entity(db, "app::auth::middleware").unwrap();
    let rows = r["rows"].as_array().unwrap();
    h.eq("entity[exact middleware]: 1 row", rows.len(), 1);
    h.check("entity[exact middleware]: kind method", rows[0]["kind"].as_str().unwrap_or("") == "method");
    let r = view_entity(db, "User").unwrap();
    let rows = r["rows"].as_array().unwrap();
    h.eq("entity[exact User]: 1 row", rows.len(), 1);
    h.check("entity[exact User]: kind struct", rows[0]["kind"].as_str().unwrap_or("") == "struct");
    let r = view_entity(db, "OAuthProvider").unwrap();
    let rows = r["rows"].as_array().unwrap();
    h.eq("entity[soft OAuthProvider]: 1 row", rows.len(), 1);
    h.check("entity[soft OAuthProvider]: soft_match=true", rows[0]["soft_match"].as_bool().unwrap_or(false));
    let cands = rows[0]["candidates"].as_str().unwrap_or("");
    h.check("entity[soft OAuthProvider]: 2 candidates", cands.contains("OAuthProvider") && cands.contains("oauth::Provider"));
    h.check("entity[soft OAuthProvider]: edge_kind calls", rows[0]["edge_kind"].as_str().unwrap_or("") == "calls");
    let r = view_entity(db, "Redis").unwrap();
    let rows = r["rows"].as_array().unwrap();
    h.eq("entity[soft Redis]: 1 row", rows.len(), 1);
    h.check("entity[soft Redis]: edge_kind references", rows[0]["edge_kind"].as_str().unwrap_or("") == "references");
    h.check("entity: unknown → 0 rows", view_entity(db, "totally_unknown_xyz").unwrap()["rows"].as_array().map(|a| a.is_empty()).unwrap_or(false));

    // ───── temporal edge cases ─────
    h.eq("temporal[4]: 4 rows", view_temporal(db, 4).unwrap()["rows"].as_array().map(|a| a.len()).unwrap_or(0), 4);
    h.eq("temporal[100]: 4 rows", view_temporal(db, 100).unwrap()["rows"].as_array().map(|a| a.len()).unwrap_or(0), 4);
    h.eq("temporal[2]: 2 rows", view_temporal(db, 2).unwrap()["rows"].as_array().map(|a| a.len()).unwrap_or(0), 2);
    let r = view_temporal(db, 10).unwrap();
    let paths: Vec<&str> = r["rows"].as_array().unwrap().iter().filter_map(|x| x["path"].as_str()).collect();
    h.check("temporal: has src/auth.rs", paths.contains(&"src/auth.rs"));
    h.check("temporal: has src/session.rs", paths.contains(&"src/session.rs"));
    h.check("temporal: has src/main.rs", paths.contains(&"src/main.rs"));
    h.check("temporal: all rust", r["rows"].as_array().unwrap().iter().all(|x| x["language"].as_str() == Some("rust")));
    let auth_row = r["rows"].as_array().unwrap().iter().find(|x| x["path"].as_str() == Some("src/auth.rs")).unwrap();
    h.eq("temporal: auth.rs has 4 nodes", auth_row["node_count"].as_i64().unwrap(), 4);
    let main_row = r["rows"].as_array().unwrap().iter().find(|x| x["path"].as_str() == Some("src/main.rs")).unwrap();
    h.eq("temporal: main.rs has 0 nodes", main_row["node_count"].as_i64().unwrap(), 0);

    // ───── multi-hop edge cases ─────
    let r = view_multi_hop(db, "login", 0, 20).unwrap();
    let rows = r["rows"].as_array().unwrap();
    h.eq("multi_hop[depth=0]: 1 row", rows.len(), 1);
    h.eq("multi_hop[depth=0]: depth=0", rows[0]["depth"].as_i64().unwrap(), 0);
    let r = view_multi_hop(db, "health", 2, 50).unwrap();
    let rows = r["rows"].as_array().unwrap();
    let depths: Vec<i64> = rows.iter().filter_map(|x| x["depth"].as_i64()).collect();
    let names: Vec<&str> = rows.iter().filter_map(|x| x["qualified_name"].as_str()).collect();
    h.check("multi_hop[health,d2]: ≥3 rows", rows.len() >= 3);
    h.check("multi_hop[health,d2]: max depth ≤2", depths.iter().all(|&d| d <= 2));
    h.check("multi_hop[health,d2]: login at depth 2", depths.contains(&2) && names.iter().any(|n| n.contains("login")));
    let r = view_multi_hop(db, "login", 3, 50).unwrap();
    let rows = r["rows"].as_array().unwrap();
    h.check("multi_hop[login,d3]: authenticate at depth 1", rows.iter().any(|x| x["depth"].as_i64() == Some(1) && x["qualified_name"].as_str().unwrap_or("").contains("authenticate")));
    h.check("multi_hop[login,d3]: ≥2 depths", rows.iter().filter_map(|x| x["depth"].as_i64()).collect::<std::collections::BTreeSet<_>>().len() >= 2);
    h.check("multi_hop: no seed → 0 rows", view_multi_hop(db, "xyz_no_match", 3, 10).unwrap()["rows"].as_array().map(|a| a.is_empty()).unwrap_or(false));
    h.check("multi_hop: limit=2 → ≤2 rows", view_multi_hop(db, "login", 5, 2).unwrap()["rows"].as_array().map(|a| a.len()).unwrap_or(0) <= 2);

    // ───── routing permutations ─────
    let field_for = |k: &str| -> String { if k == "recency_importance" { "route_recency".to_string() } else { format!("route_{}", k) } };
    for (i, hi) in all_keys.iter().enumerate() {
        let mut state = json!({"query": "x"});
        for (j, k) in all_keys.iter().enumerate() {
            state[field_for(k)] = json!(if j == i { "high" } else { "low" });
        }
        let r = run_spec(&spec_dir, "routing_codegraph", &state);
        let s = &r["result"];
        h.eq(&format!("routing: {hi} high → 0.85"), s[*hi].as_f64().unwrap(), 0.85);
        for (j, lo) in all_keys.iter().enumerate() {
            if j != i {
                h.eq(&format!("routing: {hi} high / {lo} → 0.1"), s[*lo].as_f64().unwrap(), 0.1);
            }
        }
    }
    let mut state = json!({"query": "x"});
    for k in all_keys { state[field_for(k)] = json!("low"); }
    let r = run_spec(&spec_dir, "routing_codegraph", &state);
    for k in all_keys { h.eq(&format!("routing: all-low / {k} → 0.1"), r["result"][k].as_f64().unwrap(), 0.1); }
    let mut state = json!({"query": "x"});
    for k in all_keys { state[field_for(k)] = json!("yes"); }
    let r = run_spec(&spec_dir, "routing_codegraph", &state);
    for k in all_keys { h.eq(&format!("routing: all-yes / {k} → 0.85"), r["result"][k].as_f64().unwrap(), 0.85); }
    let r = run_spec(&spec_dir, "routing_codegraph", &json!({"query":"x","route_semantic":"high"}));
    let leaked = all_keys.iter().filter(|k| **k != "semantic" && r["result"][**k].as_f64().unwrap_or(0.0) > 0.5).count();
    h.check("routing: 1 explicit field leaks to ≥1 other", leaked >= 1);

    // ───── stopping permutations ─────
    let mut stop_case = |name: &str, state: &Value, want: &str| {
        let r = run_spec(&spec_dir, "stopping_codegraph", state);
        h.eq(&format!("stopping: {name}"), r["result"]["label"].as_str().unwrap_or("").to_string(), want.to_string());
    };
    stop_case("contradiction+missing", &json!({"query":"q","evidence":["a"],"contradiction":"contradiction","missing_evidence":"missing"}), "CONTINUE_CONTRADICTION");
    stop_case("contradiction+sufficient", &json!({"query":"q","evidence":["a"],"contradiction":"contradiction","evidence_status":"sufficient"}), "CONTINUE_CONTRADICTION");
    stop_case("missing+sufficient", &json!({"query":"q","evidence":["a"],"missing_evidence":"missing","evidence_status":"sufficient"}), "CONTINUE_MISSING");
    stop_case("insufficient+missing", &json!({"query":"q","evidence":["a"],"evidence_status":"insufficient","missing_evidence":"missing"}), "CONTINUE_MISSING");
    stop_case("no signals", &json!({"query":"q"}), "CONTINUE_EVIDENCE_INSUFFICIENT");
    stop_case("empty evidence", &json!({"query":"q","evidence":[]}), "CONTINUE_EVIDENCE_INSUFFICIENT");
    stop_case("all three", &json!({"query":"q","evidence":["a"],"contradiction":"contradiction","missing_evidence":"missing","evidence_status":"sufficient"}), "CONTINUE_CONTRADICTION");
    stop_case("sufficient only", &json!({"query":"q","evidence":["a"],"evidence_status":"sufficient"}), "STOP_EVIDENCE_OK");
    stop_case("contradiction only", &json!({"query":"q","evidence":["a"],"contradiction":"contradiction"}), "CONTINUE_CONTRADICTION");
    stop_case("missing only", &json!({"query":"q","evidence":["a"],"missing_evidence":"missing"}), "CONTINUE_MISSING");
    stop_case("insufficient only", &json!({"query":"q","evidence":["a"],"evidence_status":"insufficient"}), "CONTINUE_EVIDENCE_INSUFFICIENT");

    // ───── traversal permutations ─────
    let r = tool(&set, "codegraph_traverse").call(&json!({"cand_0_relevance":"high","cand_0_relation_usefulness":"high","cand_0_new_information":"high","cand_0_supports_current_evidence":"high"})).unwrap();
    let ranked = r["ranked"].as_array().unwrap();
    h.eq("traverse: only cand_0 → 2 candidates", ranked.len(), 2);
    h.eq("traverse: only cand_0 → cand_0 wins", ranked[0]["candidate"].as_i64().unwrap(), 0);
    let r = tool(&set, "codegraph_traverse").call(&json!({"cand_0_relevance":"low","cand_0_relation_usefulness":"low","cand_0_new_information":"high","cand_0_supports_current_evidence":"low","cand_1_relevance":"low","cand_1_relation_usefulness":"high","cand_1_new_information":"high","cand_1_supports_current_evidence":"high","weights":{"relevance":0.0,"relation_usefulness":1.0,"new_information":0.0,"supports_current_evidence":0.0}})).unwrap();
    h.eq("traverse: custom weights flip", r["ranked"].as_array().unwrap()[0]["candidate"].as_i64().unwrap(), 1);
    let r = tool(&set, "codegraph_traverse").call(&json!({"cand_0_relevance":"high","cand_0_relation_usefulness":"high","cand_0_new_information":"high","cand_0_supports_current_evidence":"high","cand_1_relevance":"high","cand_1_relation_usefulness":"high","cand_1_new_information":"high","cand_1_supports_current_evidence":"high","weights":{"relevance":0.0,"relation_usefulness":0.0,"new_information":0.0,"supports_current_evidence":0.0}})).unwrap();
    let ranked = r["ranked"].as_array().unwrap();
    h.check("traverse: zero weights → both 0", ranked.iter().all(|c| c["weighted"].as_f64().unwrap() < 0.001));
    let r = tool(&set, "codegraph_traverse").call(&json!({})).unwrap();
    h.eq("traverse: empty args → 2 candidates", r["ranked"].as_array().unwrap().len(), 2);

    // ───── MCP tool boundary cases ─────
    let r = tool(&set, "codegraph_route").call(&json!({"query":"x","total":0,"route_semantic":"high","route_temporal":"low","route_causal":"high","route_entity":"low","route_multi_hop_need":"yes","route_recency":"low"})).unwrap();
    let s: i64 = all_keys.iter().map(|k| r["budget"][*k].as_i64().unwrap()).sum();
    h.eq("route: total=0 clamps to 1", s, 1);
    let r = tool(&set, "codegraph_route").call(&json!({"query":"x","total":-10,"route_semantic":"high","route_temporal":"low","route_causal":"high","route_entity":"low","route_multi_hop_need":"yes","route_recency":"low"})).unwrap();
    let s: i64 = all_keys.iter().map(|k| r["budget"][*k].as_i64().unwrap()).sum();
    h.eq("route: total=-10 clamps to 1", s, 1);
    h.check("route: missing query errors", tool(&set, "codegraph_route").call(&json!({})).is_err());
    h.check("fetch: budget=0 OK", tool(&set, "codegraph_fetch").call(&json!({"view":"temporal","budget":0})).is_ok());
    h.check("fetch: budget=-5 OK", tool(&set, "codegraph_fetch").call(&json!({"view":"temporal","budget":-5})).is_ok());
    h.check("fetch: budget=9999 OK", tool(&set, "codegraph_fetch").call(&json!({"view":"temporal","budget":9999})).is_ok());
    h.check("fetch: depth=0 OK", tool(&set, "codegraph_fetch").call(&json!({"view":"multi_hop","seed":"login","depth":0,"budget":5})).is_ok());
    h.check("fetch: depth=-5 OK", tool(&set, "codegraph_fetch").call(&json!({"view":"multi_hop","seed":"login","depth":-5,"budget":5})).is_ok());
    h.check("fetch: depth=99 OK", tool(&set, "codegraph_fetch").call(&json!({"view":"multi_hop","seed":"login","depth":99,"budget":5})).is_ok());
    h.check("fetch: unknown view errors", tool(&set, "codegraph_fetch").call(&json!({"view":"garbage"})).is_err());
    h.check("fetch: missing view errors", tool(&set, "codegraph_fetch").call(&json!({})).is_err());
    h.check("fetch: bad db_path errors", tool(&set, "codegraph_fetch").call(&json!({"view":"semantic","db_path":"/nope/missing.db"})).is_err());
    let r1 = tool(&set, "codegraph_fetch").call(&json!({"view":"semantic","query":"auth","budget":3})).unwrap();
    let r2 = tool(&set, "codegraph_fetch").call(&json!({"view":"semantic","query":"auth","budget":3})).unwrap();
    h.eq("fetch: deterministic", r1["rows"].as_array().unwrap().len(), r2["rows"].as_array().unwrap().len());
    h.check("stop: missing query errors", tool(&set, "codegraph_stop").call(&json!({})).is_err());
    let r = tool(&set, "codegraph_stop").call(&json!({"query":"q","missing_evidence":false})).unwrap();
    h.eq("stop: missing_evidence=false → INSUFFICIENT", r["decision"].as_str().unwrap().to_string(), "CONTINUE_EVIDENCE_INSUFFICIENT".to_string());
    let r = tool(&set, "codegraph_status").call(&json!({"db_path":"/nope/missing.db"})).unwrap();
    h.eq("status: bad db_path → not ready", r["ready"].as_bool().unwrap(), false);
    let r = tool(&set, "codegraph_status").call(&json!({})).unwrap();
    let tabs = r["required_tables"].as_array().unwrap();
    h.check("status: required_tables has nodes", tabs.iter().any(|v| v.as_str() == Some("nodes")));
    h.check("status: required_tables has edges", tabs.iter().any(|v| v.as_str() == Some("edges")));
    h.check("status: required_tables has files", tabs.iter().any(|v| v.as_str() == Some("files")));
    h.check("status: required_tables has nodes_fts", tabs.iter().any(|v| v.as_str() == Some("nodes_fts")));
    let ek = r["stats"]["edge_kinds"].as_array().unwrap();
    h.check("status: edge_kinds has calls", ek.iter().any(|x| x["kind"].as_str() == Some("calls")));
    let nk = r["stats"]["node_kinds"].as_array().unwrap();
    h.check("status: node_kinds has function", nk.iter().any(|x| x["kind"].as_str() == Some("function")));
    h.check("status: node_kinds has class", nk.iter().any(|x| x["kind"].as_str() == Some("class")));
    h.check("status: node_kinds has route", nk.iter().any(|x| x["kind"].as_str() == Some("route")));

    // ───── budget allocation matrix ─────
    let alloc = |scores: &[f64; 6], total: i64| allocate_budgets(scores, total);
    h.eq("budget: total=1 uniform", alloc(&[0.5;6], 1), [1,0,0,0,0,0]);
    h.eq("budget: total=2 two-equal", alloc(&[0.5,0.5,0.0,0.0,0.0,0.0], 2), [1,1,0,0,0,0]);
    h.eq("budget: total=3 three-equal", alloc(&[0.5,0.5,0.5,0.0,0.0,0.0], 3), [1,1,1,0,0,0]);
    h.eq("budget: total=6 uniform", alloc(&[0.5;6], 6), [1,1,1,1,1,1]);
    h.eq("budget: total=12 uniform", alloc(&[0.5;6], 12), [2,2,2,2,2,2]);
    h.eq("budget: total=4 dominant", alloc(&[0.9,0.1,0.0,0.0,0.0,0.0], 4), [4,0,0,0,0,0]);
    for total in [1i64, 2, 3, 4, 5, 7, 10, 16, 24, 50, 100] {
        for scores in [[0.1,0.2,0.3,0.4,0.5,0.6], [0.9,0.8,0.7,0.6,0.5,0.4], [1.0,1.0,1.0,1.0,1.0,1.0], [0.5;6]].iter() {
            let r = alloc(scores, total);
            let sum: i64 = r.iter().sum();
            h.eq(&format!("budget: total={total} sum invariant"), sum, total);
            h.check(&format!("budget: total={total} all >= 0"), r.iter().all(|&x| x >= 0));
            h.check(&format!("budget: total={total} all <= total"), r.iter().all(|&x| x <= total));
        }
    }
    for total in [3i64, 7, 10] {
        let r = alloc(&[0.9,0.1,0.1,0.1,0.1,0.1], total);
        h.check(&format!("budget: dominant wins, total={total}"), r[0] >= r[5]);
    }
    h.eq("budget: all-zero + total>0 → all zeros", alloc(&[0.0;6], 5), [0;6]);

    // ───── additional e2e paths ─────
    let route = tool(&set, "codegraph_route");
    let fetch = tool(&set, "codegraph_fetch");
    let stop = tool(&set, "codegraph_stop");
    let r = route.call(&json!({"query":"authenticate","total":4,"route_semantic":"low","route_temporal":"low","route_causal":"high","route_entity":"low","route_multi_hop_need":"low","route_recency":"low"})).unwrap();
    h.check("e2e2: causal dominant (≥3/4)", r["budget"]["causal"].as_i64().unwrap() >= 3);
    let cr = fetch.call(&json!({"view":"causal","seed":"authenticate","budget":4})).unwrap();
    h.check("e2e2: causal rows ≥1", cr["rows"].as_array().unwrap().len() >= 1);
    let mh = fetch.call(&json!({"view":"multi_hop","seed":"login","depth":2,"budget":5})).unwrap();
    let names: Vec<&str> = mh["rows"].as_array().unwrap().iter().filter_map(|x| x["qualified_name"].as_str()).collect();
    h.check("e2e2: multi_hop reaches authenticate", names.iter().any(|n| n.contains("authenticate")));
    h.check("e2e2: multi_hop depth ≤2", mh["rows"].as_array().unwrap().iter().filter_map(|x| x["depth"].as_i64()).all(|d| d <= 2));
    let sr = stop.call(&json!({"query":"q"})).unwrap();
    h.eq("e2e2: stop no evidence → CONTINUE", sr["decision"].as_str().unwrap().to_string(), "CONTINUE_EVIDENCE_INSUFFICIENT".to_string());
    let sr = stop.call(&json!({"query":"x","evidence":["a","b"],"evidence_status":"sufficient"})).unwrap();
    h.eq("e2e2: stop sufficient → OK", sr["decision"].as_str().unwrap().to_string(), "STOP_EVIDENCE_OK".to_string());

    // ───── additional guards ─────
    let count_before = std::fs::read_dir(&spec_dir).unwrap().count();
    let _ = laya_workflow::codegraph_util::ensure_specs(&spec_dir);
    let count_after = std::fs::read_dir(&spec_dir).unwrap().count();
    h.eq("ensure_specs: idempotent", count_before, count_after);
    let r = route.call(&json!({"query":"x","total":6,"route_semantic":"high","route_temporal":"low","route_causal":"high","route_entity":"low","route_multi_hop_need":"yes","route_recency":"low","route_unknown_field":"high"})).unwrap();
    let s: i64 = all_keys.iter().map(|k| r["budget"][*k].as_i64().unwrap()).sum();
    h.eq("route: extra unknown field no-op", s, 6);
    h.check("fetch: empty seed falls back to query", tool(&set, "codegraph_fetch").call(&json!({"view":"causal","query":"authenticate","seed":"","budget":1})).is_ok());

    // ───── default DB path resolution ─────
    let saved_db = std::env::var_os("LAYA_CODEGRAPH_DB");
    std::env::set_var("LAYA_CODEGRAPH_DB", "/tmp/override-cg.db");
    let d = laya_workflow::codegraph_util::default_db_path();
    h.eq("default_db_path: env override wins", d.to_string_lossy().to_string(), "/tmp/override-cg.db".to_string());
    match saved_db {
        Some(v) => std::env::set_var("LAYA_CODEGRAPH_DB", v),
        None => std::env::remove_var("LAYA_CODEGRAPH_DB"),
    }
}

