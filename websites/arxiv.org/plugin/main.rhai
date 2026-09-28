// ─────────────────────────────────────────────────────────────────────────────
// arXiv reader — Laya workflow plugin (Chrome/CDP).
//
// Two jobs: turn a phrase into arXiv search-result rows (title, authors,
// abstract, categories, abs/pdf links), and hand a paper's abs page to the
// engine's generic renderer (`host.save_article`) so it lands as Markdown.
//
// Site logic only: which arXiv id a phrase means, how a search row is read, and
// where a paper is saved. The Rust base owns the tab, the CDP transport, the
// policy gate and the tab lifecycle. Rhai sharp edges honoured: constants live
// *inside* functions, borrowed slices are never returned, no `let mut`.
// ─────────────────────────────────────────────────────────────────────────────

fn is_space(c) { c == " " || c == "\n" || c == "\t" || c == "\r" }

fn trim_str(s) {
    let start = 0;
    let end = s.len();
    while start < end && is_space(s.sub_string(start, 1)) { start += 1; }
    while end > start && is_space(s.sub_string(end - 1, 1)) { end -= 1; }
    s.sub_string(start, end - start)
}

fn s_of(v) {
    if type_of(v) == "()" { return ""; }
    "" + v
}

fn map_get(m, k, dflt) {
    if type_of(m) == "map" && m.contains(k) && type_of(m[k]) != "()" { return m[k]; }
    dflt
}

fn parse_i(s, dflt) {
    let t = trim_str(s);
    if t == "" { return dflt; }
    let i = 0;
    let neg = false;
    if t.sub_string(0, 1) == "-" { neg = true; i = 1; }
    let val = 0;
    let digits = 0;
    while i < t.len() {
        let d = "0123456789".index_of(t.sub_string(i, 1));
        if d < 0 { break; }
        val = val * 10 + d;
        digits += 1;
        i += 1;
    }
    if digits == 0 { return dflt; }
    if neg { -val } else { val }
}

fn to_int(v, dflt) {
    let t = type_of(v);
    if t == "i64" { return v; }
    if t == "f64" { return parse_i("" + v, dflt); }
    if t == "string" { return parse_i(v, dflt); }
    dflt
}

fn sf(m, k) {
    let v = map_get(m, k, ());
    if type_of(v) == "()" { return ""; }
    "" + v
}

fn nf(m, k) { to_int(map_get(m, k, ()), 0) }

// A filesystem-safe leaf for an arXiv id (old-style ids carry a `/`).
fn fs_safe(s) {
    let out = "";
    let i = 0;
    while i < s.len() {
        let c = s.sub_string(i, 1);
        if c == "/" || c == "\\" { out += "_"; } else { out += c; }
        i += 1;
    }
    out
}

// ── id handling (pure) ──────────────────────────────────────────────────────

fn cut_at(s, stops) {
    let cut = s.len();
    let i = 0;
    while i < s.len() {
        let c = s.sub_string(i, 1);
        for x in stops { if c == x { cut = i; break; } }
        if cut < s.len() { break; }
        i += 1;
    }
    s.sub_string(0, cut)
}

fn strip_suffix(s, suf) {
    if s.len() >= suf.len() && s.sub_string(s.len() - suf.len(), suf.len()) == suf {
        return s.sub_string(0, s.len() - suf.len());
    }
    s
}

// True for `2401.12345` / `2401.12345v2` style ids (digits, one dot, optional vN).
fn is_id_like(s) {
    if s.len() < 7 { return false; }
    let dots = 0;
    let digits = 0;
    let i = 0;
    while i < s.len() {
        let c = s.sub_string(i, 1);
        if c >= "0" && c <= "9" { digits += 1; }
        else if c == "." { dots += 1; }
        else if c != "v" && c != "V" { return false; }
        i += 1;
    }
    dots == 1 && digits >= 6
}

