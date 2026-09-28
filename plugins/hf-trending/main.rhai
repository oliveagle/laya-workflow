// ─────────────────────────────────────────────────────────────────────────────
// hf-trending — a HuggingFace model monitor for the Laya Rhai host.
//
// Given a natural-language query it asks the public HF models API for the
// current ranking, then, for every model, records the rank, likes, downloads,
// trending score, card metadata (tags / pipeline / license / file list) and the
// full model card (README.md). Everything is written under `out_dir` as:
//
//   snapshot_<key>_<ts>.json   the full structured snapshot (this run)
//   latest_<key>.json          the same, overwritten each run (rank-delta source)
//   report_<key>_<ts>.md       a human-readable ranking table
//   cards/<id>.md              each model's README (the model card)
//   history.jsonl              one compact line per run (time series)
//
// The plugin only touches the outside world through the audited host API, so
// `policy.allow_hosts` gates the API and `policy.allow_paths` gates every write.
//
// Rhai sharp edges honoured here: constants live *inside* functions, borrowed
// string slices are never returned, and there is no `let mut`.
// ─────────────────────────────────────────────────────────────────────────────

fn is_space(c) { c == " " || c == "\n" || c == "\t" || c == "\r" }

// Hand-rolled trim: `trim()` returns a borrowed slice that does not survive a
// `return`, so the host forbids relying on it inside plugin functions.
fn trim_str(s) {
    let start = 0;
    let end = s.len();
    while start < end && is_space(s.sub_string(start, 1)) { start += 1; }
    while end > start && is_space(s.sub_string(end - 1, 1)) { end -= 1; }
    s.sub_string(start, end - start)
}

// A filesystem-safe token (also used to build map keys), ASCII only.
fn safe(s) {
    let out = "";
    let i = 0;
    while i < s.len() {
        let c = s.sub_string(i, 1);
        let ok = (c >= "a" && c <= "z") || (c >= "A" && c <= "Z")
              || (c >= "0" && c <= "9") || c == "-" || c == "_" || c == ".";
        if ok { out += c; } else { out += "_"; }
        i += 1;
    }
    if out == "" { out = "all"; }
    out
}

fn join(arr, sep) {
    let out = "";
    let i = 0;
    while i < arr.len() {
        if i > 0 { out += sep; }
        out += "" + arr[i];
        i += 1;
    }
    out
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

fn s_of(v) {
    if type_of(v) == "()" { return ""; }
    "" + v
}

fn map_get(m, k, dflt) {
    if type_of(m) == "map" && m.contains(k) && type_of(m[k]) != "()" { return m[k]; }
    dflt
}

fn tag_value(tags, prefix) {
    if type_of(tags) != "array" { return ""; }
    let i = 0;
    while i < tags.len() {
        let t = tags[i];
        if type_of(t) == "string" && t.starts_with(prefix) {
            return t.sub_string(prefix.len(), t.len() - prefix.len());
        }
        i += 1;
    }
    ""
}

fn is_trending_word(q) {
    let l = q.to_lower();
    if l == "" || l == "hot" || l == "trending" || l == "trend" || l == "popular" { return true; }
    // "trending models" / "热门论文" style phrases still mean the feed; a bare
    // substring like "hot" is *not* used (it would match "photorealistic").
    l.contains("trending") || l.contains("trend")
        || q.contains("热门") || q.contains("最新") || q.contains("排行")
}

// Normalise the caller's arguments into a concrete plan (pure, no I/O).
fn plan(args) {
    let query = trim_str(s_of(map_get(args, "query", "")));
    let mode = trim_str(s_of(map_get(args, "mode", "")));
    if mode == "" || mode == "auto" {
        mode = if is_trending_word(query) { "trending" } else { "search" };
    }
    if mode != "trending" && mode != "search" {
        throw "hf-trending: mode must be trending | search | auto (got '" + mode + "')";
    }
    if mode == "search" && query == "" {
        throw "hf-trending: a search needs a non-empty query";
    }
    let limit = to_int(map_get(args, "limit", 10), 10);
    if limit < 1 { limit = 1; }
    if limit > 100 { limit = 100; }
    let sort = trim_str(s_of(map_get(args, "sort", "trendingScore")));
    if sort == "" { sort = "trendingScore"; }
    let fetch_cards = true;
    let fc = map_get(args, "fetch_cards", ());
    if type_of(fc) == "bool" { fetch_cards = fc; }
    // Optional timezone override: by default the *runner's* zone is used; pin a
    // fixed offset (minutes) / label so a shared or CI run is reproducible.
    let tz_offset_minutes = ();
    let tzm = map_get(args, "tz_offset_minutes", ());
    if type_of(tzm) == "i64" || type_of(tzm) == "f64" || type_of(tzm) == "string" {
        tz_offset_minutes = to_int(tzm, 0);
    }
    #{
        query: query,
        mode: mode,
        search: if mode == "search" { query } else { "" },
        limit: limit,
        sort: sort,
        direction: to_int(map_get(args, "direction", -1), -1),
        filter: trim_str(s_of(map_get(args, "filter", ""))),
        author: trim_str(s_of(map_get(args, "author", ""))),
        pipeline_tag: trim_str(s_of(map_get(args, "pipeline_tag", ""))),
        out_dir: trim_str(s_of(map_get(args, "out_dir", ""))),
        fetch_cards: fetch_cards,
        tz_offset_minutes: tz_offset_minutes,
        tz: trim_str(s_of(map_get(args, "tz", "")))
    }
}

