@decision
Feature: agent_session_probe

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: ask_agent
    Given a choice question "route" asks "Should this request be handled by the local engine, or forwarded to the external agent?"
      | answer | meaning              |
      | local  | the local engine can decide this itself |
      | agent  | an external agent should be consulted |
    When any other answer goes to "STOP"
    Then call the capability "app_server"
      | with.text      | ${state.text} |
      | project.agent_output  | /body/output |

