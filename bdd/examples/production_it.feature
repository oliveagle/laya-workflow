# Production integration tests: executed with
#   python3 scripts/bdd/build.py --profile production --base-url https://app.example.com
#   python3 scripts/bdd/run.py --profile production --base-url https://app.example.com
#
# `@production` is the tag that selects a scenario for the production profile.
# `--profile production` refuses to run without an external --base-url, never
# touches the local fixture server, and fails the build if NO scenario is tagged
# `@production` (a typo'd tag must not silently ship zero integration tests).
Feature: Production integration smoke
  A smoke suite against a real deployment. Steps use the standard vocabulary
  (scripts/bdd/steps.py) so the build's 100% coverage bar holds; `<base_url>`
  is supplied at run time from --base-url / $BDD_BASE_URL.

  @production
  Scenario: the landing page loads and renders
    Given I am on "<base_url>/"
    Then the page title contains "Laya"
    Then the element "#app" is visible

  @production
  @smoke
  Scenario: a health route responds
    Given I am on "<base_url>/"
    When I navigate to "<base_url>/health"
    Then the page url contains "/health"
    Then javascript "document.body.textContent" contains "ok"
