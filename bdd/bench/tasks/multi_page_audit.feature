# Scored by: laya-workflow bdd score
# State required: base_url
Feature: Audit two pages and reconcile them
  A workflow that spans documents: collect a record from the first page, move the
  same tab to a second document, collect from there too, and assert on what was
  collected. Every output is a state key an agent could read back.

  Background:
    Given the browser is ready

  @outputs(page_one_title, page_one_heading, page_two_title)
  Scenario: Collect a record from each page
    Given I am on "<base_url>/index.html"
    When I extract the page title into page_one_title
    When I extract the text of the element "#heading" into page_one_heading
    When I navigate to "<base_url>/second.html"
    When I extract the page title into page_two_title
    When I extract the page url into page_two_url
    Then the element "#heading" is absent
    Then the saved value "page_one_title" contains text "Fixture"
    Then the saved value "page_one_heading" contains text "fixture page"
    Then the saved value "page_two_title" contains text "second page"
