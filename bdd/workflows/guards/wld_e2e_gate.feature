@decision
Feature: wld_e2e_gate
  Port of the ole-eval AWQ-INT4 cold-vs-warm deployment e2e
  (code/inference/optimization/wld_sglang_20260823/e2e_awq_int4_wld.py): it brings a
  27B AWQ-INT4 model up on GPU1 twice -- once COLD (page cache dropped, loading from
  the source dir) and once WARM (the weight-load daemon has prewarmed the weights into
  page cache, and the engine loads through a symlink) -- measures /health and
  load-weight time, and compares the two runs' generated content. It is the bring-up
  gate under the cold/warm optimization, so it names its verdict rather than
  reporting a number.

  It makes three gating calls, in the source's own order, each of which exits non-zero
  when it fails:

    - cold: the COLD engine must answer /health within 600s. A cold engine that never
      comes up prints FAIL, dumps the log tail, and exits 1.
    - wld_ready: the weight-load daemon must have become ready (its ready-file exists)
      before the WARM run. If it did not, the script prints FAIL and exits 1.
    - warm: the WARM engine must answer /health within 600s. If it does not, the script
      prints FAIL, dumps the log tail, and exits 1.

  The walk encodes exactly that: cold -> wld_ready -> warm, each failing ends the walk
  and names itself; only a run where both engines came up and the daemon was ready
  reaches the deployment verdict.

  Two folds are documented:

    - the cold-vs-warm generated content is compared (`ok = cold_gen["content"] ==
      warm_gen["content"]`), but the script never exits on it: a mismatch prints
      `content match: WARN`, is saved into the result JSON's `content_match`, and the
      process still exits 0. The graph models the deployment's *verdict* -- exit 0 iff
      every bring-up gate cleared -- and carries the content comparison as a recorded
      observation: its last node names the two outcomes `E2E_OK_CONTENT_MATCH` and
      `E2E_OK_CONTENT_MISMATCH`, both of which are exit-0 (the second recording that the
      two runs' text differed; it is not a failure).
    - each arm's own generation call (`gen`, max_tokens 32) can also raise on a hard
      request failure, which propagates as an uncaught exception (also non-zero). The
      script's only *designed* exits are the three above; the harness folds "the arm
      returned a response" into that arm's pass verdict.

  Everything numeric stays in the harness: the /health timings, the `Load weight end.
  elapsed=` values, the startup delta, and the generated text itself are measured and
  compared there; the graph reads none of them, only the harness's per-gate verdict and
  the content-match observation.

  Reference walk (e2e_awq_int4_wld.py, main body): drop cache + start COLD engine ->
  wait_health(600) or exit 1; start the WLD daemon -> ready-file or exit 1; start the
  WARM engine through the symlink -> wait_health(600) or exit 1; compare the two
  generated contents (recorded, not gated); save the result JSON.

  Background:
    Given the workflow runs at most 8 iterations

  Scenario: cold
    Given a choice question "e2e" asks "Does the COLD engine answer /health within the bring-up window? A cold engine that never comes up ends the walk -- the deployment bring-up failed."
      | answer | meaning |
      | fail | the COLD engine did not answer /health in time (the script exits 1) |
      | pass | the COLD engine answered /health |
    And the answer comes from the field "ev_cold"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "wld_ready"
    Then the primary question is "e2e"
    Then label "COLD_UNHEALTHY" when the answer to "e2e" is "fail"
    And label "COLD_UP" otherwise

  Scenario: wld_ready
    Given a choice question "e2e" asks "Did the weight-load daemon become ready before the WARM run? A daemon that never signalled ready ends the walk -- the WARM arm cannot be trusted."
      | answer | meaning |
      | fail | the daemon's ready-file did not appear (the script exits 1) |
      | pass | the daemon signalled ready |
    And the answer comes from the field "ev_wld"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "warm"
    Then the primary question is "e2e"
    Then label "WLD_DAEMON_NOT_READY" when the answer to "e2e" is "fail"
    And label "WLD_READY" otherwise

  Scenario: warm
    Given a choice question "e2e" asks "Does the WARM engine answer /health within the bring-up window? A warm engine that never comes up ends the walk -- the deployment bring-up failed."
      | answer | meaning |
      | fail | the WARM engine did not answer /health in time (the script exits 1) |
      | pass | the WARM engine answered /health |
    And the answer comes from the field "ev_warm"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "content"
    Then the primary question is "e2e"
    Then label "WARM_UNHEALTHY" when the answer to "e2e" is "fail"
    And label "WARM_UP" otherwise

  Scenario: content
    Given a choice question "e2e" asks "Do the two runs generate the same content? The script compares this and records it, but never exits on a mismatch -- so both answers are a passing verdict, the second recording that the two runs' text differed. This is the last call."
      | answer | meaning |
      | match | the COLD and WARM runs generated identical content |
      | mismatch | the two runs' content differed (recorded as WARN; still exit 0) |
    And the answer comes from the field "ev_content"
    When any other answer goes to "STOP"
    Then the primary question is "e2e"
    Then label "E2E_OK_CONTENT_MISMATCH" when the answer to "e2e" is "mismatch"
    And label "E2E_OK_CONTENT_MATCH" otherwise
