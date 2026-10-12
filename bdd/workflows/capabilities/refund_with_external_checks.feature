@decision
Feature: refund_with_external_checks

  Background:
    Given the workflow runs at most 6 iterations

  Scenario: score_risk
    Given a score question "amount_band" asks "Rough size of the refund request?"
      | level          |
      | tokens / cents                 |
      | single invoice                 |
      | large or recurring             |
    When any other answer goes to "assess"
    Then call the capability "risk_scorer"
      | with.text      | ${state.text} |
      | project.projected_label  | /label |
      | project.projected_risk  | /risk |

  Scenario: assess
    Given a choice question "decision" asks "Based on the external risk label and the amount band, what should happen?"
      | answer | meaning              |
      | approve | low risk and small/medium amount — approve automatically |
      | review | needs a human reviewer         |
      | reject | high risk — decline the refund |
    When any other answer goes to "STOP"
    Then label "REJECTED" when the answer is "reject"
    And label "NEEDS_REVIEW" when the answer is "review"
    And label "APPROVED" otherwise

