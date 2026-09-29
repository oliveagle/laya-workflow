// Classify Feishu (飞书) unread messages into buckets and emit a digest.
//
// This is the pure half of the unread digest: it takes what the `collect`
// capability already pulled out of lark-cli and decides what each message
// *means*. No network, no exec, no browser - so it is unit-testable and runs
// in the fast gate.
//
// Why rules and not a model call: the buckets that matter here are decidable
// from surface features (who is talking, what shape the payload is, a handful
// of Chinese/English cue words). A model call per message costs seconds and
// cannot be tested; a keyword list is instant, deterministic, and a reviewer
// can see exactly why a message landed where it did. The spec still ends in a
// choice node, so the *digest* is judged by the model even though the
// per-message routing is not.

fn pick(m, key) {
    if type_of(m) == "map" && m.contains(key) { return m[key]; }
    ()
}

fn s(v) {
    if type_of(v) == "string" { return v; }
    if type_of(v) == "()" { return ""; }
    "" + v
}

// Feishu hands back `post` (rich text) bodies as an HTML fragment, so
// "<p>hello</p>" and a plain "hello" have to classify the same. `[Invalid text
// JSON]` is what the CLI emits for a payload it could not decode - it is a real
// payload, so it is kept and flagged rather than silently dropped.
fn is_space(c) {
    c == " " || c == "\t" || c == "\n" || c == "\r"
}

fn strip(str) {
    let a = 0;
    let n = str.len();
    while a < n && is_space(str.sub_string(a, 1)) { a += 1; }
    let b = n;
    while b > a && is_space(str.sub_string(b - 1, 1)) { b -= 1; }
    str.sub_string(a, b - a)
}

// This Rhai build ships neither `replace` nor `replace_all` as a builtin, nor
// `join` - the goofish plugin hand-rolls both (see its replace_all at line ~940
// and the note above strip_intent). A missing function is a *runtime* failure,
// "Function not found", so it sails straight past a compile-only gate; the
// routing assertions in scripts/rhai/check.sh are what actually catch it.
fn replace_all(str, needle, filler) {
    if needle == "" { return str; }
    let out = "";
    let i = 0;
    while i < str.len() {
        let hit = i + needle.len() <= str.len() && str.sub_string(i, needle.len()) == needle;
        if hit {
            out += filler;
            i += needle.len();
        } else {
            out += str.sub_string(i, 1);
            i += 1;
        }
    }
    out
}

fn clean_text(raw) {
    let t = s(raw);
    t = replace_all(t, "\n", " ");
    t = replace_all(t, "\r", " ");
    // Strip the markdown image/echo forms the CLI renders inline.
    t = replace_all(t, "![Image](", "[img](");
    // `post` (rich text) bodies arrive as an HTML fragment, so a real message
    // reads "<p>请问…</p>". Left in, the tags eat the first characters of the
    // snippet and the cue words still match by luck; stripped, the digest
    // quotes what the sender actually wrote.
    t = replace_all(t, "<p>", "");
    t = replace_all(t, "</p>", "");
    t = replace_all(t, "<br/>", " ");
    t = replace_all(t, "<at user_id=\"all\"></at>", "@_all");
    let out = "";
    for part in split(t, ' ') {
        let p = strip(part);
        if p == "" { continue; }
        if out != "" { out += " "; }
        out += p;
    }
    out
}

fn has_any_ci(text, words) {
    let l = text.to_lower();
    for w in words { if l.contains(w.to_lower()) { return true; } }
    false
}

// `cites_me`: the message mentions the reader. In a group chat this is the
// single strongest "this is for you" signal there is, stronger than any
// keyword, so it is checked on its own before the bucket words.
fn cites_me(text, msg) {
    let me = "@_all";
    if text.contains("@_all") { return true; }
    if text.contains("@所有人") { return true; }
    if pick(msg, "mentions_me") == true { return true; }
    false
}

// Bucket priority is deliberate. `@all` wins over everything because being
// called out in front of a whole group outranks a plain question. Direct DMs
// outrank group keywords: a DM that says "看了" is still addressed to you, and
// mis-filing it as FYI hides it. Only then do content words decide.
fn classify_one(msg, opts, ctype) {
    let text = clean_text(pick(msg, "content"));
    let mtype = s(pick(msg, "msg_type"));
    let sender = pick(msg, "sender");
    let sname = s(pick(sender, "name"));

    let b = #{};
    b.text = text;
    b.msg_type = mtype;
    b.sender = sname;
    b.message_id = s(pick(msg, "message_id"));
    b.create_time = s(pick(msg, "create_time"));
    b.chat_name = s(pick(msg, "chat_name"));
    // Carried into the digest: without it the summary says what needs you but
    // not where to go, which is the one thing a triage list has to give.
    b.app_link = s(pick(msg, "app_link"));

    // chat_type is a property of the *chat*, not of the message, so it is
    // passed in. Reading it off the message made every DM look like a group
    // post, and the DM branch below never fired.
    let p2p = ctype == "p2p";
    b.p2p = p2p;
    b.chat_type = ctype;

    let bucket = "info";
    let score = 0;

    if cites_me(text, msg) {
        bucket = "mention";
        score = 3;
    } else if p2p && has_any_ci(text, opts["action_cn"]) {
        bucket = "action";
        score = 2;
    } else if has_any_ci(text, opts["question_cn"]) || text.contains("?")
        || text.contains("？") {
        bucket = "question";
        score = 2;
    } else if has_any_ci(text, opts["decision_cn"]) {
        bucket = "decision";
        score = 2;
    } else if has_any_ci(text, opts["action_cn"]) {
        bucket = "action";
        score = 2;
    }

    // A direct message that trips no cue word is still addressed to the reader.
    // "看了" carries no keyword, but filing it under info is how a DM gets
    // missed: in a group, silence is normal; in a DM, the sender is waiting.
    // This is why a bare DM outranks the keyword buckets.
    if p2p && bucket == "info" {
        bucket = "action";
        score = 2;
    }

    // A payload the CLI could not decode is worth a human glance no matter
    // which bucket the words would have picked - it may be hiding the point.
    if text.contains("[Invalid text JSON]") {
        b.undecodable = true;
        bucket = "question";
        if score < 2 { score = 2; }
    }

    b.bucket = bucket;
    b.score = score;
    b
}

