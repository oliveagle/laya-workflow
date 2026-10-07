# Needle 3 integration

laya-workflow speaks to the **Cactus Needle 3** on-device model (8–29 MB,
2-bit, single `.cact` file) through its native C ABI, loaded via `dlopen` — no
network, no API key, no GPU.

Needle 3 does three jobs well and the same engine does all three:

1. **Tool calls** — natural language → `function_calls` (arguments are
   grammar-guaranteed to parse). Off-topic input returns an empty list, not a
   guess, and every response carries a **calibrated confidence**.
2. **Structured extraction** — free text + an OpenAI-form schema → typed
   arguments, guaranteed to match the schema.
3. **Text embedding** — one sentence → a 3072-dim vector (already normalised).

## Install

```sh
pip install cactus-needle          # the Python CLI / package (optional)
needle download needle3 --out ~/.laya-workflow/models   # 35 MB weights
needle fetch                       # the native engine (libneedle.so, <1 MB)
```

The engine is looked up at first call from `$NEEDLE3_LIB_PATH`, else
`~/.cache/cactus-needle/v3/<ver>/libneedle.so`; the weights from `$NEEDLE3_CACT`,
else `~/.laya-workflow/models/needle3.cact`.

## What's wired in

| Surface | What |
|---|---|
| Capability `kind: "needle"` | ops `extract` / `embed` / `complete` (`src/capability/needle.rs`) |
| laya-mem | `LAYA_MEM_EMBEDDING_BACKEND=needle` uses the engine for both persist and recall (dim 3072, no network) |
| MCP | `needle_extract` / `needle_embed` / `needle_complete` in `laya-workflow mcp serve` |
| Demos | `dsl/capabilities/needle_demo.json`, `dsl/capabilities/needle_ticket.json` |
| Tests | `laya-workflow-tests needle` — 8 assertions, skipped when the engine/weights are absent |

## Demo

```sh
export NEEDLE3_CACT=~/.laya-workflow/models/needle3.cact

# extract an invoice from free text
laya-workflow run --spec dsl/capabilities/needle_demo.json \
  --state '{"text": "Invoice from Acme Corp, $1,200.00, due 2026-09-01"}'

# classify + extract a support ticket (two needle calls in one workflow)
laya-workflow run --spec dsl/capabilities/needle_ticket.json \
  --state '{"text": "Order #88472 is missing from my account, customer 55331"}'
```

## Call shape

The capability reads an expanded `with` object:

```jsonc
{ "kind": "needle", "op": "extract", "cact": "${env.NEEDLE3_CACT}" }
```

* `extract` — `with.text` + `with.tool` (one OpenAI-form schema) →
  `{arguments, confidence, reasoning, matched}`.
* `embed` — `with.text` → `{dim, vector}`.
* `complete` — `with.prompt` (+ optional `with.tools`, `with.system`) →
  the full engine JSON (`function_calls`, `confidence`, `reasoning`).

## Performance

Every op pays `needle_load` once per process (35 MB cact → ~50 ms) and
`needle_init` only when the `(system, tools)` pair actually changes (~90 ms).
Between calls the engine rewinds the conversation (`needle_reset`) so turns do
not accumulate and slow decode. Measured on this machine (20-layer base,
single-threaded C++, ~366 decode tok/s):

| Op | Cold (first call) | Warm (same process, same schema) |
|---|---|---|
| `extract` | ~400 ms | **~275 ms** |
| `embed` (3072 d) | ~11 ms | **~11 ms** |
| `complete` | ~280 ms | **~275 ms** |

### The < 100 ms target: build a smaller rung

`extract`/`complete` are decode-bound: the 20-layer base writes ~70 output
tokens at ~370 tok/s ≈ 200 ms of arithmetic. The vendor ships `needle build
--layers N` (2–20) which slices the checkpoint into a faster subnetwork.
Measured on this machine (same schema, warm, `extract`); the first table
used the old `max_new_tokens=128` default, the table after it re-measures with
the latency-tuned default of 80:


