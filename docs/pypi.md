# PyPI reader (`websites/pypi.org` + `dsl/browser/pypi.json`)

Search the Python Package Index, or read one project page into Markdown, in a
background Chrome tab over CDP. Site logic lives in `websites/pypi.org/main.rhai`;
`websites/pypi.org/page/search.js` scrapes the search rows and
`websites/pypi.org/page/project.js` reads the project header.

## Use it

```bash
# search by phrase -> ranked project rows
laya-workflow run --spec dsl/browser/pypi.json --query "requests"

# read one project page into Markdown under ~/tmp/pypi
laya-workflow run --spec dsl/browser/pypi.json --query "https://pypi.org/project/requests/"
```

State is optional: `mode` (`auto|search|project`, or `read`/`package`),
`count`/`limit` (1-50) and `out_dir` (default `~/tmp/pypi`).

## What it returns

A search returns `results` rows of `rank`, `name`, `version`, `description`,
`updated` and `url`. Project mode writes `<slug>.md`, `<slug>_meta.json` and
`images/` under `out_dir`, and reports the project `version` and `written`.

## Policy

```json
"policy": {
  "allow_hosts": ["127.0.0.1", "pypi.org", "files.pythonhosted.org"],
  "allow_paths": ["${env.HOME}/.laya-workflow", "${env.HOME}/tmp/pypi"]
}
```
