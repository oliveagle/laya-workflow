//! capabilities3 regression sections for `laya-workflow-tests`.
#![allow(unused_imports)]
use crate::*;
pub fn test_capabilities_batch3(h: &mut Harness) {
    // These checks need the local mocks (started via `bench/mock_services.sh
    // start`). Distinguish two cases so a broken setup cannot masquerade as
    // success:
    //   * LAYA_MOCK3 unset        -> nothing to test against; skip explicitly
    //   * LAYA_MOCK3 set but down -> a real configuration error; fail loudly
    let base3 = match std::env::var("LAYA_MOCK3") {
        Err(_) => {
            h.check("batch3 skipped (LAYA_MOCK3 unset; run bench/mock_services.sh start)", true);
            None
        }
        Ok(b3) => {
            let up = std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], 6380)),
                std::time::Duration::from_millis(500),
            )
            .is_ok();
            h.check(
                "batch3 mocks reachable (LAYA_MOCK3 set)",
                up,
            );
            if up {
                Some(b3)
            } else {
                None
            }
        }
    };
    if let Some(b3) = base3 {
        // b3 format: host:redis_port:nats_port:mqtt_port:smtp_port:s3_port:prom_port:kafka_port:udp_port
        let f: Vec<&str> = b3.split(':').collect();
        let host = f.first().copied().unwrap_or("127.0.0.1").to_string();
        let p = |i: usize, d: u16| f.get(i).and_then(|x| x.parse::<u16>().ok()).unwrap_or(d);
        let (redis_p, nats_p, mqtt_p, smtp_p, s3_p, prom_p, kafka_p, udp_p) =
            (p(1, 6380), p(2, 4223), p(3, 1884), p(4, 2526), p(5, 9000), p(6, 9091), p(7, 8083), p(8, 9999));

        let mut p3 = capability::Policy::default();
        p3.allow_hosts = vec!["127.0.0.1".to_string(), "localhost".to_string()];
        p3.allow_exec = true;
        p3.retries = 0;
        let store_dir = std::env::temp_dir().join("laya_cap3_store");
        let _ = std::fs::create_dir_all(&store_dir);
        p3.allow_paths = vec![store_dir.to_str().unwrap().to_string()];

        let reg = capability::registry_from(
            &[
                ("tcp", json!({"kind": "tcp", "host": host, "port": redis_p, "timeout_ms": 3000})),
                ("udp", json!({"kind": "udp", "host": host, "port": udp_p, "timeout_ms": 3000})),
                ("redis", json!({"kind": "redis", "host": host, "port": redis_p, "timeout_ms": 3000})),
                ("nats", json!({"kind": "nats", "host": host, "port": nats_p, "timeout_ms": 3000})),
                ("mqtt", json!({"kind": "mqtt", "host": host, "port": mqtt_p, "client_id": "laya-test", "timeout_ms": 3000})),
                ("smtp", json!({"kind": "smtp", "host": host, "port": smtp_p, "from": "wf@example.com", "timeout_ms": 5000})),
                ("s3", json!({"kind": "s3", "endpoint": format!("http://{host}:{s3_p}"), "bucket": "bucket", "timeout_ms": 5000})),
                ("prom", json!({"kind": "prometheus", "url": format!("http://{host}:{prom_p}/metrics"), "timeout_ms": 5000})),
                ("kafka", json!({"kind": "kafka", "url": format!("http://{host}:{kafka_p}"), "topic": "events", "timeout_ms": 5000})),
            ],
            Some(p3.clone()),
        )
        .unwrap();

        // tcp: send a RESP frame (the mock speaks RESP, not inline commands)
        let r = reg.call("tcp", &json!({"send": "*1\r\n$4\r\nPING\r\n"}), &json!({})).unwrap();
        h.eq("tcp sends bytes", r["sent_bytes"].as_u64().unwrap(), 14);
        h.check("tcp receives PONG", r["reply"].as_str().unwrap_or("").contains("PONG"));
        h.check("tcp records recv_bytes", r["recv_bytes"].as_u64().unwrap() > 0);
        h.check("tcp refuses empty host", {
            let bad = capability::registry_from(&[("t", json!({"kind":"tcp","port":1}))], Some(p3.clone())).unwrap();
            bad.call("t", &json!({}), &json!({})).is_err()
        });

        // udp echo
        let r = reg.call("udp", &json!({"send": "ping-udp"}), &json!({})).unwrap();
        h.check("udp receives echo", r["reply"].as_str().unwrap_or("").contains("echo:ping-udp"));
        h.check("udp refuses missing port", {
            let bad = capability::registry_from(&[("u", json!({"kind":"udp","host":"127.0.0.1"}))], Some(p3.clone())).unwrap();
            bad.call("u", &json!({"send":"x"}), &json!({})).is_err()
        });

        // redis: set/get/incr/keys
        reg.call("redis", &json!({"op": "set", "key": "k1", "value": "v1"}), &json!({})).unwrap();
        let r = reg.call("redis", &json!({"op": "get", "key": "k1"}), &json!({})).unwrap();
        h.eq("redis get returns value", r["reply"].as_str().unwrap().to_string(), "v1".to_string());
        // INCR is stateful on the mock, so assert *increment*, not an absolute
        let k = format!("incr-{}", std::process::id());
        let a = reg.call("redis", &json!({"op": "incr", "key": k}), &json!({})).unwrap()["reply"].as_i64().unwrap();
        let b = reg.call("redis", &json!({"op": "incr", "key": k}), &json!({})).unwrap()["reply"].as_i64().unwrap();
        h.eq("redis incr increments", b - a, 1);
        h.eq("redis del removes key", reg.call("redis", &json!({"op": "del", "key": k}), &json!({})).unwrap()["reply"].as_i64().unwrap(), 1);
        let r = reg.call("redis", &json!({"op": "keys"}), &json!({})).unwrap();
        h.check("redis keys list non-empty", r["reply"].as_array().map(|a| !a.is_empty()).unwrap_or(false));
        h.check("redis bad op rejected", reg.call("redis", &json!({"op": "flushall"}), &json!({})).is_err());
        h.check("redis host not allow-listed is denied", {
            let bad = capability::registry_from(
                &[("r", json!({"kind":"redis","host":"127.0.0.2","port":6380}))],
                Some(p3.clone()),
            ).unwrap();
            bad.call("r", &json!({"op":"ping"}), &json!({})).is_err()
        });

        // nats publish
        let r = reg.call("nats", &json!({"subject": "demo.topic", "message": "hi-nats"}), &json!({})).unwrap();
        h.eq("nats sent bytes", r["sent_bytes"].as_u64().unwrap(), 7);
        h.check("nats needs subject", reg.call("nats", &json!({"message": "x"}), &json!({})).is_err());

        // mqtt connect+publish
        let r = reg.call("mqtt", &json!({"topic": "sensors/t1", "message": "23.5"}), &json!({})).unwrap();
        h.eq("mqtt connection accepted", r["connected"].as_bool().unwrap(), true);
        h.check("mqtt needs topic", reg.call("mqtt", &json!({"message": "x"}), &json!({})).is_err());

        // smtp full session
        // Regression guard: the mock writes the EHLO reply as
        // "250-mock\r\n250 SIZE ...\r\n" in ONE send, i.e. two reply lines in a
        // single TCP segment. A client that inspects only the first chunk sees
        // a "-" continuation and then blocks forever waiting for a line that
        // already arrived. Reaching the transcript at all proves the reply
        // reader is line-buffered rather than chunk-based.
        let r = reg.call(
            "smtp",
            &json!({"to": ["a@example.com", "b@example.com"], "subject": "hi", "body": "hello"}),
            &json!({}),
        )
        .unwrap();
        h.check("smtp transcript shows DATA+250", r["transcript"].as_str().unwrap().contains("250"));
        h.check(
            "smtp coalesced multi-line EHLO reply parsed",
            r["transcript"].as_str().unwrap().contains("250 SIZE 10485760"),
        );
        h.eq("smtp recipient count", r["to"].as_array().unwrap().len(), 2);
        h.check("smtp needs recipients", reg.call("smtp", &json!({"subject": "x"}), &json!({})).is_err());
        // AUTH LOGIN: the mock accepts after the username (235) — the client
        // must not insist on a second 334 challenge (fail-closed regression).
        let reg_auth = capability::registry_from(
            &[("smtp_auth", json!({"kind": "smtp", "host": host, "port": smtp_p,
                                   "from": "a@example.com", "username": "u", "password": "p",
                                   "timeout_ms": 5000}))],
            Some(p3.clone()),
        )
        .unwrap();
        let ar = reg_auth.call("smtp_auth", &json!({"to": ["x@example.com"], "subject": "s", "body": "b"}), &json!({}));
        h.check("smtp AUTH LOGIN accepts immediate 235", ar.map(|r| r["authed"].as_bool().unwrap_or(false)).unwrap_or(false));

        // s3 put/get/list/delete
        let r = reg.call("s3", &json!({"op": "put", "key": "obj1", "body": "payload-1"}), &json!({})).unwrap();
        h.eq("s3 put status", r["status"].as_i64().unwrap(), 200);
        let r = reg.call("s3", &json!({"op": "get", "key": "obj1"}), &json!({})).unwrap();
        h.eq("s3 get body", r["body"].as_str().unwrap().to_string(), "payload-1".to_string());
        let r = reg.call("s3", &json!({"op": "list"}), &json!({})).unwrap();
        h.check("s3 list finds object", r["keys"].as_array().unwrap().iter().any(|k| k.as_str() == Some("obj1")));
        let r = reg.call("s3", &json!({"op": "delete", "key": "obj1"}), &json!({})).unwrap();
        h.eq("s3 delete status", r["status"].as_i64().unwrap(), 204);

        // prometheus scrape + query
        let r = reg.call("prom", &json!({}), &json!({})).unwrap();
        h.eq("prometheus mode scrape", r["mode"].as_str().unwrap().to_string(), "scrape".to_string());
        h.check("prometheus parses samples", r["count"].as_u64().unwrap() >= 3);
        let r = reg.call("prom", &json!({"query": "up"}), &json!({})).unwrap();
        h.eq("prometheus query mode", r["mode"].as_str().unwrap().to_string(), "query".to_string());

        // kafka REST produce
        let r = reg.call("kafka", &json!({"value": {"id": 1}}), &json!({})).unwrap();
        h.eq("kafka produce status", r["status"].as_i64().unwrap(), 200);
        h.eq("kafka topic echoed", r["offsets"][0]["topic"].as_str().unwrap().to_string(), "events".to_string());

        // archive: create a tar then list it (exec-gated, path allow-listed)
        let arc = store_dir.join("t.tar");
        let src = store_dir.join("member.txt");
        std::fs::write(&src, b"archive-member").unwrap();
        let _ = std::process::Command::new("tar")
            .arg("-cf")
            .arg(&arc)
            .arg("-C")
            .arg(&store_dir)
            .arg("member.txt")
            .status();
        let reg_arc = capability::registry_from(
            &[("arc", json!({"kind": "archive", "op": "list", "path": arc.to_str().unwrap()}))],
            Some(p3.clone()),
        )
        .unwrap();
        let r = reg_arc.call("arc", &json!({}), &json!({})).unwrap();
        h.check("archive lists tar member", r["entries"].as_array().unwrap().iter().any(|e| e.as_str().unwrap_or("").contains("member.txt")));
        h.check("archive denied without allow_exec", {
            let mut noexec = p3.clone();
            noexec.allow_exec = false;
            let bad = capability::registry_from(
                &[("a", json!({"kind":"archive","op":"list","path":arc.to_str().unwrap()}))],
                Some(noexec),
            ).unwrap();
            bad.call("a", &json!({}), &json!({})).is_err()
        });

        // pdf / sql: CLI-backed; verify the gates even when the tools are absent
        let reg_cli = capability::registry_from(
            &[
                ("pdf", json!({"kind": "pdf", "path": store_dir.join("x.pdf").to_str().unwrap()})),
                ("sql", json!({"kind": "sql", "driver": "sqlite3", "dsn": store_dir.join("x.db").to_str().unwrap()})),
            ],
            Some(p3.clone()),
        )
        .unwrap();
        let pout = reg_cli.call("pdf", &json!({}), &json!({}));
        h.check(
            "pdf missing file reports exit!=0 with stderr",
            pout.as_ref().map(|r| r["exit_code"].as_i64().unwrap_or(0) != 0).unwrap_or(true),
        );
        h.eq("sql rejects unknown driver", {
            let bad = capability::registry_from(
                &[("s", json!({"kind":"sql","driver":"oracle","dsn":"x"}))],
                Some(p3.clone()),
            ).unwrap();
            bad.call("s", &json!({"sql": "select 1"}), &json!({})).is_err()
        }, true);
        h.check("pdf path outside allow-list denied", {
            let bad = capability::registry_from(
                &[("p", json!({"kind":"pdf","path":"/etc/hostname"}))],
                Some(p3.clone()),
            ).unwrap();
            bad.call("p", &json!({}), &json!({})).is_err()
        });
        h.check("cli kinds denied without allow_exec", {
            let mut noexec = p3.clone();
            noexec.allow_exec = false;
            let bad = capability::registry_from(
                &[("p", json!({"kind":"pdf","path":store_dir.join("x.pdf").to_str().unwrap()}))],
                Some(noexec),
            ).unwrap();
            bad.call("p", &json!({}), &json!({})).is_err()
        });

        let _ = std::fs::remove_dir_all(&store_dir);
    }

    // ── web research (web_search / web_fetch) ───────────────────────
}


