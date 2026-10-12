@decision
Feature: web_research

  Background:
    Given the workflow runs at most 10 iterations

  Scenario: route
    Given a choice question "mode" asks "What should this request do with the internet?"
      | answer | meaning              |
      | search | find pages for a query and then read the top hit |
      | read   | fetch one known URL and turn it into readable text |
      | markdown | fetch one known URL and keep its heading/link structure |
    When the answer "search" goes to "do_search"
    And the answer "read" goes to "do_read"
    And the answer "markdown" goes to "do_markdown"
    And any other answer goes to "STOP"
    Then keep the answer to "mode"
    And allow up to 1 retries

  Scenario: do_search
    Given a choice question "use" asks "Run the search and keep the first result?"
      | answer | meaning              |
      | yes    | query the search endpoint with ${state.query} |
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
    Given a choice question "read" asks "Read the page the search returned?"
      | answer | meaning              |
      | yes    | fetch ${state.top_url} and extract its text |
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
    Given a choice question "read" asks "Fetch the supplied URL as plain text?"
      | answer | meaning              |
      | yes    | extract readable text from ${state.url} |
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

  Scenario: do_markdown
    Given a choice question "read" asks "Fetch the supplied URL and keep its structure?"
      | answer | meaning              |
      | yes    | convert ${state.url} to markdown |
    When the answer "yes" goes to "STOP"
    And any other answer goes to "STOP"
    Then call the capability "read_markdown"
      | with.url      | ${state.url} |
      | project.status  | /status |
      | project.format  | /format |
      | project.chars  | /chars |
      | project.markdown  | /text |
    And allow up to 1 retries

