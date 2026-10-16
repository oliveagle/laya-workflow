@decision
Feature: parity_gate
  Port of the ole-eval LayaModel Rust-vs-PyTorch numeric parity gate
  (code/laya-tch/bench/parity_check.py): the `bench_parity` harness compares the Rust
  forward pass against a captured PyTorch reference and gates the *decision output* --
  the marker logits must agree to `max|delta| < 1e-3`. Before it can measure at all,
  the wrapper refuses with a named reason when a precondition is missing rather than
  silently succeeding: the binary is not built, the reference trace is absent, no
  model directory is named, or the weights file is missing.

  The walk encodes exactly that ladder: four named refusals in the wrapper's own order
  (bin, then trace, then model dir, then weights), then the one run whose verdict is
  PASS only when the binary exits 0 *and* prints PASS; anything else is FAIL.

  One fold is documented: the tolerance (`max|delta| < 1e-3`) and the marker-logit
  choice are applied inside the `bench_parity` binary, so the run's outcome -- "the
  binary returned PASS" -- is the graph's observable input, not the raw delta.

  Reference walk (code/laya-tch/bench/parity_check.py, main()): if the --bin path does
  not exist -> return 2 ("is not built"); if the trace file is missing -> return 2;
  model_dir = --model-dir, or the trace's own `model_dir`, and if neither -> return 2;
  if <model_dir>/model.safetensors is missing -> return 2; otherwise passed =
  (returncode == 0 and "PASS" in output), and the exit is 0 when passed else 1.

  Background:
    Given the workflow runs at most 6 iterations

  Scenario: bin
    Given a choice question "check" asks "Is the bench_parity binary built at the given --bin path?"
      | answer | meaning |
      | ok | the binary path exists |
      | missing | the --bin path does not exist on disk |
    And the answer comes from the field "ev_bin"
    When the answer "missing" goes to "STOP"
    And any other answer goes to "trace"
    Then the primary question is "check"
    Then label "CANNOT_RUN_NO_BIN" when the answer to "check" is "missing"
    And label "BIN_OK" otherwise

  Scenario: trace
    Given a choice question "check" asks "Is the captured PyTorch reference trace present at the given --trace path?"
      | answer | meaning |
      | ok | the trace file exists |
      | missing | the trace file does not exist |
    And the answer comes from the field "ev_trace"
    When the answer "missing" goes to "STOP"
    And any other answer goes to "model_dir"
    Then the primary question is "check"
    Then label "CANNOT_RUN_NO_TRACE" when the answer to "check" is "missing"
    And label "TRACE_OK" otherwise

  Scenario: model_dir
    Given a choice question "check" asks "Is a model directory named, either by --model-dir or by the trace's own model_dir field?"
      | answer | meaning |
      | ok | a model directory is known |
      | missing | neither --model-dir nor the trace names one |
    And the answer comes from the field "ev_model"
    When the answer "missing" goes to "STOP"
    And any other answer goes to "weights"
    Then the primary question is "check"
    Then label "CANNOT_RUN_NO_MODEL_DIR" when the answer to "check" is "missing"
    And label "MODEL_OK" otherwise

  Scenario: weights
    Given a choice question "check" asks "Does the named model directory hold model.safetensors?"
      | answer | meaning |
      | ok | model.safetensors exists under the model directory |
      | missing | model.safetensors is absent |
    And the answer comes from the field "ev_weights"
    When the answer "missing" goes to "STOP"
    And any other answer goes to "measure"
    Then the primary question is "check"
    Then label "CANNOT_RUN_NO_WEIGHTS" when the answer to "check" is "missing"
    And label "WEIGHTS_OK" otherwise

  Scenario: measure
    Given a choice question "check" asks "Did bench_parity pass? It must exit 0 and print PASS. This is the last gate: a pass here clears the parity check."
      | answer | meaning |
      | pass | the binary exited 0 and printed PASS |
      | fail | the binary exited non-zero, or did not print PASS |
    And the answer comes from the field "ev_run"
    When any other answer goes to "STOP"
    Then the primary question is "check"
    Then label "PARITY_PASS" when the answer to "check" is "pass"
    And label "PARITY_FAIL" otherwise
