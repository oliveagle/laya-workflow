# Bing web search (`plugins/bing` + `dsl/browser/bing.json`)

Web search on Bing in a background Chrome tab over CDP: a phrase in, organic
result rows out. The page scraper is `plugins/bing/page/search.js`; the intent
logic is `plugins/bing/main.rhai`.

## Use it

```bash
laya-workflow run --spec dsl/browser/bing.json --query "typescript ai framework"
laya-workflow run --spec dsl/browser/bing.json --query "特斯拉 财报" --state '{"mkt":"zh-CN","count":10}'
```

State is optional: `count`/`limit` (1-30, Bing usually returns ~10 per page) and
`mkt` (e.g. `en-US`, `zh-CN`).

## What it returns

Rows carry `rank`, `title`, `url`, `site` (the display URL) and `snippet`.

## Policy

```json
"policy": {
  "allow_hosts": ["127.0.0.1", "www.bing.com"],
  "allow_paths": []
}
```
