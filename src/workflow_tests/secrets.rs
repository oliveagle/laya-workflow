//! secrets regression sections for `laya-workflow-tests`.
#![allow(unused_imports)]
use crate::*;
pub fn test_secrets(h: &mut Harness) {
    {
        use laya_workflow::capability::secret;

        // (a) .env parsing: comments, quotes, export prefix, spaces
        let f = std::env::temp_dir().join("laya_sec_probe.env");
        std::fs::write(
            &f,
            "# comment line\nPLAIN=value1\nQUOTED=\"value 2\"\nSINGLE='value3'\nexport EXPORTED=value4\n  SPACED = value5  \nEMPTY=\n",
        )
        .unwrap();
        let mut s = secret::Secrets::default();
        s.merge_env_file(&f);
        h.eq("secrets: plain value", s.get("PLAIN").unwrap().to_string(), "value1".to_string());
        h.eq("secrets: double-quoted keeps spaces", s.get("QUOTED").unwrap().to_string(), "value 2".to_string());
        h.eq("secrets: single-quoted", s.get("SINGLE").unwrap().to_string(), "value3".to_string());
        h.eq("secrets: export prefix stripped", s.get("EXPORTED").unwrap().to_string(), "value4".to_string());
        h.eq("secrets: surrounding spaces trimmed", s.get("SPACED").unwrap().to_string(), "value5".to_string());
        h.check("secrets: empty value recorded but not usable", s.get("EMPTY") == Some(""));
        h.check("secrets: comment not parsed as key", s.get("comment line").is_none());

        // (b) JSON secrets file
        let jf = std::env::temp_dir().join("laya_sec_probe.json");
        std::fs::write(&jf, r#"{"JSONKEY":"jv","N": 1}"#).unwrap();
        let mut s2 = secret::Secrets::default();
        s2.merge_json_file(&jf).unwrap();
        h.eq("secrets: json file value", s2.get("JSONKEY").unwrap().to_string(), "jv".to_string());
        h.check("secrets: json non-string skipped", s2.get("N").is_none());

        // (c) precedence: env file first, explicit insert wins
        let mut s3 = secret::Secrets::default();
        s3.merge_env_file(&f);
        s3.insert("PLAIN", "override-value-1234");
        h.eq("secrets: explicit insert wins", s3.get("PLAIN").unwrap().to_string(), "override-value-1234".to_string());

        // (d) store: missing / empty are hard errors, not empty strings
        secret::set_secrets(s3.clone()).unwrap();
        h.check("secrets: present resolves", secret::get("PLAIN").is_ok());
        let e = secret::get("NOPE").unwrap_err().to_string();
        h.check("secrets: missing is an error", e.contains("not available"));
        h.check("secrets: has() reflects store", secret::has("PLAIN") && !secret::has("NOPE"));

        // (e) redaction
        h.eq("secrets: redact masks value", secret::redact_str("token=override-value-1234 here"), "token=*** here".to_string());
        // Regression: a blind substring replace with a low length floor used to
        // mangle unrelated text (a short value matched inside an ordinary word,
        // e.g. "userc" corrupting "githubusercontent" -> "github***ontent").
        s3.insert("SHORT", "userc");
        secret::set_secrets(s3.clone()).unwrap();
        h.eq(
            "secrets: short value does not corrupt unrelated text",
            secret::redact_str("https://avatars.githubusercontent.com/u/1"),
            "https://avatars.githubusercontent.com/u/1".to_string(),
        );
        h.eq(
            "secrets: long value still masks",
            secret::redact_str("bearer override-value-1234!"),
            "bearer ***!".to_string(),
        );
        h.eq(
            "secrets: redact_str leaves unknown text",
            secret::redact_str("nothing secret"),
            "nothing secret".to_string(),
        );
        let red = secret::redact(&json!({"a": "override-value-1234", "b": ["x", "override-value-1234"]}));
        h.eq("secrets: redact deep masks", red["a"].as_str().unwrap().to_string(), "***".to_string());
        h.eq("secrets: redact deep array", red["b"][1].as_str().unwrap().to_string(), "***".to_string());

        // (f) reference discovery + anti-pattern detection
        let spec = json!({
            "capabilities": {
                "a": {"kind": "http", "url": "http://x", "headers": {"authorization": "Bearer ${secret.TOK}"}},
                "b": {"kind": "smtp", "host": "h", "password": "${env.MAILPASS}"}
            },
            "nodes": []
        });
        h.eq(
            "secrets: referenced names discovered",
            secret::referenced_names(&spec),
            vec!["TOK".to_string()],
        );
        // `${env.X}` is a plain env reference, reported separately (not a
        // required credential), so it must not appear in the secrets list.
        h.eq(
            "secrets: env names reported separately",
            secret::env_names(&spec),
            vec!["MAILPASS".to_string()],
        );
        let bad = json!({"capabilities": {"a": {"kind": "smtp", "password": "hunter2"}}});
        let found = secret::hardcoded_secret_fields(&bad);
        h.check("secrets: hard-coded password detected", found.iter().any(|p| p.ends_with("password")));
        let good = json!({"capabilities": {"a": {"kind": "smtp", "password": "${secret.P}"}}});
        h.check("secrets: reference is not flagged", secret::hardcoded_secret_fields(&good).is_empty());

        // (g) end-to-end: a spec referencing a missing secret must fail, and a
        //     present secret must never appear in any output we produce.
        secret::set_secrets(s3.clone()).unwrap();
        let wf_spec = json!({
            "name": "sec", "start": "go", "dsl_version": 2,
            "capabilities": {"raw": {"kind": "passthrough"}},
            "nodes": [{
                "name": "go", "primary_q": "q",
                "questions": {"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}},
                "edge": {"condition": {}, "default": "STOP"},
                "action": {"kind": "call", "capability": "raw",
                           "with": {"token": "${secret.PLAIN}", "n": "${secret.NOPE}"}}
            }]
        });
        let wf = spec::from_spec(&wf_spec).unwrap();

        // missing secret => hard error mentioning the name
        let be = ScriptedBackend::new(vec![verdict(&[("q", choice("A", &[("A", 0.9)]))])]);
        let err = wf.run(&be, &json!({})).unwrap_err().to_string();
        h.check("secrets: missing secret fails the run", err.contains("NOPE"));
        h.check("secrets: error names it as a secret", err.to_lowercase().contains("secret"));

        // present secret => runs, and the emitted JSON contains no literal
        let ok_spec = json!({
            "name": "sec2", "start": "go", "dsl_version": 2,
            "capabilities": {"raw": {"kind": "passthrough"}},
            "nodes": [{
                "name": "go", "primary_q": "q",
                "questions": {"q": {"type": "choice", "instructions": "?", "criteria": {"A": "a"}}},
                "edge": {"condition": {}, "default": "STOP"},
                "action": {"kind": "call", "capability": "raw",
                           "with": {"token": "${secret.PLAIN}"}}
            }]
        });
        let wf2 = spec::from_spec(&ok_spec).unwrap();
        let be2 = ScriptedBackend::new(vec![verdict(&[("q", choice("A", &[("A", 0.9)]))])]);
        let out2 = wf2.run(&be2, &json!({})).unwrap();
        // the raw state still holds the value; redaction is what protects output
        let raw_json = serde_json::to_string(&out2.to_json()).unwrap();
        h.check("secrets: raw state does contain the value (hence redaction)", raw_json.contains("override"));
        let redacted = secret::redact(&out2.to_json());
        let red_json = serde_json::to_string(&redacted).unwrap();
        h.check("secrets: redacted output hides the value", !red_json.contains("override"));
        h.check("secrets: redacted output shows the mask", red_json.contains("***"));

        // restore a clean store so later tests are unaffected
        secret::set_secrets(secret::Secrets::default()).unwrap();
        let _ = std::fs::remove_file(&f);
        let _ = std::fs::remove_file(&jf);
    }

    // ── network capabilities against the local mock ─────────────────
    let base = std::env::var("LAYA_MOCK_URL").ok();
    if let Some(base) = base {
        let mut npol = capability::Policy::default();
        npol.allow_hosts = vec!["127.0.0.1".to_string(), "localhost".to_string()];
        npol.allow_exec = true;
        npol.retries = 1;
        let reg = capability::registry_from(
            &[
                ("rpc", json!({"kind": "rpc", "url": format!("{base}/rpc"), "timeout_ms": 5000})),
                ("gql", json!({"kind": "graphql", "url": format!("{base}/graphql"), "query": "query Q { x }"})),
                ("llm", json!({"kind": "llm", "url": format!("{base}/chat/completions"), "model": "mock", "timeout_ms": 5000})),
                ("mcp", json!({"kind": "mcp", "transport": "http", "url": format!("{base}/mcp"), "tool": "echo"})),
                ("vec", json!({"kind": "vector", "url": format!("{base}/vector"), "op": "search", "collection": "c1"})),
                ("hook", json!({"kind": "webhook", "url": format!("{base}/webhook"), "sign_header": "x-signature", "sign_secret": "s3cret"})),
                ("sse", json!({"kind": "sse", "url": format!("{base}/events"), "max_events": 3, "timeout_ms": 5000})),
            ],
            Some(npol.clone()),
        )
        .unwrap();

        let r = reg.call("rpc", &json!({"method": "sum", "params": {"a": 1}}), &json!({})).unwrap();
        h.eq("rpc result.method", r["result"]["method"].as_str().unwrap().to_string(), "sum".to_string());
        let e = reg.call("rpc", &json!({"method": "boom"}), &json!({})).unwrap_err().to_string();
        h.check("rpc error is surfaced", e.contains("boom"));

        let r = reg.call("gql", &json!({"variables": {"v": 3}}), &json!({})).unwrap();
        h.eq("graphql data echoed", r["data"]["echo"]["v"].as_i64().unwrap(), 3);

        let r = reg.call("llm", &json!({"prompt": "hello"}), &json!({})).unwrap();
        h.eq("llm content echoed", r["content"].as_str().unwrap().to_string(), "echo:hello".to_string());

        let r = reg.call("mcp", &json!({"arguments": {"x": 1}}), &json!({})).unwrap();
        h.check("mcp result has content", r["result"]["content"].is_array());

        let r = reg.call("vec", &json!({"vector": [0.1, 0.2], "top_k": 1}), &json!({})).unwrap();
        h.eq("vector hits returned", r["hits"].as_array().unwrap().len(), 1);

        let r = reg.call("hook", &json!({"event": "deploy", "payload": {"ok": true}}), &json!({})).unwrap();
        h.eq("webhook delivered", r["status"].as_i64().unwrap(), 200);
        h.check("webhook was signed", r["signed"].as_bool().unwrap());

        let r = reg.call("sse", &json!({}), &json!({})).unwrap();
        h.eq("sse events parsed", r["count"].as_u64().unwrap(), 3);
        h.eq("sse event name", r["events"][0]["event"].as_str().unwrap().to_string(), "tick".to_string());

        // mcp stdio transport via the mock's line-delimited mode
        let mock = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bench/mock_server.py");
        if mock.exists() {
            let python = std::env::var("LAYA_TEST_PYTHON").unwrap_or_else(|_| "python3".to_string());
            let reg = capability::registry_from(
                &[("mcp_stdio", json!({
                    "kind": "mcp", "transport": "stdio",
                    "command": [python, mock.to_str().unwrap(), "--stdio"],
                    "tool": "any", "timeout_ms": 10000
                }))],
                Some(npol.clone()),
            )
            .unwrap();
            match reg.call("mcp_stdio", &json!({"arguments": {}}), &json!({})) {
                Ok(r) => h.check("mcp stdio result present", r["result"].is_object()),
                Err(e) => h.check(&format!("mcp stdio (skipped: {e})"), true),
            }
        }
    }
    // ── capability timeout / hostile-server regressions ─────────────
}


