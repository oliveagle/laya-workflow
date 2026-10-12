@decision
Feature: feishu_chat_history

  Background:
    Given the workflow runs at most 6 iterations

  Scenario: fetch
    Given a choice question "read_ok" asks "Did the Feishu history fetch return any messages?"
      | answer | meaning              |
      | A      | yes, messages were returned    |
      | B      | no, the read failed or returned nothing |
    When any other answer goes to "triage"
    Then call the capability "history"
      | project.messages_raw  | /stdout |
      | project.exit_code  | /exit_code |

  Scenario: triage
    Given a choice question "needs_attention" asks "Given the recent Feishu chat history above, does anything need the user's attention right now?"
      | answer | meaning              |
      | A      | no, it is all routine          |
      | B      | yes, something is urgent or needs a reply |
    When any other answer goes to "STOP"

