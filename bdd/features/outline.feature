# Executed by: scripts/bdd/run.py
# State required: base_url, person
# Each Examples row becomes its own spec and its own Chrome run, proving the
# Scenario Outline -> example rows expansion end to end.
Feature: Greeting outline
  Background:
    Given the browser is ready
    Given I am on "<base_url>/index.html"

  Scenario Outline: Greet <person> in <colour>
    When I type "<person>" into the element "#name"
    When I select "<colour>" in the element "#color"
    When I click the element "#greet"
    When I wait for the element "#echo[data-filled]"
    Then javascript "document.querySelector('#echo').textContent" contains "Hello <person>"
    Then javascript "document.querySelector('#echo').textContent" contains "<colour>"

    Examples:
      | person | colour |
      | Ada    | blue   |
      | Grace  | green  |
