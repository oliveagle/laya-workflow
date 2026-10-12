@decision
Feature: laya_mem_stopping

  Background:
    Given the workflow runs at most 3 iterations

  Scenario: stop_check
    Given a noul question "evidence_sufficient" asks "Does `evidence` contain support for every factual part of an answer to `query`?"
    And answer "B" when the field "evidence_status" contains "sufficient" with hit 0.95
    And a noul question "continue_useful" asks "Given `query` and `evidence`, is another retrieval round likely to fill a specific gap or resolve a conflict?"
    And answer "B" when the field "evidence_status" contains "sufficient" with hit 0.8 and miss 0.1
    And a noul question "missing_evidence" asks "Is at least one fact required by `query` absent from `evidence`?"
    And answer "B" when the field "missing_evidence" contains "missing", "true" with hit 0.9 and miss 0.05
    And a noul question "contradiction" asks "Does `evidence` contain conflicting claims relevant to `query` that the supplied time/context cannot reconcile?"
    And answer "B" when the field "contradiction" contains "contradiction", "true" with hit 0.95
    When any other answer goes to "STOP"
    Then label "CONTINUE_CONTRADICTION" when "contradiction" is at least 0.4
    And label "CONTINUE_EVIDENCE_INSUFFICIENT" when "evidence_sufficient" is below 0.85
    And label "CONTINUE_MISSING" when "missing_evidence" is at least 0.4
    And label "STOP_EVIDENCE_OK" otherwise

