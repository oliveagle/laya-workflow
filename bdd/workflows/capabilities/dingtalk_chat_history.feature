@decision
Feature: dingtalk_chat_history

  Background:
    Given the workflow runs at most 6 iterations

    Given the policy
      | allow_exec | true |
      | max_timeout_ms | 60000 |
      | max_output | 1048576 |
      | retries | 0 |

    Given the capabilities
      | sessions | {"kind":"exec","argv":["${state.tool}","sessions","--json"],"timeout_ms":30000} |
      | messages | {"kind":"exec","argv":["${state.tool}","chat","--json"],"timeout_ms":30000} |
  Scenario: fetch
    Given a choice question "read_ok" asks "Did reading the DingTalk conversation list return any conversations?"
      | answer | meaning              |
      | A      | yes, conversations were returned |
      | B      | no, the read failed or returned nothing |
    When any other answer goes to "read_chat"
    Then call the capability "sessions"
      | project.sessions_raw  | /stdout |
      | project.exit_code  | /exit_code |

  Scenario: read_chat
    Given a choice question "needs_attention" asks "Given the DingTalk conversation list and the open conversation's messages above, does anything need the user's attention right now?"
      | answer | meaning              |
      | A      | no, it is all routine          |
      | B      | yes, something is urgent or needs a reply |
    When any other answer goes to "STOP"
    Then call the capability "messages"
      | project.messages_raw  | /stdout |

