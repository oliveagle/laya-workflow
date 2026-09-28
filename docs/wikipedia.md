# Wikipedia reader (`websites/wikipedia.org` + `dsl/browser/wikipedia.json`)

Read Wikipedia in a background Chrome tab over CDP. Site logic lives in
`websites/wikipedia.org/plugin/main.rhai` with the page scrapers in
`websites/wikipedia.org/plugin/page/*.js`.

## Use it

```bash
# search articles (title / snippet / link)
laya-workflow run --spec dsl/browser/wikipedia.json --query "transformer neural network" --state '{"count":5}'

# read an article into Markdown (space-separated words become the article path)
laya-workflow run --spec dsl/browser/wikipedia.json --query "Rust (programming language)" --state '{"mode":"page"}'

# any language, and a plain URL works too
laya-workflow run --spec dsl/browser/wikipedia.json --query "机器学习" --state '{"lang":"zh"}'
laya-workflow run --spec dsl/browser/wikipedia.json --query "https://zh.wikipedia.org/wiki/Transformer模型"
```

A bare phrase searches; pass `mode=page` (or a `title`) to read a page. State is optional: `mode` (`auto|search|page`), `lang` (default `en`, or the
subdomain of a `wikipedia.org` URL), `count`/`limit` (1-50) and `out_dir`
(default `~/tmp/wikipedia`).

## What it returns

Search rows carry `rank`, `title`, `url`, `snippet` and `meta` (size / word
count / last edited). `page` mode reads the article body and writes, under
`out_dir`:

```
~/tmp/wikipedia/
├── Transformer_(deep_learning).md    # the article, with figures
└── Transformer_(deep_learning)_meta.json
```

A `page/clean.js` first strips the site chrome (edit links, series/nav boxes,
maintenance banners) and puts the page title back at the top of the body, so the
generic renderer reads the article and nothing else.

## Policy

```json
"policy": {
  "allow_hosts": ["127.0.0.1", "en.wikipedia.org", "zh.wikipedia.org"],
  "allow_paths": ["${env.HOME}/.laya-workflow", "${env.HOME}/tmp/wikipedia"]
}
```

Every language is a separate host, and the policy matches hosts exactly — add the
`<lang>.wikipedia.org` you want. `127.0.0.1` is the CDP endpoint and
`~/.laya-workflow` the Chrome profile dir.
