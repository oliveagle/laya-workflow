@decision
Feature: project_structure_guard

  Background:
    Given the workflow runs at most 2 iterations

  Scenario: structure
    Given a score question "failure_count" asks "Number of FAIL-level structure findings (missing required files, tracked binaries, root/src/docs log files, tracked .env, missing/empty .gitignore)."
      | level          |
      | 0 clean                        |
      | 1 one failure                  |
      | 10 many failures               |
    And score "failure_count" from the numeric field "failures" with miss 1
    And a score question "warning_count" asks "Number of WARN-level structure findings (untracked binaries, large files, missing logs/docs dirs, scattered root docs, YAML config, temp dirs not ignored, missing .gitignore patterns)."
      | level          |
      | 0 clean                        |
      | 1 one warning                  |
      | 10 many warnings               |
    And score "warning_count" from the numeric field "warnings"
    And a score question "health_score" asks "Health score 0-100 computed by the probe as 100 - failures*10 - warnings*3, clamped to [0,100]. Passed through as the action payload value."
      | level          |
      | 0 critical                     |
      | 50 degraded                    |
      | 80 healthy                     |
      | 100 pristine                   |
    And score "health_score" from the numeric field "health_score"
    When any other answer goes to "STOP"
    Then the primary question is "health_score"
    And label "FAIL" when "failure_count" is at least 1
    And label "WARN" when "warning_count" is at least 1
    And label "OK" otherwise
    And keep the state key "failures"
    And keep the state key "warnings"
    And keep the state key "health_score"
    And keep the state key "project"

