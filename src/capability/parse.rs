//! Spec -> `Capability` parsing.
//!
//! Kept separate from `mod.rs` so the registry/dispatch logic stays readable:
//! every kind's accepted fields live here, next to each other.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use super::{
    browser, data, goal, local, net, proto, service, store, sys, web, AgentCap, Capability,
    ExecCap, HttpCap,
};

pub(super) fn parse_cap(name: &str, def: &Value) -> Result<Capability> {
    let kind = def
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("capability {name:?} missing 'kind'"))?;
    match kind {
        "http" => Ok(Capability::Http(HttpCap {
            method: def.get("method").and_then(|v| v.as_str()).unwrap_or("POST").to_uppercase(),
            url: def
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("capability {name:?} (http) missing 'url'"))?
                .to_string(),
            headers: def.get("headers").and_then(|v| v.as_object()).cloned().unwrap_or_default(),
            body: def.get("body").cloned(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(15_000),
            expect_json: def.get("expect_json").and_then(|v| v.as_bool()).unwrap_or(true),
        })),
        "exec" => Ok(Capability::Exec(ExecCap {
            argv: def
                .get("argv")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .ok_or_else(|| anyhow!("capability {name:?} (exec) missing 'argv'"))?,
            cwd: def.get("cwd").and_then(|v| v.as_str()).map(str::to_string),
            env: def.get("env").and_then(|v| v.as_object()).cloned().unwrap_or_default(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
            max_output: def.get("max_output").and_then(|v| v.as_u64()).unwrap_or(256 << 10) as usize,
        })),
        "agent" | "app_server" => Ok(Capability::Agent(AgentCap {
            transport: def
                .get("transport")
                .and_then(|v| v.as_str())
                .unwrap_or(if def.get("command").is_some() { "stdio" } else { "http" })
                .to_string(),
            url: def.get("url").and_then(|v| v.as_str()).map(str::to_string),
            command: def
                .get("command")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()),
            session: def.get("session").and_then(|v| v.as_str()).map(str::to_string),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
            max_output: def.get("max_output").and_then(|v| v.as_u64()).unwrap_or(256 << 10) as usize,
        })),
        // ── local / pure ──
        "datetime" => Ok(Capability::Datetime(local::DatetimeCap {
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("now").to_string(),
            format: def.get("format").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            offset_secs: def.get("offset_secs").and_then(|v| v.as_i64()).unwrap_or(0),
        })),
        "text" => Ok(Capability::Text(local::TextCap {
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("length").to_string(),
            pattern: def.get("pattern").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            replacement: def.get("replacement").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            separator: def.get("separator").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            hash: def.get("hash").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "file" => Ok(Capability::File(local::FileCap {
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("read").to_string(),
            allow_roots: def
                .get("allow_roots")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(std::path::PathBuf::from)).collect())
                .unwrap_or_default(),
            max_bytes: def.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(1 << 20) as usize,
        })),
        "sqlite" => Ok(Capability::Sqlite(local::SqliteCap {
            db: def.get("db").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("query").to_string(),
            readonly: def.get("readonly").and_then(|v| v.as_bool()).unwrap_or(true),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(10_000),
        })),
        "shell" => Ok(Capability::Shell(local::ShellCap {
            command: def
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("capability {name:?} (shell) missing 'command'"))?
                .to_string(),
            cwd: def.get("cwd").and_then(|v| v.as_str()).map(str::to_string),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
        })),
        // ── network ──
        "rpc" => Ok(Capability::Rpc(net::RpcCap {
            url: def.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            method: def.get("method").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(15_000),
            id: def.get("id").and_then(|v| v.as_i64()),
        })),
        "graphql" => Ok(Capability::Graphql(net::GraphqlCap {
            url: def.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            query: def.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            variables: def.get("variables").cloned().unwrap_or_else(|| json!({})),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(15_000),
            auth_header: def.get("auth_header").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            auth_value: def.get("auth_value").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "llm" | "chat" => Ok(Capability::Llm(net::LlmCap {
            url: def.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            model: def.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            system: def.get("system").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            max_tokens: def.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(256),
            temperature: def.get("temperature").and_then(|v| v.as_f64()).unwrap_or(0.0),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
            auth_header: def.get("auth_header").and_then(|v| v.as_str()).unwrap_or("authorization").to_string(),
            auth_value: def.get("auth_value").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "mcp" => Ok(Capability::Mcp(net::McpCap {
            headers: def.get("headers").and_then(|v| v.as_object()).cloned().unwrap_or_default(),
            transport: def
                .get("transport")
                .and_then(|v| v.as_str())
                .unwrap_or(if def.get("command").is_some() { "stdio" } else { "http" })
                .to_string(),
            url: def.get("url").and_then(|v| v.as_str()).map(str::to_string),
            command: def
                .get("command")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()),
            tool: def.get("tool").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
        })),
        "vector" => Ok(Capability::Vector(net::VectorCap {
            url: def.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("search").to_string(),
            collection: def.get("collection").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(15_000),
        })),
        "webhook" => Ok(Capability::Webhook(net::WebhookCap {
            url: def.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(10_000),
            sign_header: def.get("sign_header").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            sign_secret: def.get("sign_secret").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "sse" => Ok(Capability::Sse(net::SseCap {
            url: def.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(15_000),
            max_events: def.get("max_events").and_then(|v| v.as_u64()).unwrap_or(100) as usize,
        })),
        "passthrough" | "noop" => Ok(Capability::Passthrough),
        "json" => Ok(Capability::Json(data::JsonCap {
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("pick").to_string(),
        })),
        "csv" => Ok(Capability::Csv(data::CsvCap {
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("parse").to_string(),
            delimiter: def
                .get("delimiter")
                .and_then(|v| v.as_str())
                .and_then(|s| s.chars().next())
                .unwrap_or(','),
            headers: def.get("headers").and_then(|v| v.as_bool()).unwrap_or(true),
        })),
        "xml" => Ok(Capability::Xml(data::XmlCap {
            tag: def.get("tag").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("tags").to_string(),
        })),
        "markdown" | "md" => Ok(Capability::Markdown(data::MarkdownCap {
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("headings").to_string(),
        })),
        "diff" => Ok(Capability::Diff(data::DiffCap {
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("lines").to_string(),
        })),
        "validate" | "schema" => Ok(Capability::Validate(data::ValidateCap)),
        "math" => Ok(Capability::Math(data::MathCap {
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("eval").to_string(),
        })),
        "hash" => Ok(Capability::Hash(data::HashCap {
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("sha256").to_string(),
        })),
        "graph" => Ok(Capability::Graph(data::GraphCap {
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("reachable").to_string(),
        })),
        "tokenize" | "tokens" => Ok(Capability::Tokenize(data::TokenizeCap {
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("count").to_string(),
            chars_per_token: def.get("chars_per_token").and_then(|v| v.as_f64()).unwrap_or(4.0),
        })),
        "cron" => Ok(Capability::Cron(data::CronCap {
            expr: def.get("expr").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "keyvalue" | "kv" => Ok(Capability::KeyValue(store::KeyValueCap {
            path: def.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("get").to_string(),
            key: def.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "cache" => Ok(Capability::Cache(store::CacheCap {
            path: def.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("get").to_string(),
            ttl_secs: def.get("ttl_secs").and_then(|v| v.as_i64()).unwrap_or(0),
        })),
        "queue" => Ok(Capability::Queue(store::QueueCap {
            path: def.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("length").to_string(),
        })),
        "metrics" | "sysinfo" => Ok(Capability::Metrics(sys::MetricsCap {
            what: def.get("what").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            disk_path: def.get("disk_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "notify" | "notify_local" => Ok(Capability::NotifyLocal(sys::NotifyLocalCap {
            path: def.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            bell: def.get("bell").and_then(|v| v.as_bool()).unwrap_or(false),
            timestamp: def.get("timestamp").and_then(|v| v.as_bool()).unwrap_or(true),
        })),
        "tcp" => Ok(Capability::Tcp(proto::TcpCap {
            host: def.get("host").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            port: def.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16,
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(5000),
            until: def.get("until").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "udp" => Ok(Capability::Udp(proto::UdpCap {
            host: def.get("host").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            port: def.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16,
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(3000),
        })),
        "redis" => Ok(Capability::Redis(proto::RedisCap {
            host: def.get("host").and_then(|v| v.as_str()).unwrap_or("127.0.0.1").to_string(),
            port: def.get("port").and_then(|v| v.as_u64()).unwrap_or(6379) as u16,
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(5000),
            password: def.get("password").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "nats" => Ok(Capability::Nats(proto::NatsCap {
            host: def.get("host").and_then(|v| v.as_str()).unwrap_or("127.0.0.1").to_string(),
            port: def.get("port").and_then(|v| v.as_u64()).unwrap_or(4222) as u16,
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(5000),
        })),
        "mqtt" => Ok(Capability::Mqtt(proto::MqttCap {
            host: def.get("host").and_then(|v| v.as_str()).unwrap_or("127.0.0.1").to_string(),
            port: def.get("port").and_then(|v| v.as_u64()).unwrap_or(1883) as u16,
            client_id: def.get("client_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(5000),
        })),
        "smtp" | "mail_send" => Ok(Capability::Smtp(proto::SmtpCap {
            host: def.get("host").and_then(|v| v.as_str()).unwrap_or("127.0.0.1").to_string(),
            port: def.get("port").and_then(|v| v.as_u64()).unwrap_or(25) as u16,
            from: def.get("from").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(8000),
            username: def.get("username").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            password: def.get("password").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "archive" => Ok(Capability::Archive(proto::ArchiveCap {
            path: def.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("list").to_string(),
            dest: def.get("dest").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
        })),
        "s3" | "object_store" => Ok(Capability::S3(service::S3Cap {
            endpoint: def.get("endpoint").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            bucket: def.get("bucket").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            prefix: def.get("prefix").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(10_000),
            headers: def.get("headers").and_then(|v| v.as_object()).cloned().unwrap_or_default(),
        })),
        "prometheus" | "prom" => Ok(Capability::Prometheus(service::PrometheusCap {
            url: def.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(8000),
        })),
        "kafka" => Ok(Capability::Kafka(service::KafkaCap {
            url: def.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            topic: def.get("topic").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(10_000),
        })),
        "pdf" => Ok(Capability::Pdf(service::PdfCap {
            path: def.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            op: def.get("op").and_then(|v| v.as_str()).unwrap_or("text").to_string(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
        })),
        "sql" | "database" => Ok(Capability::Sql(service::SqlCap {
            driver: def.get("driver").and_then(|v| v.as_str()).unwrap_or("sqlite3").to_string(),
            dsn: def.get("dsn").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000),
        })),
        // ── singleton Chrome / CDP ──
        "browser" | "chrome_cdp" => Ok(Capability::Browser(browser::BrowserCap {
            endpoint: def.get("endpoint").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            chrome_binary: def.get("chrome_binary").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            profile_dir: def.get("profile_dir").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            extension_path: def.get("extension_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            launch: def.get("launch").and_then(|v| v.as_bool()).unwrap_or(false),
            startup_timeout_ms: def.get("startup_timeout_ms").and_then(|v| v.as_u64()).unwrap_or(15_000),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(20_000),
            max_text: def.get("max_text").and_then(|v| v.as_u64()).unwrap_or(6_000) as usize,
            max_owned_pages: def
                .get("max_owned_pages")
                .and_then(|v| v.as_u64())
                .unwrap_or(32)
                .clamp(1, 500) as usize,
            owned_idle_ms: def
                .get("owned_idle_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(5 * 60_000)
                .clamp(1_000, 24 * 60 * 60_000),
        })),
        "web_search" | "search" => Ok(Capability::WebSearch(web::WebSearchCap {
            endpoint: def.get("endpoint").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            method: def.get("method").and_then(|v| v.as_str()).unwrap_or("GET").to_uppercase(),
            headers: def.get("headers").and_then(|v| v.as_object()).cloned().unwrap_or_default(),
            results_path: def.get("results_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            query_param: def.get("query_param").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            title_field: def.get("title_field").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            url_field: def.get("url_field").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            snippet_field: def.get("snippet_field").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(15_000),
            max_results: def.get("max_results").and_then(|v| v.as_u64()).unwrap_or(10) as usize,
            max_bytes: def.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(256 << 10) as usize,
        })),
        "goal_runner" | "agent_goal" => Ok(Capability::GoalRunner(goal::GoalRunnerCap {
            runner: def.get("runner").and_then(|v| v.as_str()).unwrap_or("cxgo").to_string(),
            args: def
                .get("args")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(1_800_000),
            max_output: def.get("max_output").and_then(|v| v.as_u64()).unwrap_or(256 << 10) as usize,
            reports_dir: def.get("reports_dir").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        "web_fetch" | "fetch_url" => Ok(Capability::WebFetch(web::WebFetchCap {
            format: def.get("format").and_then(|v| v.as_str()).unwrap_or("text").to_string(),
            headers: def.get("headers").and_then(|v| v.as_object()).cloned().unwrap_or_default(),
            timeout_ms: def.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(20_000),
            max_bytes: def.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(256 << 10) as usize,
            endpoint: def.get("endpoint").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            method: def.get("method").and_then(|v| v.as_str()).unwrap_or("POST").to_uppercase(),
            body_field: def.get("body_field").and_then(|v| v.as_str()).unwrap_or("url").to_string(),
            body_urls_array: def.get("body_urls_array").and_then(|v| v.as_bool()).unwrap_or(false),
            body: def.get("body").cloned().unwrap_or(Value::Null),
            result_path: def.get("result_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            text_field: def.get("text_field").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            title_field: def.get("title_field").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })),
        other => bail!(
            "capability {name:?}: unknown kind {other:?} (see docs/benchmarks/laya_workflow_capabilities_*.md)"
        ),
    }
}
