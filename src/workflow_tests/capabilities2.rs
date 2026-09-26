//! capabilities2 regression sections for `laya-workflow-tests`.
#![allow(unused_imports)]
use crate::*;
pub fn test_capabilities_batch2(h: &mut Harness) {

    let reg = capability::registry_from(
        &[
            ("j_pick", json!({"kind": "json", "op": "pick"})),
            ("j_merge", json!({"kind": "json", "op": "merge"})),
            ("j_patch", json!({"kind": "json", "op": "patch"})),
            ("j_set", json!({"kind": "json", "op": "path_set"})),
            ("j_flat", json!({"kind": "json", "op": "flatten"})),
            ("c_parse", json!({"kind": "csv", "op": "parse"})),
            ("c_gen", json!({"kind": "csv", "op": "generate", "headers": true})),
            ("x_tags", json!({"kind": "xml", "op": "tags"})),
            ("x_text", json!({"kind": "xml", "op": "text", "tag": "b"})),
            ("m_head", json!({"kind": "markdown", "op": "headings"})),
            ("m_code", json!({"kind": "markdown", "op": "code_blocks"})),
            ("m_links", json!({"kind": "markdown", "op": "links"})),
            ("d_lines", json!({"kind": "diff", "op": "lines"})),
            ("d_json", json!({"kind": "diff", "op": "json"})),
            ("v", json!({"kind": "validate"})),
            ("math_e", json!({"kind": "math", "op": "eval"})),
            ("math_s", json!({"kind": "math", "op": "stats"})),
            ("h_sha", json!({"kind": "hash", "op": "sha256"})),
            ("h_fnv", json!({"kind": "hash", "op": "fnv1a64"})),
            ("h_crc", json!({"kind": "hash", "op": "crc32"})),
            ("g_reach", json!({"kind": "graph", "op": "reachable"})),
            ("g_topo", json!({"kind": "graph", "op": "toposort"})),
            ("tk", json!({"kind": "tokenize", "op": "count"})),
            ("cron", json!({"kind": "cron", "expr": "30 2 * * *"})),
        ],
        None,
    )
    .unwrap();

    // json
    let r = reg.call("j_pick", &json!({"value": {"a":1,"b":2,"c":3}, "keys": ["a","c"]}), &json!({})).unwrap();
    h.eq("json.pick keeps only requested keys", r["value"].as_object().unwrap().len(), 2);
    let r = reg.call("j_merge", &json!({"a": {"x":1}, "b": {"y":2}}), &json!({})).unwrap();
    h.eq("json.merge unions keys", r["value"].as_object().unwrap().len(), 2);
    let r = reg.call("j_patch", &json!({"value": {"a":1,"b":2}, "patch": {"b": null, "c": 3}}), &json!({})).unwrap();
    h.check("json.patch deletes with null", r["value"].get("b").is_none() && r["value"]["c"].as_i64() == Some(3));
    let r = reg.call("j_set", &json!({"value": {}, "path": "a/b/c", "new_value": 9}), &json!({})).unwrap();
    h.eq("json.path_set nests", r["value"]["a"]["b"]["c"].as_i64().unwrap(), 9);
    let r = reg.call("j_flat", &json!({"value": {"a": {"b": 1}}}), &json!({})).unwrap();
    h.eq("json.flatten dotted key", r["value"]["a.b"].as_i64().unwrap(), 1);
    h.check("json bad op rejected",
            capability::registry_from(&[("bad", json!({"kind":"json","op":"nope"}))], None)
                .unwrap().call("bad", &json!({}), &json!({})).is_err());

    // csv
    let r = reg.call("c_parse", &json!({"text": "a,b\n1,\"x,y\"\n"}), &json!({})).unwrap();
    h.eq("csv.parse headers", r["headers"].as_array().unwrap().len(), 2);
    h.eq("csv.parse quoted cell", r["rows"][0][1].as_str().unwrap().to_string(), "x,y".to_string());
    let r = reg.call("c_gen", &json!({"headers": ["h1","h2"], "rows": [[1,"a"], [2,"b"]]}), &json!({})).unwrap();
    h.check("csv.generate emits header + rows", r["text"].as_str().unwrap().starts_with("h1,h2"));

    // xml / markdown
    let r = reg.call("x_tags", &json!({"text": "<a><b>x</b><c/></a>"}), &json!({})).unwrap();
    h.eq("xml.tags distinct sorted", r["tags"].as_array().unwrap().len(), 3);
    let r = reg.call("x_text", &json!({"text": "<a><b>hello</b><b>world</b></a>"}), &json!({})).unwrap();
    h.eq("xml.text extracts both", r["texts"].as_array().unwrap().len(), 2);
    let md = "# Title\n\n## Sub\n\n```rust\nlet x = 1;\n```\n\n[link](http://x)\n";
    let r = reg.call("m_head", &json!({"text": md}), &json!({})).unwrap();
    h.eq("markdown.headings count", r["count"].as_u64().unwrap(), 2);
    let r = reg.call("m_code", &json!({"text": md}), &json!({})).unwrap();
    h.eq("markdown.code_blocks lang", r["blocks"][0]["lang"].as_str().unwrap().to_string(), "rust".to_string());
    let r = reg.call("m_links", &json!({"text": md}), &json!({})).unwrap();
    h.eq("markdown.links url", r["links"][0]["url"].as_str().unwrap().to_string(), "http://x".to_string());

    // diff
    let r = reg.call("d_lines", &json!({"a": "l1\nl2\nl3", "b": "l1\nX\nl3"}), &json!({})).unwrap();
    h.eq("diff.lines changed count", r["changed"].as_array().unwrap().len(), 1);
    h.eq("diff.lines not identical", r["identical"].as_bool().unwrap(), false);
    let r = reg.call("d_json", &json!({"a": {"k": 1}, "b": {"k": 2, "n": 3}}), &json!({})).unwrap();
    h.eq("diff.json deltas (change+add)", r["count"].as_u64().unwrap(), 2);

    // validate
    let r = reg.call("v", &json!({
        "value": {"name": "x", "age": 5, "kind": "b"},
        "schema": {"type": "object", "required": ["name", "missing"],
                   "properties": {"age": {"type": "number", "minimum": 10},
                                  "kind": {"enum": ["a"]}}}
    }), &json!({})).unwrap();
    h.eq("validate reports 3 errors", r["error_count"].as_u64().unwrap(), 3);
    h.eq("validate valid=false", r["valid"].as_bool().unwrap(), false);
    let ok = reg.call("v", &json!({"value": {"name": "y"}, "schema": {"type": "object", "required": ["name"]}}), &json!({})).unwrap();
    h.eq("validate passes a good value", ok["valid"].as_bool().unwrap(), true);

    // math
    h.eq("math.eval precedence", reg.call("math_e", &json!({"expression": "2 + 3 * (4 - 1)"}), &json!({})).unwrap()["value"].as_f64().unwrap(), 11.0);
    h.eq("math.eval power", reg.call("math_e", &json!({"expression": "2^3"}), &json!({})).unwrap()["value"].as_f64().unwrap(), 8.0);
    h.check("math.eval rejects garbage", reg.call("math_e", &json!({"expression": "1 + +"}), &json!({})).is_err());
    let r = reg.call("math_s", &json!({"values": [1, 2, 3, 4, 100]}), &json!({})).unwrap();
    h.eq("math.stats mean", r["mean"].as_f64().unwrap(), 22.0);
    h.eq("math.stats p50", r["p50"].as_f64().unwrap(), 3.0);
    h.check("math.stats rejects empty", reg.call("math_s", &json!({"values": []}), &json!({})).is_err());

    // hash
    let sha = reg.call("h_sha", &json!({"text": "abc"}), &json!({})).unwrap();
    h.eq("hash.sha256 known vector", sha["hex"].as_str().unwrap().to_string(),
         "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string());
    let crc = reg.call("h_crc", &json!({"text": "123456789"}), &json!({})).unwrap();
    h.eq("hash.crc32 known vector", crc["hex"].as_str().unwrap().to_string(), "cbf43926".to_string());
    h.check("hash stable", reg.call("h_fnv", &json!({"text": "x"}), &json!({})).is_ok());
    h.check("hash bad op rejected",
            capability::registry_from(&[("bad", json!({"kind":"hash","op":"md5"}))], None)
                .unwrap().call("bad", &json!({"text":"x"}), &json!({})).is_err());

    // graph
    let edges = json!([["a","b"],["b","c"],["a","d"]]);
    let r = reg.call("g_reach", &json!({"edges": edges, "start": "a"}), &json!({})).unwrap();
    h.eq("graph.reachable count", r["count"].as_u64().unwrap(), 4);
    let r = reg.call("g_topo", &json!({"edges": edges}), &json!({})).unwrap();
    h.eq("graph.toposort acyclic", r["cyclic"].as_bool().unwrap(), false);
    let cyc = reg.call("g_topo", &json!({"edges": [["a","b"],["b","a"]]}), &json!({})).unwrap();
    h.eq("graph.toposort detects cycle", cyc["cyclic"].as_bool().unwrap(), true);
    h.check("graph.reachable needs start", reg.call("g_reach", &json!({"edges": edges}), &json!({})).is_err());

    // tokenize
    let r = reg.call("tk", &json!({"text": "hello world this is a test"}), &json!({})).unwrap();
    h.eq("tokenize word count", r["words"].as_u64().unwrap(), 6);
    h.check("tokenize estimate positive", r["estimated_tokens"].as_u64().unwrap() > 0);

    // cron
    let r = reg.call("cron", &json!({"from_epoch": 1790380800i64}), &json!({})).unwrap();
    h.check("cron finds a next fire", r["matched"].as_bool().unwrap());
    h.check("cron next is in the future", r["next_epoch"].as_i64().unwrap() > 1790380800);
    h.check("cron rejects bad field count",
            capability::registry_from(&[("bad", json!({"kind":"cron","expr":"* * *"}))], None)
                .unwrap().call("bad", &json!({}), &json!({})).is_err());

    // ── stateful stores (path-scoped) ───────────────────────────────
    let sroot = std::env::temp_dir().join("laya_cap_store_test");
    let _ = std::fs::remove_dir_all(&sroot);
    std::fs::create_dir_all(&sroot).unwrap();
    let mut spol = capability::Policy::default();
    spol.allow_paths = vec![sroot.to_str().unwrap().to_string()];
    let kv_path = sroot.join("kv.json").to_str().unwrap().to_string();
    let q_path = sroot.join("q.json").to_str().unwrap().to_string();
    let ca_path = sroot.join("cache.json").to_str().unwrap().to_string();
    let n_path = sroot.join("notify.log").to_str().unwrap().to_string();
    let reg = capability::registry_from(
        &[
            ("kv_set", json!({"kind": "keyvalue", "op": "set", "path": kv_path})),
            ("kv_get", json!({"kind": "keyvalue", "op": "get", "path": kv_path})),
            ("kv_incr", json!({"kind": "keyvalue", "op": "incr", "path": kv_path})),
            ("kv_del", json!({"kind": "keyvalue", "op": "del", "path": kv_path})),
            ("kv_list", json!({"kind": "keyvalue", "op": "list", "path": kv_path})),
            ("q_push", json!({"kind": "queue", "op": "push", "path": q_path})),
            ("q_pop", json!({"kind": "queue", "op": "pop", "path": q_path})),
            ("q_len", json!({"kind": "queue", "op": "length", "path": q_path})),
            ("ca_set", json!({"kind": "cache", "op": "set", "path": ca_path, "ttl_secs": 3600})),
            ("ca_get", json!({"kind": "cache", "op": "get", "path": ca_path})),
            ("ca_ttl", json!({"kind": "cache", "op": "ttl", "path": ca_path})),
            ("ca_expired", json!({"kind": "cache", "op": "set", "path": ca_path, "ttl_secs": -1})),
            ("notify", json!({"kind": "notify", "path": n_path})),
            ("m", json!({"kind": "metrics", "what": "mem,load,uptime,cpu"})),
        ],
        Some(spol.clone()),
    )
    .unwrap();

    reg.call("kv_set", &json!({"key": "k1", "value": {"n": 1}}), &json!({})).unwrap();
    let r = reg.call("kv_get", &json!({"key": "k1"}), &json!({})).unwrap();
    h.eq("keyvalue roundtrip", r["value"]["n"].as_i64().unwrap(), 1);
    h.eq("keyvalue incr", reg.call("kv_incr", &json!({"key": "c", "by": 5}), &json!({})).unwrap()["to"].as_i64().unwrap(), 5);
    h.eq("keyvalue del", reg.call("kv_del", &json!({"key": "k1"}), &json!({})).unwrap()["removed"].as_bool().unwrap(), true);
    h.eq("keyvalue list count", reg.call("kv_list", &json!({}), &json!({})).unwrap()["count"].as_u64().unwrap(), 1);
    h.check("keyvalue missing key -> found=false",
            reg.call("kv_get", &json!({"key": "nope"}), &json!({})).unwrap()["found"].as_bool().unwrap() == false);

    reg.call("q_push", &json!({"value": "a"}), &json!({})).unwrap();
    reg.call("q_push", &json!({"value": "b"}), &json!({})).unwrap();
    h.eq("queue length", reg.call("q_len", &json!({}), &json!({})).unwrap()["length"].as_u64().unwrap(), 2);
    h.eq("queue FIFO pop", reg.call("q_pop", &json!({}), &json!({})).unwrap()["value"].as_str().unwrap().to_string(), "a".to_string());
    h.eq("queue length after pop", reg.call("q_len", &json!({}), &json!({})).unwrap()["length"].as_u64().unwrap(), 1);

    reg.call("ca_set", &json!({"key": "c1", "value": 42}), &json!({})).unwrap();
    let r = reg.call("ca_get", &json!({"key": "c1"}), &json!({})).unwrap();
    h.check("cache hit returns value", r["hit"].as_bool().unwrap() && r["value"].as_i64() == Some(42));
    h.check("cache ttl positive", reg.call("ca_ttl", &json!({"key": "c1"}), &json!({})).unwrap()["ttl_secs"].as_i64().unwrap() > 0);
    reg.call("ca_expired", &json!({"key": "c2", "value": 1}), &json!({})).unwrap();
    let r = reg.call("ca_get", &json!({"key": "c2"}), &json!({})).unwrap();
    h.eq("cache expired entry misses", r["hit"].as_bool().unwrap(), false);

    let r = reg.call("notify", &json!({"event": "test", "message": "hello"}), &json!({})).unwrap();
    h.check("notify wrote a log line", r["bytes"].as_u64().unwrap() > 0);
    h.check("notify file exists", std::path::Path::new(&n_path).exists());

    // store path is allow-listed exactly like `file`. Use a capability with NO
    // configured path so the call-time `with.path` is what gets checked.
    let reg_withpath = capability::registry_from(
        &[("kv", json!({"kind": "keyvalue", "op": "get"}))],
        Some(spol.clone()),
    )
    .unwrap();
    let outside = std::env::temp_dir().join("laya_cap_store_outside.json");
    let e = reg_withpath
        .call("kv", &json!({"path": outside.to_str().unwrap(), "key": "k"}), &json!({}))
        .unwrap_err()
        .to_string();
    h.check("store path allow-list denies outside", e.contains("outside the allowed roots"));
    h.check("store path from `with` works inside the root",
            reg_withpath.call("kv", &json!({"path": kv_path, "key": "c"}), &json!({})).is_ok());
    let no_roots = capability::registry_from(&[("kv", json!({"kind":"keyvalue","op":"get","path":"/tmp/x.json"}))], None).unwrap();
    h.check("store without allow_paths is denied", no_roots.call("kv", &json!({"key":"k"}), &json!({})).is_err());

    // metrics (read-only, no gating)
    let r = reg.call("m", &json!({}), &json!({})).unwrap();
    h.check("metrics cpu_count > 0", r["cpu_count"].as_u64().unwrap_or(0) > 0);
    h.check("metrics mem total > 0", r["mem"]["total_kb"].as_u64().unwrap_or(0) > 0);
    h.check("metrics uptime present", r["uptime_secs"].as_f64().is_some());
    h.check("metrics loadavg present", r["loadavg"]["1m"].as_f64().is_some());
    let _ = std::fs::remove_dir_all(&sroot);

    // ── batch 3: raw sockets, plaintext protocols, services ─────────
}


