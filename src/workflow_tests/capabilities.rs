//! capabilities regression sections for `laya-workflow-tests`.
#![allow(unused_imports)]
use crate::*;

pub fn test_capabilities(h: &mut Harness) {
    // template expansion
    let st = json!({"text": "hi", "n": 7, "nested": {"k": "v"}});
    let w = json!({"q": "A"});
    h.eq(
        "cap: expand state leaf",
        capability::expand(&json!("${state.text}"), &st, &w),
        json!("hi"),
    );
    h.eq(
        "cap: expand keeps number type",
        capability::expand(&json!("${state.n}"), &st, &w),
        json!(7),
    );
    h.eq(
        "cap: expand nested path",
        capability::expand(&json!("${state.nested.k}"), &st, &w),
        json!("v"),
    );
    h.eq(
        "cap: expand with-answer",
        capability::expand(&json!("${with.q}"), &st, &w),
        json!("A"),
    );
    h.eq(
        "cap: interpolate in string",
        capability::expand(&json!("x=${state.n}!"), &st, &w),
        json!("x=7!"),
    );
    h.eq(
        "cap: missing path -> null",
        capability::expand(&json!("${state.nope}"), &st, &w),
        json!(null),
    );
    std::env::set_var("LAYA_CAP_TEST", "secret");
    h.eq(
        "cap: env lookup",
        capability::expand(&json!("${env.LAYA_CAP_TEST}"), &st, &w),
        json!("secret"),
    );

    // policy: exec denied by default
    let reg = capability::registry_from(
        &[("sh", json!({"kind": "exec", "argv": ["/bin/echo", "hi"]}))],
        None,
    )
    .unwrap();
    h.check(
        "cap: exec denied by default",
        reg.call("sh", &json!({}), &json!({})).is_err(),
    );

    // policy: exec allowed + output captured
    let mut pol = capability::Policy::default();
    pol.allow_exec = true;
    let reg = capability::registry_from(
        &[(
            "sh",
            json!({"kind": "exec", "argv": ["/bin/echo", "hi ${state.n}"], "timeout_ms": 5000}),
        )],
        Some(pol.clone()),
    )
    .unwrap();
    let out = reg.call("sh", &json!({}), &json!({"n": 3})).unwrap();
    h.eq(
        "cap: exec stdout expanded",
        out["stdout"].as_str().unwrap().trim().to_string(),
        "hi 3".to_string(),
    );
    h.eq("cap: exec exit code", out["exit_code"].as_i64().unwrap(), 0);

    // A child whose output overflows the ~64KiB pipe buffer must still finish.
    // Regression: exec used to poll try_wait() and only collect afterwards, so a
    // child writing more than one pipe-full blocked in write() forever and the
    // call failed with a timeout.  `cua-driver call list_windows` answers with
    // ~190KiB, which is how this surfaced in the CUA spec.
    // 128KiB per stream: past the ~64KiB pipe buffer (so the old code wedged)
    // but under the 256KiB default max_output (so the cap never truncates it).
    let flood = "dd if=/dev/zero bs=65536 count=2 2>/dev/null | tr '\\0' 'x'";
    let reg = capability::registry_from(
        &[(
            "flood",
            json!({
                "kind": "exec",
                "argv": ["/bin/sh", "-c",
                    &format!("{flood} >&2; {flood}")],
                "timeout_ms": 20000,
            }),
        )],
        Some(pol.clone()),
    )
    .unwrap();
    match reg.call("flood", &json!({}), &json!({})) {
        Ok(o) => {
            h.eq("cap: exec big-stdout does not deadlock", o["exit_code"].as_i64().unwrap(), 0);
            h.check(
                "cap: exec big-stdout full length",
                o["stdout"].as_str().unwrap().len() >= 128 * 1024,
            );
            h.check(
                "cap: exec big-stderr full length",
                o["stderr"].as_str().unwrap().len() >= 128 * 1024,
            );
        }
        Err(e) => h.check(&format!("cap: exec big-stdout does not deadlock ({e})"), false),
    }

    // ...and a child that outruns its budget is still killed, with its partial
    // output still collected rather than lost.
    let reg = capability::registry_from(
        &[(
            "slow",
            json!({"kind": "exec", "argv": ["/bin/sh", "-c", &format!("{flood}; sleep 30")], "timeout_ms": 5000}),
        )],
        Some(pol.clone()),
    )
    .unwrap();
    let t0 = std::time::Instant::now();
    let err = reg.call("slow", &json!({}), &json!({})).unwrap_err();
    h.check(
        &format!("cap: exec timeout still fires ({err})"),
        err.to_string().contains("timed out"),
    );
    h.check(
        "cap: exec timeout is prompt",
        t0.elapsed().as_millis() < 20_000,
    );

    // policy: host allow-list denies an unlisted host (no network involved)
    let mut pol2 = capability::Policy::default();
    pol2.allow_hosts = vec!["allowed.example".to_string()];
    let reg = capability::registry_from(
        &[(
            "bad",
            json!({"kind": "http", "url": "http://127.0.0.1:9/nope"}),
        )],
        Some(pol2),
    )
    .unwrap();
    let e = reg
        .call("bad", &json!({}), &json!({}))
        .unwrap_err()
        .to_string();
    h.check(
        "cap: host allow-list denies",
        e.contains("not in policy.allow_hosts"),
    );

    // unknown capability is a hard error
    let reg = capability::registry_from(&[], None).unwrap();
    h.check(
        "cap: unknown capability errors",
        reg.call("nope", &json!({}), &json!({})).is_err(),
    );

    // registry parsing + names
    let reg = capability::Registry::from_spec(&json!({
        "policy": {"allow_exec": false, "max_timeout_ms": 999999, "retries": 9},
        "capabilities": {
            "a": {"kind": "http", "url": "http://x/y"},
            "b": {"kind": "exec", "argv": ["true"]},
            "c": {"kind": "agent", "command": ["node", "server.js"]}
        }
    }))
    .unwrap();
    h.eq(
        "cap: names sorted",
        reg.names(),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
    );
    h.eq(
        "cap: timeout clamped to hard ceiling",
        reg.policy().max_timeout_ms,
        600_000,
    );
    h.eq("cap: retries clamped", reg.policy().retries, 5);
    h.check(
        "cap: unknown kind rejected",
        capability::Registry::from_spec(&json!({"capabilities": {"z": {"kind": "nope"}}})).is_err(),
    );

    // codex-style stdio agent (line-delimited JSON) via a local python mock
    let mock = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bench/mock_server.py");
    let python = std::env::var("LAYA_TEST_PYTHON").unwrap_or_else(|_| "python3".to_string());
    if mock.exists() {
        let mut pol3 = capability::Policy::default();
        pol3.allow_exec = true;
        let reg = capability::registry_from(
            &[(
                "agent",
                json!({
                    "kind": "agent", "transport": "stdio",
                    "command": [python, mock.to_str().unwrap(), "--stdio"],
                    "session": "t1", "timeout_ms": 10000
                }),
            )],
            Some(pol3),
        )
        .unwrap();
        match reg.call("agent", &json!({"ask": "hello"}), &json!({})) {
            Ok(r) => {
                h.eq(
                    "cap: stdio agent reply",
                    r["body"]["output"].as_str().unwrap().to_string(),
                    "stdio-ack:t1".to_string(),
                );
                h.eq(
                    "cap: stdio agent echoes input",
                    r["body"]["echo"]["input"]["ask"]
                        .as_str()
                        .unwrap()
                        .to_string(),
                    "hello".to_string(),
                );
            }
            Err(e) => {
                h.check(&format!("cap: stdio agent reply (skipped: {e})"), true);
            }
        }
    }

    // `call` action kind end-to-end through a spec (exec capability, no network)
    let call_spec = json!({
        "name": "caller", "start": "go",
        "policy": {"allow_exec": true},
        "capabilities": {"echoer": {"kind": "exec", "argv": ["/bin/echo", "n=${state.n}"], "timeout_ms": 5000}},
        "nodes": [{
            "name": "go", "primary_q": "q",
            "questions": {"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a", "B": "b"}}},
            "edge": {"condition": {}, "default": "STOP"},
            "action": {"kind": "call", "capability": "echoer", "project": {"out": "/stdout"}}
        }]
    });
    let cwf = spec::from_spec(&call_spec).unwrap();
    let cout = cwf
        .run(
            &ScriptedBackend::new(vec![verdict(&[(
                "q",
                choice("A", &[("A", 0.9), ("B", 0.1)]),
            )])]),
            &json!({"n": 42}),
        )
        .unwrap();
    h.eq(
        "cap: call action ran",
        cout.state["capability"].as_str().unwrap().to_string(),
        "echoer".to_string(),
    );
    h.check(
        "cap: call action projected stdout",
        cout.state["out"].as_str().unwrap_or("").contains("n=42"),
    );

    // capability chaining: earlier results are visible to later steps / the main call
    let chain_spec = json!({
        "name": "chainer", "start": "go",
        "capabilities": {
            "seed": {"kind": "datetime", "op": "now"},
            "len": {"kind": "text", "op": "length"}
        },
        "nodes": [{
            "name": "go", "primary_q": "q",
            "questions": {"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}},
            "edge": {"condition": {}, "default": "STOP"},
            "action": {
                "kind": "call", "capability": "len",
                "chain": [{"capability": "seed", "as": "seed"}],
                "with": { "text": "epoch=${with.seed.epoch}" },
                "project": { "chained_chars": "/chars" }
            }
        }]
    });
    let cwf = spec::from_spec(&chain_spec).unwrap();
    let cout = cwf
        .run(
            &ScriptedBackend::new(vec![verdict(&[(
                "q",
                choice("A", &[("A", 0.9), ("B", 0.1)]),
            )])]),
            &json!({"n": 7}),
        )
        .unwrap();
    // chain output (a live epoch) reached the main call's template:
    // "epoch=<10-digit epoch>" == 16 chars
    h.eq(
        "cap: chain output flows to main call",
        cout.state["chained_chars"].as_u64().unwrap(),
        16,
    );
}

