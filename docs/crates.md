# crates.io reader (`websites/crates.io` + `dsl/browser/crates.json`)

Search the crates.io registry, or read one crate page into Markdown, in a
background Chrome tab over CDP. Site logic lives in `websites/crates.io/main.rhai`;
the search rows are scraped by `websites/crates.io/page/search.js`.

## Use it

```bash
# search by phrase -> ranked crate rows
laya-workflow run --spec dsl/browser/crates.json --query "serde"

# read one crate page into Markdown under ~/tmp/crates
laya-workflow run --spec dsl/browser/crates.json --query "https://crates.io/crates/serde"
```

State is optional: `mode` (`auto|search|crate`, or `read`/`package`), `count`/`limit`
(1-50) and `out_dir` (default `~/tmp/crates`).

## What it returns

A search returns `results` rows of `rank`, `name`, `version`, `description`,
`downloads`, `url` and a ready-made `docs_url`. Crate mode writes
`<slug>.md`, `<slug>_meta.json` and `images/` under `out_dir` and reports
`written`, plus `page_title` and `url`.

## Policy

```json
"policy": {
  "allow_hosts": ["127.0.0.1", "crates.io", "static.crates.io", "docs.rs"],
  "allow_paths": ["${env.HOME}/.laya-workflow", "${env.HOME}/tmp/crates"]
}
```
