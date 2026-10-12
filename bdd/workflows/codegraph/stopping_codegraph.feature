@decision
Feature: codegraph_stopping

  Background:
    Given the workflow runs at most 3 iterations

  Scenario: stop_check
    Given a noul question "evidence_sufficient" asks "Does the current `evidence` cover every part of `query` (entry point + call chain + supporting files) so a code-locating answer can be returned without guessing?"
    And answer "B" when the field "evidence_status" contains "sufficient" with hit 0.95
    And a noul question "continue_useful" asks "Would another codegraph round (another caller hop, another FTS pass, a recent-changes lookup) fill a specific gap or resolve a conflict in the current `evidence`?"
    And answer "B" when the field "evidence_status" contains "sufficient" with hit 0.8 and miss 0.1
    And a noul question "missing_evidence" asks "Is at least one symbol, file or call chain required by `query` absent from `evidence`?"
    And answer "B" when the field "missing_evidence" contains "missing", "true" with hit 0.9 and miss 0.05
    And a noul question "contradiction" asks "Does `evidence` contain conflicting symbols, files or call chains that the supplied context cannot reconcile?"
    And answer "B" when the field "contradiction" contains "contradiction", "true" with hit 0.95
    When any other answer goes to "STOP"
    Then label "CONTINUE_CONTRADICTION" when "contradiction" is at least 0.4
    And label "CONTINUE_MISSING" when "missing_evidence" is at least 0.4
    And label "CONTINUE_EVIDENCE_INSUFFICIENT" when "evidence_sufficient" is below 0.85
    And label "STOP_EVIDENCE_OK" otherwise

