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
    And answer by all-group rules on the text with hit 0.9 and miss 0.02 and fallback "other" at 0.9
      | label | all of |
      | technical | "outage", "500", "hotfix", "incident", "prod", "unavailable" |
      | billing | "refund", "invoice", "billed", "payment", "duplicate charge" |
      | sales | "pricing", "demo", "purchase", "quote", "trial" |
      | shipping | "package", "parcel", "delivery", "deliver", "courier", "shipment", "shipping", "arrived", "arrive", "customs", "tracking" |
      | account | "login", "password", "access", "profile", "reset" |
      | hr | "leave", "payroll", "hiring", "vacation", "pto" |
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
    And score "severity" by rules on the text miss 0.5
      | score | needles |
      | 2.0 | "vermin", "watch your back", "get rich", "123-45-6789" |
    When any other answer goes to "STOP"
    Then label "PAGE_ONCALL" when "severity" is at least 1.5
    And label "QUEUE_TODAY" when "severity" is at least 0.5
    And label "BACKLOG" otherwise
