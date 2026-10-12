@decision
Feature: exec_preflight_guard

  Background:
    Given the workflow runs at most 6 iterations

  Scenario: preflight
    Given a choice question "is_risky" asks "Given that this is a destructive-looking operation on the target path, is it risky to proceed?"
      | answer | meaning              |
      | A      | no, it is a safe operation on this path |
      | B      | yes, it could destroy or corrupt important state |
    When any other answer goes to "STOP"
    Then call the capability "disk_check"
      | with.target      | ${state.target} |
      | project.free_kb_line  | /stdout |

