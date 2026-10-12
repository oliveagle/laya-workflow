@decision
Feature: laya_mem_consolidation

  Background:
    Given the workflow runs at most 3 iterations

  Scenario: consolidate
    Given a noul question "pair_0_redundant" asks "Compare `new_memory.content` with `candidates[0].content`. Do these observations repeat the same fact with no additional recallable detail?"
      | truthy | One is a duplicate or paraphrase without a new detail or time-specific update. |
      | falsy  | They provide different details or describe distinct occurrences. |
    And answer "B" when the field "pair_0_redundant_status" contains "redundant", "duplicate", "paraphrase" with hit 0.9
    And a noul question "pair_0_contradiction" asks "Compare `new_memory.content` with `candidates[0].content`. Do these accounts assert incompatible facts about the same subject at the same time?"
      | truthy | Claims cannot both hold at the stated time and context. |
      | falsy  | Compatible claims, uncertainty, or a change over time that explains the difference. |
    And answer "B" when the field "pair_0_contradiction_status" contains "contradict", "conflict", "incompatible" with hit 0.9
    And a noul question "pair_0_obsolete" asks "Compare `new_memory.content` with `candidates[0].content`. Does the new memory explicitly replace the candidate's previously valid fact with an updated fact?"
      | truthy | An explicit update supersedes the earlier fact for current-state questions. |
      | falsy  | No explicit replacement; mere recency or a separate event is insufficient. |
    And answer "B" when the field "pair_0_obsolete_status" contains "obsolete", "supersede", "replaced", "updated fact" with hit 0.9
    And a noul question "pair_0_link" asks "Compare `new_memory.content` with `candidates[0].content`. Would following a link between these observations help answer a future recall question?"
      | truthy | The connection supplies related, corroborating, correcting or contrasting evidence. |
      | falsy  | There is no specific connection useful for recall. |
    And answer "B" when the field "pair_0_link_status" contains "related", "corroborat", "contrast", "useful link" with hit 0.9
    And a choice question "pair_0_representation" asks "Compare `new_memory.content` with `candidates[0].content`. Which representation best fits the relationship between these two observations? Judge from the supplied accounts; do not assume answers to other questions."
      | answer | meaning              |
      | keep_separate | Contradictory accounts, unique details that a combined representation would lose, or distinct facts/events without a supported general pattern. |
      | merge  | Compatible accounts of the same fact or event can be combined without losing unique details. |
      | promote | Distinct repeated episodes explicitly support a stable general pattern suitable for semantic abstraction; prefer this over merge for repeated events. |
      | uncertain | Insufficient evidence to choose a safe combined or separate representation. |
    And answer "B" when the text contains "keep_separate", "merge", "promote", "uncertain" with hit 0.9
    When any other answer goes to "STOP"
    Then label "CONTRADICTS" when "pair_0_contradiction" is at least 0.85
    And label "OBSOLETE" when "pair_0_obsolete" is at least 0.85
    And label "REDUNDANT_WITH" when "pair_0_redundant" is at least 0.85
    And label "RELATED_TO" when "pair_0_link" is at least 0.85
    And label "KEEP_SEPARATE" otherwise

