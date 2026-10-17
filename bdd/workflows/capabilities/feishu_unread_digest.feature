@decision
Feature: feishu_unread_digest
  飞书未读消息分类汇总 (Feishu unread digest): collect every unread message across your chats via the bundled `lark-cli`, bucket them by what they ask of you (mention / action / question / decision / info), and emit a short ranked digest. Two nodes: `collect` runs websites/feishu.com/plugin/collect.sh (exec, so policy.allow_exec is required), `classify` runs websites/feishu.com/plugin/classify.rhai which does the routing, and a choice node judges whether anything actually needs a reply. Reads only - it never sends, marks read, or reacts. Needs lark-cli on PATH plus `lark-cli auth login`; a stale user token makes the collector report ok:false rather than an empty digest. Both knobs are optional: an unset chats_limit/page_size expands to the empty string and collect.sh falls back to 15 chats / 20 messages.

  Background:
    Given the workflow runs at most 6 iterations

    Given the policy
      | allow_exec | true |
      | allow_hosts |  |
      | allow_paths | "${env.HOME}/tmp" |
      | max_timeout_ms | 180000 |
      | max_output | 8388608 |
      | retries | 0 |

    Given the capabilities
      | collect | {"kind":"shell","command":"sh websites/feishu.com/plugin/collect.sh \"${state.chats_limit}\" \"${state.page_size}\"","timeout_ms":170000} |
      | classify | {"kind":"plugin","dir":"websites/feishu.com/plugin/classify.rhai","timeout_ms":60000,"max_operations":2000000} |
  Scenario: collect
    Given a choice question "collected_ok" asks "Did the Feishu unread collection succeed? Answer no if stdout carries ok:false, if exit_code is non-zero, or if stdout is not the collector's JSON - a stale lark-cli token and a network failure both look like this."
      | answer | meaning              |
      | A      | yes: stdout is the collector's JSON with ok:true |
      | B      | no: the collection failed or returned nothing usable |
    When any other answer goes to "classify"
    Then call the capability "collect"
      | project.collected  | /stdout |
      | project.collect_exit  | /exit_code |

  Scenario: classify
    Given a choice question "classified" asks "Did the unread messages classify into buckets without error?"
      | answer | meaning              |
      | A      | yes: buckets and digest are present |
      | B      | no: classification failed      |
    When any other answer goes to "triage"
    Then call the capability "classify"
      | with.collected      | ${state.collected} |
      | project.total  | /total |
      | project.buckets  | /buckets |
      | project.digest  | /digest |
      | project.classify_error  | /error |

  Scenario: triage
    Given a choice question "needs_reply" asks "Reading the classified unread digest above, does anything need the user's reply right now? Weight the buckets in this order: mention first, then action and question, then decision. 'info' alone is not worth interrupting anyone."
      | answer | meaning              |
      | A      | no: nothing in the digest needs a reply now |
      | B      | yes: at least one message is waiting on the user |
    And answer "B" when the text contains "please", "need", "refund" with hit 0.7 and miss 0.2
    When any other answer goes to "STOP"

