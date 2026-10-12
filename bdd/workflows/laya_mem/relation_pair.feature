@decision
Feature: laya_mem_relation_pair

  Background:
    Given the workflow runs at most 3 iterations

  Scenario: relations
    Given a noul question "pair_0_semantic" asks "Would a semantic link between `new_memory.content` and `candidates[0].content` help retrieve a shared specific topic or fact?"
    And answer "B" when the field "pair_0_semantic_status" contains "semantic" with hit 0.9
    And a noul question "pair_0_causes" asks "Does the event in `new_memory.content` cause, enable or explain the candidate event in `candidates[0].content`?"
    And answer "B" when the field "pair_0_causes_status" contains "causes" with hit 0.9
    And a noul question "pair_0_caused_by" asks "Does the candidate event in `candidates[0].content` cause, enable or explain the event in `new_memory.content`?"
    And answer "B" when the field "pair_0_caused_by_status" contains "caused_by" with hit 0.9
    And a noul question "pair_0_entity" asks "Using `new_memory.entities` and `candidates[0].entities`, do any names or aliases refer to the same real-world entity?"
    And answer "B" when the field "share_entity" contains "yes" with hit 0.9
    When any other answer goes to "STOP"
    Then label "LINK_ENTITY" when "pair_0_entity" is at least 0.6
    And label "LINK_CAUSAL_OUT" when "pair_0_causes" is at least 0.6
    And label "LINK_CAUSAL_IN" when "pair_0_caused_by" is at least 0.6
    And label "LINK_SEMANTIC" when "pair_0_semantic" is at least 0.6
    And label "LINK_NONE" otherwise

