@decision
Feature: intake_pipeline_nested

  Background:
    Given the workflow runs at most 30 iterations

  Scenario: prefilter
    Given a choice question "is_noise" asks "Is this message noise that should be dropped before routing?"
      | answer | meaning              |
      | A      | no, it is a real request that needs routing |
      | B      | yes, it is automated noise or a duplicate |
    And answer by rules on the field "text" with hit 0.9 and fallback "A"
      | label | needles |
      | B | "digest", "newsletter", "archived", "automated", "no action needed", "unsubscribe" |
    When the answer "A" goes to "intake"
    And any other answer goes to "STOP"
    Then allow up to 1 retries

  Scenario: intake
    Given the sub workflow "support_ticket_router"

  Scenario: audit
    Given a choice question "needs_human" asks "Given the routing outcome, does a human need to double-check it?"
      | answer | meaning              |
      | A      | no, the automated routing is trustworthy |
      | B      | yes, a human should verify before acting |
    When any other answer goes to "STOP"
    Then label "AUDIT_REQUIRED" when the answer is "B" with at least 0.5 confidence
    And label "AUDIT_SKIPPED" otherwise

