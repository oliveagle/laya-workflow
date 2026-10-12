@decision
Feature: aml_screener

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: screen
    Given a choice question "sanctioned_country" asks "Is origin or destination country in the sanctioned set (CU, IR, KP, SY)?"
      | answer | meaning              |
      | A      | no sanctioned country          |
      | B      | yes, sanctioned country        |
    And a choice question "crypto_category" asks "Is the transaction category crypto?"
      | answer | meaning              |
      | A      | not crypto                     |
      | B      | yes, crypto category           |
    And a choice question "crypto_exchange_counterparty" asks "Is the counterparty type a crypto exchange?"
      | answer | meaning              |
      | A      | not crypto exchange            |
      | B      | yes, crypto exchange           |
    And a choice question "high_risk_country" asks "Is origin or destination country in the high-risk set (BY, MM, NG, RU + sanctioned)?"
      | answer | meaning              |
      | A      | no high-risk country           |
      | B      | yes, high-risk country         |
    And a score question "aml_risk" asks "Composite AML risk 0-100 (sum rule points for crypto +30, large_amount +25, high_risk_country +35, crypto_exchange +20, near_threshold +15, charity_high_risk +20)."
      | level          |
      | 0 clear                        |
      | 25 reviewable                  |
      | 70 block                       |
      | 100 maximum                    |
    When any other answer goes to "STOP"
    Then label "BLOCK" when the answer to "sanctioned_country" is "B"
    And label "BLOCK" when "aml_risk" is at least 70
    And label "REVIEW" when "aml_risk" is at least 25
    And label "CLEAR" otherwise
    Then the primary question is "aml_risk"
    And keep the state key "request_id"
    And keep the state key "transaction_id"

