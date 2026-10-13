@decision
Feature: subscription_churn_retention_guard
  Guard for LLM-proposed subscription retention offers as a single triage node.
  The model proposes one typed offer (request_id, customer_id,
  churn_risk_score, retention_offer {discount_pct, duration_months, tier}); this
  workflow owns the tier-limit, duration-limit, tier-downgrade and low-risk /
  high-discount checks against the customer's authoritative tier. It never
  charges, refunds or issues anything. Port of
  code/qwen_vm/app_scenarios/subscription_churn_retention_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required keys, request/customer id grammars,
    churn_risk_score in [0,1], discount_pct in (0,100], integer
    duration_months >= 1, tier enum, duplicate-key and dangerous-token scan)
    fold into the `proposal` choice.
  - The customer's authoritative tier and its max_discount_pct / max_duration
    are a fixed fixture; the harness resolves them and hands the DSL `ok`/`over`.
  - The parse gate already rejects discount_pct <= 0, so the engine's
    `discount_pct must be positive` violation is provably unreachable and folds
    into `proposal` rather than becoming a label.

  Emitted labels: reject_proposal, escalate_low_risk_high_discount,
  reject_discount, reject_duration, reject_tier_upgrade, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, customer_id, churn_risk_score, retention_offer{discount_pct, duration_months, tier}} object with valid ids, a risk in [0,1], a discount in (0,100] and a valid tier?"
      | answer    | meaning                                                       |
      | accepted  | exactly one structurally valid retention offer                |
      | malformed | parse error, missing key, bad id, out-of-range value or bad tier |
    And the answer comes from the field "ev_proposal"
    And a choice question "escalation" asks "Is a low churn risk (< 0.3) paired with a high discount (> 20%)?"
      | answer                     | meaning                                  |
      | ok                         | not the low-risk / high-discount pairing |
      | low_risk_high_discount     | risk < 0.3 and discount > 20%            |
    And the answer comes from the field "ev_escalation"
    And a choice question "discount" asks "Is the offered discount within the customer's tier maximum?"
      | answer | meaning                    |
      | ok     | discount <= tier max       |
      | over   | discount > tier max        |
    And the answer comes from the field "ev_discount"
    And a choice question "duration" asks "Is the offered duration within the customer's tier maximum?"
      | answer | meaning                    |
      | ok     | duration <= tier max       |
      | over   | duration > tier max        |
    And the answer comes from the field "ev_duration"
    And a choice question "tier" asks "Does the offered tier stay at or below the customer's tier?"
      | answer  | meaning                              |
      | ok      | offer tier <= customer tier          |
      | upgrade | offer tier is above the customer tier |
    And the answer comes from the field "ev_tier"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "escalate_low_risk_high_discount" when the answer to "escalation" is "low_risk_high_discount"
    And label "reject_discount" when the answer to "discount" is "over"
    And label "reject_duration" when the answer to "duration" is "over"
    And label "reject_tier_upgrade" when the answer to "tier" is "upgrade"
    And label "approve" otherwise
    And keep the state key "request_id"
