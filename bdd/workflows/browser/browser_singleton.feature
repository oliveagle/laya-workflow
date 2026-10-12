@decision
Feature: browser_singleton_smoke
  Port of browser_singleton: converted from dsl/browser/browser_singleton.json (BDD round-trip).

  Background:
    Given the workflow runs at most 4 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts | "127.0.0.1", "localhost" |
      | max_timeout_ms | 30000 |
      | max_output | 1048576 |
      | retries | 0 |
    Given the capabilities
      | chrome | {"kind":"chrome_cdp","endpoint":"http://127.0.0.1:9222","launch":true,"startup_timeout_ms":20000,"timeout_ms":10000,"max_text":6000,"max_owned_pages":32,"owned_idle_ms":300000} |

  Scenario: open_page
    Given a choice question "proceed" asks "Use the singleton browser for the requested localhost page?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "observe"
    Then call the capability "chrome"
      | with.op | open |
      | with.url | ${state.url} |
      | project.target_id | /target_id |

  Scenario: observe
    Given a choice question "proceed" asks "Continue after taking the element observation?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "type_message"
    Then call the capability "chrome"
      | with.op | snapshot |
      | with.target_id | ${state.target_id} |
      | project.page | /result |
      | project.element_count | /result/elements/length |

  Scenario: type_message
    Given a choice question "proceed" asks "Continue with real CDP keystrokes?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "show_badges"
    Then call the capability "chrome"
      | with.op | type |
      | with.target_id | ${state.target_id} |
      | with.selector | #message |
      | with.text | ${state.text} |
      | project.typed | /typed |

  Scenario: show_badges
    Given a choice question "done" asks "Is the browser workflow complete after the observation overlay?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "chrome"
      | with.op | highlight |
      | with.target_id | ${state.target_id} |
      | with.enabled | true |
      | project.badges | /shown |
