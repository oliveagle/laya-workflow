# Executed by: laya-workflow bdd run
# State required: base_url  (the local fixture server's root, e.g. http://127.0.0.1:8799)
Feature: Page smoke
  The smallest useful BDD scenario: get a real tab open on a real URL, then
  check the page's observable state. Every assertion is deterministic and
  fails the run when it does not hold; nothing here is judged by a model.

  Background:
    Given the browser is ready

  Scenario: A page that loads is assertable
    Given I am on "<base_url>/index.html"
    Then the page title contains "BDD Fixture"
    Then the page url contains "index.html"
    Then the element "#heading" is visible
    Then javascript "document.readyState" equals "complete"

  # A scenario that MUST fail. If this ever passes, the assertions above are
  # vacuous - they would report success no matter what the page did. The runner
  # treats a pass here as a suite failure, so the checks cannot rot into
  # decoration.
  @expected_failure(bdd.assert: FAIL visible)
  Scenario: Asserting a missing element fails the run
    Given I am on "<base_url>/index.html"
    Then the element "#definitely-not-here" is visible
