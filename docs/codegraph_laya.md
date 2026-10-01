# `codegraph + laya` — graph traversal driven by Laya controllers

**Research**: how to use Laya's System-One controllers (`routing`, `stopping`,
`traversal`) to drive *codegraph* graph queries, so that "find code that
implements business function X" becomes a typed decision-driven loop instead
of an LLM-driven trial-and-error exploration.

> Status: **design**. No code in this repo depends on any of this yet. The
> intent is to describe the seam before building it, so the seam matches
> what the engine already exposes (MCP tool sets + `dsl/laya_mem` spec
> shapes) rather than inventing a parallel one.

---

## TL;DR

codegraph stores a **SQLite code graph** (`nodes` / `edges` / `nodes_fts` /
`unresolved_refs`) with 18.5 k nodes and 44.7 k edges on a 1.9 k-file repo.
Laya already has the *decision* half of graph retrieval as reusable DSL specs
under `dsl/laya_mem/` — `routing.json` allocates per-view budget from 6
binary judgments; `stopping.json` is an AND-of-STOP gate over 4; `traversal.json`
scores candidate neighbors on 4 axes. All three run **offline** on the
heuristic backend, so the whole loop is reproducible in CI.

The missing half is a **view adapter**: the controllers name *graph views*
(`semantic` / `temporal` / `causal` / `entity` / `multi_hop_need` /
`recency`); someone has to translate "spend 3 on causal" into concrete
codegraph queries (`cg_callers`, `WHERE edges.kind='calls'`, …). That
translation is a small, well-bounded piece of Rust — naturally a new
`laya-codegraph` MCP tool set registered alongside `laya-mem` in
`workflow_cli.rs::run_mcp`.

---

## 1. The pain

Today "find code for business function X" is an LLM-driven loop:

```text
user question
  → LLM guesses a query string
  → cg_query / grep / rg
  → LLM reads 3 files, still unsure
  → LLM guesses a second query string
  → cg_callers → cg_node → cg_callees
  → …
```

Two problems, both structural:

1. **No explicit view budget.** The LLM decides "should I look at callers?"
   by vibes, not by a typed judgment, so it either over-fetches (reads 20
   files when 2 would do) or under-fetches (never follows the call chain).
2. **No stopping gate.** "Have I got enough evidence?" is answered by the
   LLM every turn, which drifts, which is why the same question asked twice
   can return different files.

Laya's controllers exist to fix exactly this shape — typed decisions,
reproducible thresholds, one place to audit.

---

## 2. codegraph's data model (evidence)

Observed on `~/.agents/.codegraph/codegraph.db` (SQLite, 59 MB, index
version 1.0.1, extraction version 24, project = the local `ole-agents`
tree).

| table | rows | shape |
|---|---:|---|
| `nodes` | **18,541** | `id`, `kind`, `name`, `qualified_name`, `file_path`, `language`, `start_line`, `end_line`, `docstring`, `signature`, `visibility`, `is_exported`, `is_async`, `is_static`, `is_abstract`, `return_type`, … |
| `edges` | **44,717** | `source`, `target`, `kind`, `metadata`, `line`, `col`, `provenance` |
| `files` | 1,902 | `path`, `content_hash`, `language`, `size`, `modified_at`, `node_count`, `errors` |
| `nodes_fts` | 18,541 | FTS5 over `name`, `qualified_name`, `docstring`, `signature` |
| `unresolved_refs` | 17,688 | `from_node_id`, `reference_name`, `reference_kind`, `candidates` (JSON) |

**Node kinds** (top): `import` 5,066 · `function` 4,412 · `method` 4,171 ·
`file` 1,222 · `variable` 1,174 · `constant` 970 · `class` 854 ·
`struct` 268 · `interface` 218 · `route` 94 · `type_alias` 53 · …

**Edge kinds** (top): `calls` 18,466 · `contains` 17,490 · `imports` 6,131 ·
`instantiates` 1,604 · `references` 973 · `extends` 44 · `implements` 6 ·
`decorates` 3.

What that buys us, concretely:

* `calls` + `extends` + `implements` + `instantiates` give a real **causal /
  dependency view** — the same shape Jev-Mem's `causal` graph view is about.
* `nodes_fts` (BM25 over `name` / `qualified_name` / `docstring` /
  `signature`) is a ready-made **semantic view** — no embedding model
  required for a first cut.
* `files.modified_at` + `files.indexed_at` give a **temporal view**.
* `qualified_name` is an **entity key** — same-name symbols across files
  (`WorkflowValidator::__init__` vs a Python `__init__`) can be resolved
  exactly.
* `unresolved_refs.candidates` is a pre-computed **multi-hop** bridge —
  where the indexer failed to bind, the candidates list is a small, ranked
  set worth one more hop before giving up.

---

## 3. The controllers Laya already ships

