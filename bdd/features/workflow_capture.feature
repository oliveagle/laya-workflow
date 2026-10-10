# Executed by: laya-workflow bdd run
# State required: base_url
#
# This file exists to prove the *workflow* half of the vocabulary, the half that
# is not about testing a page. A UI with no API (bdd/fixtures/app.html) is driven
# like a person drives it, and the result is collected into named state keys a
# caller can read back. Three steps here have no testing meaning at all:
#
#   * `extract ... into <key>` runs an expression and files the value under a
#     state key, so the workflow produces an *output* rather than a pass/fail;
#   * `wait until ... becomes <state>` waits for the page to reach a state
#     (an enabled button) that `wait for` - presence only - cannot express;
#   * `click ... if it is present` makes the workflow robust to a page that may
#     or may not show a banner, instead of failing on a missing element.
#
# The `@outputs(...)` tag turns "what this workflow produces" into a checked
# contract: the compiler refuses if a declared output nobody produces.
Feature: Turning a no-API UI into a callable workflow
  A workflow is a sequence of actions that leaves *collected state* behind, not
  a set of assertions that pass. The scenarios below drive the app fixture end
  to end and assert on what was collected, so the extraction path is held to the
  same standard as the assertion path: it has to actually read the page.

  Background:
    Given the browser is ready

  @outputs(page_title, page_url, app_version, first_row)
  Scenario: Search the no-API app and capture its record
    Given I am on "<base_url>/app.html"
    When I extract the page title into page_title
    When I extract the page url into page_url
    When I extract the attribute "data-app" of the element "#app" into app_version
    # The banner is present on this page, so this really clicks; on the ?clean=1
    # page the same step is a no-op - the point of the conditional click.
    When I click the element "#consent" if it is present
    When I type "ACME" into the element "#search"
    When I press the key "Enter"
    # Results render asynchronously; the export button enables only when they
    # have. `wait for` could not say this - it only knows presence.
    When I wait until the element "#export" becomes enabled
    When I click the element "#export" if it is present
    When I extract the text of the element "#results .row" into first_row
    Then the page title contains "Invoice"
    Then the element "#consent" is absent
    # The extracted values are the workflow's outputs; checking them here is what
    # makes `extract ... into` more than a no-op.
    Then the saved value "page_title" contains text "Invoice"
    Then the saved value "app_version" equals text "1.4.0"
    Then the saved value "first_row" contains text "ACME"
    Then javascript "document.querySelector('#app').getAttribute('data-exported')" equals "1"

  Scenario: The same workflow runs when the consent banner is already gone
    Given I am on "<base_url>/app.html?clean=1"
    # No banner here. A plain `I click` would fail; the conditional click does
    # not, which is the whole reason the workflow can be reused across pages.
    When I click the element "#consent" if it is present
    When I type "ACME" into the element "#search"
    When I press the key "Enter"
    When I wait until the element "#export" becomes enabled
    Then the element "#consent" is absent
    Then the element "#results .row" is visible

  # A state assertion that must fail, so `the saved value ... equals text` is a
  # real check and not decoration. This is also what covers the plugin's
  # `_assert_state` mismatch throw (see bdd/args_probes.json).
  @expected_failure(bdd.assert: FAIL state_equals)
  Scenario: A collected output that does not match fails the run
    Given I am on "<base_url>/app.html"
    When I extract the page title into page_title
    Then the saved value "page_title" equals text "definitely-not-the-title"
