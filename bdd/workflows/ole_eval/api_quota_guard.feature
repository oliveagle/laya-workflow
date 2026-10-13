@decision
Feature: api_quota_guard
  Deterministic API scope, quota, rate, concurrency and budget guard as a single
  triage node. The model proposes one typed request intent (principal, endpoint,
  requested_units, context_intent); this workflow owns schema validation, the
  request-id replay ledger, clock monotonicity, scope admission, the per-endpoint
  request maximum, concurrency, the token bucket, the daily/monthly unit quotas
  and the spend budget. Port of code/qwen_vm/app_scenarios/api_quota_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (size bound, duplicate-key rejection, the exact
    {proposal:{...}} shape, unknown or forbidden authority/side-effect fields)
    fold into the single `proposal` choice.
  - Token-bucket refill arithmetic (elapsed_ms * refill_units_per_second / 1000,
    capped at the bucket capacity) and the retry_after_ms it implies are
    deterministic Decimal clock arithmetic and do not belong in the DSL; the
    bucket is judged here only as `bucket` = ok | insufficient.
  - The daily-window reset hint (daily_reset_after_ms > 0 -> throttle with a
    retry, otherwise reject outright) folds into the two `quota` answers
    daily_retry / daily_no_retry.
  - Per-principal and per-endpoint fixtures (scopes, limits, budgets, costs,
    bucket capacities, refill rates) are fixed local policy tables, not model
    input.

  Emitted labels: reject_proposal, reject_replay, reject_clock_regression,
  reject_scope, reject_units_above_max, throttle_concurrency, throttle_bucket,
  throttle_daily, reject_daily_no_retry, reject_monthly, reject_spend, allow.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one object {proposal:{principal, endpoint, requested_units, context_intent}} with enum-valued principal/endpoint/context_intent, an integer requested_units in [1,1000], no duplicate JSON keys and no model-supplied authority or side-effect field?"
      | answer    | meaning                                                              |
      | accepted  | exactly one schema-valid typed request intent                        |
      | malformed | parse error, size/dup-key violation, unknown enum or forbidden field |
    And the answer comes from the field "ev_proposal"
    And a choice question "replay" asks "Is this request_id fresh, or was it already evaluated locally (the replay ledger)?"
      | answer   | meaning                          |
      | fresh    | request_id not seen before       |
      | replayed | request_id already in the ledger |
    And the answer comes from the field "ev_replay"
    And a choice question "clock" asks "Does the authoritative timestamp not precede the last bucket refill?"
      | answer    | meaning                                        |
      | ok        | timestamp_ms >= last bucket refill              |
      | regressed | timestamp_ms < last bucket refill (clock skew)  |
    And the answer comes from the field "ev_clock"
    And a choice question "scope" asks "Does the principal hold the endpoint's required scope?"
      | answer   | meaning                                     |
      | ok       | required_scope is in the principal's scopes |
      | mismatch | the principal lacks the endpoint's scope    |
    And the answer comes from the field "ev_scope"
    And a choice question "units" asks "Is requested_units within the endpoint's per-request maximum?"
      | answer    | meaning                                  |
      | ok        | requested_units <= max_units_per_request |
      | above_max | requested_units > max_units_per_request  |
    And the answer comes from the field "ev_units"
    And a choice question "concurrency" asks "Is current concurrency below the endpoint's max concurrent requests?"
      | answer   | meaning                                          |
      | ok       | current_concurrency < max_concurrent_requests     |
      | at_limit | current_concurrency >= max_concurrent_requests    |
    And the answer comes from the field "ev_concurrency"
    And a choice question "bucket" asks "After local refill arithmetic, are there enough tokens for requested_units?"
      | answer       | meaning                             |
      | ok           | available tokens >= requested_units |
      | insufficient | available tokens < requested_units  |
    And the answer comes from the field "ev_bucket"
    And a choice question "quota" asks "Projected daily/monthly unit totals vs the principal's limits (a daily-window reset hint decides throttle-with-retry vs outright reject)."
      | answer         | meaning                                                     |
      | ok             | projected daily and monthly totals stay within limits       |
      | daily_retry    | projected daily exceeds the limit, a reset hint is present  |
      | daily_no_retry | projected daily exceeds the limit, no reset hint            |
      | monthly        | projected monthly exceeds the limit                         |
    And the answer comes from the field "ev_quota"
    And a choice question "budget" asks "Is the projected spend (totals.spend + cost_per_unit * requested_units) within the principal's spend budget?"
      | answer | meaning                          |
      | ok     | projected spend <= spend_budget  |
      | over   | projected spend > spend_budget   |
    And the answer comes from the field "ev_budget"
    When any other answer goes to "STOP"
    Then the primary question is "budget"
    And the action reads the question "budget"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_replay" when the answer to "replay" is "replayed"
    And label "reject_clock_regression" when the answer to "clock" is "regressed"
    And label "reject_scope" when the answer to "scope" is "mismatch"
    And label "reject_units_above_max" when the answer to "units" is "above_max"
    And label "throttle_concurrency" when the answer to "concurrency" is "at_limit"
    And label "throttle_bucket" when the answer to "bucket" is "insufficient"
    And label "throttle_daily" when the answer to "quota" is "daily_retry"
    And label "reject_daily_no_retry" when the answer to "quota" is "daily_no_retry"
    And label "reject_monthly" when the answer to "quota" is "monthly"
    And label "reject_spend" when the answer to "budget" is "over"
    And label "allow" otherwise
    And keep the state key "request_id"