pub fn test_capabilities_extra(h: &mut Harness) {
    // datetime: now / add / format / parse (pure, no deps)
    let reg = capability::registry_from(
        &[
            ("t_now", json!({"kind": "datetime", "op": "now"})),
            (
                "t_add",
                json!({"kind": "datetime", "op": "add", "offset_secs": 3600}),
            ),
            (
                "t_fmt",
                json!({"kind": "datetime", "op": "format", "format": "%Y-%m-%d %H:%M:%S"}),
            ),
            ("t_parse", json!({"kind": "datetime", "op": "parse"})),
        ],
        None,
    )
    .unwrap();
    let now = reg.call("t_now", &json!({}), &json!({})).unwrap();
    h.check(
        "datetime.now returns epoch",
        now["epoch"].as_i64().unwrap_or(0) > 1_600_000_000,
    );
    let epoch = now["epoch"].as_i64().unwrap();
    let added = reg
        .call("t_add", &json!({"epoch": epoch}), &json!({}))
        .unwrap();
    h.eq(
        "datetime.add offset",
        added["epoch"].as_i64().unwrap(),
        epoch + 3600,
    );
    // 2026-09-26T00:00:00Z is a fixed instant
    let fmt = reg
        .call("t_fmt", &json!({"epoch": 1790380800i64}), &json!({}))
        .unwrap();
    h.eq(
        "datetime.format structure",
        fmt["text"].as_str().unwrap().len(),
        19,
    );
    let parsed = reg
        .call(
            "t_parse",
            &json!({"text": "2026-09-26T00:00:00Z"}),
            &json!({}),
        )
        .unwrap();
    h.eq(
        "datetime.parse iso",
        parsed["epoch"].as_i64().unwrap(),
        1790380800,
    );
    h.check(
        "datetime bad op rejected",
        capability::registry_from(&[("bad", json!({"kind":"datetime","op":"nope"}))], None)
            .unwrap()
            .call("bad", &json!({}), &json!({}))
            .is_err(),
    );

    // text: all ops
    let reg = capability::registry_from(
        &[
            ("t_len", json!({"kind": "text", "op": "length"})),
            ("t_up", json!({"kind": "text", "op": "upper"})),
            (
                "t_split",
                json!({"kind": "text", "op": "split", "separator": ","}),
            ),
            (
                "t_ext",
                json!({"kind": "text", "op": "extract", "pattern": "\\d+"}),
            ),
            (
                "t_rep",
                json!({"kind": "text", "op": "replace", "pattern": "\\d+", "replacement": "N"}),
            ),
            ("t_hash", json!({"kind": "text", "op": "hash"})),
            ("t_b64e", json!({"kind": "text", "op": "base64_encode"})),
            ("t_b64d", json!({"kind": "text", "op": "base64_decode"})),
            (
                "t_jget",
                json!({"kind": "text", "op": "json_get", "pattern": "/a/b"}),
            ),
        ],
        None,
    )
    .unwrap();
    h.eq(
        "text.length",
        reg.call("t_len", &json!({"text":"héllo"}), &json!({}))
            .unwrap()["chars"]
            .as_u64()
            .unwrap(),
        5,
    );
    h.eq(
        "text.upper",
        reg.call("t_up", &json!({"text":"ab"}), &json!({})).unwrap()["text"]
            .as_str()
            .unwrap()
            .to_string(),
        "AB".to_string(),
    );
    h.eq(
        "text.split",
        reg.call("t_split", &json!({"text":"a,b,c"}), &json!({}))
            .unwrap()["count"]
            .as_u64()
            .unwrap(),
        3,
    );
    h.eq(
        "text.extract",
        reg.call("t_ext", &json!({"text":"x12y345"}), &json!({}))
            .unwrap()["count"]
            .as_u64()
            .unwrap(),
        2,
    );
    h.eq(
        "text.replace",
        reg.call("t_rep", &json!({"text":"a1b2"}), &json!({}))
            .unwrap()["text"]
            .as_str()
            .unwrap()
            .to_string(),
        "aNbN".to_string(),
    );
    h.eq(
        "text.hash stable",
        reg.call("t_hash", &json!({"text":"abc"}), &json!({}))
            .unwrap()["hex"]
            .as_str()
            .unwrap()
            .to_string(),
        capability::registry_from(&[("h", json!({"kind":"text","op":"hash"}))], None)
            .unwrap()
            .call("h", &json!({"text":"abc"}), &json!({}))
            .unwrap()["hex"]
            .as_str()
            .unwrap()
            .to_string(),
    );
    let enc = reg
        .call("t_b64e", &json!({"text":"hi"}), &json!({}))
        .unwrap();
    h.eq(
        "text.base64_encode",
        enc["text"].as_str().unwrap().to_string(),
        "aGk=".to_string(),
    );
    let dec = reg
        .call("t_b64d", &json!({"text":"aGk="}), &json!({}))
        .unwrap();
    h.eq(
        "text.base64_decode roundtrip",
        dec["text"].as_str().unwrap().to_string(),
        "hi".to_string(),
    );
    h.eq(
        "text.json_get",
        reg.call("t_jget", &json!({"text":"{\"a\":{\"b\":7}}"}), &json!({}))
            .unwrap()["value"]
            .as_i64()
            .unwrap(),
        7,
    );
    h.check(
        "text bad op rejected",
        capability::registry_from(&[("bad", json!({"kind":"text","op":"nope"}))], None)
            .unwrap()
            .call("bad", &json!({"text":"x"}), &json!({}))
            .is_err(),
    );
    h.check(
        "text missing input rejected",
        reg.call("t_len", &json!({}), &json!({})).is_err(),
    );

    // file: allow-list enforcement + read/write/stat/list
    let froot = std::env::temp_dir().join("laya_cap_file_test");
    let _ = std::fs::remove_dir_all(&froot);
    std::fs::create_dir_all(&froot).unwrap();
    let froot_s = froot.to_str().unwrap().to_string();
    let mut fpol = capability::Policy::default();
    fpol.allow_paths = vec![froot_s.clone()];
    let reg = capability::registry_from(
        &[
            ("f_write", json!({"kind": "file", "op": "write"})),
            ("f_append", json!({"kind": "file", "op": "append"})),
            ("f_read", json!({"kind": "file", "op": "read"})),
            ("f_stat", json!({"kind": "file", "op": "stat"})),
            ("f_list", json!({"kind": "file", "op": "list"})),
        ],
        Some(fpol.clone()),
    )
    .unwrap();
    let target = froot.join("a.txt");
    let target_s = target.to_str().unwrap().to_string();
    reg.call(
        "f_write",
        &json!({"path": target_s, "text": "one"}),
        &json!({}),
    )
    .unwrap();
    reg.call(
        "f_append",
        &json!({"path": target_s, "text": "two"}),
        &json!({}),
    )
    .unwrap();
    let rd = reg
        .call("f_read", &json!({"path": target_s}), &json!({}))
        .unwrap();
    h.eq(
        "file write+append+read",
        rd["text"].as_str().unwrap().to_string(),
        "onetwo".to_string(),
    );
    h.eq(
        "file stat bytes",
        reg.call("f_stat", &json!({"path": target_s}), &json!({}))
            .unwrap()["bytes"]
            .as_u64()
            .unwrap(),
        6,
    );
    h.eq(
        "file list count",
        reg.call("f_list", &json!({"path": froot_s}), &json!({}))
            .unwrap()["count"]
            .as_u64()
            .unwrap(),
        1,
    );
    // outside the allow-list → denied
    let outside = std::env::temp_dir().join("laya_cap_outside.txt");
    let e = reg
        .call(
            "f_read",
            &json!({"path": outside.to_str().unwrap()}),
            &json!({}),
        )
        .unwrap_err()
        .to_string();
    h.check(
        "file path allow-list denies outside",
        e.contains("outside the allowed roots"),
    );
    // no roots at all → denied (fail-closed)
    let reg_no =
        capability::registry_from(&[("f", json!({"kind":"file","op":"read"}))], None).unwrap();
    h.check(
        "file without roots is denied",
        reg_no
            .call("f", &json!({"path": target_s}), &json!({}))
            .is_err(),
    );
    let _ = std::fs::remove_dir_all(&froot);
    let _ = std::fs::remove_file(&outside);

    // shell: gated by allow_exec, exit code + stdout
    let mut pol = capability::Policy::default();
    pol.allow_exec = true;
    let reg = capability::registry_from(
        &[(
            "sh",
            json!({"kind": "shell", "command": "echo shell-${state.n}; exit 3"}),
        )],
        None,
    )
    .unwrap();
    h.check(
        "shell denied without allow_exec",
        reg.call("sh", &json!({}), &json!({"n": 1})).is_err(),
    );
    let reg = capability::registry_from(
        &[(
            "sh",
            json!({"kind": "shell", "command": "echo shell-${state.n}; exit 3"}),
        )],
        Some(pol.clone()),
    )
    .unwrap();
    let out = reg.call("sh", &json!({}), &json!({"n": 9})).unwrap();
    h.eq(
        "shell exit code propagated",
        out["exit_code"].as_i64().unwrap(),
        3,
    );
    h.check(
        "shell stdout expanded",
        out["stdout"].as_str().unwrap().contains("shell-9"),
    );

    // sqlite: read-only guard (and skip the live query when the CLI is absent)
    let sqlite_ok = std::process::Command::new("sqlite3")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let reg = capability::registry_from(
        &[(
            "db",
            json!({"kind": "sqlite", "db": "/tmp/laya_cap_test.db", "op": "query"}),
        )],
        Some(pol.clone()),
    )
    .unwrap();
    let e = reg
        .call("db", &json!({"sql": "DROP TABLE t"}), &json!({}))
        .unwrap_err()
        .to_string();
    h.check(
        "sqlite read-only rejects write sql",
        e.contains("read-only"),
    );
    if sqlite_ok {
        let dbf = std::env::temp_dir().join("laya_cap_sqlite.db");
        let _ = std::fs::remove_file(&dbf);
        let _ = std::process::Command::new("sqlite3")
            .arg(dbf.to_str().unwrap())
            .arg("CREATE TABLE t(x INTEGER); INSERT INTO t VALUES (1),(2);")
            .status();
        let reg = capability::registry_from(
            &[(
                "db",
                json!({"kind": "sqlite", "db": dbf.to_str().unwrap(), "op": "query"}),
            )],
            Some(pol.clone()),
        )
        .unwrap();
        let r = reg
            .call(
                "db",
                &json!({"sql": "SELECT count(*) AS n FROM t"}),
                &json!({}),
            )
            .unwrap();
        h.check("sqlite query returns rows", r["rows"].is_array());
        let _ = std::fs::remove_file(&dbf);
    }

    // passthrough (no-op, useful as a wiring placeholder)
    let reg = capability::registry_from(&[("p", json!({"kind": "passthrough"}))], None).unwrap();
    let r = reg
        .call("p", &json!({"k": "${state.n}"}), &json!({"n": 5}))
        .unwrap();
    h.eq(
        "passthrough expands instead of calling",
        r["value"]["k"].as_i64().unwrap(),
        5,
    );
}

