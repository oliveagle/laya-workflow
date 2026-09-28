# Hacker News reader (`plugins/hackernews` + `dsl/browser/hackernews.json`)

Read Hacker News in a background Chrome tab over CDP. The site logic — which
feed a word means, how a front-page row is read, how a comment tree is flattened
— lives entirely in `plugins/hackernews/main.rhai`, so it can change without
rebuilding the engine. The Rust base owns the tab, the CDP transport, the policy
gate and the tab lifecycle.

## Use it

```bash
# the front page (best/new/ask/show/jobs also work as bare words)
laya-workflow run --spec dsl/browser/hackernews.json --query top --state '{"count":5}'

# a full-text search over story titles/URLs (public Algolia index, no browser)
laya-workflow run --spec dsl/browser/hackernews.json --query "rust async" --state '{"count":10}'

# one discussion, with its comment tree (depths come from the indent image)
laya-workflow run --spec dsl/browser/hackernews.json --query "item?id=49874728"
```

`--query` may be a bare mode word (`top`, `best`, `new`, `newest`, `ask`,
`show`, `jobs`, `front`, `hot`), a phrase (full-text search), a story id, or a
`news.ycombinator.com/item?id=<id>` URL. State is optional: `mode`
(`auto|top|best|new|ask|show|jobs|item|search`) and `count`/`limit` (1-100).

## What it returns

Every list row carries `rank`, `title`, `url`, `site`, `points`, `comments`,
`user`, `age` and the HN `permalink`. An item also returns `title`, `points`,
`comments_total` and a `comments` array whose entries carry `depth`, `user`,
`age` and `text`.

## Policy

```json
"policy": {
  "allow_hosts": ["127.0.0.1", "news.ycombinator.com", "hn.algolia.com"],
  "allow_paths": []
}
```

`127.0.0.1` must be allowed because the `chrome_cdp` capability with
`launch: true` opens Chrome on the CDP endpoint. The front page / item read is
DOM-only; search goes through `hn.algolia.com` with `host.http_get`.
