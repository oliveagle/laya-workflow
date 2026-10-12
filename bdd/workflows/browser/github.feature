@decision
Feature: github_reader
  Port of github: converted from dsl/browser/github.json (BDD round-trip).

  Background:
    Given the workflow runs at most 2 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts | "127.0.0.1", "github.com", "raw.githubusercontent.com", "user-images.githubusercontent.com", "camo.githubusercontent.com", "github.githubassets.com", "avatars.githubusercontent.com" |
      | allow_paths | "${env.HOME}/.laya-workflow", "${env.HOME}/tmp/github" |
      | max_timeout_ms | 60000 |
      | max_output | 4194304 |
      | retries | 0 |
    Given the capabilities
      | chrome | {"kind":"chrome_cdp","endpoint":"http://127.0.0.1:9222","launch":true,"startup_timeout_ms":20000,"timeout_ms":20000,"max_text":6000} |
      | github | {"kind":"plugin","plugin":"websites/github","browser":"chrome","timeout_ms":30000} |

  Scenario: read
    Given a choice question "done" asks "Read the site as requested?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "github"
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
      | project.repo | /repo |
      | project.since | /since |
      | project.language | /language |
      | project.stars | /stars |
      | project.forks | /forks |
      | project.description | /description |
      | project.source | /source |
