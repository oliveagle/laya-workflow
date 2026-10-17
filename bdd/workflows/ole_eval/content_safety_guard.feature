@decision
Feature: content_safety_guard
  Content-safety decision graph: one policy node with eight semantic checks
  feeding an effective-severity threshold. Demonstrates an explicit primary
  question that is *not* the first question, plus a threshold action that reads
  a different question than the primary one.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: policy
    Given a choice question "is_prohibited" asks "Does the content hit a prohibited semantic category (never allowed in any context)?"
      | answer | meaning                   |
      | A      | no, nothing prohibited    |
      | B      | yes, prohibited content   |
    And answer "B" when the text contains "targeted_abuse", "prohibited", "harassment" with hit 0.95 and miss 0.05
    And a choice question "pii_public_high" asks "Does this expose high-severity PII on a public feed (base_severity >= 80)?"
      | answer | meaning                                    |
      | A      | no PII exposure on public feed              |
      | B      | yes, high-severity PII on a public feed     |
    And answer "B" when the text matches every group with hit 0.9 and miss 0.05
      | group    | needles |
      | semantic | "personal_address", "ssn", "contact_information" |
      | context  | "public_feed", "public" |
    And a choice question "pii_public_low" asks "Does this expose low/mid-severity PII on a public feed (base_severity < 80)?"
      | answer | meaning                           |
      | A      | no                                |
      | B      | yes, low/mid PII on a public feed |
    And answer "B" when the text matches every group with hit 0.7 and miss 0.05
      | group    | needles |
      | semantic | "contact_information", "financial_promotion" |
      | context  | "public_feed", "public" |
    And a choice question "age_blocking" asks "Does any semantic exceed the audience minimum age at blocking severity (>= 80)?"
      | answer | meaning                      |
      | A      | no age restriction violation |
      | B      | yes, age-blocking content    |
    And answer "B" when the text matches every group with hit 0.9 and miss 0.05
      | group    | needles |
      | semantic | "graphic_violence", "self_harm", "artistic_nudity" |
      | audience | "child", "teen", "audience_age" |
    And a choice question "age_review" asks "Does any semantic exceed the audience minimum age at review severity (< 80)?"
      | answer | meaning                         |
      | A      | no                              |
      | B      | yes, age-restricted (review)    |
    And answer "B" when the text matches every group with hit 0.7 and miss 0.05
      | group    | needles |
      | semantic | "violence_simulation", "medical_information", "contact_information" |
      | audience | "child", "teen" |
    And a choice question "context_blocking" asks "Is any semantic not allowed in this context at blocking severity (>= 75)?"
      | answer | meaning                               |
      | A      | all allowed here                      |
      | B      | yes, out-of-context blocking content  |
    And answer "B" when the text matches every group with hit 0.9 and miss 0.05
      | group    | needles |
      | semantic | "graphic_violence", "self_harm", "artistic_nudity" |
      | context  | "public_feed", "not_allowed" |
    And a choice question "context_review" asks "Is any semantic not allowed in this context at review severity (< 75)?"
      | answer | meaning                               |
      | A      | all allowed here                      |
      | B      | yes, out-of-context review content    |
    And answer "B" when the text matches every group with hit 0.7 and miss 0.05
      | group    | needles |
      | semantic | "violence_simulation", "medical_information", "financial_promotion" |
      | context  | "public_feed", "not_allowed" |
    And a score question "effective_severity" asks "Effective severity 0-100 (base taxonomy severity plus repeat-violation boost up to +30)."
      | level           |
      | 0 benign        |
      | 50 reviewable   |
      | 90 blocking     |
      | 100 maximum     |
    And score "effective_severity" by rules on the text miss 10
      | score | needles |
      | 100   | "targeted_abuse", "prohibited" |
      | 95    | "graphic_violence", "self_harm" |
      | 80    | "personal_address" |
      | 75    | "artistic_nudity" |
      | 60    | "violence_simulation" |
      | 45    | "contact_information" |
      | 35    | "financial_promotion" |
      | 30    | "medical_information" |
    When any other answer goes to "STOP"
    Then the primary question is "effective_severity"
    And the action reads the question "effective_severity"
    And label "BLOCK" when the answer to "is_prohibited" is "B"
    And label "BLOCK" when the answer to "pii_public_high" is "B"
    And label "BLOCK" when the answer to "age_blocking" is "B"
    And label "BLOCK" when the answer to "context_blocking" is "B"
    And label "BLOCK" when "effective_severity" is at least 90
    And label "REVIEW" when the answer to "pii_public_low" is "B"
    And label "REVIEW" when the answer to "age_review" is "B"
    And label "REVIEW" when the answer to "context_review" is "B"
    And label "REVIEW" when "effective_severity" is at least 50
    And label "ALLOW" otherwise