fn run(host, ctx) {
    let args = #{};
    if type_of(ctx) == "map" && ctx.contains("with") && type_of(ctx["with"]) == "map" {
        args = ctx["with"];
    }

    // The spec passes the collector's stdout through verbatim; a failed
    // collection must say so instead of digesting an empty list as "all clear",
    // which is the failure mode that makes a digest dangerous to trust.
    let raw = pick(args, "collected");
    if type_of(raw) == "string" { raw = host.json_parse(raw); }
    if type_of(raw) != "map" {
        return #{ ok: false, error: "classify: 'collected' must be an object (the collector's JSON)", buckets: #{} };
    }
    if raw["ok"] != true {
        let e = pick(raw, "err");
        return #{
            ok: false,
            error: "collect failed: " + (if type_of(e) == "string" { e } else { "unknown" }),
            buckets: #{}
        };
    }

    let chats = pick(raw, "chats");
    if type_of(chats) != "array" { chats = []; }

    // Cue words are overridable so a team can tune them without editing this
    // file; the defaults are what the repo's own vocabulary needs.
    let opts = #{
        action_cn: ["请", "麻烦", "帮忙", "确认", "review", "帮忙看", "跟进", "催", "安排", "处理一下"],
        question_cn: ["?", "？", "怎么", "如何", "为什么", "是否", "能否", "请教", "咨询", "what", "how", "why"],
        decision_cn: ["决定", "结论", "确定", "通过了", "上线", "发布", "approved", "rejected", "决定如下"]
    };
    for key in ["action_cn", "question_cn", "decision_cn"] {
        let ov = pick(args, key);
        if type_of(ov) == "array" && ov.len() > 0 { opts[key] = ov; }
    }

    let items = [];
    let buckets = #{};
    let order = ["mention", "action", "question", "decision", "info"];
    for b in order { buckets[b] = []; }

    for c in chats {
        if type_of(c) != "map" { continue; }
        let cname = s(pick(c, "chat_name"));
        let unread = pick(c, "unread");
        if type_of(unread) != "array" { continue; }
        for m in unread {
            if type_of(m) != "map" { continue; }
            let one = classify_one(m, opts, s(pick(c, "chat_type")));
            one.chat_name = cname;
            one.chat_id = s(pick(c, "chat_id"));
            items.push(one);
            buckets[one.bucket].push(one);
        }
    }

    // Sort each bucket by score then recency, so the line that most deserves
    // attention is first and the reader can stop after two.
    for b in order { buckets[b] = sort_by_score(buckets[b]); }

    let top = [];
    for b in order {
        for it in buckets[b] {
            if top.len() >= 5 { break; }
            top.push(it);
        }
    }

    let digest = [];
    for it in top {
        digest.push(#{
            bucket: it["bucket"],
            chat: it["chat_name"],
            who: it["sender"],
            when: it["create_time"],
            text: cut(shorten(it["text"], 160), 160),
            link: it["app_link"]
        });
    }

    #{ ok: true, total: items.len(), buckets: buckets, digest: digest, order: order }
}

fn shorten(t, n) {
    if t.len() <= n { return t; }
    t.sub_string(0, n) + "…"
}

fn cut(t, n) { if t.len() > n { t.sub_string(0, n) } else { t } }

fn sort_by_score(list) {
    // Insertion sort: stable, and the list is short (one bucket of unread
    // messages, not a corpus), so a pass costs nothing and keeps the original
    // order for equal scores - which is already recency order.
    // Copied with push rather than `list.clone()`: arrays are not cloneable in
    // this build, and that surfaces as a runtime "Function not found", not a
    // compile error.
    let a = [];
    for item in list { a.push(item); }
    for i in 1..a.len() {
        let key = a[i];
        let j = i - 1;
        while j >= 0 && a[j]["score"] < key["score"] {
            a[j + 1] = a[j];
            j -= 1;
        }
        a[j + 1] = key;
    }
    a
}
