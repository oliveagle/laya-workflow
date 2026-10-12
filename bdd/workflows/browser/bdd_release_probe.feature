@decision
Feature: bdd_release_probe
  Port of bdd_release_probe: converted from dsl/browser/bdd_release_probe.json (BDD round-trip).

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
    And the answer "A" goes to "release_once"
    Then call the capability "chrome"
      | with.op | open |
      | with.url | ${state.url} |
      | project.target_id | /target_id |
    Then keep the state key "url"
    Then keep the state key "target_id"

  Scenario: release_once
    Given a choice question "ok" asks "Was the page closed by the first release?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "release_again"
    Then call the capability "bdd"
      | with.op | release |
      | with.target_id | ${state.target_id} |
      | project.target_id | /target_id |
    Then keep the state key "url"
    Then keep the state key "target_id"

  Scenario: release_again
    Given a choice question "ok" asks "Did the second release refuse, naming the target?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "STOP"
    Then call the capability "bdd"
      | with.op | release |
      | with.target_id | ${state.target_id} |
      | project.target_id | /target_id |
    Then keep the state key "url"
    Then keep the state key "target_id"
