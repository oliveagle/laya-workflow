@decision
Feature: deployment_canary_guard

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: policy
    Given a choice question "service_name_safe" asks "Is the service_name free of dangerous tokens (eval(, exec(, subprocess, os.system, javascript:, data:, rm -rf, ...)?"
      | answer | meaning              |
      | A      | yes, safe identifier           |
      | B      | no, dangerous token present    |
    And answer "B" when the text contains "eval(", "exec(", "subprocess", "rm -rf", "javascript:", "os.system" with hit 0.95 and miss 0.05
    And a choice question "health_path_safe" asks "Is the health_check_path free of dangerous tokens?"
      | answer | meaning              |
      | A      | yes, safe path                 |
      | B      | no, dangerous token present    |
    And answer "B" when the text contains "eval(", "javascript:", "data:", "os.system" with hit 0.9 and miss 0.05
    And a choice question "canary_within_cap" asks "Is every stage's canary_percent <= max_canary_percent (default 50)?"
      | answer | meaning              |
      | A      | yes, all stages within cap     |
      | B      | no, at least one stage exceeds |
    And answer "B" when the text contains "exceed_cap", "over_cap", "canary_percent_high" with hit 0.9 and miss 0.05
    And a choice question "rollback_threshold_ok" asks "Is error_rate <= max_error_rate (default 0.05) AND latency_p99_ms <= max_latency_p99_ms (default 5000)?"
      | answer | meaning              |
      | A      | yes, both within bounds        |
      | B      | no, one of them is over        |
    And answer "B" when the text contains "error_rate_high", "latency_high", "rollback_breach" with hit 0.9 and miss 0.05
    And a choice question "stage_shape_ok" asks "Is the stages list non-empty and <= max_stages (default 10), with non-negative percent and hold_minutes?"
      | answer | meaning              |
      | A      | yes, valid shape               |
      | B      | no, malformed                  |
    And answer "B" when the text contains "no_stages", "empty_stages", "too_many_stages", "negative_percent" with hit 0.9 and miss 0.05
    And a choice question "hold_within_cap" asks "Is every stage's hold_minutes <= max_hold_minutes (default 120)?"
      | answer | meaning              |
      | A      | yes                            |
      | B      | no, a stage holds too long     |
    And answer "B" when the text contains "hold_too_long", "hold_exceeds", "extended_hold" with hit 0.9 and miss 0.05
    And a score question "canary_risk" asks "Overall risky-canary risk 0-100 (stage count, hold magnitude, rollback severity, dangerous tokens)."
      | level          |
      | 0 clean                        |
      | 30 reviewable                  |
      | 70 blocking                    |
      | 100 maximum                    |
    And score "canary_risk" by rules on the text miss 10
      | score | needles |
      | 90    | "eval(", "rm -rf", "javascript:", "subprocess" |
      | 60    | "error_rate_high", "latency_high", "rollback_breach" |
      | 40    | "exceed_cap", "hold_too_long" |
      | 45    | "stages_wide" |
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