// Upstream timestamps are UTC (`…Z`); render the same instant in the caller's
// zone. Returns "" when the value cannot be parsed (the raw field is kept).
fn local_iso(host, iso, off) {
    if iso == "" { return ""; }
    let ms = host.parse_time(iso);
    if type_of(ms) != "i64" { return ""; }
    host.time_format(ms, off)
}

// "2026-09-24T11:39:57+08:00" -> "2026-09-24 11:39" (for the report table).
fn short_local(iso) {
    if iso.len() < 16 { return iso; }
    iso.sub_string(0, 10) + " " + iso.sub_string(11, 5)
}

fn api_url(host, p) {
    // Locals, not script-level `const`: `Engine::call_fn` does not inject the
    // script's global namespace into the call scope.
    const HF_API = "https://huggingface.co/api/models";
    let q = [
        "sort=" + p["sort"],
        "direction=" + ("" + p["direction"]),
        "limit=" + ("" + p["limit"]),
        "full=true"
    ];
    if p["search"] != "" { q.push("search=" + host.urlencode(p["search"])); }
    if p["filter"] != "" { q.push("filter=" + host.urlencode(p["filter"])); }
    if p["author"] != "" { q.push("author=" + host.urlencode(p["author"])); }
    if p["pipeline_tag"] != "" { q.push("pipeline_tag=" + p["pipeline_tag"]); }
    HF_API + "?" + join(q, "&")
}

