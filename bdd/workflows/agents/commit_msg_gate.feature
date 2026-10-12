@decision
Feature: agents_commit_msg_gate
  Port of commit_msg_gate: converted from dsl/agents/commit_msg_gate.json (BDD round-trip).

  Scenario: check
    Given a choice question "msg_empty" asks "Is the commit message (after stripping comment/blank lines, first non-blank line) empty?"
      | answer | meaning |
      | A      | non-empty |
      | B      | empty |
    And answer "B" when the field "message" matches "^\s*$" with hit 0.95 and miss 0.05
    And a choice question "msg_too_short" asks "Is the message shorter than 5 characters?"
      | answer | meaning |
      | A      | len >= 5 |
      | B      | len < 5 |
    And answer "B" when the field "message" matches "^.{0,4}$" with hit 0.95 and miss 0.05
    And a choice question "msg_single_token" asks "Is the message a single token (no whitespace, no colon, no paren, no em-dash) — looks like a placeholder?"
      | answer | meaning |
      | A      | has punctuation/whitespace |
      | B      | yes, single-token placeholder |
    And answer "B" when the field "message" matches "^[A-Za-z0-9_.\-]+$" with hit 0.9 and miss 0.1
    When any other answer goes to "STOP"
    Then the primary question is "msg_too_short"
    Then the action reads the question "msg_empty"
    Then label "FAIL_EMPTY" when the answer to "msg_empty" is "B"
    And label "FAIL_TOO_SHORT" when the answer to "msg_too_short" is "B"
    And label "FAIL_PLACEHOLDER" when the answer to "msg_single_token" is "B"
    And label "PASS" otherwise
    And keep the state key "message"
