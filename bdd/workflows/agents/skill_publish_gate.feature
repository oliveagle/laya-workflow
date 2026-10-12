@decision
Feature: skill_publish_gate

  Background:
    Given the workflow runs at most 2 iterations

  Scenario: check
    Given a choice question "structure_ok" asks "Are all required skill files present (skill.md, workflow.md, quickstart.md, examples.md, compose_tool.yaml, dsl/*, references/*)?"
      | answer | meaning              |
      | A      | all present                    |
      | B      | missing required file          |
    And answer "B" when the field "structure_status" contains "missing_required", "structure_fail" with hit 0.95
    And a choice question "line_count_ok" asks "Are all skill files within the line-count limit (no .md exceeds MAX_LINES)?"
      | answer | meaning              |
      | A      | all within limit               |
      | B      | file(s) over limit             |
    And answer "B" when the field "line_count_status" contains "over_limit", "lines_fail" with hit 0.9
    And a choice question "placeholder_clean" asks "Are all skill files free of placeholder markers (TODO / FIXME / PLACEHOLDER / STUB / XXX / HACK / NOTE:)?"
      | answer | meaning              |
      | A      | clean                          |
      | B      | placeholder marker found       |
    And answer "B" when the field "placeholder_status" contains "placeholder_found", "placeholder_fail" with hit 0.9
    And a choice question "links_ok" asks "Are all in-skill markdown links resolved (no broken targets)?"
      | answer | meaning              |
      | A      | all links valid                |
      | B      | broken link(s) found           |
    And answer "B" when the field "links_status" contains "broken_links", "link_fail" with hit 0.9
    When any other answer goes to "STOP"
    Then label "FAIL_STRUCTURE" when the answer to "structure_ok" is "B"
    And label "FAIL_LINE_COUNT" when the answer to "line_count_ok" is "B"
    And label "FAIL_PLACEHOLDER" when the answer to "placeholder_clean" is "B"
    And label "FAIL_LINKS" when the answer to "links_ok" is "B"
    And label "PASS" otherwise
    And keep the state key "structure_status"
    And keep the state key "line_count_status"
    And keep the state key "placeholder_status"
    And keep the state key "links_status"
    And keep the state key "skill_name"

