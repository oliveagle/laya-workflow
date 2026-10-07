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

`extract` and `complete` are decode-bound — the model writes ~75 output tokens
at 366 tok/s = ~210 ms of arithmetic. Cutting that below ~100 ms needs a
smaller subnetwork (`needle build --layers N`, 2–20 layers), which needs JAX
and a checkpoint; the base 20-layer `.cact` is the engine's shipped default.

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
