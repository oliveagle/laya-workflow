@decision
Feature: loan_dialogue
  Deterministic loan-application slot-filling dialogue and underwriting as a
  multi-node DFA. The model only maps one applicant utterance to typed facts
  (annual_income / monthly_debt / loan_amount / term_months); collection order,
  numeric validation, exact Decimal amortization, the debt-to-income and
  payment-to-income ratios, and the final underwriting verdict are all computed
  locally. Port of code/qwen_vm/app_scenarios/loan_dialogue.
  States: AWAITING_ANNUAL_INCOME -> AWAITING_MONTHLY_DEBT -> AWAITING_LOAN_AMOUNT
  -> AWAITING_TERM_MONTHS -> COMPLETE. Each turn must fill exactly the expected
  slot: a skip-step / multi-field turn, an out-of-bounds or malformed value, and
  a turn after completion are all rejected locally. The exact Decimal arithmetic
  and the ratio thresholds (debt-to-income > 43.00, payment-to-income > 28.00;
  loan_amount bounds are folded into the slot choices) are folded into the
  terminal COMPLETE node's outcome choice; the verdict is emitted as the
  engine's APPROVE / REJECT label.

  Background:
    Given the workflow runs at most 12 iterations

  Scenario: awaiting_income
    Given a choice question "turn" asks "AWAITING_ANNUAL_INCOME: classify the applicant's turn relative to the expected slot annual_income (0 < value <= 100000000)."
      | answer     | meaning                                                      |
      | fill_ok    | the turn fills annual_income with a value inside the bounds   |
      | invalid    | annual_income present but <= 0 or > 100000000                 |
      | unexpected | skip-step or multi-field: the turn fills any other field      |
    And the answer comes from the field "ev_awaiting_income"
    When the answer "fill_ok" goes to "awaiting_debt"
    And any other answer goes to "STOP"
    Then label "annual_income" when the answer to "turn" is "fill_ok"
    And label "reject_value" when the answer to "turn" is "invalid"
    And label "reject_skip" otherwise
    And keep the state key "application_id"

  Scenario: awaiting_debt
    Given a choice question "turn" asks "AWAITING_MONTHLY_DEBT: classify the applicant's turn relative to the expected slot monthly_debt (0 <= value <= 10000000)."
      | answer     | meaning                                                      |
      | fill_ok    | the turn fills monthly_debt with a value inside the bounds    |
      | invalid    | monthly_debt present but < 0 or > 10000000                    |
      | unexpected | skip-step or multi-field: the turn fills any other field      |
    And the answer comes from the field "ev_awaiting_debt"
    When the answer "fill_ok" goes to "awaiting_amount"
    And any other answer goes to "STOP"
    Then label "monthly_debt" when the answer to "turn" is "fill_ok"
    And label "reject_value" when the answer to "turn" is "invalid"
    And label "reject_skip" otherwise
    And keep the state key "application_id"

  Scenario: awaiting_amount
    Given a choice question "turn" asks "AWAITING_LOAN_AMOUNT: classify the applicant's turn relative to the expected slot loan_amount (0 < value <= 10000000)."
      | answer     | meaning                                                      |
      | fill_ok    | the turn fills loan_amount with a value inside the bounds     |
      | invalid    | loan_amount present but <= 0 or > 10000000                    |
      | unexpected | skip-step or multi-field: the turn fills any other field      |
    And the answer comes from the field "ev_awaiting_amount"
    When the answer "fill_ok" goes to "awaiting_term"
    And any other answer goes to "STOP"
    Then label "loan_amount" when the answer to "turn" is "fill_ok"
    And label "reject_value" when the answer to "turn" is "invalid"
    And label "reject_skip" otherwise
    And keep the state key "application_id"

  Scenario: awaiting_term
    Given a choice question "turn" asks "AWAITING_TERM_MONTHS: classify the applicant's turn relative to the expected slot term_months (one of 12, 24, 36, 48, 60, 72, 84)."
      | answer     | meaning                                                              |
      | fill_ok    | the turn fills term_months with one of the allowed terms              |
      | invalid    | term_months present but not one of 12/24/36/48/60/72/84                |
      | unexpected | skip-step or multi-field: the turn fills any other field              |
    And the answer comes from the field "ev_awaiting_term"
    When the answer "fill_ok" goes to "complete"
    And any other answer goes to "STOP"
    Then label "term_months" when the answer to "turn" is "fill_ok"
    And label "reject_value" when the answer to "turn" is "invalid"
    And label "reject_skip" otherwise
    And keep the state key "application_id"

  Scenario: complete
    Given a choice question "outcome" asks "COMPLETE (terminal, underwriting): all four facts are collected. The exact Decimal amortization and the ratio thresholds (debt-to-income > 43.00 or payment-to-income > 28.00 reject) are computed locally; a further applicant turn after completion is a duplicate."
      | answer         | meaning                                                |
      | approve        | both ratios within limits -> APPROVE                    |
      | reject_dti     | debt-to-income ratio exceeds 43.00                      |
      | reject_payment | payment-to-income ratio exceeds 28.00                   |
      | reject_both    | both ratios exceed their limits                         |
      | duplicate      | another applicant turn arrived after completion         |
    And the answer comes from the field "ev_outcome"
    When any other answer goes to "STOP"
    Then label "reject_duplicate" when the answer to "outcome" is "duplicate"
    And label "REJECT" when the answer to "outcome" is "reject_dti"
    And label "REJECT" when the answer to "outcome" is "reject_payment"
    And label "REJECT" when the answer to "outcome" is "reject_both"
    And label "APPROVE" otherwise
    And keep the state key "application_id"