fn run(host, ctx) {
    const HF_WEB = "https://huggingface.co/";
    let args = #{};
    if type_of(ctx) == "map" && ctx.contains("with") && type_of(ctx["with"]) == "map" {
        args = ctx["with"];
    }
    let p = plan(args);

    let out_dir = p["out_dir"];
    if out_dir == "" { out_dir = "~/tmp/hf-trending"; }
    let key = if p["mode"] == "search" { "search_" + safe(p["query"]) } else { "trending" };

    // Time fields are UTC (canonical) *and* local. The local zone is the
    // runner's own (resolved by the engine), unless the call pins an offset.
    let now = host.now();
    let tz_name = "" + now["tz"];
    let tz_abbrev = "" + now["tz_abbrev"];
    let off = now["utc_offset_secs"];
    if type_of(p["tz_offset_minutes"]) == "i64" {
        off = p["tz_offset_minutes"] * 60;
    }
    if p["tz"] != "" { tz_name = p["tz"]; }

    let url = api_url(host, p);
    let resp = ();
    try {
        resp = host.http_get(url);
    } catch (e) {
        // `http_get` raises on any non-2xx, so a bad query surfaces here.
        throw "hf-trending: GET " + url + " failed: " + ("" + e);
    }
    if resp["status"] != 200 {
        throw "hf-trending: " + url + " returned HTTP " + resp["status"];
    }
    let models = host.json_parse(resp["text"]);
    if type_of(models) != "array" {
        throw "hf-trending: unexpected HF response (expected a JSON array)";
    }

    // Previous ranks → a per-model rank delta (the "monitoring" part).
    let latest_path = out_dir + "/latest_" + key + ".json";
    let prev = #{};
    let prev_read = host.read_file(latest_path);
    if prev_read["exists"] {
        let old = host.json_parse(prev_read["text"]);
        if type_of(old) == "map" && old.contains("ranks") && type_of(old["ranks"]) == "map" {
            let ranks = old["ranks"];
            for id in ranks.keys() { prev[id] = to_int(ranks[id], 0); }
        }
    }

    let cards_dir = out_dir + "/cards";
    let records = [];
    let rank_map = #{};
    let cards_saved = 0;
    let cards_failed = 0;

    let index = 0;
    while index < models.len() {
        let m = models[index];
        if type_of(m) == "map" {
            let id = s_of(map_get(m, "id", ""));
            if id != "" {
                let rank = index + 1;
                let tags = map_get(m, "tags", []);
                let card_path = "";
                let card_bytes = 0;
                let card_error = "";
                if p["fetch_cards"] {
                    // A missing card (404) must not sink the whole run: record
                    // it and carry on to the next model.
                    try {
                        let card = host.http_get(HF_WEB + id + "/raw/main/README.md");
                        if card["status"] == 200 {
                            card_path = cards_dir + "/" + safe(id) + ".md";
                            card_bytes = host.write_file(card_path, card["text"])["bytes"];
                            cards_saved += 1;
                        } else {
                            card_error = "HTTP " + card["status"];
                            cards_failed += 1;
                        }
                    } catch (e) {
                        card_error = "card fetch failed: " + ("" + e);
                        cards_failed += 1;
                    }
                }
                let rank_delta = ();
                let is_new = true;
                if prev.contains(id) {
                    is_new = false;
                    rank_delta = prev[id] - rank;
                }
                rank_map[id] = rank;
                let model_url = HF_WEB + id;
                records.push(#{
                    rank: rank,
                    id: id,
                    author: s_of(map_get(m, "author", "")),
                    likes: to_int(map_get(m, "likes", 0), 0),
                    downloads: to_int(map_get(m, "downloads", 0), 0),
                    trending_score: to_int(map_get(m, "trendingScore", 0), 0),
                    pipeline_tag: s_of(map_get(m, "pipeline_tag", "")),
                    library_name: s_of(map_get(m, "library_name", "")),
                    license: tag_value(tags, "license:"),
                    tags: tags,
                    created_at: s_of(map_get(m, "createdAt", "")),
                    created_at_local: local_iso(host, s_of(map_get(m, "createdAt", "")), off),
                    last_modified: s_of(map_get(m, "lastModified", "")),
                    last_modified_local: local_iso(host, s_of(map_get(m, "lastModified", "")), off),
                    gated: map_get(m, "gated", false),
                    "private": map_get(m, "private", false),
                    files: if type_of(map_get(m, "siblings", [])) == "array" {
                        (map_get(m, "siblings", [])).len()
                    } else { 0 },
                    url: model_url,
                    card_url: model_url + "/raw/main/README.md",
                    card_path: card_path,
                    card_bytes: card_bytes,
                    card_error: card_error,
                    is_new: is_new,
                    rank_delta: rank_delta
                });
            }
        }
        index += 1;
    }

    if records.len() == 0 {
        throw "hf-trending: the HF API returned no models for " + url;
    }

    let stamp = "" + now["unix_ms"];
    let snapshot_path = out_dir + "/snapshot_" + key + "_" + stamp + ".json";
    let snapshot = #{
        capability: "hf-trending",
        mode: p["mode"],
        query: p["query"],
        sort: p["sort"],
        direction: p["direction"],
        limit: p["limit"],
        source_url: url,
        fetched_at: now["rfc3339"],
        fetched_at_local: host.time_format(now["unix_ms"], off),
        fetched_at_unix_ms: now["unix_ms"],
        tz: tz_name,
        tz_abbrev: tz_abbrev,
        utc_offset_secs: off,
        count: records.len(),
        out_dir: out_dir,
        models: records
    };
    host.write_file(snapshot_path, host.json_stringify(snapshot));

    host.write_file(latest_path, host.json_stringify(#{
        fetched_at: now["rfc3339"],
        fetched_at_local: host.time_format(now["unix_ms"], off),
        fetched_at_unix_ms: now["unix_ms"],
        tz: tz_name,
        utc_offset_secs: off,
        mode: p["mode"],
        query: p["query"],
        limit: p["limit"],
        count: records.len(),
        ranks: rank_map,
        models: records
    }));

    // A human-readable ranking report.
    let lines = [];
    lines.push("# HuggingFace " + p["mode"]
        + (if p["query"] != "" { " — " + p["query"] } else { "" }));
    lines.push("");
    lines.push("_fetched " + host.time_format(now["unix_ms"], off)
        + " (" + tz_name + ") · " + url + "_");
    lines.push("");
    lines.push("| # | model | likes | downloads | trending | Δrank | updated (local) | pipeline | license |");
    lines.push("|---|-------|-------|-----------|----------|-------|-----------------|----------|---------|");
    let r = 0;
    while r < records.len() {
        let row = records[r];
        let delta = "0";
        if row["is_new"] { delta = "new"; }
        else if row["rank_delta"] > 0 { delta = "+" + row["rank_delta"]; }
        else if row["rank_delta"] < 0 { delta = "" + row["rank_delta"]; }
        lines.push("| " + row["rank"] + " | [" + row["id"] + "](" + row["url"] + ") | "
            + row["likes"] + " | " + row["downloads"] + " | " + row["trending_score"]
            + " | " + delta + " | " + short_local(row["last_modified_local"])
            + " | " + row["pipeline_tag"] + " | " + row["license"] + " |");
        r += 1;
    }
    lines.push("");
    let report_path = out_dir + "/report_" + key + "_" + stamp + ".md";
    host.write_file(report_path, join(lines, "\n") + "\n");

    // One compact line per run → a cheap time series for later analysis.
    let history_path = out_dir + "/history.jsonl";
    let prev_hist = host.read_file(history_path);
    let acc = if prev_hist["exists"] { "" + prev_hist["text"] } else { "" };
    if acc != "" && acc.sub_string(acc.len() - 1, 1) != "\n" { acc += "\n"; }
    let line = host.json_stringify(#{
        fetched_at: now["rfc3339"],
        fetched_at_local: host.time_format(now["unix_ms"], off),
        fetched_at_unix_ms: now["unix_ms"],
        mode: p["mode"],
        query: p["query"],
        limit: p["limit"],
        count: records.len(),
        ranks: rank_map
    });
    host.write_file(history_path, acc + line + "\n");

    #{
        mode: p["mode"],
        query: p["query"],
        sort: p["sort"],
        direction: p["direction"],
        requested_limit: p["limit"],
        count: records.len(),
        source_url: url,
        out_dir: out_dir,
        "key": key,
        snapshot_path: snapshot_path,
        latest_path: latest_path,
        report_path: report_path,
        history_path: history_path,
        cards_dir: cards_dir,
        cards_saved: cards_saved,
        cards_failed: cards_failed,
        fetched_at: now["rfc3339"],
        fetched_at_local: host.time_format(now["unix_ms"], off),
        tz: tz_name,
        tz_abbrev: tz_abbrev,
        utc_offset_secs: off,
        models: records
    }
}
