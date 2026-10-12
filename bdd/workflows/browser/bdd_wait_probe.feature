@decision
Feature: bdd_wait_probe
  Port of bdd_wait_probe: converted from dsl/browser/bdd_wait_probe.json (BDD round-trip).

  Background:
    Given the workflow runs at most 9 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts | "127.0.0.1", "localhost" |
      | max_timeout_ms | 120000 |
      | max_output | 1048576 |
      | retries | 0 |
    Given the capabilities
      | chrome | {"kind":"chrome_cdp","endpoint":"http://127.0.0.1:${state.cdp_port}","profile_dir":"${state.cdp_profile}","launch":true,"startup_timeout_ms":30000,"timeout_ms":15000,"max_text":8000,"max_owned_pages":32,"owned_idle_ms":300000,"chrome_binary":"scripts/bdd/chrome-headless.sh"} |
      | bdd | {"kind":"plugin","plugin":"bdd","browser":"chrome","timeout_ms":60000} |

  Scenario: open
    Given a choice question "ok" asks "Open ${state.url} in a real Chrome tab and wait until it is actually loaded?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "wait_forever"
    Then call the capability "chrome"
      | with.op | open |
      | with.url | ${state.url} |
      | project.target_id | /target_id |
    Then keep the state key "url"
    Then keep the state key "target_id"

  Scenario: wait_forever
    Given a choice question "ok" asks "Did the element turn up?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "STOP"
    Then call the capability "bdd"
      | with.op | wait_for |
      | with.target_id | ${state.target_id} |
      | with.selector | #never-going-to-appear |
      | with.timeout_ms | 600 |
      | project.target_id | /target_id |
    Then keep the state key "url"
    Then keep the state key "target_id"
