# Executed by: laya-workflow bdd run
# State required: base_url
Feature: Form interaction
  The action steps drive real Chrome over CDP - real mouse events, real
  keystrokes - and the assertion steps read the DOM back and check it.
  Nothing here is stubbed or simulated.

  Background:
    Given the browser is ready

  Scenario: Typing and selecting then clicking updates the page
    Given I am on "<base_url>/index.html"
    When I wait for the element "#greet"
    When I type "Ada" into the element "#name"
    When I select "blue" in the element "#color"
    When I click the element "#greet"
    When I wait for the element "#echo[data-filled]"
    Then javascript "document.querySelector('#echo').textContent" contains "Hello Ada"
    Then javascript "document.querySelector('#echo').textContent" contains "blue"
    Then javascript "document.querySelector('#name').value" equals "Ada"
    Then javascript "document.querySelector('#color').value" equals "blue"
    Then javascript "document.querySelector('#greeting').textContent" is true
