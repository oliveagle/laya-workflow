# docs.rs reader (`websites/docs.rs` + `dsl/browser/docsrs.json`)

Search crate releases on docs.rs, or read one crate's rendered API docs into
Markdown, in a background Chrome tab over CDP. Site logic lives in
`websites/docs.rs/main.rhai`; `websites/docs.rs/page/search.js` scrapes the
release rows.

## Use it

```bash
# search crate releases by phrase -> ranked rows
laya-workflow run --spec dsl/browser/docsrs.json --query "serde"

# read one crate's API docs into Markdown under ~/tmp/docsrs
laya-workflow run --spec dsl/browser/docsrs.json --query "https://docs.rs/serde/latest/serde/"
```

State is optional: `mode` (`auto|search|crate`, or `read`/`docs`/`api`),
`count`/`limit` (1-50) and `out_dir` (default `~/tmp/docsrs`).

## What it returns

A search returns `results` rows of `rank`, `name`, `crate`, `version`,
`description`, `released` and `url`. Crate mode writes `<slug>.md`,
`<slug>_meta.json` and `images/` under `out_dir`, and reports `docs_url` and
`written`.

## Policy

```json
"policy": {
  "allow_hosts": ["127.0.0.1", "docs.rs"],
  "allow_paths": ["${env.HOME}/.laya-workflow", "${env.HOME}/tmp/docsrs"]
}
```
