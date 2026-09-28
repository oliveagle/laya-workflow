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

fn limit_of(args, dflt, max) {
    let n = to_int(map_get(args, "count", ()), 0);
    if n <= 0 { n = to_int(map_get(args, "limit", ()), dflt); }
    if n <= 0 { n = dflt; }
    if n > max { n = max; }
    n
}

fn text_field(m, k) {
    let v = map_get(m, k, ());
    if type_of(v) == "()" { return ""; }
    "" + v
}

fn int_field(m, k) { to_int(map_get(m, k, ()), 0) }

// ── intent → plan (pure) ────────────────────────────────────────────────────

fn slug_part_ok(s) {
    if s == "" { return false; }
    let i = 0;
    while i < s.len() {
        let c = s.sub_string(i, 1);
        let ok = (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") || (c >= "0" && c <= "9")
            || c == "-" || c == "_" || c == ".";
        if !ok { return false; }
        i += 1;
    }
    true
}

// `owner/repo` from a github.com URL or a bare slug; "" when it is not one.
fn repo_of(text) {
    let t = trim_str(text);
    if t == "" { return ""; }
    let at = t.index_of("github.com/");
    let rest = if at >= 0 { t.sub_string(at + 11, t.len() - (at + 11)) } else { t };
    // Drop a leading `repos/`/`-/` and any trailing sub-path.
    if rest.starts_with("repos/") { rest = rest.sub_string(6, rest.len() - 6); }
    let out = "";
    let slashes = 0;
    let i = 0;
    while i < rest.len() {
        let c = rest.sub_string(i, 1);
        if c == "/" {
            slashes += 1;
            if slashes > 1 { break; }
        } else if c == "?" || c == "#" || c == " " {
            break;
        }
        out += c;
        i += 1;
    }
    let segs = out.split("/");
    if segs.len() != 2 { return ""; }
    let a = segs[0];
    let b = segs[1];
    if b.ends_with(".git") { b = strip_suffix(b, ".git"); }
    if a == "" || b == "" || !slug_part_ok(a) || !slug_part_ok(b) { return ""; }
    a + "/" + b
}

fn strip_suffix(s, suf) {
    if s.len() >= suf.len() && s.sub_string(s.len() - suf.len(), suf.len()) == suf {
        return s.sub_string(0, s.len() - suf.len());
    }
    s
}

// daily|weekly|monthly (trending's `since`).
fn since_of(word) {
    let w = trim_str(word).to_lower();
    if w == "daily" || w == "day" || w == "today" { return "daily"; }
    if w == "weekly" || w == "week" { return "weekly"; }
    if w == "monthly" || w == "month" { return "monthly"; }
    ""
}

fn plan(args) {
    let query = trim_str(s_of(map_get(args, "query", "")));
    if query == "" { query = trim_str(s_of(map_get(args, "q", ""))); }
    let repo = trim_str(s_of(map_get(args, "repo", "")));
    let explicit = trim_str(s_of(map_get(args, "mode", ""))).to_lower();
    let since = since_of(s_of(map_get(args, "since", "")));
    let language = trim_str(s_of(map_get(args, "language", ""))).to_lower();

    let mode = explicit;
    if mode == "" || mode == "auto" {
        if repo != "" || repo_of(query) != "" { mode = "repo"; }
        else if query == "" || query.to_lower() == "trending" || query.to_lower() == "hot"
            || query.to_lower() == "popular" || query.to_lower() == "trend" { mode = "trending"; }
        else { mode = "trending"; }
    }
    if mode == "read" || mode == "repo" || mode == "repository" { mode = "repo"; }
    else if mode == "trending" || mode == "hot" || mode == "trends" { mode = "trending"; }
    else { throw "github: mode '" + mode + "' is not supported (auto|trending|repo)"; }

    if mode == "repo" {
        if repo == "" { repo = repo_of(query); }
        if repo == "" { throw "github: repo mode needs `owner/repo` or a github.com URL"; }
    }
    if since == "" { since = "daily"; }

    let dflt = if mode == "trending" { 25 } else { 1 };
    #{ mode: mode, query: query, repo: repo, since: since, language: language,
       limit: limit_of(args, dflt, 50) }
}

// ── reads ───────────────────────────────────────────────────────────────────

fn trending(host, since, language, limit) {
    let url = "https://github.com/trending";
    if language != "" { url += "/" + language; }
    url += "?since=" + since;
    let target = host.browser_open(url);
    let results = [];
    let page_title = "";
    try {
        host.browser_wait_ready(target);
        let opts = #{ limit: limit };
        let js = "var __LAYA_OPTS__ = " + host.json_stringify(opts) + ";\n" + host.js("trending.js");
        let value = host.browser_evaluate(target, js, true);
        if type_of(value) == "map" {
            let r = map_get(value, "results", []);
            if type_of(r) == "array" { results = r; }
            page_title = text_field(value, "page_title");
        }
    } catch (e) {
        host.log("github: trending failed (" + e + ")");
    }
    host.browser_release(target);
    #{ url: url, source: "trending", since: since, page_title: page_title, results: results }
}

fn read_repo(host, repo, out_dir) {
    let url = "https://github.com/" + repo;
    let target = host.browser_open(url);
    let meta = #{};
    let written = #{};
    try {
        host.browser_wait_ready(target);
        let m = host.browser_evaluate(target, host.js("repo.js"), true);
        if type_of(m) == "map" { meta = m; }
        let at = repo.index_of("/");
        let slug = repo.sub_string(0, at) + "_" + repo.sub_string(at + 1, repo.len() - at - 1);
        let leaf = #{ target_id: target, selector: "article.markdown-body", slug: slug };
        if out_dir != "" { leaf["out_dir"] = out_dir; }
        let saved = host.save_article(leaf);
        written = map_get(saved, "written", ());
    } catch (e) {
        host.log("github: repo read failed at " + url + " (" + e + ")");
    }
    host.browser_release(target);
    #{ url: url, source: "repo", repo: repo, meta: meta,
       stars: int_field(meta, "stars"), forks: int_field(meta, "forks"),
       description: text_field(meta, "description"),
       readme_present: map_get(meta, "readme_present", false),
       page_title: text_field(meta, "title"), written: written, results: [] }
}

// ── entry point ─────────────────────────────────────────────────────────────

fn run(host, ctx) {
    let args = ctx["with"];
    let p = plan(args);
    let mode = p["mode"];
    let started = host.now();
    let out_dir = trim_str(s_of(map_get(args, "out_dir", "")));
    if out_dir == "" { out_dir = "~/tmp/github"; }

    let out = #{};
    if mode == "repo" {
        out = read_repo(host, p["repo"], out_dir);
    } else {
        out = trending(host, p["since"], p["language"], p["limit"]);
    }

    let results = map_get(out, "results", []);
    let count = 0;
    if type_of(results) == "array" { count = results.len(); }
    let written = map_get(out, "written", ());
    if mode == "repo" && type_of(written) != "()" { count = 1; }
    let finished = host.now();
    out["mode"] = mode;
    out["query"] = p["query"];
    out["repo"] = p["repo"];
    out["since"] = p["since"];
    out["language"] = p["language"];
    out["count"] = count;
    out["out_dir"] = out_dir;
    out["fetched_at"] = started["rfc3339"];
    out["finished_at"] = finished["rfc3339"];
    out
}