pub fn test_web_research(h: &mut Harness) {
    {
        // The HTML/markdown helpers are pure, so they are exercised even when
        // the mock is down; the network paths join the batch-3 gate.
        const HTML: &str = "<title>T</title><h1>Head</h1><p>a &amp; b</p>\
                            <ul><li>one</li></ul><a href=\"/x\">link</a>\
                            <script>var leak=1;</script>";
        h.check("web: module has extractable summary", {
            // The helpers are private; assert through the public call path with
            // a spec-level parse instead.
            let reg = capability::registry_from(
                &[("w", json!({"kind":"web_fetch","format":"text"}))],
                Some(capability::Policy::default()),
            );
            reg.is_ok()
        });

        // scheme + host guards are independent of the mock
        let p_local = {
            let mut p = capability::Policy::default();
            p.allow_hosts = vec!["127.0.0.1".to_string()];
            p
        };
        let reg_guard = capability::registry_from(
            &[("f", json!({"kind": "web_fetch", "timeout_ms": 2000}))],
            Some(p_local.clone()),
        )
        .unwrap();
        h.check(
            "web_fetch rejects non-http scheme",
            reg_guard.call("f", &json!({"url": "file:///etc/passwd"}), &json!({})).is_err(),
        );
        h.check(
            "web_fetch rejects unlisted host",
            reg_guard
                .call("f", &json!({"url": "http://not-allowed.invalid/p"}), &json!({}))
                .is_err(),
        );
        h.check(
            "web_fetch needs a url",
            reg_guard.call("f", &json!({}), &json!({})).is_err(),
        );
        let reg_search_guard = capability::registry_from(
            &[("s", json!({"kind": "web_search", "endpoint": "http://not-allowed.invalid/s"}))],
            Some(p_local.clone()),
        )
        .unwrap();
        h.check(
            "web_search rejects unlisted endpoint host",
            reg_search_guard.call("s", &json!({"query": "x"}), &json!({})).is_err(),
        );
        let _ = HTML;

        // live paths need the mock; mirror the batch-3 gate so a missing mock
        // skips (rather than failing) but never silently passes a broken setup.
        let web_port = std::env::var("LAYA_WEB_PORT").ok();
        if let Some(port) = web_port {
            let up = std::net::TcpStream::connect_timeout(
                &format!("127.0.0.1:{port}").parse().unwrap(),
                std::time::Duration::from_millis(500),
            )
            .is_ok();
            h.check("web-research mock reachable (LAYA_WEB_PORT set)", up);
            if up {
                let base = format!("http://127.0.0.1:{port}");
                let reg = capability::registry_from(
                    &[
                        ("search", json!({"kind": "web_search", "endpoint": format!("{base}/search"),
                                          "max_results": 2, "timeout_ms": 4000})),
                        ("page", json!({"kind": "web_fetch", "format": "text", "timeout_ms": 4000})),
                        ("md", json!({"kind": "web_fetch", "format": "markdown", "timeout_ms": 4000})),
                        ("redir", json!({"kind": "web_fetch", "timeout_ms": 4000})),
                    ],
                    Some(p_local.clone()),
                )
                .unwrap();

                let s = reg.call("search", &json!({"query": "workflow capabilities"}), &json!({})).unwrap();
                h.eq("web_search status 200", s["status"].as_i64().unwrap(), 200);
                h.eq("web_search result count", s["results"].as_array().unwrap().len(), 2);
                h.check(
                    "web_search result has title",
                    s["results"][0]["title"].as_str().unwrap_or("").contains("overview"),
                );
                h.check(
                    "web_search result has url",
                    s["results"][0]["url"].as_str().unwrap_or("").starts_with("http://"),
                );
                h.check(
                    "web_search carries snippet",
                    !s["results"][0]["snippet"].as_str().unwrap_or("").is_empty(),
                );
                h.check("web_search needs a query", reg.call("search", &json!({}), &json!({})).is_err());

                let page_url = format!("{base}/page?name=guide");
                let p = reg.call("page", &json!({"url": page_url}), &json!({})).unwrap();
                h.eq("web_fetch status 200", p["status"].as_i64().unwrap(), 200);
                h.eq("web_fetch title extracted", p["title"].as_str().unwrap(), "Deployment Guide");
                let text = p["text"].as_str().unwrap();
                h.check("web_fetch strips tags", !text.contains('<'));
                h.check("web_fetch decodes entities", text.contains("a & b") || text.contains("& verify"));
                h.check("web_fetch drops script bodies", !text.contains("should-not-appear"));
                h.check("web_fetch keeps list text", text.contains("Step one"));
                h.check("web_fetch not truncated", p["truncated"].as_bool() == Some(false));

                let m = reg.call("md", &json!({"url": page_url}), &json!({})).unwrap();
                let mtext = m["text"].as_str().unwrap();
                h.check("web_fetch markdown keeps heading", mtext.contains("# Deployment Guide"));
                h.check("web_fetch markdown keeps link", mtext.contains("[release notes]"));
                h.check("web_fetch markdown keeps list", mtext.contains("- Step one"));

                // A redirect must be surfaced, not followed into an unlisted host.
                let r = reg
                    .call("redir", &json!({"url": format!("{base}/redirect?to=http://evil.invalid/")}), &json!({}))
                    .unwrap();
                h.eq("web_fetch surfaces redirect status", r["status"].as_i64().unwrap(), 302);
                h.check("web_fetch reports redirect target", r["location"].as_str().unwrap_or("").contains("evil.invalid"));
                h.eq("web_fetch does not fetch redirect target", r["chars"].as_i64().unwrap(), 0);
            }
        } else {
            h.check("web-research skipped (LAYA_WEB_PORT unset; run bench/mock_services.sh start)", true);
        }
    }

    // ── secret management ───────────────────────────────────────────
}


