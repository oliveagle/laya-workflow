# arXiv reader (`websites/arxiv.org` + `dsl/browser/arxiv.json`)

Read arXiv in a background Chrome tab over CDP. The site logic — which arXiv id
a phrase means, how a search row is read, where a paper's digest is written —
lives in `websites/arxiv.org/main.rhai`, with the page scrapers in
`websites/arxiv.org/page/*.js`.

## Use it

```bash
# search by phrase (title / authors / abstract / categories / abs+pdf links)
laya-workflow run --spec dsl/browser/arxiv.json --query "rust async" --state '{"count":3}'

# read one paper: a bare id, a versioned id, or an arxiv.org URL
laya-workflow run --spec dsl/browser/arxiv.json --query "1706.03762"
laya-workflow run --spec dsl/browser/arxiv.json --query "https://arxiv.org/abs/2401.12345v2"
```

State is optional: `mode` (`auto|search|abs`), `count`/`limit` (1-50), and
`out_dir` (default `~/tmp/arxiv`). In `auto`, a phrase searches; a bare id or an
`arxiv.org` URL reads that paper.

## What it returns

Search rows carry `rank`, `arxiv_id`, `title`, `authors`, `abstract`,
`categories`, `abs_url` and `pdf_url`.

`abs` mode reads the abstract page DOM and writes, under `out_dir`:

```
~/tmp/arxiv/
├── 1706.03762.md          # title, authors, subjects, links, abstract
└── 1706.03762_meta.json   # the same fields as JSON + fetched_at
```

## Policy

```json
"policy": {
  "allow_hosts": ["127.0.0.1", "arxiv.org"],
  "allow_paths": ["${env.HOME}/.laya-workflow", "${env.HOME}/tmp/arxiv"]
}
```

`127.0.0.1` must be allowed because the `chrome_cdp` capability launches Chrome;
`~/.laya-workflow` is the Chrome profile dir, and `~/tmp/arxiv` is where digests
are written. Writing files is gated by `policy.allow_paths`, so widen it (or
pass a different `out_dir` under it) for another location.
