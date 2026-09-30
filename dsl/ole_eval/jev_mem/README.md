# dsl/ole_eval/jev_mem/ — Jev-Mem System-One 控制器 DSL 原生移植（路线 B）

把 https://github.com/libingzheren/Jev-Mem 的 System-One 控制器（arxiv:2609.23986）原生写成 `laya-workflow` DSL spec，使我们不再依赖 Jev-Mem 的 Python 仓库就能跑同形态决策图。System-One 只用 `POST /v1/systemone` 与 `laya-tch` 通信，System-Two 答案生成仍是 OpenAI-兼容端点（不在这批 spec 内）。

> 路线 A 调研：`docs/research/topics/agentic_memory/jev_mem_laya_repro_20260930.md`（同仓库）+ `code/agentic_memory/laya_http_backend.py`（ole-eval 仓库）。

## 文件清单

| spec | 对应 Jev-Mem 决策集 | 说明 |
|---|---|---|
| `memory_type.json` | `jev_mem_policies.WritePolicy.memory_type` + `jev_questions.memory_type_questions` | 4 个 noul（episodic / semantic / procedural / preference）→ `threshold` 多语义 label |
| `admission.json` | `WritePolicy.assess_observation` + `jev_questions.observation_questions` (admission 部分) | 5 个 noul (should_store / future_utility / importance / novelty / redundancy) → `gate` 3 段 (STORE/REVIEW/DROP) |
| `relation_pair.json` | `WritePolicy.relations` + `jev_questions.relation_questions` | 3-4 个 noul（semantic / causes / caused_by / [entity]）→ per-link-type threshold rules |
| `routing.json` | `RetrievalController._query` + `jev_questions.routing_questions` | 6 个 noul → `threshold` 输出 `graph_budgets`（semantic/temporal/causal/entity/multi_hop_need/recency_importance 的 weight） |
| `stopping.json` | `RetrievalController._query` stopping loop + `jev_questions.stopping_questions` | 4 个 noul → `threshold` first-fail-on-CONTINUE 等价编码 AND-of-STOP |
| `retrieve_loop.json` | `RetrievalController` 顶层 | 把 routing → stopping → (continue) → stopping … 串成决策图 |
| `persist_memory.json` | `MemoryBuilder` persistence | `kind: "db"` SQLite 写 `memories` / `relations` 表 |

## DSL ↔ Jev-Mem 决策集映射

| Jev-Mem 函数 | Jev-Mem 决策集 (noul/choice) | DSL 节点 | DSL action |
|---|---|---|---|
| `WritePolicy.memory_type` | 4 noul | single node | `threshold` rules → sorted list |
| `WritePolicy.assess_observation` admission | 5 noul | single node | `gate` 3 段 (STORE / REVIEW / DROP) |
| `WritePolicy.relations` | 3-4 noul × N pair | single node (候选数受 model context 限制 ≤5) | `threshold` per-link-type rules |
| `RetrievalController` routing | 6 noul | single node `routing` | `threshold` rules 按 weight 分配 budget |
| `RetrievalController` stopping | 4 noul | single node `stopping` | `threshold` first-fail-on-CONTINUE |
| `RetrievalController` traversal | 4 noul × N candidate | single node `traversal` | `threshold` rules per-graph |
| `MemoryBuilder` persistence | SQL `exec` | single node `persist` | `kind: "db"` capability |

## 关键 DSL 表达（AND-of-STOP ↔ first-fail-on-CONTINUE）

Jev-Mem stopping 语义：

```python
if (evidence_sufficient >= 0.85
    and missing_evidence < 0.40
    and contradiction < 0.40):
    stop_reason = "evidence_sufficient"
if continue_useful < 0.40:
    stop_reason = "further_retrieval_unhelpful"
```

DSL `threshold` 规则每条只能引用一个 `when.question`（单条件），所以用 first-fail-on-CONTINUE 等价编码：

```jsonc
{
  "kind": "threshold",
  "question": "evidence_sufficient",     // primary q 只是注册项；rules 按 question 字段独立匹配
  "rules": [
    { "when": { "question": "evidence_sufficient", "value_lt": 0.85 },   "label": "CONTINUE_EVIDENCE_INSUFFICIENT" },
    { "when": { "question": "missing_evidence",    "value_gte": 0.40 }, "label": "CONTINUE_MISSING" },
    { "when": { "question": "contradiction",       "value_gte": 0.40 }, "label": "CONTINUE_CONTRADICTION" },
    { "when": { "question": "continue_useful",     "value_lt": 0.40 },  "label": "STOP_FURTHER_USELESS" },
    { "when": { "always": true },                                          "label": "STOP_EVIDENCE_OK" }
  ]
}
```

任一 CONTINUE 条件命中 → 继续；都没有命中 → STOP。这是 Jev-Mem AND 语义在 DSL first-match-wins 下的等价变换（CONTINUE ∨ STOP_EVIDENCE_INSUFFICIENT 等价于 ¬STOP_EVIDENCE_OK）。

## 运行

```bash
# 1) 校验（应零 warning）
laya-workflow validate --spec dsl/ole_eval/jev_mem/memory_type.json
laya-workflow validate --spec dsl/ole_eval/jev_mem/admission.json
laya-workflow validate --spec dsl/ole_eval/jev_mem/relation_pair.json
laya-workflow validate --spec dsl/ole_eval/jev_mem/routing.json
laya-workflow validate --spec dsl/ole_eval/jev_mem/stopping.json
laya-workflow validate --spec dsl/ole_eval/jev_mem/retrieve_loop.json

# 2) 真模型跑（laya-tch 服务须先在 :8400）
laya-workflow --base-url http://127.0.0.1:8400 run \
    --spec dsl/ole_eval/jev_mem/memory_type.json \
    --state '{"observation":"Mira planted basil and wants a weekly reminder."}'

laya-workflow --base-url http://127.0.0.1:8400 run \
    --spec dsl/ole_eval/jev_mem/stopping.json \
    --state '{"query":"What does Alice prefer?", "evidence":["Alice prefers concise explanations.","Alice prefers short answers."]}'
```
