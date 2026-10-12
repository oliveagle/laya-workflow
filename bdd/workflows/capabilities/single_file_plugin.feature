@decision
Feature: single_file_plugin

  Background:
    Given the workflow runs at most 2 iterations

    Given the policy
      | allow_exec | false |
      | allow_hosts |  |
      | max_output | 65536 |

    Given the capabilities
      | greet | {"kind":"plugin","dir":"dsl/capabilities/hello.rhai"} |
  Scenario: greet
    Given a choice question "done" asks "Run the single-file plugin?"
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When any other answer goes to "STOP"
    Then call the capability "greet"
      | with.who      | ${state.who} |
      | project.hi  | /hi |
      | project.plugin  | /plugin |
      | project.op  | /op |

