@decision
Feature: triage_then_moderate
  Port of triage_then_moderate: converted from dsl/routing/triage_then_moderate.json (BDD round-trip).

  Background:
    Given the workflow runs at most 30 iterations

  Scenario: triage
    Given a choice question "queue" asks "Which downstream queue should this item go through first?"
      | answer | meaning |
      | safety_review | content that may violate policy |
      | billing | money-related requests |
      | support | product or technical questions |
      | drop | low value, no action |
    And the answer comes from the field "queue"
    When any other answer goes to "STOP"
    And the answer "safety_review" goes to "safety_gate"
    And the answer "billing" goes to "billing_gate"
    Then allow up to 1 retries

  Scenario: safety_gate
    Given a choice question "is_unsafe" asks "Is this content unsafe to publish or forward as-is?"
      | answer | meaning |
      | A | no, it is safe to forward |
      | B | yes, it should not be forwarded |
    When any other answer goes to "STOP"
    Then gate "is_unsafe" on option "B" with block 0.6 allow 0.7

  Scenario: billing_gate
    Given a choice question "is_refund" asks "Is the user asking for money back?"
      | answer | meaning |
      | A | no, it is informational |
      | B | yes, it requests a refund or credit |
    And the answer comes from the field "is_refund"
    When any other answer goes to "STOP"
    And the answer "B" goes to "refund_amount"
    Then keep the answer to "is_refund"

  Scenario: refund_amount
    Given a score question "amount_band" asks "Rough size of the refund request?"
      | level |
      | tokens / cents |
      | single invoice |
      | large or recurring |
    When any other answer goes to "STOP"
    Then label "ESCALATE_MANAGER" when "amount_band" is at least 2.0
    And label "AUTO_APPROVE" when "amount_band" is at least 1.0
    And label "AUTO_APPROVE_SMALL" otherwise
