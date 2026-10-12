@decision
Feature: support_routing
  Port of support_routing: converted from dsl/capabilities/support_routing.json (BDD round-trip).

  Background:
    Given the workflow runs at most 3 iterations

  Scenario: classify
    Given a choice question "department" asks "Which team should handle the customer's primary request in message? Choose other if unclear or outside the options."
      | answer | meaning |
      | billing | Invoices, charged amounts, refunds, or payment methods. |
      | technical | Application failures, outages, or integration bugs. |
      | account | Login, account access, or account profile changes. |
      | other | No clear primary request, overlapping requests, or none of the teams fit. |
    And answer by rules on the field "message" with hit 0.94 and fallback "other"
      | label | needles |
      | billing | "invoice", "charged", "refund", "payment method", "credit card", "bill" |
      | technical | "crash", "outage", "export", "integration", "bug", "broken", "error", "500" |
      | account | "login", "log in", "log into", "account", "password", "sign in", "profile", "locked out" |
    And a noul question "explicit_urgency" asks "Does message explicitly describe a time-critical operational impact or deadline? Judge the words, not the department."
      | answer | meaning |
    And answer "B" when the field "message" contains "due today", "deadline", "today", "urgent", "asap", "blocked", "cannot deliver", "before sprint" with hit 0.93 and miss 0.1
    When any other answer goes to "STOP"
    Then combine "department" with signal "explicit_urgency" fallback "other" review "human_review" requiring 0.8 high 0.85 low 0.15
    Then allow up to 1 retries
