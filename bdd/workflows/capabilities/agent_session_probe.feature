@decision
Feature: agent_session_probe

  Background:
    Given the workflow runs at most 5 iterations

    Given the policy
      | allow_exec | true |
      | allow_hosts | "127.0.0.1", "localhost" |
      | max_timeout_ms | 10000 |
      | max_output | 65536 |

    Given the capabilities
      | app_server | {"kind":"agent","transport":"http","url":"http://127.0.0.1:8791/agent","session":"probe-1","timeout_ms":5000} |
      | app_server_stdio | {"kind":"agent","transport":"stdio","command":["laya-workflow","mock","stdio"],"session":"probe-stdio","timeout_ms":10000} |
  Scenario: ask_agent
    Given a choice question "route" asks "Should this request be handled by the local engine, or forwarded to the external agent?"
      | answer | meaning              |
      | local  | the local engine can decide this itself |
      | agent  | an external agent should be consulted |
    When any other answer goes to "STOP"
    Then call the capability "app_server"
      | with.text      | ${state.text} |
      | project.agent_output  | /body/output |