All three live under `dsl/laya_mem/` and are reusable **as-is** (they read a
plain JSON `state`, so any adapter can feed them):

| spec | question set | shape |
|---|---|---|
| `routing.json` | 6 × `noul` (`semantic` / `temporal` / `causal` / `entity` / `multi_hop_need` / `recency_importance`) | single node, `action: copy_keys` — returns the 6 raw 0..1 scores. Caller applies largest-remainder to turn them into per-view budget. |
| `stopping.json` | 4 × `noul` (`evidence_sufficient`, `continue_useful`, `missing_evidence`, `contradiction`) | single node, `action: threshold` — first-fail-on-CONTINUE is the DSL idiom for AND-of-STOP; falls through to `STOP_EVIDENCE_OK`. |
| `traversal.json` | 4 × `noul` **per candidate** (`relevance`, `relation_usefulness`, `new_information`, `supports_current_evidence`) | single node, `action: copy_keys` — returns 4 scores per candidate; caller applies `transition_weights` for the final rank. |

`retrieve_loop.json` is the composed pattern: `route → stop_check → …` with
`max_iterations: 6`. It is deliberately generic — nothing in it knows about
memories.

**That is the reuse opportunity.** The whole decision side of a codegraph
retrieval loop can be lifted verbatim; only the *fetch* side (what "causal
view" means in codegraph terms) is new code.

### ⚠️ Heuristic-backend calling contract (verified)

The offline heuristic backend resolves `heuristic.field` by **substring match
on the targeted state value** — but **if the targeted key is missing it falls
back to substring-matching the whole serialised state** (`src/backend.rs`
~L691: `unwrap_or_else(|| text.clone())`). For the routing spec this means
the fetch adapter **must always populate every one of the six `route_*`
hint fields explicitly** (`high` / `low` / `yes` / `no`). If even one is
missing, the substring needles (`high`, `yes`) can leak across questions
and every noul will hit.

Verified end-to-end with the live CLI:

```text
state = {"query":"auth","route_semantic":"high","route_temporal":"low",
         "route_causal":"high","route_entity":"low",
         "route_multi_hop_need":"yes","route_recency":"low"}
routing.json → semantic 0.85 · temporal 0.1 · causal 0.85 · entity 0.1
              · multi_hop_need 0.85 · recency_importance 0.1
```

(`stopping.json` shows the same shape: `evidence_status:"sufficient"`
→ `STOP_EVIDENCE_OK`; `"contradiction"` → `CONTINUE_CONTRADICTION`.) So
the adapter contract is concrete and provable, not a worry to ignore.

---

## 4. Mapping controllers → codegraph views

| Jev-Mem view | codegraph query | why it is the same view |
|---|---|---|
| `semantic` | `nodes_fts MATCH '<query>'` (BM25) over `name` / `qualified_name` / `docstring` / `signature` | "topically related records" — the FTS index *is* the semantic view. |
| `temporal` | `files ORDER BY modified_at DESC LIMIT N` → intersect with node `file_path` | "what changed recently" — codegraph already tracks it per file. |
| `causal` | `edges WHERE kind IN ('calls','extends','implements','instantiates')` (from/to a seed node) | "what causes / enables / depends on what" — call and inheritance edges are the causal graph. |
| `entity` | exact `qualified_name` match, or `unresolved_refs` candidate expansion | "connect mentions of the same thing" — qualified name is the entity key. |
| `multi_hop_need` | 2-3 hop BFS over `calls` / `extends` / `imports`, depth-bounded by the budget | "requires combining distinct pieces" — a chain of hops, not one hop. |
| `recency_importance` | same `temporal` view but re-weighted toward `recency_importance` | keeps "latest fact wins" from the controller. |

The mapping is 1:1 — no new controller questions needed.

---

## 5. Design: a `laya-codegraph` MCP tool set

Register a new `McpToolSet` next to `laya-mem` in
`src/workflow_cli.rs::run_mcp` (a 2-line edit once the trait is implemented).
Proposed tools, mirroring `laya-mem`'s four:

| tool | inputs | outputs | controller used |
|---|---|---|---|
| `codegraph_route` | `query`, optional `evidence` | 6 raw view scores + per-view budget | `routing.json` |
| `codegraph_fetch` | `view`, `query`, `seed` (optional `qualified_name`), `budget` | ranked candidates (top-k nodes with `kind`, `qualified_name`, `file_path`, `line`, snippet) | — (view adapter) |
| `codegraph_traverse` | `seed`, `candidates[]` (from a prior fetch), optional `transition_weights` | per-candidate 4-axis scores, ranked | `traversal.json` |
| `codegraph_stop` | `query`, `evidence[]`, optional flags | `STOP_EVIDENCE_OK` or `CONTINUE_*` + reason | `stopping.json` |
| `codegraph_answer` | `query`, `evidence[]` | final answer: symbols + call chain + file refs, ready for the caller to read | — (composition) |

`codegraph_fetch` is the only piece with real logic — it turns
`(view, query, seed, budget)` into a SQL query against the codegraph DB. The
four controllers are loaded from the **same** `dsl/laya_mem` spec directory
(already compiled into the binary), so the decision layer stays
byte-identical between the memory and the code-graph domain.

### Where the codegraph DB comes from

`codegraph.db` is a plain SQLite file at `~/.agents/.codegraph/codegraph.db`
today. The tool needs only a read handle, so the natural config surface is:

```
LAYA_CODEGRAPH_DB=~/.agents/.codegraph/codegraph.db   # default
LAYA_CODEGRAPH_SPECS=dsl/laya_mem                      # reuse the memory specs
```

No new SQLite writer, no index format, no daemon — codegraph itself is the
system of record; the MCP tool is a read-only decision-driven adapter.

---

## 6. Worked example

> *"Find the code that handles authentication for the scheduler API."*

Today, an agent would call `cg_query` on a guessed string like
`"auth scheduler"`, get a few hits, read two files, and repeat. With the
proposed loop:

```text
1. codegraph_route(query="auth scheduler API")
   → semantic 0.9, causal 0.7, entity 0.3, temporal 0.1, multi_hop 0.6, recency 0.1
   → budgets: semantic 3, causal 2, multi_hop 1, others 0

2. codegraph_fetch(view=semantic, query="auth scheduler", budget=3)
   → [authenticate(), AuthMiddleware, token_refresh()]       (top-3 by BM25)

3. codegraph_fetch(view=causal, query=…, seed=authenticate(), budget=2)
   → [SessionStore, OAuthProvider]                           (2-hop callers)

4. codegraph_traverse(seed=authenticate(), candidates=[…6 hits…])
   → ranked: [authenticate, AuthMiddleware, SessionStore, …] (4-axis scores)

5. codegraph_stop(query=…, evidence=[…6 facts…])
   → STOP_EVIDENCE_OK (evidence_sufficient 0.92, contradiction 0.05, missing 0.1)

6. codegraph_answer(query=…, evidence=[…])
   → "auth flows through authenticate() → AuthMiddleware → SessionStore;
      entry point: src/auth/middleware.rs:42"
```

Six tool calls, every one of them a typed decision, every one auditable in
`laya-workflow state`. If the caller disagrees with a decision, the same
DSL spec can be re-run on the live `laya-tch` server instead of the
heuristic backend — same questions, real model answers — with zero code
change.

---

## 7. What this buys

| axis | today | with the loop |
|---|---|---|
| **tool calls** for "find X" | 5-15 (LLM guess each turn) | 3-6 (budget-driven) |
| **accuracy** | varies with the LLM's mood; no audit trail | typed scores + threshold; reproducible |
| **stop condition** | "I think I have enough" | `STOP_EVIDENCE_OK` / `CONTINUE_*` with a reason |
| **cost** | every hop is a full LLM turn | controllers run offline; only the final `answer` turn is LLM |
| **CI** | no | the whole loop runs on the heuristic backend in <1 s, regression-checkable |

---

## 8. Open questions / next steps

1. **Should `codegraph_fetch` live in the MCP tool set or as a `kind: "db"`
   workflow?** The `db` capability already speaks SQLite and would let a DSL
   spec drive the same loop without any Rust. Worth a spike: write the
   worked example above as a `dsl/codegraph/auth_flow.json` using only
   `kind: "db"` + the existing `routing/stopping/traversal` specs, and see
   how far it gets. If it reads cleanly, the MCP tool set may be a thin
   wrapper rather than a new surface.
2. **`unresolved_refs.candidates`** — 17.6 k rows of pre-ranked candidates
   the indexer could not bind. Worth a dedicated multi-hop view: it is the
   cheapest source of "one more hop" evidence.
3. **Spec reuse vs copy.** `dsl/laya_mem/{routing,stopping,traversal}.json`
   work as-is, but the *questions* are worded for memories. A second pass
   should probably fork them into `dsl/codegraph/*.json` with code-specific
   wording ("does this query need call-chain evidence?") so the heuristic
   backend gets better `p_hit`/`p_miss` defaults on code data.
4. **A `cg_*` skill bridge.** `~/.agents/skills/codegraph-pi` already
   documents a `cg_query` / `cg_callers` / `cg_impact` surface exposed by
   the pi runtime, but the CLI itself is not on `$PATH` here. The MCP tool
   reads the DB directly, which sidesteps the CLI entirely — but a CLI
   bridge would give the same loop to repos where the DB is not accessible.

---

*Research note written 2026-10-01. Evidence source:
`~/.agents/.codegraph/codegraph.db` (schema + counts), `dsl/laya_mem/*.json`
(spec shapes), `src/laya_mem.rs` + `src/mcp.rs` + `src/workflow_cli.rs`
(MCP registration pattern).*
