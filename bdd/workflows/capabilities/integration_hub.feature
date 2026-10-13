@decision
Feature: integration_hub

  Background:
    Given the workflow runs at most 20 iterations

    Given the policy
      | allow_exec | true |
      | allow_hosts | "127.0.0.1", "localhost" |
      | max_timeout_ms | 8000 |
      | max_output | 65536 |
      | retries | 1 |

    Given the capabilities
      | backend_rpc | {"kind":"rpc","url":"http://127.0.0.1:8792/rpc","timeout_ms":4000} |
      | catalog | {"kind":"graphql","url":"http://127.0.0.1:8792/graphql","query":"query Lookup($q: String!) { search(q: $q) { id } }","timeout_ms":4000} |
      | summarizer | {"kind":"llm","url":"http://127.0.0.1:8792/chat/completions","model":"mock-model","system":"Summarize briefly.","max_tokens":64,"timeout_ms":5000} |
      | tool_gateway | {"kind":"mcp","transport":"http","url":"http://127.0.0.1:8792/mcp","tool":"echo","timeout_ms":4000} |
      | index | {"kind":"vector","url":"http://127.0.0.1:8792/vector","op":"search","collection":"support-kb","timeout_ms":4000} |
      | notify | {"kind":"webhook","url":"http://127.0.0.1:8792/webhook","sign_header":"x-signature","sign_secret":"${env.LAYA_WEBHOOK_SECRET}","timeout_ms":4000} |
      | stream | {"kind":"sse","url":"http://127.0.0.1:8792/events","max_events":3,"timeout_ms":4000} |
  Scenario: lookup
    Given a choice question "channel" asks "Which integration channel should handle this request?"
      | answer | meaning              |
      | rpc    | an internal RPC backend call   |
      | graphql | the federated graph            |
      | llm    | an LLM summarizer              |
      | mcp    | a tool gateway (MCP)           |
      | vector | a retrieval / vector search    |
      | notify | fire-and-forget notification   |
      | stream | watch an event stream          |
    And the answer comes from the field "channel"
    When the answer "rpc" goes to "via_rpc"
    And the answer "graphql" goes to "via_graphql"
    And the answer "llm" goes to "via_llm"
    And the answer "mcp" goes to "via_mcp"
    And the answer "vector" goes to "via_vector"
    And the answer "notify" goes to "via_webhook"
    And the answer "stream" goes to "via_sse"
    And any other answer goes to "STOP"
    Then keep the answer to "channel"
    And allow up to 1 retries

  Scenario: via_rpc
    Given a choice question "ok" asks "Should we proceed?"
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When any other answer goes to "STOP"
    Then call the capability "backend_rpc"
      | with.method      | sum |
      | with.params      | {"a": 1, "b": 2} |
      | project.rpc_method  | /result/method |

  Scenario: via_graphql
    Given a choice question "ok" asks "Should we proceed?"
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When any other answer goes to "STOP"
    Then call the capability "catalog"
      | with.variables      | {"q": "${state.text}"} |
      | project.graph_echo  | /data/echo/q |

  Scenario: via_llm
    Given a choice question "ok" asks "Should we proceed?"
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When any other answer goes to "STOP"
    Then call the capability "summarizer"
      | with.prompt      | ${state.text} |
      | project.summary  | /content |

  Scenario: via_mcp
    Given a choice question "ok" asks "Should we proceed?"
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When any other answer goes to "STOP"
    Then call the capability "tool_gateway"
      | with.arguments      | {"text": "${state.text}"} |
      | project.tool_text  | /result/content/0/text |

  Scenario: via_vector
    Given a choice question "ok" asks "Should we proceed?"
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When any other answer goes to "STOP"
    Then call the capability "index"
      | with.vector      | [0.1, 0.2, 0.3] |
      | with.top_k      | 1 |
      | project.top_score  | /hits/0/score |

  Scenario: via_webhook
    Given a choice question "ok" asks "Should we proceed?"
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When any other answer goes to "STOP"
    Then call the capability "notify"
      | with.event      | routed |
      | with.payload      | {"text": "${state.text}"} |
      | project.notify_status  | /status |

  Scenario: via_sse
    Given a choice question "ok" asks "Should we proceed?"
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When any other answer goes to "STOP"
    Then call the capability "stream"
      | project.event_count  | /count |

