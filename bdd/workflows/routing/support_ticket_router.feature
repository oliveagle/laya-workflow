@decision
Feature: support_ticket_router
  Multi-node routing: classify (choice) -> escalate (score threshold) or STOP.
  Demonstrates data-driven edges and ordered threshold rules.

  Background:
    Given the workflow runs at most 20 iterations
  Scenario: classify
    Given a choice question "category" asks "Which team should handle this ticket?"
      | answer    | meaning                      |
      | billing   | invoices, payments, refunds  |
      | technical | bugs, outages, system errors |
      | sales     | pricing, new contracts       |
      | other     | everything else              |
    When the answer "technical" goes to "severity_probe"
    And the answer "billing" goes to "severity_probe"
    And the answer "sales" goes to "STOP"
    And the answer "other" goes to "STOP"
    And any other answer goes to "STOP"
    Then keep the answer to "category"
    And require confidence at least 0.4
    And allow up to 3 retries

  Scenario: severity_probe
    Given a score question "severity" asks "How severe is the impact?"
      | level    |
      | minor    |
      | degraded |
      | outage   |
    When any other answer goes to "STOP"
    Then label "PAGE_ONCALL" when "severity" is at least 1.5
    And label "QUEUE_TODAY" when "severity" is at least 0.5
    And label "BACKLOG" otherwise
