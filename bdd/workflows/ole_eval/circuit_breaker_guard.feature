@decision
Feature: circuit_breaker_guard
  Guard for LLM-proposed circuit-breaker state transitions as a single triage node.
  The model proposes one typed transition (request_id, service_id, current_state,
  proposed_state, error_rate, latency_p99_ms, open_duration_seconds); this workflow
  owns the state-machine legality, the open/close error-rate and the half-open
  probe-duration rules. Port of code/qwen_vm/app_scenarios/circuit_breaker_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required fields, dangerous-token rejection) fold into the
    `proposal` choice.
  - The fixed transition table {closed->open, open->half_open, half_open->closed,
    half_open->open, any no-op} is a local fixture; the legality verdict folds into
    the `transition` choice.
  - The Python guard accumulates every error and joins the reasons; the ordered
    ladder here reports the first failing check (which decides reject vs approve
    identically), documented as a folding boundary.

  Emitted labels: reject_proposal, reject_invalid_state, reject_illegal_transition,
  reject_open_threshold, reject_half_open_duration, reject_close_error_rate, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, service_id, current_state, proposed_state, error_rate, latency_p99_ms, open_duration_seconds} object (no dangerous tokens)?"
      | answer    | meaning                                       |
      | accepted  | exactly one schema-valid transition proposal  |
      | malformed | parse error, missing field or dangerous token |
    And the answer comes from the field "ev_proposal"
    And a choice question "state" asks "Are both current_state and proposed_state in {closed, open, half_open}?"
      | answer  | meaning                                    |
      | valid   | both states are known                      |
      | invalid | current_state or proposed_state is unknown |
    And the answer comes from the field "ev_state"
    And a choice question "transition" asks "Is current_state -> proposed_state an allowed transition (a no-op is always allowed)?"
      | answer  | meaning                                  |
      | legal   | the transition is allowed                |
      | illegal | the transition is not in the allowed set |
    And the answer comes from the field "ev_transition"
    And a choice question "open_threshold" asks "If proposed_state is open, is error_rate at or above the policy threshold (or force_open)?"
      | answer | meaning                                             |
      | ok     | not opening, or error_rate >= threshold             |
      | below  | opening while error_rate < threshold and not forced |
    And the answer comes from the field "ev_open_threshold"
    And a choice question "half_open_duration" asks "If proposed_state is half_open with min_open_duration > 0, is open_duration_seconds at or above the minimum?"
      | answer    | meaning                                               |
      | ok        | not half-opening, or below the min rule is satisfied  |
      | too_short | half-opening before the minimum open duration elapsed |
    And the answer comes from the field "ev_half_open_duration"
    And a choice question "close_error_rate" asks "If proposed_state is closed, is error_rate at or below the policy close_error_rate_max?"
      | answer   | meaning                                            |
      | ok       | not closing, or error_rate <= close_error_rate_max |
      | too_high | closing while error_rate > close_error_rate_max    |
    And the answer comes from the field "ev_close_error_rate"
    When any other answer goes to "STOP"
    Then the primary question is "close_error_rate"
    And the action reads the question "close_error_rate"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_invalid_state" when the answer to "state" is "invalid"
    And label "reject_illegal_transition" when the answer to "transition" is "illegal"
    And label "reject_open_threshold" when the answer to "open_threshold" is "below"
    And label "reject_half_open_duration" when the answer to "half_open_duration" is "too_short"
    And label "reject_close_error_rate" when the answer to "close_error_rate" is "too_high"
    And label "approve" otherwise
    And keep the state key "request_id"
