@decision
Feature: bitx_wrapper_test

  Background:
    Given the workflow runs at most 2 iterations

  Scenario: decision
    Given a score question "fail_count" asks "Number of wrapper invocations whose exit code mismatched the expected one."
      | level          |
      | 0 all matched                  |
      | 1 one mismatch                 |
      | 10 many                        |
    And score "fail_count" from the numeric field "failed" with miss 1
    And a score question "skip_count" asks "Number of wrapper cases skipped (env-only or unsupported by fake-bitx)."
      | level          |
      | 0 none                         |
      | 1 one skipped                  |
      | 10 many                        |
    And score "skip_count" from the numeric field "skipped"
    And a score question "pass_count" asks "Number of wrapper cases that matched their expected exit code."
      | level          |
      | 0 none                         |
      | 5 few                          |
      | 12 all                         |
    And score "pass_count" from the numeric field "passed"
    When any other answer goes to "STOP"
    Then label "FAIL" when "fail_count" is at least 1
    And label "WARN" when "skip_count" is at least 1
    And label "PASS" otherwise
    And keep the state key "failed"
    And keep the state key "skipped"
    And keep the state key "passed"

