@decision
Feature: inventory_replenishment_guard
  Guard for LLM-proposed inventory replenishment as a single triage node. The
  model proposes only a typed intent (sku, demand_scenario, service_level,
  intent); this workflow owns the demand forecast, the reorder point, the
  pack-size / minimum-order and budget constraints and the exact order
  quantity. It never places an order. Port of
  code/qwen_vm/app_scenarios/inventory_replenishment_guard.

  Sibling of pricing_discount_guard: the engine also returns APPROVE / MODIFY /
  REJECT — exact reorder approved, local model modified the reorder, or the
  request is refused.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (raw shape, sku / demand_scenario / service_level /
    intent enums, duplicate-key and non-finite-number scan) fold into the
    `proposal` choice.
  - The item catalog (costs, pack sizes, minimum order quantity, lead times,
    demand samples, reorder points per scenario/level) and the budget are a fixed
    fixture; the harness runs the deterministic forecast + candidate selection
    and hands the DSL one answer per check.
  - The engine's adjustment list (its MODIFY reasons) becomes one `adjustment`
    question with one answer per reason; the dynamic `capped_by_<breach>`
    adjustment folds into a single `capped_by_constraint` answer, since the
    decision is MODIFY whichever breach it was.

  Emitted labels: reject_proposal, reject_unknown_sku, reject_duplicate,
  reject_no_reorder_needed, approve_no_order_needed, reject_infeasible,
  reject_no_quantity, modify_intent_raised, modify_service_fallback,
  modify_rounded_to_pack, modify_min_order_qty, modify_capped_by_constraint,
  approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {sku, demand_scenario, service_level, intent} object with valid enums and no duplicate keys or non-finite numbers?"
      | answer    | meaning                                                              |
      | accepted  | exactly one structurally valid typed replenishment intent             |
      | malformed | parse error, missing/extra field, bad enum or duplicate key           |
    And the answer comes from the field "ev_proposal"
    And a choice question "sku" asks "Is the sku present in the fixed catalog?"
      | answer  | meaning            |
      | known   | sku resolved       |
      | unknown | no such sku        |
    And the answer comes from the field "ev_sku"
    And a choice question "duplicate" asks "Is this (sku, demand_scenario, service_level, intent) proposal unseen?"
      | answer    | meaning                          |
      | fresh     | identical proposal is new        |
      | duplicate | identical proposal already seen  |
    And the answer comes from the field "ev_duplicate"
    And a choice question "need" asks "For a reorder intent, is the inventory position below the reorder point?"
      | answer            | meaning                             |
      | needed            | reorder is warranted                |
      | no_reorder_needed | position is already at/above the reorder point |
    And the answer comes from the field "ev_need"
    And a choice question "no_order" asks "For a hold/review intent, is the inventory position at/above the reorder point (so no order is placed)?"
      | answer          | meaning                                |
      | ok              | not a no-order case                     |
      | no_order_needed | hold/review with position at/above the point |
    And the answer comes from the field "ev_no_order"
    And a choice question "feasibility" asks "Does budget / pack / lead-time policy admit a positive order after capping?"
      | answer     | meaning                                     |
      | ok         | the capped candidate is feasible            |
      | infeasible | constraints make the order infeasible       |
    And the answer comes from the field "ev_feasibility"
    And a choice question "quantity" asks "Is the chosen order quantity strictly positive?"
      | answer | meaning              |
      | ok     | quantity > 0         |
      | none   | quantity <= 0        |
    And the answer comes from the field "ev_quantity"
    And a choice question "adjustment" asks "Did local policy leave the exact reorder unchanged, or which single adjustment did it apply?"
      | answer                 | meaning                                            |
      | ok                     | no adjustment, exact reorder approved               |
      | intent_raised          | intent raised to reorder                            |
      | service_fallback       | service level lowered for feasibility               |
      | rounded_to_pack        | quantity rounded up to the pack size                |
      | min_order_qty          | minimum order quantity enforced                     |
      | capped_by_constraint   | quantity capped by a hard constraint (budget/pack/lead) |
    And the answer comes from the field "ev_adjustment"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_unknown_sku" when the answer to "sku" is "unknown"
    And label "reject_duplicate" when the answer to "duplicate" is "duplicate"
    And label "reject_no_reorder_needed" when the answer to "need" is "no_reorder_needed"
    And label "approve_no_order_needed" when the answer to "no_order" is "no_order_needed"
    And label "reject_infeasible" when the answer to "feasibility" is "infeasible"
    And label "reject_no_quantity" when the answer to "quantity" is "none"
    And label "modify_intent_raised" when the answer to "adjustment" is "intent_raised"
    And label "modify_service_fallback" when the answer to "adjustment" is "service_fallback"
    And label "modify_rounded_to_pack" when the answer to "adjustment" is "rounded_to_pack"
    And label "modify_min_order_qty" when the answer to "adjustment" is "min_order_qty"
    And label "modify_capped_by_constraint" when the answer to "adjustment" is "capped_by_constraint"
    And label "approve" otherwise
    And keep the state key "request_id"
