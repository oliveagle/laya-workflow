//! `browser-demo`: run the singleton-Chrome demo against a temporary local
//! page. Rust replacement for `bench/browser_demo.py` — no Python.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

fn free_port() -> Result<u16> {
    // A small deterministic range avoids a macOS Chrome issue with some
    // OS-assigned ephemeral loopback ports (kept from the Python original).
    for port in 18777..18787 {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(_) => return Ok(port),
            Err(_) => continue,
        }
    }
    anyhow::bail!("demo ports 18777-18786 are busy")
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html",
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    }
}

/// Serve `bench/demo-site` over plain HTTP on the given port until dropped.
fn serve_site(port: u16, site_root: PathBuf) -> std::io::Result<TcpListener> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let listener = std::sync::Arc::new(listener);
    for _ in 0..4 {
        let l = listener.clone();
        let root = site_root.clone();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = l.accept() {
                let root = root.clone();
                let _ = std::thread::spawn(move || handle(stream, &root));
            }
        });
    }
    Ok(listener.as_ref().try_clone()?)
}

fn handle(mut stream: TcpStream, root: &Path) {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    let req = String::from_utf8_lossy(&buf[..n]);
    let target = req
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/");
    let rel = target.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    let path = root.join(rel);
    if path.exists() && path.is_file() {
        let body = std::fs::read(&path).unwrap_or_default();
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            content_type(&path),
            body.len()
        );
        let _ = stream.write_all(resp.as_bytes());
        let _ = stream.write_all(&body);
    } else {
        let body = b"not found";
        let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 8\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(resp.as_bytes());
        let _ = stream.write_all(body);
    }
    let _ = stream.flush();
}

pub struct BrowserDemoOptions {
    /// Optional explicit site root (default `<manifest>/bench/demo-site`).
    pub site: Option<String>,
    /// Optional explicit port (default: first free of 18777..18787).
    pub port: Option<u16>,
    /// Optional spec override (default dsl/browser/browser_singleton.json).
    pub spec: Option<String>,
    /// Text to type into the demo page.
    pub text: Option<String>,
}

pub fn run(opts: &BrowserDemoOptions) -> Result<Value> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let site_root = opts
        .site
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("bench/demo-site"));
    if !site_root.exists() {
        anyhow::bail!("demo site root not found: {}", site_root.display());
    }
    let spec_path = opts
        .spec
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("dsl/browser/browser_singleton.json"));

    let port = match opts.port {
        Some(p) => p,
        None => free_port()?,
    };
    let listener = serve_site(port, site_root.clone()).context("start demo site server")?;
    let _keep = listener;

    let url = format!("http://127.0.0.1:{port}/index.html");
    let text = opts.text.clone().unwrap_or_else(|| "hello from Laya".to_string());
    let state = json!({ "url": url, "text": text });

    println!("demo page: {url}");
    println!("state: {state}");
    println!("spec: {}", spec_path.display());
    println!("command: laya-workflow run --spec {} --state '{{\"url\": \"{url}\", \"text\": \"{text}\"}}'", spec_path.display());

    let wf = crate::spec::load_file(&spec_path.to_string_lossy())?;
    let backend: Box<dyn crate::workflow::Decide> = Box::new(crate::backend::HeuristicBackend);
    let out = wf.run(backend.as_ref(), &state)?;
    let v = out.to_json();
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(v)
}
