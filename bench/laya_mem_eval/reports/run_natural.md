# laya-mem evaluation report (2026-09-30 16:19)

- binary: `<repo>/target/debug/laya-workflow`
- spec dir: `<repo>/dsl/laya_mem`


## Natural-phrasing diagnostic (heuristic generalization)

- type classification: 3/17 = 17.6%
- admission gate:      5/10 = 50.0%
- rows (want_type/want_adv vs got):

| observation | want_type | got_type | want_adv | got_adv |
|---|---|---|---|---|
| Yesterday Alice got a new puppy | TYPE_EPISODIC | TYPE_OTHER |  | ALLOW |
| Bob went hiking on Sunday | TYPE_EPISODIC | TYPE_OTHER |  | ALLOW |
| Last Tuesday Carol joined the engineerin | TYPE_EPISODIC | TYPE_OTHER |  | ALLOW |
| On Monday morning the deploy went live | TYPE_EPISODIC | TYPE_OTHER |  | ALLOW |
| Alice works at Acme Corp as a senior eng | TYPE_SEMANTIC | TYPE_SEMANTIC |  | ALLOW |
| Bob's hometown is Munich, Germany | TYPE_SEMANTIC | TYPE_OTHER |  | ALLOW |
| Python was first released in 1991 | TYPE_SEMANTIC | TYPE_OTHER |  | ALLOW |
| Tokyo is the capital of Japan | TYPE_SEMANTIC | TYPE_OTHER |  | ALLOW |
| First wash the rice, then soak for 30 mi | TYPE_PROCEDURAL | TYPE_OTHER |  | ALLOW |
| To reset your password click forgot pass | TYPE_PROCEDURAL | TYPE_OTHER |  | ALLOW |
| Deploy by tagging commit and pushing to  | TYPE_PROCEDURAL | TYPE_OTHER |  | ALLOW |
| Alice would rather not be interrupted du | TYPE_PREFERENCE | TYPE_OTHER |  | ALLOW |
| Bob always orders oat milk in his coffee | TYPE_PREFERENCE | TYPE_OTHER |  | ALLOW |
| Carol enjoys writing tests before code | TYPE_PREFERENCE | TYPE_OTHER |  | ALLOW |
| The user dislikes long meetings | TYPE_PREFERENCE | TYPE_OTHER |  | ALLOW |
| Hmm, interesting point | TYPE_OTHER | TYPE_OTHER |  | ALLOW |
| Sounds good | TYPE_OTHER | TYPE_OTHER |  | ALLOW |
| got it, thanks |  | TYPE_OTHER | BLOCK | BLOCK |
| right, acknowledged |  | TYPE_OTHER | BLOCK | ALLOW |
| no need to track that |  | TYPE_OTHER | BLOCK | ALLOW |
| we could go either way on this |  | TYPE_OTHER | CONFIRM | ALLOW |
| not sure if it matters yet |  | TYPE_OTHER | CONFIRM | ALLOW |
| still debating |  | TYPE_OTHER | CONFIRM | ALLOW |
| Alice moved to Berlin in 2018 |  | TYPE_OTHER | ALLOW | ALLOW |
| Bob's role is now tech lead |  | TYPE_OTHER | ALLOW | ALLOW |
| The launch is scheduled for next quarter |  | TYPE_OTHER | ALLOW | ALLOW |
| Mira keeps basil on her kitchen windowsi |  | TYPE_OTHER | ALLOW | ALLOW |


**Overall: PASS**