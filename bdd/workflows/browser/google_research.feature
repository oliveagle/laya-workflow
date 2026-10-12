@decision
Feature: google_research_open_extract_score
  Port of google_research: converted from dsl/browser/google_research.json (BDD round-trip).

  Background:
    Given the workflow runs at most 3 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts |  |
      | max_timeout_ms | 30000 |
      | max_output | 4194304 |
      | retries | 0 |
    Given the capabilities
      | chrome | {"kind":"chrome_cdp","endpoint":"http://127.0.0.1:9222","launch":true,"startup_timeout_ms":20000,"timeout_ms":15000,"max_text":5000} |

  Scenario: research
    Given a choice question "done" asks "Run background Google research and collect ten meaningful pages?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "chrome"
      | with.op | research |
      | with.engine | google |
      | with.query | ${state.query} |
      | with.timeout_ms | 30000 |
      | with.result_count | ${state.result_count} |
      | with.max_open | ${state.max_open} |
      | with.tab_concurrency | ${state.tab_concurrency} |
      | with.skip_video | ${state.skip_video} |
      | with.skip_kinds | ${state.skip_kinds} |
      | with.min_score | 0.5 |
      | with.max_text | ${state.max_text} |
      | project.query | /query |
      | project.search_candidates | /search_candidates |
      | project.ads_found | /ads_found |
      | project.ads | /ads |
      | project.skipped_found | /skipped_found |
      | project.skipped | /skipped |
      | project.scored_pages | /scored_pages |
      | project.accepted_pages | /accepted_pages |
      | project.collected_count | /collected_count |
      | project.failures | /failures |
      | project.complete | /complete |
      | project.cleanup | /cleanup |
