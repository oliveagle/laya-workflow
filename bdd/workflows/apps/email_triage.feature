@decision
Feature: email_triage
  Email triage: read an email on six questions and route it to one action.
  Behaviour-equivalent to the built-in `email_triage` app's route(): QUARANTINE a
  phishing attempt, TRASH spam, page the owning team when a blocking billing or
  technical issue needs a human, reply today when a reply is needed on a blocking
  issue, queue the reply when a reply is needed at all, else FYI only. The two
  "blocking issue for team X" branches are a conjunction of the category answer
  and the urgency score, written as `label ... when <cond> and <cond>`.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: triage
    Given a choice question "category" asks "Which team should handle this email?"
      | answer | meaning |
      | billing | invoices, payments, refunds |
      | technical | bugs, outages, integrations |
      | sales | pricing, demos, new purchases |
      | account | login, access, profile changes |
      | hr | hiring, leave, payroll |
      | other | none of the above |
    And answer by all-group rules on the text with hit 0.9 and miss 0.02 and fallback "other" at 0.9
      | label      | all of |
      | technical  | "outage", "500", "hotfix", "incident", "prod", "unavailable" |
      | billing    | "refund", "invoice", "billed", "payment", "duplicate charge" |
      | sales      | "pricing", "demo", "purchase", "quote", "trial" |
      | shipping   | "package", "parcel", "delivery", "deliver", "courier", "shipment", "shipping", "arrived", "arrive", "customs", "tracking" |
      | account    | "login", "password", "access", "profile", "reset" |
      | hr         | "leave", "payroll", "hiring", "vacation", "pto" |
    And a choice question "spam" asks "Is this email unsolicited bulk marketing or spam?"
      | answer | meaning |
      | A | no, it is a legitimate one-to-one or business email |
      | B | yes, it is spam, bulk marketing, or unsolicited promo |
    And answer "B" when the text contains "get rich", "limited offer", "buy now", "5000/week" with hit 0.9 and miss 0.1
    And a choice question "phishing" asks "Is this email a phishing or scam attempt to steal money, credentials, or personal data?"
      | answer | meaning |
      | A | no, it is a legitimate business email |
      | B | yes, it tries to steal credentials, money, or personal data |
    And answer "B" when the text contains "confirm credentials", "suspended", "verify your account", "http://evil" with hit 0.9 and miss 0.05
    And a score question "urgency" asks "How urgent is the issue?"
      | level |
      | no time pressure |
      | needs attention soon |
      | blocking issue or hard deadline |
    And score "urgency" by rules on the text miss 0.8
      | score | needles |
      | 2.0   | "outage", "refund today", "before sprint end" |
    And a choice question "needs_reply" asks "Does the sender expect a reply or follow-up action?"
      | answer | meaning |
      | A | no, FYI only or auto-notification |
      | B | yes, the sender needs a response |
    And answer "B" when the text contains "please", "need", "refund" with hit 0.7 and miss 0.2
    And a score question "sentiment" asks "Sender tone?"
      | level |
      | angry or very negative |
      | negative |
      | neutral |
      | positive |
    And score "sentiment" by rules on the text miss 2.0
      | score | needles |
      | 0.3   | "unacceptable", "angry" |
    When any other answer goes to "STOP"
    Then the primary question is "category"
    And the action reads the question "category"
    And label "QUARANTINE" when the answer to "phishing" is "B" with at least 0.5 confidence
    And label "TRASH" when the answer to "spam" is "B" with at least 0.5 confidence
    And label "PAGER_BILLING" when the answer to "category" is "billing" and "urgency" is at least 1.5
    And label "PAGER_ONCALL" when the answer to "category" is "technical" and "urgency" is at least 1.5
    And label "REPLY_TODAY" when the answer to "needs_reply" is "B" with at least 0.5 confidence and "urgency" is at least 1.5
    And label "REPLY_QUEUE" when the answer to "needs_reply" is "B" with at least 0.5 confidence
    And label "FYI_ONLY" otherwise