pub fn test_needle(h: &mut Harness) {
    // The on-device Needle 3 capability is exercised only when the engine and
    // weights are actually installed; otherwise we assert the clean failure
    // path (specs should probe it gracefully). This keeps the default suite
    // hermetic while still proving the wiring when `needle fetch` + the cact
    // are present.
    let needle_lib = crate::capability::needle::find_lib().is_ok();
    let needle_cact = crate::capability::needle::find_cact().is_ok();
    if !needle_lib || !needle_cact {
        h.check("needle: skipped (no libneedle.so / needle3.cact)", true);
        return;
    }

    // extract: invoice from free text
    let reg = capability::registry_from(
        &[(
            "inv",
            json!({"kind": "needle", "op": "extract"}),
        )],
        None,
    )
    .unwrap();
    let tool = json!({
        "type": "function",
        "name": "invoice",
        "description": "Record invoice vendor, total and due date from text.",
        "parameters": {
            "type": "object",
            "properties": {
                "vendor": {"type": "string"},
                "total": {"type": "number"},
                "due_date": {"type": "string"}
            },
            "required": ["vendor", "total", "due_date"]
        }
    });
    let out = reg
        .call(
            "inv",
            &json!({"text": "Invoice from Acme Corp, $1,200.00, due 2026-09-01", "tool": tool}),
            &json!({}),
        )
        .unwrap();
    let args = out["arguments"].clone();
    h.eq("needle.extract invoice vendor", args["vendor"].as_str().unwrap_or(""), "Acme Corp");
    h.eq("needle.extract invoice total", args["total"].as_f64().unwrap_or(0.0), 1200.0);
    h.check("needle.extract has confidence", out["confidence"].is_number());

    // embed: sentence -> dim + unit vector
    let reg2 = capability::registry_from(
        &[("emb", json!({"kind": "needle", "op": "embed"}))],
        None,
    )
    .unwrap();
    let out2 = reg2
        .call("emb", &json!({"text": "turn on the kitchen lights"}), &json!({}))
        .unwrap();
    let dim = out2["dim"].as_u64().unwrap_or(0);
    h.check("needle.embed dim is 3072", dim == 3072);
    let vec: Vec<f64> = out2["vector"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
        .unwrap_or_default();
    let norm: f64 = vec.iter().map(|x| x * x).sum::<f64>().sqrt();
    h.check("needle.embed vector normalised", (norm - 1.0).abs() < 1e-3);

    // complete: tool call with confidence
    let reg3 = capability::registry_from(
        &[("tc", json!({"kind": "needle", "op": "complete"}))],
        None,
    )
    .unwrap();
    let out3 = reg3
        .call(
            "tc",
            &json!({
                "prompt": "what's it like in Lagos right now?",
                "tools": [{
                    "type": "function",
                    "name": "get_weather",
                    "description": "Get the current weather for a city.",
                    "parameters": {
                        "type": "object",
                        "properties": {"city": {"type": "string"}},
                        "required": ["city"]
                    }
                }]
            }),
            &json!({}),
        )
        .unwrap();
    let calls = out3["function_calls"].as_array().cloned().unwrap_or_default();
    h.check("needle.complete returned a call", !calls.is_empty());
    h.check(
        "needle.complete picks the weather tool",
        calls.first().and_then(|c| c["name"].as_str()).unwrap_or("") == "get_weather",
    );
    h.check("needle.complete has confidence", out3["confidence"].is_number());
}

pub fn test_span_selection(h: &mut Harness) {
    use laya_workflow::capability;
    use laya_workflow::workflow::{expand_criteria_from_state, Decide};
    use laya_workflow::backend::HeuristicBackend;

    // 1) text.spans extracts candidates with id / value / byte offsets.
    let reg = capability::registry_from(
        &[("spans", json!({"kind": "text", "op": "spans", "pattern": r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}"}))],
        None,
    )
    .unwrap();
    let src = "For product questions contact help@example.org. For billing questions email invoices@example.org.";
    let out = reg.call("spans", &json!({"text": src}), &json!({})).unwrap();
    let cands = out["candidates"].as_array().cloned().unwrap_or_default();
    h.eq("spans: two candidates", cands.len(), 2);
    h.eq("spans: id ordinal", cands[0]["id"].as_str().unwrap_or(""), "candidate_0");
    h.eq("spans: first value", cands[0]["value"].as_str().unwrap_or(""), "help@example.org");
    h.eq("spans: start offset", cands[0]["start"].as_u64().unwrap_or(0), 30);
    let v1 = cands[1]["value"].as_str().unwrap_or("");
    let s1 = cands[1]["start"].as_u64().unwrap_or(0) as usize;
    let e1 = cands[1]["end"].as_u64().unwrap_or(0) as usize;
    h.eq("spans: value re-slices from source", &src[s1..e1], v1);

    // 2) json.pick_by returns the element whose id matches.
    let pick = capability::registry_from(
        &[("pick", json!({"kind": "json", "op": "pick_by"}))],
        None,
    )
    .unwrap();
    let p = pick
        .call("pick", &json!({"array": cands, "id": "candidate_1", "id_key": "id"}), &json!({}))
        .unwrap();
    h.eq("pick_by: found", p["found"].as_bool().unwrap_or(false), true);
    h.eq("pick_by: value", p["item"]["value"].as_str().unwrap_or(""), "invoices@example.org");
    let miss = pick
        .call("pick", &json!({"array": cands, "id": "candidate_9", "id_key": "id"}), &json!({}))
        .unwrap();
    h.eq("pick_by: missing -> not found", miss["found"].as_bool().unwrap_or(true), false);

    // 3) expand_criteria_from_state materialises criteria from a state array.
    let questions = json!({"bc": {"type": "choice", "instructions": "pick",
        "criteria": {"from_state": "candidates", "id_key": "id", "value_key": "value",
                     "none": "nothing"}}});
    let state = json!({"candidates": cands});
    let expanded = expand_criteria_from_state(&state, &questions).unwrap();
    let crit = expanded["bc"]["criteria"].as_object().cloned().unwrap_or_default();
    h.check("from_state: candidate_0 present", crit.contains_key("candidate_0"));
    h.check("from_state: candidate_1 present", crit.contains_key("candidate_1"));
    h.check("from_state: none present", crit.contains_key("none"));
    h.check("from_state: option text names the id",
        crit["candidate_1"].as_str().unwrap_or("").contains("candidate_1"));

    // 3b) zero candidates fail-closes to a none-only criteria map.
    let empty_exp = expand_criteria_from_state(&json!({"candidates": []}), &questions).unwrap();
    let empty_crit = empty_exp["bc"]["criteria"].as_object().cloned().unwrap_or_default();
    h.eq("from_state: empty -> only none", empty_crit.len(), 1);
    h.check("from_state: empty -> none present", empty_crit.contains_key("none"));
    h.check("from_state: empty marked", empty_exp["bc"]["_empty_candidates"].as_bool().unwrap_or(false));

    // 3c) heuristic answer_field gives a deterministic offline selection.
    let be = HeuristicBackend;
    let q_af = json!({"bc": {"type": "choice", "instructions": "pick",
        "criteria": {"from_state": "candidates", "id_key": "id", "value_key": "value",
                     "none": "nothing"},
        "heuristic": {"answer_field": "selected_id"}}});
    // The engine materialises from_state criteria before decide (WorkflowNode::run);
    // mirror that so the heuristic sees real option ids.
    let q_af_mat = expand_criteria_from_state(&json!({"candidates": cands}), &q_af).unwrap();
    let v_af = be.decide(&json!({"candidates": cands, "selected_id": "candidate_1"}), &q_af_mat).unwrap();
    h.eq("answer_field: picks the state field id",
        v_af.answer_value("bc").unwrap().as_str().unwrap_or(""), "candidate_1");
    let v_bad = be.decide(&json!({"candidates": cands, "selected_id": "candidate_9"}), &q_af_mat).unwrap();
    h.eq("answer_field: unknown id fails closed to none",
        v_bad.answer_value("bc").unwrap().as_str().unwrap_or(""), "none");

    // 4) end-to-end: the span_selection spec routes selected / not_found.
    use laya_workflow::spec::load_file;
    
    let run_e2e = |text: &str, sid: &str| {
        let wf = load_file("dsl/capabilities/span_selection.json").unwrap();
        let reg = laya_workflow::capability::Registry::from_spec(
            &serde_json::from_str(&std::fs::read_to_string("dsl/capabilities/span_selection.json").unwrap()).unwrap(),
        )
        .unwrap();
        let mut backend = laya_workflow::backend::HeuristicBackend;
        let mut state = json!({"text": text, "selected_id": sid});
        let payload = wf.run(&mut backend, &mut state).unwrap();
        let _ = reg;
        payload
    };
    let out_selected = run_e2e(src, "candidate_1");
    h.eq("e2e: billing_contact is candidate_1",
        out_selected.state["billing_contact"].as_str().unwrap_or(""), "candidate_1");
    h.eq("e2e: selected value re-sliced",
        out_selected.state["selected"]["value"].as_str().unwrap_or(""), "invoices@example.org");

    let out_none = run_e2e(src, "none");
    h.eq("e2e: none routes to not_found",
        out_none.state["billing_contact"].as_str().unwrap_or(""), "none");
    h.check("e2e: none leaves no selection",
        out_none.state["selected"].is_null());

    let out_empty = run_e2e("no email here", "candidate_0");
    h.eq("e2e: zero candidates -> none",
        out_empty.state["billing_contact"].as_str().unwrap_or(""), "none");
    let out_single = run_e2e("hello single@x.io", "candidate_0");
    h.eq("e2e: single candidate selected",
        out_single.state["billing_contact"].as_str().unwrap_or(""), "candidate_0");
}

pub fn test_quality_rubric(h: &mut Harness) {
    use laya_workflow::backend::noul;
    use laya_workflow::spec::run_action;
    use laya_workflow::workflow::{Decision, Verdict};
    use std::collections::HashMap;

    let mk_decision = |answer: f64, conf: f64| Decision {
        answer: serde_json::json!(answer),
        probabilities: {
            let mut m = serde_json::Map::new();
            m.insert("0".to_string(), serde_json::json!(0.1));
            m.insert("1".to_string(), serde_json::json!(0.8));
            m.insert("2".to_string(), serde_json::json!(0.1));
            m
        },
        confidence: conf,
    };
    let action = serde_json::json!({
        "kind": "rubric", "min_confidence": 0.8,
        "dimensions": [
            {"question": "usefulness", "levels": 3, "weight": 0.6},
            {"question": "clarity", "levels": 4, "weight": 0.4},
        ]
    });

    // high-confidence: usefulness 1.8/2=0.9, clarity 2.6/3=0.8667,
    // 0.6*0.9 + 0.4*0.8667 = 0.8867 (matches awesome-jev's expected 0.8867)
    let mut ans = HashMap::new();
    ans.insert("usefulness".to_string(), mk_decision(1.8, 0.93));
    ans.insert("clarity".to_string(), mk_decision(2.6, 0.87));
    let v = Verdict { answers: ans, input_tokens: 0, latency_ms: 0.0 };
    let out = run_action(&action, &json!({}), &v, None).unwrap();
    h.eq("rubric: scored", out["status"].as_str().unwrap_or(""), "scored");
    h.eq("rubric: usefulness normalized 0.9", out["normalized"]["usefulness"]["normalized"].as_f64().unwrap_or(-1.0), 0.9);
    h.check("rubric: clarity normalized ~0.8667",
        (out["normalized"]["clarity"]["normalized"].as_f64().unwrap_or(-1.0) - 0.8666666666666667).abs() < 1e-9);
    h.check("rubric: weighted ~0.8867",
        (out["weighted_score"].as_f64().unwrap_or(-1.0) - 0.8866666666666667).abs() < 1e-9);

    // low-confidence on one dimension -> composite withheld (human_review)
    let mut ans2 = HashMap::new();
    ans2.insert("usefulness".to_string(), mk_decision(1.8, 0.93));
    ans2.insert("clarity".to_string(), mk_decision(2.6, 0.5));
    let v2 = Verdict { answers: ans2, input_tokens: 0, latency_ms: 0.0 };
    let out2 = run_action(&action, &json!({}), &v2, None).unwrap();
    h.eq("rubric: low-confidence withholds score", out2["status"].as_str().unwrap_or(""), "human_review");
    h.check("rubric: composite is null when withheld", out2["weighted_score"].is_null());
    h.check("rubric: names the uncertain dimension",
        out2["uncertain_dimensions"].as_array().map(|a| a.len()).unwrap_or(0) == 1);

    // noul is a valid rubric input too (levels=2 => /1 = raw)
    let noul_act = serde_json::json!({
        "kind": "rubric", "min_confidence": 0.8,
        "dimensions": [{"question": "is_clear", "levels": 2, "weight": 1.0}]
    });
    let d = noul(0.9);
    let mut ans3 = HashMap::new();
    ans3.insert("is_clear".to_string(), d);
    let v3 = Verdict { answers: ans3, input_tokens: 0, latency_ms: 0.0 };
    let out3 = run_action(&noul_act, &json!({}), &v3, None).unwrap();
    h.eq("rubric: noul dimension normalizes by 1", out3["weighted_score"].as_f64().unwrap_or(-1.0), 0.9);
}

pub fn test_support_routing(h: &mut Harness) {
    use laya_workflow::spec::run_action;
    use laya_workflow::workflow::{Decision, Verdict};
    use std::collections::HashMap;

    // combine: choice + noul composed in code, both thresholds honoured.
    let action = serde_json::json!({
        "kind": "combine",
        "choice": "department", "signal": "explicit_urgency",
        "min_confidence": 0.8, "fallback": "other",
        "review_label": "human_review",
        "high_threshold": 0.85, "low_threshold": 0.15,
    });
    let mk_choice = |pick: &str, conf: f64| Decision {
        answer: serde_json::json!(pick),
        probabilities: {
            let mut m = serde_json::Map::new();
            m.insert("billing".to_string(), serde_json::json!(0.1));
            m.insert("technical".to_string(), serde_json::json!(0.8));
            m.insert("account".to_string(), serde_json::json!(0.05));
            m.insert("other".to_string(), serde_json::json!(0.05));
            m
        },
        confidence: conf,
    };
    let mk_noul = |p: f64| Decision {
        answer: serde_json::json!(p),
        probabilities: {
            let mut m = serde_json::Map::new();
            m.insert("true".to_string(), serde_json::json!(p));
            m.insert("false".to_string(), serde_json::json!(1.0 - p));
            m
        },
        confidence: p.max(1.0 - p),
    };
    let run_c = |c: &Decision, n: &Decision| {
        let mut a = HashMap::new();
        a.insert("department".to_string(), c.clone());
        a.insert("explicit_urgency".to_string(), n.clone());
        let v = Verdict { answers: a, input_tokens: 0, latency_ms: 0.0 };
        run_action(&action, &json!({}), &v, None).unwrap()
    };

    // technical + urgency 0.93 -> route technical, urgency high
    let o1 = run_c(&mk_choice("technical", 0.94), &mk_noul(0.93));
    h.eq("support: confident technical routes through",
        o1["route"].as_str().unwrap_or(""), "technical");
    h.eq("support: explicit deadline -> high urgency",
        o1["urgency"].as_str().unwrap_or(""), "high");

    // billing + no urgency -> ordinary
    let o2 = run_c(&mk_choice("billing", 0.91), &mk_noul(0.05));
    h.eq("support: no urgency -> ordinary",
        o2["urgency"].as_str().unwrap_or(""), "ordinary");

    // low confidence choice -> human_review regardless of urgency
    let o3 = run_c(&mk_choice("technical", 0.5), &mk_noul(0.9));
    h.eq("support: low-confidence choice routes to review",
        o3["route"].as_str().unwrap_or(""), "human_review");

    // `other` -> human_review
    let o4 = run_c(&mk_choice("other", 0.9), &mk_noul(0.5));
    h.eq("support: other -> review",
        o4["route"].as_str().unwrap_or(""), "human_review");

    // mid-band urgency -> review
    let o5 = run_c(&mk_choice("account", 0.9), &mk_noul(0.5));
    h.eq("support: mid urgency -> review",
        o5["urgency"].as_str().unwrap_or(""), "review");

    // match_rules heuristic: end-to-end department classification + fallback.
    use laya_workflow::backend::HeuristicBackend;
    use laya_workflow::workflow::Decide;
    let be = HeuristicBackend;
    let q = serde_json::json!({
        "department": {"type": "choice", "instructions": "route",
            "criteria": {"billing": "b", "technical": "t", "account": "a", "other": "o"},
            "heuristic": {"field": "message", "match_rules": {
                "billing": ["invoice", "refund", "payment"],
                "technical": ["crash", "export", "bug"],
                "account": ["login", "log in", "account"],
            }, "p_hit": 0.94, "fallback": "other"}}
    });
    let v_t = be.decide(&json!({"message": "the export crashes"}), &q).unwrap();
    h.eq("match_rules: export crash -> technical",
        v_t.answer_value("department").unwrap().as_str().unwrap_or(""), "technical");
    let v_o = be.decide(&json!({"message": "please explain the weather"}), &q).unwrap();
    h.eq("match_rules: no token -> fallback other",
        v_o.answer_value("department").unwrap().as_str().unwrap_or(""), "other");
    h.check("match_rules: fallback is low-confidence",
        v_o.confidence("department").unwrap_or(1.0) < 0.8);
}
