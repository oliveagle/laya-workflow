# Scored by: laya-workflow bdd score
# State required: base_url
Feature: Search, export, and capture the markers
  The "turn a no-API UI into a tool" shape end to end: dismiss an optional
  banner, search by key, wait for the app to be ready, export, and read the
  app's own markers back out. The two collected values are the tool's outputs.

  Background:
    Given the browser is ready

  @outputs(exported, page_url)
  Scenario: Export results and record the app's markers
    Given I am on "<base_url>/app.html?clean=1"
    When I click the element "#consent" if it is present
    When I type "ACME" into the element "#search"
    When I press the key "Enter"
    When I wait until the element "#export" becomes enabled
    When I click the element "#export" if it is present
    When I extract the attribute "data-exported" of the element "#app" into exported
    When I extract the page url into page_url
    Then the saved value "exported" equals text "1"
    Then the element "#results .row" is visible
    Then the saved value "page_url" contains text "app.html"
