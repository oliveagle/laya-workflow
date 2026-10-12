@decision
Feature: agents_rebase_push_gate
  Port of rebase_push_gate: converted from dsl/agents/rebase_push_gate.json (BDD round-trip).

  Scenario: decision
    Given a choice question "fetch_ok" asks "Did git fetch origin succeed?"
      | answer | meaning |
      | A      | fetch OK |
      | B      | fetch failed |
    And answer "B" when the text contains "fetch_failed", "fetch_refused", "origin_unreachable" with hit 0.95 and miss 0.05
    And a choice question "ahead_positive" asks "Are there local commits ahead of origin (ahead > 0)?"
      | answer | meaning |
      | A      | ahead > 0 |
      | B      | ahead == 0 |
    And answer "B" when the text contains "ahead_zero", "no_new_commit", "no_commit_to_push" with hit 0.95 and miss 0.05
    And a choice question "rebase_clean" asks "Did git rebase --autostash complete without conflicts?"
      | answer | meaning |
      | A      | rebase clean |
      | B      | rebase conflict |
    And answer "B" when the text contains "rebase_conflict", "rebase_failed", "unmerged_paths", "CONFLICT" with hit 0.95 and miss 0.05
    And a choice question "secret_scan_clean" asks "Did the secret regex on origin..HEAD diff find any credential pattern?"
      | answer | meaning |
      | A      | no secret |
      | B      | credential hit |
    And answer "B" when the field "secret_diff" matches "(?i)(api[_-]?key|secret|password|access[_-]?key|token)\s*[:=]\s*['\"]([a-z0-9]{16,}|sk-[a-zA-Z0-9]+|ghp_[a-zA-Z0-9]+|AKIA[0-9A-Z]{16})" with hit 0.95 and miss 0.05
    When any other answer goes to "STOP"
    Then label "FAIL_FETCH" when the answer to "fetch_ok" is "B"
    And label "NO_COMMIT" when the answer to "ahead_positive" is "B"
    And label "FAIL_REBASE_CONFLICT" when the answer to "rebase_clean" is "B"
    And label "FAIL_SECRET_BLOCK" when the answer to "secret_scan_clean" is "B"
    And label "PASS" otherwise
    And keep the state key "fetch_failed"
    And keep the state key "ahead_zero"
    And keep the state key "rebase_conflict"
    And keep the state key "secret_diff"
    And keep the state key "branch"
