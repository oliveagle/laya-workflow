@decision
Feature: rate_limit_budget_guard
  Guard for LLM-proposed API rate limits as a single triage node. The model
  proposes one typed rate request (request_id, endpoint, requested_rate
  {rpm, tpm, burst}); this workflow owns the endpoint budget table, the
  over-limit rejection and the near-limit escalation. Port of
  code/qwen_vm/app_scenarios/rate_limit_budget_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required keys, request_id grammar, requested_rate
    dict with positive rpm/tpm/burst, dangerous-token rejection) fold into the
    `proposal` choice.
  - The budget table (endpoint -> requests_per_minute / tokens_per_minute /
    burst_limit) is a fixed fixture. The per-key ratio requested/limit
    (reject if > 1.0, escalate if >= 0.8) is folded into one `budget` verdict
    choice over the strictest configured key.

  Emitted labels: reject_proposal, reject_unknown_endpoint,
  reject_over_budget, escalate_near_budget, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, endpoint, requested_rate {rpm, tpm, burst}} object (request_id grammar, positive numeric rate fields, no dangerous tokens)?"
      | answer    | meaning                                                     |
      | accepted  | exactly one schema-valid rate request                        |
      | malformed | parse error, missing key, bad request_id or dangerous token |
    And the answer comes from the field "ev_proposal"
    And a choice question "endpoint" asks "Is the requested endpoint present in the local budget table?"
      | answer  | meaning                          |
      | known   | endpoint has a configured budget |
      | unknown | endpoint is not in the table     |
    And the answer comes from the field "ev_endpoint"
    And a choice question "budget" asks "Across the budget table's configured keys (rpm / tpm / burst), how does the highest requested/limit ratio compare to the limit?"
      | answer | meaning                                          |
      | ok     | every ratio < 0.8                                |
      | near   | highest ratio is in [0.8, 1.0] (at or above 80%) |
      | over   | highest ratio > 1.0 (some request exceeds limit) |
    And the answer comes from the field "ev_budget"
    When any other answer goes to "STOP"
    Then the primary question is "budget"
    And the action reads the question "budget"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_unknown_endpoint" when the answer to "endpoint" is "unknown"
    And label "reject_over_budget" when the answer to "budget" is "over"
    And label "escalate_near_budget" when the answer to "budget" is "near"
    And label "approve" otherwise
    And keep the state key "request_id"
