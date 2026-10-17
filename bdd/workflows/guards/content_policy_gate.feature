@decision
Feature: content_policy_gate
  Moderation gate: noul + choice + score signals combined; BLOCK anything that
  trips a threat/hate/PII/spam probability, else WARN on severity or a non-safe
  category, else APPROVE. Behaviour-equivalent to the built-in
  `content_moderation` app's route(): the probability rules are ordered *before*
  the severity/category rules so a high threat/hate/PII/spam probability blocks
  even when the best single label reads safe, and the WARN branch matches the
  app's `severity >= 1.5 || category != "safe"`.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: moderate
    Given a choice question "is_hate" asks "Does the text contain slurs, dehumanising language, or hate speech targeting a group?"
      | answer | meaning              |
      | A      | no, it is neutral or positive  |
      | B      | yes, it dehumanises or attacks an identity group |
    And answer "B" when the text contains "vermin", "deport", "slur" with hit 0.9 and miss 0.05
    And a choice question "is_threat" asks "Does the text contain a threat of violence or actionable harm against a person?"
      | answer | meaning              |
      | A      | no, it is safe or merely critical |
      | B      | yes, it threatens violence or harm |
    And answer "B" when the text contains "find you", "watch your back", "kill you" with hit 0.9 and miss 0.05
    And a choice question "is_pii" asks "Does the text contain personally identifiable information?"
      | answer | meaning              |
      | A      | no, it does not expose personal data |
      | B      | yes, it leaks full SSN / card / home address |
    And answer "B" when the text contains "ssn", "123-45-6789", "credit card" with hit 0.9 and miss 0.05
    And a choice question "is_spam" asks "Is the text unsolicited commercial or promotional bulk content?"
      | answer | meaning              |
      | A      | no, it is a normal message     |
      | B      | yes, it is spam or unsolicited promo |
    And answer "B" when the text contains "get rich", "limited offer", "buy now", "5000/week" with hit 0.9 and miss 0.1
    And a score question "severity" asks "Overall severity?"
      | level          |
      | safe to publish                |
      | needs light edit               |
      | must be blocked                |
    And score "2.0" when the text contains "vermin", "watch your back", "get rich", "123-45-6789" scoring 2.0 miss 0.5
    And a choice question "category" asks "Best single label?"
      | answer | meaning              |
      | safe   | no concerns                    |
      | hateful | hate or harassment             |
      | threatening | violent threat                 |
      | leaky_pii | leaks personal data            |
      | spam   | spam or promo                  |
      | off_topic | irrelevant or low-quality      |
    And answer by rules on the text with hit 0.9 and fallback "safe"
      | label      | needles |
      | hateful    | "vermin", "deport", "slur" |
      | threatening | "find you", "watch your back", "kill you" |
      | leaky_pii  | "ssn", "123-45-6789", "credit card" |
      | spam       | "get rich", "limited offer", "buy now", "5000/week" |
    When any other answer goes to "STOP"
    Then the primary question is "category"
    And the action reads the question "severity"
    And label "BLOCK" when the answer to "is_threat" is "B" with at least 0.7 confidence
    And label "BLOCK" when the answer to "is_hate" is "B" with at least 0.7 confidence
    And label "BLOCK" when the answer to "is_pii" is "B" with at least 0.6 confidence
    And label "BLOCK" when the answer to "is_spam" is "B" with at least 0.7 confidence
    And label "WARN" when "severity" is at least 1.5
    And label "WARN" when the answer to "category" is one of "hateful", "threatening", "leaky_pii", "spam", "off_topic"
    And label "APPROVE" otherwise
