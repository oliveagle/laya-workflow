@decision
Feature: web_research_tinyfish

  Background:
    Given the workflow runs at most 10 iterations

  Scenario: route
    Given a choice question "mode" asks "Research the web how?"
      | answer | meaning              |
      | search | search a query, then read the top hit through TinyFish |
      | read   | fetch one known URL through TinyFish |
    When the answer "search" goes to "do_search"
    And the answer "read" goes to "do_read"
    And any other answer goes to "STOP"
    Then keep the answer to "mode"
    And allow up to 1 retries

  Scenario: do_search
    Given a choice question "use" asks "Run the live search and read the top result?"
      | answer | meaning              |
      | yes    | query TinyFish with ${state.query} |
    When the answer "yes" goes to "read_top"
    And any other answer goes to "STOP"
    Then call the capability "search"
      | with.query      | ${state.query} |
      | project.search_status  | /status |
      | project.result_count  | /count |
      | project.top_title  | /results/0/title |
      | project.top_url  | /results/0/url |
      | project.top_snippet  | /results/0/snippet |
    And allow up to 1 retries

  Scenario: read_top
    Given a choice question "read" asks "Read ${state.top_url} through TinyFish?"
      | answer | meaning              |
      | yes    | extract the page text          |
    When the answer "yes" goes to "STOP"
    And any other answer goes to "STOP"
    Then call the capability "read_page"
      | with.url      | ${state.top_url} |
      | project.page_status  | /status |
      | project.page_title  | /title |
      | project.page_chars  | /chars |
      | project.page_truncated  | /truncated |
      | project.page_text  | /text |
    And allow up to 1 retries

  Scenario: do_read
    Given a choice question "read" asks "Fetch ${state.url} through TinyFish?"
      | answer | meaning              |
      | yes    | extract the page text          |
    When the answer "yes" goes to "STOP"
    And any other answer goes to "STOP"
    Then call the capability "read_page"
      | with.url      | ${state.url} |
      | project.status  | /status |
      | project.title  | /title |
      | project.chars  | /chars |
      | project.truncated  | /truncated |
      | project.text  | /text |
    And allow up to 1 retries

