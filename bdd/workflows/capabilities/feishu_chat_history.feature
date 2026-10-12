@decision
Feature: feishu_chat_history

  Background:
    Given the workflow runs at most 6 iterations

    Given the policy
      | allow_exec | true |
      | max_timeout_ms | 60000 |
      | max_output | 1048576 |
      | retries | 0 |

    Given the capabilities
      | history | {"kind":"exec","argv":["lark-cli","im","+chat-messages-list","--chat-id","${state.chat_id}","--page-size","20","--order","desc","--no-reactions","--format","json"],"timeout_ms":30000} |
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

