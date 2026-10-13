@decision
Feature: rate_limiter_config_guard
  Guard for LLM-proposed rate-limiter configuration as a single triage node.
  The model proposes one typed config (request_id, endpoint, algorithm,
  rate_per_second, burst_size, reason); this workflow owns the algorithm
  vocabulary, the rate band, the burst bound and the endpoint path check.
  Port of code/qwen_vm/app_scenarios/rate_limiter_config_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (dangerous tokens, required fields, JSON object) fold
    into the `proposal` choice.
  - The policy band (min_rate_per_second / max_rate_per_second /
    max_burst_size) plus the constants (1..1_000_000, burst <= 100x rate) are a
    fixed fixture; each check folds into one verdict choice. The Python engine
    accumulates every error; the port expresses «first failing check» in the
    Python source order (algorithm, rate, burst, endpoint), documented here.

  Emitted labels: reject_proposal, reject_algorithm, reject_rate,
  reject_burst, reject_endpoint, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, endpoint, algorithm, rate_per_second, reason} object (no dangerous tokens, all required fields present)?"
      | answer    | meaning                                      |
      | accepted  | exactly one schema-valid config              |
      | malformed | dangerous token, bad JSON or missing field   |
    And the answer comes from the field "ev_proposal"
    And a choice question "algorithm" asks "Is the proposed algorithm one of the allowed set token_bucket / sliding_window / fixed_window / leaky_bucket?"
      | answer  | meaning                    |
      | ok      | algorithm is in the set    |
      | invalid | algorithm is not allowed   |
    And the answer comes from the field "ev_algorithm"
    And a choice question "rate" asks "Is rate_per_second inside the allowed band (>= minimum, <= maximum, and within the policy band)?"
      | answer | meaning                            |
      | ok     | rate is within the band            |
      | below  | rate is below the allowed minimum  |
      | above  | rate exceeds the allowed maximum   |
    And the answer comes from the field "ev_rate"
    And a choice question "burst" asks "Is burst_size within bounds (0 means unset; otherwise >= 1, <= 100x rate and <= policy max_burst_size)?"
      | answer  | meaning                                       |
      | ok      | burst is unset or within bounds               |
      | invalid | burst is below 1 or above its bound           |
    And the answer comes from the field "ev_burst"
    And a choice question "endpoint" asks "Is the endpoint path free of directory-traversal markers?"
      | answer     | meaning                          |
      | ok         | endpoint path is clean          |
      | suspicious | endpoint contains a .. marker   |
    And the answer comes from the field "ev_endpoint"
    When any other answer goes to "STOP"
    Then the primary question is "rate"
    And the action reads the question "rate"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_algorithm" when the answer to "algorithm" is "invalid"
    And label "reject_rate" when the answer to "rate" is "below"
    And label "reject_rate" when the answer to "rate" is "above"
    And label "reject_burst" when the answer to "burst" is "invalid"
    And label "reject_endpoint" when the answer to "endpoint" is "suspicious"
    And label "approve" otherwise
    And keep the state key "request_id"
