@decision
Feature: jev_deep_dive
  Port of jev_deep_dive: converted from dsl/browser/jev_deep_dive.json (BDD round-trip).

  Background:
    Given the workflow runs at most 200 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts |  |
      | max_timeout_ms | 30000 |
      | max_output | 1048576 |
      | retries | 0 |
      | allow_paths | "${env.HOME}/.laya-workflow", "${env.HOME}/tmp/jev-deep" |
    Given the capabilities
      | chrome | {"kind":"chrome_cdp","endpoint":"http://127.0.0.1:9222","launch":true,"startup_timeout_ms":20000,"timeout_ms":20000,"max_text":8000} |
      | planner | {"kind":"plugin","dir":"plugins/jev-planner","browser":"chrome","max_operations":8000000,"timeout_ms":20000} |
      | journal | {"kind":"file","op":"append","max_bytes":262144} |

  Scenario: open
    Given a choice question "go" asks "Open one scratch tab in the shared singleton browser and drive it for the whole run?"
      | answer | meaning |
      | A | yes, open the tab |
      | B | no, stop |
    When any other answer goes to "STOP"
    And the answer "A" goes to "goto"
    Then call the capability "chrome"
      | with.op | open |
      | with.url | about:blank |
      | project.browser_target | /target_id |

  Scenario: goto
    Given a choice question "go" asks "Navigate the owned tab to the seed URL and wait for the real navigation commit?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "step"
    Then call the capability "planner"
      | with.op | visit |
      | with.target_id | ${state.browser_target} |
      | with.url | ${state.url} |
      | with.query | ${state.query} |
      | project.landing_url | /url |
      | project.landing_title | /title |

  Scenario: step
    Given a choice question "go" asks "Observe the tab and produce the next Jev decision?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "route"
    Then call the capability "planner"
      | with.op | step |
      | with.target_id | ${state.browser_target} |
      | with.goal | ${state.goal} |
      | project.decision | / |
      | project.step_action | /operation |
      | project.step_round | /round |
      | project.step_url | /url |
      | project.step_rationale | /rationale |
      | project.step_goal_p | /goal_probability |
      | project.step_stuck_p | /stuck_probability |

  Scenario: route
    Given a choice question "keep_going" asks "The planner just returned an operation. Is there anything left to do on this page, or is the page exhausted?"
      | answer | meaning |
      | A | keep working the page |
      | B | the page is exhausted — stop |
    And answer "B" when the field "step_action" contains "DONE", "BLOCKED" with hit 0.9 and miss 0.05
    When any other answer goes to "STOP"
    And the answer "A" goes to "act"
    And the answer "B" goes to "STOP"

  Scenario: act
    Given a choice question "go" asks "Execute the typed decision with real CDP input (no synthetic DOM events)?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "journal"
    Then call the capability "chrome"
      | with.op | agent_step |
      | with.target_id | ${state.browser_target} |
      | with.decision | ${state.decision} |
      | project.act_payload | / |

  Scenario: journal
    Given a choice question "go" asks "Append this round to the journal and take another step?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "step"
    Then call the capability "journal"
      | with.path | ${env.HOME}/tmp/jev-deep/rounds.jsonl |
      | with.text | "${state.decision.line}\n${state.act_payload}\n" |
      | project.journal_bytes | /bytes |
