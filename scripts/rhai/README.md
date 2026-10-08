# scripts/rhai — the fast loop for editing a Rhai plugin

A plugin here is a 2000+ line `main.rhai`. The engine that runs it is the only
compiler, and the only obvious way to use it is `laya-workflow run` on the real
DSL — which browses live pages, costs ~23s, and reports a parse error as a line
number without saying which line is wrong. These four tools replace that with a
feedback loop that costs **0.33s**.

| tool | what it answers | cost |
|---|---|---|
| `laya-workflow plugin compile plugin.rhai` | does it parse? | 0.07s |
| `laya-workflow plugin compile plugin.rhai --bisect` | which *block* is the parse error in? | 0.09s |
| `laya-workflow plugin check plugin.rhai --body "…"` | what does this one helper return? | 0.15s |
| `laya-workflow plugin check --probe` | what does this Rhai build even support? | 0.25s, once |
| `check.sh` | the whole gate: parse → routing → learn | 0.33s |
| `check.sh --cargo` | …and the Rust tests, when `src/**` changed | +1s warm |

## The loop

```sh
laya-workflow plugin check                 # after every edit
laya-workflow plugin compile <plugin> --bisect   # only when it does not compile
laya-workflow plugin check <plugin> --run --body '#{ x: my_helper(2) }'
laya-workflow plugin check         # once per session, not once per question
cargo test --release                  # once, at the end, only if src/** changed
laya-workflow run --spec dsl/browser/goofish_item.json --query "…"   # once, last
```

Order matters: the cheap stages fail first, the expensive one runs once.

## Why each piece exists

`laya-workflow run` (and `laya-workflow plugin compile` for syntax-only checks) — `laya-workflow run` is the only compiler, but running a whole
workflow to learn "does this parse" throws away the entire run. This drives the
plugin capability alone. `--bisect` exists because the parser *recovers*: a
stray `)` three hundred lines from anything real is reported as
`Expecting '{' to start a statement block (line 1965)`, because that is where it
gave up, not where the mistake is. So the question worth asking is not "what is
wrong on line 1965" but "what is the least I can take out to make it parse" —
the predicate is a clean compile, and it doubles outward from the reported line
then halves in. Verified against an injected `let bogus = 1);`: window 1935–1995,
reported line marked, **9 compiles, 0.09s**. Doing that by hand was 7 tool calls
and three throwaway scripts.

The plugin loader (Rhai runtime) — a plugin is one file with `fn run(host, ctx)` at the bottom
and a library above it. This keeps the library verbatim and swaps in your own
`run`, so "does my helper do what I think" is a 0.15s call instead of a 23s
browse, and it tests the *real* helpers rather than a copy that can drift.

**`laya-workflow plugin check --probe`** — this Rhai build is not the one in the docs. Every fact below was
re-derived by trial and error during one session, which is exactly the cost this
file exists to end. One question per file, so a construct the build rejects does
not kill the run; three outcomes kept apart (`OK` / `NO-COMPILE` / `NO-RUNTIME`)
because "it compiled" is not "it worked".

    sort() returns unit, sorts in place      Array::join missing
    for k in map not iterable (.keys())     closures / nested fn forbidden
    push(map) into an array COPIES it        `with` is a reserved keyword
    try is a statement, not an expression    String::replace compiles, returns null
    sub_string/len() are char-based (CJK ok) int()/floor()/round() exist, int_of does not

**`check.sh`** — the one command. Compile, then assert the intent routing still
works (`价格进化 CMP 170HX` keeps the noun and drops the verb), then fold:
`mode=evolve` over a *copy* of the corpus, so it runs the real `learn()` and
writes the real JSON without browsing or touching your out_dir. It then compares
the corpus item count before and after and **fails if it grew** — a fold that
records items went browsing, which is both the 20s cost and the signal that an
intent word leaked into a search term.

## Gotchas that cost real time

- **`try` is a statement.** `let v = try { … }` does not compile; assign inside
  the block. This one produced a false bug report: a helper appeared to return
  null on float input purely because the test swallowed the value.
- **A map pushed into an array is copied.** Mutating the local afterwards does
  nothing; write it back (`arr[i] = m`).
- **Heredocs in this shell are flaky.** zsh intermittently fails with
  `parse error near '<'`, even inside `<<'EOF'`, and a swallowed terminator
  leaves the target file half-written. After any heredoc write, verify:
  `bash -n f.sh`.
- **`cargo test` once, at the end.** The 87 Rust tests do not execute Rhai, so
  running them before touching `src/**` tests nothing you wrote.
- The engine enforces `policy.allow_paths`; a scratch dir outside it fails with
  a wall of text. `check.sh` asks the spec where that is.
