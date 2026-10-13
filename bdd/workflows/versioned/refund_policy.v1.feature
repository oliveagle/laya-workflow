@decision
Feature: refund_policy

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: band
    Given a score question "amount_band" asks "Rough size of the refund request?"
      | level          |
      | tokens / cents                 |
      | single invoice                 |
      | large or recurring             |
    And score "amount_band" when the field "text" contains "large" scoring 2.0 and "recurring" scoring 1.5 miss 0.5
    When any other answer goes to "STOP"
    Then label "V1_ESCALATE" when "amount_band" is at least 1.5
    And label "V1_AUTO_APPROVE" otherwise

