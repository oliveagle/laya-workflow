@decision
Feature: alphaxiv_papers
  Port of alphaxiv_paper: converted from dsl/browser/alphaxiv_paper.json (BDD round-trip).

  Background:
    Given the workflow runs at most 2 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts |  |
      | allow_paths | "${env.HOME}/.laya-workflow", "${env.HOME}/tmp/alphaxiv" |
      | max_timeout_ms | 180000 |
      | max_output | 8388608 |
      | retries | 0 |
    Given the capabilities
      | browser | {"kind":"browser","endpoint":"http://127.0.0.1:9222","launch":true,"startup_timeout_ms":20000,"timeout_ms":30000,"max_text":8000} |
      | alphaxiv | {"kind":"plugin","plugin":"websites/alphaxiv","browser":"browser","timeout_ms":30000} |

  Scenario: fetch_papers
    Given a choice question "done" asks "Discover and save the requested alphaXiv papers?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "alphaxiv"
      | with.query | ${state.query} |
      | with.mode | ${state.mode} |
      | with.count | ${state.count} |
      | with.paper_url | ${state.paper_url} |
      | with.out_dir | ${state.out_dir} |
      | with.selector | ${state.selector} |
      | with.lang | ${state.lang} |
      | with.interval | ${state.interval} |
      | with.generate_overview | ${state.generate_overview} |
      | with.overview_wait_ms | ${state.overview_wait_ms} |
      | with.overview_max_papers | ${state.overview_max_papers} |
      | project.mode | /mode |
      | project.discovery | /discovery |
      | project.source_url | /source_url |
      | project.selected_count | /selected_count |
      | project.saved_count | /saved_count |
      | project.failed_count | /failed_count |
      | project.results | /results |
      | project.failures | /failures |
