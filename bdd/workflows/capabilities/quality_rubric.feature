@decision
Feature: quality_rubric
  Port of quality_rubric: converted from dsl/capabilities/quality_rubric.json (BDD round-trip).

  Background:
    Given the workflow runs at most 3 iterations

  Scenario: score_reply
    Given a score question "rubric_usefulness" asks "How actionable is reply for the request? Judge only the information supplied; do not invent product facts."
      | level |
      | Does not address the request. |
      | Partly addresses it but next steps are incomplete. |
      | Directly addresses it with concrete next steps. |
    And score "rubric_usefulness" when the field "rubric_usefulness_hint" contains "high" scoring 1.8 and "mid" scoring 1.0 miss 0.2
    And a score question "rubric_clarity" asks "How clear is reply, independent of its completeness?"
      | level |
      | Unintelligible. |
      | Hard to follow. |
      | Understandable with minor ambiguity. |
      | Clear and easy to follow. |
    And score "rubric_clarity" when the field "rubric_clarity_hint" contains "high" scoring 2.6 and "mid" scoring 1.5 miss 0.3
    When any other answer goes to "STOP"
    Then rubric "rubric_usefulness" at 3 levels weight 0.6 and "rubric_clarity" at 4 levels weight 0.4 requiring 0.8
    Then allow up to 1 retries
