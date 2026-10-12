@decision
Feature: crates_reader
  Port of crates: converted from dsl/browser/crates.json (BDD round-trip).

  Background:
    Given the workflow runs at most 2 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts | "127.0.0.1", "crates.io", "static.crates.io", "docs.rs", "raw.githubusercontent.com", "user-images.githubusercontent.com" |
      | allow_paths | "${env.HOME}/.laya-workflow", "${env.HOME}/tmp/crates" |
      | max_timeout_ms | 60000 |
      | max_output | 4194304 |
      | retries | 0 |
    Given the capabilities
      | chrome | {"kind":"chrome_cdp","endpoint":"http://127.0.0.1:9222","launch":true,"startup_timeout_ms":20000,"timeout_ms":20000,"max_text":6000} |
      | crates | {"kind":"plugin","plugin":"websites/crates","browser":"chrome","timeout_ms":30000} |

  Scenario: read
    Given a choice question "done" asks "Read the site as requested?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "crates"
      | with.query | ${state.query} |
      | with.mode | ${state.mode} |
      | with.count | ${state.count} |
      | project.mode | /mode |
      | project.query | /query |
      | project.count | /count |
      | project.url | /url |
      | project.page_title | /page_title |
      | project.results | /results |
      | project.written | /written |
      | project.out_dir | /out_dir |
      | project.fetched_at | /fetched_at |
      | project.name | /name |
      | project.source | /source |
