# rules 演示 fixture

`laya-workflow rules` （rules 子命令）的离线演示：一个两规则 rubric + 好/坏两份 diff。

## 复现

```bash
BIN=./target/debug/laya-workflow

# 校验 rubric
$BIN rules validate --file bench/rules/rubric.json

# 坏 diff：往 stdout 打了 println!，应该 block（no-stdout-noise 是 edit 规则）
mkdir -p /tmp/rules-demo/src && cp /dev/null /tmp/rules-demo/src/worker.rs
$BIN rules check --diff-file bench/rules/bad.diff \
  --file-path src/worker.rs --phase edit --root /tmp/rules-demo

# 好 diff：只加了个局部变量，应该 silent
$BIN rules check --diff-file bench/rules/good.diff \
  --file-path src/worker.rs --phase edit --root /tmp/rules-demo

# turn 阶段：bad 里没有 unwrap()，no-silent-swallow 不触发，应该 silent
$BIN rules check --diff-file bench/rules/bad.diff \
  --file-path src/worker.rs --phase turn --root /tmp/rules-demo

# 汇总
$BIN rules report --root /tmp/rules-demo
```

diff 文件本身用的是简化 hunk（`--- a/` / `+++ b/` + 内容），与
`rules check --diff-json '[{"file":"...","text":"..."}]'` 等价。
