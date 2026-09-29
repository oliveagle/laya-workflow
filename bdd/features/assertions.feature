# Executed by: scripts/bdd/run.py
# State required: base_url
Feature: Assertion vocabulary
  Every assertion the bdd plugin offers, checked twice against the real
  fixture: once against a value that is actually there, so none of them is
  broken; and once against a value that is not, tagged @expected_failure, so
  none of them is decoration.

  An assertion that cannot fail is not a check. The @expected_failure half of
  this file is the guard: the runner treats a pass there as a suite failure, so
  the vocabulary cannot quietly rot into "everything is true".

  Background:
    Given the browser is ready
    Given I am on "<base_url>/index.html"

  Scenario: Every assertion holds for a value that is actually there
    Then the page title contains "BDD Fixture"
    Then the page url contains "index.html"
    Then the element "#heading" is visible
    Then the element "#definitely-not-here" is absent
    Then javascript "document.readyState === 'complete'" is true
    Then javascript "document.readyState === 'loading'" is false
    Then javascript "document.querySelectorAll('#facts li').length" equals 3
    Then javascript "document.body.textContent" contains "BDD fixture page"
    Then javascript "document.getElementById('greeting').textContent" equals text "Not greeted yet."

  @expected_failure
  Scenario: title_contains must reject a title it does not have
    Then the page title contains "A Title That Is Not There"

  @expected_failure
  Scenario: url_contains must reject a url it does not have
    Then the page url contains "not-in-this-url"

  @expected_failure
  Scenario: visible must reject an element that is not rendered
    Then the element "#definitely-not-here" is visible

  @expected_failure
  Scenario: visible must reject an element that exists but is hidden
    Then the element "#hidden-note" is visible

  @expected_failure
  Scenario: absent must reject an element that is present
    Then the element "#heading" is absent

  @expected_failure
  Scenario: is_true must reject a falsy expression
    Then javascript "document.readyState === 'loading'" is true

  @expected_failure
  Scenario: is_false must reject a truthy expression
    Then javascript "document.readyState === 'complete'" is false

  @expected_failure
  Scenario: equals must reject the wrong value
    Then javascript "document.querySelectorAll('#facts li').length" equals 4

  @expected_failure
  Scenario: equals must not coerce a number into a string
    Then javascript "document.querySelectorAll('#facts li').length" equals "3"

  @expected_failure
  Scenario: equals must not coerce a string into a number
    Then javascript "document.getElementById('heading').textContent" equals 0

  @expected_failure
  Scenario: contains must reject text that is not there
    Then javascript "document.getElementById('greeting').textContent" contains "Greeted"

  @expected_failure
  Scenario: equals_text must reject text that is merely a substring
    Then javascript "document.getElementById('greeting').textContent" equals text "greeted yet"