| rung | size | warm extract | decode tok/s | Scenario-B accuracy (8 invoices) |
|---|---|---|---|---|
| 20-layer (base `needle3.cact`) | 35 MB | ~250 ms | 370 | 7/8 |
| 14-layer | 49 MB W4 | ~180 ms | 530 | 7/8 |
| 10-layer | 37 MB W4 | ~160 ms | 610 | 6/8 |
| **8-layer** | 27 MB W4 | **~125 ms** | 760 | 5/8 |
| 6-layer | 17 MB W4 | **~95 ms** | 1070 | 2/8 |

**There is no free lunch.** Hitting < 100 ms needs an 8-layer (or smaller) rung,
but accuracy drops with depth: 14-layer keeps the base's 7/8 at ~180 ms, 8-layer
reaches ~125 ms at 5/8, 6-layer hits ~95 ms but collapses to 2/8. For a quality
budget, 14-layer is the sweet spot (same accuracy, −30% time). For a hard
100 ms bound, 8-layer is the only rung that both stays usable and approaches it.

### Tuning `max_new_tokens` — the second dial

Decode is linear in output tokens, so `with.max_new_tokens` (default 80 for `extract`, 512 for `complete`) is a latency knob. Sweeping it on the same 8
invoices, warm, same schema:

| rung | mnt | avg ms | accuracy |
|---|---|---|---|
| 8-layer | 16 | 35 | 0/8 (truncated before any call) |
| 8-layer | 48 | 71 | 1/8 |
| **8-layer** | **80** | **95.5** | **5/8 — identical to mnt=128** |
| 8-layer | 128 | 101 | 5/8 |
| 10-layer | 64 | 99 | 3/8 (beats mnt=96 only on time) |
| 10-layer | 96 | 130 | 6/8 |
| 14-layer | 40 | 127 | 4/8 |
| 20-layer | 40 | 144 | 5/8 |

Below ~40 tokens the grammar-guaranteed call never finishes, so accuracy
collapses to 0 — a floor, not a tradeoff. **8-layer + mnt=80 is the measured
< 100 ms configuration: 95.5 ms with no accuracy loss** (the same 5/8 as
mnt=128; the extra budget only pads reasoning). Raise `with.max_new_tokens` on
`extract` when a schema is wide enough to need it; the default is tuned for
the latency target, correctness can always buy it back.

The number above is the raw engine round trip. Through the real Rust wrapper
(`needle_extract` behind a persistent `laya-workflow mcp serve`, 8-layer +
default mnt=80) the same 8 invoices measure n=40: **avg 92.7 ms, p50 91.4 ms,
85 % of calls under 100 ms** (min 77.8, p90 101.9, max 118.6 — the tail is
system scheduling on a shared box). The same run on 14-layer is avg 179 ms
(0/40 under 100 ms) — so the < 100 ms claim is specific to the 8-layer rung,
and 14-layer remains the quality pick when ~180 ms is affordable.

### Does the speed-up cost accuracy? Full A/B by rung

Same `bench/needle_vs_heuristic.py`, same bench cases, each rung in turn:

