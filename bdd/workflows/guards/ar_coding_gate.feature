@decision
Feature: ar_coding_gate
  Port of the ole-eval AR-coding sanity check's front-door gate
  (code/dllm/eval/eval_ar_coding.py): the diagnostic that loads base + LoRA, generates
  with HF generate() (causal, low temperature) instead of the diffusion sampler,
  executes each candidate against the benchmark's unit tests and reports pass@1. It
  decides whether the requested run is well-formed enough to start -- it exits non-zero
  when a `--sources` name is not a known benchmark, or when no problems loaded -- so it
  names its verdict rather than reporting a number; whether the candidates passed is
  drawn by hand from the jsonl it writes.

  Two questions are asked, in the source's own order:

    - sources: is every `--sources` entry a known benchmark? `load_all` walks the
      comma-separated names in order and raises SystemExit on the first one that is not
      `humaneval` / `mbpp` / `bigcodebench`. An unknown source name (a typo, a benchmark
      that does not exist yet) is refused before anything runs.
    - problems: did any problem load? After the load, `if not recs: raise SystemExit` with
      a no-problems-loaded message. An empty corpus, or a `--limit` that sliced to
      nothing, ends the run here.

  The walk encodes exactly that: sources -> problems. A request that clears both questions
  reaches the generation loop; the first question that fails names itself and ends the walk.

  Folds are documented:

    - the two SystemExits are the *only* exits before the loop, so the graph stops there;
      the rest is the run. `build_test_file` raises a plain ValueError for a fourth source,
      but `load_all` only ever produces the three known ones, so that branch is unreachable
      from the front door and is not modeled.
    - the generation and execution loop is a body, not a gate: a candidate that fails its
      unit tests is written to the jsonl as passed=false and the loop carries on. `run_test`
      never raises for a failing candidate -- a subprocess that times out returns
      (False, TIMEOUT) -- so once the run has started it always exits 0, printing an
      `=== AR pass@1 ===` summary whose promote/reject is drawn by hand. The last label is
      therefore AR_PASS_AT_1_REPORTED, not a pass/fail verdict.
    - the AR-specific mode switches (`--no-lora`, `--causal`, the default `--temperature`
      0.2, the Qwen3_5 hybrid-cache patch) only change what the model produces; none of
      them can exit non-zero, so none of them becomes a node.

  Everything the run produced stays in the harness -- the generated completions, the
  subprocess tails, the per-source pass@1 rates, the jsonl rows. The graph reads none of it,
  only whether the request cleared the two front-door checks.

  Reference walk (eval_ar_coding.py, module level): `load_all(args.sources, limit)` -> an
  unknown source raises SystemExit -> exit 1; `if not recs: raise SystemExit` -> exit 1;
  else loop, write the jsonl, print the summary -> exit 0.

  Background:
    Given the workflow runs at most 4 iterations

  Scenario: sources
    Given a choice question "request" asks "Is every `--sources` entry a known benchmark? `load_all` walks the comma-separated names in order and raises SystemExit on the first one that is not `humaneval` / `mbpp` / `bigcodebench`, so an unknown source is refused before anything runs. An unknown source ends the walk."
      | answer | meaning |
      | ok | every `--sources` entry is one of the three known benchmarks |
      | unknown | at least one `--sources` entry is not a known benchmark: the run exits 1 |
    And the answer comes from the field "ev_sources"
    When the answer "unknown" goes to "STOP"
    And any other answer goes to "problems"
    Then the primary question is "request"
    Then label "UNKNOWN_SOURCE" when the answer to "request" is "unknown"
    And label "KNOWN_SOURCES" otherwise

  Scenario: problems
    Given a choice question "request" asks "Did any problem load? After the load, `if not recs: raise SystemExit` ends the run: an empty corpus, or a `--limit` that sliced to nothing. This is the last question, so clearing it means the run starts and then always exits 0, writing a jsonl and printing an `=== AR pass@1 ===` summary."
      | answer | meaning |
      | none | no problem loaded: the run exits 1 |
      | some | at least one problem loaded: the run starts |
    And the answer comes from the field "ev_problems"
    When any other answer goes to "STOP"
    Then the primary question is "request"
    Then label "NO_PROBLEMS_LOADED" when the answer to "request" is "none"
    And label "AR_PASS_AT_1_REPORTED" otherwise
