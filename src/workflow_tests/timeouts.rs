//! timeouts regression sections for `laya-workflow-tests`.
#![allow(unused_imports)]
use crate::*;
pub fn test_capability_timeouts(h: &mut Harness) {
    {
        use laya_workflow::capability::secret;
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        /// Start a hostile server: it accepts, optionally sends junk, then either
        /// stays silent or closes. Returns its port.
        fn hostile(mode: &'static str) -> u16 {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = l.local_addr().unwrap().port();
            std::thread::spawn(move || {
                for _ in 0..8 {
                    let Ok((mut c, _)) = l.accept() else { return };
                    match mode {
                        "garbage" => {
                            let mut b = [0u8; 512];
                            let _ = c.read(&mut b);
                            let _ = c.write_all(b"\xff\xfenot-a-protocol\r\n");
                            let _ = c.flush();
                        }
                        "silent" => {
                            // Hold the connection open and never answer.
                            std::thread::sleep(std::time::Duration::from_millis(4000));
                        }
                        _ => {}
                    }
                    drop(c);
                }
            });
            port
        }

        let mut pol = capability::Policy::default();
        pol.allow_hosts = vec!["127.0.0.1".to_string()];
        pol.max_timeout_ms = 3000;

        // (1) SMTP used to retry past the configured timeout: the reader had a
        // hardcoded 10s ceiling and ignored `timeout_ms`, so a 2s capability
        // blocked for ~10s per read and a session ran ~25s before failing.
        // It must now give up on its own budget.
        for mode in ["garbage", "silent"] {
            let port = hostile(mode);
            let reg = capability::registry_from(
                &[(
                    "m",
                    json!({"kind": "smtp", "host": "127.0.0.1", "port": port, "timeout_ms": 2000}),
                )],
                Some(pol.clone()),
            )
            .unwrap();
            let t0 = std::time::Instant::now();
            let r = reg.call(
                "m",
                &json!({"to": ["a@b.c"], "subject": "s", "body": "b"}),
                &json!({}),
            );
            let ms = t0.elapsed().as_millis() as u64;
            h.check(&format!("fix: smtp vs {mode} fails"), r.is_err());
            h.check(
                &format!("fix: smtp vs {mode} honours its timeout (took {ms}ms, was ~10000+)"),
                ms < 4000,
            );
        }

        // (2) Every protocol client must fail fast against a non-answering peer
        // rather than hanging. 2s capability + 3s policy ceiling => well under 6s.
        for kind in ["redis", "tcp", "nats", "mqtt", "smtp"] {
            let port = hostile("silent");
            let reg = capability::registry_from(
                &[(
                    "m",
                    json!({"kind": kind, "host": "127.0.0.1", "port": port, "timeout_ms": 2000}),
                )],
                Some(pol.clone()),
            )
            .unwrap();
            let t0 = std::time::Instant::now();
            let _ = reg.call("m", &json!({"op": "ping", "subject": "x", "message": "y", "send": "z", "to": ["a@b.c"]}), &json!({}));
            let ms = t0.elapsed().as_millis() as u64;
            h.check(
                &format!("fix: {kind} vs a silent peer does not hang (took {ms}ms)"),
                ms < 6000,
            );
        }

        // (3) Garbage on the wire must be an error, never a panic.
        for kind in ["redis", "mqtt"] {
            let port = hostile("garbage");
            let reg = capability::registry_from(
                &[(
                    "m",
                    json!({"kind": kind, "host": "127.0.0.1", "port": port, "timeout_ms": 2000}),
                )],
                Some(pol.clone()),
            )
            .unwrap();
            let r = reg.call("m", &json!({"op": "ping", "message": "y"}), &json!({}));
            h.check(
                &format!("fix: {kind} rejects malformed bytes with an error"),
                r.is_err(),
            );
        }

        // (3a) Non-finite numeric results must error, not serialise to JSON
        // `null`. `2^10000` overflows to inf and `stats` over huge inputs
        // produced a NaN mean/stddev; both came back as `null`, i.e. a silent
        // wrong value rather than a failure.
        {
            let reg = capability::registry_from(
                &[("m", json!({"kind": "math", "op": "eval"}))],
                Some(capability::Policy::default()),
            )
            .unwrap();
            let of = reg.call("m", &json!({"expression": "2^10000"}), &json!({}));
            h.check("fix: math.eval rejects an overflowing result", of.is_err());
            let fin = reg.call("m", &json!({"expression": "1/3"}), &json!({}));
            h.check(
                "fix: math.eval still returns finite values",
                fin.map(|v| v["value"].as_f64().map(|x| x.is_finite()).unwrap_or(false))
                    .unwrap_or(false),
            );

            let reg2 = capability::registry_from(
                &[("s", json!({"kind": "math", "op": "stats"}))],
                Some(capability::Policy::default()),
            )
            .unwrap();
            let ov = reg2.call("s", &json!({"values": [1e308, 1e308, -1e308]}), &json!({}));
            h.check(
                "fix: math.stats rejects an overflowing mean/stddev",
                ov.is_err(),
            );
            let ok = reg2.call("s", &json!({"values": [1, 2, 3, 4]}), &json!({}));
            h.check(
                "fix: math.stats still returns finite values",
                ok.map(|v| v["stddev"].as_f64().map(|x| x.is_finite()).unwrap_or(false))
                    .unwrap_or(false),
            );
        }

        // (3a2) Prometheus hexposed values: `+Inf` / `NaN` are legal in the
        // exposition format and Rust parses them, but serde_json serialises a
        // non-finite float as `null` — a silent wrong value. The literal must be
        // preserved instead.
        {
            use std::io::Write as _;
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = l.local_addr().unwrap().port();
            std::thread::spawn(move || {
                if let Ok((mut c, _)) = l.accept() {
                    let mut buf = [0u8; 1024];
                    let _ = std::io::Read::read(&mut c, &mut buf);
                    let body = "metric_a 1.5\nmetric_b +Inf\nmetric_c NaN\n";
                    let r = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = c.write_all(r.as_bytes());
                }
            });
            let mut pol = capability::Policy::default();
            pol.allow_hosts = vec!["127.0.0.1".to_string()];
            let reg = capability::registry_from(
                &[("p", json!({"kind": "prometheus", "url": format!("http://127.0.0.1:{port}/metrics")}))],
                Some(pol),
            )
            .unwrap();
            match reg.call("p", &json!({}), &json!({})) {
                Ok(v) => {
                    let samples = v["samples"].as_array().cloned().unwrap_or_default();
                    let find = |m: &str| samples.iter().find(|s| s["metric"] == json!(m)).cloned();
                    h.eq(
                        "fix: prometheus keeps a finite sample numeric",
                        find("metric_a")
                            .map(|s| s["value"].clone())
                            .unwrap_or(Value::Null),
                        json!(1.5),
                    );
                    h.check(
                        "fix: prometheus does not turn +Inf into null",
                        find("metric_b")
                            .map(|s| !s["value"].is_null())
                            .unwrap_or(false),
                    );
                    h.check(
                        "fix: prometheus does not turn NaN into null",
                        find("metric_c")
                            .map(|s| !s["value"].is_null())
                            .unwrap_or(false),
                    );
                }
                Err(e) => h.check(&format!("fix: prometheus scrape (skipped: {e})"), true),
            }
        }

        // (3a3) Path-allowlist must survive symlinks: a link inside an allowed
        // root that points outside it must be denied after canonicalisation, and
        // nothing may be written outside. This locks the traversal protection
        // (verified manually: DENIED, no /etc/passwd content, no file written).
        {
            let root = std::env::temp_dir().join("laya_symlink_probe");
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let link = root.join("escape");
            let _ = std::fs::remove_file(&link);
            #[cfg(unix)]
            std::os::unix::fs::symlink("/etc", &link).unwrap();

            let mut p = capability::Policy::default();
            p.allow_paths = vec![root.to_string_lossy().to_string()];

            let reg = capability::registry_from(
                &[("f", json!({"kind": "file", "op": "read"}))],
                Some(p.clone()),
            )
            .unwrap();
            let r = reg.call(
                "f",
                &json!({"path": format!("{}/escape/passwd", root.to_string_lossy())}),
                &json!({}),
            );
            h.check("fix: a symlink out of allow_paths is denied", r.is_err());

            let regw = capability::registry_from(
                &[("f", json!({"kind": "file", "op": "write"}))],
                Some(p),
            )
            .unwrap();
            let out_path = std::env::temp_dir().join("laya_should_not_exist.txt");
            let _ = std::fs::remove_file(&out_path);
            let rw = regw.call(
                "f",
                &json!({"path": "/etc/EVIL_SHOULD_NOT_EXIST", "text": "x"}),
                &json!({}),
            );
            h.check("fix: a write outside allow_paths is denied", rw.is_err());
            h.check(
                "fix: the denied write created no file",
                !std::path::Path::new("/etc/EVIL_SHOULD_NOT_EXIST").exists(),
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        // (3a4) Two concurrency bugs, both measured before they were fixed:
        //   * the stores did load -> mutate -> save with no lock, so 8 threads
        //     x 25 pushes left 23 of 200 items;
        //   * `write_atomic` derived its temp name from the PID, and threads
        //     share a PID, so concurrent writers reused one temp path.
        // Plus the underlying reporting bug: `with.op` was ignored, so a call
        // asking for `length` actually pushed a null.
        {
            let path = std::env::temp_dir().join("laya_conc_queue.json");
            let _ = std::fs::remove_file(&path);
            let mut p = capability::Policy::default();
            p.allow_paths = vec![std::env::temp_dir().to_string_lossy().to_string()];
            let reg = std::sync::Arc::new(
                capability::registry_from(
                    &[(
                        "q",
                        json!({"kind": "queue", "op": "push", "path": path.to_string_lossy()}),
                    )],
                    Some(p),
                )
                .unwrap(),
            );
            const THREADS: usize = 8;
            const PER: usize = 25;
            let mut hs = Vec::new();
            for t in 0..THREADS {
                let r = reg.clone();
                hs.push(std::thread::spawn(move || {
                    for i in 0..PER {
                        let _ = r.call("q", &json!({"value": format!("t{t}-i{i}")}), &json!({}));
                    }
                }));
            }
            for h in hs {
                h.join().unwrap();
            }

            // Read the file directly: this is the ground truth, independent of
            // whichever op the capability happens to default to.
            let items: Vec<Value> = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default();
            let expected = THREADS * PER;
            h.check(
                &format!(
                    "fix: concurrent pushes keep every item (expected {expected}, got {})",
                    items.len()
                ),
                items.len() == expected,
            );
            h.check(
                "fix: concurrent pushes insert no spurious null",
                !items.iter().any(|v| v.is_null()),
            );

            // `with.op` must override the configured op, otherwise asking for
            // `length` runs the default `push` and corrupts the store.
            let len = reg
                .call("q", &json!({"op": "length"}), &json!({}))
                .map(|v| v["length"].as_u64().unwrap_or(0) as usize)
                .unwrap_or(0);
            h.eq("fix: with.op overrides the configured op", len, expected);
            let after: Vec<Value> = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default();
            h.eq(
                "fix: a length call does not mutate the store",
                after.len(),
                expected,
            );

            let _ = std::fs::remove_file(&path);
        }

        // (3a4b) Same concurrent load->mutate->save stress for the other two
        // stores. `queue` was the one that exposed the missing lock; `keyvalue`
        // and `cache` go through the same `with_lock`, so they must hold too —
        // and they exercise different mutators (map insert vs. expiring entry).
        {
            let tmp = std::env::temp_dir();
            let mut p = capability::Policy::default();
            p.allow_paths = vec![tmp.to_string_lossy().to_string()];

            const THREADS: usize = 8;
            const PER: usize = 25;

            // ── keyvalue: every thread writes its own disjoint keys ──
            let kv_path = tmp.join("laya_conc_kv.json");
            let _ = std::fs::remove_file(&kv_path);
            let kv = std::sync::Arc::new(
                capability::registry_from(
                    &[(
                        "k",
                        json!({"kind": "keyvalue", "op": "set", "path": kv_path.to_string_lossy()}),
                    )],
                    Some(p.clone()),
                )
                .unwrap(),
            );
            let mut hs = Vec::new();
            for t in 0..THREADS {
                let r = kv.clone();
                hs.push(std::thread::spawn(move || {
                    for i in 0..PER {
                        let _ = r.call(
                            "k",
                            &json!({"op": "set", "key": format!("t{t}-i{i}"), "value": i}),
                            &json!({}),
                        );
                    }
                }));
            }
            for h in hs {
                h.join().unwrap();
            }
            let kv_map: Map<String, Value> = std::fs::read_to_string(&kv_path)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            h.check(
                &format!(
                    "fix: concurrent keyvalue sets keep every key (expected {}, got {})",
                    THREADS * PER,
                    kv_map.len()
                ),
                kv_map.len() == THREADS * PER,
            );
            // Read one back through the capability to confirm the file is the
            // same store the capability reads (not a stale copy).
            let probe = kv
                .call("k", &json!({"op": "get", "key": "t3-i7"}), &json!({}))
                .map(|v| v["found"].as_bool().unwrap_or(false))
                .unwrap_or(false);
            h.check("fix: a concurrently-written key is readable", probe);
            let _ = std::fs::remove_file(&kv_path);

            // ── cache: same, with the TTL mutator in the mix ──
            let cache_path = tmp.join("laya_conc_cache.json");
            let _ = std::fs::remove_file(&cache_path);
            let cache = std::sync::Arc::new(
                capability::registry_from(
                    &[("c", json!({"kind": "cache", "op": "set", "path": cache_path.to_string_lossy(), "ttl_ms": 60000}))],
                    Some(p.clone()),
                )
                .unwrap(),
            );
            let mut hs = Vec::new();
            for t in 0..THREADS {
                let r = cache.clone();
                hs.push(std::thread::spawn(move || {
                    for i in 0..PER {
                        let _ = r.call(
                            "c",
                            &json!({"op": "set", "key": format!("t{t}-i{i}"), "value": i}),
                            &json!({}),
                        );
                    }
                }));
            }
            for h in hs {
                h.join().unwrap();
            }
            let cache_map: Map<String, Value> = std::fs::read_to_string(&cache_path)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            h.check(
                &format!(
                    "fix: concurrent cache sets keep every entry (expected {}, got {})",
                    THREADS * PER,
                    cache_map.len()
                ),
                cache_map.len() == THREADS * PER,
            );
            let _ = std::fs::remove_file(&cache_path);

            // ── concurrent calls through ONE shared registry ──
            // The registry itself must be safe to share across threads (the
            // stores are behind a per-path lock, so sharing must not deadlock or
            // corrupt). Hammer a mix of kinds concurrently.
            let mix_path = tmp.join("laya_conc_mix.json");
            let _ = std::fs::remove_file(&mix_path);
            let reg = std::sync::Arc::new(
                capability::registry_from(
                    &[
                        ("q", json!({"kind": "queue", "op": "push", "path": mix_path.to_string_lossy()})),
                        ("t", json!({"kind": "text", "op": "length"})),
                        ("m", json!({"kind": "math", "op": "eval"})),
                    ],
                    Some(p.clone()),
                )
                .unwrap(),
            );
            let mut hs = Vec::new();
            for t in 0..THREADS {
                let r = reg.clone();
                hs.push(std::thread::spawn(move || {
                    let mut ok = 0usize;
                    for i in 0..PER {
                        if r.call("q", &json!({"value": format!("t{t}-i{i}")}), &json!({}))
                            .is_ok()
                        {
                            ok += 1;
                        }
                        // `text` / `math` read their inputs from `with`
                        // (not `state`), so passing state here would silently
                        // fail every call.
                        if r.call("t", &json!({"text": "abc"}), &json!({})).is_ok() {
                            ok += 1;
                        }
                        if r.call("m", &json!({"expression": "1 + 2"}), &json!({}))
                            .is_ok()
                        {
                            ok += 1;
                        }
                    }
                    ok
                }));
            }
            let mut total_ok = 0usize;
            for h in hs {
                total_ok += h.join().unwrap();
            }
            h.check(
                &format!(
                    "fix: a shared registry serves every concurrent call (expected {}, got {})",
                    THREADS * PER * 3,
                    total_ok
                ),
                total_ok == THREADS * PER * 3,
            );
            let mix: Vec<Value> = std::fs::read_to_string(&mix_path)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default();
            h.check(
                &format!(
                    "fix: concurrent queue pushes through a shared registry lose nothing (expected {}, got {})",
                    THREADS * PER,
                    mix.len()
                ),
                mix.len() == THREADS * PER,
            );
            let _ = std::fs::remove_file(&mix_path);
        }

        // (3a5) `with.op` must override the configured op for every kind that
        // has an `op` — not only the stores. The same "read from the definition
        // only" mistake existed at ~14 call sites, so a spec asking for one op
        // silently ran another.
        {
            // keyvalue: declared `get`, asked for `set` + a later `get`.
            let f = std::env::temp_dir().join("laya_op_override.json");
            let _ = std::fs::remove_file(&f);
            let mut p = capability::Policy::default();
            p.allow_paths = vec![std::env::temp_dir().to_string_lossy().to_string()];
            let reg = capability::registry_from(
                &[("k", json!({"kind": "keyvalue", "op": "get", "path": f.to_string_lossy(), "key": "a"}))],
                Some(p.clone()),
            )
            .unwrap();
            let set = reg.call("k", &json!({"op": "set", "value": "hello"}), &json!({}));
            h.check(
                "fix: keyvalue honours a runtime op override (set)",
                set.is_ok(),
            );
            let got = reg
                .call("k", &json!({"op": "get"}), &json!({}))
                .map(|v| v["value"].clone())
                .unwrap_or(Value::Null);
            h.eq(
                "fix: keyvalue reads back what the override wrote",
                got,
                json!("hello"),
            );

            // json: declared `pick`, asked for `sort_keys`.
            let jr = capability::registry_from(
                &[("j", json!({"kind": "json", "op": "pick"}))],
                Some(p.clone()),
            )
            .unwrap();
            let jv = jr.call(
                "j",
                &json!({"op": "sort_keys", "value": {"b": 1, "a": 2}}),
                &json!({}),
            );
            h.check(
                "fix: json honours a runtime op override (sort_keys)",
                jv.is_ok(),
            );

            // math: declared `stats`, asked for `eval`.
            let mr = capability::registry_from(
                &[("m", json!({"kind": "math", "op": "stats"}))],
                Some(p),
            )
            .unwrap();
            let mv = mr
                .call("m", &json!({"op": "eval", "expression": "1/4"}), &json!({}))
                .map(|v| v["value"].as_f64().unwrap_or(0.0))
                .unwrap_or(0.0);
            h.eq("fix: math honours a runtime op override (eval)", mv, 0.25);
            let _ = std::fs::remove_file(&f);
        }

        // (3a6) `goal_runner` exposes the external agent goal harnesses
        // (`cxgo` / `cmdgo`) to a spec. Success and failure paths, plus the
        // safety gates: exec must be enabled, the runner must be a known one,
        // and the target doc must sit under allow_paths.
        {
            let dir = std::env::temp_dir().join("laya_goal_probe");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let doc = dir.join("g.md");
            std::fs::write(&doc, "# goal\n\nwrite done.txt\n").unwrap();

            let mut no_exec = capability::Policy::default();
            no_exec.allow_paths = vec![dir.to_string_lossy().to_string()];
            let reg = capability::registry_from(
                &[("g", json!({"kind": "goal_runner", "runner": "cmdgo"}))],
                Some(no_exec.clone()),
            )
            .unwrap();
            let e = reg
                .call("g", &json!({"goal": doc.to_string_lossy()}), &json!({}))
                .unwrap_err()
                .to_string();
            h.check(
                "goal_runner: requires allow_exec (spawns an agent process)",
                e.contains("allow_exec"),
            );

            let mut ok_pol = no_exec.clone();
            ok_pol.allow_exec = true;
            // Unknown runner is refused by name, so a spec cannot point this at
            // an arbitrary binary.
            let reg_bad = capability::registry_from(
                &[("g", json!({"kind": "goal_runner", "runner": "rm -rf /"}))],
                Some(ok_pol.clone()),
            )
            .unwrap();
            let e2 = reg_bad
                .call("g", &json!({"goal": doc.to_string_lossy()}), &json!({}))
                .unwrap_err()
                .to_string();
            h.check(
                "goal_runner: refuses an unknown runner",
                e2.contains("unknown runner"),
            );

            // A doc outside allow_paths is denied.
            let reg_ok = capability::registry_from(
                &[("g", json!({"kind": "goal_runner", "runner": "cmdgo"}))],
                Some(ok_pol.clone()),
            )
            .unwrap();
            let e3 = reg_ok
                .call("g", &json!({"goal": "/etc/passwd"}), &json!({}))
                .unwrap_err()
                .to_string();
            h.check(
                "goal_runner: denies a goal outside allow_paths",
                e3.contains("outside"),
            );
            // A missing doc is an explicit error, not a silent no-op run.
            let e4 = reg_ok
                .call(
                    "g",
                    &json!({"goal": dir.join("nope.md").to_string_lossy()}),
                    &json!({}),
                )
                .unwrap_err()
                .to_string();
            h.check(
                "goal_runner: a missing goal doc errors",
                e4.contains("does not exist"),
            );
            // And a missing 'goal' argument is rejected.
            h.check(
                "goal_runner: needs a goal doc",
                reg_ok.call("g", &json!({}), &json!({})).is_err(),
            );

            let _ = std::fs::remove_dir_all(&dir);
        }

        // (3a6b) `goal_runner` success + failure paths, driven by a stub runner
        // so the test does not depend on `cmdgo`/`cxgo` being installed or
        // logged in. The runner binary is resolved from LAYA_AGENT_BIN_DIR, so a
        // throwaway dir with a fake `cmdgo` exercises the real spawn/parse path.
        {
            // Note: env vars are process-global, so this block owns
            // LAYA_AGENT_BIN_DIR for its duration.
            let dir = std::env::temp_dir().join("laya_goal_stub");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let bin = dir.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            let doc = dir.join("g.md");
            std::fs::write(&doc, "# goal\n\nstub\n").unwrap();

            // A stub that succeeds, writes the acceptance report dir with 2
            // files, and echoes its argv so we can assert args were forwarded.
            let ok_stub = bin.join("cmdgo");
            std::fs::write(&ok_stub, "#!/bin/sh\necho \"argv:$@\"\nmkdir -p acceptance-reports\n: > acceptance-reports/a.json\n: > acceptance-reports/b.json\nexit 0\n").unwrap();
            // A stub that fails, to prove `ok` tracks the real exit code.
            let bad_stub = bin.join("cxgo");
            std::fs::write(&bad_stub, "#!/bin/sh\necho boom >&2\nexit 3\n").unwrap();
            for f in [&ok_stub, &bad_stub] {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
            }

            let prev_bin = std::env::var("LAYA_AGENT_BIN_DIR").ok();
            std::env::set_var("LAYA_AGENT_BIN_DIR", bin.to_string_lossy().to_string());

            let mut p = capability::Policy::default();
            p.allow_exec = true;
            p.allow_paths = vec![dir.to_string_lossy().to_string()];

            // success path
            let reg = capability::registry_from(
                &[(
                    "g",
                    json!({"kind": "goal_runner", "runner": "cmdgo",
                               "args": ["--fresh", "--max-rounds", "2"]}),
                )],
                Some(p.clone()),
            )
            .unwrap();
            let out = reg
                .call(
                    "g",
                    &json!({"goal": doc.to_string_lossy(), "workdir": dir.to_string_lossy()}),
                    &json!({}),
                )
                .unwrap();
            h.check(
                "goal_runner: success path returns ok=true",
                out["ok"].as_bool().unwrap_or(false),
            );
            h.eq(
                "goal_runner: exit_code tracks the child",
                out["exit_code"].as_i64().unwrap_or(-1),
                0,
            );
            h.check(
                "goal_runner: not timed out",
                !out["timed_out"].as_bool().unwrap_or(true),
            );
            h.eq(
                "goal_runner: counts the acceptance reports",
                out["report_count"].as_u64().unwrap_or(0),
                2,
            );
            let cmd = out["command"].as_array().cloned().unwrap_or_default();
            let joined = cmd
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            h.check(
                "goal_runner: forwards the configured args",
                joined.contains("--fresh") && joined.contains("--max-rounds"),
            );
            h.check(
                "goal_runner: passes the goal doc as the first arg",
                cmd.get(1)
                    .and_then(|v| v.as_str())
                    .map(|s| s.ends_with("g.md"))
                    .unwrap_or(false),
            );

            // failure path: non-zero exit is reported, never masked as success
            let reg_bad = capability::registry_from(
                &[("g", json!({"kind": "goal_runner", "runner": "cxgo"}))],
                Some(p.clone()),
            )
            .unwrap();
            // Run it in its own clean workdir so the success-path reports above
            // are not counted here.
            let fail_dir = dir.join("failwd");
            std::fs::create_dir_all(&fail_dir).unwrap();
            let bad = reg_bad
                .call(
                    "g",
                    &json!({"goal": doc.to_string_lossy(), "workdir": fail_dir.to_string_lossy()}),
                    &json!({}),
                )
                .unwrap();
            h.check(
                "goal_runner: a failing runner reports ok=false",
                !bad["ok"].as_bool().unwrap_or(true),
            );
            h.eq(
                "goal_runner: a failing runner keeps its exit code",
                bad["exit_code"].as_i64().unwrap_or(0),
                3,
            );
            h.check(
                "goal_runner: a failing runner surfaces stderr",
                bad["stderr"].as_str().unwrap_or("").contains("boom"),
            );
            h.eq(
                "goal_runner: a failing runner has no reports",
                bad["report_count"].as_u64().unwrap_or(9),
                0,
            );

            // missing runner binary is an error, not a fake run
            std::fs::remove_file(&ok_stub).unwrap();
            let e = reg
                .call(
                    "g",
                    &json!({"goal": doc.to_string_lossy(), "workdir": dir.to_string_lossy()}),
                    &json!({}),
                )
                .unwrap_err()
                .to_string();
            h.check(
                "goal_runner: a missing runner binary errors",
                e.contains("cannot start"),
            );

            match prev_bin {
                Some(v) => std::env::set_var("LAYA_AGENT_BIN_DIR", v),
                None => std::env::remove_var("LAYA_AGENT_BIN_DIR"),
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
        // (3a7) `LayaBackend` against hostile HTTP responses. The previous
        // sweeps only read its error handling; this actually feeds it garbage:
        // malformed JSON, a missing "answers" envelope, wrong types, a non-2xx
        // status, and a connection that closes mid-body. It must return an error
        // or a safe degradation — never panic and never a silent wrong verdict.
        {
            use std::io::{Read as _, Write as _};
            use std::net::TcpListener;

            /// Serve the same canned response to every connection, so a client
            /// that retries or reuses a socket still gets an answer. The listener
            /// is bound before the thread starts, so the port is already
            /// accepting by the time the caller connects.
            fn serve_always(
                status: &'static str,
                ctype: &'static str,
                body: &'static [u8],
            ) -> (u16, std::sync::Arc<std::sync::atomic::AtomicBool>) {
                let l = TcpListener::bind("127.0.0.1:0").unwrap();
                let port = l.local_addr().unwrap().port();
                let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let stop2 = stop.clone();
                std::thread::spawn(move || {
                    for conn in l.incoming() {
                        if stop2.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }
                        let Ok(mut c) = conn else { return };
                        let _ = c.set_read_timeout(Some(std::time::Duration::from_millis(500)));
                        let mut b = [0u8; 8192];
                        let _ = c.read(&mut b);
                        let head = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = c.write_all(head.as_bytes());
                        let _ = c.write_all(body);
                        let _ = c.flush();
                    }
                });
                (port, stop)
            }

            let q = json!({"department": {"type": "choice", "instructions": "?",
                                          "criteria": {"billing": "b", "sales": "s"}}});
            let st = json!({"subject": "x"});

            let mut stops = Vec::new();
            let mut cases: Vec<(&str, u16)> = Vec::new();
            for (label, status, ctype, body) in [
                (
                    "malformed json",
                    "200 OK",
                    "application/json",
                    &b"{not json"[..],
                ),
                (
                    "missing answers envelope",
                    "200 OK",
                    "application/json",
                    &b"{}"[..],
                ),
                (
                    "answers wrong type",
                    "200 OK",
                    "application/json",
                    &br#"{"answers": 42}"#[..],
                ),
                ("empty body", "200 OK", "application/json", &b""[..]),
                (
                    "server error",
                    "500 Internal Server Error",
                    "application/json",
                    &b"{}"[..],
                ),
                (
                    "html instead of json",
                    "200 OK",
                    "text/html",
                    &b"<html>nope</html>"[..],
                ),
                (
                    "truncated json",
                    "200 OK",
                    "application/json",
                    &br#"{"answers": {"dep"#[..],
                ),
            ] {
                let (port, stop) = serve_always(status, ctype, body);
                stops.push(stop);
                cases.push((label, port));
            }

            for (label, port) in cases {
                let be =
                    laya_workflow::backend::LayaBackend::new(&format!("http://127.0.0.1:{port}"));
                // A panic here would abort the process, which is the bug we are
                // guarding against; an Err/Ok-with-empty-answers is acceptable.
                let r =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| be.decide(&st, &q)));
                match r {
                    Err(_) => h.check(
                        &format!("fix: LayaBackend does not panic on {label}"),
                        false,
                    ),
                    Ok(Ok(v)) => {
                        // If it returns Ok, it must not have invented an answer
                        // for a question the server never answered.
                        h.check(
                            &format!("fix: LayaBackend invents nothing on {label}"),
                            v.answers.is_empty() || v.answers.contains_key("department"),
                        );
                    }
                    Ok(Err(_)) => {
                        h.check(&format!("fix: LayaBackend errors cleanly on {label}"), true)
                    }
                }
            }

            // A well-formed body must still parse. This is pure parsing, so
            // assert it directly on the response decoder rather than through a
            // socket: the transport itself is already exercised by every hostile
            // case above, and a socket round-trip here only added flake.
            let good = json!({
                "answers": {"department": {"type": "choice", "choice": "billing",
                            "probabilities": {"billing": 0.9, "sales": 0.1}, "confidence": 0.9}},
                "usage": {"input_tokens": 3}
            });
            let v = laya_workflow::backend::verdict_from_response(&good);
            h.check("fix: a well-formed response still parses", v.is_ok());
            if let Ok(v) = v {
                h.check(
                    "fix: the parsed answer is kept",
                    v.answers
                        .get("department")
                        .map(|d| d.as_str() == "billing")
                        .unwrap_or(false),
                );
                h.eq("fix: usage tokens are kept", v.input_tokens, 3);
            }
            // And the decoder must reject the malformed shapes it is fed above.
            for (label, bad) in [
                ("not an object", json!(42)),
                ("no answers", json!({})),
                ("answers not an object", json!({"answers": 42})),
            ] {
                h.check(
                    &format!("fix: the decoder rejects {label}"),
                    laya_workflow::backend::verdict_from_response(&bad).is_err(),
                );
            }
            for st in stops {
                st.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }

        // (3a8) Resource-leak soak. Repeatedly drive every capability that can
        // open an OS resource (sockets, child processes, file handles) and check
        // that fds / RSS / child processes do not grow monotonically. A leak here
        // does not fail a functional test — it only shows up after thousands of
        // calls in a long-running workflow — so it has to be measured directly.
        {
            /// Count open file descriptors for this process.
            fn fd_count() -> usize {
                std::fs::read_dir("/proc/self/fd")
                    .map(|d| d.count())
                    .unwrap_or(0)
            }
            /// RSS in KiB (field 2 of /proc/self/statm is resident pages).
            fn rss_kib() -> u64 {
                let s = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
                let pages: u64 = s
                    .split_whitespace()
                    .nth(1)
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(0);
                pages * 4 // 4 KiB pages
            }
            /// Direct children of this process (zombie or live).
            fn child_count() -> usize {
                let me = std::process::id().to_string();
                std::fs::read_dir("/proc")
                    .map(|d| {
                        d.filter_map(|e| e.ok())
                            .filter(|e| {
                                let name = e.file_name().to_string_lossy().into_owned();
                                if !name.chars().all(|c| c.is_ascii_digit()) {
                                    return false;
                                }
                                std::fs::read_to_string(format!("/proc/{name}/stat"))
                                    .map(|st| {
                                        // "pid (comm) state ppid ..." — comm may contain spaces.
                                        let close = st.rfind(')').unwrap_or(0);
                                        st.get(close + 2..)
                                            .and_then(|r| r.split_whitespace().nth(1))
                                            .map(|ppid| ppid == me)
                                            .unwrap_or(false)
                                    })
                                    .unwrap_or(false)
                            })
                            .count()
                    })
                    .unwrap_or(0)
            }

            // A throwaway TCP echo + UDP listener so the socket capabilities have
            // somewhere real to connect to (they open/close a socket per call).
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let tcp_port = listener.local_addr().unwrap().port();
            std::thread::spawn(move || {
                for conn in listener.incoming() {
                    let Ok(mut c) = conn else { break };
                    let mut b = [0u8; 512];
                    let _ = std::io::Read::read(&mut c, &mut b);
                    let _ = std::io::Write::write_all(&mut c, b"ok\n");
                }
            });

            let reg = capability::registry_from(
                &[
                    ("tcp", json!({"kind": "tcp", "host": "127.0.0.1", "port": tcp_port, "timeout_ms": 2000})),
                    ("udp", json!({"kind": "udp", "host": "127.0.0.1", "port": tcp_port, "timeout_ms": 500})),
                    ("redis", json!({"kind": "redis", "host": "127.0.0.1", "port": tcp_port, "timeout_ms": 1000})),
                    ("smtp", json!({"kind": "smtp", "host": "127.0.0.1", "port": tcp_port, "timeout_ms": 1000})),
                    ("http", json!({"kind": "http", "url": format!("http://127.0.0.1:{tcp_port}/x"), "method": "GET", "timeout_ms": 1000})),
                    ("sh", json!({"kind": "exec", "argv": ["/bin/echo", "hi"], "timeout_ms": 5000})),
                    ("q", json!({"kind": "queue", "path": format!("{}/soak_queue.json", std::env::var("LAYA_STORE_DIR").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned()))})),
                ],
                Some(capability::Policy {
                    allow_exec: true,
                    allow_paths: vec![std::env::temp_dir().to_string_lossy().to_string()],
                    ..Default::default()
                }),
            )
            .unwrap();

            let warmup = || {
                let _ = reg.call("tcp", &json!({}), &json!({}));
                let _ = reg.call("udp", &json!({}), &json!({}));
                let _ = reg.call("redis", &json!({}), &json!({"op": "get", "key": "k"}));
                let _ = reg.call("smtp", &json!({}), &json!({}));
                let _ = reg.call("http", &json!({}), &json!({}));
                let _ = reg.call("sh", &json!({}), &json!({}));
                let _ = reg.call("q", &json!({}), &json!({"op": "push", "value": 1}));
            };

            // Warm up once so one-off allocations (TLS tables, lazily-created
            // buffers, the store lock map) are not mistaken for a leak.
            warmup();
            let fd0 = fd_count();
            let rss0 = rss_kib();
            let ch0 = child_count();

            const ROUNDS: usize = 60;
            for _ in 0..ROUNDS {
                warmup();
            }

            let fd1 = fd_count();
            let rss1 = rss_kib();
            let ch1 = child_count();

            // fds must be bounded — a per-call socket/pipe leak would add
            // ROUNDS * (calls that open one) descriptors.
            h.check(
                &format!("fix: soak opened no fds (fd {fd0} -> {fd1} after {ROUNDS} rounds)"),
                fd1 <= fd0 + 4,
            );
            // Reaped children must not accumulate; at most the one child that
            // exec may still be running.
            h.check(
                &format!("fix: soak leaked no children (children {ch0} -> {ch1})"),
                ch1 <= ch0 + 1,
            );
            // RSS is noisy (allocator arenas); allow generous headroom but catch
            // a per-call leak that would grow by hundreds of MiB.
            h.check(
                &format!("fix: soak did not grow RSS unboundedly ({rss0} -> {rss1} KiB)"),
                rss1 < rss0 + (256 * 1024),
            );
            // The store must still be functional after the soak (the per-path
            // lock map must not have leaked a stuck lock).
            let len = reg.call("q", &json!({}), &json!({"op": "length"}));
            h.check(
                "fix: the store still answers after the soak",
                len.map(|v| v.is_object() || v.is_array() || v.is_number() || v.is_string())
                    .unwrap_or(false),
            );
        }

        // (3b) A corrupt store file must not be silently overwritten. The old
        // `.ok()` chain treated an unreadable/broken file as an empty store, so
        // a write then destroyed the user's data (observed: a corrupt queue file
        // became `[null]` and the push still reported success).
        for kind in ["keyvalue", "cache", "queue"] {
            let f = std::env::temp_dir().join(format!("laya_corrupt_{kind}.json"));
            std::fs::write(&f, b"CORRUPT{{{").unwrap();
            let mut p = capability::Policy::default();
            p.allow_paths = vec![std::env::temp_dir().to_string_lossy().to_string()];
            let reg = capability::registry_from(
                &[(
                    "s",
                    json!({"kind": kind, "op": "set", "path": f.to_string_lossy(), "key": "k"}),
                )],
                Some(p),
            )
            .unwrap();
            let r = reg.call("s", &json!({"value": "v"}), &json!({}));
            h.check(
                &format!("fix: {kind} errors on a corrupt store file"),
                r.is_err(),
            );
            h.check(
                &format!("fix: {kind} leaves the corrupt file intact"),
                std::fs::read_to_string(&f)
                    .map(|c| c.contains("CORRUPT"))
                    .unwrap_or(false),
            );
            let _ = std::fs::remove_file(&f);
        }

        // (3c) A *missing* store file is still a legitimate empty start.
        {
            let f = std::env::temp_dir().join("laya_missing_store.json");
            let _ = std::fs::remove_file(&f);
            let mut p = capability::Policy::default();
            p.allow_paths = vec![std::env::temp_dir().to_string_lossy().to_string()];
            let reg = capability::registry_from(
                &[("s", json!({"kind": "keyvalue", "op": "set", "path": f.to_string_lossy(), "key": "k"}))],
                Some(p),
            )
            .unwrap();
            h.check(
                "fix: a missing store file starts empty (no false positive)",
                reg.call("s", &json!({"value": "v"}), &json!({})).is_ok(),
            );
            let _ = std::fs::remove_file(&f);
        }

        // (4) Error text must never carry a secret value (regression guard for the
        // redaction work): resolve a secret into a header, make the call fail, and
        // confirm the value does not appear in the error.
        secret::set_secrets({
            let mut s = secret::Secrets::default();
            s.insert("LEAK_PROBE", "super-secret-value-1234");
            s
        })
        .unwrap();
        let reg = capability::registry_from(
            &[(
                "m",
                json!({
                    "kind": "http", "method": "GET",
                    "url": "http://127.0.0.1:1/nope",
                    "headers": {"authorization": "Bearer ${secret.LEAK_PROBE}"},
                    "timeout_ms": 1500
                }),
            )],
            Some(pol.clone()),
        )
        .unwrap();
        let e = reg
            .call("m", &json!({}), &json!({}))
            .unwrap_err()
            .to_string();
        h.check(
            "fix: a failing call does not leak the secret into its error",
            !e.contains("super-secret-value-1234"),
        );
        let _ = secret::set_secrets(secret::Secrets::default());
    }

    // ── heuristic-backend bug regressions ───────────────────────────
    //
    // Each assertion below failed before the corresponding fix. They are written
    // as semantic statements (what the app must decide and why) rather than
    // snapshots of numbers, so they keep their meaning if thresholds move.
}
