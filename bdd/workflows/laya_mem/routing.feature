@decision
Feature: laya_mem_routing

  Background:
    Given the workflow runs at most 3 iterations

  Scenario: route
    Given a noul question "semantic" asks "Would finding topically or semantically related memories help answer `query`?"
      | truthy | Recall of related facts is useful. |
      | falsy  | No related-memory lookup is needed. |
    And answer "B" when the field "route_semantic" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "temporal" asks "Does answering `query` require event dates, durations, ordering or changes over time?"
      | truthy | A time relation is needed to answer correctly. |
      | falsy  | Dates or ordering are incidental to the answer. |
    And answer "B" when the field "route_temporal" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "causal" asks "Does answering `query` require explaining a cause, motivation, enabling condition or effect?"
      | truthy | Causal or explanatory evidence is needed. |
      | falsy  | Only factual association or chronology is requested. |
    And answer "B" when the field "route_causal" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "entity" asks "Would connecting mentions of the same person, place, object or organization help answer `query`?"
      | truthy | Combining entity-specific facts or aliases is useful. |
      | falsy  | Entity identity is irrelevant to the answer. |
    And answer "B" when the field "route_entity" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "multi_hop_need" asks "Does `query` require combining at least two distinct pieces of remembered evidence?"
      | truthy | The question asks for a comparison, aggregation or chained inference. |
      | falsy  | One direct remembered fact suffices. |
    And answer "B" when the field "route_multi_hop_need" contains "yes", "high" with hit 0.85 and miss 0.1
    And a noul question "recency_importance" asks "Does `query` require the latest applicable fact rather than a historical fact?"
      | truthy | Current state, latest update or recent status is requested. |
      | falsy  | Historical or timeless facts answer the question. |
    And answer "B" when the field "route_recency" contains "yes", "high" with hit 0.85 and miss 0.1
    When any other answer goes to "STOP"
    Then keep the answer to "semantic"
    And keep the answer to "temporal"
    And keep the answer to "causal"
    And keep the answer to "entity"
    And keep the answer to "multi_hop_need"
    And keep the answer to "recency_importance"