// The arXiv id inside a URL (`/abs/<id>`, `/pdf/<id>.pdf`) or a bare id.
fn arxiv_id_of(text) {
    let t = trim_str(text);
    if t == "" { return ""; }
    let at = t.index_of("/abs/");
    if at >= 0 {
        let rest = t.sub_string(at + 5, t.len() - (at + 5));
        return cut_at(rest, ["?", "#", "/"]);
    }
    at = t.index_of("/pdf/");
    if at >= 0 {
        let rest = t.sub_string(at + 5, t.len() - (at + 5));
        return strip_suffix(cut_at(rest, ["?", "#", "/"]), ".pdf");
    }
    let w = cut_at(t, [" ", "\t", "\n"]);
    if is_id_like(w) { return w; }
    ""
}

// ── intent → plan (pure) ────────────────────────────────────────────────────

fn plan(args) {
    let query = trim_str(s_of(map_get(args, "query", "")));
    if query == "" { query = trim_str(s_of(map_get(args, "q", ""))); }
    let paper = trim_str(s_of(map_get(args, "paper", "")));
    if paper == "" { paper = trim_str(s_of(map_get(args, "url", ""))); }
    if paper == "" { paper = trim_str(s_of(map_get(args, "arxiv_id", ""))); }
    let explicit = trim_str(s_of(map_get(args, "mode", ""))).to_lower();

    let mode = explicit;
    let id = "";
    if mode == "" || mode == "auto" {
        if paper != "" { mode = "abs"; }
        else if query != "" {
            let found = arxiv_id_of(query);
            if found != "" && !query.contains(" ") { mode = "abs"; }
            else { mode = "search"; }
        } else {
            throw "arxiv: needs a 'query' to search, or a paper id / arxiv.org URL";
        }
    }
    if mode == "abs" || mode == "paper" || mode == "pdf" || mode == "id" {
        mode = "abs";
        id = arxiv_id_of(paper);
        if id == "" { id = arxiv_id_of(query); }
        if id == "" { throw "arxiv: abs mode needs an arXiv id (2401.12345) or an arxiv.org URL"; }
    } else if mode == "search" || mode == "find" || mode == "query" || mode == "list" {
        mode = "search";
        if query == "" { throw "arxiv: search mode needs a query"; }
    } else {
        throw "arxiv: mode '" + mode + "' is not supported (auto|search|abs)";
    }

    let dflt = if mode == "search" { 10 } else { 1 };
    let limit = to_int(map_get(args, "count", ()), 0);
    if limit <= 0 { limit = to_int(map_get(args, "limit", ()), dflt); }
    if limit <= 0 { limit = dflt; }
    if limit > 50 { limit = 50; }

    #{ mode: mode, query: query, id: id, limit: limit }
}

// ── reads ───────────────────────────────────────────────────────────────────

fn search(host, query, limit) {
    let url = "https://arxiv.org/search/?searchtype=all&size=50&query=" + host.urlencode(query);
    let target = host.browser_open(url);
    let results = [];
    let page_title = "";
    try {
        host.browser_wait_ready(target);
        let opts = #{ limit: limit };
        let js = "var __LAYA_OPTS__ = " + host.json_stringify(opts) + ";\n" + host.js("search.js");
        let value = host.browser_evaluate(target, js, true);
        if type_of(value) == "map" {
            let r = map_get(value, "results", []);
            if type_of(r) == "array" { results = r; }
            page_title = sf(value, "title");
        }
    } catch (e) {
        host.log("arxiv: search failed (" + e + ")");
    }
    host.browser_release(target);
    #{ url: url, source: "search", page_title: page_title, results: results }
}

