@decision
Feature: exec_preflight_guard

  Background:
    Given the workflow runs at most 6 iterations

    Given the policy
      | allow_exec | true |
      | max_timeout_ms | 5000 |
      | max_output | 8192 |
      | retries | 0 |

    Given the capabilities
      | disk_check | {"kind":"exec","argv":["/bin/sh","-c","echo 'target=${state.target}'; df -Pk '${state.target}' \| tail -1 \| awk '{print $4}'"],"timeout_ms":4000,"max_output":4096} |
  Scenario: preflight
    Given a choice question "is_risky" asks "Given that this is a destructive-looking operation on the target path, is it risky to proceed?"
      | answer | meaning              |
      | A      | no, it is a safe operation on this path |
      | B      | yes, it could destroy or corrupt important state |
    When any other answer goes to "STOP"
    Then call the capability "disk_check"
      | with.target      | ${state.target} |
      | project.free_kb_line  | /stdout |

