@decision
Feature: notify_macos

  Background:
    Given the workflow runs at most 4 iterations

    Given the policy
      | allow_exec | true |
      | allow_paths | "${env.LAYA_WORK_DIR}" |
      | max_timeout_ms | 10000 |
      | max_output | 8192 |
      | retries | 0 |

    Given the capabilities
      | banner | {"kind":"notify","channel":"auto","title":"laya-workflow","subtitle":"${state.topic}","sound":"Glass","path":"${env.LAYA_WORK_DIR}/notify.log","timestamp":true} |
  Scenario: announce
    Given a choice question "urgency" asks "How urgent is this notification?"
      | answer | meaning              |
      | A      | informational, no action needed |
      | B      | action needed soon             |
    When any other answer goes to "STOP"
    Then call the capability "banner"
      | with.event      | notify |
      | with.message      | ${state.text} |
      | project.channel  | /channel |
      | project.message  | /message |

