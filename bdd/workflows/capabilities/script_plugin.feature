@decision
Feature: textdigest_plugin

  Background:
    Given the workflow runs at most 2 iterations

    Given the policy
      | allow_exec | false |
      | allow_hosts |  |
      | max_output | 262144 |

    Given the capabilities
      | digest | {"kind":"plugin","plugin":"plugins/textdigest","max_operations":2000000} |
  Scenario: digest
    Given a choice question "done" asks "Digest the text with the local plugin?"
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When any other answer goes to "STOP"
    Then call the capability "digest"
      | with.text      | ${state.text} |
      | project.chars  | /chars |
      | project.words  | /words |
      | project.tokens  | /tokens |
      | project.top  | /top |

