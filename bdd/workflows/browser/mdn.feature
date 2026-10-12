@decision
Feature: mdn_reader
  Port of mdn: converted from dsl/browser/mdn.json (BDD round-trip).

  Background:
    Given the workflow runs at most 2 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts | "127.0.0.1", "developer.mozilla.org" |
      | allow_paths | "${env.HOME}/.laya-workflow", "${env.HOME}/tmp/mdn" |
      | max_timeout_ms | 60000 |
      | max_output | 8388608 |
      | retries | 0 |
    Given the capabilities
      | chrome | {"kind":"chrome_cdp","endpoint":"http://127.0.0.1:9222","launch":true,"startup_timeout_ms":20000,"timeout_ms":30000,"max_text":8000} |
      | mdn | {"kind":"plugin","plugin":"websites/mdn","browser":"chrome","timeout_ms":30000} |

  Scenario: read
    Given a choice question "done" asks "Read MDN as requested?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "mdn"
      | with.query | ${state.query} |
      | with.mode | ${state.mode} |
      | with.lang | ${state.lang} |
      | with.count | ${state.count} |
      | with.out_dir | ${state.out_dir} |
      | project.mode | /mode |
      | project.query | /query |
      | project.path | /path |
      | project.locale | /locale |
      | project.count | /count |
      | project.url | /url |
      | project.results | /results |
      | project.written | /written |
      | project.out_dir | /out_dir |
      | project.page_title | /page_title |
      | project.fetched_at | /fetched_at |
