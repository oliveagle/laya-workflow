@decision
Feature: adaptive_risk_control

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: risk_score
    Given a choice question "intent" asks "Which user intent does this event match?"
      | answer | meaning              |
      | legitimate_purchase | normal payment / transfer      |
      | legitimate_recovery | account recovery flow          |
      | credential_stuffing | automated credential testing   |
      | card_testing | many micro-charges to validate stolen cards |
      | account_takeover | anomalous session then drain   |
      | unknown | unclear                        |
    And answer by all-group rules on the text with hit 0.9 and miss 0.02 and fallback "legitimate_purchase" at 0.9
      | label | all of |
      | credential_stuffing | "credential_stuffing", "credential stuffing" |
      | card_testing | "card_testing", "card testing" |
      | account_takeover | "account_takeover", "account takeover" |
      | legitimate_recovery | "recovery" |
    And a choice question "behaviour" asks "Which behaviour pattern dominates this event?"
      | answer | meaning              |
      | normal_usage | routine use                    |
      | repeated_failures | many prior failures            |
      | micro_transaction_burst | burst of tiny charges          |
      | new_device_payment | new device making a payment    |
      | recovery_then_withdrawal | recovery followed by withdrawal |
      | inconsistent_session | session signals disagree       |
    And answer by all-group rules on the text with hit 0.9 and miss 0.02 and fallback "normal_usage" at 0.9
      | label | all of |
      | repeated_failures | "repeated_failure", "repeated failure" |
      | micro_transaction_burst | "micro_transaction", "micro transaction", "burst" |
      | new_device_payment | "new_device", "new device" |
      | recovery_then_withdrawal | "recovery_then_withdrawal", "recovery then withdrawal", "withdrawal" |
      | inconsistent_session | "inconsistent", "session signals" |
    And a choice question "mfa_failed_pending" asks "Did MFA fail while a challenge was already pending?"
      | answer | meaning              |
      | A      | no                             |
      | B      | yes                            |
    And answer "B" when the text contains "mfa_failed", "mfa failed", "mfa-failed", "challenge_pending" with hit 0.9 and miss 0.05
    And a score question "composite_risk" asks "Composite risk score 0-100 (intent + behaviour + velocity + amount + new device)."
      | level          |
      | 0 clean                        |
      | 20 challenge                   |
      | 40 reviewable                  |
      | 100 maximum                    |
    And score "composite_risk" by rules on the text miss 10
      | score | needles |
      | 80 | "credential_stuffing", "card_testing", "account_takeover" |
      | 60 | "recovery_then_withdrawal" |
      | 45 | "micro_transaction_burst", "new_device_payment" |
      | 25 | "repeated_failures", "inconsistent_session" |
    When any other answer goes to "STOP"
    Then label "DENY" when the answer to "intent" is one of "credential_stuffing", "card_testing", "account_takeover"
    Then label "DENY" when the answer to "mfa_failed_pending" is "B"
    And label "DENY" when "composite_risk" is at least 70
    And label "REVIEW" when "composite_risk" is at least 40
    And label "CHALLENGE" when "composite_risk" is at least 20
    And label "ALLOW" otherwise
    Then the primary question is "composite_risk"

