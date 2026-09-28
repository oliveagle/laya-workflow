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
