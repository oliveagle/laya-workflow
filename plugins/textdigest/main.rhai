// ─────────────────────────────────────────────────────────────────────────────
// textdigest — a tiny, entirely local Laya plugin.
//
// It exists to show the extension seam end-to-end *offline*: a workflow declares
// `{"kind": "plugin", "plugin": "textdigest"}`, this script runs on the same Rhai
// host as the alphaXiv downloader, and the workflow routes on a structured
// result. No browser, no network — but the same policy and sandbox apply.
// ─────────────────────────────────────────────────────────────────────────────

fn run(host, ctx) {
    // Locals, not script-level `const`: `Engine::call_fn` does not inject a
    // script's global namespace into the call scope.
    const STOP = ["the", "a", "of", "and", "to", "in", "is", "it", "for", "on", "with"];
    const TOP_N = 5;

    let args = ctx["with"];
    let text = "";
    if args.contains("text") && type_of(args["text"]) != "()" { text = "" + args["text"]; }

    // Tokenize by hand: Rhai's `trim()`/`replace()` return borrowed slices that
    // do not survive a return, and the string methods that matter here are cheap.
    let lower = text.to_lower();
    let toks = [];
    let cur = "";
    let i = 0;
    while i < lower.len() {
        let c = lower.sub_string(i, 1);
        let is_word = (c >= "a" && c <= "z") || (c >= "0" && c <= "9");
        if is_word {
            cur += c;
        } else {
            if cur != "" { toks.push(cur); cur = ""; }
        }
        i += 1;
    }
    if cur != "" { toks.push(cur); }

    let counts = #{};
    let words = 0;
    for t in toks {
        if STOP.contains(t) { continue; }
        words += 1;
        if counts.contains(t) { counts[t] += 1; } else { counts[t] = 1; }
    }

    // Pick the TOP_N most frequent, most-recently-inserted ties first.
    let top = [];
    while top.len() < TOP_N {
        let winner = "";
        let hits = 0;
        for t in toks {
            if !counts.contains(t) { continue; }
            let c = counts[t];
            if c > hits { hits = c; winner = t; }
        }
        if winner == "" { break; }
        top.push(#{ token: winner, count: hits });
        counts.remove(winner);
    }

    #{
        chars: text.len(),
        words: words,
        tokens: toks.len(),
        top: top,
        digested_at: host.now()["rfc3339"]
    }
}
