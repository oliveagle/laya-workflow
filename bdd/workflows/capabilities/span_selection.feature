@decision
Feature: span_selection
  Port of span_selection: converted from dsl/capabilities/span_selection.json (BDD round-trip).

  Background:
    Given the workflow runs at most 6 iterations
    Given the capabilities
      | spans | {"kind":"text","op":"spans","pattern":"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\\.[A-Za-z]{2,}"} |
      | pick_candidate | {"kind":"json","op":"pick_by"} |

  Scenario: extract
    Given a choice question "has_text" asks "Is there source text to scan?"
      | answer | meaning |
      | A | no |
      | B | yes |
    And answer "B" when the field "text" contains "@" with hit 0.9
    When any other answer goes to "select"
    Then call the capability "spans"
      | with.text | ${state.text} |
      | project.candidates | /candidates |
      | project.candidate_count | /count |
    Then allow up to 1 retries

  Scenario: select
    Given a choice question "billing_contact" asks "Which candidate email address is explicitly identified by text as the billing contact? Select none when the text does not establish exactly one."
      | answer | meaning |
      | from_state | candidates |
      | id_key | id |
      | value_key | value |
      | none | No candidate is explicitly identified as the billing contact, or the wording is ambiguous. |
    And the answer comes from the field "selected_id"
    When any other answer goes to "pick"
    And the answer "none" goes to "not_found"
    Then keep the answer to "billing_contact"
    Then allow up to 1 retries

  Scenario: pick
    Given a choice question "ok" asks "proceed"
      | answer | meaning |
      | A | yes |
    When any other answer goes to "STOP"
    Then call the capability "pick_candidate"
      | with.array | ${state.candidates} |
      | with.id | ${state.billing_contact} |
      | with.id_key | id |
      | project.selected | /item |
    Then allow up to 1 retries

  Scenario: not_found
    Given a choice question "ok" asks "proceed"
      | answer | meaning |
      | A | yes |
    When any other answer goes to "STOP"
    Then keep the answer to "billing_contact"
    Then allow up to 1 retries
