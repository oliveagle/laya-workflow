# MDN Web Docs reader (`plugins/mdn` + `dsl/browser/mdn.json`)

Read MDN Web Docs. Search uses MDN's public search API (no tab, fast); reading a
doc opens the page in a background Chrome tab. Site logic lives in
`plugins/mdn/main.rhai`.

## Use it

```bash
laya-workflow run --spec dsl/browser/mdn.json --query "fetch api" --state '{"count":5}'
laya-workflow run --spec dsl/browser/mdn.json --query "/en-US/docs/Web/API/Fetch_API"
laya-workflow run --spec dsl/browser/mdn.json --query "https://developer.mozilla.org/fr/docs/Web/API/Fetch_API" --state '{"lang":"fr"}'
```

State is optional: `mode` (`auto|search|page`), `lang`/`locale` (default
`en-US`), `count`/`limit` (1-30) and `out_dir` (default `~/tmp/mdn`). `auto`
searches a phrase and reads a doc path / URL.

## What it returns

Search rows carry `rank`, `title`, `url`, `path` and `summary` (from
`developer.mozilla.org/api/v1/search`). `page` mode writes the doc under
`out_dir` as Markdown + figures, via the generic renderer with a `#content`
selector.

## Policy

```json
"policy": {
  "allow_hosts": ["127.0.0.1", "developer.mozilla.org"],
  "allow_paths": ["${env.HOME}/.laya-workflow", "${env.HOME}/tmp/mdn"]
}
```
