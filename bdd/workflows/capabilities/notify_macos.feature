@decision
Feature: notify_macos

  Background:
    Given the workflow runs at most 4 iterations

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

