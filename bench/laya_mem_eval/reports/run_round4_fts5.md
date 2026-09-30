# laya-mem evaluation report (2026-09-30 17:52)

- binary: `/home/oliveagle/repos/github.com/oliveagle/laya-workflow/target/debug/laya-workflow`
- spec dir: `/home/oliveagle/repos/github.com/oliveagle/laya-workflow/dsl/laya_mem`

## Gate metrics

| task | n | want_acc | oracle_agree | avg_ms | p50_ms | p95_ms |
|------|---|----------|--------------|--------|--------|--------|
| admission | 30 | 100.0% | 100.0% | 1.9 | 1.8 | 2.0 |
| memory_type | 30 | 100.0% | 100.0% | 1.8 | 1.8 | 1.9 |
| stopping | 29 | 100.0% | 100.0% | 1.3 | 1.3 | 1.3 |

## Recall metrics

- n = 19, recall@5 = 100.0%

## Natural-phrasing diagnostic (heuristic generalization)

- type classification: 15/17 = 88.2%
- admission gate:      10/10 = 100.0%
- rows (want_type/want_adv vs got):

| observation | want_type | got_type | want_adv | got_adv |
|---|---|---|---|---|
| Yesterday Alice got a new puppy | TYPE_EPISODIC | TYPE_EPISODIC |  | ALLOW |
| Bob went hiking on Sunday | TYPE_EPISODIC | TYPE_EPISODIC |  | ALLOW |
| Last Tuesday Carol joined the engineerin | TYPE_EPISODIC | TYPE_EPISODIC |  | ALLOW |
| On Monday morning the deploy went live | TYPE_EPISODIC | TYPE_EPISODIC |  | ALLOW |
| Alice works at Acme Corp as a senior eng | TYPE_SEMANTIC | TYPE_SEMANTIC |  | ALLOW |
| Bob's hometown is Munich, Germany | TYPE_SEMANTIC | TYPE_SEMANTIC |  | ALLOW |
| Python was first released in 1991 | TYPE_SEMANTIC | TYPE_PROCEDURAL |  | ALLOW |
| Tokyo is the capital of Japan | TYPE_SEMANTIC | TYPE_SEMANTIC |  | ALLOW |
| First wash the rice, then soak for 30 mi | TYPE_PROCEDURAL | TYPE_PROCEDURAL |  | ALLOW |
| To reset your password click forgot pass | TYPE_PROCEDURAL | TYPE_PROCEDURAL |  | ALLOW |
| Deploy by tagging commit and pushing to  | TYPE_PROCEDURAL | TYPE_EPISODIC |  | ALLOW |
| Alice would rather not be interrupted du | TYPE_PREFERENCE | TYPE_PREFERENCE |  | ALLOW |
| Bob always orders oat milk in his coffee | TYPE_PREFERENCE | TYPE_PREFERENCE |  | ALLOW |
| Carol enjoys writing tests before code | TYPE_PREFERENCE | TYPE_PREFERENCE |  | ALLOW |
| The user dislikes long meetings | TYPE_PREFERENCE | TYPE_PREFERENCE |  | ALLOW |
| Hmm, interesting point | TYPE_OTHER | TYPE_OTHER |  | ALLOW |
| Sounds good | TYPE_OTHER | TYPE_OTHER |  | ALLOW |
| got it, thanks |  | TYPE_OTHER | BLOCK | BLOCK |
| right, acknowledged |  | TYPE_OTHER | BLOCK | BLOCK |
| no need to track that |  | TYPE_OTHER | BLOCK | BLOCK |
| we could go either way on this |  | TYPE_OTHER | CONFIRM | CONFIRM |
| not sure if it matters yet |  | TYPE_OTHER | CONFIRM | CONFIRM |
| still debating |  | TYPE_OTHER | CONFIRM | CONFIRM |
| Alice moved to Berlin in 2018 |  | TYPE_OTHER | ALLOW | ALLOW |
| Bob's role is now tech lead |  | TYPE_OTHER | ALLOW | ALLOW |
| The launch is scheduled for next quarter |  | TYPE_OTHER | ALLOW | ALLOW |
| Mira keeps basil on her kitchen windowsi |  | TYPE_OTHER | ALLOW | ALLOW |


**Overall: PASS**