fn save_paper(host, id, out_dir) {
    let url = "https://arxiv.org/abs/" + id;
    let target = host.browser_open(url);
    let meta = #{};
    try {
        host.browser_wait_ready(target);
        let value = host.browser_evaluate(target, host.js("abs.js"), true);
        if type_of(value) == "map" { meta = value; }
    } catch (e) {
        host.log("arxiv: abs read failed for " + url + " (" + e + ")");
    }
    host.browser_release(target);

    let base = sf(meta, "arxiv_id");
    if base == "" { base = strip_suffix(id, sf(meta, "version")); }
    if base == "" { base = id; }
    let abs_url = sf(meta, "abs_url");
    if abs_url == "" { abs_url = url; }
    let title = sf(meta, "title");
    if title == "" { title = base; }
    let authors = sf(meta, "authors_text");
    let abstract = sf(meta, "abstract");
    let dateline = sf(meta, "dateline");
    let subjects = sf(meta, "subjects");
    let comments = sf(meta, "comments");
    let journal = sf(meta, "journal_ref");
    let doi = sf(meta, "doi");
    let pdf = sf(meta, "pdf_url");
    let html_url = sf(meta, "html_url");

    // A small, readable digest the engine can hand to a human or a model.
    let md = "# " + title + "\n\n";
    if authors != "" { md += "- **Authors**: " + authors + "\n"; }
    if dateline != "" { md += "- **Submitted**: " + dateline + "\n"; }
    if subjects != "" { md += "- **Subjects**: " + subjects + "\n"; }
    if comments != "" { md += "- **Comments**: " + comments + "\n"; }
    if journal != "" { md += "- **Journal ref**: " + journal + "\n"; }
    if doi != "" { md += "- **DOI**: " + doi + "\n"; }
    md += "- **arXiv**: " + base + "\n";
    md += "- **Abstract page**: " + abs_url + "\n";
    if pdf != "" { md += "- **PDF**: " + pdf + "\n"; }
    if html_url != "" { md += "- **HTML**: " + html_url + "\n"; }
    md += "\n## Abstract\n\n" + abstract + "\n";

    let written = #{};
    try {
        let leaf = fs_safe(base);
        let md_path = out_dir + "/" + leaf + ".md";
        let w = host.write_file(md_path, md);
        let meta_path = out_dir + "/" + leaf + "_meta.json";
        let w2 = host.write_file(meta_path, host.json_stringify(#{
            arxiv_id: base, version: sf(meta, "version"), title: title,
            authors: sf(meta, "authors_text"), subjects: subjects, comments: comments,
            dateline: dateline, doi: doi, abs_url: abs_url, pdf_url: pdf,
            fetched_at: host.now()["rfc3339"]
        }));
        written = #{
            out_dir: out_dir,
            markdown_file: w["path"],
            markdown_chars: md.len(),
            meta_file: w2["path"]
        };
    } catch (e) {
        host.log("arxiv: write failed under " + out_dir + " (" + e + ")");
    }
    #{ url: url, source: "abs", arxiv_id: base, saved: written.contains("markdown_file"),
       page_title: title, markdown_chars: md.len(), written: written, results: [] }
}

// ── entry point ─────────────────────────────────────────────────────────────

fn run(host, ctx) {
    let args = ctx["with"];
    let p = plan(args);
    let mode = p["mode"];
    let started = host.now();
    let out_dir = trim_str(s_of(map_get(args, "out_dir", "")));
    if out_dir == "" { out_dir = "~/tmp/arxiv"; }

    let out = #{};
    if mode == "search" {
        out = search(host, p["query"], p["limit"]);
    } else {
        out = save_paper(host, p["id"], out_dir);
    }

    let results = map_get(out, "results", []);
    let count = 0;
    if type_of(results) == "array" { count = results.len(); }
    if mode == "abs" && map_get(out, "saved", false) == true { count = 1; }
    let finished = host.now();
    out["mode"] = mode;
    out["query"] = p["query"];
    out["count"] = count;
    out["out_dir"] = out_dir;
    out["fetched_at"] = started["rfc3339"];
    out["finished_at"] = finished["rfc3339"];
    out
}
