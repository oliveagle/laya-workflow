@decision
Feature: iterative_refine_loop
  Port of iterative_refine_loop: converted from dsl/loops/iterative_refine_loop.json (BDD round-trip).

  Background:
    Given the workflow runs at most 8 iterations

  Scenario: review
    Given a choice question "verdict" asks "Given the current draft and the number of revision rounds already spent, should we accept it, revise once more, or escalate to a human editor?"
      | answer | meaning |
      | accept | the draft is good enough to send as-is |
      | revise | it can be improved by another revision round |
      | escalate | it needs a human editor's judgement |
    And a score question "quality" asks "How good is the current draft?"
      | level |
      | poor |
      | usable with edits |
      | publication ready |
    When any other answer goes to "STOP"
    And the answer "accept" goes to "STOP"
    And the answer "revise" goes to "review"
    And the answer "escalate" goes to "STOP"
    Then the primary question is "verdict"
    Then merge open probabilities on "verdict"
    Then allow up to 3 retries
