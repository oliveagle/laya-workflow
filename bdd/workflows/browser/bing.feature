@decision
Feature: bing_search
  Port of bing: converted from dsl/browser/bing.json (BDD round-trip).

  Background:
    Given the workflow runs at most 2 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts | "127.0.0.1", "www.bing.com" |
      | allow_paths |  |
      | max_timeout_ms | 60000 |
      | max_output | 4194304 |
      | retries | 0 |
    Given the capabilities
      | chrome | {"kind":"chrome_cdp","endpoint":"http://127.0.0.1:9222","launch":true,"startup_timeout_ms":20000,"timeout_ms":20000,"max_text":6000} |
      | bing | {"kind":"plugin","plugin":"websites/bing","browser":"chrome","timeout_ms":30000} |

  Scenario: search
    Given a choice question "done" asks "Search Bing as requested?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "bing"
      | with.query | ${state.query} |
      | with.count | ${state.count} |
      | with.mkt | ${state.mkt} |
      | project.mode | /mode |
      | project.query | /query |
      | project.mkt | /mkt |
      | project.count | /count |
      | project.url | /url |
      | project.results | /results |
      | project.page_title | /page_title |
      | project.fetched_at | /fetched_at |
