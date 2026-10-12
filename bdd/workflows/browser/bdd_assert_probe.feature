@decision
Feature: bdd_assert_probe
  Port of bdd_assert_probe: converted from dsl/browser/bdd_assert_probe.json (BDD round-trip).

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
    And the answer "A" goes to "title"
    Then call the capability "chrome"
      | with.op | open |
      | with.url | ${state.url} |
      | project.target_id | /target_id |
    Then keep the state key "url"
    Then keep the state key "target_id"

  Scenario: title
    Given a choice question "ok" asks "Does the page title contain "BDD Fixture"?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "heading"
    Then call the capability "bdd"
      | with.assertion | title_contains |
      | with.value | BDD Fixture |
      | with.target_id | ${state.target_id} |
      | with.op | assert |
      | project.target_id | /target_id |
    Then keep the state key "url"
    Then keep the state key "target_id"

  Scenario: heading
    Given a choice question "ok" asks "Is #heading rendered?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "count"
    Then call the capability "bdd"
      | with.assertion | visible |
      | with.value | #heading |
      | with.target_id | ${state.target_id} |
      | with.op | assert |
      | project.target_id | /target_id |
    Then keep the state key "url"
    Then keep the state key "target_id"

  Scenario: count
    Given a choice question "ok" asks "Does the fact list hold exactly 3 items?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "absent"
    Then call the capability "bdd"
      | with.assertion | equals |
      | with.value | document.querySelectorAll('#facts li').length |
      | with.expected | 3 |
      | with.target_id | ${state.target_id} |
      | with.op | assert |
      | project.target_id | /target_id |
    Then keep the state key "url"
    Then keep the state key "target_id"

  Scenario: absent
    Given a choice question "ok" asks "Is #definitely-not-here really absent?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "ready"
    Then call the capability "bdd"
      | with.assertion | absent |
      | with.value | #definitely-not-here |
      | with.target_id | ${state.target_id} |
      | with.op | assert |
      | project.target_id | /target_id |
    Then keep the state key "url"
    Then keep the state key "target_id"

  Scenario: ready
    Given a choice question "ok" asks "Has the document finished loading?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "done"
    Then call the capability "bdd"
      | with.assertion | is_true |
      | with.value | document.readyState === 'complete' |
      | with.target_id | ${state.target_id} |
      | with.op | assert |
      | project.target_id | /target_id |
    Then keep the state key "url"
    Then keep the state key "target_id"

  Scenario: done
    Given a choice question "ok" asks "Did every hand-written assertion hold?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    And the answer "A" goes to "STOP"
    Then keep the state key "url"
    Then keep the state key "target_id"
