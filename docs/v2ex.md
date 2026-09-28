# V2EX reader (`websites/v2ex.com` + `dsl/browser/v2ex.json`)

Read V2EX in a background Chrome tab over CDP. Site logic lives in
`websites/v2ex.com/plugin/main.rhai`, with `websites/v2ex.com/plugin/page/{list,topic}.js` for the DOM.

## Use it

```bash
# a tab: latest / hot / tech / creative / play / apple / jobs / deals / city / qna / all
laya-workflow run --spec dsl/browser/v2ex.json --query hot --state '{"count":10}'

# a node's topics
laya-workflow run --spec dsl/browser/v2ex.json --query "go/rust"

# one topic with its replies (a bare id or a /t/<id> URL, with or without #replyN)
laya-workflow run --spec dsl/browser/v2ex.json --query "https://www.v2ex.com/t/1245140"
```

State is optional: `mode` (`auto|node|topic`, or a tab word) and `count`/`limit`
(1-100).

## What it returns

List rows carry `rank`, `id`, `title`, `url`, `node`, `node_url`, `author`,
`time`, `last_reply_by` and `replies`. A topic returns `title`, `node`,
`content` (the opening post) and a `replies` array of `{floor, id, user, time,
content}`.

## Policy

```json
"policy": {
  "allow_hosts": ["127.0.0.1", "www.v2ex.com"],
  "allow_paths": []
}
```
