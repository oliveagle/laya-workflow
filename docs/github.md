# GitHub reader (`websites/github.com` + `dsl/browser/github.json`)

Read GitHub's trending page, or one repository's stars / forks / description and
its README, in a background Chrome tab over CDP. Site logic lives in
`websites/github.com/plugin/main.rhai`; `websites/github.com/plugin/page/trending.js` and
`page/repo.js` scrape the DOM (both wait for GitHub's late React paint).

## Use it

```bash
# the trending page (default: today), optionally for one language
laya-workflow run --spec dsl/browser/github.json --query trending --state '{"since":"daily","count":5}'
laya-workflow run --spec dsl/browser/github.json --query trending --state '{"since":"weekly","language":"rust"}'

# one repository: stars / forks / description + its README as Markdown under ~/tmp/github
laya-workflow run --spec dsl/browser/github.json --query "BurntSushi/ripgrep"
```

State is optional: `mode` (`auto|trending|repo`), `since`
(`daily|weekly|monthly`), `language`, `count`/`limit` (1-50) and `out_dir`
(default `~/tmp/github`).

## What it returns

Trending returns `results` rows of `rank`, `name` (`owner/repo`), `owner`,
`repo`, `url`, `description`, `language`, `stars`, `forks`, `stars_today` and
`stars_today_text`. Repo mode reports `repo`, `stars`, `forks`, `description`,
`readme_present`, `page_title`, and writes the README as `<owner>_<repo>.md`
(+ `_meta.json`, `images/`) under `out_dir`.

## Policy

```json
"policy": {
  "allow_hosts": ["127.0.0.1", "github.com", "raw.githubusercontent.com",
                  "user-images.githubusercontent.com", "camo.githubusercontent.com"],
  "allow_paths": ["${env.HOME}/.laya-workflow", "${env.HOME}/tmp/github"]
}
```
