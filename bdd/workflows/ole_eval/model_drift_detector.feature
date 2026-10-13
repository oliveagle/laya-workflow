@decision
Feature: model_drift_detector
  Guard for LLM-proposed model drift assessments as a single triage node. The
  model proposes one typed assessment (request_id, model_id, metrics,
  baseline_metrics {accuracy, latency_p99_ms, throughput_rps}, drift_threshold);
  this workflow owns the accuracy-drop rejection and the latency/throughput
  escalation. Port of code/qwen_vm/app_scenarios/model_drift_detector.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required keys, non-empty model_id, numeric metric
    triples, drift_threshold in (0.0, 1.0], dangerous-token rejection) fold into
    the `proposal` choice.
  - The drift policy (max_accuracy_drop, max_latency_increase_pct,
    max_throughput_drop_pct) is a fixed fixture. The accuracy drop, the latency
    increase percentage and the throughput drop percentage are folded into two
    verdict choices.

  Emitted labels: reject_proposal, reject_accuracy_drop, escalate_performance,
  approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, model_id, metrics, baseline_metrics, drift_threshold} object (numeric metric triples, threshold in (0.0, 1.0], no dangerous tokens)?"
      | answer    | meaning                                                              |
      | accepted  | exactly one schema-valid drift assessment                            |
      | malformed | parse error, missing key, bad model_id, bad metric or bad threshold  |
    And the answer comes from the field "ev_proposal"
    And a choice question "accuracy" asks "Is the accuracy drop (baseline minus current) within the policy max_accuracy_drop?"
      | answer | meaning                                     |
      | ok     | accuracy drop <= max_accuracy_drop          |
      | over   | accuracy drop > max_accuracy_drop           |
    And the answer comes from the field "ev_accuracy"
    And a choice question "performance" asks "Are the latency increase percentage and throughput drop percentage both within their policy bounds?"
      | answer | meaning                                                       |
      | ok     | latency increase and throughput drop both within policy       |
      | over   | latency increase or throughput drop exceeds its policy bound  |
    And the answer comes from the field "ev_performance"
    When any other answer goes to "STOP"
    Then the primary question is "accuracy"
    And the action reads the question "accuracy"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_accuracy_drop" when the answer to "accuracy" is "over"
    And label "escalate_performance" when the answer to "performance" is "over"
    And label "approve" otherwise
    And keep the state key "request_id"
