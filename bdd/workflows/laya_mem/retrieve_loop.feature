@decision
Feature: laya_mem_retrieve_loop

  Background:
    Given the workflow runs at most 6 iterations

  Scenario: route
    Given a noul question "semantic" asks "Would finding topically or semantically related memories help answer `query`?"
    And answer "B" when the field "route_semantic" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "temporal" asks "Does answering `query` require event dates, durations, ordering or changes over time?"
    And answer "B" when the field "route_temporal" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "causal" asks "Does answering `query` require explaining a cause, motivation, enabling condition or effect?"
    And answer "B" when the field "route_causal" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "entity" asks "Would connecting mentions of the same person, place, object or organization help answer `query`?"
    And answer "B" when the field "route_entity" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "multi_hop_need" asks "Does `query` require combining at least two distinct pieces of remembered evidence?"
    And answer "B" when the field "route_multi_hop_need" contains "yes", "high" with hit 0.85 and miss 0.1
    And a noul question "recency_importance" asks "Does `query` require the latest applicable fact rather than a historical fact?"
    And answer "B" when the field "route_recency" contains "yes", "high" with hit 0.85 and miss 0.1
    When any other answer goes to "stop_check"
    Then keep the answer to "semantic"
    And keep the answer to "temporal"
    And keep the answer to "causal"
    And keep the answer to "entity"
    And keep the answer to "multi_hop_need"
    And keep the answer to "recency_importance"

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

