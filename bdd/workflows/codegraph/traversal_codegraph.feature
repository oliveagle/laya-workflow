@decision
Feature: codegraph_traversal

  Background:
    Given the workflow runs at most 3 iterations

  Scenario: traverse
    Given a noul question "cand_0_relevance" asks "Is candidate 0 (one neighbour node or id) a relevant answer to the question, on its own?"
      | truthy | The candidate's signature / docstring matches the question. |
      | falsy  | The candidate is unrelated to the question. |
    And answer "B" when the field "cand_0_relevance" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "cand_0_relation_usefulness" asks "Is the *edge* connecting candidate 0 to the current evidence (call / extends / implements / instantiates) useful for answering?"
      | truthy | The relation edges carry information. |
      | falsy  | The relation is decorative or unrelated. |
    And answer "B" when the field "cand_0_relation_usefulness" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "cand_0_new_information" asks "Does candidate 0 add information not already present in `evidence`?"
      | truthy | The candidate adds a new fact or file. |
      | falsy  | The candidate restates something already known. |
    And answer "B" when the field "cand_0_new_information" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "cand_0_supports_current_evidence" asks "Does candidate 0 corroborate a current candidate or strengthen an existing evidence claim?"
      | truthy | The candidate supports an existing claim. |
      | falsy  | The candidate is independent of the existing evidence. |
    And answer "B" when the field "cand_0_supports_current_evidence" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "cand_1_relevance" asks "Is candidate 1 a relevant answer to the question, on its own?"
      | truthy | The candidate's signature / docstring matches the question. |
      | falsy  | The candidate is unrelated to the question. |
    And answer "B" when the field "cand_1_relevance" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "cand_1_relation_usefulness" asks "Is the *edge* connecting candidate 1 useful?"
      | truthy | The relation edges carry information. |
      | falsy  | The relation is decorative or unrelated. |
    And answer "B" when the field "cand_1_relation_usefulness" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "cand_1_new_information" asks "Does candidate 1 add information not already in `evidence`?"
      | truthy | The candidate adds a new fact or file. |
      | falsy  | The candidate restates something already known. |
    And answer "B" when the field "cand_1_new_information" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "cand_1_supports_current_evidence" asks "Does candidate 1 corroborate or support an existing evidence claim?"
      | truthy | The candidate supports an existing claim. |
      | falsy  | The candidate is independent of the existing evidence. |
    And answer "B" when the field "cand_1_supports_current_evidence" contains "high", "yes" with hit 0.85 and miss 0.1
    When any other answer goes to "STOP"
    Then keep the answer to "cand_0_relevance"
    And keep the answer to "cand_0_relation_usefulness"
    And keep the answer to "cand_0_new_information"
    And keep the answer to "cand_0_supports_current_evidence"
    And keep the answer to "cand_1_relevance"
    And keep the answer to "cand_1_relation_usefulness"
    And keep the answer to "cand_1_new_information"
    And keep the answer to "cand_1_supports_current_evidence"

