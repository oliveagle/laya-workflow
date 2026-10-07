# abide 演示 fixture

`laya-workflow abide` 的离线演示：一个两规则 rubric + 好/坏两份 diff。

## 复现

```bash
BIN=./target/debug/laya-workflow

# 校验 rubric
$BIN abide validate --file bench/abide/rubric.json

# 坏 diff：往 stdout 打了 println!，应该 block（no-stdout-noise 是 edit 规则）
mkdir -p /tmp/abide-demo/src && cp /dev/null /tmp/abide-demo/src/worker.rs
$BIN abide check --diff-file bench/abide/bad.diff \
  --file-path src/worker.rs --phase edit --root /tmp/abide-demo

# 好 diff：只加了个局部变量，应该 silent
$BIN abide check --diff-file bench/abide/good.diff \
  --file-path src/worker.rs --phase edit --root /tmp/abide-demo

# turn 阶段：bad 里没有 unwrap()，no-silent-swallow 不触发，应该 silent
$BIN abide check --diff-file bench/abide/bad.diff \
  --file-path src/worker.rs --phase turn --root /tmp/abide-demo

# 汇总
$BIN abide report --root /tmp/abide-demo
```

diff 文件本身用的是简化 hunk（`--- a/` / `+++ b/` + 内容），与
`abide check --diff-json '[{"file":"...","text":"..."}]'` 等价。
