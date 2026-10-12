@decision
Feature: codegraph_routing

  Background:
    Given the workflow runs at most 3 iterations

  Scenario: route
    Given a noul question "semantic" asks "Would a full-text search over symbol names / docstrings / signatures help answer `query` about the codebase?"
      | truthy | Symbol-name or docstring search is useful. |
      | falsy  | No name/description search is needed. |
    And answer "B" when the field "route_semantic" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "temporal" asks "Does answering `query` require recently-changed files or the latest revision of a function?"
      | truthy | File mtime / recency matters. |
      | falsy  | Code history is irrelevant to the answer. |
    And answer "B" when the field "route_temporal" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "causal" asks "Does answering `query` require following the call chain (who calls X, what X calls, who extends X)?"
      | truthy | A call-chain or inheritance walk is needed. |
      | falsy  | The answer does not depend on the call graph. |
    And answer "B" when the field "route_causal" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "entity" asks "Does `query` name a specific symbol, file or module (entity lookup by qualified_name)?"
      | truthy | A specific symbol/file is being asked about. |
      | falsy  | The query is thematic rather than a named entity. |
    And answer "B" when the field "route_entity" contains "high", "yes" with hit 0.85 and miss 0.1
    And a noul question "multi_hop_need" asks "Does answering `query` require combining at least two distinct code-graph pieces (e.g. a caller chain of depth ≥ 2, or a caller + callee merge)?"
      | truthy | The question needs a graph walk of depth ≥ 2. |
      | falsy  | One hop or a direct lookup suffices. |
    And answer "B" when the field "route_multi_hop_need" contains "yes", "high" with hit 0.85 and miss 0.1
    And a noul question "recency_importance" asks "Does `query` ask about the latest revision / a recent change rather than timeless code?"
      | truthy | Current-state recency is important. |
      | falsy  | The answer is historically stable. |
    And answer "B" when the field "route_recency" contains "yes", "high" with hit 0.85 and miss 0.1
    When any other answer goes to "STOP"
    Then keep the answer to "semantic"
    And keep the answer to "temporal"
    And keep the answer to "causal"
    And keep the answer to "entity"
    And keep the answer to "multi_hop_need"
    And keep the answer to "recency_importance"

