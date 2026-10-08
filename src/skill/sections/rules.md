# rules — 把 AGENTS.md 的规则变成可执行的检查

从 [coldteadotai/abide](https://github.com/coldteadotai/abide) 移植。目标：让
coding agent **真的遵守** AGENTS.md / CLAUDE.md 里的规则，而不是只读一遍。

核心循环：
1. **compile**：把规则文本交给 agent（或你自己）变成 `.rules/rubric.json` 里的
   machine-checkable 规则。
2. **check**：每次编辑（`--phase edit`）或整轮改动（`--phase turn`）跑一遍检查，
   违规概率 >= `thresholds.act`（默认 0.8）就发 `{kind:"block"}`；>= `flag`
   （默认 0.5）发 `{kind:"notice"}`；否则 `{kind:"silent"}`。
3. **report / audit**：事后从 `~/.laya-workflow/rules/events.jsonl` 汇总哪些规则被违反过。

## 快速上手

```bash
# 1. 生成骨架（<repo>/.rules/ + 空的 rubric.json + .rulesignore；events 走 ~/.laya-workflow/rules/）
laya-workflow rules init

# 2. 拿到 compile prompt，粘进一个 agent 会话，让它写 rubric
laya-workflow rules compile
# 或者先 --init 再打印：
laya-workflow rules compile --init

# 3. 校验 rubric
laya-workflow rules validate

# 4. 对一次改动做检查（离线，用 question.heuristic 做确定性判定）
laya-workflow rules check \
  --diff-file /tmp/hunk.diff \
  --file-path src/worker.rs \
  --phase edit

# 5. 用真模型判定（laya-tch /v1/systemone）
laya-workflow rules check \
  --diff-file /tmp/hunk.diff \
  --file-path src/worker.rs \
  --base-url http://127.0.0.1:8400

# 6. 汇总
laya-workflow rules report
laya-workflow rules audit
```

## Rubric 结构

`.rules/rubric.json`：

```json
{
  "version": 1,
  "compiledAt": "2026-10-08T00:00:00Z",
  "sources": [{ "path": "AGENTS.md", "sha": "…", "scope": "**" }],
  "thresholds": { "act": 0.8, "flag": 0.5 },
  "rules": [
    {
      "id": "no-stdout-noise",
      "text": "Hook 只能输出 JSON，不能打别的东西到 stdout。",
      "source": { "path": "AGENTS.md", "line": 12 },
      "when": "edit",
      "check": {
        "type": "model",
        "question": {
          "type": "boolean",
          "instructions": "这次编辑是不是往 stdout 写了 JSON 以外的东西？",
          "criteria": { "true": "违规", "false": "合规" },
          "heuristic": {
            "match_any": ["println!", "console.log"],
            "p_violated": 0.95,
            "p_ok": 0.05
          }
        }
      },
      "status": "active"
    }
  ]
}
```

关键点：
- **规则 id 必须 kebab-case 且唯一**。
- **model-checked 规则必须写 `when`**（`edit` 按 hunk、`turn` 整轮）。
- **lint 型规则**（`check.type = "lint"`）只记录、不判定——那本该是 linter 的活。
- **`question.heuristic` 是 laya 的离线扩展**：没配它也能跑（离线判定为合规，
  在线用真模型判）；配了它 `rules check` 不用模型也能确定性判。

## 判定输出

`check` 子命令把 hook 输出直接打到 stdout：

```json
{ "kind": "block", "reason": "Rules: … Repair src/worker.rs now, …" }
```

- `block`：概率 ≥ act。给 agent 的一句话修复指令，含具体规则 id / 源文件行号 /
  严重度。hook 场景下 exit 0（不把 agent 会话打崩），stdout 是信号。
- `notice`：概率 ≥ flag 但 < act。写在 `systemMessage`，不 block agent。
- `silent`：合规，或者违规落在已删除文件上（没法修复）。

## 事件

每次 check 追加一行到 **`~/.laya-workflow/rules/events.jsonl`**（用户级审计日志，不进 git）。
规则文件本身在 `<repo>/.rules/rubric.json`（可提交、团队共享）。
事件 schema 与原 abide 一致：

```
{"kind":"check","at":"…","phase":"edit","files":[…],"rules":1,
 "latency_ms":2,"verdicts":[{"rule_id":"…","probability":0.95,
 "band":"act","file":"src/worker.rs"}],"blocked":true}
```

`report` 汇总，`audit` pretty-print 全部事件。

## 与原 abide 的差异

| 原 abide 项目 | laya-workflow rules |
|---|---|
| Node CLI + Claude Code / Codex / OpenCode / Pi 4 套 hook | `laya-workflow rules <sub>` 独立子命令；hook 接线留给宿主自己（`kind: block` JSON 就是约定） |
| `compile` 委托给 headless agent | `rules compile` 打印 compile prompt 给用户，粘进 agent 会话即可 |
| 判定走 TypeSafe / Vercel AI Gateway 的 `jev-latest` | `--base-url` 指 `laya-tch` 的 `/v1/systemone`；或用 `question.heuristic` 完全离线 |
| rubric 在 `.abide/rubric.json`（跟代码提交） | rubric 在 `<repo>/.rules/rubric.json`（跟代码提交） |
| events 在 `.abide/events.jsonl`（跟代码提交） | events 在 `~/.laya-workflow/rules/events.jsonl`（用户级，不进 git） |
| `violationProbability` / `bandFor` / thresholds / repair reason | 一模一样 |

## 下一步

- `skill --section dsl` 看 spec 如何写
- `skill --section run` 看怎么在真实工作流里跑
- 参考 [coldteadotai/abide 的 README](https://github.com/coldteadotai/abide)
  理解设计取舍（"hook never breaks your agent" 是它的第一条规则）
