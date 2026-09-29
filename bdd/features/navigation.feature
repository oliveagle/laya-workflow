# Executed by: scripts/bdd/run.py
# State required: base_url
#
# `navigate` and `evaluate` are the two plugin ops the rest of the suite never
# used, which is how `I run javascript` sat in the step table compiling to an
# empty expression for a whole release cycle. An op nobody executes is not a
# feature; this file is what makes them one.
Feature: Navigation and script steps
  Background:
    include: setup/fixture-page.feature

  Scenario: Navigating moves the same tab to a different document
    When I navigate to "<base_url>/second.html"
    Then the page title contains "second page"
    Then the page url contains "second.html"
    Then the element "#second-marker" is visible
    # The old document must really be gone. A navigate that quietly opened a
    # second tab, or returned before the commit, would still satisfy the three
    # assertions above; this is the one that would catch it.
    Then the element "#heading" is absent

  Scenario: A script step changes the page and the change is readable after
    When I run javascript "document.getElementById('heading').textContent = 'set by script'; true"
    Then javascript "document.getElementById('heading').textContent" equals "set by script"
    Then the element "#heading" is visible
