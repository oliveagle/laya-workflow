@decision
Feature: v2ex_reader
  Port of v2ex: converted from dsl/browser/v2ex.json (BDD round-trip).

  Background:
    Given the workflow runs at most 2 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts | "127.0.0.1", "www.v2ex.com" |
      | allow_paths |  |
      | max_timeout_ms | 60000 |
      | max_output | 4194304 |
      | retries | 0 |
    Given the capabilities
      | chrome | {"kind":"chrome_cdp","endpoint":"http://127.0.0.1:9222","launch":true,"startup_timeout_ms":20000,"timeout_ms":20000,"max_text":6000} |
      | v2ex | {"kind":"plugin","plugin":"websites/v2ex","browser":"chrome","timeout_ms":30000} |

  Scenario: read
    Given a choice question "done" asks "Read V2EX as requested?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "v2ex"
      | with.query | ${state.query} |
      | with.mode | ${state.mode} |
      | with.count | ${state.count} |
      | project.mode | /mode |
      | project.feed | /feed |
      | project.query | /query |
      | project.node | /node |
      | project.count | /count |
      | project.url | /url |
      | project.results | /results |
      | project.title | /title |
      | project.replies | /replies |
      | project.content | /content |
      | project.fetched_at | /fetched_at |
