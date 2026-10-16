@decision
Feature: deploy_gate
  Port of the ole-eval deployment validation gate (code/eval/deploy_gate/gate.py):
  a change to a served deployment may not ship unless every one of five checks
  passes. The gate runs smoke, config, quality, perf and lmcache in that order and
  returns PASS only when all five pass; otherwise it refuses and names the first
  check that failed, so the operator sees the blocking check rather than a generic
  red. That ladder -- which checks gate a ship, that all must pass, and the first
  failure in the gate's own order -- is the decision this graph encodes.

  One fold is documented: each check's own pass/fail is the observable input here.
  Turning a measurement into pass/fail is threshold arithmetic over live-server
  numbers that live in code/eval/deploy_gate/thresholds.json and is applied by the
  harness that produces the check status (the smoke conditions, the config bounds
  max_model_len >= 1048576 and the expected model id, the tier1 pass_rate >= 0.9 and
  the deterministic golden match_rate >= 0.9, the per-context prefill/gen/ttft
  bounds, the lmcache metrics 200 / valkey L2 dbsize > 0 / warm speedup >= 1.2). So
  the graph is the gate's policy over five check outcomes, not the measurement.

  Reference order (gate.py main()): results are appended smoke, config, quality,
  perf, lmcache; `all_pass = all(status == "PASS")`, and the first non-PASS result
  is the blocking check.

  Background:
    Given the workflow runs at most 6 iterations

  Scenario: smoke
    Given a choice question "check" asks "smoke: is the server up? /health returns 200, /v1/models lists at least one model, a chat completion reports ok, and its output is non-empty."
      | answer | meaning |
      | pass | every smoke condition held |
      | fail | at least one smoke condition failed |
    And the answer comes from the field "ev_smoke"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "config"
    Then the primary question is "check"
    Then label "FAIL_SMOKE" when the answer to "check" is "fail"
    And label "SMOKE_OK" otherwise

  Scenario: config
    Given a choice question "check" asks "config: does the served model match the deployment? max_model_len is at least the required length and the model id is the expected one."
      | answer | meaning |
      | pass | both config conditions held |
      | fail | max_model_len is below the bound, or the model id is not the expected one |
    And the answer comes from the field "ev_config"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "quality"
    Then the primary question is "check"
    Then label "FAIL_CONFIG" when the answer to "check" is "fail"
    And label "CONFIG_OK" otherwise

  Scenario: quality
    Given a choice question "check" asks "quality: does the tier1 corpus pass the bar? pass_rate is at least the threshold, and when a golden file is present the deterministic match_rate is at least its threshold."
      | answer | meaning |
      | pass | the pass_rate, and any golden match, cleared their thresholds |
      | fail | pass_rate, or a golden match, fell below its threshold |
    And the answer comes from the field "ev_quality"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "perf"
    Then the primary question is "check"
    Then label "FAIL_QUALITY" when the answer to "check" is "fail"
    And label "QUALITY_OK" otherwise

  Scenario: perf
    Given a choice question "check" asks "perf: is throughput good enough at every measured context? prefill and generation tokens/s are at least their floors and time-to-first-token is within its ceiling, for each context band."
      | answer | meaning |
      | pass | every context band met its prefill, generation and ttft bounds |
      | fail | a context band missed a prefill, generation or ttft bound, or its measurement did not succeed |
    And the answer comes from the field "ev_perf"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "lmcache"
    Then the primary question is "check"
    Then label "FAIL_PERF" when the answer to "check" is "fail"
    And label "PERF_OK" otherwise

  Scenario: lmcache
    Given a choice question "check" asks "lmcache: is the shared KV cache link live end to end? the lmcache metrics endpoint is 200, the valkey L2 dbsize is above zero, and a warm request is faster than the cold one by at least the speedup floor. The last check: a pass here ships."
      | answer | meaning |
      | pass | the lmcache endpoint, the valkey L2 store and the warm speedup all held |
      | fail | the endpoint, the L2 store or the warm speedup did not hold |
    And the answer comes from the field "ev_lmcache"
    When any other answer goes to "STOP"
    Then the primary question is "check"
    Then label "FAIL_LMCACHE" when the answer to "check" is "fail"
    And label "PASS" otherwise
