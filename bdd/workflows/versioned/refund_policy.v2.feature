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
    And a choice question "is_suspicious" asks "Does the request look fraudulent or scripted?"
      | answer | meaning              |
      | A      | no, it looks like a genuine customer request |
      | B      | yes, it looks fraudulent, scripted, or otherwise suspicious |
    And answer "B" when the text contains "fraud", "scripted", "bulk", "urgent" with hit 0.95
    When any other answer goes to "STOP"
    Then label "V2_MANUAL_REVIEW" when the answer to "is_suspicious" is "B" with at least 0.5 confidence
    And label "V2_ESCALATE" when "amount_band" is at least 1.5
    And label "V2_AUTO_APPROVE" otherwise

