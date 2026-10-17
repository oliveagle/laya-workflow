@decision
Feature: aml_screener

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: screen
    Given a choice question "sanctioned_country" asks "Is origin or destination country in the sanctioned set (CU, IR, KP, SY)?"
      | answer | meaning              |
      | A      | no sanctioned country          |
      | B      | yes, sanctioned country        |
    And answer "B" when the text matches "(?i)(?:\bcuba\b|\biran\b|north korea|\bsyria\b| country_cu|\bcountry_ir\b|\bcountry_kp\b|\bcountry_sy\b|\"CU\"|\"IR\"|\"KP\"|\"SY\")" with hit 0.95 and miss 0.05
    And a choice question "crypto_category" asks "Is the transaction category crypto?"
      | answer | meaning              |
      | A      | not crypto                     |
      | B      | yes, crypto category           |
    And answer "B" when the text contains "crypto", "bitcoin", "ethereum" with hit 0.9 and miss 0.1
    And a choice question "crypto_exchange_counterparty" asks "Is the counterparty type a crypto exchange?"
      | answer | meaning              |
      | A      | not crypto exchange            |
      | B      | yes, crypto exchange           |
    And answer "B" when the text contains "crypto_exchange", "exchange" with hit 0.9 and miss 0.1
    And a choice question "high_risk_country" asks "Is origin or destination country in the high-risk set (BY, MM, NG, RU + sanctioned)?"
      | answer | meaning              |
      | A      | no high-risk country           |
      | B      | yes, high-risk country         |
    And answer "B" when the text contains "belarus", "myanmar", "nigeria", "russia", "country_by", "country_mm", "country_ng", "country_ru" with hit 0.9 and miss 0.1
    And a score question "aml_risk" asks "Composite AML risk 0-100 (sum rule points for crypto +30, large_amount +25, high_risk_country +35, crypto_exchange +20, near_threshold +15, charity_high_risk +20)."
      | level          |
      | 0 clear                        |
      | 25 reviewable                  |
      | 70 block                       |
      | 100 maximum                    |
    And score "aml_risk" by summing rules on the text capped at 100
      | points | needles |
      | 30 | "crypto", "bitcoin", "ethereum" |
      | 25 | "large_amount", "large amount", "above threshold" |
      | 35 | "high_risk", "high risk", "belarus", "russia", "country_by", "country_ru" |
      | 20 | "crypto_exchange", "exchange" |
      | 15 | "near_threshold", "near threshold" |
      | 20 | "charity_high_risk", "charity" |
      | 100 | "sanctioned", "country_cu", "country_ir", "country_kp", "country_sy" |
    When any other answer goes to "STOP"
    Then label "BLOCK" when the answer to "sanctioned_country" is "B"
    And label "BLOCK" when "aml_risk" is at least 70
    And label "REVIEW" when "aml_risk" is at least 25
    And label "CLEAR" otherwise
    Then the primary question is "aml_risk"

