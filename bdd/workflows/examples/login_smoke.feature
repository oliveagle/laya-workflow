# User login smoke test — demonstrates the unified BDD pipeline.
#
#   local:      laya-workflow bdd build bdd/examples/login_smoke.feature
#   prod IT:    laya-workflow bdd build bdd/examples/login_smoke.feature \
#                 --profile production --base-url https://app.example.com
#   with assist:laya-workflow bdd build bdd/examples/login_smoke.feature --assist
#
# All steps use the standard vocabulary (laya-workflow bdd vocabulary-check), so the build's
# 100% coverage gate holds. <base_url> is supplied at run time: the local
# profile injects the fixture server URL, the production profile injects
# --base-url. Tags: @smoke = always run, @production = prod IT only.
Feature: User login smoke
  End-to-end smoke test for the user login flow. The @smoke scenario runs in
  every build; the @production scenario is selected by `--profile production`,
  running against a real deployment instead of the local fixture server.

  @smoke
  Scenario: successful login with valid credentials
    Given I am on "<base_url>/login"
    Then the page title contains "Login"
    When I type "alice@example.com" into the element "#email"
    And I type "secret123" into the element "#password"
    And I click the element "#login-button"
    Then the page url contains "/dashboard"
    And the element "#welcome-message" is visible
    And javascript "document.querySelector('#username').textContent" contains "Alice"

  @production
  Scenario: login form renders correctly on production
    Given I am on "<base_url>/login"
    Then the page title contains "Login"
    And the element "#login-form" is visible
    And the element "#email" is visible
    And the element "#password" is visible
    And the element "#login-button" is visible