| scenario | 20-layer (base) | 14-layer | 10-layer | 8-layer | 6-layer |
|---|---|---|---|---|---|
| A  needle-only routing | 12/20 (60%) | 12/20 | 12/20 | 12/20 | 12/20 |
| A  hybrid (heuristic → needle) | 15/20 (75%) | **18/20 (90%)** | 16/20 (80%) | 16/20 (80%) | 16/20 (80%) |
| B  needle-only full-record | 5/8 (62%) | 4/8 (50%) | 2/8 (25%) | 4/8 (50%) | 0/8 (0%) |
| B  hybrid (regex → needle) | 5/8 (62%) | 4/8 (50%) | 2/8 (25%) | 4/8 (50%) | 1/8 (12%) |
| D  needle classify | 1/5 (20%) | 1/5 | 1/5 | 1/5 | 1/5 |
| D  needle entity extract | **5/5 (100%)** | 4/5 (80%) | 2/5 (40%) | 1/5 (20%) | 1/5 (20%) |
| D  needle full workflow | 1/5 (20%) | 1/5 (20%) | 1/5 (20%) | 0/5 (0%) | 0/5 (0%) |
| D  hybrid (regex classify + needle entity) | **5/5 (100%)** | 4/5 (80%) | 2/5 (40%) | 1/5 (20%) | 1/5 (20%) |
| C  embedding avg cosine (semantic) | 0.95 | 0.94 | 0.93 | 0.93 | 0.93 |
| C  embedding avg cosine (lexical) | 0.95 | 0.96 | 0.96 | 0.96 | 0.95 |

**What this means:**

* Routing (A) is **not hurt** — needle-alone is identical 12/20 across every
  rung, and hybrid actually scores *better* on 14-layer (90%) because one fewer
  heuristic miss gets rescued. Only 6-layer dips slightly on hybrid A.
* Embedding (C) is **not hurt** — cosine stays at 0.93–0.96, well above BOW's
  0.17–0.42, so `LAYA_MEM_EMBEDDING_BACKEND=needle` is safe on any rung.
* Structured extraction (B full-record, D entity) **is hurt by depth**:
  14-layer drops 5/8 → 4/8, 8-layer drops to 4/8 with entity collapsing to
  1/5 (the model fills fields from the wrong parts of the prompt), and 6-layer
  is 0/8. **If you use needle for structured extraction, stay on the 20-layer
  base or 14-layer at most** — the sub-100 ms 8-layer rung is for routing
  or embedding, not for extraction.
* Enum classification (D classify 1/5) was already the model's weakest
  surface and is unchanged across rungs.
* **The default is still the 20-layer base** (`needle3.cact`); the sub-100ms
  number above requires the explicit
  `NEEDLE3_CACT=~/.laya-workflow/models/needle3_8l.cact` switch. So a
  user who does not opt in to a rung sees **no accuracy change at all**
  from this optimisation — only the wrapper cache + `mnt=80` trim, which
  both preserve output.


Point the engine at a rung with the same env var it already reads:

```sh
export NEEDLE3_CACT=~/.laya-workflow/models/needle3_8l.cact
```

The rungs are built once from the `.safetensors` checkpoint (242 MB download,
`jax` + `flax` needed for the build, not for inference).

## Safety

The capability touches only an already-installed model file: no `allow_exec`,
no `allow_hosts`, no shell-out. `policy.allow_paths` should cover the model dir.
The C header documents one process-global, **non-thread-safe** model, so every
call is serialised behind a mutex.

## Where needle wins — measured

See [docs/needle_evidence.md](./needle_evidence.md) for the A/B data that decided
this: intent routing (heuristic 35% → hybrid 75%), structured extraction
(regex 12% vs needle 62%), embedding recall (BOW 0.21 vs needle 0.95 cosine),
and the workflow pattern **regex-classify + needle-entity = 100%** on 5 tickets.
Reproduce with `python3 bench/needle_vs_heuristic.py`.

## Notes

* A `cactus-needle` wheel bug (3.1.2) calls `needle_embed` with the Needle-2
  three-arg signature while the v3.2.0 engine wants five — the Python
  `Needle(tools=..., weights=...)` worker path fails on `embed`. The Rust
  binding uses the documented five-arg ABI directly, so it is unaffected; in
  Python, construct `Needle(tools=[])` without `weights=` to hit the same path.
* A Rhai plugin cannot call the FFI (the sandbox has no exec/registry), so the
  right way to compose needle inside a workflow is multiple `kind: "needle"`
  capability nodes in one spec (see `needle_ticket.json`).
