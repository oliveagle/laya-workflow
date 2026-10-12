@decision
Feature: deployment_canary_guard

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: policy
    Given a choice question "service_name_safe" asks "Is the service_name free of dangerous tokens (eval(, exec(, subprocess, os.system, javascript:, data:, rm -rf, ...)?"
      | answer | meaning              |
      | A      | yes, safe identifier           |
      | B      | no, dangerous token present    |
    And a choice question "health_path_safe" asks "Is the health_check_path free of dangerous tokens?"
      | answer | meaning              |
      | A      | yes, safe path                 |
      | B      | no, dangerous token present    |
    And a choice question "canary_within_cap" asks "Is every stage's canary_percent <= max_canary_percent (default 50)?"
      | answer | meaning              |
      | A      | yes, all stages within cap     |
      | B      | no, at least one stage exceeds |
    And a choice question "rollback_threshold_ok" asks "Is error_rate <= max_error_rate (default 0.05) AND latency_p99_ms <= max_latency_p99_ms (default 5000)?"
      | answer | meaning              |
      | A      | yes, both within bounds        |
      | B      | no, one of them is over        |
    And a choice question "stage_shape_ok" asks "Is the stages list non-empty and <= max_stages (default 10), with non-negative percent and hold_minutes?"
      | answer | meaning              |
      | A      | yes, valid shape               |
      | B      | no, malformed                  |
    And a choice question "hold_within_cap" asks "Is every stage's hold_minutes <= max_hold_minutes (default 120)?"
      | answer | meaning              |
      | A      | yes                            |
      | B      | no, a stage holds too long     |
    And a score question "canary_risk" asks "Overall risky-canary risk 0-100 (stage count, hold magnitude, rollback severity, dangerous tokens)."
      | level          |
      | 0 clean                        |
      | 30 reviewable                  |
      | 70 blocking                    |
      | 100 maximum                    |
    When any other answer goes to "STOP"
    Then label "REJECT" when the answer to "service_name_safe" is "B"
    Then label "REJECT" when the answer to "health_path_safe" is "B"
    Then label "REJECT" when the answer to "stage_shape_ok" is "B"
    Then label "REJECT" when the answer to "canary_within_cap" is "B"
    Then label "REJECT" when the answer to "rollback_threshold_ok" is "B"
    Then label "REJECT" when the answer to "hold_within_cap" is "B"
    And label "REJECT" when "canary_risk" is at least 70
    And label "ESCALATE" when "canary_risk" is at least 30
    And label "APPROVE" otherwise
    Then the primary question is "canary_risk"
    And keep the state key "request_id"
    And keep the state key "service_name"

