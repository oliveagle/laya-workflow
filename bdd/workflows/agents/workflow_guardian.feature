@decision
Feature: workflow_guardian

  Background:
    Given the workflow runs at most 2 iterations

  Scenario: check
    Given a choice question "task_layer" asks "Does the task layer scan pass (tasks dir exists, all inbox .md have Front Matter, all active .md have agent field)?"
      | answer | meaning              |
      | A      | pass                           |
      | B      | FAIL (hard violation)          |
    And answer "B" when the field "task_layer" contains "fail", "missing_frontmatter", "missing_agent", "tasks_missing" with hit 0.95
    And a choice question "knowledge_layer" asks "Does the knowledge layer scan pass (records dir exists, all year records have work-item table)?"
      | answer | meaning              |
      | A      | pass                           |
      | B      | WARN (soft violation)          |
    And answer "B" when the field "knowledge_layer" contains "warn", "missing_table", "records_missing" with hit 0.9
    And a choice question "collaboration_layer" asks "Does the collaboration layer scan pass (machines dir exists)?"
      | answer | meaning              |
      | A      | pass                           |
      | B      | FAIL (hard violation)          |
    And answer "B" when the field "collaboration_layer" contains "fail", "machines_missing" with hit 0.95
    When any other answer goes to "STOP"
    Then label "FAIL_TASK_LAYER" when the answer to "task_layer" is "B"
    And label "FAIL_COLLABORATION_LAYER" when the answer to "collaboration_layer" is "B"
    And label "WARN_KNOWLEDGE_LAYER" when the answer to "knowledge_layer" is "B"
    And label "OK" otherwise
    And keep the state key "task_layer"
    And keep the state key "knowledge_layer"
    And keep the state key "collaboration_layer"
    And keep the state key "scan_mode"

