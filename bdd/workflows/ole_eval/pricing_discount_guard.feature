@decision
Feature: pricing_discount_guard
  Guard for LLM-proposed pricing/discount requests as a single triage node. The
  model proposes only a typed intent (sku, segment, discount_intent); this
  workflow owns the catalog lookup, segment eligibility, the promotion window,
  volume qualification, coupon permission and stacking rules, the margin floor
  and the approval threshold, computing the exact price itself. It never charges
  or issues anything. Port of code/qwen_vm/app_scenarios/pricing_discount_guard.

  Third three-valued port with its own verbs: the engine returns APPROVE /
  MODIFY / REJECT — exact price approved, local engine modified the discount, or
  the request is refused.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (raw size cap, exact typed shape, sku/segment/intent
    enums, request-id grammar, duplicate-key scan, no authority or side-effect
    keys) fold into the `proposal` choice.
  - The catalog (list price, cost price, margin floor, volume threshold /
    percent, approval threshold, currency), the coupon table and the promotion
    windows are a fixed fixture; the harness resolves them and hands the DSL one
    answer per check.
  - The engine's adjustment list (its MODIFY reasons) becomes one `adjustment`
    question with one answer per reason, so each keeps its own label without
    eight separate questions; when several apply the engine still returns MODIFY
    and the ladder emits the first in the engine's own append order.

  Emitted labels: reject_proposal, reject_duplicate, reject_segment,
  reject_approval_infeasible, modify_promotion_fallback, modify_volume_fallback,
  modify_coupon_disallowed, modify_promotion_window, modify_stacking_coupon,
  modify_stacking_list, modify_margin_floor, modify_approval_cap, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {sku, segment, discount_intent} object with valid enums, a valid request id and no authority or side-effect keys?"
      | answer    | meaning                                                             |
      | accepted  | exactly one structurally valid typed pricing intent                 |
      | malformed | parse error, missing/extra field, bad enum, bad request id or forbidden key |
    And the answer comes from the field "ev_proposal"
    And a choice question "duplicate" asks "Is the request_id unseen by this guard?"
      | answer    | meaning                       |
      | fresh     | request_id is new             |
      | duplicate | request_id already evaluated  |
    And the answer comes from the field "ev_duplicate"
    And a choice question "segment" asks "Is the proposal segment eligible for this SKU?"
      | answer       | meaning                    |
      | eligible     | segment is allowed        |
      | not_eligible | segment is not allowed    |
    And the answer comes from the field "ev_segment"
    And a choice question "feasibility" asks "When an approval-threshold cap applies, can the capped price still respect the margin floor?"
      | answer     | meaning                                              |
      | ok         | no cap needed, or the capped price clears the floor  |
      | infeasible | the capped price would break the margin floor        |
    And the answer comes from the field "ev_feasibility"
    And a choice question "adjustment" asks "Did local policy leave the exact price unchanged, or which single adjustment did it apply?"
      | answer                    | meaning                                                 |
      | ok                        | no adjustment, exact requested price approved           |
      | promotion_fallback        | promotion window/segment missed, fell back to list      |
      | volume_fallback           | volume threshold not reached, fell back to list         |
      | coupon_disallowed         | coupon not allowed for this SKU, fell back to list      |
      | promotion_window          | promotion+coupon stack missed, promotion window fallback |
      | stacking_coupon           | stacking disallowed, fell back to the coupon            |
      | stacking_list             | stacking disallowed and no coupon, fell back to list    |
      | margin_floor              | computed price raised to the margin floor               |
      | approval_cap              | price capped at the approval threshold                  |
    And the answer comes from the field "ev_adjustment"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_duplicate" when the answer to "duplicate" is "duplicate"
    And label "reject_segment" when the answer to "segment" is "not_eligible"
    And label "reject_approval_infeasible" when the answer to "feasibility" is "infeasible"
    And label "modify_promotion_fallback" when the answer to "adjustment" is "promotion_fallback"
    And label "modify_volume_fallback" when the answer to "adjustment" is "volume_fallback"
    And label "modify_coupon_disallowed" when the answer to "adjustment" is "coupon_disallowed"
    And label "modify_promotion_window" when the answer to "adjustment" is "promotion_window"
    And label "modify_stacking_coupon" when the answer to "adjustment" is "stacking_coupon"
    And label "modify_stacking_list" when the answer to "adjustment" is "stacking_list"
    And label "modify_margin_floor" when the answer to "adjustment" is "margin_floor"
    And label "modify_approval_cap" when the answer to "adjustment" is "approval_cap"
    And label "approve" otherwise
    And keep the state key "request_id"
