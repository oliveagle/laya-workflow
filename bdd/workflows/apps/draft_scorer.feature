@decision
Feature: draft_scorer
  Draft scorer: read a message on tone / clarity / typo / sensitivity and the
  author's own send-now choice, then recommend one action. Behaviour-equivalent
  to the built-in `draft_scorer` app's route(): a typo on a below-clear draft is
  "polish", a sensitive message in a hostile tone is "sleep", a confused draft is
  "rewrite", else fall back to the author's own `send_now` choice. The first two
  branches are a conjunction of a choice signal and a score cutoff, written as
  `label ... when <cond> and <cond>`.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: score
    Given a score question "tone" asks "Overall tone?"
      | level |
      | angry / hostile |
      | tense |
      | neutral |
      | warm / friendly |
    And a score question "clarity" asks "How clearly does the message convey its point?"
      | level |
      | confusing |
      | ambiguous |
      | clear |
      | crystal clear |
    And a choice question "has_typo" asks "Does the message contain obvious typos, broken sentences, or autocorrect errors?"
      | answer | meaning |
      | A | no, it reads cleanly |
      | B | yes, there are typos or broken sentences |
    And a choice question "is_sensitive" asks "Is the message on a sensitive topic (layoffs, legal, money conflict, breakup, politics)?"
      | answer | meaning |
      | A | no, it is a normal business or social message |
      | B | yes, it touches layoffs / legal / money conflict / politics |
    And a choice question "send_now" asks "Best next action?"
      | answer | meaning |
      | send_now | send as-is |
      | polish | light edits recommended |
      | sleep | sleep on it / get a second opinion |
      | rewrite | major rewrite required |
    When any other answer goes to "STOP"
    Then the primary question is "send_now"
    And the action reads the question "clarity"
    And label "polish" when the answer to "has_typo" is "B" with at least 0.5 confidence and "clarity" is below 1.5
    And label "sleep" when the answer to "is_sensitive" is "B" with at least 0.5 confidence and "tone" is below 1.0
    And label "rewrite" when "clarity" is below 0.5
    And label "polish" when the answer to "send_now" is "polish"
    And label "sleep" when the answer to "send_now" is "sleep"
    And label "rewrite" when the answer to "send_now" is "rewrite"
    And label "send_now" otherwise
