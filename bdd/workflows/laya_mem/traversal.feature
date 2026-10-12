@decision
Feature: laya_mem_traversal

  Background:
    Given the workflow runs at most 3 iterations

  Scenario: traverse
    Given a noul question "cand_0_relevance" asks "Does `candidates[0].content` contain a fact needed to answer `query`?"
      | truthy | Direct answer evidence or a necessary intermediate fact. |
      | falsy  | Only topic overlap or unrelated content. |
    And answer "B" when the field "cand_0_relevance" contains "high", "yes" with hit 0.9
    And a noul question "cand_0_relation_usefulness" asks "Does the stated graph relation of `candidates[0]` connect `evidence` to information useful for `query`?"
      | truthy | The relation and its direction support an answer-relevant connection. |
      | falsy  | A graph edge exists but has no demonstrated usefulness for this question. |
    And answer "B" when the field "cand_0_relation_usefulness" contains "high", "yes" with hit 0.85
    And a noul question "cand_0_new_information" asks "Does `candidates[0].content` add an answer-relevant detail absent from `evidence`?"
      | truthy | A distinct relevant detail or missing reasoning link. |
      | falsy  | Only duplicated evidence or irrelevant new details. |
    And answer "B" when the field "cand_0_new_information" contains "high", "yes" with hit 0.85
    And a noul question "cand_0_supports_current_evidence" asks "Does `candidates[0].content` independently corroborate a claim in `evidence` relevant to `query`?"
      | truthy | Provides compatible corroborating evidence for a specific claim. |
      | falsy  | No specific corroboration, or contradicts that claim. |
    And answer "B" when the field "cand_0_supports_current_evidence" contains "high", "yes" with hit 0.85
    And a noul question "cand_1_relevance" asks "Does `candidates[1].content` contain a fact needed to answer `query`?"
      | truthy | Direct answer evidence or a necessary intermediate fact. |
      | falsy  | Only topic overlap or unrelated content. |
    And answer "B" when the field "cand_1_relevance" contains "high", "yes" with hit 0.9
    And a noul question "cand_1_relation_usefulness" asks "Does the stated graph relation of `candidates[1]` connect `evidence` to information useful for `query`?"
      | truthy | The relation and its direction support an answer-relevant connection. |
      | falsy  | A graph edge exists but has no demonstrated usefulness for this question. |
    And answer "B" when the field "cand_1_relation_usefulness" contains "high", "yes" with hit 0.85
    And a noul question "cand_1_new_information" asks "Does `candidates[1].content` add an answer-relevant detail absent from `evidence`?"
      | truthy | A distinct relevant detail or missing reasoning link. |
      | falsy  | Only duplicated evidence or irrelevant new details. |
    And answer "B" when the field "cand_1_new_information" contains "high", "yes" with hit 0.85
    And a noul question "cand_1_supports_current_evidence" asks "Does `candidates[1].content` independently corroborate a claim in `evidence` relevant to `query`?"
      | truthy | Provides compatible corroborating evidence for a specific claim. |
      | falsy  | No specific corroboration, or contradicts that claim. |
    And answer "B" when the field "cand_1_supports_current_evidence" contains "high", "yes" with hit 0.85
    When any other answer goes to "STOP"
    Then keep the answer to "cand_0_relevance"
    And keep the answer to "cand_0_relation_usefulness"
    And keep the answer to "cand_0_new_information"
    And keep the answer to "cand_0_supports_current_evidence"
    And keep the answer to "cand_1_relevance"
    And keep the answer to "cand_1_relation_usefulness"
    And keep the answer to "cand_1_new_information"
    And keep the answer to "cand_1_supports_current_evidence"

