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
    And a choice question "behaviour" asks "Which behaviour pattern dominates this event?"
      | answer | meaning              |
      | normal_usage | routine use                    |
      | repeated_failures | many prior failures            |
      | micro_transaction_burst | burst of tiny charges          |
      | new_device_payment | new device making a payment    |
      | recovery_then_withdrawal | recovery followed by withdrawal |
      | inconsistent_session | session signals disagree       |
    And a choice question "mfa_failed_pending" asks "Did MFA fail while a challenge was already pending?"
      | answer | meaning              |
      | A      | no                             |
      | B      | yes                            |
    And a score question "composite_risk" asks "Composite risk score 0-100 (intent + behaviour + velocity + amount + new device)."
      | level          |
      | 0 clean                        |
      | 40 reviewable                  |
      | 70 deny                        |
      | 100 maximum                    |
    When any other answer goes to "STOP"
    Then label "DENY" when the answer to "intent" is one of "credential_stuffing", "card_testing", "account_takeover"
    Then label "DENY" when the answer to "mfa_failed_pending" is "B"
    And label "DENY" when "composite_risk" is at least 70
    And label "REVIEW" when "composite_risk" is at least 40
    And label "CHALLENGE" when "composite_risk" is at least 20
    And label "ALLOW" otherwise
    Then the primary question is "composite_risk"
    And keep the state key "request_id"
    And keep the state key "actor_id"

