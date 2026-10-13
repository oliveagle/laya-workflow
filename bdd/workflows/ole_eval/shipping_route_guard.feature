@decision
Feature: shipping_route_guard
  Guard for LLM-proposed shipping route preferences as a single triage node. The
  model proposes one typed route preference (request_id, shipment_id,
  preference_intent, preferred_mode) and *only* that; this workflow owns the
  logistics graph search, the capacity/temperature/hazard/document constraints,
  the best-route selection and the cost-vs-limit check. It never routes a
  shipment or takes a logistics action. Port of
  code/qwen_vm/app_scenarios/shipping_route_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (exact four-field shape, request_id grammar, known
    shipment_id, valid preference_intent and preferred_mode enums) fold into the
    `proposal` choice.
  - The logistics graph solver (lane search with capacity / temperature / hazard
    / document / mode filters) folds into the `feasible` choice: the harness
    runs the deterministic DFS and hands the DSL `ok` or `none`.
  - The cost computation and the shipment cost_limit comparison fold into the
    `cost` choice.

  Emitted labels: reject_proposal, reject_no_route, reject_over_cost, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, shipment_id, preference_intent, preferred_mode} object with a valid request id, known shipment and valid enums?"
      | answer    | meaning                                                             |
      | accepted  | exactly one structurally valid typed route preference               |
      | malformed | parse error, missing/extra field, bad request id, unknown shipment or bad enum |
    And the answer comes from the field "ev_proposal"
    And a choice question "feasible" asks "Does the lane graph admit at least one route from origin to destination honouring capacity, temperature, hazard, document and mode constraints?"
      | answer | meaning                       |
      | ok     | at least one feasible route   |
      | none   | no feasible route             |
    And the answer comes from the field "ev_feasible"
    And a choice question "cost" asks "Is the best route's computed cost within the shipment cost limit?"
      | answer | meaning                     |
      | ok     | cost <= cost_limit          |
      | over   | cost > cost_limit           |
    And the answer comes from the field "ev_cost"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_no_route" when the answer to "feasible" is "none"
    And label "reject_over_cost" when the answer to "cost" is "over"
    And label "approve" otherwise
    And keep the state key "request_id"
