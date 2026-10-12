@decision
Feature: hackernews_reader
  Port of hackernews: converted from dsl/browser/hackernews.json (BDD round-trip).

  Background:
    Given the workflow runs at most 2 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts | "127.0.0.1", "news.ycombinator.com", "hn.algolia.com" |
      | allow_paths |  |
      | max_timeout_ms | 60000 |
      | max_output | 4194304 |
      | retries | 0 |
    Given the capabilities
      | chrome | {"kind":"chrome_cdp","endpoint":"http://127.0.0.1:9222","launch":true,"startup_timeout_ms":20000,"timeout_ms":20000,"max_text":6000} |
      | hn | {"kind":"plugin","plugin":"websites/hackernews","browser":"chrome","timeout_ms":30000} |

  Scenario: read
    Given a choice question "done" asks "Read Hacker News as requested?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "hn"
      | with.query | ${state.query} |
      | with.mode | ${state.mode} |
      | with.count | ${state.count} |
      | project.feed | /feed |
      | project.query | /query |
      | project.count | /count |
      | project.url | /url |
      | project.stories | /stories |
      | project.comments | /comments |
      | project.comments_total | /comments_total |
      | project.title | /title |
      | project.fetched_at | /fetched_at |
