@decision
Feature: textdigest_plugin

  Background:
    Given the workflow runs at most 2 iterations

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

