@decision
Feature: coding_env_check

  Background:
    Given the workflow runs at most 2 iterations

  Scenario: decision
    Given a score question "failed_count" asks "Number of FAIL-level env checks (missing tool, missing dir, wrong perms, nvidia-smi unavailable)."
      | level          |
      | 0 clean                        |
      | 1 one fail                     |
      | 10 many fails                  |
    And score "failed_count" from the numeric field "failed" with miss 1
    And a score question "warned_count" asks "Number of WARN-level env checks (version below min, sysctl sub-optimal, missing optional tool)."
      | level          |
      | 0 clean                        |
      | 1 one warn                     |
      | 10 many warns                  |
    And score "warned_count" from the numeric field "warned"
    And a score question "pass_rate" asks "Pass rate percent passed/(passed+warned+failed)*100."
      | level          |
      | 0 broken                       |
      | 60 degraded                    |
      | 90 acceptable                  |
      | 100 perfect                    |
    And score "pass_rate" from the numeric field "pass_rate"
    When any other answer goes to "STOP"
    Then label "FAIL" when "failed_count" is at least 1
    And label "WARN" when "warned_count" is at least 1
    And label "PERFECT" otherwise
    And keep the state key "failed"
    And keep the state key "warned"
    And keep the state key "passed"
    And keep the state key "pass_rate"

